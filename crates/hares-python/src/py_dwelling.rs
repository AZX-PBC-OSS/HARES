//! Python bindings for dwelling simulation.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, TimeZone};
use hares_control::PriceSignal;
use hares_core::{ActorConfig, ActorRegistry, Dwelling, DwellingConfig};
use hares_equipment::config::ConfigValue;
use hares_io::{OutputFormat, SimulationConfig};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyType};

use crate::conversions::{record_batches_to_polars_df, steps_to_polars_df};
use crate::py_actor::PyActor;
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
    registry: ActorRegistry,
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
            registry: ActorRegistry::new(),
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
        py.detach(|| {
            let mut dwelling = self
                .dwelling
                .lock()
                .map_err(|_| "failed to lock dwelling state".to_string())?;
            dwelling.simulate().map_err(|err| err.to_string())?;
            Ok::<_, String>(())
        })
        .map_err(PyValueError::new_err)?;

        let dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        let batches = dwelling.flushed_batches();
        if batches.is_empty() {
            // Fallback to steps path if no batches were flushed.
            return steps_to_polars_df(py, &dwelling.results().steps);
        }
        record_batches_to_polars_df(py, batches)
    }

    pub fn step(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let step = py
            .detach(|| self.step_core())
            .map_err(PyValueError::new_err)?;

        let out = PyDict::new(py);
        out.set_item("time", chrono_to_py_datetime(py, step.timestamp)?)?;
        out.set_item("net_electric_power_kw", step.net_electric_power_kw)?;
        out.set_item("hvac_heating_w", step.hvac_heating_w)?;
        out.set_item("hvac_cooling_w", step.hvac_cooling_w)?;
        for (zone_id, temp_c) in &step.zone_temperatures_c {
            let key = if zone_id.0 == 0 {
                "Temperature - Indoor (C)".to_string()
            } else {
                format!("Temperature - Zone_{} (C)", zone_id.0)
            };
            out.set_item(key, *temp_c)?;
        }
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

    /// Adds a Python actor to the dwelling's decision-making loop.
    pub fn add_actor(&self, py: Python<'_>, actor: Py<PyActor>) -> PyResult<()> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        let wrapper = crate::py_actor::PyActorWrapper::new(py, actor);
        dwelling.add_actor(Box::new(wrapper));
        Ok(())
    }

    /// Creates and adds an actor from the registry using configuration.
    ///
    /// # Arguments
    ///
    /// * `actor_type` - Registered actor type name (e.g., "IdealThermostat", "Occupant", "DrCompliance")
    /// * `name` - Actor instance name
    /// * `params` - Configuration parameters as a dictionary
    pub fn add_actor_by_name(
        &self,
        actor_type: String,
        name: String,
        params: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let mut config = ActorConfig::new(name, actor_type);

        if let Some(dict) = params {
            for (key, value) in dict.iter() {
                let key: String = key.extract()?;
                let config_value = py_to_config_value(&value)?;
                config = config.with_param(key, config_value);
            }
        }

        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling
            .add_actor_by_name(&self.registry, config)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
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

    #[cfg(feature = "observe")]
    pub fn enable_observer(&self, capacity: usize) -> PyResult<()> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        dwelling.enable_observer(capacity);
        Ok(())
    }

    #[cfg(feature = "observe")]
    pub fn drain_observations(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let mut dwelling = self
            .dwelling
            .lock()
            .map_err(|_| PyValueError::new_err("failed to lock dwelling state"))?;
        let snapshots = dwelling.drain_observations();
        snapshots
            .into_iter()
            .map(|snap| snapshot_to_py(py, &snap))
            .collect()
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
        let batches = dwelling.flushed_batches();
        if batches.is_empty() {
            return steps_to_polars_df(py, &dwelling.results().steps);
        }
        record_batches_to_polars_df(py, batches)
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
    start: DateTime<FixedOffset>,
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

    let initialization_duration = kwargs
        .as_ref()
        .and_then(|k| k.get_item("initialization_duration").ok().flatten())
        .map(|obj| {
            let secs = extract_seconds(&obj)?;
            if secs < 0 {
                return Err(PyValueError::new_err(
                    "initialization_duration must be non-negative",
                ));
            }
            Ok(secs)
        })
        .transpose()?
        .and_then(|secs| {
            if secs == 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(secs as u64))
            }
        });

    let civil_timezone = kwargs
        .as_ref()
        .and_then(|k| k.get_item("civil_timezone").ok().flatten())
        .map(|obj| obj.extract::<String>())
        .transpose()?;

    let defaults_path = kwargs
        .as_ref()
        .and_then(|k| k.get_item("defaults_path").ok().flatten())
        .map(|obj| obj.extract::<String>())
        .transpose()?
        .map(PathBuf::from);

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
        civil_timezone,
    };

    Ok(DwellingConfig {
        hpxml_path: PathBuf::from(hpxml),
        schedule_path: PathBuf::from(schedule),
        weather_path: PathBuf::from(weather),
        defaults_path,
        sim_config,
        overrides: None,
        bldg_id,
        initialization_duration,
        resample_overrides: None,
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

fn extract_datetime(obj: &Bound<'_, PyAny>) -> PyResult<DateTime<FixedOffset>> {
    if let Ok(value) = obj.extract::<String>() {
        return parse_datetime_str(&value);
    }

    let iso: String = obj.call_method0("isoformat")?.extract()?;
    parse_datetime_str(&iso)
}

fn parse_datetime_str(value: &str) -> PyResult<DateTime<FixedOffset>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt);
    }

    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        let utc_offset = FixedOffset::east_opt(0).expect("valid UTC offset");
        return Ok(utc_offset.from_utc_datetime(&naive));
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

