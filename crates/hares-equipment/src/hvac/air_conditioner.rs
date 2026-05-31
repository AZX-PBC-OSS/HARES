//! Central and room air conditioner models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;
use hares_physics::ground::SourceTemperature;
use hares_physics::units::{power_kw_to_w, power_w_to_kw};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, ThermalCategory,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

#[cfg(test)]
use crate::HvacSetpointConfig;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::ac_config::{
    CentralAirConditionerConfig, RoomAcConfig, default_telemetry, load_curve_pair, telemetry_fields,
};
use super::coil_physics::{
    CoilResult, LatentDegradationParams, calculate_shr, effective_shr_with_latent_degradation,
};
use super::latent_degradation::compute_coil_ao_by_stage;
use super::speed_control::{SpeedSelection, capacity_fractions_for, interpolate_speed_stages};
use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, SpeedControlMode, ThermostatMode,
    helpers::{
        equipment_id_from_config, lookup_zone, operating_mode_code, zone_id_from_config_or_default,
    },
};

const CRANKCASE_HEATER_KW: f64 = 0.05;
const CRANKCASE_HEATER_THRESHOLD_C: f64 = 12.8;
const MIN_LOAD_FRACTION_DEADBAND_C: f64 = 0.5;

pub struct AirConditioner {
    pub(super) core: CoolingCore,
}

pub struct RoomAC {
    core: CoolingCore,
}

pub(super) struct CoolingCore {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    pub(super) hvac: HvacEquipment,
    operating_mode: OperatingMode,
    /// How the equipment determines its source-side temperature for curve evaluation.
    pub(super) source_temp: SourceTemperature,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    crankcase_heater_on: bool,
    crankcase_heater_kw: f64,
    /// Rated crankcase heater power [kW]; loaded from config, variant-dependent default.
    pub(super) crankcase_rated_kw: f64,
    /// Outdoor temperature threshold [°C] below which crankcase heater activates.
    pub(super) crankcase_threshold_c: f64,
    /// Optional polynomial coefficients [c0, c1, c2] for temperature-dependent
    /// crankcase heater capacity: effective_kw = rated_kw * (c0 + c1*T + c2*T^2).
    crankcase_capacity_curve: Option<[f64; 3]>,
    /// Runtime fraction of the cooling coil in the most recent step (PLR value).
    /// Used so the HP system can pass it as companion RTF to the heater side.
    pub(super) last_cooling_rtf: f64,
    flow_fraction_correction: f64,
    coil_ao_by_stage: Vec<f64>,
    is_room_ac: bool,
    /// Rated SHR at AHRI conditions, stored for latent degradation model.
    rated_shr: f64,
    /// Per-stage rated SHR values, if provided by config.
    pub(crate) stage_shrs: Vec<f64>,
    /// Henderson-Rengarajan latent degradation model parameters.
    /// All four fields must be > 0 (checked via `is_active()`) to enable the model.
    latent_degradation: LatentDegradationParams,
    last_adp_c: f64,
    last_bypass_factor: f64,

    /// Minimum outdoor air temperature for compressor operation [°C].
    /// EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
    pub(super) min_oat_cooling_c: f64,
    /// Transient flag: true when the current step's cooling was suppressed
    /// by the OAT lockout; consumed by step() for telemetry.
    cooling_oat_locked_out: bool,

    // --- Ideal capacity (solver-driven) ---
    /// Cached from last update_control; true when timestep >= 5 min.
    use_ideal: bool,
    /// Solver-provided ideal capacity [W]; set via IdealCapacity signal.
    ideal_capacity_w: f64,

    // --- External control signals (sticky) ---
    ctrl_duty_cycle: f64,
    ctrl_power_limit_kw: f64,
    ctrl_mode_override: Option<OperatingMode>,

    // --- Transient signals (reset each step) ---
    ctrl_load_fraction: f64,

    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duty_cycle: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AirConditionerState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    crankcase_heater_on: bool,
    startup_c_d: f64,
    startup_time_since_start_min: f64,
    plf_state: f64,
    last_speed_index: usize,
    last_speed_frac: f64,
    electric_kw: f64,
    sensible_cooling_w: f64,
    latent_cooling_w: f64,
    shr: f64,
    operating_mode_code: f64,
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
    last_adp_c: f64,
    last_bypass_factor: f64,
    thermostat_hysteresis_c: f64,
    time_at_current_speed_s: f64,
}

#[derive(Clone, Copy)]
struct PerformanceResult {
    sensible_cooling_w: f64,
    latent_cooling_w: f64,
    compressor_kw: f64,
    fan_kw: f64,
    shr: f64,
    /// Supply air dry-bulb temperature [°C] from the coil solver.
    supply_temp_c: f64,
    /// Runtime fraction (PLR / PLF) for this step -- used for crankcase accounting.
    rtf: f64,
}

impl AirConditioner {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: CoolingCore::new(config, false),
        }
    }

    /// Calculate crankcase heater power accounting for companion coil operation.
    ///
    /// For heat pump systems, the crankcase heater only draws power when neither
    /// the cooling nor the heating coil is running.
    #[cfg(test)]
    pub(super) fn crankcase_heater_power(
        &self,
        outdoor_temp_c: f64,
        cooling_rtf: f64,
        companion_heating_rtf: Option<f64>,
    ) -> f64 {
        self.core.crankcase_heater_power_internal(
            outdoor_temp_c,
            cooling_rtf,
            companion_heating_rtf,
        )
    }

    /// Set a numeric telemetry key on the inner cooling core.
    ///
    /// Used by `GshpCooler` to publish pump power telemetry after the
    /// inner step has completed.
    pub(super) fn set_telemetry(&mut self, key: &str, value: f64) {
        self.core.telemetry.set(key, value);
    }

    /// Override crankcase heater parameters after `init()`.
    /// Used by `HpCooler::mshp_cooler` to apply MSHP-specific defaults
    /// (15 W / 0 °C) when the user did not supply explicit config keys.
    pub(super) fn set_crankcase_defaults_if_unconfigured(
        &mut self,
        config: &EquipmentConfig,
        rated_kw: f64,
        threshold_c: f64,
    ) {
        let crankcase_kw_configured = config
            .typed::<CentralAirConditionerConfig>()
            .ok()
            .and_then(|cfg| cfg.crankcase_heater_kw)
            .or_else(|| {
                config
                    .typed::<RoomAcConfig>()
                    .ok()
                    .and_then(|cfg| cfg.crankcase_heater_kw)
            })
            .is_some();
        let crankcase_threshold_configured = config
            .typed::<CentralAirConditionerConfig>()
            .ok()
            .and_then(|cfg| cfg.crankcase_heater_threshold_c)
            .or_else(|| {
                config
                    .typed::<RoomAcConfig>()
                    .ok()
                    .and_then(|cfg| cfg.crankcase_heater_threshold_c)
            })
            .is_some();

        if !crankcase_kw_configured {
            self.core.crankcase_rated_kw = rated_kw;
        }
        if !crankcase_threshold_configured {
            self.core.crankcase_threshold_c = threshold_c;
        }
    }
}

impl RoomAC {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: CoolingCore::new(config, true),
        }
    }
}

impl Equipment for AirConditioner {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.core.descriptor
    }

    fn zone_id_explicit(&self) -> bool {
        self.core.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.core.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.core.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.core.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.core.step(env, dt, ports, None)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.core.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        self.core.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.core.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.core.apply_control_unchecked(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.core.ideal_target()
    }
}

impl Equipment for RoomAC {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.core.descriptor
    }

    fn zone_id_explicit(&self) -> bool {
        self.core.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.core.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.core.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.core.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.core.step(env, dt, ports, None)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.core.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        self.core.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.core.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.core.apply_control_unchecked(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.core.ideal_target()
    }
}

impl CoolingCore {
    fn apply_cooling_startup_cd(&mut self, cd: Option<f64>) {
        if let Some(cd) = cd {
            self.hvac.runtime.plf_cooling_degradation_coeff = cd;
            self.hvac.runtime.startup.c_d = cd;
        }
    }

