//! Shared HVAC types and utilities.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hares_physics::biquadratic::BiquadraticCurve;
use hares_types::{
    ControlSignal, EnvironmentState, HaresError, PortContribution, PortSlots, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

const IDEAL_CAPACITY_TIME_RES_THRESHOLD_S: i64 = 300;
const HEATING_DISABLED_SETPOINT_C: f64 = -999.0;
const COOLING_DISABLED_SETPOINT_C: f64 = 999.0;
const AIRFLOW_DEFAULT_CFM_PER_TON: f64 = 375.0;
const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// HVAC thermostat configuration shared across heating/cooling equipment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermostatConfig {
    pub hysteresis_c: f64,
    pub cutout_ratio: f64,
    pub min_cycle_time_s: f64,
    pub use_ideal_capacity: bool,
}

impl Default for ThermostatConfig {
    fn default() -> Self {
        Self {
            hysteresis_c: 1.0,
            cutout_ratio: 0.0,
            min_cycle_time_s: 0.0,
            use_ideal_capacity: false,
        }
    }
}

impl ThermostatConfig {
    pub fn validate(self, env: &EnvironmentState) -> crate::Result<()> {
        if !(0.0..=1.0).contains(&self.cutout_ratio) {
            return Err(HaresError::Equipment(format!(
                "cutout_ratio must be in [0.0, 1.0], got {}",
                self.cutout_ratio
            )));
        }
        if !self.hysteresis_c.is_finite() || self.hysteresis_c < 0.0 {
            return Err(HaresError::Equipment(format!(
                "hysteresis_c must be finite and >= 0.0, got {}",
                self.hysteresis_c
            )));
        }
        if !self.min_cycle_time_s.is_finite() || self.min_cycle_time_s < 0.0 {
            return Err(HaresError::Equipment(format!(
                "min_cycle_time_s must be finite and >= 0.0, got {}",
                self.min_cycle_time_s
            )));
        }
        if self.min_cycle_time_s > 0.0 {
            let time_res_s = env.time_res.num_milliseconds() as f64 / 1000.0;
            if self.min_cycle_time_s < time_res_s {
                return Err(HaresError::Equipment(format!(
                    "min_cycle_time_s must be 0 or >= simulation timestep ({time_res_s} s), got {}",
                    self.min_cycle_time_s
                )));
            }
        }
        Ok(())
    }
}

/// Thermostat finite-state mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThermostatMode {
    Heating,
    Cooling,
    #[default]
    Deadband,
}

/// Setpoint pair in Celsius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermalSetpoints {
    pub heating_c: f64,
    pub cooling_c: f64,
}

impl ThermalSetpoints {
    pub fn validate_for_deadband(self, hysteresis_c: f64) -> crate::Result<()> {
        if self.cooling_c - self.heating_c < 2.0 * hysteresis_c {
            return Err(HaresError::Equipment(format!(
                "invalid setpoints: cooling-heating must be >= {}C, got {}",
                2.0 * hysteresis_c,
                self.cooling_c - self.heating_c
            )));
        }
        Ok(())
    }

    pub fn with_schedule_override(self, schedule: Option<ScheduleSetpoints>) -> Self {
        let Some(schedule) = schedule else {
            return self;
        };
        let mut merged = self;
        if schedule.no_space_heating {
            merged.heating_c = HEATING_DISABLED_SETPOINT_C;
        } else if let Some(value) = schedule.heating_c {
            merged.heating_c = value;
        }
        if schedule.no_space_cooling {
            merged.cooling_c = COOLING_DISABLED_SETPOINT_C;
        } else if let Some(value) = schedule.cooling_c {
            merged.cooling_c = value;
        }
        merged
    }

    pub fn with_control_override(self, control: Option<RuntimeSetpointOverride>) -> Self {
        let Some(control) = control else {
            return self;
        };
        Self {
            heating_c: control.heating_c.unwrap_or(self.heating_c),
            cooling_c: control.cooling_c.unwrap_or(self.cooling_c),
        }
    }
}

/// Time-varying schedule setpoints and ResStock no-space-conditioning sentinels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScheduleSetpoints {
    pub heating_c: Option<f64>,
    pub cooling_c: Option<f64>,
    pub no_space_heating: bool,
    pub no_space_cooling: bool,
}

/// Runtime control-signal override values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSetpointOverride {
    pub heating_c: Option<f64>,
    pub cooling_c: Option<f64>,
}

