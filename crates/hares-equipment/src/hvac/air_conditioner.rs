//! Central and room air conditioner models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CoreState,
    DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::ac_config::{
    CentralAirConditionerConfig, RoomAcConfig, default_telemetry, load_curve_pair, telemetry_fields,
};
use super::coil_physics::{
    CoilResult, LatentDegradationParams, calculate_shr, effective_shr_with_latent_degradation,
};
use super::latent_degradation::compute_coil_ao_by_stage;
use super::speed_control::SpeedSelection;
use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, SpeedControlMode, ThermostatMode,
    helpers::{equipment_id_from_config, lookup_zone, operating_mode_code, zone_id_from_config},
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
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    crankcase_heater_on: bool,
    crankcase_heater_kw: f64,
    /// Rated crankcase heater power [kW]; loaded from config, variant-dependent default.
    crankcase_rated_kw: f64,
    /// Outdoor temperature threshold [°C] below which crankcase heater activates.
    crankcase_threshold_c: f64,
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
    /// Runtime fraction (PLR / PLF) for this step — used for crankcase accounting.
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
}

impl CoolingCore {
    fn apply_cooling_startup_cd(&mut self, cd: Option<f64>) {
        if let Some(cd) = cd {
            self.hvac.plf_cooling_degradation_coeff = cd;
            self.hvac.startup.c_d = cd;
        }
    }