    fn select_variable_speed_cooling(
        &mut self,
        requested_capacity_fraction: f64,
    ) -> SpeedSelection {
        let capacities = &self.hvac.config.cooling_capacities_w;
        let cap_fracs = capacity_fractions_for(capacities);
        if cap_fracs.is_empty() {
            let selection = SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0,
            };
            self.hvac.runtime.last_speed_index = selection.speed_index;
            self.hvac.runtime.last_speed_frac = selection.speed_frac;
            return selection;
        }
        if cap_fracs.len() == 1 {
            let lf = requested_capacity_fraction.clamp(0.0, 1.0);
            let selection = SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: lf,
            };
            self.hvac.runtime.last_speed_index = selection.speed_index;
            self.hvac.runtime.last_speed_frac = selection.speed_frac;
            return selection;
        }
        let selection = interpolate_speed_stages(requested_capacity_fraction, &cap_fracs, true);
        self.hvac.runtime.last_speed_index = selection.speed_index;
        self.hvac.runtime.last_speed_frac = selection.speed_frac;
        selection
    }

    fn variable_speed_point(&self, selection: SpeedSelection) -> (f64, f64, f64) {
        let stage_capacity_w =
            if self.hvac.config.cooling_capacities_w.len() > 1 && selection.speed_frac > 0.0 {
                self.hvac.interpolated_capacity(
                    &self.hvac.config.cooling_capacities_w,
                    selection.speed_index,
                    selection.speed_frac,
                )
            } else if self.hvac.config.cooling_capacities_w.len() == 1 {
                self.hvac
                    .config
                    .cooling_capacities_w
                    .first()
                    .copied()
                    .unwrap_or_default()
            } else {
                HvacEquipment::capacity_at_stage(
                    &self.hvac.config.cooling_capacities_w,
                    selection.speed_index,
                )
            };
        let stage_eir =
            if self.hvac.config.cooling_capacities_w.len() > 1 && selection.speed_frac > 0.0 {
                self.hvac
                    .interpolated_eir(selection.speed_index, selection.speed_frac)
            } else {
                self.hvac.eir_at_stage(selection.speed_index)
            };
        let part_load_ratio = if self.hvac.config.cooling_capacities_w.len() == 1
            || (selection.speed_index == 0
                && selection.speed_frac == 0.0
                && selection.part_load_ratio < 1.0)
        {
            selection.part_load_ratio.clamp(0.0, 1.0)
        } else {
            1.0
        };
        (stage_capacity_w, stage_eir, part_load_ratio)
    }

    fn new(config: EquipmentConfig, is_room_ac: bool) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let equipment_type = if is_room_ac {
            "Room AC"
        } else {
            "Air Conditioner"
        };

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::HVAC_COOLING,
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
                    | CoreCapabilities::HAS_COP,
                telemetry_fields: telemetry_fields(),
                zone_type: None,
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
                PortDeclaration::humidity(zone),
            ],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::AcCooler, zone),
            operating_mode: OperatingMode::Off,
            source_temp: SourceTemperature::OutdoorAir,
            run_time_s: 0.0,
            cycle_on_steps: 0,
            cycle_off_steps: 0,
            crankcase_heater_on: false,
            crankcase_heater_kw: 0.0,
            crankcase_rated_kw: CRANKCASE_HEATER_KW,
            crankcase_threshold_c: CRANKCASE_HEATER_THRESHOLD_C,
            crankcase_capacity_curve: None,
            last_cooling_rtf: 0.0,
            flow_fraction_correction: 1.0,
            coil_ao_by_stage: vec![10.0],
            is_room_ac,
            rated_shr: 0.75,
            stage_shrs: vec![],
            latent_degradation: LatentDegradationParams::default(),
            last_adp_c: 0.0,
            last_bypass_factor: 0.0,
            min_oat_cooling_c: -25.0,
            cooling_oat_locked_out: false,
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
        }
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;

        if self.is_room_ac {
            if self.hvac.config.speed_control_mode != SpeedControlMode::SingleSpeed {
                return Err(HaresError::Equipment(
                    "Room AC supports only single-speed mode".to_string(),
                ));
            }
            self.hvac.config.speed_control_mode = SpeedControlMode::SingleSpeed;
            self.hvac.config.duct_dse = 1.0;
            self.hvac.config.duct_zone_id = None;
        } else {
            self.hvac.config.duct_zone_id =
                super::helpers::parse_zone_id_key(config, "duct_zone_id");
        }

        self.init_from_typed(config, env)
    }

    fn init_from_typed(
        &mut self,
        config: &EquipmentConfig,
        _env: &EnvironmentState,
    ) -> crate::Result<()> {
        if self.is_room_ac {
            let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
            cfg.validate()?;

            self.hvac.config.cooling_capacities_w = vec![cfg.capacity_w];

            self.hvac.config.eir_by_stage = vec![cfg.eir];

            self.hvac.config.airflow_m3_s_per_w = cfg
                .airflow_m3_s_per_w
                .unwrap_or(super::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W);
            let default_shr = super::hvac_core::derive_default_shr(
                Some(self.hvac.config.airflow_m3_s_per_w),
                Some(cfg.capacity_w),
                true,
            );
            if cfg.shr.is_none() {
                tracing::debug!(
                    equipment_name = %config.name,
                    shr = default_shr,
                    airflow_m3_s_per_w = self.hvac.config.airflow_m3_s_per_w,
                    capacity_w = cfg.capacity_w,
                    "Room AC derived default SHR from EnergyPlus auto-sizing formula"
                );
            }
            self.rated_shr = cfg.shr.unwrap_or(default_shr).clamp(0.0, 1.0);
            // Priority: explicit user Cd → SEER-derived Cd → EnergyPlus SEER2 default 0.20.
            // EnergyPlus `StandardRatings.cc:177–180`: SEER2 Cd=0.20.
            let derived = cfg.derived_cooling_startup_cd();
            if cfg.startup_cd.is_none() {
                if let Some(cd_val) = derived {
                    tracing::debug!(
                        equipment_name = %config.name,
                        startup_cd = cd_val,
                        seer_bucket = if cd_val < 0.11 { "SEER >= 13" } else { "SEER < 13" },
                        "Room AC derived cycling degradation coefficient from EIR"
                    );
                }
            }
            let cd = derived.unwrap_or(0.20);
            self.hvac.runtime.plf_cooling_degradation_coeff = cd;
            self.hvac.runtime.startup.c_d = cd;
        } else {
            let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
            cfg.validate()?;

            self.hvac.config.cooling_capacities_w = if let Some(stages) = &cfg.stage_capacities_w {
                stages.clone()
            } else {
                vec![cfg.capacity_w]
            };

            let default_eir = cfg.eir;
            let stage_count = self.hvac.config.cooling_capacities_w.len();

            self.hvac.config.eir_by_stage = if let Some(stages) = &cfg.stage_eirs {
                stages.clone()
            } else {
                vec![default_eir; stage_count]
            };

            if self.hvac.config.cooling_capacities_w.len() != self.hvac.config.eir_by_stage.len() {
                return Err(HaresError::Equipment(
                    "cooling capacity and EIR stage counts must match".to_string(),
                ));
            }

            if let Some(r) = cfg.charge_defect_ratio {
                super::hvac_core::apply_charge_defect_correction(
                    &mut self.hvac.config.cooling_capacities_w,
                    &mut self.hvac.config.eir_by_stage,
                    r,
                );
            }

            if let Some(airflow_m3_s_per_w) = cfg.airflow_m3_s_per_w {
                self.hvac.config.airflow_m3_s_per_w = airflow_m3_s_per_w;
            }

            let rated_cap = self
                .hvac
                .config
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let fan_flow = self.hvac.config.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.config.cooling_capacities_w.len().min(255) as u8;
            let cap_low = (n_speeds > 1)
                .then(|| self.hvac.config.cooling_capacities_w.first().copied())
                .flatten();
            let flow_low = cap_low.map(|c| self.hvac.config.airflow_m3_s_per_w * c);
            self.hvac.config.duct_dse = if let Some(dse) = cfg.duct.dse_cool {
                dse
            } else {
                super::helpers::resolve_duct_dse(
                    config,
                    &super::helpers::DuctDseContext {
                        is_heating: false,
                        capacity_w: rated_cap,
                        fan_flow_m3_s: fan_flow,
                        n_speeds,
                        capacity_low_w: cap_low,
                        fan_flow_low_m3_s: flow_low,
                        is_heat_pump: false,
                    },
                )
            };

            let speed_mode = cfg.cooling_speed_control_mode();
            self.hvac.config.speed_control_mode = speed_mode;

            let default_shr = super::hvac_core::derive_default_shr(
                Some(self.hvac.config.airflow_m3_s_per_w),
                Some(rated_cap),
                false,
            );
            if cfg.shr.is_none() {
                tracing::debug!(
                    equipment_name = %config.name,
                    shr = default_shr,
                    airflow_m3_s_per_w = self.hvac.config.airflow_m3_s_per_w,
                    capacity_w = rated_cap,
                    "Central AC derived default SHR from EnergyPlus auto-sizing formula"
                );
            }
            self.rated_shr = cfg.shr.unwrap_or(default_shr).clamp(0.0, 1.0);

            // Per-stage SHR values if provided.
            self.stage_shrs = cfg.stage_shrs.clone().unwrap_or_default();
            if !self.stage_shrs.is_empty() {
                self.rated_shr = self.stage_shrs[0].clamp(0.0, 1.0);
            }

            self.apply_cooling_startup_cd(cfg.derived_cooling_startup_cd());

            // EnergyPlus Coil:Cooling:DX field N11 (Nominal Time for Condensate
            // Removal to Begin), suggested value 1000 s (V26-1-0 IDD §Coil:Cooling:DX);
            // zero means the latent degradation model is disabled.
            self.latent_degradation = LatentDegradationParams {
                twet_rated_s: 1000.0,
                gamma_rated: 1.5,
                max_cycling_rate: 3.0,
                latent_time_constant_s: 45.0,
            };
        }

        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, true);
        self.hvac.config.biquadratic_coeffs = load_curve_pair(config, self.is_room_ac)?;
        self.compute_coil_ao(self.rated_shr)?;

        let (crankcase_rated_kw, crankcase_threshold_c, crankcase_capacity_curve) =
            if self.is_room_ac {
                let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
                (
                    // Room ACs have no crankcase heater per OCHRE convention
                    // (MinisplitAHSPCooler class uses 15 W; window/room ACs use 0 W).
                    // Default to 0.0 kW, not the central-AC default of 0.05 kW.
                    cfg.crankcase_heater_kw.unwrap_or(0.0),
                    cfg.crankcase_heater_threshold_c
                        .unwrap_or(CRANKCASE_HEATER_THRESHOLD_C),
                    cfg.crankcase_capacity_curve_coeffs,
                )
            } else {
                let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
                (
                    cfg.crankcase_heater_kw.unwrap_or(CRANKCASE_HEATER_KW),
                    cfg.crankcase_heater_threshold_c
                        .unwrap_or(CRANKCASE_HEATER_THRESHOLD_C),
                    cfg.crankcase_capacity_curve_coeffs,
                )
            };
        self.crankcase_rated_kw = crankcase_rated_kw;
        self.crankcase_threshold_c = crankcase_threshold_c;
        self.crankcase_capacity_curve = crankcase_capacity_curve;

        // Minimum OAT lockout for cooling compressor operation.
        // EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
        self.min_oat_cooling_c = if self.is_room_ac {
            let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
            cfg.min_oat_compressor_cooling_c.unwrap_or(-25.0)
        } else {
            let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
            cfg.min_oat_compressor_cooling_c.unwrap_or(-25.0)
        };

        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.cycle_on_steps = 0;
        self.cycle_off_steps = 0;
        self.crankcase_heater_on = false;
        self.crankcase_heater_kw = 0.0;
        self.last_cooling_rtf = 0.0;
        self.telemetry = default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.cooling_oat_locked_out = false;
        self.use_ideal = self.hvac.use_ideal_capacity(env);

        // Advance DR duration; auto-revert to Normal when expired.
        if let Some(remaining) = self.dr_duration_remaining_s.as_mut() {
            let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;
            *remaining -= dt_s;
            if *remaining <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.dr_level = DRLevel::Normal;
                self.dr_setpoint_offset_c = 0.0;
                self.dr_load_fraction = 1.0;
                self.dr_duty_cycle = 1.0;
            }
        }

        // Short-circuit: GridEmergency or explicit Off override.
        if self.dr_load_fraction <= 0.0 || self.ctrl_mode_override == Some(OperatingMode::Off) {
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }

        // Minimum OAT lockout for cooling compressor operation.
        // EnergyPlus `DXCoils.cc:731, 9536`: compressor is locked out below
        // `minOATCompDXCooling` to prevent liquid slugging and oil foaming.
        self.cooling_oat_locked_out = env.weather.outdoor_temp_c < self.min_oat_cooling_c;
        if self.cooling_oat_locked_out {
            #[cfg(feature = "observe")]
            tracing::debug!(
                outdoor_temp_c = env.weather.outdoor_temp_c,
                min_oat_cooling_c = self.min_oat_cooling_c,
                "Cooling OAT lockout active"
            );
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
            return OperatingMode::Off;
        }

        let mode = self
            .hvac
            .update_mode(env)
            .unwrap_or(ThermostatMode::Deadband);
        if mode == ThermostatMode::Cooling {
            let base_setpoint = self.hvac.effective_setpoints().cooling_c;
            let setpoint = base_setpoint + self.dr_setpoint_offset_c;
            let zone_temp = lookup_zone(env, self.hvac.config.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(setpoint);

            // When DR raises the effective setpoint above the zone temperature, suppress cooling
            // even though the base thermostat is calling for it. This only applies when the DR
            // offset is active (non-zero) and the zone has cooled below the DR-adjusted threshold.
            let dr_suppressed = self.dr_setpoint_offset_c > 0.0 && zone_temp <= setpoint;
            if dr_suppressed {
                self.hvac.runtime.duty_cycle = 0.0;
                self.operating_mode = OperatingMode::Off;
                self.hvac.update_prev_zone_temp(None);
            } else {
                let deadband = self
                    .hvac
                    .thermostat_fsm
                    .thermostat
                    .hysteresis_c
                    .max(MIN_LOAD_FRACTION_DEADBAND_C);
                let load_fraction =
                    if self.hvac.config.speed_control_mode == SpeedControlMode::SingleSpeed {
                        1.0
                    } else {
                        ((zone_temp - setpoint) / deadband).clamp(0.0, 1.0)
                    };
                self.hvac.update_prev_zone_temp(Some(zone_temp));
                self.hvac.runtime.duty_cycle = match self.hvac.config.speed_control_mode {
                    SpeedControlMode::VariableSpeedIdeal => {
                        self.select_variable_speed_cooling(load_fraction)
                            .part_load_ratio
                    }
                    _ if self.use_ideal => 1.0,
                    _ => {
                        self.hvac
                            .select_speed_with_zone_temp(load_fraction, Some(zone_temp), false)
                            .part_load_ratio
                    }
                };
                self.operating_mode = if self.hvac.runtime.duty_cycle > 0.0 {
                    OperatingMode::Cooling
                } else {
                    OperatingMode::Off
                };
            }
        } else {
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                self.operating_mode != OperatingMode::Cooling
                    || env.weather.outdoor_temp_c >= self.min_oat_cooling_c,
                "Cooling active when OAT {:.1}°C < min_oat_cooling_c {:.1}°C",
                env.weather.outdoor_temp_c,
                self.min_oat_cooling_c
            );
        }

        self.operating_mode
    }

    /// Run one simulation step.
    ///
    /// `companion_heating_rtf` -- if `Some`, this AC is the cooling side of a
    /// heat pump; the provided value is the heating coil's RTF from the previous
    /// step, used to compute crankcase heater power correctly.
    pub(super) fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
        companion_heating_rtf: Option<f64>,
    ) -> std::result::Result<(), HaresError> {
        self.crankcase_heater_on = false;
        self.crankcase_heater_kw = 0.0;

        let mut compressor_kw = 0.0;
        let mut fan_kw = 0.0;
        let mut sensible_cooling_w = 0.0;
        let mut latent_cooling_w = 0.0;

        let dt_min = dt.as_secs_f64() / 60.0;
        if self.operating_mode == OperatingMode::Cooling {
            let perf = self.calculate_performance(env, dt_min)?;
            compressor_kw = perf.compressor_kw;
            fan_kw = perf.fan_kw;
            sensible_cooling_w = perf.sensible_cooling_w;
            latent_cooling_w = perf.latent_cooling_w;
            self.hvac.config.shr = perf.shr;
            self.hvac.config.supply_air_temp_c = perf.supply_temp_c;

            // Apply compound load multipliers (duty cycle * transient load * DR).
            let effective_load = (self.ctrl_duty_cycle
                * self.ctrl_load_fraction
                * self.dr_load_fraction
                * self.dr_duty_cycle)
                .clamp(0.0, 1.0);
            self.last_cooling_rtf = (perf.rtf * effective_load).clamp(0.0, 1.0);

            // Apply PowerLimit: clamp electric, proportionally reduce thermal.
            let total_electric_kw = compressor_kw + fan_kw;
            let (final_electric_kw, thermal_ratio) = if self.ctrl_power_limit_kw.is_finite()
                && total_electric_kw > self.ctrl_power_limit_kw
            {
                let ratio = self.ctrl_power_limit_kw / total_electric_kw.max(f64::MIN_POSITIVE);
                (self.ctrl_power_limit_kw, ratio)
            } else {
                (total_electric_kw, 1.0)
            };

            compressor_kw = (compressor_kw * thermal_ratio * effective_load).max(0.0);
            fan_kw = (fan_kw * thermal_ratio * effective_load).max(0.0);
            let _ = final_electric_kw; // used via thermal_ratio above
            sensible_cooling_w *= thermal_ratio * effective_load;
            latent_cooling_w *= thermal_ratio * effective_load;

            // Fan waste heat partially offsets cooling capacity delivered to
            // the zone (OCHRE HVAC.py line 543: delivered_heat = heat_gain * shr + fan_power).
            // For cooling: heat_gain is negative, fan_power is positive.
            let fan_heat_w = power_kw_to_w(fan_kw);
            self.hvac.write_zone_thermal_contributions(
                ports,
                -sensible_cooling_w + fan_heat_w,
                -latent_cooling_w,
                ThermalCategory::HvacCooling,
            )?;

            if latent_cooling_w.abs() > 0.0 {
                let moisture_mass_flow_kg_s = -latent_cooling_w / LATENT_HEAT_VAPORISATION_0C_J_KG;
                for &(zone, fraction) in &self.hvac.config.zone_heat_fractions {
                    if fraction > 0.0 {
                        ports.accumulate(&PortContribution::Humidity {
                            zone,
                            moisture_mass_flow_kg_s: moisture_mass_flow_kg_s * fraction,
                        })?;
                    }
                }
            }

            self.hvac.advance_speed_timer(dt.as_secs_f64());
            self.run_time_s += dt.as_secs_f64();
            self.cycle_on_steps += 1;
        } else {
            self.last_cooling_rtf = 0.0;
            self.cycle_off_steps += 1;
            self.hvac.runtime.time_at_current_speed_s = 0.0;
            self.hvac.update_prev_zone_temp(None);
            // apply_startup_capacity_degradation resets the ramp timer when off.
        }

        // Crankcase heater: draws power when outdoor temp is below threshold AND
        // neither coil (cooling or companion heating) is running. For HP systems
        // the max of both RTFs determines the off-time fraction.
        let crankcase_kw = self.crankcase_heater_power_internal(
            env.weather.outdoor_temp_c,
            self.last_cooling_rtf,
            companion_heating_rtf,
        );
        if crankcase_kw > 0.0 {
            self.crankcase_heater_on = true;
            self.crankcase_heater_kw = crankcase_kw;
        }

        let electric_kw =
            (compressor_kw + fan_kw + self.crankcase_heater_kw) * self.hvac.config.space_fraction;
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
                reactive_power_kvar: 0.0,
            })?;
        }

        // Telemetry reports delivered (post-DSE) values for the conditioned zone.
        let dse = self.hvac.config.duct_dse.clamp(0.0, 1.0);
        let fan_heat_w = power_kw_to_w(fan_kw);
        // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
        // sensible_cooling_w + latent_cooling_w are pre-DSE (gross) values from compute_performance.
        let gross_cooling_w = sensible_cooling_w + latent_cooling_w;
        let duct_loss_w = gross_cooling_w * (1.0 - dse);
        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry
            .set(tk::SENSIBLE_COOLING_W, sensible_cooling_w * dse);
        self.telemetry
            .set(tk::LATENT_COOLING_W, latent_cooling_w * dse);
        self.telemetry
            .set(tk::COIL_SENSIBLE_COOLING_W, sensible_cooling_w);
        self.telemetry
            .set(tk::COIL_LATENT_COOLING_W, latent_cooling_w);
        // OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction (pre-DSE).
        // coil_latent_cooling_w is the pre-DSE gross latent; space_fraction scales
        // the output to the conditioned-space fraction served by this equipment.
        self.telemetry.set(
            tk::LATENT_GAINS_W,
            latent_cooling_w * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::FAN_HEAT_W, fan_heat_w);
        // OCHRE HVAC.py:575: main_power = total_input_kw - fan_kw.
        // For cooling equipment total_input = compressor + fan, so main = compressor.
        self.telemetry.set(
            tk::MAIN_POWER_KW,
            compressor_kw * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::DUCT_LOSS_W, duct_loss_w);
        self.telemetry.set(tk::SHR, self.hvac.config.shr);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.telemetry.set(
            tk::COOLING_OAT_LOCKOUT,
            if self.cooling_oat_locked_out {
                1.0
            } else {
                0.0
            },
        );
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.runtime.last_speed_index as f64);
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        let compressor_only_w = power_kw_to_w(compressor_kw);
        let total_cooling_w = sensible_cooling_w + latent_cooling_w;
        let cop = if compressor_only_w > 1e-6 {
            total_cooling_w / compressor_only_w
        } else {
            0.0
        }
        // AHRI 210/240-2023: AC cooling COP ~2.3–4.1 W/W; clamp to [0.0, 8.0]
        // to exclude physically impossible values from telemetry.
        .clamp(0.0, 8.0);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        debug_assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "AC cooling COP {cop} not in [0.0, 8.0]"
        );
        self.telemetry.set(tk::COP, cop);
        self.telemetry
            .set(tk::RUNTIME_FRACTION, self.last_cooling_rtf.clamp(0.0, 1.0));
        self.telemetry.set(tk::COMPRESSOR_KW, compressor_kw);
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry
            .set(tk::SUPPLY_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry
            .set(tk::APPARATUS_DEW_POINT_C, self.last_adp_c);
        self.telemetry
            .set(tk::BYPASS_FACTOR, self.last_bypass_factor);
        self.telemetry.set(
            tk::MAX_CAPACITY_FRACTION,
            self.hvac.control.max_capacity_fraction,
        );
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
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
        let post_dse_sensible_w = sensible_cooling_w * dse;
        let post_dse_latent_w = latent_cooling_w * dse;
        let active_setpoint_c = match self.operating_mode {
            OperatingMode::Cooling => sp.cooling_c,
            OperatingMode::Heating => sp.heating_c,
            _ => sp.cooling_c,
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: None,
                thermal_output_w: Some(-(post_dse_sensible_w + post_dse_latent_w)),
                sensible_cooling_w: Some(-post_dse_sensible_w),
                latent_cooling_w: Some(-post_dse_latent_w),
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: Some(self.hvac.runtime.last_speed_index as u8),
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance {
                cop: Some(cop),
                main_power_kw: Some(compressor_kw * self.hvac.config.space_fraction),
            },
        };

        // Clear solver-provided capacity so next step starts fresh.
        self.ideal_capacity_w = 0.0;
        // LoadFraction is a one-step post-thermostat multiplier.
        self.ctrl_load_fraction = 1.0;

        Ok(())
    }

    /// Internal crankcase heater power computation.
    ///
    /// Returns 0 when OAT >= threshold. When below the threshold, power scales
    /// with the fraction of time NEITHER coil is running: `rated * (1 - max_rtf)`.
    /// For HP systems `companion_heating_rtf` is the heating side's RTF.
    fn crankcase_heater_power_internal(
        &self,
        outdoor_temp_c: f64,
        cooling_rtf: f64,
        companion_heating_rtf: Option<f64>,
    ) -> f64 {
        if outdoor_temp_c >= self.crankcase_threshold_c {
            return 0.0;
        }
        let max_rtf = match companion_heating_rtf {
            Some(heating_rtf) => cooling_rtf.max(heating_rtf),
            None => cooling_rtf,
        };
        let effective_rated = match self.crankcase_capacity_curve {
            Some([c0, c1, c2]) => {
                let t = outdoor_temp_c;
                self.crankcase_rated_kw * (c0 + c1 * t + c2 * t * t).max(0.0)
            }
            None => self.crankcase_rated_kw,
        };
        effective_rated * (1.0 - max_rtf.clamp(0.0, 1.0))
    }

    fn calculate_performance(
        &mut self,
        env: &EnvironmentState,
        dt_min: f64,
    ) -> crate::Result<PerformanceResult> {
        let zone = lookup_zone(env, self.hvac.config.zone_id)?;
        if !zone.wet_bulb_c.is_finite() {
            return Err(HaresError::Equipment(format!(
                "zone {:?} wet_bulb_c must be finite before HVAC cooling step",
                self.hvac.config.zone_id
            )));
        }

        let is_variable_speed =
            self.hvac.config.speed_control_mode == SpeedControlMode::VariableSpeedIdeal;
        let mut speed_index = self.hvac.runtime.last_speed_index;
        let speed_frac = self.hvac.runtime.last_speed_frac;
        let mut variable_selection = SpeedSelection {
            speed_index,
            speed_frac,
            part_load_ratio: self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
        };
        let (mut stage_cap_w, mut stage_eir, mut variable_plr) =
            match self.hvac.config.speed_control_mode {
                SpeedControlMode::VariableSpeedIdeal => {
                    self.variable_speed_point(variable_selection)
                }
                SpeedControlMode::MultiSpeedInterpolated => (
                    self.hvac.interpolated_capacity(
                        &self.hvac.config.cooling_capacities_w,
                        speed_index,
                        speed_frac,
                    ),
                    self.hvac.interpolated_eir(speed_index, speed_frac),
                    self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
                ),
                _ => (
                    HvacEquipment::capacity_at_stage(
                        &self.hvac.config.cooling_capacities_w,
                        speed_index,
                    ),
                    self.hvac.eir_at_stage(speed_index),
                    self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
                ),
            };

        let source_temp_c = self.source_temp.compute(env);

        fn curve_inputs(
            stage_capacity_w: f64,
            speed_index: usize,
            hvac: &HvacEquipment,
            zone: &hares_types::ZoneState,
            env: &EnvironmentState,
            source_temp_c: f64,
            flow_fraction_correction: f64,
        ) -> (f64, f64, f64, f64, f64) {
            let flow_m3_s_for_fan = stage_capacity_w.max(0.0) * hvac.config.airflow_m3_s_per_w;
            let fan_shaft_heat_correction_c = if flow_m3_s_for_fan > 0.0 {
                use hares_physics::{
                    air_properties::moist_air_density_kg_m3,
                    psychrometrics::SPECIFIC_HEAT_DRY_AIR_KJ_KG_K,
                };
                let fan_power_w = hvac.config.fan_power_w_per_m3_s * flow_m3_s_for_fan;
                let rho = moist_air_density_kg_m3(
                    env.weather.pressure_kpa * 1000.0,
                    zone.temperature_c,
                    zone.humidity_ratio.max(0.0),
                );
                let mfr = flow_m3_s_for_fan * rho;
                if mfr > 0.0 {
                    power_w_to_kw(fan_power_w) / (mfr * SPECIFIC_HEAT_DRY_AIR_KJ_KG_K)
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let coil_entering_db_c = zone.temperature_c + fan_shaft_heat_correction_c;
            let coil_entering_wb_c = if fan_shaft_heat_correction_c.abs() > f64::EPSILON {
                hares_physics::psychrometrics::wet_bulb_from_humidity_ratio(
                    coil_entering_db_c,
                    zone.humidity_ratio.max(0.0),
                    env.weather.pressure_kpa * 1000.0,
                )
            } else {
                zone.wet_bulb_c
            };
            let (_, cap_ratio) = hvac.evaluate_biquadratic_with_flow(
                speed_index * 2,
                coil_entering_wb_c,
                source_temp_c,
                flow_fraction_correction,
            );
            let (_, eir_ratio_base) = hvac.evaluate_biquadratic_with_flow(
                speed_index * 2 + 1,
                coil_entering_wb_c,
                source_temp_c,
                flow_fraction_correction,
            );
            (
                flow_m3_s_for_fan,
                coil_entering_db_c,
                coil_entering_wb_c,
                cap_ratio,
                eir_ratio_base,
            )
        }

        let (
            mut flow_m3_s_for_fan,
            mut coil_entering_db_c,
            mut coil_entering_wb_c,
            mut cap_ratio,
            mut eir_ratio_base,
        ) = curve_inputs(
            stage_cap_w,
            speed_index,
            &self.hvac,
            zone,
            env,
            source_temp_c,
            self.flow_fraction_correction,
        );

        // OCHRE HVAC.py:1044-1050 -- interpolate biquadratic between bracket stages.
        if self.hvac.config.speed_control_mode == SpeedControlMode::MultiSpeedInterpolated
            && speed_frac > 0.0
        {
            let (_, _, _, cap_ratio_high, eir_ratio_high) = curve_inputs(
                stage_cap_w,
                speed_index + 1,
                &self.hvac,
                zone,
                env,
                source_temp_c,
                self.flow_fraction_correction,
            );
            cap_ratio = cap_ratio * (1.0 - speed_frac) + cap_ratio_high * speed_frac;
            eir_ratio_base = eir_ratio_base * (1.0 - speed_frac) + eir_ratio_high * speed_frac;
        }

        if is_variable_speed && self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            let max_capacity_w = self
                .hvac
                .config
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or_default();
            let requested_capacity_fraction =
                (-self.ideal_capacity_w / (max_capacity_w * cap_ratio).max(1.0)).clamp(0.0, 1.0);
            variable_selection = self.select_variable_speed_cooling(requested_capacity_fraction);
            speed_index = variable_selection.speed_index;
            self.hvac.runtime.duty_cycle = variable_selection.part_load_ratio;
            (stage_cap_w, stage_eir, variable_plr) = self.variable_speed_point(variable_selection);
            (
                flow_m3_s_for_fan,
                coil_entering_db_c,
                coil_entering_wb_c,
                cap_ratio,
                eir_ratio_base,
            ) = curve_inputs(
                stage_cap_w,
                speed_index,
                &self.hvac,
                zone,
                env,
                source_temp_c,
                self.flow_fraction_correction,
            );
        }

        // Derive PLR: from solver's ideal capacity (coarse timestep) or
        // thermostat duty cycle (fine timestep).
        let steady_capacity_w = (stage_cap_w * cap_ratio).max(0.0);
        let plr = if is_variable_speed {
            variable_plr
        } else if self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            // Solver-provided ideal capacity (negative for cooling): derive PLR
            // from biquadratic-corrected capacity at current conditions.
            // Use 1% of nominal as floor to prevent near-zero denominators
            // when biquadratic evaluates near zero at extreme conditions.
            let min_cap = (stage_cap_w * 0.01).max(1.0);
            let p = (-self.ideal_capacity_w / steady_capacity_w.max(min_cap)).clamp(0.0, 1.0);
            // Write back so telemetry and RTF reporting see the solver-derived value.
            self.hvac.runtime.duty_cycle = p;
            p
        } else {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = if is_variable_speed {
            1.0
        } else {
            self.hvac.part_load_factor(plr)
        };

        // EIR curve: divide by PLF -- a lower PLF (more cycling) means worse efficiency.
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let staged_capacity_w = self
            .hvac
            .apply_startup_capacity_degradation(steady_capacity_w, dt_min);
        // OCHRE HVAC.py:445-448 -- clip to rated_max * ext_capacity_frac.
        let capacity_ceiling = steady_capacity_w * self.hvac.control.max_capacity_fraction;
        let total_capacity_w = (staged_capacity_w * plr).max(0.0).min(capacity_ceiling);

        let flow_m3_s = flow_m3_s_for_fan;

        let ao = self.ao_for_speed(speed_index);
        let CoilResult {
            shr,
            supply_temp_c,
            adp_temp_c,
            bypass_factor,
            ..
        } = calculate_shr(
            coil_entering_db_c,
            zone.humidity_ratio,
            env.weather.pressure_kpa,
            power_w_to_kw(total_capacity_w).max(0.0),
            flow_m3_s,
            ao,
        )?;
        let steady_state_shr = shr.clamp(0.0, 1.0);
        self.last_adp_c = adp_temp_c;
        self.last_bypass_factor = bypass_factor;

        // Apply Henderson-Rengarajan latent degradation at part load.
        // RTF = PLR / PLF; at continuous operation (plf==0 guard is already
        // handled above) this reduces to PLR / PLF.
        let rtf = if plf > 0.0 {
            (plr / plf).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let shr = if self.latent_degradation.is_active() {
            // Rated latent capacity at AHRI conditions (single-stage nominal).
            let rated_cap_w = self
                .hvac
                .config
                .cooling_capacities_w
                .get(speed_index)
                .copied()
                .unwrap_or_else(|| {
                    self.hvac
                        .config
                        .cooling_capacities_w
                        .last()
                        .copied()
                        .unwrap_or(0.0)
                });
            let rated_latent_w = rated_cap_w * (1.0 - self.rated_shr);
            let actual_latent_w = total_capacity_w * (1.0 - steady_state_shr);
            effective_shr_with_latent_degradation(
                steady_state_shr,
                rtf,
                zone.temperature_c,
                coil_entering_wb_c,
                rated_latent_w,
                actual_latent_w,
                &self.latent_degradation,
                None,
            )
        } else {
            steady_state_shr
        };

        self.hvac.config.shr = shr;

        // Return gross (pre-DSE) sensible/latent. zone_heat_fractions (set from
        // duct_dse during init) distributes gross output to conditioned and duct zones.
        let (sensible_cooling_w, latent_cooling_w) =
            self.hvac.sensible_latent_from_shr(total_capacity_w);

        let compressor_kw = power_w_to_kw((total_capacity_w * stage_eir * eir_ratio).max(0.0));
        let fan_kw = power_w_to_kw(self.hvac.fan_power_w(flow_m3_s) * plr);

        Ok(PerformanceResult {
            sensible_cooling_w,
            latent_cooling_w,
            compressor_kw,
            fan_kw,
            shr,
            supply_temp_c,
            rtf,
        })
    }

    fn compute_coil_ao(&mut self, rated_shr: f64) -> crate::Result<()> {
        self.coil_ao_by_stage = compute_coil_ao_by_stage(
            &self.hvac.config.cooling_capacities_w,
            self.hvac.config.airflow_m3_s_per_w,
            rated_shr,
        )?;
        Ok(())
    }

    fn ao_for_speed(&self, speed_index: usize) -> f64 {
        if self.coil_ao_by_stage.is_empty() {
            return 10.0;
        }
        self.coil_ao_by_stage[speed_index.min(self.coil_ao_by_stage.len() - 1)]
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&AirConditionerState {
            mode: self.hvac.thermostat_fsm.mode,
            duty_cycle: self.hvac.runtime.duty_cycle,
            last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
            mode_start_at: self.hvac.thermostat_fsm.mode_start_at,
            runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            cycle_on_steps: self.cycle_on_steps,
            cycle_off_steps: self.cycle_off_steps,
            crankcase_heater_on: self.crankcase_heater_on,
            startup_c_d: self.hvac.runtime.startup.c_d,
            startup_time_since_start_min: self.hvac.runtime.startup.time_since_start_min,
            plf_state: self.hvac.runtime.plf_state,
            last_speed_index: self.hvac.runtime.last_speed_index,
            last_speed_frac: self.hvac.runtime.last_speed_frac,
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            sensible_cooling_w: self.telemetry.get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0),
            latent_cooling_w: self.telemetry.get(tk::LATENT_COOLING_W).unwrap_or(0.0),
            shr: self.telemetry.get(tk::SHR).unwrap_or(self.hvac.config.shr),
            operating_mode_code: self.telemetry.get(tk::OPERATING_MODE).unwrap_or(0.0),
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
            last_adp_c: self.last_adp_c,
            last_bypass_factor: self.last_bypass_factor,
            thermostat_hysteresis_c: self.hvac.thermostat_fsm.thermostat.hysteresis_c,
            time_at_current_speed_s: self.hvac.runtime.time_at_current_speed_s,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: AirConditionerState = load_postcard(state)?;
        self.hvac.thermostat_fsm.mode = decoded.mode;
        self.hvac.runtime.duty_cycle = decoded.duty_cycle;
        self.hvac.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.thermostat_fsm.mode_start_at = decoded.mode_start_at;
        self.hvac.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.cycle_on_steps = decoded.cycle_on_steps;
        self.cycle_off_steps = decoded.cycle_off_steps;
        self.crankcase_heater_on = decoded.crankcase_heater_on;
        self.hvac.runtime.startup.c_d = decoded.startup_c_d;
        self.hvac.runtime.startup.time_since_start_min = decoded.startup_time_since_start_min;
        self.hvac.runtime.plf_state = decoded.plf_state;
        self.hvac.runtime.last_speed_index = decoded.last_speed_index;
        self.hvac.runtime.last_speed_frac = decoded.last_speed_frac;
        self.hvac.config.shr = decoded.shr;
        self.ctrl_duty_cycle = decoded.ctrl_duty_cycle;
        self.ctrl_power_limit_kw = decoded.ctrl_power_limit_kw.unwrap_or(f64::INFINITY);
        self.ctrl_mode_override = decoded.ctrl_mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duty_cycle = decoded.dr_duty_cycle;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.last_adp_c = decoded.last_adp_c;
        self.last_bypass_factor = decoded.last_bypass_factor;
        self.hvac.thermostat_fsm.thermostat.hysteresis_c = decoded.thermostat_hysteresis_c;
        self.hvac.runtime.time_at_current_speed_s = decoded.time_at_current_speed_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::SENSIBLE_COOLING_W, decoded.sensible_cooling_w);
        self.telemetry
            .insert(tk::LATENT_COOLING_W, decoded.latent_cooling_w);
        self.telemetry.insert(tk::SHR, decoded.shr);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode_code);
        // Telemetry fields are recomputed on next step; not restored from checkpoint.
        // Setpoints, COP, fan_kw, etc. will be updated on the next step() call.
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ThermalSetpoint { deadband_c, .. } => {
                self.hvac.apply_control_signal(signal);
                if let Some(db) = deadband_c {
                    if !db.is_finite() || *db < 0.0 {
                        return Err(HaresError::Control(format!(
                            "invalid deadband_c for {}: {db}",
                            self.descriptor.equipment_type
                        )));
                    }
                    self.hvac.thermostat_fsm.thermostat.hysteresis_c = *db;
                }
            }
            ControlSignal::DutyCycle { on_fraction, .. } => {
                self.ctrl_duty_cycle = on_fraction.clamp(0.0, 1.0);
            }
            ControlSignal::LoadFraction { fraction } => {
                self.ctrl_load_fraction = fraction.clamp(0.0, 1.0);
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                self.ctrl_power_limit_kw = if *max_power_kw >= 0.0 {
                    *max_power_kw
                } else {
                    f64::INFINITY
                };
            }
            ControlSignal::ModeOverride { mode } => {
                self.ctrl_mode_override = Some(*mode);
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.apply_dr_level(*level);
                self.dr_duration_remaining_s = *duration_s;
            }
            ControlSignal::IdealCapacity { capacity_w } => {
                // Solver-provided load: negative = cooling needed.
                self.ideal_capacity_w = *capacity_w;
            }
            ControlSignal::MaxCapacityFraction { fraction } => {
                if !fraction.is_finite() || !(0.0..=1.0).contains(fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid max capacity fraction for {}: {fraction}",
                        self.descriptor.equipment_type
                    )));
                }
                self.hvac.control.max_capacity_fraction = *fraction;
            }
            _ => {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    // Signals reaching the hvac catch-all path with a declared
                    // capability must be handled by HvacEquipment::apply_control_signal.
                    // Currently ThermalSetpointDelta is the only variant that falls
                    // through to this path; ThermalSetpoint and MaxCapacityFraction
                    // are handled by explicit arms above.
                    let required = signal.required_capability();
                    if self.descriptor.control_capabilities.contains(required) {
                        debug_assert!(
                            matches!(signal, ControlSignal::ThermalSetpointDelta { .. }),
                            "CoolingCore '{}': signal {:?} (capability {:?}) \
                             reached the hvac catch-all path — add an explicit \
                             match arm in apply_control_unchecked for this signal variant",
                            self.descriptor.equipment_type,
                            signal,
                            required,
                        );
                    }
                }
                #[cfg(feature = "observe")]
                tracing::debug!(
                    signal_variant = ?signal,
                    equipment = %self.descriptor.equipment_type,
                    "control signal routed through hvac catch-all path",
                );
                self.hvac.apply_control_signal(signal);
            }
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        // Only report when thermostat says we need cooling.
        if self.operating_mode != OperatingMode::Cooling {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().cooling_c + self.dr_setpoint_offset_c;
        Some((self.hvac.config.zone_id, setpoint))
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
                // Raise cooling setpoint by 1°C → less cooling demand.
                self.dr_setpoint_offset_c = 1.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::High => {
                self.dr_setpoint_offset_c = 2.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.8;
            }
            DRLevel::Critical => {
                self.dr_setpoint_offset_c = 3.0;
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
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Air Conditioner",
        Box::new(|config| Box::new(AirConditioner::new(config))),
    );
    registry.register("Room AC", Box::new(|config| Box::new(RoomAC::new(config))));
}

