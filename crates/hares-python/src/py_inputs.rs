//! The parsed-input handles: parse once, share the handle across every
//! dwelling built from it. A handle built from a deleted file still
//! builds dwellings: construction reads the handle, never the file.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};

use hares_io::{
    DefaultsSource, InputSource, ParsedDefaults, ParsedHpxml, ParsedSchedule, ParsedWeather,
    WeatherFormat, WeatherInput,
};

use crate::py_dwelling::{construct_detached, to_py_err};

/// The weather format a text or bytes weather input parses as: the path
/// form detects it from the file, content cannot be re-detected.
fn weather_format_from_str(name: &str) -> PyResult<WeatherFormat> {
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

/// One parsed HPXML document, shared by reference.
#[pyclass(frozen, name = "ParsedHpxml")]
#[doc = "One parsed HPXML document: parse once, build many dwellings."]
pub struct PyParsedHpxml {
    pub(crate) inner: Arc<ParsedHpxml>,
}

#[pymethods]
impl PyParsedHpxml {
    /// Parse the document at `path`, with the GIL released.
    #[classmethod]
    pub fn from_path(_cls: &Bound<'_, PyType>, py: Python<'_>, path: &str) -> PyResult<Self> {
        let source = InputSource::Path(path.into());
        let inner =
            construct_detached(py, move || ParsedHpxml::parse(&source)).map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Parse the document from `text`, with the GIL released.
    #[classmethod]
    pub fn from_text(_cls: &Bound<'_, PyType>, py: Python<'_>, text: &str) -> PyResult<Self> {
        let source = InputSource::Text(text.into());
        let inner =
            construct_detached(py, move || ParsedHpxml::parse(&source)).map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

/// One parsed weather file, shared by reference; each step resolution's
/// resampled series is computed once and shared.
#[pyclass(frozen, name = "ParsedWeather")]
#[doc = "One parsed weather file: parse once, build many dwellings."]
pub struct PyParsedWeather {
    pub(crate) inner: Arc<ParsedWeather>,
}

#[pymethods]
impl PyParsedWeather {
    /// Parse the weather file at `path` (the format detected from the
    /// file), with the GIL released.
    #[classmethod]
    pub fn from_path(_cls: &Bound<'_, PyType>, py: Python<'_>, path: &str) -> PyResult<Self> {
        let input = WeatherInput::Path(path.into());
        let inner =
            construct_detached(py, move || ParsedWeather::parse(&input)).map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Parse the weather `text` as `format` (`'epw'`, `'psm3'`,
    /// `'res_stock_csv'` or `'tmy3'`; content cannot be re-detected), with
    /// the GIL released.
    #[classmethod]
    pub fn from_text(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        text: &str,
        format: &str,
    ) -> PyResult<Self> {
        let format = weather_format_from_str(format)?;
        let input = WeatherInput::Text {
            content: text.into(),
            format,
        };
        let inner =
            construct_detached(py, move || ParsedWeather::parse(&input)).map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// The file header's elevation, metres (the site resolution's
    /// fallback when the HPXML's site declares none).
    #[getter]
    fn elevation_m(&self) -> f64 {
        self.inner.meta().elevation_m
    }

    /// The file header's latitude, degrees.
    #[getter]
    fn latitude(&self) -> f64 {
        self.inner.meta().latitude
    }

    /// The file header's longitude, degrees.
    #[getter]
    fn longitude(&self) -> f64 {
        self.inner.meta().longitude
    }

    /// The file header's UTC offset, hours.
    #[getter]
    fn timezone_offset_h(&self) -> f64 {
        self.inner.meta().timezone_offset_h
    }
}

/// One parsed schedule (a CSV or the generated schedule), shared by
/// reference.
#[pyclass(frozen, name = "ParsedSchedule")]
#[doc = "One parsed schedule: parse once, build many dwellings."]
pub struct PyParsedSchedule {
    pub(crate) inner: Arc<ParsedSchedule>,
}

#[pymethods]
impl PyParsedSchedule {
    /// Parse the schedule CSV at `path`, with the GIL released.
    /// `weather_meta` is the parsed weather whose meta a schedule's
    /// offset-less timestamps resolve against, the same fallback the
    /// config path passes.
    #[classmethod]
    #[pyo3(signature = (path, weather_meta = None))]
    pub fn from_path(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        path: &str,
        weather_meta: Option<&PyParsedWeather>,
    ) -> PyResult<Self> {
        let meta = weather_meta.map(|weather| weather.inner.meta().clone());
        let source = InputSource::Path(path.into());
        let inner = construct_detached(py, move || ParsedSchedule::parse(&source, meta.as_ref()))
            .map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Parse the schedule CSV from `text`, with the GIL released.
    /// `weather_meta` is the parsed weather whose meta a schedule's
    /// offset-less timestamps resolve against, the same fallback the
    /// config path passes.
    #[classmethod]
    #[pyo3(signature = (text, weather_meta = None))]
    pub fn from_text(
        _cls: &Bound<'_, PyType>,
        py: Python<'_>,
        text: &str,
        weather_meta: Option<&PyParsedWeather>,
    ) -> PyResult<Self> {
        let meta = weather_meta.map(|weather| weather.inner.meta().clone());
        let source = InputSource::Text(text.into());
        let inner = construct_detached(py, move || ParsedSchedule::parse(&source, meta.as_ref()))
            .map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

/// The parsed defaults tree: the base directory plus per-file
/// replacements, loaded once and shared.
#[pyclass(frozen, name = "ParsedDefaults")]
#[doc = "The parsed defaults tree: load once, build many dwellings."]
pub struct PyParsedDefaults {
    pub(crate) inner: Arc<ParsedDefaults>,
}

#[pymethods]
impl PyParsedDefaults {
    #[new]
    #[pyo3(signature = (base, files = None))]
    pub fn new(
        py: Python<'_>,
        base: Option<&str>,
        files: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let mut replacements = BTreeMap::new();
        if let Some(files) = files {
            for (key, value) in files.iter() {
                let relative: String = key.extract()?;
                let content: String = value.extract()?;
                replacements.insert(relative, InputSource::Text(content.into()));
            }
        }
        let source = DefaultsSource {
            base: base.map(std::path::PathBuf::from),
            files: replacements,
        };
        let inner =
            construct_detached(py, move || ParsedDefaults::parse(&source)).map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// The defaults files the load read, in load order: the relative
    /// paths of the store's input log, the read list the input schema's
    /// `defaults_files` publishes.
    #[pyo3(name = "read_files")]
    pub fn read_files(&self) -> Vec<String> {
        self.inner.read_files()
    }

    /// The tree with one file replaced: that file re-parsed, every other
    /// table shared with this handle.
    pub fn with_file(&self, py: Python<'_>, relative_path: &str, content: &str) -> PyResult<Self> {
        let inner = construct_detached(py, {
            let inner = Arc::clone(&self.inner);
            let source = InputSource::Text(content.into());
            let relative = relative_path.to_string();
            move || inner.with_file(&relative, source)
        })
        .map_err(to_py_err)?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

/// One Python constructor argument that is either a path/content or an
/// already-parsed handle: the source form parses through the handle seam,
/// the handle form shares its parse.
#[derive(Clone)]
pub(crate) enum HpxmlArg {
    Source(InputSource),
    Parsed(Arc<ParsedHpxml>),
}

#[derive(Clone)]
pub(crate) enum ScheduleArg {
    Source(InputSource),
    Parsed(Arc<ParsedSchedule>),
}

#[derive(Clone)]
pub(crate) enum WeatherArg {
    Source(WeatherInput),
    Parsed(Arc<ParsedWeather>),
}

#[derive(Clone)]
pub(crate) enum DefaultsArg {
    Source(DefaultsSource),
    Parsed(Arc<ParsedDefaults>),
}

/// The `hpxml` argument: a path string or a `ParsedHpxml`.
pub(crate) fn hpxml_arg(obj: &Bound<'_, PyAny>) -> PyResult<HpxmlArg> {
    if let Ok(handle) = obj.extract::<PyRef<'_, PyParsedHpxml>>() {
        return Ok(HpxmlArg::Parsed(Arc::clone(&handle.inner)));
    }
    let path: String = obj
        .extract()
        .map_err(|_| PyValueError::new_err("hpxml must be a path or a ParsedHpxml"))?;
    Ok(HpxmlArg::Source(InputSource::Path(path.into())))
}

/// The `schedule` argument: a path string, a `ParsedSchedule`, or `None`.
pub(crate) fn schedule_arg(obj: &Bound<'_, PyAny>) -> PyResult<Option<ScheduleArg>> {
    if obj.is_none() {
        return Ok(None);
    }
    if let Ok(handle) = obj.extract::<PyRef<'_, PyParsedSchedule>>() {
        return Ok(Some(ScheduleArg::Parsed(Arc::clone(&handle.inner))));
    }
    let path: String = obj
        .extract()
        .map_err(|_| PyValueError::new_err("schedule must be a path, a ParsedSchedule, or None"))?;
    Ok(Some(ScheduleArg::Source(InputSource::Path(path.into()))))
}

/// The `weather` argument: a path string or a `ParsedWeather`.
pub(crate) fn weather_arg(obj: &Bound<'_, PyAny>) -> PyResult<WeatherArg> {
    if let Ok(handle) = obj.extract::<PyRef<'_, PyParsedWeather>>() {
        return Ok(WeatherArg::Parsed(Arc::clone(&handle.inner)));
    }
    let path: String = obj
        .extract()
        .map_err(|_| PyValueError::new_err("weather must be a path or a ParsedWeather"))?;
    Ok(WeatherArg::Source(WeatherInput::Path(path.into())))
}

/// The `defaults_path` argument: a path string or a `ParsedDefaults`.
pub(crate) fn defaults_arg(obj: &Bound<'_, PyAny>) -> PyResult<DefaultsArg> {
    if obj.is_none() {
        return Ok(DefaultsArg::Source(DefaultsSource::default()));
    }
    if let Ok(handle) = obj.extract::<PyRef<'_, PyParsedDefaults>>() {
        return Ok(DefaultsArg::Parsed(Arc::clone(&handle.inner)));
    }
    let path: String = obj
        .extract()
        .map_err(|_| PyValueError::new_err("defaults_path must be a path or a ParsedDefaults"))?;
    Ok(DefaultsArg::Source(DefaultsSource::directory(path)))
}
