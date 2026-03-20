//! Central and room air conditioner models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hares_physics::psychrometrics::humidity_ratio_from_twb;
use hares_types::{
    ControlCapabilities, ControlSignal, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, Telemetry, TelemetryField, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::parse_trimmed_f64;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::coil_physics::{
    CoilResult, LatentDegradationParams, calculate_shr, coil_ao_factor,
    effective_shr_with_latent_degradation,
};
use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    SpeedControlMode,
    helpers::{
        equipment_id_from_config, first_f64, load_stage_values, lookup_zone,
        operating_mode_code, zone_id_from_config,
    },
};
use super::hvac_core::parse_biquadratic_list;

const CRANKCASE_HEATER_KW: f64 = 0.05;
const CRANKCASE_HEATER_THRESHOLD_C: f64 = 12.8;
const MIN_LOAD_FRACTION_DEADBAND_C: f64 = 0.5;
const DEFAULT_CENTRAL_AC_CAPACITY_W: f64 = 12_000.0;
const DEFAULT_ROOM_AC_CAPACITY_W: f64 = 3_500.0;
const DEFAULT_EIR_FALLBACK: f64 = 0.35;
const BTU_PER_HR_PER_W: f64 = 3.412_141_633;

/// Henderson-Rengarajan latent degradation defaults (EnergyPlus/ASHRAE RP-1120).
const DEFAULT_TWET_RATED_S: f64 = 1000.0;
const DEFAULT_GAMMA_RATED: f64 = 1.5;
const DEFAULT_MAX_CYCLING_RATE: f64 = 3.0;
const DEFAULT_LATENT_TIME_CONSTANT_S: f64 = 45.0;

const AHRI_RATED_INDOOR_DB_C: f64 = 26.666_666_666_7;
const AHRI_RATED_INDOOR_WB_C: f64 = 19.444_444_444_4;
const AHRI_RATED_OUTDOOR_DB_C: f64 = 35.0;
const RATED_PRESSURE_KPA: f64 = 101.3;

const DEFAULT_AC_CAPACITY_CURVE: [f64; 6] = [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043];
const DEFAULT_AC_EIR_CURVE: [f64; 6] = [-0.30428, 0.11805, -0.00342, -0.00626, 0.0007, -0.00047];
const DEFAULT_ROOM_AC_CAPACITY_CURVE: [f64; 6] =
    [0.6405, 0.01568, 0.0004531, 0.001615, -0.0001825, 0.00006614];
