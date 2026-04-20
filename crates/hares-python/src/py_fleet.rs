//! Python bindings for fleet simulation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use hares_fleet::aggregation::{AggregationResolution, aggregate};
use hares_fleet::{DwellingBuildError, Fleet, FleetResults, SimError, SteppableFleet};
use hares_io::ResStockVersion as IoResStockVersion;
use pyo3::Python;
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyType};
use tracing::warn;

use crate::conversions::{
    chrono_to_py_datetime, dwelling_metrics_to_polars_df, fleet_results_to_py,
    record_batches_to_polars_df,
};
use crate::py_config::PyDwellingConfig;
use crate::py_control::PyControlSignal;
use crate::py_enums::{PyAggregationResolution, PyResStockVersion};
use crate::py_telemetry::PyTelemetry;

fn parse_resstock_version(s: &str) -> PyResult<IoResStockVersion> {
    let normalized: String = s
        .to_lowercase()
        .chars()
        .filter(|c| *c != '.' && *c != '_')
        .collect();
    match normalized.as_str() {
        "20241" | "v20241" => Ok(IoResStockVersion::V2024_1),
        "20242" | "v20242" => Ok(IoResStockVersion::V2024_2),
        "20251" | "v20251" => Ok(IoResStockVersion::V2025_1),
        _ => Err(PyValueError::new_err(format!(
            "invalid ResStockVersion variant: {}",
            s
        ))),
    }
}

fn parse_aggregation_resolution(s: &str) -> PyResult<AggregationResolution> {
    match s.to_lowercase().as_str() {
        "15min" | "fifteenmin" | "fifteen_min" => Ok(AggregationResolution::FifteenMin),
        "hourly" => Ok(AggregationResolution::Hourly),
        _ => Err(PyValueError::new_err(format!(
            "invalid AggregationResolution variant: {}",
            s
        ))),
    }
}

fn extract_resstock_version(obj: Bound<'_, PyAny>) -> PyResult<Option<IoResStockVersion>> {
    if let Ok(v) = obj.extract::<PyResStockVersion>() {
        Ok(Some(IoResStockVersion::from(v)))
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(Some(parse_resstock_version(&s)?))
    } else {
        Err(PyValueError::new_err(
            "resstock_version must be a ResStockVersion enum or string",
        ))
    }
}

fn extract_aggregation_resolution(
    obj: Bound<'_, PyAny>,
) -> PyResult<Option<AggregationResolution>> {
    if let Ok(v) = obj.extract::<PyAggregationResolution>() {
        Ok(Some(AggregationResolution::from(v)))
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(Some(parse_aggregation_resolution(&s)?))
    } else {
        Err(PyValueError::new_err(
            "resolution must be an AggregationResolution enum or string",
        ))
    }
}

#[pyclass(name = "Fleet")]
pub struct PyFleet {
    fleet: Fleet,
}

#[pymethods]
impl PyFleet {
    #[classmethod]
    #[pyo3(signature = (configs, sample_weights=None))]
    pub fn from_buildings(
        _cls: &Bound<'_, PyType>,
        configs: Vec<PyDwellingConfig>,
        sample_weights: Option<Vec<f64>>,
    ) -> PyResult<Self> {
        if configs.is_empty() {
            return Err(PyValueError::new_err(
                "from_buildings requires at least one dwelling config",
            ));
        }

        if let Some(ref weights) = sample_weights {
            if weights.len() != configs.len() {
                return Err(PyValueError::new_err(format!(
                    "sample_weights length ({}) must match configs length ({})",
                    weights.len(),
                    configs.len()
                )));
            }
        }

        let dwelling_configs: Vec<_> = configs
            .iter()
            .map(|c| c.to_dwelling_config())
            .collect::<PyResult<Vec<_>>>()?;

        let fleet = Fleet::from_buildings(dwelling_configs);

        let fleet = if let Some(weights) = sample_weights {
            fleet.with_sample_weights(weights)
        } else {
            fleet
        };

        Ok(Self { fleet })
    }

    #[classmethod]
    #[pyo3(signature = (metadata_path, hpxml_dir, weather_dir, filter=None, resstock_version=None))]
    pub fn from_resstock(
        _cls: &Bound<'_, PyType>,
        metadata_path: String,
        hpxml_dir: String,
        weather_dir: String,
        filter: Option<&Bound<'_, PyDict>>,
        resstock_version: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let parsed_filter = filter
            .map(dict_to_filter)
            .transpose()?
            .filter(|criteria| !criteria.is_empty());

        let parsed_version = match resstock_version {
            Some(obj) => extract_resstock_version(obj)?,
            None => Some(IoResStockVersion::V2025_1),
        };

        let fleet = Fleet::from_resstock(
            Path::new(&metadata_path),
            Path::new(&hpxml_dir),
            Path::new(&weather_dir),
            parsed_version,
            parsed_filter,
        )
        .map_err(to_py_err)?;

        Ok(Self { fleet })
    }

