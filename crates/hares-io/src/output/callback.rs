//! The batch callback sink: a run's flushed batches streamed to an
//! attached Rust closure or trait object.
//!
//! A caller that wants to stream batches into its own store attaches a
//! [`BatchCallback`] through `Dwelling::attach_callback` (in
//! `hares-core`); the recorder calls it once per built batch of its
//! report, in flush order, with [`BatchContext::batch_index`] counting
//! from 0. Attaching is opt-in and zero cost when unused: a recorder with
//! no attachment is a branch on `None` per flush.
//!
//! The sink is the minimal form of the destination model: one report, one
//! attachment, the recorder the run already builds. The destinations work
//! plugs in here: `Destination::Callback` becomes the serialisable marker
//! a report carries in `SimulationConfig.reports`, the recorder becomes
//! per-report and constructs one adapter per `Callback` report (a
//! `Callback` report with nothing attached fails before the first step,
//! and attaching to a report whose destination is not `Callback` is an
//! error), the config marker round-trips through `to_toml`/`from_toml` as
//! the string `"callback"`, and a run's file destinations gain the
//! partial-name publish whose removal this module's error path keeps for
//! failed runs. The Python constructor of the marker arrives with the
//! Python surface row that adds `Destination` to the config.

use std::fmt;
use std::sync::{Arc, Mutex};

use arrow::record_batch::RecordBatch;

/// The report the engine builds today: the run's timestep table. The
/// destinations work replaces this fixed name with the report names the
/// configuration declares.
pub const TIMESTEP_REPORT: &str = "timestep";

/// Why a callback delivery failed. The message is the callback's own
/// cause, carried through to the run's error verbatim.
#[derive(Debug)]
pub struct CallbackError {
    pub message: String,
}

impl CallbackError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CallbackError {}

/// What the recorder tells the callback about the batch it is handing
/// over: the report's name, the building the run simulates and the
/// batch's index in the run, counting from 0.
pub struct BatchContext<'a> {
    pub report: &'a str,
    pub bldg_id: i64,
    pub batch_index: u64,
}

/// A sink the run streams its report's flushed batches to, once per built
/// batch, in flush order.
///
/// An error stops the run at that flush; the sink must not be called
/// again after the run's outputs are published (the final partial batch
/// at close, today) or after the run ended in an error.
pub trait BatchCallback: Send + Sync {
    fn on_batch(
        &mut self,
        ctx: &BatchContext<'_>,
        batch: &RecordBatch,
    ) -> Result<(), CallbackError>;
}

/// A callback attached to a report: the shared sink, the report's name
/// and the building id every context carries.
///
/// The attachment is shared on purpose: the dwelling holds one and hands
/// clones to every recorder the roster plan builds while the run is still
/// fresh, so a roster change before the first recorded row keeps the
/// attachment instead of silently dropping it.
#[derive(Clone)]
pub struct CallbackAttachment {
    report: Arc<str>,
    bldg_id: i64,
    callback: Arc<Mutex<Box<dyn BatchCallback>>>,
}

impl CallbackAttachment {
    pub fn new(report: impl Into<String>, bldg_id: i64, callback: Box<dyn BatchCallback>) -> Self {
        Self {
            report: Arc::from(report.into().as_str()),
            bldg_id,
            callback: Arc::new(Mutex::new(callback)),
        }
    }

    #[must_use]
    pub fn report(&self) -> &str {
        &self.report
    }

    #[must_use]
    pub fn bldg_id(&self) -> i64 {
        self.bldg_id
    }

    /// Delivers one batch to the sink. A sink that panicked is reported
    /// as a callback error, never as a panic through the recorder.
    pub(crate) fn deliver(
        &self,
        batch: &RecordBatch,
        batch_index: u64,
    ) -> Result<(), CallbackError> {
        let mut callback = self.callback.lock().map_err(|poisoned| {
            CallbackError::new(format!(
                "the callback panicked in an earlier delivery: {poisoned}"
            ))
        })?;
        let ctx = BatchContext {
            report: &self.report,
            bldg_id: self.bldg_id,
            batch_index,
        };
        callback.on_batch(&ctx, batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_callback_after_finish() {
        let schema = arrow::datatypes::Schema::new(vec![arrow::datatypes::Field::new(
            "Time",
            arrow::datatypes::DataType::Utf8,
            false,
        )]);

        struct Counting(Arc<std::sync::atomic::AtomicU64>);
        impl BatchCallback for Counting {
            fn on_batch(
                &mut self,
                _ctx: &BatchContext<'_>,
                _batch: &RecordBatch,
            ) -> Result<(), CallbackError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut recorder = crate::output::writer::StreamingRecorder::new(
            schema,
            10,
            crate::output::report::Destination::Memory,
        )
        .expect("the recorder builds");
        recorder.attach_callback(CallbackAttachment::new(
            TIMESTEP_REPORT,
            1,
            Box::new(Counting(Arc::clone(&calls))),
        ));

        for i in 0..25 {
            recorder
                .push_row(&format!("T{i}"), &[])
                .expect("the row records");
        }
        // 25 rows at chunk 10: two full flushes delivered, five buffered.
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one delivery per built batch, in order"
        );

        recorder.flush_and_close().expect("the recorder closes");
        let after_finish = calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            after_finish, 3,
            "the final partial batch is delivered at close"
        );

        // Rows pushed after the outputs are published never reach the
        // callback.
        for i in 25..40 {
            recorder
                .push_row(&format!("T{i}"), &[])
                .expect("the row buffers");
        }
        recorder.flush().expect("the flush is a no-op write");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            after_finish,
            "no callback after finish"
        );
    }
}
