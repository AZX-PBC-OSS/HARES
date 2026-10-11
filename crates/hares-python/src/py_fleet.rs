//! Python bindings for fleet simulation.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use hares_fleet::{Coverage, Fleet, FleetReport, FleetResult};
use hares_io::ResStockVersion as IoResStockVersion;
use pyo3::Python;
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyType};
use tracing::warn;

use crate::conversions::{naive_chrono_to_py_datetime, record_batches_to_polars_df};
use crate::py_config::{MAX_CHRONO_SECONDS, PyDwellingConfig, PyHealthPolicy};
use crate::py_control::PyControlSignal;
use crate::py_dwelling::HaresSimulationError;
use crate::py_enums::PyResStockVersion;
use crate::py_report::{PyFrequency, PySelection};
use crate::py_telemetry::PyTelemetry;
use crate::utils::check_unexpected_kwargs;

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

/// The fleet's report request: the variable selection, the reporting
/// frequency the fold reduces onto, and the fleet's health policy, which
/// is applied to every home's run.
///
/// `FleetReport(selection, frequency, health_policy=HealthPolicy())`.
#[pyclass(name = "FleetReport", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyFleetReport(pub FleetReport);

#[pymethods]
impl PyFleetReport {
    #[new]
    #[pyo3(signature = (selection, frequency, health_policy=None))]
    pub fn new(
        selection: PySelection,
        frequency: PyFrequency,
        health_policy: Option<PyHealthPolicy>,
    ) -> Self {
        Self(FleetReport {
            selection: selection.0,
            frequency: frequency.0,
            health_policy: health_policy.map(|policy| policy.0).unwrap_or_default(),
        })
    }

    #[getter]
    pub fn selection(&self) -> PySelection {
        PySelection(self.0.selection.clone())
    }

    #[getter]
    pub fn frequency(&self) -> PyFrequency {
        PyFrequency(self.0.frequency)
    }

    #[getter]
    pub fn health_policy(&self) -> PyHealthPolicy {
        PyHealthPolicy(self.0.health_policy.clone())
    }

    pub fn __repr__(&self) -> String {
        format!(
            "FleetReport(selection={:?}, frequency={:?}, health_policy=...)",
            self.0.selection, self.0.frequency,
        )
    }
}

/// The fleet's coverage weights, by home status.
#[pyclass(name = "Coverage", from_py_object)]
#[derive(Debug, Clone, Copy)]
pub struct PyCoverage(pub Coverage);

#[pymethods]
impl PyCoverage {
    #[getter]
    pub fn weight_total(&self) -> f64 {
        self.0.weight_total
    }

    #[getter]
    pub fn weight_ok(&self) -> f64 {
        self.0.weight_ok
    }

    #[getter]
    pub fn weight_flagged(&self) -> f64 {
        self.0.weight_flagged
    }

    #[getter]
    pub fn weight_failed(&self) -> f64 {
        self.0.weight_failed
    }

    pub fn __repr__(&self) -> String {
        format!(
            "Coverage(weight_total={}, weight_ok={}, weight_flagged={}, weight_failed={})",
            self.0.weight_total, self.0.weight_ok, self.0.weight_flagged, self.0.weight_failed,
        )
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

        let dwelling_configs: Vec<_> = configs
            .iter()
            .map(|c| c.to_dwelling_config())
            .collect::<PyResult<Vec<_>>>()?;

        let fleet = Fleet::from_buildings(dwelling_configs).map_err(to_py_err)?;

        // `with_sample_weights` validates both the length and each weight
        // (rejecting NaN/infinite/negative), so the Python layer defers to it
        // rather than duplicating the checks.
        let fleet = if let Some(weights) = sample_weights {
            fleet.with_sample_weights(weights).map_err(to_py_err)?
        } else {
            fleet
        };

        Ok(Self { fleet })
    }

