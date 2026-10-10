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

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use arrow::record_batch::RecordBatch;
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_types::panic_hook::panic_payload_to_string;
use pyo3::prelude::*;

use crate::conversions::record_batches_to_polars_df;

/// The message a dwelling accessor raises when a batch callback calls
/// back into the dwelling: the delivery thread already holds the
/// dwelling's mutex and the GIL across the call, so a re-entrant accessor
/// would park forever instead of returning.
pub(crate) const REENTRY_MESSAGE: &str = "a callback may not re-enter the dwelling";

thread_local! {
    /// Set on the delivery thread for the duration of the attached
    /// callable's invocation. A dwelling accessor checks this before
    /// acquiring the dwelling and refuses re-entry instead of parking.
    static DELIVERY_IN_PROGRESS: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread is inside a batch delivery: dwelling
/// accessor acquisition checks this and refuses re-entry.
pub(crate) fn delivery_in_progress() -> bool {
    DELIVERY_IN_PROGRESS.with(Cell::get)
}

/// Sets the delivery flag for its lifetime: the callable's invocation and
/// every nested call it makes run with the flag on, and the flag is
/// cleared when the delivery unwinds or returns.
struct DeliveryGuard;

impl DeliveryGuard {
    fn new() -> Self {
        DELIVERY_IN_PROGRESS.with(|flag| flag.set(true));
        Self
    }
}

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        DELIVERY_IN_PROGRESS.with(|flag| flag.set(false));
    }
}

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
        // The delivery thread may not re-enter the dwelling: the step
        // holds the dwelling's mutex (and the GIL is re-acquired for the
        // call itself), so an accessor called from here would park
        // forever. The guard marks this thread; the dwelling's accessors
        // read it and refuse.
        let _delivery = DeliveryGuard::new();
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
