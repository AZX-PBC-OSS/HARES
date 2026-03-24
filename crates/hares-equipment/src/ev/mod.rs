//! Electric vehicle charging equipment model.

use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField,
};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

mod archetype;
mod charging_curve;
mod config;
mod schedule;

use archetype::{DriverArchetype, default_distribution};
use charging_curve::{ChargingCurveLut, parse_pybamm_lut_csv};
use config::*;
use schedule::{EventDistributionRow, parse_distribution_rows, sample_distribution};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ChargingLevel {
    L1,
    L2,
}

impl ChargingLevel {
    fn from_config(config: &EquipmentConfig) -> Self {
        let level = config
            .get_str(KEY_CHARGING_LEVEL)
            .or_else(|| config.get_str(KEY_CHARGING_LEVEL_HPIXML))
            .unwrap_or("L2")
            .trim()
            .replace(' ', "")
            .to_ascii_lowercase();

        match level.as_str() {
            "l1" | "level1" => Self::L1,
            _ => Self::L2,
        }
    }

    fn telemetry_code(self) -> f64 {
        match self {
            Self::L1 => 1.0,
            Self::L2 => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PlugInPolicy {
    Always,
    LowSoc,
}

impl PlugInPolicy {
    fn from_config(config: &EquipmentConfig) -> Self {
        match config
            .get_str(KEY_PLUG_IN_POLICY)
            .unwrap_or("always")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "low_soc" | "lowsoc" => Self::LowSoc,
            _ => Self::Always,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct SampledEvent {
    start_second_of_day: i64,
    end_second_of_day: i64,
    start_soc: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct EvCheckpoint {
    soc: f64,
    connected: bool,
    active_power_kw: f64,
    time_until_departure_s: f64,
    current_day_ordinal: Option<i32>,
    todays_event: Option<SampledEvent>,
    arrival_soc_applied: bool,
    rng_seed: [u8; 32],
    rng_draws: u64,
    power_limit_kw: Option<f64>,
    power_setpoint_kw: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,
    battery_temp_c: f64,
    heater_active: bool,
    current_day_drive_miles: f64,
    shift_phase_days: u32,
    plug_in_policy: PlugInPolicy,
    plug_in_soc_threshold: f64,
    v2l_enabled: bool,
    v2l_soc_reserve: f64,
    v2l_max_discharge_kw: f64,
    v2g_enabled: bool,
    v2g_soc_reserve: f64,
    v2g_max_discharge_kw: f64,
    driver_archetype: DriverArchetype,
}

pub struct Ev {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,

    battery_capacity_kwh: f64,
    charging_level: ChargingLevel,
    rated_power_kw: f64,
    charging_efficiency: f64,
    l1_current_a: Option<f64>,
    l1_voltage_v: f64,
    soc_max: f64,
    event_day_ratio: f64,
    immediate_target_soc: Option<f64>,
    delay_until_hour: Option<f64>,
    tou_avoid_peak: bool,
    tou_peak_start_hour: Option<f64>,
    tou_peak_end_hour: Option<f64>,
    ready_by_hour: Option<f64>,
    ready_target_soc: Option<f64>,
    charging_curve_lut: Option<ChargingCurveLut>,
    distributions: Vec<EventDistributionRow>,
    min_charge_temp_c: f64,
    full_power_temp_c: f64,
    heater_power_w: f64,
    heater_threshold_c: f64,
    thermal_mass_j_per_k: f64,
    ua_w_per_k: f64,
    driver_archetype: DriverArchetype,
    arrival_fuzz_minutes: f64,
    departure_fuzz_minutes: f64,
    daily_drive_miles_mean: f64,
    daily_drive_miles_stddev: f64,
    shift_rotation_days: u32,
    shift_on_days: u32,
    shift_duration_fuzz_minutes: f64,
    plug_in_policy: PlugInPolicy,
    plug_in_soc_threshold: f64,
    v2l_enabled: bool,
    v2l_soc_reserve: f64,
    v2l_max_discharge_kw: f64,
    v2g_enabled: bool,
    v2g_soc_reserve: f64,
    v2g_max_discharge_kw: f64,

    soc: f64,
    battery_temp_c: f64,
    heater_active: bool,
    connected: bool,
    active_power_kw: f64,
    time_until_departure_s: f64,
    current_day_ordinal: Option<i32>,
    current_day_drive_miles: f64,
    todays_event: Option<SampledEvent>,
    arrival_soc_applied: bool,
    shift_phase_days: u32,

    v2l_active: bool,
    v2l_power_kw: f64,

    rng_seed: [u8; 32],
    rng_draws: u64,
    rng: ChaCha8Rng,

    power_limit_kw: Option<f64>,
    power_setpoint_kw: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,
}

impl Ev {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(config.get_f64(KEY_EQUIPMENT_ID).unwrap_or_default() as u32),
            name: config.name.clone(),
            end_use: EndUse::EV,
            equipment_type: Cow::Borrowed("EV"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::SOC_TARGET
                | ControlCapabilities::POWER_LIMIT,
            telemetry_fields: telemetry_fields(),
        };

        let rng_seed = derive_rng_seed(&config);
        let rng = ChaCha8Rng::from_seed(rng_seed);
        let charging_level = ChargingLevel::from_config(&config);
        let battery_capacity_kwh = resolve_capacity_kwh(&config).unwrap_or(DEFAULT_CAPACITY_KWH);
        let rated_power_kw = resolve_rated_power_kw(&config, charging_level, battery_capacity_kwh)
            .unwrap_or_else(|| default_max_power_kw(&config, charging_level, battery_capacity_kwh));
        let driver_archetype = DriverArchetype::from_config(&config);

        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(charging_level),
            battery_capacity_kwh,
            charging_level,
            rated_power_kw,
            charging_efficiency: config.get_f64(KEY_EFFICIENCY).unwrap_or(DEFAULT_EFFICIENCY),
            l1_current_a: config.get_f64(KEY_L1_CURRENT_A),
            l1_voltage_v: config
                .get_f64(KEY_L1_VOLTAGE_V)
                .unwrap_or(DEFAULT_L1_VOLTAGE_V),
            soc_max: config.get_f64(KEY_SOC_MAX).unwrap_or(DEFAULT_SOC_MAX),
            event_day_ratio: config
                .get_f64(KEY_EVENT_DAY_RATIO)
                .unwrap_or_else(|| default_event_day_ratio(battery_capacity_kwh, charging_level)),
            immediate_target_soc: config.get_f64(KEY_IMMEDIATE_TARGET_SOC),
            delay_until_hour: config.get_f64(KEY_DELAY_UNTIL_HOUR),
            tou_avoid_peak: config.get_bool(KEY_TOU_AVOID_PEAK).unwrap_or(false),
            tou_peak_start_hour: config.get_f64(KEY_TOU_PEAK_START_HOUR),
            tou_peak_end_hour: config.get_f64(KEY_TOU_PEAK_END_HOUR),
            ready_by_hour: config.get_f64(KEY_READY_BY_HOUR),
            ready_target_soc: config.get_f64(KEY_READY_TARGET_SOC),
            charging_curve_lut: None,
            distributions: default_distribution(driver_archetype),
            min_charge_temp_c: DEFAULT_MIN_CHARGE_TEMP_C,
            full_power_temp_c: DEFAULT_FULL_POWER_TEMP_C,
            heater_power_w: DEFAULT_HEATER_POWER_W,
            heater_threshold_c: DEFAULT_HEATER_THRESHOLD_C,
            thermal_mass_j_per_k: DEFAULT_THERMAL_MASS_J_PER_K,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            driver_archetype,
            arrival_fuzz_minutes: config.get_f64(KEY_ARRIVAL_FUZZ_MINUTES).unwrap_or(0.0),
            departure_fuzz_minutes: config.get_f64(KEY_DEPARTURE_FUZZ_MINUTES).unwrap_or(0.0),
            daily_drive_miles_mean: config
                .get_f64(KEY_DAILY_DRIVE_MILES_MEAN)
                .unwrap_or(DEFAULT_DAILY_DRIVE_MILES_MEAN),
            daily_drive_miles_stddev: config
                .get_f64(KEY_DAILY_DRIVE_MILES_STDDEV)
                .unwrap_or(DEFAULT_DAILY_DRIVE_MILES_STDDEV),
            shift_rotation_days: config.get_f64(KEY_SHIFT_ROTATION_DAYS).unwrap_or(14.0) as u32,
            shift_on_days: config.get_f64(KEY_SHIFT_ON_DAYS).unwrap_or(4.0) as u32,
            shift_duration_fuzz_minutes: config
                .get_f64(KEY_SHIFT_DURATION_FUZZ_MINUTES)
                .unwrap_or(30.0),
            plug_in_policy: PlugInPolicy::from_config(&config),
            plug_in_soc_threshold: config
                .get_f64(KEY_PLUG_IN_SOC_THRESHOLD)
                .unwrap_or(DEFAULT_PLUG_IN_SOC_THRESHOLD),
            v2l_enabled: config.get_bool(KEY_V2L_ENABLED).unwrap_or(false),
            v2l_soc_reserve: config
                .get_f64(KEY_V2L_SOC_RESERVE)
                .unwrap_or(DEFAULT_V2L_SOC_RESERVE),
            v2l_max_discharge_kw: config
                .get_f64(KEY_V2L_MAX_DISCHARGE_KW)
                .unwrap_or(DEFAULT_V2L_MAX_DISCHARGE_KW),
            v2g_enabled: config.get_bool(KEY_V2G_ENABLED).unwrap_or(false),
            v2g_soc_reserve: config
                .get_f64(KEY_V2G_SOC_RESERVE)
                .unwrap_or(DEFAULT_V2G_SOC_RESERVE),
            v2g_max_discharge_kw: config
                .get_f64(KEY_V2G_MAX_DISCHARGE_KW)
                .unwrap_or(DEFAULT_V2G_MAX_DISCHARGE_KW),
            soc: config.get_f64(KEY_INITIAL_SOC).unwrap_or(DEFAULT_SOC),
            battery_temp_c: config.get_f64(KEY_BATTERY_TEMP_C).unwrap_or(20.0),
            heater_active: false,
            connected: false,
            active_power_kw: 0.0,
            time_until_departure_s: 0.0,
            current_day_ordinal: None,
            current_day_drive_miles: 0.0,
            todays_event: None,
            arrival_soc_applied: false,
            shift_phase_days: 0,
            v2l_active: false,
            v2l_power_kw: 0.0,
            rng_seed,
            rng_draws: 0,
            rng,
            power_limit_kw: config.get_f64(KEY_POWER_LIMIT_KW),
            power_setpoint_kw: None,
            soc_target: None,
            soc_target_min: None,
            soc_target_max: None,
        }
    }

    fn init_from_config(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        self.battery_capacity_kwh =
            resolve_capacity_kwh(config).unwrap_or(self.battery_capacity_kwh);
        self.charging_level = ChargingLevel::from_config(config);
        self.rated_power_kw =
            resolve_rated_power_kw(config, self.charging_level, self.battery_capacity_kwh)
                .unwrap_or_else(|| {
                    default_max_power_kw(config, self.charging_level, self.battery_capacity_kwh)
                });

        self.charging_efficiency = config.get_f64(KEY_EFFICIENCY).unwrap_or(DEFAULT_EFFICIENCY);
        if !(0.0..=1.0).contains(&self.charging_efficiency) || !self.charging_efficiency.is_finite()
        {
            return Err(HaresError::Equipment(
                "EV charging_efficiency must be finite and within [0, 1]".to_string(),
            ));
        }

        self.soc_max = config.get_f64(KEY_SOC_MAX).unwrap_or(DEFAULT_SOC_MAX);
        if !(0.0..=1.0).contains(&self.soc_max) || !self.soc_max.is_finite() {
            return Err(HaresError::Equipment(
                "EV soc_max must be finite and within [0, 1]".to_string(),
            ));
        }

        self.event_day_ratio = config.get_f64(KEY_EVENT_DAY_RATIO).unwrap_or_else(|| {
            default_event_day_ratio(self.battery_capacity_kwh, self.charging_level)
        });
        if !(0.0..=1.0).contains(&self.event_day_ratio) || !self.event_day_ratio.is_finite() {
            return Err(HaresError::Equipment(
                "EV event_day_ratio must be finite and within [0, 1]".to_string(),
            ));
        }

        self.l1_current_a = config.get_f64(KEY_L1_CURRENT_A);
        if let Some(current_a) = self.l1_current_a
            && (!current_a.is_finite() || current_a <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV l1_current_a must be finite and > 0".to_string(),
            ));
        }
        self.l1_voltage_v = config
            .get_f64(KEY_L1_VOLTAGE_V)
            .unwrap_or(DEFAULT_L1_VOLTAGE_V);
        if !self.l1_voltage_v.is_finite() || self.l1_voltage_v <= 0.0 {
            return Err(HaresError::Equipment(
                "EV l1_voltage_v must be finite and > 0".to_string(),
            ));
        }

        self.immediate_target_soc = config.get_f64(KEY_IMMEDIATE_TARGET_SOC);
        if let Some(soc) = self.immediate_target_soc
            && (!soc.is_finite() || !(0.0..=1.0).contains(&soc))
        {
            return Err(HaresError::Equipment(
                "EV immediate_target_soc must be finite and within [0, 1]".to_string(),
            ));
        }

        self.delay_until_hour = config.get_f64(KEY_DELAY_UNTIL_HOUR);
        validate_optional_hour("delay_until_hour", self.delay_until_hour)?;

        self.tou_avoid_peak = config.get_bool(KEY_TOU_AVOID_PEAK).unwrap_or(false);
        self.tou_peak_start_hour = config.get_f64(KEY_TOU_PEAK_START_HOUR);
        self.tou_peak_end_hour = config.get_f64(KEY_TOU_PEAK_END_HOUR);
        validate_optional_hour("tou_peak_start_hour", self.tou_peak_start_hour)?;
        validate_optional_hour("tou_peak_end_hour", self.tou_peak_end_hour)?;

        self.ready_by_hour = config.get_f64(KEY_READY_BY_HOUR);
        validate_optional_hour("ready_by_hour", self.ready_by_hour)?;
        self.ready_target_soc = config.get_f64(KEY_READY_TARGET_SOC);
        if let Some(soc) = self.ready_target_soc
            && (!soc.is_finite() || !(0.0..=1.0).contains(&soc))
        {
            return Err(HaresError::Equipment(
                "EV ready_target_soc must be finite and within [0, 1]".to_string(),
            ));
        }

        self.charging_curve_lut = match config.get_str(KEY_PYBAMM_LUT_PATH) {
            Some(path) => Some(parse_pybamm_lut_csv(Path::new(path))?),
            None => None,
        };
        self.min_charge_temp_c = config
            .get_f64(KEY_MIN_CHARGE_TEMP_C)
            .unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C);
        self.full_power_temp_c = config
            .get_f64(KEY_FULL_POWER_TEMP_C)
            .unwrap_or(DEFAULT_FULL_POWER_TEMP_C);
        self.heater_power_w = config
            .get_f64(KEY_HEATER_POWER_W)
            .unwrap_or(DEFAULT_HEATER_POWER_W);
        self.heater_threshold_c = config
            .get_f64(KEY_HEATER_THRESHOLD_C)
            .unwrap_or(DEFAULT_HEATER_THRESHOLD_C);
        self.thermal_mass_j_per_k = config
            .get_f64(KEY_THERMAL_MASS_J_PER_K)
            .unwrap_or(DEFAULT_THERMAL_MASS_J_PER_K);
        self.ua_w_per_k = config.get_f64(KEY_UA_W_PER_K).unwrap_or(DEFAULT_UA_W_PER_K);
        self.driver_archetype = DriverArchetype::from_config(config);
        self.arrival_fuzz_minutes = config.get_f64(KEY_ARRIVAL_FUZZ_MINUTES).unwrap_or(0.0);
        self.departure_fuzz_minutes = config.get_f64(KEY_DEPARTURE_FUZZ_MINUTES).unwrap_or(0.0);
        self.daily_drive_miles_mean = config
            .get_f64(KEY_DAILY_DRIVE_MILES_MEAN)
            .unwrap_or(DEFAULT_DAILY_DRIVE_MILES_MEAN);
        self.daily_drive_miles_stddev = config
            .get_f64(KEY_DAILY_DRIVE_MILES_STDDEV)
            .unwrap_or(DEFAULT_DAILY_DRIVE_MILES_STDDEV);
        self.shift_rotation_days = config.get_f64(KEY_SHIFT_ROTATION_DAYS).unwrap_or(14.0) as u32;
        self.shift_on_days = config.get_f64(KEY_SHIFT_ON_DAYS).unwrap_or(4.0) as u32;
        self.shift_duration_fuzz_minutes = config
            .get_f64(KEY_SHIFT_DURATION_FUZZ_MINUTES)
            .unwrap_or(30.0);

        if !self.min_charge_temp_c.is_finite()
            || !self.full_power_temp_c.is_finite()
            || self.full_power_temp_c < self.min_charge_temp_c
        {
            return Err(HaresError::Equipment(
                "EV temperature thresholds must be finite and full_power_temp_c >= min_charge_temp_c"
                    .to_string(),
            ));
        }
        if !self.heater_power_w.is_finite()
            || self.heater_power_w < 0.0
            || !self.heater_threshold_c.is_finite()
        {
            return Err(HaresError::Equipment(
                "EV heater parameters must be finite and heater_power_w >= 0".to_string(),
            ));
        }
        if !self.thermal_mass_j_per_k.is_finite()
            || self.thermal_mass_j_per_k <= 0.0
            || !self.ua_w_per_k.is_finite()
            || self.ua_w_per_k < 0.0
        {
            return Err(HaresError::Equipment(
                "EV thermal parameters must be finite with thermal_mass_j_per_k > 0 and ua_w_per_k >= 0"
                    .to_string(),
            ));
        }
        if !self.arrival_fuzz_minutes.is_finite()
            || self.arrival_fuzz_minutes < 0.0
            || !self.departure_fuzz_minutes.is_finite()
            || self.departure_fuzz_minutes < 0.0
            || !self.daily_drive_miles_mean.is_finite()
            || self.daily_drive_miles_mean < 0.0
            || !self.daily_drive_miles_stddev.is_finite()
            || self.daily_drive_miles_stddev < 0.0
            || !self.shift_duration_fuzz_minutes.is_finite()
            || self.shift_duration_fuzz_minutes < 0.0
            || self.shift_rotation_days == 0
            || self.shift_on_days == 0
            || self.shift_on_days > self.shift_rotation_days
        {
            return Err(HaresError::Equipment(
                "EV randomness parameters are invalid".to_string(),
            ));
        }

        self.plug_in_policy = PlugInPolicy::from_config(config);
        self.plug_in_soc_threshold = config
            .get_f64(KEY_PLUG_IN_SOC_THRESHOLD)
            .unwrap_or(DEFAULT_PLUG_IN_SOC_THRESHOLD);
        if !self.plug_in_soc_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.plug_in_soc_threshold)
        {
            return Err(HaresError::Equipment(
                "EV plug_in_soc_threshold must be finite and within [0, 1]".to_string(),
            ));
        }
        self.v2l_enabled = config.get_bool(KEY_V2L_ENABLED).unwrap_or(false);
        self.v2l_soc_reserve = config
            .get_f64(KEY_V2L_SOC_RESERVE)
            .unwrap_or(DEFAULT_V2L_SOC_RESERVE);
        if !self.v2l_soc_reserve.is_finite() || !(0.0..=1.0).contains(&self.v2l_soc_reserve) {
            return Err(HaresError::Equipment(
                "EV v2l_soc_reserve must be finite and within [0, 1]".to_string(),
            ));
        }
        self.v2l_max_discharge_kw = config
            .get_f64(KEY_V2L_MAX_DISCHARGE_KW)
            .unwrap_or(DEFAULT_V2L_MAX_DISCHARGE_KW);
        if !self.v2l_max_discharge_kw.is_finite() || self.v2l_max_discharge_kw < 0.0 {
            return Err(HaresError::Equipment(
                "EV v2l_max_discharge_kw must be finite and >= 0".to_string(),
            ));
        }
        self.v2g_enabled = config.get_bool(KEY_V2G_ENABLED).unwrap_or(false);
        self.v2g_soc_reserve = config
            .get_f64(KEY_V2G_SOC_RESERVE)
            .unwrap_or(DEFAULT_V2G_SOC_RESERVE);
        if !self.v2g_soc_reserve.is_finite() || !(0.0..=1.0).contains(&self.v2g_soc_reserve) {
            return Err(HaresError::Equipment(
                "EV v2g_soc_reserve must be finite and within [0, 1]".to_string(),
            ));
        }
        self.v2g_max_discharge_kw = config
            .get_f64(KEY_V2G_MAX_DISCHARGE_KW)
            .unwrap_or(DEFAULT_V2G_MAX_DISCHARGE_KW);
        if !self.v2g_max_discharge_kw.is_finite() || self.v2g_max_discharge_kw < 0.0 {
            return Err(HaresError::Equipment(
                "EV v2g_max_discharge_kw must be finite and >= 0".to_string(),
            ));
        }

        self.distributions = parse_distribution_rows(config, self.driver_archetype)?;
        if self.distributions.is_empty() {
            return Err(HaresError::Equipment(
                "EV requires at least one schedule distribution row".to_string(),
            ));
        }

        self.soc = config
            .get_f64(KEY_INITIAL_SOC)
            .unwrap_or(self.soc)
            .clamp(0.0, 1.0);
        self.connected = false;
        self.heater_active = false;
        self.battery_temp_c = config
            .get_f64(KEY_BATTERY_TEMP_C)
            .unwrap_or(self.battery_temp_c);
        self.active_power_kw = 0.0;
        self.time_until_departure_s = 0.0;
        self.current_day_ordinal = None;
        self.current_day_drive_miles = 0.0;
        self.todays_event = None;
        self.arrival_soc_applied = false;

        self.rng_seed = derive_rng_seed(config);
        self.rng_draws = 0;
        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        self.shift_phase_days =
            (self.random_unit() * f64::from(self.shift_rotation_days)).floor() as u32;

        self.power_setpoint_kw = None;
        self.soc_target = None;
        self.soc_target_min = None;
        self.soc_target_max = None;
        self.telemetry = default_telemetry(self.charging_level);
        self.write_telemetry();

        Ok(())
    }

