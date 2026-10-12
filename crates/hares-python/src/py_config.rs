//! Python bindings for simulation and dwelling configuration.
//!
//! Every construction path builds the one [`SimulationConfig`]: the Python
//! constructor wraps it directly (`PySimulationConfig(SimulationConfig)`),
//! its keywords are the Rust field names and its defaults the Rust serde
//! defaults, so a config built here resolves exactly like one parsed from
//! TOML.

use chrono::Duration;
use hares_core::DwellingConfig;
use hares_io::{HealthPolicy, ReportSpec, SimulationConfig, SiteLocationOverride};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyType};

use crate::utils::parse_datetime_str;
use crate::utils::python_to_json_value;

pub(crate) const MAX_CHRONO_SECONDS: i64 = i64::MAX / 1_000;

/// Whole seconds from a Python `int` or `datetime.timedelta`: the two
/// duration forms the TOML scalar and the plan's segment arithmetic carry.
/// A non-whole-second `timedelta` is a `ValueError`; anything else is a
/// `TypeError` naming the type, as Python's own extraction is.
#[derive(Debug, Clone, Copy)]
pub struct WholeSeconds(pub i64);

impl<'a, 'py> FromPyObject<'a, 'py> for WholeSeconds {
    type Error = PyErr;

    fn extract(obj: pyo3::Borrowed<'a, 'py, PyAny>) -> Result<Self, PyErr> {
        if let Ok(v) = obj.extract::<i64>() {
            return Ok(Self(v));
        }
        let type_name = obj
            .get_type()
            .name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let type_name = type_name.as_str();
        let is_timedelta = type_name == "timedelta";
        if !is_timedelta {
            return Err(PyTypeError::new_err(format!(
                "expected int seconds or a datetime.timedelta, got {type_name}"
            )));
        }
        let total: f64 = obj
            .call_method0("total_seconds")
            .and_then(|v| v.extract())
            .map_err(|_| PyValueError::new_err("timedelta seconds are outside the f64 range"))?;
        if !total.is_finite() {
            return Err(PyValueError::new_err(format!(
                "timedelta must be finite seconds, got {total}"
            )));
        }
        if total.fract() != 0.0 {
            return Err(PyValueError::new_err(format!(
                "timedelta must be a whole number of seconds, got {total}"
            )));
        }
        let rounded = total.round();
        if !(i64::MIN as f64..=i64::MAX as f64).contains(&rounded) {
            return Err(PyValueError::new_err(format!(
                "timedelta is outside the seconds range a chrono Duration carries: {total}"
            )));
        }
        Ok(Self(rounded as i64))
    }
}

/// Validate the first unexpected keyword against `scopes`' removed keys and
/// raise: the removed-key message when one matches, else Python's own
/// unexpected-keyword `TypeError` naming the key.
fn unexpected_keyword_error(
    class: &str,
    unexpected: &Bound<'_, PyDict>,
    scopes: &[hares_io::KeyScope],
) -> PyResult<()> {
    // The first unexpected keyword is the one named, as Python's own
    // check does; the rest are irrelevant to the raised error.
    if let Some(key) = unexpected.keys().into_iter().next() {
        let key_str: String = key.extract()?;
        for scope in scopes {
            if let Some(message) = hares_io::removed_key_message(*scope, &key_str) {
                return Err(PyTypeError::new_err(message));
            }
        }
        return Err(PyTypeError::new_err(format!(
            "{class}.__new__() got an unexpected keyword argument '{key_str}'"
        )));
    }
    Ok(())
}

/// The explicit site-location override, one frozen Python object over
/// [`hares_io::SiteLocationOverride`]. Its constructor runs the same
/// site-location check `SimulationConfig::validate` runs and raises that
/// message as `ValueError`.
#[pyclass(name = "SiteLocationOverride", frozen, from_py_object)]
#[derive(Debug, Clone)]
pub struct PySiteLocationOverride(pub SiteLocationOverride);

