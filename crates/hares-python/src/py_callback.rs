//! The batch callback in Python: a callable attached to a dwelling
//! receives every flushed batch of the run's report.
//!
//! The attached object is called with the batch's info and the batch
//! itself as the frame surface the Python bindings already hand out (a
//! polars `DataFrame` built over the Arrow batch). The engine's step runs
//! with the GIL released, so the call re-acquires it here; a Python
//! exception surfaces as the typed callback error that fails the run,
//! never as a panic through the recorder. The destinations work plugs in
//! the config-side marker constructor, and the dedicated Python exception
//! class with the original exception chained as `__cause__` arrives with
//! the surface row that owns it; until then the mapped simulation error
//! carries the cause's message.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arrow::record_batch::RecordBatch;
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_types::panic_hook::panic_payload_to_string;
use pyo3::prelude::*;

use crate::conversions::record_batches_to_polars_df;

/// The `info` the attached callable receives with every batch: the
/// report's name, the building the run simulates and the batch's index in
/// the run, counting from 0.
#[pyclass(frozen, name = "BatchInfo")]
pub struct PyBatchInfo {
    #[pyo3(get)]
    pub report: String,
    #[pyo3(get)]
    pub bldg_id: i64,
    #[pyo3(get)]
    pub batch_index: u64,
}

/// The Rust sink over a Python callable: the attached object is called
/// once per flushed batch, in flush order.
pub(crate) struct PyBatchCallback {
    callable: Py<PyAny>,
}

impl PyBatchCallback {
    pub(crate) fn new(callable: Py<PyAny>) -> Self {
        Self { callable }
    }
}

impl BatchCallback for PyBatchCallback {
    fn on_batch(
        &mut self,
        ctx: &BatchContext<'_>,
        batch: &RecordBatch,
    ) -> Result<(), CallbackError> {
        let report = ctx.report.to_string();
        let bldg_id = ctx.bldg_id;
        let batch_index = ctx.batch_index;
        let batch = batch.clone();

        // The step runs with the GIL released (the binding detaches
        // around it); the call re-acquires it here. A panic inside the
        // callable is caught and typed, never let through the recorder as
        // an unwind.
        let delivered = catch_unwind(AssertUnwindSafe(|| {
            Python::attach(|py| -> Result<(), String> {
                let info = Py::new(
                    py,
                    PyBatchInfo {
                        report: report.clone(),
                        bldg_id,
                        batch_index,
                    },
                )
                .map_err(|err| format!("the batch info object could not be built: {err}"))?;
                let frame = record_batches_to_polars_df(py, std::slice::from_ref(&batch))
                    .map_err(|err| format!("the batch frame could not be built: {err}"))?;
                self.callable
                    .call1(py, (info, frame))
                    .map_err(|err| format!("{err}"))?;
                Ok(())
            })
        }));

        match delivered {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(CallbackError::new(message)),
            Err(payload) => Err(CallbackError::new(format!(
                "the attached callable panicked: {}",
                panic_payload_to_string(payload)
            ))),
        }
    }
}