    #[pyo3(signature = (n_threads=None, resolution=None, raise_on_failure=None, progress=None))]
    pub fn simulate(
        &self,
        py: Python<'_>,
        n_threads: Option<isize>,
        resolution: Option<Bound<'_, PyAny>>,
        raise_on_failure: Option<bool>,
        progress: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyFleetResults> {
        let thread_count = normalize_thread_count(n_threads)?;
        let resolution = match resolution {
            Some(obj) => {
                extract_aggregation_resolution(obj)?.unwrap_or(AggregationResolution::FifteenMin)
            }
            None => AggregationResolution::FifteenMin,
        };
        let raise_on_failure = raise_on_failure.unwrap_or(false);

        let outcomes = if let Some(callback) = progress {
            // PyO3 `#[pymethods]` require `&self`, but `Fleet::set_progress` mutates
            // the fleet to install the callback. Cloning is the only way to attach
            // the progress callback without forcing `&mut self` on the Python API.
            let mut fleet_clone = self.fleet.clone();
            let callback: Py<PyAny> = callback.clone().unbind();
            fleet_clone.set_progress(move |done: usize, total: usize| {
                Python::attach(|py| {
                    if let Err(err) = callback.call1(py, (done, total)) {
                        warn!("progress callback error: {}", err);
                    }
                });
            });
            py.detach(|| fleet_clone.simulate(thread_count))
        } else {
            py.detach(|| self.fleet.simulate(thread_count))
        };

        let outcomes = outcomes.into_iter().collect::<Vec<_>>();

        let mut success = Vec::with_capacity(outcomes.len());
        let mut failures = Vec::new();

        for outcome in outcomes {
            match outcome {
                Ok(ok) => success.push(ok),
                Err(err) => {
                    let bldg_id = match &err {
                        SimError::Failed { bldg_id, .. } => *bldg_id,
                        SimError::Engine { bldg_id, .. } => *bldg_id,
                        SimError::Panic { bldg_id, .. } => *bldg_id,
                        SimError::ThreadPoolBuild(_) => -1,
                    };
                    failures.push((bldg_id, err.to_string()));
                }
            }
        }

        if !failures.is_empty() && raise_on_failure {
            let failure_msgs: Vec<_> = failures.iter().map(|(_, m)| m.clone()).collect();
            return Err(PyValueError::new_err(format!(
                "fleet simulation failed for {} dwelling(s): {}",
                failures.len(),
                failure_msgs.join("; ")
            )));
        }

        if success.is_empty() && !failures.is_empty() {
            return Err(PyValueError::new_err(
                "all dwellings in the fleet failed to simulate",
            ));
        }

        let aggregated = aggregate(&success, resolution);
        let mut results = fleet_results_to_py(aggregated);
        results.failures = failures;
        results.n_succeeded = success.len();
        Ok(results)
    }

    fn __len__(&self) -> usize {
        self.fleet.len()
    }

    fn __repr__(&self) -> String {
        format!("Fleet(n_dwellings={})", self.fleet.len())
    }
}

impl From<PyAggregationResolution> for AggregationResolution {
    fn from(py_res: PyAggregationResolution) -> Self {
        match py_res {
            PyAggregationResolution::FifteenMin => AggregationResolution::FifteenMin,
            PyAggregationResolution::Hourly => AggregationResolution::Hourly,
        }
    }
}

#[pyclass(name = "FleetResults")]
pub struct PyFleetResults {
    pub(crate) results: FleetResults,
    pub(crate) failures: Vec<(i64, String)>,
    pub(crate) n_succeeded: usize,
}

impl PyFleetResults {
    pub(crate) fn new(results: FleetResults) -> Self {
        let n_succeeded = results.per_dwelling_metrics.len();
        Self {
            results,
            failures: Vec::new(),
            n_succeeded,
        }
    }
}

#[pymethods]
impl PyFleetResults {
    #[getter]
    pub fn per_dwelling_metrics(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        dwelling_metrics_to_polars_df(py, &self.results.per_dwelling_metrics)
    }

    #[getter]
    pub fn aggregate_timeseries(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        record_batches_to_polars_df(py, std::slice::from_ref(&self.results.aggregate_timeseries))
    }

