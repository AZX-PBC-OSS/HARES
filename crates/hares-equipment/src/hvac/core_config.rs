//! Config parsing helpers for HVAC equipment initialization.

use hares_types::normalize_ascii;
use hares_types::{BoundaryPolicy, HaresError, ScheduleSource};
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

use super::hvac_core::DEFAULT_BIQUADRATIC_COEFFS;
use super::speed_control::SpeedControlMode;

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

pub(super) fn extract_numeric(config: &EquipmentConfig, key: &str) -> Option<f64> {
    config.get_f64(key)
}

pub(super) fn extract_bool(config: &EquipmentConfig, key: &str) -> Option<bool> {
    config
        .get_bool(key)
        .or_else(|| config.get_f64(key).map(|v| v > 0.0))
}

/// Build a `ScheduleSource` from config params for the given prefix ("heating" or "cooling").
///
/// Priority:
///   1. `{prefix}_setpoint_schedule_col` → `ColumnRef` (per-timestep CSV column)
///   2. `{prefix}_weekday_setpoints_c` + `{prefix}_weekend_setpoints_c` → `DailyProfile`
///   3. None — static setpoints will be used
pub(super) fn build_setpoint_source(
    config: &EquipmentConfig,
    prefix: &str,
) -> Option<ScheduleSource> {
    // Priority 1: per-timestep CSV column index
    let col_key = format!("{prefix}_setpoint_schedule_col");
    if let Some(col) = extract_numeric(config, &col_key) {
        return Some(ScheduleSource::ColumnRef {
            col_idx: col as usize,
            boundary: BoundaryPolicy::Clamp,
        });
    }

    // Priority 2: 24-hour weekday/weekend profiles
    let wd_key = format!("{prefix}_weekday_setpoints_c");
    let we_key = format!("{prefix}_weekend_setpoints_c");
    let weekday = config.get_f64_array(&wd_key);
    let weekend = config.get_f64_array(&we_key);
    if let Some(wd) = weekday {
        let we = weekend.unwrap_or(wd);
        return Some(ScheduleSource::DailyProfile {
            weekday: slice_to_24(wd),
            weekend: slice_to_24(we),
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
    }

    None
}

pub(super) fn slice_to_24(src: &[f64]) -> [f64; 24] {
    let mut arr = [0.0; 24];
    let n = src.len().min(24);
    arr[..n].copy_from_slice(&src[..n]);
    if let Some(&last) = src.last() {
        arr[n..].fill(last);
    }
    arr
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

pub(super) fn parse_speed_control_mode(config: &EquipmentConfig) -> SpeedControlMode {
    let from_text = config.get_str("speed_control_mode").map(normalize_ascii);
    if let Some(value) = from_text {
        return match value.as_str() {
            "single" | "single_speed" | "single-speed" => SpeedControlMode::SingleSpeed,
            "two" | "two_speed" | "two-speed" | "two_speed_setpoint" => {
                SpeedControlMode::TwoSpeedSetpoint
            }
            "two_speed_time" | "two-speed-time" | "time" => SpeedControlMode::TwoSpeedTime,
            "two_speed_alternating" | "two-speed-alternating" | "time2" | "alternating" => {
                SpeedControlMode::TwoSpeedAlternating
            }
            "four"
            | "four_speed"
            | "four-speed"
            | "multi_speed"
            | "multi-speed"
            | "multi_speed_interpolated" => SpeedControlMode::MultiSpeedInterpolated,
            "variable" | "variable_speed" | "variable-speed" | "ideal" => {
                SpeedControlMode::VariableSpeedIdeal
            }
            _ => SpeedControlMode::SingleSpeed,
        };
    }
    match config.get_f64("speed_control_mode") {
        Some(2.0) => SpeedControlMode::TwoSpeedSetpoint,
        Some(4.0) => SpeedControlMode::MultiSpeedInterpolated,
        Some(3.0) | Some(0.0) => SpeedControlMode::VariableSpeedIdeal,
        _ => SpeedControlMode::SingleSpeed,
    }
}

/// Load per-speed PLR quadratic coefficients `[a, b, c]` from config.
///
/// Key: `"eir_plr_coefficients"` — a comma/space-separated list of floats.
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