/// Equipment category for HVAC defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HvacEquipmentType {
    GasFurnace,
    ElectricFurnace,
    AshpHeatPumpOnly,
    AshpHeatPumpAux,
    MiniSplitHeat,
    Baseboard,
    Other,
}

/// Dynamic speed-control mode for HVAC performance selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeedControlMode {
    #[default]
    SingleSpeed,
    TwoSpeedSetpoint,
    FourSpeed,
    VariableSpeedIdeal,
}

/// Startup degradation configuration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupConfig {
    pub capacity_fraction: f64,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            capacity_fraction: 1.0,
        }
    }
}

impl StartupConfig {
    pub fn validate(self) -> crate::Result<()> {
        if !self.capacity_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.capacity_fraction)
        {
            return Err(HaresError::Equipment(format!(
                "startup capacity_fraction must be finite and in [0,1], got {}",
                self.capacity_fraction
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedSelection {
    pub speed_index: usize,
    pub part_load_ratio: f64,
    pub speed_fraction: f64,
}

impl HvacEquipmentType {
    pub fn default_supply_air_temp_c(self, outdoor_temp_c: f64) -> f64 {
        match self {
            Self::GasFurnace => 54.4,
            Self::ElectricFurnace => 48.9,
            Self::AshpHeatPumpOnly => 32.2 + 0.15 * (outdoor_temp_c - 8.3),
            Self::AshpHeatPumpAux => 40.6,
            Self::MiniSplitHeat => 43.3,
            Self::Baseboard => 0.0,
            Self::Other => 40.6,
        }
    }
}

/// Ideal-capacity callback implemented by thermal-domain solvers.
pub trait IdealCapacitySolver {
    fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64;
}

impl<F> IdealCapacitySolver for F
where
    F: Fn(&EnvironmentState, ZoneId) -> f64,
{
    fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64 {
        self(env, zone)
    }
}

/// Shared HVAC wrapper with thermostat state and base helper parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacEquipment {
    pub equipment_type: HvacEquipmentType,
    pub zone_id: ZoneId,
    pub thermostat: ThermostatConfig,
    pub mode: ThermostatMode,
    pub duty_cycle: f64,
    pub static_setpoints: ThermalSetpoints,
    pub schedule_setpoints: Option<ScheduleSetpoints>,
    pub runtime_setpoints: Option<RuntimeSetpointOverride>,
    pub last_mode_switch_at: Option<DateTime<Utc>>,
    pub heating_capacities_w: Vec<f64>,
    pub cooling_capacities_w: Vec<f64>,
    pub eir_by_stage: Vec<f64>,
    pub fan_power_w_per_cfm: f64,
    pub shr: f64,
    pub duct_dse: f64,
    pub supply_air_temp_c: f64,
    pub airflow_cfm_per_ton: f64,
    pub zone_heat_fractions: Vec<(ZoneId, f64)>,
    pub biquadratic_coeffs: Vec<[f64; 6]>,
    pub biquadratic_x1_bounds: (f64, f64),
    pub biquadratic_x2_bounds: (f64, f64),
    pub speed_control_mode: SpeedControlMode,
    pub low_speed_capacity_fraction: f64,
    pub plf_cooling_degradation_coeff: f64,
    pub plf_state: f64,
    pub startup: StartupConfig,
    pub startup_active: bool,
    pub startup_applied_this_step: bool,
    pub last_speed_index: usize,
    pub variable_speed_fraction: f64,
}

impl HvacEquipment {
    pub fn new(equipment_type: HvacEquipmentType, zone_id: ZoneId) -> Self {
        Self {
            equipment_type,
            zone_id,
            thermostat: ThermostatConfig::default(),
            mode: ThermostatMode::Deadband,
            duty_cycle: 0.0,
            static_setpoints: ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            },
            schedule_setpoints: None,
            runtime_setpoints: None,
            last_mode_switch_at: None,
            heating_capacities_w: vec![],
            cooling_capacities_w: vec![],
            eir_by_stage: vec![],
            fan_power_w_per_cfm: 0.365,
            shr: 1.0,
            duct_dse: 1.0,
            supply_air_temp_c: equipment_type.default_supply_air_temp_c(8.3),
            airflow_cfm_per_ton: AIRFLOW_DEFAULT_CFM_PER_TON,
            zone_heat_fractions: vec![(zone_id, 1.0)],
            biquadratic_coeffs: vec![DEFAULT_BIQUADRATIC_COEFFS],
            biquadratic_x1_bounds: (f64::NEG_INFINITY, f64::INFINITY),
            biquadratic_x2_bounds: (f64::NEG_INFINITY, f64::INFINITY),
            speed_control_mode: SpeedControlMode::SingleSpeed,
            low_speed_capacity_fraction: 0.5,
            plf_cooling_degradation_coeff: 0.25,
            plf_state: 1.0,
            startup: StartupConfig::default(),
            startup_active: false,
            startup_applied_this_step: false,
            last_speed_index: 0,
            variable_speed_fraction: 0.0,
        }
    }

    pub fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.thermostat = ThermostatConfig {
            hysteresis_c: extract_numeric(config, "hysteresis_c").unwrap_or(1.0),
            cutout_ratio: extract_numeric(config, "cutout_ratio").unwrap_or(0.0),
            min_cycle_time_s: extract_numeric(config, "min_cycle_time_s").unwrap_or(0.0),
            use_ideal_capacity: extract_bool(config, "use_ideal_capacity").unwrap_or(false),
        };
        self.thermostat.validate(env)?;

        if let Some(value) = extract_numeric(config, "heating_setpoint_c") {
            self.static_setpoints.heating_c = value;
        }
        if let Some(value) = extract_numeric(config, "cooling_setpoint_c") {
            self.static_setpoints.cooling_c = value;
        }
        self.static_setpoints
            .validate_for_deadband(self.thermostat.hysteresis_c)?;

        self.airflow_cfm_per_ton =
            extract_numeric(config, "airflow_cfm_per_ton").unwrap_or(AIRFLOW_DEFAULT_CFM_PER_TON);
        let airflow_defect_ratio = extract_numeric(config, "AirflowDefectRatio")
            .or_else(|| extract_numeric(config, "airflow_defect_ratio"))
            .unwrap_or(1.0);
        self.airflow_cfm_per_ton *= airflow_defect_ratio;

        self.supply_air_temp_c =
            extract_numeric(config, "supply_air_temp_c").unwrap_or_else(|| {
                self.equipment_type
                    .default_supply_air_temp_c(env.weather.outdoor_temp_c)
            });

        self.speed_control_mode = parse_speed_control_mode(config);
        self.low_speed_capacity_fraction =
            extract_numeric(config, "low_speed_capacity_fraction").unwrap_or(0.5);
        self.plf_cooling_degradation_coeff = extract_numeric(config, "cooling_cd")
            .or_else(|| extract_numeric(config, "cd"))
            .unwrap_or(0.25);
        self.startup = StartupConfig {
            capacity_fraction: extract_numeric(config, "startup_capacity_fraction").unwrap_or(1.0),
        };
        self.startup.validate()?;
        self.biquadratic_coeffs = load_biquadratic_coeffs(config, "biquadratic_coeffs")?;

        Ok(())
    }

    pub fn apply_control_signal(&mut self, signal: &ControlSignal) {
        if let ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            ..
        } = signal
        {
            self.runtime_setpoints = Some(RuntimeSetpointOverride {
                heating_c: *heating_setpoint_c,
                cooling_c: *cooling_setpoint_c,
            });
        }
    }

    pub fn set_schedule_setpoints(&mut self, schedule: ScheduleSetpoints) {
        self.schedule_setpoints = Some(schedule);
    }

    pub fn clear_schedule_setpoints(&mut self) {
        self.schedule_setpoints = None;
    }

    pub fn effective_setpoints(&self) -> ThermalSetpoints {
        self.static_setpoints
            .with_schedule_override(self.schedule_setpoints)
            .with_control_override(self.runtime_setpoints)
    }

    pub fn update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode> {
        let zone_temp = lookup_zone_temp(env, self.zone_id)?;
        let setpoints = self.effective_setpoints();

        if !self.is_cycle_change_allowed(env.current_time) {
            return Ok(self.mode);
        }

        let hysteresis = self.thermostat.hysteresis_c;
        let cutout = self.thermostat.cutout_ratio;

        let next_mode = match self.mode {
            ThermostatMode::Heating => {
                if zone_temp > setpoints.heating_c + hysteresis * cutout {
                    ThermostatMode::Deadband
                } else {
                    ThermostatMode::Heating
                }
            }
            ThermostatMode::Cooling => {
                if zone_temp < setpoints.cooling_c - hysteresis * cutout {
                    ThermostatMode::Deadband
                } else {
                    ThermostatMode::Cooling
                }
            }
            ThermostatMode::Deadband => {
                if zone_temp < setpoints.heating_c - hysteresis {
                    ThermostatMode::Heating
                } else if zone_temp > setpoints.cooling_c + hysteresis {
                    ThermostatMode::Cooling
                } else {
                    ThermostatMode::Deadband
                }
            }
        };

        self.set_mode(next_mode, env.current_time);
        Ok(self.mode)
    }

    pub fn update_mode_and_duty_with_ideal_solver<S: IdealCapacitySolver>(
        &mut self,
        env: &EnvironmentState,
        solver: &S,
    ) -> crate::Result<ThermostatMode> {
        let mode = self.update_mode(env)?;
        if mode == ThermostatMode::Deadband {
            self.duty_cycle = 0.0;
            return Ok(mode);
        }

        if self.use_ideal_capacity(env) {
            let ideal_rate_w = solver.solve_ideal_capacity(env, self.zone_id);
            let numerator = match mode {
                ThermostatMode::Heating => ideal_rate_w.max(0.0),
                ThermostatMode::Cooling => (-ideal_rate_w).max(0.0),
                ThermostatMode::Deadband => 0.0,
            };
            let rated_capacity = self.rated_capacity_w(mode).max(0.0);
            self.duty_cycle = if rated_capacity > 0.0 {
                (numerator / rated_capacity).clamp(0.0, 1.0)
            } else {
                0.0
            };
        } else {
            self.duty_cycle = 1.0;
        }

        Ok(mode)
    }

    pub fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
        self.thermostat.use_ideal_capacity
            || env.time_res >= ChronoDuration::seconds(IDEAL_CAPACITY_TIME_RES_THRESHOLD_S)
    }

