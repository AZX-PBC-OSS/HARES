//! Python bindings for the report requests: `ReportSpec`, `Selection`,
//! `Frequency` and `Destination`.
//!
//! Each wraps the Rust value the configuration records, so a report built
//! here resolves exactly like one parsed from TOML
//! (`[[simulation.reports]]`). The variants this entry adds are the ones
//! that can run; every later variant's constructor lands with its entry.

use hares_io::output::report::{Destination, Frequency, ReportSpec, Selection};
use pyo3::prelude::*;
/// What a report selects.
#[pyclass(name = "Selection", from_py_object)]
#[derive(Debug, Clone)]
pub struct PySelection(pub Selection);

#[pymethods]
impl PySelection {
    /// The verbosity preset: every variable the tier emits.
    #[staticmethod]
    pub fn verbosity(verbosity: u8) -> Self {
        Self(Selection::Verbosity(verbosity))
    }

    /// Explicit variable names, resolved exactly and case-sensitively.
    #[staticmethod]
    pub fn named(names: Vec<String>) -> Self {
        Self(Selection::Named(names))
    }

    /// A glob over variable names (`*`, `?`; case-sensitive).
    #[staticmethod]
    pub fn pattern(pattern: String) -> Self {
        Self(Selection::Pattern(pattern))
    }

    pub fn __repr__(&self) -> String {
        match &self.0 {
            Selection::Verbosity(v) => format!("Selection.verbosity({v})"),
            Selection::Named(names) => format!("Selection.named({names:?})"),
            Selection::Pattern(pattern) => format!("Selection.pattern({pattern:?})"),
            Selection::Diagnostics => "Selection.diagnostics()".to_string(),
        }
    }
}

/// The reporting frequency. Only the timestep frequency runs in this
/// build; the period frequencies arrive with the reporting-frequency entry.
#[pyclass(name = "Frequency", from_py_object)]
#[derive(Debug, Clone, Copy)]
pub struct PyFrequency(pub Frequency);

#[pymethods]
impl PyFrequency {
    /// Every timestep is a row.
    #[staticmethod]
    pub fn timestep() -> Self {
        Self(Frequency::Timestep)
    }

    pub fn __repr__(&self) -> String {
        "Frequency.timestep()".to_string()
    }
}

/// Where a report's batches go.
#[pyclass(name = "Destination", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyDestination(pub Destination);

#[pymethods]
impl PyDestination {
    /// Every flushed batch retained in memory: the report's frame.
    #[staticmethod]
    pub fn memory() -> Self {
        Self(Destination::Memory)
    }

    /// Stream to a Parquet file, published at the run's end. An existing
    /// target fails the run unless `overwrite` is set.
    #[staticmethod]
    #[pyo3(signature = (path, overwrite = false))]
    pub fn parquet(path: String, overwrite: bool) -> Self {
        Self(Destination::Parquet {
            path: path.into(),
            overwrite,
        })
    }

    /// Stream to a CSV file, published at the run's end. An existing
    /// target fails the run unless `overwrite` is set.
    #[staticmethod]
    #[pyo3(signature = (path, overwrite = false))]
    pub fn csv(path: String, overwrite: bool) -> Self {
        Self(Destination::Csv {
            path: path.into(),
            overwrite,
        })
    }

    pub fn __repr__(&self) -> String {
        match &self.0 {
            Destination::Memory => "Destination.memory()".to_string(),
            Destination::Parquet { path, overwrite } => format!(
                "Destination.parquet({}, overwrite={overwrite:?})",
                path.display()
            ),
            Destination::Csv { path, overwrite } => format!(
                "Destination.csv({}, overwrite={overwrite:?})",
                path.display()
            ),
        }
    }
}

/// One report request: what a run records and where it goes.
///
/// `ReportSpec(name, selection, destination, frequency=Frequency.timestep())`.
#[pyclass(name = "ReportSpec", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyReportSpec(pub ReportSpec);

#[pymethods]
impl PyReportSpec {
    #[new]
    #[pyo3(signature = (name, selection, destination, frequency = None))]
    pub fn new(
        name: String,
        selection: PySelection,
        destination: PyDestination,
        frequency: Option<PyFrequency>,
    ) -> PyResult<Self> {
        let spec = ReportSpec {
            name,
            selection: selection.0,
            frequency: frequency.map_or(Frequency::Timestep, |f| f.0),
            destination: destination.0,
        };
        Ok(Self(spec))
    }

    #[getter]
    pub fn name(&self) -> String {
        self.0.name.clone()
    }

    #[getter]
    pub fn selection(&self) -> PySelection {
        PySelection(self.0.selection.clone())
    }

    #[getter]
    pub fn destination(&self) -> PyDestination {
        PyDestination(self.0.destination.clone())
    }

    #[getter]
    pub fn frequency(&self) -> PyFrequency {
        PyFrequency(self.0.frequency)
    }

    pub fn __repr__(&self) -> String {
        format!(
            "ReportSpec(name={:?}, selection={}, destination={})",
            self.0.name,
            PySelection(self.0.selection.clone()).__repr__(),
            PyDestination(self.0.destination.clone()).__repr__(),
        )
    }
}