    fn select_variable_speed_cooling(
        &mut self,
        requested_capacity_fraction: f64,
    ) -> SpeedSelection {
        let requested_capacity_fraction = requested_capacity_fraction.clamp(0.0, 1.0);
        let capacities = &self.hvac.cooling_capacities_w;
        let selection = if capacities.is_empty() {
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0,
            }
        } else if capacities.len() == 1 {
            SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: requested_capacity_fraction,
            }
        } else {
            let max_capacity_w = capacities.last().copied().unwrap_or_default();
            let capacity_fractions: Vec<f64> = capacities
                .iter()
                .map(|capacity_w| capacity_w / max_capacity_w.max(f64::MIN_POSITIVE))
                .collect();
            if requested_capacity_fraction <= capacity_fractions[0] {
                SpeedSelection {
                    speed_index: 0,
                    speed_frac: 0.0,
                    part_load_ratio: (requested_capacity_fraction
                        / capacity_fractions[0].max(f64::MIN_POSITIVE))
                    .clamp(0.0, 1.0),
                }
            } else if requested_capacity_fraction
                >= *capacity_fractions
                    .last()
                    .expect("non-empty capacity fractions")
            {
                SpeedSelection {
                    speed_index: capacity_fractions.len() - 1,
                    speed_frac: 0.0,
                    part_load_ratio: 1.0,
                }
            } else {
                let hi = capacity_fractions
                    .partition_point(|&fraction| fraction < requested_capacity_fraction);
                let lo = hi - 1;
                let span = capacity_fractions[hi] - capacity_fractions[lo];
                let speed_frac = if span > f64::EPSILON {
                    (requested_capacity_fraction - capacity_fractions[lo]) / span
                } else {
                    0.0
                };
                SpeedSelection {
                    speed_index: lo,
                    speed_frac,
                    part_load_ratio: 1.0,
                }
            }
        };
        self.hvac.last_speed_index = selection.speed_index;
        self.hvac.last_speed_frac = selection.speed_frac;
        selection
    }

    fn variable_speed_point(&self, selection: SpeedSelection) -> (f64, f64, f64) {
        let stage_capacity_w = if self.hvac.cooling_capacities_w.len() > 1
            && selection.speed_frac > 0.0
        {
            self.hvac.interpolated_capacity(
                &self.hvac.cooling_capacities_w,
                selection.speed_index,
                selection.speed_frac,
            )
        } else if self.hvac.cooling_capacities_w.len() == 1 {
            self.hvac
                .cooling_capacities_w
                .first()
                .copied()
                .unwrap_or_default()
        } else {
            HvacEquipment::capacity_at_stage(&self.hvac.cooling_capacities_w, selection.speed_index)
        };
        let stage_eir = if self.hvac.cooling_capacities_w.len() > 1 && selection.speed_frac > 0.0 {
            self.hvac
                .interpolated_eir(selection.speed_index, selection.speed_frac)
        } else {
            self.hvac.eir_at_stage(selection.speed_index)
        };
        let part_load_ratio = if self.hvac.cooling_capacities_w.len() == 1
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
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
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
                    | ControlCapabilities::IDEAL_CAPACITY,
                core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
                telemetry_fields: telemetry_fields(),
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::AcCooler, zone),
            operating_mode: OperatingMode::Off,
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
        }
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;

        if self.is_room_ac {
            if self.hvac.speed_control_mode != SpeedControlMode::SingleSpeed {
                return Err(HaresError::Equipment(
                    "Room AC supports only single-speed mode".to_string(),
                ));
            }
            self.hvac.speed_control_mode = SpeedControlMode::SingleSpeed;
            self.hvac.duct_dse = 1.0;
            self.hvac.duct_zone_id = None;
        } else {
            self.hvac.duct_zone_id = super::helpers::parse_zone_id_key(config, "duct_zone_id");
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

            self.hvac.cooling_capacities_w = vec![cfg.capacity_w];

            self.hvac.eir_by_stage = vec![cfg.eir];

            self.hvac.airflow_m3_s_per_w = cfg
                .airflow_m3_s_per_w
                .unwrap_or(super::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W);
            self.hvac.plf_cooling_degradation_coeff = 0.22;
            self.hvac.startup.c_d = 0.22;
        } else {
            let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
            cfg.validate()?;

            self.hvac.cooling_capacities_w = if let Some(stages) = &cfg.stage_capacities_w {
                stages.clone()
            } else {
                vec![cfg.capacity_w]
            };

            let default_eir = cfg.eir;
            let stage_count = self.hvac.cooling_capacities_w.len();

            self.hvac.eir_by_stage = if let Some(stages) = &cfg.stage_eirs {
                stages.clone()
            } else {
                vec![default_eir; stage_count]
            };

            if self.hvac.cooling_capacities_w.len() != self.hvac.eir_by_stage.len() {
                return Err(HaresError::Equipment(
                    "cooling capacity and EIR stage counts must match".to_string(),
                ));
            }

            if let Some(airflow_m3_s_per_w) = cfg.airflow_m3_s_per_w {
                self.hvac.airflow_m3_s_per_w = airflow_m3_s_per_w;
            }

            let rated_cap = self
                .hvac
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let fan_flow = self.hvac.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.cooling_capacities_w.len().min(255) as u8;
            let cap_low = (n_speeds > 1)
                .then(|| self.hvac.cooling_capacities_w.first().copied())
                .flatten();
            let flow_low = cap_low.map(|c| self.hvac.airflow_m3_s_per_w * c);
            self.hvac.duct_dse = if let Some(dse) = cfg.duct.dse_cool {
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
            self.hvac.speed_control_mode = speed_mode;

            self.rated_shr = cfg.shr.unwrap_or(0.75).clamp(0.0, 1.0);

            // Per-stage SHR values if provided.
            self.stage_shrs = cfg.stage_shrs.clone().unwrap_or_default();
            if !self.stage_shrs.is_empty() {
                self.rated_shr = self.stage_shrs[0].clamp(0.0, 1.0);
            }

            self.apply_cooling_startup_cd(cfg.derived_cooling_startup_cd());

            // EnergyPlus residential DX coil defaults (Engineering Reference §16.5).
            self.latent_degradation = LatentDegradationParams {
                twet_rated_s: 1500.0,
                gamma_rated: 1.5,
                max_cycling_rate: 3.0,
                latent_time_constant_s: 45.0,
            };
        }

        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports);
        self.hvac.biquadratic_coeffs = load_curve_pair(config, self.is_room_ac)?;
        self.compute_coil_ao(self.rated_shr)?;

        let (crankcase_rated_kw, crankcase_threshold_c, crankcase_capacity_curve) =
            if self.is_room_ac {
                let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
                (
                    cfg.crankcase_heater_kw.unwrap_or(CRANKCASE_HEATER_KW),
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
            self.hvac.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }

        let mode = self
            .hvac
            .update_mode(env)
            .unwrap_or(ThermostatMode::Deadband);
        if mode == ThermostatMode::Cooling {
            let base_setpoint = self.hvac.effective_setpoints().cooling_c;
            let setpoint = base_setpoint + self.dr_setpoint_offset_c;
            let zone_temp = lookup_zone(env, self.hvac.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(setpoint);

            // When DR raises the effective setpoint above the zone temperature, suppress cooling
            // even though the base thermostat is calling for it. This only applies when the DR
            // offset is active (non-zero) and the zone has cooled below the DR-adjusted threshold.
            let dr_suppressed =
                self.dr_setpoint_offset_c > 0.0 && zone_temp <= setpoint;
            if dr_suppressed {
                self.hvac.duty_cycle = 0.0;
                self.operating_mode = OperatingMode::Off;
                self.hvac.update_prev_zone_temp(None);
            } else {
                let deadband = self
                    .hvac
                    .thermostat
                    .hysteresis_c
                    .max(MIN_LOAD_FRACTION_DEADBAND_C);
                let load_fraction = if self.hvac.speed_control_mode == SpeedControlMode::SingleSpeed
                {
                    1.0
                } else {
                    ((zone_temp - setpoint) / deadband).clamp(0.0, 1.0)
                };
                self.hvac.update_prev_zone_temp(Some(zone_temp));
                self.hvac.duty_cycle = match self.hvac.speed_control_mode {
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
                self.operating_mode = if self.hvac.duty_cycle > 0.0 {
                    OperatingMode::Cooling
                } else {
                    OperatingMode::Off
                };
            }
        } else {
            self.hvac.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
        }
        self.operating_mode
    }

    /// Run one simulation step.
    ///
    /// `companion_heating_rtf` — if `Some`, this AC is the cooling side of a
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
            self.hvac.shr = perf.shr;
            self.hvac.supply_air_temp_c = perf.supply_temp_c;

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
            let fan_heat_w = fan_kw * 1000.0;
            self.hvac.write_zone_thermal_contributions(
                ports,
                -sensible_cooling_w + fan_heat_w,
                -latent_cooling_w,
                ThermalCategory::HvacCooling,
            )?;

            self.hvac.advance_speed_timer(dt.as_secs_f64());
            self.run_time_s += dt.as_secs_f64();
            self.cycle_on_steps += 1;
        } else {
            self.last_cooling_rtf = 0.0;
            self.cycle_off_steps += 1;
            self.hvac.time_at_current_speed_s = 0.0;
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
            (compressor_kw + fan_kw + self.crankcase_heater_kw) * self.hvac.space_fraction;
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        // Telemetry reports delivered (post-DSE) values for the conditioned zone.
        let dse = self.hvac.duct_dse.clamp(0.0, 1.0);
        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry
            .set(tk::SENSIBLE_COOLING_W, sensible_cooling_w * dse);
        self.telemetry
            .set(tk::LATENT_COOLING_W, latent_cooling_w * dse);
        self.telemetry.set(tk::SHR, self.hvac.shr);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.last_speed_index as f64);
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        let compressor_only_w = compressor_kw * 1000.0;
        let total_cooling_w = sensible_cooling_w + latent_cooling_w;
        let cop = if compressor_only_w > 1e-6 {
            total_cooling_w / compressor_only_w
        } else {
            0.0
        };
        self.telemetry.set(tk::COP, cop);
        self.telemetry
            .set(tk::RUNTIME_FRACTION, self.last_cooling_rtf.clamp(0.0, 1.0));
        self.telemetry.set(tk::COMPRESSOR_KW, compressor_kw);
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry
            .set(tk::SUPPLY_TEMP_C, self.hvac.supply_air_temp_c);
        self.telemetry
            .set(tk::APPARATUS_DEW_POINT_C, self.last_adp_c);
        self.telemetry
            .set(tk::BYPASS_FACTOR, self.last_bypass_factor);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
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
        let zone = lookup_zone(env, self.hvac.zone_id)?;
        if !zone.wet_bulb_c.is_finite() {
            return Err(HaresError::Equipment(format!(
                "zone {:?} wet_bulb_c must be finite before HVAC cooling step",
                self.hvac.zone_id
            )));
        }

        let is_variable_speed =
            self.hvac.speed_control_mode == SpeedControlMode::VariableSpeedIdeal;
        let mut speed_index = self.hvac.last_speed_index;
        let speed_frac = self.hvac.last_speed_frac;
        let mut variable_selection = SpeedSelection {
            speed_index,
            speed_frac,
            part_load_ratio: self.hvac.duty_cycle.clamp(0.0, 1.0),
        };
        let (mut stage_cap_w, mut stage_eir, mut variable_plr) = match self.hvac.speed_control_mode
        {
            SpeedControlMode::VariableSpeedIdeal => self.variable_speed_point(variable_selection),
            SpeedControlMode::MultiSpeedInterpolated => (
                self.hvac.interpolated_capacity(
                    &self.hvac.cooling_capacities_w,
                    speed_index,
                    speed_frac,
                ),
                self.hvac.interpolated_eir(speed_index, speed_frac),
                self.hvac.duty_cycle.clamp(0.0, 1.0),
            ),
            _ => (
                HvacEquipment::capacity_at_stage(&self.hvac.cooling_capacities_w, speed_index),
                self.hvac.eir_at_stage(speed_index),
                self.hvac.duty_cycle.clamp(0.0, 1.0),
            ),
        };

        let outdoor_c = env.weather.outdoor_temp_c;

        fn curve_inputs(
            stage_capacity_w: f64,
            hvac: &HvacEquipment,
            zone: &hares_types::ZoneState,
            env: &EnvironmentState,
            outdoor_c: f64,
            flow_fraction_correction: f64,
        ) -> (f64, f64, f64, f64, f64) {
            let flow_m3_s_for_fan = stage_capacity_w.max(0.0) * hvac.airflow_m3_s_per_w;
            let fan_shaft_heat_correction_c = if flow_m3_s_for_fan > 0.0 {
                use hares_physics::{
                    air_properties::moist_air_density_kg_m3,
                    psychrometrics::SPECIFIC_HEAT_DRY_AIR_KJ_KG_K,
                };
                let fan_power_w = hvac.fan_power_w_per_m3_s * flow_m3_s_for_fan;
                let rho = moist_air_density_kg_m3(
                    env.weather.pressure_kpa * 1000.0,
                    zone.temperature_c,
                    zone.humidity_ratio.max(0.0),
                );
                let mfr = flow_m3_s_for_fan * rho;
                if mfr > 0.0 {
                    (fan_power_w / 1000.0) / (mfr * SPECIFIC_HEAT_DRY_AIR_KJ_KG_K)
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
                0,
                coil_entering_wb_c,
                outdoor_c,
                flow_fraction_correction,
            );
            let (_, eir_ratio_base) = hvac.evaluate_biquadratic_with_flow(
                1,
                coil_entering_wb_c,
                outdoor_c,
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
            &self.hvac,
            zone,
            env,
            outdoor_c,
            self.flow_fraction_correction,
        );

        if is_variable_speed && self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            let max_capacity_w = self
                .hvac
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or_default();
            let requested_capacity_fraction =
                (-self.ideal_capacity_w / (max_capacity_w * cap_ratio).max(1.0)).clamp(0.0, 1.0);
            variable_selection = self.select_variable_speed_cooling(requested_capacity_fraction);
            speed_index = variable_selection.speed_index;
            self.hvac.duty_cycle = variable_selection.part_load_ratio;
            (stage_cap_w, stage_eir, variable_plr) = self.variable_speed_point(variable_selection);
            (
                flow_m3_s_for_fan,
                coil_entering_db_c,
                coil_entering_wb_c,
                cap_ratio,
                eir_ratio_base,
            ) = curve_inputs(
                stage_cap_w,
                &self.hvac,
                zone,
                env,
                outdoor_c,
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
            self.hvac.duty_cycle = p;
            p
        } else {
            self.hvac.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = if is_variable_speed {
            1.0
        } else {
            self.hvac.part_load_factor(plr)
        };

        // EIR curve: divide by PLF — a lower PLF (more cycling) means worse efficiency.
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let staged_capacity_w = self
            .hvac
            .apply_startup_capacity_degradation(steady_capacity_w, dt_min);
        let total_capacity_w = (staged_capacity_w * plr).max(0.0);

        let flow_m3_s = flow_m3_s_for_fan;

        let ao = self.ao_for_speed(speed_index);
        let CoilResult {
            shr,
            supply_temp_c,
            adp_temp_c,
            bypass_factor,
        } = calculate_shr(
            coil_entering_db_c,
            zone.humidity_ratio,
            env.weather.pressure_kpa,
            (total_capacity_w / 1000.0).max(0.0),
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
                .cooling_capacities_w
                .get(speed_index)
                .copied()
                .unwrap_or_else(|| {
                    self.hvac
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

        self.hvac.shr = shr;

        // Return gross (pre-DSE) sensible/latent. zone_heat_fractions (set from
        // duct_dse during init) distributes gross output to conditioned and duct zones.
        let (sensible_cooling_w, latent_cooling_w) =
            self.hvac.sensible_latent_from_shr(total_capacity_w);

        let compressor_kw = (total_capacity_w * stage_eir * eir_ratio).max(0.0) / 1000.0;
        let fan_kw = self.hvac.fan_power_w(flow_m3_s) * plr / 1000.0;

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
            &self.hvac.cooling_capacities_w,
            self.hvac.airflow_m3_s_per_w,
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
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            mode_start_at: self.hvac.mode_start_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            cycle_on_steps: self.cycle_on_steps,
            cycle_off_steps: self.cycle_off_steps,
            crankcase_heater_on: self.crankcase_heater_on,
            startup_c_d: self.hvac.startup.c_d,
            startup_time_since_start_min: self.hvac.startup.time_since_start_min,
            plf_state: self.hvac.plf_state,
            last_speed_index: self.hvac.last_speed_index,
            last_speed_frac: self.hvac.last_speed_frac,
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            sensible_cooling_w: self.telemetry.get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0),
            latent_cooling_w: self.telemetry.get(tk::LATENT_COOLING_W).unwrap_or(0.0),
            shr: self.telemetry.get(tk::SHR).unwrap_or(self.hvac.shr),
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
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: AirConditionerState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.mode_start_at = decoded.mode_start_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.cycle_on_steps = decoded.cycle_on_steps;
        self.cycle_off_steps = decoded.cycle_off_steps;
        self.crankcase_heater_on = decoded.crankcase_heater_on;
        self.hvac.startup.c_d = decoded.startup_c_d;
        self.hvac.startup.time_since_start_min = decoded.startup_time_since_start_min;
        self.hvac.plf_state = decoded.plf_state;
        self.hvac.last_speed_index = decoded.last_speed_index;
        self.hvac.last_speed_frac = decoded.last_speed_frac;
        self.hvac.shr = decoded.shr;
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

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::SENSIBLE_COOLING_W, decoded.sensible_cooling_w);
        self.telemetry
            .insert(tk::LATENT_COOLING_W, decoded.latent_cooling_w);
        self.telemetry.insert(tk::SHR, decoded.shr);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode_code);
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
                    self.hvac.thermostat.hysteresis_c = *db;
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
            _ => {}
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
        Some((self.hvac.zone_id, setpoint))
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
            cooling_setpoint_c: Some(24.0),
            heating_setpoint_c: Some(18.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
        },
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, ExecutionStage, GridState, OperatingMode, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::{AirConditioner, RoomAC, SpeedControlMode};

    use crate::{
        CentralAirConditionerConfig, Equipment, EquipmentConfig, EquipmentRegistry, RoomAcConfig,
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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
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
            eq.core.hvac.speed_control_mode,
            super::SpeedControlMode::SingleSpeed
        );
        assert_eq!(eq.core.hvac.duct_dse, 1.0);
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
            ..PortSlots::default()
        };
        let env = env(22.0, 0.008, 15.0, 5.0);
        eq.init(&cfg, &env).unwrap();

        eq.update_control(&env);
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.net_active_kw() - 0.05).abs() < 1e-9);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);
    }

    #[test]
    fn sensible_and_latent_sum_to_total_cooling() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
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
        let fan_heat_w = fan_kw * 1000.0;

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
            ..PortSlots::default()
        };
        let mut ports_high = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
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
            eq.core.hvac.equipment_type,
            crate::hvac::hvac_core::HvacEquipmentType::AcCooler,
        );
        // Tolerance 1e-9: uom ft³ constant (0.02831685) differs from NIST-exact
        // (0.028316846592) by ~3 ppm; implementation uses the NIST constant.
        assert!(
            (eq.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "Central AC default airflow mismatch, got {} m3/s/W",
            eq.core.hvac.airflow_m3_s_per_w
        );

        let environment = env(27.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).expect("init must succeed");
        assert!(
            (eq.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "init() must preserve default airflow ratio, got {} m3/s/W",
            eq.core.hvac.airflow_m3_s_per_w
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
            (eq.core.hvac.duct_dse - 1.0).abs() < f64::EPSILON,
            "Room AC duct_dse must be 1.0, got {}",
            eq.core.hvac.duct_dse
        );
        assert!(
            eq.core.hvac.duct_zone_id.is_none(),
            "Room AC must have no duct zone"
        );
    }

    #[test]
    fn room_ac_step_produces_cooling() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
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

        let fracs = &eq.core.hvac.zone_heat_fractions;
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
        let actual = eq.core.hvac.eir_by_stage[0];
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
            eq.core.hvac.eir_by_stage,
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
            (eq.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "RoomAC airflow must equal the typed SI default after init(), got {} m3/s/W",
            eq.core.hvac.airflow_m3_s_per_w
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
        let duty_full = eq_full.core.hvac.duty_cycle;

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        eq_part.update_control(&env_part);
        let duty_part = eq_part.core.hvac.duty_cycle;

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
            eq_part.core.hvac.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        eq_part.update_control(&env_part);
        assert!(
            (eq_part.core.hvac.duty_cycle - 0.5).abs() < 1e-9,
            "variable-speed ideal should preserve fractional duty cycle; got {}",
            eq_part.core.hvac.duty_cycle
        );

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        eq_full.update_control(&env_full);
        assert!(
            (eq_full.core.hvac.duty_cycle - 1.0).abs() < 1e-9,
            "high load should clamp to full duty cycle; got {}",
            eq_full.core.hvac.duty_cycle
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
            eq.core.hvac.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(
            eq.core.hvac.startup.c_d, 0.0,
            "OCHRE 4-speed variable cooling should map to zero startup Cd"
        );

        eq.update_control(&env);
        assert!(
            (eq.core.hvac.duty_cycle - 0.5).abs() < 1e-9,
            "central 4-speed variable cooling should preserve fractional duty; got {}",
            eq.core.hvac.duty_cycle
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
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        assert_eq!(eq.core.hvac.last_speed_index, 0);
        assert!((eq.core.hvac.last_speed_frac - 1.0).abs() < 1e-9);
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
            eq.core.hvac.speed_control_mode,
            SpeedControlMode::TwoSpeedSetpoint
        );
        assert!(
            (eq.core.hvac.startup.c_d - 0.11).abs() < 1e-9,
            "two-speed typed cooling must derive startup Cd=0.11, got {}",
            eq.core.hvac.startup.c_d
        );
        assert!(
            (eq.core.hvac.plf_cooling_degradation_coeff - 0.11).abs() < 1e-9,
            "two-speed typed cooling must derive PLF Cd=0.11, got {}",
            eq.core.hvac.plf_cooling_degradation_coeff
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
            capacity_w: -4_000.0,
        })
        .unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();

        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        assert_eq!(eq.core.hvac.last_speed_index, 1);
        assert!(
            compressor_kw > 0.0,
            "IdealCapacity=-4 kW on a 4-speed ladder must energize the second stage, got compressor_kw={compressor_kw}"
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
            (ports.electrical.net_active_kw() - electric_kw).abs() < 1e-9,
            "electrical port and telemetry must match: ports={} telemetry={electric_kw}",
            ports.electrical.net_active_kw()
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
            eq.core.hvac.last_speed_index as f64,
            "SPEED_INDEX telemetry must match hvac.last_speed_index"
        );
    }
}

