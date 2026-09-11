//! Mechanical ventilation equipment: exhaust fans, HRV, and ERV.
//!
//! Models residential mechanical ventilation per ASHRAE 62.2 with optional
//! heat/energy recovery. Better than OCHRE (which uses ScheduledLoad) by
//! computing actual supply air conditions from recovery effectiveness.
//!
//! EnergyPlus-grade physics:
//! - `T_supply = T_outdoor + ε_sensible × (T_indoor - T_outdoor)`
//! - `W_supply = W_outdoor + ε_latent × (W_indoor - W_outdoor)` (ERV only)
//! - Bypass mode for free cooling when outdoor conditions are favorable
//! - Defrost derating at low outdoor temperatures

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::air_properties::dry_air_density_kg_m3;
use hares_physics::constants::{
    CP_DRY_AIR_J_KG_K, LATENT_HEAT_VAPORISATION_0C_J_KG, SEA_LEVEL_PRESSURE_PA,
};
use hares_physics::units::power_w_to_kw;
use hares_types::zip::{ResolvedZip, ZipLoad};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, ScheduleSource, Telemetry, TelemetryField, ZoneId, ZoneRole,
};
use serde::{Deserialize, Serialize};
use tracing::error;

use hares_types::telemetry_keys as tk;

use crate::config::EquipmentTypedConfig;
use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

// ---------------------------------------------------------------------------
// Typed config
// ---------------------------------------------------------------------------

/// Typed configuration for mechanical ventilation (exhaust fan, HRV, ERV).
///
/// `flow_rate_m3_s` must be provided in SI units (m³/s). Convert CFM at the
/// parse boundary using `hares_physics::constants::CFM_TO_M3_S`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VentilationConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub flow_rate_m3_s: f64,
    /// Total combined fan power [W] for exhaust + supply fans.
    /// For balanced systems (HRV/ERV) when supply/exhaust fan power is not
    /// provided separately, this is assumed to be the total and is split
    /// equally between supply and exhaust fans. For exhaust-only systems
    /// this is the single exhaust fan power.
    pub fan_power_w: Option<f64>,
    /// Rated supply fan power [W] for balanced systems.
    /// When both supply and exhaust fan power are provided, fan_power_w
    /// is ignored. For exhaust-only systems this should be None or 0.
    pub supply_fan_power_w: Option<f64>,
    /// Rated exhaust fan power [W]. See supply_fan_power_w for interaction rules.
    pub exhaust_fan_power_w: Option<f64>,
    pub sensible_effectiveness: Option<f64>,
    pub latent_effectiveness: Option<f64>,
    pub bypass_temp_min_c: Option<f64>,
    pub bypass_temp_max_c: Option<f64>,
    pub defrost_temp_c: Option<f64>,
    /// Initial defrost time fraction at exactly the threshold temperature [—].
    ///
    /// EnergyPlus ExhaustOnly / ExhaustAirRecirculation frost control:
    /// `InitialDefrostTime = 0.083` (IDD default). HARES defaults to 0.0
    /// so that at the threshold temperature the recovery operates at full
    /// rated effectiveness with no defrost derating — a practical
    /// residential default that corresponds to frost-control strategies
    /// that do not cycle the supply fan at the initiation temperature
    /// (EnergyPlus HeatRecovery.cc:2996–2997,3034–3035).
    pub defrost_initial_time_fraction: Option<f64>,
    /// Rate of defrost time fraction increase per Kelvin below the threshold [1/K].
    ///
    /// EnergyPlus ExhaustOnly / ExhaustAirRecirculation frost control:
    /// `RateofDefrostTimeIncrease = 0.012` (IDD default). HARES uses
    /// 0.05 as a conservative residential default that reaches full
    /// defrost at 20 K below threshold — consistent with the observation
    /// that residential HRVs in cold climates reach DFFraction ≈ 1.0
    /// at roughly −25 °C outdoor air temperature.
    /// EnergyPlus HeatRecovery.cc:2997,3035.
    pub defrost_time_increase_rate_per_k: Option<f64>,
    /// Ventilation type: "exhaust_fan", "hrv", or "erv"
    pub ventilation_type: Option<String>,
    /// Informational: whether the system is balanced (HRV/ERV) or one-directional
    /// (exhaust/supply). Populated by HPXML parser for reporting/diagnostics.
    /// Does not affect simulation behaviour — `init_typed` derives balanced state
    /// from `ventilation_type`, not this field.
    pub balanced: Option<bool>,
    /// Daily hours of operation (0–24). Used by the EA-001 energy audit model.
    pub hours_in_operation: Option<f64>,
}

impl EquipmentTypedConfig for VentilationConfig {
    fn equipment_type_name() -> &'static str {
        "Ventilation"
    }
}

impl VentilationConfig {
    /// Validate fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.flow_rate_m3_s.is_finite() || self.flow_rate_m3_s < 0.0 {
            return Err(HaresError::Equipment(
                "ventilation flow_rate_m3_s must be finite and >= 0".to_string(),
            ));
        }
        if let Some(power) = self.fan_power_w {
            if !power.is_finite() || power < 0.0 {
                return Err(HaresError::Equipment(
                    "ventilation fan_power_w must be finite and >= 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("supply_fan_power_w", self.supply_fan_power_w),
            ("exhaust_fan_power_w", self.exhaust_fan_power_w),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v < 0.0 {
                    return Err(HaresError::Equipment(format!(
                        "ventilation {name} must be finite and >= 0"
                    )));
                }
            }
        }
        for (name, val) in [
            ("sensible_effectiveness", self.sensible_effectiveness),
            ("latent_effectiveness", self.latent_effectiveness),
            (
                "defrost_initial_time_fraction",
                self.defrost_initial_time_fraction,
            ),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(HaresError::Equipment(format!(
                        "ventilation {name} must be finite and within [0, 1]"
                    )));
                }
            }
        }
        if let Some(rate) = self.defrost_time_increase_rate_per_k {
            if !rate.is_finite() || rate < 0.0 {
                return Err(HaresError::Equipment(
                    "ventilation defrost_time_increase_rate_per_k must be finite and >= 0"
                        .to_string(),
                ));
            }
        }
        if let Some(hours) = self.hours_in_operation
            && (!hours.is_finite() || !(0.0..=24.0).contains(&hours))
        {
            return Err(HaresError::Equipment(
                "ventilation hours_in_operation must be finite and within [0, 24]".to_string(),
            ));
        }
        Ok(())
    }
}

const KEY_EQUIPMENT_ID: &str = "equipment_id";
use crate::config::KEY_ZONE_ID;

/// Default rated fan power [W].
///
/// ASHRAE 62.2-2016 §4.1 Table 1: minimum fan efficacy 1.4 cfm/W for
/// non-HRV/ERV mechanical exhaust. At the default 75 CFM whole-house
/// ventilation rate, the minimum allowable fan power is 75/1.4 ≈ 54 W.
/// 50 W exceeds this minimum and represents a reasonably efficient
/// residential exhaust fan.
const DEFAULT_FAN_POWER_W: f64 = 50.0;

/// Default ventilation flow rate [m³/s].
///
/// ASHRAE 62.2-2016 §4.1 Eq.1: Q_fan = 0.01×A_floor + 7.5×(N_br+1) CFM.
/// For a typical 3-bedroom 2000 ft² home the minimum is 50 CFM (0.024 m³/s).
/// 0.035 m³/s (~75 CFM) provides a practical margin above the code minimum
/// that matches common residential installer practice and the typical
/// 0.35 ACH for a 350 m³ volume home.
const DEFAULT_FLOW_RATE_M3_S: f64 = 0.035;

/// Default sensible heat recovery effectiveness [—].
///
/// ASHRAE HoF 2021 Ch.25 Table 3: typical residential HRV sensible
/// effectiveness at balanced flow is 0.65–0.80 under CSA-C439
/// (now ASHRAE 84) test conditions. 0.70 is a conservative midpoint.
const DEFAULT_SENSIBLE_EFFECTIVENESS: f64 = 0.70;

/// Default latent heat recovery effectiveness [—].
///
/// HRV default: no latent recovery. An ERV configuration would
/// set a positive value, typically 0.45–0.65 per the same ASHRAE
/// HoF 2021 Ch.25 Table 3 range for ERV latent effectiveness.
const DEFAULT_LATENT_EFFECTIVENESS: f64 = 0.0;

/// Default bypass temperature minimum [°C].
///
/// ASHRAE 55-2017 §5.3 Fig.5.3.1: winter comfort zone lower
/// bound (operative temperature ~20 °C at 1.0 clo, ~18 °C at the lower
/// 80%-acceptability limit). When outdoor temperature exceeds this
/// threshold and is within the comfort range, bypassing the HX core
/// provides free cooling without over-cooling the zone.
const DEFAULT_BYPASS_TEMP_MIN_C: f64 = 18.0;

/// Default bypass temperature maximum [°C].
///
/// ASHRAE 55-2017 §5.3 Fig.5.3.1: summer comfort zone upper
/// bound (operative temperature ~24 °C at 0.5 clo). Above this
/// threshold outdoor air would add unwanted heating to the zone.
const DEFAULT_BYPASS_TEMP_MAX_C: f64 = 24.0;

/// Default outdoor air temperature below which defrost derating is
/// applied [°C].
///
/// Residential HRV manufacturers typically set exhaust-air defrost
/// initiation at −5 to −10 °C to prevent core frosting. −5 °C is the
/// default outdoor-air defrost threshold in Canadian and northern
/// U.S. residential compliance modelling. (EnergyPlus HeatExchanger:
/// AirToAir:SensibleAndLatent uses a 1.7 °C default threshold for
/// commercial systems; HARES uses −5 °C for residential HRV where
/// higher indoor exhaust temperatures delay frost formation.)
const DEFAULT_DEFROST_TEMP_C: f64 = -5.0;