#[cfg(test)]
fn typed_ac_test_config(eir: f64) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "AC".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 8_000.0,
            eir,
            shr: Some(0.75),
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: crate::DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, ExecutionStage, GridState, HumidityAccumulator,
        OperatingMode, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{AirConditioner, CoolingCore, RoomAC, SpeedControlMode};

    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, EquipmentRegistry,
        HvacSetpointConfig, RoomAcConfig,
    };
    use hares_physics::constants::BTU_PER_HR_PER_W;
    use hares_physics::units::power_kw_to_w;

    fn env(
        zone_temp_c: f64,
        humidity_ratio: f64,
        wet_bulb_c: f64,
        outdoor_c: f64,
    ) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio,
                relative_humidity: 0.45,
                wet_bulb_c,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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

    fn ac_config() -> EquipmentConfig {
        super::typed_ac_test_config(0.33)
    }

    fn room_ac_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "AC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                shr: None,
                startup_cd: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
    }

    fn ac_config_with(mutator: impl FnOnce(&mut CentralAirConditionerConfig)) -> EquipmentConfig {
        let mut typed = ac_config().typed::<CentralAirConditionerConfig>().unwrap();
        mutator(&mut typed);
        EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed)
    }

    #[test]
    fn air_conditioner_descriptor_contracts() {
        let cfg = ac_config();
        let eq = AirConditioner::new(cfg);
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::THERMAL_SETPOINT)
        );

        let telemetry_names: Vec<_> = eq
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert!(telemetry_names.contains(&tk::ELECTRIC_KW));
        assert!(telemetry_names.contains(&tk::SENSIBLE_COOLING_W));
        assert!(telemetry_names.contains(&tk::LATENT_COOLING_W));
        assert!(telemetry_names.contains(&tk::SHR));
        assert!(telemetry_names.contains(&tk::OPERATING_MODE));
    }

    #[test]
    fn room_ac_forces_single_speed_and_duct_dse_one() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            super::SpeedControlMode::SingleSpeed
        );
        assert_eq!(eq.core.hvac.config.duct_dse, 1.0);
    }

    #[test]
    fn room_ac_init_uses_explicit_startup_cd() {
        // Explicit startup_cd takes priority over SEER-derived value.
        let explicit_cd = 0.15;
        let cfg = EquipmentConfig::from_typed(
            "room_ac_explicit".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: BTU_PER_HR_PER_W / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: Some(explicit_cd),
                ..room_ac_defaults()
            },
        );
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        assert!((eq.core.hvac.runtime.plf_cooling_degradation_coeff - explicit_cd).abs() < 1e-9);
        assert!((eq.core.hvac.runtime.startup.c_d - explicit_cd).abs() < 1e-9);
    }

    #[test]
    fn room_ac_init_uses_derived_cd_when_no_explicit() {
        // No explicit startup_cd → derived from SEER. SEER < 13 → Cd = 0.20.
        let eir = BTU_PER_HR_PER_W / 10.0;
        let cfg = EquipmentConfig::from_typed(
            "room_ac_derived".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: None,
                ..room_ac_defaults()
            },
        );
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        assert!((eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-9);
        assert!((eq.core.hvac.runtime.startup.c_d - 0.20).abs() < 1e-9);
    }

    #[test]
    fn room_ac_init_uses_static_fallback_when_no_derived() {
        // EIR is zero → derived_cooling_startup_cd returns None → fallback 0.20.
        // However, validation will reject zero EIR before Cd init is reached.
        let cfg = EquipmentConfig::from_typed(
            "room_ac_fallback".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: None,
                ..room_ac_defaults()
            },
        );
        let mut eq = RoomAC::new(cfg.clone());
        assert!(
            eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).is_err(),
            "zero eir should fail validation before reaching Cd init"
        );
    }

    fn room_ac_defaults() -> RoomAcConfig {
        RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            eir: 0.0,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: None,
                heating_setpoint_c: None,
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        }
    }

    /// Regression: AC step() previously called update_control() internally,
    /// causing the thermostat FSM to advance twice per timestep.
    /// step() must be pure physics: calling it without a prior update_control()
    /// must leave the equipment in its initial Off state.
    #[test]
    fn step_does_not_call_update_control_internally() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        // Zone well above setpoint (28°C vs 24°C setpoint); an internal
        // update_control() would switch to Cooling mode and draw power.
        let environment = env(28.0, 0.01, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        // Deliberately skip update_control() before step().
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            eq.telemetry().get(tk::OPERATING_MODE),
            Some(0.0),
            "step() must not change operating mode; it must remain Off when \
             update_control() was never called",
        );
        assert_eq!(
            eq.telemetry().get(tk::ELECTRIC_KW),
            Some(0.0),
            "no power should be drawn when operating mode is Off",
        );
    }

    #[test]
    fn wet_bulb_validation_returns_error_when_nan() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // NaN wet-bulb is uninitialized data and must be rejected.
        let env_bad = env(27.0, 0.01, f64::NAN, 35.0);
        eq.init(&cfg, &env_bad).unwrap();
        eq.update_control(&env_bad);
        let err = eq
            .step(&env_bad, Duration::from_secs(60), &mut ports)
            .expect_err("wet bulb NaN should be validated");
        assert!(err.to_string().contains("wet_bulb_c"));
    }

    #[test]
    fn zero_wet_bulb_does_not_reject_calculation() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // 0.0°C wet-bulb is a valid winter condition and must not be rejected.
        let env_zero_wb = env(27.0, 0.01, 0.0, 35.0);
        eq.init(&cfg, &env_zero_wb).unwrap();
        eq.update_control(&env_zero_wb);
        eq.step(&env_zero_wb, Duration::from_secs(60), &mut ports)
            .expect("0.0°C wet-bulb is a valid winter condition and must not return an error");
    }

    #[test]
    fn crankcase_heater_draws_when_off_and_cold() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(22.0, 0.008, 15.0, 5.0);
        eq.init(&cfg, &env).unwrap();

        eq.update_control(&env);
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.net_active_w() - 50.0).abs() < 1.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);
    }

    #[test]
    fn sensible_and_latent_sum_to_total_cooling() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        eq.update_control(&env);
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let sens = eq.telemetry().get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0);
        let lat = eq.telemetry().get(tk::LATENT_COOLING_W).unwrap_or(0.0);
        let fan_kw = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0);
        let fan_heat_w = power_kw_to_w(fan_kw);

        // Telemetry reports gross cooling (sensible + latent) before fan-heat offset.
        // Port thermal sensible includes fan waste heat: gain = -sensible + fan_heat.
        // So: telemetry_total = -(port_sensible + port_latent) + fan_heat.
        let telemetry_total = sens + lat;
        let port_total = -(ports.thermal[0].sensible_gain_w + ports.thermal[0].latent_gain_w);
        assert!(
            (telemetry_total - (port_total + fan_heat_w)).abs() < 1e-6,
            "telemetry total {telemetry_total} != port total {port_total} + fan heat {fan_heat_w}",
        );
    }

    #[test]
    fn shr_drops_with_higher_humidity_ratio() {
        // This test exercises SHR coil physics; disable the startup ramp (c_d=0)
        // so it does not obscure the result on the first step.
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));

        let mut eq_low = AirConditioner::new(cfg.clone());
        let mut eq_high = AirConditioner::new(cfg.clone());

        let mut ports_low = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let mut ports_high = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        let env_low = env(28.0, 0.0075, 18.0, 35.0);
        let env_high = env(28.0, 0.0150, 22.0, 35.0);

        eq_low.init(&cfg, &env_low).unwrap();
        eq_high.init(&cfg, &env_high).unwrap();

        eq_low.update_control(&env_low);
        eq_low
            .step(&env_low, Duration::from_secs(60), &mut ports_low)
            .unwrap();
        eq_high.update_control(&env_high);
        eq_high
            .step(&env_high, Duration::from_secs(60), &mut ports_high)
            .unwrap();

        let shr_low = eq_low.telemetry().get(tk::SHR).unwrap_or(1.0);
        let shr_high = eq_high.telemetry().get(tk::SHR).unwrap_or(1.0);
        assert!(shr_high < shr_low, "shr_high={shr_high}, shr_low={shr_low}");
    }

    #[test]
    fn state_round_trip_preserves_cycle_state() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(28.0, 0.01, 19.0, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(restored.telemetry().get(tk::OPERATING_MODE), Some(2.0));
        assert!(restored.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0) > 0.0);
    }

    /// Central AC uses the canonical SI airflow ratio for central cooling.
    /// Reference provenance is RESNET HERS Addendum 82 / OpenStudio-HPXML defaults.
    #[test]
    fn ac_uses_default_airflow_ratio() {
        let expected = crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W;

        let cfg = ac_config();
        let eq = AirConditioner::new(cfg.clone());
        assert_eq!(
            eq.core.hvac.config.equipment_type,
            crate::hvac::hvac_core::HvacEquipmentType::AcCooler,
        );
        // Tolerance 1e-9: uom ft³ constant (0.02831685) differs from NIST-exact
        // (0.028316846592) by ~3 ppm; implementation uses the NIST constant.
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "Central AC default airflow mismatch, got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );

        let environment = env(27.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).expect("init must succeed");
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "init() must preserve default airflow ratio, got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );
    }

    #[test]
    fn registry_registers_ochre_names_with_thermal_stage() {
        let registry = EquipmentRegistry::new();

        let ac = registry.create("Air Conditioner", ac_config()).unwrap();
        assert_eq!(ac.descriptor().stage, ExecutionStage::Thermal);

        let room = registry.create("Room AC", room_ac_config()).unwrap();
        assert_eq!(room.descriptor().stage, ExecutionStage::Thermal);
    }

    #[test]
    fn room_ac_duct_dse_is_one() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        assert!(
            (eq.core.hvac.config.duct_dse - 1.0).abs() < f64::EPSILON,
            "Room AC duct_dse must be 1.0, got {}",
            eq.core.hvac.config.duct_dse
        );
        assert!(
            eq.core.hvac.config.duct_zone_id.is_none(),
            "Room AC must have no duct zone"
        );
    }

    #[test]
    fn room_ac_step_produces_cooling() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let sens = eq.telemetry().get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0);
        let elec = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            sens > 0.0,
            "sensible_cooling_w must be positive (magnitude), got {sens}"
        );
        assert!(elec > 0.0, "electric_kw must be positive, got {elec}");
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "thermal port sensible gain must be negative (cooling), got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn room_ac_no_duct_zone_entry() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        let fracs = &eq.core.hvac.config.zone_heat_fractions;
        assert_eq!(
            fracs.len(),
            1,
            "Room AC must have exactly one zone heat fraction entry, got {fracs:?}"
        );
        assert_eq!(fracs[0].0, ZoneId(1));
        assert!(
            (fracs[0].1 - 1.0).abs() < f64::EPSILON,
            "Room AC zone heat fraction must be 1.0, got {}",
            fracs[0].1
        );
    }

    #[test]
    fn typed_ac_eir_sets_expected_eir() {
        let cfg = super::typed_ac_test_config(0.25);
        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        let expected = 0.25;
        let actual = eq.core.hvac.config.eir_by_stage[0];
        assert!(
            (actual - expected).abs() < 1e-9,
            "typed eir=0.25 must be preserved, got {actual}"
        );
    }

    #[test]
    fn typed_stage_eir_overrides_base_eir() {
        let mut typed = super::typed_ac_test_config(0.25)
            .typed::<CentralAirConditionerConfig>()
            .expect("typed central AC config");
        // Intentionally contradictory legacy efficiency; SI-native stage EIR must win.
        typed.eir = 0.8;
        typed.stage_eirs = Some(vec![0.30]);
        typed.stage_capacities_w = Some(vec![typed.capacity_w]);

        let cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed);
        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(
            eq.core.hvac.config.eir_by_stage,
            vec![0.30],
            "SI stage_eirs must override eir-derived EIR"
        );
    }

    #[test]
    fn room_ac_vs_central_different_dse() {
        let central_with_duct_losses = ac_config_with(|typed| {
            typed.capacity_w = 3_500.0;
            typed.hysteresis_c = Some(0.0);
            typed.duct.dse_cool = Some(0.80);
        });
        let central_no_duct_losses = ac_config_with(|typed| {
            typed.capacity_w = 3_500.0;
            typed.hysteresis_c = Some(0.0);
            typed.duct.dse_cool = Some(1.0);
        });

        let environment = env(28.0, 0.012, 20.0, 35.0);

        let mut with_losses = AirConditioner::new(central_with_duct_losses.clone());
        with_losses
            .init(&central_with_duct_losses, &environment)
            .unwrap();
        let mut ports_with_losses = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        with_losses.update_control(&environment);
        with_losses
            .step(
                &environment,
                Duration::from_secs(60),
                &mut ports_with_losses,
            )
            .unwrap();

        let mut no_losses = AirConditioner::new(central_no_duct_losses.clone());
        no_losses
            .init(&central_no_duct_losses, &environment)
            .unwrap();
        let mut ports_no_losses = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        no_losses.update_control(&environment);
        no_losses
            .step(&environment, Duration::from_secs(60), &mut ports_no_losses)
            .unwrap();

        let central_cooling = -ports_with_losses.thermal[0].sensible_gain_w;
        let no_loss_cooling = -ports_no_losses.thermal[0].sensible_gain_w;
        assert!(
            central_cooling > 0.0,
            "central AC must produce cooling, got {central_cooling}"
        );
        assert!(
            no_loss_cooling > 0.0,
            "central AC (dse=1.0) must produce cooling, got {no_loss_cooling}"
        );
        assert!(
            no_loss_cooling > central_cooling,
            "Central AC with DSE=1.0 must deliver more cooling to zone ({no_loss_cooling:.1} W) \
             than central AC with DSE=0.80 ({central_cooling:.1} W)"
        );
    }

    /// Room AC uses the canonical SI airflow ratio for room cooling.
    /// Reference provenance is manufacturer/AHRI room-unit airflow medians.
    #[test]
    fn room_ac_airflow_rate() {
        let expected = crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W;

        // Construction-time default is central-cooling airflow; init() overrides to room-AC airflow.
        let environment = env(27.0, 0.010, 19.0, 35.0);
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &environment).expect("init must succeed");
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "RoomAC airflow must equal the typed SI default after init(), got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );
    }

    /// Two-speed AC selects the high stage when load fraction exceeds
    /// `low_speed_capacity_fraction` (default 0.72 per OCHRE/AHRI), and the low stage otherwise.
    /// With `hysteresis_c=0` the thermostat activates at zone_temp > setpoint (24°C)
    /// and `MIN_LOAD_FRACTION_DEADBAND_C=0.5°C` governs the load fraction:
    ///   zone 24.4°C → load_fraction = 0.4/0.5 = 0.8 > 0.5 → stage 1 (8 000 W)
    ///   zone 24.1°C → load_fraction = 0.1/0.5 = 0.2 ≤ 0.5 → stage 0 (4 000 W)
    /// The test verifies that two-speed stage selection routes to different capacity
    /// stages by observing the resulting electrical draw.
    #[test]
    fn two_speed_ac_draws_more_power_at_high_load_than_moderate_load() {
        // Zero hysteresis so activation threshold equals setpoint, not setpoint+1.
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.hysteresis_c = Some(0.0);
            typed.stage_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.33]);
        });

        // load_fraction = (zone - 24) / 0.5; 24.4 → 0.8 > 0.5 → high stage
        let env_high_load = env(24.4, 0.010, 18.0, 35.0);
        // load_fraction = (24.1 - 24) / 0.5 = 0.2 ≤ 0.5 → low stage
        let env_low_load = env(24.1, 0.010, 18.0, 35.0);

        let mut eq_high = AirConditioner::new(cfg.clone());
        eq_high.init(&cfg, &env_high_load).unwrap();
        let mut ports_high = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_high.update_control(&env_high_load);
        eq_high
            .step(&env_high_load, Duration::from_secs(60), &mut ports_high)
            .unwrap();
        let kw_high = eq_high.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        let mut eq_low = AirConditioner::new(cfg.clone());
        eq_low.init(&cfg, &env_low_load).unwrap();
        let mut ports_low = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_low.update_control(&env_low_load);
        eq_low
            .step(&env_low_load, Duration::from_secs(60), &mut ports_low)
            .unwrap();
        let kw_low = eq_low.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(
            kw_high > 0.0,
            "high-stage AC must draw positive electrical power, got {kw_high}"
        );
        assert!(
            kw_low > 0.0,
            "low-stage AC must draw positive electrical power, got {kw_low}"
        );
        assert!(
            kw_high > kw_low,
            "high-stage draw ({kw_high:.4} kW) must exceed low-stage draw ({kw_low:.4} kW)"
        );
    }

    /// Single-speed AC must run binary on/off at the thermostat timestep.
    /// When cooling is called, duty is 1.0 regardless of setpoint error magnitude.
    #[test]
    fn single_speed_cooling_call_uses_full_duty_cycle() {
        // Zero hysteresis so thermostat activates right at setpoint (24°C).
        let cfg = ac_config_with(|typed| typed.hysteresis_c = Some(0.0));

        // Both environments are above setpoint so cooling is On in both cases.
        let env_full = env(24.6, 0.010, 18.0, 35.0);
        let env_part = env(24.2, 0.010, 18.0, 35.0);

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        eq_full.update_control(&env_full);
        let duty_full = eq_full.core.hvac.runtime.duty_cycle;

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        eq_part.update_control(&env_part);
        let duty_part = eq_part.core.hvac.runtime.duty_cycle;

        assert!(
            (duty_full - 1.0).abs() < 1e-9,
            "single-speed cooling call must use duty=1.0, got {duty_full}"
        );
        assert!(
            (duty_part - 1.0).abs() < 1e-9,
            "single-speed cooling call must use duty=1.0 even near setpoint, got {duty_part}"
        );
    }

    #[test]
    fn variable_speed_ideal_uses_fractional_duty_cycle() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(0.0);
        });
        let env_part = env(24.25, 0.010, 18.0, 35.0); // load_fraction=(0.25/0.5)=0.5
        let env_full = env(24.60, 0.010, 18.0, 35.0); // clamped to 1.0

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        assert_eq!(
            eq_part.core.hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        eq_part.update_control(&env_part);
        assert!(
            (eq_part.core.hvac.runtime.duty_cycle - 0.5).abs() < 1e-9,
            "variable-speed ideal should preserve fractional duty cycle; got {}",
            eq_part.core.hvac.runtime.duty_cycle
        );

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        eq_full.update_control(&env_full);
        assert!(
            (eq_full.core.hvac.runtime.duty_cycle - 1.0).abs() < 1e-9,
            "high load should clamp to full duty cycle; got {}",
            eq_full.core.hvac.runtime.duty_cycle
        );
    }

    #[test]
    fn central_four_speed_ac_uses_variable_speed_mode() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(0.0);
            typed.startup_cd = None;
        });
        let env = env(24.25, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(
            eq.core.hvac.runtime.startup.c_d, 0.0,
            "OCHRE 4-speed variable cooling should map to zero startup Cd"
        );

        eq.update_control(&env);
        assert!(
            (eq.core.hvac.runtime.duty_cycle - 0.5).abs() < 1e-9,
            "central 4-speed variable cooling should preserve fractional duty; got {}",
            eq.core.hvac.runtime.duty_cycle
        );
    }

    #[test]
    fn central_four_speed_variable_speed_interpolates_stage_ladder() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(0.0);
            typed.stage_capacities_w = Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.20, 0.25, 0.30, 0.35]);
            typed.stage_shrs = Some(vec![0.75, 0.75, 0.75, 0.75]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let environment = env(24.25, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        assert_eq!(eq.core.hvac.runtime.last_speed_index, 0);
        assert!((eq.core.hvac.runtime.last_speed_frac - 1.0).abs() < 1e-9);
        assert!(
            compressor_kw > 0.0,
            "load_fraction=0.5 with a 4-speed ladder must energize the second stage, got compressor_kw={compressor_kw}"
        );
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "an exact variable-speed stage match must run continuously, got runtime_fraction={rtf}"
        );
    }

    #[test]
    fn typed_two_speed_ac_derives_two_speed_startup_cd() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.startup_cd = None;
        });
        let env = env(28.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            SpeedControlMode::TwoSpeedSetpoint
        );
        assert!(
            (eq.core.hvac.runtime.startup.c_d - 0.11).abs() < 1e-9,
            "two-speed typed cooling must derive startup Cd=0.11, got {}",
            eq.core.hvac.runtime.startup.c_d
        );
        assert!(
            (eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.11).abs() < 1e-9,
            "two-speed typed cooling must derive PLF Cd=0.11, got {}",
            eq.core.hvac.runtime.plf_cooling_degradation_coeff
        );
    }

    #[test]
    fn single_speed_ac_startup_ramp_reduces_first_step_capacity() {
        // SEER 14 → EIR = 3.412141633 / 14 ≈ 0.2437; c_d=0.07 gives t_full=1.8 min.
        // Step 1 fires at time_since_start=0.5 min (< t_full) → capacity multiplier < 1.
        // By step 3, time_since_start=2.5 min > t_full → multiplier = 1.0.
        let seer = 14.0_f64;
        let eir = 3.412_141_633_f64 / seer;
        let cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: crate::DuctConfig::default(),
                system_type: None,
                startup_cd: Some(0.07),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let environment = env(26.0, 0.010, 18.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        for _ in 1..5 {
            eq.update_control(&environment);
            let mut p = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&environment, Duration::from_secs(60), &mut p)
                .unwrap();
        }
        let kw_steady = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(
            kw_step1 > 0.0,
            "AC must draw power on step 1; got {kw_step1}"
        );
        assert!(
            kw_step1 < kw_steady,
            "startup ramp must reduce step-1 kW below steady-state; step1={kw_step1:.4}, steady={kw_steady:.4}"
        );
    }

    #[test]
    fn transient_load_fraction_can_be_applied_before_update_control() {
        let cfg = ac_config();
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(
            kw, 0.0,
            "LoadFraction applied before update_control must still affect the current step"
        );

        let mut ports_next = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports_next)
            .unwrap();
        let kw_next = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            kw_next > 0.0,
            "transient LoadFraction must clear after one step; got {kw_next}"
        );
    }

    #[test]
    fn runtime_fraction_tracks_post_control_duty_cycle() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 0.5).abs() < 0.1,
            "Runtime fraction must reflect post-control duty cycle; got {rtf}"
        );
    }

    #[test]
    fn coarse_timestep_variable_speed_ideal_signal_selects_stage_from_capacity() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.stage_capacities_w = Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.20, 0.25, 0.30, 0.35]);
            typed.stage_shrs = Some(vec![0.75, 0.75, 0.75, 0.75]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let mut environment = env(26.0, 0.010, 19.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -5_000.0,
        })
        .unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        // 5000W = 62.5% of 8000W max -- falls between stage 1 (50%) and stage 2 (75%),
        // so stage 1 is selected (lo index in the interpolation range).
        assert_eq!(eq.core.hvac.runtime.last_speed_index, 1);
        assert!(
            compressor_kw > 0.0,
            "IdealCapacity=-5 kW on a 4-speed ladder must energize the second stage, got compressor_kw={compressor_kw}"
        );
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "exact ideal-capacity stage selection must run continuously, got runtime_fraction={rtf}"
        );
    }

    #[test]
    fn cooling_core_output_matches_telemetry_and_ports() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let operating_mode = eq.telemetry().get(tk::OPERATING_MODE).unwrap_or(-1.0);
        let core = eq.core_output();
        let core_electric_kw = match core.flows.electric_kw {
            Some(super::ElectricPower::Consumption(value)) => value,
            other => panic!("expected electric consumption flow, got {other:?}"),
        };

        assert!(
            (ports.electrical.net_active_w() - power_kw_to_w(electric_kw)).abs() < 1e-3,
            "electrical port and telemetry must match: ports={} telemetry={electric_kw}",
            ports.electrical.net_active_w()
        );
        assert!(
            (core_electric_kw - electric_kw).abs() < 1e-9,
            "CoreOutput electric flow and telemetry must match: core={core_electric_kw} telemetry={electric_kw}"
        );
        assert_eq!(core.state.operating_mode, Some(OperatingMode::Cooling));
        assert_eq!(operating_mode, 2.0);
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "cooling thermal port must remove sensible heat, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn speed_index_appears_in_telemetry_when_cooling() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let speed_index = eq.telemetry().get(tk::SPEED_INDEX);
        assert!(
            speed_index.is_some(),
            "SPEED_INDEX must be present in AC telemetry after a cooling step"
        );
        assert_eq!(
            speed_index.unwrap(),
            eq.core.hvac.runtime.last_speed_index as f64,
            "SPEED_INDEX telemetry must match hvac.last_speed_index"
        );
    }

    #[test]
    fn multi_speed_stage1_uses_second_biquadratic_pair() {
        // Directly verify that calculate_performance uses biquadratic pair N*2 for
        // cap and N*2+1 for EIR when last_speed_index=N.  Stage 0 curves have c0=1.0
        // (cap_ratio≈1.0); stage 1 cap curve has c0=2.0 (cap_ratio≈2.0).
        // When stage 1 is set, sensible_w must exceed the rated 4 kW capacity.
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 1;
            typed.stage_capacities_w = Some(vec![4_000.0]);
            typed.stage_eirs = Some(vec![0.33]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = Some(0.0);
            typed.hysteresis_c = Some(0.0);
        });
        let environment = env(26.0, 0.010, 19.0, 35.0);

        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &environment).unwrap();

        core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        core.operating_mode = OperatingMode::Cooling;
        core.hvac.runtime.duty_cycle = 1.0;
        core.hvac.runtime.last_speed_index = 1;

        let perf = core.calculate_performance(&environment, 15.0).unwrap();

        assert!(
            perf.sensible_cooling_w > 4_000.0,
            "stage-1 biquadratic (c0=2.0) must double cap_ratio; got sensible_w={}",
            perf.sensible_cooling_w
        );
    }

    #[test]
    fn room_ac_explicit_shr_reaches_rated_shr() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: Some(0.65),
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let environment = env(27.0, 0.010, 19.0, 35.0);
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        assert!(
            (eq.core.rated_shr - 0.65).abs() < 1e-12,
            "rated_shr must be 0.65 after init with shr=Some(0.65), got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn checkpoint_thermostat_hysteresis_c() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(1.0);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply a non-default deadband via ThermalSetpoint control.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: Some(3.0),
        })
        .unwrap();
        assert_eq!(eq.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 3.0);

        // Step once so state is fully populated.
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let state = eq.save_state();

        // Create a fresh instance and restore.
        let mut restored = AirConditioner::new(cfg.clone());
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
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(0.0);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Step 10 times at 60s to accumulate speed time.
        for _ in 0..10 {
            eq.update_control(&environment);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
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

        let state = eq.save_state();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(restored.core.hvac.runtime.time_at_current_speed_s, 0.0);

        restored.load_state(&state).unwrap();
        assert!(
            (restored.core.hvac.runtime.time_at_current_speed_s - accumulated).abs() < 1e-9,
            "time_at_current_speed_s must survive checkpoint; expected {accumulated}, got {}",
            restored.core.hvac.runtime.time_at_current_speed_s
        );
    }

    // ── Duct loss and main power telemetry ──────────────────────────────────

    /// AC duct_loss_w must equal gross_cooling * (1 - dse).
    /// ASHRAE 152. Uses pre-DSE gross values, not post-DSE telemetry.
    #[test]
    fn ac_duct_loss_uses_pre_dse_gross_capacity() {
        let cfg = ac_config_with(|c| {
            c.duct = DuctConfig {
                dse_cool: Some(0.8),
                ..DuctConfig::default()
            };
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let duct_loss_w = eq.telemetry().get(tk::DUCT_LOSS_W).unwrap();
        let coil_sensible = eq.telemetry().get(tk::COIL_SENSIBLE_COOLING_W).unwrap();
        let coil_latent = eq.telemetry().get(tk::COIL_LATENT_COOLING_W).unwrap();
        let gross = coil_sensible + coil_latent;
        let dse = eq.core.hvac.config.duct_dse.clamp(0.0, 1.0);
        let expected = gross * (1.0 - dse);
        assert!(
            (duct_loss_w - expected).abs() < 1e-3,
            "duct_loss_w must be gross * (1 - dse) = {expected}, got {duct_loss_w}"
        );
    }

    /// AC main_power_kw equals compressor_kw (total_input - fan = compressor).
    /// OCHRE HVAC.py:575.
    #[test]
    fn ac_main_power_equals_compressor_kw() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let main_kw = eq.telemetry().get(tk::MAIN_POWER_KW).unwrap();
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap();
        let sf = eq.core.hvac.config.space_fraction;
        assert!(
            (main_kw - compressor_kw * sf).abs() < 1e-9,
            "AC main_power must equal compressor_kw * space_fraction, got main={main_kw} compressor={compressor_kw} sf={sf}"
        );
    }

    /// latent_gains_w must equal pre-DSE coil latent * space_fraction.
    /// OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction.
    /// Must NOT use post-DSE latent_cooling_w which is 80% of the correct value at dse=0.8.
    #[test]
    fn ac_latent_gains_uses_pre_dse_gross_latent_times_space_fraction() {
        let cfg = ac_config_with(|c| {
            c.duct = DuctConfig {
                dse_cool: Some(0.8),
                ..DuctConfig::default()
            };
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let latent_gains_w = eq.telemetry().get(tk::LATENT_GAINS_W).unwrap();
        let coil_latent = eq.telemetry().get(tk::COIL_LATENT_COOLING_W).unwrap();
        let sf = eq.core.hvac.config.space_fraction;
        let expected = coil_latent * sf;
        assert!(
            (latent_gains_w - expected).abs() < 1e-3,
            "latent_gains_w must be coil_latent * space_fraction = {expected}, got {latent_gains_w}"
        );
        // Verify it is NOT the post-DSE value.
        let post_dse_latent = eq.telemetry().get(tk::LATENT_COOLING_W).unwrap();
        if sf > 0.0 && coil_latent > 0.0 {
            assert!(
                (latent_gains_w - post_dse_latent).abs() > 1e-3,
                "latent_gains_w must NOT equal post-DSE latent_cooling_w; got gains={latent_gains_w} post_dse={post_dse_latent}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Charge defect ratio — DoD physics-level regression tests
    //
    // These verify that the correction is actually applied to the rated
    // capacity and EIR values in hares-equipment (not just propagated
    // through the typed config). A change to the coefficient constants
    // or to the ordering of the correction relative to other init steps
    // will cause these tests to fail.
    // -----------------------------------------------------------------------

    fn ac_charge_defect_config(charge_defect_ratio: Option<f64>) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                charge_defect_ratio,
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 0.25,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
    }

    #[test]
    fn charge_defect_reduces_rated_cooling_capacity_and_raises_eir() {
        // r = -0.10 (10% undercharge):
        //   capacity × (1 + 0.9 × (−0.10)) = × 0.91 — cooling output falls 9 %
        //   EIR     × (1 + (−0.9) × (−0.10)) = × 1.09 — efficiency degrades 9 %
        // This matches the physics: an undercharged compressor moves less
        // refrigerant and works harder per unit of cooling, so COP falls.
        // Reference: OpenStudio-HPXML (NREL) hvac.rb installation-quality EMS
        // model confirms EIR rises for undercharge (p_values give Y_CH_COP < 1
        // relative to Y_CH_Q for negative f_ch).
        let cfg = ac_charge_defect_config(Some(-0.10));
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        let expected_cap = 8_000.0 * 0.91;
        assert!(
            (capacity - expected_cap).abs() / expected_cap < 1e-3,
            "charge_defect_ratio=-0.10 must reduce capacity by 9%; expected {expected_cap} got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        let expected_eir = 0.25 * 1.09;
        assert!(
            (eir - expected_eir).abs() / expected_eir < 1e-3,
            "charge_defect_ratio=-0.10 must raise EIR by 9% (efficiency degrades); expected {expected_eir} got {eir}"
        );
    }

    #[test]
    fn zero_charge_defect_leaves_capacity_and_eir_unchanged() {
        let cfg = ac_charge_defect_config(Some(0.0));
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        assert!(
            (capacity - 8_000.0).abs() < 1e-9,
            "charge_defect_ratio=0.0 must leave capacity unchanged; got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        assert!(
            (eir - 0.25).abs() < 1e-9,
            "charge_defect_ratio=0.0 must leave EIR unchanged; got {eir}"
        );
    }

    #[test]
    fn absent_charge_defect_leaves_capacity_and_eir_unchanged() {
        let cfg = ac_charge_defect_config(None);
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        assert!(
            (capacity - 8_000.0).abs() < 1e-9,
            "absent charge_defect_ratio must leave capacity unchanged; got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        assert!(
            (eir - 0.25).abs() < 1e-9,
            "absent charge_defect_ratio must leave EIR unchanged; got {eir}"
        );
    }

    // Regression: RoomAcConfig { crankcase_heater_kw: None } must default to 0 kW,
    // not the central-AC default of 0.05 kW. Room ACs have no crankcase heater
    // per OCHRE convention (OCHRE HVAC.py: room/window ACs use 0 W crankcase).
    // OAT at 5 °C — below central-AC threshold (12.8 °C) — distinguishes the two:
    //   Room AC correct:    crankcase = 0.0 kW (no heater)
    //   Central AC default: crankcase = 0.05 kW (wrong for Room AC)
    #[test]
    fn room_ac_crankcase_none_defaults_to_zero_not_central_ac_default() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                // Intentionally None: the equipment init must default to 0.0 kW (Room AC),
                // not 0.05 kW (central-AC default used if the code path is wrong).
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let mut rac = RoomAC::new(cfg.clone());
        // Zone in deadband (21 °C, between heating=18 °C and cooling=24 °C).
        // OAT = 5 °C — below central-AC crankcase threshold (12.8 °C).
        // Compressor is off; crankcase heater power must be 0 kW for Room AC.
        let e = env(21.0, 0.010, 15.0, 5.0);
        rac.init(&cfg, &e).unwrap();
        rac.update_control(&e);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        rac.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "Room AC crankcase_heater_kw=None must default to 0 kW (no crankcase heater); \
             central-AC default would draw 0.05 kW at 5 °C. Got {}",
            ports.electrical.net_active_w()
        );
    }

    #[test]
    fn thermal_setpoint_delta_adjusts_cooling_setpoint_on_ac() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env_state = env(28.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &env_state).unwrap();

        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::THERMAL_SETPOINT_DELTA),
            "AirConditioner must declare THERMAL_SETPOINT_DELTA capability"
        );

        let baseline = eq.core.hvac.effective_setpoints();

        // Dispatch ThermalSetpointDelta: raise cooling setpoint by 1 °C.
        eq.apply_control(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(1.0),
        })
        .unwrap();

        let adjusted = eq.core.hvac.effective_setpoints();
        let expected_cooling = baseline.cooling_c + 1.0;
        assert!(
            (adjusted.cooling_c - expected_cooling).abs() < 1e-9,
            "cooling setpoint must increase by 1 °C: baseline={}, expected={}, got={}",
            baseline.cooling_c,
            expected_cooling,
            adjusted.cooling_c,
        );
        // Heating setpoint unchanged — delta only specified for cooling.
        assert_eq!(
            adjusted.heating_c, baseline.heating_c,
            "heating setpoint must not change when only cooling_delta_c is specified"
        );
    }

    /// Every signal variant corresponding to a declared capability must return
    /// Ok(()) when dispatched to the AirConditioner (T-0048 regression guard).
    #[test]
    fn all_declared_ac_capabilities_return_ok_on_apply_control() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env_state = env(28.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &env_state).unwrap();

        let declared = eq.descriptor().control_capabilities;

        let signals: &[(&str, ControlSignal)] = &[
            (
                "ThermalSetpoint",
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_c: Some(24.0),
                    deadband_c: Some(1.5),
                },
            ),
            (
                "ThermalSetpointDelta",
                ControlSignal::ThermalSetpointDelta {
                    heating_delta_c: None,
                    cooling_delta_c: Some(1.0),
                },
            ),
            (
                "DutyCycle",
                ControlSignal::DutyCycle {
                    on_fraction: 0.5,
                    period_s: None,
                    component: None,
                },
            ),
            (
                "LoadFraction",
                ControlSignal::LoadFraction { fraction: 0.8 },
            ),
            (
                "PowerLimit",
                ControlSignal::PowerLimit {
                    max_power_kw: 2.0,
                    ramp_rate_kw_per_s: None,
                },
            ),
            (
                "ModeOverride",
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Cooling,
                },
            ),
            (
                "DemandResponse",
                ControlSignal::DemandResponse {
                    level: DRLevel::Moderate,
                    duration_s: None,
                },
            ),
            (
                "IdealCapacity",
                ControlSignal::IdealCapacity {
                    capacity_w: -3_000.0,
                },
            ),
            (
                "MaxCapacityFraction",
                ControlSignal::MaxCapacityFraction { fraction: 0.75 },
            ),
        ];

        for (label, signal) in signals {
            let required = signal.required_capability();
            assert!(
                declared.contains(required),
                "test invariant: signal '{label}' requires capability {required:?} \
                 which must be in AC's declared capabilities {declared:?}",
            );
            let result = eq.apply_control(signal);
            assert!(
                result.is_ok(),
                "apply_control for '{label}' signal must return Ok(()), got {result:?}",
            );
        }
    }

    // ── Minimum OAT compressor lockout tests ────────────────────────────

    fn oat_lockout_env(outdoor_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 28.0,
                humidity_ratio: 0.010,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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

    /// Create and init an AirConditioner with a specific `min_oat_cooling_c` and speed mode.
    fn init_ac_with_lockout(min_oat_cooling_c: f64, n_speeds: u8) -> AirConditioner {
        let inner_cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 0.25,
            shr: None,
            number_of_speeds: n_speeds,
            stage_capacities_w: if n_speeds >= 4 {
                Some(vec![3_000.0, 6_000.0, 8_000.0, 10_000.0])
            } else {
                None
            },
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: Some(min_oat_cooling_c),
        };
        let cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), inner_cfg);
        let mut ac = AirConditioner::new(cfg.clone());
        let init_env = oat_lockout_env(30.0);
        ac.init(&cfg, &init_env).expect("init must succeed");
        ac
    }

    /// Single-speed AC: update_control forces Off when OAT is below the
    /// compressor lockout threshold, even though the thermostat calls for cooling.
    #[test]
    fn cooling_locked_out_below_min_oat_single_speed() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_cold = oat_lockout_env(-30.0);
        let mode = ac.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::Off, "lockout must suppress cooling");
    }

    /// Variable-speed AC: update_control forces Off below the lockout threshold.
    #[test]
    fn cooling_locked_out_below_min_oat_variable_speed() {
        let mut ac = init_ac_with_lockout(-25.0, 4);
        let env_cold = oat_lockout_env(-30.0);
        let mode = ac.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::Off);
    }

    /// Cooling operates normally when OAT is at or above the lockout threshold.
    #[test]
    fn cooling_operates_when_oat_above_min() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_warm = oat_lockout_env(30.0);
        let mode = ac.update_control(&env_warm);
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// At exactly the lockout threshold, cooling is not suppressed.
    #[test]
    fn cooling_operates_at_lockout_threshold() {
        let mut ac = init_ac_with_lockout(-20.0, 1);
        let env_at_threshold = oat_lockout_env(-20.0);
        let mode = ac.update_control(&env_at_threshold);
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// The lockout condition is evaluated each step. After a lockout step, a
    /// subsequent warm step recovers to normal operation.
    #[test]
    fn lockout_flag_reset_on_warm_step() {
        let mut ac = init_ac_with_lockout(-25.0, 1);

        // Step 1: cold — lockout active, returns Off.
        let mode = ac.update_control(&oat_lockout_env(-30.0));
        assert_eq!(mode, OperatingMode::Off);

        // Step 2: warm — cooling operates.
        let mode = ac.update_control(&oat_lockout_env(30.0));
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// Cooling telemetry flag is published in step() when lockout was active.
    #[test]
    fn cooling_oat_lockout_telemetry_published() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_cold = oat_lockout_env(-30.0);

        ac.update_control(&env_cold);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let dt = Duration::from_secs(60);
        ac.step(&env_cold, dt, &mut ports).expect("step");

        let val = ac.telemetry().get(tk::COOLING_OAT_LOCKOUT);
        assert!(
            (val.unwrap_or(0.0) - 1.0).abs() < 1e-9,
            "COOLING_OAT_LOCKOUT must be 1.0 when lockout active, got {val:?}"
        );
    }

    /// Cooling telemetry flag is 0.0 when lockout is not active.
    #[test]
    fn cooling_oat_lockout_telemetry_zero_when_normal() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_warm = oat_lockout_env(30.0);

        ac.update_control(&env_warm);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let dt = Duration::from_secs(60);
        ac.step(&env_warm, dt, &mut ports).expect("step");

        let val = ac.telemetry().get(tk::COOLING_OAT_LOCKOUT);
        assert!(
            (val.unwrap_or(1.0) - 0.0).abs() < 1e-9,
            "COOLING_OAT_LOCKOUT must be 0.0 when not locked out"
        );
    }

    #[test]
    fn ac_cop_clamped_to_physical_range() {
        // AC with pathological capacity curve (c0=100 × rated) and identity
        // eir curve produces unbounded COP ~1033. Verify telemetry COP is
        // clamped to [0.0, 8.0].
        let mut cfg = ac_config();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[100,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        // Zone at 26°C (above 24°C setpoint) so AC runs, OAT 30°C.
        let e = env(26.0, 0.009, 18.0, 30.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        eq.update_control(&e);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "AC cooling COP must be in [0.0, 8.0], got {cop}"
        );
        // Unclamped COP would be ~1033; clamping must have reduced it.
        assert!(
            cop < 100.0,
            "COP must have been clamped below 100 (raw ~1033), got {cop}"
        );
    }
}

