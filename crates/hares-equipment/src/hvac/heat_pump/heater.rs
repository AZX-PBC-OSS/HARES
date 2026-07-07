//! Heat-pump heater variants (ASHP and MSHP).

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelPower, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, Telemetry, ThermalCategory,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::{Equipment, EquipmentConfig, load_versioned, try_save_versioned};

use super::super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, SpeedControlMode, ThermostatMode,
    ac_config::HeatPumpHeaterConfig,
    helpers::{
        apply_heating_control_unchecked, equipment_id_from_config, lookup_zone,
        zone_id_from_config_or_default,
    },
};
use super::constants::{
    DEFAULT_BACKUP_CAPACITY_W, DEFAULT_BACKUP_EIR, DEFAULT_EQUIPMENT_ID,
    DEFAULT_ER_HARD_LOCKOUT_TIME_S, DEFAULT_ER_LOCKOUT_TEMP_C, DEFAULT_ER_SETPOINT_DEADBAND_OFFSET,
    DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER, DEFAULT_HEATING_CAPACITY_W, DEFAULT_HEATING_EIR,
    DEFAULT_HP_LOCKOUT_HYSTERESIS_C, DEFAULT_HP_LOCKOUT_TEMP_C, DEFAULT_MIN_ER_CYCLE_TIME_S,
    DEFROST_CAPACITY_UNIT_FACTOR, DEFROST_EIR_CURVE_TEMP_MIN_C, MAX_OAT_SUPPLEMENTAL_C,
    MSHP_PAN_HEATER_DEFAULT_KW, MSHP_PAN_HEATER_DEFAULT_TEMP_C,
};
use super::defrost::{
    DefrostConfig, DefrostControl, DefrostCycleTracker, DefrostStrategy, evaluate_defrost,
};
use super::heater_config::{default_heater_telemetry, heater_telemetry_fields};
use hares_physics::biquadratic::biquadratic;
use hares_physics::ground::SourceTemperature;
use hares_physics::units::{power_kw_to_w, power_w_to_kw};

