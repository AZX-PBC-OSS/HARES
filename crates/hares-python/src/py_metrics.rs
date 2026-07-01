//! Python bindings for simulation metrics.

use std::collections::BTreeMap;

use hares_io::output::metrics::{
    EnvelopeComponentLoadsKwh, FullSimulationMetrics, GasEnergyMetrics, Reliability,
    SimulationCoverage,
};
use pyo3::prelude::*;

#[pyclass(frozen, name = "TotalEnergyKwh")]
pub struct PyTotalEnergyKwh {
    pub(crate) inner: hares_io::output::metrics::TotalEnergyKwh,
}

#[pymethods]
impl PyTotalEnergyKwh {
    #[getter]
    fn total(&self) -> f64 {
        self.inner.total
    }

    #[getter]
    fn per_end_use(&self) -> BTreeMap<String, f64> {
        self.inner.per_end_use.clone()
    }

    #[getter]
    fn duration_hours(&self) -> f64 {
        self.inner.duration_hours
    }

    fn __repr__(&self) -> String {
        format!(
            "TotalEnergyKwh(total={:.2} kWh, duration={:.1} h)",
            self.inner.total, self.inner.duration_hours
        )
    }
}

#[pyclass(frozen, name = "RollingPeakKw")]
pub struct PyRollingPeakKw {
    pub(crate) inner: hares_io::output::metrics::RollingPeakKw,
}

#[pymethods]
impl PyRollingPeakKw {
    #[getter]
    fn peak_15min_kw(&self) -> f64 {
        self.inner.peak_15min_kw
    }

    #[getter]
    fn peak_30min_kw(&self) -> f64 {
        self.inner.peak_30min_kw
    }

    #[getter]
    fn peak_60min_kw(&self) -> f64 {
        self.inner.peak_60min_kw
    }

    fn __repr__(&self) -> String {
        format!(
            "RollingPeakKw(15min={:.2}, 30min={:.2}, 60min={:.2} kW)",
            self.inner.peak_15min_kw, self.inner.peak_30min_kw, self.inner.peak_60min_kw
        )
    }
}

#[pyclass(frozen, name = "PeakPowerKw")]
pub struct PyPeakPowerKw {
    pub(crate) inner: hares_io::output::metrics::PeakPowerKw,
}

#[pymethods]
impl PyPeakPowerKw {
    #[getter]
    fn per_end_use(&self) -> BTreeMap<String, f64> {
        self.inner.per_end_use.clone()
    }

    #[getter]
    fn rolling_15min_kw(&self) -> f64 {
        self.inner.rolling.peak_15min_kw
    }

    #[getter]
    fn rolling_30min_kw(&self) -> f64 {
        self.inner.rolling.peak_30min_kw
    }

    #[getter]
    fn rolling_60min_kw(&self) -> f64 {
        self.inner.rolling.peak_60min_kw
    }

    fn __repr__(&self) -> String {
        format!(
            "PeakPowerKw(rolling_15min={:.2} kW, {} end uses)",
            self.inner.rolling.peak_15min_kw,
            self.inner.per_end_use.len()
        )
    }
}

#[pyclass(frozen, name = "GridInteractionMetrics")]
pub struct PyGridInteractionMetrics {
    pub(crate) inner: hares_io::output::metrics::GridInteractionMetrics,
}

#[pymethods]
impl PyGridInteractionMetrics {
    #[getter]
    fn peak_import_kw(&self) -> f64 {
        self.inner.peak_import_kw
    }

    #[getter]
    fn peak_export_kw(&self) -> f64 {
        self.inner.peak_export_kw
    }

    fn __repr__(&self) -> String {
        format!(
            "GridInteractionMetrics(import={:.2}, export={:.2} kW)",
            self.inner.peak_import_kw, self.inner.peak_export_kw
        )
    }
}

#[pyclass(frozen, name = "EnvelopeComponentLoadsKwh")]
pub struct PyEnvelopeComponentLoadsKwh {
    pub(crate) inner: EnvelopeComponentLoadsKwh,
}

#[pymethods]
impl PyEnvelopeComponentLoadsKwh {
    #[getter]
    fn window_solar_kwh(&self) -> f64 {
        self.inner.window_solar_kwh
    }

    #[getter]
    fn opaque_solar_lwr_kwh(&self) -> f64 {
        self.inner.opaque_solar_lwr_kwh
    }

    #[getter]
    fn interior_lwr_kwh(&self) -> f64 {
        self.inner.interior_lwr_kwh
    }

    #[getter]
    fn infiltration_kwh(&self) -> f64 {
        self.inner.infiltration_kwh
    }

    #[getter]
    fn ventilation_kwh(&self) -> f64 {
        self.inner.ventilation_kwh
    }

    #[getter]
    fn hvac_heating_kwh(&self) -> f64 {
        self.inner.hvac_heating_kwh
    }

    #[getter]
    fn hvac_cooling_kwh(&self) -> f64 {
        self.inner.hvac_cooling_kwh
    }

    #[getter]
    fn internal_gains_kwh(&self) -> f64 {
        self.inner.internal_gains_kwh
    }

    #[getter]
    fn duct_loss_kwh(&self) -> f64 {
        self.inner.duct_loss_kwh
    }

    fn __repr__(&self) -> String {
        format!(
            "EnvelopeComponentLoadsKwh(window_solar={:.2}, hvac_heating={:.2}, hvac_cooling={:.2} kWh)",
            self.inner.window_solar_kwh, self.inner.hvac_heating_kwh, self.inner.hvac_cooling_kwh
        )
    }
}

