use hares_types::HaresError;

use crate::EquipmentConfig;

use super::ChargingLevel;

pub(super) use crate::config::KEY_EQUIPMENT_ID;
pub(super) const KEY_MASTER_SEED: &str = "master_seed";
pub(super) const KEY_BUILDING_ID: &str = "building_id";
pub(super) const KEY_BATTERY_CAPACITY_KWH: &str = "capacity_kwh";
pub(super) const KEY_BATTERY_CAPACITY_HPIXML_KWH: &str = "BatteryCapacity";
pub(super) const KEY_CHARGING_LEVEL: &str = "charging_level";
pub(super) const KEY_CHARGING_LEVEL_HPIXML: &str = "ChargingLevel";
pub(super) const KEY_MAX_CHARGING_POWER_KW: &str = "max_charging_power_kw";
pub(super) const KEY_MAX_CHARGING_POWER_HPIXML_KW: &str = "MaxChargingPower";
pub(super) const KEY_VEHICLE_TYPE: &str = "vehicle_type";
pub(super) const KEY_RANGE_MILES: &str = "range_miles";
pub(super) const KEY_SCHEDULE_CSV_PATH: &str = "schedule_csv_path";
pub(super) const KEY_SCHEDULE_CSV_REF: &str = "schedule_csv";
pub(super) const KEY_EVENT_DAY_RATIO: &str = "event_day_ratio";
pub(super) const KEY_INITIAL_SOC: &str = "initial_soc";
pub(super) const KEY_SOC_MAX: &str = "soc_max";
pub(super) const KEY_EFFICIENCY: &str = "charging_efficiency";
pub(super) const KEY_SCHEDULE_LEN: &str = "schedule_len";
pub(super) const KEY_POWER_LIMIT_KW: &str = "power_limit_kw";
pub(super) const KEY_L1_CURRENT_A: &str = "l1_current_a";
pub(super) const KEY_L1_VOLTAGE_V: &str = "l1_voltage_v";
pub(super) const KEY_IMMEDIATE_TARGET_SOC: &str = "immediate_target_soc";
pub(super) const KEY_DELAY_UNTIL_HOUR: &str = "delay_until_hour";
pub(super) const KEY_TOU_AVOID_PEAK: &str = "tou_avoid_peak";
pub(super) const KEY_TOU_PEAK_START_HOUR: &str = "tou_peak_start_hour";
pub(super) const KEY_TOU_PEAK_END_HOUR: &str = "tou_peak_end_hour";
pub(super) const KEY_READY_BY_HOUR: &str = "ready_by_hour";
pub(super) const KEY_READY_TARGET_SOC: &str = "ready_target_soc";
pub(super) const KEY_PYBAMM_LUT_PATH: &str = "pybamm_lut_path";
pub(super) const KEY_BATTERY_TEMP_C: &str = "battery_temp_c";
pub(super) const KEY_MIN_CHARGE_TEMP_C: &str = "min_charge_temp_c";
pub(super) const KEY_FULL_POWER_TEMP_C: &str = "full_power_temp_c";
pub(super) const KEY_HEATER_POWER_W: &str = "heater_power_w";
pub(super) const KEY_HEATER_THRESHOLD_C: &str = "heater_threshold_c";
pub(super) const KEY_THERMAL_MASS_J_PER_K: &str = "thermal_mass_j_per_k";
pub(super) const KEY_UA_W_PER_K: &str = "ua_w_per_k";
pub(super) const KEY_DRIVER_ARCHETYPE: &str = "driver_archetype";
pub(super) const KEY_ARRIVAL_FUZZ_MINUTES: &str = "arrival_fuzz_minutes";
pub(super) const KEY_DEPARTURE_FUZZ_MINUTES: &str = "departure_fuzz_minutes";
pub(super) const KEY_DAILY_DRIVE_MILES_MEAN: &str = "daily_drive_miles_mean";
pub(super) const KEY_DAILY_DRIVE_MILES_STDDEV: &str = "daily_drive_miles_stddev";
pub(super) const KEY_SHIFT_ROTATION_DAYS: &str = "shift_rotation_days";
pub(super) const KEY_SHIFT_ON_DAYS: &str = "shift_on_days";
pub(super) const KEY_SHIFT_DURATION_FUZZ_MINUTES: &str = "shift_duration_fuzz_minutes";
pub(super) const KEY_PLUG_IN_POLICY: &str = "plug_in_policy";
pub(super) const KEY_PLUG_IN_SOC_THRESHOLD: &str = "plug_in_soc_threshold";
pub(super) const KEY_V2L_ENABLED: &str = "v2l_enabled";
pub(super) const KEY_V2L_SOC_RESERVE: &str = "v2l_soc_reserve";
pub(super) const KEY_V2L_MAX_DISCHARGE_KW: &str = "v2l_max_discharge_kw";
pub(super) const KEY_V2G_ENABLED: &str = "v2g_enabled";
pub(super) const KEY_V2G_SOC_RESERVE: &str = "v2g_soc_reserve";
pub(super) const KEY_V2G_MAX_DISCHARGE_KW: &str = "v2g_max_discharge_kw";

