//! The input schema's Python surface: `hares.input_schema()` returns the
//! machine-readable schema of every input the engine reads, as a dict of
//! the schema's sections.

use pyo3::prelude::*;

/// The machine-readable input schema, as a dict: the config fields, the
/// dwelling and settings fields, the override classes, the removed keys,
/// the schedule columns (read and unused), the defaults files, and the
/// HPXML elements (read and rejected). Serialises to JSON unchanged.
#[pyfunction]
pub fn input_schema(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let schema = hares_core::input_schema();
    let json = serde_json::to_value(&schema)
        .map_err(|err| pyo3::exceptions::PyValueError::new_err(err.to_string()))?;
    crate::py_tariff::json_value_to_py(py, &json)
}