    fn random_unit(&mut self) -> f64 {
        self.rng_draws = self.rng_draws.saturating_add(1);
        self.rng.random::<f64>()
    }

    fn maybe_roll_event_for_day(&mut self, now: DateTime<FixedOffset>) {
        let day_ordinal = now.date_naive().num_days_from_ce();
        if self.current_day_ordinal == Some(day_ordinal) {
            return;
        }

        self.current_day_ordinal = Some(day_ordinal);
        self.arrival_soc_applied = false;

        let weekday = now.weekday().number_from_monday();
        let is_weekend = weekday >= 6;

        let mut day_ratio = self.event_day_ratio;
        match self.driver_archetype {
            DriverArchetype::WorkFromHome => {
                day_ratio *= if is_weekend { 0.85 } else { 0.7 };
            }
            DriverArchetype::ShiftWorker => {
                if !self.is_shift_on_day(day_ordinal) {
                    day_ratio *= 0.35;
                }
            }
            DriverArchetype::WeekendWarrior => {
                day_ratio *= if is_weekend { 1.2 } else { 0.6 };
            }
            DriverArchetype::SeniorRetiree => {
                day_ratio *= 0.75;
            }
            DriverArchetype::SchoolRunFamily => {
                day_ratio *= if is_weekend { 0.55 } else { 1.1 };
            }
            DriverArchetype::SingleCarSharedHousehold => {
                day_ratio *= 1.15;
            }
            DriverArchetype::Commuter => {}
        }
        day_ratio = day_ratio.clamp(0.0, 1.0);

        if self.random_unit() > day_ratio {
            self.todays_event = None;
            return;
        }

        self.current_day_drive_miles = self.sample_daily_drive_miles(is_weekend);

        self.todays_event = sample_distribution(
            &mut self.rng,
            &mut self.rng_draws,
            &self.distributions,
        )
        .map(|row| {
            let arrival_jitter_min = self.sample_symmetric_minutes(self.arrival_fuzz_minutes);
            let mut duration_jitter_min =
                self.sample_symmetric_minutes(self.departure_fuzz_minutes);
            if matches!(self.driver_archetype, DriverArchetype::ShiftWorker) {
                duration_jitter_min +=
                    self.sample_symmetric_minutes(self.shift_duration_fuzz_minutes);
            }

            let start_min =
                (i64::from(row.arrival_minute) + arrival_jitter_min).clamp(0, 24 * 60 - 1);
            let duration_min = (i64::from(row.duration_minutes) + duration_jitter_min).max(15);
            let start = start_min * 60;
            let end = (start + duration_min * 60).min(24 * 60 * 60);

            let trip_kwh = self.current_day_drive_miles * EV_FUEL_ECONOMY_KWH_PER_MI;
            let soc_after_driving =
                (self.soc - trip_kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);
            let start_soc = row.start_soc.min(soc_after_driving).clamp(0.0, 1.0);

            SampledEvent {
                start_second_of_day: start,
                end_second_of_day: end,
                start_soc,
            }
        });
    }