    pub fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<Utc>) {
        if self.mode != mode {
            self.mode = mode;
            self.last_mode_switch_at = Some(when);
        }
    }

    pub fn evaluate_biquadratic(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
    ) -> f64 {
        let coeffs = self
            .biquadratic_coeffs
            .get(curve_index)
            .copied()
            .or_else(|| self.biquadratic_coeffs.last().copied())
            .unwrap_or(DEFAULT_BIQUADRATIC_COEFFS);
        let curve = BiquadraticCurve {
            coeffs,
            x1_bounds: self.biquadratic_x1_bounds,
            x2_bounds: self.biquadratic_x2_bounds,
        };
        curve.evaluate(t_indoor_c, t_outdoor_c)
    }

    pub fn evaluate_biquadratic_with_flow_and_plf(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
        flow_fraction_correction: f64,
        plf: f64,
    ) -> (f64, f64, f64) {
        let raw = self.evaluate_biquadratic(curve_index, t_indoor_c, t_outdoor_c);
        let adjusted = raw * flow_fraction_correction;
        let final_value = adjusted * plf;
        (raw, adjusted, final_value)
    }

    pub fn part_load_factor(&mut self, plr: f64) -> f64 {
        let plr = plr.clamp(0.0, 1.0);
        let cd = self.plf_cooling_degradation_coeff.clamp(0.0, 1.0);
        let plf = 1.0 - cd * (1.0 - plr);
        self.plf_state = plf;
        plf
    }

    pub fn select_speed(&mut self, load_fraction: f64) -> SpeedSelection {
        let load_fraction = load_fraction.clamp(0.0, 1.0);
        let selection = match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => SpeedSelection {
                speed_index: 0,
                part_load_ratio: load_fraction,
                speed_fraction: load_fraction,
            },
            SpeedControlMode::TwoSpeedSetpoint => {
                let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
                if load_fraction > low_cap {
                    SpeedSelection {
                        speed_index: 1,
                        part_load_ratio: load_fraction,
                        speed_fraction: 1.0,
                    }
                } else {
                    SpeedSelection {
                        speed_index: 0,
                        part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                        speed_fraction: low_cap,
                    }
                }
            }
            SpeedControlMode::FourSpeed => {
                let thresholds = [0.25, 0.50, 0.75, 1.0];
                let mut index = 0usize;
                for (i, threshold) in thresholds.iter().enumerate() {
                    if load_fraction <= *threshold {
                        index = i;
                        break;
                    }
                }
                let stage_capacity_fraction = thresholds[index];
                SpeedSelection {
                    speed_index: index,
                    part_load_ratio: (load_fraction / stage_capacity_fraction).clamp(0.0, 1.0),
                    speed_fraction: stage_capacity_fraction,
                }
            }
            SpeedControlMode::VariableSpeedIdeal => SpeedSelection {
                speed_index: 0,
                part_load_ratio: 1.0,
                speed_fraction: load_fraction,
            },
        };
        self.last_speed_index = selection.speed_index;
        self.variable_speed_fraction = selection.speed_fraction;
        selection
    }

    pub fn apply_startup_capacity_degradation(&mut self, steady_capacity_w: f64) -> f64 {
        let on_now = self.duty_cycle > 0.0;
        let startup_step = on_now && !self.startup_active;
        self.startup_applied_this_step = startup_step;
        self.startup_active = on_now;
        if startup_step {
            steady_capacity_w * self.startup.capacity_fraction
        } else {
            steady_capacity_w
        }
    }

    pub fn rated_capacity_w(&self, mode: ThermostatMode) -> f64 {
        match mode {
            ThermostatMode::Heating => self
                .heating_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Cooling => self
                .cooling_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Deadband => 0.0,
        }
    }

    pub fn capacity_at_stage(capacities: &[f64], stage_index: usize) -> f64 {
        if capacities.is_empty() {
            return 0.0;
        }
        capacities[stage_index.min(capacities.len() - 1)]
    }

    pub fn eir_at_stage(&self, stage_index: usize) -> f64 {
        if self.eir_by_stage.is_empty() {
            return 1.0;
        }
        self.eir_by_stage[stage_index.min(self.eir_by_stage.len() - 1)]
    }

    pub fn fan_power_w(&self, airflow_cfm: f64) -> f64 {
        (airflow_cfm.max(0.0)) * self.fan_power_w_per_cfm
    }

    pub fn sensible_latent_from_shr(&self, total_cooling_w: f64) -> (f64, f64) {
        let shr = self.shr.clamp(0.0, 1.0);
        let sensible = total_cooling_w * shr;
        let latent = total_cooling_w - sensible;
        (sensible, latent)
    }

    pub fn apply_duct_dse(&self, capacity_w: f64) -> f64 {
        capacity_w * self.duct_dse.clamp(0.0, 1.0)
    }

    pub fn write_zone_thermal_contributions(
        &self,
        ports: &mut PortSlots,
        sensible_gain_w: f64,
        latent_gain_w: f64,
    ) -> crate::Result<()> {
        let fractions: &[(ZoneId, f64)] = if self.zone_heat_fractions.is_empty() {
            &[(self.zone_id, 1.0)]
        } else {
            &self.zone_heat_fractions
        };

        // Fast path: single zone (>95% of dwellings)
        if fractions.len() == 1 {
            return ports.accumulate(&PortContribution::Thermal {
                zone: fractions[0].0,
                sensible_gain_w,
                latent_gain_w,
            });
        }

        // Multi-zone: normalize fractions and write each
        let sum: f64 = fractions.iter().map(|(_, f)| f.max(0.0)).sum();
        if sum <= f64::EPSILON {
            return ports.accumulate(&PortContribution::Thermal {
                zone: self.zone_id,
                sensible_gain_w,
                latent_gain_w,
            });
        }
        for &(zone, fraction) in fractions {
            let normalized = fraction.max(0.0) / sum;
            ports.accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: sensible_gain_w * normalized,
                latent_gain_w: latent_gain_w * normalized,
            })?;
        }
        Ok(())
    }

    pub fn update_supply_air_temp(&mut self, env: &EnvironmentState) {
        if self.equipment_type == HvacEquipmentType::AshpHeatPumpOnly {
            self.supply_air_temp_c = self
                .equipment_type
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        }
    }

    fn is_cycle_change_allowed(&self, now: DateTime<Utc>) -> bool {
        if self.thermostat.min_cycle_time_s <= 0.0 {
            return true;
        }
        let Some(last_switch) = self.last_mode_switch_at else {
            return true;
        };
        let elapsed_ms = (now - last_switch).num_milliseconds().max(0) as f64;
        elapsed_ms / 1000.0 >= self.thermostat.min_cycle_time_s
    }
}