#[cfg(test)]
mod dr_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
    };

    /// Zone above cooling setpoint, suitable for triggering active cooling.
    fn hot_env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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

    fn hot_env_at_time(zone_temp_c: f64, second: i64) -> EnvironmentState {
        EnvironmentState {
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            ..hot_env(zone_temp_c)
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(0.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: Some(0.10),
                crankcase_heater_threshold_c: Some(12.8),
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    /// Run one full control+step cycle. Returns electric_kw.
    fn step_once(eq: &mut AirConditioner, env: &EnvironmentState) -> f64 {
        let mut ports = make_ports();
        eq.update_control(env);
        eq.step(env, Duration::from_secs(60), &mut ports).unwrap();
        eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0)
    }

    // DR Moderate: cooling setpoint offset = +1°C → unit turns off when zone is between
    // the original and the DR-shifted setpoint.
    #[test]
    fn dr_moderate_shifts_cooling_setpoint_up() {
        let cfg = base_config();
        // Zone at 24.5°C: above base setpoint (24°C) so AC runs normally.
        // After DR Moderate (+1°C), effective setpoint = 25°C > 24.5°C → AC turns off.
        let environment = hot_env(24.5);

        // Baseline: without DR, AC should be cooling.
        let mut eq_base = AirConditioner::new(cfg.clone());
        eq_base.init(&cfg, &environment).unwrap();
        let kw_no_dr = step_once(&mut eq_base, &environment);
        assert!(kw_no_dr > 0.0, "AC must be active without DR at 24.5°C");

        // With DR Moderate: effective setpoint raised to 25°C; zone 24.5°C < 25°C → no cooling.
        let mut eq_dr = AirConditioner::new(cfg.clone());
        eq_dr.init(&cfg, &environment).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        let kw_with_dr = step_once(&mut eq_dr, &environment);
        assert!(
            kw_with_dr < kw_no_dr,
            "DR Moderate must eliminate cooling demand when zone is between original and DR setpoint; baseline={kw_no_dr:.4} kW, dr={kw_with_dr:.4} kW"
        );
    }

    // DR Critical: load_fraction = 0.5, output must be halved vs the Critical-free run.
    #[test]
    fn dr_critical_reduces_load_fraction() {
        let cfg = base_config();
        // Zone far above both base (24°C) and critical-adjusted (27°C) setpoints.
        let environment = hot_env(32.0);

        let mut eq_base = AirConditioner::new(cfg.clone());
        eq_base.init(&cfg, &environment).unwrap();
        let kw_full = step_once(&mut eq_base, &environment);

        let mut eq_dr = AirConditioner::new(cfg.clone());
        eq_dr.init(&cfg, &environment).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        let kw_dr = step_once(&mut eq_dr, &environment);

        assert!(kw_full > 0.0, "baseline must draw power");
        assert!(kw_dr > 0.0, "DR Critical must not completely shut off");
        // DR Critical sets dr_load_fraction=0.5, so power must be ≈ half.
        // Allow 15% tolerance for cooling setpoint offset effects on load selection.
        let ratio = kw_dr / kw_full;
        assert!(
            ratio < 0.6,
            "DR Critical load_fraction=0.5 must reduce power by ≥40%; ratio={ratio:.3}"
        );
    }

    // GridEmergency: dr_load_fraction=0 → full shed → zero output.
    #[test]
    fn dr_grid_emergency_forces_equipment_off() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        let kw = step_once(&mut eq, &environment);
        assert_eq!(kw, 0.0, "GridEmergency must force zero output");
    }

    // Normal after Critical: all overrides cleared, setpoint_offset=0, fractions=1.
    #[test]
    fn dr_normal_clears_all_overrides() {
        let cfg = base_config();
        let environment = hot_env(32.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply Critical DR first.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();
        let kw_critical = step_once(&mut eq, &environment);

        // Now clear with Normal.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        })
        .unwrap();
        let kw_normal = step_once(&mut eq, &environment);

        assert!(kw_critical > 0.0, "Critical must allow some output");
        // After Normal, output must recover toward the baseline (unrestricted) level.
        assert!(
            kw_normal > kw_critical,
            "Normal must clear DR load reduction; normal={kw_normal:.4}, critical={kw_critical:.4}"
        );
    }

    // DR with duration_s: auto-reverts to Normal after the duration elapses.
    // The AC's update_control decrements dr_duration_remaining_s by time_res (1 min = 60 s)
    // each call. DR persists while remaining > 0 and auto-reverts when it reaches <= 0.
    #[test]
    fn dr_duration_auto_reverts() {
        let cfg = base_config();
        let env_t0 = hot_env_at_time(28.0, 0);
        let env_t60 = hot_env_at_time(28.0, 60);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env_t0).unwrap();

        // GridEmergency for 120 s (two 60-s steps); time_res = 1 min = 60 s.
        // First update_control at t=0 decrements by 60 s → remaining = 60 s > 0 → still active.
        // Second update_control at t=60 decrements by 60 s → remaining = 0 s → reverts to Normal.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(120.0),
        })
        .unwrap();

        // Step at t=0: remaining decrements 60→60 s (still active) → must be off.
        let kw_during = step_once(&mut eq, &env_t0);
        assert_eq!(
            kw_during, 0.0,
            "GridEmergency must shed load while duration active"
        );

        // Step at t=60: remaining decrements 60→0 s → auto-reverts → AC can cool.
        let kw_after = step_once(&mut eq, &env_t60);
        assert!(
            kw_after > 0.0,
            "After DR duration expires, AC must recover; kw_after={kw_after}"
        );
    }

    // DutyCycle 0.5: output approximately halved vs duty_cycle=1.
    #[test]
    fn duty_cycle_scales_cooling_output() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &environment).unwrap();
        let kw_full = step_once(&mut eq_full, &environment);

        let mut eq_half = AirConditioner::new(cfg.clone());
        eq_half.init(&cfg, &environment).unwrap();
        eq_half
            .apply_control(&ControlSignal::DutyCycle {
                on_fraction: 0.5,
                period_s: None,
                component: None,
            })
            .unwrap();
        let kw_half = step_once(&mut eq_half, &environment);

        assert!(kw_full > 0.0, "full duty must draw power");
        assert!(kw_half > 0.0, "half duty must draw power");
        let ratio = kw_half / kw_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "DutyCycle 0.5 must halve output ±10%; ratio={ratio:.3}"
        );
    }

    // LoadFraction 0.0: forces AC off for that step.
    // The signal is one-step transient, but it survives update_control and clears
    // at the end of step().
    #[test]
    fn load_fraction_zero_forces_off() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // update_control determines thermostat mode first (sets operating_mode = Cooling).
        eq.update_control(&environment);
        // Applying after update_control still affects the current step.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports = make_ports();
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw, 0.0, "LoadFraction 0.0 must force zero output this step");
    }

    // LoadFraction is transient for one step. The next step restores the default
    // unless the control is re-applied.
    #[test]
    fn load_fraction_resets_each_step() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Step 1: update_control → apply LoadFraction=0 → step → must be off.
        eq.update_control(&environment);
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports1 = make_ports();
        eq.step(&environment, Duration::from_secs(60), &mut ports1)
            .unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw_step1, 0.0, "step 1 with LoadFraction=0 must be off");

        // Step 2: no signal reapplied → AC runs again.
        let kw_step2 = step_once(&mut eq, &environment);
        assert!(
            kw_step2 > 0.0,
            "step 2 must recover after transient LoadFraction reset; got {kw_step2}"
        );
    }

    // PowerLimit clamps electrical draw and scales thermal proportionally.
    #[test]
    fn power_limit_clamps_electric_draw() {
        let cfg = base_config();
        // Zone well above setpoint to drive full AC output.
        let environment = hot_env(30.0);

        let mut eq_unlimited = AirConditioner::new(cfg.clone());
        eq_unlimited.init(&cfg, &environment).unwrap();
        let kw_unlimited = step_once(&mut eq_unlimited, &environment);

        // Set limit well below expected draw.
        let limit_kw = (kw_unlimited * 0.5).max(0.1);
        let mut eq_limited = AirConditioner::new(cfg.clone());
        eq_limited.init(&cfg, &environment).unwrap();
        eq_limited
            .apply_control(&ControlSignal::PowerLimit {
                max_power_kw: limit_kw,
                ramp_rate_kw_per_s: None,
            })
            .unwrap();
        let kw_limited = step_once(&mut eq_limited, &environment);

        assert!(
            kw_unlimited > limit_kw,
            "unlimited draw must exceed limit for this test to be meaningful"
        );
        assert!(
            kw_limited <= limit_kw + 1e-9,
            "PowerLimit must clamp electric draw: limited={kw_limited:.4}, limit={limit_kw:.4}"
        );
        assert!(
            kw_limited > 0.0,
            "PowerLimit must not zero out the equipment"
        );
    }
}