    fn sample_symmetric_minutes(&mut self, max_abs_minutes: f64) -> i64 {
        if max_abs_minutes <= 0.0 {
            return 0;
        }
        let draw = self.random_unit();
        ((draw * 2.0 - 1.0) * max_abs_minutes).round() as i64
    }

    fn sample_daily_drive_miles(&mut self, is_weekend: bool) -> f64 {
        let weekend_multiplier = match self.driver_archetype {
            DriverArchetype::WeekendWarrior => {
                if is_weekend {
                    1.8
                } else {
                    0.6
                }
            }
            DriverArchetype::SchoolRunFamily => {
                if is_weekend {
                    0.7
                } else {
                    1.1
                }
            }
            DriverArchetype::SeniorRetiree => 0.75,
            _ => 1.0,
        };

        let mean = self.daily_drive_miles_mean * weekend_multiplier;
        if self.daily_drive_miles_stddev <= 0.0 {
            return mean.max(0.0);
        }
        let u1 = self.random_unit().clamp(1e-12, 1.0);
        let u2 = self.random_unit();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        (mean + self.daily_drive_miles_stddev * z).max(0.0)
    }

    fn is_shift_on_day(&self, day_ordinal: i32) -> bool {
        let cycle_day =
            ((day_ordinal.max(0) as u32) + self.shift_phase_days) % self.shift_rotation_days;
        cycle_day < self.shift_on_days
    }

    fn update_connection(&mut self, now: DateTime<FixedOffset>) {
        let sec_of_day = i64::from(now.num_seconds_from_midnight());
        let mut connected = false;
        let mut time_until_departure_s = 0.0;

        if let Some(event) = self.todays_event {
            let in_window =
                sec_of_day >= event.start_second_of_day && sec_of_day < event.end_second_of_day;
            if in_window {
                // Under LowSoc policy, skip connecting when SOC is above threshold
                let policy_blocks = self.plug_in_policy == PlugInPolicy::LowSoc
                    && self.soc >= self.plug_in_soc_threshold;
                connected = !policy_blocks;
                if connected {
                    if !self.arrival_soc_applied {
                        self.soc = event.start_soc;
                        self.arrival_soc_applied = true;
                    }
                    time_until_departure_s = (event.end_second_of_day - sec_of_day).max(0) as f64;
                }
            }
        }

        self.connected = connected;
        self.time_until_departure_s = time_until_departure_s;
    }

    fn effective_soc_limit(&self) -> f64 {
        let mut limit = self.soc_target.unwrap_or(self.soc_max);
        if let Some(min) = self.soc_target_min {
            limit = limit.max(min);
        }
        if let Some(max) = self.soc_target_max {
            limit = limit.min(max);
        }
        limit.clamp(0.0, self.soc_max)
    }

    /// Compute grid-side charging power for this timestep.
    ///
    /// `charge_derate` is applied before the taper limit so that the taper
    /// correctly prevents SOC overshoot even under cold-temperature derating.
    /// Without this, the underated taper limit could allow more DC energy
    /// than the battery can accept at the derated rate.
    fn compute_power_kw_with_time(
        &self,
        now: DateTime<FixedOffset>,
        dt: Duration,
        charge_derate: f64,
    ) -> f64 {
        if !self.connected {
            return 0.0;
        }

        // V2L/V2G discharge path: negative setpoint with discharge enabled
        if let Some(setpoint) = self.power_setpoint_kw {
            if setpoint < 0.0 {
                if self.v2g_enabled {
                    return self.compute_v2g_discharge(dt);
                } else if self.v2l_enabled {
                    return self.compute_v2l_discharge(dt);
                }
            }
        }

        let soc_limit = self.effective_soc_limit();
        if self.soc >= soc_limit {
            return 0.0;
        }

        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let rated = match self.charging_level {
            ChargingLevel::L1 => self.l1_power_kw(),
            ChargingLevel::L2 => self.rated_power_kw,
        };
        let curve_limited_rated = if let Some(lut) = &self.charging_curve_lut {
            rated * lut.power_fraction_at(self.soc)
        } else {
            rated
        };
        // Apply temperature derate to the rated grid-side power ceiling before
        // computing the taper limit, so both bounds use the same effective power.
        let derated_rated = curve_limited_rated * charge_derate;

        let mut requested = self
            .power_setpoint_kw
            .unwrap_or(derated_rated)
            .max(0.0)
            .min(derated_rated);
        let ready_kw = self.required_ready_power_kw(now, dt, soc_limit);
        let blocked = self.delay_hold_active(now) || self.in_tou_block_window(now);
        if blocked {
            if ready_kw > 0.0 {
                requested = requested.max(ready_kw);
            } else {
                requested = 0.0;
            }
        } else if ready_kw > 0.0 {
            requested = requested.max(ready_kw);
        }

        // Taper limit uses derated power as the effective DC ceiling to prevent
        // SOC overshoot when charge rate is reduced by temperature derating.
        let taper_limit = (soc_limit - self.soc).max(0.0) * self.battery_capacity_kwh
            / dt_hours
            / self.charging_efficiency;

        let mut power = requested.min(derated_rated).min(taper_limit).max(0.0);
        if let Some(limit) = self.power_limit_kw {
            power = power.min(limit.max(0.0));
        }
        power
    }

    /// Compute V2L (vehicle-to-load) discharge power as a negative value.
    /// Respects SOC reserve and max discharge limits.
    fn compute_v2l_discharge(&self, dt: Duration) -> f64 {
        if self.soc <= self.v2l_soc_reserve {
            return 0.0;
        }
        let setpoint_magnitude = self.power_setpoint_kw.unwrap_or(0.0).abs();
        let capped = setpoint_magnitude.min(self.v2l_max_discharge_kw);

        // Prevent discharging below the SOC reserve in this timestep
        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let available_kwh = (self.soc - self.v2l_soc_reserve) * self.battery_capacity_kwh;
        let max_discharge_kw = available_kwh / dt_hours;

        -(capped.min(max_discharge_kw).max(0.0))
    }

    /// Compute V2G (vehicle-to-grid) discharge power as a negative value.
    /// Uses V2G-specific SOC reserve (higher than V2L) and max discharge limit.
    fn compute_v2g_discharge(&self, dt: Duration) -> f64 {
        if self.soc <= self.v2g_soc_reserve {
            return 0.0;
        }
        let setpoint_magnitude = self.power_setpoint_kw.unwrap_or(0.0).abs();
        let capped = setpoint_magnitude.min(self.v2g_max_discharge_kw);

        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let available_kwh = (self.soc - self.v2g_soc_reserve) * self.battery_capacity_kwh;
        let max_discharge_kw = available_kwh / dt_hours;

        -(capped.min(max_discharge_kw).max(0.0))
    }

    fn charge_derate_factor(&self) -> f64 {
        if self.battery_temp_c <= self.min_charge_temp_c {
            0.0
        } else if self.battery_temp_c >= self.full_power_temp_c {
            1.0
        } else {
            let span = self.full_power_temp_c - self.min_charge_temp_c;
            if span <= f64::EPSILON {
                1.0
            } else {
                (self.battery_temp_c - self.min_charge_temp_c) / span
            }
        }
    }

    fn l1_power_kw(&self) -> f64 {
        match self.l1_current_a {
            Some(current_a) => (current_a * self.l1_voltage_v / 1_000.0).max(0.0),
            None => self.rated_power_kw,
        }
    }