fn extract_numeric(config: &EquipmentConfig, key: &str) -> Option<f64> {
    config.get_f64(key)
}

fn extract_bool(config: &EquipmentConfig, key: &str) -> Option<bool> {
    config.get_bool(key).or_else(|| {
        config.get_f64(key).map(|v| v > 0.0)
    })
}

fn parse_speed_control_mode(config: &EquipmentConfig) -> SpeedControlMode {
    let from_text = config
        .get_str("speed_control_mode")
        .map(|v| v.trim().to_ascii_lowercase());
    if let Some(value) = from_text {
        return match value.as_str() {
            "single" | "single_speed" | "single-speed" => SpeedControlMode::SingleSpeed,
            "two" | "two_speed" | "two-speed" | "two_speed_setpoint" => {
                SpeedControlMode::TwoSpeedSetpoint
            }
            "four" | "four_speed" | "four-speed" => SpeedControlMode::FourSpeed,
            "variable" | "variable_speed" | "variable-speed" | "ideal" => {
                SpeedControlMode::VariableSpeedIdeal
            }
            _ => SpeedControlMode::SingleSpeed,
        };
    }
    match config.get_f64("speed_control_mode") {
        Some(2.0) => SpeedControlMode::TwoSpeedSetpoint,
        Some(4.0) => SpeedControlMode::FourSpeed,
        Some(3.0) | Some(0.0) => SpeedControlMode::VariableSpeedIdeal,
        _ => SpeedControlMode::SingleSpeed,
    }
}

