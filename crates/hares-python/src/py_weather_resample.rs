//! The engine's per-step weather channels, exposed: `hares.resample_weather`
//! returns the run's internal weather series as a polars frame, one row
//! per step of the configured horizon.

use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict};

use hares_io::{WeatherFormat, WeatherInput};

use crate::py_inputs::PyParsedWeather;

/// The `weather` argument: a path, content with its format (a
/// `(text, format)` pair), or a `ParsedWeather` handle.
fn weather_arg(obj: &Bound<'_, PyAny>) -> PyResult<WeatherInput> {
    if let Ok(handle) = obj.extract::<PyRef<'_, PyParsedWeather>>() {
        let input = handle.inner.record();
        // A handle's own source: the path form re-reads nothing (the
        // handle shares its parse below); content keeps its format.
        return Ok(match input {
            WeatherInput::Path(path) => WeatherInput::Path(path.clone()),
            WeatherInput::Text { content, format } => WeatherInput::Text {
                content: Arc::clone(content),
                format: *format,
            },
            WeatherInput::Bytes { content, format } => WeatherInput::Bytes {
                content: Arc::clone(content),
                format: *format,
            },
        });
    }
    if let Ok((text, format)) = obj.extract::<(String, String)>() {
        let format = weather_format(&format)?;
        return Ok(WeatherInput::Text {
            content: text.into(),
            format,
        });
    }
    if let Ok(path) = obj.extract::<String>() {
        return Ok(WeatherInput::Path(path.into()));
    }
    Err(PyTypeError::new_err(
        "weather must be a path, a (text, format) pair, or a ParsedWeather",
    ))
}

fn weather_format(name: &str) -> PyResult<WeatherFormat> {
    match name {
        "epw" => Ok(WeatherFormat::Epw),
        "psm3" => Ok(WeatherFormat::Psm3),
        "res_stock_csv" => Ok(WeatherFormat::ResStockCsv),
        "tmy3" => Ok(WeatherFormat::Tmy3),
        other => Err(PyValueError::new_err(format!(
            "unknown weather format '{other}': expected one of \
             'epw', 'psm3', 'res_stock_csv', 'tmy3'"
        ))),
    }
}

/// The `site` argument: the site dict's four values, the form
/// `DwellingBlueprint.validate()`'s `site` entry carries. A missing
/// coordinate is a `KeyError` naming it; the dict's source keys, when
/// present, are not read (no source changes the resampled weather).
fn site_arg(obj: &Bound<'_, PyAny>) -> PyResult<hares_io::SiteLocation> {
    let dict = obj
        .cast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("site must be a dict of the resolved site's values"))?;
    let field = |name: &str| -> PyResult<f64> {
        match dict.get_item(name)? {
            Some(value) => value
                .extract::<f64>()
                .map_err(|_| PyValueError::new_err(format!("site['{name}'] must be a number"))),
            None => Err(PyKeyError::new_err(format!(
                "site is missing the resolved coordinate '{name}'"
            ))),
        }
    };
    let latitude_deg = field("latitude_deg")?;
    let longitude_deg = field("longitude_deg")?;
    let elevation_m = field("elevation_m")?;
    let utc_offset_h = field("utc_offset_h")?;
    // The caller supplied every value: every field's source is the
    // caller's override.
    let source = hares_io::FieldSource::CallerOverride;
    Ok(hares_io::SiteLocation {
        latitude_deg,
        longitude_deg,
        elevation_m,
        utc_offset_h,
        latitude_source: source,
        longitude_source: source,
        elevation_source: source,
        utc_offset_source: source,
    })
}

/// The engine's per-step weather channels for one weather input, site and
/// config: one row per step of the configured horizon, the `Time` column
/// and the weather channels the run's own state carries, in the
/// catalogue's names. The evaluation is the environment update's own, so
/// the channels match the run's internal series bitwise.
///
/// Args:
///     weather: a path, a `(text, format)` pair, or a `ParsedWeather`.
///     site: the resolved site dict (`latitude_deg`, `longitude_deg`,
///         `elevation_m`, `utc_offset_h`).
///     config: the run's `SimulationConfig`.
///
/// Returns:
///     A polars `DataFrame`.
#[pyfunction]
#[pyo3(signature = (weather, site, config))]
pub fn resample_weather<'py>(
    py: Python<'py>,
    weather: &Bound<'_, PyAny>,
    site: &Bound<'_, PyAny>,
    config: &Bound<'_, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let weather = weather_arg(weather)?;
    let site = site_arg(site)?;
    let config = config
        .extract::<pyo3::PyRef<'_, crate::py_config::PySimulationConfig>>()
        .map_err(|_| PyTypeError::new_err("config must be a SimulationConfig"))?;

    // The parse and the resample: the GIL released (the weather parse is
    // the cost), the frame built from the batch's own buffers.
    let config = config.0.clone();
    let batch = py
        .detach(move || hares_core::resample_weather(&weather, &site, &config))
        .map_err(|err| PyValueError::new_err(err.to_string()))?;

    let table = crate::conversions::batches_to_py_table(vec![batch])?;
    let table = pyo3::Py::new(py, table)?;
    let polars = py.import("polars")?;
    let df = polars.getattr("DataFrame")?.call1((table,))?;
    Ok(df)
}
