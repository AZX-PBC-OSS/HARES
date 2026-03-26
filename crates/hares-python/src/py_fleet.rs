//! Python bindings for fleet simulation.

use std::collections::HashMap;
use std::path::Path;

use hares_fleet::aggregation::{AggregationResolution, aggregate};
use hares_fleet::{Fleet, FleetResults, SimError};
use hares_io::ResStockVersion as IoResStockVersion;
use pyo3::Python;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyType};
use tracing::warn;

use crate::conversions::{
    dwelling_metrics_to_polars_df, fleet_results_to_py, record_batches_to_polars_df,
};
use crate::py_config::PyDwellingConfig;
use crate::py_enums::{PyAggregationResolution, PyResStockVersion};

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
            // TODO(M-2): Fleet::set_progress requires &mut self but PyFleet::simulate
            // takes &self (PyO3 constraint). Until Fleet exposes a simulate_with_progress
            // that accepts an external callback without mutation, a clone is unavoidable.
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
                        SimError::ThreadPoolBuild(_) => 0,
                    };
                    failures.push((bldg_id, format_sim_error(&err)));
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

fn format_sim_error(err: &SimError) -> String {
    err.to_string()
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyValueError::new_err(err.to_string())
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
