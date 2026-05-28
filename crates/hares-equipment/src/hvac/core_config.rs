//! Config parsing helpers for HVAC equipment initialization.

use hares_types::normalize_ascii;
use hares_types::{HaresError, ScheduleSource, ScheduleSourceConfig};
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;
use crate::config::ConfigPayload;

use super::hvac_core::DEFAULT_BIQUADRATIC_COEFFS;
use super::speed_control::SpeedControlMode;

/// Serde default for `number_of_speeds` fields across all HVAC config structs.
/// Centralised here so the same function is not defined independently in
/// cooling_config, heat_pump_config, and heating_config.
pub(super) fn default_one() -> u8 {
    1
}

/// Compile-time-verified equipment type name constants.
///
/// Each constant corresponds to an `EquipmentTypedConfig::equipment_type_name()`
/// implementation. A mismatch between these constants and the trait impls is
/// caught by the test `equipment_type_names_match_constants` in each config module.
/// Adding a new equipment type without adding a constant here will be caught
/// when the matching test is updated.
pub mod equipment_type_name {
    pub const CENTRAL_AC: &str = "Central AC";
    pub const ROOM_AC: &str = "Room AC";
    pub const ASHP_HEATER: &str = "ASHP Heater";
    pub const ASHP_COOLER: &str = "ASHP Cooler";
    pub const DEHUMIDIFIER: &str = "Dehumidifier";
    pub const GAS_FURNACE: &str = "Gas Furnace";
    pub const ELECTRIC_FURNACE: &str = "Electric Furnace";
    pub const GAS_BOILER: &str = "Gas Boiler";
    pub const ELECTRIC_BOILER: &str = "Electric Boiler";
    pub const ELECTRIC_BASEBOARD: &str = "Electric Baseboard";
    pub const IDEAL_HVAC: &str = "Ideal HVAC";
}

/// Resolve the Cd (degradation coefficient) from config keys and equipment-type
/// defaults. Precedence:
///   1. Explicit key: "startup_cd" > "cooling_cd" > "cd"
///   2. Derived from speed_control_mode, SEER, HSPF (equipment-type default)
///   3. DEFAULT_PLF_DEGRADATION_COEFF (0.25)
///
/// OCHRE utils/equipment.py:470–500 `calc_c_d` uses the identical decision table.
/// Variable-speed: 0.0; two-speed: 0.11; single-speed: SEER < 13 → 0.20, else 0.07
/// (cooling) or HSPF < 7 → 0.20, else 0.11 (heating).
pub(super) fn resolve_cd(
    config: &EquipmentConfig,
    speed_mode: SpeedControlMode,
    rated_seer: Option<f64>,
    rated_hspf: Option<f64>,
    default_cd: f64,
) -> f64 {
    if let Some(cd) = extract_numeric(config, "startup_cd")
        .or_else(|| extract_numeric(config, "cooling_cd"))
        .or_else(|| extract_numeric(config, "cd"))
    {
        return cd;
    }
    match speed_mode {
        SpeedControlMode::VariableSpeedIdeal => 0.0,
        SpeedControlMode::TwoSpeedSetpoint
        | SpeedControlMode::TwoSpeedTime
        | SpeedControlMode::TwoSpeedAlternating => 0.11,
        SpeedControlMode::SingleSpeed => {
            let from_seer = rated_seer.map(|s| if s < 13.0 { 0.20 } else { 0.07 });
            let from_hspf = rated_hspf.map(|h| if h < 7.0 { 0.20 } else { 0.11 });
            from_seer.or(from_hspf).unwrap_or(default_cd)
        }
        SpeedControlMode::MultiSpeedInterpolated => default_cd,
    }
}

/// Shared duct configuration fields for heating/cooling equipment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuctConfig {
    /// Heating duct distribution system efficiency (DSE), [0, 1].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "duct_dse",
        alias = "duct_distribution_efficiency"
    )]
    pub dse_heat: Option<f64>,
    /// Cooling duct distribution system efficiency (DSE), [0, 1].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dse_cool: Option<f64>,
    /// Explicit airflow in m^3/s/W for ducted heating equipment.
    ///
    /// When present, this carries the HPXML explicit airflow field through the
    /// typed furnace config so equipment init can skip nominal airflow defaults
    /// and airflow defect adjustments.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "duct_airflow_m3_s_per_w"
    )]
    pub airflow_m3_s_per_w: Option<f64>,
    /// Zone where duct losses are deposited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_zone_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_house_volume_m3: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_supply_leakage_frac: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_supply_area_m2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_supply_r_m2_k_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_return_leakage_frac: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_return_area_m2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_return_r_m2_k_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duct_zone_type: Option<String>,
}