const DEFAULT_ROOM_AC_EIR_CURVE: [f64; 6] =
    [2.287, -0.1732, 0.004745, 0.01662, 0.000484, -0.001306];

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
    /// Henderson-Rengarajan latent degradation model parameters.
    /// All four fields must be > 0 (checked via `is_active()`) to enable the model.
    latent_degradation: LatentDegradationParams,

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
    last_mode_switch_at: Option<DateTime<Utc>>,
    mode_start_at: Option<DateTime<Utc>>,
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
        self.core
            .crankcase_heater_power_internal(outdoor_temp_c, cooling_rtf, companion_heating_rtf)
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
        if config.get_f64("crankcase_heater_kw").is_none() {
            self.core.crankcase_rated_kw = rated_kw;
        }
        if config.get_f64("crankcase_heater_threshold_c").is_none() {
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
                end_use: EndUse::HvacCooling,
                equipment_type: Cow::Borrowed(equipment_type),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE,
                telemetry_fields: telemetry_fields(),
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_telemetry(),
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
            latent_degradation: LatentDegradationParams::default(),
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
            // Room AC Cd = 0.22 (AHRI test data for window/through-wall units).
            // Only apply when no explicit Cd key is present in config.
            let explicit_cd = config
                .get_f64("startup_cd")
                .or_else(|| config.get_f64("cooling_cd"))
                .or_else(|| config.get_f64("cd"));
            if explicit_cd.is_none() {
                self.hvac.plf_cooling_degradation_coeff = 0.22;
                self.hvac.startup.c_d = 0.22;
            }
        } else {
            // DSE computed below after capacity is resolved.
            self.hvac.duct_zone_id =
                super::helpers::parse_zone_id_key(config, "duct_zone_id");
        }

        self.flow_fraction_correction = first_f64(
            config,
            &["flow_fraction_correction", "airflow_fraction_correction"],
        )
        .unwrap_or(1.0);

        self.hvac.cooling_capacities_w = load_stage_values(
            config,
            &[
                "capacity_w",
                "cooling_capacity_w",
                "capacity",
                "HVAC Cooling Capacity (W)",
            ],
            "cooling_capacity_w_stage",
            if self.is_room_ac {
                DEFAULT_ROOM_AC_CAPACITY_W
            } else {
                DEFAULT_CENTRAL_AC_CAPACITY_W
            },
        );
        if self.is_room_ac && self.hvac.cooling_capacities_w.len() > 1 {
            return Err(HaresError::Equipment(
                "Room AC cannot define multiple cooling stages".to_string(),
            ));
        }

        let default_eir =
            first_f64(config, &["seer", "SEER"]).map_or(DEFAULT_EIR_FALLBACK, |seer| {
                if seer.is_finite() && seer > 0.0 {
                    BTU_PER_HR_PER_W / seer
                } else {
                    DEFAULT_EIR_FALLBACK
                }
            });
        self.hvac.eir_by_stage = load_stage_values(
            config,
            &["eir", "cooling_eir", "EIR", "HVAC Cooling EIR (-)"],
            "cooling_eir_stage",
            default_eir,
        );

        if self.hvac.cooling_capacities_w.len() != self.hvac.eir_by_stage.len() {
            if self.hvac.eir_by_stage.len() == 1 {
                self.hvac.eir_by_stage =
                    vec![self.hvac.eir_by_stage[0]; self.hvac.cooling_capacities_w.len()];
            } else {
                return Err(HaresError::Equipment(
                    "cooling capacity and EIR stage counts must match".to_string(),
                ));
            }
        }

        // Resolve DSE now that cooling capacity is known.
        if !self.is_room_ac {
            let rated_cap = self.hvac.cooling_capacities_w.last().copied().unwrap_or(0.0);
            let fan_flow = self.hvac.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.cooling_capacities_w.len().min(255) as u8;
            self.hvac.duct_dse = super::helpers::resolve_duct_dse(
                config, false, rated_cap, fan_flow, n_speeds, false,
            );
        }
        self.hvac.update_zone_heat_fractions();

        self.hvac.biquadratic_coeffs = load_curve_pair(config, self.is_room_ac)?;
        // Rated SHR at the AHRI test point, used only for coil Ao initialisation.
        // Defaults to 0.75 — ASHRAE Handbook HVAC Systems and Equipment Ch. 42 typical value.
        let rated_shr = first_f64(config, &["rated_shr", "shr", "SHR", "shr_rated"]).unwrap_or(0.75);
        self.rated_shr = rated_shr.clamp(0.0, 1.0);
        self.compute_coil_ao_by_stage(rated_shr)?;

        self.crankcase_rated_kw =
            first_f64(config, &["crankcase_heater_kw"]).unwrap_or(CRANKCASE_HEATER_KW);
        self.crankcase_threshold_c = first_f64(config, &["crankcase_heater_threshold_c"])
            .unwrap_or(CRANKCASE_HEATER_THRESHOLD_C);
        self.crankcase_capacity_curve =
            parse_crankcase_capacity_curve(config.get_str("crankcase_capacity_curve_coeffs"))?;

        // Latent degradation defaults moved to module scope.

        self.latent_degradation = LatentDegradationParams {
            twet_rated_s: first_f64(config, &["twet_rated_s", "latent_twet_rated_s"])
                .unwrap_or(DEFAULT_TWET_RATED_S),
            gamma_rated: first_f64(config, &["gamma_rated", "latent_gamma_rated"])
                .unwrap_or(DEFAULT_GAMMA_RATED),
            max_cycling_rate: first_f64(config, &["max_cycling_rate", "latent_max_cycling_rate"])
                .unwrap_or(DEFAULT_MAX_CYCLING_RATE),
            latent_time_constant_s: first_f64(
                config,
                &["latent_time_constant_s", "latent_capacity_time_constant_s"],
            )
            .unwrap_or(DEFAULT_LATENT_TIME_CONSTANT_S),
        };

        // fan_power_w_per_cfm → fan_power_w_per_m3_s conversion handled by hvac.init()

        // fan_power_w_per_cfm → fan_power_w_per_m3_s conversion handled by hvac.init()

        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.cycle_on_steps = 0;
        self.cycle_off_steps = 0;
        self.crankcase_heater_on = false;
        self.crankcase_heater_kw = 0.0;
        self.last_cooling_rtf = 0.0;
        self.telemetry = default_telemetry();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        // Reset transient signals each step.
        self.ctrl_load_fraction = 1.0;

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
            // Apply DR setpoint offset: positive offset raises cooling setpoint → less demand.
            let setpoint = self.hvac.effective_setpoints().cooling_c + self.dr_setpoint_offset_c;
            let zone_temp = lookup_zone(env, self.hvac.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(setpoint);
            let deadband = self
                .hvac
                .thermostat
                .hysteresis_c
                .max(MIN_LOAD_FRACTION_DEADBAND_C);
            let load_fraction = ((zone_temp - setpoint) / deadband).clamp(0.0, 1.0);
            let selection = self.hvac.select_speed(load_fraction);
            self.hvac.duty_cycle = match self.hvac.speed_control_mode {
                SpeedControlMode::VariableSpeedIdeal => {
                    if selection.speed_frac > 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => selection.part_load_ratio,
            };
            self.operating_mode = if self.hvac.duty_cycle > 0.0 {
                OperatingMode::Cooling
            } else {
                OperatingMode::Off
            };
        } else {
            self.hvac.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
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
            self.last_cooling_rtf = perf.rtf;

            // Apply compound load multipliers (duty cycle * transient load * DR).
            let effective_load = (self.ctrl_duty_cycle
                * self.ctrl_load_fraction
                * self.dr_load_fraction
                * self.dr_duty_cycle)
                .clamp(0.0, 1.0);

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

            self.hvac.write_zone_thermal_contributions(
                ports,
                -sensible_cooling_w,
                -latent_cooling_w,
            )?;

            self.hvac.advance_speed_timer(dt.as_secs_f64());
            self.run_time_s += dt.as_secs_f64();
            self.cycle_on_steps += 1;
        } else {
            self.last_cooling_rtf = 0.0;
            self.cycle_off_steps += 1;
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

        let electric_kw = (compressor_kw + fan_kw + self.crankcase_heater_kw)
            * self.hvac.space_fraction;
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        // Telemetry reports delivered (post-DSE) values for the conditioned zone.
        let dse = self.hvac.duct_dse.clamp(0.0, 1.0);
        self.telemetry.set("electric_kw", electric_kw);
        self.telemetry.set("sensible_cooling_w", sensible_cooling_w * dse);
        self.telemetry.set("latent_cooling_w", latent_cooling_w * dse);
        self.telemetry.set("shr", self.hvac.shr);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        let compressor_only_w = compressor_kw * 1000.0;
        let total_cooling_w = sensible_cooling_w + latent_cooling_w;
        let cop = if compressor_only_w > 1e-6 {
            total_cooling_w / compressor_only_w
        } else {
            0.0
        };
        self.telemetry.set("cop", cop);
        self.telemetry
            .set("runtime_fraction", self.last_cooling_rtf.clamp(0.0, 1.0));

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

        let speed_index = self.hvac.last_speed_index;
        let speed_frac = self.hvac.last_speed_frac;
        let (stage_cap_w, stage_eir) = match self.hvac.speed_control_mode {
            SpeedControlMode::VariableSpeedIdeal => {
                let max_cap = self
                    .hvac
                    .cooling_capacities_w
                    .last()
                    .copied()
                    .unwrap_or_default();
                (
                    max_cap * speed_frac.clamp(0.0, 1.0),
                    self.hvac.eir_at_stage(speed_index),
                )
            }
            SpeedControlMode::MultiSpeedInterpolated => (
                self.hvac.interpolated_capacity(
                    &self.hvac.cooling_capacities_w,
                    speed_index,
                    speed_frac,
                ),
                self.hvac.interpolated_eir(speed_index, speed_frac),
            ),
            _ => (
                HvacEquipment::capacity_at_stage(&self.hvac.cooling_capacities_w, speed_index),
                self.hvac.eir_at_stage(speed_index),
            ),
        };

        let plr = if self.hvac.speed_control_mode == SpeedControlMode::VariableSpeedIdeal {
            1.0
        } else {
            self.hvac.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = if self.hvac.speed_control_mode == SpeedControlMode::VariableSpeedIdeal {
            1.0
        } else {
            self.hvac.part_load_factor(plr)
        };

        // Fan shaft heat raises entering dry-bulb temperature seen by the coil.
        // OCHRE: coil_input_db += fan_power_per_flow_rate / 1000 / rho_air / cp_air
        // Applied as a first-order correction to the indoor wet-bulb temperature
        // used as the biquadratic x1 input (same ΔT in °C).
        let flow_m3_s_for_fan = stage_cap_w.max(0.0) * self.hvac.airflow_m3_s_per_w;
        let fan_shaft_heat_correction_c = if flow_m3_s_for_fan > 0.0 {
            use hares_physics::{
                air_properties::moist_air_density_kg_m3,
                psychrometrics::SPECIFIC_HEAT_DRY_AIR_KJ_KG_K,
            };
            let fan_power_w = self.hvac.fan_power_w_per_m3_s * flow_m3_s_for_fan;
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
        let coil_entering_wb_c = zone.wet_bulb_c + fan_shaft_heat_correction_c;

        let outdoor_c = env.weather.outdoor_temp_c;
        // Capacity curve: PLF does not apply (PLF only penalises EIR).
        let (_, cap_ratio) = self.hvac.evaluate_biquadratic_with_flow(
            0,
            coil_entering_wb_c,
            outdoor_c,
            self.flow_fraction_correction,
        );
        // EIR curve: divide by PLF — a lower PLF (more cycling) means worse efficiency.
        let (_, eir_ratio_base) = self.hvac.evaluate_biquadratic_with_flow(
            1,
            coil_entering_wb_c,
            outdoor_c,
            self.flow_fraction_correction,
        );
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let steady_capacity_w = (stage_cap_w * cap_ratio).max(0.0);
        let staged_capacity_w = self
            .hvac
            .apply_startup_capacity_degradation(steady_capacity_w, dt_min);
        let total_capacity_w = (staged_capacity_w * plr).max(0.0);

        let flow_m3_s = flow_m3_s_for_fan;

        let ao = self.ao_for_speed(speed_index);
        let CoilResult {
            shr, supply_temp_c, ..
        } = calculate_shr(
            zone.temperature_c,
            zone.humidity_ratio,
            env.weather.pressure_kpa,
            (total_capacity_w / 1000.0).max(0.0),
            flow_m3_s,
            ao,
        )?;
        let steady_state_shr = shr.clamp(0.0, 1.0);

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

    /// Compute coil Ao factors per speed stage using the rated SHR at the AHRI
    /// 80°F/67°F indoor / 95°F outdoor test point.
    ///
    /// `rated_shr` must come from configuration, not from `self.hvac.shr` which
    /// is the runtime SHR and defaults to 1.0 at construction.
    fn compute_coil_ao_by_stage(&mut self, rated_shr: f64) -> crate::Result<()> {
        let rated_w = humidity_ratio_from_twb(
            AHRI_RATED_INDOOR_DB_C,
            AHRI_RATED_INDOOR_WB_C,
            RATED_PRESSURE_KPA * 1000.0,
        );

        let mut ao = Vec::with_capacity(self.hvac.cooling_capacities_w.len().max(1));
        for (idx, cap_w) in self.hvac.cooling_capacities_w.iter().copied().enumerate() {
            let flow_m3_s = cap_w.max(0.0) * self.hvac.airflow_m3_s_per_w;
            let shr = rated_shr.clamp(0.0, 1.0);
            let ao_i = coil_ao_factor(
                AHRI_RATED_INDOOR_DB_C,
                rated_w,
                RATED_PRESSURE_KPA,
                (cap_w / 1000.0).max(0.0),
                flow_m3_s,
                shr,
            )
            .map_err(|err| {
                HaresError::Equipment(format!(
                    "failed to compute coil Ao for stage {} at {} C ambient: {}",
                    idx + 1,
                    AHRI_RATED_OUTDOOR_DB_C,
                    err
                ))
            })?;
            ao.push(ao_i);
        }
        if ao.is_empty() {
            ao.push(10.0);
        }
        self.coil_ao_by_stage = ao;
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
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            sensible_cooling_w: self.telemetry.get("sensible_cooling_w").unwrap_or(0.0),
            latent_cooling_w: self.telemetry.get("latent_cooling_w").unwrap_or(0.0),
            shr: self.telemetry.get("shr").unwrap_or(self.hvac.shr),
            operating_mode_code: self.telemetry.get("operating_mode").unwrap_or(0.0),
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

        self.telemetry.insert("electric_kw", decoded.electric_kw);
        self.telemetry
            .insert("sensible_cooling_w", decoded.sensible_cooling_w);
        self.telemetry
            .insert("latent_cooling_w", decoded.latent_cooling_w);
        self.telemetry.insert("shr", decoded.shr);
        self.telemetry
            .insert("operating_mode", decoded.operating_mode_code);

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
            _ => {}
        }
        Ok(())
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

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(7);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("sensible_cooling_w", 0.0);
    telemetry.insert("latent_cooling_w", 0.0);
    telemetry.insert("shr", 1.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry.insert("cop", 0.0);
    telemetry.insert("runtime_fraction", 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Total cooling electric power: compressor + fan + crankcase".to_string(),
        },
        TelemetryField {
            name: "sensible_cooling_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible cooling magnitude".to_string(),
        },
        TelemetryField {
            name: "latent_cooling_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered latent cooling magnitude".to_string(),
        },
        TelemetryField {
            name: "shr".to_string(),
            unit: "-".to_string(),
            description: "Sensible heat ratio".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 2=Cooling".to_string(),
        },
    ]
}

fn load_curve_pair(config: &EquipmentConfig, is_room_ac: bool) -> crate::Result<Vec<[f64; 6]>> {
    let mut curves = if let Some(raw) = config.get_str("biquadratic_coeffs") {
        parse_biquadratic_list(raw)?
    } else {
        Vec::new()
    };

    if let Some(cap) = parse_single_coeff_array(config.get_str("capacity_biquadratic_coeffs"))? {
        if curves.is_empty() {
            curves.push(cap);
        } else {
            curves[0] = cap;
        }
    }

    if let Some(eir) = parse_single_coeff_array(config.get_str("eir_biquadratic_coeffs"))? {
        if curves.len() < 2 {
            curves.resize(2, eir);
        }
        curves[1] = eir;
    }

    if curves.is_empty() {
        curves.push(if is_room_ac {
            DEFAULT_ROOM_AC_CAPACITY_CURVE
        } else {
            DEFAULT_AC_CAPACITY_CURVE
        });
        curves.push(if is_room_ac {
            DEFAULT_ROOM_AC_EIR_CURVE
        } else {
            DEFAULT_AC_EIR_CURVE
        });
    } else if curves.len() == 1 {
        tracing::warn!(
            "Only one biquadratic curve provided; duplicating for both capacity and EIR. \
             This is likely incorrect."
        );
        curves.push(curves[0]);
    }

    Ok(curves)
}

fn parse_single_coeff_array(raw: Option<&str>) -> crate::Result<Option<[f64; 6]>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let curves = parse_biquadratic_list(raw)?;
    match curves.len() {
        0 => Ok(None),
        1 => Ok(Some(curves[0])),
        n => Err(HaresError::Equipment(format!(
            "expected exactly 6 biquadratic coefficients, got {}",
            n * 6
        ))),
    }
}

/// Parse optional crankcase heater capacity curve coefficients `[c0, c1, c2]`
/// from a config string such as `"[1.0, -0.02, 0.0]"`.
/// effective_capacity = rated * (c0 + c1*T + c2*T^2), clamped to >= 0.
fn parse_crankcase_capacity_curve(raw: Option<&str>) -> crate::Result<Option<[f64; 3]>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let values: Vec<f64> = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter_map(parse_trimmed_f64)
        .collect();
    if values.len() != 3 {
        return Err(HaresError::Equipment(format!(
            "crankcase_capacity_curve_coeffs must contain exactly 3 values [c0, c1, c2], \
             got {}",
            values.len()
        )));
    }
    Ok(Some([values[0], values[1], values[2]]))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        EnvironmentState, ExecutionStage, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState,
    };

    use super::{AirConditioner, RoomAC, register_with_registry};
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

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
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn ac_config() -> EquipmentConfig {
        let mut raw_config = HashMap::new();
        raw_config.insert("zone_id".to_string(), 1.0.into());
        raw_config.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw_config.insert("eir".to_string(), 0.33.into());
        raw_config.insert("cooling_setpoint_c".to_string(), 24.0.into());
        raw_config.insert("heating_setpoint_c".to_string(), 18.0.into());
        // Explicit airflow so coil bypass-factor init stays stable across
        // equipment-type default changes; airflow defaults are tested in hvac_core.
        raw_config.insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        raw_config.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());

        EquipmentConfig {
            name: "AC".to_string(),
            ochre_class: "Air Conditioner".to_string(),
            raw_config,
        }
    }

    #[test]
    fn air_conditioner_descriptor_contracts_match_ticket() {
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
        assert!(telemetry_names.contains(&"electric_kw"));
        assert!(telemetry_names.contains(&"sensible_cooling_w"));
        assert!(telemetry_names.contains(&"latent_cooling_w"));
        assert!(telemetry_names.contains(&"shr"));
        assert!(telemetry_names.contains(&"operating_mode"));
    }

    #[test]
    fn room_ac_forces_single_speed_and_duct_dse_one() {
        let mut cfg = ac_config();
        cfg.ochre_class = "Room AC".to_string();
        cfg.raw_config
            .insert("speed_control_mode".to_string(), "two_speed".into());

        let mut eq = RoomAC::new(cfg.clone());
        let err = eq
            .init(&cfg, &env(26.0, 0.009, 18.0, 30.0))
            .expect_err("room ac cannot be multi speed");
        assert!(err.to_string().contains("single-speed"));
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
            eq.telemetry().get("operating_mode"),
            Some(0.0),
            "step() must not change operating mode; it must remain Off when \
             update_control() was never called",
        );
        assert_eq!(
            eq.telemetry().get("electric_kw"),
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

        let sens = eq.telemetry().get("sensible_cooling_w").unwrap_or(0.0);
        let lat = eq.telemetry().get("latent_cooling_w").unwrap_or(0.0);
        let sum = sens + lat;
        let total = -(ports.thermal[0].sensible_gain_w + ports.thermal[0].latent_gain_w);
        assert!((sum - total).abs() < 1e-6);
    }

    #[test]
    fn shr_drops_with_higher_humidity_ratio() {
        // This test exercises SHR coil physics; disable the startup ramp (c_d=0)
        // so it does not obscure the result on the first step.
        let mut cfg = ac_config();
        cfg.raw_config.insert("startup_cd".to_string(), 0.0.into());

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

        let shr_low = eq_low.telemetry().get("shr").unwrap_or(1.0);
        let shr_high = eq_high.telemetry().get("shr").unwrap_or(1.0);
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

        assert_eq!(restored.telemetry().get("operating_mode"), Some(2.0));
        assert!(restored.telemetry().get("electric_kw").unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn ac_uses_ac_cooler_equipment_type_with_312_cfm_per_ton_default() {
        use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};
        let cfg = ac_config();
        let eq = AirConditioner::new(cfg.clone());
        assert_eq!(
            eq.core.hvac.equipment_type,
            super::super::hvac_core::HvacEquipmentType::AcCooler,
            "AirConditioner must use AcCooler equipment type"
        );
        // Construction-time airflow default is 312 CFM/ton.
        let expected = 312.0 * CFM_TO_M3_S / W_PER_TON;
        assert!(
            (eq.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-12,
            "AcCooler default airflow must be 312 CFM/ton, got {} m3/s/W",
            eq.core.hvac.airflow_m3_s_per_w
        );

        // init() preserves the 312 default when no explicit airflow configured.
        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment)
            .expect("init must succeed with 312 CFM/ton default");
        assert!(
            (eq.core.hvac.airflow_m3_s_per_w - expected).abs() < 1e-12,
            "init() must preserve 312 CFM/ton default, got {} m3/s/W",
            eq.core.hvac.airflow_m3_s_per_w
        );
    }

    #[test]
    fn registry_registers_ochre_names_with_thermal_stage() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);

        let ac = registry.create("Air Conditioner", ac_config()).unwrap();
        assert_eq!(ac.descriptor().stage, ExecutionStage::Thermal);

        let mut room_cfg = ac_config();
        room_cfg.ochre_class = "Room AC".to_string();
        let room = registry.create("Room AC", room_cfg).unwrap();
        assert_eq!(room.descriptor().stage, ExecutionStage::Thermal);
    }

    /// Two-speed AC selects the high stage when load fraction exceeds
    /// `low_speed_capacity_fraction` (default 0.5), and the low stage otherwise.
    /// With `hysteresis_c=0` the thermostat activates at zone_temp > setpoint (24°C)
    /// and `MIN_LOAD_FRACTION_DEADBAND_C=0.5°C` governs the load fraction:
    ///   zone 24.4°C → load_fraction = 0.4/0.5 = 0.8 > 0.5 → stage 1 (8 000 W)
    ///   zone 24.1°C → load_fraction = 0.1/0.5 = 0.2 ≤ 0.5 → stage 0 (4 000 W)
    /// The test verifies that two-speed stage selection routes to different capacity
    /// stages by observing the resulting electrical draw.
    #[test]
    fn two_speed_ac_draws_more_power_at_high_load_than_moderate_load() {
        let mut cfg = ac_config();
        cfg.raw_config
            .insert("speed_control_mode".to_string(), "two_speed".into());
        // Zero hysteresis so activation threshold equals setpoint, not setpoint+1.
        cfg.raw_config
            .insert("hysteresis_c".to_string(), 0.0.into());
        cfg.raw_config
            .insert("cooling_capacity_w_stage_0".to_string(), 4_000.0.into());
        cfg.raw_config
            .insert("cooling_capacity_w_stage_1".to_string(), 8_000.0.into());
        cfg.raw_config
            .insert("cooling_eir_stage_0".to_string(), 0.33.into());
        cfg.raw_config
            .insert("cooling_eir_stage_1".to_string(), 0.33.into());

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
        let kw_high = eq_high.telemetry().get("electric_kw").unwrap_or(0.0);

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
        let kw_low = eq_low.telemetry().get("electric_kw").unwrap_or(0.0);

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

    /// AHRI 210/240 PLF degradation: at part load (duty_cycle < 1) a single-speed
    /// AC must draw less total power and deliver less cooling than at full load.
    /// With `hysteresis_c=0` and the single-step thermostat logic, load fraction
    /// = (zone - setpoint) / MIN_LOAD_FRACTION_DEADBAND, making it possible to
    /// produce duty < 1 at zones just above setpoint.
    ///   zone 24.4°C → load_fraction = 0.8 → duty_cycle = 0.8 (part load)
    ///   zone 24.6°C → load_fraction = 1.0 (clamped) → duty_cycle = 1.0 (full load)
    /// Electrical draw and cooling delivered must both be strictly greater at full
    /// load than at part load.
    #[test]
    fn part_load_operation_draws_less_power_than_full_load() {
        let mut cfg = ac_config();
        // Zero hysteresis so thermostat activates right at setpoint (24°C).
        cfg.raw_config
            .insert("hysteresis_c".to_string(), 0.0.into());

        // Full load: load_fraction = (24.6-24)/0.5 = 1.2 → clamped to 1.0
        let env_full = env(24.6, 0.010, 18.0, 35.0);
        // Part load: load_fraction = (24.2-24)/0.5 = 0.4 → duty_cycle = 0.4
        let env_part = env(24.2, 0.010, 18.0, 35.0);

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        let mut ports_full = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_full.update_control(&env_full);
        eq_full
            .step(&env_full, Duration::from_secs(60), &mut ports_full)
            .unwrap();
        let kw_full = eq_full.telemetry().get("electric_kw").unwrap_or(0.0);
        let cool_full = eq_full.telemetry().get("sensible_cooling_w").unwrap_or(0.0);

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        let mut ports_part = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_part.update_control(&env_part);
        eq_part
            .step(&env_part, Duration::from_secs(60), &mut ports_part)
            .unwrap();
        let kw_part = eq_part.telemetry().get("electric_kw").unwrap_or(0.0);
        let cool_part = eq_part.telemetry().get("sensible_cooling_w").unwrap_or(0.0);

        assert!(kw_full > 0.0, "full-load AC must draw power, got {kw_full}");
        assert!(kw_part > 0.0, "part-load AC must draw power, got {kw_part}");
        assert!(
            kw_full > kw_part,
            "full-load draw ({kw_full:.4} kW) must exceed part-load draw ({kw_part:.4} kW)"
        );
        assert!(
            cool_full > cool_part,
            "full-load sensible cooling ({cool_full:.1} W) must exceed part-load ({cool_part:.1} W)"
        );
    }
}

