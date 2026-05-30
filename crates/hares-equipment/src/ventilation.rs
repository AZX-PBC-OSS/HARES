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

use hares_physics::constants::{
    CP_DRY_AIR_J_KG_K, DRY_AIR_DENSITY_AT_20C_SEA_LEVEL_KG_M3, LATENT_HEAT_VAPORISATION_0C_J_KG,
};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, ScheduleSource, Telemetry, TelemetryField, ZoneId,
};
use serde::{Deserialize, Serialize};
use tracing::error;

use hares_types::telemetry_keys as tk;

use crate::config::EquipmentTypedConfig;
use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

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
    pub defrost_effectiveness_fraction: Option<f64>,
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
                "defrost_effectiveness_fraction",
                self.defrost_effectiveness_fraction,
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

/// Default defrost effectiveness derating fraction [—].
///
/// During defrost the sensible effectiveness is derated to 50 % of
/// rated. This corresponds to an exhaust-only defrost strategy where
/// the supply fan is cycled off for approximately half the time
/// (EnergyPlus HeatExchanger:AirToAir:SensibleAndLatent 'ExhaustOnly'
/// frost control, IDD default initial defrost time fraction 0.083
/// rising at 0.012 1/K below threshold). HARES uses a simplified
/// constant 0.5 derating as a conservative first-order approximation.
const DEFAULT_DEFROST_EFFECTIVENESS_FRACTION: f64 = 0.5;

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

    // Defrost: at low outdoor temps, reduce effectiveness.
    defrost_temp_c: f64,
    defrost_effectiveness_fraction: f64,

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
        let zone_id = config
            .get_f64(KEY_ZONE_ID)
            .map(|v| ZoneId(v as u16))
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
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
            telemetry_fields: telemetry_fields(),
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
            defrost_effectiveness_fraction: DEFAULT_DEFROST_EFFECTIVENESS_FRACTION,
            schedule_source: ScheduleSource::Constant(1.0),
            mode: OperatingMode::Off,
            dr_level: DRLevel::Normal,
            dr_duration_remaining_s: None,
        }
    }

    /// Effective sensible effectiveness after bypass and defrost adjustments.
    fn compute_effective_sensible_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type == VentilationType::ExhaustFan {
            return 0.0;
        }
        // Bypass: when outdoor is within comfort range, bypass recovery entirely.
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        // Defrost: at very low outdoor temps, reduce effectiveness.
        let base = self.sensible_effectiveness;
        if t_outdoor_c < self.defrost_temp_c {
            base * self.defrost_effectiveness_fraction
        } else {
            base
        }
    }

    /// Effective latent effectiveness (ERV only).
    fn compute_effective_latent_effectiveness(&self, t_outdoor_c: f64) -> f64 {
        if self.ventilation_type != VentilationType::Erv {
            return 0.0;
        }
        if t_outdoor_c >= self.bypass_temp_min_c && t_outdoor_c <= self.bypass_temp_max_c {
            return 0.0;
        }
        let base = self.latent_effectiveness;
        if t_outdoor_c < self.defrost_temp_c {
            base * self.defrost_effectiveness_fraction
        } else {
            base
        }
    }
}

impl Ventilation {
    fn init_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let c = config.require_typed::<VentilationConfig>("Ventilation")?;
        c.validate()?;

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
        self.defrost_effectiveness_fraction = c
            .defrost_effectiveness_fraction
            .unwrap_or(DEFAULT_DEFROST_EFFECTIVENESS_FRACTION)
            .clamp(0.0, 1.0);

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

        Ok(())
    }
}

impl Equipment for Ventilation {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
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
        let is_running = self.mode != OperatingMode::Off && self.dr_level != DRLevel::GridEmergency;

        if !is_running {
            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::SENSIBLE_RECOVERY_W, 0.0);
            self.telemetry.set(tk::LATENT_RECOVERY_W, 0.0);
            self.telemetry
                .set(tk::SUPPLY_TEMP_C, env.weather.outdoor_temp_c);
            self.telemetry.set(tk::BYPASS_ACTIVE, 0.0);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: None,
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

        let schedule_frac = self.schedule_source.value_at(env)?.clamp(0.0, 1.0);
        let effective_flow_rate_m3_s = self.flow_rate_m3_s * schedule_frac;