pub(super) const EV_FUEL_ECONOMY_KWH_PER_MI: f64 = 0.325;
pub(super) const L1_CHARGING_POWER_KW: f64 = 1.4;
pub(super) const L1_MIN_POWER_KW: f64 = 1.0;
pub(super) const L1_MAX_POWER_KW: f64 = 1.8;
pub(super) const L2_MIN_POWER_KW: f64 = 3.6;
pub(super) const L2_MAX_POWER_KW: f64 = 11.5;
pub(super) const DEFAULT_CAPACITY_KWH: f64 = 75.0;
pub(super) const DEFAULT_SOC: f64 = 1.0;
pub(super) const DEFAULT_SOC_MAX: f64 = 1.0;
pub(super) const DEFAULT_EFFICIENCY: f64 = 0.9;
pub(super) const DEFAULT_L1_VOLTAGE_V: f64 = 120.0;
pub(super) const DEFAULT_MIN_CHARGE_TEMP_C: f64 = 0.0;
pub(super) const DEFAULT_FULL_POWER_TEMP_C: f64 = 10.0;
pub(super) const DEFAULT_HEATER_POWER_W: f64 = 0.0;
pub(super) const DEFAULT_HEATER_THRESHOLD_C: f64 = 0.0;
pub(super) const DEFAULT_THERMAL_MASS_J_PER_K: f64 = 20_000.0;
pub(super) const DEFAULT_UA_W_PER_K: f64 = 4.0;
pub(super) const DEFAULT_DAILY_DRIVE_MILES_MEAN: f64 = 30.0;
pub(super) const DEFAULT_DAILY_DRIVE_MILES_STDDEV: f64 = 12.0;
pub(super) const DEFAULT_PLUG_IN_SOC_THRESHOLD: f64 = 0.3;
pub(super) const DEFAULT_V2L_SOC_RESERVE: f64 = 0.2;
pub(super) const DEFAULT_V2L_MAX_DISCHARGE_KW: f64 = 3.0;
pub(super) const DEFAULT_V2G_SOC_RESERVE: f64 = 0.3;
pub(super) const DEFAULT_V2G_MAX_DISCHARGE_KW: f64 = 5.0;
pub(super) const SECONDS_PER_HOUR: f64 = 3600.0;
pub(super) const SECONDS_PER_DAY: i64 = 86_400;
pub(super) const MIN_TIMESTEP_HOURS: f64 = 1e-9;

pub(super) fn derive_rng_seed(config: &EquipmentConfig) -> [u8; 32] {
    let master_seed = config.get_f64(KEY_MASTER_SEED).unwrap_or_default() as u64;
    let building_id = config.get_f64(KEY_BUILDING_ID).unwrap_or_default() as i64;

    // Hash equipment name to discriminate multiple EV instances in the same dwelling
    let eq_hash = {
        let name_bytes = config.name.as_bytes();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
        for &b in name_bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3); // FNV-1a prime
        }
        h
    };

    let mut seed = [0_u8; 32];
    seed[0..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&building_id.to_le_bytes());
    seed[16..24].copy_from_slice(&eq_hash.to_le_bytes());
    seed
}