    /// Build a fleet from ResStock metadata and the unpacked buildings
    /// directory. ``buildings_dir`` must hold one directory per building,
    /// ``bldg<id:07>``, containing that building's ``home.xml`` and
    /// ``in.schedules.csv``: the layout ``hares.data.resstock`` unpacks the
    /// dataset zips to. The fleet never unpacks a zip; a building whose
    /// directory or files are missing is an error naming the building and
    /// the fix. Each building's weather file is resolved from the FIPS code
    /// in its own HPXML (``WeatherStation/Name``) against ``weather_dir``.
    /// Unresolved weather only warns and leaves the dwelling an empty
    /// weather path for now; a later row tightens this to an error.
    #[classmethod]
    #[pyo3(signature = (metadata_path, buildings_dir, weather_dir, filter=None, resstock_version=None, duration_s=None, defaults_path=None, **unexpected))]
    #[allow(clippy::too_many_arguments)]
    pub fn from_resstock(
        _cls: &Bound<'_, PyType>,
        metadata_path: String,
        buildings_dir: String,
        weather_dir: String,
        filter: Option<&Bound<'_, PyDict>>,
        resstock_version: Option<Bound<'_, PyAny>>,
        duration_s: Option<i64>,
        defaults_path: Option<String>,
        unexpected: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        check_unexpected_kwargs("from_resstock", unexpected)?;
        let parsed_filter = filter
            .map(dict_to_filter)
            .transpose()?
            .filter(|criteria| !criteria.is_empty());

        let parsed_version = match resstock_version {
            Some(obj) => extract_resstock_version(obj)?,
            None => Some(IoResStockVersion::V2025_1),
        };

        let duration = match duration_s {
            Some(s) => {
                if s <= 0 {
                    return Err(PyValueError::new_err("duration_s must be positive"));
                }
                if s > MAX_CHRONO_SECONDS {
                    return Err(PyValueError::new_err(format!(
                        "duration_s too large for chrono Duration: {s}"
                    )));
                }
                Some(chrono::Duration::seconds(s))
            }
            None => None,
        };

        let fleet = Fleet::from_resstock(
            Path::new(&metadata_path),
            Path::new(&buildings_dir),
            Path::new(&weather_dir),
            parsed_version,
            parsed_filter,
            duration,
            defaults_path.as_deref().map(Path::new),
        )
        .map_err(to_py_err)?;

        Ok(Self { fleet })
    }

    /// Simulates the fleet through the streaming ordered fold: the homes
    /// run in consecutive chunks of the fleet's entry order, and each
    /// finished chunk's homes fold into the running aggregate one at a
    /// time in entry order before the next chunk starts. The global fold
    /// order is the entry order for every chunk size and thread count.
    #[pyo3(signature = (report, *, n_threads=None, chunk_size=64, raise_on_failure=None, progress=None))]
    pub fn simulate(
        &self,
        py: Python<'_>,
        report: &PyFleetReport,
        n_threads: Option<isize>,
        chunk_size: usize,
        raise_on_failure: Option<bool>,
        progress: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyFleetResult> {
        let thread_count = normalize_thread_count(n_threads)?;
        let chunk = NonZeroUsize::new(chunk_size)
            .ok_or_else(|| PyValueError::new_err("chunk_size must be at least 1"))?;
        let raise_on_failure = raise_on_failure.unwrap_or(false);

        let result = if let Some(callback) = progress {
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
            py.detach(|| fleet_clone.simulate(&report.0, thread_count, chunk))
        } else {
            py.detach(|| self.fleet.simulate(&report.0, thread_count, chunk))
        };

        let result = result.map_err(to_py_err)?;

        if raise_on_failure {
            let failed: Vec<String> = result
                .homes
                .iter()
                .filter_map(|home| match &home.status {
                    hares_fleet::SimStatus::Failed(reason) => {
                        Some(format!("home {}: {reason}", home.bldg_id))
                    }
                    _ => None,
                })
                .collect();
            if !failed.is_empty() {
                return Err(HaresSimulationError::new_err(format!(
                    "fleet simulation failed for {} home(s): {}",
                    failed.len(),
                    failed.join("; ")
                )));
            }
        }

        Ok(PyFleetResult { result })
    }

    fn __len__(&self) -> usize {
        self.fleet.len()
    }

    fn __repr__(&self) -> String {
        format!("Fleet(n_dwellings={})", self.fleet.len())
    }
}

/// The fleet's result: the aggregate, the covered weight behind it, the
/// homes table and the coverage weights.
#[pyclass(name = "FleetResult")]
pub struct PyFleetResult {
    pub(crate) result: FleetResult,
}

#[pymethods]
impl PyFleetResult {
    #[getter]
    pub fn aggregate(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        record_batches_to_polars_df(py, std::slice::from_ref(&self.result.aggregate))
    }

    #[getter]
    pub fn covered_weight(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        record_batches_to_polars_df(py, std::slice::from_ref(&self.result.covered_weight))
    }

