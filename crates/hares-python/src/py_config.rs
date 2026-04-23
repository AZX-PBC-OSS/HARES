//! Python bindings for simulation and dwelling configuration.

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::DwellingConfig;
use hares_io::{OutputFormat, ResampleMethod, ResampleOverrides, SimulationConfig};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyList};

use crate::utils::parse_datetime_str;

const DEFAULT_START: &str = "2019-01-01T00:00:00Z";
const DEFAULT_DURATION_S: i64 = 24 * 60 * 60;
const DEFAULT_STEP_S: i64 = 60;
const DEFAULT_CHUNK_SIZE: usize = 10_000;

fn python_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    // Bool must be checked before int/f64 because Python bool is a subclass of int.
    if obj.is_instance_of::<PyBool>() {
        return Ok(serde_json::Value::Bool(obj.extract::<bool>()?));
    }
    // Check for int before f64 since integers can extract as f64.
    if let Ok(n) = obj.extract::<i64>() {
        return Ok(serde_json::Value::Number(n.into()));
    }
    if let Ok(n) = obj.extract::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return Ok(serde_json::Value::Number(num));
        }
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(serde_json::Value::String(s));
    }
    if let Ok(list) = obj.cast::<PyList>() {
        let mut values = Vec::new();
        for item in list.iter() {
            values.push(python_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(values));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut obj_map = serde_json::Map::new();
        for (k, v) in dict.iter() {
            let k_str: String = k.extract()?;
            let json_val = python_to_json(&v)?;
            obj_map.insert(k_str, json_val);
        }
        return Ok(serde_json::Value::Object(obj_map));
    }
    Err(PyValueError::new_err(format!(
        "unsupported Python type for JSON conversion: {:?}",
        obj.get_type().name()
    )))
}

fn default_start_time() -> DateTime<FixedOffset> {
    // SAFETY: DEFAULT_START is a hardcoded constant known to be valid ISO 8601.
    DateTime::parse_from_rfc3339(DEFAULT_START).expect("valid default start time")
}

#[pyclass(name = "SimulationConfig", from_py_object)]
#[derive(Debug, Clone)]
pub struct PySimulationConfig {
    start_time: DateTime<FixedOffset>,
    duration: i64,
    time_res: i64,
    output_verbosity: u8,
    output_path: Option<String>,
    write_output: bool,
    output_to_parquet: bool,
    output_chunk_size: usize,
    setpoint_deadband_c: Option<f64>,
    master_seed: u64,
    civil_timezone: Option<String>,
}