#[cfg(test)]
mod dr_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig};

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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(0.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
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
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
        ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig};

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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
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
                cooling_setpoint_c: Some(26.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(0.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
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
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let port_kw = ports.electrical.net_active_kw();
        // Crankcase (0.10 kW) must appear in both telemetry and port contribution.
        assert!(
            (electric_kw - 0.10).abs() < 1e-9,
            "telemetry electric_kw must equal crankcase rated 0.10 kW, got {electric_kw}"
        );
        assert!(
            (port_kw - 0.10).abs() < 1e-9,
            "port electrical draw must equal crankcase rated 0.10 kW, got {port_kw}"
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
        ControlSignal, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{Equipment, EquipmentConfig};

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
}

#[cfg(test)]
mod defaults_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{AirConditioner, RoomAC};
    use crate::{CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, RoomAcConfig};

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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
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
            (eq.core.hvac.low_speed_capacity_fraction - 0.72).abs() < 1e-12,
            "2-speed AC must default to low_speed_capacity_fraction=0.72 (OCHRE/AHRI default), got {}",
            eq.core.hvac.low_speed_capacity_fraction
        );
    }

    // Single-stage AC initialised without explicit latent degradation params must
    // have the Henderson-Rengarajan model active with EnergyPlus residential DX
    // defaults (twet=1500 s, gamma=1.5, Nmax=3 cyc/hr, tau=45 s).
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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
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
            (eq.core.latent_degradation.twet_rated_s - 1500.0).abs() < 1e-9,
            "twet_rated_s must be 1500 s (EnergyPlus residential default), got {}",
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
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                airflow_m3_s_per_w: None,
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
