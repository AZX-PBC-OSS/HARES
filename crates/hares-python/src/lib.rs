//! PyO3 bindings for the HARES simulation engine.

use pyo3::prelude::*;

mod conversions;
mod py_actor;
mod py_control;
mod py_dwelling;
mod py_equipment;
mod py_fleet;
mod py_gym;
mod py_telemetry;

use py_actor::{PyActor, PyDRLevel, PyDispatchRequest, PyMode, PyPriority, PySignal};
use py_control::PyControlSignal;
use py_dwelling::{PyDwelling, PyTimestepsIter};
use py_equipment::{PyBattery, PyEv, PyPv, PyPvSoilingConfig};
use py_fleet::{PyFleet, PyFleetResults};
use py_telemetry::PyTelemetry;

#[pymodule]
fn _hares(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDwelling>()?;
    m.add_class::<PyTimestepsIter>()?;
    m.add_class::<PyControlSignal>()?;
    m.add_class::<PyTelemetry>()?;
    m.add_class::<PyBattery>()?;
    m.add_class::<PyPv>()?;
    m.add_class::<PyPvSoilingConfig>()?;
    m.add_class::<PyEv>()?;
    m.add_class::<PyFleet>()?;
    m.add_class::<PyFleetResults>()?;
    m.add_class::<PyActor>()?;
    m.add_class::<PyDispatchRequest>()?;
    m.add_class::<PyMode>()?;
    m.add_class::<PyPriority>()?;
    m.add_class::<PySignal>()?;
    m.add_class::<PyDRLevel>()?;
    m.add_function(wrap_pyfunction!(py_gym::batch_step_py, m)?)?;
    Ok(())
}