#[pymethods]
impl PySimulationConfig {
    #[new]
    #[pyo3(
        signature = (
            start_time = None,
            duration_s = None,
            time_res_s = None,
            output_verbosity = None,
            output_path = None,
            write_output = None,
            output_to_parquet = None,
            output_chunk_size = None,
            master_seed = None,
            civil_timezone = None,
            setpoint_deadband_c = None,
        )
    )]
    // PyO3 #[new] with kwargs maps 1:1 to Python kwargs; builder adds no value here.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_time: Option<String>,
        duration_s: Option<i64>,
        time_res_s: Option<i64>,
        output_verbosity: Option<u8>,
        output_path: Option<String>,
        write_output: Option<bool>,
        output_to_parquet: Option<bool>,
        output_chunk_size: Option<usize>,
        master_seed: Option<u64>,
        civil_timezone: Option<String>,
        setpoint_deadband_c: Option<f64>,
    ) -> PyResult<Self> {
        let start_time = start_time
            .map(|s| parse_datetime_str(&s))
            .transpose()?
            .unwrap_or_else(default_start_time);

        let duration = duration_s.unwrap_or(DEFAULT_DURATION_S);
        if duration <= 0 {
            return Err(PyValueError::new_err("duration_s must be positive"));
        }

        let time_res = time_res_s.unwrap_or(DEFAULT_STEP_S);
        if time_res <= 0 {
            return Err(PyValueError::new_err("time_res_s must be positive"));
        }

        if duration % time_res != 0 {
            return Err(PyValueError::new_err(format!(
                "duration_s ({}) must be divisible by time_res_s ({})",
                duration, time_res
            )));
        }

        let output_verbosity = output_verbosity.unwrap_or(0);
        if output_verbosity > 8 {
            return Err(PyValueError::new_err("output_verbosity must be 0-8"));
        }

        if let Some(deadband) = setpoint_deadband_c {
            if !deadband.is_finite() || deadband < 0.0 {
                return Err(PyValueError::new_err(
                    "setpoint_deadband_c must be finite and >= 0",
                ));
            }
        }

        let output_chunk_size = output_chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);

        Ok(Self {
            start_time,
            duration,
            time_res,
            output_verbosity,
            output_path,
            write_output: write_output.unwrap_or(true),
            output_to_parquet: output_to_parquet.unwrap_or(false),
            output_chunk_size,
            setpoint_deadband_c,
            master_seed: master_seed.unwrap_or(0),
            civil_timezone,
        })
    }

    #[getter]
    pub fn start_time(&self) -> String {
        self.start_time.to_rfc3339()
    }

    #[setter]
    pub fn set_start_time(&mut self, value: String) -> PyResult<()> {
        self.start_time = parse_datetime_str(&value)?;
        Ok(())
    }

    #[getter]
    pub fn duration_s(&self) -> i64 {
        self.duration
    }

    #[setter]
    pub fn set_duration_s(&mut self, value: i64) -> PyResult<()> {
        if value <= 0 {
            return Err(PyValueError::new_err("duration_s must be positive"));
        }
        self.duration = value;
        Ok(())
    }

    #[getter]
    pub fn time_res_s(&self) -> i64 {
        self.time_res
    }

    #[setter]
    pub fn set_time_res_s(&mut self, value: i64) -> PyResult<()> {
        if value <= 0 {
            return Err(PyValueError::new_err("time_res_s must be positive"));
        }
        self.time_res = value;
        Ok(())
    }

    #[getter]
    pub fn output_verbosity(&self) -> u8 {
        self.output_verbosity
    }

    #[setter]
    pub fn set_output_verbosity(&mut self, value: u8) -> PyResult<()> {
        if value > 8 {
            return Err(PyValueError::new_err("output_verbosity must be 0-8"));
        }
        self.output_verbosity = value;
        Ok(())
    }

    #[getter]
    pub fn output_path(&self) -> Option<String> {
        self.output_path.clone()
    }

    #[setter]
    pub fn set_output_path(&mut self, value: Option<String>) {
        self.output_path = value;
    }

    #[getter]
    pub fn output_to_parquet(&self) -> bool {
        self.output_to_parquet
    }

    #[setter]
    pub fn set_output_to_parquet(&mut self, value: bool) {
        self.output_to_parquet = value;
    }

    #[getter]
    pub fn write_output(&self) -> bool {
        self.write_output
    }

    #[setter]
    pub fn set_write_output(&mut self, value: bool) {
        self.write_output = value;
    }

    #[getter]
    pub fn output_chunk_size(&self) -> usize {
        self.output_chunk_size
    }

    #[setter]
    pub fn set_output_chunk_size(&mut self, value: usize) {
        self.output_chunk_size = value;
    }

    #[getter]
    pub fn setpoint_deadband_c(&self) -> Option<f64> {
        self.setpoint_deadband_c
    }

    #[setter]
    pub fn set_setpoint_deadband_c(&mut self, value: Option<f64>) -> PyResult<()> {
        if let Some(deadband) = value {
            if !deadband.is_finite() || deadband < 0.0 {
                return Err(PyValueError::new_err(
                    "setpoint_deadband_c must be finite and >= 0",
                ));
            }
        }
        self.setpoint_deadband_c = value;
        Ok(())
    }

    #[getter]
    pub fn master_seed(&self) -> u64 {
        self.master_seed
    }

    #[setter]
    pub fn set_master_seed(&mut self, value: u64) {
        self.master_seed = value;
    }

    #[getter]
    pub fn civil_timezone(&self) -> Option<String> {
        self.civil_timezone.clone()
    }

    #[setter]
    pub fn set_civil_timezone(&mut self, value: Option<String>) {
        self.civil_timezone = value;
    }

    pub fn __repr__(&self) -> String {
        format!(
            "SimulationConfig(start_time='{}', duration_s={}, time_res_s={}, write_output={}, output_to_parquet={}, master_seed={})",
            self.start_time.to_rfc3339(),
            self.duration,
            self.time_res,
            self.write_output,
            self.output_to_parquet,
            self.master_seed
        )
    }
}

