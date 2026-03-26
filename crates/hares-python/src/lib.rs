//! PyO3 bindings for the HARES simulation engine.

use pyo3::prelude::*;

mod conversions;
mod py_actor;
mod py_config;
mod py_control;
mod py_dwelling;
mod py_enums;
mod py_equipment;
mod py_fleet;
mod py_gym;
mod py_metrics;
mod py_telemetry;
mod py_weather;
mod utils;

use py_actor::{PyActor, PyDRLevel, PyDispatchRequest, PyMode, PyPriority, PySignal};
use py_config::{PyDwellingConfig, PySimulationConfig};
use py_control::PyControlSignal;
use py_dwelling::{PyDwelling, PyTimestepsIter};
use py_enums::{
    PyAggregationResolution, PyBatteryChemistry, PyChargingLevel, PyControlCapabilities,
    PyDriverArchetype, PyDutyCycleComponent, PyEndUse, PyExecutionStage, PyFluidType, PyFuelType,
    PyInverterPriority, PyLutType, PyResStockVersion, PySimStatus,
};
use py_equipment::{
    PyBattery, PyEquipmentDescriptor, PyEv, PyPv, PyPvSoilingConfig, PyTelemetryField,
};
use py_fleet::{PyFleet, PyFleetResults};
use py_metrics::{
    PyAnnualEnergyKwh, PyEfficiencyMetrics, PyEnvelopeComponentLoadsKwh, PyGasEnergyMetrics,
    PyGridInteractionMetrics, PyPeakPowerKw, PyRollingPeakKw, PySimulationMetrics,
};
use py_telemetry::PyTelemetry;
use py_weather::PyWeatherTimeSeries;

#[pymodule]
fn _hares(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDwelling>()?;
    m.add_class::<PyTimestepsIter>()?;
    m.add_class::<PySimulationConfig>()?;
    m.add_class::<PyDwellingConfig>()?;
    m.add_class::<PyControlSignal>()?;
    m.add_class::<PyTelemetry>()?;
    m.add_class::<PyBattery>()?;
    m.add_class::<PyPv>()?;
    m.add_class::<PyPvSoilingConfig>()?;
    m.add_class::<PyEv>()?;
    m.add_class::<PyTelemetryField>()?;
    m.add_class::<PyEquipmentDescriptor>()?;
    m.add_class::<PyFleet>()?;
    m.add_class::<PyFleetResults>()?;
    m.add_class::<PyActor>()?;
    m.add_class::<PyDispatchRequest>()?;
    m.add_class::<PyMode>()?;
    m.add_class::<PyPriority>()?;
    m.add_class::<PySignal>()?;
    m.add_class::<PyDRLevel>()?;
    m.add_class::<PyEndUse>()?;
    m.add_class::<PyFuelType>()?;
    m.add_class::<PyExecutionStage>()?;
    m.add_class::<PyFluidType>()?;
    m.add_class::<PyInverterPriority>()?;
    m.add_class::<PyDutyCycleComponent>()?;
    m.add_class::<PySimStatus>()?;
    m.add_class::<PyAggregationResolution>()?;
    m.add_class::<PyResStockVersion>()?;
    m.add_class::<PyControlCapabilities>()?;
    m.add_class::<PyLutType>()?;
    m.add_class::<PyBatteryChemistry>()?;
    m.add_class::<PyChargingLevel>()?;
    m.add_class::<PyDriverArchetype>()?;
    m.add_class::<PySimulationMetrics>()?;
    m.add_class::<PyAnnualEnergyKwh>()?;
    m.add_class::<PyPeakPowerKw>()?;
    m.add_class::<PyRollingPeakKw>()?;
    m.add_class::<PyGridInteractionMetrics>()?;
    m.add_class::<PyEnvelopeComponentLoadsKwh>()?;
    m.add_class::<PyEfficiencyMetrics>()?;
    m.add_class::<PyGasEnergyMetrics>()?;
    m.add_class::<PyWeatherTimeSeries>()?;

    m.add("OperatingMode", m.getattr("Mode")?)?;
    m.add("Mode", m.getattr("Mode")?)?;
    m.add("PyMode", m.getattr("Mode")?)?;

    m.add_function(wrap_pyfunction!(py_gym::batch_step_py, m)?)?;
    m.add_function(wrap_pyfunction!(py_weather::parse_weather, m)?)?;
    m.add_function(wrap_pyfunction!(py_weather::parse_epw, m)?)?;
    m.add_function(wrap_pyfunction!(py_weather::parse_psm3, m)?)?;
    m.add_function(wrap_pyfunction!(py_weather::parse_tmy3, m)?)?;
    m.add_function(wrap_pyfunction!(py_weather::parse_resstock_csv, m)?)?;
    Ok(())
}
