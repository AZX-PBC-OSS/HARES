//! Shared utilities for Python bindings.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use hares_equipment::EquipmentConfig;
use hares_io::{EquipmentSpec, ResampleMethod};
use hares_types::{DayFilter, FuelType};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::{Map, Value, json};

/// The one Python→JSON boundary converter — shared by every free-form
/// Value channel into the dwelling build (equipment `overrides`, the
/// `zip` sub-channel, tariff `from_dict`, config overrides).
///
/// Non-finite floats are **rejected loudly**: serde_json writes them as
/// `null` (JSON cannot represent them), and downstream an `Option<f64>`
/// field deserializes `null` as `None` or the raw converter drops the
/// key — so a NaN/±inf value would silently become the field's
/// documented default with no error anywhere, the silent-substitution
/// class the equipment typed-config boundary rejects through
/// `EquipmentConfig::from_typed`'s finite walk. A pandas-fed pipeline
/// producing a NaN (an unset column, a bad join) must fail at the
/// boundary, not silently run a differently-configured dwelling or
/// tariff. The three former per-module copies of this converter are
/// unified here — one reviewed converter, not two reviewed and one
/// missed (the DRY failure that let the class survive its own sweep).
pub(crate) fn python_to_json_value(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // Bool first: Python bool is a subclass of int and extracts as f64.
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(json!(i));
    }
    if let Ok(f) = obj.extract::<f64>() {
        // Rejects genuine non-finite floats (`float("nan")`, ±inf) —
        // the values serde_json would silently null. Python ints too
        // large for i64 (e.g. 10**400) do NOT reach this arm: pyo3's f64
        // extraction of such ints fails in practice, so they fall
        // through to the unsupported-type catch-all below and are
        // rejected loudly there — both layers stay loud, and the
        // huge-int face is pinned by
        // `test_non_finite_overrides_value_fails_loudly`
        // (`tests/python/test_py_config.py`, layer unpinned by design —
        // the boundary contract is the loudness, not the raising layer).
        if !f.is_finite() {
            return Err(PyValueError::new_err(format!(
                "non-finite float value ({f}) cannot cross the Python boundary: \
                 JSON cannot represent non-finite floats and the field's \
                 default would silently apply — pass a finite value or omit \
                 the field"
            )));
        }
        return Ok(json!(f));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(Value::String(s));
    }
    if let Ok(list) = obj.cast::<PyList>() {
        let items: Vec<Value> = list
            .iter()
            .map(|item| python_to_json_value(&item))
            .collect::<PyResult<_>>()?;
        return Ok(Value::Array(items));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut map = Map::new();
        for (key, value) in dict.iter() {
            let key_str: String = key
                .extract()
                .map_err(|_| PyValueError::new_err("dict keys must be strings"))?;
            map.insert(key_str, python_to_json_value(&value)?);
        }
        return Ok(Value::Object(map));
    }
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "?".to_string());
    Err(PyValueError::new_err(format!(
        "cannot convert Python object of type '{type_name}' to JSON"
    )))
}

/// Insert an optional value into a JSON map under the given key.
/// Generic over any `Serialize` type (f64, u16, u8, bool, String, etc.).
pub fn insert_opt<T: serde::Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(v) = value {
        map.insert(key.into(), json!(v));
    }
}

/// Construct an [`EquipmentSpec`] with the common defaults shared by all
/// `*_spec_from_py` functions.
///
/// Rejects any `null` in the spec parameters map, at any depth: the map
/// is built by `json!` on values extracted from Python, and `json!` writes
/// non-finite floats as `null` (JSON cannot represent them) — so a null
/// here means a NaN/±inf field value was silently nulled and the field's
/// documented default would apply downstream with no error anywhere, the
/// silent-substitution class the Python→JSON converter and the
/// `from_typed` finite walk both reject. No legitimate null can appear:
/// `insert_opt` skips `None`s and the explicit `json!` sites insert
/// computed finite values.
pub fn make_spec(
    canonical_name: &str,
    instance_name: &str,
    fuel_type: FuelType,
    params: Map<String, Value>,
    typed_config: Option<EquipmentConfig>,
) -> PyResult<EquipmentSpec> {
    for (key, value) in &params {
        reject_nulled_value(key, value)?;
    }
    Ok(EquipmentSpec {
        name: canonical_name.to_string(),
        instance_name: Some(instance_name.to_string()),
        fuel_type,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    })
}