fn typed_value<'a>(config: &'a EquipmentConfig, key: &str) -> Option<&'a serde_json::Value> {
    match &config.payload {
        ConfigPayload::Typed { data, .. } => {
            data.get(key)
                .or_else(|| data.get("setpoint").and_then(|sp| sp.get(key)))
        }
        ConfigPayload::Raw { .. } => None,
    }
}

fn extract_schedule_source(config: &EquipmentConfig, key: &str) -> Option<ScheduleSource> {
    typed_value(config, key)
        .and_then(|value| serde_json::from_value::<ScheduleSourceConfig>(value.clone()).ok())
        .map(ScheduleSourceConfig::into_runtime)
}

#[cfg(not(test))]
pub(super) fn extract_numeric(config: &EquipmentConfig, key: &str) -> Option<f64> {
    typed_value(config, key).and_then(serde_json::Value::as_f64)
}

#[cfg(test)]
pub(super) fn extract_numeric(config: &EquipmentConfig, key: &str) -> Option<f64> {
    config
        .get_f64(key)
        .or_else(|| typed_value(config, key).and_then(serde_json::Value::as_f64))
}

#[cfg(not(test))]
pub(super) fn extract_text<'a>(config: &'a EquipmentConfig, key: &str) -> Option<&'a str> {
    typed_value(config, key).and_then(serde_json::Value::as_str)
}

#[cfg(test)]
pub(super) fn extract_text<'a>(config: &'a EquipmentConfig, key: &str) -> Option<&'a str> {
    config
        .get_str(key)
        .or_else(|| typed_value(config, key).and_then(serde_json::Value::as_str))
}

#[cfg(not(test))]
pub(super) fn extract_bool(config: &EquipmentConfig, key: &str) -> Option<bool> {
    typed_value(config, key)
        .and_then(serde_json::Value::as_bool)
        .or_else(|| extract_numeric(config, key).map(|v| v > 0.0))
}

#[cfg(test)]
pub(super) fn extract_bool(config: &EquipmentConfig, key: &str) -> Option<bool> {
    config
        .get_bool(key)
        .or_else(|| typed_value(config, key).and_then(serde_json::Value::as_bool))
        .or_else(|| extract_numeric(config, key).map(|v| v > 0.0))
}

/// Build a `ScheduleSource` from typed config params for the given prefix ("heating" or "cooling").
///
/// Runtime direction is typed-only: callers should populate `{prefix}_setpoint_source`
/// with a serialized `ScheduleSourceConfig`.
#[cfg(not(test))]
pub(super) fn build_setpoint_source(
    config: &EquipmentConfig,
    prefix: &str,
) -> Option<ScheduleSource> {
    let source_key = format!("{prefix}_setpoint_source");
    extract_schedule_source(config, &source_key)
}

#[cfg(test)]
pub(super) fn build_setpoint_source(
    config: &EquipmentConfig,
    prefix: &str,
) -> Option<ScheduleSource> {
    let source_key = format!("{prefix}_setpoint_source");
    extract_schedule_source(config, &source_key)
}

pub(super) fn load_bounds_pair(
    config: &EquipmentConfig,
    min_key: &str,
    max_key: &str,
    default: (f64, f64),
) -> (f64, f64) {
    let lo = extract_numeric(config, min_key).unwrap_or(default.0);
    let hi = extract_numeric(config, max_key).unwrap_or(default.1);
    (lo, hi)
}