#[pymethods]
impl PySiteLocationOverride {
    #[new]
    #[pyo3(signature = (
        *,
        latitude_deg = None,
        longitude_deg = None,
        elevation_m = None,
        utc_offset_h = None,
    ))]
    pub fn new(
        latitude_deg: Option<f64>,
        longitude_deg: Option<f64>,
        elevation_m: Option<f64>,
        utc_offset_h: Option<f64>,
    ) -> PyResult<Self> {
        let inner = SiteLocationOverride {
            latitude_deg,
            longitude_deg,
            elevation_m,
            utc_offset_h,
        };
        inner
            .validate()
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok(Self(inner))
    }

    #[getter]
    pub fn latitude_deg(&self) -> Option<f64> {
        self.0.latitude_deg
    }

    #[getter]
    pub fn longitude_deg(&self) -> Option<f64> {
        self.0.longitude_deg
    }

    #[getter]
    pub fn elevation_m(&self) -> Option<f64> {
        self.0.elevation_m
    }

    #[getter]
    pub fn utc_offset_h(&self) -> Option<f64> {
        self.0.utc_offset_h
    }

    pub fn __repr__(&self) -> String {
        format!(
            "SiteLocationOverride(latitude_deg={}, longitude_deg={}, elevation_m={}, utc_offset_h={})",
            opt_f64(self.0.latitude_deg),
            opt_f64(self.0.longitude_deg),
            opt_f64(self.0.elevation_m),
            opt_f64(self.0.utc_offset_h),
        )
    }
}

fn opt_f64(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v}"),
        None => "None".to_string(),
    }
}

/// The run's health policy: which counted health events escalate to a run
/// failure, and which stay counted. The empty policy (the default) keeps
/// every event counted and nothing escalated; `strict` rejects an
/// evaluation on its first counted event. The class is frozen: a policy is
/// built once and passed to a config.
#[pyclass(name = "HealthPolicy", frozen, from_py_object)]
#[derive(Debug, Clone)]
pub struct PyHealthPolicy(pub HealthPolicy);

#[pymethods]
impl PyHealthPolicy {
    #[new]
    #[pyo3(signature = (
        *,
        max_port_rollbacks = None,
        max_rejected_control_signals = None,
        max_clamped_actions = None,
        max_curve_index_clamps = None,
        require_warmup_convergence = false
    ))]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_port_rollbacks: Option<u64>,
        max_rejected_control_signals: Option<u64>,
        max_clamped_actions: Option<u64>,
        max_curve_index_clamps: Option<u64>,
        require_warmup_convergence: bool,
    ) -> Self {
        Self(HealthPolicy {
            max_port_rollbacks,
            max_rejected_control_signals,
            max_clamped_actions,
            max_curve_index_clamps,
            require_warmup_convergence,
        })
    }

    /// The policy that rejects an evaluation on its first counted event:
    /// every limit 0 and warm-up convergence required.
    #[classmethod]
    pub fn strict(_cls: &Bound<'_, PyType>) -> Self {
        Self(HealthPolicy::strict())
    }

    #[getter]
    pub fn max_port_rollbacks(&self) -> Option<u64> {
        self.0.max_port_rollbacks
    }

    #[getter]
    pub fn max_rejected_control_signals(&self) -> Option<u64> {
        self.0.max_rejected_control_signals
    }

    #[getter]
    pub fn max_clamped_actions(&self) -> Option<u64> {
        self.0.max_clamped_actions
    }

    #[getter]
    pub fn max_curve_index_clamps(&self) -> Option<u64> {
        self.0.max_curve_index_clamps
    }

    #[getter]
    pub fn require_warmup_convergence(&self) -> bool {
        self.0.require_warmup_convergence
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.0 == other.0
    }

    pub fn __repr__(&self) -> String {
        let limit = |value: Option<u64>| match value {
            Some(v) => format!("Some({v})"),
            None => "None".to_string(),
        };
        format!(
            "HealthPolicy(max_port_rollbacks={}, max_rejected_control_signals={}, \
             max_clamped_actions={}, max_curve_index_clamps={}, \
             require_warmup_convergence={})",
            limit(self.0.max_port_rollbacks),
            limit(self.0.max_rejected_control_signals),
            limit(self.0.max_clamped_actions),
            limit(self.0.max_curve_index_clamps),
            self.0.require_warmup_convergence,
        )
    }
}

