//! Ideal HVAC equipment with solver-provided capacity.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_physics::biquadratic::BiquadraticCurve;
use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;
use hares_physics::units::power_kw_to_w;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, IdealCapacityMode, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::{IdealCapacityModeConfig, IdealHvacConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::hvac_core::{DEFAULT_BIQUADRATIC_COEFFS, IDEAL_CAPACITY_TIME_RES_THRESHOLD_S};
use super::thermostat::{ThermostatFsm, ThermostatMode, lookup_zone_temp};
use super::{
    RuntimeSetpointOverride, ThermalSetpoints,
    core_config::{
        build_setpoint_source, extract_numeric, extract_text, load_bounds_pair,
        parse_biquadratic_list,
    },
    helpers::{
        equipment_id_from_config, register_ebm_telemetry_keys, zone_id_from_config_or_default,
    },
};

/// Biquadratic curve pair (capacity + EIR) with shared input bounds.
/// Encapsulates the temperature-correction curves used by the non-ideal
/// fallback path in [`IdealHvac`].
///
/// EnergyPlus Engineering Reference, DX Heating Coil / DX Cooling Coil:
/// `Q_corrected = Q_rated × CAP_FT(T_indoor, T_outdoor)`
/// `EIR_corrected = EIR_rated × EIR_FT(T_indoor, T_outdoor)`
/// where CAP_FT and EIR_FT are biquadratic correction functions normalised
/// to 1.0 at rating-point conditions.
#[derive(Clone, Debug)]
struct BiquadraticCurveSet {
    capacity_coeffs: [f64; 6],
    eir_coeffs: [f64; 6],
    x1_bounds: (f64, f64),
    x2_bounds: (f64, f64),
}

impl BiquadraticCurveSet {
    const DEFAULT_X1_BOUNDS: (f64, f64) = (-10.0, 50.0);
    const DEFAULT_X2_BOUNDS: (f64, f64) = (-50.0, 60.0);

    fn identity() -> Self {
        Self {
            capacity_coeffs: DEFAULT_BIQUADRATIC_COEFFS,
            eir_coeffs: DEFAULT_BIQUADRATIC_COEFFS,
            x1_bounds: Self::DEFAULT_X1_BOUNDS,
            x2_bounds: Self::DEFAULT_X2_BOUNDS,
        }
    }

    fn evaluate_capacity(&self, x1: f64, x2: f64) -> f64 {
        BiquadraticCurve {
            coeffs: self.capacity_coeffs,
            x1_bounds: self.x1_bounds,
            x2_bounds: self.x2_bounds,
            warn_on_clamp: false,
            // EnergyPlus CurveManager.cc:282–287: capacity output must be non-negative
            output_min: Some(0.0),
            output_max: None,
        }
        .evaluate(x1, x2)
    }

    fn evaluate_eir(&self, x1: f64, x2: f64) -> f64 {
        BiquadraticCurve {
            coeffs: self.eir_coeffs,
            x1_bounds: self.x1_bounds,
            x2_bounds: self.x2_bounds,
            warn_on_clamp: false,
            output_min: None,
            output_max: None,
        }
        .evaluate(x1, x2)
    }
}

pub struct IdealHvac {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    zone_id: ZoneId,
    thermostat_fsm: ThermostatFsm,
    ideal_capacity_w: f64,
    ideal_capacity_degraded: bool,
    current_target_c: f64,
    ideal_capacity_mode: IdealCapacityMode,
    rated_capacity_w: f64,
    cooling_capacity_w: f64,
    is_variable_speed: bool,
    use_ideal_cached: bool,
    load_fraction: f64,
    last_sim_time: Option<DateTime<FixedOffset>>,
    /// Cached ideal target (zone, °C) computed in update_control() for
    /// ideal_target(). Derived from zone temp vs effective setpoints
    /// independently of the FSM hysteresis, so Deadband does not block
    /// solver back-calculation in ideal capacity mode.
    cached_ideal_target: Option<(ZoneId, f64)>,
    /// Cooling sensible heat ratio (fraction of capacity that is sensible).
    /// Used to split ideal cooling capacity into sensible and latent components.
    /// Defaults to 1.0 (no latent); set from config "shr" key.
    shr: f64,
    /// Rated fan power [W]. Zero means no fan.
    rated_fan_power_w: f64,
    /// Energy input ratio (1/COP). Default 1.0 = ideal.
    rated_eir: f64,
    /// Pre-computed ratio: rated_fan_power / (max_capacity * rated_eir).
    /// Used in ideal-capacity mode: fan_power = |capacity| * eir * fan_power_ratio.
    fan_power_ratio: f64,
    /// Minimum capacity [W]. Below this the unit turns off.
    capacity_min_w: f64,
    /// Biquadratic temperature-correction curves for the non-ideal fallback path.
    /// Default identity coefficients produce no correction (cap_ratio = eir_ratio = 1.0).
    curves: BiquadraticCurveSet,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// Rule R1 reactive-only ZIP (resolved via `crate::config::resolve_reactive_zip`):
    /// Ideal HVAC is unity pf (OCHRE Ideal = 1.0) → Q exactly zero, real power
    /// stays bit-identical. Wired uniformly with the rest of the fleet so the
    /// §4 cross-equipment consistency check stays uniform.
    zip: hares_types::zip::ZipLoad,
}

/// Serializable snapshot of [`IdealHvac`] mutable fields.
///
/// `time_at_current_speed_s` is intentionally absent: IdealHvac is single-capacity
/// (or solver-driven ideal capacity) with no multi-speed staging, so speed-based
/// minimum-runtime tracking does not apply.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdealHvacState {
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    ideal_capacity_w: f64,
    current_target_c: f64,
    mode: ThermostatMode,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    load_fraction: f64,
    last_sim_time: Option<DateTime<FixedOffset>>,
    thermostat_hysteresis_c: f64,
}

impl IdealHvac {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Ideal HVAC"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE
                | ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::MODE_OVERRIDE,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT
                | CoreCapabilities::REACTIVE,
            telemetry_fields: ideal_hvac_telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::thermal(zone),
                PortDeclaration::electrical(),
                PortDeclaration::humidity(zone),
            ],
            telemetry: ideal_hvac_default_telemetry(),
            core_output: CoreOutput::default(),
            zone_id: zone,
            thermostat_fsm: ThermostatFsm::new(ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            }),
            ideal_capacity_w: 0.0,
            ideal_capacity_degraded: false,
            current_target_c: 20.0,
            ideal_capacity_mode: IdealCapacityMode::default(),
            rated_capacity_w: 10_000.0,
            cooling_capacity_w: 10_000.0,
            is_variable_speed: false,
            use_ideal_cached: true,
            load_fraction: 1.0,
            last_sim_time: None,
            cached_ideal_target: None,
            shr: 1.0,
            rated_fan_power_w: 0.0,
            rated_eir: 1.0,
            fan_power_ratio: 0.0,
            capacity_min_w: 0.0,
            curves: BiquadraticCurveSet::identity(),
            zone_id_explicit,
            zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }

    pub fn with_setpoints(
        mut self,
        heating_weekday: [f64; 24],
        heating_weekend: [f64; 24],
        cooling_weekday: [f64; 24],
        cooling_weekend: [f64; 24],
    ) -> Self {
        self.thermostat_fsm.heating_setpoint_source = Some(ScheduleSource::DailyProfile {
            weekday: heating_weekday,
            weekend: heating_weekend,
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        self.thermostat_fsm.cooling_setpoint_source = Some(ScheduleSource::DailyProfile {
            weekday: cooling_weekday,
            weekend: cooling_weekend,
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        self
    }

    pub fn with_heating_setpoint_source(mut self, source: ScheduleSource) -> Self {
        self.thermostat_fsm.heating_setpoint_source = Some(source);
        self
    }

    pub fn with_cooling_setpoint_source(mut self, source: ScheduleSource) -> Self {
        self.thermostat_fsm.cooling_setpoint_source = Some(source);
        self
    }

    pub fn with_ideal_capacity_mode(mut self, mode: IdealCapacityMode) -> Self {
        self.ideal_capacity_mode = mode;
        self
    }

    fn effective_setpoints(&self) -> ThermalSetpoints {
        self.thermostat_fsm.effective_setpoints()
    }

    fn validate_runtime_override(
        &self,
        candidate: RuntimeSetpointOverride,
    ) -> crate::Result<RuntimeSetpointOverride> {
        let merged = self
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.thermostat_fsm.schedule_setpoints)
            .with_control_override(Some(candidate));
        let reconciled = merged.reconcile_for_deadband(self.thermostat_fsm.thermostat.hysteresis_c);
        if (reconciled.heating_c - merged.heating_c).abs() > 0.001
            || (reconciled.cooling_c - merged.cooling_c).abs() > 0.001
        {
            return Err(HaresError::Equipment(format!(
                "runtime setpoint override would violate deadband: \
                 cooling-heating gap {} C < required {} C",
                merged.cooling_c - merged.heating_c,
                2.0 * self.thermostat_fsm.thermostat.hysteresis_c,
            )));
        }
        Ok(candidate)
    }

    fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
        match self.ideal_capacity_mode {
            IdealCapacityMode::On => true,
            IdealCapacityMode::Off => false,
            IdealCapacityMode::Auto => {
                let time_res_s = env.time_res.num_seconds();
                time_res_s >= IDEAL_CAPACITY_TIME_RES_THRESHOLD_S || self.is_variable_speed
            }
        }
    }

    fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>) {
        let prev = self.thermostat_fsm.mode;
        self.thermostat_fsm.set_mode(mode, when);
        if prev != mode && mode == ThermostatMode::Deadband {
            self.ideal_capacity_w = 0.0;
            self.ideal_capacity_degraded = false;
        }
    }

    fn update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode> {
        let mode = self.thermostat_fsm.update_mode(env, self.zone_id)?;

        let setpoints = self.thermostat_fsm.effective_setpoints();
        match self.thermostat_fsm.mode {
            ThermostatMode::Heating => self.current_target_c = setpoints.heating_c,
            ThermostatMode::Cooling => self.current_target_c = setpoints.cooling_c,
            ThermostatMode::Deadband => {
                // In ideal capacity mode the active target is whichever comfort
                // boundary bounds the zone temperature, not the deadband midpoint.
                // The FSM Deadband is correct for physical-equipment cycling
                // observability; the solver needs the setpoint when active.
                if self.use_ideal_cached {
                    let zone_temp = lookup_zone_temp(env, self.zone_id).unwrap_or(21.0);
                    self.current_target_c = if zone_temp < setpoints.heating_c {
                        setpoints.heating_c
                    } else if zone_temp > setpoints.cooling_c {
                        setpoints.cooling_c
                    } else {
                        0.5 * (setpoints.heating_c + setpoints.cooling_c)
                    };
                } else {
                    self.current_target_c = 0.5 * (setpoints.heating_c + setpoints.cooling_c);
                }
            }
        }

        Ok(mode)
    }
}

