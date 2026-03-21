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

#[pyclass(name = "PvSoilingConfig")]
#[derive(Debug, Clone)]
pub struct PyPvSoilingConfig {
    #[pyo3(get)]
    pub cleaning_threshold_mm: f64,
    #[pyo3(get)]
    pub loss_rate_per_day: f64,
    #[pyo3(get)]
    pub grace_period_days: f64,
    #[pyo3(get)]
    pub max_loss: f64,
    #[pyo3(get)]
    pub initial_loss: f64,
    #[pyo3(get)]
    pub rain_accum_hours: f64,
}

#[pymethods]
impl PyPvSoilingConfig {
    #[new]
    #[pyo3(signature = (
        cleaning_threshold_mm=6.0,
        loss_rate_per_day=0.0015,
        grace_period_days=14.0,
        max_loss=0.30,
        initial_loss=0.0,
        rain_accum_hours=24.0,
    ))]
    fn new(
        cleaning_threshold_mm: f64,
        loss_rate_per_day: f64,
        grace_period_days: f64,
        max_loss: f64,
        initial_loss: f64,
        rain_accum_hours: f64,
    ) -> Self {
        Self {
            cleaning_threshold_mm,
            loss_rate_per_day,
            grace_period_days,
            max_loss,
            initial_loss,
            rain_accum_hours,
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
    #[pyo3(get)]
    pub soiling: Option<PyPvSoilingConfig>,
}

#[pymethods]
impl PyPv {
    #[new]
    #[pyo3(signature = (name, capacity_kw, tilt, azimuth, soiling=None))]
    fn new(
        name: String,
        capacity_kw: f64,
        tilt: f64,
        azimuth: f64,
        soiling: Option<PyPvSoilingConfig>,
    ) -> Self {
        Self {
            name,
            capacity_kw,
            tilt,
            azimuth,
            soiling,
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