        if effective_flow_rate_m3_s <= 0.0 {
            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
            self.telemetry.set(tk::SENSIBLE_RECOVERY_W, 0.0);
            self.telemetry.set(tk::LATENT_RECOVERY_W, 0.0);
            self.telemetry
                .set(tk::SUPPLY_TEMP_C, env.weather.outdoor_temp_c);
            self.telemetry.set(tk::BYPASS_ACTIVE, 0.0);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: None,
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

        let defrost_active = self.ventilation_type != VentilationType::ExhaustFan
            && t_outdoor_c < self.defrost_temp_c;

        // Effective fan power per fan, scaled by schedule and operating conditions.
        let mut effective_supply_fan_power_w = self.supply_fan_power_w * schedule_frac;
        let effective_exhaust_fan_power_w = self.exhaust_fan_power_w * schedule_frac;

        if bypass_active {
            // Supply fan sees reduced pressure drop when air bypasses the HX core.
            effective_supply_fan_power_w *= BYPASS_SUPPLY_FAN_POWER_FRACTION;
        }
        if defrost_active {
            // Known approximation: supply fan power is scaled by the defrost
            // effectiveness fraction as a proxy for reduced supply-side operation.
            // The ticket (directive 3) calls for scaling by "the actual mass flow
            // fraction through each fan"; HARES does not yet model per-fan defrost
            // flow fractions. A proper defrost flow fraction requires the T-0590
            // defrost model (time-fraction-based frost control with supply/exhaust
            // modulation). Until that lands, the effectiveness fraction is used as
            // a conservative first-order proxy.
            effective_supply_fan_power_w *= self.defrost_effectiveness_fraction;
        }

        let effective_fan_power_w = effective_supply_fan_power_w + effective_exhaust_fan_power_w;

        // Store effective values so the dwelling orchestration can propagate them
        // to ThermalSolverConfig.ventilation before the thermal solver integrates.
        self.effective_sensible_effectiveness = eff_s;
        self.effective_latent_effectiveness = eff_l;

        // Supply air conditions after heat recovery
        let t_supply_c = t_outdoor_c + eff_s * (t_indoor_c - t_outdoor_c);
        let w_supply = w_outdoor + eff_l * (w_indoor - w_outdoor);

        // Mass flow rate [kg/s]
        let m_dot_kg_s = effective_flow_rate_m3_s * DRY_AIR_DENSITY_AT_20C_SEA_LEVEL_KG_M3;

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
        let fan_kw = effective_fan_power_w / 1000.0;

        // Write ports
        ports.accumulate(&PortContribution::Electrical {
            active_power_kw: fan_kw,
            reactive_power_kvar: 0.0,
        })?;

        // Ventilation thermal load is handled by the envelope solver's
        // apply_infiltration_and_ventilation() -- do NOT add it here to avoid
        // double-counting. Equipment reports fan power and telemetry only.

        // Telemetry
        self.telemetry.set(tk::ELECTRIC_KW, fan_kw);
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
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(fan_kw.max(0.0))),
                reactive_power_kvar: None,
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

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&VentilationCheckpoint {
            mode: self.mode,
            dr_level: self.dr_level,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
            schedule_source_state: capture_schedule_source_state(&self.schedule_source),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: VentilationCheckpoint = load_postcard(state)?;
        self.mode = cp.mode;
        self.dr_level = cp.dr_level;
        self.dr_duration_remaining_s = cp.dr_duration_remaining_s;
        restore_schedule_source_state(&mut self.schedule_source, &cp.schedule_source_state)?;
        self.core_output = CoreOutput::default();
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
    let mut t = Telemetry::with_capacity(8);
    t.insert(tk::ELECTRIC_KW, 0.0);
    t.insert(tk::FAN_POWER_W, 0.0);
    t.insert(tk::VENT_SUPPLY_FAN_POWER_W, 0.0);
    t.insert(tk::VENT_EXHAUST_FAN_POWER_W, 0.0);
    t.insert(tk::SENSIBLE_RECOVERY_W, 0.0);
    t.insert(tk::LATENT_RECOVERY_W, 0.0);
    t.insert(tk::SUPPLY_TEMP_C, 20.0);
    t.insert(tk::BYPASS_ACTIVE, 0.0);
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigPayload;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneState};