pub(super) fn parse_speed_control_mode(
    config: &EquipmentConfig,
) -> crate::Result<SpeedControlMode> {
    let from_text = config.get_str("speed_control_mode").map(normalize_ascii);
    if let Some(value) = from_text {
        return match value.as_str() {
            "single" | "single_speed" | "single-speed" => Ok(SpeedControlMode::SingleSpeed),
            "two" | "two_speed" | "two-speed" | "two_speed_setpoint" => {
                Ok(SpeedControlMode::TwoSpeedSetpoint)
            }
            "two_speed_time" | "two-speed-time" | "time" => Ok(SpeedControlMode::TwoSpeedTime),
            "two_speed_alternating" | "two-speed-alternating" | "time2" | "alternating" => {
                Ok(SpeedControlMode::TwoSpeedAlternating)
            }
            "four"
            | "four_speed"
            | "four-speed"
            | "multi_speed"
            | "multi-speed"
            | "multi_speed_interpolated" => Ok(SpeedControlMode::MultiSpeedInterpolated),
            "variable" | "variable_speed" | "variable-speed" | "ideal" => {
                Ok(SpeedControlMode::VariableSpeedIdeal)
            }
            _ => Err(HaresError::Equipment(format!(
                "unrecognised speed_control_mode '{value}'; \
                 expected one of: single, single_speed, single-speed, two, two_speed, two-speed, \
                 two_speed_setpoint, two_speed_time, two-speed-time, time, two_speed_alternating, \
                 two-speed-alternating, time2, alternating, four, four_speed, four-speed, \
                 multi_speed, multi-speed, multi_speed_interpolated, variable, variable_speed, \
                 variable-speed, ideal"
            ))),
        };
    }
    match config.get_f64("speed_control_mode") {
        None => Ok(SpeedControlMode::SingleSpeed),
        Some(1.0) => Ok(SpeedControlMode::SingleSpeed),
        Some(2.0) => Ok(SpeedControlMode::TwoSpeedSetpoint),
        Some(4.0) => Ok(SpeedControlMode::MultiSpeedInterpolated),
        Some(3.0) | Some(0.0) => Ok(SpeedControlMode::VariableSpeedIdeal),
        Some(v) => Err(HaresError::Equipment(format!(
            "unrecognised numeric speed_control_mode {v}; expected one of: 0, 1, 2, 3, 4"
        ))),
    }
}

/// Load per-speed PLR quadratic coefficients `[a, b, c]` from config.
///
/// Key: `"eir_plr_coefficients"` -- a comma/space-separated list of floats.
/// Must be a multiple of 3. Returns `None` when the key is absent or empty.
pub(super) fn load_plr_coefficients(
    config: &EquipmentConfig,
    key: &str,
) -> crate::Result<Option<Vec<[f64; 3]>>> {
    let Some(raw) = config.get_str(key) else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    let mut numbers = Vec::new();
    let mut token = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            token.push(ch);
        } else if !token.is_empty() {
            let value = token.parse::<f64>().map_err(|err| {
                HaresError::Equipment(format!("invalid eir_plr coefficient '{token}': {err}"))
            })?;
            numbers.push(value);
            token.clear();
        }
    }
    if !token.is_empty() {
        let value = token.parse::<f64>().map_err(|err| {
            HaresError::Equipment(format!("invalid eir_plr coefficient '{token}': {err}"))
        })?;
        numbers.push(value);
    }

    if numbers.is_empty() {
        return Ok(None);
    }
    if !numbers.len().is_multiple_of(3) {
        return Err(HaresError::Equipment(format!(
            "eir_plr_coefficients must be a multiple of 3 values, got {}",
            numbers.len()
        )));
    }

    let curves = numbers
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    Ok(Some(curves))
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
        return Err(HaresError::Equipment(
            "biquadratic coefficient list contained zero parseable coefficients".to_string(),
        ));
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

pub(super) fn load_biquadratic_coeffs(
    config: &EquipmentConfig,
    key: &str,
) -> crate::Result<Vec<[f64; 6]>> {
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

/// Parse a 3-element f64 array from a bracketed string like `[1.0, 0.0, 0.0]`.
pub(super) fn parse_f64_array_3(raw: &str) -> crate::Result<[f64; 3]> {
    let mut numbers = Vec::new();
    let mut token = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            token.push(ch);
        } else if !token.is_empty() {
            let value = token.parse::<f64>().map_err(|err| {
                HaresError::Equipment(format!("invalid coefficient '{token}': {err}"))
            })?;
            numbers.push(value);
            token.clear();
        }
    }
    if !token.is_empty() {
        let value = token.parse::<f64>().map_err(|err| {
            HaresError::Equipment(format!("invalid coefficient '{token}': {err}"))
        })?;
        numbers.push(value);
    }
    if numbers.len() != 3 {
        return Err(HaresError::Equipment(format!(
            "expected 3 coefficients, got {}",
            numbers.len()
        )));
    }
    Ok([numbers[0], numbers[1], numbers[2]])
}
