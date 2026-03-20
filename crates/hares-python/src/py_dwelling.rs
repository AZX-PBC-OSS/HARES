//! Python bindings for dwelling simulation.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use hares_control::PriceSignal;
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{OutputFormat, SimulationConfig};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyType};

use crate::conversions::steps_to_polars_df;
use crate::py_control::PyControlSignal;
use crate::py_telemetry::PyTelemetry;

const DEFAULT_START: &str = "2019-01-01T00:00:00Z";
const DEFAULT_DURATION_S: i64 = 24 * 60 * 60;
const DEFAULT_STEP_S: i64 = 60;

#[pyclass(name = "PyDwelling")]
pub struct PyDwelling {
    pub(crate) dwelling: Mutex<Dwelling>,
    pub(crate) config: DwellingConfig,
    initial_state: Option<Vec<u8>>,
    initialized: bool,
}

#[pymethods]
impl PyDwelling {
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, **kwargs))]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: String,
        schedule: String,
        weather: String,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let config = build_config(hpxml, schedule, weather, kwargs)?;
        let dwelling = Dwelling::from_config(config.clone()).map_err(to_py_err)?;
        Ok(Self {
            dwelling: Mutex::new(dwelling),
            config,
            initial_state: None,
            initialized: false,
        })
    }

    pub fn initialize(&mut self) -> PyResult<()> {
        if self.initialized || self.initial_state.is_some() {
            self.initialized = true;
            return Ok(());
        }

        let bytes = {
            let dwelling = self
                .dwelling
                .lock()
                .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
            serde_json::to_vec(&dwelling.save_checkpoint()).map_err(to_py_err)?
        };

        self.initial_state = Some(bytes);
        self.initialized = true;
        Ok(())
    }

    pub fn timesteps(&self) -> PyResult<PyTimestepsIter> {
        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;

        let start = dwelling.clock.start_time;
        let time_res_s = dwelling.clock.time_res.num_seconds();
        let total_steps = dwelling.clock.total_steps();
        let current_step = dwelling.clock.current_step();

        Ok(PyTimestepsIter {
            start,
            time_res_s,
            next_step: current_step,
            total_steps,
        })
    }

    pub fn simulate(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let steps = py
            .detach(|| {
                let mut dwelling = self
                    .dwelling
                    .lock()
                    .map_err(|_| "failed to lock dwelling state".to_string())?;
                let results = dwelling.simulate().map_err(|err| err.to_string())?;
                Ok::<_, String>(results.steps)
            })
            .map_err(PyValueError::new_err)?;

        steps_to_polars_df(py, &steps)
    }

    pub fn step(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let step = py
            .detach(|| self.step_core())
            .map_err(PyValueError::new_err)?;

        let out = PyDict::new(py);
        out.set_item("time", chrono_to_py_datetime(py, step.timestamp)?)?;
        out.set_item("net_electric_power_kw", step.net_electric_power_kw)?;
        Ok(out.unbind().into())
    }

    pub fn apply_control(&self, name: String, signal: &PyControlSignal) -> PyResult<()> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling.apply_control(&name, signal.signal.clone());
        Ok(())
    }

    pub fn set_price_signal(&self, signal: &Bound<'_, PyDict>) -> PyResult<()> {
        let price = PriceSignal {
            electricity_price: dict_optional(signal, "electricity_price")?,
            export_price: dict_optional(signal, "export_price")?,
            ghg_intensity: dict_optional(signal, "ghg_intensity")?,
        };
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling.set_price_signal(price);
        Ok(())
    }

    pub fn set_grid_voltage(&self, voltage_pu: f64) -> PyResult<()> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling.set_grid_voltage(voltage_pu);
        Ok(())
    }

    pub fn telemetry(&self) -> PyResult<PyTelemetry> {
        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        Ok(PyTelemetry::new(dwelling.telemetry()))
    }

    pub fn results(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        steps_to_polars_df(py, &dwelling.results().steps)
    }

    pub fn reset_with_seed(&mut self, seed: u64) -> PyResult<()> {
        let mut config = self.config.clone();
        config.sim_config.master_seed = seed;

        let dwelling = Dwelling::from_config(config.clone()).map_err(to_py_err)?;
        self.dwelling = Mutex::new(dwelling);
        self.config = config;
        self.initialized = false;
        self.initial_state = None;

        self.initialize()
    }

    pub fn save_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        let bytes = serde_json::to_vec(&dwelling.save_checkpoint()).map_err(to_py_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    pub fn load_state(&self, state: &[u8]) -> PyResult<()> {
        let checkpoint = serde_json::from_slice(state).map_err(to_py_err)?;
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling.load_checkpoint(checkpoint).map_err(to_py_err)
    }
}

impl PyDwelling {
    pub(crate) fn step_core(&self) -> Result<hares_core::StepResult, String> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| "failed to lock dwelling state".to_string())?;
        dwelling.step().map_err(|err| err.to_string())
    }

    pub(crate) fn observation(&self) -> Result<Vec<f64>, String> {
        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| "failed to lock dwelling state".to_string())?;
        let telemetry = dwelling.telemetry();
        telemetry
            .to_observation_vec(&["total_power_kw", "outdoor_temp", "outdoor_rh"])
            .map_err(|err| err.to_string())
    }
}