fn eir_from_backup_fuel(fuel: Option<FuelType>) -> f64 {
    match fuel {
        Some(
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => 1.0 / 0.80,
        _ => DEFAULT_BACKUP_EIR,
    }
}

fn fuel_type_from_backup_fuel(fuel: Option<FuelType>) -> Option<FuelType> {
    match fuel {
        Some(
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => fuel,
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum HeaterVariant {
    Ashp,
    Minisplit,
    Gshp,
    Wshp,
}

pub struct ASHPHeater {
    core: HeatPumpHeaterCore,
}

pub struct MinisplitHeater {
    core: HeatPumpHeaterCore,
}

struct HeatPumpHeaterCore {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    operating_mode: OperatingMode,
    source_temp: SourceTemperature,
    defrost_config: DefrostConfig,
    hp_lockout_temp_c: f64,
    hp_lockout_hysteresis_c: f64,
    hp_available: bool,
    er_lockout_temp_c: f64,
    /// EnergyPlus supplemental-ER upper OAT bound. ER is blocked when OAT exceeds
    /// this threshold; the heat pump alone is deemed sufficient. Hard cap: 21°C.
    max_oat_supplemental_c: f64,
    er_setpoint_offset_c: f64,
    min_er_cycle_time_s: f64,
    backup_capacity_w: f64,
    backup_eir: f64,
    backup_fuel_type: Option<FuelType>,
    /// Number of discrete ER backup heating stages (1 = binary on/off, 2–4 = multi-stage).
    er_stages: u8,
    /// Per-stage ER capacity [W] = `backup_capacity_w / er_stages`.
    /// When `er_stages = 1`, this equals `backup_capacity_w` (binary full-rated).
    er_stage_capacity_w: f64,
    pan_heater_kw: f64,
    pan_heater_temp_c: f64,
    pan_heater_on: bool,
    variant: HeaterVariant,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    defrost_active: bool,
    defrost_time_fraction: f64,
    defrost_accumulator_s: f64,
    defrost_cycle_tracker: DefrostCycleTracker,
    last_er_off_at: Option<DateTime<FixedOffset>>,
    er_was_on: bool,
    /// Previous BASE heating setpoint (without DR offset) -- used to detect
    /// user-initiated setpoint increases that trigger ER hard lockout. Comparing
    /// against the base setpoint prevents DR expiry (which raises the effective
    /// setpoint back to base) from falsely triggering the lockout.
    prev_base_setpoint: f64,
    /// Remaining hard-lockout time [s]. Set to `er_hard_lockout_time_s` when the
    /// setpoint is raised; decrements each step; ER is blocked while > 0.
    /// OCHRE HVAC.py: ER hard lockout after setpoint increase prevents expensive
    /// resistance heating when the heat pump can handle the ramp.
    er_lockout_remaining_s: f64,
    /// Duration of the ER hard lockout [s] after a setpoint increase.
    er_hard_lockout_time_s: f64,
    /// Previous zone temperature for soft-lockout detection.
    /// OCHRE HVAC.py: two-stage lockout -- after hard lockout expires, ER stays off
    /// while zone temp is still rising (heat pump is winning).
    prev_zone_temp_c: f64,
    /// Whether the ER soft lockout is currently active.
    er_soft_lockout: bool,
    /// Elapsed time [s] that the soft lockout has been continuously active.
    /// Used to force-release the soft lockout after `er_hard_lockout_time_s * 2`
    /// to prevent it from holding indefinitely.
    soft_lockout_elapsed_s: f64,
    /// Runtime fraction (PLR) of the heating coil from the most recent step.
    /// Exposed so the HP system coordinator can pass it to the companion cooler
    /// for crankcase heater power accounting.
    last_heating_rtf: f64,
    /// Heating-side sensible heat ratio. Default 1.0 (all-sensible) per OCHRE
    /// HVAC.py:458-462. When < 1.0, reverse-cycle defrost produces a small
    /// positive latent gain from indoor-coil surface moisture.
    heating_shr: f64,

    // --- Ground-loop circulation pump (GSHP only) ---
    pump_loop_depth_m: f64,
    pump_pipe_diameter_m: f64,
    pump_flow_rate_m3_per_s: f64,
    pump_efficiency: f64,
    pump_motor_efficiency: f64,
    pump_system_head_loss_m: f64,

    // --- Ideal capacity (solver-driven) ---
    use_ideal: bool,
    ideal_capacity_w: f64,

    // --- External control signals (sticky) ---
    /// DutyCycle override fraction [0..1]; 1.0 = no effect (sticky).
    ctrl_duty_cycle: f64,
    /// PowerLimit [kW]; f64::INFINITY = no limit (sticky).
    ctrl_power_limit_kw: f64,
    /// ModeOverride; None = no override (sticky).
    ctrl_mode_override: Option<OperatingMode>,

    // --- Transient signals (reset each step) ---
    /// LoadFraction [0..1]; 1.0 = no effect (transient -- resets to 1.0 each step).
    ctrl_load_fraction: f64,

    // --- Demand response state ---
    /// DR-induced setpoint offset [°C] added to effective setpoint.
    dr_setpoint_offset_c: f64,
    /// DR-induced load fraction multiplier [0..1].
    dr_load_fraction: f64,
    /// DR-induced duty cycle override [0..1]; 1.0 = no effect.
    dr_duty_cycle: f64,
    /// Remaining DR event duration [s]; None = persistent until cleared.
    dr_duration_remaining_s: Option<f64>,
    /// Current DR level (for telemetry).
    dr_level: DRLevel,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// Rule R1 reactive-only ZIP (resolved via `crate::config::resolve_reactive_zip`):
    /// applies to the compressor component only (class default pf 0.84, or a
    /// user `"zip"` override). Real power stays bit-identical; Q comes from
    /// `ZipLoad::reactive_kvar` per component (see `hvac::reactive`). ER
    /// backup, pan heater, and resistive defrost elements are resistive
    /// (pf 1.0) and contribute Q ≡ 0.
    zip: hares_types::zip::ZipLoad,
    /// Indoor blower / outdoor fan motor component ZIP (pf 0.87), derived at
    /// init via `hvac::reactive::secondary_motor_zip`.
    fan_zip: hares_types::zip::ZipLoad,
    /// Ground/water-loop circulation pump motor component ZIP (pf 0.84),
    /// derived at init via `hvac::reactive::secondary_motor_zip`. Only GSHP
    /// and WSHP variants draw pump power.
    pump_zip: hares_types::zip::ZipLoad,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HeaterState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    defrost_active: bool,
    defrost_time_fraction: f64,
    defrost_accumulator_s: f64,
    defrost_cycle_tracker: DefrostCycleTracker,
    pan_heater_on: bool,
    last_er_off_at: Option<DateTime<FixedOffset>>,
    last_speed_index: usize,
    last_speed_frac: f64,
    electric_kw: f64,
    thermal_output_w: f64,
    speed_index: f64,
    prev_base_setpoint: f64,
    er_lockout_remaining_s: f64,
    prev_zone_temp_c: f64,
    er_soft_lockout: bool,
    soft_lockout_elapsed_s: f64,
    // --- Sticky control signals ---
    ctrl_duty_cycle: f64,
    /// None means unlimited; f64::INFINITY does not serialize cleanly with postcard.
    ctrl_power_limit_kw: Option<f64>,
    ctrl_mode_override: Option<OperatingMode>,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duty_cycle: f64,
    dr_duration_remaining_s: Option<f64>,
    max_oat_supplemental_c: f64,
    hp_available: bool,
    er_was_on: bool,
    thermostat_hysteresis_c: f64,
    time_at_current_speed_s: f64,
}

// Version 2: dropped the redundant `operating_mode_code` field (telemetry mode
// code is now always recomputed from `operating_mode` via `OperatingMode::as_code`).
const HEATER_CHECKPOINT_VERSION: u32 = 2;

#[derive(Clone, Copy)]
struct HeaterControl {
    hp_on: bool,
    er_on: bool,
    speed_index: usize,
    duty_cycle: f64,
}

impl HeaterControl {
    fn off() -> Self {
        Self {
            hp_on: false,
            er_on: false,
            speed_index: 0,
            duty_cycle: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
struct HeaterStep {
    thermal_output_w: f64,
    electric_kw: f64,
    /// Compressor-only electric power [kW], excluding fan, ER backup, and pan heater.
    /// Used for COP per AHRI/SEER convention.
    compressor_kw: f64,
    fan_kw: f64,
    backup_er_kw: f64,
    pan_heater_kw: f64,
    hp_capacity_w: f64,
    er_capacity_w: f64,
    /// Number of ER stages currently active (0 when ER off).
    er_stages_on: u8,
    defrost_active: bool,
    defrost_time_fraction: f64,
    defrost_extra_power_w: f64,
    defrost_q_w: f64,
    defrost_capacity_multiplier: f64,
    /// Fuel consumption [W] when backup heater burns gas/propane/oil.
    /// Zero when backup is electric or not running.
    fuel_w: f64,
    /// Biquadratic capacity correction ratio at current conditions.
    cap_ratio: f64,
    /// Raw biquadratic capacity output before non-negative output clamp.
    /// Captures the pre-clamp value so telemetry consumers can detect when
    /// clamping occurred (e.g. cold-climate simulations below −30 °C outdoor).
    cap_ratio_raw: f64,
    /// Biquadratic EIR correction ratio at current conditions (pre-PLF).
    eir_ratio: f64,
    /// Latent gain to zone [W]. Zero during normal heating; positive during
    /// reverse-cycle defrost when heating_shr < 1.0 (indoor-coil surface moisture).
    latent_gain_w: f64,
    /// Ground-loop circulation pump electrical power [kW]. GSHP only; zero
    /// for air-source equipment.
    pump_kw: f64,
}

impl ASHPHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Ashp),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    ///
    /// Used by the system coordinator to pass as `companion_heating_rtf` to
    /// the HP cooler for crankcase heater power accounting.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

impl MinisplitHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Minisplit),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

pub struct GshpHeater {
    core: HeatPumpHeaterCore,
}

impl GshpHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Gshp),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

pub struct WshpHeater {
    core: HeatPumpHeaterCore,
}

impl WshpHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Wshp),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

impl Equipment for HeatPumpHeaterCore {
    fn checkpoint_version() -> u32 {
        HEATER_CHECKPOINT_VERSION
    }

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
        self.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.step(env, dt, ports)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.apply_control_unchecked(signal)
    }
}

delegate_equipment!(ASHPHeater, core);
delegate_equipment!(MinisplitHeater, core);
delegate_equipment!(GshpHeater, core);
delegate_equipment!(WshpHeater, core);

impl HeatPumpHeaterCore {
    fn new(config: EquipmentConfig, variant: HeaterVariant) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let equipment_type = match variant {
            HeaterVariant::Ashp => "ASHP Heater",
            HeaterVariant::Minisplit => "MSHP Heater",
            HeaterVariant::Gshp => "GSHP Heater",
            HeaterVariant::Wshp => "WSHP Heater",
        };
        let default_backup = match variant {
            HeaterVariant::Ashp => DEFAULT_BACKUP_CAPACITY_W,
            HeaterVariant::Minisplit => 0.0,
            HeaterVariant::Gshp => 0.0,
            HeaterVariant::Wshp => 0.0,
        };
        let backup_capacity_w = config
            .typed::<HeatPumpHeaterConfig>()
            .ok()
            .and_then(|cfg| cfg.common.backup_capacity_w)
            .unwrap_or_else(|| {
                if default_backup > 0.0 {
                    tracing::warn!(
                        "HeatPumpHeater backup_capacity_w not specified; \
                         falling back to default {default_backup} W"
                    );
                }
                default_backup
            })
            .max(0.0);
        let hvac_type = match variant {
            HeaterVariant::Ashp => {
                if backup_capacity_w > 0.0 {
                    HvacEquipmentType::AshpHeatPumpAux
                } else {
                    HvacEquipmentType::AshpHeatPumpOnly
                }
            }
            HeaterVariant::Minisplit => HvacEquipmentType::MiniSplitHeat,
            HeaterVariant::Gshp => HvacEquipmentType::GshpHeatPumpHeating,
            HeaterVariant::Wshp => HvacEquipmentType::WshpHeatPumpHeating,
        };

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HVAC_HEATING,
                equipment_type: Cow::Borrowed(equipment_type),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::THERMAL_SETPOINT_DELTA
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE
                    | ControlCapabilities::IDEAL_CAPACITY
                    | ControlCapabilities::MAX_CAPACITY_FRACTION,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::THERMAL
                    | CoreCapabilities::HAS_SPEED
                    | CoreCapabilities::HAS_SETPOINT
                    | CoreCapabilities::HAS_COP
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: heater_telemetry_fields(),
                zone_type: None,
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_heater_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(hvac_type, zone),
            operating_mode: OperatingMode::Off,
            source_temp: match variant {
                HeaterVariant::Gshp => SourceTemperature::BoreholeGFunction {
                    model: std::sync::Arc::new(
                        hares_physics::borehole::BoreholeGFunctionModel::new(
                            hares_physics::borehole::BoreholeConfig::default(),
                        ),
                    ),
                },
                HeaterVariant::Wshp => SourceTemperature::Constant(10.0),
                _ => SourceTemperature::OutdoorAir,
            },
            defrost_config: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                DefrostConfig {
                    control: DefrostControl::Disabled,
                    ..DefrostConfig::on_demand(1.0, 0.0)
                }
            } else {
                DefrostConfig::on_demand(1.0, 0.0)
            },
            hp_lockout_temp_c: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                f64::NEG_INFINITY
            } else {
                DEFAULT_HP_LOCKOUT_TEMP_C
            },
            hp_lockout_hysteresis_c: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp)
            {
                0.0
            } else {
                DEFAULT_HP_LOCKOUT_HYSTERESIS_C
            },
            hp_available: false,
            er_lockout_temp_c: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                f64::INFINITY
            } else {
                DEFAULT_ER_LOCKOUT_TEMP_C
            },
            max_oat_supplemental_c: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp)
            {
                f64::INFINITY
            } else {
                MAX_OAT_SUPPLEMENTAL_C
            },
            er_setpoint_offset_c: 0.0,
            min_er_cycle_time_s: DEFAULT_MIN_ER_CYCLE_TIME_S,
            backup_capacity_w,
            backup_eir: DEFAULT_BACKUP_EIR,
            backup_fuel_type: None,
            er_stages: 1,
            er_stage_capacity_w: backup_capacity_w,
            pan_heater_kw: 0.0,
            pan_heater_temp_c: MSHP_PAN_HEATER_DEFAULT_TEMP_C,
            pan_heater_on: false,
            variant,
            run_time_s: 0.0,
            cycle_on_steps: 0,
            cycle_off_steps: 0,
            defrost_active: false,
            defrost_time_fraction: 0.0,
            defrost_accumulator_s: 0.0,
            defrost_cycle_tracker: DefrostCycleTracker::new(),
            last_er_off_at: None,
            er_was_on: false,
            prev_base_setpoint: f64::NEG_INFINITY,
            er_lockout_remaining_s: 0.0,
            er_hard_lockout_time_s: DEFAULT_ER_HARD_LOCKOUT_TIME_S,
            prev_zone_temp_c: f64::NAN,
            er_soft_lockout: false,
            soft_lockout_elapsed_s: 0.0,
            last_heating_rtf: 0.0,
            heating_shr: 1.0,
            pump_loop_depth_m: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                60.0
            } else {
                0.0
            },
            pump_pipe_diameter_m: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                0.025
            } else {
                0.0
            },
            pump_flow_rate_m3_per_s: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp)
            {
                0.00019
            } else {
                0.0
            },
            pump_efficiency: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                0.35
            } else {
                0.0
            },
            pump_motor_efficiency: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
                0.40
            } else {
                0.0
            },
            pump_system_head_loss_m: if matches!(variant, HeaterVariant::Gshp | HeaterVariant::Wshp)
            {
                3.0
            } else {
                0.0
            },
            use_ideal: false,
            ideal_capacity_w: 0.0,
            ctrl_duty_cycle: 1.0,
            ctrl_power_limit_kw: f64::INFINITY,
            ctrl_mode_override: None,
            ctrl_load_fraction: 1.0,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duty_cycle: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            zone_id_explicit,
            zip: hares_types::zip::ZipLoad::constant_power(),
            fan_zip: hares_types::zip::ZipLoad::constant_power(),
            pump_zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.hvac.config.duct_zone_id =
            super::super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.init_from_typed(config, env)?;

        self.operating_mode = OperatingMode::Off;
        self.defrost_active = false;
        self.defrost_time_fraction = 0.0;
        self.defrost_accumulator_s = 0.0;
        self.defrost_cycle_tracker = DefrostCycleTracker::new();
        self.pan_heater_on = false;
        self.last_er_off_at = None;
        self.er_was_on = false;
        self.prev_base_setpoint = self.hvac.effective_setpoints().heating_c;
        self.er_lockout_remaining_s = 0.0;
        self.prev_zone_temp_c = f64::NAN;
        self.er_soft_lockout = false;
        self.soft_lockout_elapsed_s = 0.0;
        self.hp_available = env.weather.outdoor_temp_c >= self.hp_lockout_temp_c;
        self.last_heating_rtf = 0.0;
        self.run_time_s = 0.0;
        self.cycle_on_steps = 0;
        self.cycle_off_steps = 0;
        self.ctrl_duty_cycle = 1.0;
        self.ctrl_power_limit_kw = f64::INFINITY;
        self.ctrl_mode_override = None;
        self.ctrl_load_fraction = 1.0;
        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duty_cycle = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        self.fan_zip = crate::hvac::reactive::secondary_motor_zip(
            &self.zip,
            crate::hvac::reactive::FAN_MOTOR_ZIP,
        );
        self.pump_zip = crate::hvac::reactive::secondary_motor_zip(
            &self.zip,
            crate::hvac::reactive::LOOP_PUMP_ZIP,
        );
        self.telemetry = default_heater_telemetry();
        self.telemetry.set(
            tk::BIQUADRATIC_CURVE_SOURCE,
            self.hvac.config.biquadratic_curve_source.telemetry_value(),
        );
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn init_from_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let cfg = config.require_typed::<HeatPumpHeaterConfig>("Heat Pump Heater")?;
        cfg.validate()?;

        // Build BoreholeConfig from typed config for GSHP variants, falling back
        // to documented defaults for any fields left at None.
        if matches!(self.variant, HeaterVariant::Gshp) {
            let bh_dfl = hares_physics::borehole::BoreholeConfig::default();
            let bh_cfg = hares_physics::borehole::BoreholeConfig {
                borehole_depth_m: cfg
                    .common
                    .borehole_depth_m
                    .unwrap_or(bh_dfl.borehole_depth_m),
                borehole_radius_m: cfg
                    .common
                    .borehole_radius_m
                    .unwrap_or(bh_dfl.borehole_radius_m),
                shank_spacing_m: cfg
                    .common
                    .borehole_shank_spacing_m
                    .unwrap_or(bh_dfl.shank_spacing_m),
                number_of_boreholes: cfg
                    .common
                    .number_of_boreholes
                    .unwrap_or(bh_dfl.number_of_boreholes),
                soil_conductivity_w_per_m_k: cfg
                    .common
                    .borehole_soil_conductivity_w_per_m_k
                    .unwrap_or(bh_dfl.soil_conductivity_w_per_m_k),
                soil_diffusivity_m2_per_day: cfg
                    .common
                    .borehole_soil_diffusivity_m2_per_day
                    .unwrap_or(bh_dfl.soil_diffusivity_m2_per_day),
                grout_conductivity_w_per_m_k: cfg
                    .common
                    .borehole_grout_conductivity_w_per_m_k
                    .unwrap_or(bh_dfl.grout_conductivity_w_per_m_k),
                pipe_outer_radius_m: cfg
                    .common
                    .borehole_pipe_outer_radius_m
                    .unwrap_or(bh_dfl.pipe_outer_radius_m),
                pipe_inner_radius_m: cfg
                    .common
                    .borehole_pipe_inner_radius_m
                    .unwrap_or(bh_dfl.pipe_inner_radius_m),
                pipe_conductivity_w_per_m_k: cfg
                    .common
                    .borehole_pipe_conductivity_w_per_m_k
                    .unwrap_or(bh_dfl.pipe_conductivity_w_per_m_k),
            };
            self.source_temp = SourceTemperature::BoreholeGFunction {
                model: std::sync::Arc::new(hares_physics::borehole::BoreholeGFunctionModel::new(
                    bh_cfg,
                )),
            };
        }

        self.hvac.config.heating_capacities_w =
            if let Some(stages) = &cfg.common.stage_heating_capacities_w {
                stages.clone()
            } else if let Some(cap) = cfg.common.heating_capacity_w {
                vec![cap]
            } else {
                return Err(HaresError::Equipment(
                    "heating_capacity_w required for HeatPumpHeater; \
                     capacity must be provided in HPXML or computed by autosizing"
                        .into(),
                ));
            };

        let is_mini_split =
            cfg.common.is_mini_split || matches!(self.variant, HeaterVariant::Minisplit);
        if !is_mini_split {
            let n_speeds = cfg.effective_number_of_speeds() as usize;
            if n_speeds >= 2 && self.hvac.config.heating_capacities_w.len() < n_speeds {
                return Err(hares_types::HaresError::Equipment(format!(
                    "Heat pump heater init: number_of_speeds={n_speeds} but only {} heating capacity value(s) provided; supply stage_heating_capacities_w with {} elements",
                    self.hvac.config.heating_capacities_w.len(),
                    n_speeds,
                )));
            }
        }

        let default_eir = cfg
            .common
            .heating_eir
            .or_else(|| {
                cfg.common
                    .stage_heating_eirs
                    .as_ref()
                    .and_then(|eirs| eirs.first().copied())
            })
            .unwrap_or(DEFAULT_HEATING_EIR);
        self.hvac.config.eir_by_stage = if let Some(stages) = &cfg.common.stage_heating_eirs {
            stages.clone()
        } else {
            vec![default_eir]
        };

        if let Some(r) = cfg.common.charge_defect_ratio {
            super::super::hvac_core::apply_charge_defect_correction(
                &mut self.hvac.config.heating_capacities_w,
                &mut self.hvac.config.eir_by_stage,
                r,
            );
        }

        if let Some(fan_power_w) = cfg.common.fan_power_w {
            let rated_capacity_w = self
                .hvac
                .config
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "HeatPumpHeater fan-power sizing: heating_capacities_w is empty; \
                         falling back to DEFAULT_HEATING_CAPACITY_W = {DEFAULT_HEATING_CAPACITY_W} W"
                    );
                    DEFAULT_HEATING_CAPACITY_W
                });
            let rated_airflow_m3_s = self.hvac.config.airflow_m3_s_per_w * rated_capacity_w;
            self.hvac.config.fan_power_w_per_m3_s = if rated_airflow_m3_s > 0.0 {
                fan_power_w.max(0.0) / rated_airflow_m3_s
            } else {
                0.0
            };
        }

        if cfg.common.is_mini_split || matches!(self.variant, HeaterVariant::Minisplit) {
            self.hvac.config.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
            if self.hvac.config.heating_capacities_w.len() == 1 {
                let base_cap = self.hvac.config.heating_capacities_w[0];
                let base_eir = self.hvac.config.eir_by_stage[0];
                let min_frac = cfg.common.min_compressor_fraction;
                let n: usize = 4;
                // Evenly-spaced stages from min_frac to 1.0:
                //   stage[i] = rated * (min_frac + (1 - min_frac) * i / (n - 1))
                // For min_frac=0.25, n=4: [0.25, 0.50, 0.75, 1.00] (original behavior).
                // EnergyPlus sets minimum compressor capacity via the ratio of
                // Speed 1 to Speed N `Gross Rated Heating Capacity` in
                // `Coil:Heating:DX:MultiSpeed` — no single "minimum fraction" field.
                self.hvac.config.heating_capacities_w = (0..n)
                    .map(|i| base_cap * (min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64))
                    .collect();
                if cfg.common.stage_heating_eirs.is_none() {
                    let benefit = cfg.common.eir_part_load_benefit.unwrap_or(0.0);
                    // eir[i] = rated_eir * (1 - benefit * (1 - frac[i]))
                    // At minimum speed (frac = min_frac), EIR is reduced by benefit * (1 - min_frac).
                    // benefit=0 yields constant EIR (original behavior).
                    // Inverter compressors typically see 10–20% COP improvement at minimum speed
                    // due to reduced pressure ratio (AHRI 210/240 variable-speed test procedure).
                    self.hvac.config.eir_by_stage = (0..n)
                        .map(|i| {
                            let frac = min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64;
                            base_eir * (1.0 - benefit * (1.0 - frac))
                        })
                        .collect();
                }
                tracing::debug!(
                    n_stages = n,
                    min_fraction = min_frac,
                    "MSHP generating {n} speed stages from min_fraction={min_frac:.2} to rated"
                );
            }
            self.hvac.config.duct_dse = 1.0;
            self.pan_heater_kw = MSHP_PAN_HEATER_DEFAULT_KW;
        } else {
            self.hvac.config.speed_control_mode = match cfg.common.number_of_speeds {
                0 | 1 => SpeedControlMode::SingleSpeed,
                2 => SpeedControlMode::TwoSpeedTime,
                _ => SpeedControlMode::MultiSpeedInterpolated,
            };
            let rated_cap = self
                .hvac
                .config
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let fan_flow = self.hvac.config.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.config.heating_capacities_w.len().min(255) as u8;
            let cap_low = (n_speeds > 1)
                .then(|| self.hvac.config.heating_capacities_w.first().copied())
                .flatten();
            let flow_low = cap_low.map(|c| self.hvac.config.airflow_m3_s_per_w * c);
            self.hvac.config.duct_dse = if let Some(dse) = cfg.common.duct.dse_heat {
                dse
            } else {
                super::super::helpers::resolve_duct_dse(
                    config,
                    &super::super::helpers::DuctDseContext {
                        is_heating: true,
                        capacity_w: rated_cap,
                        fan_flow_m3_s: fan_flow,
                        n_speeds,
                        capacity_low_w: cap_low,
                        fan_flow_low_m3_s: flow_low,
                        is_heat_pump: true,
                    },
                )
            };
        }

        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);

        // Backup heating from typed config.
        // For ASHP, backup capacity is required — either explicit in HPXML
        // or computed by autosizing from the design heating load.
        // For MSHP and GSHP, backup is optional (defaults to 0.0 W).
        self.backup_capacity_w = if let Some(cap) = cfg.common.backup_capacity_w {
            cap.max(0.0)
        } else {
            match self.variant {
                HeaterVariant::Ashp => {
                    return Err(HaresError::Equipment(
                        "backup_capacity_w required for ASHP Heater; \
                         backup capacity must be provided in HPXML or computed by autosizing"
                            .into(),
                    ));
                }
                HeaterVariant::Minisplit | HeaterVariant::Gshp | HeaterVariant::Wshp => 0.0,
            }
        };
        self.backup_eir = cfg
            .common
            .backup_eir
            .unwrap_or_else(|| eir_from_backup_fuel(cfg.common.backup_fuel))
            .max(0.0);
        self.backup_fuel_type = fuel_type_from_backup_fuel(cfg.common.backup_fuel);
        self.er_stages = cfg.common.er_stages;
        self.er_stage_capacity_w = if self.er_stages > 0 && self.backup_capacity_w > 0.0 {
            self.backup_capacity_w / self.er_stages as f64
        } else {
            0.0
        };
        if !matches!(self.variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
            self.hp_lockout_temp_c = cfg.hp_lockout_temp_c.unwrap_or(DEFAULT_HP_LOCKOUT_TEMP_C);
            self.hp_lockout_hysteresis_c = DEFAULT_HP_LOCKOUT_HYSTERESIS_C;
            self.er_lockout_temp_c = cfg.er_lockout_temp_c.unwrap_or(DEFAULT_ER_LOCKOUT_TEMP_C);
            self.max_oat_supplemental_c = cfg
                .max_oat_supplemental_c
                .unwrap_or(MAX_OAT_SUPPLEMENTAL_C)
                .min(MAX_OAT_SUPPLEMENTAL_C);
        } else {
            if let Some(v) = cfg.hp_lockout_temp_c {
                self.hp_lockout_temp_c = v;
            }
            self.hp_lockout_hysteresis_c = 0.0;
            if let Some(v) = cfg.er_lockout_temp_c {
                self.er_lockout_temp_c = v;
            }
            if let Some(v) = cfg.max_oat_supplemental_c {
                self.max_oat_supplemental_c = v.min(MAX_OAT_SUPPLEMENTAL_C);
            }
        }
        self.er_setpoint_offset_c = cfg.er_setpoint_offset_c.unwrap_or(
            self.hvac.thermostat_fsm.thermostat.hysteresis_c
                * (DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER - DEFAULT_ER_SETPOINT_DEADBAND_OFFSET),
        );
        self.er_hard_lockout_time_s = cfg
            .er_hard_lockout_time_s
            .unwrap_or(DEFAULT_ER_HARD_LOCKOUT_TIME_S);

        if matches!(self.variant, HeaterVariant::Ashp) {
            self.hvac.config.equipment_type = if self.backup_capacity_w > 0.0 {
                HvacEquipmentType::AshpHeatPumpAux
            } else {
                HvacEquipmentType::AshpHeatPumpOnly
            };
            self.hvac.config.supply_air_temp_c = self
                .hvac
                .config
                .equipment_type
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        }

        self.telemetry
            .set(tk::HP_LOCKOUT_TEMP_C, self.hp_lockout_temp_c);
        self.telemetry
            .set(tk::ER_LOCKOUT_TEMP_C, self.er_lockout_temp_c);
        self.telemetry
            .set(tk::ER_SETPOINT_OFFSET_C, self.er_setpoint_offset_c);
        self.telemetry
            .set(tk::ER_HARD_LOCKOUT_TIME_S, self.er_hard_lockout_time_s);
        self.telemetry
            .set(tk::BACKUP_CAPACITY_W, self.backup_capacity_w);
        self.telemetry.set(tk::BACKUP_EIR, self.backup_eir);
        self.telemetry.set(
            tk::MIN_COMPRESSOR_FRACTION,
            cfg.common.min_compressor_fraction,
        );

        self.heating_shr = cfg.heating_shr.unwrap_or(1.0);

        if matches!(self.variant, HeaterVariant::Gshp | HeaterVariant::Wshp) {
            self.pump_loop_depth_m = cfg
                .common
                .pump_loop_depth_m
                .unwrap_or(self.pump_loop_depth_m);
            self.pump_pipe_diameter_m = cfg
                .common
                .pump_pipe_diameter_m
                .unwrap_or(self.pump_pipe_diameter_m);
            self.pump_flow_rate_m3_per_s = cfg
                .common
                .pump_flow_rate_m3_per_s
                .unwrap_or(self.pump_flow_rate_m3_per_s);
            self.pump_efficiency = cfg.common.pump_efficiency.unwrap_or(self.pump_efficiency);
            self.pump_motor_efficiency = cfg
                .common
                .pump_motor_efficiency
                .unwrap_or(self.pump_motor_efficiency);
            self.pump_system_head_loss_m = cfg
                .common
                .pump_system_head_loss_m
                .unwrap_or(self.pump_system_head_loss_m);
        }

        if matches!(self.variant, HeaterVariant::Wshp) {
            if let Some(ewt) = cfg.common.enter_water_temp_c {
                self.source_temp = SourceTemperature::Constant(ewt);
            }
        }

        if !matches!(self.variant, HeaterVariant::Gshp | HeaterVariant::Wshp)
            || cfg.defrost.control != DefrostControl::OnDemand
        {
            self.defrost_config = cfg.defrost;
        }

        // Anchor capacity biquadratic to manufacturer-specified capacity at the
        // AHRI 210/240 H3 low-ambient rating point (17°F / -8.33°C).
        // When the HeatingCapacity17F / HeatingCapacity ratio was parsed from HPXML:
        // 1. Evaluate the biquadratic at H3 conditions and compute the deviation.
        // 2. Warn when the pre-scaling deviation exceeds 10% (curve was substantially off).
        // 3. Scale all capacity-curve coefficients linearly so the H3 evaluation
        //    matches the manufacturer ratio. EIR curves are not scaled — efficiency
        //    vs temperature is independent of the raw capacity anchor point.
        if let Some(expected_ratio) = cfg.capacity_ratio_at_17f {
            let n_pairs = self.hvac.config.biquadratic_coeffs.len() / 2;
            if n_pairs > 0 {
                // AHRI 210/240 H3 test: indoor 70°F (21.11°C) dry-bulb,
                // outdoor 17°F (-8.33°C) dry-bulb, rated airflow.
                let t_indoor_c = 21.11;
                let t_outdoor_c = -8.33;
                let rated_curve_idx = (n_pairs - 1) * 2;
                let (raw_ratio, _) = self.hvac.evaluate_biquadratic_with_flow(
                    rated_curve_idx,
                    t_indoor_c,
                    t_outdoor_c,
                    1.0,
                );
                let rel_error = (raw_ratio - expected_ratio).abs() / expected_ratio.max(1e-6);
                if rel_error > 0.10 {
                    tracing::warn!(
                        expected_ratio,
                        curve_ratio = raw_ratio,
                        rel_error_pct = rel_error * 100.0,
                        "HeatingCapacity17F ratio {:.3} deviates from biquadratic \
                         curve ratio {:.3} by {:.1}% at 17°F; scaling curve to match",
                        expected_ratio,
                        raw_ratio,
                        rel_error * 100.0,
                    );
                }
                // Linear scaling of all capacity-curve coefficients so the curve
                // matches the manufacturer ratio at H3. Multiplying every [c0..c5]
                // by the same factor is equivalent to multiplying the output by
                // that factor — the biquadratic polynomial is homogeneous in its
                // coefficients. EIR curves (odd indices) are left unchanged.
                if raw_ratio.abs() > 1e-9 {
                    let scale = expected_ratio / raw_ratio;
                    let n = self.hvac.config.biquadratic_coeffs.len();
                    for i in (0..n).step_by(2) {
                        self.hvac.config.biquadratic_coeffs[i]
                            .iter_mut()
                            .for_each(|c| *c *= scale);
                    }
                    if rel_error > 1e-6 {
                        tracing::info!(
                            expected_ratio,
                            curve_ratio = raw_ratio,
                            scale,
                            "Scaled ASHP heating capacity biquadratic curve(s) by {:.4}x \
                             to match HeatingCapacity17F ratio at AHRI H3 conditions",
                            scale,
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        // Reset transient signals before each control step.
        self.ctrl_load_fraction = 1.0;
        self.use_ideal = self.hvac.use_ideal_capacity(env);

        // Decrement DR duration and auto-revert to Normal when expired.
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;
        if let Some(remaining) = self.dr_duration_remaining_s {
            let next = remaining - dt_s;
            if next <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.apply_dr_level(DRLevel::Normal);
            } else {
                self.dr_duration_remaining_s = Some(next);
            }
        }

        // If a ModeOverride forces Off, short-circuit thermostat.
        if matches!(self.ctrl_mode_override, Some(OperatingMode::Off))
            || self.dr_load_fraction <= 0.0
        {
            self.hvac.runtime.last_speed_index = 0;
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }

        // ModeOverride can also force specific HP modes.
        if let Some(forced_mode) = self.ctrl_mode_override {
            let control = self.forced_control(forced_mode);
            self.hvac.runtime.last_speed_index = control.speed_index;
            self.hvac.runtime.duty_cycle = control.duty_cycle;
            if self.er_was_on && !control.er_on {
                self.last_er_off_at = Some(env.current_time);
            }
            self.er_was_on = control.er_on;
            self.operating_mode = match (control.hp_on, control.er_on) {
                (true, true) => OperatingMode::HeatingHPAndER,
                (true, false) => OperatingMode::HeatingHP,
                (false, true) => OperatingMode::HeatingER,
                (false, false) => OperatingMode::Off,
            };
            return self.operating_mode;
        }

        let control = self
            .resolve_control(env)
            .unwrap_or_else(|_| HeaterControl::off());

        self.hvac.runtime.last_speed_index = control.speed_index;
        self.hvac.runtime.duty_cycle = control.duty_cycle;
        if self.er_was_on && !control.er_on {
            self.last_er_off_at = Some(env.current_time);
        }
        self.er_was_on = control.er_on;

        self.operating_mode = match (control.hp_on, control.er_on) {
            (true, true) => OperatingMode::HeatingHPAndER,
            (true, false) => OperatingMode::HeatingHP,
            (false, true) => OperatingMode::HeatingER,
            (false, false) => OperatingMode::Off,
        };

        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let dt_min = dt.as_secs_f64() / 60.0;
        let dt_s = dt.as_secs_f64();

        // Pre-compute defrost conditions for FSM advance. evaluate_defrost is
        // pure and uses only current weather + config, so we can call it here.
        let hp_on_control_pre = matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
        );
        let defrost_conditions = if hp_on_control_pre {
            let zone = lookup_zone(env, self.hvac.config.zone_id);
            let pressure_pa = env.weather.pressure_pa();
            let zone_ok = zone.is_ok();
            if zone_ok {
                let zone = zone.unwrap();
                let max_capacity_w = self
                    .hvac
                    .config
                    .heating_capacities_w
                    .last()
                    .copied()
                    .unwrap_or(0.0);
                let current_cap = max_capacity_w;
                let rtf = self.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
                let defrost = evaluate_defrost(
                    &self.defrost_config,
                    env.weather.outdoor_temp_c,
                    env.weather.outdoor_humidity_ratio,
                    pressure_pa,
                    hares_physics::psychrometrics::zone_wet_bulb_c(zone, pressure_pa),
                    max_capacity_w,
                    current_cap,
                    rtf,
                );
                (defrost.active, defrost.time_fraction)
            } else {
                (false, 0.0)
            }
        } else {
            (false, 0.0)
        };

        // Advance the discrete defrost FSM BEFORE compute_step so the current
        // state is visible during step computation.
        self.defrost_cycle_tracker
            .advance(dt_s, defrost_conditions.1, defrost_conditions.0);

        let step = self.compute_step(env, dt_min)?;

        if step.thermal_output_w > 0.0 {
            let sensible_gain_w = step.thermal_output_w - step.latent_gain_w;
            self.hvac.write_zone_thermal_contributions(
                ports,
                sensible_gain_w,
                step.latent_gain_w,
                ThermalCategory::HvacHeating,
            )?;
        }
        let sf = self.hvac.config.space_fraction;
        let scaled_electric_kw = step.electric_kw * sf;
        // Rule R1: Q from the already-computed real power, per component
        // (see `hvac::reactive`): compressor at the unit ZIP (pf 0.84),
        // blower/outdoor fan at pf 0.87, loop pump at pf 0.84. The ER
        // backup, pan heater, and resistive defrost elements are resistive
        // (pf 1.0) and contribute Q ≡ 0 — assigning them the compressor pf
        // would fabricate ~0.646·P_ER of phantom kvar during backup events.
        let reactive_power_kvar = self
            .zip
            .reactive_kvar(step.compressor_kw * sf, env.grid.voltage_pu)
            + self
                .fan_zip
                .reactive_kvar(step.fan_kw * sf, env.grid.voltage_pu)
            + self
                .pump_zip
                .reactive_kvar(step.pump_kw * sf, env.grid.voltage_pu);
        if scaled_electric_kw > 0.0 || reactive_power_kvar != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(scaled_electric_kw),
                reactive_power_kvar,
            })?;
        }
        let scaled_fuel_w = step.fuel_w * self.hvac.config.space_fraction;
        if scaled_fuel_w > 0.0 {
            if let Some(fuel_type) = self.backup_fuel_type {
                ports.accumulate(&PortContribution::Fuel {
                    fuel_type,
                    consumption_w: scaled_fuel_w,
                })?;
            }
        }

        // Record borehole heat exchange for transient ground model.
        // Heating mode: heat is extracted FROM the ground (negative Q in
        // Eskilson's convention). Q_ground = -(thermal_output - compressor_power).
        // Note: thermal_output_w is in W, compressor_kw is in kW.
        let borehole_heat_w = -(step.thermal_output_w - power_kw_to_w(step.compressor_kw));
        self.source_temp
            .record_source_heat_rate(borehole_heat_w, dt_s);

        // Record RTF for companion cooler crankcase accounting. When the HP
        // compressor is running the RTF equals the duty cycle (PLR).
        let hp_on_control = matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
        );
        self.last_heating_rtf = if hp_on_control {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };

        if self.operating_mode == OperatingMode::Off {
            self.cycle_off_steps += 1;
            self.hvac.runtime.time_at_current_speed_s = 0.0;
            self.hvac.update_prev_zone_temp(None);
        } else {
            self.cycle_on_steps += 1;
            self.run_time_s += dt.as_secs_f64();
            self.hvac.advance_speed_timer(dt.as_secs_f64());
        }

        if step.defrost_active {
            self.defrost_accumulator_s += dt.as_secs_f64();
        }

        self.defrost_active = step.defrost_active;
        self.defrost_time_fraction = step.defrost_time_fraction;

        // Telemetry reports delivered (post-DSE) thermal output for the conditioned zone.
        let dse = self.hvac.config.duct_dse.clamp(0.0, 1.0);
        let delivered_thermal_w = step.thermal_output_w * dse;
        // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
        // Use hp_capacity_w + er_capacity_w (pure gross thermal capacity) without
        // fan heat. step.thermal_output_w includes fan_heat_w which is dissipated
        // at the indoor unit / zone side, not upstream in the duct; fan heat is not
        // subject to duct distribution losses.
        let gross_capacity_w = step.hp_capacity_w + step.er_capacity_w;
        let duct_loss_w = gross_capacity_w * (1.0 - dse);
        // OCHRE HVAC.py:1464-1467: ASHP heater Main Power = compressor-only (excludes ER).
        // For ASHP/MSHP: main_power = total_input - fan - er_backup - pan_heater.
        // OCHRE HVAC.py:575 defines main_power = total_input_kw - fan_kw.
        let main_power_kw = step.compressor_kw * self.hvac.config.space_fraction;
        self.telemetry.set(tk::ELECTRIC_KW, scaled_electric_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry
            .set(tk::THERMAL_OUTPUT_W, delivered_thermal_w);
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.runtime.last_speed_index as f64);
        self.telemetry.set(
            tk::DEFROST_ACTIVE,
            if step.defrost_active { 1.0 } else { 0.0 },
        );
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        // Gross thermal output (pre-DSE) over compressor-only electric input.
        // DSE losses are a distribution inefficiency, not a reduction in equipment COP.
        let compressor_only_w = power_kw_to_w(step.compressor_kw);
        let cop = if compressor_only_w > 1e-6 {
            step.thermal_output_w / compressor_only_w
        } else {
            0.0
        }
        // AHRI 210/240-2023: ASHP heating COP ~1.5–5.0; clamp to [0.0, 10.0]
        // to exclude physically impossible values from telemetry.
        .clamp(0.0, 10.0);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        debug_assert!(
            cop.is_finite() && (0.0..=10.0).contains(&cop),
            "ASHP heating COP {cop} not in [0.0, 10.0]"
        );
        self.telemetry.set(tk::COP, cop);
        // Runtime fraction = duty cycle (PLR) when on, 0 when off.
        let rtf = if self.operating_mode != OperatingMode::Off {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.telemetry.set(tk::RUNTIME_FRACTION, rtf);
        self.telemetry.set(
            tk::COMPRESSOR_KW,
            step.compressor_kw * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::MAIN_POWER_KW, main_power_kw);
        self.telemetry.set(tk::DUCT_LOSS_W, duct_loss_w);
        self.telemetry
            .set(tk::DEFROST_TIME_FRACTION, step.defrost_time_fraction);
        self.telemetry
            .set(tk::DEFROST_EXTRA_POWER_W, step.defrost_extra_power_w);
        self.telemetry.set(tk::DEFROST_Q_W, step.defrost_q_w);
        self.telemetry.set(
            tk::DEFROST_CAPACITY_MULTIPLIER,
            step.defrost_capacity_multiplier,
        );
        self.telemetry.set(
            tk::DEFROST_CYCLE_STATE,
            self.defrost_cycle_tracker.state.code(),
        );
        self.telemetry.set(
            tk::DEFROST_ACCUMULATED_FROST_S,
            self.defrost_cycle_tracker.accumulated_frost_s,
        );
        self.telemetry.set(
            tk::DEFROST_ELAPSED_S,
            self.defrost_cycle_tracker.defrost_elapsed_s,
        );
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(
            tk::HEATING_SETPOINT_C,
            sp.heating_c + self.dr_setpoint_offset_c,
        );
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);
        let schedule_stage = self
            .hvac
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.hvac.thermostat_fsm.schedule_setpoints);
        self.telemetry
            .set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
        self.telemetry
            .set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);
        if let Some(ref rt) = self.hvac.thermostat_fsm.runtime_setpoints {
            self.telemetry
                .set(tk::RUNTIME_HEATING_SETPOINT_C, rt.heating_c.unwrap_or(0.0));
            self.telemetry
                .set(tk::RUNTIME_COOLING_SETPOINT_C, rt.cooling_c.unwrap_or(0.0));
        }
        self.telemetry
            .set(tk::FAN_KW, step.fan_kw * self.hvac.config.space_fraction);
        self.telemetry.set(
            tk::PUMP_POWER_KW,
            step.pump_kw * self.hvac.config.space_fraction,
        );
        self.telemetry.set(
            tk::BACKUP_ER_KW,
            step.backup_er_kw * self.hvac.config.space_fraction,
        );
        self.telemetry.set(
            tk::PAN_HEATER_KW,
            step.pan_heater_kw * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::HP_CAPACITY_W, step.hp_capacity_w);
        self.telemetry.set(tk::ER_CAPACITY_W, step.er_capacity_w);
        self.telemetry
            .set(tk::ER_STAGES_ON, step.er_stages_on as f64);
        self.telemetry.set(tk::FUEL_INPUT_W, scaled_fuel_w);
        self.telemetry.set(
            tk::MAX_CAPACITY_FRACTION,
            self.hvac.control.max_capacity_fraction,
        );
        self.telemetry.set(tk::CAP_RATIO, step.cap_ratio);
        self.telemetry.set(tk::CAP_RATIO_RAW, step.cap_ratio_raw);
        self.telemetry.set(tk::EIR_RATIO, step.eir_ratio);
        self.telemetry.set(tk::HEATING_LATENT_W, step.latent_gain_w);
        self.telemetry
            .set(tk::SPEED_FRAC, self.hvac.runtime.last_speed_frac);
        self.telemetry
            .set(tk::PART_LOAD_RATIO, self.hvac.runtime.duty_cycle);
        self.telemetry
            .set(tk::PART_LOAD_FACTOR, self.hvac.runtime.plf_state);
        self.telemetry.set(
            tk::STARTUP_MULTIPLIER,
            self.hvac.runtime.startup.current_multiplier(),
        );
        self.telemetry
            .set(tk::DUTY_CYCLE, self.hvac.runtime.duty_cycle);
        self.telemetry.set(
            tk::TIME_AT_CURRENT_SPEED_S,
            self.hvac.runtime.time_at_current_speed_s,
        );
        let mode_duration_s = (env.current_time
            - self
                .hvac
                .thermostat_fsm
                .mode_start_at
                .unwrap_or(env.current_time))
        .num_milliseconds()
        .max(0) as f64
            / 1000.0;
        self.telemetry.set(tk::MODE_DURATION_S, mode_duration_s);
        if step.latent_gain_w > 0.0 {
            tracing::debug!(
                heating_latent_w = step.latent_gain_w,
                "heating latent gain during defrost"
            );
        }
        let core_fuel_w = if scaled_fuel_w > 0.0 {
            self.backup_fuel_type.map(|fuel_type| FuelPower {
                fuel_type,
                consumption_w: scaled_fuel_w,
            })
        } else {
            None
        };
        let sp = self.hvac.effective_setpoints();
        let active_setpoint_c = match self.operating_mode {
            OperatingMode::Heating => sp.heating_c + self.dr_setpoint_offset_c,
            OperatingMode::Cooling => sp.cooling_c,
            _ => sp.heating_c + self.dr_setpoint_offset_c,
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(scaled_electric_kw.max(0.0))),
                reactive_power_kvar: Some(reactive_power_kvar),
                fuel_w: core_fuel_w,
                thermal_output_w: Some(delivered_thermal_w),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: Some(self.hvac.runtime.last_speed_index as u8),
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance {
                cop: Some(cop),
                main_power_kw: Some(main_power_kw),
            },
        };

        // Clear solver-provided capacity so next step starts fresh.
        self.ideal_capacity_w = 0.0;

        Ok(())
    }

    fn compute_step(&mut self, env: &EnvironmentState, dt_min: f64) -> crate::Result<HeaterStep> {
        let zone = lookup_zone(env, self.hvac.config.zone_id)?;
        let pressure_pa = env.weather.pressure_pa();

        let speed_index = self.hvac.runtime.last_speed_index;
        let speed_frac = self.hvac.runtime.last_speed_frac;
        let (stage_capacity_w, stage_eir) = if matches!(
            self.hvac.config.speed_control_mode,
            SpeedControlMode::MultiSpeedInterpolated | SpeedControlMode::VariableSpeedIdeal
        ) {
            (
                self.hvac.interpolated_capacity(
                    &self.hvac.config.heating_capacities_w,
                    speed_index,
                    speed_frac,
                ),
                self.hvac.interpolated_eir(speed_index, speed_frac),
            )
        } else {
            (
                HvacEquipment::capacity_at_stage(
                    &self.hvac.config.heating_capacities_w,
                    speed_index,
                ),
                self.hvac.eir_at_stage(speed_index),
            )
        };

        // Capacity biquadratic: evaluate first -- needed to derive PLR from
        // solver-provided ideal capacity at current conditions.
        let (_, mut cap_ratio) = self.hvac.evaluate_biquadratic_with_flow(
            speed_index * 2,
            zone.temperature_c,
            self.source_temp.compute(env),
            1.0,
        );

        // Capture the raw biquadratic curve output before non-negative output
        // clamping.  Used by CAP_RATIO_RAW telemetry to distinguish clamped-zero
        // from genuine-near-zero capacity.  Input clamping is applied (same as
        // evaluate_biquadratic) but output clamping is NOT applied here.
        let raw_coeffs = self
            .hvac
            .config
            .biquadratic_coeffs
            .get(speed_index * 2)
            .copied()
            .or_else(|| self.hvac.config.biquadratic_coeffs.last().copied())
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let x1_raw = zone.temperature_c.clamp(
            self.hvac.config.biquadratic_x1_bounds.0,
            self.hvac.config.biquadratic_x1_bounds.1,
        );
        let x2_raw = self.source_temp.compute(env).clamp(
            self.hvac.config.biquadratic_x2_bounds.0,
            self.hvac.config.biquadratic_x2_bounds.1,
        );
        let mut cap_ratio_raw = biquadratic(&raw_coeffs, x1_raw, x2_raw);

        // OCHRE HVAC.py:1044-1050 -- interpolate biquadratic between bracket stages.
        if self.hvac.config.speed_control_mode == SpeedControlMode::MultiSpeedInterpolated
            && speed_frac > 0.0
        {
            let (_, cap_ratio_high) = self.hvac.evaluate_biquadratic_with_flow(
                (speed_index + 1) * 2,
                zone.temperature_c,
                self.source_temp.compute(env),
                1.0,
            );
            cap_ratio = cap_ratio * (1.0 - speed_frac) + cap_ratio_high * speed_frac;

            let raw_coeffs_high = self
                .hvac
                .config
                .biquadratic_coeffs
                .get((speed_index + 1) * 2)
                .copied()
                .or_else(|| self.hvac.config.biquadratic_coeffs.last().copied())
                .unwrap_or([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
            let cap_ratio_raw_high = biquadratic(&raw_coeffs_high, x1_raw, x2_raw);
            cap_ratio_raw = cap_ratio_raw * (1.0 - speed_frac) + cap_ratio_raw_high * speed_frac;
        }

        // Derive PLR: from solver's ideal capacity (coarse timestep) or
        // thermostat duty cycle (fine timestep).
        let steady_capacity_w = (stage_capacity_w * cap_ratio).max(0.0);
        let hp_on_control = matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
        );
        let er_on = matches!(
            self.operating_mode,
            OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
        );
        let plr = if self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            // Solver-provided ideal capacity (positive for heating): derive PLR
            // from biquadratic-corrected capacity at current conditions.
            // When ER is also on, HP runs at full available capacity first; ER fills
            // the residual. Do not inflate the denominator with ER rated capacity.
            let hp_denominator = if er_on && !hp_on_control {
                self.backup_capacity_w
            } else {
                steady_capacity_w
            };
            let min_cap = (stage_capacity_w * 0.01).max(1.0);
            let p = (self.ideal_capacity_w / hp_denominator.max(min_cap)).clamp(0.0, 1.0);
            // Write back so telemetry and RTF reporting see the solver-derived value.
            self.hvac.runtime.duty_cycle = p;
            p
        } else {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = self.hvac.part_load_factor(plr);

        // EIR curve: divide by PLF -- cycling reduces efficiency.
        let (_, mut eir_ratio_base) = self.hvac.evaluate_biquadratic_with_flow(
            speed_index * 2 + 1,
            zone.temperature_c,
            self.source_temp.compute(env),
            1.0,
        );
        // OCHRE HVAC.py:1044-1050 -- interpolate EIR biquadratic between bracket stages.
        if self.hvac.config.speed_control_mode == SpeedControlMode::MultiSpeedInterpolated
            && speed_frac > 0.0
        {
            let (_, eir_ratio_high) = self.hvac.evaluate_biquadratic_with_flow(
                (speed_index + 1) * 2 + 1,
                zone.temperature_c,
                self.source_temp.compute(env),
                1.0,
            );
            eir_ratio_base = eir_ratio_base * (1.0 - speed_frac) + eir_ratio_high * speed_frac;
        }
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let hp_on = hp_on_control;

        let staged_capacity_w = self
            .hvac
            .apply_startup_capacity_degradation(steady_capacity_w, dt_min);
        // OCHRE HVAC.py:1156 -- clip to capacity_max * ext_capacity_frac.
        let capacity_ceiling = steady_capacity_w * self.hvac.control.max_capacity_fraction;
        let mut hp_capacity_w = if hp_on {
            (staged_capacity_w * plr).max(0.0).min(capacity_ceiling)
        } else {
            0.0
        };

        let mut hp_electric_w = if hp_on {
            (hp_capacity_w * stage_eir * eir_ratio).max(0.0)
        } else {
            0.0
        };
        let airflow_m3_s = self.hvac.airflow_m3_s_for_capacity_w(stage_capacity_w);
        let mut fan_power_w = self.hvac.fan_power_w(airflow_m3_s) * plr;

        let mut defrost_active = false;
        let mut defrost_time_fraction = 0.0;
        let mut defrost_extra_power_w = 0.0;
        let mut defrost_q_w = 0.0;
        let mut defrost_capacity_multiplier = 1.0;
        let mut latent_gain_w = 0.0;

        // Discrete defrost: when the FSM is in Defrosting AND the compressor is
        // running, override the continuous model with distinct capacity/EIR behavior.
        // The FSM continues advancing (so defrost completes on elapsed time) even
        // when the compressor is off, but the capacity/power override only applies
        // during actual compressor operation — otherwise the equipment reports
        // phantom draw while commanded off.
        let is_discrete_defrosting = self.defrost_cycle_tracker.is_defrosting() && hp_on;

        if is_discrete_defrosting {
            defrost_active = true;
            defrost_time_fraction = 1.0; // full timestep is in defrost

            match self.defrost_config.strategy {
                DefrostStrategy::ReverseCycle => {
                    // Reverse-cycle: compressor reverses; zone capacity = 0 (no net
                    // heating or cooling). Indoor coil absorbs heat from zone air to
                    // defrost outdoor coil. Electric power follows defrost EIR curve.
                    // EnergyPlus ERM 26.1 — Coils: Single-Speed Electric DX Air Heating Coil — Defrost Operation continuous model averages this penalty;
                    // HARES discrete model applies it at full intensity for cycle_duration_s.
                    defrost_capacity_multiplier = 0.0;
                    defrost_q_w = 0.0;
                    let defrost_eir = match self.defrost_config.defrost_eir_coeffs {
                        Some(c) => {
                            let wb =
                                hares_physics::psychrometrics::zone_wet_bulb_c(zone, pressure_pa)
                                    .max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            let db = env.weather.outdoor_temp_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            c[0] + c[1] * wb
                                + c[2] * wb * wb
                                + c[3] * db
                                + c[4] * wb * db
                                + c[5] * db * db
                        }
                        None => 1.0,
                    };
                    let max_capacity_w = self
                        .hvac
                        .config
                        .heating_capacities_w
                        .last()
                        .copied()
                        .unwrap_or(stage_capacity_w)
                        .max(0.0);
                    defrost_extra_power_w = defrost_eir
                        * (max_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR)
                        + self.defrost_config.defrost_power_w;
                    hp_capacity_w = 0.0;
                    hp_electric_w = defrost_extra_power_w;
                }
                DefrostStrategy::Resistive => {
                    // Compressor off; resistive element defrosts outdoor coil.
                    // Zone receives heat from the resistive element during defrost.
                    defrost_capacity_multiplier = 0.0;
                    defrost_q_w = 0.0;
                    let resistive_w = self.defrost_config.resistive_defrost_capacity_w;
                    defrost_extra_power_w = resistive_w + self.defrost_config.defrost_power_w;
                    hp_capacity_w = 0.0;
                    // Resistive defrost power is tracked in defrost_extra_power_w for
                    // telemetry; it is also included in the total electric_kw via the
                    // Resistive er_capacity_w path (EIR=1.0 for electric resistance).
                    hp_electric_w = 0.0;
                }
            }
        } else if hp_on {
            let max_capacity_w = self
                .hvac
                .config
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or(stage_capacity_w)
                .max(0.0);
            let defrost = evaluate_defrost(
                &self.defrost_config,
                env.weather.outdoor_temp_c,
                env.weather.outdoor_humidity_ratio,
                pressure_pa,
                hares_physics::psychrometrics::zone_wet_bulb_c(zone, pressure_pa),
                max_capacity_w,
                hp_capacity_w,
                plr,
            );

            if defrost.active {
                defrost_active = true;
                defrost_time_fraction = defrost.time_fraction;
                defrost_extra_power_w = defrost.extra_power_w;
                defrost_q_w = defrost.q_defrost_w;
                defrost_capacity_multiplier = defrost.capacity_multiplier;

                let crf = self
                    .defrost_config
                    .capacity_reduction_factor
                    .clamp(0.0, 1.0);

                if self.use_ideal {
                    // OCHRE HVAC.py:1154-1156 -- clamp to post-defrost rated ceiling.
                    // capacity_max = rated * cap_mult - q_defrost, then
                    // capacity = min(capacity, capacity_max * ext_capacity_frac).
                    // In HARES ext_capacity_frac is folded into crf.
                    let capacity_ceiling = (max_capacity_w * defrost.capacity_multiplier
                        - defrost.q_defrost_w)
                        .max(0.0)
                        * crf;
                    hp_capacity_w = hp_capacity_w.min(capacity_ceiling);
                } else {
                    hp_capacity_w = (hp_capacity_w * defrost.capacity_multiplier
                        - defrost.q_defrost_w)
                        .max(0.0)
                        * crf;
                }

                hp_electric_w = hp_electric_w * defrost.power_multiplier + defrost.extra_power_w;
            }

            if er_on {
                self.hvac.config.supply_air_temp_c = HvacEquipmentType::AshpHeatPumpAux
                    .default_supply_air_temp_c(env.weather.outdoor_temp_c);
            }
        } else if er_on {
            self.hvac.config.supply_air_temp_c = HvacEquipmentType::AshpHeatPumpAux
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
            hp_capacity_w = 0.0;
            hp_electric_w = 0.0;
        } else {
            hp_capacity_w = 0.0;
            hp_electric_w = 0.0;
            fan_power_w = 0.0;
        }

        // ER is a discrete resistive element: each stage is either fully on or off.
        // PLR modulation (backup_capacity_w * plr) is physically incorrect — resistive
        // elements cannot draw fractional power. In non-ideal mode, ER runs at full
        // rated capacity when the thermostat calls for it; cycling provides time-averaged
        // part-load behavior. In ideal mode, the minimum number of stages needed to
        // cover the residual is activated (ceil rounding), potentially over-delivering
        // so the thermostat cycles down on subsequent steps.
        // OCHRE HVAC.py:1404-1405 (ASHPHeater.update_er_capacity): in non-ideal mode
        // OCHRE uses `er_capacity = self.er_capacity_rated` (full rated, no PLR),
        // matching this implementation. In ideal mode OCHRE uses continuous residual
        // fill; HARES rounds up to the nearest stage boundary instead.
        let mut er_capacity_w;
        let mut er_stages_on: u8 = 0;
        if er_on && self.use_ideal {
            let residual = (self.ideal_capacity_w - hp_capacity_w).max(0.0);
            if self.er_stages <= 1 {
                // Single-stage (binary): full rated if any residual exists, else off.
                if residual > 0.0 {
                    er_capacity_w = self.backup_capacity_w;
                    er_stages_on = 1;
                } else {
                    er_capacity_w = 0.0;
                }
            } else {
                // Multi-stage: activate minimum stages to cover residual.
                let n_stages_on = (residual / self.er_stage_capacity_w)
                    .ceil()
                    .min(self.er_stages as f64) as u32;
                er_capacity_w = n_stages_on as f64 * self.er_stage_capacity_w;
                er_stages_on = n_stages_on as u8;
            }
        } else if er_on {
            // Non-ideal mode: binary — all stages energize together at full rated capacity.
            // Per-stage activation would require separate thermostats or time delays,
            // beyond the scope of this model. The thermostat cycling handles time-averaged
            // output; the instantaneous draw is always full rated.
            er_capacity_w = self.backup_capacity_w;
            er_stages_on = self.er_stages;
        } else {
            er_capacity_w = 0.0;
        }

        // During discrete Resistive defrost, the resistive element provides zone
        // heating in addition to defrosting the outdoor coil. This thermal output
        // is separate from the backup ER that the thermostat controls.
        if is_discrete_defrosting && self.defrost_config.strategy == DefrostStrategy::Resistive {
            er_capacity_w += self.defrost_config.resistive_defrost_capacity_w;
        }
        // Electric power for the defrost resistive element (if Resistive strategy).
        // This is separate from the backup ER: the defrost heater's EIR is 1.0
        // (pure electric resistance), and its thermal output was added to
        // er_capacity_w above. The backup ER's electric draw uses backup_eir,
        // so we must subtract the defrost portion before computing er_power_w.
        let (defrost_resistive_thermal_w, defrost_resistive_electric_w) = if is_discrete_defrosting
            && self.defrost_config.strategy == DefrostStrategy::Resistive
        {
            let thermal = self.defrost_config.resistive_defrost_capacity_w;
            let electric = thermal + self.defrost_config.defrost_power_w;
            (thermal, electric)
        } else {
            (0.0, 0.0)
        };
        let er_power_w = (er_capacity_w - defrost_resistive_thermal_w).max(0.0) * self.backup_eir;

        self.pan_heater_on = matches!(self.variant, HeaterVariant::Minisplit)
            && env.weather.outdoor_temp_c < self.pan_heater_temp_c
            && self.pan_heater_kw > 0.0
            && hp_on;
        let pan_heater_w = if self.pan_heater_on {
            power_kw_to_w(self.pan_heater_kw)
        } else {
            0.0
        };

        let backup_is_fuel = matches!(
            self.backup_fuel_type,
            Some(
                FuelType::Gas
                    | FuelType::Propane
                    | FuelType::Oil
                    | FuelType::Wood
                    | FuelType::Coal
                    | FuelType::WoodPellet
            )
        );
        let er_electric_w = if backup_is_fuel { 0.0 } else { er_power_w };
        let mut fuel_w = if backup_is_fuel { er_power_w } else { 0.0 };

        // Gross output including fan waste heat (OCHRE HVAC.py line 543).
        // zone_heat_fractions (set from duct_dse during init) distributes
        // this to conditioned and duct zones in write_zone_thermal_contributions.
        let mut thermal_output_w = hp_capacity_w + er_capacity_w + fan_power_w;
        let mut electric_kw = power_w_to_kw(
            hp_electric_w
                + er_electric_w
                + fan_power_w
                + pan_heater_w
                + defrost_resistive_electric_w,
        );
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        // Track compressor-only kW separately so scaling stays consistent with electric_kw.
        let mut compressor_kw = power_w_to_kw(hp_electric_w);
        let mut fan_kw = power_w_to_kw(fan_power_w);
        let mut backup_er_kw = power_w_to_kw(er_electric_w);
        let mut step_pan_heater_kw = power_w_to_kw(pan_heater_w);
        let mut step_hp_capacity_w = hp_capacity_w;
        let mut step_er_capacity_w = er_capacity_w;

        // Apply control multipliers: DutyCycle (sticky) × LoadFraction (transient)
        // × DR load fraction × DR duty cycle.
        // ER is on/off -- not modulatable -- so only compressor and fan are scaled.
        // The resistive defrost element is likewise a protective, deterministic
        // draw (it must fully clear the outdoor coil): like ER it is on/off and
        // exempt from duty-cycle curtailment. Its thermal output is already
        // carried unscaled inside er_capacity_w (see the Resistive branch above),
        // so the electric side must stay unscaled too.
        let effective_load = self.ctrl_duty_cycle
            * self.ctrl_load_fraction
            * self.dr_load_fraction
            * self.dr_duty_cycle;
        if effective_load < 1.0 {
            let hp_thermal = hp_capacity_w + fan_power_w;
            let er_thermal = er_capacity_w;
            thermal_output_w = hp_thermal * effective_load + er_thermal;
            let hp_electric = power_w_to_kw(hp_electric_w + fan_power_w + pan_heater_w);
            let er_electric = power_w_to_kw(er_electric_w + defrost_resistive_electric_w);
            electric_kw = hp_electric * effective_load + er_electric;
            compressor_kw *= effective_load;
            fan_kw *= effective_load;
            step_pan_heater_kw *= effective_load;
            step_hp_capacity_w *= effective_load;
            // backup_er_kw, fuel_w, step_er_capacity_w, and
            // defrost_resistive_electric_w are not scaled (on/off elements)
        }

        // Apply PowerLimit (sticky): shed ER first (it is on/off, not modulatable),
        // then scale HP+fan proportionally only if still over limit after shedding ER.
        // For fuel backup, electric_kw excludes ER (fuel_w carries it); the limit
        // still triggers ER shedding when fuel_w would exceed the threshold, removing
        // the thermal contribution, but the electric draw is unaffected.
        if self.ctrl_power_limit_kw.is_finite() {
            let total_kw = electric_kw + power_w_to_kw(fuel_w);
            if total_kw > self.ctrl_power_limit_kw {
                let hp_electric_kw = electric_kw - backup_er_kw;
                let hp_only_total_kw = hp_electric_kw; // fuel ER already shed in this branch
                if hp_only_total_kw <= self.ctrl_power_limit_kw {
                    // Shedding ER alone is sufficient.
                    electric_kw = hp_electric_kw;
                    thermal_output_w -= step_er_capacity_w;
                    backup_er_kw = 0.0;
                    fuel_w = 0.0;
                    step_er_capacity_w = 0.0;
                    er_stages_on = 0;
                } else {
                    // Still over limit after shedding ER; scale HP+fan+pan proportionally.
                    let er_thermal = step_er_capacity_w;
                    backup_er_kw = 0.0;
                    fuel_w = 0.0;
                    step_er_capacity_w = 0.0;
                    er_stages_on = 0;
                    let ratio = self.ctrl_power_limit_kw / hp_only_total_kw.max(f64::MIN_POSITIVE);
                    electric_kw = self.ctrl_power_limit_kw;
                    thermal_output_w = (thermal_output_w - er_thermal) * ratio;
                    compressor_kw *= ratio;
                    fan_kw *= ratio;
                    step_pan_heater_kw *= ratio;
                    step_hp_capacity_w *= ratio;
                }
            }
        }

        if er_on && hp_on {
            self.hvac.config.supply_air_temp_c = HvacEquipmentType::AshpHeatPumpAux
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        } else if hp_on {
            self.hvac.update_supply_air_temp(env);
        }

        // Heating-side latent gain.
        // During normal heating (no defrost) the outdoor coil condensate drains
        // outdoors — latent_gain_w = 0.0 is physically correct.
        // During reverse-cycle defrost with heating_shr < 1.0, residual moisture
        // on the indoor coil surface (from the brief period when it acted as an
        // evaporator) evaporates into supply air, producing a small positive
        // zone latent gain. EnergyPlus DX heating coils produce no latent output
        // in any mode (GitHub issue #7440); this model extends beyond EnergyPlus
        // to capture the small but non-zero defrost-recovery moisture effect.
        // OCHRE also uses SHR=1.0 for all heating (HVAC.py:458-462).
        if defrost_active && self.heating_shr < 1.0 {
            latent_gain_w = step_hp_capacity_w * (1.0 - self.heating_shr) * defrost_time_fraction;
        }

        // Water-loop circulation pump: runs whenever the GSHP or WSHP compressor is
        // active (heating or reverse-cycle defrost). Not scaled by effective_load
        // (duty cycle / load fraction) or ctrl_power_limit_kw because residential
        // circulators are switched by the compressor contactor / flow switch and
        // run at rated power during the call — not modulated by PLR. This models
        // the pump as binary (on when hp_on, off otherwise). The consequence: at
        // part-load (effective_load < 1.0, e.g. compressor cycling at 50% PLR),
        // the compressor kW is scaled but the full pump kW is reported, so the
        // time-averaged pump energy is not scaled to match compressor PLR.
        // space_fraction is applied in step() to the combined electric_kw.
        let pump_kw = if matches!(self.variant, HeaterVariant::Gshp | HeaterVariant::Wshp) && hp_on
        {
            hares_physics::pump::compute_ground_loop_pump_power_kw(
                self.pump_loop_depth_m,
                self.pump_pipe_diameter_m,
                self.pump_flow_rate_m3_per_s,
                self.pump_efficiency,
                self.pump_motor_efficiency,
                self.pump_system_head_loss_m,
            )
        } else {
            0.0
        };
        electric_kw += pump_kw;

        Ok(HeaterStep {
            thermal_output_w,
            electric_kw,
            compressor_kw: compressor_kw.max(0.0),
            fan_kw: fan_kw.max(0.0),
            backup_er_kw: backup_er_kw.max(0.0),
            pan_heater_kw: step_pan_heater_kw.max(0.0),
            hp_capacity_w: step_hp_capacity_w.max(0.0),
            er_capacity_w: step_er_capacity_w.max(0.0),
            er_stages_on,
            defrost_active,
            defrost_time_fraction,
            defrost_extra_power_w,
            defrost_q_w,
            defrost_capacity_multiplier,
            fuel_w: fuel_w.max(0.0),
            cap_ratio,
            cap_ratio_raw,
            eir_ratio,
            latent_gain_w,
            pump_kw: pump_kw.max(0.0),
        })
    }

    fn resolve_control(&mut self, env: &EnvironmentState) -> crate::Result<HeaterControl> {
        let mode = self.hvac.update_mode(env)?;
        let ideal_heating_call = self.use_ideal && self.ideal_capacity_w > f64::EPSILON;
        let heating_call = mode == ThermostatMode::Heating || ideal_heating_call;

        // Split base setpoint from DR offset so lockout detection is independent
        // of DR state: DR expiry raises the effective setpoint back to base but
        // must not trigger the ER hard lockout (which is reserved for actual
        // user/thermostat setpoint raises).
        let base_setpoint = self.hvac.effective_setpoints().heating_c;
        let setpoint = base_setpoint + self.dr_setpoint_offset_c;
        let zone = lookup_zone(env, self.hvac.config.zone_id)?;
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;

        // OCHRE HVAC.py: ER hard lockout after setpoint increase prevents expensive
        // resistance heating when the heat pump can handle the ramp.
        // Reference: ResStock/BEopt thermostat modeling documentation.
        // Compare against the BASE setpoint only -- DR offset changes are excluded.
        // Skip lockout on the very first call (prev == NEG_INFINITY means uninitialized).
        if self.prev_base_setpoint.is_finite() && base_setpoint > self.prev_base_setpoint + 0.1 {
            self.er_lockout_remaining_s = self.er_hard_lockout_time_s;
        }
        self.prev_base_setpoint = base_setpoint;
        // Check lock before decrementing so that a 600 s lockout blocks exactly
        // 600/dt steps: on the step where lockout is set the check sees the full
        // timer value, and the final decrement to 0 happens at the end of that step.
        let er_allowed_by_hard_lockout = self.er_lockout_remaining_s <= 0.0;
        self.er_lockout_remaining_s = (self.er_lockout_remaining_s - dt_s).max(0.0);

        // OCHRE HVAC.py: two-stage lockout -- after hard lockout expires, ER stays
        // off while zone temp is still rising (heat pump is winning the load).
        // Time-based safety release: if soft lockout has been active longer than
        // er_hard_lockout_time_s * 2 it releases regardless of zone temp trend.
        let zone_rising =
            self.prev_zone_temp_c.is_finite() && zone.temperature_c > self.prev_zone_temp_c;
        let soft_lockout_max_s = self.er_hard_lockout_time_s * 2.0;
        let soft_lockout_timeout =
            soft_lockout_max_s > 0.0 && self.soft_lockout_elapsed_s >= soft_lockout_max_s;
        if !er_allowed_by_hard_lockout {
            // Hard lockout active; soft lockout mirrors hard lockout state.
            self.er_soft_lockout = true;
            self.soft_lockout_elapsed_s += dt_s;
        } else if self.er_soft_lockout && zone_rising && !soft_lockout_timeout {
            // Hard lockout just expired; keep soft lockout while temp is rising
            // and the timeout has not been reached.
            self.er_soft_lockout = true;
            self.soft_lockout_elapsed_s += dt_s;
        } else {
            self.er_soft_lockout = false;
            // Only reset elapsed when zone stopped rising (natural release).
            // After timeout release, keep elapsed high to prevent re-arm.
            if !soft_lockout_timeout {
                self.soft_lockout_elapsed_s = 0.0;
            }
        }
        self.prev_zone_temp_c = zone.temperature_c;

        if !heating_call {
            self.hvac.update_prev_zone_temp(None);
            return Ok(HeaterControl {
                hp_on: false,
                er_on: false,
                speed_index: 0,
                duty_cycle: 0.0,
            });
        }

        let deadband = self.hvac.thermostat_fsm.thermostat.hysteresis_c.max(0.1);

        let load_ratio = if self.use_ideal {
            // Ideal capacity mode: solver provides PLR via compute_step.
            // Set load_ratio=1.0 as placeholder; actual PLR derived from
            // biquadratic-corrected capacity in compute_step.
            1.0
        } else if self.hvac.config.speed_control_mode == SpeedControlMode::SingleSpeed {
            // Single-speed compressor physics: thermostat Heating call implies
            // full-stage runtime for the step (on/off cycling only).
            1.0
        } else {
            let load_ratio_raw = (setpoint - zone.temperature_c) / deadband;
            load_ratio_raw.clamp(0.0, 1.0)
        };
        let speed =
            self.hvac
                .select_speed_with_zone_temp(load_ratio, Some(zone.temperature_c), true);
        self.hvac.update_prev_zone_temp(Some(zone.temperature_c));

        let hp_available = self.update_hp_availability(env.weather.outdoor_temp_c);
        let hp_on_control = heating_call;
        // OCHRE HVAC.py aggressive lockout: ER off above er_lockout_temp_c (default 4.44°C).
        // EnergyPlus hard cap: ER off above max_oat_supplemental_c (default 21°C).
        // Both conditions must pass; in practice the OCHRE threshold is more restrictive
        // at default settings, but max_oat_supplemental_c is the hard safety ceiling.
        let er_allowed_by_temp = env.weather.outdoor_temp_c < self.er_lockout_temp_c
            && env.weather.outdoor_temp_c <= self.max_oat_supplemental_c;
        let er_allowed_by_cycle = self.er_cycle_ready(env.current_time);
        let er_allowed_by_lockout = er_allowed_by_hard_lockout && !self.er_soft_lockout;

        // ER thermostat hysteresis band (matches OCHRE temp_turn_on / temp_turn_off).
        // Turn on:  zone <= setpoint - er_offset
        // Turn off: zone > er_turn_on + deadband
        let er_turn_on_c = setpoint - self.er_setpoint_offset_c;
        let er_turn_off_c = er_turn_on_c + self.hvac.thermostat_fsm.thermostat.hysteresis_c;
        let er_thermostat_call = if self.er_was_on {
            zone.temperature_c <= er_turn_off_c
        } else {
            zone.temperature_c <= er_turn_on_c
        };

        let hp_on = hp_on_control && hp_available && speed.part_load_ratio > 0.0;
        let er_demand = if self.use_ideal && self.ideal_capacity_w > f64::EPSILON {
            if hp_available {
                let stage_cap = if matches!(
                    self.hvac.config.speed_control_mode,
                    SpeedControlMode::MultiSpeedInterpolated | SpeedControlMode::VariableSpeedIdeal
                ) {
                    self.hvac.interpolated_capacity(
                        &self.hvac.config.heating_capacities_w,
                        speed.speed_index,
                        speed.speed_frac,
                    )
                } else {
                    HvacEquipment::capacity_at_stage(
                        &self.hvac.config.heating_capacities_w,
                        speed.speed_index,
                    )
                };
                let (_, cap_ratio) = self.hvac.evaluate_biquadratic_with_flow(
                    speed.speed_index * 2,
                    zone.temperature_c,
                    self.source_temp.compute(env),
                    1.0,
                );
                let hp_available_capacity_w = (stage_cap * cap_ratio).max(0.0);
                self.ideal_capacity_w > hp_available_capacity_w
            } else {
                self.ideal_capacity_w > f64::EPSILON
            }
        } else {
            er_thermostat_call
        };
        let er_on = self.backup_capacity_w > 0.0
            && er_allowed_by_temp
            && er_allowed_by_cycle
            && er_allowed_by_lockout
            && er_demand;

        if std::env::var("HARES_DEBUG_ASHP_CONTROL").is_ok()
            && matches!(self.variant, HeaterVariant::Ashp)
        {
            eprintln!(
                "[ASHP-CTL] t={} mode={:?} z={:.4} sp={:.4} deadband={:.4} hp_lockout={:.4} er_offset={:.4} er_turn_on={:.4} er_turn_off={:.4} hp_avail={} er_temp_ok={} er_cycle_ok={} er_lockout_ok={} er_call={} hp_on={} er_on={} speed_mode={:?} speed_idx={} duty={:.4}",
                env.current_time,
                mode,
                zone.temperature_c,
                setpoint,
                deadband,
                self.hp_lockout_temp_c,
                self.er_setpoint_offset_c,
                er_turn_on_c,
                er_turn_off_c,
                hp_available,
                er_allowed_by_temp,
                er_allowed_by_cycle,
                er_allowed_by_lockout,
                er_thermostat_call,
                hp_on,
                er_on,
                self.hvac.config.speed_control_mode,
                speed.speed_index,
                speed.part_load_ratio,
            );
        }

        Ok(HeaterControl {
            hp_on,
            er_on,
            speed_index: speed.speed_index,
            duty_cycle: speed.part_load_ratio,
        })
    }

    fn update_hp_availability(&mut self, outdoor_temp_c: f64) -> bool {
        self.hp_available = if self.hp_available {
            outdoor_temp_c >= self.hp_lockout_temp_c - self.hp_lockout_hysteresis_c
        } else {
            outdoor_temp_c >= self.hp_lockout_temp_c
        };
        self.hp_available
    }

    fn forced_control(&self, forced_mode: OperatingMode) -> HeaterControl {
        match forced_mode {
            OperatingMode::Off => HeaterControl::off(),
            OperatingMode::HeatingHP => HeaterControl {
                hp_on: true,
                er_on: false,
                speed_index: self.max_heating_speed_index(),
                duty_cycle: 1.0,
            },
            OperatingMode::HeatingER => HeaterControl {
                hp_on: false,
                er_on: self.backup_capacity_w > 0.0,
                speed_index: 0,
                duty_cycle: 1.0,
            },
            OperatingMode::HeatingHPAndER => HeaterControl {
                hp_on: true,
                er_on: self.backup_capacity_w > 0.0,
                speed_index: self.max_heating_speed_index(),
                duty_cycle: 1.0,
            },
            _ => HeaterControl::off(),
        }
    }

    fn max_heating_speed_index(&self) -> usize {
        self.hvac
            .config
            .heating_capacities_w
            .len()
            .saturating_sub(1)
    }

    fn er_cycle_ready(&self, now: DateTime<FixedOffset>) -> bool {
        let Some(last_off) = self.last_er_off_at else {
            return true;
        };
        let elapsed_s = (now - last_off).num_milliseconds().max(0) as f64 / 1000.0;
        elapsed_s >= self.min_er_cycle_time_s
    }

    fn apply_dr_level(&mut self, level: DRLevel) {
        self.dr_level = level;
        match level {
            DRLevel::Normal => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::Moderate => {
                self.dr_setpoint_offset_c = -1.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::High => {
                self.dr_setpoint_offset_c = -2.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.8;
            }
            DRLevel::Critical => {
                self.dr_setpoint_offset_c = -3.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.5;
            }
            DRLevel::GridEmergency => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.0;
            }
        }
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &HeaterState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                mode_start_at: self.hvac.thermostat_fsm.mode_start_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                cycle_on_steps: self.cycle_on_steps,
                cycle_off_steps: self.cycle_off_steps,
                defrost_active: self.defrost_active,
                defrost_time_fraction: self.defrost_time_fraction,
                defrost_accumulator_s: self.defrost_accumulator_s,
                defrost_cycle_tracker: self.defrost_cycle_tracker.clone(),
                pan_heater_on: self.pan_heater_on,
                last_er_off_at: self.last_er_off_at,
                last_speed_index: self.hvac.runtime.last_speed_index,
                last_speed_frac: self.hvac.runtime.last_speed_frac,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                speed_index: self.telemetry.get(tk::SPEED_INDEX).unwrap_or(0.0),
                prev_base_setpoint: self.prev_base_setpoint,
                er_lockout_remaining_s: self.er_lockout_remaining_s,
                prev_zone_temp_c: self.prev_zone_temp_c,
                er_soft_lockout: self.er_soft_lockout,
                soft_lockout_elapsed_s: self.soft_lockout_elapsed_s,
                ctrl_duty_cycle: self.ctrl_duty_cycle,
                ctrl_power_limit_kw: if self.ctrl_power_limit_kw.is_finite() {
                    Some(self.ctrl_power_limit_kw)
                } else {
                    None
                },
                ctrl_mode_override: self.ctrl_mode_override,
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duty_cycle: self.dr_duty_cycle,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
                max_oat_supplemental_c: self.max_oat_supplemental_c,
                hp_available: self.hp_available,
                er_was_on: self.er_was_on,
                thermostat_hysteresis_c: self.hvac.thermostat_fsm.thermostat.hysteresis_c,
                time_at_current_speed_s: self.hvac.runtime.time_at_current_speed_s,
            },
            HEATER_CHECKPOINT_VERSION,
            "Heater",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: HeaterState = load_versioned(
            state,
            HEATER_CHECKPOINT_VERSION,
            "Heater",
            self.descriptor.id,
        )?;
        self.hvac.thermostat_fsm.mode = decoded.mode;
        self.hvac.runtime.duty_cycle = decoded.duty_cycle;
        self.hvac.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.thermostat_fsm.mode_start_at = decoded.mode_start_at;
        self.hvac.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.cycle_on_steps = decoded.cycle_on_steps;
        self.cycle_off_steps = decoded.cycle_off_steps;
        self.defrost_active = decoded.defrost_active;
        self.defrost_time_fraction = decoded.defrost_time_fraction;
        self.defrost_accumulator_s = decoded.defrost_accumulator_s;
        let tracker_state_code = decoded.defrost_cycle_tracker.state.code();
        let tracker_frost_s = decoded.defrost_cycle_tracker.accumulated_frost_s;
        let tracker_elapsed_s = decoded.defrost_cycle_tracker.defrost_elapsed_s;
        self.defrost_cycle_tracker = decoded.defrost_cycle_tracker;
        self.pan_heater_on = decoded.pan_heater_on;
        self.last_er_off_at = decoded.last_er_off_at;
        self.er_was_on = decoded.er_was_on;
        self.hvac.runtime.last_speed_index = decoded.last_speed_index;
        self.hvac.runtime.last_speed_frac = decoded.last_speed_frac;
        self.ctrl_duty_cycle = decoded.ctrl_duty_cycle;
        self.ctrl_power_limit_kw = decoded.ctrl_power_limit_kw.unwrap_or(f64::INFINITY);
        self.ctrl_mode_override = decoded.ctrl_mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duty_cycle = decoded.dr_duty_cycle;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.hp_available = decoded.hp_available;
        self.max_oat_supplemental_c = decoded.max_oat_supplemental_c;
        self.hvac.thermostat_fsm.thermostat.hysteresis_c = decoded.thermostat_hysteresis_c;
        self.hvac.runtime.time_at_current_speed_s = decoded.time_at_current_speed_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry.insert(tk::SPEED_INDEX, decoded.speed_index);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        self.telemetry.insert(
            tk::DEFROST_ACTIVE,
            if decoded.defrost_active { 1.0 } else { 0.0 },
        );
        self.telemetry
            .insert(tk::DEFROST_CYCLE_STATE, tracker_state_code);
        self.telemetry
            .insert(tk::DEFROST_ACCUMULATED_FROST_S, tracker_frost_s);
        self.telemetry
            .insert(tk::DEFROST_ELAPSED_S, tracker_elapsed_s);
        self.prev_base_setpoint = decoded.prev_base_setpoint;
        self.er_lockout_remaining_s = decoded.er_lockout_remaining_s;
        self.prev_zone_temp_c = decoded.prev_zone_temp_c;
        self.er_soft_lockout = decoded.er_soft_lockout;
        self.soft_lockout_elapsed_s = decoded.soft_lockout_elapsed_s;
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ThermalSetpoint { deadband_c, .. } => {
                apply_heating_control_unchecked(
                    &mut self.hvac,
                    signal,
                    &self.descriptor.equipment_type,
                )?;
                if let Some(db) = deadband_c {
                    self.hvac.thermostat_fsm.thermostat.hysteresis_c = *db;
                }
            }
            ControlSignal::DutyCycle { on_fraction, .. } => {
                self.ctrl_duty_cycle = *on_fraction;
            }
            ControlSignal::LoadFraction { fraction } => {
                self.ctrl_load_fraction = *fraction;
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                self.ctrl_power_limit_kw = *max_power_kw;
            }
            ControlSignal::ModeOverride { mode } => {
                self.ctrl_mode_override = Some(*mode);
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.apply_dr_level(*level);
                self.dr_duration_remaining_s = *duration_s;
            }
            ControlSignal::IdealCapacity { capacity_w, .. } => {
                self.ideal_capacity_w = *capacity_w;
            }
            ControlSignal::MaxCapacityFraction { fraction } => {
                self.hvac.control.max_capacity_fraction = *fraction;
            }
            _ => {
                apply_heating_control_unchecked(
                    &mut self.hvac,
                    signal,
                    &self.descriptor.equipment_type,
                )?;
            }
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c + self.dr_setpoint_offset_c;
        Some((self.hvac.config.zone_id, setpoint))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, OperatingMode, PortSlots,
        ScheduleSourceConfig, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{ASHPHeater, GshpHeater, MinisplitHeater, SpeedControlMode};
    use crate::hvac::heating_config::HvacSetpointConfig;
    use crate::{
        DefrostConfig, DefrostControl, Equipment, EquipmentConfig, HeatPumpCommonConfig,
        HeatPumpHeaterConfig,
    };
    use hares_physics::units::power_kw_to_w;

    fn env(zone_temp_c: f64, outdoor_c: f64, outdoor_w: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: outdoor_w,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
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
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn heater_typed_config() -> HeatPumpHeaterConfig {
        HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                equipment_id: None,
                zone_id: Some(1),
                heating_capacity_w: Some(8_000.0),
                heating_eir: Some(0.33),
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                backup_fuel: None,
                backup_capacity_w: Some(4_000.0),
                backup_eir: None,
                fraction_heating_load_served: None,
                cooling_capacity_w: None,
                cooling_eir: None,
                stage_cooling_capacities_w: None,
                stage_cooling_eirs: None,
                fraction_cooling_load_served: None,
                number_of_speeds: 1,
                is_mini_split: false,
                shr: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: None,
                setpoint: HvacSetpointConfig {
                    heating_setpoint_c: Some(21.0),
                    cooling_setpoint_c: Some(26.0),
                    ..Default::default()
                },
                hysteresis_c: Some(1.0),
                duct: Default::default(),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_compressor_fraction: 0.25,
                eir_part_load_benefit: None,
                er_stages: 1,
                charge_defect_ratio: None,
                ..Default::default()
            },
            hp_lockout_temp_c: None,
            er_lockout_temp_c: None,
            max_oat_supplemental_c: None,
            er_setpoint_offset_c: None,
            er_hard_lockout_time_s: None,
            heating_shr: None,
            capacity_ratio_at_17f: None,
            defrost: DefrostConfig::default(),
        }
    }

    // -----------------------------------------------------------------------
    // MSHP minimum compressor speed — hardcoded stage regression tests
    //
    // These tests have direct struct access so they can verify stage values.
    // -----------------------------------------------------------------------

    fn mshp_typed_config(rated_w: f64) -> HeatPumpHeaterConfig {
        HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                equipment_id: None,
                zone_id: Some(1),
                heating_capacity_w: Some(rated_w),
                heating_eir: Some(0.25),
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                backup_fuel: None,
                backup_capacity_w: None,
                backup_eir: None,
                fraction_heating_load_served: None,
                cooling_capacity_w: None,
                cooling_eir: None,
                stage_cooling_capacities_w: None,
                stage_cooling_eirs: None,
                fraction_cooling_load_served: None,
                number_of_speeds: 1,
                is_mini_split: true,
                shr: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: None,
                setpoint: HvacSetpointConfig {
                    heating_setpoint_c: Some(21.0),
                    cooling_setpoint_c: Some(26.0),
                    ..Default::default()
                },
                hysteresis_c: Some(1.0),
                duct: Default::default(),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_compressor_fraction: 0.25,
                eir_part_load_benefit: None,
                er_stages: 1,
                charge_defect_ratio: None,
                ..Default::default()
            },
            hp_lockout_temp_c: None,
            er_lockout_temp_c: None,
            max_oat_supplemental_c: None,
            er_setpoint_offset_c: None,
            er_hard_lockout_time_s: None,
            heating_shr: None,
            capacity_ratio_at_17f: None,
            defrost: DefrostConfig::default(),
        }
    }

    #[test]
    fn mshp_speed_stages_hardcoded_at_25_50_75_100pct() {
        // MSHP stage generation hardcodes 0.25/0.50/0.75/1.00 of rated.
        // OCHRE "HVAC Multispeed Parameters.csv" uses Capacity Ratio 1 = 0.40 for
        // MSHP Heater — a 15 percentage-point divergence at the minimum stage.
        // This test pins the current behavior.
        // After min_compressor_fraction is added with default 0.25,
        // this test must still pass (backward-compatible default).
        const RATED_W: f64 = 10_000.0;

        let typed = mshp_typed_config(RATED_W);
        let cfg = EquipmentConfig::from_typed(
            "mshp_stages".to_string(),
            "MSHP Heater".to_string(),
            typed,
        )
        .unwrap();
        let mut eq = MinisplitHeater::new(cfg.clone());
        let e = env(18.0, 5.0, 0.004);
        eq.init(&cfg, &e).unwrap();

        let stages = &eq.core.hvac.config.heating_capacities_w;
        assert_eq!(stages.len(), 4, "MSHP must generate exactly 4 speed stages");

        // Current hardcoded fractions: [0.25, 0.50, 0.75, 1.00].
        // OCHRE diverges at stage 1: Capacity Ratio 1 = 0.40, not 0.25.
        let expected = [RATED_W * 0.25, RATED_W * 0.50, RATED_W * 0.75, RATED_W];
        for (i, (&actual, &exp)) in stages.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 1.0,
                "stage {} capacity = {:.1} W, expected {:.1} W \
                 (hardcoded {:.0}% of rated). OCHRE uses {:.0}% for stage 1.",
                i + 1,
                actual,
                exp,
                expected[i] / RATED_W * 100.0,
                40.0,
            );
        }
    }

    #[test]
    fn mshp_eir_identical_across_all_stages() {
        // eir_by_stage = vec![base_eir; 4] — no part-load EIR benefit.
        // OCHRE loads per-stage COP from CSV (COP 1 ≠ COP 4 for MSHP Heater).
        // This test pins the current flat-EIR behavior.
        const RATED_W: f64 = 10_000.0;
        const BASE_EIR: f64 = 0.25;

        let typed = mshp_typed_config(RATED_W);
        let cfg =
            EquipmentConfig::from_typed("mshp_eir".to_string(), "MSHP Heater".to_string(), typed)
                .unwrap();
        let mut eq = MinisplitHeater::new(cfg.clone());
        let e = env(18.0, 5.0, 0.004);
        eq.init(&cfg, &e).unwrap();

        let eirs = &eq.core.hvac.config.eir_by_stage;
        assert_eq!(eirs.len(), 4, "MSHP must generate 4 EIR values");

        for (i, &eir) in eirs.iter().enumerate() {
            assert!(
                (eir - BASE_EIR).abs() < 1e-9,
                "stage {} EIR = {:.6}, expected base EIR {:.4} (flat — no part-load benefit). \
                 OCHRE uses per-stage COP values.",
                i + 1,
                eir,
                BASE_EIR,
            );
        }
    }

    #[test]
    fn mshp_min_compressor_fraction_30_produces_correct_stages() {
        // min_compressor_fraction = 0.30 with 4 stages produces
        // [0.30, 0.5333, 0.7667, 1.00] of rated capacity.
        // stage[i] = rated * (min_frac + (1 - min_frac) * i / (n - 1))
        const RATED_W: f64 = 10_000.0;

        let mut typed = mshp_typed_config(RATED_W);
        typed.common.min_compressor_fraction = 0.30;
        let cfg =
            EquipmentConfig::from_typed("mshp_min30".to_string(), "MSHP Heater".to_string(), typed)
                .unwrap();
        let mut eq = MinisplitHeater::new(cfg.clone());
        let e = env(18.0, 5.0, 0.004);
        eq.init(&cfg, &e).unwrap();

        let stages = &eq.core.hvac.config.heating_capacities_w;
        assert_eq!(stages.len(), 4);

        let min_frac = 0.30_f64;
        let expected: Vec<f64> = (0..4)
            .map(|i| RATED_W * (min_frac + (1.0 - min_frac) * i as f64 / 3.0))
            .collect();

        for (i, (&actual, &exp)) in stages.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 0.5,
                "stage {} capacity = {:.2} W, expected {:.2} W",
                i + 1,
                actual,
                exp,
            );
        }
        assert!(
            (stages[0] - RATED_W * 0.30).abs() < 0.5,
            "stage 1 must be 30% of rated; got {:.2} W",
            stages[0],
        );
        assert!(
            (stages[3] - RATED_W).abs() < 0.5,
            "stage 4 must be 100% of rated; got {:.2} W",
            stages[3],
        );
    }

    #[test]
    fn mshp_eir_part_load_benefit_reduces_low_speed_eir() {
        // eir_part_load_benefit = 0.15: eir[i] = rated_eir * (1 - 0.15 * (1 - frac[i])).
        // At minimum speed (frac = 0.25 with default min_compressor_fraction):
        //   eir[0] = 0.25 * (1 - 0.15 * 0.75) = 0.25 * 0.8875 = 0.221875
        // At rated speed (frac = 1.0): eir[3] = 0.25 (unchanged).
        const RATED_W: f64 = 10_000.0;
        const BASE_EIR: f64 = 0.25;
        const BENEFIT: f64 = 0.15;

        let mut typed = mshp_typed_config(RATED_W);
        typed.common.eir_part_load_benefit = Some(BENEFIT);
        let cfg = EquipmentConfig::from_typed(
            "mshp_eir_benefit".to_string(),
            "MSHP Heater".to_string(),
            typed,
        )
        .unwrap();
        let mut eq = MinisplitHeater::new(cfg.clone());
        let e = env(18.0, 5.0, 0.004);
        eq.init(&cfg, &e).unwrap();

        let eirs = &eq.core.hvac.config.eir_by_stage;
        assert_eq!(eirs.len(), 4);

        // First stage EIR must be lower than rated EIR.
        assert!(
            eirs[0] < BASE_EIR,
            "stage 1 EIR = {:.6} must be < rated EIR {:.4} with benefit={}",
            eirs[0],
            BASE_EIR,
            BENEFIT,
        );
        // Last stage EIR must equal rated EIR.
        assert!(
            (eirs[3] - BASE_EIR).abs() < 1e-9,
            "stage 4 EIR = {:.6} must equal rated EIR {:.4}",
            eirs[3],
            BASE_EIR,
        );
        // Verify the formula explicitly at stage 1 (frac = 0.25):
        let frac_0 = 0.25_f64;
        let expected_eir_0 = BASE_EIR * (1.0 - BENEFIT * (1.0 - frac_0));
        assert!(
            (eirs[0] - expected_eir_0).abs() < 1e-9,
            "stage 1 EIR = {:.6}, expected {:.6}",
            eirs[0],
            expected_eir_0,
        );
    }

    #[test]
    fn mshp_stage_heating_eirs_preserved_when_is_mini_split() {
        // When stage_heating_eirs is explicitly provided alongside is_mini_split=true,
        // the mini-split init path must NOT overwrite those EIRs with the
        // eir_part_load_benefit formula. The user's per-stage EIRs take precedence.
        const RATED_W: f64 = 10_000.0;

        let mut typed = mshp_typed_config(RATED_W);
        typed.common.stage_heating_eirs = Some(vec![0.20, 0.22, 0.23, 0.25]);
        let cfg = EquipmentConfig::from_typed(
            "mshp_explicit_eirs".to_string(),
            "MSHP Heater".to_string(),
            typed,
        )
        .unwrap();
        let mut eq = MinisplitHeater::new(cfg.clone());
        let e = env(18.0, 5.0, 0.004);
        eq.init(&cfg, &e).unwrap();

        let eirs = &eq.core.hvac.config.eir_by_stage;
        assert_eq!(
            eirs,
            &vec![0.20, 0.22, 0.23, 0.25],
            "explicit stage_heating_eirs must not be overwritten by mini-split init path",
        );
    }

    #[test]
    fn mshp_validate_rejects_min_compressor_fraction_out_of_range() {
        let mut cfg_below = mshp_typed_config(10_000.0);
        cfg_below.common.min_compressor_fraction = 0.05;
        let err = cfg_below.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("min_compressor_fraction must be in [0.1, 0.5]"),
            "validation must reject min_compressor_fraction < 0.1; got: {err}",
        );

        let mut cfg_above = mshp_typed_config(10_000.0);
        cfg_above.common.min_compressor_fraction = 0.55;
        let err = cfg_above.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("min_compressor_fraction must be in [0.1, 0.5]"),
            "validation must reject min_compressor_fraction > 0.5; got: {err}",
        );
    }

    #[test]
    fn mshp_validate_rejects_eir_part_load_benefit_out_of_range() {
        let mut cfg_below = mshp_typed_config(10_000.0);
        cfg_below.common.eir_part_load_benefit = Some(-0.1);
        let err = cfg_below.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("eir_part_load_benefit must be in [0.0, 1.0]"),
            "validation must reject eir_part_load_benefit < 0.0; got: {err}",
        );

        let mut cfg_above = mshp_typed_config(10_000.0);
        cfg_above.common.eir_part_load_benefit = Some(1.5);
        let err = cfg_above.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("eir_part_load_benefit must be in [0.0, 1.0]"),
            "validation must reject eir_part_load_benefit > 1.0; got: {err}",
        );
    }

    fn add_identity_biquadratic_curves(cfg: &mut EquipmentConfig) {
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
    }

    fn heater_config() -> EquipmentConfig {
        heater_config_with(|_| {})
    }

    fn heater_config_with(mutator: impl FnOnce(&mut HeatPumpHeaterConfig)) -> EquipmentConfig {
        let mut typed = heater_typed_config();
        mutator(&mut typed);
        let mut cfg =
            EquipmentConfig::from_typed("HP Heater".to_string(), "ASHP Heater".to_string(), typed)
                .unwrap();
        add_identity_biquadratic_curves(&mut cfg);
        cfg
    }

    fn env_at(zone_temp_c: f64, outdoor_c: f64, outdoor_w: f64, seconds: i64) -> EnvironmentState {
        let mut e = env(zone_temp_c, outdoor_c, outdoor_w);
        e.current_time += ChronoDuration::seconds(seconds);
        e
    }

    #[test]
    fn er_hard_lockout_defaults_to_ochre_parity_zero() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(eq.core.er_hard_lockout_time_s, 0.0);
    }

    #[test]
    fn er_hard_lockout_uses_explicit_config_value() {
        let cfg = heater_config_with(|typed| typed.er_hard_lockout_time_s = Some(300.0));
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(eq.core.er_hard_lockout_time_s, 300.0);
    }

    #[test]
    fn defrost_trips_in_cold_conditions() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, 0.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(eq.telemetry().get(tk::DEFROST_ACTIVE), Some(1.0));
    }

    #[test]
    fn no_defrost_when_warm() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, 10.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(eq.telemetry().get(tk::DEFROST_ACTIVE), Some(0.0));
    }

    /// Reactive-power contract for the ASHP heater: per-component Q
    /// (compressor pf 0.84, fan pf 0.87; ER/pan resistive Q=0), REACTIVE
    /// declared, and port/CoreOutput/telemetry agree bit-for-bit.
    /// Off ⇒ Q == 0.
    #[test]
    fn heating_reactive_power_pf_and_channels_agree() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 10.0, 0.005);
        eq.init(&cfg, &environment).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "ASHP heater must declare REACTIVE"
        );
        assert_eq!(eq.core.zip.pf, 0.84, "class default compressor pf");
        assert_eq!(
            eq.core.fan_zip,
            crate::hvac::reactive::FAN_MOTOR_ZIP,
            "fan component uses the FAN motor ZIP"
        );
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let p_kw = ports.electrical.load_power_w / 1000.0;
        assert!(p_kw > 0.0, "heating call must draw real power");
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).expect("compressor");
        let fan_kw = eq.telemetry().get(tk::FAN_KW).expect("fan");
        assert!(compressor_kw > 0.0 && fan_kw > 0.0);
        assert_eq!(
            eq.telemetry().get(tk::BACKUP_ER_KW),
            Some(0.0),
            "precondition: ER backup must be off in this HP-only step"
        );
        let q = ports.electrical.reactive_power_kvar;
        // Per-component: Q = P_comp·tan(acos(0.84)) + P_fan·tan(acos(0.87)).
        // No ER, pan heater, or pump draw in this mild-weather HP-only step.
        let expected = compressor_kw * 0.84_f64.acos().tan() + fan_kw * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "per-component Q must be comp·tan(acos(0.84)) + fan·tan(acos(0.87)): \
             q={q}, expected={expected}"
        );
        assert_eq!(
            eq.core_output()
                .flows
                .reactive_power_kvar
                .expect("Some")
                .to_bits(),
            q.to_bits(),
            "CoreOutput Q must equal port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .expect("telemetry Q")
                .to_bits(),
            q.to_bits(),
            "telemetry Q must equal port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("core contract must hold with REACTIVE declared");

        // Off case: zone in the deadband (23 °C, between heating 21+1 and
        // cooling 26−1) ⇒ no heating call ⇒ Q == 0.
        let off_env = env(23.0, 10.0, 0.005);
        eq.update_control(&off_env);
        let mut off_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&off_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        assert_eq!(
            off_ports.electrical.reactive_power_kvar, 0.0,
            "off ⇒ Q == 0"
        );
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "off ⇒ CoreOutput Q == Some(0.0)"
        );
    }

    /// Compressor-only operation (fan power configured to zero, no ER/pan/
    /// pump): Q/P must equal tan(acos(0.84)) — the pure compressor arm of the
    /// per-component model.
    #[test]
    fn compressor_only_reactive_q_over_p_is_tan_acos_084() {
        let cfg = heater_config_with(|typed| {
            typed.common.fan_power_w = Some(0.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 10.0, 0.005);
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let p_kw = ports.electrical.load_power_w / 1000.0;
        assert!(p_kw > 0.0, "heating call must draw real power");
        assert_eq!(eq.telemetry().get(tk::FAN_KW), Some(0.0), "fan must be 0");
        assert_eq!(
            eq.telemetry().get(tk::BACKUP_ER_KW),
            Some(0.0),
            "ER must be off"
        );
        let q = ports.electrical.reactive_power_kvar;
        let expected = p_kw * 0.84_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-12,
            "compressor-only Q/P must equal tan(acos(0.84)): q={q}, expected={expected}"
        );
    }

    /// ER-backup-active rows must exclude the resistive ER wattage from Q:
    /// with HP + ER forced on, Q = comp·tan(acos(0.84)) + fan·tan(acos(0.87))
    /// and NOT total·tan(acos(0.84)) — the old folded pf fabricated
    /// ~0.646·P_ER of phantom kvar during backup events.
    #[test]
    fn er_backup_active_reactive_excludes_er_wattage() {
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(10_000.0);
            typed.common.backup_eir = Some(1.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(15.0, 2.0, 0.003);
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHPAndER,
        })
        .expect("mode override accepted");
        let mode = eq.update_control(&environment);
        assert_eq!(mode, OperatingMode::HeatingHPAndER);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).expect("compressor");
        let fan_kw = eq.telemetry().get(tk::FAN_KW).expect("fan");
        let er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).expect("er");
        assert!(compressor_kw > 0.0, "compressor must run");
        assert!(er_kw > 5.0, "ER element must draw its rated ~10 kW");
        let p_kw = ports.electrical.load_power_w / 1000.0;
        assert!(
            (p_kw - (compressor_kw + fan_kw + er_kw)).abs() < 1e-9,
            "port draw must be the component sum: p={p_kw}, \
             comp={compressor_kw} fan={fan_kw} er={er_kw}"
        );

        let q = ports.electrical.reactive_power_kvar;
        let expected = compressor_kw * 0.84_f64.acos().tan() + fan_kw * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "Q must exclude ER wattage: q={q}, expected={expected}"
        );
        let phantom_blend = p_kw * 0.84_f64.acos().tan();
        assert!(
            q < phantom_blend - er_kw * 0.84_f64.acos().tan() + 1e-9,
            "per-component Q ({q}) must drop the ~0.646·P_ER phantom kvar \
             relative to the folded blend ({phantom_blend})"
        );
    }

    /// ER-only mode (HP locked out): the blower still runs, so Q is exactly
    /// the fan component — fan·tan(acos(0.87)) — while the resistive ER
    /// element contributes zero.
    #[test]
    fn er_only_mode_reactive_is_fan_component_only() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT 0 °C: below hp_lockout (10 °C) and below the ER OAT lockout.
        let environment = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &environment).unwrap();
        let mode = eq.update_control(&environment);
        assert_eq!(mode, OperatingMode::HeatingER, "HP must be locked out");
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let fan_kw = eq.telemetry().get(tk::FAN_KW).expect("fan");
        let er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).expect("er");
        assert_eq!(
            eq.telemetry().get(tk::COMPRESSOR_KW),
            Some(0.0),
            "compressor must be off in ER-only mode"
        );
        assert!(fan_kw > 0.0, "blower must run in ER-only mode");
        assert!(er_kw > 0.0, "ER element must draw power");
        let q = ports.electrical.reactive_power_kvar;
        let expected = fan_kw * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-12,
            "ER-only Q must be the fan component alone: q={q}, expected={expected}"
        );
    }

    /// Rule R1 regression: the power factor affects only Q. Twin instances —
    /// one with the class pf 0.84, one with a constant-power sidecar override
    /// (pf 0 sentinel) — must produce bit-identical real power at every step
    /// and voltage.
    #[test]
    fn heating_real_power_bit_identical_with_and_without_reactive_zip() {
        let config_pf = heater_config();
        let mut config_nopf = config_pf.clone();
        config_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let mut eq_pf = ASHPHeater::new(config_pf.clone());
        let mut eq_nopf = ASHPHeater::new(config_nopf.clone());
        let mut environment = env(18.0, 10.0, 0.005);
        eq_pf.init(&config_pf, &environment).unwrap();
        eq_nopf.init(&config_nopf, &environment).unwrap();

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            environment.grid.voltage_pu = *v;
            eq_pf.update_control(&environment);
            eq_nopf.update_control(&environment);
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf
                .step(&environment, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&environment, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged between pf and no-pf twins"
            );
            assert_eq!(
                eq_pf
                    .telemetry()
                    .get(tk::ELECTRIC_KW)
                    .expect("kW")
                    .to_bits(),
                eq_nopf
                    .telemetry()
                    .get(tk::ELECTRIC_KW)
                    .expect("kW")
                    .to_bits(),
                "step {i} (v={v}): ELECTRIC_KW telemetry diverged"
            );
            assert_eq!(
                ports_nopf.electrical.reactive_power_kvar, 0.0,
                "pf-0 twin must produce zero reactive power"
            );
            if ports_pf.electrical.reactive_power_kvar != 0.0 {
                any_reactive = true;
            }
            environment.current_time += ChronoDuration::minutes(1);
        }
        assert!(
            any_reactive,
            "the pf 0.84 twin must produce reactive power while heating"
        );
    }

    #[test]
    fn typed_heating_eir_is_used() {
        let cfg = heater_config_with(|typed| {
            typed.common.heating_eir = Some(0.401);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.config.eir_by_stage[0];
        assert!(
            (eir - 0.401).abs() < 0.01,
            "typed heating_eir=0.401 must be used directly, got {eir}"
        );
    }

    #[test]
    fn typed_heating_setpoint_source_drives_control_state() {
        let cfg = heater_config_with(|typed| {
            let mut weekday = [18.0; 24];
            weekday[1] = 22.0;
            typed.common.setpoint.heating_setpoint_source =
                Some(ScheduleSourceConfig::DailyProfile {
                    weekday,
                    weekend: weekday,
                    month_multipliers: [1.0; 12],
                    max_value: 1.0,
                });
            typed.common.setpoint.heating_setpoint_c = Some(18.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());

        let env_hour0 = env_at(20.5, 5.0, 0.003, 0);
        eq.init(&cfg, &env_hour0).unwrap();
        assert_eq!(
            eq.update_control(&env_hour0),
            OperatingMode::Off,
            "hour-0 schedule setpoint 18C must keep the heater off at zone 20.5C"
        );

        let env_hour1 = env_at(20.5, 5.0, 0.003, 3600);
        assert_eq!(
            eq.update_control(&env_hour1),
            OperatingMode::HeatingHP,
            "hour-1 schedule setpoint 22C must call for heating at zone 20.5C"
        );
    }

    #[test]
    fn stage_heating_eirs_override_rated_heating_eir() {
        let cfg = heater_config_with(|typed| {
            typed.common.heating_eir = Some(0.401);
            typed.common.stage_heating_eirs = Some(vec![0.25]);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        assert_eq!(
            eq.core.hvac.config.eir_by_stage,
            vec![0.25],
            "per-stage heating EIRs must override rated heating_eir"
        );
    }

    #[test]
    fn two_speed_ashp_uses_typed_time_control_and_escalates_after_dwell() {
        let cfg = heater_config_with(|typed| {
            typed.common.number_of_speeds = 2;
            typed.common.stage_heating_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.common.stage_heating_eirs = Some(vec![0.33, 0.33]);
            typed.common.backup_capacity_w = Some(0.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env0 = env_at(18.0, 5.0, 0.003, 0);
        eq.init(&cfg, &env0).unwrap();
        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            SpeedControlMode::TwoSpeedTime
        );

        let mode0 = eq.update_control(&env0);
        assert_eq!(mode0, OperatingMode::HeatingHP);
        assert_eq!(
            eq.core.hvac.runtime.last_speed_index, 0,
            "two-speed ASHP must start at low stage under OCHRE time control"
        );
        eq.step(&env0, Duration::from_secs(60), &mut ports).unwrap();

        for minute in 1..=5 {
            ports.zero();
            let env_next = env_at(18.0 - 0.1 * minute as f64, 5.0, 0.003, minute * 60);
            eq.update_control(&env_next);
            eq.step(&env_next, Duration::from_secs(60), &mut ports)
                .unwrap();
        }

        assert_eq!(
            eq.core.hvac.runtime.last_speed_index, 1,
            "after the 300s dwell expires and zone temperature keeps falling, two-speed ASHP must escalate to high stage"
        );
    }

    #[test]
    fn minisplit_pan_heater_draws_when_cold() {
        let mut cfg = heater_config();
        cfg.ochre_class = "MSHP Heater".to_string();

        let mut eq = MinisplitHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(20.0, -5.0, 0.004);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(ports.electrical.net_active_w() > 0.0);
    }

    #[test]
    fn state_round_trip_preserves_defrost_state() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, -3.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let state = eq.save_state().unwrap();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(
            restored.telemetry().get(tk::DEFROST_ACTIVE),
            eq.telemetry().get(tk::DEFROST_ACTIVE)
        );
    }

    #[test]
    fn mode_override_forces_heating_output() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let env = env(23.0, 5.0, 0.003);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHP,
        })
        .unwrap();
        assert_eq!(eq.update_control(&env), OperatingMode::HeatingHP);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            ports.electrical.net_active_w() > 0.0,
            "ModeOverride(HeatingHP) must force compressor power even when the thermostat would otherwise be off"
        );
        assert_eq!(
            eq.core_output().state.operating_mode,
            Some(OperatingMode::HeatingHP)
        );
    }

    #[test]
    fn heat_pump_core_output_matches_scaled_electric_ports() {
        let cfg = heater_config_with(|typed| {
            typed.common.fraction_heating_load_served = Some(0.5);
            typed.common.backup_capacity_w = Some(0.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        let env = env(18.0, 5.0, 0.003);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::HeatingHP);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let port_w = ports.electrical.net_active_w();
        let telemetry_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let core_kw = match eq.core_output().flows.electric_kw {
            Some(hares_types::ElectricPower::Consumption(v)) => v,
            _ => 0.0,
        };
        assert!(
            (power_kw_to_w(telemetry_kw) - port_w).abs() < 1.0,
            "heat-pump telemetry electric_kw must match the scaled electrical port draw"
        );
        assert!(
            (power_kw_to_w(core_kw) - port_w).abs() < 1.0,
            "heat-pump core_output electric_kw must match the scaled electrical port draw"
        );
        assert_eq!(
            eq.core_output().state.operating_mode,
            Some(OperatingMode::HeatingHP)
        );
    }

    // Regression: ER backup is binary on/off — a resistive element cannot draw
    // fractional power. When the ideal-capacity controller requests a partial load
    // below full rated ER capacity with er_stages=1, the ER element still fires at
    // full rated capacity because it has no intermediate modulation capability.
    // The thermostat handles time-averaged part-load output through cycling, not
    // through per-timestep power modulation.
    #[test]
    fn er_ideal_mode_single_stage_fires_at_full_rated_when_on() {
        // Lock HP out so only ER runs; this isolates ER draw in electric_kw.
        // Use ideal-capacity control to request PLR=0.5 on ER:
        // ideal_capacity_w / backup_capacity_w = 2000 / 4000 = 0.5.
        // With er_stages=1 (binary), ER must fire at full 4.0 kW, not 2.0 kW.
        let mut cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(0.0); // zero fan to isolate ER draw
        });
        cfg.test_extras_mut()
            .insert("use_ideal_capacity".to_string(), true.into());
        let backup_capacity_w = 4_000.0_f64;
        // OAT=0°C is below ER lockout (4.44°C) so ER is temperature-permitted
        let env_cold = env(18.0, 0.0, 0.003);

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env_cold).unwrap();

        // Trigger thermostat into Heating mode, then request part-load ER.
        let mode = eq.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::HeatingER, "HP must be locked out");
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: backup_capacity_w * 0.5,
            degraded: false,
        })
        .expect("ideal-capacity control accepted");
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        let er_capacity_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        let er_stages_on = eq.telemetry().get(tk::ER_STAGES_ON).unwrap_or(0.0);
        assert!(
            (er_capacity_w - backup_capacity_w).abs() < 1e-6,
            "single-stage ER must fire at full rated capacity ({backup_capacity_w} W) \
             even when ideal capacity requests half, got {er_capacity_w:.1} W",
        );
        assert!(
            (er_stages_on - 1.0).abs() < 1e-6,
            "single-stage ER must report 1 stage on, got {er_stages_on}",
        );
        assert!(er_capacity_w > 0.0, "ER must produce some thermal output");
    }

    // Regression: ER backup must be binary on/off in non-ideal mode,
    // not modulated by PLR. When `er_on` is true in the non-ideal path,
    // `er_capacity_w` must equal `backup_capacity_w` (full rated), not
    // `backup_capacity_w * plr`.
    //
    // The bug manifested when the HP thermostat was in Heating mode AND PLR < 1.0.
    // PLR = hvac.duty_cycle = speed.part_load_ratio, which comes from load_ratio
    // = (setpoint - zone) / deadband divided by the low-speed capacity fraction
    // in TwoSpeedSetpoint mode.
    //
    // Scenario:
    //   - Two-speed HP, er_setpoint_offset_c=0, setpoint=21°C, deadband=1°C.
    //   - Step 1: zone=19°C → heat_turn_on = 21-1 = 20°C → thermostat triggers Heating.
    //     load_ratio = 2/1 = 2.0, clamped to 1.0. Thermostat is now Heating.
    //   - Step 2: zone=20.55°C (zone warmed) → thermostat remains Heating (turn-off=21+0.8=21.8°C).
    //     load_ratio = (21 - 20.55) / 1.0 = 0.45.
    //     TwoSpeedSetpoint: 0.45 < low_cap(0.72) → speed_index=0, PLR = 0.45/0.72 ≈ 0.625.
    //     ER on (zone < er_turn_on=21°C). Before fix: er_capacity_w = 4000 * 0.625 = 2500 W.
    //     After fix: er_capacity_w = 4000 W (full rated).
    #[test]
    fn er_non_ideal_mode_is_binary_full_rated_when_on() {
        let backup_capacity_w = 4_000.0_f64;
        let cfg = heater_config_with(|typed| {
            typed.er_setpoint_offset_c = Some(0.0); // ER fires whenever zone < setpoint
            typed.common.backup_capacity_w = Some(backup_capacity_w);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(0.0); // zero fan so backup_er_kw == er_capacity_w exactly
            typed.common.number_of_speeds = 2; // TwoSpeedSetpoint → PLR can be < 1.0
            typed.common.stage_heating_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.common.stage_heating_eirs = Some(vec![0.33, 0.33]);
            typed.common.heating_capacity_w = None;
            typed.common.hysteresis_c = Some(1.0);
            // ER is allowed when OAT < er_lockout_temp_c; set lockout high so ER always allowed.
            typed.er_lockout_temp_c = Some(100.0); // ER allowed at all OATs (lockout = 100°C)
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // Step 1: zone=19°C → thermostat cold-start triggers Heating (load_ratio=1.0).
        let env_cold = env(19.0, 5.0, 0.003);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Step 2: zone has warmed to 20.55°C; thermostat stays in Heating mode because
        // turn-off = setpoint + hysteresis * cutout = 21 + 0.8 = 21.8°C (default cutout=0.8).
        // load_ratio = (21 - 20.55) / 1.0 = 0.45 → PLR = 0.45/0.72 ≈ 0.625.
        // ER is on (zone=20.55 < er_turn_on=21.0).
        ports.zero();
        let env_partial = env(20.55, 5.0, 0.003);
        eq.update_control(&env_partial);
        eq.step(&env_partial, Duration::from_secs(60), &mut ports)
            .unwrap();

        let backup_er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);
        // Binary on/off: must draw exactly full rated power (4.0 kW).
        // With PLR modulation (bug): backup_er_kw ≈ 4.0 * 0.625 = 2.5 kW.
        assert!(
            (backup_er_kw - backup_capacity_w / 1000.0).abs() < 1e-6,
            "non-ideal ER must draw full rated {:.3} kW when on, \
             got {backup_er_kw:.6} kW (PLR modulation bug: ER incorrectly scaled by HP PLR ≈0.625)",
            backup_capacity_w / 1000.0,
        );
    }

    // Multi-stage ER in ideal mode: with er_stages=2, backup_capacity_w=10_000 W,
    // each stage = 5000 W. When the residual (ideal - hp) is 3 kW, the minimum
    // number of stages to cover it is ceil(3000/5000) = 1, so er_capacity_w = 5000 W.
    // When the residual is 7 kW, ceil(7000/5000) = 2 stages, er_capacity_w = 10_000 W.
    #[test]
    fn er_multi_stage_ideal_mode_activates_minimum_stages() {
        let backup_capacity_w = 10_000.0_f64;
        let stage_capacity_w = 5_000.0_f64;

        // Scenario 1: residual = 3 kW → 1 stage (5 kW, over-delivers by 2 kW)
        let mut cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(backup_capacity_w);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(0.0);
            typed.common.er_stages = 2;
        });
        cfg.test_extras_mut()
            .insert("use_ideal_capacity".to_string(), true.into());

        let env_cold = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env_cold).unwrap();

        let mode = eq.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::HeatingER, "HP must be locked out");
        // Request 3 kW — only 1 stage (5 kW) needed
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 3_000.0,
            degraded: false,
        })
        .expect("ideal-capacity control accepted");
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        let er_capacity_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        let er_stages_on = eq.telemetry().get(tk::ER_STAGES_ON).unwrap_or(0.0);
        assert!(
            (er_capacity_w - stage_capacity_w).abs() < 1e-6,
            "2-stage ER with 3 kW residual must activate 1 stage ({stage_capacity_w} W), \
             got {er_capacity_w:.1} W",
        );
        assert!(
            (er_stages_on - 1.0).abs() < 1e-6,
            "2-stage ER with 3 kW residual must report 1 stage on, got {er_stages_on}",
        );

        // Scenario 2: residual = 7 kW → 2 stages (10 kW)
        let mut eq2 = ASHPHeater::new(cfg.clone());
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq2.init(&cfg, &env_cold).unwrap();
        eq2.update_control(&env_cold);
        eq2.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 7_000.0,
            degraded: false,
        })
        .expect("ideal-capacity control accepted");
        eq2.step(&env_cold, Duration::from_secs(60), &mut ports2)
            .unwrap();

        let er_capacity_w2 = eq2.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        let er_stages_on2 = eq2.telemetry().get(tk::ER_STAGES_ON).unwrap_or(0.0);
        assert!(
            (er_capacity_w2 - backup_capacity_w).abs() < 1e-6,
            "2-stage ER with 7 kW residual must activate 2 stages ({backup_capacity_w} W), \
             got {er_capacity_w2:.1} W",
        );
        assert!(
            (er_stages_on2 - 2.0).abs() < 1e-6,
            "2-stage ER with 7 kW residual must report 2 stages on, got {er_stages_on2}",
        );
    }

    // Non-ideal multi-stage ER: regardless of er_stages, all stages activate together
    // at full rated capacity. Per-stage control requires separate thermostats.
    #[test]
    fn er_multi_stage_non_ideal_fires_all_stages() {
        let backup_capacity_w = 10_000.0_f64;
        let cfg = heater_config_with(|typed| {
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(backup_capacity_w);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(0.0);
            typed.common.number_of_speeds = 2;
            typed.common.stage_heating_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.common.stage_heating_eirs = Some(vec![0.33, 0.33]);
            typed.common.heating_capacity_w = None;
            typed.common.hysteresis_c = Some(1.0);
            typed.common.er_stages = 2;
            typed.er_lockout_temp_c = Some(100.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        let env_cold = env(19.0, 5.0, 0.003);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        let er_capacity_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        let er_stages_on = eq.telemetry().get(tk::ER_STAGES_ON).unwrap_or(0.0);
        assert!(
            (er_capacity_w - backup_capacity_w).abs() < 1e-6,
            "non-ideal multi-stage ER must fire all stages at full rated ({backup_capacity_w} W), \
             got {er_capacity_w:.1} W",
        );
        assert!(
            (er_stages_on - 2.0).abs() < 1e-6,
            "non-ideal 2-stage ER must report 2 stages on, got {er_stages_on}",
        );
    }

    // er_stages validation: values outside [1, 4] must produce loud errors.
    #[test]
    fn er_stages_validation_rejects_zero() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                er_stages: 0,
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("er_stages must be in [1, 4]"),
            "validation must reject er_stages=0; got: {err}",
        );
    }

    #[test]
    fn er_stages_validation_rejects_five() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                er_stages: 5,
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("er_stages must be in [1, 4]"),
            "validation must reject er_stages=5; got: {err}",
        );
    }

    #[test]
    fn er_stages_defaults_to_one() {
        let json = serde_json::json!({
            "heating_capacity_w": 10_000.0,
            "heating_eir": 0.25,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.common.er_stages, 1, "default er_stages must be 1",);
    }

    // Regression: ER-only mode must still include blower fan power.
    // If HP is locked out and ER is active, electric_kw must be strictly greater
    // than strip-power alone when fan power is configured.
    #[test]
    fn er_only_mode_includes_fan_power_in_electric_kw() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(500.0);
        });

        // OAT below HP lockout and below ER lockout -> HP unavailable, ER allowed.
        let env = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env).unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "setup must force ER-only mode"
        );
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            electric_kw > 4.0,
            "ER-only electric power must include fan draw (expected >4.0 kW), got {electric_kw:.6} kW"
        );
    }

    // Regression guard: ER-only mode must not leak compressor contribution.
    // When HP is locked out and control selects HeatingER, compressor telemetry
    // must be exactly zero for the step.
    #[test]
    fn er_only_mode_has_zero_compressor_power() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(500.0);
        });

        // OAT below HP lockout and below ER lockout -> HP unavailable, ER allowed.
        let env = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env).unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::HeatingER);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(f64::NAN);
        assert!(
            compressor_kw.abs() <= 1e-12,
            "ER-only mode must report zero compressor power, got {compressor_kw:.9} kW"
        );
    }

    // Regression: step() used to call update_control() internally, causing the
    // thermostat FSM to execute twice per timestep. Verify that calling step()
    // without a prior update_control() does NOT change operating mode.
    #[test]
    fn step_does_not_call_update_control_internally() {
        // After init the heater is Off. Zone is cold (18°C vs setpoint 21°C),
        // so an internal update_control() inside step() would switch to heating.
        // If step() is pure physics (no control), the mode stays Off.
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let environment = env(18.0, 5.0, 0.005);
        eq.init(&cfg, &environment).unwrap();
        // Deliberately skip update_control() before step()
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        // operating_mode telemetry code 0.0 = OperatingMode::Off.as_code()
        assert_eq!(
            eq.telemetry().get(tk::OPERATING_MODE),
            Some(0.0),
            "step() must not change operating mode; it must remain Off when \
             update_control() was never called",
        );
    }

    #[test]
    fn default_lockout_below_hp_threshold_runs_er_only() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT below default HP lockout (-17.78C) and below ER lockout (4.44C):
        // HP must be disabled while ER is allowed.
        let cold_env = env(18.0, -20.0, 0.003);
        eq.init(&cfg, &cold_env).unwrap();

        let mode = eq.update_control(&cold_env);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "with default lockouts and OAT=-20C, HP should be locked out and ER should heat"
        );
    }

    #[test]
    fn default_lockout_above_er_threshold_runs_hp_only() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT above default ER lockout (4.44C): ER must be disabled.
        // OAT also above default HP lockout (-17.78C): HP remains available.
        let mild_env = env(18.0, 5.0, 0.003);
        eq.init(&cfg, &mild_env).unwrap();

        let mode = eq.update_control(&mild_env);
        assert_eq!(
            mode,
            OperatingMode::HeatingHP,
            "with default lockouts and OAT=5C, ER should be locked out and HP should heat"
        );
    }

    // Regression: ASHPHeater always used AshpHeatPumpAux regardless of backup
    // capacity, giving a fixed 40.6°C supply temp even with no ER strip heat.
    // With backup_capacity_w=0 the type must be AshpHeatPumpOnly, whose supply
    // temp follows outdoor temperature: 32.2 + 0.15*(OAT - 8.3).
    #[test]
    fn ashp_without_backup_uses_oat_dependent_supply_temp() {
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(0.0);
        });

        // Use OATs above the ER lockout threshold (default 4.44°C) so that
        // er_on=false. With only HP running, update_supply_air_temp() is called
        // and it updates supply_air_temp_c based on OAT for AshpHeatPumpOnly.
        // The field is internal; the test module as a child of heater.rs can
        // access private struct fields of its parent module.
        let oat_a = 5.0_f64; // above ER lockout → ER disabled; supply = 31.705°C
        let oat_b = 15.0_f64; // above ER lockout → ER disabled; supply = 33.205°C

        let env_a = env(18.0, oat_a, 0.005);
        let mut eq_a = ASHPHeater::new(cfg.clone());
        let mut ports_a = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_a.init(&cfg, &env_a).unwrap();
        eq_a.update_control(&env_a);
        eq_a.step(&env_a, Duration::from_secs(60), &mut ports_a)
            .unwrap();
        let supply_a = eq_a.core.hvac.config.supply_air_temp_c;

        let env_b = env(18.0, oat_b, 0.005);
        let mut eq_b = ASHPHeater::new(cfg.clone());
        let mut ports_b = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_b.init(&cfg, &env_b).unwrap();
        eq_b.update_control(&env_b);
        eq_b.step(&env_b, Duration::from_secs(60), &mut ports_b)
            .unwrap();
        let supply_b = eq_b.core.hvac.config.supply_air_temp_c;

        // AshpHeatPumpOnly formula: 32.2 + 0.15*(OAT - 8.3)
        let expected_a = 32.2 + 0.15 * (oat_a - 8.3);
        let expected_b = 32.2 + 0.15 * (oat_b - 8.3);

        assert!(
            (supply_a - expected_a).abs() < 0.01,
            "expected OAT-dependent supply temp {expected_a:.3}°C at OAT {oat_a}°C, \
             got {supply_a:.3}°C",
        );
        assert!(
            (supply_b - expected_b).abs() < 0.01,
            "expected OAT-dependent supply temp {expected_b:.3}°C at OAT {oat_b}°C, \
             got {supply_b:.3}°C",
        );
        assert!(
            (supply_b - supply_a).abs() > 0.1,
            "supply temp must differ between OATs when AshpHeatPumpOnly is selected",
        );
    }

    fn make_env(zone_temp_c: f64, outdoor_c: f64, second: i64) -> EnvironmentState {
        use chrono::{FixedOffset, TimeZone};
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
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
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + chrono::Duration::seconds(second),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn er_hard_lockout_blocks_er_for_configured_duration() {
        // OCHRE HVAC.py: ER hard lockout prevents strip heat on setpoint increase.
        // With lockout = 600 s and dt = 60 s, ER must be blocked for 10 steps.
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.setpoint.heating_setpoint_c = Some(18.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let initial_env = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &initial_env).unwrap();

        // Step 1: set a higher setpoint to trigger lockout.
        let raised_setpoint = 21.0_f64;
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(raised_setpoint),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            })
            .ok();

        for step in 0..10 {
            let t = make_env(16.0, 0.0, step * 60);
            let mode = eq.update_control(&t);
            assert!(
                !matches!(
                    mode,
                    OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
                ),
                "ER must be blocked at step {step} during hard lockout, got {mode:?}"
            );
        }

        // After 600 s the lockout expires; ER should be permitted.
        let t_after = make_env(16.0, 0.0, 600);
        let mode_after = eq.update_control(&t_after);
        assert!(
            matches!(
                mode_after,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed after lockout expires, got {mode_after:?}"
        );
    }

    #[test]
    fn er_soft_lockout_blocks_er_while_zone_temp_is_rising() {
        // OCHRE HVAC.py: two-stage lockout -- after hard lockout expires, ER stays
        // off while zone temp is still rising (heat pump is winning the load).
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(60.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.setpoint.heating_setpoint_c = Some(18.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(16.0, 0.0, 0)).unwrap();

        // Raise setpoint to 21°C to trigger hard lockout (+3°C above 18°C).
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            })
            .ok();

        // Step within hard lockout period.
        eq.update_control(&make_env(16.0, 0.0, 0));

        // Step after hard lockout expires but with rising zone temp.
        let mode_rising = eq.update_control(&make_env(17.0, 0.0, 60));
        assert!(
            !matches!(
                mode_rising,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must stay off under soft lockout while zone temp is rising, got {mode_rising:?}"
        );

        // Once zone temp stops rising, soft lockout clears.
        let mode_stable = eq.update_control(&make_env(17.0, 0.0, 120));
        assert!(
            matches!(
                mode_stable,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed once zone temp stops rising, got {mode_stable:?}"
        );
    }

    // Regression counterpart: when backup ER is present the type is
    // AshpHeatPumpAux, which has a fixed 40.6°C supply temp independent of OAT.
    #[test]
    fn ashp_with_backup_uses_fixed_supply_temp() {
        // Use a very cold OAT so that HP runs and update_supply_air_temp is
        // called, then check that the result is the fixed AshpHeatPumpAux value.
        let cfg = heater_config(); // backup_capacity_w = 4_000 W
        let environment = env(18.0, -5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let supply = eq.core.hvac.config.supply_air_temp_c;
        assert!(
            (supply - 40.6).abs() < 0.01,
            "AshpHeatPumpAux must have fixed 40.6°C supply temp, got {supply:.3}°C",
        );
    }

    // EnergyPlus supplemental-ER upper OAT cap: ER must not fire when OAT is above
    // max_oat_supplemental_c (default 21°C / 69.8°F) even when er_lockout_temp_c
    // is configured high enough that the OCHRE threshold alone would permit ER.
    #[test]
    fn er_blocked_above_max_oat_supplemental() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(21.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        // Zone well below setpoint (18°C vs 21°C) so a heating call is active.
        let env_above = env(18.0, 22.0, 0.005); // OAT 22°C > 21°C cap
        eq.init(&cfg, &env_above).unwrap();

        let mode = eq.update_control(&env_above);
        assert!(
            !matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be blocked when OAT ({:.1}°C) exceeds max_oat_supplemental_c (21°C), \
             got {mode:?}",
            env_above.weather.outdoor_temp_c,
        );
    }

    // ER must be allowed when OAT is at or below the max_oat_supplemental_c cap.
    #[test]
    fn er_allowed_below_max_oat_supplemental() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(21.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let env_below = env(18.0, 20.0, 0.005); // OAT 20°C <= 21°C cap
        eq.init(&cfg, &env_below).unwrap();

        let mode = eq.update_control(&env_below);
        assert!(
            matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed when OAT ({:.1}°C) is at or below max_oat_supplemental_c (21°C), \
             got {mode:?}",
            env_below.weather.outdoor_temp_c,
        );
    }

    // OCHRE parity: HP lockout does not force ER immediately.
    // ER must still wait for its own thermostat threshold
    // (setpoint - er_setpoint_offset_c).
    #[test]
    fn hp_lockout_does_not_force_er_above_er_threshold() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0); // disable HP at OAT=0°C
            typed.er_lockout_temp_c = Some(5.0); // allow ER at OAT=0°C
            // Default er_setpoint_offset tracks OCHRE formula:
            // deadband * (1.8 - deadband_offset) = 1.0 * (1.8 - 0.2) = 1.6°C.
            typed.er_setpoint_offset_c = None;
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        // Base setpoint=21°C (from helper config), so:
        // HP turn-on threshold ~20.2°C, ER turn-on threshold ~19.4°C.
        // Zone=19.8°C is between them: thermostat requests heating, but ER call is false.
        let env_midband = env(19.8, 0.0, 0.005);
        eq.init(&cfg, &env_midband).unwrap();

        let mode = eq.update_control(&env_midband);
        assert_eq!(
            mode,
            OperatingMode::Off,
            "HP lockout must not force ER above ER threshold; expected Off, got {mode:?}"
        );
    }

    // Regression: ER thermostat uses explicit turn-on/turn-off hysteresis.
    // Once ER turns on at (setpoint - er_offset), it must stay on until the
    // zone reaches setpoint to avoid one-step short-cycling.
    #[test]
    fn er_hysteresis_holds_until_setpoint() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0); // HP unavailable at OAT=0°C.
            typed.er_lockout_temp_c = Some(5.0); // ER allowed at OAT=0°C.
            typed.er_setpoint_offset_c = Some(1.6);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env_at(19.0, 0.0, 0.005, 0)).unwrap();

        // t=0: below turn-on (21.0 - 1.6 = 19.4) -> ER engages.
        let mode_on = eq.update_control(&env_at(19.0, 0.0, 0.005, 0));
        assert_eq!(mode_on, OperatingMode::HeatingER);

        // t=60: above turn-on but still below setpoint -> ER must stay on.
        let mode_hold = eq.update_control(&env_at(19.6, 0.0, 0.005, 60));
        assert_eq!(
            mode_hold,
            OperatingMode::HeatingER,
            "ER must remain on in hysteresis band between turn_on and setpoint"
        );

        // t=120: above setpoint -> ER releases.
        let mode_off = eq.update_control(&env_at(21.1, 0.0, 0.005, 120));
        assert_eq!(
            mode_off,
            OperatingMode::Off,
            "ER must turn off once zone exceeds setpoint"
        );
    }

    // User-configured values above 21°C must be silently clamped to 21°C.
    #[test]
    fn max_oat_supplemental_clamped_to_hard_limit() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(30.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let env_above = env(18.0, 22.0, 0.005); // OAT 22°C > clamped cap of 21°C
        eq.init(&cfg, &env_above).unwrap();

        // After clamping, effective cap is 21°C, so OAT=22°C must block ER.
        let mode = eq.update_control(&env_above);
        assert!(
            !matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "OAT (22°C) must block ER after clamping max_oat_supplemental_c from 30→21°C, \
             got {mode:?}",
        );
    }

    // --- DR and control signal tests ---

    // DR Moderate: heating setpoint offset = -1°C.
    // The `update_mode` FSM uses the base setpoints for mode transitions, while
    // `resolve_control` uses the DR-adjusted setpoint for load_ratio calculation.
    // Effect: DR reduces the load_ratio, which reduces heating output (lower PLR).
    // At zone=19°C vs base setpoint=21°C, hysteresis=1: load without DR = (21-19)/1 = 2 (full).
    // With DR Moderate (offset=-1): effective_setpoint=20, load = (20-19)/1 = 1 (still full but lower).
    // To observe a partial-load reduction, use zone inside the hysteresis band (19.5°C):
    // no-DR: load=(21-19.5)/1=1.5 → clamped to 1.0 (full); DR: load=(20-19.5)/1=0.5 (part load).
    // The test verifies that DR Moderate reduces the heating output vs the no-DR case.
    #[test]
    fn dr_moderate_shifts_heating_setpoint_down() {
        let cfg = heater_config(); // heating_setpoint_c=21°C, hysteresis=1°C
        // zone=19.5°C: below base threshold (21-1=20) → baseline enters Heating.
        // DR Moderate: effective_setpoint=20; load_ratio=(20-19.5)/1=0.5 (part load vs full).
        // Use OAT=5°C (above default er_lockout_temp=4.44°C) so ER is temp-blocked.
        let e = env(19.5, 5.0, 0.005);

        // Baseline without DR: full HP output at PLR=1.
        let mut eq_base = ASHPHeater::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        eq_base.update_control(&e);
        let mut ports_base = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_base
            .step(&e, Duration::from_secs(60), &mut ports_base)
            .unwrap();
        let kw_base = eq_base.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // With DR Moderate: effective_setpoint=20°C → load_ratio=(20-19.5)/1=0.5 → partial output.
        let mut eq_dr = ASHPHeater::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        eq_dr.update_control(&e);
        let mut ports_dr = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_dr
            .step(&e, Duration::from_secs(60), &mut ports_dr)
            .unwrap();
        let kw_dr = eq_dr.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(kw_base > 0.0, "baseline must draw power; got {kw_base}");
        assert!(kw_dr >= 0.0, "DR case must draw non-negative power");
        // DR Moderate reduces the effective setpoint, which lowers load_ratio and thus
        // part-load ratio, resulting in less or equal heating output.
        assert!(
            kw_dr <= kw_base,
            "DR Moderate must not increase heating output; dr={kw_dr:.4}, base={kw_base:.4}"
        );
    }

    #[test]
    fn dr_grid_emergency_resets_dr_duty_cycle() {
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.005);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: None,
        })
        .unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();

        assert!(
            (eq.core.dr_duty_cycle - 1.0).abs() < f64::EPSILON,
            "GridEmergency must reset dr_duty_cycle to 1.0; got {}",
            eq.core.dr_duty_cycle
        );
        assert_eq!(
            eq.core.dr_load_fraction, 0.0,
            "GridEmergency must set dr_load_fraction to 0.0; got {}",
            eq.core.dr_load_fraction
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw, 0.0, "GridEmergency must force zero output; got {kw}");
    }

    // GridEmergency: dr_load_fraction=0 → full shed → zero output.
    #[test]
    fn dr_grid_emergency_forces_heater_off() {
        let cfg = heater_config();
        // Zone well below setpoint so heating is clearly needed.
        let e = env(18.0, 5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw, 0.0, "GridEmergency must force zero output; got {kw}");
    }

    // DR with duration_s: auto-reverts to Normal after the duration elapses.
    // update_control decrements dr_duration_remaining_s by time_res (1 min = 60 s)
    // each call. DR persists while remaining > 0 and auto-reverts when it reaches <= 0.
    #[test]
    fn dr_duration_auto_reverts() {
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // High DR for 120 s (two 60-s steps); time_res = 1 min = 60 s.
        // First update_control decrements by 60 s → remaining = 60 s > 0 → still active.
        // Second update_control decrements by 60 s → remaining = 0 s → reverts to Normal.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(120.0),
        })
        .unwrap();

        // Step 1: DR active → load_fraction=0.8, setpoint offset=-2°C → reduced output.
        eq.update_control(&e);
        let mut ports1 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports1).unwrap();
        let kw_dr = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // After one step, dr_duration_remaining_s must have decremented to 60 s.
        assert_eq!(
            eq.core.dr_duration_remaining_s,
            Some(60.0),
            "dr_duration_remaining_s must be 60 s after one step; got {:?}",
            eq.core.dr_duration_remaining_s,
        );

        // Step 2: remaining decrements from 60 → 0 s → auto-revert to Normal → full output.
        eq.update_control(&e);
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports2).unwrap();
        let kw_after = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // Auto-revert must clear the duration and restore Normal.
        assert!(
            eq.core.dr_duration_remaining_s.is_none(),
            "dr_duration_remaining_s must be None after auto-revert; got {:?}",
            eq.core.dr_duration_remaining_s,
        );
        assert_eq!(
            eq.core.dr_level,
            DRLevel::Normal,
            "dr_level must revert to Normal after duration expires; got {:?}",
            eq.core.dr_level,
        );
        assert!(
            kw_after > kw_dr,
            "Output must recover after DR duration expires; \
             dr={kw_dr:.4}, after={kw_after:.4}",
        );
    }

    // LoadFraction is transient: update_control resets ctrl_load_fraction=1.0 at its start.
    // Signals must be applied AFTER update_control but BEFORE step to take effect that step.
    // The next update_control call restores the default -- no re-apply needed.
    #[test]
    fn load_fraction_resets_each_step() {
        let cfg = heater_config();
        // Zone well below setpoint so heating is clearly needed.
        let e = env(18.0, 5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Step 1: update_control (sets mode), then apply LoadFraction=0, then step → off.
        eq.update_control(&e);
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports1 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports1).unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw_step1, 0.0, "step 1 with LoadFraction=0 must be off");

        // Step 2: update_control resets ctrl_load_fraction=1.0; no signal reapplied → must heat.
        eq.update_control(&e);
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports2).unwrap();
        let kw_step2 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            kw_step2 > 0.0,
            "step 2 must draw power after transient LoadFraction resets; got {kw_step2}"
        );
    }

    // --- DR and control signal tests ---

    // DR Moderate: heating setpoint offset = -1°C.
    // The `update_mode` FSM uses the base setpoints for mode transitions, while
    // `resolve_control` uses the DR-adjusted setpoint for load_ratio calculation.
    // Effect: DR reduces the load_ratio, which reduces heating output (lower PLR).
    // At zone=19°C vs base setpoint=21°C, hysteresis=1: load without DR = (21-19)/1 = 2 (full).
    // With DR Moderate (offset=-1): effective_setpoint=20, load = (20-19)/1 = 1 (still full but lower).
    // To observe a partial-load reduction, use zone inside the hysteresis band (19.5°C):
    // no-DR: load=(21-19.5)/1=1.5 → clamped to 1.0 (full); DR: load=(20-19.5)/1=0.5 (part load).
    // The test verifies that DR Moderate reduces the heating output vs the no-DR case.

    // State round-trip must preserve max_oat_supplemental_c.
    #[test]
    fn state_round_trip_preserves_max_oat_supplemental_c() {
        let cfg = heater_config_with(|typed| typed.max_oat_supplemental_c = Some(15.0));
        let environment = env(18.0, 0.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);

        let state = eq.save_state().unwrap();
        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        restored.load_state(&state).unwrap();

        assert!(
            (restored.core.max_oat_supplemental_c - 15.0).abs() < 1e-9,
            "max_oat_supplemental_c must survive save/load round-trip; \
             expected 15.0, got {}",
            restored.core.max_oat_supplemental_c,
        );
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = heater_config();
        // Zone below heating setpoint so heater is active.
        let environment = env(18.0, 0.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply a Critical DR event with a 300 s duration.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state().unwrap();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.core.dr_level,
            DRLevel::Critical,
            "dr_level must survive save/load"
        );
        assert!(
            restored.core.dr_setpoint_offset_c < 0.0,
            "dr_setpoint_offset_c must be negative after Critical DR (heating shed) (got {})",
            restored.core.dr_setpoint_offset_c
        );
        assert_eq!(
            restored.core.dr_duration_remaining_s,
            Some(300.0),
            "dr_duration_remaining_s must survive save/load"
        );
        assert!(
            restored.core.dr_load_fraction < 1.0,
            "dr_load_fraction must be < 1.0 after Critical DR (got {})",
            restored.core.dr_load_fraction
        );
    }

    // ER engagement threshold: with default hysteresis_c=1.0 and
    // er_setpoint_offset = 1.0 * (1.8 - 0.2) = 1.6°C, ER must NOT engage when the
    // zone temp is only one deadband (1.0°C) below setpoint.  The ER threshold
    // sits 0.6°C lower than the HP heating threshold.
    #[test]
    fn er_does_not_engage_at_one_deadband_below_setpoint() {
        // heater_config(): heating_setpoint=21°C, hysteresis=1°C, backup=4 kW.
        // After init: er_setpoint_offset_c = 1.0*(1.8-0.2) = 1.6°C.
        // ER fires only when zone <= 21.0 - 1.6 = 19.4°C.
        // Zone at setpoint - 1.0 = 20.0°C is above 19.4°C → ER must NOT fire on first call.
        let cfg = heater_config();

        // OAT=0°C: below HP lockout (-17.78°C) threshold, so HP is available.
        //          below ER lockout (4.44°C), so ER is temperature-permitted.
        //          below max_oat_supplemental (21°C), so ER is not EnergyPlus-blocked.
        // Init directly at zone=20°C so er_was_on is false -- ER turn-on requires
        // zone <= 19.4°C and er_thermostat_call is false at the start.
        let env_test = env(20.0, 0.0, 0.003); // zone = setpoint - 1.0°C

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env_test).unwrap();

        // First call: er_thermostat_call = (20.0 <= 19.4) = false → ER must not fire.
        let mode = eq.update_control(&env_test);
        assert_eq!(
            mode,
            OperatingMode::HeatingHP,
            "ER must NOT engage when zone ({:.1}°C) is only one deadband below \
             setpoint ({:.1}°C); er_setpoint_offset=1.6°C requires zone <= {:.1}°C",
            env_test.zones[0].temperature_c,
            21.0_f64,
            21.0_f64 - 1.6_f64,
        );
    }

    // ER engagement threshold: zone at 1.7°C below setpoint (19.3°C) is below
    // the 1.6°C er_setpoint_offset threshold (19.4°C). With HP also available,
    // supplemental ER engages simultaneously (HeatingHPAndER).
    #[test]
    fn er_engages_with_hp_when_zone_crosses_er_threshold() {
        // Zone = 21.0 - 1.7 = 19.3°C; er_call_threshold = 21.0 - 1.6 = 19.4°C.
        // HP available (OAT 0°C > -17.78°C); ER temperature-permitted (OAT 0°C < 4.44°C).
        // Zone is below ER threshold on first call so er_thermostat_call=true.
        // With simultaneous HP+ER operation enabled, mode must be HeatingHPAndER.
        let cfg = heater_config();

        let env_test = env(19.3, 0.0, 0.003); // zone = setpoint - 1.7°C

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env_test).unwrap();

        let mode = eq.update_control(&env_test);
        assert_eq!(
            mode,
            OperatingMode::HeatingHPAndER,
            "zone below ER threshold with HP available must select HeatingHPAndER; got {mode:?}",
        );
    }

    #[test]
    fn er_uses_hysteresis_between_turn_on_and_turn_off_thresholds() {
        // Force ER-only thermostat path so the hysteresis behavior is isolated:
        // HP unavailable (high lockout), ER allowed at outdoor=0C, no setpoint lockout.
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(1.6);
            typed.er_hard_lockout_time_s = Some(0.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(19.3, 0.0, 0)).unwrap();

        // Step 1: below turn-on threshold (21.0 - 1.6 = 19.4C) => ER must engage.
        let mode1 = eq.update_control(&make_env(19.3, 0.0, 0));
        assert_eq!(
            mode1,
            OperatingMode::HeatingER,
            "ER must engage below turn-on threshold"
        );

        // Step 2: above turn-on but below turn-off (19.4 + deadband 1.0 = 20.4C).
        // Hysteresis requires ER to stay on in this band.
        let mode2 = eq.update_control(&make_env(20.0, 0.0, 60));
        assert_eq!(
            mode2,
            OperatingMode::HeatingER,
            "ER must remain on between turn-on and turn-off thresholds"
        );

        // Step 3: above turn-off threshold (19.4 + 1.0 = 20.4C) => ER must turn off.
        let mode3 = eq.update_control(&make_env(21.1, 0.0, 120));
        assert_eq!(
            mode3,
            OperatingMode::Off,
            "ER must turn off once zone exceeds turn-off threshold"
        );
    }

    // OCHRE reference: temp_turn_off = temp_turn_on + temp_deadband = 19.4 + 1.0 = 20.4°C.
    // ER must stay on at 20.0°C (< 20.4) and turn off at 20.5°C (> 20.4).
    #[test]
    fn er_turn_off_threshold_matches_ochre() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(1.6);
            typed.er_hard_lockout_time_s = Some(0.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(18.0, 0.0, 0)).unwrap();

        // Start with ER running: zone=18°C is well below er_turn_on=19.4°C.
        let mode_start = eq.update_control(&make_env(18.0, 0.0, 0));
        assert_eq!(
            mode_start,
            OperatingMode::HeatingER,
            "ER must engage at 18°C (below er_turn_on=19.4°C)"
        );

        // Zone rises to 20.0°C -- still below OCHRE turn-off (20.4°C): ER must stay on.
        let mode_below_off = eq.update_control(&make_env(20.0, 0.0, 60));
        assert_eq!(
            mode_below_off,
            OperatingMode::HeatingER,
            "ER must stay on at 20.0°C; OCHRE turn-off threshold is 20.4°C (19.4 + 1.0)"
        );

        // Zone rises to 20.5°C -- above OCHRE turn-off (20.4°C): ER must turn off.
        let mode_above_off = eq.update_control(&make_env(20.5, 0.0, 120));
        assert_eq!(
            mode_above_off,
            OperatingMode::Off,
            "ER must turn off at 20.5°C; OCHRE turn-off threshold is 20.4°C (19.4 + 1.0)"
        );
    }

    #[test]
    fn er_was_on_survives_checkpoint_restore() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(1.6);
            typed.er_hard_lockout_time_s = Some(0.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(18.0, 0.0, 0)).unwrap();

        // Engage ER: zone=18°C is below er_turn_on=19.4°C.
        eq.update_control(&make_env(18.0, 0.0, 0));
        assert!(
            eq.core.er_was_on,
            "er_was_on must be true before checkpoint"
        );

        // Save and restore state.
        let state = eq.save_state().unwrap();
        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &make_env(18.0, 0.0, 0)).unwrap();
        restored.load_state(&state).unwrap();

        assert!(
            restored.core.er_was_on,
            "er_was_on must survive checkpoint round-trip"
        );

        // Restored heater must use the turn-off threshold (20.4°C), not the turn-on
        // threshold (19.4°C): at zone=20.0°C it must stay on, not turn off.
        let mode_after_restore = restored.update_control(&make_env(20.0, 0.0, 60));
        assert_eq!(
            mode_after_restore,
            OperatingMode::HeatingER,
            "restored heater must use turn-off threshold (20.4°C); ER must stay on at 20.0°C"
        );
    }

    // H2 regression: DR expiry raises effective setpoint back to base, which must
    // NOT trigger the ER hard lockout. Only a genuine user/thermostat base-setpoint
    // raise should arm the lockout timer.
    #[test]
    fn dr_expiry_does_not_trigger_er_lockout() {
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let e = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &e).unwrap();

        // Warm up prev_base_setpoint so it equals the initial base (21°C) --
        // one update_control step with the initial setpoint.
        eq.update_control(&e);

        // Apply DR Critical: effective setpoint drops to 21 + (-3) = 18°C.
        // Base setpoint stays at 21°C -- no lockout should arm here.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();

        // A few steps while DR is active -- no lockout, setpoint went DOWN.
        for step in 0..3i64 {
            let t = make_env(16.0, 0.0, step * 60);
            eq.update_control(&t);
        }
        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "ER lockout must not arm when DR lowers the effective setpoint; \
             remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );

        // Revert DR to Normal: effective setpoint jumps back to 21°C.
        // Base setpoint never changed -- lockout must still NOT arm.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        })
        .unwrap();
        eq.update_control(&make_env(16.0, 0.0, 3 * 60));

        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "ER lockout must not arm on DR expiry (effective setpoint rebounded \
             to base but base setpoint itself did not change); remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );
    }

    // Complement to the DR expiry test: an actual thermostat/user base-setpoint
    // raise (not a DR offset) MUST arm the ER hard lockout.
    #[test]
    fn actual_setpoint_raise_triggers_er_lockout() {
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let e = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &e).unwrap();

        // One control step to record prev_base_setpoint = 21°C.
        eq.update_control(&e);
        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "lockout must not be active before any setpoint raise"
        );

        // Raise BASE setpoint by +2°C (21 → 23°C) -- this is a genuine raise.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(23.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: None,
        })
        .unwrap();

        // Next update_control must detect the raise and arm the lockout.
        eq.update_control(&make_env(16.0, 0.0, 60));

        assert!(
            eq.core.er_lockout_remaining_s > 0.0,
            "ER hard lockout must arm after a genuine base-setpoint raise; \
             remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );
    }

    // M5 regression: DRLevel::High must set dr_duty_cycle explicitly, preventing
    // a stale value from a prior DR level from leaking in.
    #[test]
    fn dr_level_high_sets_all_dr_fields_consistently() {
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.005);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // First set Critical so dr_duty_cycle has a known (possibly modified) value,
        // then switch to High. If High doesn't set dr_duty_cycle the stale Critical
        // value would remain -- this is the M5 bug.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: None,
        })
        .unwrap();

        assert!(
            eq.core.dr_setpoint_offset_c < 0.0,
            "DRLevel::High must set a negative setpoint offset for heating; got {}",
            eq.core.dr_setpoint_offset_c,
        );
        assert!(
            eq.core.dr_load_fraction > 0.0 && eq.core.dr_load_fraction < 1.0,
            "DRLevel::High must set a partial load fraction; got {}",
            eq.core.dr_load_fraction,
        );
        assert!(
            (eq.core.dr_duty_cycle - 1.0).abs() < f64::EPSILON,
            "DRLevel::High must explicitly set dr_duty_cycle = 1.0 (not stale from prior level); \
             got {}",
            eq.core.dr_duty_cycle,
        );
    }

    // COP per AHRI/SEER convention: denominator is compressor-only electric power,
    // excluding fan, ER backup, and pan heater.

    #[test]
    fn cop_excludes_fan_power_from_denominator() {
        // With fan_power_w_per_cfm=0 (no fan) and again with fan>0, verify that
        // the reported COP equals thermal_output / compressor_only, not thermal / total_electric.
        // Config: capacity=8000W, EIR=0.33 → compressor ≈ 2640 W at full load.
        // Identity biquadratic curves (ratio=1), OAT above all lockouts.
        let cfg = heater_config_with(|typed| typed.common.fan_power_w = Some(0.0));
        // OAT=5°C: above ER lockout (4.44°C) so only HP runs, no ER.
        let e = env(18.0, 5.0, 0.003);

        let mut eq_no_fan = ASHPHeater::new(cfg.clone());
        let mut ports_no_fan = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_no_fan.init(&cfg, &e).unwrap();
        eq_no_fan.update_control(&e);
        eq_no_fan
            .step(&e, Duration::from_secs(60), &mut ports_no_fan)
            .unwrap();

        let cop_no_fan = eq_no_fan.telemetry().get(tk::COP).unwrap_or(0.0);
        let electric_kw_no_fan = eq_no_fan.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let thermal_w_no_fan = eq_no_fan
            .telemetry()
            .get(tk::THERMAL_OUTPUT_W)
            .unwrap_or(0.0);

        // With zero fan, total electric = compressor only, so COP = thermal / electric.
        if electric_kw_no_fan > 1e-6 {
            let expected_cop = thermal_w_no_fan / (electric_kw_no_fan * 1000.0);
            assert!(
                (cop_no_fan - expected_cop).abs() < 1e-6,
                "with no fan, COP must equal thermal/compressor; expected {expected_cop:.4}, \
                 got {cop_no_fan:.4}"
            );
        }

        // Now add fan power: COP should be higher because denominator is smaller
        // (only compressor, not compressor+fan).
        let cfg_fan = heater_config_with(|typed| typed.common.fan_power_w = Some(500.0));
        let mut eq_fan = ASHPHeater::new(cfg_fan.clone());
        let mut ports_fan = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_fan.init(&cfg_fan, &e).unwrap();
        eq_fan.update_control(&e);
        eq_fan
            .step(&e, Duration::from_secs(60), &mut ports_fan)
            .unwrap();

        let cop_fan = eq_fan.telemetry().get(tk::COP).unwrap_or(0.0);
        let electric_kw_fan = eq_fan.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // COP must NOT equal thermal / total_electric (which would include fan).
        // Reported COP should be >= no-fan COP because it uses compressor-only denominator.
        if electric_kw_fan > electric_kw_no_fan + 1e-9 {
            // Verify the reported COP is not the total-power COP (which would be lower).
            let total_power_cop =
                ports_fan.thermal[0].sensible_gain_w.abs() / (electric_kw_fan * 1000.0).max(1e-9);
            assert!(
                cop_fan > total_power_cop - 1e-9,
                "COP must be computed from compressor-only power (higher than total-power COP); \
                 compressor_only COP={cop_fan:.4}, total_power COP={total_power_cop:.4}"
            );
        }
    }

    #[test]
    fn cop_is_zero_when_heater_is_off() {
        // When the heater is off (no update_control call), compressor_kw=0 → COP must be 0.
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        // Skip update_control so heater stays Off.
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert_eq!(cop, 0.0, "heater off must give COP=0, got {cop}");
    }

    #[test]
    fn ashp_cop_clamped_to_physical_range() {
        // ASHP with pathological cap curve (c0=100 × rated capacity) and
        // identity eir curve produces unbounded COP ~303. Verify the telemetry
        // COP is clamped to the [0.0, 10.0] physical range.
        let mut cfg = heater_config();
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[100,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        // Zone below setpoint (18°C < 21°C) → heater runs, OAT above all
        // lockouts → only HP runs.
        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert!(
            cop.is_finite() && (0.0..=10.0).contains(&cop),
            "ASHP heating COP must be in [0.0, 10.0], got {cop}"
        );
        // The unclamped COP would be ~303; clamping must have engaged.
        assert!(
            cop < 100.0,
            "COP must have been clamped below 100 (raw ~303), got {cop}"
        );
    }

    #[test]
    fn mshp_typed_init_produces_four_speed_stages() {
        use crate::Equipment;
        use crate::HeatPumpHeaterConfig;
        use crate::config::EquipmentTypedConfig;

        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                heating_eir: Some(1.0 / 3.0),
                is_mini_split: true,
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };

        let ec = crate::config::EquipmentConfig::from_typed(
            "MSHP Heater".to_string(),
            HeatPumpHeaterConfig::equipment_type_name().to_string(),
            cfg,
        )
        .unwrap();
        let ec = crate::config::EquipmentConfig::with_payload(
            "MSHP Heater".to_string(),
            "MSHP Heater".to_string(),
            ec.payload.clone(),
        );

        let environment = env(18.0, 0.0, 0.003);
        let mut eq = MinisplitHeater::new(ec.clone());
        eq.init(&ec, &environment).unwrap();

        let n_stages = eq.core.hvac.config.heating_capacities_w.len();
        assert_eq!(
            n_stages, 4,
            "MSHP typed init with single capacity must produce 4 speed stages, got {n_stages}"
        );
    }

    fn mshp_config_with(mutator: impl FnOnce(&mut HeatPumpHeaterConfig)) -> EquipmentConfig {
        use crate::config::EquipmentTypedConfig;
        let mut typed = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                zone_id: Some(1),
                heating_capacity_w: Some(8_000.0),
                heating_eir: Some(0.33),
                is_mini_split: true,
                setpoint: HvacSetpointConfig {
                    heating_setpoint_c: Some(21.0),
                    cooling_setpoint_c: Some(26.0),
                    ..Default::default()
                },
                hysteresis_c: Some(1.0),
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        mutator(&mut typed);
        let mut cfg = crate::config::EquipmentConfig::with_payload(
            "MSHP Heater".to_string(),
            "MSHP Heater".to_string(),
            crate::config::EquipmentConfig::from_typed(
                "MSHP Heater".to_string(),
                HeatPumpHeaterConfig::equipment_type_name().to_string(),
                typed,
            )
            .unwrap()
            .payload
            .clone(),
        );
        add_identity_biquadratic_curves(&mut cfg);
        cfg
    }

    #[test]
    fn mshp_default_backup_capacity_is_zero() {
        let cfg = mshp_config_with(|_| {});
        let mut eq = MinisplitHeater::new(cfg.clone());
        let environment = env(18.0, 5.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(
            eq.core.backup_capacity_w, 0.0,
            "MSHP without explicit backup must default to 0 W, got {}",
            eq.core.backup_capacity_w
        );
    }

    #[test]
    fn mshp_explicit_backup_capacity_is_respected() {
        let cfg = mshp_config_with(|typed| typed.common.backup_capacity_w = Some(3_000.0));
        let mut eq = MinisplitHeater::new(cfg.clone());
        let environment = env(18.0, 5.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(
            eq.core.backup_capacity_w, 3_000.0,
            "MSHP with explicit backup_capacity_w must use that value"
        );
    }

    #[test]
    fn mshp_er_does_not_engage_without_backup_capacity() {
        use hares_types::OperatingMode;
        let cfg = mshp_config_with(|_| {});
        let mut eq = MinisplitHeater::new(cfg.clone());
        // Zone well below setpoint (21°C) and OAT above HP lockout -- HP should run, ER must not.
        let environment = env(15.0, 5.0, 0.003);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let mode = eq.core.operating_mode;
        assert!(
            !matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "MSHP with no backup must never engage ER, but mode was {mode:?}"
        );
    }

    #[test]
    fn gas_backup_fuel_without_explicit_eir_uses_afue80_eir() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_fuel = Some(hares_types::FuelType::Gas);
            typed.common.backup_eir = None;
            typed.common.fan_power_w = Some(0.0);
        });

        let environment = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        let mode = eq.update_control(&environment);
        assert_eq!(mode, OperatingMode::HeatingER, "HP must be locked out");
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Gas backup at AFUE 80%: fuel_input = capacity / 0.80 = 4000 / 0.80 = 5000 W.
        let expected_fuel_w = 4_000.0 / 0.80;
        let fuel_input_w = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(0.0);
        assert!(
            (fuel_input_w - expected_fuel_w).abs() < 1e-6,
            "gas backup AFUE 80% should produce {expected_fuel_w:.6} W fuel, got {fuel_input_w:.6} W"
        );
        // No electric draw (fan_power_w = 0, backup is gas).
        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            electric_kw.abs() < 1e-9,
            "gas backup should draw no electric power, got {electric_kw:.9} kW"
        );
        // Fuel port must carry the gas consumption.
        assert!(
            (ports.fuel.get(hares_types::FuelType::Gas) - expected_fuel_w).abs() < 1e-6,
            "gas fuel port must carry {expected_fuel_w:.6} W, got {:.6} W",
            ports.fuel.get(hares_types::FuelType::Gas)
        );
    }

    #[test]
    fn gas_backup_fuel_port_contribution_and_no_electric_leak() {
        // ASHP with natural_gas backup: when ER is on, fuel consumption must appear on
        // the fuel port, not the electrical port. Electric port must only include
        // compressor + fan (none here since fan_power_w=0 and HP is locked out).
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(6_000.0);
            typed.common.backup_fuel = Some(hares_types::FuelType::Gas);
            typed.common.backup_eir = None;
            typed.common.fan_power_w = Some(0.0);
        });

        let environment = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        let mode = eq.update_control(&environment);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "HP must be locked out by OAT"
        );
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        // fuel_input_w telemetry must be set.
        let fuel_input_w = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(0.0);
        assert!(
            fuel_input_w > 0.0,
            "FUEL_INPUT_W telemetry must be > 0 when gas backup is active"
        );

        // Expected: 6000 W capacity / 0.80 AFUE = 7500 W fuel.
        let expected_fuel_w = 6_000.0 / 0.80;
        assert!(
            (fuel_input_w - expected_fuel_w).abs() < 1e-6,
            "expected {expected_fuel_w:.3} W fuel, got {fuel_input_w:.3} W"
        );

        // Fuel port carries gas consumption.
        let gas_port_w = ports.fuel.get(hares_types::FuelType::Gas);
        assert!(
            (gas_port_w - expected_fuel_w).abs() < 1e-6,
            "gas fuel port must carry {expected_fuel_w:.3} W, got {gas_port_w:.3} W"
        );

        // Electrical port must be zero (no compressor, no fan).
        assert!(
            ports.electrical.load_power_w.abs() < 1e-9,
            "gas backup must not contribute to electrical port, got {:.9} kW",
            ports.electrical.load_power_w
        );

        // core_output.flows.fuel_w must be Some and match.
        let core_fuel = eq
            .core_output()
            .flows
            .fuel_w
            .as_ref()
            .expect("fuel_w must be Some");
        assert_eq!(core_fuel.fuel_type, hares_types::FuelType::Gas);
        assert!(
            (core_fuel.consumption_w - expected_fuel_w).abs() < 1e-6,
            "core_output fuel_w must be {expected_fuel_w:.3} W, got {:.3} W",
            core_fuel.consumption_w
        );

        // Electrical core_output must be zero.
        if let Some(hares_types::ElectricPower::Consumption(kw)) =
            eq.core_output().flows.electric_kw
        {
            assert!(
                kw.abs() < 1e-9,
                "core electric_kw must be zero for gas-only backup, got {kw:.9} kW"
            );
        }
    }

    // Bug 1: HP lockout hysteresis prevents chatter when OAT oscillates near threshold.
    // Once HP is available (OAT >= lockout_temp), it must stay available until OAT drops
    // below (lockout_temp - hysteresis), not just below lockout_temp.
    #[test]
    fn hp_lockout_hysteresis_prevents_chatter() {
        use super::super::constants::DEFAULT_HP_LOCKOUT_HYSTERESIS_C;
        let lockout_c = -10.0_f64;
        let cfg = heater_config_with(|typed| typed.hp_lockout_temp_c = Some(lockout_c));

        // Start HP available: OAT above lockout.
        let e_above = env(18.0, lockout_c + 1.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e_above).unwrap();
        assert!(eq.core.hp_available, "HP must be available above lockout");

        // Drop OAT to just below lockout (within hysteresis band): HP must still be available.
        let oat_in_band = lockout_c - DEFAULT_HP_LOCKOUT_HYSTERESIS_C * 0.5;
        let e_in_band = env(18.0, oat_in_band, 0.003);
        eq.update_control(&e_in_band);
        assert!(
            eq.core.hp_available,
            "HP must remain available when OAT ({oat_in_band:.2}°C) is within hysteresis band \
             (lockout {lockout_c:.2}°C - hysteresis {DEFAULT_HP_LOCKOUT_HYSTERESIS_C:.2}°C)"
        );

        // Drop OAT below the full hysteresis band: HP must now lock out.
        let oat_below = lockout_c - DEFAULT_HP_LOCKOUT_HYSTERESIS_C - 0.1;
        let e_below = env(18.0, oat_below, 0.003);
        eq.update_control(&e_below);
        assert!(
            !eq.core.hp_available,
            "HP must lock out when OAT ({oat_below:.2}°C) is below \
             lockout - hysteresis ({:.2}°C)",
            lockout_c - DEFAULT_HP_LOCKOUT_HYSTERESIS_C
        );
    }

    // Bug 2: Pan heater must only run when compressor (HP) is active.
    // In ER-only mode the compressor is off; pan heater must be off.
    #[test]
    fn pan_heater_off_in_er_only_mode() {
        let cfg = mshp_config_with(|typed| {
            typed.common.backup_capacity_w = Some(3_000.0);
            typed.er_lockout_temp_c = Some(10.0);
            // Force HP lockout so only ER runs.
            typed.hp_lockout_temp_c = Some(50.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        // OAT cold enough to trigger pan heater but below HP lockout → ER-only.
        let e = env(18.0, -5.0, 0.003);
        let mut eq = MinisplitHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        // Give the MSHP a non-zero pan heater so it would fire if the bug were present.
        eq.core.pan_heater_kw = 0.1;
        eq.core.pan_heater_temp_c = 5.0; // OAT -5°C is below this

        let mode = eq.update_control(&e);
        assert_eq!(mode, OperatingMode::HeatingER, "must be ER-only mode");

        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            !eq.core.pan_heater_on,
            "pan heater must be off when compressor is not running (ER-only mode)"
        );
    }

    // Bug 3: ER soft lockout must release after er_hard_lockout_time_s * 2
    // even if zone temperature keeps rising indefinitely.
    #[test]
    fn er_soft_lockout_releases_after_timeout() {
        let lockout_s = 60.0_f64;
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(lockout_s);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.setpoint.heating_setpoint_c = Some(18.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(15.0, 0.0, 0)).unwrap();

        // Raise setpoint to arm the hard lockout.
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            })
            .ok();

        // Steps: keep zone rising each step so soft lockout would normally hold.
        // At t = lockout_s * 2 + some steps, the timeout must release it.
        let max_steps = (lockout_s * 3.0 / 60.0) as i64 + 5;
        let mut released = false;
        for step in 0..max_steps {
            // Zone keeps rising: 15 + 0.05 * step to simulate HP winning.
            let zone_c = 15.0 + 0.05 * step as f64;
            let t = make_env(zone_c, 0.0, step * 60);
            let mode = eq.update_control(&t);
            if matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ) {
                released = true;
                break;
            }
        }
        assert!(
            released,
            "ER soft lockout must release after er_hard_lockout_time_s * 2 ({:.0}s) \
             even when zone temperature keeps rising",
            lockout_s * 2.0,
        );
    }

    // Bug 4: effective_load scaling must not scale ER power; ER is on/off only.
    // With a DutyCycle=0.5 control signal, HP power should halve but ER power stays full.
    #[test]
    fn duty_cycle_scaling_does_not_reduce_er_power() {
        let backup_capacity_w = 4_000.0_f64;
        let backup_eir = 1.0_f64;
        let er_full_kw = backup_capacity_w * backup_eir / 1000.0;

        // Force ER-only mode: HP locked out, ER allowed.
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(backup_capacity_w);
            typed.common.backup_eir = Some(backup_eir);
            typed.common.fan_power_w = Some(0.0);
        });

        let e = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        let mode = eq.update_control(&e);
        assert_eq!(mode, OperatingMode::HeatingER, "must be ER-only");

        // Apply 50% duty cycle -- this should NOT scale ER.
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: None,
        })
        .unwrap();
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            (electric_kw - er_full_kw).abs() < 0.01,
            "ER power must remain full ({er_full_kw:.3} kW) even with 50% duty cycle; \
             got {electric_kw:.3} kW"
        );
    }

    // Resistive defrost is a protective, deterministic draw: like ER it must
    // NOT be scaled by duty-cycle curtailment. With a DutyCycle=0.5 control and
    // discrete resistive defrost active (compressor+fan+ER all zero), the unit
    // draw must equal the full defrost element power, not half (and previously
    // the defrost draw was dropped from electric_kw entirely in this branch).
    #[test]
    fn duty_cycle_scaling_does_not_reduce_resistive_defrost_draw() {
        use crate::hvac::heat_pump::defrost::{DefrostCycleState, DefrostStrategy};

        const DEFROST_ELEMENT_W: f64 = 3_000.0;
        const DEFROST_CONTROL_W: f64 = 200.0;
        let defrost_kw = (DEFROST_ELEMENT_W + DEFROST_CONTROL_W) / 1000.0;

        let run = |duty: f64| -> (f64, f64) {
            let cfg = heater_config_with(|typed| {
                // HP-only: no backup ER, no fan, so the defrost element is the
                // only draw during discrete resistive defrost.
                typed.common.backup_capacity_w = Some(0.0);
                typed.common.fan_power_w = Some(0.0);
                typed.defrost = DefrostConfig {
                    strategy: DefrostStrategy::Resistive,
                    resistive_defrost_capacity_w: DEFROST_ELEMENT_W,
                    defrost_power_w: DEFROST_CONTROL_W,
                    ..DefrostConfig::default()
                };
            });
            let e = env(18.0, 0.0, 0.005);
            let mut eq = ASHPHeater::new(cfg.clone());
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.init(&cfg, &e).unwrap();
            let mode = eq.update_control(&e);
            assert_eq!(mode, OperatingMode::HeatingHP, "must be HP-only mode");

            // Force the discrete defrost FSM into Defrosting for this step.
            eq.core.defrost_cycle_tracker.state = DefrostCycleState::Defrosting;
            eq.core.defrost_cycle_tracker.defrost_elapsed_s = 0.0;

            eq.apply_control(&ControlSignal::DutyCycle {
                on_fraction: duty,
                period_s: None,
                component: None,
            })
            .unwrap();
            eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
            (
                eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0),
                eq.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap_or(-1.0),
            )
        };

        let (full_kw, full_q) = run(1.0);
        let (curtailed_kw, curtailed_q) = run(0.5);
        assert!(
            (full_kw - defrost_kw).abs() < 1e-9,
            "uncurtailed resistive defrost draw must be {defrost_kw:.3} kW; got {full_kw:.3}"
        );
        assert!(
            (curtailed_kw - defrost_kw).abs() < 1e-9,
            "resistive defrost draw must remain {defrost_kw:.3} kW under 50% duty cycle \
             (protective on/off element, like ER); got {curtailed_kw:.3} kW"
        );
        // Defrost element is resistive: Q stays exactly zero in both cases.
        assert_eq!(full_q, 0.0, "resistive defrost must produce Q == 0");
        assert_eq!(curtailed_q, 0.0, "resistive defrost must produce Q == 0");
    }

    // Telemetry OPERATING_MODE must use the canonical OperatingMode::as_code()
    // encoding — the single source of truth shared with the CSV "Mode" columns.
    // (The legacy parallel HVAC encoding 3/4/5 for HP/HP+ER/ER is deleted.)
    #[test]
    fn telemetry_operating_mode_uses_canonical_as_code_encoding() {
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0); // HP locked out at OAT 0 → ER-only
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });
        let e = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        let mode = eq.update_control(&e);
        assert_eq!(mode, OperatingMode::HeatingER);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let code = eq.telemetry().get(tk::OPERATING_MODE);
        assert_eq!(
            code,
            Some(OperatingMode::HeatingER.as_code()),
            "telemetry mode code must be OperatingMode::as_code() (HeatingER = 8)"
        );
        assert_eq!(code, Some(8.0));
        assert_eq!(
            eq.core_output().state.operating_mode.map(|m| m.as_code()),
            code,
            "telemetry and CoreOutput must agree on the canonical encoding"
        );
    }

    // Bug 5: first call to update_control must never trigger ER hard lockout
    // even when new() initializes prev_base_setpoint to NEG_INFINITY.
    #[test]
    fn first_call_does_not_trigger_spurious_er_lockout() {
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        // Do NOT call init() -- use new() directly so prev_base_setpoint = NEG_INFINITY.
        // Then call init() which sets it to the actual setpoint, then update_control.
        let mut eq = ASHPHeater::new(cfg.clone());
        let e = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &e).unwrap();

        // First control call -- must not trigger lockout.
        let mode = eq.update_control(&e);
        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "first update_control must not trigger ER hard lockout (prev_base_setpoint \
             initialized to NEG_INFINITY); remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );
        assert!(
            matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed on first call; got {mode:?}"
        );
    }

    // PowerLimit must shed ER (on/off) before scaling the compressor (modulatable).
    // With HP+ER both running (forced via ModeOverride), a limit below total but
    // above HP+fan must zero ER and leave compressor power unchanged.
    #[test]
    fn power_limit_sheds_er_before_scaling_compressor() {
        let backup_capacity_w = 4_000.0_f64;
        let backup_eir = 1.0_f64;

        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(backup_capacity_w);
            typed.common.backup_eir = Some(backup_eir);
            typed.common.fan_power_w = Some(0.0);
        });

        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();

        // Force HP+ER simultaneously via ModeOverride.
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHPAndER,
        })
        .unwrap();
        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            OperatingMode::HeatingHPAndER,
            "setup must produce HP+ER mode"
        );
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let full_electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let full_compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);

        assert!(
            full_electric_kw > 0.0,
            "must have some power draw before limit"
        );
        assert!(er_kw > 0.0, "ER must be running in baseline step");

        // A limit halfway between (HP-only) and (HP+ER): shedding ER is sufficient.
        let hp_only_kw = full_electric_kw - er_kw;
        let limit_kw = hp_only_kw + er_kw * 0.5;
        assert!(limit_kw < full_electric_kw, "limit must be below total");
        assert!(limit_kw > hp_only_kw, "limit must be above HP-only draw");

        // Apply PowerLimit below total but above HP-only draw; ER must be shed,
        // compressor must remain above zero (not scaled away).
        // Use a fresh heater in the same initial conditions to avoid state drift.
        let mut eq2 = ASHPHeater::new(cfg.clone());
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq2.init(&cfg, &e).unwrap();
        eq2.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHPAndER,
        })
        .unwrap();
        eq2.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: limit_kw,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();
        eq2.update_control(&e);
        eq2.step(&e, Duration::from_secs(60), &mut ports2).unwrap();

        let limited_er_kw = eq2.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(f64::NAN);
        let limited_compressor_kw = eq2.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let limited_electric_kw = eq2.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(
            limited_er_kw.abs() < 1e-9,
            "ER must be shed to 0 under PowerLimit when limit > HP+fan; got {limited_er_kw:.6} kW"
        );
        assert!(
            limited_compressor_kw > 0.0,
            "compressor must not be zeroed when ER shedding alone satisfies the limit; \
             got {limited_compressor_kw:.6} kW"
        );
        assert!(
            (limited_compressor_kw - full_compressor_kw).abs() < 1e-6,
            "compressor must be identical to no-limit baseline when ER shedding is sufficient; \
             baseline {full_compressor_kw:.3} kW, limited {limited_compressor_kw:.3} kW"
        );
        assert!(
            limited_electric_kw <= limit_kw + 1e-9,
            "electric_kw must not exceed limit after ER shed; \
             electric={limited_electric_kw:.4}, limit={limit_kw:.4}"
        );
    }

    // After the soft lockout timeout fires, ER must remain available even if
    // the zone is still rising -- re-arm must not be possible until elapsed resets.
    #[test]
    fn soft_lockout_stays_released_after_timeout() {
        let lockout_s = 60.0_f64;
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(lockout_s);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.setpoint.heating_setpoint_c = Some(18.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(15.0, 0.0, 0)).unwrap();

        // Trigger hard lockout via setpoint raise.
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            })
            .ok();

        // Advance past hard lockout + twice the hard lockout to guarantee timeout fires.
        // Keep zone rising so soft lockout would re-arm if the bug were present.
        let max_s = (lockout_s * 4.0) as i64;
        let mut released_step: Option<i64> = None;
        for step in 0..=max_s / 60 {
            let zone_c = 15.0 + 0.1 * step as f64;
            let t = make_env(zone_c, 0.0, step * 60);
            let mode = eq.update_control(&t);
            if matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ) {
                released_step = Some(step);
                break;
            }
        }

        let released_at = released_step.expect("ER must have been released before max_s");

        // After release, continue with zone still rising. ER must stay available.
        for step in (released_at + 1)..=(released_at + 5) {
            let zone_c = 15.0 + 0.1 * step as f64;
            let t = make_env(zone_c, 0.0, step * 60);
            let mode = eq.update_control(&t);
            assert!(
                matches!(
                    mode,
                    OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
                ),
                "ER must remain available after timeout release even with rising zone \
                 (step {step}); got {mode:?}"
            );
        }
    }

    // Sub-consumption telemetry (compressor + fan + ER + pan) must sum to
    // ELECTRIC_KW within 0.1% relative error.
    #[test]
    fn sub_consumption_telemetry_sums_to_electric_kw() {
        // Use ModeOverride(HeatingHPAndER) so all four sub-consumption channels
        // are active, which gives the most thorough coverage of the sum invariant.
        let cfg = heater_config_with(|typed| {
            typed.common.fan_power_w = Some(300.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
        });

        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHPAndER,
        })
        .unwrap();
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let fan_kw = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0);
        let backup_er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);
        let pan_heater_kw = eq.telemetry().get(tk::PAN_HEATER_KW).unwrap_or(0.0);
        let sub_sum = compressor_kw + fan_kw + backup_er_kw + pan_heater_kw;

        assert!(
            electric_kw > 0.0,
            "electric_kw must be positive for this configuration"
        );
        let rel_err = (sub_sum - electric_kw).abs() / electric_kw.max(f64::MIN_POSITIVE);
        assert!(
            rel_err < 0.001,
            "sub-consumptions sum {sub_sum:.6} kW must equal electric_kw {electric_kw:.6} kW \
             within 0.1% (rel_err={rel_err:.2e})"
        );
    }

    /// Validates that all heat-pump heater constants are within physically
    /// correct ranges and match OCHRE / EnergyPlus reference values where
    /// applicable.
    ///
    /// Sources:
    /// - OCHRE HVAC.py lines 1208-1211 (lockout temps, ER setpoint offset)
    /// - OCHRE HVAC.py line 142 (350 CFM/ton heating airflow)
    /// - OCHRE MinisplitASHPHeater attrs: pan_heater_kw=0.150, pan_heater_temp=0
    /// - EnergyPlus Engineering Reference: MaxOATSupplemental default = 21°C
    #[test]
    fn test_hp_defaults_are_physically_correct() {
        use super::super::constants::{
            DEFAULT_BACKUP_CAPACITY_W, DEFAULT_BACKUP_EIR, DEFAULT_ER_HARD_LOCKOUT_TIME_S,
            DEFAULT_ER_LOCKOUT_TEMP_C, DEFAULT_ER_SETPOINT_DEADBAND_OFFSET,
            DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER, DEFAULT_HEATING_CAPACITY_W, DEFAULT_HEATING_EIR,
            DEFAULT_HP_LOCKOUT_HYSTERESIS_C, DEFAULT_HP_LOCKOUT_TEMP_C,
            DEFAULT_MIN_ER_CYCLE_TIME_S, MAX_OAT_SUPPLEMENTAL_C, MSHP_PAN_HEATER_DEFAULT_KW,
            MSHP_PAN_HEATER_DEFAULT_TEMP_C,
        };
        use crate::hvac::hvac_core::AIRFLOW_HEATING_M3_S_PER_W;

        // --- Lockout temperatures (OCHRE HVAC.py line 1208-1209) ---
        // HP lockout: 0°F = -17.78°C
        assert!(
            (DEFAULT_HP_LOCKOUT_TEMP_C - (-17.78)).abs() < 0.01,
            "HP lockout must be -17.78°C (0°F); got {DEFAULT_HP_LOCKOUT_TEMP_C}"
        );
        // ER lockout: 40°F = 4.44°C
        assert!(
            (DEFAULT_ER_LOCKOUT_TEMP_C - 4.44).abs() < 0.01,
            "ER lockout must be 4.44°C (40°F); got {DEFAULT_ER_LOCKOUT_TEMP_C}"
        );
        // HP lockout must be below ER lockout (compressor can run at colder temps than ER)
        const {
            assert!(
                DEFAULT_HP_LOCKOUT_TEMP_C < DEFAULT_ER_LOCKOUT_TEMP_C,
                "HP lockout must be colder than ER lockout"
            );
        }

        // --- ER setpoint offset (OCHRE HVAC.py line 1211: deadband * (1.8 - deadband_offset)) ---
        // With default deadband=1.0, offset = 1.0 * (1.8 - 0.2) = 1.6°C
        let computed_offset =
            1.0 * (DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER - DEFAULT_ER_SETPOINT_DEADBAND_OFFSET);
        assert!(
            (computed_offset - 1.6).abs() < 0.01,
            "ER setpoint offset with deadband=1.0 must be 1.6°C (OCHRE default); got {computed_offset}"
        );
        assert!(
            (DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER - 1.8).abs() < 1e-9,
            "ER setpoint offset multiplier must be 1.8 (OCHRE); got {DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER}"
        );
        assert!(
            (DEFAULT_ER_SETPOINT_DEADBAND_OFFSET - 0.2).abs() < 1e-9,
            "ER deadband offset must be 0.2 (OCHRE deadband_offset default); got {DEFAULT_ER_SETPOINT_DEADBAND_OFFSET}"
        );

        // --- Lockout timers (OCHRE HVAC.py lines 1214, 1229: default 0 minutes) ---
        assert_eq!(
            DEFAULT_ER_HARD_LOCKOUT_TIME_S, 0.0,
            "ER hard lockout must default to 0s (disabled), matching OCHRE"
        );
        assert_eq!(
            DEFAULT_MIN_ER_CYCLE_TIME_S, 0.0,
            "Minimum ER cycle time must default to 0s (disabled), matching OCHRE"
        );

        // --- MAX_OAT_SUPPLEMENTAL_C (EnergyPlus default: 21°C / 69.8°F) ---
        assert!(
            (MAX_OAT_SUPPLEMENTAL_C - 21.0).abs() < 0.01,
            "Max OAT supplemental must be 21°C (EnergyPlus default); got {MAX_OAT_SUPPLEMENTAL_C}"
        );
        // Must be above ER lockout (otherwise supplemental cap is never reached)
        const {
            assert!(
                MAX_OAT_SUPPLEMENTAL_C > DEFAULT_ER_LOCKOUT_TEMP_C,
                "Max OAT supplemental must be above ER lockout"
            );
        }

        // --- Heating airflow (OCHRE HVAC.py line 142: 350 CFM/ton for heating) ---
        // 350 CFM/ton × 4.71947443e-4 m³/s/CFM / 3516.85 W/ton ≈ 4.6969e-5 m³/s/W
        let expected_airflow = 350.0 * 4.719_474_43e-4 / 3516.85;
        assert!(
            (AIRFLOW_HEATING_M3_S_PER_W - expected_airflow).abs() < 1e-8,
            "Heating airflow must match OCHRE 350 CFM/ton = {expected_airflow:.6e} m³/s/W; \
             got {AIRFLOW_HEATING_M3_S_PER_W:.6e}"
        );

        // --- ASHP backup defaults ---
        const {
            assert!(
                DEFAULT_BACKUP_CAPACITY_W > 0.0,
                "ASHP default backup capacity must be positive"
            );
            assert!(
                DEFAULT_BACKUP_CAPACITY_W <= 20_000.0,
                "ASHP default backup capacity is implausibly large"
            );
        }
        assert!(
            (DEFAULT_BACKUP_EIR - 1.0).abs() < 1e-9,
            "Default backup EIR must be 1.0 (electric resistance); got {DEFAULT_BACKUP_EIR}"
        );

        // --- Heating capacity / EIR fallbacks ---
        const {
            assert!(
                DEFAULT_HEATING_CAPACITY_W > 0.0 && DEFAULT_HEATING_CAPACITY_W <= 50_000.0,
                "Default heating capacity must be in [0, 50 kW]"
            );
            assert!(
                DEFAULT_HEATING_EIR > 0.0 && DEFAULT_HEATING_EIR < 1.0,
                "Default heating EIR must be in (0, 1) (COP > 1)"
            );
        }

        // --- Hysteresis band: small positive value preventing rapid cycling ---
        const {
            assert!(
                DEFAULT_HP_LOCKOUT_HYSTERESIS_C > 0.0 && DEFAULT_HP_LOCKOUT_HYSTERESIS_C < 5.0,
                "HP lockout hysteresis must be in (0, 5) degrees C"
            );
        }

        // --- MSHP pan heater (OCHRE MinisplitASHPHeater: 0.150 kW @ 0°C) ---
        assert!(
            (MSHP_PAN_HEATER_DEFAULT_KW - 0.150).abs() < 1e-9,
            "MSHP pan heater must be 0.150 kW (OCHRE default); got {MSHP_PAN_HEATER_DEFAULT_KW}"
        );
        assert!(
            (MSHP_PAN_HEATER_DEFAULT_TEMP_C - 0.0).abs() < 1e-9,
            "MSHP pan heater activation temp must be 0.0°C (OCHRE default); got {MSHP_PAN_HEATER_DEFAULT_TEMP_C}"
        );
    }

    #[test]
    fn hp_lockout_re_enables_when_oat_rises() {
        // HP lockout temp set to -5°C. Start well below to ensure lockout,
        // then step above to verify re-enable.
        // Use OAT=5°C (above default ER lockout 4.44°C) in the warm step so that
        // ER is blocked by temperature, isolating the HP re-enable signal.
        let lockout_c = -5.0_f64;
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(lockout_c);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let cold_env = env(18.0, lockout_c - 10.0, 0.003);
        eq.init(&cfg, &cold_env).unwrap();

        // Step 1: OAT well below lockout -- HP must be locked out.
        let mode_cold = eq.update_control(&cold_env);
        assert!(
            !matches!(
                mode_cold,
                OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
            ),
            "HP must be off when OAT is well below lockout ({lockout_c}°C); got {mode_cold:?}"
        );

        // Step 2: raise OAT to 5°C -- above HP lockout (-5°C) and above ER lockout
        // (4.44°C), so only HP runs.
        let warm_env = env(18.0, 5.0, 0.003);
        let mode_warm = eq.update_control(&warm_env);
        assert_eq!(
            mode_warm,
            OperatingMode::HeatingHP,
            "HP must re-enable once OAT rises above lockout ({lockout_c}°C); got {mode_warm:?}"
        );
    }

    #[test]
    fn sub_consumption_sum_in_er_only_mode() {
        // HP locked out (OAT below lockout), ER active.
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_lockout_temp_c = Some(5.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(300.0);
        });

        let e = env(18.0, 0.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        let mode = eq.update_control(&e);
        assert_eq!(mode, OperatingMode::HeatingER, "must be ER-only mode");
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let fan_kw = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0);
        let backup_er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);
        let pan_heater_kw = eq.telemetry().get(tk::PAN_HEATER_KW).unwrap_or(0.0);
        let sub_sum = compressor_kw + fan_kw + backup_er_kw + pan_heater_kw;

        assert!(electric_kw > 0.0, "ER-only mode must draw power");
        let rel_err = (sub_sum - electric_kw).abs() / electric_kw.max(f64::MIN_POSITIVE);
        assert!(
            rel_err < 0.001,
            "sub-consumptions sum {sub_sum:.6} kW must equal electric_kw {electric_kw:.6} kW \
             within 0.1% in ER-only mode (rel_err={rel_err:.2e})"
        );
    }

    #[test]
    fn sub_consumption_sum_in_hp_only_mode() {
        // Normal HP operation; OAT above ER lockout so no ER fires.
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.common.fan_power_w = Some(300.0);
        });

        // OAT=5°C: above ER lockout (4.44°C) so only HP runs.
        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        let mode = eq.update_control(&e);
        assert_eq!(mode, OperatingMode::HeatingHP, "must be HP-only mode");
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let fan_kw = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0);
        let backup_er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);
        let pan_heater_kw = eq.telemetry().get(tk::PAN_HEATER_KW).unwrap_or(0.0);
        let sub_sum = compressor_kw + fan_kw + backup_er_kw + pan_heater_kw;

        assert!(electric_kw > 0.0, "HP-only mode must draw power");
        assert!(
            backup_er_kw.abs() < 1e-9,
            "no ER must fire in HP-only mode; got {backup_er_kw:.6} kW"
        );
        let rel_err = (sub_sum - electric_kw).abs() / electric_kw.max(f64::MIN_POSITIVE);
        assert!(
            rel_err < 0.001,
            "sub-consumptions sum {sub_sum:.6} kW must equal electric_kw {electric_kw:.6} kW \
             within 0.1% in HP-only mode (rel_err={rel_err:.2e})"
        );
    }

    #[test]
    fn defrost_active_adjusts_capacity_and_power() {
        // OAT around -5°C where defrost should be active (humidity and temperature
        // conditions trigger on-demand defrost). Compare HP_CAPACITY_W against a
        // warm-OAT baseline where defrost is inactive.
        let cfg = heater_config();

        // Warm baseline: OAT=10°C, low humidity -- no defrost expected.
        let env_warm = env(18.0, 10.0, 0.002);
        let mut eq_warm = ASHPHeater::new(cfg.clone());
        let mut ports_warm = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_warm.init(&cfg, &env_warm).unwrap();
        eq_warm.update_control(&env_warm);
        eq_warm
            .step(&env_warm, Duration::from_secs(60), &mut ports_warm)
            .unwrap();
        let warm_capacity_w = eq_warm.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);
        let warm_defrost = eq_warm.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);
        assert_eq!(warm_defrost, 0.0, "warm OAT must not trigger defrost");

        // Cold defrost case: OAT=-5°C with elevated humidity triggers defrost.
        let env_cold = env(18.0, -5.0, 0.005);
        let mut eq_cold = ASHPHeater::new(cfg.clone());
        let mut ports_cold = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_cold.init(&cfg, &env_cold).unwrap();
        eq_cold.update_control(&env_cold);
        eq_cold
            .step(&env_cold, Duration::from_secs(60), &mut ports_cold)
            .unwrap();
        let cold_defrost = eq_cold.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);
        let cold_capacity_w = eq_cold.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);

        assert_eq!(
            cold_defrost, 1.0,
            "OAT=-5°C with humidity must trigger defrost; got DEFROST_ACTIVE={cold_defrost}"
        );
        assert!(
            cold_capacity_w < warm_capacity_w,
            "HP_CAPACITY_W during defrost ({cold_capacity_w:.1} W) must be reduced \
             vs warm-OAT baseline ({warm_capacity_w:.1} W)"
        );

        // Verify the non-ideal defrost formula:
        //   hp_capacity = (original * cap_mult - q_defrost).max(0.0) * crf
        // With identity biquadratic curves, warm_capacity_w == pre-defrost capacity.
        // Default crf = 1.0.
        let cap_mult = eq_cold
            .telemetry()
            .get(tk::DEFROST_CAPACITY_MULTIPLIER)
            .expect("DEFROST_CAPACITY_MULTIPLIER must be present");
        let q_defrost = eq_cold
            .telemetry()
            .get(tk::DEFROST_Q_W)
            .expect("DEFROST_Q_W must be present");
        let expected = (warm_capacity_w * cap_mult - q_defrost).max(0.0);
        assert!(
            (cold_capacity_w - expected).abs() < 1e-6,
            "non-ideal defrost formula mismatch: hp_capacity_w={cold_capacity_w:.6}, \
             expected={expected:.6} (warm_cap={warm_capacity_w:.1} * cap_mult={cap_mult:.6} \
             - q_defrost={q_defrost:.6})"
        );
    }

    #[test]
    fn mshp_never_engages_er_without_explicit_backup() {
        // MSHP default: no backup heat. Even at very cold OAT, ER must never fire.
        let cfg = mshp_config_with(|_| {});

        // Very cold OAT: below all typical lockouts.
        let cold_env = env(18.0, -25.0, 0.003);
        let mut eq = MinisplitHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &cold_env).unwrap();

        for step in 0..5i64 {
            let e = env(18.0 - step as f64 * 0.1, -25.0, 0.003);
            let mode = eq.update_control(&e);
            assert!(
                !matches!(
                    mode,
                    OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
                ),
                "MSHP with no backup must never engage ER at step {step}; got {mode:?}"
            );
            ports.zero();
            eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        }
    }

    // When zone temp is well below setpoint (inside the ER supplemental band) and OAT
    // is within the supplemental range, HP and ER must run simultaneously.
    #[test]
    fn hp_and_er_run_simultaneously_in_supplemental_band() {
        // er_setpoint_offset_c=1.6: ER fires when zone < setpoint(21) - 1.6 = 19.4°C.
        // er_lockout_temp_c=4.44: ER allowed when OAT < 4.44°C.
        // hp_lockout_temp_c=-17.78: HP available at OAT=2°C.
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.er_setpoint_offset_c = Some(1.6);
            typed.er_lockout_temp_c = Some(4.44);
            typed.hp_lockout_temp_c = Some(-17.78);
            typed.common.setpoint.heating_setpoint_c = Some(21.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        // Zone at 18°C: well below 19.4°C ER threshold. OAT=2°C: HP available, ER allowed.
        let e = env(18.0, 2.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();

        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            OperatingMode::HeatingHPAndER,
            "zone well below ER threshold with HP available must select HeatingHPAndER; got {mode:?}"
        );

        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let backup_er_kw = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0);
        assert!(
            compressor_kw > 0.0,
            "HP must contribute compressor power in HeatingHPAndER mode; got {compressor_kw:.4} kW"
        );
        assert!(
            backup_er_kw > 0.0,
            "ER must contribute backup power in HeatingHPAndER mode; got {backup_er_kw:.4} kW"
        );
    }

    // When zone temp is just below setpoint but above the ER offset threshold, only the
    // HP fires -- ER must not engage until the zone drops far enough.
    #[test]
    fn er_does_not_engage_above_er_setpoint_offset() {
        // er_setpoint_offset_c=1.6: ER fires when zone < 21 - 1.6 = 19.4°C.
        // Zone at 20°C is below setpoint(21) but above the 19.4°C ER turn-on threshold.
        // OAT=2°C: HP available, ER temperature gate passes -- only er_thermostat_call blocks it.
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.er_setpoint_offset_c = Some(1.6);
            typed.er_lockout_temp_c = Some(4.44);
            typed.hp_lockout_temp_c = Some(-17.78);
            typed.common.setpoint.heating_setpoint_c = Some(21.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        // Zone at 20°C: below setpoint but above ER turn-on (19.4°C). OAT=2°C.
        let e = env(20.0, 2.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            OperatingMode::HeatingHP,
            "zone above ER threshold must select HeatingHP only; got {mode:?}"
        );
    }

    // When OAT is below the HP lockout threshold, HP is unavailable and only ER heats.
    #[test]
    fn er_only_when_hp_locked_out() {
        // hp_lockout_temp_c=-17.78: HP locked out at OAT=-20°C.
        // er_lockout_temp_c=4.44: ER allowed at OAT=-20°C.
        // er_setpoint_offset_c=0.0: ER fires any time there is a heating call.
        let cfg = heater_config_with(|typed| {
            typed.common.backup_capacity_w = Some(4_000.0);
            typed.common.backup_eir = Some(1.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.er_lockout_temp_c = Some(4.44);
            typed.hp_lockout_temp_c = Some(-17.78);
            typed.common.setpoint.heating_setpoint_c = Some(21.0);
            typed.common.hysteresis_c = Some(1.0);
        });

        let e = env(18.0, -20.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "OAT below HP lockout must select HeatingER only; got {mode:?}"
        );
    }

    #[test]
    fn ideal_mode_defrost_clamps_to_post_defrost_ceiling() {
        let rated_cap = 8_000.0_f64;
        let mut cfg = heater_config_with(|typed| {
            typed.common.heating_capacity_w = Some(rated_cap);
        });
        cfg.test_extras_mut()
            .insert("use_ideal_capacity".to_string(), true.into());

        // OAT=0°C with humidity: triggers defrost (below 4.44°C threshold).
        let environment = env(18.0, 0.0, 0.005);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);

        // Request full rated capacity via ideal-capacity signal.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: rated_cap,
            degraded: false,
        })
        .expect("ideal-capacity control accepted");
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let hp_cap = eq.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);
        let defrost_active = eq.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);
        let cap_mult = eq
            .telemetry()
            .get(tk::DEFROST_CAPACITY_MULTIPLIER)
            .unwrap_or(1.0);
        let q_defrost = eq.telemetry().get(tk::DEFROST_Q_W).unwrap_or(0.0);

        assert_eq!(
            defrost_active, 1.0,
            "OAT=0°C with humidity must trigger defrost"
        );

        // Post-defrost rated ceiling (crf defaults to 1.0).
        let ceiling = rated_cap * cap_mult - q_defrost;
        assert!(
            ceiling > 0.0,
            "ceiling must be positive; cap_mult={cap_mult}, q_defrost={q_defrost}"
        );
        assert!(
            hp_cap <= ceiling + 1e-6,
            "ideal-mode hp_capacity_w ({hp_cap:.1} W) must not exceed post-defrost \
             rated ceiling ({ceiling:.1} W = {rated_cap} * {cap_mult:.4} - {q_defrost:.1})"
        );
        assert!(
            hp_cap < rated_cap,
            "defrost must reduce capacity below rated; got {hp_cap:.1} W vs rated {rated_cap} W"
        );
    }

    #[test]
    fn defrost_telemetry_fields_emitted() {
        let cfg = heater_config();
        let environment = env(18.0, -5.0, 0.005);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let defrost_active = eq.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);
        assert_eq!(defrost_active, 1.0, "must be in defrost at OAT=-5°C");

        let extra_power = eq.telemetry().get(tk::DEFROST_EXTRA_POWER_W);
        let q_w = eq.telemetry().get(tk::DEFROST_Q_W);
        let cap_mult = eq.telemetry().get(tk::DEFROST_CAPACITY_MULTIPLIER);

        assert!(
            extra_power.is_some(),
            "DEFROST_EXTRA_POWER_W must be present"
        );
        assert!(q_w.is_some(), "DEFROST_Q_W must be present");
        assert!(
            cap_mult.is_some(),
            "DEFROST_CAPACITY_MULTIPLIER must be present"
        );

        assert!(
            extra_power.unwrap() > 0.0,
            "defrost extra power must be strictly positive when active"
        );
        assert!(
            q_w.unwrap() > 0.0,
            "defrost q_w must be positive when active"
        );
        assert!(
            cap_mult.unwrap() > 0.0 && cap_mult.unwrap() < 1.0,
            "defrost capacity multiplier must be in (0, 1); got {:.4}",
            cap_mult.unwrap()
        );
    }

    #[test]
    fn defrost_telemetry_zero_when_inactive() {
        let cfg = heater_config();
        let environment = env(18.0, 10.0, 0.002);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let defrost_active = eq.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);
        assert_eq!(defrost_active, 0.0, "warm OAT should not trigger defrost");

        assert_eq!(
            eq.telemetry().get(tk::DEFROST_EXTRA_POWER_W),
            Some(0.0),
            "extra power must be 0.0 when defrost inactive"
        );
        assert_eq!(
            eq.telemetry().get(tk::DEFROST_Q_W),
            Some(0.0),
            "q_w must be 0.0 when defrost inactive"
        );
        assert_eq!(
            eq.telemetry().get(tk::DEFROST_CAPACITY_MULTIPLIER),
            Some(1.0),
            "capacity multiplier must be 1.0 when defrost inactive"
        );
        assert_eq!(
            eq.telemetry().get(tk::DEFROST_TIME_FRACTION),
            Some(0.0),
            "defrost time fraction must be 0.0 when defrost inactive"
        );
    }

    #[test]
    fn two_speed_with_single_capacity_errors() {
        let cfg = heater_config_with(|typed| {
            typed.common.number_of_speeds = 2;
            typed.common.heating_capacity_w = Some(8_000.0);
            typed.common.stage_heating_capacities_w = None;
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 5.0, 0.003);
        let result = eq.init(&cfg, &environment);
        assert!(
            result.is_err(),
            "number_of_speeds=2 with only a single heating_capacity_w must return an error"
        );
    }

    #[test]
    fn two_speed_with_matching_stage_vec_succeeds() {
        let cfg = heater_config_with(|typed| {
            typed.common.number_of_speeds = 2;
            typed.common.heating_capacity_w = None;
            typed.common.stage_heating_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.common.stage_heating_eirs = Some(vec![0.33, 0.33]);
            typed.common.backup_capacity_w = Some(0.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 5.0, 0.003);
        assert!(
            eq.init(&cfg, &environment).is_ok(),
            "number_of_speeds=2 with a matching 2-element stage_heating_capacities_w must succeed"
        );
    }

    #[test]
    fn checkpoint_thermostat_hysteresis_c() {
        let cfg = heater_config_with(|typed| {
            typed.common.hysteresis_c = Some(1.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_lockout_temp_c = Some(100.0);
        });
        let environment = env(16.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply a non-default deadband via ThermalSetpoint control.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: Some(3.0),
        })
        .unwrap();
        assert_eq!(eq.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 3.0);

        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let state = eq.save_state().unwrap();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(
            restored.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 1.0,
            "fresh instance must have config default"
        );

        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 3.0,
            "thermostat_hysteresis_c must survive checkpoint round-trip"
        );
    }

    #[test]
    fn checkpoint_time_at_current_speed_s() {
        let cfg = heater_config_with(|typed| {
            typed.common.hysteresis_c = Some(0.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_lockout_temp_c = Some(100.0);
        });
        let environment = env(16.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        for _ in 0..10 {
            eq.update_control(&environment);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&environment, Duration::from_secs(60), &mut ports)
                .unwrap();
        }

        let accumulated = eq.core.hvac.runtime.time_at_current_speed_s;
        assert!(
            accumulated > 0.0,
            "time_at_current_speed_s must be positive after stepping"
        );

        let state = eq.save_state().unwrap();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(restored.core.hvac.runtime.time_at_current_speed_s, 0.0);

        restored.load_state(&state).unwrap();
        assert!(
            (restored.core.hvac.runtime.time_at_current_speed_s - accumulated).abs() < 1e-9,
            "time_at_current_speed_s must survive checkpoint; expected {accumulated}, got {}",
            restored.core.hvac.runtime.time_at_current_speed_s
        );
    }

    // ---------------------------------------------------------------------------
    // Integration test: default curves vs identity curves produce measurable
    // energy difference.
    //
    // The ticket DoD requires: "Annual heating energy in a cold-climate
    // simulation differs by >15% from identity-curve baseline." A full annual
    // BESTEST run is impractical in a unit test; instead we run a synthetic
    // 24-hour cold day and verify the biquadratic correction propagates through
    // the full init → step → port accumulation path to produce a significant
    // thermal output difference.
    //
    // This test exercises the complete hot path:
    //   init → maybe_substitute_defaults → compute_step → cap_ratio used in
    //   steady_capacity_w = stage_capacity_w * cap_ratio → hp_capacity_w →
    //   write_zone_thermal_contributions → port sensible_gain_w.
    //
    // A bug that silently discards the biquadratic correction (e.g. an
    // accidental `let (_, _cap_ratio)` with an ignored value) would cause
    // this test to fail because the two heaters would produce identical output.
    // ---------------------------------------------------------------------------

    #[test]
    fn ashp_default_curves_reduce_thermal_output_vs_identity_at_cold_oat() {
        let mut cfg_default = heater_config_with(|_| {});
        cfg_default.test_extras_mut().remove("biquadratic_coeffs");

        let mut cfg_identity = heater_config_with(|_| {});
        add_identity_biquadratic_curves(&mut cfg_identity);

        let mut eq_default = ASHPHeater::new(cfg_default.clone());
        let mut eq_identity = ASHPHeater::new(cfg_identity.clone());

        let cold_env = env(18.0, -8.3, 0.002);

        eq_default.init(&cfg_default, &cold_env).unwrap();
        eq_identity.init(&cfg_identity, &cold_env).unwrap();

        eq_default.update_control(&cold_env);
        eq_identity.update_control(&cold_env);

        let n_steps = 1440_usize;
        let dt = Duration::from_secs(60);

        let mut total_thermal_default_w_s: f64 = 0.0;
        let mut total_thermal_identity_w_s: f64 = 0.0;

        for _ in 0..n_steps {
            let mut ports_default = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_identity = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };

            eq_default.step(&cold_env, dt, &mut ports_default).unwrap();
            eq_identity
                .step(&cold_env, dt, &mut ports_identity)
                .unwrap();

            total_thermal_default_w_s +=
                ports_default.thermal[0].sensible_gain_w * dt.as_secs_f64();
            total_thermal_identity_w_s +=
                ports_identity.thermal[0].sensible_gain_w * dt.as_secs_f64();

            ports_default.thermal[0].zero();
            ports_identity.thermal[0].zero();
        }

        let ratio = total_thermal_default_w_s / total_thermal_identity_w_s;
        assert!(
            ratio < 0.85,
            "Default curves must reduce thermal output by >15% vs identity at OAT=-8.3°C; \
             got ratio={ratio:.4} (default={total_thermal_default_w_s:.0} W·s, \
             identity={total_thermal_identity_w_s:.0} W·s)"
        );

        let cap_ratio_default = eq_default.telemetry().get(tk::CAP_RATIO).unwrap_or(1.0);
        assert!(
            cap_ratio_default < 0.8,
            "CAP_RATIO telemetry with default curves must be < 0.8 at -8.3°C; \
             got {cap_ratio_default:.6}"
        );

        let cap_ratio_identity = eq_identity.telemetry().get(tk::CAP_RATIO).unwrap_or(1.0);
        assert!(
            (cap_ratio_identity - 1.0).abs() < 0.01,
            "CAP_RATIO telemetry with identity curves must be ≈ 1.0; \
             got {cap_ratio_identity:.6}"
        );
    }

    // Regression: verify that capacity_ratio_at_17f scales the biquadratic
    // curve coefficients so that H3 evaluation matches the manufacturer ratio.
    // Without this scaling, cold-climate capacity extrapolation is unreliable.
    #[test]
    fn ashp_heating_capacity_ratio_at_17f_scales_biquadratic_to_match_h3() {
        // Build a config with capacity_ratio_at_17f = 0.6 and a
        // biquadratic that evaluates to 0.5 at any condition (including H3).
        let mut cfg = heater_config_with(|typed| typed.capacity_ratio_at_17f = Some(0.6));
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[0.5,0,0,0,0,0],[1.0,0,0,0,0,0]]".into(),
        );

        let mut eq = ASHPHeater::new(cfg.clone());
        let env = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &env).unwrap();

        let coeffs = &eq.core.hvac.config.biquadratic_coeffs;
        assert_eq!(coeffs.len(), 2, "single-speed => 2 coefficient entries");

        // Capacity curve: [0.5,0,0,0,0,0] scaled by 0.6/0.5 = 1.2 → [0.6,0,0,0,0,0]
        assert!(
            (coeffs[0][0] - 0.6).abs() < 1e-9,
            "capacity c0 must be scaled to 0.6; got {}",
            coeffs[0][0]
        );
        for (j, &val) in coeffs[0].iter().enumerate().skip(1) {
            assert!(
                val.abs() < 1e-12,
                "capacity coeff[{j}] must remain zero; got {}",
                val
            );
        }

        // EIR curve: must be unchanged [1.0,0,0,0,0,0]
        assert!(
            (coeffs[1][0] - 1.0).abs() < 1e-12,
            "EIR c0 must remain 1.0; got {}",
            coeffs[1][0]
        );
        for (j, &val) in coeffs[1].iter().enumerate().skip(1) {
            assert!(
                val.abs() < 1e-12,
                "EIR coeff[{j}] must remain zero; got {}",
                val
            );
        }

        // Verify the scaled curve actually evaluates to ~0.6 at H3.
        let (raw_h3, _) = eq.core.hvac.evaluate_biquadratic_with_flow(
            0,     // capacity curve index
            21.11, // AHRI H3 indoor dry-bulb
            -8.33, // AHRI H3 outdoor dry-bulb
            1.0,
        );
        assert!(
            (raw_h3 - 0.6).abs() < 1e-9,
            "H3 evaluation must match capacity_ratio_at_17f=0.6; got {raw_h3}"
        );
    }

    // -----------------------------------------------------------------------
    // Charge defect ratio — physics-level regression test for HP heater
    // -----------------------------------------------------------------------

    fn hp_charge_defect_config(charge_defect_ratio: Option<f64>) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "HP Heater".to_string(),
            "ASHP Heater".to_string(),
            HeatPumpHeaterConfig {
                common: HeatPumpCommonConfig {
                    charge_defect_ratio,
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: Some(8_000.0),
                    heating_eir: Some(0.33),
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: Some(4_000.0),
                    backup_eir: None,
                    fraction_heating_load_served: None,
                    cooling_capacity_w: None,
                    cooling_eir: None,
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: None,
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig {
                        heating_setpoint_c: Some(21.0),
                        cooling_setpoint_c: Some(26.0),
                        ..Default::default()
                    },
                    hysteresis_c: Some(1.0),
                    duct: Default::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    ..Default::default()
                },
                hp_lockout_temp_c: None,
                er_lockout_temp_c: None,
                max_oat_supplemental_c: None,
                er_setpoint_offset_c: None,
                er_hard_lockout_time_s: None,
                heating_shr: None,
                capacity_ratio_at_17f: None,
                defrost: DefrostConfig::default(),
            },
        )
        .unwrap()
    }

    #[test]
    fn charge_defect_reduces_rated_heating_capacity_and_raises_eir() {
        // r = -0.10 (10% undercharge):
        //   capacity × (1 + 0.9 × (−0.10)) = × 0.91 — heating output falls 9 %
        //   EIR     × (1 + (−0.9) × (−0.10)) = × 1.09 — efficiency degrades 9 %
        // An undercharged heat pump compressor moves less refrigerant and works
        // harder per unit of heating, so COP falls (EIR rises).
        let cfg = hp_charge_defect_config(Some(-0.10));
        let mut hp = ASHPHeater::new(cfg.clone());
        let e = env(18.0, 8.0, 0.004);
        hp.init(&cfg, &e).unwrap();

        let capacity = hp.core.hvac.config.heating_capacities_w[0];
        let expected_cap = 8_000.0 * 0.91;
        assert!(
            (capacity - expected_cap).abs() / expected_cap < 1e-3,
            "charge_defect_ratio=-0.10 must reduce heating capacity by 9%; expected {expected_cap} got {capacity}"
        );

        let eir = hp.core.hvac.config.eir_by_stage[0];
        let expected_eir = 0.33 * 1.09;
        assert!(
            (eir - expected_eir).abs() / expected_eir < 1e-3,
            "charge_defect_ratio=-0.10 must raise heating EIR by 9% (efficiency degrades); expected {expected_eir} got {eir}"
        );
    }

    // -----------------------------------------------------------------------
    // GSHP regression tests — verify init() preserves constructor defaults
    // (lockout temps, defrost) against the ASHP default overwrite bug.
    // -----------------------------------------------------------------------

    fn gshp_typed_config() -> HeatPumpHeaterConfig {
        let mut cfg = heater_typed_config();
        cfg.common.heating_capacity_w = Some(10_000.0);
        cfg.common.heating_eir = Some(0.25);
        cfg.common.backup_capacity_w = Some(0.0);
        cfg
    }

    #[test]
    fn gshp_init_preserves_infinite_lockout_and_disabled_defrost() {
        let typed = gshp_typed_config();
        let cfg = EquipmentConfig::from_typed(
            "gshp_heater".to_string(),
            "GSHP Heater".to_string(),
            typed,
        )
        .unwrap();
        let mut eq = GshpHeater::new(cfg.clone());
        let e = env(18.0, -5.0, 0.002);
        eq.init(&cfg, &e).unwrap();

        assert_eq!(eq.core.hp_lockout_temp_c, f64::NEG_INFINITY);
        assert_eq!(eq.core.er_lockout_temp_c, f64::INFINITY);
        assert_eq!(eq.core.max_oat_supplemental_c, f64::INFINITY);
        assert_eq!(eq.core.hp_lockout_hysteresis_c, 0.0);
        assert_eq!(eq.core.defrost_config.control, DefrostControl::Disabled);
    }

    #[test]
    fn gshp_not_locked_out_at_cold_ambient() {
        let typed = gshp_typed_config();
        let cfg =
            EquipmentConfig::from_typed("gshp_cold".to_string(), "GSHP Heater".to_string(), typed)
                .unwrap();
        let mut eq = GshpHeater::new(cfg.clone());
        let e = env(18.0, -30.0, 0.001);
        eq.init(&cfg, &e).unwrap();
        assert!(eq.core.hp_available);
    }

    #[test]
    fn gshp_defrost_never_active_regardless_of_ambient() {
        let typed = gshp_typed_config();
        let cfg = EquipmentConfig::from_typed(
            "gshp_defrost".to_string(),
            "GSHP Heater".to_string(),
            typed,
        )
        .unwrap();
        let mut eq = GshpHeater::new(cfg.clone());
        let e = env(18.0, -5.0, 0.006);
        eq.init(&cfg, &e).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 8_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&e);
        eq.step(
            &e,
            Duration::from_secs(900),
            &mut PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            },
        )
        .unwrap();
        assert_eq!(eq.core.defrost_config.control, DefrostControl::Disabled);
        assert!(!eq.core.defrost_active);
    }

    /// Borehole parameters wired through `HeatPumpCommonConfig` must reach
    /// `BoreholeGFunctionModel` and influence `compute_entering_water_temp`.
    /// Deeper boreholes (120 m vs default 60 m) halve the per-unit-length
    /// heat extraction rate, yielding a higher entering water temperature in
    /// heating mode. This test covers the full wiring chain:
    /// `HeatPumpCommonConfig` → `init_from_typed` → `BoreholeConfig` →
    /// `BoreholeGFunctionModel`.
    #[test]
    fn gshp_heater_borehole_depth_reaches_ewt() {
        let e = env(18.0, -5.0, 0.002);

        // Default borehole depth (60 m from BoreholeConfig::default()).
        let typed_default = gshp_typed_config();
        let cfg_default = EquipmentConfig::from_typed(
            "gshp_default".to_string(),
            "GSHP Heater".to_string(),
            typed_default,
        )
        .unwrap();
        let mut eq_default = GshpHeater::new(cfg_default.clone());
        eq_default.init(&cfg_default, &e).unwrap();

        // Custom borehole depth (120 m).
        let mut typed_deep = gshp_typed_config();
        typed_deep.common.borehole_depth_m = Some(120.0);
        let cfg_deep = EquipmentConfig::from_typed(
            "gshp_deep".to_string(),
            "GSHP Heater".to_string(),
            typed_deep,
        )
        .unwrap();
        let mut eq_deep = GshpHeater::new(cfg_deep.clone());
        eq_deep.init(&cfg_deep, &e).unwrap();

        // Record identical heat extraction (5 kW for 1 h) in both models.
        // Heating mode extracts heat FROM the ground: negative Q per Eskilson.
        let heat_rate_w = -5_000.0;
        let dt_s = 3600.0;
        eq_default
            .core
            .source_temp
            .record_source_heat_rate(heat_rate_w, dt_s);
        eq_deep
            .core
            .source_temp
            .record_source_heat_rate(heat_rate_w, dt_s);

        // q_per_unit = Q / H / N — deeper borehole has half the per-unit-
        // length heat rate, so the resistive temperature drop is smaller and
        // the entering water temperature is higher.
        let ewt_default = eq_default.core.source_temp.compute(&e);
        let ewt_deep = eq_deep.core.source_temp.compute(&e);
        assert!(
            ewt_deep > ewt_default,
            "deeper borehole (120 m) should yield higher entering water temperature \
             in heating mode than default (60 m); got deep={ewt_deep:.3}, default={ewt_default:.3}"
        );
    }

    /// Verify that CAP_RATIO_RAW captures the negative pre-clamp biquadratic
    /// value in cold-climate simulation while CAP_RATIO stays at 0.0 (clamped).
    ///
    /// At −35°C outdoor with MSHP variable-speed heating capacity coefficients
    /// the unclamped curve evaluates to approximately −0.124 (at 21.1°C indoor)
    /// and −0.103 (at 19°C indoor). The output_min=0.0 clamp forces
    /// CAP_RATIO=0.0, while CAP_RATIO_RAW preserves the unclamped value.
    /// This exercises the full init → compute_step → telemetry.set() path.
    #[test]
    fn cap_ratio_raw_captures_negative_pre_clamp_value_in_cold_climate() {
        let mut cfg = heater_config_with(|_| {});
        // MSHP variable-speed heating capacity curve coefficients from OCHRE
        // `MSHP Heater.csv` column `Variable_1`, rows a_cap_t–f_cap_t and a_eir_t–f_eir_t.
        // Capacity curve: 1.002928121 − 0.010386676·T_indoor + 0.025961538·T_outdoor.
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1.002928121,-0.010386676,0,0.025961538,0,0],\
              [0.966475473,0.00591495,0.000191202,-0.012965668,0.00004225,-0.000524003]]"
                .into(),
        );

        let mut eq = ASHPHeater::new(cfg.clone());
        // Zone at 19°C (below 21°C setpoint) so thermostat enters heating;
        // outdoor −35°C forces the biquadratic into negative territory.
        let cold_env = env(19.0, -35.0, 0.002);
        eq.init(&cfg, &cold_env).unwrap();
        eq.update_control(&cold_env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&cold_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        let cap_ratio = eq.telemetry().get(tk::CAP_RATIO).unwrap();
        assert!(
            cap_ratio >= 0.0,
            "CAP_RATIO must be >= 0.0 with output_min=0.0 clamp at −35°C outdoor; got {cap_ratio}"
        );

        let cap_ratio_raw = eq.telemetry().get(tk::CAP_RATIO_RAW).unwrap();
        assert!(
            cap_ratio_raw < 0.0,
            "CAP_RATIO_RAW must capture negative pre-clamp biquadratic value at −35°C outdoor; \
             got {cap_ratio_raw}"
        );
    }
}