impl PySimulationConfig {
    pub fn to_sim_config(&self) -> PyResult<SimulationConfig> {
        if self.duration % self.time_res != 0 {
            return Err(PyValueError::new_err(format!(
                "duration_s ({}) must be divisible by time_res_s ({})",
                self.duration, self.time_res
            )));
        }
        Ok(SimulationConfig {
            start_time: self.start_time,
            duration: Duration::seconds(self.duration),
            time_res: Duration::seconds(self.time_res),
            output_verbosity: self.output_verbosity,
            output_path: self.output_path.clone().map(PathBuf::from),
            write_output: self.write_output,
            output_format: if self.output_to_parquet {
                OutputFormat::Parquet
            } else {
                OutputFormat::Csv
            },
            output_chunk_size: self.output_chunk_size,
            setpoint_deadband_c: self.setpoint_deadband_c,
            master_seed: self.master_seed,
            civil_timezone: self.civil_timezone.clone(),
        })
    }

    pub fn from_sim_config(config: &SimulationConfig) -> Self {
        Self {
            start_time: config.start_time,
            duration: config.duration.num_seconds(),
            time_res: config.time_res.num_seconds(),
            output_verbosity: config.output_verbosity,
            output_path: config
                .output_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            write_output: config.write_output,
            output_to_parquet: config.output_format == OutputFormat::Parquet,
            output_chunk_size: config.output_chunk_size,
            setpoint_deadband_c: config.setpoint_deadband_c,
            master_seed: config.master_seed,
            civil_timezone: config.civil_timezone.clone(),
        }
    }
}

#[pyclass(name = "DwellingConfig", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyDwellingConfig {
    hpxml: String,
    schedule: String,
    weather: String,
    config: Option<PySimulationConfig>,
    defaults_path: Option<String>,
    bldg_id: i64,
    initialization_duration: Option<i64>,
    resample_overrides: Option<std::collections::HashMap<String, String>>,
    overrides: Option<std::collections::HashMap<String, serde_json::Value>>,
}

const VALID_RESAMPLE_KEYS: &[&str] = &[
    "dry_bulb",
    "dew_point",
    "rel_humidity",
    "pressure",
    "infrared",
    "ground_temp",
    "opaque_sky_cover",
    "ghi",
    "dni",
    "dhi",
    "wind_speed",
    "wind_dir",
];

#[pymethods]
impl PyDwellingConfig {
    #[new]
    #[pyo3(signature = (hpxml, schedule, weather, config=None, defaults_path=None, bldg_id=None, initialization_duration=None, resample_overrides=None, overrides=None))]
    // PyO3 #[new] with kwargs maps 1:1 to Python kwargs; builder adds no value here.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hpxml: String,
        schedule: String,
        weather: String,
        config: Option<PySimulationConfig>,
        defaults_path: Option<String>,
        bldg_id: Option<i64>,
        initialization_duration: Option<i64>,
        resample_overrides: Option<Bound<'_, PyDict>>,
        overrides: Option<Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let parsed_resample = if let Some(dict) = resample_overrides {
            let mut map = std::collections::HashMap::new();
            for key in dict.keys() {
                let key_str = key.extract::<String>()?;
                if !VALID_RESAMPLE_KEYS.contains(&key_str.as_str()) {
                    return Err(PyValueError::new_err(format!(
                        "invalid resample key '{}'. Valid keys are: {}",
                        key_str,
                        VALID_RESAMPLE_KEYS.join(", ")
                    )));
                }
                let value = dict.get_item(&key)?.ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "missing value for resample override key: {}",
                        key_str
                    ))
                })?;
                let value_str = value.extract::<String>()?;
                let valid_methods = ["pchip", "zoh", "linear"];
                if !valid_methods.contains(&value_str.as_str()) {
                    return Err(PyValueError::new_err(format!(
                        "invalid resample method '{}'. Expected one of: pchip, zoh, linear",
                        value_str
                    )));
                }
                map.insert(key_str, value_str);
            }
            if map.is_empty() { None } else { Some(map) }
        } else {
            None
        };

        let parsed_overrides = if let Some(dict) = overrides {
            let mut map = std::collections::HashMap::new();
            for key in dict.keys() {
                let key_str = key.extract::<String>()?;
                let value = dict.get_item(&key)?.ok_or_else(|| {
                    PyValueError::new_err(format!("missing value for override key: {}", key_str))
                })?;
                let py_any = value.as_ref();
                let json_value = python_to_json(py_any)?;
                map.insert(key_str, json_value);
            }
            if map.is_empty() { None } else { Some(map) }
        } else {
            None
        };

        if let Some(init_dur) = initialization_duration {
            if init_dur < 0 {
                return Err(PyValueError::new_err(
                    "initialization_duration must be non-negative",
                ));
            }
        }

        let initialization_duration =
            initialization_duration.and_then(|d| if d == 0 { None } else { Some(d) });

        Ok(Self {
            hpxml,
            schedule,
            weather,
            config,
            defaults_path,
            bldg_id: bldg_id.unwrap_or(0),
            initialization_duration,
            resample_overrides: parsed_resample,
            overrides: parsed_overrides,
        })
    }

    #[getter]
    pub fn hpxml(&self) -> &str {
        &self.hpxml
    }

    #[getter]
    pub fn schedule(&self) -> &str {
        &self.schedule
    }

    #[getter]
    pub fn weather(&self) -> &str {
        &self.weather
    }

    #[getter]
    pub fn config(&self) -> Option<PySimulationConfig> {
        self.config.clone()
    }

    #[getter]
    pub fn defaults_path(&self) -> Option<String> {
        self.defaults_path.clone()
    }

    #[getter]
    pub fn bldg_id(&self) -> i64 {
        self.bldg_id
    }

    #[getter]
    pub fn initialization_duration(&self) -> Option<i64> {
        self.initialization_duration
    }

    #[getter]
    pub fn resample_overrides(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        if let Some(ref map) = self.resample_overrides {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, v)?;
            }
            Ok(Some(dict.into()))
        } else {
            Ok(None)
        }
    }

    #[getter]
    pub fn overrides(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        if let Some(ref map) = self.overrides {
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
        } else {
            Ok(None)
        }
    }

    pub fn __repr__(&self) -> String {
        format!(
            "DwellingConfig(hpxml='{}', schedule='{}', weather='{}', bldg_id={})",
            self.hpxml, self.schedule, self.weather, self.bldg_id
        )
    }
}

