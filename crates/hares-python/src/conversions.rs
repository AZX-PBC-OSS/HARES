//! Type conversions between Rust and Python.

use std::sync::Arc;

use arrow::array::{Array, Float64Array, StringArray};
use arrow::record_batch::RecordBatch;
use chrono::SecondsFormat;
use hares_core::StepResult;
use hares_fleet::{DwellingMetrics, FleetResults, SimStatus};
use numpy::{IntoPyArray, PyArray1};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::types::{PyDict, PyList};

use crate::py_fleet::PyFleetResults;

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

/// Convert one or more Arrow `RecordBatch` values into a polars `DataFrame`.
pub fn record_batches_to_polars_df(py: Python<'_>, batches: &[RecordBatch]) -> PyResult<Py<PyAny>> {
    if batches.is_empty() {
        let polars = py.import("polars")?;
        let df = polars.getattr("DataFrame")?.call1((PyDict::new(py),))?;
        return Ok(df.unbind());
    }

    let schema = batches[0].schema();
    let mut columns: Vec<(String, Vec<Py<PyAny>>)> = schema
        .fields()
        .iter()
        .map(|field| (field.name().to_string(), Vec::new()))
        .collect();

    for batch in batches {
        if batch.schema().fields() != schema.fields() {
            return Err(PyValueError::new_err(
                "all record batches must share the same schema",
            ));
        }

        for (col_idx, (_name, values)) in columns.iter_mut().enumerate() {
            let array = batch.column(col_idx);
            if let Some(arr) = array.as_any().downcast_ref::<Float64Array>() {
                for row in 0..batch.num_rows() {
                    if arr.is_null(row) {
                        values.push(py.None());
                    } else {
                        values.push(arr.value(row).into_pyobject(py)?.unbind().into());
                    }
                }
                continue;
            }

            if let Some(arr) = array.as_any().downcast_ref::<StringArray>() {
                for row in 0..batch.num_rows() {
                    if arr.is_null(row) {
                        values.push(py.None());
                    } else {
                        values.push(arr.value(row).into_pyobject(py)?.unbind().into());
                    }
                }
                continue;
            }

            return Err(PyValueError::new_err(format!(
                "unsupported Arrow column type for `{}`",
                schema.field(col_idx).name()
            )));
        }
    }

    let data = PyDict::new(py);
    for (name, values) in columns {
        let list = PyList::new(py, values)?;
        data.set_item(name, list)?;
    }

    let polars = py.import("polars")?;
    let df = polars.getattr("DataFrame")?.call1((data,))?;
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

fn sim_status_to_string(status: &SimStatus) -> String {
    match status {
        SimStatus::Ok => "ok".to_string(),
        SimStatus::Flagged(message) => format!("flagged:{message}"),
        SimStatus::Failed(message) => format!("failed:{message}"),
    }
}
