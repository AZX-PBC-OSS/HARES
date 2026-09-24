//! Python bindings for telemetry output.

use chrono::{DateTime, FixedOffset};
use hares_core::DwellingTelemetry;
use hares_tariff::billing::BillingPeriodSummary;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn tz_to_py_datetime(py: Python<'_>, dt: DateTime<chrono_tz::Tz>) -> PyResult<Py<PyAny>> {
    let fixed: DateTime<FixedOffset> = dt.fixed_offset();
    fixed_to_py_datetime(py, fixed)
}

fn fixed_to_py_datetime(py: Python<'_>, dt: DateTime<FixedOffset>) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (dt.to_rfc3339(),))?;
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
        out.set_item("outdoor_humidity_ratio", self.inner.outdoor_humidity_ratio)?;
        out.set_item(
            "energy_balance_residuals",
            &self.inner.energy_balance_residuals,
        )?;
        Ok(out)
    }

    /// Non-authoritative diagnostics only.
    ///
    /// This telemetry dict is intended for debugging/inspection. For
    /// simulation-critical typed values, use `Dwelling.equipment()` and read
    /// `Equipment.core_output`.
    pub fn equipment<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        out.set_item("names", &self.inner.equipment_names)?;
        out.set_item("modes", &self.inner.equipment_modes)?;
        out.set_item("soc", &self.inner.equipment_soc)?;
        out.set_item("power_kw", &self.inner.equipment_power_kw)?;
        Ok(out)
    }

    /// Per-actor telemetry: actor_name → channel_name → value.
    ///
    /// Returns a dict mapping each actor name to its telemetry channels dict.
    /// Actors with no observable state (telemetry() returns None) are omitted.
    pub fn actors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for (name, channels) in &self.inner.actor_telemetry {
            let channel_dict = PyDict::new(py);
            for (key, value) in channels {
                channel_dict.set_item(key, *value)?;
            }
            out.set_item(name, channel_dict)?;
        }
        Ok(out)
    }

    #[getter]
    pub fn timestep_index(&self) -> u64 {
        self.inner.timestep_index
    }

    #[getter]
    pub fn initialized(&self) -> bool {
        self.inner.initialized
    }

    #[getter]
    pub fn current_time<'py>(&self, py: Python<'py>) -> PyResult<Py<PyAny>> {
        fixed_to_py_datetime(py, self.inner.current_time)
    }

    #[getter]
    pub fn total_power_kw(&self) -> f64 {
        self.inner.total_power_kw
    }

    #[getter]
    pub fn reactive_power_kvar(&self) -> f64 {
        self.inner.reactive_power_kvar
    }

    /// Load [kW] the island sources failed to cover during islanded
    /// operation (would-be phantom grid import). 0.0 when not islanded.
    #[getter]
    pub fn island_unserved_kw(&self) -> f64 {
        self.inner.island_unserved_kw
    }

    /// Surplus on-site generation [kW] the island could not absorb during
    /// islanded operation (would-be phantom grid export). 0.0 when not
    /// islanded.
    #[getter]
    pub fn island_excess_kw(&self) -> f64 {
        self.inner.island_excess_kw
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