/// The one simulation config: a newtype over [`SimulationConfig`], so the
/// TOML path, the synthetic TOML path and this constructor build the same
/// typed config with the same defaults.
#[pyclass(name = "SimulationConfig", from_py_object)]
#[derive(Debug, Clone)]
pub struct PySimulationConfig(pub SimulationConfig);

#[pymethods]
impl PySimulationConfig {
    #[new]
    #[pyo3(signature = (
        *,
        start_time,
        duration,
        time_res = WholeSeconds(hares_io::DEFAULT_TIME_RES_SECS),
        reports = None,
        diagnostics = false,
        metrics = false,
        output_chunk_size = hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
        setpoint_deadband_c = None,
        master_seed = 0,
        civil_timezone = None,
        site_location = None,
        max_consecutive_step_failures = hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy = None,
        **unexpected
    ))]
    // PyO3 #[new] with kwargs maps 1:1 to Python kwargs; builder adds no value here.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_time: &str,
        duration: WholeSeconds,
        time_res: WholeSeconds,
        reports: Option<Vec<crate::py_report::PyReportSpec>>,
        diagnostics: bool,
        metrics: bool,
        output_chunk_size: usize,
        setpoint_deadband_c: Option<f64>,
        master_seed: u64,
        civil_timezone: Option<String>,
        site_location: Option<PySiteLocationOverride>,
        max_consecutive_step_failures: u32,
        health_policy: Option<PyHealthPolicy>,
        unexpected: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        if let Some(unexpected) = unexpected {
            unexpected_keyword_error(
                "SimulationConfig",
                unexpected,
                &[
                    hares_io::KeyScope::SimulationConfig,
                    hares_io::KeyScope::PythonKeyword,
                ],
            )?;
        }

        if duration.0 <= 0 {
            return Err(PyValueError::new_err("duration must be positive"));
        }
        if time_res.0 <= 0 {
            return Err(PyValueError::new_err("time_res must be positive"));
        }

        let start = parse_datetime_str(start_time)?;

        let inner = SimulationConfig {
            start_time: start,
            duration: Duration::try_seconds(duration.0).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "duration too large for chrono Duration: {}",
                    duration.0
                ))
            })?,
            time_res: Duration::try_seconds(time_res.0).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "time_res too large for chrono Duration: {}",
                    time_res.0
                ))
            })?,
            reports: reports
                .unwrap_or_default()
                .into_iter()
                .map(|report| report.0)
                .collect::<Vec<ReportSpec>>(),
            diagnostics,
            metrics,
            output_chunk_size,
            setpoint_deadband_c,
            master_seed,
            civil_timezone,
            site_location: site_location.map(|s| s.0).unwrap_or_default(),
            max_consecutive_step_failures,
            health_policy: health_policy.map(|p| p.0).unwrap_or_default(),
        };
        inner
            .validate()
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok(Self(inner))
    }

    /// The one validation every construction path runs.
    pub fn validate(&self) -> PyResult<()> {
        self.0
            .validate()
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    /// Serialise to TOML: `from_toml`'s inverse, so a config built here
    /// round-trips the same way a parsed one does.
    pub fn to_toml(&self) -> PyResult<String> {
        self.0
            .to_toml()
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    #[classmethod]
    pub fn from_toml(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        let cfg =
            SimulationConfig::from_toml(s).map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok(Self(cfg))
    }

    #[getter]
    pub fn start_time(&self) -> String {
        self.0.start_time.to_rfc3339()
    }

    #[setter]
    pub fn set_start_time(&mut self, value: String) -> PyResult<()> {
        let parsed = parse_datetime_str(&value)?;
        self.assign(|cfg| cfg.start_time = parsed)
    }

    #[getter]
    pub fn duration(&self) -> i64 {
        self.0.duration.num_seconds()
    }

    #[setter]
    pub fn set_duration(&mut self, value: WholeSeconds) -> PyResult<()> {
        let d = Duration::try_seconds(value.0).ok_or_else(|| {
            PyValueError::new_err(format!(
                "duration too large for chrono Duration: {}",
                value.0
            ))
        })?;
        self.assign(|cfg| cfg.duration = d)
    }

    #[getter]
    pub fn time_res(&self) -> i64 {
        self.0.time_res.num_seconds()
    }

    #[setter]
    pub fn set_time_res(&mut self, value: WholeSeconds) -> PyResult<()> {
        let d = Duration::try_seconds(value.0).ok_or_else(|| {
            PyValueError::new_err(format!(
                "time_res too large for chrono Duration: {}",
                value.0
            ))
        })?;
        self.assign(|cfg| cfg.time_res = d)
    }

    /// Whether the run requests the diagnostics report: the per-step
    /// diagnostics variables join the run's schema, and the per-step
    /// diagnostics recording runs. Without it a run records no diagnostics
    /// variable and pays nothing for them.
    #[getter]
    pub fn diagnostics(&self) -> bool {
        self.0.diagnostics
    }

    #[setter]
    pub fn set_diagnostics(&mut self, value: bool) -> PyResult<()> {
        self.assign(|cfg| cfg.diagnostics = value)
    }

    /// Whether the run requests the run metrics: a fixed metrics input
    /// plan freezes from the catalogue and the accumulator is fed per
    /// step. Without the request the run builds no metrics plan and no
    /// accumulator, feeds nothing per step, and its result carries no
    /// metrics.
    #[getter]
    pub fn metrics(&self) -> bool {
        self.0.metrics
    }

    #[setter]
    pub fn set_metrics(&mut self, value: bool) -> PyResult<()> {
        self.assign(|cfg| cfg.metrics = value)
    }

    #[getter]
    pub fn reports(&self) -> Vec<crate::py_report::PyReportSpec> {
        self.0
            .reports
            .iter()
            .cloned()
            .map(crate::py_report::PyReportSpec)
            .collect()
    }

    #[setter]
    pub fn set_reports(&mut self, value: Vec<crate::py_report::PyReportSpec>) -> PyResult<()> {
        self.assign(move |cfg| {
            cfg.reports = value.into_iter().map(|report| report.0).collect();
        })
    }

    #[getter]
    pub fn output_chunk_size(&self) -> usize {
        self.0.output_chunk_size
    }

    #[setter]
    pub fn set_output_chunk_size(&mut self, value: usize) -> PyResult<()> {
        self.assign(|cfg| cfg.output_chunk_size = value)
    }

    #[getter]
    pub fn setpoint_deadband_c(&self) -> Option<f64> {
        self.0.setpoint_deadband_c
    }

    #[setter]
    pub fn set_setpoint_deadband_c(&mut self, value: Option<f64>) -> PyResult<()> {
        self.assign(|cfg| cfg.setpoint_deadband_c = value)
    }

    #[getter]
    pub fn master_seed(&self) -> u64 {
        self.0.master_seed
    }

    #[setter]
    pub fn set_master_seed(&mut self, value: u64) -> PyResult<()> {
        self.assign(|cfg| cfg.master_seed = value)
    }

    #[getter]
    pub fn civil_timezone(&self) -> Option<String> {
        self.0.civil_timezone.clone()
    }

    #[setter]
    pub fn set_civil_timezone(&mut self, value: Option<String>) -> PyResult<()> {
        self.assign(|cfg| cfg.civil_timezone = value)
    }

    #[getter]
    pub fn site_location(&self) -> PySiteLocationOverride {
        PySiteLocationOverride(self.0.site_location)
    }

    #[setter]
    pub fn set_site_location(&mut self, value: PySiteLocationOverride) -> PyResult<()> {
        let site = value.0;
        self.assign(move |cfg| cfg.site_location = site)
    }

    #[getter]
    pub fn max_consecutive_step_failures(&self) -> u32 {
        self.0.max_consecutive_step_failures
    }

    #[setter]
    pub fn set_max_consecutive_step_failures(&mut self, value: u32) -> PyResult<()> {
        self.assign(|cfg| cfg.max_consecutive_step_failures = value)
    }

    #[getter]
    pub fn health_policy(&self) -> PyHealthPolicy {
        PyHealthPolicy(self.0.health_policy.clone())
    }

    #[setter]
    pub fn set_health_policy(&mut self, value: PyHealthPolicy) -> PyResult<()> {
        let policy = value.0;
        self.assign(move |cfg| cfg.health_policy = policy)
    }

    pub fn __repr__(&self) -> String {
        format!(
            "SimulationConfig(start_time='{}', duration={}, time_res={}, reports={}, master_seed={})",
            self.0.start_time.to_rfc3339(),
            self.0.duration.num_seconds(),
            self.0.time_res.num_seconds(),
            self.0.reports.len(),
            self.0.master_seed,
        )
    }
}

impl PySimulationConfig {
    /// Set a field through the one validation: the candidate config is
    /// validated and only a valid one is stored, so a config object is
    /// valid after every successful mutation.
    fn assign<F>(&mut self, f: F) -> PyResult<()>
    where
        F: FnOnce(&mut SimulationConfig),
    {
        let mut candidate = self.0.clone();
        f(&mut candidate);
        candidate
            .validate()
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        self.0 = candidate;
        Ok(())
    }
}

/// The dwelling-level config, a wrapper over [`DwellingConfig`]: the same
/// field names the Rust struct carries, with the sim config passed as the
/// one `SimulationConfig` object.
#[pyclass(name = "DwellingConfig", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyDwellingConfig(pub DwellingConfig);

/// The dwelling-constructor arguments every Python-facing dwelling
/// constructor shares: the input triple, the one config object and the
/// dwelling-level keywords. One struct, so the dwelling config's, the
/// dwelling's and the blueprint's constructors forward one shape instead
/// of nine arguments. Every input argument is either a path/content
/// source or an already-parsed handle: the handle is shared (its parse is
/// used as is), the source is parsed in the constructor's detached
/// section.
pub(crate) struct DwellingConfigArgs<'a> {
    pub hpxml: crate::py_inputs::HpxmlArg,
    pub schedule: Option<crate::py_inputs::ScheduleArg>,
    pub weather: crate::py_inputs::WeatherArg,
    pub defaults: crate::py_inputs::DefaultsArg,
    pub config: PySimulationConfig,
    pub bldg_id: i64,
    pub initialization_duration: Option<i64>,
    pub overrides: Option<&'a Bound<'a, PyDict>>,
    pub unexpected: Option<&'a Bound<'a, PyDict>>,
}

