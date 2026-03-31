//! Python bindings for telemetry output.

use chrono::{DateTime, FixedOffset};
use hares_core::DwellingTelemetry;
use hares_tariff::billing::BillingPeriodSummary;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn tz_to_py_datetime(py: Python<'_>, dt: DateTime<chrono_tz::Tz>) -> PyResult<Py<PyAny>> {
    let fixed: DateTime<FixedOffset> = dt.fixed_offset();
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (fixed.to_rfc3339(),))?;
    Ok(obj.unbind())
}

#[pyclass(name = "Telemetry")]
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

    #[getter]
    pub fn total_power_kw(&self) -> f64 {
        self.inner.total_power_kw
    }

    #[getter]
    pub fn reactive_power_kvar(&self) -> f64 {
        self.inner.reactive_power_kvar
    }

    fn __repr__(&self) -> String {
        format!(
            "Telemetry(step={}, time={})",
            self.inner.timestep_index, self.inner.current_time
        )
    }
}

// ---------------------------------------------------------------------------
// BillingPeriodSummary
// ---------------------------------------------------------------------------

#[pyclass(name = "BillingPeriodSummary")]
pub struct PyBillingPeriodSummary {
    #[pyo3(get)]
    pub energy_charge_usd: f64,
    #[pyo3(get)]
    pub demand_charge_usd: f64,
    #[pyo3(get)]
    pub fixed_charge_usd: f64,
    #[pyo3(get)]
    pub export_credit_usd: f64,
    #[pyo3(get)]
    pub net_bill_usd: f64,
    #[pyo3(get)]
    pub peak_demand_kw: f64,
    #[pyo3(get)]
    pub total_import_kwh: f64,
    #[pyo3(get)]
    pub total_export_kwh: f64,
    period_start: DateTime<chrono_tz::Tz>,
    period_end: DateTime<chrono_tz::Tz>,
}

impl PyBillingPeriodSummary {
    pub(crate) fn from_rust(s: &BillingPeriodSummary) -> Self {
        Self {
            energy_charge_usd: s.energy_charge_usd,
            demand_charge_usd: s.demand_charge_usd,
            fixed_charge_usd: s.fixed_charge_usd,
            export_credit_usd: s.export_credit_usd,
            net_bill_usd: s.net_bill_usd,
            peak_demand_kw: s.peak_demand_kw,
            total_import_kwh: s.total_import_kwh,
            total_export_kwh: s.total_export_kwh,
            period_start: s.period_start,
            period_end: s.period_end,
        }
    }
}

#[pymethods]
impl PyBillingPeriodSummary {
    #[getter]
    fn period_start<'py>(&self, py: Python<'py>) -> PyResult<Py<PyAny>> {
        tz_to_py_datetime(py, self.period_start)
    }

    #[getter]
    fn period_end<'py>(&self, py: Python<'py>) -> PyResult<Py<PyAny>> {
        tz_to_py_datetime(py, self.period_end)
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("period_start", self.period_start(py)?)?;
        d.set_item("period_end", self.period_end(py)?)?;
        d.set_item("energy_charge_usd", self.energy_charge_usd)?;
        d.set_item("demand_charge_usd", self.demand_charge_usd)?;
        d.set_item("fixed_charge_usd", self.fixed_charge_usd)?;
        d.set_item("export_credit_usd", self.export_credit_usd)?;
        d.set_item("net_bill_usd", self.net_bill_usd)?;
        d.set_item("peak_demand_kw", self.peak_demand_kw)?;
        d.set_item("total_import_kwh", self.total_import_kwh)?;
        d.set_item("total_export_kwh", self.total_export_kwh)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "BillingPeriodSummary(period={} to {}, net_bill=${:.2})",
            self.period_start.format("%Y-%m-%d"),
            self.period_end.format("%Y-%m-%d"),
            self.net_bill_usd,
        )
    }
}

// ---------------------------------------------------------------------------
// TariffTelemetry
// ---------------------------------------------------------------------------

#[pyclass(name = "TariffTelemetry")]
pub struct PyTariffTelemetry {
    #[pyo3(get)]
    pub period_name: String,
    #[pyo3(get)]
    pub current_rate_usd_per_kwh: f64,
    #[pyo3(get)]
    pub export_rate_usd_per_kwh: f64,
    #[pyo3(get)]
    pub cumulative_import_kwh: f64,
    #[pyo3(get)]
    pub cumulative_export_kwh: f64,
    #[pyo3(get)]
    pub peak_demand_kw: f64,
    #[pyo3(get)]
    pub cumulative_energy_cost_usd: f64,
}

#[pymethods]
impl PyTariffTelemetry {
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("period_name", &self.period_name)?;
        d.set_item("current_rate_usd_per_kwh", self.current_rate_usd_per_kwh)?;
        d.set_item("export_rate_usd_per_kwh", self.export_rate_usd_per_kwh)?;
        d.set_item("cumulative_import_kwh", self.cumulative_import_kwh)?;
        d.set_item("cumulative_export_kwh", self.cumulative_export_kwh)?;
        d.set_item("peak_demand_kw", self.peak_demand_kw)?;
        d.set_item(
            "cumulative_energy_cost_usd",
            self.cumulative_energy_cost_usd,
        )?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "TariffTelemetry(period='{}', rate=${:.4}/kWh, import={:.2}kWh)",
            self.period_name, self.current_rate_usd_per_kwh, self.cumulative_import_kwh,
        )
    }
}