/// Default defrost initiation time fraction at the threshold temperature [—].
///
/// Zero is a practical residential default: at exactly the threshold
/// temperature the recovery core operates at full rated effectiveness with
/// no defrost derating. This corresponds to frost-control strategies that
/// begin cycling the supply fan only below the initiation temperature.
/// EnergyPlus ExhaustOnly frost control uses InitialDefrostTime = 0.083
/// (HeatRecovery.cc:2996–2997); HARES defaults to 0.0 for a more
/// conservative onset of derating in residential compliance modelling.
const DEFAULT_DEFROST_INITIAL_TIME_FRACTION: f64 = 0.0;

/// Default defrost time increase rate per Kelvin below threshold [1/K].
///
/// At 0.05 1/K the defrost fraction reaches 1.0 at 20 K below the
/// threshold, e.g. at −25 °C with the default threshold of −5 °C.
/// EnergyPlus ExhaustOnly default is 0.012 1/K (HeatRecovery.cc:2997),
/// which reaches full defrost at ~76 K below threshold. The steeper HARES
/// rate is chosen because residential HRVs in cold climates (ASHRAE
/// climate zones 6–8) are observed to reach continuous defrost at
/// approximately −25 °C to −30 °C outdoor air temperature per
/// manufacturer field data (Venmar, Lifebreath, Zehnder).
const DEFAULT_DEFROST_TIME_INCREASE_RATE_PER_K: f64 = 0.05;

/// Fraction of rated supply fan power consumed during bypass [—].
///
/// Fan affinity laws: for a fixed-speed fan operating against a duct
/// system, shaft power P ∝ ΔP³´² where ΔP is the system pressure drop.
/// The HX core typically accounts for ~40 % of total system pressure
/// drop in a residential HRV (ASHRAE HoF 2021 Ch.21 Fig.4: ~60–80 Pa
/// core drop out of ~150 Pa total). Bypassing the core reduces ΔP to
/// ~60 % of rated, so P_bypass / P_rated = 0.60³´² ≈ 0.46–0.54.
/// 0.6 is a conservative value that slightly overestimates fan power
/// during bypass, which is safe for energy-consumption estimates.
const BYPASS_SUPPLY_FAN_POWER_FRACTION: f64 = 0.6;

/// Ventilation type determines whether latent recovery is modeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VentilationType {
    /// Simple exhaust fan -- no recovery.
    ExhaustFan,
    /// Heat recovery ventilator -- sensible recovery only.
    Hrv,
    /// Energy recovery ventilator -- sensible + latent recovery.
    Erv,
}

fn parse_ventilation_type(raw: Option<&str>) -> crate::Result<VentilationType> {
    let Some(raw) = raw else {
        return Ok(VentilationType::Hrv);
    };
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "exhaust_fan" | "exhaust fan" | "exhaust only" | "supply only" | "whole house fan" => {
            Ok(VentilationType::ExhaustFan)
        }
        "hrv" | "heat recovery ventilator" => Ok(VentilationType::Hrv),
        "erv" | "energy recovery ventilator" => Ok(VentilationType::Erv),
        _ => Err(HaresError::Equipment(format!(
            "unrecognised ventilation_type '{value}'; \
             expected one of: exhaust_fan, exhaust fan, exhaust only, supply only, \
             whole house fan, hrv, heat recovery ventilator, erv, energy recovery ventilator"
        ))),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VentilationCheckpoint {
    mode: OperatingMode,
    dr_level: DRLevel,
    dr_duration_remaining_s: Option<f64>,
    schedule_source_state: ScheduleSourceState,
}

pub struct Ventilation {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    /// Rule R1 reactive-only ZIP: real power stays bit-identical;
    /// Q comes from ZipLoad::reactive_kvar. Ventilation fan pf 0.87.
    zip: ResolvedZip,

    ventilation_type: VentilationType,
    zone_id: ZoneId,
    /// Rated supply fan power [W] for balanced systems; 0 for exhaust-only.
    supply_fan_power_w: f64,
    /// Rated exhaust fan power [W].
    exhaust_fan_power_w: f64,
    flow_rate_m3_s: f64,
    sensible_effectiveness: f64,
    latent_effectiveness: f64,

    // Per-timestep effective values (accounting for bypass and defrost).
    // Initialised to rated values; overwritten each step by step().
    // Read by the dwelling orchestration layer to update ThermalSolverConfig.ventilation
    // before the thermal solver integrates.
    effective_sensible_effectiveness: f64,
    effective_latent_effectiveness: f64,

    // Bypass: when outdoor temp is within comfort range, bypass recovery (free cooling).
    bypass_temp_min_c: f64,
    bypass_temp_max_c: f64,

    // Defrost: at low outdoor temps, reduce effectiveness continuously
    // per EnergyPlus frost-control fraction formula.
    defrost_temp_c: f64,
    defrost_initial_time_fraction: f64,
    defrost_time_increase_rate_per_k: f64,

    schedule_source: ScheduleSource,

    mode: OperatingMode,
    dr_level: DRLevel,
    dr_duration_remaining_s: Option<f64>,
}

impl Ventilation {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let equipment_id = config
            .get_f64(KEY_EQUIPMENT_ID)
            .map(|v| v as u32)
            .unwrap_or(0);
        // Resolve zone: explicit config value first, then ZoneMap, then
        // ZoneId(1) as last-resort default (overridden by init_typed() when
        // ZoneMap is available after dwelling construction).
        let zone_id = config
            .get_f64(KEY_ZONE_ID)
            .map(|v| ZoneId(v as u16))
            .or_else(|| {
                config
                    .zone_map
                    .as_ref()
                    .and_then(|zm| zm.get(ZoneRole::Indoor))
            })
            .unwrap_or(ZoneId(1));

        let ventilation_type = match parse_ventilation_type(config.get_str("ventilation_type")) {
            Ok(vt) => vt,
            Err(e) => {
                error!("{e}");
                VentilationType::Hrv
            }
        };

        let end_use = EndUse::VENTILATION;

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use,
            equipment_type: Cow::Borrowed(match ventilation_type {
                VentilationType::ExhaustFan => "ExhaustFan",
                VentilationType::Hrv => "HRV",
                VentilationType::Erv => "ERV",
            }),
            zone: Some(zone_id),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE
                | ControlCapabilities::LOAD_FRACTION,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: telemetry_fields(),
            zone_type: None,
        };

        let ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(zone_id),
        ];

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            zip: ResolvedZip::reactive_only(ZipLoad::constant_power()),
            ventilation_type,
            zone_id,
            // Default type is HRV (balanced): pre-init split is 25 W / 25 W.
            // init_typed() overrides with actual config values.
            supply_fan_power_w: DEFAULT_FAN_POWER_W / 2.0,
            exhaust_fan_power_w: DEFAULT_FAN_POWER_W / 2.0,
            flow_rate_m3_s: DEFAULT_FLOW_RATE_M3_S,
            sensible_effectiveness: DEFAULT_SENSIBLE_EFFECTIVENESS,
            latent_effectiveness: DEFAULT_LATENT_EFFECTIVENESS,
            effective_sensible_effectiveness: DEFAULT_SENSIBLE_EFFECTIVENESS,
            effective_latent_effectiveness: DEFAULT_LATENT_EFFECTIVENESS,
            bypass_temp_min_c: DEFAULT_BYPASS_TEMP_MIN_C,
            bypass_temp_max_c: DEFAULT_BYPASS_TEMP_MAX_C,
            defrost_temp_c: DEFAULT_DEFROST_TEMP_C,
            defrost_initial_time_fraction: DEFAULT_DEFROST_INITIAL_TIME_FRACTION,
            defrost_time_increase_rate_per_k: DEFAULT_DEFROST_TIME_INCREASE_RATE_PER_K,
            schedule_source: ScheduleSource::Constant(1.0),
            mode: OperatingMode::Off,
            dr_level: DRLevel::Normal,
            dr_duration_remaining_s: None,
        }
    }

    /// Continuous defrost time fraction per EnergyPlus `ExhaustOnly` /
    /// `ExhaustAirRecirculation` frost control.
    ///
    /// EnergyPlus HeatRecovery.cc:2996,3034:
    ///   DFFraction = max(0, min(initial_defrost_time
    ///     + rate_of_increase × (threshold − T_outdoor), 1))
    fn compute_defrost_fraction(&self, t_outdoor_c: f64) -> f64 {
        let deficit = self.defrost_temp_c - t_outdoor_c;
        if deficit <= 0.0 {
            return 0.0;
        }
        (self.defrost_initial_time_fraction + self.defrost_time_increase_rate_per_k * deficit)
            .clamp(0.0, 1.0)
    }

    /// Effective sensible effectiveness after bypass and defrost adjustments.
    fn compute_effective_sensible_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type == VentilationType::ExhaustFan {
            return 0.0;
        }
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        self.sensible_effectiveness * (1.0 - self.compute_defrost_fraction(t_outdoor_c))
    }

    /// Effective latent effectiveness (ERV only).
    fn compute_effective_latent_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type != VentilationType::Erv {
            return 0.0;
        }
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        self.latent_effectiveness * (1.0 - self.compute_defrost_fraction(t_outdoor_c))
    }
}

impl Ventilation {
    fn init_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let c = config.require_typed::<VentilationConfig>("Ventilation")?;
        c.validate()?;
        self.zip = crate::config::resolve_reactive_zip(config)?;

        self.ventilation_type = parse_ventilation_type(c.ventilation_type.as_deref())?;
        self.descriptor.equipment_type = Cow::Borrowed(match self.ventilation_type {
            VentilationType::ExhaustFan => "ExhaustFan",
            VentilationType::Hrv => "HRV",
            VentilationType::Erv => "ERV",
        });
        self.flow_rate_m3_s = c.flow_rate_m3_s;

