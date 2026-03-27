use hares_types::HaresError;

use crate::EquipmentConfig;

use hares_types::ChargingLevel;

pub(super) use crate::config::KEY_EQUIPMENT_ID;
pub(crate) const KEY_BATTERY_CAPACITY_KWH: &str = "capacity_kwh";
pub(super) const KEY_BATTERY_CAPACITY_HPIXML_KWH: &str = "BatteryCapacity";
pub(crate) const KEY_CHARGING_LEVEL: &str = "charging_level";
pub(super) const KEY_CHARGING_LEVEL_HPIXML: &str = "ChargingLevel";
pub(crate) const KEY_MAX_CHARGING_POWER_KW: &str = "max_charging_power_kw";
pub(super) const KEY_MAX_CHARGING_POWER_HPIXML_KW: &str = "MaxChargingPower";
pub(crate) const KEY_VEHICLE_TYPE: &str = "vehicle_type";
pub(crate) const KEY_RANGE_MILES: &str = "range_miles";
pub(super) const KEY_INITIAL_SOC: &str = "initial_soc";
pub(super) const KEY_INITIAL_CONNECTION_STATE: &str = "initial_connection_state";
pub(super) const KEY_SOC_MAX: &str = "soc_max";
pub(super) const KEY_EFFICIENCY: &str = "charging_efficiency";
pub(super) const KEY_POWER_LIMIT_KW: &str = "power_limit_kw";
pub(super) const KEY_L1_CURRENT_A: &str = "l1_current_a";
pub(super) const KEY_L1_VOLTAGE_V: &str = "l1_voltage_v";
pub(super) const KEY_BATTERY_TEMP_C: &str = "battery_temp_c";
pub(super) const KEY_MIN_CHARGE_TEMP_C: &str = "min_charge_temp_c";
pub(super) const KEY_FULL_POWER_TEMP_C: &str = "full_power_temp_c";
pub(super) const KEY_HEATER_POWER_W: &str = "heater_power_w";
pub(super) const KEY_HEATER_THRESHOLD_C: &str = "heater_threshold_c";
pub(super) const KEY_THERMAL_MASS_J_PER_K: &str = "thermal_mass_j_per_k";
pub(super) const KEY_UA_W_PER_K: &str = "ua_w_per_k";
pub(super) const KEY_V2L_ENABLED: &str = "v2l_enabled";
pub(super) const KEY_V2L_SOC_RESERVE: &str = "v2l_soc_reserve";
pub(super) const KEY_V2L_MAX_DISCHARGE_KW: &str = "v2l_max_discharge_kw";
pub(super) const KEY_V2G_ENABLED: &str = "v2g_enabled";
pub(super) const KEY_V2G_SOC_RESERVE: &str = "v2g_soc_reserve";
pub(super) const KEY_V2G_MAX_DISCHARGE_KW: &str = "v2g_max_discharge_kw";
pub(crate) const KEY_READY_SOC: &str = "ready_soc";
pub(crate) const KEY_FUEL_ECONOMY_KWH_PER_MI: &str = "fuel_economy_kwh_per_mi";
pub(crate) const KEY_CHEMISTRY: &str = "chemistry";
pub(super) const KEY_CHARGING_STRATEGY: &str = "charging_strategy";
pub(super) const KEY_PLUG_IN_POLICY: &str = "plug_in_policy";

pub(super) const DEFAULT_FUEL_ECONOMY_KWH_PER_MI: f64 = 0.325;
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
pub(super) const DEFAULT_V2L_SOC_RESERVE: f64 = 0.2;
pub(super) const DEFAULT_V2L_MAX_DISCHARGE_KW: f64 = 3.0;
pub(super) const DEFAULT_V2G_SOC_RESERVE: f64 = 0.3;
pub(super) const DEFAULT_V2G_MAX_DISCHARGE_KW: f64 = 5.0;
pub(super) const SECONDS_PER_HOUR: f64 = 3600.0;
pub(super) const MIN_TIMESTEP_HOURS: f64 = 1e-9;

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

    Some(range_miles * DEFAULT_FUEL_ECONOMY_KWH_PER_MI)
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
        .or_else(|| {
            (capacity_kwh > 0.0).then_some(capacity_kwh / DEFAULT_FUEL_ECONOMY_KWH_PER_MI)
        })
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
