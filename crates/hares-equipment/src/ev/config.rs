use hares_types::HaresError;
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;
use crate::config::EquipmentTypedConfig;

use hares_types::ChargingLevel;

pub(super) use crate::config::KEY_EQUIPMENT_ID;
pub(crate) const KEY_BATTERY_CAPACITY_KWH: &str = "capacity_kwh";
pub(super) const KEY_BATTERY_CAPACITY_HPXML_KWH: &str = "BatteryCapacity";
pub(crate) const KEY_CHARGING_LEVEL: &str = "charging_level";
pub(super) const KEY_CHARGING_LEVEL_HPXML: &str = "ChargingLevel";
pub(crate) const KEY_MAX_CHARGING_POWER_KW: &str = "max_charging_power_kw";
pub(super) const KEY_MAX_CHARGING_POWER_HPXML_KW: &str = "MaxChargingPower";
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
pub(super) const KEY_POWER_FACTOR: &str = "power_factor";
pub(super) const KEY_CHARGER_CAPACITY_KVA: &str = "charger_capacity_kva";

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
// Standard laboratory reference temperature (20 °C / 293.15 K). Used as EV battery
// initial temperature when ambient conditions are unavailable at construction time.
// This is a conventional engineering default, not a standard mandate. No ASHRAE or
// SAE standard specifies an EV battery initialisation temperature for simulation.
pub(super) const DEFAULT_BATTERY_TEMP_C: f64 = 20.0;
pub(super) const DEFAULT_V2L_SOC_RESERVE: f64 = 0.2;
pub(super) const DEFAULT_V2L_MAX_DISCHARGE_KW: f64 = 3.0;
pub(super) const DEFAULT_V2G_SOC_RESERVE: f64 = 0.3;
pub(super) const DEFAULT_V2G_MAX_DISCHARGE_KW: f64 = 5.0;
pub(super) use hares_physics::constants::SECONDS_PER_HOUR;
pub(super) const MIN_TIMESTEP_HOURS: f64 = 1e-9;

pub(super) fn resolve_capacity_kwh(config: &EquipmentConfig) -> Option<f64> {
    let direct = config
        .get_f64(KEY_BATTERY_CAPACITY_KWH)
        .or_else(|| config.get_f64(KEY_BATTERY_CAPACITY_HPXML_KWH));
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
        .or_else(|| config.get_f64(KEY_MAX_CHARGING_POWER_HPXML_KW));

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
        .or_else(|| (capacity_kwh > 0.0).then_some(capacity_kwh / DEFAULT_FUEL_ECONOMY_KWH_PER_MI))
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

/// Typed configuration for an electric vehicle charger.
///
/// All power values are in kW, energy in kWh, temperatures in °C.
/// `charging_efficiency` is the AC→DC onboard charger efficiency as a fraction in (0, 1].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvConfig {
    /// Equipment instance identifier.
    pub equipment_id: Option<u32>,
    /// Battery pack capacity (kWh). Resolvers map "battery_capacity_kwh" to this field.
    pub capacity_kwh: f64,
    /// EVSE charging level: "L1" or "L2".
    pub charging_level: Option<String>,
    /// Maximum charging power (kW). Clamped to level-appropriate bounds.
    pub max_charging_power_kw: f64,
    /// AC→DC onboard charger efficiency [0, 1].
    pub charging_efficiency: Option<f64>,
    /// L1 circuit current (A). Only used when charging_level = "L1".
    pub l1_current_a: Option<f64>,
    /// L1 circuit voltage (V). Defaults to 120 V.
    pub l1_voltage_v: Option<f64>,
    /// Maximum state-of-charge for regular charging [0, 1].
    pub soc_max: Option<f64>,
    /// Initial SOC [0, 1].
    pub initial_soc: Option<f64>,
    /// Initial battery temperature (°C). Defaults to DEFAULT_BATTERY_TEMP_C (20.0 °C).
    pub battery_temp_c: Option<f64>,
    /// Minimum temperature for charging (°C).
    pub min_charge_temp_c: Option<f64>,
    /// Temperature above which full charge power is available (°C).
    pub full_power_temp_c: Option<f64>,
    /// Battery heater power (W).
    pub heater_power_w: Option<f64>,
    /// Temperature threshold to activate battery heater (°C).
    pub heater_threshold_c: Option<f64>,
    /// Battery pack thermal mass (J/K).
    pub thermal_mass_j_per_k: Option<f64>,
    /// Pack-to-ambient heat transfer coefficient (W/K).
    pub ua_w_per_k: Option<f64>,
    /// Enable vehicle-to-load (V2L) discharge.
    pub v2l_enabled: Option<bool>,
    /// Minimum SOC to retain for V2L [0, 1].
    pub v2l_soc_reserve: Option<f64>,
    /// Maximum V2L discharge power (kW).
    pub v2l_max_discharge_kw: Option<f64>,
    /// Enable vehicle-to-grid (V2G) discharge.
    pub v2g_enabled: Option<bool>,
    /// Minimum SOC to retain for V2G [0, 1].
    pub v2g_soc_reserve: Option<f64>,
    /// Maximum V2G discharge power (kW).
    pub v2g_max_discharge_kw: Option<f64>,
    /// Battery chemistry string, e.g. "NMC".
    pub chemistry: Option<String>,
    /// Fuel economy (kWh/mile) for range estimation.
    pub fuel_economy_kwh_per_mi: Option<f64>,
    /// SOC target when the vehicle must be ready.
    pub ready_soc: Option<f64>,
    /// Charging strategy JSON blob.
    pub charging_strategy: Option<String>,
    /// Plug-in policy JSON blob.
    pub plug_in_policy: Option<String>,
    /// Hard power limit from external source (kW).
    pub power_limit_kw: Option<f64>,
    /// Initial connection state string.
    pub initial_connection_state: Option<String>,
    /// Power factor magnitude for the charger/inverter front-end.
    /// Defaults to 1.0 (unity — EV onboard chargers have PFC front-ends at
    /// ~0.99+; default runs bit-identical to pre-reactive behaviour).
    pub power_factor: Option<f64>,
    /// Charger/inverter AC apparent-power rating [kVA]. Used as the kVA
    /// clamp for reactive power (active-power priority: P never curtailed).
    /// Defaults to `max(max_charging_power_kw, v2g_max_discharge_kw,
    /// v2l_max_discharge_kw)` at init time.
    pub charger_capacity_kva: Option<f64>,
}