#[cfg(test)]
mod dr_tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState,
    };

    use super::AirConditioner;
    use crate::{Equipment, EquipmentConfig};

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
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn hot_env_at_time(zone_temp_c: f64, second: i64) -> EnvironmentState {
        EnvironmentState {
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            ..hot_env(zone_temp_c)
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw.insert("eir".to_string(), 0.33.into());
        raw.insert("cooling_setpoint_c".to_string(), 24.0.into());
        raw.insert("heating_setpoint_c".to_string(), 18.0.into());
        raw.insert("hysteresis_c".to_string(), 0.0.into());
        raw.insert("airflow_cfm_per_ton".to_string(), 375.0.into());
        raw.insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        raw.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        EquipmentConfig {
            name: "AC".to_string(),
            ochre_class: "Air Conditioner".to_string(),
            raw_config: raw,
        }
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
        eq.telemetry().get("electric_kw").unwrap_or(0.0)
    }

    // DR Moderate: cooling setpoint offset = +1°C → unit is less likely to cool at
    // zone_temp just above the original setpoint.
    #[test]
    fn dr_moderate_shifts_cooling_setpoint_up() {
        let cfg = base_config();
        // Zone at 24.3°C: above setpoint (24°C) so AC would normally cool.
        // After DR Moderate offset (+1°C), effective setpoint = 25°C > zone → no cooling.
        let environment = hot_env(24.3);

        // Baseline: without DR, AC should be cooling.
        let mut eq_base = AirConditioner::new(cfg.clone());
        eq_base.init(&cfg, &environment).unwrap();
        let kw_no_dr = step_once(&mut eq_base, &environment);
        assert!(kw_no_dr > 0.0, "AC must be active without DR at 24.3°C");

        // With DR Moderate: effective setpoint raised to 25°C, zone is below → no cooling.
        let mut eq_dr = AirConditioner::new(cfg.clone());
        eq_dr.init(&cfg, &environment).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        let kw_with_dr = step_once(&mut eq_dr, &environment);
        assert_eq!(
            kw_with_dr, 0.0,
            "DR Moderate must suppress cooling by raising effective setpoint above zone temp"
        );
    }

    // DR Critical: load_fraction = 0.5, output must be halved vs the Critical-free run.
    #[test]
    fn dr_critical_reduces_load_fraction() {
        let cfg = base_config();
        // Zone well above setpoint so the AC runs at full output.
        let environment = hot_env(28.0);

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
        let environment = hot_env(28.0);

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
        assert_eq!(kw_during, 0.0, "GridEmergency must shed load while duration active");

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
    // LoadFraction is transient — it must be applied AFTER update_control but BEFORE step,
    // because update_control resets ctrl_load_fraction = 1.0 at its start.
    #[test]
    fn load_fraction_zero_forces_off() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // update_control determines thermostat mode first (sets operating_mode = Cooling).
        eq.update_control(&environment);
        // Then apply LoadFraction=0 after update_control so the reset does not clobber it.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports = make_ports();
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw = eq.telemetry().get("electric_kw").unwrap_or(0.0);
        assert_eq!(kw, 0.0, "LoadFraction 0.0 must force zero output this step");
    }

    // LoadFraction is transient: update_control resets ctrl_load_fraction = 1.0 each step.
    // Applying LoadFraction=0 between update_control and step affects only that step;
    // the next update_control call restores the default.
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
        let kw_step1 = eq.telemetry().get("electric_kw").unwrap_or(0.0);
        assert_eq!(kw_step1, 0.0, "step 1 with LoadFraction=0 must be off");

        // Step 2: update_control resets ctrl_load_fraction=1.0; no signal reapplied → AC runs.
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
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::AirConditioner;
    use crate::{Equipment, EquipmentConfig};

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
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn ac_config() -> EquipmentConfig {
        let mut raw_config = HashMap::new();
        raw_config.insert("zone_id".to_string(), 1.0.into());
        raw_config.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw_config.insert("eir".to_string(), 0.33.into());
        raw_config.insert("cooling_setpoint_c".to_string(), 24.0.into());
        raw_config.insert("heating_setpoint_c".to_string(), 18.0.into());
        raw_config.insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        raw_config.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        EquipmentConfig {
            name: "AC".to_string(),
            ochre_class: "Air Conditioner".to_string(),
            raw_config,
        }
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
            current_time: Utc
                .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("cooling_capacity_w".to_string(), 8_000.0.into());
        raw.insert("eir".to_string(), 0.33.into());
        raw.insert("cooling_setpoint_c".to_string(), 26.0.into());
        raw.insert("heating_setpoint_c".to_string(), 18.0.into());
        raw.insert("airflow_cfm_per_ton".to_string(), 375.0.into());
        raw.insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        raw.insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        raw.insert("crankcase_heater_kw".to_string(), 0.10.into()); // 100 W rated
        raw.insert(
            "crankcase_heater_threshold_c".to_string(),
            12.8_f64.into(),
        );
        EquipmentConfig {
            name: "AC".to_string(),
            ochre_class: "Air Conditioner".to_string(),
            raw_config: raw,
        }
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
        eq.telemetry().get("electric_kw").unwrap_or(0.0)
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
        let mut cfg = base_config();
        // Curve: effective = rated * (1.0 + 0.1*T + 0.0*T^2); at T=5C: multiplier=1.5
        cfg.raw_config.insert(
            "crankcase_capacity_curve_coeffs".to_string(),
            "[1.0, 0.1, 0.0]".into(),
        );
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

        let electric_kw = eq.telemetry().get("electric_kw").unwrap_or(0.0);
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
