//! Python bindings for fleet simulation.

use std::collections::HashMap;
use std::path::Path;

use hares_fleet::aggregation::{AggregationResolution, aggregate};
use hares_fleet::{Fleet, FleetResults, SimError};
use hares_io::ResStockVersion;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyType};

use crate::conversions::{
    dwelling_metrics_to_polars_df, fleet_results_to_py, record_batches_to_polars_df,
};

#[pyclass(name = "PyFleet")]
pub struct PyFleet {
    fleet: Fleet,
}

#[pymethods]
impl PyFleet {
    #[classmethod]
    #[pyo3(signature = (metadata_path, hpxml_dir, weather_dir, filter=None, resstock_version=None))]
    pub fn from_resstock(
        _cls: &Bound<'_, PyType>,
        metadata_path: String,
        hpxml_dir: String,
        weather_dir: String,
        filter: Option<&Bound<'_, PyDict>>,
        resstock_version: Option<String>,
    ) -> PyResult<Self> {
        let parsed_filter = filter
            .map(dict_to_filter)
            .transpose()?
            .filter(|criteria| !criteria.is_empty());

        let parsed_version = resstock_version
            .as_deref()
            .map(parse_resstock_version)
            .transpose()?;

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

    #[pyo3(signature = (n_threads=None))]
    pub fn simulate(&self, py: Python<'_>, n_threads: Option<isize>) -> PyResult<PyFleetResults> {
        let thread_count = normalize_thread_count(n_threads)?;
        let outcomes = py
            .detach(|| self.fleet.simulate(thread_count))
            .into_iter()
            .collect::<Vec<_>>();

        let mut success = Vec::with_capacity(outcomes.len());
        let mut failures = Vec::new();

        for outcome in outcomes {
            match outcome {
                Ok(ok) => success.push(ok),
                Err(err) => failures.push(format_sim_error(&err)),
            }
        }

        if !failures.is_empty() {
            return Err(PyValueError::new_err(format!(
                "fleet simulation failed for {} dwelling(s): {}",
                failures.len(),
                failures.join("; ")
            )));
        }

        let aggregated = aggregate(&success, AggregationResolution::FifteenMin);
        Ok(fleet_results_to_py(aggregated))
    }
}

#[pyclass(name = "PyFleetResults")]
pub struct PyFleetResults {
    pub(crate) results: FleetResults,
}

impl PyFleetResults {
    pub(crate) fn new(results: FleetResults) -> Self {
        Self { results }
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

    fn __repr__(&self) -> String {
        format!(
            "PyFleetResults(per_dwelling_rows={}, aggregate_rows={})",
            self.results.per_dwelling_metrics.len(),
            self.results.aggregate_timeseries.num_rows()
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

fn parse_resstock_version(version: &str) -> PyResult<ResStockVersion> {
    match version {
        "2024.1" => Ok(ResStockVersion::V2024_1),
        "2024.2" => Ok(ResStockVersion::V2024_2),
        "2025.1" => Ok(ResStockVersion::V2025_1),
        _ => Err(PyValueError::new_err(format!(
            "invalid resstock_version `{version}`; expected one of: 2024.1, 2024.2, 2025.1"
        ))),
    }
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
    fn version_parser_accepts_expected_values() {
        assert_eq!(
            parse_resstock_version("2024.1").expect("valid version"),
            ResStockVersion::V2024_1
        );
        assert_eq!(
            parse_resstock_version("2024.2").expect("valid version"),
            ResStockVersion::V2024_2
        );
        assert_eq!(
            parse_resstock_version("2025.1").expect("valid version"),
            ResStockVersion::V2025_1
        );
    }

    #[test]
    fn version_parser_rejects_invalid_values() {
        let err = parse_resstock_version("2.2.1").expect_err("invalid version");
        assert!(err.to_string().contains("invalid resstock_version"));
    }

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