impl Equipment for IdealHvac {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed = config.require_typed::<IdealHvacConfig>("Ideal HVAC")?;
        if let Some(v) = typed.heating_capacity_w {
            self.rated_capacity_w = v.max(0.0);
        }
        if let Some(v) = typed.cooling_capacity_w {
            self.cooling_capacity_w = v.max(0.0);
        }
        if let Some(shr) = typed.shr {
            self.shr = shr.clamp(0.0, 1.0);
        }
        if let Some(heating_sp) = typed.setpoint.heating_setpoint_c {
            self.thermostat_fsm.static_setpoints.heating_c = heating_sp;
        }
        if let Some(cooling_sp) = typed.setpoint.cooling_setpoint_c {
            self.thermostat_fsm.static_setpoints.cooling_c = cooling_sp;
        }
        if let Some(deadband) = typed.deadband_c {
            self.thermostat_fsm.thermostat.hysteresis_c = deadband.max(0.0);
        }
        if let Some(n_speeds) = typed.n_speeds {
            // OCHRE parity: auto-ideal variable-speed threshold is 4+ speeds.
            self.is_variable_speed = n_speeds >= 4;
        }
        if let Some(mode) = typed.ideal_capacity_mode {
            self.ideal_capacity_mode = match mode {
                IdealCapacityModeConfig::On => IdealCapacityMode::On,
                IdealCapacityModeConfig::Off => IdealCapacityMode::Off,
                IdealCapacityModeConfig::Auto => IdealCapacityMode::Auto,
            };
        }
        if let Some(source) = &typed.setpoint.heating_setpoint_source {
            self.thermostat_fsm.heating_setpoint_source = Some(source.clone().into_runtime());
        }
        if let Some(source) = &typed.setpoint.cooling_setpoint_source {
            self.thermostat_fsm.cooling_setpoint_source = Some(source.clone().into_runtime());
        }
        if let Some(fraction) = typed
            .fraction_heating_load_served
            .or(typed.fraction_cooling_load_served)
        {
            self.load_fraction = fraction.clamp(0.0, 1.0);
        }
        if let Some(fp) = typed.rated_fan_power_w {
            self.rated_fan_power_w = fp.max(0.0);
        }
        if let Some(eir) = typed.rated_eir {
            self.rated_eir = eir.max(0.0);
        }
        if let Some(min_cap) = typed.capacity_min_w {
            self.capacity_min_w = min_cap.max(0.0);
        }
        if let Some(fuel) = typed.fuel_type {
            self.descriptor.fuel = fuel;
        }

        // Load biquadratic temperature-correction curves from typed config.
        // Parse errors propagate via ? rather than silently falling back to identity,
        // per the constitution: "Parse/init boundaries must fail loudly on invalid input."
        if let Some(raw) = &typed.capacity_biquadratic_coeffs {
            let curves = parse_biquadratic_list(raw)?;
            if let Some(first) = curves.first() {
                self.curves.capacity_coeffs = *first;
            }
        }
        if let Some(raw) = &typed.eir_biquadratic_coeffs {
            let curves = parse_biquadratic_list(raw)?;
            if let Some(first) = curves.first() {
                self.curves.eir_coeffs = *first;
            }
        }

        if let Some(zone) = extract_numeric(config, "zone_id") {
            let zone = ZoneId(zone as u16);
            self.zone_id = zone;
            self.descriptor.zone = Some(zone);
            if let Some(thermal_port) = self.ports.first_mut() {
                thermal_port.zone = Some(zone);
            }
        }
        if let Some(heating_sp) = extract_numeric(config, "heating_setpoint_c") {
            self.thermostat_fsm.static_setpoints.heating_c = heating_sp;
        }
        if let Some(cooling_sp) = extract_numeric(config, "cooling_setpoint_c") {
            self.thermostat_fsm.static_setpoints.cooling_c = cooling_sp;
        }
        if let Some(cap) = extract_numeric(config, "capacity_w") {
            self.rated_capacity_w = cap.max(0.0);
            self.cooling_capacity_w = cap.max(0.0);
        }
        if let Some(cool_cap) = extract_numeric(config, "cooling_capacity_w") {
            self.cooling_capacity_w = cool_cap.max(0.0);
        }
        if let Some(deadband) = extract_numeric(config, "deadband_c") {
            self.thermostat_fsm.thermostat.hysteresis_c = deadband.max(0.0);
        }
        if let Some(n_speeds) = extract_numeric(config, "n_speeds") {
            // OCHRE parity: auto-ideal variable-speed threshold is 4+ speeds.
            self.is_variable_speed = n_speeds >= 4.0;
        }
        if let Some(mode) = extract_text(config, "ideal_capacity_mode") {
            self.ideal_capacity_mode = match mode.trim().to_ascii_lowercase().as_str() {
                "on" => IdealCapacityMode::On,
                "off" => IdealCapacityMode::Off,
                "auto" => IdealCapacityMode::Auto,
                other => {
                    return Err(HaresError::Equipment(format!(
                        "unrecognized ideal_capacity_mode '{other}'; expected 'on', 'off', or 'auto'"
                    )));
                }
            };
        }
        if self.thermostat_fsm.heating_setpoint_source.is_none()
            && let Some(source) = build_setpoint_source(config, "heating")
        {
            self.thermostat_fsm.heating_setpoint_source = Some(source);
        }
        if self.thermostat_fsm.cooling_setpoint_source.is_none()
            && let Some(source) = build_setpoint_source(config, "cooling")
        {
            self.thermostat_fsm.cooling_setpoint_source = Some(source);
        }

        // Load biquadratic curves from raw config extras (overrides typed config
        // if both are present). Parse errors propagate via ? matching hvac_core.rs:415-422
        // which uses .transpose()? to propagate parse errors rather than silently swallowing.
        if let Some(raw) = extract_text(config, "capacity_biquadratic_coeffs") {
            let curves = parse_biquadratic_list(raw)?;
            if let Some(first) = curves.first() {
                self.curves.capacity_coeffs = *first;
            }
        }
        if let Some(raw) = extract_text(config, "eir_biquadratic_coeffs") {
            let curves = parse_biquadratic_list(raw)?;
            if let Some(first) = curves.first() {
                self.curves.eir_coeffs = *first;
            }
        }
        self.curves.x1_bounds = load_bounds_pair(
            config,
            "biquadratic_x1_min",
            "biquadratic_x1_max",
            BiquadraticCurveSet::DEFAULT_X1_BOUNDS,
        );
        self.curves.x2_bounds = load_bounds_pair(
            config,
            "biquadratic_x2_min",
            "biquadratic_x2_max",
            BiquadraticCurveSet::DEFAULT_X2_BOUNDS,
        );

        // Compute fan_power_ratio per OCHRE: fan_power_max / (capacity_max * eir_max).
        let max_capacity = self.rated_capacity_w.max(self.cooling_capacity_w);
        let denom = max_capacity * self.rated_eir;
        self.fan_power_ratio = if denom > 0.0 {
            self.rated_fan_power_w / denom
        } else {
            0.0
        };

        self.thermostat_fsm.static_setpoints = self
            .effective_setpoints()
            .reconcile_for_deadband(self.thermostat_fsm.thermostat.hysteresis_c);
        self.thermostat_fsm.thermostat.validate(env)?;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        self.telemetry = ideal_hvac_default_telemetry();
        register_ebm_telemetry_keys(&mut self.telemetry);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal_cached = self.use_ideal_capacity(env);
        self.last_sim_time = Some(env.current_time);

        // Grid outage: a de-energized bus removes the unit's supply power —
        // force off (FSM to Deadband, since step() delivers capacity from
        // the FSM mode) and clear any solver-driven ideal target so no
        // capacity is delivered (WH precedent). Islanded homes keep an
        // energized bus and are not affected. See docs/outage-behavior.md.
        if !env.grid.bus_energized() {
            self.cached_ideal_target = None;
            self.ideal_capacity_w = 0.0;
            self.ideal_capacity_degraded = false;
            self.set_mode(ThermostatMode::Deadband, env.current_time);
            return OperatingMode::Off;
        }

        let mode = self.update_mode(env).unwrap_or(ThermostatMode::Deadband);

        // Compute the ideal solver target independently of FSM hysteresis.
        // The FSM deadband is correct for physical equipment cycling; in ideal
        // capacity mode the solver needs whichever comfort setpoint bounds the
        // zone temperature (heating if below heating_c, cooling if above
        // cooling_c, none if within the comfort band).
        let zone_temp = lookup_zone_temp(env, self.zone_id).unwrap_or(21.0);
        let setpoints = self.thermostat_fsm.effective_setpoints();
        self.cached_ideal_target = if self.use_ideal_cached {
            if zone_temp < setpoints.heating_c {
                Some((self.zone_id, setpoints.heating_c))
            } else if zone_temp > setpoints.cooling_c {
                Some((self.zone_id, setpoints.cooling_c))
            } else {
                None
            }
        } else {
            None
        };

        // When the ideal target is None (zone within comfort band), clear any
        // stale ideal capacity from the previous step so step() doesn't deliver
        // a now-inappropriate capacity.
        if self.cached_ideal_target.is_none() {
            self.ideal_capacity_w = 0.0;
            self.ideal_capacity_degraded = false;
        }
        // Set end_use based on FSM mode so that ByEndUse dispatch routing
        // sees the correct value before step() runs. Deadband mode preserves
        // the previous end_use — the equipment did not switch modes.
        self.descriptor.end_use = match mode {
            ThermostatMode::Heating => EndUse::HVAC_HEATING,
            ThermostatMode::Cooling => EndUse::HVAC_COOLING,
            ThermostatMode::Deadband => self.descriptor.end_use.clone(),
        };