/// Rejects a `null` anywhere in a spec parameter value tree, naming the
/// key path (e.g. `rated_resistances[2]`).
fn reject_nulled_value(key: &str, value: &Value) -> PyResult<()> {
    match value {
        Value::Null => Err(PyValueError::new_err(format!(
            "non-finite float value at spec parameter '{key}': JSON cannot \
             represent non-finite floats, so the value was nulled and the \
             field's default would silently apply — pass a finite value or \
             omit the field"
        ))),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                reject_nulled_value(&format!("{key}[{i}]"), item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (sub_key, sub_value) in map {
                reject_nulled_value(&format!("{key}.{sub_key}"), sub_value)?;
            }
            Ok(())
        }
        _ => Ok(()),
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

// ---------------------------------------------------------------------------
// Shared Python-boundary f64 validation + dict extraction helpers
// ---------------------------------------------------------------------------
// The loud-at-the-boundary validators for raw f64 crossings (pyo3 kwargs,
// `dict_required`/`dict_optional` extractions). Non-finite floats must be
// rejected **here** — at the boundary that received the user's value —
// never downstream: a NaN's comparisons are always false, so a crossed
// NaN silently stops every consuming conditional (a NaN price never arms
// the price actors; a NaN heating setpoint never heats), and the
// dwelling's downstream finiteness screens are cfg-gated to
// debug/`check_invariants` builds — silent in release. One shared home
// (this file), not per-module copies: the two former `dict_optional`
// copies (py_dwelling.rs, py_control.rs) and py_control.rs's local
// validators are unified here so a future extraction site grabs a
// finite-checking or dict helper from one reviewed place.

/// Reject non-finite f64 values at the boundary, naming the field.
pub(crate) fn validate_finite(value: f64, name: &str) -> PyResult<()> {
    if !value.is_finite() {
        return Err(PyValueError::new_err(format!(
            "{name} must be finite, got {value}"
        )));
    }
    Ok(())
}

/// [`validate_finite`] plus a non-negativity check (the order matters:
/// a NaN fails `value < 0.0` — the one-sided comparison the OCV/U-neg
/// table constructors' positivity checks exhibited — so finiteness is
/// checked first, never relied on the comparison to catch).
pub(crate) fn validate_non_negative(value: f64, name: &str) -> PyResult<()> {
    validate_finite(value, name)?;
    if value < 0.0 {
        return Err(PyValueError::new_err(format!(
            "{name} must be >= 0, got {value}"
        )));
    }
    Ok(())
}

/// [`validate_finite`] plus a `[min, max]` range check (finiteness first,
/// same reason as [`validate_non_negative`]).
pub(crate) fn validate_range(value: f64, min: f64, max: f64, name: &str) -> PyResult<()> {
    validate_finite(value, name)?;
    if value < min || value > max {
        return Err(PyValueError::new_err(format!(
            "{name} must be in [{min}, {max}], got {value}"
        )));
    }
    Ok(())
}

/// Extract a **required** dict value for `key`, mapping extraction
/// failures to a field-named `PyValueError`.
pub(crate) fn dict_required<T>(d: &Bound<'_, PyDict>, key: &str) -> PyResult<T>
where
    T: for<'a, 'py> FromPyObject<'a, 'py>,
{
    let Some(value) = d.get_item(key)? else {
        return Err(PyValueError::new_err(format!(
            "missing required key `{key}`"
        )));
    };
    value
        .extract::<T>()
        .map_err(|_| PyValueError::new_err(format!("invalid value for `{key}`")))
}

/// Extract an **optional** dict value for `key` (`None` and absent →
/// `None`), mapping extraction failures to a field-named `PyValueError`.
pub(crate) fn dict_optional<T>(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: for<'a, 'py> FromPyObject<'a, 'py>,
{
    let Some(value) = d.get_item(key)? else {
        return Ok(None);
    };
    if value.is_none() {
        Ok(None)
    } else {
        Ok(Some(value.extract::<T>().map_err(|_| {
            PyValueError::new_err(format!("invalid value for `{key}`"))
        })?))
    }
}

/// [`dict_required`] for f64 fields — the boundary-guarded spelling: a
/// non-finite value never crosses (see the section comment above).
pub(crate) fn finite_f64_required(d: &Bound<'_, PyDict>, key: &str) -> PyResult<f64> {
    let v: f64 = dict_required(d, key)?;
    validate_finite(v, key)?;
    Ok(v)
}

/// [`dict_optional`] for f64 fields — the boundary-guarded spelling (a
/// present non-finite value never crosses; `None`/absent stay `None`).
pub(crate) fn finite_f64_optional(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<f64>> {
    let v: Option<f64> = dict_optional(d, key)?;
    if let Some(x) = v {
        validate_finite(x, key)?;
    }
    Ok(v)
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