    fn delay_hold_active(&self, now: DateTime<FixedOffset>) -> bool {
        let Some(delay_until_hour) = self.delay_until_hour else {
            return false;
        };
        let Some(immediate_target_soc) = self.immediate_target_soc else {
            return false;
        };
        if self.soc < immediate_target_soc {
            return false;
        }
        let now_sec = i64::from(now.num_seconds_from_midnight());
        let release_sec = (delay_until_hour * SECONDS_PER_HOUR).round() as i64;
        now_sec < release_sec
    }

    fn in_tou_block_window(&self, now: DateTime<FixedOffset>) -> bool {
        if !self.tou_avoid_peak {
            return false;
        }
        let (Some(start_h), Some(end_h)) = (self.tou_peak_start_hour, self.tou_peak_end_hour)
        else {
            return false;
        };
        let now_sec = i64::from(now.num_seconds_from_midnight());
        let start_sec = (start_h * SECONDS_PER_HOUR).round() as i64;
        let end_sec = (end_h * SECONDS_PER_HOUR).round() as i64;
        if start_sec == end_sec {
            return false;
        }
        if start_sec < end_sec {
            now_sec >= start_sec && now_sec < end_sec
        } else {
            // Wrap-around window (e.g. 22:00 to 06:00)
            now_sec >= start_sec || now_sec < end_sec
        }
    }

    fn required_ready_power_kw(&self, now: DateTime<FixedOffset>, dt: Duration, soc_limit: f64) -> f64 {
        let Some(ready_by_hour) = self.ready_by_hour else {
            return 0.0;
        };
        let target_soc = self
            .ready_target_soc
            .unwrap_or(soc_limit)
            .max(soc_limit)
            .clamp(0.0, self.soc_max);
        if self.soc >= target_soc {
            return 0.0;
        }

        let now_sec = i64::from(now.num_seconds_from_midnight());
        let target_sec = (ready_by_hour * SECONDS_PER_HOUR).round() as i64;
        let remaining_sec = if now_sec < target_sec {
            target_sec - now_sec
        } else {
            SECONDS_PER_DAY - now_sec + target_sec
        };

        let min_dt_sec = dt.as_secs_f64().max(1.0);
        let remaining_hours =
            (remaining_sec as f64 / SECONDS_PER_HOUR).max(min_dt_sec / SECONDS_PER_HOUR);
        let dc_required_kw = (target_soc - self.soc) * self.battery_capacity_kwh / remaining_hours;
        (dc_required_kw / self.charging_efficiency).max(0.0)
    }

    fn write_telemetry(&mut self) {
        self.telemetry.set("soc", self.soc);
        self.telemetry.set("active_power_kw", self.active_power_kw);
        self.telemetry
            .set("is_connected", if self.connected { 1.0 } else { 0.0 });
        self.telemetry
            .set("charging_level", self.charging_level.telemetry_code());
        self.telemetry
            .set("time_until_departure_s", self.time_until_departure_s);
        self.telemetry.set("battery_temp_c", self.battery_temp_c);
        self.telemetry.set(
            "heater_power_w",
            if self.heater_active {
                self.heater_power_w
            } else {
                0.0
            },
        );
        self.telemetry
            .set("charge_derate", self.charge_derate_factor());
        self.telemetry
            .set("daily_drive_miles", self.current_day_drive_miles);
        self.telemetry
            .set("v2l_active", if self.v2l_active { 1.0 } else { 0.0 });
        self.telemetry.set("v2l_power_kw", self.v2l_power_kw);
    }
}

impl Equipment for Ev {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.init_from_config(config)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        if self.connected && self.active_power_kw > 0.0 {
            OperatingMode::Charging
        } else {
            OperatingMode::Off
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.maybe_roll_event_for_day(env.current_time);
        self.update_connection(env.current_time);

        let charge_derate = self.charge_derate_factor();
        let charger_kw = self.compute_power_kw_with_time(env.current_time, dt, charge_derate);

        let is_v2l_discharge = charger_kw < 0.0;
        self.v2l_active = is_v2l_discharge;
        self.v2l_power_kw = if is_v2l_discharge {
            charger_kw.abs()
        } else {
            0.0
        };

        // Heater intent is checked against pre-derate conditions: the pack heater
        // should activate whenever the vehicle is connected with SOC headroom and
        // the battery is below the heater threshold, regardless of charge_derate.
        // This enables "pre-heat before charge" when the pack is fully cold-derated.
        let would_charge_underated =
            self.connected && !is_v2l_discharge && self.soc < self.effective_soc_limit();
        self.heater_active = self.heater_power_w > 0.0
            && would_charge_underated
            && self.battery_temp_c <= self.heater_threshold_c;
        let heater_kw = if self.heater_active {
            self.heater_power_w / 1000.0
        } else {
            0.0
        };

        self.active_power_kw = if self.connected {
            if is_v2l_discharge {
                charger_kw // negative = generation
            } else {
                charger_kw + heater_kw
            }
        } else {
            0.0
        };

        // Passive thermal decay toward ambient via Newton's law of cooling.
        // Runs unconditionally so the pack temperature evolves even when
        // disconnected or cold-derated to zero charging power.
        // Reference: lumped-capacitance thermal model, Incropera & DeWitt Ch. 5.
        let ambient_c = env.weather.outdoor_temp_c;
        let dt_s = dt.as_secs_f64();
        let q_loss_w = self.ua_w_per_k * (self.battery_temp_c - ambient_c);
        self.battery_temp_c -= (q_loss_w * dt_s) / self.thermal_mass_j_per_k;

        if is_v2l_discharge {
            // V2L discharge: negative active_power_kw = generation
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: self.active_power_kw,
                reactive_power_kvar: 0.0,
            })?;
            let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
            let dc_discharge_kwh = charger_kw.abs() * dt_hours;
            self.soc = (self.soc - dc_discharge_kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);
        } else if self.active_power_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: self.active_power_kw,
                reactive_power_kvar: 0.0,
            })?;
            let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
            let net_charge_kw = (charger_kw - heater_kw).max(0.0);
            let dc_stored_kw = net_charge_kw * self.charging_efficiency;
            let dc_energy_kwh = dc_stored_kw * dt_hours;
            self.soc = (self.soc + dc_energy_kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);

            // Ohmic and heater heat added only during active charging.
            let ohmic_like_heat_w = (net_charge_kw - dc_stored_kw) * 1000.0;
            let heater_w = heater_kw * 1000.0;
            self.battery_temp_c +=
                ((ohmic_like_heat_w + heater_w) * dt_s) / self.thermal_mass_j_per_k;
        }

        if !self.connected {
            self.active_power_kw = 0.0;
            self.heater_active = false;
            self.v2l_active = false;
            self.v2l_power_kw = 0.0;
        }

        self.write_telemetry();
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&EvCheckpoint {
            soc: self.soc,
            connected: self.connected,
            active_power_kw: self.active_power_kw,
            time_until_departure_s: self.time_until_departure_s,
            current_day_ordinal: self.current_day_ordinal,
            todays_event: self.todays_event,
            arrival_soc_applied: self.arrival_soc_applied,
            rng_seed: self.rng_seed,
            rng_draws: self.rng_draws,
            power_limit_kw: self.power_limit_kw,
            power_setpoint_kw: self.power_setpoint_kw,
            soc_target: self.soc_target,
            soc_target_min: self.soc_target_min,
            soc_target_max: self.soc_target_max,
            battery_temp_c: self.battery_temp_c,
            heater_active: self.heater_active,
            current_day_drive_miles: self.current_day_drive_miles,
            shift_phase_days: self.shift_phase_days,
            plug_in_policy: self.plug_in_policy,
            plug_in_soc_threshold: self.plug_in_soc_threshold,
            v2l_enabled: self.v2l_enabled,
            v2l_soc_reserve: self.v2l_soc_reserve,
            v2l_max_discharge_kw: self.v2l_max_discharge_kw,
            v2g_enabled: self.v2g_enabled,
            v2g_soc_reserve: self.v2g_soc_reserve,
            v2g_max_discharge_kw: self.v2g_max_discharge_kw,
            driver_archetype: self.driver_archetype,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        // Do NOT call init() after load_state(). The checkpoint includes all
        // dynamic state (shift_phase_days, rng position, SOC, etc.) needed to
        // resume a simulation. Calling init() would re-randomize shift_phase_days
        // and reset the RNG, breaking checkpoint reproducibility.
        let cp: EvCheckpoint = load_postcard(state)?;
        self.soc = cp.soc;
        self.connected = cp.connected;
        self.active_power_kw = cp.active_power_kw;
        self.time_until_departure_s = cp.time_until_departure_s;
        self.current_day_ordinal = cp.current_day_ordinal;
        self.todays_event = cp.todays_event;
        self.arrival_soc_applied = cp.arrival_soc_applied;
        self.rng_seed = cp.rng_seed;
        self.rng_draws = cp.rng_draws;
        self.power_limit_kw = cp.power_limit_kw;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.soc_target = cp.soc_target;
        self.soc_target_min = cp.soc_target_min;
        self.soc_target_max = cp.soc_target_max;
        self.battery_temp_c = cp.battery_temp_c;
        self.heater_active = cp.heater_active;
        self.current_day_drive_miles = cp.current_day_drive_miles;
        self.shift_phase_days = cp.shift_phase_days;
        self.plug_in_policy = cp.plug_in_policy;
        self.plug_in_soc_threshold = cp.plug_in_soc_threshold;
        self.v2l_enabled = cp.v2l_enabled;
        self.v2l_soc_reserve = cp.v2l_soc_reserve;
        self.v2l_max_discharge_kw = cp.v2l_max_discharge_kw;
        self.v2g_enabled = cp.v2g_enabled;
        self.v2g_soc_reserve = cp.v2g_soc_reserve;
        self.v2g_max_discharge_kw = cp.v2g_max_discharge_kw;
        self.driver_archetype = cp.driver_archetype;

        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        // Each random::<f64>() consumes 2 u32 words from the ChaCha8 stream.
        // Use O(1) seeking instead of replaying draws.
        self.rng.set_word_pos((self.rng_draws as u128) * 2);

        self.write_telemetry();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                if *active_power_kw < 0.0 && !self.v2l_enabled && !self.v2g_enabled {
                    return Err(HaresError::Control(
                        "negative PowerSetpoint requires v2l_enabled or v2g_enabled".to_string(),
                    ));
                }
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "EV PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                self.power_setpoint_kw = Some(*active_power_kw);
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                if !max_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "EV PowerLimit max_power_kw must be finite".to_string(),
                    ));
                }
                self.power_limit_kw = Some((*max_power_kw).max(0.0));
            }
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                if !target_soc.is_finite() {
                    return Err(HaresError::Control(
                        "EV SOCTarget target_soc must be finite".to_string(),
                    ));
                }
                if let Some(min) = min_soc
                    && (!min.is_finite() || !(0.0..=1.0).contains(min))
                {
                    return Err(HaresError::Control(
                        "EV SOCTarget min_soc must be finite and within [0, 1]".to_string(),
                    ));
                }
                if let Some(max) = max_soc
                    && (!max.is_finite() || !(0.0..=1.0).contains(max))
                {
                    return Err(HaresError::Control(
                        "EV SOCTarget max_soc must be finite and within [0, 1]".to_string(),
                    ));
                }
                self.soc_target = Some((*target_soc).clamp(0.0, 1.0));
                self.soc_target_min = *min_soc;
                self.soc_target_max = *max_soc;
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "EV does not handle control signal: {signal:?}"
                )));
            }
        }

        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register("EV", Box::new(|config| Box::new(Ev::new(config))));
}