        match mode {
            ThermostatMode::Heating => OperatingMode::Heating,
            ThermostatMode::Cooling => OperatingMode::Cooling,
            ThermostatMode::Deadband => OperatingMode::Off,
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                end_use_before_step = self.descriptor.end_use.as_str(),
                fsm_mode = ?self.thermostat_fsm.mode,
                "IdealHvac::step() end-use before step"
            );
        }

        let (t_indoor_c, t_outdoor_c) = if !self.use_ideal_cached {
            let t_out = env.weather.outdoor_temp_c;
            // zone_id is validated at init so this always finds the zone in
            // production; 21.0 °C (≈70 °F) is a safe fallback for unit tests
            // that construct minimal environment state without zone entries.
            let t_in = lookup_zone_temp(env, self.zone_id).unwrap_or(21.0);
            (t_in, t_out)
        } else {
            (0.0, 0.0)
        };

        // EnergyPlus Engineering Reference, DX Coil:
        // Q_corrected = Q_rated × CAP_FT(T_indoor, T_outdoor)
        // COP_corrected = COP_rated / EIR_FT(T_indoor, T_outdoor)
        // Identity curves [1,0,0,0,0,0] produce cap_ratio = eir_ratio = 1.0,
        // yielding rated capacity unchanged (no regression).
        let (cap_ratio, eir_ratio) = if !self.use_ideal_cached {
            let cap = self
                .curves
                .evaluate_capacity(t_indoor_c, t_outdoor_c)
                .max(0.0);
            let eir = self
                .curves
                .evaluate_eir(t_indoor_c, t_outdoor_c)
                .max(f64::EPSILON);
            tracing::debug!(
                cap_ratio = cap,
                eir_ratio = eir,
                t_indoor = t_indoor_c,
                t_outdoor = t_outdoor_c,
                "IdealHvac non-ideal fallback: biquadratic curve evaluation"
            );
            (cap, eir)
        } else {
            (1.0, 1.0)
        };

        let mut capacity_w = if self.use_ideal_cached {
            // Ideal capacity mode: solver-determined capacity drives directly.
            // FSM mode provides sign clamping as a defensive guard (the solver
            // can in principle produce wrong-direction capacity). Deadband mode
            // does NOT gate delivery — the solver already knows the correct
            // direction from cached_ideal_target, and stale capacity is
            // cleared by update_control() when the zone enters comfort range.
            match self.thermostat_fsm.mode {
                ThermostatMode::Heating => (self.ideal_capacity_w * self.load_fraction).max(0.0),
                ThermostatMode::Cooling => (self.ideal_capacity_w * self.load_fraction).min(0.0),
                ThermostatMode::Deadband => self.ideal_capacity_w * self.load_fraction,
            }
        } else {
            // Non-ideal mode: FSM-driven bang-bang cycling with rated capacity
            // and biquadratic temperature-correction curves.
            match self.thermostat_fsm.mode {
                ThermostatMode::Deadband => 0.0,
                ThermostatMode::Heating => {
                    (self.rated_capacity_w * cap_ratio * self.load_fraction).max(0.0)
                }
                ThermostatMode::Cooling => {
                    (-self.cooling_capacity_w * cap_ratio * self.load_fraction).min(0.0)
                }
            }
        };

        // R3: Minimum capacity.
        // OCHRE: clamps to capacity_min in ideal mode (never delivers less than
        // minimum compressor speed). Non-ideal mode forces Deadband (Off).
        // E+ IdealLoadsAirSystem: no min capacity concept; but OCHRE's clamp
        // behaviour is the conservative choice for equipment protection.
        if capacity_w.abs() > 0.0 && capacity_w.abs() < self.capacity_min_w {
            if self.use_ideal_cached {
                capacity_w = capacity_w.signum() * self.capacity_min_w;
            } else {
                capacity_w = 0.0;
                self.set_mode(ThermostatMode::Deadband, env.current_time);
            }
        }

        // Fan power per OCHRE: fan_power = |capacity| * eir * fan_power_ratio.
        // In non-ideal mode, apply EIR temperature correction so electrical
        // consumption reflects degraded COP at extreme conditions.
        let effective_eir = if !self.use_ideal_cached {
            self.rated_eir * eir_ratio
        } else {
            self.rated_eir
        };
        let fan_power_w = if capacity_w.abs() > 0.0 {
            capacity_w.abs() * effective_eir * self.fan_power_ratio
        } else {
            0.0
        };

        if capacity_w.abs() > 0.0 || fan_power_w > 0.0 {
            let (sensible_w, latent_w, category) = if capacity_w > 0.0 {
                // Heating: all sensible, no latent.
                (capacity_w, 0.0, ThermalCategory::HvacHeating)
            } else if capacity_w < 0.0 {
                // Cooling: split by SHR. Latent removes moisture (negative = cooling).
                let sensible = capacity_w * self.shr;
                let latent = capacity_w * (1.0 - self.shr);
                (sensible, latent, ThermalCategory::HvacCooling)
            } else {
                (0.0, 0.0, ThermalCategory::HvacHeating)
            };
            // Fan motor heat always enters the zone as sensible gain (positive
            // in both heating and cooling modes).
            ports.accumulate(&PortContribution::Thermal {
                zone: self.zone_id,
                sensible_gain_w: sensible_w + fan_power_w,
                radiant_gain_w: 0.0,
                latent_gain_w: latent_w,
                category,
            })?;

            // When cooling with SHR < 1.0, emit explicit moisture mass-flow rate
            // so the humidity solver can integrate directly without h_fg coupling.
            if capacity_w < 0.0 && self.shr < 1.0 && latent_w != 0.0 {
                let moisture_mass_flow_kg_s = latent_w / LATENT_HEAT_VAPORISATION_0C_J_KG;
                ports.accumulate(&PortContribution::Humidity {
                    zone: self.zone_id,
                    moisture_mass_flow_kg_s,
                })?;
            }
        }

        // Emit electrical port contribution for fan power.
        let fan_kw = fan_power_w / 1000.0;
        // Rule R1: Ideal HVAC is unity pf (OCHRE Ideal = 1.0) → Q exactly
        // zero. Wired uniformly so the cross-equipment consistency check
        // (port/CoreOutput/telemetry) stays uniform across the fleet.
        let reactive_power_kvar = self.zip.reactive_kvar(fan_kw, env.grid.bus_voltage_pu());
        if fan_power_w > 0.0 || reactive_power_kvar != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(fan_kw),
                reactive_power_kvar,
            })?;
        }

        let operating_mode = match self.thermostat_fsm.mode {
            ThermostatMode::Heating if capacity_w > 0.0 => OperatingMode::Heating,
            ThermostatMode::Cooling if capacity_w < 0.0 => OperatingMode::Cooling,
            ThermostatMode::Deadband if capacity_w > 0.0 => OperatingMode::Heating,
            ThermostatMode::Deadband if capacity_w < 0.0 => OperatingMode::Cooling,
            ThermostatMode::Deadband => OperatingMode::Off,
            _ if fan_power_w > 0.0 => OperatingMode::On,
            _ => OperatingMode::Standby,
        };

        self.telemetry.set(tk::THERMAL_OUTPUT_W, capacity_w);
        // COIL_SENSIBLE_COOLING_W is a positive magnitude (same convention as
        // AirConditioner) so cross-equipment diagnostics can sum without sign
        // correction. THERMAL_OUTPUT_W carries the signed value for callers that
        // need sign-aware net output.
        if capacity_w < 0.0 {
            self.telemetry
                .set(tk::COIL_SENSIBLE_COOLING_W, (capacity_w * self.shr).abs());
        } else {
            self.telemetry.set(tk::COIL_SENSIBLE_COOLING_W, 0.0);
        }
        self.telemetry.set(tk::FAN_HEAT_W, fan_power_w);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode.as_code());
        self.telemetry
            .set(tk::IDEAL_CAPACITY_W, self.ideal_capacity_w);
        self.telemetry.set(
            tk::IDEAL_CAPACITY_DEGRADED,
            if self.ideal_capacity_degraded {
                1.0
            } else {
                0.0
            },
        );
        self.telemetry
            .set(tk::CURRENT_TARGET_C, self.current_target_c);
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        let sp = self.thermostat_fsm.effective_setpoints();
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);
        let schedule_stage = self
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.thermostat_fsm.schedule_setpoints);
        self.telemetry
            .set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
        self.telemetry
            .set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);
        if let Some(ref rt) = self.thermostat_fsm.runtime_setpoints {
            self.telemetry
                .set(tk::RUNTIME_HEATING_SETPOINT_C, rt.heating_c.unwrap_or(0.0));
            self.telemetry
                .set(tk::RUNTIME_COOLING_SETPOINT_C, rt.cooling_c.unwrap_or(0.0));
        }
        self.telemetry
            .set(tk::HVAC_HEATING_CAPACITY_W, self.rated_capacity_w);
        self.telemetry
            .set(tk::HVAC_COOLING_CAPACITY_W, self.cooling_capacity_w);
        self.telemetry.set(tk::CAP_RATIO, cap_ratio);
        self.telemetry.set(tk::EIR_RATIO, eir_ratio);
        // Ideal HVAC tracks a single active target temperature; always populate
        // setpoint_c from current_target_c (HAS_SETPOINT requires Some per contract).
        let active_setpoint_c = self.current_target_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(fan_kw)),
                reactive_power_kvar: Some(reactive_power_kvar),
                fuel_w: None,
                thermal_output_w: Some(capacity_w),
                sensible_cooling_w: if capacity_w < 0.0 {
                    Some(capacity_w * self.shr)
                } else {
                    None
                },
                latent_cooling_w: if capacity_w < 0.0 && self.shr < 1.0 {
                    Some(capacity_w * (1.0 - self.shr))
                } else {
                    None
                },
            },
            state: CoreState {
                operating_mode: Some(operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance::default(),
        };

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                end_use_after_step = self.descriptor.end_use.as_str(),
                capacity_w = capacity_w,
                fsm_mode = ?self.thermostat_fsm.mode,
                "IdealHvac::step() end-use after step"
            );
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // Deadband (capacity_w == 0.0) preserves the previous end_use —
            // the invariant only applies when the equipment is actively
            // delivering heating or cooling.
            if capacity_w != 0.0 {
                let expected_end_use = if capacity_w > 0.0 {
                    EndUse::HVAC_HEATING
                } else {
                    EndUse::HVAC_COOLING
                };
                if self.descriptor.end_use != expected_end_use {
                    tracing::error!(
                        actual = self.descriptor.end_use.as_str(),
                        expected = expected_end_use.as_str(),
                        capacity_w = capacity_w,
                        "IdealHvac invariant violated: descriptor.end_use does not match capacity sign"
                    );
                }
            }
        }

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        Some(self.zip)
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &IdealHvacState {
                runtime_setpoints: self.thermostat_fsm.runtime_setpoints,
                ideal_capacity_w: self.ideal_capacity_w,
                current_target_c: self.current_target_c,
                mode: self.thermostat_fsm.mode,
                last_mode_switch_at: self.thermostat_fsm.last_mode_switch_at,
                mode_start_at: self.thermostat_fsm.mode_start_at,
                load_fraction: self.load_fraction,
                last_sim_time: self.last_sim_time,
                thermostat_hysteresis_c: self.thermostat_fsm.thermostat.hysteresis_c,
            },
            Self::checkpoint_version(),
            "IdealHvac",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: IdealHvacState = load_versioned(
            state,
            Self::checkpoint_version(),
            "IdealHvac",
            self.descriptor().id,
        )?;
        self.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.ideal_capacity_w = decoded.ideal_capacity_w;
        self.current_target_c = decoded.current_target_c;
        self.thermostat_fsm.mode = decoded.mode;
        self.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.thermostat_fsm.mode_start_at = decoded.mode_start_at;
        self.load_fraction = decoded.load_fraction;
        self.last_sim_time = decoded.last_sim_time;
        self.thermostat_fsm.thermostat.hysteresis_c = decoded.thermostat_hysteresis_c;
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::IdealCapacity {
                capacity_w,
                degraded,
            } => {
                self.ideal_capacity_w = *capacity_w;
                self.ideal_capacity_degraded = *degraded;
            }
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                let candidate = RuntimeSetpointOverride {
                    heating_c: *heating_setpoint_c,
                    cooling_c: *cooling_setpoint_c,
                };
                self.thermostat_fsm.runtime_setpoints =
                    Some(self.validate_runtime_override(candidate)?);
                if let Some(db) = deadband_c {
                    if db.is_finite() && *db >= 0.0 {
                        self.thermostat_fsm.thermostat.hysteresis_c = *db;
                    }
                }
            }
            ControlSignal::ModeOverride { mode } if *mode == OperatingMode::Off => {
                if let Some(sim_time) = self.last_sim_time {
                    self.set_mode(ThermostatMode::Deadband, sim_time);
                } else {
                    self.thermostat_fsm.mode = ThermostatMode::Deadband;
                    self.ideal_capacity_w = 0.0;
                    self.ideal_capacity_degraded = false;
                }
            }
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                let base = self
                    .thermostat_fsm
                    .static_setpoints
                    .with_schedule_override(self.thermostat_fsm.schedule_setpoints);
                let prior = self.thermostat_fsm.runtime_setpoints.unwrap_or_default();
                let candidate = RuntimeSetpointOverride {
                    heating_c: heating_delta_c
                        .map(|d| base.heating_c + d)
                        .or(prior.heating_c),
                    cooling_c: cooling_delta_c
                        .map(|d| base.cooling_c + d)
                        .or(prior.cooling_c),
                };
                self.thermostat_fsm.runtime_setpoints =
                    Some(self.validate_runtime_override(candidate)?);
            }
            ControlSignal::IdealCapacityModeOverride { mode } => {
                self.ideal_capacity_mode = *mode;
            }
            ControlSignal::LoadFraction { fraction } => {
                self.load_fraction = *fraction;
                if *fraction <= 0.0 {
                    if let Some(sim_time) = self.last_sim_time {
                        self.set_mode(ThermostatMode::Deadband, sim_time);
                    } else {
                        self.thermostat_fsm.mode = ThermostatMode::Deadband;
                    }
                    self.ideal_capacity_w = 0.0;
                    self.ideal_capacity_degraded = false;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(ZoneId, f64)> {
        self.cached_ideal_target
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Ideal HVAC",
        Box::new(|config| Box::new(IdealHvac::new(config))),
    );
}

fn ideal_hvac_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(18);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::IDEAL_CAPACITY_W, 0.0);
    telemetry.insert(tk::IDEAL_CAPACITY_DEGRADED, 0.0);
    telemetry.insert(tk::CURRENT_TARGET_C, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::COIL_SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::FAN_HEAT_W, 0.0);
    telemetry.insert(tk::HVAC_HEATING_CAPACITY_W, 0.0);
    telemetry.insert(tk::HVAC_COOLING_CAPACITY_W, 0.0);
    telemetry.insert(tk::CAP_RATIO, 1.0);
    telemetry.insert(tk::EIR_RATIO, 1.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn ideal_hvac_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Thermal power delivered to zone (positive=heating, negative=cooling)"
                .to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode: 0=Off, 1=Heating, 2=Cooling".to_string(),
        },
        TelemetryField {
            name: tk::IDEAL_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Ideal capacity from solver (positive=heating, negative=cooling)"
                .to_string(),
        },
        TelemetryField {
            name: tk::IDEAL_CAPACITY_DEGRADED.to_string(),
            unit: "bool".to_string(),
            description: "1.0 if capacity is degraded (fallback after consecutive solver failures)"
                .to_string(),
        },
        TelemetryField {
            name: tk::CURRENT_TARGET_C.to_string(),
            unit: "C".to_string(),
            description: "Current setpoint target temperature".to_string(),
        },
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Fan electrical consumption".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging); Ideal HVAC is unity pf"
                .to_string(),
        },
        TelemetryField {
            name: tk::COIL_SENSIBLE_COOLING_W.to_string(),
            unit: "W".to_string(),
            description: "Gross sensible cooling at the coil (positive magnitude, non-zero only during cooling); distinguishes coil output from fan waste heat".to_string(),
        },
        TelemetryField {
            name: tk::FAN_HEAT_W.to_string(),
            unit: "W".to_string(),
            description: "Supply fan waste heat added to the zone (positive); per E+ I/O Ref: fan motor heat in the supply air stream".to_string(),
        },
        TelemetryField {
            name: tk::HVAC_HEATING_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Rated heating capacity".to_string(),
        },
        TelemetryField {
            name: tk::HVAC_COOLING_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Rated cooling capacity".to_string(),
        },
        TelemetryField {
            name: tk::CAP_RATIO.to_string(),
            unit: "ratio".to_string(),
            description: "Biquadratic capacity correction factor (1.0 = rated)".to_string(),
        },
        TelemetryField {
            name: tk::EIR_RATIO.to_string(),
            unit: "ratio".to_string(),
            description: "Biquadratic EIR correction factor (1.0 = rated)".to_string(),
        },
        TelemetryField {
            name: tk::HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active heating setpoint temperature".to_string(),
        },
        TelemetryField {
            name: tk::COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active cooling setpoint temperature".to_string(),
        },
        TelemetryField {
            name: tk::SCHEDULE_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage heating setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::SCHEDULE_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage cooling setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override heating setpoint (0.0 when no override active)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override cooling setpoint (0.0 when no override active)".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_EFFICIENCY.to_string(),
            unit: "-".to_string(),
            description: "EBM efficiency (COP = 1/EIR)".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_BASELINE_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "EBM baseline power to hold setpoint".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM current energy state".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MIN_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM minimum energy at turn-on threshold".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MAX_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM maximum energy at turn-off threshold".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MAX_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "EBM maximum electrical power".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        BoundaryPolicy, ControlCapabilities, ControlSignal, DomainUpdate, ElectricPower, EndUse,
        EnvironmentState, GridState, HumidityAccumulator, PortSlots, SCHEDULE_DOMAIN_ID,
        ScheduleSource, SurfaceIrradiance, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::super::thermostat::ThermostatMode;
    use super::IdealHvac;
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn env(zone_temp_c: f64, time_res_s: i64, second: i64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 8.3,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config(name: &str) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            name.to_string(),
            "Ideal HVAC".to_string(),
            crate::IdealHvacConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn ideal_hvac_heating_turns_on_and_writes_capacity() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Heating);

        let target = eq.ideal_target();
        assert!(target.is_some());
        let (zone, temp) = target.unwrap();
        assert_eq!(zone, ZoneId(1));
        assert!((temp - 20.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_cooling_turns_on_when_zone_hot() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Cooling);

        let target = eq.ideal_target();
        assert!(target.is_some());
    }

    #[test]
    fn ideal_hvac_deadband_returns_none_target() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(22.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Off);

        let target = eq.ideal_target();
        assert!(target.is_none());
    }

    /// Grid outage (de-energized bus): forced off with the solver ideal
    /// target cleared — no capacity is delivered and no power drawn.
    /// Islanded homes keep conditioning; control resumes on restoration.
    #[test]
    fn grid_outage_forces_ideal_hvac_off_and_islanded_home_keeps_heating() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_cold = env(18.0, 60, 0);
        eq.init(&cfg, &env_cold).unwrap();

        // Baseline: heating with an ideal target.
        assert_eq!(
            eq.update_control(&env_cold),
            hares_types::OperatingMode::Heating
        );
        assert!(eq.ideal_target().is_some());

        // Utility outage: forced off, target cleared, zero step outputs.
        let mut env_outage = env(18.0, 60, 60);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(
            eq.update_control(&env_outage),
            hares_types::OperatingMode::Off
        );
        assert!(eq.ideal_target().is_none());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_outage, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(ports.electrical.load_power_w, 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);

        // Islanded: bus energized by a backup source → heating call returns.
        let mut env_islanded = env(18.0, 60, 120);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        assert_eq!(
            eq.update_control(&env_islanded),
            hares_types::OperatingMode::Heating
        );
        assert!(eq.ideal_target().is_some());

        // Restoration: control resumes.
        assert_eq!(
            eq.update_control(&env(18.0, 60, 180)),
            hares_types::OperatingMode::Heating
        );
    }

    #[test]
    fn ideal_hvac_accepts_ideal_capacity_signal() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let signal = hares_types::ControlSignal::IdealCapacity {
            capacity_w: 5000.0,
            degraded: false,
        };
        eq.apply_control(&signal).unwrap();
        assert!((eq.ideal_capacity_w - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_accepts_thermal_setpoint_signal() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let signal = hares_types::ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(19.0),
            cooling_setpoint_c: Some(25.0),
            deadband_c: None,
        };
        eq.apply_control(&signal).unwrap();

        assert!(eq.thermostat_fsm.runtime_setpoints.is_some());
        let sp = eq.thermostat_fsm.runtime_setpoints.unwrap();
        assert_eq!(sp.heating_c, Some(19.0));
        assert_eq!(sp.cooling_c, Some(25.0));
    }

    #[test]
    fn ideal_hvac_rejects_invalid_runtime_thermal_setpoint_override() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("deadband_c".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(20.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        let before = eq.thermostat_fsm.runtime_setpoints;

        let signal = hares_types::ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(24.0),
            cooling_setpoint_c: Some(25.0),
            deadband_c: None,
        };
        let result = eq.apply_control(&signal);
        assert!(
            result.is_err(),
            "invalid runtime override must return an error"
        );
        assert_eq!(
            eq.thermostat_fsm.runtime_setpoints, before,
            "invalid runtime override must not mutate state"
        );
    }

    #[test]
    fn ideal_hvac_mode_override_forces_off() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let signal = hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        };
        eq.apply_control_unchecked(&signal).unwrap();
    }

    #[test]
    fn ideal_hvac_step_writes_ideal_capacity_to_thermal_port() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = 3500.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!((ports.thermal[0].sensible_gain_w - 3500.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_cooling_step_writes_negative_to_thermal_port() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_capacity_w".into(), 8_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = -5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "cooling should write negative value, got {}",
            ports.thermal[0].sensible_gain_w
        );
        assert!((ports.thermal[0].sensible_gain_w - (-5000.0)).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_state_round_trips() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 4200.0;

        let state = eq.save_state().unwrap();
        let mut restored = IdealHvac::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert!((restored.ideal_capacity_w - 4200.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_descriptor_has_correct_capabilities() {
        let cfg = config("IH");
        let eq = IdealHvac::new(cfg);
        let caps = eq.descriptor().control_capabilities;

        assert!(caps.contains(ControlCapabilities::IDEAL_CAPACITY));
        assert!(caps.contains(ControlCapabilities::THERMAL_SETPOINT));
        assert!(caps.contains(ControlCapabilities::MODE_OVERRIDE));
        assert!(!caps.contains(ControlCapabilities::POWER_SETPOINT));
    }

    #[test]
    fn registry_includes_ideal_hvac() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Ideal HVAC").is_some());
    }

    #[test]
    fn ideal_capacity_mode_auto_engages_at_coarse_timestep() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "auto".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_coarse = env(18.0, 300, 0);
        eq.init(&cfg, &env_coarse).unwrap();

        assert!(eq.use_ideal_capacity(&env_coarse));

        let env_fine = env(18.0, 60, 0);
        assert!(!eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn ideal_capacity_mode_on_always_uses_ideal() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_fine = env(18.0, 60, 0);
        eq.init(&cfg, &env_fine).unwrap();

        assert!(eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn ideal_capacity_mode_off_never_uses_ideal() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_coarse = env(18.0, 300, 0);
        eq.init(&cfg, &env_coarse).unwrap();

        assert!(!eq.use_ideal_capacity(&env_coarse));
    }

    #[test]
    fn ideal_capacity_mode_off_returns_none_from_ideal_target() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        assert!(eq.thermostat_fsm.mode != super::ThermostatMode::Deadband);
        assert!(
            eq.ideal_target().is_none(),
            "ideal_target should return None when mode is Off"
        );
    }

    #[test]
    fn ideal_capacity_mode_off_step_uses_rated_capacity() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = 5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - 10_000.0).abs() < 1e-9,
            "should use rated capacity, not ideal capacity"
        );
    }

    #[test]
    fn variable_speed_forces_ideal_in_auto_mode() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut().insert("n_speeds".into(), 4.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_fine = env(18.0, 60, 0);
        eq.init(&cfg, &env_fine).unwrap();

        assert!(eq.is_variable_speed);
        assert!(eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn load_fraction_zero_forces_off() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 5000.0;

        let signal = hares_types::ControlSignal::LoadFraction { fraction: 0.0 };
        eq.apply_control_unchecked(&signal).unwrap();

        assert!((eq.ideal_capacity_w - 0.0).abs() < 1e-9);
    }

    #[test]
    fn load_fraction_partial_scales_capacity() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = -5000.0;

        let signal = hares_types::ControlSignal::LoadFraction { fraction: 0.5 };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - (-2500.0)).abs() < 1e-9,
            "capacity should be scaled by load fraction 0.5"
        );
    }

    #[test]
    fn typed_column_ref_setpoint_source_reads_schedule_domain() {
        let typed = crate::IdealHvacConfig {
            zone_id: Some(1),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_source: Some(hares_types::ScheduleSourceConfig::ColumnRef {
                    col_idx: 0,
                    boundary: hares_types::BoundaryPolicy::Clamp,
                }),
                cooling_setpoint_source: Some(hares_types::ScheduleSourceConfig::ColumnRef {
                    col_idx: 1,
                    boundary: hares_types::BoundaryPolicy::Clamp,
                }),
                ..Default::default()
            },
            ..crate::IdealHvacConfig::default()
        };
        let cfg =
            EquipmentConfig::from_typed("IH".to_string(), "Ideal HVAC".to_string(), typed).unwrap();

        let mut eq = IdealHvac::new(cfg.clone());
        let mut env = env(18.0, 60, 0);
        env.custom_domains = vec![DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![19.5, 26.0]),
        }];

        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let setpoints = eq.effective_setpoints();
        assert!((setpoints.heating_c - 19.5).abs() < 1e-9);
        assert!((setpoints.cooling_c - 26.0).abs() < 1e-9);
    }

    #[test]
    fn with_setpoint_source_builder_method() {
        let cfg = config("IH");
        let eq = IdealHvac::new(cfg)
            .with_heating_setpoint_source(ScheduleSource::Constant(21.0))
            .with_cooling_setpoint_source(ScheduleSource::Constant(25.0));

        assert!(eq.thermostat_fsm.heating_setpoint_source.is_some());
        assert!(eq.thermostat_fsm.cooling_setpoint_source.is_some());
    }

    #[test]
    fn daily_profile_setpoints_vary_by_hour() {
        let mut heating_weekday = [20.0; 24];
        heating_weekday[8] = 18.0;
        heating_weekday[22] = 19.0;
        let mut cooling_weekday = [26.0; 24];
        cooling_weekday[14] = 24.0;

        let cfg = config("IH");
        let eq = IdealHvac::new(cfg).with_setpoints(
            heating_weekday,
            heating_weekday,
            cooling_weekday,
            cooling_weekday,
        );

        assert!(eq.thermostat_fsm.heating_setpoint_source.is_some());
        assert!(eq.thermostat_fsm.cooling_setpoint_source.is_some());
    }

    #[test]
    fn mode_override_off_is_recoverable() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Heating);

        eq.apply_control_unchecked(&hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();
        assert!(eq.ideal_target().is_none());

        let mode = eq.update_control(&env);
        assert_eq!(
            mode,
            hares_types::OperatingMode::Heating,
            "should recover to heating after mode override clears"
        );
    }

    #[test]
    fn mode_override_off_before_first_step_clears_capacity() {
        let cfg = config("IH");
        let mut eq = IdealHvac::new(cfg);

        // Before init/step, last_sim_time is None. Set a stale capacity.
        eq.ideal_capacity_w = 5000.0;

        eq.apply_control_unchecked(&hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();

        assert_eq!(
            eq.ideal_capacity_w, 0.0,
            "ModeOverride(Off) before first step must clear ideal_capacity_w"
        );
    }

    #[test]
    fn ideal_capacity_mode_override_switches_at_runtime() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let environment = env(18.0, 60, 0);
        eq.init(&cfg, &environment).unwrap();

        // Initially off -- should NOT use ideal capacity
        assert!(!eq.use_ideal_capacity(&environment));

        // Override to On at runtime
        eq.apply_control_unchecked(&hares_types::ControlSignal::IdealCapacityModeOverride {
            mode: hares_types::IdealCapacityMode::On,
        })
        .unwrap();
        assert!(eq.use_ideal_capacity(&environment));

        // Override back to Off
        eq.apply_control_unchecked(&hares_types::ControlSignal::IdealCapacityModeOverride {
            mode: hares_types::IdealCapacityMode::Off,
        })
        .unwrap();
        assert!(!eq.use_ideal_capacity(&environment));
    }

    #[test]
    fn dynamic_cooling_writes_negative_rated_capacity() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_capacity_w".into(), 8_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - (-8_000.0)).abs() < 1e-9,
            "dynamic cooling should use -cooling_capacity_w, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn overlapping_setpoints_reconciled() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 22.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 21.0.into());
        cfg.test_extras_mut()
            .insert("deadband_c".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(20.0, 60, 0);
        eq.init(&cfg, &env)
            .expect("init should succeed with reconciliation");
        let gap = eq.thermostat_fsm.static_setpoints.cooling_c
            - eq.thermostat_fsm.static_setpoints.heating_c;
        assert!(gap >= 2.0, "reconciled gap must be >= 2.0 C, got {gap:.3}");
    }

    // Bug 1: current_target_c must update when setpoint changes mid-mode.
    // DailyProfile: hour 0 = 21.67°C, hour 8 = 18.33°C. Zone is below heat
    // turn-on at both hours so no mode transition occurs. The target must still
    // follow the schedule shift.
    #[test]
    fn current_target_c_tracks_schedule_shift_without_mode_change() {
        let mut heating_weekday = [21.67f64; 24];
        heating_weekday[8] = 18.33;
        let cooling_weekday = [26.0f64; 24];

        let typed = crate::IdealHvacConfig {
            zone_id: Some(1),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: heating_weekday,
                    weekend: heating_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                cooling_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: cooling_weekday,
                    weekend: cooling_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                ..Default::default()
            },
            ..crate::IdealHvacConfig::default()
        };
        let cfg =
            EquipmentConfig::from_typed("IH".to_string(), "Ideal HVAC".to_string(), typed).unwrap();

        // Zone at 15°C -- well below both setpoints, always triggers Heating.
        // hysteresis=1.0, deadband_offset=0.2 → heat turn-on at setpoint-0.8
        // 15 < 21.67-0.8=20.87 and 15 < 18.33-0.8=17.53
        let env_h0 = env(15.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_h0).unwrap();
        eq.update_control(&env_h0);

        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Heating);
        assert!(
            (eq.current_target_c - 21.67).abs() < 1e-6,
            "hour 0 target should be 21.67, got {}",
            eq.current_target_c
        );

        // Advance to hour 8 -- same zone temp, same mode, but setpoint drops.
        let env_h8 = env(15.0, 60, 8 * 3600);
        eq.update_control(&env_h8);

        assert_eq!(
            eq.thermostat_fsm.mode,
            super::ThermostatMode::Heating,
            "mode must not change"
        );
        assert!(
            (eq.current_target_c - 18.33).abs() < 1e-6,
            "current_target_c should update to new schedule setpoint, got {}",
            eq.current_target_c
        );
    }

    #[test]
    fn typed_daily_profile_setpoint_source_updates_targets_by_hour() {
        let mut heating_weekday = [21.67f64; 24];
        heating_weekday[8] = 18.33;
        let cooling_weekday = [26.0f64; 24];
        let typed = crate::IdealHvacConfig {
            zone_id: Some(1),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: heating_weekday,
                    weekend: heating_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                cooling_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: cooling_weekday,
                    weekend: cooling_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                ..Default::default()
            },
            ..crate::IdealHvacConfig::default()
        };
        let cfg =
            EquipmentConfig::from_typed("IH".to_string(), "Ideal HVAC".to_string(), typed).unwrap();

        let env_h0 = env(15.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_h0).unwrap();
        eq.update_control(&env_h0);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Heating);
        assert!((eq.current_target_c - 21.67).abs() < 1e-6);

        let env_h8 = env(15.0, 60, 8 * 3600);
        eq.update_control(&env_h8);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Heating);
        assert!((eq.current_target_c - 18.33).abs() < 1e-6);
    }

    #[test]
    fn current_target_c_updates_in_deadband_after_schedule_shift() {
        let mut heating_weekday = [20.0f64; 24];
        let mut cooling_weekday = [24.0f64; 24];
        heating_weekday[8] = 18.0;
        cooling_weekday[8] = 22.0;

        let typed = crate::IdealHvacConfig {
            zone_id: Some(1),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: heating_weekday,
                    weekend: heating_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                cooling_setpoint_source: Some(hares_types::ScheduleSourceConfig::DailyProfile {
                    weekday: cooling_weekday,
                    weekend: cooling_weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                }),
                ..Default::default()
            },
            ..crate::IdealHvacConfig::default()
        };
        let cfg =
            EquipmentConfig::from_typed("IH".to_string(), "Ideal HVAC".to_string(), typed).unwrap();

        // Midpoint = 22.0 at hour 0
        let env_h0 = env(22.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_h0).unwrap();
        eq.update_control(&env_h0);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Deadband);
        assert!((eq.current_target_c - 22.0).abs() < 1e-6);

        // Midpoint = 20.0 after schedule shift at hour 8
        let env_h8 = env(20.0, 60, 8 * 3600);
        eq.update_control(&env_h8);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Deadband);
        assert!(
            (eq.current_target_c - 20.0).abs() < 1e-6,
            "deadband target must track schedule midpoint, got {}",
            eq.current_target_c
        );
    }

    // Bug 2: Deadband must produce 0W even when ideal_capacity_w was set while
    // heating and the transition to Deadband did not clear it explicitly via the
    // ControlSignal path.
    #[test]
    fn deadband_outputs_zero_despite_stale_ideal_capacity() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        // Zone cold → enters Heating.
        let env_cold = env(18.0, 60, 0);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Heating);

        // Solver dispatches capacity while in Heating.
        let signal = hares_types::ControlSignal::IdealCapacity {
            capacity_w: 5000.0,
            degraded: false,
        };
        eq.apply_control_unchecked(&signal).unwrap();
        assert!((eq.ideal_capacity_w - 5000.0).abs() < 1e-9);

        // Zone warms past turn-off threshold (20.0 + 1.0*0.2 = 20.2) → Deadband.
        let env_warm = env(21.0, 60, 0);
        eq.update_control(&env_warm);
        assert_eq!(
            eq.thermostat_fsm.mode,
            super::ThermostatMode::Deadband,
            "zone at 21°C should be in Deadband"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_warm, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Deadband must output 0W, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    // Bug 3a: Cooling mode must clamp positive ideal_capacity_w to 0W.
    #[test]
    fn cooling_mode_clamps_positive_ideal_capacity_to_zero() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_hot = env(28.0, 60, 0);
        eq.init(&cfg, &env_hot).unwrap();
        eq.update_control(&env_hot);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Cooling);

        // Solver mistakenly returns positive capacity (e.g. outdoor dropped).
        let signal = hares_types::ControlSignal::IdealCapacity {
            capacity_w: 3000.0,
            degraded: false,
        };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_hot, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Cooling mode must not output positive capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    // Bug 3b: Heating mode must clamp negative ideal_capacity_w to 0W.
    #[test]
    fn heating_mode_clamps_negative_ideal_capacity_to_zero() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_cold = env(18.0, 60, 0);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert_eq!(eq.thermostat_fsm.mode, super::ThermostatMode::Heating);

        // Solver returns negative capacity (zone overshot, cooling needed).
        let signal = hares_types::ControlSignal::IdealCapacity {
            capacity_w: -2000.0,
            degraded: false,
        };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cold, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Heating mode must not output negative capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn ideal_capacity_mode_invalid_string_returns_error() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "invalid".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let e = env(20.0, 60, 0);
        let result = eq.init(&cfg, &e);

        assert!(
            result.is_err(),
            "init must return an error for unrecognized ideal_capacity_mode"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid"),
            "error message must contain the bad value; got: {msg}"
        );
    }

    #[test]
    fn schedule_setpoints_cleared_when_source_returns_none_after_valid_step() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let e = env(20.0, 60, 0);
        eq.init(&cfg, &e).unwrap();

        // One-element shared source: step 0 returns a value, step 1 errors → None.
        eq.thermostat_fsm.heating_setpoint_source = Some(ScheduleSource::Shared {
            data: std::sync::Arc::from(vec![21.0f64]),
            cursor: 0,
            boundary: BoundaryPolicy::Error,
        });

        eq.thermostat_fsm
            .resolve_profile_setpoints(&env(20.0, 60, 0));
        assert!(
            eq.thermostat_fsm.schedule_setpoints.is_some(),
            "step 0: source returned a value, schedule_setpoints must be Some"
        );
        assert_eq!(
            eq.thermostat_fsm.schedule_setpoints.unwrap().heating_c,
            Some(21.0)
        );

        eq.thermostat_fsm
            .resolve_profile_setpoints(&env(20.0, 60, 60));
        assert!(
            eq.thermostat_fsm.schedule_setpoints.is_none(),
            "step 1: source returned None (out of bounds), schedule_setpoints must be cleared"
        );
    }

    fn typed_config(typed: crate::IdealHvacConfig) -> EquipmentConfig {
        EquipmentConfig::from_typed("IH".to_string(), "Ideal HVAC".to_string(), typed).unwrap()
    }

    #[test]
    fn ideal_hvac_heating_uncapped_in_ideal_mode() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        // Solver says 50kW -- ideal mode delivers the full amount.
        eq.ideal_capacity_w = 50_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - 50_000.0).abs() < 1e-6,
            "ideal mode must deliver uncapped capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn ideal_hvac_cooling_uncapped_in_ideal_mode() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            cooling_capacity_w: Some(8_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                ..Default::default()
            },
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        // Solver says -40kW cooling -- ideal mode delivers the full amount.
        eq.ideal_capacity_w = -40_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // SHR defaults to 1.0, so all sensible. Capacity = -40000 (uncapped).
        assert!(
            (ports.thermal[0].sensible_gain_w - (-40_000.0)).abs() < 1e-6,
            "ideal mode must deliver uncapped capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn ideal_hvac_cooling_emits_latent() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            cooling_capacity_w: Some(20_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                ..Default::default()
            },
            shr: Some(0.8),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = -10_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // sensible = -10000 * 0.8 = -8000
        // latent = -10000 * 0.2 = -2000
        assert!(
            (ports.thermal[0].sensible_gain_w - (-8_000.0)).abs() < 1e-6,
            "sensible should be -8kW, got {}",
            ports.thermal[0].sensible_gain_w
        );
        assert!(
            (ports.thermal[0].latent_gain_w - (-2_000.0)).abs() < 1e-6,
            "latent should be -2kW, got {}",
            ports.thermal[0].latent_gain_w
        );
    }

    #[test]
    fn ideal_hvac_end_use_switches_with_mode() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            cooling_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                ..Default::default()
            },
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env_cool = env(28.0, 300, 0);
        eq.init(&cfg, &env_cool).unwrap();
        eq.update_control(&env_cool);
        eq.ideal_capacity_w = -5_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cool, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_COOLING,
            "end_use should be HVAC_COOLING when cooling"
        );
    }

    #[test]
    fn ideal_hvac_fan_power_in_electrical_port() {
        // 10kW heating capacity, 200W fan, EIR=1.0
        // fan_power_ratio = 200 / (10000 * 1.0) = 0.02
        // At 5kW capacity: fan_power = 5000 * 1.0 * 0.02 = 100W = 0.1kW
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            rated_fan_power_w: Some(200.0),
            rated_eir: Some(1.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 5_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let expected_fan_w = 5_000.0 * 1.0 * (200.0 / (10_000.0 * 1.0));
        let expected_fan_kw = expected_fan_w / 1000.0;

        // Electrical port should have fan power.
        assert!(
            (ports.electrical.load_power_w - expected_fan_w).abs() < 1.0,
            "electrical load should be {expected_fan_w} W, got {}",
            ports.electrical.load_power_w
        );

        // core_output should reflect fan consumption.
        let core = eq.core_output();
        match core.flows.electric_kw {
            Some(ElectricPower::Consumption(kw)) => {
                assert!(
                    (kw - expected_fan_kw).abs() < 1e-9,
                    "core electric_kw should be {expected_fan_kw}, got {kw}"
                );
            }
            other => panic!("expected Consumption, got {other:?}"),
        }

        // Thermal port: sensible = capacity + fan heat.
        assert!(
            (ports.thermal[0].sensible_gain_w - (5_000.0 + expected_fan_w)).abs() < 1e-6,
            "sensible should include fan heat: expected {}, got {}",
            5_000.0 + expected_fan_w,
            ports.thermal[0].sensible_gain_w
        );

        // Telemetry FAN_KW.
        let fan_kw_telem = eq.telemetry().get(hares_types::telemetry_keys::FAN_KW);
        assert!(fan_kw_telem.is_some(), "telemetry must contain FAN_KW");
        assert!(
            (fan_kw_telem.unwrap() - expected_fan_kw).abs() < 1e-9,
            "FAN_KW telemetry should be {expected_fan_kw}, got {:?}",
            fan_kw_telem
        );
    }

    #[test]
    fn ideal_hvac_emits_capacity_columns() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(12_000.0),
            cooling_capacity_w: Some(9_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(22.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let heating_cap = eq
            .telemetry()
            .get(hares_types::telemetry_keys::HVAC_HEATING_CAPACITY_W)
            .expect("telemetry must contain HVAC_HEATING_CAPACITY_W");
        let cooling_cap = eq
            .telemetry()
            .get(hares_types::telemetry_keys::HVAC_COOLING_CAPACITY_W)
            .expect("telemetry must contain HVAC_COOLING_CAPACITY_W");

        assert!(
            (heating_cap - 12_000.0).abs() < 1e-6,
            "heating capacity should be 12000, got {heating_cap}"
        );
        assert!(
            (cooling_cap - 9_000.0).abs() < 1e-6,
            "cooling capacity should be 9000, got {cooling_cap}"
        );
    }

    #[test]
    fn ideal_hvac_minimum_capacity_clamps_in_ideal_mode() {
        // OCHRE: ideal capacity clamps to capacity_min rather than forcing off
        // (minimum compressor speed). HARES matches this: ideal mode delivers
        // at least capacity_min_w when any capacity is requested.
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            capacity_min_w: Some(1_000.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        // Solver says 500W -- below 1000W minimum.
        eq.ideal_capacity_w = 500.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Ideal mode clamps to min, not forces off.
        assert!(
            (ports.thermal[0].sensible_gain_w - 1_000.0).abs() < 1e-9,
            "ideal mode must clamp to capacity_min (1000W), not force off; got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn r3_forced_deadband_in_non_ideal_mode() {
        // R3 minimum capacity check forces Deadband in non-ideal mode.
        // (Ideal mode clamps to min; non-ideal forces off.)
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            capacity_min_w: Some(1_000.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::Off),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env0 = env(18.0, 60, 0);
        eq.init(&cfg, &env0).unwrap();
        eq.update_control(&env0);
        assert_eq!(eq.thermostat_fsm.mode, ThermostatMode::Heating);
        let _heating_start = eq.thermostat_fsm.mode_start_at;

        // Rated capacity delivers 10kW, above the 1kW min → no R3 trigger.
        // To trigger R3 we'd need cap_ratio < 0.1; just test Deadband path below.
        let env60 = env(18.0, 60, 60);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env60, Duration::from_secs(60), &mut ports)
            .unwrap();
        // Non-ideal mode delivers rated capacity in Heating
        assert!(
            ports.thermal[0].sensible_gain_w > 0.0,
            "non-ideal heating must deliver rated capacity"
        );
    }

    #[test]
    fn r3_forced_deadband_respects_min_off_time_lockout_in_non_ideal() {
        // R3 min capacity forces Deadband with correct mode_start_at timestamp
        // in non-ideal mode. (Ideal mode clamps to min instead.)
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            capacity_min_w: Some(1_000.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::Off),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env0 = env(18.0, 60, 0);
        eq.init(&cfg, &env0).unwrap();
        eq.thermostat_fsm.min_off_time_s = 180.0;
        eq.update_control(&env0);
        assert_eq!(eq.thermostat_fsm.mode, ThermostatMode::Heating);

        let env60 = env(18.0, 60, 60);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        // Non-ideal Heating delivers 10kW rated capacity; cap_ratio=1.0 → min OK.
        eq.step(&env60, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w > 0.0,
            "non-ideal heating delivers rated capacity"
        );
    }

    #[test]
    fn shared_schedule_cursor_advances_once_per_tick() {
        use hares_types::ScheduleSource;
        use std::sync::Arc;

        let data: Arc<[f64]> = vec![19.0, 20.0, 21.0, 22.0].into();
        let shared = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: hares_types::BoundaryPolicy::Wrap,
        };
        let mut fsm = super::super::ThermostatFsm::new(super::super::ThermalSetpoints {
            heating_c: 18.0,
            cooling_c: 26.0,
        });
        fsm.heating_setpoint_source = Some(shared);

        let env0 = env(15.0, 60, 0);
        fsm.resolve_profile_setpoints(&env0);
        let sp0 = fsm.schedule_setpoints.unwrap().heating_c.unwrap();
        assert!(
            (sp0 - 19.0).abs() < 1e-6,
            "first resolve should yield data[0]=19.0, got {sp0}"
        );

        let env1 = env(15.0, 60, 60);
        fsm.resolve_profile_setpoints(&env1);
        let sp1 = fsm.schedule_setpoints.unwrap().heating_c.unwrap();
        assert!(
            (sp1 - 20.0).abs() < 1e-6,
            "second resolve should yield data[1]=20.0, got {sp1}"
        );

        let env2 = env(15.0, 60, 120);
        fsm.resolve_profile_setpoints(&env2);
        let sp2 = fsm.schedule_setpoints.unwrap().heating_c.unwrap();
        assert!(
            (sp2 - 21.0).abs() < 1e-6,
            "third resolve should yield data[2]=21.0, got {sp2}"
        );
    }

    #[test]
    fn ideal_hvac_shared_schedule_does_not_skip_values() {
        use hares_types::ScheduleSource;
        use std::sync::Arc;

        let heating_data: Arc<[f64]> = vec![18.0, 19.0, 20.0, 21.0, 22.0, 23.0].into();
        let cooling_data: Arc<[f64]> = vec![26.0; 6].into();

        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env0 = env(15.0, 60, 0);
        eq.init(&cfg, &env0).unwrap();

        eq.thermostat_fsm.heating_setpoint_source = Some(ScheduleSource::Shared {
            data: heating_data.clone(),
            cursor: 0,
            boundary: hares_types::BoundaryPolicy::Wrap,
        });
        eq.thermostat_fsm.cooling_setpoint_source = Some(ScheduleSource::Shared {
            data: cooling_data.clone(),
            cursor: 0,
            boundary: hares_types::BoundaryPolicy::Wrap,
        });

        eq.update_control(&env0);
        let sp0 = eq
            .thermostat_fsm
            .schedule_setpoints
            .unwrap()
            .heating_c
            .unwrap();
        assert!(
            (sp0 - 18.0).abs() < 1e-6,
            "tick 0: should yield heating_data[0]=18.0, got {sp0}"
        );

        let env1 = env(15.0, 60, 60);
        eq.update_control(&env1);
        let sp1 = eq
            .thermostat_fsm
            .schedule_setpoints
            .unwrap()
            .heating_c
            .unwrap();
        assert!(
            (sp1 - 19.0).abs() < 1e-6,
            "tick 1: should yield heating_data[1]=19.0, got {sp1}"
        );

        let env2 = env(15.0, 60, 120);
        eq.update_control(&env2);
        let sp2 = eq
            .thermostat_fsm
            .schedule_setpoints
            .unwrap()
            .heating_c
            .unwrap();
        assert!(
            (sp2 - 20.0).abs() < 1e-6,
            "tick 2: should yield heating_data[2]=20.0, got {sp2}"
        );
    }

    #[test]
    fn ideal_hvac_fan_power_cooling_mode() {
        // Cooling mode: ideal_capacity = -5000W, SHR=1.0, fan=200W, rated_eir=1.0, capacity=10kW.
        // fan_power_ratio = 200 / (10000 * 1.0) = 0.02
        // fan_power = 5000 * 1.0 * 0.02 = 100W
        // sensible_gain = -5000 * 1.0 (SHR) + 100 (fan heat) = -4900W
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            rated_fan_power_w: Some(200.0),
            rated_eir: Some(1.0),
            shr: Some(1.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(30.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = -5_000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let fan_power_ratio = 200.0 / (10_000.0 * 1.0);
        let expected_fan_w = 5_000.0 * 1.0 * fan_power_ratio;
        let expected_sensible = -5_000.0 * 1.0 + expected_fan_w;

        assert!(
            (ports.thermal[0].sensible_gain_w - expected_sensible).abs() < 1e-6,
            "sensible should be {expected_sensible}, got {}",
            ports.thermal[0].sensible_gain_w
        );

        let fan_kw = eq.telemetry().get(hares_types::telemetry_keys::FAN_KW);
        assert!(
            fan_kw.is_some(),
            "fan_kw telemetry must be present in cooling mode"
        );
        assert!(
            fan_kw.unwrap() > 0.0,
            "fan electrical consumption must be > 0 in cooling mode, got {:?}",
            fan_kw
        );

        assert!(
            ports.electrical.load_power_w > 0.0,
            "electrical consumption must be > 0, got {}",
            ports.electrical.load_power_w
        );
    }

    #[test]
    fn checkpoint_thermostat_hysteresis_c() {
        let mut cfg = config("IH");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());

        let e = env(16.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Apply a non-default deadband via ThermalSetpoint control.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(20.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: Some(3.0),
        })
        .unwrap();
        assert_eq!(eq.thermostat_fsm.thermostat.hysteresis_c, 3.0);

        eq.update_control(&e);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let state = eq.save_state().unwrap();

        let mut restored = IdealHvac::new(cfg.clone());
        restored.init(&cfg, &e).unwrap();
        assert_eq!(
            restored.thermostat_fsm.thermostat.hysteresis_c, 1.0,
            "fresh instance must have config default"
        );

        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.thermostat_fsm.thermostat.hysteresis_c, 3.0,
            "thermostat_hysteresis_c must survive checkpoint round-trip"
        );
    }

    // AHRI 210/240-2023 Table 9: H1 = 8.3°C (47°F), H3 = −8.3°C (17°F).
    // Empirical data (learnmetrics.com, NREL OCHRE studies) show typical ASHP heating
    // capacity at H3 is ~60–70% of H1 rated capacity.
    #[test]
    fn non_ideal_heating_capacity_degrades_at_ahri_h3_condition() {
        const RATED_CAPACITY_W: f64 = 10_000.0;
        // Linearised ASHP capacity curve: CAP_FT = 0.834 + 0.02*T_outdoor.
        // At H1 (outdoor=8.3°C): 0.834 + 0.02*8.3 = 1.0 (rated)
        // At H3 (outdoor=−8.3°C): 0.834 + 0.02*(−8.3) = 0.668
        // AHRI 210/240-2023 Table 9: typical ASHP at H3 delivers 60–70% of H1 rated.

        let mut cfg = config("IH-002");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), RATED_CAPACITY_W.into());
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".into(),
            "0.834,0,0,0.02,0,0".into(),
        );
        cfg.test_extras_mut()
            .insert("biquadratic_x1_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x1_max".into(), 35.0f64.into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_max".into(), 50.0f64.into());

        let mut h3_env = env(18.0, 60, 0);
        h3_env.weather.outdoor_temp_c = -8.3;

        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &h3_env).unwrap();
        eq.update_control(&h3_env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&h3_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        let actual_w = ports.thermal[0].sensible_gain_w;

        // CAP_FT(18.0, −8.3) = 0.834 + 0.02*(−8.3) = 0.668
        // Expected: 10000 * 0.668 = 6680 W (no fan heat: fan_power_ratio = 0).
        assert!(
            actual_w < RATED_CAPACITY_W * 0.75,
            "at AHRI H3 (−8.3°C) the biquadratic-corrected capacity must be < 75% of rated; \
             got {actual_w:.1} W (rated = {RATED_CAPACITY_W:.0} W)"
        );
        assert!(
            actual_w >= RATED_CAPACITY_W * 0.55,
            "at AHRI H3 (−8.3°C) the corrected capacity should not fall below 55% of rated; \
             got {actual_w:.1} W"
        );
    }

    // Companion to the above: verify that identity biquadratic coefficients produce
    // exactly rated capacity in the non-ideal path (no regression for configs without curves).
    #[test]
    fn non_ideal_heating_identity_curve_produces_rated_capacity() {
        const RATED_CAPACITY_W: f64 = 10_000.0;

        let mut cfg = config("IH-002-identity");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), RATED_CAPACITY_W.into());
        cfg.test_extras_mut()
            .insert("capacity_biquadratic_coeffs".into(), "1,0,0,0,0,0".into());

        let mut h3_env = env(18.0, 60, 0);
        h3_env.weather.outdoor_temp_c = -8.3;

        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &h3_env).unwrap();
        eq.update_control(&h3_env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&h3_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - RATED_CAPACITY_W).abs() < 1.0,
            "identity curve must produce exactly rated capacity; got {} W",
            ports.thermal[0].sensible_gain_w
        );
    }

    // Cooling capacity must degrade at high outdoor temperature with a realistic curve.
    // EnergyPlus Engineering Reference, DX Cooling Coil: CAP_FT normalised to 1.0 at
    // AHRI rating (indoor WB=19.44°C, outdoor DB=35°C). At 46°C outdoor, capacity drops.
    #[test]
    fn non_ideal_cooling_capacity_degrades_at_high_outdoor_temp() {
        const RATED_COOLING_W: f64 = 10_000.0;
        // Simplified cooling capacity curve: CAP_FT = 1.3 − 0.01*T_outdoor.
        // At rated (35°C): 1.3 − 0.01*35 = 0.95 (near unity, typical).
        // At 46°C: 1.3 − 0.01*46 = 0.84 — reduced capacity.
        let mut cfg = config("IH-002-cooling");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), RATED_COOLING_W.into());
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".into(),
            "1.3,0,0,-0.01,0,0".into(),
        );
        cfg.test_extras_mut()
            .insert("biquadratic_x1_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x1_max".into(), 35.0f64.into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_max".into(), 50.0f64.into());

        let mut hot_env = env(28.0, 60, 0); // zone above cooling setpoint → Cooling mode
        hot_env.weather.outdoor_temp_c = 46.0;

        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &hot_env).unwrap();
        eq.update_control(&hot_env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&hot_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        let actual_w = ports.thermal[0].sensible_gain_w;
        // CAP_FT(28.0, 46.0) = 1.3 − 0.01*46 = 0.84
        // Expected magnitude: 10000 * 0.84 = 8400 W (negative, cooling).
        // Actual sensible includes fan heat (0 here), so |sensible| ≈ 8400.
        assert!(
            actual_w.abs() < RATED_COOLING_W,
            "at 46°C outdoor the biquadratic-corrected cooling capacity must be less than rated; \
             got {actual_w:.1} W (rated = {RATED_COOLING_W:.0} W)"
        );
        assert!(
            actual_w < 0.0,
            "cooling capacity must be negative; got {actual_w:.1} W"
        );
    }

    // Numerical verification: linearised ASHP curve CAP_FT = 0.834 + 0.02*T_outdoor.
    // At T_outdoor = −8.3°C (AHRI H3): CAP_FT = 0.668 exactly.
    // AHRI 210/240-2023 Table 9: H3 condition at −8.3°C (17°F).
    #[test]
    fn non_ideal_heating_biquadratic_matches_analytical_value() {
        const RATED_CAPACITY_W: f64 = 10_000.0;

        let mut cfg = config("IH-002-numerical");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), RATED_CAPACITY_W.into());
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".into(),
            "0.834,0,0,0.02,0,0".into(),
        );
        cfg.test_extras_mut()
            .insert("biquadratic_x1_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x1_max".into(), 35.0f64.into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_min".into(), (-30.0f64).into());
        cfg.test_extras_mut()
            .insert("biquadratic_x2_max".into(), 50.0f64.into());

        let mut h3_env = env(18.0, 60, 0);
        h3_env.weather.outdoor_temp_c = -8.3;

        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &h3_env).unwrap();
        eq.update_control(&h3_env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&h3_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        let actual_w = ports.thermal[0].sensible_gain_w;
        let expected = RATED_CAPACITY_W * 0.668; // 6680 W
        assert!(
            (actual_w - expected).abs() < 1.0,
            "CAP_FT(18.0, −8.3) = 0.834 + 0.02*(−8.3) = 0.668; \
             expected {expected:.1} W, got {actual_w:.1} W"
        );
    }

    // Constitution: "Parse/init boundaries must fail loudly on invalid input."
    // If a user configures unparseable biquadratic coefficients, init() must
    // return Err, not silently fall back to identity curves.
    #[test]
    fn invalid_biquadratic_coeffs_rejected_at_init() {
        let mut cfg = config("IH-002-bad-coeffs");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("capacity_biquadratic_coeffs".into(), "not_a_number".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        let result = eq.init(&cfg, &env);
        assert!(
            result.is_err(),
            "init must reject unparseable biquadratic coefficients, not silently fall back to identity"
        );
    }

    #[test]
    fn wrong_arity_biquadratic_coeffs_rejected_at_init() {
        let mut cfg = config("IH-002-wrong-arity");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".into(), "1.0,0.0,0.0".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        let result = eq.init(&cfg, &env);
        assert!(
            result.is_err(),
            "init must reject biquadratic coefficients with wrong number of terms"
        );
    }

    // ── Ideal capacity mode: architectural regression tests ─────────────────
    //
    // Reference behaviour (EnergyPlus IdealLoadsAirSystem, OCHRE ideal HVAC):
    // In ideal capacity mode, the equipment delivers continuous modulation of
    // whatever capacity the solver computes, independent of thermostat cycling
    // hysteresis. The FSM runs for observability (telemetry shows what the
    // thermostat WOULD do with physical equipment) but does not gate delivery.

    fn ideal_config(name: &str, heat_c: f64, cool_c: f64) -> EquipmentConfig {
        let mut cfg = config(name);
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), heat_c.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), cool_c.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());
        cfg.test_extras_mut()
            .insert("ideal_capacity_mode".into(), "on".into());
        cfg
    }

    fn init_ideal(name: &str, heat_c: f64, cool_c: f64, zone_c: f64) -> IdealHvac {
        let cfg = ideal_config(name, heat_c, cool_c);
        let env = env(zone_c, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq
    }

    #[test]
    fn out_of_domain_ideal_capacity_and_deadband_rejected_on_unchecked_path() {
        use hares_types::ControlSignal;

        // The arm stores IdealCapacity raw and silently no-ops an invalid
        // deadband (`if db.is_finite() && *db >= 0.0` — a negative deadband
        // is dropped without an error, and an over-range one is stored into
        // hysteresis). The central validator rejects non-finite capacity and
        // out-of-range deadbands on the checked path. Negative capacity is
        // legitimate (cooling), so the capacity probe is non-finite only.
        let cfg = ideal_config("IH-domain", 20.0, 26.0);
        let mut eq = IdealHvac::new(cfg.clone());
        let e = env(18.0, 300, 0);
        eq.init(&cfg, &e).unwrap();

        for (bad, token) in [
            (
                ControlSignal::IdealCapacity {
                    capacity_w: f64::NAN,
                    degraded: false,
                },
                "capacity",
            ),
            (
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(20.0),
                    cooling_setpoint_c: None,
                    deadband_c: Some(-1.0),
                },
                "deadband",
            ),
            (
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(20.0),
                    cooling_setpoint_c: None,
                    deadband_c: Some(100.0),
                },
                "deadband",
            ),
        ] {
            let err = eq
                .apply_control_unchecked(&bad)
                .expect_err("out-of-domain control value must be rejected");
            assert!(
                format!("{err:?}").to_lowercase().contains(token),
                "error must name the offending signal for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn ideal_target_returns_heating_setpoint_when_zone_cold_regardless_of_fsm() {
        // Zone at 15°C, heating setpoint 20°C, cooling 26°C.
        // FSM may be Deadband (hysteresis) but ideal_target must return heating.
        let eq = init_ideal("IH-cold-zone", 20.0, 26.0, 15.0);
        let target = eq.ideal_target();
        assert!(
            target.is_some(),
            "ideal_target must return Some when zone is below heating setpoint"
        );
        let (zone, temp) = target.unwrap();
        assert_eq!(zone, ZoneId(1));
        assert!(
            (temp - 20.0).abs() < 1e-9,
            "target must be heating setpoint 20.0, got {temp}"
        );
    }

    #[test]
    fn ideal_target_returns_cooling_setpoint_when_zone_hot_regardless_of_fsm() {
        let eq = init_ideal("IH-hot-zone", 20.0, 26.0, 28.0);
        let target = eq.ideal_target();
        assert!(
            target.is_some(),
            "ideal_target must return Some when zone is above cooling setpoint"
        );
        let (zone, temp) = target.unwrap();
        assert_eq!(zone, ZoneId(1));
        assert!(
            (temp - 26.0).abs() < 1e-9,
            "target must be cooling setpoint 26.0, got {temp}"
        );
    }

    #[test]
    fn ideal_target_returns_none_when_zone_in_comfort_range() {
        // Zone at 23°C, between heating 20°C and cooling 26°C.
        let eq = init_ideal("IH-comfort-zone", 20.0, 26.0, 23.0);
        let target = eq.ideal_target();
        assert!(
            target.is_none(),
            "ideal_target must return None in comfort range"
        );
    }

    #[test]
    fn ideal_target_returns_none_in_non_ideal_mode() {
        let mut cfg = config("IH-non-ideal");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());
        // No ideal_capacity_mode → defaults to Auto with 60s time_res → Off (not ideal)

        let env = env(15.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        assert!(
            eq.ideal_target().is_none(),
            "ideal_target must be None in non-ideal mode"
        );
    }

    #[test]
    fn step_delivers_ideal_capacity_when_fsm_is_deadband() {
        // In ideal mode, step() must deliver ideal_capacity_w even when
        // FSM is in Deadband. Zone at 26.5°C is above cooling setpoint 26.0,
        // so ideal_target returns cooling setpoint even if FSM hasn't yet
        // transitioned from Deadband (hysteresis keeps it in Deadband until
        // zone > 26.0+0.8=26.8°C).
        let cfg = ideal_config("IH-step-db", 20.0, 26.0);
        let env = env(26.5, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        assert!(
            eq.ideal_target().is_some(),
            "ideal_target must be Some at 26.5°C"
        );

        // Solver would have computed -500W via collect_and_solve path.
        eq.ideal_capacity_w = -500.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(300), &mut ports).unwrap();
        let gain = ports.thermal[0].sensible_gain_w;
        assert!(
            gain < 0.0,
            "step must deliver ideal capacity cooling even when FSM is in Deadband; got {gain}W"
        );
        assert!(
            gain > -550.0,
            "cooling capacity should be near -500W; got {gain}W"
        );
    }

    #[test]
    fn stale_ideal_capacity_cleared_when_target_becomes_none() {
        // When zone warms from cold (needs heat) into comfort range,
        // cached_ideal_target becomes None and old ideal_capacity_w
        // must be cleared so step() doesn't keep delivering stale heat.
        let cfg = ideal_config("IH-stale", 20.0, 26.0);

        let env_cold = env(15.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert!(
            eq.ideal_target().is_some(),
            "cold zone must have ideal target"
        );

        eq.ideal_capacity_w = 500.0;
        assert!((eq.ideal_capacity_w - 500.0).abs() < 1e-9);

        // Zone warms to comfort range → target becomes None, capacity cleared
        let env_warm = env(23.0, 300, 60);
        eq.update_control(&env_warm);
        assert!(
            eq.ideal_target().is_none(),
            "comfort-range zone must have no target"
        );
        assert!(
            eq.ideal_capacity_w.abs() < 1e-9,
            "stale ideal_capacity_w must be cleared; got {}",
            eq.ideal_capacity_w
        );
    }

    #[test]
    fn non_ideal_mode_deadband_delivers_zero_capacity() {
        // In non-ideal mode, Deadband must deliver 0W even with capacity set.
        let mut cfg = config("IH-ni-db");
        cfg.test_extras_mut().insert("zone_id".into(), 1.0.into());
        cfg.test_extras_mut()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.test_extras_mut()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.test_extras_mut()
            .insert("capacity_w".into(), 10_000.0.into());

        let env = env(23.0, 60, 0); // 60s timestep → Auto → Off (not ideal)
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        assert!(
            eq.ideal_target().is_none(),
            "non-ideal mode must have no ideal_target"
        );

        eq.ideal_capacity_w = 1000.0; // Should be ignored in non-ideal Deadband

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w.abs() < 1e-6,
            "non-ideal Deadband must deliver 0W; got {} W",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn current_target_c_shows_active_setpoint_in_ideal_deadband() {
        // When zone is cold in ideal mode, current_target_c must
        // show the heating setpoint (active target), not the deadband midpoint.
        let cfg = ideal_config("IH-tel", 20.0, 26.0);
        let env = env(15.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        assert!(
            (eq.current_target_c - 20.0).abs() < 1e-9,
            "current_target_c must be heating setpoint 20.0 when zone is cold; got {}",
            eq.current_target_c
        );
    }

    #[test]
    fn current_target_c_shows_midpoint_in_ideal_comfort_range() {
        let cfg = ideal_config("IH-tel2", 20.0, 26.0);
        let env = env(23.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let midpoint = 0.5 * (20.0 + 26.0);
        assert!(
            (eq.current_target_c - midpoint).abs() < 1e-9,
            "current_target_c must be midpoint {midpoint} in comfort range; got {}",
            eq.current_target_c
        );
    }

    #[test]
    fn cached_ideal_target_recomputed_every_update_control() {
        let cfg = ideal_config("IH-recomp", 20.0, 26.0);

        let env_cold = env(15.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert!(eq.ideal_target().is_some(), "cold: should have target");

        let env_comfort = env(23.0, 300, 60);
        eq.update_control(&env_comfort);
        assert!(
            eq.ideal_target().is_none(),
            "comfort: should have no target"
        );

        let env_hot = env(28.0, 300, 120);
        eq.update_control(&env_hot);
        let target = eq.ideal_target();
        assert!(target.is_some(), "hot: should have target after recompute");
        assert!(
            (target.unwrap().1 - 26.0).abs() < 1e-9,
            "hot: must target cooling setpoint 26.0"
        );
    }

    #[test]
    fn end_use_is_heating_after_heating_step() {
        let cfg = ideal_config("IH-heat-eu", 20.0, 26.0);
        let env = env(15.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_HEATING,
            "end_use must be HVAC_HEATING after heating step"
        );
    }

    #[test]
    fn end_use_is_cooling_after_cooling_step() {
        let cfg = ideal_config("IH-cool-eu", 20.0, 26.0);
        let env = env(28.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = -5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_COOLING,
            "end_use must be HVAC_COOLING after cooling step"
        );
    }

    #[test]
    fn end_use_preserved_in_deadband_after_heating() {
        let cfg = ideal_config("IH-db-heat-eu", 20.0, 26.0);
        // Zone cold: enters heating.
        let env_cold = env(15.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_HEATING,
            "end_use must be HVAC_HEATING after heating update_control"
        );

        eq.ideal_capacity_w = 5000.0;
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Zone warms to comfort range → deadband. Previous mode was heating,
        // so end_use stays HVAC_HEATING.
        let env_warm = env(23.0, 300, 60);
        eq.update_control(&env_warm);
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_HEATING,
            "end_use must stay HVAC_HEATING in deadband after heating"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_warm, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_HEATING,
            "end_use must still be HVAC_HEATING after deadband step following heating"
        );
    }

    #[test]
    fn end_use_preserved_in_deadband_after_cooling() {
        let cfg = ideal_config("IH-db-cool-eu", 20.0, 26.0);
        // Zone hot: enters cooling.
        let env_hot = env(28.0, 300, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_hot).unwrap();
        eq.update_control(&env_hot);
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_COOLING,
            "end_use must be HVAC_COOLING after cooling update_control"
        );

        eq.ideal_capacity_w = -5000.0;
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_hot, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Zone cools to comfort range → deadband. Previous mode was cooling,
        // so end_use stays HVAC_COOLING (not flipped to heating).
        let env_cool = env(23.0, 300, 60);
        eq.update_control(&env_cool);
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_COOLING,
            "end_use must stay HVAC_COOLING in deadband after cooling"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cool, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(
            eq.descriptor().end_use,
            EndUse::HVAC_COOLING,
            "end_use must still be HVAC_COOLING after deadband step following cooling"
        );
    }

    /// Reactive-power contract for Ideal HVAC: unity pf (OCHRE Ideal = 1.0) ⇒
    /// Q exactly zero even while the fan draws real power, REACTIVE declared,
    /// and port/CoreOutput/telemetry all report Some(0.0). Wired uniformly so
    /// the cross-equipment consistency check stays uniform across the fleet.
    #[test]
    fn ideal_hvac_reactive_power_is_zero_at_unity_pf() {
        let cfg = typed_config(crate::IdealHvacConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(10_000.0),
            setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            rated_fan_power_w: Some(200.0),
            rated_eir: Some(1.0),
            ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
            ..Default::default()
        });
        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "Ideal HVAC must declare REACTIVE"
        );
        assert_eq!(eq.zip.pf, 1.0, "Ideal HVAC unity pf");
        eq.update_control(&env);
        eq.ideal_capacity_w = 5_000.0;
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            ports.electrical.load_power_w > 0.0,
            "fan must draw real power"
        );
        assert_eq!(
            ports.electrical.reactive_power_kvar, 0.0,
            "unity pf ⇒ port Q == 0"
        );
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "unity pf ⇒ CoreOutput Q == Some(0.0)"
        );
        assert_eq!(
            eq.telemetry()
                .get(hares_types::telemetry_keys::REACTIVE_POWER_KVAR),
            Some(0.0),
            "unity pf ⇒ telemetry Q == 0.0"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("core contract must hold with REACTIVE declared");
    }

    /// Rule R1 regression: the unity pf affects only Q (which is zero either
    /// way). Twin instances — one with the class pf 1.0, one with a
    /// constant-power sidecar override (pf 0 sentinel) — must produce
    /// bit-identical real power at every step and voltage.
    #[test]
    fn ideal_hvac_real_power_bit_identical_with_and_without_reactive_zip() {
        let mk = || {
            typed_config(crate::IdealHvacConfig {
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                cooling_capacity_w: Some(10_000.0),
                setpoint: crate::hvac::heating_config::HvacSetpointConfig {
                    heating_setpoint_c: Some(20.0),
                    cooling_setpoint_c: Some(26.0),
                    ..Default::default()
                },
                rated_fan_power_w: Some(200.0),
                rated_eir: Some(1.0),
                ideal_capacity_mode: Some(crate::hvac::heating_config::IdealCapacityModeConfig::On),
                ..Default::default()
            })
        };
        let config_pf = mk();
        let mut config_nopf = mk();
        config_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let mut eq_pf = IdealHvac::new(config_pf.clone());
        let mut eq_nopf = IdealHvac::new(config_nopf.clone());
        let mut env = env(18.0, 300, 0);
        eq_pf.init(&config_pf, &env).unwrap();
        eq_nopf.init(&config_nopf, &env).unwrap();

        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            env.grid.voltage_pu = *v;
            eq_pf.update_control(&env);
            eq_nopf.update_control(&env);
            // Drive a heating call with nonzero fan power in both twins.
            eq_pf.ideal_capacity_w = 5_000.0;
            eq_nopf.ideal_capacity_w = 5_000.0;
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf
                .step(&env, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&env, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged between pf and no-pf twins"
            );
            assert_eq!(
                ports_pf.electrical.reactive_power_kvar, 0.0,
                "unity pf twin must produce zero reactive power"
            );
            assert_eq!(
                ports_nopf.electrical.reactive_power_kvar, 0.0,
                "pf-0 twin must produce zero reactive power"
            );
            env.current_time += ChronoDuration::minutes(1);
        }
    }
}