fn parse_overrides(overrides: Option<&Bound<'_, PyDict>>) -> PyResult<Option<serde_json::Value>> {
    let Some(dict) = overrides else {
        return Ok(None);
    };
    let mut map = serde_json::Map::new();
    for key in dict.keys() {
        let key_str = key.extract::<String>()?;
        let value = dict.get_item(&key)?.ok_or_else(|| {
            PyValueError::new_err(format!("missing value for override key: {key_str}"))
        })?;
        let json_value = python_to_json_value(value.as_ref())?;
        map.insert(key_str, json_value);
    }
    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::Value::Object(map)))
    }
}

#[pymethods]
impl PyDwellingConfig {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        hpxml,
        schedule,
        weather,
        config,
        *,
        bldg_id = 0,
        initialization_duration = None,
        defaults_path = None,
        overrides = None,
        **unexpected,
    ))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hpxml: &Bound<'_, PyAny>,
        schedule: Option<&Bound<'_, PyAny>>,
        weather: &Bound<'_, PyAny>,
        config: PySimulationConfig,
        bldg_id: i64,
        initialization_duration: Option<i64>,
        defaults_path: Option<&Bound<'_, PyAny>>,
        overrides: Option<&Bound<'_, PyDict>>,
        unexpected: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        Self::from_args(DwellingConfigArgs {
            hpxml: crate::py_inputs::hpxml_arg(hpxml)?,
            schedule: crate::py_blueprint::schedule_arg_maybe(schedule)?,
            weather: crate::py_inputs::weather_arg(weather)?,
            defaults: match defaults_path {
                Some(path) => crate::py_inputs::defaults_arg(path)?,
                None => crate::py_inputs::DefaultsArg::Source(hares_io::DefaultsSource::default()),
            },
            config,
            bldg_id,
            initialization_duration,
            overrides,
            unexpected,
        })
    }

    #[getter]
    pub fn hpxml(&self) -> String {
        self.0.hpxml.display()
    }

    #[getter]
    pub fn schedule(&self) -> Option<String> {
        self.0.schedule.as_ref().map(|source| source.display())
    }

    #[getter]
    pub fn weather(&self) -> String {
        match self.0.weather.path() {
            Some(path) => path.display().to_string(),
            None => "<weather content>".to_string(),
        }
    }

    #[getter]
    pub fn config(&self) -> PySimulationConfig {
        PySimulationConfig(self.0.settings.sim_config.clone())
    }

    #[getter]
    pub fn defaults_path(&self) -> Option<String> {
        self.0
            .defaults
            .base
            .as_ref()
            .map(|p| p.display().to_string())
    }

    #[getter]
    pub fn bldg_id(&self) -> i64 {
        self.0.settings.bldg_id
    }

    #[getter]
    pub fn initialization_duration(&self) -> Option<i64> {
        self.0
            .settings
            .initialization_duration
            .map(|d| d.as_secs() as i64)
    }

    #[getter]
    pub fn overrides(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        let Some(serde_json::Value::Object(map)) = self.0.settings.overrides.as_ref() else {
            return Ok(None);
        };
        let dict = PyDict::new(py);
        for (k, v) in map {
            let json_str = serde_json::to_string(v).unwrap_or_default();
            let py_obj: Py<PyAny> = py
                .import("json")?
                .call_method1("loads", (json_str,))?
                .into();
            dict.set_item(k, py_obj)?;
        }
        Ok(Some(dict.into()))
    }

    pub fn __repr__(&self) -> String {
        let schedule = self
            .schedule()
            .map(|s| format!("'{s}'"))
            .unwrap_or_else(|| "None".to_string());
        format!(
            "DwellingConfig(hpxml='{}', schedule={}, weather='{}', bldg_id={})",
            self.hpxml(),
            schedule,
            self.weather(),
            self.0.settings.bldg_id
        )
    }
}