impl PyDwellingConfig {
    pub fn to_dwelling_config(&self) -> PyResult<DwellingConfig> {
        let sim_config = self
            .config
            .as_ref()
            .map(|c| c.to_sim_config())
            .transpose()?
            .unwrap_or_else(|| SimulationConfig {
                start_time: default_start_time(),
                duration: Duration::seconds(DEFAULT_DURATION_S),
                time_res: Duration::seconds(DEFAULT_STEP_S),
                output_verbosity: 0,
                output_path: None,
                write_output: true,
                output_format: OutputFormat::Csv,
                output_chunk_size: DEFAULT_CHUNK_SIZE,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
            });

        let resample_overrides: Option<ResampleOverrides> =
            self.resample_overrides.as_ref().map(|map| {
                let mut overrides = ResampleOverrides::default();
                macro_rules! set_resample {
                    ($($field:ident),*) => {
                        $(
                            if let Some(v) = map.get(stringify!($field)) {
                                if let Ok(m) = parse_resample_method(v) {
                                    overrides.$field = Some(m);
                                }
                            }
                        )*
                    };
                }
                set_resample!(
                    dry_bulb,
                    dew_point,
                    rel_humidity,
                    pressure,
                    infrared,
                    ground_temp,
                    opaque_sky_cover,
                    ghi,
                    dni,
                    dhi,
                    wind_speed,
                    wind_dir
                );
                overrides
            });

        let overrides_value = self.overrides.as_ref().map(|map| {
            let mut obj = serde_json::Map::new();
            for (key, value) in map {
                obj.insert(key.clone(), value.clone());
            }
            serde_json::Value::Object(obj)
        });

        Ok(DwellingConfig {
            hpxml_path: PathBuf::from(&self.hpxml),
            schedule_path: PathBuf::from(&self.schedule),
            weather_path: PathBuf::from(&self.weather),
            defaults_path: self.defaults_path.clone().map(PathBuf::from),
            sim_config,
            overrides: overrides_value,
            bldg_id: self.bldg_id,
            initialization_duration: self
                .initialization_duration
                .map(|d| StdDuration::from_secs(d as u64)),
            resample_overrides,
        })
    }
}

fn parse_resample_method(s: &str) -> PyResult<ResampleMethod> {
    match s {
        "pchip" => Ok(ResampleMethod::Pchip),
        "pchip_cyclic" => Ok(ResampleMethod::PchipCyclic),
        "zoh" => Ok(ResampleMethod::Zoh),
        "linear" => Ok(ResampleMethod::Linear),
        _ => Err(PyValueError::new_err(format!(
            "invalid resample method: '{}'. Expected pchip, pchip_cyclic, zoh, or linear",
            s
        ))),
    }
}