fn default_telemetry(charging_level: ChargingLevel) -> Telemetry {
    let mut t = Telemetry::with_capacity(11);
    t.insert("soc", DEFAULT_SOC);
    t.insert("active_power_kw", 0.0);
    t.insert("is_connected", 0.0);
    t.insert("charging_level", charging_level.telemetry_code());
    t.insert("time_until_departure_s", 0.0);
    t.insert("battery_temp_c", 20.0);
    t.insert("heater_power_w", 0.0);
    t.insert("charge_derate", 1.0);
    t.insert("daily_drive_miles", 0.0);
    t.insert("v2l_active", 0.0);
    t.insert("v2l_power_kw", 0.0);
    t
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "soc".to_string(),
            unit: "-".to_string(),
            description: "EV battery state of charge [0..1]".to_string(),
        },
        TelemetryField {
            name: "active_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Grid-side EV charging power (positive = load)".to_string(),
        },
        TelemetryField {
            name: "is_connected".to_string(),
            unit: "-".to_string(),
            description: "Connection state (1 connected, 0 disconnected)".to_string(),
        },
        TelemetryField {
            name: "charging_level".to_string(),
            unit: "code".to_string(),
            description: "Charging level code (1=L1, 2=L2)".to_string(),
        },
        TelemetryField {
            name: "time_until_departure_s".to_string(),
            unit: "s".to_string(),
            description: "Remaining connected time until departure".to_string(),
        },
        TelemetryField {
            name: "battery_temp_c".to_string(),
            unit: "C".to_string(),
            description: "EV pack temperature used for cold-charge derating".to_string(),
        },
        TelemetryField {
            name: "heater_power_w".to_string(),
            unit: "W".to_string(),
            description: "Battery heater power draw when active".to_string(),
        },
        TelemetryField {
            name: "charge_derate".to_string(),
            unit: "-".to_string(),
            description: "Temperature-based charging derate factor [0..1]".to_string(),
        },
        TelemetryField {
            name: "daily_drive_miles".to_string(),
            unit: "mi/day".to_string(),
            description: "Sampled daily drive miles used to fuzz EV arrival SOC".to_string(),
        },
        TelemetryField {
            name: "v2l_active".to_string(),
            unit: "-".to_string(),
            description: "V2L discharge active (1 discharging, 0 not)".to_string(),
        },
        TelemetryField {
            name: "v2l_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "V2L discharge power magnitude".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, PortSlots, SurfaceIrradiance, WeatherState,
        ZoneState,
    };

    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(y, mo, d, h, mi, s)
            .unwrap()
    }

    fn sample_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: hares_types::ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 220.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 10.0,
                outdoor_enthalpy_j_kg: 0.0,
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
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: dt(2026, 1, 1, 0, 0, 0),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn ev_config(raw: HashMap<String, crate::config::ConfigValue>) -> EquipmentConfig {
        EquipmentConfig {
            name: "EV #1".to_string(),
            ochre_class: "EV".to_string(),
            raw_config: raw,
        }
    }

    fn base_raw() -> HashMap<String, crate::config::ConfigValue> {
        let mut raw = HashMap::new();
        raw.insert(KEY_MASTER_SEED.to_string(), 1234.0.into());
        raw.insert(KEY_BUILDING_ID.to_string(), 77.0.into());
        raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), 60.0.into());
        raw.insert(KEY_CHARGING_LEVEL.to_string(), "L2".into());
        raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 7.2.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
        raw.insert(KEY_EVENT_DAY_RATIO.to_string(), 1.0.into());
        raw.insert(KEY_SCHEDULE_LEN.to_string(), 1.0.into());
        raw.insert(
            "schedule_arrival_minute_0".to_string(),
            (18.0 * 60.0).into(),
        );
        raw.insert(
            "schedule_duration_minute_0".to_string(),
            (10.0 * 60.0).into(),
        );
        raw.insert("schedule_start_soc_0".to_string(), 0.2.into());
        raw.insert("schedule_weight_0".to_string(), 1.0.into());
        raw
    }

    #[test]
    fn no_power_when_disconnected() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 12, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

        assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
        assert_eq!(ev.telemetry().get("is_connected"), Some(0.0));
        assert_eq!(ports.electrical.load_power_kw, 0.0);
    }

    #[test]
    fn l1_default_power_is_1p4_kw() {
        // When no max_charging_power_kw is configured for L1, the default is 1.4 kW.
        let mut raw = base_raw();
        raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
        raw.remove(KEY_MAX_CHARGING_POWER_KW);
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

        let p = ev.telemetry().get("active_power_kw").unwrap();
        assert!((p - 1.4).abs() < 1e-12);
    }

    #[test]
    fn l2_constant_power_and_linear_taper() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let p_low_soc = ev.telemetry().get("active_power_kw").unwrap();
        assert!((p_low_soc - 7.2).abs() < 1e-9);

        ev.soc = 0.99;
        env.current_time = env.current_time + ChronoDuration::minutes(1);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let p_taper = ev.telemetry().get("active_power_kw").unwrap();
        assert!(p_taper > 0.0);
        assert!(p_taper < 7.2);

        ev.soc = 1.0;
        env.current_time = env.current_time + ChronoDuration::minutes(1);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let p_full = ev.telemetry().get("active_power_kw").unwrap();
        assert_eq!(p_full, 0.0);
    }

    #[test]
    fn negative_power_setpoint_rejected_without_v2l_or_v2g() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let env = sample_env();
        ev.init(&config, &env).unwrap();

        let err = ev
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: -1.0,
                reactive_power_kvar: None,
            })
            .unwrap_err();

        assert!(
            err.to_string().contains("v2l_enabled or v2g_enabled"),
            "expected rejection message, got: {err}"
        );
    }

    #[test]
    fn deterministic_for_same_seed_and_building_id() {
        let config = ev_config(base_raw());
        let mut a = Ev::new(config.clone());
        let mut b = Ev::new(config.clone());
        let mut env = sample_env();
        a.init(&config, &env).unwrap();
        b.init(&config, &env).unwrap();

        for _ in 0..200 {
            let mut pa = PortSlots::default();
            let mut pb = PortSlots::default();
            a.step(&env, Duration::minutes(15), &mut pa).unwrap();
            b.step(&env, Duration::minutes(15), &mut pb).unwrap();
            assert_eq!(a.telemetry().get("soc"), b.telemetry().get("soc"));
            assert_eq!(
                a.telemetry().get("active_power_kw"),
                b.telemetry().get("active_power_kw")
            );
            env.current_time = env.current_time + ChronoDuration::minutes(15);
        }
    }

    #[test]
    fn two_instances_with_different_seeds_evolve_independently() {
        let config_a = ev_config(base_raw());
        let mut raw_b = base_raw();
        raw_b.insert(KEY_BUILDING_ID.to_string(), 78.0.into());
        raw_b.insert("schedule_start_soc_0".to_string(), 0.8.into());
        let config_b = ev_config(raw_b);

        let mut a = Ev::new(config_a.clone());
        let mut b = Ev::new(config_b.clone());
        let mut env = sample_env();
        a.init(&config_a, &env).unwrap();
        b.init(&config_b, &env).unwrap();

        let mut diverged = false;
        for _ in 0..120 {
            let mut pa = PortSlots::default();
            let mut pb = PortSlots::default();
            a.step(&env, Duration::minutes(10), &mut pa).unwrap();
            b.step(&env, Duration::minutes(10), &mut pb).unwrap();
            if a.telemetry().get("soc") != b.telemetry().get("soc") {
                diverged = true;
                break;
            }
            env.current_time = env.current_time + ChronoDuration::minutes(10);
        }

        assert!(diverged);
    }

    #[test]
    fn save_load_preserves_trajectory() {
        let config = ev_config(base_raw());
        let mut env = sample_env();
        let mut a = Ev::new(config.clone());
        let mut b = Ev::new(config.clone());

        a.init(&config, &env).unwrap();
        b.init(&config, &env).unwrap();

        for _ in 0..50 {
            let mut ports = PortSlots::default();
            a.step(&env, Duration::minutes(5), &mut ports).unwrap();
            env.current_time = env.current_time + ChronoDuration::minutes(5);
        }

        let checkpoint = a.save_state();
        b.load_state(&checkpoint).unwrap();

        for _ in 0..120 {
            let mut pa = PortSlots::default();
            let mut pb = PortSlots::default();
            a.step(&env, Duration::minutes(5), &mut pa).unwrap();
            b.step(&env, Duration::minutes(5), &mut pb).unwrap();
            assert_eq!(a.telemetry().get("soc"), b.telemetry().get("soc"));
            assert_eq!(
                a.telemetry().get("active_power_kw"),
                b.telemetry().get("active_power_kw")
            );
            env.current_time = env.current_time + ChronoDuration::minutes(5);
        }
    }

    #[test]
    fn hpxml_range_to_capacity_derivation_uses_verified_constant() {
        let mut raw = base_raw();
        raw.remove(KEY_BATTERY_CAPACITY_KWH);
        raw.insert(KEY_RANGE_MILES.to_string(), 200.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        assert!((ev.battery_capacity_kwh - 65.0).abs() < 1e-9);
    }

    trait DurationExt {
        fn minutes(n: u64) -> Duration;
    }

    impl DurationExt for Duration {
        fn minutes(n: u64) -> Duration {
            Duration::from_secs(n * 60)
        }
    }

    #[test]
    fn update_control_reports_charging_when_connected_and_drawing_power() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();
        env.current_time = dt(2026, 1, 1, 18, 0, 0);

        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        assert_eq!(ev.update_control(&env), OperatingMode::Charging);
    }

    #[test]
    fn registry_registration_creates_ev() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);

        let eq = registry
            .create("EV", ev_config(base_raw()))
            .expect("EV registered");
        assert_eq!(eq.descriptor().equipment_type, "EV");
        assert_eq!(eq.descriptor().stage, ExecutionStage::Electrical);
    }

    #[test]
    fn csv_distribution_parses_start_soc_percent() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("hares_ev_schedule_test.csv");
        let data = "start_time,duration,start_soc,weight\n1080,480,40,1\n";
        std::fs::write(&path, data).unwrap();

        let mut raw = base_raw();
        raw.insert(
            KEY_SCHEDULE_CSV_PATH.to_string(),
            path.to_string_lossy().to_string().into(),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        assert_eq!(ev.distributions.len(), 1);
        assert!((ev.distributions[0].start_soc - 0.4).abs() < 1e-12);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn time_until_departure_reports_remaining_seconds() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        let remaining = ev.telemetry().get("time_until_departure_s").unwrap();
        assert!(remaining > 0.0);
        assert!(remaining <= SECONDS_PER_DAY as f64);
    }

    #[test]
    fn apply_soc_target_limits_charging() {
        let config = ev_config(base_raw());
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        ev.apply_control(&ControlSignal::SOCTarget {
            target_soc: 0.3,
            min_soc: None,
            max_soc: Some(0.3),
        })
        .unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        for _ in 0..100 {
            let mut ports = PortSlots::default();
            ev.step(&env, Duration::minutes(5), &mut ports).unwrap();
            env.current_time = env.current_time + ChronoDuration::minutes(5);
        }

        assert!(ev.soc <= 0.301);
    }

    #[test]
    fn supports_hpxml_key_aliases() {
        let mut raw = base_raw();
        raw.remove(KEY_BATTERY_CAPACITY_KWH);
        raw.remove(KEY_MAX_CHARGING_POWER_KW);
        raw.remove(KEY_CHARGING_LEVEL);
        raw.insert(KEY_BATTERY_CAPACITY_HPIXML_KWH.to_string(), 64.0.into());
        raw.insert(KEY_MAX_CHARGING_POWER_HPIXML_KW.to_string(), 11.5.into());
        raw.insert(KEY_CHARGING_LEVEL_HPIXML.to_string(), "Level2".into());

        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        assert!((ev.battery_capacity_kwh - 64.0).abs() < 1e-9);
        assert!((ev.rated_power_kw - 11.5).abs() < 1e-9);
        assert_eq!(ev.charging_level, ChargingLevel::L2);
    }

    #[test]
    fn parse_distribution_rejects_invalid_soc() {
        let mut raw = base_raw();
        raw.insert("schedule_start_soc_0".to_string(), 1.5.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let err = ev.init(&config, &sample_env()).unwrap_err();
        assert!(err.to_string().contains("start_soc"));
    }

    #[test]
    fn l1_8amp_mode_is_supported() {
        let mut raw = base_raw();
        raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
        raw.insert(KEY_L1_CURRENT_A.to_string(), 8.0.into());
        raw.insert(KEY_L1_VOLTAGE_V.to_string(), 120.0.into());
        let config = ev_config(raw);

        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let p = ev.telemetry().get("active_power_kw").unwrap();
        assert!((p - 0.96).abs() < 1e-12);
    }

    #[test]
    fn tou_window_blocks_charging() {
        let mut raw = base_raw();
        raw.insert(KEY_TOU_AVOID_PEAK.to_string(), true.into());
        raw.insert(KEY_TOU_PEAK_START_HOUR.to_string(), 17.0.into());
        raw.insert(KEY_TOU_PEAK_END_HOUR.to_string(), 21.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(30), &mut ports).unwrap();
        assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
    }

    #[test]
    fn ready_by_can_override_tou_block() {
        let mut raw = base_raw();
        raw.insert(KEY_TOU_AVOID_PEAK.to_string(), true.into());
        raw.insert(KEY_TOU_PEAK_START_HOUR.to_string(), 17.0.into());
        raw.insert(KEY_TOU_PEAK_END_HOUR.to_string(), 23.0.into());
        raw.insert(KEY_READY_BY_HOUR.to_string(), 20.0.into());
        raw.insert(KEY_READY_TARGET_SOC.to_string(), 0.9.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.2.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 19, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(30), &mut ports).unwrap();

        assert!(ev.telemetry().get("active_power_kw").unwrap() > 0.0);
    }

    #[test]
    fn pybamm_csv_lut_limits_power() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("hares_ev_pybamm_lut_test.csv");
        let data = "soc,power_fraction\n0.0,1.0\n0.5,0.5\n1.0,0.0\n";
        std::fs::write(&path, data).unwrap();

        let mut raw = base_raw();
        raw.insert(
            KEY_PYBAMM_LUT_PATH.to_string(),
            path.to_string_lossy().to_string().into(),
        );
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 0.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let power_kw = ev.telemetry().get("active_power_kw").unwrap();
        assert!(power_kw <= 3.6 + 1e-9);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cold_temperature_blocks_charging_without_heater() {
        let mut raw = base_raw();
        raw.insert(
            KEY_BATTERY_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-5.0),
        );
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        // Disable thermal exchange so temperature stays at -5°C throughout the
        // step; this isolates the charge-blocking logic from thermal dynamics.
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(30), &mut ports).unwrap();
        assert_eq!(ev.telemetry().get("active_power_kw"), Some(0.0));
        assert_eq!(ev.telemetry().get("charge_derate"), Some(0.0));
    }

    #[test]
    fn heater_draw_slows_soc_gain() {
        let mut raw = base_raw();
        raw.insert(
            KEY_BATTERY_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-1.0),
        );
        raw.insert(
            KEY_MIN_CHARGE_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-2.0),
        );
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 1200.0.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();
        let soc_before = ev.soc;

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let soc_delta_with_heater = ev.soc - soc_before;

        let mut raw_no_heater = base_raw();
        raw_no_heater.insert(
            KEY_BATTERY_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-1.0),
        );
        raw_no_heater.insert(
            KEY_MIN_CHARGE_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-2.0),
        );
        raw_no_heater.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        let config_no_heater = ev_config(raw_no_heater);
        let mut ev_no_heater = Ev::new(config_no_heater.clone());
        ev_no_heater.init(&config_no_heater, &sample_env()).unwrap();
        let soc_before_no_heater = ev_no_heater.soc;
        let mut ports = PortSlots::default();
        ev_no_heater
            .step(&env, Duration::minutes(60), &mut ports)
            .unwrap();
        let soc_delta_no_heater = ev_no_heater.soc - soc_before_no_heater;

        assert!(soc_delta_with_heater < soc_delta_no_heater);
        assert!(ev.telemetry().get("heater_power_w").unwrap() > 0.0);
    }

    #[test]
    fn archetype_parsing_includes_new_variants() {
        let mut school_raw = base_raw();
        school_raw.insert(KEY_DRIVER_ARCHETYPE.to_string(), "school_run_family".into());
        let school = Ev::new(ev_config(school_raw));
        assert_eq!(school.driver_archetype, DriverArchetype::SchoolRunFamily);

        let mut shared_raw = base_raw();
        shared_raw.insert(
            KEY_DRIVER_ARCHETYPE.to_string(),
            "single_car_shared_household".into(),
        );
        let shared = Ev::new(ev_config(shared_raw));
        assert_eq!(
            shared.driver_archetype,
            DriverArchetype::SingleCarSharedHousehold
        );
    }

    #[test]
    fn school_and_shared_archetypes_have_distinct_default_profiles() {
        let school = default_distribution(DriverArchetype::SchoolRunFamily);
        assert_eq!(school.len(), 2);
        assert!(school.iter().any(|r| r.arrival_minute < 12 * 60));
        assert!(school.iter().any(|r| r.arrival_minute > 14 * 60));

        let shared = default_distribution(DriverArchetype::SingleCarSharedHousehold);
        assert_eq!(shared.len(), 2);
        let spread_min = (i32::from(shared[0].arrival_minute)
            - i32::from(shared[1].arrival_minute))
        .unsigned_abs();
        assert!(spread_min >= 120);
    }

    #[test]
    fn weekend_warrior_weekend_miles_exceed_weekday_when_stddev_zero() {
        let mut raw = base_raw();
        raw.insert(KEY_DRIVER_ARCHETYPE.to_string(), "weekend_warrior".into());
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 20.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        raw.remove(KEY_SCHEDULE_LEN);
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        let weekday = ev.sample_daily_drive_miles(false);
        let weekend = ev.sample_daily_drive_miles(true);
        assert!(weekend > weekday);
    }

    #[test]
    fn ohmic_heat_nonzero_during_active_charging() {
        // Charging efficiency < 1 means net_charge_kw > dc_stored_kw, so ohmic heat > 0.
        // Heater is off (battery_temp_c >> heater_threshold_c), so all heat comes from losses.
        let mut raw = base_raw();
        raw.insert(KEY_EFFICIENCY.to_string(), 0.85.into());
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 0.0.into());
        raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into()); // disable heat loss to isolate heat gain
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        let temp_before = ev.battery_temp_c;
        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

        let power_kw = ev.telemetry().get("active_power_kw").unwrap();
        // EV is connected and drawing power at 7.2 kW
        assert!(power_kw > 0.0, "EV should be charging");
        // With efficiency=0.85, losses = 0.15 * charger_kw, battery should heat up
        assert!(
            ev.battery_temp_c > temp_before,
            "battery should warm from ohmic losses (efficiency < 1)"
        );
    }

    #[test]
    fn checkpoint_restore_preserves_current_day_drive_miles() {
        let mut raw = base_raw();
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 40.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        // Advance to a time that triggers event rolling (mid-day after event start)
        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        // current_day_drive_miles is set when an event is rolled for the day
        let miles_before = ev.current_day_drive_miles;

        let checkpoint = ev.save_state();

        let mut ev2 = Ev::new(config.clone());
        ev2.init(&config, &sample_env()).unwrap();
        ev2.load_state(&checkpoint).unwrap();

        assert_eq!(
            ev2.current_day_drive_miles, miles_before,
            "load_state must restore current_day_drive_miles"
        );
    }

    #[test]
    fn l1_configured_power_is_respected_not_hardcoded() {
        // Configured 1.8 kW (max L1 allowed) should be used, not the 1.4 kW hardcoded default.
        let mut raw = base_raw();
        raw.insert(KEY_CHARGING_LEVEL.to_string(), "L1".into());
        raw.insert(KEY_MAX_CHARGING_POWER_KW.to_string(), 1.8.into());
        raw.remove(KEY_L1_CURRENT_A); // ensure current-based path is not taken
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        assert!(
            (ev.rated_power_kw - 1.8).abs() < 1e-9,
            "L1 rated_power_kw should reflect configured max_charging_power_kw=1.8, got {}",
            ev.rated_power_kw
        );

        // Step at connection time and verify 1.8 kW is the actual power drawn
        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();
        let p = ev.telemetry().get("active_power_kw").unwrap();
        assert!(
            (p - 1.8).abs() < 1e-9,
            "L1 step power should be 1.8 kW when configured, got {}",
            p
        );
    }

    #[test]
    fn thermal_decay_occurs_when_disconnected() {
        // Battery starts above ambient; pack must cool toward ambient over 100 steps
        // even with no charging activity (disconnected).
        let mut raw = base_raw();
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 30.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 4.0.into());
        raw.insert(KEY_THERMAL_MASS_J_PER_K.to_string(), 20_000.0.into());
        // EV is disconnected at noon; base schedule has event starting at 18:00.
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        env.weather.outdoor_temp_c = 0.0;
        ev.init(&config, &env).unwrap();

        // Step at 12:00 for 100 minutes; EV is disconnected, pack should cool.
        env.current_time = dt(2026, 1, 1,12, 0, 0);
        for _ in 0..100 {
            let mut ports = PortSlots::default();
            ev.step(&env, Duration::minutes(1), &mut ports).unwrap();
            env.current_time = env.current_time + ChronoDuration::minutes(1);
        }

        assert!(
            ev.battery_temp_c < 30.0,
            "battery should cool toward ambient when disconnected, got {}°C",
            ev.battery_temp_c
        );
        assert_eq!(
            ev.telemetry().get("active_power_kw"),
            Some(0.0),
            "no grid power when disconnected"
        );
    }

    #[test]
    fn heater_only_grid_draw_when_fully_cold_derated() {
        // When battery_temp_c <= min_charge_temp_c, charge_derate = 0 so
        // no charging power flows to the battery. However if a heater is configured
        // and the vehicle is connected and "wants" to charge, the heater should
        // still draw grid power (pre-heat before charge behavior).
        // SOC must remain unchanged since no DC energy reaches the battery.
        let mut raw = base_raw();
        raw.insert(
            KEY_BATTERY_TEMP_C.to_string(),
            crate::config::ConfigValue::Float(-5.0),
        );
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 500.0.into());
        // Zero driving so the arrival SOC equals initial SOC (no trip drain).
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 0.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        // Heater threshold above battery temp so heater activates.
        raw.insert(
            KEY_HEATER_THRESHOLD_C.to_string(),
            crate::config::ConfigValue::Float(-4.0),
        );
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();
        let soc_before = ev.soc;

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        let power_kw = ev.telemetry().get("active_power_kw").unwrap();
        let expected_heater_kw = 500.0 / 1000.0;
        assert!(
            (power_kw - expected_heater_kw).abs() < 1e-9,
            "grid draw should equal heater power only ({} kW), got {} kW",
            expected_heater_kw,
            power_kw
        );
        assert_eq!(
            ev.soc, soc_before,
            "SOC must not change when charge_derate=0 (pre-heat only)"
        );
        assert!(
            ev.telemetry().get("heater_power_w").unwrap() > 0.0,
            "heater should be active"
        );
    }


    #[test]
    fn deterministic_fuzzing_replays_same_event_sequence_for_same_seed() {
        let mut raw = base_raw();
        raw.insert(KEY_EVENT_DAY_RATIO.to_string(), 1.0.into());
        raw.insert(KEY_ARRIVAL_FUZZ_MINUTES.to_string(), 60.0.into());
        raw.insert(KEY_DEPARTURE_FUZZ_MINUTES.to_string(), 60.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 30.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 8.0.into());
        raw.insert(
            KEY_DRIVER_ARCHETYPE.to_string(),
            "single_car_shared_household".into(),
        );
        raw.remove(KEY_SCHEDULE_LEN);
        let config = ev_config(raw);

        let mut a = Ev::new(config.clone());
        let mut b = Ev::new(config.clone());
        let mut env = sample_env();
        a.init(&config, &env).unwrap();
        b.init(&config, &env).unwrap();

        for day in 0u32..7 {
            env.current_time = dt(2026, 1, 1 + day, 12, 0, 0);
            let mut pa = PortSlots::default();
            let mut pb = PortSlots::default();
            a.step(&env, Duration::minutes(5), &mut pa).unwrap();
            b.step(&env, Duration::minutes(5), &mut pb).unwrap();
            assert_eq!(a.current_day_drive_miles, b.current_day_drive_miles);
            assert_eq!(a.todays_event, b.todays_event);
        }
    }

    /// Charge derate must be applied BEFORE the taper limit, not after.
    /// At partial derate, the effective power ceiling is lower, so the taper
    /// limit (which prevents SOC overshoot) must use the derated power.
    /// We use moderate SOC (0.5) with a short timestep so taper doesn't dominate,
    /// confirming that derated power is lower than underated power.
    #[test]
    fn charge_derate_applied_before_taper_limit() {
        let mut raw = base_raw();
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        // Partial derate: battery at 5C, min=0, full=10 => derate=0.5.
        raw.insert(KEY_BATTERY_TEMP_C.to_string(), 5.0.into());
        raw.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        raw.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 0.0.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        let config_cold = ev_config(raw);

        let mut raw_warm = base_raw();
        raw_warm.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw_warm.insert("schedule_start_soc_0".to_string(), 0.5.into());
        raw_warm.insert(KEY_BATTERY_TEMP_C.to_string(), 20.0.into());
        raw_warm.insert(KEY_MIN_CHARGE_TEMP_C.to_string(), 0.0.into());
        raw_warm.insert(KEY_FULL_POWER_TEMP_C.to_string(), 10.0.into());
        raw_warm.insert(KEY_UA_W_PER_K.to_string(), 0.0.into());
        raw_warm.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), 0.0.into());
        raw_warm.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        let config_warm = ev_config(raw_warm);

        let mut ev_cold = Ev::new(config_cold.clone());
        let mut ev_warm = Ev::new(config_warm.clone());
        let mut env = sample_env();
        ev_cold.init(&config_cold, &env).unwrap();
        ev_warm.init(&config_warm, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut p_cold = PortSlots::default();
        let mut p_warm = PortSlots::default();
        ev_cold
            .step(&env, Duration::minutes(15), &mut p_cold)
            .unwrap();
        ev_warm
            .step(&env, Duration::minutes(15), &mut p_warm)
            .unwrap();

        let power_cold = ev_cold.telemetry().get("active_power_kw").unwrap();
        let power_warm = ev_warm.telemetry().get("active_power_kw").unwrap();
        // Cold derate (0.5) should produce ~3.6 kW (half of 7.2);
        // warm (full power) should produce 7.2 kW.
        assert!(
            power_cold < power_warm,
            "derated power ({power_cold}) must be less than full power ({power_warm}) at same SOC"
        );
        // Verify the derate factor is approximately 0.5.
        let ratio = power_cold / power_warm;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "power ratio should be ~0.5 (derate factor), got {ratio}"
        );
    }

    // ── plug_in_policy tests ──────────────────────────────────────────

    #[test]
    fn plug_in_low_soc_prevents_connection_when_soc_above_threshold() {
        let mut raw = base_raw();
        raw.insert(KEY_PLUG_IN_POLICY.to_string(), "low_soc".into());
        raw.insert(KEY_PLUG_IN_SOC_THRESHOLD.to_string(), 0.5.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.6.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.6.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        assert_eq!(
            ev.telemetry().get("is_connected"),
            Some(0.0),
            "LowSoc policy must prevent connection when SOC >= threshold"
        );
    }

    #[test]
    fn plug_in_low_soc_allows_connection_when_soc_below_threshold() {
        let mut raw = base_raw();
        raw.insert(KEY_PLUG_IN_POLICY.to_string(), "low_soc".into());
        raw.insert(KEY_PLUG_IN_SOC_THRESHOLD.to_string(), 0.5.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.3.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.3.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        assert_eq!(
            ev.telemetry().get("is_connected"),
            Some(1.0),
            "LowSoc policy must allow connection when SOC < threshold"
        );
    }

    #[test]
    fn plug_in_low_soc_edge_exactly_at_threshold() {
        // SOC == threshold => blocked (>= comparison)
        let mut raw = base_raw();
        raw.insert(KEY_PLUG_IN_POLICY.to_string(), "low_soc".into());
        raw.insert(KEY_PLUG_IN_SOC_THRESHOLD.to_string(), 0.5.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        assert_eq!(
            ev.telemetry().get("is_connected"),
            Some(0.0),
            "LowSoc policy blocks connection when SOC == threshold (>= check)"
        );
    }

    #[test]
    fn plug_in_always_connects_regardless_of_soc() {
        let mut raw = base_raw();
        raw.insert(KEY_PLUG_IN_POLICY.to_string(), "always".into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.99.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.99.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        assert_eq!(
            ev.telemetry().get("is_connected"),
            Some(1.0),
            "Always policy must connect regardless of SOC"
        );
    }

    // ── V2L mode tests ────────────────────────────────────────────────

    #[test]
    fn v2l_allows_negative_power_when_enabled_and_soc_above_reserve() {
        let mut raw = base_raw();
        raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.2.into());
        raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -2.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        let power = ev.telemetry().get("active_power_kw").unwrap();
        assert!(
            power < 0.0,
            "V2L should produce negative active_power_kw, got {power}"
        );
        assert_eq!(ev.telemetry().get("v2l_active"), Some(1.0));
    }

    #[test]
    fn v2l_respects_soc_reserve_floor() {
        let mut raw = base_raw();
        raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.5.into());
        raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        // SOC at reserve floor
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        // SOC at exactly the reserve => compute_v2l_discharge returns 0
        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        let power = ev.telemetry().get("active_power_kw").unwrap();
        assert_eq!(power, 0.0, "V2L must not discharge when SOC <= reserve");
    }

    #[test]
    fn v2l_respects_max_discharge_limit() {
        let mut raw = base_raw();
        raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
        raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 2.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.8.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        // Request 10 kW discharge, but max is 2 kW
        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -10.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(60), &mut ports).unwrap();

        let power = ev.telemetry().get("active_power_kw").unwrap();
        assert!(
            power >= -2.0 - 1e-9,
            "V2L discharge must respect max_discharge_kw limit of 2.0, got {power}"
        );
    }

    #[test]
    fn v2l_rejected_when_disabled() {
        let mut raw = base_raw();
        raw.insert(KEY_V2L_ENABLED.to_string(), false.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &sample_env()).unwrap();

        let err = ev
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: -1.0,
                reactive_power_kvar: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("v2l_enabled or v2g_enabled"),
            "negative setpoint without V2L should be rejected"
        );
    }

    #[test]
    fn v2l_does_not_export_to_grid() {
        // V2L discharge uses a negative active_power_kw through the port system.
        // The port accumulator routes negative values to generation_power_kw.
        // V2L is building-side load offset -- the controller must prevent
        // grid export. Here we verify the EV equipment itself caps discharge
        // at v2l_max_discharge_kw and does not exceed it.
        let mut raw = base_raw();
        raw.insert(KEY_V2L_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2L_SOC_RESERVE.to_string(), 0.1.into());
        raw.insert(KEY_V2L_MAX_DISCHARGE_KW.to_string(), 3.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.8.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -2.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        // V2L discharge routes through generation_power_kw (negative active_power_kw)
        assert!(
            ports.electrical.generation_power_kw < 0.0,
            "V2L discharge should appear in generation_power_kw, got {}",
            ports.electrical.generation_power_kw
        );
        // Discharge magnitude must not exceed v2l_max_discharge_kw
        assert!(
            ports.electrical.generation_power_kw.abs() <= 3.0 + 1e-9,
            "V2L must not exceed max_discharge_kw={}, got {}",
            3.0,
            ports.electrical.generation_power_kw.abs()
        );
        // Load port should be zero (V2L is pure discharge, not a load)
        assert_eq!(
            ports.electrical.load_power_kw, 0.0,
            "V2L discharge should not appear in load_power_kw"
        );
    }

    // ── Arrival SOC formula tests ─────────────────────────────────────

    #[test]
    fn arrival_soc_uses_departure_soc_minus_trip_not_soc_max() {
        // Vehicle at SOC 0.3, drives 50 miles.
        // Correct arrival SOC = 0.3 - (50 * 0.325 / capacity)
        // NOT soc_max - trip/capacity.
        let capacity = 60.0;
        let departure_soc = 0.3;
        let drive_miles = 50.0;
        let trip_kwh = drive_miles * EV_FUEL_ECONOMY_KWH_PER_MI;
        let expected_soc = (departure_soc - trip_kwh / capacity).max(0.0);

        let mut raw = base_raw();
        raw.insert(KEY_BATTERY_CAPACITY_KWH.to_string(), capacity.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), departure_soc.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.9.into()); // distribution row start_soc
        raw.insert(KEY_EVENT_DAY_RATIO.to_string(), 1.0.into());
        // Zero fuzz so arrival time is deterministic from the distribution
        raw.insert(KEY_ARRIVAL_FUZZ_MINUTES.to_string(), 0.0.into());
        raw.insert(KEY_DEPARTURE_FUZZ_MINUTES.to_string(), 0.0.into());
        // Fixed drive distance: mean=50, stddev=0
        raw.insert(KEY_DAILY_DRIVE_MILES_MEAN.to_string(), drive_miles.into());
        raw.insert(KEY_DAILY_DRIVE_MILES_STDDEV.to_string(), 0.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).unwrap();

        // Trigger event roll at 18:00 (arrival time from schedule)
        env.current_time = dt(2026, 1, 1, 18, 0, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).unwrap();

        // The event's start_soc should be min(distribution_row.start_soc, soc_after_driving)
        // soc_after_driving = 0.3 - (50 * 0.325 / 60) = 0.3 - 0.2708... = 0.0292
        // min(0.9, 0.0292) = 0.0292
        if let Some(event) = &ev.todays_event {
            assert!(
                (event.start_soc - expected_soc).abs() < 1e-9,
                "arrival SOC should be departure_soc - trip/capacity = {expected_soc}, got {}",
                event.start_soc
            );
        } else {
            panic!("event should have been rolled with event_day_ratio=1.0");
        }
    }

    // ── event_day_ratio OCHRE tiers tests ─────────────────────────────

    #[test]
    fn event_day_ratio_l2_80kwh_is_020() {
        let ratio = default_event_day_ratio(80.0, ChargingLevel::L2);
        assert!(
            (ratio - 0.20).abs() < 1e-9,
            "L2 80 kWh should yield 0.20, got {ratio}"
        );
    }

    #[test]
    fn event_day_ratio_l2_40kwh_is_033() {
        let ratio = default_event_day_ratio(40.0, ChargingLevel::L2);
        assert!(
            (ratio - 0.33).abs() < 1e-9,
            "L2 40 kWh should yield 0.33, got {ratio}"
        );
    }

    #[test]
    fn event_day_ratio_l2_20kwh_is_050() {
        let ratio = default_event_day_ratio(20.0, ChargingLevel::L2);
        assert!(
            (ratio - 0.50).abs() < 1e-9,
            "L2 20 kWh should yield 0.50, got {ratio}"
        );
    }

    #[test]
    fn event_day_ratio_l1_is_090_flat() {
        for capacity in [20.0, 40.0, 80.0, 120.0] {
            let ratio = default_event_day_ratio(capacity, ChargingLevel::L1);
            assert!(
                (ratio - 0.9).abs() < 1e-9,
                "L1 {capacity} kWh should yield 0.9, got {ratio}"
            );
        }
    }

    // ── ChaCha8 set_word_pos regression test ──────────────────────────

    #[test]
    fn load_state_rng_matches_fresh_instance_after_n_draws() {
        // A fresh RNG that runs N draws sequentially must produce the same
        // next value as an RNG that used set_word_pos(N*2) to seek.
        use rand::{RngExt, SeedableRng};
        use rand_chacha::ChaCha8Rng;

        let seed = [42_u8; 32];
        let n_draws: u64 = 100;

        // Path A: sequential draws
        let mut rng_seq = ChaCha8Rng::from_seed(seed);
        for _ in 0..n_draws {
            let _ = rng_seq.random::<f64>();
        }
        let next_seq = rng_seq.random::<f64>();

        // Path B: O(1) seeking via set_word_pos
        let mut rng_seek = ChaCha8Rng::from_seed(seed);
        rng_seek.set_word_pos((n_draws as u128) * 2);
        let next_seek = rng_seek.random::<f64>();

        assert_eq!(
            next_seq, next_seek,
            "set_word_pos seeking must produce identical stream to sequential draws"
        );
    }

    #[test]
    fn load_state_rng_regression_across_save_restore() {
        // End-to-end: save an EV after N steps, load into a fresh instance,
        // verify subsequent draws match.
        let config = ev_config(base_raw());
        let mut env = sample_env();
        let mut ev_a = Ev::new(config.clone());
        ev_a.init(&config, &env).unwrap();

        // Run 30 steps to accumulate rng_draws
        for _ in 0..30 {
            let mut ports = PortSlots::default();
            ev_a.step(&env, Duration::minutes(5), &mut ports).unwrap();
            env.current_time = env.current_time + ChronoDuration::minutes(5);
        }

        let checkpoint = ev_a.save_state();
        let draws_at_checkpoint = ev_a.rng_draws;

        // Restore into ev_b
        let mut ev_b = Ev::new(config.clone());
        ev_b.init(&config, &sample_env()).unwrap();
        ev_b.load_state(&checkpoint).unwrap();

        assert_eq!(ev_b.rng_draws, draws_at_checkpoint);

        // Next random draws must match
        let next_a = ev_a.random_unit();
        let next_b = ev_b.random_unit();
        assert_eq!(
            next_a, next_b,
            "RNG must produce identical values after checkpoint restore"
        );
    }

    // ── equipment_id in RNG seed tests ────────────────────────────────

    #[test]
    fn different_equipment_names_produce_different_rng_seeds() {
        let raw_a = base_raw();
        let config_a = EquipmentConfig {
            name: "EV_Front".to_string(),
            ochre_class: "EV".to_string(),
            raw_config: raw_a.clone(),
        };

        let config_b = EquipmentConfig {
            name: "EV_Rear".to_string(),
            ochre_class: "EV".to_string(),
            raw_config: raw_a,
        };

        let seed_a = derive_rng_seed(&config_a);
        let seed_b = derive_rng_seed(&config_b);

        assert_ne!(
            seed_a, seed_b,
            "same master_seed + building_id but different equipment names must produce different seeds"
        );
    }

    #[test]
    fn different_equipment_names_diverge_event_sequences() {
        let raw = base_raw();
        let config_a = EquipmentConfig {
            name: "EV_garage_left".to_string(),
            ochre_class: "EV".to_string(),
            raw_config: raw.clone(),
        };
        let config_b = EquipmentConfig {
            name: "EV_garage_right".to_string(),
            ochre_class: "EV".to_string(),
            raw_config: raw,
        };

        let mut ev_a = Ev::new(config_a.clone());
        let mut ev_b = Ev::new(config_b.clone());
        let mut env = sample_env();
        ev_a.init(&config_a, &env).unwrap();
        ev_b.init(&config_b, &env).unwrap();

        let mut diverged = false;
        for day in 0u32..30 {
            env.current_time = dt(2026, 1, 1 + day, 18, 0, 0);
            let mut pa = PortSlots::default();
            let mut pb = PortSlots::default();
            ev_a.step(&env, Duration::minutes(5), &mut pa).unwrap();
            ev_b.step(&env, Duration::minutes(5), &mut pb).unwrap();
            if ev_a.todays_event != ev_b.todays_event {
                diverged = true;
                break;
            }
        }

        assert!(
            diverged,
            "two EVs with different names should produce different event sequences"
        );
    }

    // ── DriverArchetype serde round-trip tests ────────────────────────

    #[test]
    fn driver_archetype_postcard_round_trip() {
        let variants = [
            DriverArchetype::Commuter,
            DriverArchetype::ShiftWorker,
            DriverArchetype::WorkFromHome,
            DriverArchetype::WeekendWarrior,
            DriverArchetype::SeniorRetiree,
            DriverArchetype::SchoolRunFamily,
            DriverArchetype::SingleCarSharedHousehold,
        ];

        for variant in &variants {
            let bytes = save_postcard(variant);
            let decoded: DriverArchetype = load_postcard(&bytes)
                .unwrap_or_else(|e| panic!("postcard round-trip failed for {variant:?}: {e}"));
            assert_eq!(*variant, decoded);
        }
    }

    // ── Registry tests ────────────────────────────────────────────────

    #[test]
    fn registry_new_can_look_up_ev_by_name() {
        let registry = crate::EquipmentRegistry::new();
        assert!(
            registry.get("EV").is_some(),
            "EquipmentRegistry::new() must register 'EV'"
        );
    }

    #[test]
    fn registry_creates_ev_with_correct_type() {
        let registry = crate::EquipmentRegistry::new();
        let eq = registry
            .create("EV", ev_config(base_raw()))
            .expect("EV should be registered");
        assert_eq!(eq.descriptor().equipment_type, "EV");
    }

    // ── V2G tests ──────────────────────────────────────────────────

    #[test]
    fn v2g_allows_negative_power_when_enabled() {
        let mut raw = base_raw();
        raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.3.into());
        raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let env = sample_env();
        ev.init(&config, &env).expect("init");

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: None,
        })
        .expect("V2G negative setpoint should be accepted");
    }

    #[test]
    fn v2g_discharge_produces_negative_power() {
        let mut raw = base_raw();
        raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.2.into());
        raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.8.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.8.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).expect("init");

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: None,
        })
        .expect("setpoint");

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).expect("step");
        let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
        assert!(
            power < -0.1,
            "V2G should produce negative active_power_kw, got {power}"
        );
    }

    #[test]
    fn v2g_respects_soc_reserve() {
        let mut raw = base_raw();
        raw.insert(KEY_V2G_ENABLED.to_string(), true.into());
        raw.insert(KEY_V2G_SOC_RESERVE.to_string(), 0.5.into());
        raw.insert(KEY_V2G_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert("schedule_start_soc_0".to_string(), 0.5.into());
        let config = ev_config(raw);
        let mut ev = Ev::new(config.clone());
        let mut env = sample_env();
        ev.init(&config, &env).expect("init");

        ev.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .expect("setpoint");

        env.current_time = dt(2026, 1, 1, 18, 30, 0);
        let mut ports = PortSlots::default();
        ev.step(&env, Duration::minutes(15), &mut ports).expect("step");
        let power = ev.telemetry().get("active_power_kw").unwrap_or(0.0);
        assert!(
            power.abs() < 0.01,
            "V2G must not discharge when SOC at reserve, got {power}"
        );
    }

    #[test]
    fn v2g_disabled_by_default() {
        let config = ev_config(base_raw());
        let ev = Ev::new(config);
        assert!(!ev.v2g_enabled, "V2G should be disabled by default");
    }
}