impl PyDwellingConfig {
    /// The one dwelling-config construction the Python-facing constructors
    /// share.
    ///
    /// # Errors
    ///
    /// A removed keyword, a negative `initialization_duration`, or an
    /// override that is not a JSON object.
    pub(crate) fn from_args(args: DwellingConfigArgs<'_>) -> PyResult<Self> {
        // Every unexpected keyword is either a removed key this surface once
        // carried (the removed-keys table's message names where the
        // capability lives now) or a typo Python itself would refuse. The
        // first offending keyword is named, as Python's own check does.
        // Every scope is in the lookup, so a removed key names its
        // replacement on the dwelling constructors too, not only on the
        // config's own.
        if let Some(unexpected) = args.unexpected {
            unexpected_keyword_error(
                "DwellingConfig",
                unexpected,
                &[
                    hares_io::KeyScope::DwellingTable,
                    hares_io::KeyScope::SimulationConfig,
                    hares_io::KeyScope::PythonKeyword,
                ],
            )?;
        }

        if let Some(init_dur) = args.initialization_duration
            && init_dur < 0
        {
            return Err(PyValueError::new_err(
                "initialization_duration must be non-negative",
            ));
        }

        let parsed_overrides = parse_overrides(args.overrides)?;

        Ok(Self(DwellingConfig {
            hpxml: match args.hpxml {
                crate::py_inputs::HpxmlArg::Source(source) => source,
                crate::py_inputs::HpxmlArg::Parsed(ref handle) => handle.record().clone(),
            },
            schedule: args.schedule.map(|arg| match arg {
                crate::py_inputs::ScheduleArg::Source(source) => source,
                crate::py_inputs::ScheduleArg::Parsed(ref handle) => handle
                    .record()
                    .cloned()
                    .unwrap_or_else(|| hares_io::InputSource::Text(std::sync::Arc::from(""))),
            }),
            weather: match args.weather {
                crate::py_inputs::WeatherArg::Source(source) => source,
                crate::py_inputs::WeatherArg::Parsed(ref handle) => handle.record().clone(),
            },
            defaults: match args.defaults {
                crate::py_inputs::DefaultsArg::Source(source) => source,
                crate::py_inputs::DefaultsArg::Parsed(ref handle) => handle.record().clone(),
            },
            settings: hares_core::DwellingSettings::new(
                args.config.0,
                parsed_overrides,
                args.bldg_id,
                args.initialization_duration
                    .filter(|&d| d != 0)
                    .map(|d| std::time::Duration::from_secs(d as u64)),
                None,
            ),
        }))
    }

