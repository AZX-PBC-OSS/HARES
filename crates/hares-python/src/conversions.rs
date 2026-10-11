//! Type conversions between Rust and Python.

use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use chrono::NaiveDateTime;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::types::PyDict;

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

    // Detect duplicate field names before writing IPC. Arrow IPC readers
    // (including Polars) deduplicate schemas by field name, causing a
    // panic when batch-column-count and schema-column-count diverge.
    // This guard catches duplicate column names produced by schema
    // builders so the failure is a clean PyValueError instead of a panic.
    {
        let mut seen = std::collections::HashSet::new();
        for field in schema.fields() {
            if !seen.insert(field.name()) {
                return Err(PyValueError::new_err(format!(
                    "duplicate column name in output schema: '{}'. \
                     This is a bug in the schema builder — report it.",
                    field.name()
                )));
            }
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

/// Convert a naive local wall-clock time into a naive Python `datetime`:
/// the representation the step dict's timestamp and the environment's
/// `current_time` carry, the value the frame's Time column spells.
pub fn naive_chrono_to_py_datetime(py: Python<'_>, dt: NaiveDateTime) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1(
        "fromisoformat",
        (dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),),
    )?;
    Ok(obj.unbind())
}