#[cfg(test)]
mod crankcase_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, HumidityAccumulator, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
    };

    fn env(
        zone_temp_c: f64,
        humidity_ratio: f64,
        wet_bulb_c: f64,
        outdoor_c: f64,
    ) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio,
                relative_humidity: 0.45,
                wet_bulb_c,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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

    fn ac_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn cold_env(outdoor_c: f64, zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp_c - 3.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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
                .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(26.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(0.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: Some(0.10),
                crankcase_heater_threshold_c: Some(12.8),
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn base_config_with(mutator: impl FnOnce(&mut CentralAirConditionerConfig)) -> EquipmentConfig {
        let mut typed = base_config()
            .typed::<CentralAirConditionerConfig>()
            .unwrap();
        mutator(&mut typed);
        let mut cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed);
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn step_ac_off(cfg: &EquipmentConfig, outdoor_c: f64) -> f64 {
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(outdoor_c, 20.0); // zone below cooling setpoint (26 C) - AC Off
        eq.init(cfg, &e).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0)
    }

    // Test 1: Standalone AC crankcase power = rated when cooling RTF=0 and OAT < threshold
    #[test]
    fn standalone_ac_crankcase_full_power_when_off_and_cold() {
        let cfg = base_config();
        let kw = step_ac_off(&cfg, 5.0);
        // 100 W rated, fully off (RTF=0) -> crankcase = 0.10 kW
        assert!(
            (kw - 0.10).abs() < 1e-9,
            "expected 0.10 kW crankcase, got {kw}"
        );
    }

    // Test 2: Standalone AC crankcase power = 0 when OAT >= threshold
    #[test]
    fn standalone_ac_no_crankcase_when_warm() {
        let cfg = base_config();
        let kw = step_ac_off(&cfg, 15.0); // 15.0 >= 12.8 threshold
        assert_eq!(kw, 0.0, "no crankcase power above OAT threshold, got {kw}");
    }

    // Test 3: HP system crankcase uses max(heating_RTF, cooling_RTF)
    #[test]
    fn hp_crankcase_uses_max_rtf() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling_rtf=0.3, heating_rtf=0.7 -> max=0.7 -> power = 0.10*(1-0.7)=0.03 kW
        let kw = eq.crankcase_heater_power(5.0, 0.3, Some(0.7));
        assert!((kw - 0.03).abs() < 1e-9, "expected 0.03 kW, got {kw}");
    }

    // Test 4: HP cooling mode active (RTF=0.6), heating off -> crankcase = rated * 0.4
    #[test]
    fn hp_cooling_active_reduces_crankcase_proportionally() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling_rtf=0.6, heating off (0.0) -> max=0.6 -> power = 0.10*(1-0.6)=0.04 kW
        let kw = eq.crankcase_heater_power(5.0, 0.6, Some(0.0));
        assert!((kw - 0.04).abs() < 1e-9, "expected 0.04 kW, got {kw}");
    }

    // Test 5: HP heating mode active (RTF=0.8), cooling off -> crankcase = rated * 0.2
    #[test]
    fn hp_heating_active_reduces_crankcase_proportionally() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling_rtf=0.0, heating_rtf=0.8 -> max=0.8 -> power = 0.10*(1-0.8)=0.02 kW
        let kw = eq.crankcase_heater_power(5.0, 0.0, Some(0.8));
        assert!((kw - 0.02).abs() < 1e-9, "expected 0.02 kW, got {kw}");
    }

    // Test 6: Both coils off -> full crankcase power
    #[test]
    fn hp_both_coils_off_gives_full_crankcase_power() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        let kw = eq.crankcase_heater_power(5.0, 0.0, Some(0.0));
        assert!((kw - 0.10).abs() < 1e-9, "expected 0.10 kW, got {kw}");
    }

    // Test 7: Both coils have RTF -> uses the maximum
    #[test]
    fn hp_uses_maximum_rtf_when_both_coils_running() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling=0.4, heating=0.9 -> max=0.9 -> power = 0.10*(1-0.9)=0.01 kW
        let kw = eq.crankcase_heater_power(5.0, 0.4, Some(0.9));
        assert!((kw - 0.01).abs() < 1e-9, "expected 0.01 kW, got {kw}");
    }

    // Test 8: Temperature curve modifies capacity correctly
    #[test]
    fn crankcase_capacity_curve_scales_rated_power() {
        // Curve: effective = rated * (1.0 + 0.1*T + 0.0*T^2); at T=5C: multiplier=1.5
        let cfg = base_config_with(|typed| {
            typed.crankcase_capacity_curve_coeffs = Some([1.0, 0.1, 0.0]);
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // crankcase_heater_power: rated=0.10 * curve(5C)=1.5 * (1-0.0) = 0.15 kW
        let kw = eq.crankcase_heater_power(5.0, 0.0, None);
        assert!(
            (kw - 0.15).abs() < 1e-9,
            "expected 0.15 kW with curve at T=5C, got {kw}"
        );
    }

    // Test 9: Crankcase power included in total electric consumption telemetry
    #[test]
    fn crankcase_power_in_electric_kw_telemetry() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0); // cold outdoor, zone below setpoint -> AC off
        eq.init(&cfg, &e).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let port_w = ports.electrical.net_active_w();
        // Crankcase (0.10 kW = 100 W) must appear in both telemetry and port contribution.
        assert!(
            (electric_kw - 0.10).abs() < 1e-9,
            "telemetry electric_kw must equal crankcase rated 0.10 kW, got {electric_kw}"
        );
        assert!(
            (port_w - 100.0).abs() < 1.0,
            "port electrical draw must equal crankcase rated 100 W (0.10 kW), got {port_w}"
        );
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = ac_config();
        // Zone is above cooling setpoint so cooling is active.
        let e = env(27.0, 0.008, 19.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Apply a Critical DR event with a 300 s duration.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &e).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.core.dr_level,
            DRLevel::Critical,
            "dr_level must survive save/load"
        );
        assert!(
            restored.core.dr_setpoint_offset_c > 0.0,
            "dr_setpoint_offset_c must be non-zero after Critical DR (got {})",
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
}