    pub fn to_dwelling_config(&self) -> PyResult<DwellingConfig> {
        Ok(self.0.clone())
    }
}

/// The constructor's detached-section inputs: every input argument in its
/// handle-or-source form (Send: no Python object inside), the generated
/// schedule's span from the sim config. The detached section parses the
/// sources and assembles the builder's inputs from them.
pub(crate) struct ConstructorInputs {
    pub hpxml: crate::py_inputs::HpxmlArg,
    pub schedule: Option<crate::py_inputs::ScheduleArg>,
    pub weather: crate::py_inputs::WeatherArg,
    pub defaults: crate::py_inputs::DefaultsArg,
    pub generation: Option<hares_io::ScheduleGeneration>,
}

/// The one dwelling-constructor resolution the Python-facing constructors
/// share, part one (the GIL held): the config's validation and the
/// sources' record forms, plus the Send form the constructor's detached
/// section assembles. A schedule source absent generates from the
/// defaults handle's profiles over the settings' span.
///
/// # Errors
///
/// The config's own errors: a removed keyword, a negative
/// `initialization_duration`, an override that is not a JSON object.
pub(crate) fn resolve_args(
    args: DwellingConfigArgs<'_>,
) -> PyResult<(DwellingConfig, ConstructorInputs)> {
    let generation = hares_io::ScheduleGeneration {
        start: args.config.0.start_time,
        duration: args.config.0.duration,
        interval: std::time::Duration::from_secs(
            args.config
                .0
                .time_res
                .num_seconds()
                .clamp(1, i64::from(u32::MAX)) as u64,
        ),
    };
    let detached = ConstructorInputs {
        hpxml: args.hpxml.clone(),
        schedule: args.schedule.clone(),
        weather: args.weather.clone(),
        defaults: args.defaults.clone(),
        generation: Some(generation),
    };
    let config = PyDwellingConfig::from_args(args)?.0;
    Ok((config, detached))
}