fn load_biquadratic_coeffs(config: &EquipmentConfig, key: &str) -> crate::Result<Vec<[f64; 6]>> {
    if let Some(raw) = config.get_str(key) {
        return parse_biquadratic_list(raw);
    }

    let mut curves = Vec::new();
    let mut curve_index = 0usize;
    loop {
        let mut coeffs = [0.0; 6];
        let mut any = false;
        for (coeff_index, slot) in coeffs.iter_mut().enumerate() {
            let coeff_key = format!("{key}_{curve_index}_{coeff_index}");
            if let Some(value) = config.get_f64(&coeff_key) {
                any = true;
                *slot = value;
            } else if any {
                return Err(HaresError::Equipment(format!(
                    "missing coefficient {coeff_key}"
                )));
            }
        }
        if !any {
            break;
        }
        curves.push(coeffs);
        curve_index += 1;
    }

    if curves.is_empty() {
        Ok(vec![DEFAULT_BIQUADRATIC_COEFFS])
    } else {
        Ok(curves)
    }
}

pub(super) fn parse_biquadratic_list(raw: &str) -> crate::Result<Vec<[f64; 6]>> {
    let mut parsed = Vec::new();
    let mut numbers = Vec::new();
    let mut token = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            token.push(ch);
        } else if !token.is_empty() {
            let value = token.parse::<f64>().map_err(|err| {
                HaresError::Equipment(format!("invalid biquadratic coefficient '{token}': {err}"))
            })?;
            numbers.push(value);
            token.clear();
        }
    }
    if !token.is_empty() {
        let value = token.parse::<f64>().map_err(|err| {
            HaresError::Equipment(format!("invalid biquadratic coefficient '{token}': {err}"))
        })?;
        numbers.push(value);
    }

    if numbers.is_empty() {
        return Ok(vec![DEFAULT_BIQUADRATIC_COEFFS]);
    }
    if !numbers.len().is_multiple_of(6) {
        return Err(HaresError::Equipment(format!(
            "biquadratic coefficient count must be multiple of 6, got {}",
            numbers.len()
        )));
    }
    for chunk in numbers.chunks_exact(6) {
        parsed.push([chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5]]);
    }
    Ok(parsed)
}