    #[getter]
    pub fn homes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let homes = self.result.homes_table();
        record_batches_to_polars_df(py, std::slice::from_ref(&homes))
    }

    #[getter]
    pub fn coverage(&self) -> PyCoverage {
        PyCoverage(self.result.coverage)
    }

    #[getter]
    pub fn n_succeeded(&self) -> usize {
        self.result.n_succeeded
    }

    #[getter]
    pub fn n_failed(&self) -> usize {
        self.result
            .homes
            .iter()
            .filter(|home| matches!(home.status, hares_fleet::SimStatus::Failed(_)))
            .count()
    }

    #[getter]
    pub fn peak_live_home_results(&self) -> usize {
        self.result.peak_live_home_results
    }

    fn __repr__(&self) -> String {
        format!(
            "FleetResult(aggregate_rows={}, homes={}, n_succeeded={}, peak_live_home_results={})",
            self.result.aggregate.num_rows(),
            self.result.homes.len(),
            self.result.n_succeeded,
            self.result.peak_live_home_results,
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

#[pyclass(name = "SteppableFleet")]
pub struct PySteppableFleet {
    fleet: Mutex<hares_fleet::SteppableFleet>,
    poisoned: AtomicBool,
    build_errors: Vec<hares_fleet::DwellingBuildError>,
}

impl PySteppableFleet {
    fn acquire(&self) -> PyResult<std::sync::MutexGuard<'_, hares_fleet::SteppableFleet>> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err(
                "steppable fleet is in a fatal error state (internal state corruption); \
                 create a new SteppableFleet instance",
            ));
        }
        self.fleet.lock().map_err(|e| {
            self.poisoned.store(true, Ordering::SeqCst);
            PyRuntimeError::new_err(format!(
                "steppable fleet state is corrupted (internal panic: {}); \
                 create a new SteppableFleet instance",
                e
            ))
        })
    }
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
            hares_fleet::SteppableFleet::from_configs(dwelling_configs, thread_count)
                .map_err(to_py_err)?;
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
            poisoned: AtomicBool::new(false),
            build_errors,
        })
    }

    pub fn step(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err(
                "steppable fleet is in a fatal error state (internal state corruption); \
                 create a new SteppableFleet instance",
            ));
        }
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
            .map_err(|e| {
                // If the closure returned an error string because the fleet mutex is
                // poisoned, mark the fleet as dead so subsequent calls fail fast.
                self.poisoned.store(true, Ordering::SeqCst);
                PyRuntimeError::new_err(e)
            })?;

        let out = PyList::empty(py);
        for (idx, entry) in step_results.into_iter().enumerate() {
            let item = PyDict::new(py);
            match entry {
                Ok(step) => {
                    let result = PyDict::new(py);
                    result.set_item(
                        "timestamp",
                        naive_chrono_to_py_datetime(py, step.timestamp)?,
                    )?;
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
                        hares_fleet::SimError::Failed { bldg_id, .. } => bldg_id,
                        hares_fleet::SimError::Engine { bldg_id, .. } => bldg_id,
                        hares_fleet::SimError::Panic { bldg_id, .. } => bldg_id,
                        hares_fleet::SimError::Skipped { bldg_id, .. } => bldg_id,
                        hares_fleet::SimError::ThreadPoolBuild(_) => -1,
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
        let mut fleet = self.acquire()?;
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
        let mut fleet = self.acquire()?;
        fleet.set_grid_voltage_all(voltage_pu);
        Ok(())
    }

    pub fn apply_control(
        &self,
        dwelling_index: usize,
        name: String,
        signal: &PyControlSignal,
    ) -> PyResult<()> {
        let mut fleet = self.acquire()?;
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
        let fleet = self.acquire()?;
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
        let fleet = self.acquire()?;
        Ok(fleet.is_finished())
    }

    pub fn time_res_s(&self) -> PyResult<f64> {
        let fleet = self.acquire()?;
        Ok(fleet.time_res_s())
    }

    pub fn total_steps(&self) -> PyResult<u64> {
        let fleet = self.acquire()?;
        Ok(fleet.total_steps())
    }

    pub fn current_step(&self) -> PyResult<u64> {
        let fleet = self.acquire()?;
        Ok(fleet.current_step())
    }

    fn __len__(&self) -> PyResult<usize> {
        let fleet = self.acquire()?;
        Ok(fleet.len())
    }

    fn __repr__(&self) -> PyResult<String> {
        let fleet = self.acquire()?;
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
