//! The batch callback in Python: a callable attached to a dwelling
//! receives every flushed batch of the run's report.
//!
//! The attached object is called with the batch's info and the batch
//! itself as pyo3-arrow's `PyRecordBatch`: the Arrow PyCapsule array
//! interface over the batch's own buffers, so every Arrow consumer
//! (polars, pyarrow, pandas through pyarrow) imports it zero-copy. The
//! engine's step runs with the GIL released, so the call re-acquires it
//! here; a Python exception surfaces as the typed callback error that
//! fails the run, never as a panic through the recorder. The exception is
//! stored in a slot shared with the dwelling wrapper, and the binding
//! chains it as `__cause__` on the typed error it raises. The
//! config-side marker constructor is `Destination.callback()`.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use arrow::record_batch::RecordBatch;
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_types::panic_hook::panic_payload_to_string;
use pyo3::prelude::*;

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

/// The slot the adapter stores a callable's exception in: the dwelling
/// wrapper reads it when it maps the run's callback error, chaining the
/// exception as `__cause__` on the typed error it raises.
pub(crate) type CallbackCauseSlot = Arc<Mutex<Option<PyErr>>>;

/// The Rust sink over a Python callable: the attached object is called
/// once per flushed batch, in flush order.
pub(crate) struct PyBatchCallback {
    callable: Py<PyAny>,
    cause: CallbackCauseSlot,
}

impl PyBatchCallback {
    pub(crate) fn new(callable: Py<PyAny>, cause: CallbackCauseSlot) -> Self {
        Self { callable, cause }
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
            Python::attach(|py| -> Result<(), (String, PyErr)> {
                let info = Py::new(
                    py,
                    PyBatchInfo {
                        report: report.clone(),
                        bldg_id,
                        batch_index,
                    },
                )
                .map_err(|err| {
                    (
                        format!("the batch info object could not be built: {err}"),
                        err,
                    )
                })?;
                let record = Py::new(py, pyo3_arrow::PyRecordBatch::new(batch.clone()))
                    .map_err(|err| (format!("the batch frame could not be built: {err}"), err))?;
                if let Err(err) = self.callable.call1(py, (info, record)) {
                    let message = err.to_string();
                    return Err((message, err));
                }
                Ok(())
            })
        }));

        match delivered {
            Ok(Ok(())) => Ok(()),
            Ok(Err((message, cause))) => {
                if let Ok(mut slot) = self.cause.lock() {
                    *slot = Some(cause);
                }
                Err(CallbackError::new(message))
            }
            Err(payload) => Err(CallbackError::new(format!(
                "the attached callable panicked: {}",
                panic_payload_to_string(payload)
            ))),
        }
    }
}