fn lookup_zone_temp(env: &EnvironmentState, zone: ZoneId) -> crate::Result<f64> {
    env.zones
        .iter()
        .find(|z| z.id == zone)
        .map(|z| z.temperature_c)
        .ok_or_else(|| HaresError::Equipment(format!("zone {zone:?} not found")))
}


#[cfg(test)]
mod tests {
        use chrono::{Duration as ChronoDuration, TimeZone};
        use hares_types::{
            ControlSignal, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
            ThermalAccumulator, WeatherState, ZoneState,
        };
    
        use super::*;
    
        fn env(zone_temp_c: f64, time_res_s: i64, second: i64) -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp_c,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: 14.0,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: 8.3,
                    outdoor_humidity_ratio: 0.005,
                    wind_speed_m_s: 2.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: 10.0,
                    sky_temp_c: 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: 1,
                        direct_w_m2: 0.0,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                    }],
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
                current_time: chrono::Utc
                    .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                    .single()
                    .expect("valid")
                    + ChronoDuration::seconds(second),
                time_res: ChronoDuration::seconds(time_res_s),
            }
        }
    
        struct FixedIdealSolver(f64);
    
        impl IdealCapacitySolver for FixedIdealSolver {
            fn solve_ideal_capacity(&self, _env: &EnvironmentState, _zone: ZoneId) -> f64 {
                self.0
            }
        }
    
        #[test]
        fn thermostat_cutout_ratio_is_validated() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            let mut config = EquipmentConfig::default();
            config.raw_config.insert("cutout_ratio".to_string(), 1.2.into());
            let err = hvac
                .init(&config, &env(20.0, 60, 0))
                .expect_err("must fail");
            assert!(err.to_string().contains("cutout_ratio"));
        }
    
        #[test]
        fn thermostat_setpoint_deadband_invariant_is_validated() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            let mut config = EquipmentConfig::default();
            config.raw_config.insert("hysteresis_c".to_string(), 1.0.into());
            config
                .raw_config
                .insert("heating_setpoint_c".to_string(), 21.0.into());
            config
                .raw_config
                .insert("cooling_setpoint_c".to_string(), 22.0.into());
            let err = hvac
                .init(&config, &env(20.0, 60, 0))
                .expect_err("must fail");
            assert!(err.to_string().contains("cooling-heating"));
        }
    
        #[test]
        fn thermostat_fsm_heating_cycle_turns_on_and_off() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.thermostat.hysteresis_c = 1.0;
            hvac.thermostat.cutout_ratio = 0.5;
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 25.0,
            };
    
            let mode = hvac.update_mode(&env(18.9, 60, 0)).expect("mode updates");
            assert_eq!(mode, ThermostatMode::Heating);
    
            let mode = hvac.update_mode(&env(20.6, 60, 61)).expect("mode updates");
            assert_eq!(mode, ThermostatMode::Deadband);
        }
    
        #[test]
        fn min_cycle_time_holds_mode_until_elapsed() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.thermostat.min_cycle_time_s = 300.0;
            hvac.thermostat.hysteresis_c = 1.0;
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 25.0,
            };
    
            let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode updates");
            assert_eq!(mode, ThermostatMode::Heating);
    
            let mode = hvac.update_mode(&env(22.0, 60, 120)).expect("locked");
            assert_eq!(mode, ThermostatMode::Heating);
    
            let mode = hvac.update_mode(&env(22.0, 60, 301)).expect("unlocked");
            assert_eq!(mode, ThermostatMode::Deadband);
        }
    
        #[test]
        fn control_signal_thermal_setpoint_has_top_priority() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            };
            hvac.set_schedule_setpoints(ScheduleSetpoints {
                heating_c: Some(19.0),
                cooling_c: Some(25.0),
                ..ScheduleSetpoints::default()
            });
            hvac.apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            });
    
            let setpoints = hvac.effective_setpoints();
            assert_eq!(setpoints.heating_c, 18.0);
            assert_eq!(setpoints.cooling_c, 26.0);
        }
    
        #[test]
        fn no_space_heating_schedule_disables_heating_call() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 26.0,
            };
            hvac.set_schedule_setpoints(ScheduleSetpoints {
                no_space_heating: true,
                ..ScheduleSetpoints::default()
            });
            let mode = hvac.update_mode(&env(-20.0, 60, 0)).expect("mode updates");
            assert_eq!(mode, ThermostatMode::Deadband);
            assert_eq!(
                hvac.effective_setpoints().heating_c,
                HEATING_DISABLED_SETPOINT_C
            );
        }
    
        #[test]
        fn no_space_cooling_schedule_disables_cooling_call() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            };
            hvac.set_schedule_setpoints(ScheduleSetpoints {
                no_space_cooling: true,
                ..ScheduleSetpoints::default()
            });
            let mode = hvac.update_mode(&env(45.0, 60, 0)).expect("mode updates");
            assert_eq!(mode, ThermostatMode::Deadband);
            assert_eq!(
                hvac.effective_setpoints().cooling_c,
                COOLING_DISABLED_SETPOINT_C
            );
        }
    
        #[test]
        fn dse_reduces_effective_capacity() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.duct_dse = 0.8;
            assert_eq!(hvac.apply_duct_dse(10_000.0), 8_000.0);
        }
    
        #[test]
        fn supply_air_defaults_match_equipment_tables() {
            assert!((HvacEquipmentType::GasFurnace.default_supply_air_temp_c(8.3) - 54.4).abs() < 1e-9);
            assert!(
                (HvacEquipmentType::ElectricFurnace.default_supply_air_temp_c(8.3) - 48.9).abs() < 1e-9
            );
            assert!(
                (HvacEquipmentType::AshpHeatPumpOnly.default_supply_air_temp_c(8.3) - 32.2).abs()
                    < 1e-9
            );
            assert!(
                (HvacEquipmentType::AshpHeatPumpAux.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
            );
            assert!(
                (HvacEquipmentType::MiniSplitHeat.default_supply_air_temp_c(8.3) - 43.3).abs() < 1e-9
            );
        }
    
        #[test]
        fn airflow_defaults_to_375_and_scales_by_defect_ratio() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            let mut config = EquipmentConfig::default();
            config
                .raw_config
                .insert("AirflowDefectRatio".to_string(), 0.8.into());
            hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
            assert_eq!(hvac.airflow_cfm_per_ton, 300.0);
        }
    
        #[test]
        fn ideal_capacity_mode_engages_on_coarse_timesteps_and_sets_duty() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            hvac.heating_capacities_w = vec![10_000.0];
            hvac.static_setpoints = ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 26.0,
            };
            let solver = FixedIdealSolver(5_000.0);
            let mode = hvac
                .update_mode_and_duty_with_ideal_solver(&env(18.0, 300, 0), &solver)
                .expect("mode updates");
            assert_eq!(mode, ThermostatMode::Heating);
            assert!((hvac.duty_cycle - 0.5).abs() < 1e-9);
        }
    
        #[test]
        fn two_hvac_instances_same_zone_accumulate_thermal_ports() {
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let hvac_a = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
            let hvac_b = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
    
            hvac_a
                .write_zone_thermal_contributions(&mut ports, 1000.0, 0.0)
                .expect("write a");
            hvac_b
                .write_zone_thermal_contributions(&mut ports, 1500.0, 0.0)
                .expect("write b");
    
            assert_eq!(ports.thermal[0].sensible_gain_w, 2500.0);
        }
    
        #[test]
        fn biquadratic_evaluation_matches_reference_value() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.biquadratic_coeffs = vec![[1.0, 0.2, 0.01, -0.1, 0.005, 0.02]];
            let x = 20.0;
            let y = 30.0;
            let expected = 1.0 + 0.2 * x + 0.01 * x * x - 0.1 * y + 0.005 * y * y + 0.02 * x * y;
            let got = hvac.evaluate_biquadratic(0, x, y);
            assert!((got - expected).abs() < 1e-12);
        }
    
        #[test]
        fn flow_and_plf_are_applied_after_biquadratic() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.biquadratic_coeffs = vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
            let (raw, adjusted, final_value) =
                hvac.evaluate_biquadratic_with_flow_and_plf(0, 19.0, 35.0, 1.1, 0.9);
            assert!((raw - 1.0).abs() < 1e-12);
            assert!((adjusted - 1.1).abs() < 1e-12);
            assert!((final_value - 0.99).abs() < 1e-12);
        }
    
        #[test]
        fn part_load_factor_matches_ticket_formula() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.plf_cooling_degradation_coeff = 0.25;
            let plf = hvac.part_load_factor(0.5);
            assert!((plf - 0.875).abs() < 1e-12);
        }
    
        #[test]
        fn two_speed_setpoint_control_switches_at_threshold() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
            hvac.low_speed_capacity_fraction = 0.6;
            let low = hvac.select_speed(0.4);
            let high = hvac.select_speed(0.8);
            assert_eq!(low.speed_index, 0);
            assert_eq!(high.speed_index, 1);
        }
    
        #[test]
        fn four_speed_control_picks_expected_indices() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.speed_control_mode = SpeedControlMode::FourSpeed;
            assert_eq!(hvac.select_speed(0.25).speed_index, 0);
            assert_eq!(hvac.select_speed(0.50).speed_index, 1);
            assert_eq!(hvac.select_speed(0.75).speed_index, 2);
            assert_eq!(hvac.select_speed(1.00).speed_index, 3);
        }
    
        #[test]
        fn variable_speed_matches_requested_capacity_fraction() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
            let sel = hvac.select_speed(0.63);
            assert!((sel.speed_fraction - 0.63).abs() < 1e-12);
            assert_eq!(sel.part_load_ratio, 1.0);
        }
    
        #[test]
        fn startup_degradation_applies_only_first_on_step() {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.startup.capacity_fraction = 0.7;
            hvac.duty_cycle = 1.0;
            let step1 = hvac.apply_startup_capacity_degradation(10_000.0);
            let step2 = hvac.apply_startup_capacity_degradation(10_000.0);
            hvac.duty_cycle = 0.0;
            let _off = hvac.apply_startup_capacity_degradation(10_000.0);
            hvac.duty_cycle = 1.0;
            let step3 = hvac.apply_startup_capacity_degradation(10_000.0);
            assert!((step1 - 7_000.0).abs() < 1e-9);
            assert!((step2 - 10_000.0).abs() < 1e-9);
            assert!((step3 - 7_000.0).abs() < 1e-9);
        }
}