        // Resolve fan power: prefer explicit supply/exhaust, fall back to fan_power_w.
        match (c.supply_fan_power_w, c.exhaust_fan_power_w) {
            (Some(s), Some(e)) => {
                self.supply_fan_power_w = s;
                self.exhaust_fan_power_w = e;
            }
            _ => {
                let total = c.fan_power_w.unwrap_or(DEFAULT_FAN_POWER_W);
                let is_balanced = self.ventilation_type == VentilationType::Hrv
                    || self.ventilation_type == VentilationType::Erv;
                if is_balanced {
                    tracing::info!(
                        total_fan_power_w = total,
                        "fan_power_w used as total for balanced system; \
                         splitting equally between supply and exhaust"
                    );
                    self.supply_fan_power_w = total / 2.0;
                    self.exhaust_fan_power_w = total / 2.0;
                } else {
                    self.supply_fan_power_w = 0.0;
                    self.exhaust_fan_power_w = total;
                }
            }
        }
        self.sensible_effectiveness = c
            .sensible_effectiveness
            .unwrap_or(DEFAULT_SENSIBLE_EFFECTIVENESS)
            .clamp(0.0, 1.0);
        self.latent_effectiveness = c
            .latent_effectiveness
            .unwrap_or(DEFAULT_LATENT_EFFECTIVENESS)
            .clamp(0.0, 1.0);
        self.bypass_temp_min_c = c.bypass_temp_min_c.unwrap_or(DEFAULT_BYPASS_TEMP_MIN_C);
        self.bypass_temp_max_c = c.bypass_temp_max_c.unwrap_or(DEFAULT_BYPASS_TEMP_MAX_C);
        self.defrost_temp_c = c.defrost_temp_c.unwrap_or(DEFAULT_DEFROST_TEMP_C);
        self.defrost_initial_time_fraction = c
            .defrost_initial_time_fraction
            .unwrap_or(DEFAULT_DEFROST_INITIAL_TIME_FRACTION)
            .clamp(0.0, 1.0);
        self.defrost_time_increase_rate_per_k = c
            .defrost_time_increase_rate_per_k
            .unwrap_or(DEFAULT_DEFROST_TIME_INCREASE_RATE_PER_K)
            .max(0.0);

        // hours_in_operation is already validated by c.validate() above.
        let schedule_frac = c
            .hours_in_operation
            .map(|hours| (hours / 24.0).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        self.schedule_source = ScheduleSource::Constant(schedule_frac);

        self.mode = OperatingMode::Standby;
        self.core_output = CoreOutput::default();

        // Sync effective fields to the configured rated values so that
        // consumers calling effective_ventilation_effectiveness() before
        // the first step() (e.g. during pre-run diagnostics) see the
        // correct rated defaults rather than the hard-coded new() values.
        self.effective_sensible_effectiveness = self.sensible_effectiveness;
        self.effective_latent_effectiveness = self.latent_effectiveness;

        // Resolve zone from ZoneMap when available. Ventilation equipment
        // routes thermal contributions to the indoor conditioned zone. The
        // ZoneMap provides the correct ZoneId from the building envelope
        // configuration, replacing the hardcoded ZoneId(1) default set in new().
        if let Some(zone_map) = &config.zone_map {
            if let Some(resolved_id) = zone_map.get(ZoneRole::Indoor) {
                self.zone_id = resolved_id;
                self.descriptor.zone = Some(resolved_id);
                self.ports = vec![
                    PortDeclaration::electrical(),
                    PortDeclaration::thermal(resolved_id),
                ];
            }
        }

        Ok(())
    }
}

impl Equipment for Ventilation {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.init_typed(config)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        if let Some(remaining) = self.dr_duration_remaining_s.as_mut() {
            *remaining -= env.time_res.num_seconds() as f64;
            if *remaining <= 0.0 {
                self.dr_level = DRLevel::Normal;
                self.dr_duration_remaining_s = None;
            }
        }
        self.mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        // Grid outage: a de-energized bus removes the fan supply, so the unit
        // cannot run (no airflow, no recovery, no draw) — gated at the root
        // of the on/off decision (WH precedent). Islanded homes keep an
        // energized bus and are not affected. See docs/outage-behavior.md.
        let is_running = self.mode != OperatingMode::Off
            && self.dr_level != DRLevel::GridEmergency
            && env.grid.bus_energized();