#[cfg(test)]
mod tests {
    use pyo3::prelude::*;
    use pyo3::types::PyDict;
    use pyo3::types::PyDictMethods;

    use super::PySimulationConfig;
    use super::PySiteLocationOverride;
    use super::WholeSeconds;
    use hares_core::Dwelling;
    use hares_io::SimulationConfig;

    /// The Python constructor with only the two required keywords: the same
    /// logical configuration the TOML string carries.
    fn required_only(start_time: &str, duration: i64) -> PyResult<SimulationConfig> {
        let cfg = PySimulationConfig::new(
            start_time,
            WholeSeconds(duration),
            WholeSeconds(hares_io::DEFAULT_TIME_RES_SECS),
            None,
            false,
            false,
            hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
            None,
            0,
            None,
            None,
            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            None,
            None,
        )?;
        Ok(cfg.0)
    }

    /// The differential test: the same logical configuration through the
    /// Python constructor and `from_toml` resolves field for field equal,
    /// defaults included.
    #[test]
    fn python_and_toml_defaults_come_from_one_table() {
        Python::attach(|_py| {
            let py_cfg =
                required_only("2024-01-01T00:00:00Z", 3600).expect("the python constructor builds");
            let toml_cfg = SimulationConfig::from_toml(
                "\nstart_time = \"2024-01-01T00:00:00Z\"\nduration = 3600\n",
            )
            .expect("from_toml");
            assert_eq!(
                py_cfg, toml_cfg,
                "the same logical configuration through the Python constructor and the \
                 TOML path must resolve field for field equal"
            );
        });
    }

