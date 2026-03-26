//! Python bindings for equipment configuration.

use hares_types::{
    EquipmentDescriptor as RustEquipmentDescriptor, TelemetryField as RustTelemetryField,
};
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

    fn __repr__(&self) -> String {
        format!(
            "Battery(name={:?}, capacity_kwh={})",
            self.name, self.capacity_kwh
        )
    }
}

#[pyclass(name = "PvSoilingConfig", from_py_object)]
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

    fn __repr__(&self) -> String {
        format!(
            "PvSoilingConfig(cleaning_threshold_mm={}, loss_rate_per_day={})",
            self.cleaning_threshold_mm, self.loss_rate_per_day
        )
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

    fn __repr__(&self) -> String {
        format!(
            "PV(name={:?}, capacity_kw={}, tilt={}, azimuth={})",
            self.name, self.capacity_kw, self.tilt, self.azimuth
        )
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

    fn __repr__(&self) -> String {
        format!(
            "EV(name={:?}, capacity_kwh={:?}, max_charging_kw={:?})",
            self.name, self.capacity_kwh, self.max_charging_kw
        )
    }
}

#[pyclass(name = "TelemetryField", from_py_object)]
#[derive(Clone)]
pub struct PyTelemetryField {
    inner: RustTelemetryField,
}

impl PyTelemetryField {
    pub(crate) fn new(inner: RustTelemetryField) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyTelemetryField {
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    fn __repr__(&self) -> String {
        format!("TelemetryField({} ({}))", self.inner.name, self.inner.unit)
    }
}

impl From<RustTelemetryField> for PyTelemetryField {
    fn from(inner: RustTelemetryField) -> Self {
        Self::new(inner)
    }
}

#[pyclass(name = "EquipmentDescriptor", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyEquipmentDescriptor {
    inner: RustEquipmentDescriptor,
}

impl PyEquipmentDescriptor {
    pub(crate) fn new(inner: RustEquipmentDescriptor) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyEquipmentDescriptor {
    #[getter]
    fn id(&self) -> u32 {
        self.inner.id.0
    }

    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn end_use(&self) -> crate::py_enums::PyEndUse {
        crate::py_enums::PyEndUse::new(self.inner.end_use.clone())
    }

    #[getter]
    fn equipment_type(&self) -> String {
        self.inner.equipment_type.to_string()
    }

    #[getter]
    fn zone(&self) -> Option<u16> {
        self.inner.zone.map(|z| z.0)
    }

    #[getter]
    fn fuel_type(&self) -> crate::py_enums::PyFuelType {
        self.inner.fuel.into()
    }

    #[getter]
    fn stage(&self) -> crate::py_enums::PyExecutionStage {
        self.inner.stage.into()
    }

    #[getter]
    fn control_capabilities(&self) -> crate::py_enums::PyControlCapabilities {
        crate::py_enums::PyControlCapabilities::new(self.inner.control_capabilities)
    }

    #[getter]
    fn telemetry_fields(&self) -> Vec<PyTelemetryField> {
        self.inner
            .telemetry_fields
            .iter()
            .map(|f| PyTelemetryField::new(f.clone()))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "EquipmentDescriptor(name={:?}, equipment_type={:?}, end_use={})",
            self.inner.name,
            self.inner.equipment_type,
            self.inner.end_use.as_str()
        )
    }
}

impl From<RustEquipmentDescriptor> for PyEquipmentDescriptor {
    fn from(inner: RustEquipmentDescriptor) -> Self {
        Self::new(inner)
    }
}