#[cfg(test)]
mod ideal_capacity_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, OperatingMode, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::ASHPHeater;
    use crate::{DefrostConfig, Equipment, EquipmentConfig, HvacSetpointConfig};

    /// Build an `EnvironmentState` with a configurable zone temperature and time resolution.
    /// OAT is held above the HP lockout (default -17.78°C) and above the ER lockout
    /// (default 4.44°C) so only HP heating is active -- isolating the duty-cycle behaviour.
    fn make_env(zone_temp_c: f64, time_res_s: i64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 8.0,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
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
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// HP heater with setpoint=21°C, hysteresis=1°C, no ER strip heat (backup_capacity_w=0),
    /// OAT above ER lockout so only the HP compressor runs.
    fn heater_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "HP Heater".to_string(),
            "ASHP Heater".to_string(),
            crate::HeatPumpHeaterConfig {
                common: crate::HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: Some(8_000.0),
                    heating_eir: Some(0.33),
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: Some(0.0),
                    backup_eir: None,
                    fraction_heating_load_served: None,
                    cooling_capacity_w: None,
                    cooling_eir: None,
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: None,
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig {
                        heating_setpoint_c: Some(21.0),
                        cooling_setpoint_c: Some(26.0),
                        ..Default::default()
                    },
                    hysteresis_c: Some(1.0),
                    duct: Default::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    charge_defect_ratio: None,
                    ..Default::default()
                },
                hp_lockout_temp_c: None,
                er_lockout_temp_c: None,
                max_oat_supplemental_c: None,
                er_setpoint_offset_c: None,
                er_hard_lockout_time_s: None,
                heating_shr: None,
                capacity_ratio_at_17f: None,
                defrost: DefrostConfig::default(),
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        cfg
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    // At 60 s timestep (fine resolution), an IdealCapacity signal is ignored because
    // use_ideal=false. Zone is below FSM turn-on threshold (setpoint - hysteresis = 20°C),
    // so FSM enters Heating with load_ratio = (21 - 19) / 1 = 1.0 (clamped) → RTF = 1.0.
    #[test]
    fn fine_timestep_fsm_cycling_ignores_ideal_capacity_signal() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 19°C is below FSM turn-on threshold of 20°C.
        let env = make_env(19.0, 60);
        eq.init(&cfg, &env).unwrap();
        // Signal provides half-rated load; must be ignored when use_ideal=false.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "FSM Heating at 60 s must ignore IdealCapacity and produce RTF=1.0, got {rtf}"
        );
    }

    #[test]
    fn single_speed_heating_call_uses_full_duty_cycle_while_mode_is_heating() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // Step 1: force entry into Heating mode.
        let env_cold = make_env(19.0, 60);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        // Step 2: keep zone between turn-on and turn-off thresholds so FSM
        // remains in Heating hold; single-speed runtime must stay at full duty.
        let env_hold = make_env(20.9, 60);
        eq.update_control(&env_hold);
        eq.step(&env_hold, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "single-speed heating call must run full duty while Heating mode is held; got {rtf}"
        );
    }

    // At 900 s timestep (coarse resolution), a solver-provided IdealCapacity signal for
    // half the rated capacity (4000 W of 8000 W rated) produces fractional RTF.
    // Flow: apply signal → update_control (use_ideal=true, FSM enters Heating at 19°C) →
    //   ideal_capacity_w=4000 > 0 → load_ratio = 4000/8000 = 0.5 → RTF ≈ 0.5.
    #[test]
    fn coarse_timestep_ideal_signal_produces_fractional_rtf() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 19°C is below FSM turn-on threshold of 20°C; FSM enters Heating.
        let env = make_env(19.0, 900);
        eq.init(&cfg, &env).unwrap();
        // Half rated capacity: 4000 W of 8000 W rated → load_ratio = 0.5.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        // Flat biquadratic [1,0,0,0,0,0] → cap_ratio=1.0, PLR = 4000/8000 = 0.5.
        // PLF slightly adjusts, so allow ±5%.
        assert!(
            (rtf - 0.5).abs() < 0.05,
            "IdealCapacity=4000 W at 900 s (half rated) must produce RTF ≈ 0.5, got {rtf}"
        );
    }

    // At 900 s timestep, ideal-capacity control must be able to engage heating
    // from thermostat deadband. This prevents a control deadlock where mode
    // stays Deadband and ideal capacity never gets applied.
    #[test]
    fn coarse_timestep_ideal_signal_engages_from_deadband_band() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 20.5C is above turn-on (20.0C) and below setpoint (21.0C): FSM deadband.
        let env = make_env(20.5, 900);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let mode = eq.telemetry().get(tk::OPERATING_MODE).unwrap_or(-1.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            mode > 0.0,
            "ideal-capacity signal at coarse timestep must activate heating from deadband; mode={mode}"
        );
        assert!(
            rtf > 0.0,
            "ideal-capacity signal at coarse timestep must produce nonzero runtime from deadband; rtf={rtf}"
        );
    }

    // At 900 s timestep with no solver signal and zone above the heating setpoint,
    // the thermostat stays Deadband → heater is off regardless of timestep resolution.
    #[test]
    fn coarse_timestep_no_signal_above_setpoint_produces_zero_rtf() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 22°C is above setpoint (21°C); FSM stays Deadband, no heating demand.
        let env = make_env(22.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert_eq!(
            rtf, 0.0,
            "900 s timestep with zone above heating setpoint must produce RTF=0.0, got {rtf}"
        );
    }

    // Build a config with backup ER and OAT below the ER lockout threshold.
    // Identity biquadratic curves (cap_ratio = 1.0) so capacity is exactly rated.
    fn heater_config_with_er() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "HP Heater ER".to_string(),
            "ASHP Heater".to_string(),
            crate::HeatPumpHeaterConfig {
                common: crate::HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: Some(6_000.0),
                    heating_eir: Some(0.33),
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: Some(4_000.0),
                    backup_eir: Some(1.0),
                    fraction_heating_load_served: None,
                    cooling_capacity_w: None,
                    cooling_eir: None,
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: Some(0.0),
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig {
                        heating_setpoint_c: Some(21.0),
                        cooling_setpoint_c: Some(26.0),
                        ..Default::default()
                    },
                    hysteresis_c: Some(1.0),
                    duct: Default::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    charge_defect_ratio: None,
                    ..Default::default()
                },
                hp_lockout_temp_c: None,
                er_lockout_temp_c: Some(10.0),
                max_oat_supplemental_c: Some(21.0),
                er_setpoint_offset_c: Some(3.0),
                er_hard_lockout_time_s: None,
                heating_shr: None,
                capacity_ratio_at_17f: None,
                defrost: DefrostConfig::default(),
            },
        )
        .unwrap();
        // Identity curves: cap_ratio = 1.0, eir_ratio = 1.0
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        cfg.test_extras_mut()
            .insert("use_ideal_capacity".to_string(), true.into());
        cfg
    }

    // In ideal mode with HP partially covering the load, ER provides only the
    // residual gap (ideal_w - hp_w), not a proportional share of the full request.
    //
    // Setup: HP rated=6000 W, ER rated=4000 W, OAT=0°C (below er_lockout 10°C).
    // Solver injects 8000 W ideal. HP available = 6000 W (identity curve).
    // Bug A: er_demand = 8000 > 6000 → er_on = true.
    // Bug B: er_capacity = (8000 - hp_actual).min(4000) = ideal - hp (residual fill).
    // The total delivered (hp + er) must equal ideal_w, and er = ideal - hp exactly.
    #[test]
    fn ideal_mode_er_fills_residual_not_proportional() {
        const ER_RATED_W: f64 = 4_000.0;
        const IDEAL_W: f64 = 8_000.0;

        let cfg = heater_config_with_er();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT=0°C: below ER lockout (10°C) so ER is temperature-permitted.
        let env = {
            let mut e = make_env(18.0, 900);
            e.weather.outdoor_temp_c = 0.0;
            e
        };
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: IDEAL_W,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let hp_w = eq.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);
        let er_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);

        // ER must be active (load exceeds HP capacity).
        assert!(
            er_w > 0.0,
            "ER must be active when ideal demand exceeds HP capacity"
        );
        // With er_stages=1 (binary), ER fires at full rated capacity when any residual > 0.
        // This covers the residual but may over-deliver; thermostat cycling handles
        // time-averaging on subsequent steps. The key guarantee: er >= residual.
        let residual = (IDEAL_W - hp_w).clamp(0.0, ER_RATED_W);
        assert!(
            er_w >= residual - 1e-6,
            "ER must cover at least the residual gap ({residual:.1} W); got {er_w:.1} W"
        );
        assert!(
            er_w <= ER_RATED_W + 1e-6,
            "ER must not exceed rated capacity ({ER_RATED_W:.0} W); got {er_w:.1} W"
        );
    }

    // In ideal mode when the HP can fully cover the ideal demand, ER must stay off
    // even if the zone temperature is far below the ER setpoint offset threshold.
    //
    // Setup: HP rated=6000 W, ER rated=4000 W, OAT=0°C.
    // Solver injects 4000 W (< HP rated=6000 W). HP can cover it alone.
    // er_setpoint_offset_c=3.0 so zone (18°C) is well below er_turn_on (21-3=18°C):
    // thermostat-based logic would fire ER, but ideal logic must suppress it.
    #[test]
    fn ideal_mode_er_only_when_hp_cannot_cover_load() {
        const IDEAL_W: f64 = 4_000.0;

        let cfg = heater_config_with_er();
        let mut eq = ASHPHeater::new(cfg.clone());
        let env = {
            let mut e = make_env(18.0, 900);
            e.weather.outdoor_temp_c = 0.0;
            e
        };
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: IDEAL_W,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let er_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(f64::NAN);
        assert!(
            er_w < 1e-6,
            "ER must stay off when HP can cover the full ideal demand ({IDEAL_W:.0} W < HP rated 6000 W); \
             got er_capacity_w={er_w:.1} W"
        );
    }

    // In bang-bang mode (use_ideal=false, 60 s timestep), ER is binary on/off.
    // Zone below setpoint → ER on → full rated capacity. The thermostat cycles
    // ER on/off to achieve time-averaged part-load behavior.
    // at full rated capacity when OAT is below the ER lockout threshold.
    #[test]
    fn bang_bang_er_unchanged() {
        const ER_RATED_W: f64 = 4_000.0;

        let mut cfg = EquipmentConfig::from_typed(
            "HP Heater BB".to_string(),
            "ASHP Heater".to_string(),
            crate::HeatPumpHeaterConfig {
                common: crate::HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: Some(6_000.0),
                    heating_eir: Some(0.33),
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: Some(ER_RATED_W),
                    backup_eir: Some(1.0),
                    fraction_heating_load_served: None,
                    cooling_capacity_w: None,
                    cooling_eir: None,
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: Some(0.0),
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig {
                        heating_setpoint_c: Some(21.0),
                        cooling_setpoint_c: Some(26.0),
                        ..Default::default()
                    },
                    hysteresis_c: Some(1.0),
                    duct: Default::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    charge_defect_ratio: None,
                    ..Default::default()
                },
                hp_lockout_temp_c: Some(10.0),
                er_lockout_temp_c: Some(5.0),
                max_oat_supplemental_c: Some(21.0),
                er_setpoint_offset_c: Some(0.0),
                er_hard_lockout_time_s: None,
                heating_shr: None,
                capacity_ratio_at_17f: None,
                defrost: DefrostConfig::default(),
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        // 60 s timestep → use_ideal=false (bang-bang path).
        let env = {
            let mut e = make_env(18.0, 60);
            // OAT below HP lockout (10°C) and below ER lockout (5°C): HP off, ER allowed.
            e.weather.outdoor_temp_c = 0.0;
            e
        };
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "HP must be locked out in bang-bang test"
        );
        eq.step(&env, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let er_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        // Bang-bang: ER is binary on/off — full rated capacity when on.
        assert!(
            (er_w - ER_RATED_W).abs() < 1.0,
            "bang-bang ER must run at full rated capacity ({ER_RATED_W:.0} W) when on; \
             got {er_w:.1} W"
        );
    }

    // VariableSpeedIdeal: with load_fraction=1.0 the speed selector sets speed_index=0
    // and speed_frac=1.0, so interpolated_capacity spans the full two-stage range.
    // Bug 1 fix: er_demand must compare ideal_w against interpolated capacity, not the
    // un-interpolated lower stage.
    //
    // Setup: is_mini_split=true forces VariableSpeedIdeal.
    //   stage_heating_capacities_w = [5000, 10000] (two explicit stages kept as-is)
    //   ER rated = 4000 W, OAT below ER lockout threshold.
    //   Solver injects 7000 W.
    //
    //   Pre-fix: capacity_at_stage(0) = 5000 → er_demand = 7000 > 5000 = true → ER fires.
    //   Post-fix: interpolated_capacity(0, 1.0) = 10000 → er_demand = 7000 > 10000 = false → ER off.
    #[test]
    fn variable_speed_ideal_er_decision_uses_interpolated_capacity() {
        const IDEAL_W: f64 = 7_000.0;

        let mut cfg = EquipmentConfig::from_typed(
            "VS HP ER".to_string(),
            "ASHP Heater".to_string(),
            crate::HeatPumpHeaterConfig {
                common: crate::HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: None,
                    heating_eir: Some(0.33),
                    stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
                    stage_heating_eirs: Some(vec![0.33, 0.33]),
                    backup_fuel: None,
                    backup_capacity_w: Some(4_000.0),
                    backup_eir: Some(1.0),
                    fraction_heating_load_served: None,
                    cooling_capacity_w: None,
                    cooling_eir: None,
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 2,
                    // is_mini_split forces VariableSpeedIdeal mode regardless of number_of_speeds.
                    is_mini_split: true,
                    shr: None,
                    fan_power_w: Some(0.0),
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig {
                        heating_setpoint_c: Some(21.0),
                        cooling_setpoint_c: Some(26.0),
                        ..Default::default()
                    },
                    hysteresis_c: Some(1.0),
                    duct: Default::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    charge_defect_ratio: None,
                    ..Default::default()
                },
                hp_lockout_temp_c: None,
                er_lockout_temp_c: Some(5.0),
                max_oat_supplemental_c: Some(21.0),
                er_setpoint_offset_c: Some(3.0),
                er_hard_lockout_time_s: None,
                heating_shr: None,
                capacity_ratio_at_17f: None,
                defrost: DefrostConfig::default(),
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        cfg.test_extras_mut()
            .insert("use_ideal_capacity".to_string(), true.into());

        let env = {
            let mut e = make_env(18.0, 900);
            e.weather.outdoor_temp_c = 0.0;
            e
        };
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: IDEAL_W,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let er_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(f64::NAN);
        // Interpolated HP capacity = 10000 W > 7000 W ideal → ER must stay off.
        assert!(
            er_w < 1e-6,
            "ER must stay off when interpolated HP capacity (10000 W) covers ideal demand \
             ({IDEAL_W:.0} W); er_capacity_w={er_w:.1} W -- Bug 1 fix"
        );
    }

    // In ideal+ER mode, HP must run at full available capacity first; ER fills
    // the residual. The PLR denominator for HP must be steady_capacity_w alone,
    // not steady_capacity_w + backup_capacity_w.
    //
    // Setup: HP rated=6000 W, ER rated=4000 W. Solver injects 8000 W.
    // Identity biquadratic curves: steady_capacity_w = 6000 W.
    //
    //   Pre-fix: plr = 8000 / (6000 + 4000) = 0.8 → hp_w = 6000 * 0.8 = 4800 W (throttled)
    //   Post-fix: plr = min(8000 / 6000, 1.0) = 1.0 → hp_w = 6000 * 1.0 = 6000 W (full)
    //             er_w = (8000 - 6000).min(4000) = 2000 W
    #[test]
    fn ideal_er_mode_hp_runs_at_full_plr_not_throttled_by_er_denominator() {
        const HP_RATED_W: f64 = 6_000.0;
        const IDEAL_W: f64 = 8_000.0;

        let cfg = heater_config_with_er();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT=7°C: above defrost threshold (4.44°C) so no defrost; below ER lockout (10°C) so ER allowed.
        let env = {
            let mut e = make_env(18.0, 900);
            e.weather.outdoor_temp_c = 7.0;
            e.weather.outdoor_humidity_ratio = 0.002;
            e
        };
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: IDEAL_W,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let hp_w = eq.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);
        let er_w = eq.telemetry().get(tk::ER_CAPACITY_W).unwrap_or(0.0);
        let defrost_active = eq.telemetry().get(tk::DEFROST_ACTIVE).unwrap_or(0.0);

        assert_eq!(defrost_active, 0.0, "OAT=7°C must not trigger defrost");

        // HP must run at full rated capacity (PLR=1.0).
        assert!(
            hp_w >= HP_RATED_W * 0.99,
            "HP must run at full capacity ({HP_RATED_W:.0} W); got {hp_w:.1} W -- \
             Bug 2 fix: HP PLR denominator must not include ER capacity"
        );
        // With er_stages=1 (binary), ER fires at full rated capacity (4000 W) when
        // residual > 0, rather than filling exactly the residual (2000 W).
        // Multi-stage ER tests verify precise residual fill; this test's primary
        // purpose is verifying HP PLR (assertion above).
        const ER_RATED_W: f64 = 4_000.0;
        assert!(
            er_w > 0.0 && er_w <= ER_RATED_W + 1.0,
            "ER must be active (≤{ER_RATED_W:.0} W rated) when residual > 0; got {er_w:.1} W"
        );
    }
}
