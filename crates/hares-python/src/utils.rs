//! Shared utilities for Python bindings.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use hares_equipment::EquipmentConfig;
use hares_io::{EquipmentSpec, ResampleMethod};
use hares_types::{DayFilter, FuelType};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{Map, Value, json};

/// Insert an optional value into a JSON map under the given key.
/// Generic over any `Serialize` type (f64, u16, u8, bool, String, etc.).
pub fn insert_opt<T: serde::Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(v) = value {
        map.insert(key.into(), json!(v));
    }
}

/// Construct an [`EquipmentSpec`] with the common defaults shared by all
/// `*_spec_from_py` functions.
pub fn make_spec(
    canonical_name: &str,
    instance_name: &str,
    fuel_type: FuelType,
    params: Map<String, Value>,
    typed_config: Option<EquipmentConfig>,
) -> EquipmentSpec {
    EquipmentSpec {
        name: canonical_name.to_string(),
        instance_name: Some(instance_name.to_string()),
        fuel_type,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

/// Case-insensitive day filter parser shared by tariff and enum modules.
pub fn parse_day_filter(s: &str) -> PyResult<DayFilter> {
    match s.to_lowercase().as_str() {
        "any" => Ok(DayFilter::Any),
        "weekdays" => Ok(DayFilter::Weekdays),
        "weekends" => Ok(DayFilter::Weekends),
        "monday" => Ok(DayFilter::Day(chrono::Weekday::Mon)),
        "tuesday" => Ok(DayFilter::Day(chrono::Weekday::Tue)),
        "wednesday" => Ok(DayFilter::Day(chrono::Weekday::Wed)),
        "thursday" => Ok(DayFilter::Day(chrono::Weekday::Thu)),
        "friday" => Ok(DayFilter::Day(chrono::Weekday::Fri)),
        "saturday" => Ok(DayFilter::Day(chrono::Weekday::Sat)),
        "sunday" => Ok(DayFilter::Day(chrono::Weekday::Sun)),
        _ => Err(PyValueError::new_err(format!(
            "unknown day filter '{s}', expected 'any', 'weekdays', 'weekends', or a day name"
        ))),
    }
}

/// Parse a datetime string from Python to a FixedOffset DateTime.
///
/// Accepts ISO 8601 formats:
/// - RFC3339: "2019-01-01T00:00:00Z" or "2019-01-01T00:00:00+00:00"
/// - ISO format with T separator: "2019-01-01T00:00:00"
pub fn parse_datetime_str(value: &str) -> PyResult<DateTime<FixedOffset>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt);
    }

    // Try NaiveDateTime with T separator and assume UTC
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(assume_utc(naive));
    }

    // Try date-only format (assume midnight UTC)
    if let Ok(naive) =
        NaiveDateTime::parse_from_str(&format!("{}T00:00:00", value), "%Y-%m-%dT%H:%M:%S%.f")
    {
        return Ok(assume_utc(naive));
    }

    Err(PyValueError::new_err(format!(
        "invalid start_time format: '{}'. Expected ISO 8601 format (e.g., '2019-01-01T00:00:00Z')",
        value
    )))
}

/// Assign UTC offset (east 0) to a [`NaiveDateTime`].
///
/// FixedOffset::east_opt(0) always returns Some; offset zero is a valid fixed offset.
fn assume_utc(naive: NaiveDateTime) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("zero east offset always valid")
        .from_utc_datetime(&naive)
}

/// Parse a datetime from a Python object (string or has isoformat method).
pub fn extract_datetime(obj: &Bound<'_, PyAny>) -> PyResult<DateTime<FixedOffset>> {
    if let Ok(value) = obj.extract::<String>() {
        return parse_datetime_str(&value);
    }

    let iso: String = obj.call_method0("isoformat")?.extract()?;
    parse_datetime_str(&iso)
}

/// Extract a duration in seconds from a Python object.
///
/// Accepts:
/// - raw `int`/`float` seconds
/// - `datetime.timedelta` objects (via `total_seconds()`)
///
/// Values are rounded to the nearest integer second. Returns an error for
/// non-finite, out-of-range, or unrecognised inputs.
pub fn extract_seconds(obj: &Bound<'_, PyAny>) -> PyResult<i64> {
    if let Ok(v) = obj.extract::<i64>() {
        return Ok(v);
    }

    if let Ok(v) = obj.extract::<f64>() {
        return ok_f64_seconds(v);
    }

    if let Ok(seconds) = obj.getattr("total_seconds")?.call0()?.extract::<f64>() {
        return ok_f64_seconds(seconds);
    }

    Err(PyValueError::new_err(
        "expected seconds as int/float or datetime.timedelta",
    ))
}

/// Round and validate a floating-point duration value for conversion to [`i64`].
///
/// Rejects non-finite values and values outside the safe `i64` range.
/// Includes a debug-assertion round-trip check when debug assertions are enabled.
pub fn ok_f64_seconds(v: f64) -> PyResult<i64> {
    if !v.is_finite() {
        return Err(PyValueError::new_err(format!(
            "duration must be finite, got {v}"
        )));
    }
    let rounded = v.round();
    if rounded < (i64::MIN as f64) || rounded > (i64::MAX as f64) {
        return Err(PyValueError::new_err(format!(
            "duration too large for i64: {v}"
        )));
    }
    let result = rounded as i64;
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert!(
            (result as f64 - v).abs() < 1.0,
            "extract_seconds: round-trip error f64 {v} -> i64 {result} exceeds 1-second tolerance"
        );
    }
    Ok(result)
}

/// Parse a resample method name string, returning a typed [`ResampleMethod`]
/// or a `PyValueError` listing the valid methods.
///
/// Matching is case-sensitive. All `ResampleMethod` variant names and the
/// HPXML 4.2 data dictionary use lowercase identifiers; case-sensitivity
/// catches typos like `"Zoh"` that would otherwise go unnoticed.
pub fn parse_resample_method(field: &str, value: &str) -> PyResult<ResampleMethod> {
    match value {
        "pchip" => Ok(ResampleMethod::Pchip),
        "pchip_cyclic" => Ok(ResampleMethod::PchipCyclic),
        "zoh" => Ok(ResampleMethod::Zoh),
        "linear" => Ok(ResampleMethod::Linear),
        "circular_linear" => Ok(ResampleMethod::CircularLinear),
        "triangular" => Ok(ResampleMethod::Triangular),
        other => Err(PyValueError::new_err(format!(
            "unknown resample method '{other}' for field '{field}'; \
             valid methods: zoh, pchip, pchip_cyclic, linear, circular_linear, triangular"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_valid_methods_parse_successfully() {
        for method in &[
            "pchip",
            "pchip_cyclic",
            "zoh",
            "linear",
            "circular_linear",
            "triangular",
        ] {
            assert!(
                parse_resample_method("test_field", method).is_ok(),
                "valid method '{method}' should parse successfully"
            );
        }
    }

    #[test]
    fn typo_returns_value_error_with_message() {
        let result = parse_resample_method("dry_bulb", "tringular");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("tringular"),
            "error message should contain the invalid name, got: {msg}"
        );
        assert!(
            msg.contains("dry_bulb"),
            "error message should contain the field name, got: {msg}"
        );
        assert!(
            msg.contains("valid methods"),
            "error message should list valid methods, got: {msg}"
        );
    }

    #[test]
    fn empty_string_returns_value_error() {
        let result = parse_resample_method("ghi", "");
        assert!(result.is_err());
    }

    #[test]
    fn random_garbage_returns_value_error() {
        let result = parse_resample_method("dni", "not_a_real_method");
        assert!(result.is_err());
    }
}