    /// A keyword in `unexpected` is looked up in the `SimulationConfig` and
    /// `PythonKeyword` scopes and raises the removed-key message; any other
    /// is a `TypeError` naming it, as Python's own is.
    #[test]
    fn python_removed_keyword_names_replacement() {
        Python::attach(|py| {
            for (key, replacement) in [
                ("latitude", "site_location"),
                ("longitude", "site_location"),
                ("elevation_m", "site_location"),
                ("utc_offset_h", "site_location"),
                ("output_to_parquet", "reports"),
                ("write_output", "reports"),
                ("output_format", "reports"),
                ("output_path", "reports"),
                ("output_verbosity", "reports"),
                ("retain_batches", "reports"),
                ("rotation", "reports"),
                ("duration_s", "duration"),
                ("time_res_s", "time_res"),
            ] {
                let dict = PyDict::new(py);
                dict.set_item(key, 1.0).expect("set item");
                let err = PySimulationConfig::new(
                    "2024-01-01T00:00:00Z",
                    WholeSeconds(3600),
                    WholeSeconds(hares_io::DEFAULT_TIME_RES_SECS),
                    None,
                    false,
                    false,
                    hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                    None,
                    0,
                    None,
                    None,
                    hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                    None,
                    Some(&dict),
                )
                .expect_err("a removed keyword is an error");
                let message = err.to_string();
                assert!(
                    message.contains(key) && message.contains(replacement),
                    "the error names {key} and its replacement {replacement}, got: {message}"
                );
                assert!(
                    err.is_instance_of::<pyo3::exceptions::PyTypeError>(py),
                    "the removed keyword is a TypeError, like Python's own"
                );
            }
            let dict = PyDict::new(py);
            dict.set_item("not_a_real_keyword", 1.0).expect("set item");
            let err = PySimulationConfig::new(
                "2024-01-01T00:00:00Z",
                WholeSeconds(3600),
                WholeSeconds(hares_io::DEFAULT_TIME_RES_SECS),
                None,
                false,
                false,
                hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                None,
                0,
                None,
                None,
                hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                None,
                Some(&dict),
            )
            .expect_err("an unknown keyword is an error");
            assert!(
                err.to_string().contains("not_a_real_keyword"),
                "the error names the keyword, as Python's own does, got: {err}"
            );
        });
    }

    /// The same removed key gives the same message from the synthetic
    /// TOML's [simulation] table and from the Python keyword surface.
    #[test]
    fn removed_key_messages_equal_across_toml_and_python() {
        Python::attach(|py| {
            let dict = PyDict::new(py);
            dict.set_item("duration_s", 3600).expect("set item");
            let py_err = PySimulationConfig::new(
                "2024-01-01T00:00:00Z",
                WholeSeconds(3600),
                WholeSeconds(hares_io::DEFAULT_TIME_RES_SECS),
                None,
                false,
                false,
                hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                None,
                0,
                None,
                None,
                hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                None,
                Some(&dict),
            )
            .expect_err("duration_s is a removed keyword");
            let toml_err = {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("removed_key.toml");
                std::fs::write(
                    &path,
                    "building_id = 1\n\n[simulation]\nstart_time = \"2024-01-01T00:00:00Z\"\n\
                     time_res = 60\nduration = 3600\nduration_s = 3600\n",
                )
                .expect("write temp TOML");
                let Err(err) = Dwelling::from_toml_config(&path) else {
                    panic!("duration_s is a removed [simulation] key");
                };
                err.to_string()
            };
            // Each surface prefixes its own exception class; the removed-key
            // message both raise is the one string the table carries.
            let canonical =
                hares_io::removed_key_message(hares_io::KeyScope::SimulationConfig, "duration_s")
                    .expect("duration_s is a removed key of the simulation config");
            assert!(
                py_err.to_string().contains(&canonical) && toml_err.contains(&canonical),
                "one removed key, one message, on both surfaces; python gives \
                 {py_err}, the synthetic [simulation] table gives {toml_err}"
            );
        });
    }
    /// `SiteLocationOverride(latitude_deg=95.0)` raises the message
    /// `validate()` runs, and the pyclass round-trips its fields.
    #[test]
    fn site_location_override_constructor_validates() {
        Python::attach(|_py| {
            let site = PySiteLocationOverride::new(Some(40.0), None, None, None)
                .expect("a valid override constructs");
            assert_eq!(site.0.latitude_deg, Some(40.0));
            let err = PySiteLocationOverride::new(Some(95.0), None, None, None)
                .expect_err("an out-of-range latitude is a ValueError");
            assert!(
                err.to_string().contains("latitude") && err.to_string().contains("[-90, 90]"),
                "the message names the field and its range, got: {err}"
            );
        });
    }
}
