//! Python bindings for equipment configuration.

use pyo3::prelude::*;

#[pyclass(name = "Battery")]
#[derive(Debug)]
pub struct PyBattery {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kwh: f64,
    #[pyo3(get)]
    pub max_charge_kw: Option<f64>,
    #[pyo3(get)]
    pub max_discharge_kw: Option<f64>,
}

#[pymethods]
impl PyBattery {
    #[new]
    #[pyo3(signature = (name, capacity_kwh, max_charge_kw=None, max_discharge_kw=None))]
    fn new(
        name: String,
        capacity_kwh: f64,
        max_charge_kw: Option<f64>,
        max_discharge_kw: Option<f64>,
    ) -> Self {
        Self {
            name,
            capacity_kwh,
            max_charge_kw,
            max_discharge_kw,
        }
    }
}

#[pyclass(name = "PV")]
#[derive(Debug)]
pub struct PyPv {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kw: f64,
    #[pyo3(get)]
    pub tilt: f64,
    #[pyo3(get)]
    pub azimuth: f64,
}

#[pymethods]
impl PyPv {
    #[new]
    #[pyo3(signature = (name, capacity_kw, tilt, azimuth))]
    fn new(name: String, capacity_kw: f64, tilt: f64, azimuth: f64) -> Self {
        Self {
            name,
            capacity_kw,
            tilt,
            azimuth,
        }
    }
}

#[pyclass(name = "EV")]
#[derive(Debug)]
pub struct PyEv {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kwh: Option<f64>,
    #[pyo3(get)]
    pub max_charging_kw: Option<f64>,
}

#[pymethods]
impl PyEv {
    #[new]
    #[pyo3(signature = (name, capacity_kwh=None, max_charging_kw=None))]
    fn new(name: String, capacity_kwh: Option<f64>, max_charging_kw: Option<f64>) -> Self {
        Self {
            name,
            capacity_kwh,
            max_charging_kw,
        }
    }
}