pub(super) fn resolve_capacity_kwh(config: &EquipmentConfig) -> Option<f64> {
    let direct = config
        .get_f64(KEY_BATTERY_CAPACITY_KWH)
        .or_else(|| config.get_f64(KEY_BATTERY_CAPACITY_HPIXML_KWH));
    if let Some(capacity_kwh) = direct
        && capacity_kwh.is_finite()
        && capacity_kwh > 0.0
    {
        return Some(capacity_kwh);
    }

    let range_miles = config.get_f64(KEY_RANGE_MILES)?;
    if !range_miles.is_finite() || range_miles <= 0.0 {
        return None;
    }

    // OCHRE EV_FUEL_ECONOMY in EV.py is represented as miles/kWh.
    // Here we use the verified, explicit unit form requested by the ticket: kWh/mi.
    Some(range_miles * EV_FUEL_ECONOMY_KWH_PER_MI)
}

pub(super) fn resolve_rated_power_kw(
    config: &EquipmentConfig,
    charging_level: ChargingLevel,
    capacity_kwh: f64,
) -> Option<f64> {
    let direct = config
        .get_f64(KEY_MAX_CHARGING_POWER_KW)
        .or_else(|| config.get_f64(KEY_MAX_CHARGING_POWER_HPIXML_KW));

    if let Some(power_kw) = direct
        && power_kw.is_finite()
        && power_kw > 0.0
    {
        return Some(match charging_level {
            ChargingLevel::L1 => power_kw.clamp(L1_MIN_POWER_KW, L1_MAX_POWER_KW),
            ChargingLevel::L2 => power_kw.clamp(L2_MIN_POWER_KW, L2_MAX_POWER_KW),
        });
    }

    Some(default_max_power_kw(config, charging_level, capacity_kwh))
}

pub(super) fn default_max_power_kw(
    config: &EquipmentConfig,
    charging_level: ChargingLevel,
    capacity_kwh: f64,
) -> f64 {
    match charging_level {
        ChargingLevel::L1 => L1_CHARGING_POWER_KW,
        ChargingLevel::L2 => {
            let vehicle_number = vehicle_number_from_config(config, capacity_kwh);
            // OCHRE EV_MAX_POWER for Level2 by vehicle # (1..4): [3.6, 3.6, 7.2, 11.5].
            match vehicle_number {
                1 | 2 => 3.6,
                3 => 7.2,
                _ => 11.5,
            }
        }
    }
}

pub(super) fn vehicle_number_from_config(config: &EquipmentConfig, capacity_kwh: f64) -> u8 {
    let range_miles = config
        .get_f64(KEY_RANGE_MILES)
        .or_else(|| (capacity_kwh > 0.0).then_some(capacity_kwh / EV_FUEL_ECONOMY_KWH_PER_MI))
        .unwrap_or_default();
    let vehicle_type = config
        .get_str(KEY_VEHICLE_TYPE)
        .unwrap_or("BEV")
        .trim()
        .to_ascii_uppercase();

    if vehicle_type == "PHEV" {
        if range_miles < 35.0 { 1 } else { 2 }
    } else if range_miles < 175.0 {
        3
    } else {
        4
    }
}

/// Derive a default event_day_ratio when none is explicitly configured.
///
/// Uses OCHRE's discrete capacity tiers (OCHRE EV.py lines 125-137):
/// - L1: flat 0.9 regardless of capacity
/// - L2: capacity >= 70 kWh -> 0.20, >= 35 kWh -> 0.33, else -> 0.50
pub(super) fn default_event_day_ratio(capacity_kwh: f64, level: ChargingLevel) -> f64 {
    match level {
        // OCHRE EV.py:126-128 -- Level 1 plug charges most days
        ChargingLevel::L1 => 0.9,
        // OCHRE EV.py:129-137 -- Level 2 uses capacity-based tiers
        ChargingLevel::L2 => {
            if capacity_kwh >= 70.0 {
                0.2
            } else if capacity_kwh >= 35.0 {
                0.33
            } else {
                0.5
            }
        }
    }
}

pub(super) fn validate_optional_hour(field_name: &str, value: Option<f64>) -> crate::Result<()> {
    if let Some(v) = value
        && (!v.is_finite() || !(0.0..24.0).contains(&v))
    {
        return Err(HaresError::Equipment(format!(
            "EV {field_name} must be finite and within [0, 24)"
        )));
    }
    Ok(())
}
