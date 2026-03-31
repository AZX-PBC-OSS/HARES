//! Type conversions between Rust and Python.

use std::sync::Arc;

use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, FixedOffset, SecondsFormat};
use hares_core::StepResult;
use hares_fleet::{DwellingMetrics, FleetResults, SimStatus};
use numpy::{IntoPyArray, PyArray1};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::types::PyDict;

use crate::py_fleet::PyFleetResults;

/// Route to the IPC path when Arrow batches are available, otherwise fall back to the
/// step-result path.
pub fn batches_or_steps_to_polars_df(
    py: Python<'_>,
    batches: &[RecordBatch],
    steps: &[StepResult],
) -> PyResult<Py<PyAny>> {
    if batches.is_empty() {
        steps_to_polars_df(py, steps)
    } else {
        record_batches_to_polars_df(py, batches)
    }
}

/// Convert in-memory dwelling step results to a polars `DataFrame`.
pub fn steps_to_polars_df(py: Python<'_>, steps: &[StepResult]) -> PyResult<Py<PyAny>> {
    let mut times = Vec::with_capacity(steps.len());
    let mut total_power_kw = Vec::with_capacity(steps.len());

    for step in steps {
        times.push(step.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true));
        total_power_kw.push(step.net_electric_power_kw);
    }

    let data = PyDict::new(py);
    data.set_item("Time", times)?;
    data.set_item("Total Electric Power (kW)", total_power_kw)?;

    let polars = py.import("polars")?;
    let df = polars.getattr("DataFrame")?.call1((data,))?;
    Ok(df.unbind())
}

/// Convert one or more Arrow `RecordBatch` values into a polars `DataFrame` via Arrow IPC.
pub fn record_batches_to_polars_df(py: Python<'_>, batches: &[RecordBatch]) -> PyResult<Py<PyAny>> {
    if batches.is_empty() {
        let polars = py.import("polars")?;
        let df = polars.getattr("DataFrame")?.call1((PyDict::new(py),))?;
        return Ok(df.unbind());
    }

    let schema = batches[0].schema();
    for batch in batches {
        if batch.schema().fields() != schema.fields() {
            return Err(PyValueError::new_err(
                "all record batches must share the same schema",
            ));
        }
    }

    let mut ipc_buffer = Vec::new();
    {
        let mut writer = FileWriter::try_new(&mut ipc_buffer, &schema)
            .map_err(|e| PyValueError::new_err(format!("failed to create IPC writer: {}", e)))?;

        for batch in batches {
            writer.write(batch).map_err(|e| {
                PyValueError::new_err(format!("failed to write batch to IPC: {}", e))
            })?;
        }

        writer
            .finish()
            .map_err(|e| PyValueError::new_err(format!("failed to finish IPC writer: {}", e)))?;
    }

    let io = py.import("io")?;
    let buf = io
        .getattr("BytesIO")?
        .call1((pyo3::types::PyBytes::new(py, &ipc_buffer),))?;

    let polars = py.import("polars")?;
    let read_ipc = polars.getattr("read_ipc")?;
    let df = read_ipc.call1((buf,))?;
    Ok(df.unbind())
}

/// Convert fleet per-dwelling metrics into a polars `DataFrame`.
pub fn dwelling_metrics_to_polars_df(
    py: Python<'_>,
    metrics: &[DwellingMetrics],
) -> PyResult<Py<PyAny>> {
    let mut annual_energy_kwh = Vec::with_capacity(metrics.len());
    let mut peak_power_kw = Vec::with_capacity(metrics.len());
    let mut sample_weight = Vec::with_capacity(metrics.len());
    let mut status = Vec::with_capacity(metrics.len());
    let mut failed = Vec::with_capacity(metrics.len());

    for metric in metrics {
        annual_energy_kwh.push(metric.annual_energy_kwh);
        peak_power_kw.push(metric.peak_power_kw);
        sample_weight.push(metric.sample_weight);
        status.push(sim_status_to_string(&metric.status));
        failed.push(metric.failed);
    }

    let data = PyDict::new(py);
    data.set_item("annual_energy_kwh", annual_energy_kwh)?;
    data.set_item("peak_power_kw", peak_power_kw)?;
    data.set_item("sample_weight", sample_weight)?;
    data.set_item("status", status)?;
    data.set_item("failed", failed)?;

    let polars = py.import("polars")?;
    let df = polars.getattr("DataFrame")?.call1((data,))?;
    Ok(df.unbind())
}

/// Convert fleet results into the Python wrapper type.
#[must_use]
pub fn fleet_results_to_py(results: FleetResults) -> PyFleetResults {
    PyFleetResults::new(results)
}

/// Expose contiguous `f64` observations as a NumPy array.
#[allow(dead_code)]
pub fn obs_to_numpy<'py>(py: Python<'py>, values: Vec<f64>) -> Bound<'py, PyArray1<f64>> {
    values.into_pyarray(py)
}

/// Convenience helper for converting a shared Arrow batch reference into a DataFrame.
#[allow(dead_code)]
pub fn record_batch_arc_to_polars_df(
    py: Python<'_>,
    batch: &Arc<RecordBatch>,
) -> PyResult<Py<PyAny>> {
    record_batches_to_polars_df(py, std::slice::from_ref(batch.as_ref()))
}

pub fn chrono_to_py_datetime(py: Python<'_>, dt: DateTime<FixedOffset>) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (dt.to_rfc3339(),))?;
    Ok(obj.unbind())
}

fn sim_status_to_string(status: &SimStatus) -> String {
    match status {
        SimStatus::Ok => "ok".to_string(),
        SimStatus::Flagged(message) => format!("flagged:{message}"),
        SimStatus::Failed(message) => format!("failed:{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::sim_status_to_string;
    use hares_fleet::SimStatus;

    #[test]
    fn test_sim_status_to_string_ok() {
        let status = SimStatus::Ok;
        let result = sim_status_to_string(&status);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_sim_status_to_string_flagged() {
        let status = SimStatus::Flagged("soft limit exceeded".to_string());
        let result = sim_status_to_string(&status);
        assert_eq!(result, "flagged:soft limit exceeded");
    }

    #[test]
    fn test_sim_status_to_string_flagged_no_message() {
        let status = SimStatus::Flagged(String::new());
        let result = sim_status_to_string(&status);
        assert_eq!(result, "flagged:");
    }

    #[test]
    fn test_sim_status_to_string_failed() {
        let status = SimStatus::Failed("division by zero".to_string());
        let result = sim_status_to_string(&status);
        assert_eq!(result, "failed:division by zero");
    }

    #[test]
    fn test_sim_status_to_string_failed_no_message() {
        let status = SimStatus::Failed(String::new());
        let result = sim_status_to_string(&status);
        assert_eq!(result, "failed:");
    }
}
