//! Shared utilities for Python bindings.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

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
