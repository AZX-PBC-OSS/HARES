//! Shared utilities for Python bindings.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use hares_io::ResampleMethod;
use hares_types::DayFilter;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{json, Map, Value};

/// Insert an optional f64 value into a JSON map under the given key.
pub fn insert_opt(map: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(v) = value {
        map.insert(key.into(), json!(v));
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
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        let utc_offset =
            FixedOffset::east_opt(0).ok_or_else(|| PyValueError::new_err("invalid UTC offset"))?;
        return Ok(utc_offset.from_utc_datetime(&naive));
    }

    // Try date-only format (assume midnight UTC)
    if let Ok(naive) =
        NaiveDateTime::parse_from_str(&format!("{}T00:00:00", value), "%Y-%m-%dT%H:%M:%S")
    {
        let utc_offset =
            FixedOffset::east_opt(0).ok_or_else(|| PyValueError::new_err("invalid UTC offset"))?;
        return Ok(utc_offset.from_utc_datetime(&naive));
    }

    Err(PyValueError::new_err(format!(
        "invalid start_time format: '{}'. Expected ISO 8601 format (e.g., '2019-01-01T00:00:00Z')",
        value
    )))
}

/// Parse a datetime from a Python object (string or has isoformat method).
pub fn extract_datetime(obj: &Bound<'_, PyAny>) -> PyResult<DateTime<FixedOffset>> {
    if let Ok(value) = obj.extract::<String>() {
        return parse_datetime_str(&value);
    }

    let iso: String = obj.call_method0("isoformat")?.extract()?;
    parse_datetime_str(&iso)
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