fn default_start() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(DEFAULT_START).expect("valid default timestamp")
}

fn chrono_to_py_datetime(py: Python<'_>, dt: DateTime<FixedOffset>) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (dt.to_rfc3339(),))?;
    Ok(obj.unbind())
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyValueError::new_err(err.to_string())
}

#[cfg(feature = "observe")]
fn snapshot_to_py(
    py: Python<'_>,
    snap: &hares_core::observer::StepSnapshot,
) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    dict.set_item("step_index", snap.step_index)?;
    dict.set_item("timestamp", snap.timestamp.to_rfc3339())?;

    let phases = &snap.phases;

    if let Some(env) = &phases.post_environment {
        let d = PyDict::new(py);
        d.set_item("outdoor_temp_c", env.outdoor_temp_c)?;
        d.set_item("ghi_w_m2", env.ghi_w_m2)?;
        d.set_item("wind_speed_m_s", env.wind_speed_m_s)?;
        d.set_item("mains_temp_c", env.mains_temp_c)?;
        d.set_item(
            "zone_temps_c",
            env.zone_temps_c
                .iter()
                .map(|(z, t)| (z.0, *t))
                .collect::<Vec<_>>(),
        )?;
        d.set_item(
            "zone_humidity_ratios",
            env.zone_humidity_ratios
                .iter()
                .map(|(z, h)| (z.0, *h))
                .collect::<Vec<_>>(),
        )?;
        dict.set_item("post_environment", d)?;
    }

    for (key, phase) in [
        (
            "post_nonthermal_equipment",
            &phases.post_nonthermal_equipment,
        ),
        ("post_thermal_equipment", &phases.post_thermal_equipment),
    ] {
        if let Some(eq_phase) = phase {
            let d = PyDict::new(py);
            let eq_list: Vec<Py<PyAny>> = eq_phase
                .equipment
                .iter()
                .map(|obs| {
                    let ed = PyDict::new(py);
                    ed.set_item("name", &obs.name)?;
                    ed.set_item("equipment_type", &obs.equipment_type)?;
                    ed.set_item("end_use", format!("{:?}", obs.end_use))?;
                    let telem: Vec<(&String, &f64)> = obs.telemetry.0.iter().collect();
                    let td = PyDict::new(py);
                    for (k, v) in telem {
                        td.set_item(k, v)?;
                    }
                    ed.set_item("telemetry", td)?;
                    Ok(ed.unbind().into())
                })
                .collect::<PyResult<Vec<_>>>()?;
            d.set_item("equipment", eq_list)?;

            let ports = &eq_phase.ports;
            let pd = PyDict::new(py);
            pd.set_item(
                "thermal",
                ports
                    .thermal
                    .iter()
                    .map(|(z, s, l)| (z.0, *s, *l))
                    .collect::<Vec<_>>(),
            )?;
            pd.set_item("electrical_load_kw", ports.electrical_load_kw)?;
            pd.set_item("electrical_gen_kw", ports.electrical_gen_kw)?;
            pd.set_item("electrical_reactive_kvar", ports.electrical_reactive_kvar)?;
            pd.set_item(
                "fuel_consumption_w",
                ports
                    .fuel_consumption_w
                    .iter()
                    .map(|(ft, v)| (format!("{ft:?}"), *v))
                    .collect::<Vec<_>>(),
            )?;
            d.set_item("ports", pd)?;
            dict.set_item(key, d)?;
        }
    }

    if let Some(solvers) = &phases.post_solvers {
        let d = PyDict::new(py);
        let gains = &solvers.envelope_gains;
        d.set_item("window_solar_w", gains.window_solar_w)?;
        d.set_item("opaque_solar_lwr_w", gains.opaque_solar_lwr_w)?;
        d.set_item("interior_lwr_w", gains.interior_lwr_w)?;
        d.set_item("infiltration_w", gains.infiltration_w)?;
        d.set_item("ventilation_w", gains.ventilation_w)?;
        d.set_item("natural_ventilation_w", gains.natural_ventilation_w)?;
        d.set_item("port_sensible_w", gains.port_sensible_w)?;
        d.set_item("internal_gain_w", gains.internal_gain_w)?;
        dict.set_item("post_solvers", d)?;
    }

    if let Some(zu) = &phases.post_zone_update {
        let d = PyDict::new(py);
        d.set_item(
            "zone_temps_c",
            zu.zone_temps_c
                .iter()
                .map(|(z, t)| (z.0, *t))
                .collect::<Vec<_>>(),
        )?;
        d.set_item(
            "zone_humidity_ratios",
            zu.zone_humidity_ratios
                .iter()
                .map(|(z, h)| (z.0, *h))
                .collect::<Vec<_>>(),
        )?;
        dict.set_item("post_zone_update", d)?;
    }

    Ok(dict.unbind().into())
}

fn py_to_config_value(value: &Bound<'_, PyAny>) -> PyResult<ConfigValue> {
    // Bool must be checked before f64 because Python bool extracts as f64 (True → 1.0)
    if let Ok(b) = value.extract::<bool>() {
        return Ok(ConfigValue::Bool(b));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(ConfigValue::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(ConfigValue::Text(s));
    }
    if let Ok(arr) = value.extract::<Vec<f64>>() {
        return Ok(ConfigValue::FloatArray(arr));
    }
    Err(PyValueError::new_err(
        "config value must be bool, float, string, or list of floats",
    ))
}