    #[getter]
    pub fn failures(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for (bldg_id, error) in &self.failures {
            let dict = PyDict::new(py);
            dict.set_item("bldg_id", *bldg_id)?;
            dict.set_item("error", error)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    #[getter]
    pub fn n_succeeded(&self) -> usize {
        self.n_succeeded
    }

    #[getter]
    pub fn n_failed(&self) -> usize {
        self.failures.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "PyFleetResults(per_dwelling_rows={}, aggregate_rows={}, failures={})",
            self.results.per_dwelling_metrics.len(),
            self.results.aggregate_timeseries.num_rows(),
            self.failures.len()
        )
    }
}

fn dict_to_filter(d: &Bound<'_, PyDict>) -> PyResult<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(d.len());
    for (key, value) in d.iter() {
        let key_str: String = key
            .extract()
            .map_err(|_| PyValueError::new_err("filter keys must be strings"))?;
        let value_str: String = value
            .extract()
            .map_err(|_| PyValueError::new_err("filter values must be strings"))?;
        out.insert(key_str, value_str);
    }
    Ok(out)
}

fn normalize_thread_count(n_threads: Option<isize>) -> PyResult<usize> {
    match n_threads {
        None => Ok(0),
        Some(0) => Ok(0),
        Some(value) if value > 0 => usize::try_from(value)
            .map_err(|_| PyValueError::new_err("n_threads is out of range for this platform")),
        Some(_) => Err(PyValueError::new_err("n_threads must be >= 0")),
    }
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn lock_steppable_fleet(fleet: &Mutex<SteppableFleet>) -> PyResult<MutexGuard<'_, SteppableFleet>> {
    fleet.lock().map_err(|e| {
        PyRuntimeError::new_err(format!(
            "steppable fleet state is corrupted (internal panic: {}); create a new SteppableFleet instance",
            e
        ))
    })
}

#[pyclass(name = "SteppableFleet")]
pub struct PySteppableFleet {
    fleet: Mutex<SteppableFleet>,
    build_errors: Vec<DwellingBuildError>,
}

#[pymethods]
impl PySteppableFleet {
    #[classmethod]
    #[pyo3(signature = (configs, n_threads=0))]
    pub fn from_configs(
        _cls: &Bound<'_, PyType>,
        configs: Vec<PyDwellingConfig>,
        n_threads: isize,
    ) -> PyResult<Self> {
        let thread_count = normalize_thread_count(Some(n_threads))?;
        let dwelling_configs: Vec<_> = configs
            .iter()
            .map(|c| c.to_dwelling_config())
            .collect::<PyResult<Vec<_>>>()?;

        let (fleet, build_errors) =
            SteppableFleet::from_configs(dwelling_configs, thread_count).map_err(to_py_err)?;
        if !build_errors.is_empty() {
            warn!(
                "SteppableFleet initialized with {} build failure(s)",
                build_errors.len()
            );
            for err in &build_errors {
                warn!(
                    bldg_id = err.bldg_id,
                    error = err.message,
                    "dwelling failed during SteppableFleet init"
                );
            }
        }
        Ok(Self {
            fleet: Mutex::new(fleet),
            build_errors,
        })
    }