#[pyclass(frozen, name = "EfficiencyMetrics")]
pub struct PyEfficiencyMetrics {
    pub(crate) inner: hares_io::output::metrics::EfficiencyMetrics,
}

#[pymethods]
impl PyEfficiencyMetrics {
    #[getter]
    fn hvac_heating_cop(&self) -> Option<f64> {
        self.inner.hvac_heating_cop
    }

    #[getter]
    fn hvac_cooling_cop(&self) -> Option<f64> {
        self.inner.hvac_cooling_cop
    }

    #[getter]
    fn water_heater_cop(&self) -> Option<f64> {
        self.inner.water_heater_cop
    }

    #[getter]
    fn battery_round_trip_efficiency(&self) -> Option<f64> {
        self.inner.battery_round_trip_efficiency
    }

    fn __repr__(&self) -> String {
        format!(
            "EfficiencyMetrics(heating_cop={:?}, cooling_cop={:?}, battery_eff={:?})",
            self.inner.hvac_heating_cop,
            self.inner.hvac_cooling_cop,
            self.inner.battery_round_trip_efficiency
        )
    }
}

#[pyclass(frozen, name = "GasEnergyMetrics")]
pub struct PyGasEnergyMetrics {
    pub(crate) inner: GasEnergyMetrics,
}

#[pymethods]
impl PyGasEnergyMetrics {
    #[getter]
    fn total_therms(&self) -> f64 {
        self.inner.total_therms
    }

    #[getter]
    fn total_kwh_equivalent(&self) -> f64 {
        self.inner.total_kwh_equivalent
    }

    fn __repr__(&self) -> String {
        format!(
            "GasEnergyMetrics({:.2} therms, {:.2} kWh)",
            self.inner.total_therms, self.inner.total_kwh_equivalent
        )
    }
}

#[pyclass(frozen, name = "SimulationMetrics")]
pub struct PySimulationMetrics {
    pub(crate) inner: FullSimulationMetrics,
}

#[pymethods]
impl PySimulationMetrics {
    #[getter]
    fn total_energy_kwh(&self) -> PyTotalEnergyKwh {
        PyTotalEnergyKwh {
            inner: self.inner.metrics.total_energy_kwh.clone(),
        }
    }

    #[getter]
    fn peak_power_kw(&self) -> PyPeakPowerKw {
        PyPeakPowerKw {
            inner: self.inner.metrics.peak_power_kw.clone(),
        }
    }

    #[getter]
    fn comfort_hours(&self) -> Option<f64> {
        self.inner.metrics.comfort_hours
    }

    #[getter]
    fn unmet_load_hours(&self) -> Option<f64> {
        self.inner.metrics.unmet_load_hours
    }

    #[getter]
    fn renewable_energy_fraction(&self) -> Option<f64> {
        self.inner.metrics.renewable_energy_fraction
    }

    #[getter]
    fn grid_interaction(&self) -> PyGridInteractionMetrics {
        PyGridInteractionMetrics {
            inner: self.inner.metrics.grid_interaction_metrics.clone(),
        }
    }

    #[getter]
    fn envelope_loads_kwh(&self) -> Option<PyEnvelopeComponentLoadsKwh> {
        self.inner
            .metrics
            .envelope_loads_kwh
            .as_ref()
            .map(|e| PyEnvelopeComponentLoadsKwh { inner: e.clone() })
    }

    #[getter]
    fn efficiency(&self) -> PyEfficiencyMetrics {
        PyEfficiencyMetrics {
            inner: self.inner.metrics.efficiency.clone(),
        }
    }

    #[getter]
    fn rows_with_partial_setpoint_data_fraction(&self) -> Option<f64> {
        self.inner.metrics.rows_with_partial_setpoint_data_fraction
    }

    #[getter]
    fn simulation_duration_hours(&self) -> f64 {
        self.inner.metrics.simulation_duration_hours
    }

    #[getter]
    fn coverage(&self) -> String {
        match self.inner.metrics.coverage {
            SimulationCoverage::FullYear => "FullYear".to_string(),
            SimulationCoverage::LeapYear => "LeapYear".to_string(),
            SimulationCoverage::PartialYear => "PartialYear".to_string(),
            SimulationCoverage::MultiYear => "MultiYear".to_string(),
        }
    }

    #[getter]
    fn nan_step_count(&self) -> u64 {
        self.inner.metrics.nan_step_count
    }

    #[getter]
    fn metrics_reliability(&self) -> String {
        match self.inner.metrics.metrics_reliability {
            Reliability::Reliable => "Reliable".to_string(),
            Reliability::Degraded => "Degraded".to_string(),
        }
    }

    #[getter]
    fn gas_energy(&self) -> Option<PyGasEnergyMetrics> {
        self.inner
            .gas_energy
            .as_ref()
            .map(|g| PyGasEnergyMetrics { inner: g.clone() })
    }

    fn __repr__(&self) -> String {
        let total = &self.inner.metrics.total_energy_kwh;
        let gas = self.inner.gas_energy.as_ref();
        if let Some(g) = gas {
            format!(
                "SimulationMetrics(electric={:.0} kWh, gas={:.0} therms, peak={:.1} kW)",
                total.total, g.total_therms, self.inner.metrics.peak_power_kw.rolling.peak_15min_kw
            )
        } else {
            format!(
                "SimulationMetrics(electric={:.0} kWh, peak={:.1} kW)",
                total.total, self.inner.metrics.peak_power_kw.rolling.peak_15min_kw
            )
        }
    }
}