#[cfg(test)]
mod ideal_capacity_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{Equipment, EquipmentConfig, HvacSetpointConfig};

    /// Build an `EnvironmentState` with a configurable zone temperature and time resolution.
    fn make_env(zone_temp_c: f64, time_res_s: i64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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

    /// AC with setpoint=24°C, hysteresis=1°C, single-speed, flat biquadratic curves.
    fn ac_config() -> EquipmentConfig {
        super::typed_ac_test_config(0.33)
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    // At 60 s timestep (fine resolution), an IdealCapacity signal is ignored because
    // use_ideal=false. Zone is above FSM turn-on threshold (setpoint + hysteresis = 25°C),
    // so thermostat-driven load_fraction = (26 - 24) / 1 = 1.0 → RTF = 1.0 (full on).
    #[test]
    fn fine_timestep_fsm_cycling_ignores_ideal_capacity_signal() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 26°C exceeds FSM turn-on threshold of 25°C.
        let env = make_env(26.0, 60);
        eq.init(&cfg, &env).unwrap();
        // Signal provides half-rated load; must be ignored when use_ideal=false.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -4_000.0,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "FSM Cooling at 60 s must ignore IdealCapacity and produce RTF=1.0, got {rtf}"
        );
    }

    // At 900 s timestep (coarse resolution), a solver-provided IdealCapacity signal for
    // half the rated capacity (4000 W of 8000 W rated) produces fractional RTF.
    // Flow: apply signal → update_control (use_ideal=true, FSM enters Cooling at 26°C) →
    //   ideal_capacity_w=-4000 < 0 → load_fraction = 4000/8000 = 0.5 → RTF ≈ 0.5.
    #[test]
    fn coarse_timestep_ideal_signal_produces_fractional_rtf() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 26°C exceeds FSM turn-on threshold of 25°C; FSM enters Cooling.
        let env = make_env(26.0, 900);
        eq.init(&cfg, &env).unwrap();
        // Half rated capacity: 4000 W of 8000 W rated → load_fraction = 0.5.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -4_000.0,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        // Flat biquadratic [1,0,0,0,0,0] → cap_ratio=1.0. Raw PLR = 4000/8000 = 0.5,
        // but PLF cycling-degradation (Cd=0.25) raises effective PLR to ~0.57.
        assert!(
            (rtf - 0.5).abs() < 0.15,
            "IdealCapacity=-4000 W at 900 s (half rated) must produce RTF near 0.5, got {rtf}"
        );
    }

    // At 900 s timestep with no solver signal and zone below the FSM cooling turn-on
    // threshold (25°C), the thermostat stays Deadband → AC is off.
    #[test]
    fn coarse_timestep_no_signal_below_fsm_threshold_produces_zero_rtf() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 22°C is below FSM turn-on threshold (25°C); thermostat stays Deadband.
        let env = make_env(22.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert_eq!(
            rtf, 0.0,
            "900 s timestep with zone below FSM turn-on and no solver signal must \
             produce RTF=0.0, got {rtf}"
        );
    }

    #[test]
    fn max_capacity_fraction_clips_cooling_output() {
        let cfg = ac_config();
        let environment = make_env(26.0, 900);

        // Baseline: run at full ideal capacity (rated = 8000 W).
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -8_000.0,
        })
        .unwrap();
        eq.update_control(&environment);
        let mut ports = make_ports();
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        let full_cooling_w = eq
            .telemetry()
            .get(tk::SENSIBLE_COOLING_W)
            .unwrap_or(0.0)
            .abs()
            + eq.telemetry()
                .get(tk::LATENT_COOLING_W)
                .unwrap_or(0.0)
                .abs();
        assert!(
            full_cooling_w > 1000.0,
            "baseline should produce meaningful output"
        );

        // Apply MaxCapacityFraction(0.5) -- output must not exceed ~50% of baseline.
        let mut eq2 = AirConditioner::new(cfg.clone());
        eq2.init(&cfg, &environment).unwrap();
        eq2.apply_control(&ControlSignal::MaxCapacityFraction { fraction: 0.5 })
            .unwrap();
        eq2.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -8_000.0,
        })
        .unwrap();
        eq2.update_control(&environment);
        let mut ports2 = make_ports();
        eq2.step(&environment, Duration::from_secs(900), &mut ports2)
            .unwrap();
        let capped_cooling_w = eq2
            .telemetry()
            .get(tk::SENSIBLE_COOLING_W)
            .unwrap_or(0.0)
            .abs()
            + eq2
                .telemetry()
                .get(tk::LATENT_COOLING_W)
                .unwrap_or(0.0)
                .abs();
        let limit = full_cooling_w * 0.55;
        assert!(
            capped_cooling_w <= limit,
            "MaxCapacityFraction(0.5) should limit output to ~50%; \
             got {capped_cooling_w:.1}W vs full {full_cooling_w:.1}W (limit={limit:.1}W)"
        );
    }

    // Test: RoomAC ideal_target() delegation to CoolingCore.
    //
    // At a coarse timestep (>= 5 min), use_ideal_capacity() returns true for AcCooler
    // equipment types (which RoomAC uses). RoomAC delegates ideal_target() to
    // CoolingCore, same delegation pattern as AirConditioner.
    #[test]
    fn room_ac_ideal_target_returns_some_at_coarse_timestep() {
        use super::RoomAC;
        use crate::RoomAcConfig;

        // Build a RoomAC: zone_id=1, cooling_setpoint=24°C, hysteresis=1°C.
        let cfg = EquipmentConfig::from_typed(
            "rac".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let mut eq = RoomAC::new(cfg.clone());
        // Zone above cooling turn-on threshold: 26°C > setpoint+hysteresis = 25°C.
        // Coarse timestep (900 s >= 300 s threshold) → use_ideal = true.
        let env = make_env(26.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        // After update_control with a hot zone and coarse timestep, ideal_target() must
        // return Some((ZoneId(1), ~24.0)) — the zone id and cooling setpoint.
        let target = eq.ideal_target();
        assert!(
            target.is_some(),
            "RoomAC ideal_target() must return Some at coarse timestep (900 s) when zone \
             (26°C) is above cooling turn-on threshold; got None"
        );
        let (zone_id, setpoint_c) = target.unwrap();
        assert_eq!(
            zone_id,
            ZoneId(1),
            "RoomAC ideal_target zone must match configured zone_id"
        );
        assert!(
            (setpoint_c - 24.0).abs() < 0.5,
            "RoomAC ideal_target setpoint must reflect cooling setpoint (~24°C); got {setpoint_c:.2}°C"
        );
    }

    // Companion: at a fine timestep (60 s), use_ideal is false and ideal_target() must
    // return None — same behavior as AirConditioner (parity check).
    #[test]
    fn room_ac_ideal_target_returns_none_at_fine_timestep() {
        use super::RoomAC;
        use crate::RoomAcConfig;

        let cfg = EquipmentConfig::from_typed(
            "rac_fine".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let mut eq = RoomAC::new(cfg.clone());
        // Fine timestep (60 s < 300 s threshold) → use_ideal = false.
        let env = make_env(26.0, 60);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        assert!(
            eq.ideal_target().is_none(),
            "RoomAC ideal_target() must return None at fine timestep (60 s); \
             use_ideal=false so solver should not query this unit"
        );
    }

    // Verify RoomAC and AirConditioner return identical ideal_target() outputs
    // for identical input conditions — the same delegation to CoolingCore.
    #[test]
    fn room_ac_ideal_target_matches_air_conditioner_parity() {
        use crate::{CentralAirConditionerConfig, DuctConfig, RoomAcConfig};

        let ac_cfg = EquipmentConfig::from_typed(
            "ac_par".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                shr: None,
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                fraction_load_served: Some(1.0),
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let rac_cfg = EquipmentConfig::from_typed(
            "rac_par".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let env = make_env(26.0, 900);

        let mut ac = AirConditioner::new(ac_cfg.clone());
        ac.init(&ac_cfg, &env).unwrap();
        ac.update_control(&env);

        let mut rac = super::RoomAC::new(rac_cfg.clone());
        rac.init(&rac_cfg, &env).unwrap();
        rac.update_control(&env);

        let ac_target = ac.ideal_target();
        let rac_target = rac.ideal_target();

        // Both must return Some with the same zone and setpoint.
        assert!(
            ac_target.is_some(),
            "AirConditioner must return Some ideal_target at coarse timestep"
        );
        assert!(
            rac_target.is_some(),
            "RoomAC must return Some ideal_target at coarse timestep"
        );
        let (ac_zone, ac_sp) = ac_target.unwrap();
        let (rac_zone, rac_sp) = rac_target.unwrap();
        assert_eq!(ac_zone, rac_zone, "zone IDs must match");
        assert!(
            (ac_sp - rac_sp).abs() < 0.01,
            "AirConditioner setpoint ({ac_sp:.3}°C) and RoomAC setpoint ({rac_sp:.3}°C) must match"
        );
    }
}

#[cfg(test)]
mod defaults_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, HumidityAccumulator, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::{AirConditioner, RoomAC};
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
        RoomAcConfig,
    };

    fn make_env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn two_speed_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 10_000.0,
                eir: 3.412_141_633 / 16.0,
                shr: Some(0.75),
                number_of_speeds: 2,
                stage_capacities_w: Some(vec![7_200.0, 10_000.0]),
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    // Two-speed AC initialised without an explicit low_speed_capacity_fraction must
    // default to 0.72, matching the OCHRE/AHRI lookup table for 2-speed AC coolers.
    #[test]
    fn two_speed_ac_defaults_to_0_72_low_speed_capacity_fraction() {
        let cfg = two_speed_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env = make_env(28.0);
        eq.init(&cfg, &env).unwrap();

        assert!(
            (eq.core.hvac.config.low_speed_capacity_fraction - 0.72).abs() < 1e-12,
            "2-speed AC must default to low_speed_capacity_fraction=0.72 (OCHRE/AHRI default), got {}",
            eq.core.hvac.config.low_speed_capacity_fraction
        );
    }

    // Single-stage AC initialised without explicit latent degradation params must
    // have the Henderson-Rengarajan model active with EnergyPlus residential DX
    // defaults (twet=1000 s, gamma=1.5, Nmax=3 cyc/hr, tau=45 s).
    // At part load (RTF < 1) the model must return SHR > steady-state SHR,
    // reflecting moisture re-evaporation during the off cycle.
    #[test]
    fn central_ac_defaults_latent_degradation_active_and_raises_shr_at_part_load() {
        let cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 16.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let env = make_env(28.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.core.latent_degradation.is_active(),
            "latent degradation must be active after init for central AC"
        );
        assert!(
            (eq.core.latent_degradation.twet_rated_s - 1000.0).abs() < 1e-9,
            "twet_rated_s must be 1000 s (EnergyPlus Coil:Cooling:DX suggested default), got {}",
            eq.core.latent_degradation.twet_rated_s
        );
        assert!(
            (eq.core.latent_degradation.gamma_rated - 1.5).abs() < 1e-9,
            "gamma_rated must be 1.5 (EnergyPlus residential default), got {}",
            eq.core.latent_degradation.gamma_rated
        );

        // Run the AC at partial load (zone barely above setpoint) so RTF < 1
        // and the Henderson-Rengarajan model has room to degrade latent capacity.
        // The reported SHR must be strictly above the rated SHR of 0.75 because
        // moisture re-evaporation during off cycles raises the effective SHR.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let reported_shr = eq.telemetry().get(tk::SHR).unwrap_or(0.0);
        assert!(
            reported_shr >= 0.75,
            "SHR with latent degradation at part load must be >= rated SHR 0.75, got {reported_shr:.4}"
        );
        assert!(
            reported_shr <= 1.0,
            "SHR must be <= 1.0, got {reported_shr:.4}"
        );
    }

    #[test]
    fn room_ac_does_not_have_latent_degradation() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                shr: None,
                startup_cd: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        );

        let env = make_env(28.0);
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert!(
            !eq.core.latent_degradation.is_active(),
            "Room AC must not have latent degradation active after init"
        );
    }
}

