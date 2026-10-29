//! The scenario spec's Python surface: the declared spec as a dict, the
//! deterministic sampler, and the sampled record's forms. The spec's
//! schema is the Rust `ScenarioSpec`'s serde shape (the distributions'
//! names, the targets, the correlation groups); the errors are the
//! spec's own validation errors.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use hares_types::scenario::ScenarioSpec;

/// Convert a Python object to JSON through its `dict` form: the spec's
/// declaration is a plain dict of plain values.
fn spec_from_object(obj: &Bound<'_, PyAny>) -> PyResult<ScenarioSpec> {
    let json = python_to_json(obj)?;
    serde_json::from_value(json)
        .map_err(|err| PyValueError::new_err(format!("the scenario spec does not parse: {err}")))
}

fn python_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(serde_json::json!(i));
    }
    if let Ok(u) = obj.extract::<u64>() {
        return Ok(serde_json::json!(u));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Ok(serde_json::json!(f));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(serde_json::json!(s));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (key, value) in dict.iter() {
            let key: String = key.extract()?;
            map.insert(key, python_to_json(&value)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    if let Ok(list) = obj.extract::<Vec<Bound<'_, PyAny>>>() {
        let items = list
            .iter()
            .map(python_to_json)
            .collect::<PyResult<Vec<_>>>()?;
        return Ok(serde_json::Value::Array(items));
    }
    Err(PyValueError::new_err(format!(
        "the scenario spec carries an unsupported value of type {}",
        obj.get_type()
            .name()
            .map(|name| name.to_string())
            .unwrap_or_default()
    )))
}

fn json_to_python(py: Python<'_>, json: serde_json::Value) -> PyResult<Py<PyAny>> {
    Ok(match json {
        serde_json::Value::Null => py.None(),
        serde_json::Value::Bool(b) => b.into_pyobject(py)?.to_owned().unbind().into_any(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.unbind().into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.unbind().into_any()
            } else {
                n.as_f64()
                    .unwrap_or(f64::NAN)
                    .into_pyobject(py)?
                    .unbind()
                    .into_any()
            }
        }
        serde_json::Value::String(s) => s.into_pyobject(py)?.unbind().into_any(),
        serde_json::Value::Array(items) => {
            let list = pyo3::types::PyList::empty(py);
            for item in items {
                list.append(json_to_python(py, item)?)?;
            }
            list.into_pyobject(py)?.unbind().into_any()
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, value) in map {
                dict.set_item(key, json_to_python(py, value)?)?;
            }
            dict.into_pyobject(py)?.unbind().into_any()
        }
    })
}

fn spec_error(err: hares_types::HaresError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

/// The declared scenario: the parameters' distributions, units, targets
/// and correlation groups, and the deterministic sampler that turns
/// `(master_seed, scenario_id, bldg_id)` into each home's parameter set.
///
/// The spec is declared as a dict, the Rust schema's serde shape: for
/// example ``{"name": "study", "parameters": [{"name": "occupants",
/// "unit": "count", "target": {"override": {"equipment": "Occupancy",
/// "field": "number_of_occupants"}}, "distribution": {"uniform":
/// {"low": 1.0, "high": 5.0}}}]}``. The target is either
/// ``{"override": {"equipment": ..., "field": ...}}`` (the value lands
/// in the dwelling's overrides map, validated by the assembly's own
/// unknown-field rejection) or ``{"patch": {"field": ...}}`` (the HPXML
/// data patches). An optional ``"correlation": {"group": ..., "rho":
/// ...}`` correlates the parameters sharing a group.
#[pyclass(name = "ScenarioSpec", from_py_object)]
#[derive(Clone)]
pub struct PyScenarioSpec {
    pub(crate) inner: ScenarioSpec,
}

#[pymethods]
impl PyScenarioSpec {
    #[new]
    pub fn new(spec: &Bound<'_, PyAny>) -> PyResult<Self> {
        let inner = spec_from_object(spec)?;
        inner.validate().map_err(spec_error)?;
        Ok(Self { inner })
    }

    /// The scenario's name.
    #[getter]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// The home's sampled scenario: the same (master seed, scenario id,
    /// building id) reproduces the same parameter set bitwise; a
    /// different scenario, building or master seed draws differently.
    /// The record is a dict of the scenario id, the building id, and the
    /// values (each parameter's name, value and unit).
    pub fn sample(&self, master_seed: u64, scenario_id: u64, bldg_id: i64) -> PyResult<Py<PyAny>> {
        let sample = self
            .inner
            .sample(master_seed, scenario_id, bldg_id)
            .map_err(spec_error)?;
        let json = serde_json::to_value(&sample).map_err(|err| {
            PyValueError::new_err(format!("the sample does not serialize: {err}"))
        })?;
        Python::attach(|py| json_to_python(py, json))
    }

    /// The home's sampled scenario in the overrides form the dwelling
    /// constructor's ``overrides=`` accept: every Override target's
    /// value at ``{"<equipment>": {"<field>": value}}``. The Patch
    /// targets do not appear here (they apply through the settings'
    /// patches).
    pub fn sample_overrides(
        &self,
        master_seed: u64,
        scenario_id: u64,
        bldg_id: i64,
    ) -> PyResult<Py<PyAny>> {
        let sample = self
            .inner
            .sample(master_seed, scenario_id, bldg_id)
            .map_err(spec_error)?;
        let json = sample.overrides_json();
        Python::attach(|py| json_to_python(py, json))
    }
}