        if !is_running {
            // Clear stored effective effectiveness so that the thermal solver
            // computes the full unconditioned ventilation load when the equipment
            // is off or in GridEmergency. Without this reset, stale values from
            // the most recent running timestep persist and are returned by
            // effective_ventilation_effectiveness(), causing the solver to
            // under-estimate the ventilation load by deducting recovery that
            // is not occurring.
            self.effective_sensible_effectiveness = 0.0;
            self.effective_latent_effectiveness = 0.0;

            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::REACTIVE_POWER_KVAR, 0.0);
            self.telemetry.set(tk::FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::SENSIBLE_RECOVERY_W, 0.0);
            self.telemetry.set(tk::LATENT_RECOVERY_W, 0.0);
            self.telemetry
                .set(tk::SUPPLY_TEMP_C, env.weather.outdoor_temp_c);
            self.telemetry.set(tk::BYPASS_ACTIVE, 0.0);
            self.mode = self.mode.resolve_idle(false, None);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: Some(0.0),
                    fuel_w: None,
                    thermal_output_w: None,
                    sensible_cooling_w: None,
                    latent_cooling_w: None,
                },
                state: CoreState {
                    operating_mode: Some(self.mode),
                    soc: None,
                    speed_index: None,
                    setpoint_c: None,
                },
                performance: CorePerformance::default(),
            };

            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                if self.effective_sensible_effectiveness != 0.0 {
                    return Err(HaresError::InvariantViolation {
                        check_name: "ventilation_effectiveness_zero_when_off".to_string(),
                        value: self.effective_sensible_effectiveness,
                        tolerance: 0.0,
                    });
                }
                if self.effective_latent_effectiveness != 0.0 {
                    return Err(HaresError::InvariantViolation {
                        check_name: "ventilation_effectiveness_zero_when_off".to_string(),
                        value: self.effective_latent_effectiveness,
                        tolerance: 0.0,
                    });
                }
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    mode = ?self.mode,
                    dr_level = ?self.dr_level,
                    effective_sensible_effectiveness = self.effective_sensible_effectiveness,
                    effective_latent_effectiveness = self.effective_latent_effectiveness,
                    "Ventilation not running: effectiveness cleared"
                );
            }

            return Ok(());
        }

        let schedule_frac = self.schedule_source.value_at(env)?.clamp(0.0, 1.0);
        let effective_flow_rate_m3_s = self.flow_rate_m3_s * schedule_frac;

        if effective_flow_rate_m3_s <= 0.0 {
            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::REACTIVE_POWER_KVAR, 0.0);
            self.telemetry.set(tk::FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::SENSIBLE_RECOVERY_W, 0.0);
            self.telemetry.set(tk::LATENT_RECOVERY_W, 0.0);
            self.telemetry
                .set(tk::SUPPLY_TEMP_C, env.weather.outdoor_temp_c);
            self.telemetry.set(tk::BYPASS_ACTIVE, 0.0);
            self.effective_sensible_effectiveness = 0.0;
            self.effective_latent_effectiveness = 0.0;
            self.mode = self.mode.resolve_idle(false, None);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: Some(0.0),
                    fuel_w: None,
                    thermal_output_w: None,
                    sensible_cooling_w: None,
                    latent_cooling_w: None,
                },
                state: CoreState {
                    operating_mode: Some(self.mode),
                    soc: None,
                    speed_index: None,
                    setpoint_c: None,
                },
                performance: CorePerformance::default(),
            };
            return Ok(());
        }

        let t_outdoor_c = env.weather.outdoor_temp_c;
        let zone = env.zones.iter().find(|z| z.id == self.zone_id);
        let t_indoor_c = zone.map(|z| z.temperature_c).unwrap_or(20.0);
        let w_indoor = zone.map(|z| z.humidity_ratio).unwrap_or(0.008);
        let w_outdoor = env.weather.outdoor_humidity_ratio;

        let eff_s = self.compute_effective_sensible_effectiveness(t_outdoor_c);
        let eff_l = self.compute_effective_latent_effectiveness(t_outdoor_c);
        let bypass_active = self.ventilation_type != VentilationType::ExhaustFan
            && t_outdoor_c >= self.bypass_temp_min_c
            && t_outdoor_c <= self.bypass_temp_max_c;

        let defrost_fraction = self.compute_defrost_fraction(t_outdoor_c);

        // Effective fan power per fan, scaled by schedule and operating conditions.
        let mut effective_supply_fan_power_w = self.supply_fan_power_w * schedule_frac;
        let effective_exhaust_fan_power_w = self.exhaust_fan_power_w * schedule_frac;

        if bypass_active {
            // Supply fan sees reduced pressure drop when air bypasses the HX core.
            effective_supply_fan_power_w *= BYPASS_SUPPLY_FAN_POWER_FRACTION;
        }
        if defrost_fraction > 0.0 {
            // Known approximation: supply fan power is scaled by (1 −
            // defrost_fraction) as a proxy for reduced supply-side operation
            // duration. The ticket (directive 3) calls for scaling by "the
            // actual mass flow fraction through each fan"; HARES does not yet
            // model per-fan defrost flow fractions. A proper defrost flow
            // fraction requires the T-0590 defrost model (time-fraction-based
            // frost control with supply/exhaust modulation). Until that lands,
            // the continuous defrost fraction is used as a conservative
            // first-order proxy.
            effective_supply_fan_power_w *= 1.0 - defrost_fraction;
        }

        let effective_fan_power_w = effective_supply_fan_power_w + effective_exhaust_fan_power_w;

        // Store effective values so the dwelling orchestration can propagate them
        // to ThermalSolverConfig.ventilation before the thermal solver integrates.
        self.effective_sensible_effectiveness = eff_s;
        self.effective_latent_effectiveness = eff_l;

        // Supply air conditions after heat recovery
        let t_supply_c = t_outdoor_c + eff_s * (t_indoor_c - t_outdoor_c);
        let w_supply = w_outdoor + eff_l * (w_indoor - w_outdoor);

        // Mass flow rate [kg/s]. Density computed from outdoor T, P each timestep
        // so mass flow is altitude-aware. Falls back to sea-level pressure when
        // pressure data is not available (pressure_kpa <= 0).
        // ASHRAE HoF 2021 Ch.1 Eq.28: ρ = P / (R_da × T)
        let pressure_pa = if env.weather.pressure_kpa > 0.0 {
            env.weather.pressure_kpa * 1000.0
        } else {
            SEA_LEVEL_PRESSURE_PA
        };
        let rho_kg_m3 = dry_air_density_kg_m3(pressure_pa, env.weather.outdoor_temp_c);
        let m_dot_kg_s = effective_flow_rate_m3_s * rho_kg_m3;

        // Sensible ventilation load to zone [W]:
        // Positive = heating the zone (supply warmer than outdoor but still cooler than indoor).
        // Sensible/latent ventilation loads are computed for diagnostic telemetry
        // but NOT pushed to thermal ports -- the envelope solver handles ventilation
        // heat exchange via apply_infiltration_and_ventilation().
        let _q_sensible_w = m_dot_kg_s * CP_DRY_AIR_J_KG_K * (t_supply_c - t_indoor_c);
        let _q_latent_w = m_dot_kg_s * LATENT_HEAT_VAPORISATION_0C_J_KG * (w_supply - w_indoor);

        // Sensible recovery [W] -- how much the HRV/ERV saved vs raw ventilation
        let q_recovery_sensible_w =
            m_dot_kg_s * CP_DRY_AIR_J_KG_K * eff_s * (t_indoor_c - t_outdoor_c);
        let q_recovery_latent_w =
            m_dot_kg_s * LATENT_HEAT_VAPORISATION_0C_J_KG * eff_l * (w_indoor - w_outdoor);

        // Fan electrical power [kW]
        let fan_kw = power_w_to_kw(effective_fan_power_w);
        let reactive_power_kvar = self.zip.reactive_kvar(fan_kw, env.grid.bus_voltage_pu());

        // Write ports
        ports.accumulate(&PortContribution::Electrical {
            active_power_w: effective_fan_power_w,
            reactive_power_kvar,
        })?;

        // Ventilation thermal load is handled by the envelope solver's
        // apply_infiltration_and_ventilation() -- do NOT add it here to avoid
        // double-counting. Equipment reports fan power and telemetry only.

        // Telemetry
        self.telemetry.set(tk::ELECTRIC_KW, fan_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::FAN_POWER_W, effective_fan_power_w);
        self.telemetry
            .set(tk::VENT_SUPPLY_FAN_POWER_W, effective_supply_fan_power_w);
        self.telemetry
            .set(tk::VENT_EXHAUST_FAN_POWER_W, effective_exhaust_fan_power_w);
        self.telemetry
            .set(tk::SENSIBLE_RECOVERY_W, q_recovery_sensible_w);
        self.telemetry
            .set(tk::LATENT_RECOVERY_W, q_recovery_latent_w);
        self.telemetry.set(tk::SUPPLY_TEMP_C, t_supply_c);
        self.telemetry
            .set(tk::BYPASS_ACTIVE, if bypass_active { 1.0 } else { 0.0 });

        self.telemetry
            .set(tk::VENT_DEFROST_FRACTION, defrost_fraction);

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                mode = ?self.mode,
                dr_level = ?self.dr_level,
                t_outdoor_c,
                t_indoor_c,
                defrost_fraction,
                eff_s,
                eff_l,
                bypass_active,
                "Ventilation running: effective effectiveness and mode"
            );
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if !(0.0..=1.0).contains(&defrost_fraction) {
                return Err(HaresError::InvariantViolation {
                    check_name: "ventilation_defrost_fraction_range".to_string(),
                    value: defrost_fraction,
                    tolerance: 1e-12,
                });
            }
            if self.ventilation_type != VentilationType::ExhaustFan {
                let rated = self.sensible_effectiveness;
                if !(0.0..=rated).contains(&self.effective_sensible_effectiveness) {
                    return Err(HaresError::InvariantViolation {
                        check_name: "ventilation_effective_sensible_effectiveness_range"
                            .to_string(),
                        value: self.effective_sensible_effectiveness,
                        tolerance: 1e-12,
                    });
                }
            }
        }

        self.mode = self.mode.resolve_idle(true, None);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(fan_kw.max(0.0))),
                reactive_power_kvar: Some(reactive_power_kvar),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn effective_ventilation_effectiveness(&self) -> Option<(f64, f64)> {
        Some((
            self.effective_sensible_effectiveness,
            self.effective_latent_effectiveness,
        ))
    }

    fn resolved_zip(&self) -> Option<ResolvedZip> {
        Some(self.zip)
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &VentilationCheckpoint {
                mode: self.mode,
                dr_level: self.dr_level,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
                schedule_source_state: capture_schedule_source_state(&self.schedule_source),
            },
            Self::checkpoint_version(),
            "Ventilation",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: VentilationCheckpoint = load_versioned(
            state,
            Self::checkpoint_version(),
            "Ventilation",
            self.descriptor().id,
        )?;
        self.mode = cp.mode;
        self.dr_level = cp.dr_level;
        self.dr_duration_remaining_s = cp.dr_duration_remaining_s;
        restore_schedule_source_state(&mut self.schedule_source, &cp.schedule_source_state)?;
        // Reconstruct core_output structure: operating_mode is checkpointed,
        // and the ventilation always contributes to the electric port (even
        // when off). The per-step fan power depends on live environmental
        // conditions (outdoor/indoor temp, humidity) not stored in the
        // checkpoint, so electric_kw is set to Consumption(0.0) — the next
        // step() recalculates the correct value. See Known Limitations.
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(0.0)),
                reactive_power_kvar: Some(0.0),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ModeOverride { mode } => {
                self.mode = *mode;
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.dr_level = *level;
                self.dr_duration_remaining_s = *duration_s;
            }
            ControlSignal::LoadFraction { fraction } => {
                if *fraction <= 0.0 {
                    self.mode = OperatingMode::Off;
                } else {
                    self.mode = OperatingMode::On;
                }
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "Ventilation does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Ventilation Fan",
        Box::new(|config| Box::new(Ventilation::new(config))),
    );
    registry.register("HRV", Box::new(|config| Box::new(Ventilation::new(config))));
    registry.register("ERV", Box::new(|config| Box::new(Ventilation::new(config))));
}