// Parity regression tests: select_variable_speed_cooling and
// HvacEquipment::select_multi_speed both delegate to interpolate_speed_stages.
// These tests verify that both paths produce identical SpeedSelection values for
// the same normalised load fractions and capacity stages.  If either caller is
// ever re-implemented without going through the shared function, a divergence
// will appear here first.
//
// The tests exercise select_variable_speed_cooling directly by constructing a
// minimal CoolingCore and overriding cooling_capacities_w after init.
#[cfg(test)]
mod speed_selection_parity_tests {
    use super::CoolingCore;
    use super::{AirConditioner, RoomAC};
    use crate::hvac::{HvacEquipment, HvacEquipmentType, SpeedControlMode, ThermostatMode};
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
        RoomAcConfig,
    };
    use hares_types::{EnvironmentState, GridState, WeatherState, ZoneId, ZoneState};

    fn minimal_env() -> EnvironmentState {
        use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 26.0,
                humidity_ratio: 0.010,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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
                .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Build a CoolingCore with the given capacity stages and then override
    /// cooling_capacities_w directly so both sides use the same stage values.
    fn make_cooling_core(capacities_w: Vec<f64>) -> CoolingCore {
        let typed = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: *capacities_w.last().unwrap_or(&10_000.0),
            eir: 3.412_141_633 / 14.0,
            shr: Some(0.75),
            number_of_speeds: capacities_w.len() as u8,
            stage_capacities_w: Some(capacities_w.clone()),
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: Some(0.0),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        let mut cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed);
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        let env = minimal_env();
        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &env).unwrap();
        // Override capacities so the test controls the exact stage values.
        core.hvac.config.cooling_capacities_w = capacities_w;
        core
    }

    /// Build an HvacEquipment in MultiSpeedInterpolated/Cooling mode with the
    /// same capacity stages so we can call select_speed() (→ select_multi_speed).
    fn make_hvac_cooling(capacities_w: Vec<f64>) -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        hvac.config.cooling_capacities_w = capacities_w;
        hvac.thermostat_fsm.mode = ThermostatMode::Cooling;
        hvac
    }

    // Regression: select_variable_speed_cooling must produce the same
    // speed_index and speed_frac as select_multi_speed for every interior bracket.
    // If the two paths diverge this test will catch it.
    #[test]
    fn variable_speed_multi_speed_parity_interior_brackets() {
        // 4-stage: fractions [0.4, 0.6, 0.8, 1.0]
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let probes: &[(f64, &str)] = &[
            (0.5, "between stage 0-1"),
            (0.7, "between stage 1-2"),
            (0.9, "between stage 2-3"),
        ];
        for &(frac, label) in probes {
            let vs = core.select_variable_speed_cooling(frac);
            let ms = hvac.select_speed(frac);
            assert_eq!(
                vs.speed_index, ms.speed_index,
                "{label}: speed_index mismatch — variable_speed={} multi_speed={}",
                vs.speed_index, ms.speed_index
            );
            assert!(
                (vs.speed_frac - ms.speed_frac).abs() < 1e-12,
                "{label}: speed_frac mismatch — variable_speed={} multi_speed={}",
                vs.speed_frac,
                ms.speed_frac
            );
            assert_eq!(
                vs.part_load_ratio, ms.part_load_ratio,
                "{label}: part_load_ratio mismatch — variable_speed={} multi_speed={}",
                vs.part_load_ratio, ms.part_load_ratio
            );
        }
    }

    // Regression: below the lowest stage, PLR computation must agree between paths.
    #[test]
    fn variable_speed_multi_speed_parity_below_lowest_stage() {
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let load = 0.3; // below cap_frac[0]=0.4
        let vs = core.select_variable_speed_cooling(load);
        let ms = hvac.select_speed(load);

        assert_eq!(vs.speed_index, 0);
        assert_eq!(vs.speed_frac, 0.0);
        // PLR = 0.3 / 0.4 = 0.75 for both paths.
        assert!(
            (vs.part_load_ratio - ms.part_load_ratio).abs() < 1e-12,
            "PLR below lowest stage must agree: variable_speed={} multi_speed={}",
            vs.part_load_ratio,
            ms.part_load_ratio
        );
    }

    // Regression: at full load both paths must return the last index.
    #[test]
    fn variable_speed_multi_speed_parity_at_full_load() {
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let vs = core.select_variable_speed_cooling(1.0);
        let ms = hvac.select_speed(1.0);

        assert_eq!(vs.speed_index, ms.speed_index, "top-speed index must agree");
        assert_eq!(vs.speed_frac, 0.0);
        assert_eq!(vs.part_load_ratio, 1.0);
        assert_eq!(ms.part_load_ratio, 1.0);
    }

    #[test]
    fn room_ac_explicit_shr_used_not_derived_default() {
        // When shr is explicitly set, the derived default must not be used.
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: Some(0.65),
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let env = minimal_env();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        assert!(
            (eq.core.rated_shr - 0.65).abs() < 1e-12,
            "explicit shr=0.65 must be used, got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn room_ac_default_shr_matches_derived_value() {
        // Without explicit SHR, the room AC must derive SHR from airflow
        // using the EnergyPlus auto-sizing formula.
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let env = minimal_env();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        // At room AC airflow (320 CFM/ton), EnergyPlus auto-sizing predicts SHR ≈ 0.69.
        assert!(
            (eq.core.rated_shr - 0.69).abs() < 0.01,
            "room AC default SHR should be ~0.69, got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn room_ac_and_central_ac_default_shr_differ() {
        // Room AC and central AC must have distinct default SHR values
        // because they operate at different airflow-per-ton ratios.
        // Room AC (lower airflow) → lower SHR.
        // Central AC (higher airflow) → higher SHR.
        let room_cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let central_cfg = EquipmentConfig::from_typed(
            "CAC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 10.0,
                shr: None,
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        );
        let env_state = minimal_env();
        let mut room_eq = RoomAC::new(room_cfg.clone());
        room_eq.init(&room_cfg, &env_state).unwrap();
        let mut central_eq = AirConditioner::new(central_cfg.clone());
        central_eq.init(&central_cfg, &env_state).unwrap();

        let room_shr = room_eq.core.rated_shr;
        let central_shr = central_eq.core.rated_shr;

        assert!(
            room_shr < central_shr,
            "room AC default SHR ({room_shr}) must be less than central AC default SHR ({central_shr})"
        );
        assert!(
            room_shr > 0.0,
            "room AC default SHR must be positive, got {room_shr}"
        );
        assert!(
            central_shr > 0.0,
            "central AC default SHR must be positive, got {central_shr}"
        );
    }
}