    pub fn step(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let (step_results, bldg_ids, reactive_power_kvar) = py
            .detach(|| {
                let mut fleet = self.fleet.lock().map_err(|e| {
                    format!(
                        "steppable fleet state is corrupted (internal panic: {e}); \
                         create a new SteppableFleet instance"
                    )
                })?;
                let step_results = fleet.step();
                let bldg_ids = (0..fleet.len())
                    .map(|idx| fleet.bldg_id(idx).unwrap_or(-1))
                    .collect::<Vec<_>>();
                let reactive_power_kvar = (0..fleet.len())
                    .map(|idx| fleet.telemetry(idx).map(|t| t.reactive_power_kvar))
                    .collect::<Vec<_>>();
                Ok::<_, String>((step_results, bldg_ids, reactive_power_kvar))
            })
            .map_err(PyRuntimeError::new_err)?;

        let out = PyList::empty(py);
        for (idx, entry) in step_results.into_iter().enumerate() {
            let item = PyDict::new(py);
            match entry {
                Ok(step) => {
                    let result = PyDict::new(py);
                    result.set_item("timestamp", chrono_to_py_datetime(py, step.timestamp)?)?;
                    result.set_item("net_electric_power_kw", step.net_electric_power_kw)?;
                    result.set_item("hvac_heating_w", step.hvac_heating_w)?;
                    result.set_item("hvac_cooling_w", step.hvac_cooling_w)?;
                    result.set_item("gas_power_w", step.gas_power_w)?;
                    let reactive = reactive_power_kvar.get(idx).and_then(|v| *v).unwrap_or(0.0);
                    result.set_item("reactive_power_kvar", reactive)?;
                    for (zone_id, temp_c) in &step.zone_temperatures_c {
                        let key = if zone_id.0 == 0 {
                            "Temperature - Indoor (C)".to_string()
                        } else {
                            format!("Temperature - Zone_{} (C)", zone_id.0)
                        };
                        result.set_item(key, *temp_c)?;
                    }
                    item.set_item("ok", true)?;
                    item.set_item("bldg_id", bldg_ids[idx])?;
                    item.set_item("result", result)?;
                }
                Err(err) => {
                    item.set_item("ok", false)?;
                    item.set_item("error", err.to_string())?;
                    let bldg_id = match err {
                        SimError::Failed { bldg_id, .. } => bldg_id,
                        SimError::Engine { bldg_id, .. } => bldg_id,
                        SimError::Panic { bldg_id, .. } => bldg_id,
                        SimError::ThreadPoolBuild(_) => -1,
                    };
                    item.set_item("bldg_id", bldg_id)?;
                }
            }
            out.append(item)?;
        }

        Ok(out.unbind())
    }

    #[getter]
    pub fn build_errors(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        for err in &self.build_errors {
            let item = PyDict::new(py);
            item.set_item("bldg_id", err.bldg_id)?;
            item.set_item("error", &err.message)?;
            list.append(item)?;
        }
        Ok(list.unbind())
    }

    pub fn set_grid_voltage(&self, dwelling_index: usize, voltage_pu: f64) -> PyResult<()> {
        let mut fleet = lock_steppable_fleet(&self.fleet)?;
        if dwelling_index >= fleet.len() {
            return Err(PyIndexError::new_err(format!(
                "dwelling_index {dwelling_index} out of range (len={})",
                fleet.len()
            )));
        }
        fleet.set_grid_voltage(dwelling_index, voltage_pu);
        Ok(())
    }

    pub fn set_grid_voltage_all(&self, voltage_pu: f64) -> PyResult<()> {
        let mut fleet = lock_steppable_fleet(&self.fleet)?;
        fleet.set_grid_voltage_all(voltage_pu);
        Ok(())
    }

    pub fn apply_control(
        &self,
        dwelling_index: usize,
        name: String,
        signal: &PyControlSignal,
    ) -> PyResult<()> {
        let mut fleet = lock_steppable_fleet(&self.fleet)?;
        if dwelling_index >= fleet.len() {
            return Err(PyIndexError::new_err(format!(
                "dwelling_index {dwelling_index} out of range (len={})",
                fleet.len()
            )));
        }
        fleet.apply_control(dwelling_index, &name, signal.signal.clone());
        Ok(())
    }

    pub fn telemetry(&self, dwelling_index: usize) -> PyResult<PyTelemetry> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        fleet
            .telemetry(dwelling_index)
            .map(PyTelemetry::new)
            .ok_or_else(|| {
                PyIndexError::new_err(format!(
                    "dwelling_index {dwelling_index} out of range (len={})",
                    fleet.len()
                ))
            })
    }

    pub fn is_finished(&self) -> PyResult<bool> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(fleet.is_finished())
    }

    pub fn time_res_s(&self) -> PyResult<f64> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(fleet.time_res_s())
    }

    pub fn total_steps(&self) -> PyResult<u64> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(fleet.total_steps())
    }

    pub fn current_step(&self) -> PyResult<u64> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(fleet.current_step())
    }

    fn __len__(&self) -> PyResult<usize> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(fleet.len())
    }

    fn __repr__(&self) -> PyResult<String> {
        let fleet = lock_steppable_fleet(&self.fleet)?;
        Ok(format!(
            "SteppableFleet(n_dwellings={}, current_step={}, total_steps={}, build_errors={})",
            fleet.len(),
            fleet.current_step(),
            fleet.total_steps(),
            self.build_errors.len(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_normalization_matches_contract() {
        assert_eq!(normalize_thread_count(None).expect("none maps to zero"), 0);
        assert_eq!(
            normalize_thread_count(Some(0)).expect("zero passes through"),
            0
        );
        assert_eq!(normalize_thread_count(Some(2)).expect("positive maps"), 2);
        assert!(normalize_thread_count(Some(-1)).is_err());
    }
}