    fn env(outdoor_c: f64, indoor_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: indoor_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.5,
                wet_bulb_c: 14.0,
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("erv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        )
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

    #[test]
    fn hrv_supply_temp_at_minus_20c_with_defrost_derating() {
        // At -20°C (below defrost threshold -5°C), effectiveness is halved to 35%.
        let cfg = hrv_config();
        let mut hrv = Ventilation::new(cfg.clone());
        let e = env(-20.0, 20.0);
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        // T_supply = -20 + 0.35 * (20 - (-20)) = -20 + 14 = -6°C
        let t_supply = hrv
            .telemetry()
            .get(tk::SUPPLY_TEMP_C)
            .expect("supply_temp_c");
        assert!(
            (t_supply - (-6.0)).abs() < 0.5,
            "HRV at -20°C with defrost (35% eff) should give ~-6°C supply, got {t_supply}"
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

        // m_dot = 0.035 * 1.2 = 0.042 kg/s
        // Raw load = 0.042 * 1006 * 20 = 845 W
        // Recovery = 0.042 * 1006 * 0.70 * 20 = 591 W
        // Reduction = 591/845 = 70%
        let m_dot = 0.035 * DRY_AIR_DENSITY_AT_20C_SEA_LEVEL_KG_M3;
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

        let electric_kw = ports.electrical.net_active_kw();
        assert!(
            (electric_kw - 0.05).abs() < 0.001,
            "fan power should be 50W = 0.05 kW, got {electric_kw}"
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
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
            EquipmentConfig::from_typed("HRV-half".to_string(), "HRV".to_string(), half_config);
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
        let elec_half = ports_half.electrical.net_active_kw();
        let elec_full = ports_full.electrical.net_active_kw();
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
            defrost_effectiveness_fraction: None,
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
        );
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
        );
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
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

        // Defrost range: effectiveness must be derated (35% = 0.50 × 0.70 = 0.35).
        let e_cold = env(-20.0, 20.0);
        hrv.step(&e_cold, Duration::from_secs(300), &mut ports)
            .expect("step defrost");
        let (eff_s, _eff_l) = hrv
            .effective_ventilation_effectiveness()
            .expect("HRV provides effectiveness");
        assert!(
            (eff_s - 0.35).abs() < 0.01,
            "defrost: sensible eff should be ~0.35 (50% derating), got {eff_s}"
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
        let e = env(5.0, 20.0);
        let mut hrv = Ventilation::new(cfg.clone());
        hrv.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_kw = ports.electrical.net_active_kw();
        let expected_kw = (30.0 + 25.0) / 1000.0;
        assert!(
            (electric_kw - expected_kw).abs() < 0.001,
            "total fan power should be 55W = {expected_kw} kW, got {electric_kw}"
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

        let electric_kw = ports.electrical.net_active_kw();
        assert!(
            (electric_kw - 0.05).abs() < 0.001,
            "total fan power should still be 50W = 0.05 kW with fan_power_w alone, got {electric_kw}"
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
        let e = env(5.0, 20.0);
        let mut fan = Ventilation::new(cfg.clone());
        fan.init(&cfg, &e).expect("init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        fan.step(&e, Duration::from_secs(300), &mut ports)
            .expect("step");

        let electric_kw = ports.electrical.net_active_kw();
        assert!(
            (electric_kw - 0.06).abs() < 0.001,
            "exhaust fan power should be 60W = 0.06 kW, got {electric_kw}"
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
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
        let normal_total_kw = ports.electrical.net_active_kw();

        // Bypass: supply fan power scaled down
        let mut ports_bypass = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hrv.step(&e_bypass, Duration::from_secs(300), &mut ports_bypass)
            .expect("step bypass");
        let bypass_total_kw = ports_bypass.electrical.net_active_kw();

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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );
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
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        );

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

        let combined_kw = ports_combined.electrical.net_active_kw();
        let split_kw = ports_split.electrical.net_active_kw();
        assert!(
            (combined_kw - split_kw).abs() < 0.001,
            "combined model ({combined_kw:.6} kW) and split model ({split_kw:.6} kW) should match when supply+exhaust sum equals old total"
        );
    }
}