fn default_telemetry() -> Telemetry {
    let mut t = Telemetry::with_capacity(10);
    t.insert(tk::ELECTRIC_KW, 0.0);
    t.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    t.insert(tk::FAN_POWER_W, 0.0);
    t.insert(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
    t.insert(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
    t.insert(tk::SENSIBLE_RECOVERY_W, 0.0);
    t.insert(tk::LATENT_RECOVERY_W, 0.0);
    t.insert(tk::SUPPLY_TEMP_C, 20.0);
    t.insert(tk::BYPASS_ACTIVE, 0.0);
    t.insert(tk::VENT_DEFROST_FRACTION, 0.0);
    t
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total electrical power draw".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging)".to_string(),
        },
        TelemetryField {
            name: tk::FAN_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Total fan electrical power consumption".to_string(),
        },
        TelemetryField {
            name: tk::VENT_SUPPLY_FAN_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Supply-side fan electrical power".to_string(),
        },
        TelemetryField {
            name: tk::VENT_EXHAUST_FAN_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Exhaust-side fan electrical power".to_string(),
        },
        TelemetryField {
            name: tk::SENSIBLE_RECOVERY_W.to_string(),
            unit: "W".to_string(),
            description: "Sensible heat recovered by HRV/ERV".to_string(),
        },
        TelemetryField {
            name: tk::LATENT_RECOVERY_W.to_string(),
            unit: "W".to_string(),
            description: "Latent heat recovered by ERV".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Supply air temperature after recovery".to_string(),
        },
        TelemetryField {
            name: tk::BYPASS_ACTIVE.to_string(),
            unit: "-".to_string(),
            description: "Bypass mode active (1 = bypassing recovery)".to_string(),
        },
        TelemetryField {
            name: tk::VENT_DEFROST_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Continuous defrost fraction for HRV/ERV recovery derating".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigPayload;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneMap, ZoneRole,
        ZoneState,
    };

    fn env(outdoor_c: f64, indoor_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: indoor_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
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
                .expect("UTC")
                .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::TimeDelta::minutes(5),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn hrv_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "HRV".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: Some(50.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap()
    }

    fn erv_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "ERV".to_string(),
            "ERV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: Some(60.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.50),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("erv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn parse_ventilation_type_rejects_unrecognised_text_value() {
        let err = parse_ventilation_type(Some("hrv_plus")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("hrv_plus"),
            "error must name the unrecognised value, got: {msg}"
        );
        assert!(
            msg.contains("unrecognised"),
            "error must indicate the value was unrecognised, got: {msg}"
        );
    }

    #[test]
    fn parse_ventilation_type_accepts_known_string_aliases() {
        for text in [
            "exhaust_fan",
            "exhaust fan",
            "exhaust only",
            "supply only",
            "whole house fan",
        ] {
            assert_eq!(
                parse_ventilation_type(Some(text)).unwrap(),
                VentilationType::ExhaustFan,
                "text '{text}' must map to ExhaustFan"
            );
        }
        for text in ["hrv", "heat recovery ventilator"] {
            assert_eq!(
                parse_ventilation_type(Some(text)).unwrap(),
                VentilationType::Hrv,
                "text '{text}' must map to Hrv"
            );
        }
        for text in ["erv", "energy recovery ventilator"] {
            assert_eq!(
                parse_ventilation_type(Some(text)).unwrap(),
                VentilationType::Erv,
                "text '{text}' must map to Erv"
            );
        }
    }

    #[test]
    fn parse_ventilation_type_none_defaults_to_hrv() {
        assert_eq!(
            parse_ventilation_type(None).unwrap(),
            VentilationType::Hrv,
            "absent ventilation_type must default to Hrv"
        );
    }

    #[test]
    fn hrv_supply_temp_with_70pct_effectiveness_no_defrost() {
        // Use 0°C outdoor (above defrost threshold of -5°C) so full 70% effectiveness applies.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        // T_supply = 0 + 0.70 * (20 - 0) = 14°C
        let t_supply = hrv
            .telemetry()
            .get(tk::SUPPLY_TEMP_C)
            .expect("supply_temp_c");
        assert!(
            (t_supply - 14.0).abs() < 0.5,
            "HRV supply at 0°C outdoor / 20°C indoor / 70% eff should be ~14°C, got {t_supply}"
        );
    }

    /// Grid outage (de-energized bus): the fans have no supply — no airflow,
    /// no recovery, no draw. Islanded homes keep ventilating; operation
    /// resumes on restoration.
    #[test]
    fn grid_outage_stops_ventilation_and_islanded_home_keeps_running() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        // Baseline: fan draws power.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");
        assert!(ports.electrical.load_power_w > 0.0);

        // Utility outage: zero draw, zero recovery.
        let mut e_outage = env(0.0, 20.0);
        e_outage.grid.voltage_pu = 0.0;
        ports.zero();
        hrv.step(&e_outage, Duration::from_secs(300), &mut ports)
            .expect("step");
        assert_eq!(ports.electrical.load_power_w, 0.0);
        assert_eq!(hrv.telemetry().get(tk::FAN_POWER_W), Some(0.0));
        assert_eq!(hrv.telemetry().get(tk::SENSIBLE_RECOVERY_W), Some(0.0));

        // Islanded: bus energized by a backup source → fans run.
        let mut e_islanded = env(0.0, 20.0);
        e_islanded.grid.voltage_pu = 0.0;
        e_islanded.grid.island_bus_voltage_pu = Some(1.0);
        ports.zero();
        hrv.step(&e_islanded, Duration::from_secs(300), &mut ports)
            .expect("step");
        assert!(ports.electrical.load_power_w > 0.0);

        // Restoration: operation resumes.
        ports.zero();
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");
        assert!(ports.electrical.load_power_w > 0.0);
    }

    #[test]
    fn hrv_supply_temp_at_minus_20c_with_defrost_derating() {
        // At -25°C (20 K below threshold), defrost_fraction = 0.0 + 0.05 × 20 = 1.0.
        // Effectiveness = 0.70 × (1 − 1.0) = 0.0 → no recovery, T_supply = T_outdoor.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-25.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let t_supply = hrv
            .telemetry()
            .get(tk::SUPPLY_TEMP_C)
            .expect("supply_temp_c");
        assert!(
            (t_supply - (-25.0)).abs() < 0.5,
            "at -25°C with full defrost, T_supply should be ~-25°C (no recovery), got {t_supply}"
        );
        assert!(
            (hrv.telemetry()
                .get(tk::VENT_DEFROST_FRACTION)
                .unwrap_or(0.0)
                - 1.0)
                .abs()
                < 0.01,
            "defrost fraction should be 1.0 at 20 K below threshold"
        );
    }

    #[test]
    fn hrv_reduces_ventilation_heating_load() {
        // Use 0°C outdoor (above defrost threshold) so full 70% effectiveness.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        // Thermal port is no longer written by ventilation equipment;
        // the envelope solver handles ventilation heat exchange. Verify via telemetry instead.
        let supply_temp = hrv
            .telemetry()
            .get(tk::SUPPLY_TEMP_C)
            .expect("supply_temp_c");
        assert!(
            supply_temp < 20.0,
            "supply air should be cooler than indoor (outdoor is 0°C), got {supply_temp}"
        );

        let recovery = hrv
            .telemetry()
            .get(tk::SENSIBLE_RECOVERY_W)
            .expect("recovery");
        assert!(recovery > 0.0, "HRV should recover positive sensible heat");

        // Density computed at default pressure (101.325 kPa = SEA_LEVEL_PRESSURE_PA)
        // and 0 °C outdoor, consistent with the env() used in step().
        let rho = dry_air_density_kg_m3(SEA_LEVEL_PRESSURE_PA, 0.0);
        let m_dot = 0.035 * rho;
        let raw_load = m_dot * CP_DRY_AIR_J_KG_K * 20.0;
        let reduction = recovery / raw_load;
        assert!(
            reduction > 0.6 && reduction < 0.8,
            "HRV should reduce heating load by 60-80%, got {:.0}%",
            reduction * 100.0
        );
    }

    #[test]
    fn erv_also_reduces_latent_load() {
        let cfg = erv_config();
        let mut erv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0); // above defrost threshold
        erv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        erv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let latent_recovery = erv.telemetry().get(tk::LATENT_RECOVERY_W).expect("latent");
        assert!(
            latent_recovery > 0.0,
            "ERV should recover positive latent heat, got {latent_recovery}"
        );
    }

    #[test]
    fn fan_power_appears_in_electrical_port() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_w = ports.electrical.net_active_w();
        assert!(
            (electric_w - 50.0).abs() < 1.0,
            "fan power should be 50 W, got {electric_w}"
        );
    }

    #[test]
    fn bypass_activates_in_comfort_range() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(21.0, 22.0); // outdoor within [18, 24] comfort range
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let bypass = hrv.telemetry().get(tk::BYPASS_ACTIVE).expect("bypass");
        assert!(
            (bypass - 1.0).abs() < 0.01,
            "bypass should be active when outdoor is in comfort range"
        );
        let recovery = hrv
            .telemetry()
            .get(tk::SENSIBLE_RECOVERY_W)
            .expect("recovery");
        assert!(
            recovery.abs() < 0.1,
            "no recovery expected during bypass, got {recovery}"
        );
    }

    #[test]
    fn defrost_reduces_effectiveness_at_low_temps() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let cold_env = env(-20.0, 20.0); // below defrost threshold (-5°C)
        let mild_env = env(0.0, 20.0); // above defrost threshold
        hrv.init(&cfg, &cold_env).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&cold_env, Duration::from_secs(300), &mut ports)
            .expect("step cold");
        let eff_cold = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after cold step")
            .0;

        hrv.step(&mild_env, Duration::from_secs(300), &mut ports)
            .expect("step mild");
        let eff_mild = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after mild step")
            .0;

        assert!(
            eff_cold < eff_mild,
            "defrost should reduce effectiveness: cold={eff_cold}, mild={eff_mild}"
        );
    }

    #[test]
    fn exhaust_fan_has_no_recovery() {
        let cfg = EquipmentConfig::from_typed(
            "Exhaust".to_string(),
            "Ventilation Fan".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.025,
                fan_power_w: Some(30.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.0),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let mut fan = Ventilation::new(cfg.clone());
        let e = env(-10.0, 20.0);
        fan.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        fan.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let recovery = fan
            .telemetry()
            .get(tk::SENSIBLE_RECOVERY_W)
            .expect("recovery");
        assert!(
            recovery.abs() < 0.01,
            "exhaust fan should have zero recovery, got {recovery}"
        );
    }

    #[test]
    fn registry_includes_ventilation_types() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("HRV").is_some());
        assert!(registry.get("ERV").is_some());
        assert!(registry.get("Ventilation Fan").is_some());
    }

    #[test]
    fn constant_half_schedule_halves_flow_and_power() {
        let e = env(0.0, 20.0);

        // Half-schedule instance: hours_in_operation = 12 → schedule_frac = 0.5.
        let half_config = VentilationConfig {
            hours_in_operation: Some(12.0),
            sensible_effectiveness: Some(0.70),
            latent_effectiveness: Some(0.0),
            fan_power_w: Some(50.0),
            ventilation_type: Some("hrv".to_string()),
            balanced: None,
            ..minimal_ventilation_config()
        };
        let cfg_half =
            EquipmentConfig::from_typed("HRV-half".to_string(), "HRV".to_string(), half_config)
                .unwrap();
        let mut hrv = Ventilation::new(cfg_half.clone());
        hrv.init(&cfg_half, &e).expect("init");

        // Full-schedule reference: hours_in_operation = None → schedule_frac = 1.0.
        let cfg_full = hrv_config();
        let mut hrv_full = Ventilation::new(cfg_full.clone());
        hrv_full.init(&cfg_full, &e).expect("init full");

        let mut ports_half = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports_half)
            .expect("step half");

        let mut ports_full = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv_full
            .step(&e, Duration::from_secs(300), &mut ports_full)
            .expect("step full");

        let power_half = hrv
            .telemetry()
            .get(tk::FAN_POWER_W)
            .expect("fan_power_w half");
        let power_full = hrv_full
            .telemetry()
            .get(tk::FAN_POWER_W)
            .expect("fan_power_w full");
        assert!(
            (power_half - power_full * 0.5).abs() < 0.01,
            "half-schedule should halve fan power: got {power_half}, expected {}",
            power_full * 0.5
        );

        // Thermal port no longer written; verify recovery telemetry scales instead
        let recovery_half = hrv
            .telemetry()
            .get(tk::SENSIBLE_RECOVERY_W)
            .expect("recovery half");
        let recovery_full = hrv_full
            .telemetry()
            .get(tk::SENSIBLE_RECOVERY_W)
            .expect("recovery full");
        assert!(
            (recovery_half - recovery_full * 0.5).abs() < 0.1,
            "half-schedule should halve sensible recovery: got {recovery_half}, expected {}",
            recovery_full * 0.5
        );

        // Electrical port should also be halved
        let elec_half = ports_half.electrical.net_active_w();
        let elec_full = ports_full.electrical.net_active_w();
        assert!(
            (elec_half - elec_full * 0.5).abs() < 0.001,
            "half-schedule should halve electrical draw: got {elec_half}, expected {}",
            elec_full * 0.5
        );
    }

    // VentilationConfig typed round-trip and validation tests

    fn minimal_ventilation_config() -> VentilationConfig {
        VentilationConfig {
            equipment_id: None,
            zone_id: None,
            flow_rate_m3_s: 0.035,
            fan_power_w: None,
            supply_fan_power_w: None,
            exhaust_fan_power_w: None,
            sensible_effectiveness: None,
            latent_effectiveness: None,
            bypass_temp_min_c: None,
            bypass_temp_max_c: None,
            defrost_temp_c: None,
            defrost_initial_time_fraction: None,
            defrost_time_increase_rate_per_k: None,
            ventilation_type: None,
            balanced: None,
            hours_in_operation: None,
        }
    }

    #[test]
    fn ventilation_config_round_trips_via_equipment_config() {
        let cfg = minimal_ventilation_config();
        let ec = EquipmentConfig::from_typed(
            "test_vent".to_string(),
            "Ventilation Fan".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: VentilationConfig = ec.typed().unwrap();
        assert_eq!(recovered.flow_rate_m3_s, cfg.flow_rate_m3_s);
    }

    #[test]
    fn ventilation_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "flow_rate_m3_s": 0.035,
            "mystery_key": 99
        });
        let ec = EquipmentConfig::with_payload(
            "vent".to_string(),
            "Ventilation Fan".to_string(),
            ConfigPayload::Typed {
                type_name: "Ventilation".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: crate::Result<VentilationConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn ventilation_config_validate_rejects_negative_flow_rate() {
        let mut cfg = minimal_ventilation_config();
        cfg.flow_rate_m3_s = -0.01;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn ventilation_config_validate_rejects_out_of_range_effectiveness() {
        let mut cfg = minimal_ventilation_config();
        cfg.sensible_effectiveness = Some(1.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn ventilation_config_validate_passes_for_valid_config() {
        let cfg = minimal_ventilation_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn ventilation_config_validate_allows_defrost_rate_above_one() {
        let mut cfg = minimal_ventilation_config();
        cfg.defrost_time_increase_rate_per_k = Some(2.0);
        assert!(
            cfg.validate().is_ok(),
            "defrost_time_increase_rate_per_k is a rate (1/K), not a fraction; \
             values > 1.0 must be accepted"
        );
    }

    #[test]
    fn ventilation_config_validate_rejects_negative_defrost_rate() {
        let mut cfg = minimal_ventilation_config();
        cfg.defrost_time_increase_rate_per_k = Some(-0.1);
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("defrost_time_increase_rate_per_k"),
            "negative defrost_time_increase_rate_per_k must fail validation; got: {err}"
        );
    }

    #[test]
    fn typed_init_uses_hours_in_operation_and_ventilation_type() {
        let cfg = VentilationConfig {
            hours_in_operation: Some(8.0),
            ventilation_type: Some("erv".to_string()),
            fan_power_w: Some(40.0),
            supply_fan_power_w: None,
            exhaust_fan_power_w: None,
            sensible_effectiveness: Some(0.75),
            latent_effectiveness: Some(0.10),
            balanced: Some(true),
            ..minimal_ventilation_config()
        };
        let ec = EquipmentConfig::from_typed(
            "typed_vent".to_string(),
            "Ventilation Fan".to_string(),
            cfg,
        )
        .unwrap();
        let env = env(0.0, 20.0);
        let mut fan = Ventilation::new(ec.clone());
        fan.init(&ec, &env).expect("typed init");

        assert_eq!(fan.ventilation_type, VentilationType::Erv);
        assert!((fan.flow_rate_m3_s - 0.035).abs() < 1e-9);
        assert!(
            (fan.supply_fan_power_w + fan.exhaust_fan_power_w - 40.0).abs() < 1e-9,
            "total fan power from config fan_power_w=40 should be 40W for balanced ERV"
        );
        assert_eq!(fan.sensible_effectiveness, 0.75);
        assert_eq!(fan.latent_effectiveness, 0.10);
        assert!((fan.schedule_source.value_at(&env).unwrap() - (8.0 / 24.0)).abs() < 1e-9);
    }

    #[test]
    fn exhaust_fan_no_bypass_at_mild_outdoor_temp() {
        let cfg = EquipmentConfig::from_typed(
            "Exhaust".to_string(),
            "Ventilation Fan".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.025,
                fan_power_w: Some(30.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.0),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let mut fan = Ventilation::new(cfg.clone());
        // 21°C is within the default bypass range [18, 24]°C
        let e = env(21.0, 22.0);
        fan.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        fan.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let bypass = fan
            .telemetry()
            .get(tk::BYPASS_ACTIVE)
            .expect("bypass_active");
        assert!(
            (bypass - 0.0).abs() < 1e-9,
            "exhaust fan should never report bypass active, got {bypass}"
        );
    }

    /// Verifies that after `step()` the stored effective effectiveness fields
    /// match the expected per-timestep values — zero during bypass, derated
    /// during defrost, and rated otherwise.
    #[test]
    fn effective_effectiveness_stored_after_step() {
        // Mild outdoor temp (above defrost, outside bypass range): rated effectiveness.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e_mild = env(10.0, 20.0); // below bypass min (18°C), above defrost (-5°C)
        hrv.init(&cfg, &e_mild).expect("init");
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e_mild, Duration::from_secs(300), &mut ports)
            .expect("step");
        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            (eff_s - 0.70).abs() < 0.01,
            "mild weather: rated sensible eff should be 0.70, got {eff_s}"
        );
        assert!(
            (eff_l - 0.0).abs() < 0.01,
            "HRV: latent eff should be 0.0, got {eff_l}"
        );

        // Bypass range: effectiveness must be zero.
        let e_bypass = env(21.0, 22.0);
        hrv.step(&e_bypass, Duration::from_secs(300), &mut ports)
            .expect("step bypass");
        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            (eff_s - 0.0).abs() < 0.01,
            "bypass: sensible eff should be 0.0, got {eff_s}"
        );
        assert!(
            (eff_l - 0.0).abs() < 0.01,
            "bypass: latent eff should be 0.0, got {eff_l}"
        );

        // Defrost range: at -25°C (20 K below threshold) defrost_fraction = 1.0,
        // so effectiveness = 0.70 × (1.0 − 1.0) = 0.0.
        let e_cold = env(-25.0, 20.0);
        hrv.step(&e_cold, Duration::from_secs(300), &mut ports)
            .expect("step defrost");
        let (eff_s, _eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            (eff_s - 0.0).abs() < 0.01,
            "deep defrost: sensible eff should be ~0.0 (full defrost), got {eff_s}"
        );
    }

    /// Ventilation equipment returns Some(eff_s, eff_l) via the trait method.
    #[test]
    fn ventilation_effectiveness_returns_some_for_hrv() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(10.0, 20.0);
        hrv.init(&cfg, &e).expect("init");
        let result = hrv.effective_ventilation_effectiveness();
        assert!(result.is_some(), "Ventilation must return Some");
        let (eff_s, _eff_l) = result.unwrap();
        assert!(
            (0.0..=1.0).contains(&eff_s),
            "sensible effectiveness in range"
        );
    }

    #[test]
    fn validate_rejects_non_finite_supply_fan_power() {
        let mut cfg = minimal_ventilation_config();
        cfg.supply_fan_power_w = Some(f64::NAN);
        assert!(cfg.validate().is_err());
        cfg.supply_fan_power_w = Some(f64::NEG_INFINITY);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_negative_supply_fan_power() {
        let mut cfg = minimal_ventilation_config();
        cfg.supply_fan_power_w = Some(-1.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_negative_exhaust_fan_power() {
        let mut cfg = minimal_ventilation_config();
        cfg.exhaust_fan_power_w = Some(-5.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn separate_supply_and_exhaust_fan_power_sum_to_total() {
        let cfg = EquipmentConfig::from_typed(
            "HRV-separate".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: None,
                supply_fan_power_w: Some(30.0),
                exhaust_fan_power_w: Some(25.0),
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let e = env(5.0, 20.0);
        let mut hrv = Ventilation::new(cfg.clone());
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_w = ports.electrical.net_active_w();
        let expected_w = 30.0 + 25.0;
        assert!(
            (electric_w - expected_w).abs() < 1.0,
            "total fan power should be {expected_w} W, got {electric_w}"
        );

        let supply_w = hrv
            .telemetry()
            .get(tk::VENT_SUPPLY_FAN_POWER_W)
            .expect("supply_fan_power_w");
        let exhaust_w = hrv
            .telemetry()
            .get(tk::VENT_EXHAUST_FAN_POWER_W)
            .expect("exhaust_fan_power_w");
        assert!(
            (supply_w - 30.0).abs() < 0.01,
            "supply fan power should be 30W, got {supply_w}"
        );
        assert!(
            (exhaust_w - 25.0).abs() < 0.01,
            "exhaust fan power should be 25W, got {exhaust_w}"
        );
    }

    #[test]
    fn fan_power_w_alone_retains_backward_compat_for_balanced_hrv() {
        let cfg = hrv_config(); // fan_power_w = 50, balanced HRV
        let e = env(5.0, 20.0);
        let mut hrv = Ventilation::new(cfg.clone());
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_w = ports.electrical.net_active_w();
        assert!(
            (electric_w - 50.0).abs() < 1.0,
            "total fan power should still be 50 W with fan_power_w alone, got {electric_w}"
        );

        let total_telemetry = hrv
            .telemetry()
            .get(tk::FAN_POWER_W)
            .expect("fan_power_w telemetry");
        assert!(
            (total_telemetry - 50.0).abs() < 0.01,
            "total fan power telemetry should be 50W, got {total_telemetry}"
        );
    }

    #[test]
    fn fan_power_w_alone_works_for_exhaust_only_system() {
        let cfg = EquipmentConfig::from_typed(
            "Exhaust".to_string(),
            "Ventilation Fan".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.025,
                fan_power_w: Some(60.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.0),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let e = env(5.0, 20.0);
        let mut fan = Ventilation::new(cfg.clone());
        fan.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        fan.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_w = ports.electrical.net_active_w();
        assert!(
            (electric_w - 60.0).abs() < 1.0,
            "exhaust fan power should be 60 W, got {electric_w}"
        );

        let supply_w = fan
            .telemetry()
            .get(tk::VENT_SUPPLY_FAN_POWER_W)
            .expect("supply_fan_power_w");
        assert!(
            supply_w.abs() < 0.01,
            "exhaust-only system should have zero supply fan power, got {supply_w}"
        );
    }

    #[test]
    fn supply_fan_power_scaled_during_bypass() {
        let cfg = EquipmentConfig::from_typed(
            "HRV-bypass".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: None,
                supply_fan_power_w: Some(40.0),
                exhaust_fan_power_w: Some(40.0),
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: Some(18.0),
                bypass_temp_max_c: Some(24.0),
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let e_bypass = env(21.0, 22.0); // within bypass comfort range
        let e_normal = env(10.0, 20.0); // outside bypass range

        let mut hrv = Ventilation::new(cfg.clone());
        hrv.init(&cfg, &e_normal).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // Normal operation: supply + exhaust both at rated
        hrv.step(&e_normal, Duration::from_secs(300), &mut ports)
            .expect("step normal");
        let normal_total_kw = ports.electrical.net_active_w();

        // Bypass: supply fan power scaled down
        let mut ports_bypass = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e_bypass, Duration::from_secs(300), &mut ports_bypass)
            .expect("step bypass");
        let bypass_total_kw = ports_bypass.electrical.net_active_w();

        assert!(
            bypass_total_kw < normal_total_kw,
            "bypass total power ({bypass_total_kw:.6} kW) should be less than normal ({normal_total_kw:.6} kW)"
        );

        let bypass_supply_w = hrv
            .telemetry()
            .get(tk::VENT_SUPPLY_FAN_POWER_W)
            .expect("supply_fan_power_w during bypass");
        let expected_supply_bypass = 40.0 * BYPASS_SUPPLY_FAN_POWER_FRACTION;
        assert!(
            (bypass_supply_w - expected_supply_bypass).abs() < 0.5,
            "supply fan power during bypass should be ~{expected_supply_bypass}W, got {bypass_supply_w}"
        );
    }

    #[test]
    fn regression_split_vs_combined_total_matches() {
        // Original model: fan_power_w = 80 (combined)
        let cfg_combined = EquipmentConfig::from_typed(
            "HRV-combined".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: Some(80.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();
        let cfg_split = EquipmentConfig::from_typed(
            "HRV-split".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: None,
                supply_fan_power_w: Some(40.0),
                exhaust_fan_power_w: Some(40.0),
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
        .unwrap();

        let e = env(10.0, 20.0); // normal operation, no bypass/defrost
        let mut combined = Ventilation::new(cfg_combined.clone());
        combined.init(&cfg_combined, &e).expect("init combined");
        let mut split = Ventilation::new(cfg_split.clone());
        split.init(&cfg_split, &e).expect("init split");

        let mut ports_combined = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let mut ports_split = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        combined
            .step(&e, Duration::from_secs(300), &mut ports_combined)
            .expect("step combined");
        split
            .step(&e, Duration::from_secs(300), &mut ports_split)
            .expect("step split");

        let combined_kw = ports_combined.electrical.net_active_w();
        let split_kw = ports_split.electrical.net_active_w();
        assert!(
            (combined_kw - split_kw).abs() < 0.001,
            "combined model ({combined_kw:.6} kW) and split model ({split_kw:.6} kW) should match when supply+exhaust sum equals old total"
        );
    }

    #[test]
    fn ventilation_resolves_zone_via_zone_map_with_non_standard_numbering() {
        // Regression: Ventilation should use ZoneMap to resolve the indoor
        // conditioned zone rather than hardcoding ZoneId(1). This test uses
        // ZoneId(5) for the conditioned zone to prove the mapping works
        // independent of zone sort order.
        let mut cfg = hrv_config();
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(5));
        cfg.zone_map = Some(zone_map);

        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        assert_eq!(
            hrv.descriptor().zone,
            Some(ZoneId(5)),
            "ventilation zone should be resolved from ZoneMap Indoor role (ZoneId(5))"
        );
        let has_thermal_port_for_zone_5 = hrv.ports().iter().any(|p| p.zone == Some(ZoneId(5)));
        assert!(
            has_thermal_port_for_zone_5,
            "ventilation ports should include thermal port for resolved ZoneId(5)"
        );
    }

    #[test]
    fn reactive_power_blended_pf_and_channels_agree() {
        let cfg = hrv_config();
        let mut eq = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        eq.init(&cfg, &e).expect("init");
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );
        let pf = 0.87_f64;
        assert_eq!(eq.zip.pf, pf);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let p_kw = ports.electrical.net_active_w() / 1000.0;
        let q = ports.electrical.reactive_power_kvar;
        let expected = p_kw * pf.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "Q/P must equal tan(acos({pf})): {q} vs {expected}"
        );

        let co_q = eq.core_output().flows.reactive_power_kvar.expect("Some");
        assert_eq!(
            co_q.to_bits(),
            q.to_bits(),
            "CoreOutput Q must match port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .unwrap()
                .to_bits(),
            q.to_bits(),
            "telemetry Q must match port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn reactive_power_zero_when_off() {
        let cfg = hrv_config();
        let mut eq = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        eq.init(&cfg, &e).expect("init");
        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .expect("mode override off");

        let mut ports = PortSlots::default();
        eq.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "off equipment must report Some(0.0)"
        );
    }

    #[test]
    fn real_power_bit_identical_with_and_without_reactive_zip() {
        let cfg_pf = hrv_config();
        let mut cfg_nopf = hrv_config();
        cfg_nopf.zip = Some(ZipLoad::constant_power());

        let e = env(5.0, 20.0);
        let mut eq_pf = Ventilation::new(cfg_pf.clone());
        let mut eq_nopf = Ventilation::new(cfg_nopf.clone());
        eq_pf.init(&cfg_pf, &e).expect("init pf");
        eq_nopf.init(&cfg_nopf, &e).expect("init no-pf");

        assert!(
            eq_pf
                .descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            let mut env_v = env(5.0, 20.0);
            env_v.grid.voltage_pu = *v;
            let mut ports_pf = PortSlots::default();
            let mut ports_nopf = PortSlots::default();
            eq_pf
                .step(&env_v, Duration::from_secs(300), &mut ports_pf)
                .expect("step pf");
            eq_nopf
                .step(&env_v, Duration::from_secs(300), &mut ports_nopf)
                .expect("step no-pf");
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged between pf and no-pf twins"
            );
            if ports_pf.electrical.reactive_power_kvar != 0.0 {
                any_reactive = true;
            }
            assert_eq!(
                ports_nopf.electrical.reactive_power_kvar, 0.0,
                "no-pf twin must produce zero Q"
            );
        }
        assert!(any_reactive, "the pf 0.87 twin must produce reactive power");
    }

    #[test]
    fn ventilation_and_scheduled_load_fan_same_reactive_power() {
        let e = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 20.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 5.0,
                outdoor_humidity_ratio: 0.003,
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
                .expect("UTC")
                .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::TimeDelta::minutes(5),
            price_signal: Default::default(),
            electrical: Default::default(),
        };

        let cfg = hrv_config();
        let mut vent = Ventilation::new(cfg.clone());
        vent.init(&cfg, &e).expect("init vent");

        let mut ports_vent = PortSlots::default();
        vent.step(&e, Duration::from_secs(300), &mut ports_vent)
            .expect("step vent");

        let p_vent = ports_vent.electrical.net_active_w() / 1000.0;
        let q_vent = ports_vent.electrical.reactive_power_kvar;

        let pf = 0.87_f64;
        let expected_q = p_vent * pf.acos().tan();
        assert!(
            (q_vent - expected_q).abs() < 1e-9,
            "ventilation Q/P must equal tan(acos({pf}))"
        );
    }

    #[test]
    fn mode_on_with_grid_outage_reconciles_to_standby() {
        let cfg = hrv_config();
        let mut v = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        v.init(&cfg, &e).expect("init");

        v.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::On,
        })
        .unwrap();

        let mut e_outage = env(0.0, 20.0);
        e_outage.grid.voltage_pu = 0.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        v.step(&e_outage, Duration::from_secs(300), &mut ports)
            .expect("step");

        let co = v.core_output();
        assert_eq!(
            co.state.operating_mode,
            Some(OperatingMode::Standby),
            "active On mode with zero flow (grid outage) must reconcile to Standby"
        );
        hares_types::validate_core_contract(v.descriptor(), v.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn mode_on_with_zero_schedule_reconciles_to_standby() {
        let cfg = EquipmentConfig::from_typed(
            "HRV-zero-sched".to_string(),
            "HRV".to_string(),
            VentilationConfig {
                equipment_id: None,
                zone_id: Some(1),
                flow_rate_m3_s: 0.035,
                fan_power_w: Some(50.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.70),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: Some(0.0),
            },
        )
        .unwrap();

        let mut v = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        v.init(&cfg, &e).expect("init");

        v.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::On,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        v.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let co = v.core_output();
        assert_eq!(
            co.state.operating_mode,
            Some(OperatingMode::Standby),
            "active On mode with zero schedule fraction must reconcile to Standby"
        );
        hares_types::validate_core_contract(v.descriptor(), v.core_output())
            .expect("validate_core_contract");
    }

    // ── Continuous defrost fraction tests ───────────────────────────────────

    /// At the threshold temperature (−5°C default), defrost_fraction = 0.0,
    /// so effectiveness equals rated (0.70). Contrasts with the old binary
    /// model which would have applied 0.5× at any temperature below threshold.
    #[test]
    fn defrost_returns_rated_effectiveness_at_threshold() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-5.0, 20.0); // exactly at default threshold
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, _) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            (eff_s - 0.70).abs() < 0.01,
            "at threshold temperature, effectiveness should be rated 0.70, got {eff_s}"
        );
        let defrost_frac = hrv
            .telemetry()
            .get(tk::VENT_DEFROST_FRACTION)
            .expect("defrost_fraction");
        assert!(
            defrost_frac.abs() < 0.01,
            "defrost_fraction should be 0.0 at threshold, got {defrost_frac}"
        );
    }

    /// At 20 K below threshold (−25°C), defrost_fraction = 0.0 + 0.05 × 20 = 1.0.
    /// Effectiveness should be zero (no recovery at all during continuous defrost).
    #[test]
    fn defrost_returns_zero_effectiveness_when_fraction_reaches_one() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-25.0, 20.0); // 20 K below -5°C default threshold
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, _) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            eff_s.abs() < 0.01,
            "at -25°C, defrost_fraction should be 1.0, effectiveness should be 0.0, got {eff_s}"
        );
        let defrost_frac = hrv
            .telemetry()
            .get(tk::VENT_DEFROST_FRACTION)
            .expect("defrost_fraction");
        assert!(
            (defrost_frac - 1.0).abs() < 0.01,
            "defrost_fraction should be 1.0 at -25°C, got {defrost_frac}"
        );
    }

    /// Intermediate temperatures between threshold and full defrost produce
    /// intermediate effectiveness values in (0, rated).
    #[test]
    fn defrost_produces_intermediate_effectiveness() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-12.5, 20.0); // 7.5 K below threshold, frac = 0.05 × 7.5 = 0.375
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, _) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        // defrost_fraction = 0.0 + 0.05 × 7.5 = 0.375
        // eff = 0.70 × (1 − 0.375) = 0.70 × 0.625 = 0.4375
        let expected = 0.70 * (1.0 - 0.375);
        assert!(
            (eff_s - expected).abs() < 0.01,
            "at -12.5°C, effectiveness should be ~{expected}, got {eff_s}"
        );
        assert!(
            eff_s > 0.0 && eff_s < 0.70,
            "effectiveness should be strictly between 0 and rated, got {eff_s}"
        );
    }

    /// Regression: at −6°C (1 K below −5°C threshold), the continuous model
    /// gives effectiveness = 0.70 × 0.95 = 0.665, not the old binary model's
    /// 0.70 × 0.50 = 0.35.
    #[test]
    fn defrost_not_binary_regression_minus_6c() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-6.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, _) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        // defrost_fraction = 0.0 + 0.05 × 1 = 0.05
        // eff = 0.70 × 0.95 = 0.665
        let expected = 0.70 * 0.95;
        assert!(
            (eff_s - expected).abs() < 0.01,
            "at -6°C, effectiveness should be ~{expected}, got {eff_s} (not 0.35 — old binary)"
        );

        let defrost_frac = hrv
            .telemetry()
            .get(tk::VENT_DEFROST_FRACTION)
            .expect("defrost_fraction");
        assert!(
            (defrost_frac - 0.05).abs() < 0.01,
            "defrost_fraction should be 0.05 at -6°C, got {defrost_frac}"
        );
    }

    /// After calling step() with mode = Off, effective_ventilation_effectiveness()
    /// returns (0.0, 0.0) — the solver must compute the full unconditioned load.
    #[test]
    fn effectiveness_is_zero_when_mode_is_off() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(0.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        hrv.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("Ventilation provides effectiveness");
        assert!(
            eff_s.abs() < 1e-12,
            "sensible effectiveness should be 0.0 when mode is Off, got {eff_s}"
        );
        assert!(
            eff_l.abs() < 1e-12,
            "latent effectiveness should be 0.0 when mode is Off, got {eff_l}"
        );
    }

    /// After running with mode = On (producing non-zero effectiveness), then
    /// switching to mode = Off, the second step returns (0.0, 0.0) — stale
    /// recovery efficiency from the previous running timestep must not persist.
    #[test]
    fn effectiveness_is_zero_after_running_then_turning_off() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        hrv.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::On,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step running");

        // Verify running step produced non-zero effectiveness
        let (eff_s_before, _) = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after running");
        assert!(
            eff_s_before > 0.0,
            "effectiveness should be non-zero while running, got {eff_s_before}"
        );

        // Now turn off and step again
        hrv.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step off");

        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after off");
        assert!(
            eff_s.abs() < 1e-12,
            "sensible effectiveness should be 0.0 after turning off, got {eff_s}"
        );
        assert!(
            eff_l.abs() < 1e-12,
            "latent effectiveness should be 0.0 after turning off, got {eff_l}"
        );
    }

    /// After setting dr_level to GridEmergency, effectiveness is (0.0, 0.0).
    #[test]
    fn effectiveness_is_zero_during_grid_emergency() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        hrv.apply_control_unchecked(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(3600.0),
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("Ventilation provides effectiveness");
        assert!(
            eff_s.abs() < 1e-12,
            "sensible effectiveness should be 0.0 during GridEmergency, got {eff_s}"
        );
        assert!(
            eff_l.abs() < 1e-12,
            "latent effectiveness should be 0.0 during GridEmergency, got {eff_l}"
        );
    }

    /// Regression: ventilation toggled Off after running for several timesteps
    /// must not propagate stale recovery efficiency to ports or telemetry.
    /// The stale-state bug caused the thermal solver to deduct recovery when
    /// no recovery was occurring, under-estimating the ventilation load.
    #[test]
    fn off_after_running_does_not_propagate_stale_effectiveness() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // Run several timesteps to establish non-zero effectiveness
        for _ in 0..3 {
            hrv.step(&e, Duration::from_secs(300), &mut ports)
                .expect("step running");
            assert!(
                hrv.effective_ventilation_effectiveness().unwrap().0 > 0.0,
                "running step should produce non-zero effectiveness"
            );
            assert!(
                hrv.telemetry().get(tk::SENSIBLE_RECOVERY_W).unwrap() > 0.0,
                "running step should report non-zero recovery in telemetry"
            );
        }

        // Toggle off
        hrv.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        ports.zero();
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step off");

        // Effectiveness must be zero after turning off
        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after off");
        assert_eq!(
            eff_s, 0.0,
            "sensible effectiveness should be 0.0 after turning off, got {eff_s}"
        );
        assert_eq!(
            eff_l, 0.0,
            "latent effectiveness should be 0.0 after turning off, got {eff_l}"
        );

        // Telemetry must not report stale recovery values
        assert_eq!(
            hrv.telemetry().get(tk::SENSIBLE_RECOVERY_W),
            Some(0.0),
            "sensible recovery telemetry should be 0.0 when off"
        );
        assert_eq!(
            hrv.telemetry().get(tk::LATENT_RECOVERY_W),
            Some(0.0),
            "latent recovery telemetry should be 0.0 when off"
        );

        // Electrical port must draw zero power
        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "electrical port should draw 0 W when off"
        );

        // Core output must report off
        let co = hrv.core_output();
        assert_eq!(
            co.flows.electric_kw,
            Some(ElectricPower::Consumption(0.0)),
            "core output must report zero consumption when off"
        );
    }

    /// Mode On with a zero-valued schedule (e.g. `hours_in_operation = 0`)
    /// must clear effective effectiveness, matching the same reset that the
    /// `Off`/`GridEmergency` early-return path applies.  Without this fix the
    /// `effective_flow_rate_m3_s <= 0.0` branch leaves stale non-zero
    /// effectiveness from a prior running step while zeroing telemetry,
    /// producing a silent mismatch between reported recovery and the values
    /// driving the thermal solver.
    #[test]
    fn on_with_zero_schedule_clears_effectiveness() {
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(5.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // Run several steps to establish non-zero effectiveness
        for _ in 0..3 {
            hrv.step(&e, Duration::from_secs(300), &mut ports)
                .expect("step running");
            assert!(
                hrv.effective_ventilation_effectiveness().unwrap().0 > 0.0,
                "running step should produce non-zero effectiveness"
            );
        }

        // Force the schedule to zero while mode stays On — this reaches the
        // `effective_flow_rate_m3_s <= 0.0` early-return path that the fix
        // targets.  schedule_source is `Constant` in the current Ventilation
        // implementation; the test uses direct mutation because the public
        // API provides no per-timestep schedule variation.
        hrv.schedule_source = ScheduleSource::Constant(0.0);
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step with zero schedule");

        let (eff_s, eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("effectiveness after zero-schedule step");
        assert_eq!(
            eff_s, 0.0,
            "sensible effectiveness should be 0.0 with zero schedule, got {eff_s}"
        );
        assert_eq!(
            eff_l, 0.0,
            "latent effectiveness should be 0.0 with zero schedule, got {eff_l}"
        );
    }
}