#[pyclass(name = "TimestepsIter")]
pub struct PyTimestepsIter {
    start: DateTime<Utc>,
    time_res_s: i64,
    next_step: u64,
    total_steps: u64,
}

#[pymethods]
impl PyTimestepsIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        if self.next_step >= self.total_steps {
            return Ok(None);
        }

        let ts = self.start
            + Duration::seconds(
                self.time_res_s
                    .saturating_mul(i64::try_from(self.next_step).unwrap_or(i64::MAX)),
            );
        self.next_step = self.next_step.saturating_add(1);
        Ok(Some(chrono_to_py_datetime(py, ts)?))
    }
}

fn build_config(
    hpxml: String,
    schedule: String,
    weather: String,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<DwellingConfig> {
    let kwargs = kwargs.cloned();

    let start_time = kwargs
        .as_ref()
        .and_then(|k| k.get_item("start_time").ok().flatten())
        .map(|obj| extract_datetime(&obj))
        .transpose()?
        .unwrap_or_else(default_start);

    let time_res = Duration::seconds(
        kwargs
            .as_ref()
            .and_then(|k| k.get_item("time_res").ok().flatten())
            .map(|obj| extract_seconds(&obj))
            .transpose()?
            .unwrap_or(DEFAULT_STEP_S),
    );

    let duration = Duration::seconds(
        kwargs
            .as_ref()
            .and_then(|k| k.get_item("duration").ok().flatten())
            .map(|obj| extract_seconds(&obj))
            .transpose()?
            .unwrap_or(DEFAULT_DURATION_S),
    );

    let output_to_parquet = kwargs
        .as_ref()
        .and_then(|k| k.get_item("output_to_parquet").ok().flatten())
        .map(|obj| obj.extract::<bool>())
        .transpose()?
        .unwrap_or(false);

    let output_path = kwargs
        .as_ref()
        .and_then(|k| k.get_item("output_path").ok().flatten())
        .map(|obj| obj.extract::<String>().map(PathBuf::from))
        .transpose()?;

    let output_verbosity = kwargs
        .as_ref()
        .and_then(|k| k.get_item("output_verbosity").ok().flatten())
        .map(|obj| obj.extract::<u8>())
        .transpose()?
        .unwrap_or(0);

    let output_chunk_size = kwargs
        .as_ref()
        .and_then(|k| k.get_item("output_chunk_size").ok().flatten())
        .map(|obj| obj.extract::<usize>())
        .transpose()?
        .unwrap_or(10_000);

    let bldg_id = kwargs
        .as_ref()
        .and_then(|k| k.get_item("bldg_id").ok().flatten())
        .map(|obj| obj.extract::<i64>())
        .transpose()?
        .unwrap_or(0);

    let master_seed = kwargs
        .as_ref()
        .and_then(|k| k.get_item("master_seed").ok().flatten())
        .map(|obj| obj.extract::<u64>())
        .transpose()?
        .unwrap_or(0);

    let sim_config = SimulationConfig {
        start_time,
        duration,
        time_res,
        output_verbosity,
        output_path,
        output_format: if output_to_parquet {
            OutputFormat::Parquet
        } else {
            OutputFormat::Csv
        },
        output_chunk_size,
        setpoint_deadband_c: None,
        master_seed,
    };

    Ok(DwellingConfig {
        hpxml_path: PathBuf::from(hpxml),
        schedule_path: PathBuf::from(schedule),
        weather_path: PathBuf::from(weather),
        defaults_path: None,
        sim_config,
        overrides: None,
        bldg_id,
        initialization_duration: None,
    })
}

fn dict_optional<T>(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<T>>
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

fn extract_datetime(obj: &Bound<'_, PyAny>) -> PyResult<DateTime<Utc>> {
    if let Ok(value) = obj.extract::<String>() {
        return parse_datetime_str(&value);
    }

    let iso: String = obj.call_method0("isoformat")?.extract()?;
    parse_datetime_str(&iso)
}

fn parse_datetime_str(value: &str) -> PyResult<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&naive));
    }

    Err(PyValueError::new_err(format!(
        "failed to parse datetime `{value}`"
    )))
}

fn extract_seconds(obj: &Bound<'_, PyAny>) -> PyResult<i64> {
    if let Ok(v) = obj.extract::<i64>() {
        return Ok(v);
    }

    if let Ok(v) = obj.extract::<f64>() {
        return Ok(v.round() as i64);
    }

    if let Ok(seconds) = obj.getattr("total_seconds")?.call0()?.extract::<f64>() {
        return Ok(seconds.round() as i64);
    }

    Err(PyValueError::new_err(
        "expected seconds as int/float or datetime.timedelta",
    ))
}

fn default_start() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(DEFAULT_START)
        .expect("valid default timestamp")
        .with_timezone(&Utc)
}

fn chrono_to_py_datetime(py: Python<'_>, dt: DateTime<Utc>) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (dt.to_rfc3339(),))?;
    Ok(obj.unbind())
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyValueError::new_err(err.to_string())
}
