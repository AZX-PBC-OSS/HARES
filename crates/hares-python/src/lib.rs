//! PyO3 bindings for the HARES simulation engine.

use pyo3::prelude::*;

mod conversions;
mod py_control;
mod py_dwelling;
mod py_equipment;
mod py_fleet;
mod py_gym;
mod py_telemetry;

#[pymodule]
fn _hares(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
