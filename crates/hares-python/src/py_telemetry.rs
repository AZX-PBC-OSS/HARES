//! Python bindings for telemetry output.

use hares_core::DwellingTelemetry;
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "PyTelemetry")]
#[derive(Debug)]
pub struct PyTelemetry {
    pub(crate) inner: DwellingTelemetry,
}

impl PyTelemetry {
    pub(crate) fn new(inner: DwellingTelemetry) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyTelemetry {
    pub fn zone<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        out.set_item("names", &self.inner.zone_names)?;
        out.set_item("temperature_c", &self.inner.zone_temperatures_c)?;
        out.set_item("setpoint_heat_c", &self.inner.setpoint_heat_c)?;
        out.set_item("setpoint_cool_c", &self.inner.setpoint_cool_c)?;
        out.set_item("outdoor_temp_c", self.inner.outdoor_temp_c)?;
        out.set_item("outdoor_rh", self.inner.outdoor_rh)?;
        Ok(out)
    }

    pub fn equipment<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        out.set_item("names", &self.inner.equipment_names)?;
        out.set_item("modes", &self.inner.equipment_modes)?;
        out.set_item("states", &self.inner.equipment_states)?;
        out.set_item("soc", &self.inner.equipment_soc)?;
        out.set_item("power_kw", &self.inner.equipment_power_kw)?;
        Ok(out)
    }

    pub fn total_power_kw(&self) -> f64 {
        self.inner.total_power_kw
    }

    fn __repr__(&self) -> String {
        format!(
            "PyTelemetry(step={}, time={})",
            self.inner.timestep_index, self.inner.current_time
        )
    }
}