impl EquipmentTypedConfig for EvConfig {
    fn equipment_type_name() -> &'static str {
        "EV"
    }
}

impl EvConfig {
    /// Validate all fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.capacity_kwh.is_finite() || self.capacity_kwh <= 0.0 {
            return Err(HaresError::Equipment(
                "EV capacity_kwh must be finite and > 0".to_string(),
            ));
        }
        if !self.max_charging_power_kw.is_finite() || self.max_charging_power_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "EV max_charging_power_kw must be finite and > 0".to_string(),
            ));
        }
        if let Some(eff) = self.charging_efficiency {
            if eff <= 0.0 || eff > 1.0 || !eff.is_finite() {
                return Err(HaresError::Equipment(
                    "EV charging_efficiency must be finite and within (0, 1]".to_string(),
                ));
            }
        }
        if let Some(soc_max) = self.soc_max {
            if !soc_max.is_finite() || !(0.0..=1.0).contains(&soc_max) {
                return Err(HaresError::Equipment(
                    "EV soc_max must be finite and within [0, 1]".to_string(),
                ));
            }
        }
        if let Some(soc) = self.initial_soc {
            if !soc.is_finite() || !(0.0..=1.0).contains(&soc) {
                return Err(HaresError::Equipment(
                    "EV initial_soc must be finite and within [0, 1]".to_string(),
                ));
            }
        }
        if let Some(current) = self.l1_current_a {
            if !current.is_finite() || current <= 0.0 {
                return Err(HaresError::Equipment(
                    "EV l1_current_a must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(voltage) = self.l1_voltage_v {
            if !voltage.is_finite() || voltage <= 0.0 {
                return Err(HaresError::Equipment(
                    "EV l1_voltage_v must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(power_w) = self.heater_power_w {
            if !power_w.is_finite() || power_w < 0.0 {
                return Err(HaresError::Equipment(
                    "EV heater_power_w must be finite and >= 0".to_string(),
                ));
            }
        }
        if let Some(mass) = self.thermal_mass_j_per_k {
            if !mass.is_finite() || mass <= 0.0 {
                return Err(HaresError::Equipment(
                    "EV thermal_mass_j_per_k must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(ua) = self.ua_w_per_k {
            if !ua.is_finite() || ua < 0.0 {
                return Err(HaresError::Equipment(
                    "EV ua_w_per_k must be finite and >= 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("v2l_soc_reserve", self.v2l_soc_reserve),
            ("v2g_soc_reserve", self.v2g_soc_reserve),
            ("ready_soc", self.ready_soc),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(HaresError::Equipment(format!(
                        "EV {name} must be finite and within [0, 1]"
                    )));
                }
            }
        }
        for (name, val) in [
            ("v2l_max_discharge_kw", self.v2l_max_discharge_kw),
            ("v2g_max_discharge_kw", self.v2g_max_discharge_kw),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v < 0.0 {
                    return Err(HaresError::Equipment(format!(
                        "EV {name} must be finite and >= 0"
                    )));
                }
            }
        }
        if let Some(pf) = self.power_factor {
            if !pf.is_finite() || pf <= 0.0 || pf > 1.0 {
                return Err(HaresError::Equipment(
                    "EV power_factor must be finite and within (0, 1]".to_string(),
                ));
            }
        }
        if let Some(kva) = self.charger_capacity_kva {
            if !kva.is_finite() || kva <= 0.0 {
                return Err(HaresError::Equipment(
                    "EV charger_capacity_kva must be finite and > 0".to_string(),
                ));
            }
        }
        Ok(())
    }
}
