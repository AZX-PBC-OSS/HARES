//! Streaming output writer (Parquet / CSV) and the in-memory destination.
//!
//! One recorder per report. A `Memory` destination retains every flushed
//! batch; a `Parquet` or `Csv` destination streams rows to a partial file
//! created lazily at the first batch and published at close. Rows are
//! buffered up to `chunk_size` and flushed incrementally, keeping peak
//! memory proportional to `chunk_size` rather than simulation length.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{Float64Array, RecordBatch, TimestampMillisecondArray};
use arrow::datatypes::Schema;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::callback::CallbackAttachment;
use crate::output::report::Destination;
use tracing;

use super::OutputSummary;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("I/O error at {path}: {source}")]
    PathIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("row has {got} values but schema has {expected} columns")]
    ColumnMismatch { expected: usize, got: usize },
    #[error("chunk_size must be > 0")]
    InvalidChunkSize,
    #[error(
        "cannot publish {target}: the target exists and the report does not \
         overwrite it; set overwrite: true to replace it"
    )]
    TargetExists { target: PathBuf },
    #[error(
        "cannot publish {target}: this filesystem provides no hard links, and \
         a report that does not overwrite publishes by hard link; set \
         overwrite: true to publish by rename"
    )]
    HardLinkUnavailable { target: PathBuf },
    #[error("callback for report '{report}' failed: {message}")]
    Callback { report: String, message: String },
    /// A rows snapshot did not fit the recorder it restores into: the
    /// values' width does not match the schema, the buffered tail exceeds
    /// the rows, or the batch count disagrees with the flushed shape.
    #[error("report rows snapshot: {0}")]
    Snapshot(String),
}

/// Whether a `hard_link` failure means this filesystem cannot complete a
/// hard link, as opposed to an ordinary path failure: the capability
/// errors are `Unsupported` and `CrossesDevices`. `PermissionDenied`
/// (EPERM, a filesystem that refuses links without declaring them
/// unsupported) and every other kind is a path error.
fn hard_link_unavailable(source: &std::io::Error) -> bool {
    matches!(
        source.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::CrossesDevices
    )
}

/// Inner writer abstraction over Parquet and CSV backends.
enum Backend {
    Parquet(Box<ArrowWriter<BufWriter<File>>>),
    Csv(Box<arrow::csv::Writer<BufWriter<File>>>),
}

/// The per-recorder partial-name counter: two recorders in one process
/// never share a partial file. The pid and the creation nanos in the name
/// do the cross-process part: a pid is only unique among concurrently
/// running processes, so a reused pid must not reconstruct a stale
/// partial name.
static PARTIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A recorder's row stream, as a checkpoint section: every row's
/// timestamp and values in row order, the trailing rows that were still
/// buffered when the checkpoint was taken, the batch count (the
/// batch index a resumed run's callback deliveries continue from) and
/// the chunk size the writing run's recorder flushed at.
///
/// The rows carry the recorder's whole output state: a fresh recorder
/// restored from the snapshot holds the written run's batches bitwise,
/// the batches rebuilt from the rows deterministically (the flushes ran
/// at the chunk boundaries, so the batch boundaries follow from the row
/// stream and the chunk size, which is why the chunk size travels with
/// the rows: a recorder built at another chunk size would rebuild the
/// same rows into batches of different shapes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRowsSnapshot {
    /// Every row's timestamps, in row order (the naive local wall-clock
    /// milliseconds the Time field carries).
    pub timestamps: Vec<i64>,
    /// Every row's values, row-major: `timestamps.len()` rows of one
    /// entry per value column, in the schema's order.
    pub values: Vec<f64>,
    /// The trailing rows that were still buffered (not yet flushed) when
    /// the checkpoint was taken.
    pub buffered_rows: usize,
    /// Batches the recorder had built (and delivered) so far.
    pub batches_built: u64,
    /// The chunk size the writing run's recorder flushed at. The restore
    /// refuses a recorder built at another: its flush boundaries would
    /// re-chunk the rows into batches the writing run never built.
    pub chunk_size: usize,
}

impl StreamingRecorder {
    /// The recorder's row stream as a checkpoint section; the counterpart
    /// of [`Self::restore_rows`].
    #[must_use]
    pub fn rows_snapshot(&self) -> ReportRowsSnapshot {
        let width = self.value_bufs.len();
        let buffered = self.timestamp_buf.len();
        let mut timestamps = Vec::with_capacity(self.total_rows);
        let mut values = Vec::with_capacity(self.total_rows * width);
        // The flushed batches' rows first, in flush order: the batches'
        // schema is the recorder's own.
        for batch in &self.flushed_batches {
            let ts = batch
                .column(0)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("the recorder's batches carry the Time column");
            let columns: Vec<&Float64Array> = batch
                .columns()
                .iter()
                .skip(1)
                .map(|c| {
                    c.as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("the recorder's batches carry f64 value columns")
                })
                .collect();
            for r in 0..batch.num_rows() {
                timestamps.push(ts.value(r));
                for column in &columns {
                    values.push(column.value(r));
                }
            }
        }
        // Then the buffered rows.
        for r in 0..buffered {
            timestamps.push(self.timestamp_buf[r]);
            for buf in &self.value_bufs {
                values.push(buf[r]);
            }
        }
        ReportRowsSnapshot {
            timestamps,
            values,
            buffered_rows: buffered,
            batches_built: self.batches_built,
            chunk_size: self.chunk_size,
        }
    }

    /// Restores the recorder's row stream from its checkpoint section:
    /// the flushed batches rebuild from the rows (the flushes ran at the
    /// chunk boundaries, the section's buffered tail marks the boundary),
    /// and the counters take the snapshot's. The attached callback, if
    /// any, sees none of the restored rows: the run that wrote the
    /// checkpoint delivered them from its own recorder; the restored
    /// counters keep the resumed run's deliveries continuing the
    /// original's index.
    ///
    /// # Errors
    ///
    /// [`OutputError::Snapshot`] when the section does not fit this
    /// recorder: a chunk size off this recorder's, a values width off the
    /// schema's column count, a buffered tail beyond the rows, or a batch
    /// count that disagrees with the flushed shape.
    pub fn restore_rows(&mut self, snapshot: &ReportRowsSnapshot) -> Result<(), OutputError> {
        if snapshot.chunk_size != self.chunk_size {
            return Err(OutputError::Snapshot(format!(
                "the report was written with chunk size {}, this run uses {}",
                snapshot.chunk_size, self.chunk_size
            )));
        }
        let width = self.value_bufs.len();
        let rows = snapshot.timestamps.len();
        if snapshot.values.len() != rows * width {
            return Err(OutputError::Snapshot(format!(
                "the rows carry {} values for {} rows of {} columns",
                snapshot.values.len(),
                rows,
                width
            )));
        }
        if snapshot.buffered_rows > rows {
            return Err(OutputError::Snapshot(format!(
                "the buffered tail ({}) exceeds the rows ({rows})",
                snapshot.buffered_rows
            )));
        }
        let flushed = rows - snapshot.buffered_rows;
        let expected_batches = if flushed == 0 {
            0
        } else {
            u64::try_from(flushed.div_ceil(self.chunk_size)).expect("row count fits u64")
        };
        if snapshot.batches_built != expected_batches {
            return Err(OutputError::Snapshot(format!(
                "the section names {} built batches but its rows rebuild {}",
                snapshot.batches_built, expected_batches
            )));
        }

        let row_at = |r: usize| -> (i64, &[f64]) {
            (
                snapshot.timestamps[r],
                &snapshot.values[r * width..(r + 1) * width],
            )
        };

        // The flushed prefix rebuilds as batches of the chunk size, the
        // final one possibly short when the written run closed its output
        // before the checkpoint.
        self.flushed_batches.clear();
        let mut start = 0;
        while start < flushed {
            let end = (start + self.chunk_size).min(flushed);
            let mut columns: Vec<Arc<dyn arrow::array::Array>> = Vec::with_capacity(width + 1);
            columns.push(Arc::new(TimestampMillisecondArray::from(
                snapshot.timestamps[start..end].to_vec(),
            )));
            for c in 0..width {
                let column: Vec<f64> = (start..end)
                    .map(|r| snapshot.values[r * width + c])
                    .collect();
                columns.push(Arc::new(Float64Array::from(column)));
            }
            let batch = RecordBatch::try_new(Arc::clone(&self.schema), columns)?;
            self.flushed_batches.push(batch);
            start = end;
        }

        // The buffered tail lands back in the buffers.
        self.timestamp_buf.clear();
        for buf in &mut self.value_bufs {
            buf.clear();
        }
        for r in flushed..rows {
            let (timestamp, values) = row_at(r);
            self.timestamp_buf.push(timestamp);
            for (buf, &value) in self.value_bufs.iter_mut().zip(values) {
                buf.push(value);
            }
        }
        self.total_rows = rows;
        self.batches_built = snapshot.batches_built;
        Ok(())
    }
}

/// One report's streaming recorder: buffers rows, flushes batches to the
/// report's destination.
///
/// A `Memory` destination retains every flushed batch (the report's frame).
/// A `Parquet` or `Csv` destination creates its file lazily at the first
/// batch, under a unique partial name in the target's directory, and
/// publishes it at close: by hard link (which never replaces an existing
/// file) with `overwrite: false`, by rename with `overwrite: true`. A run
/// that produces zero rows creates no file. `flush_and_close` publishes;
/// on a failed run the partial file is removed with
/// [`StreamingRecorder::discard_partial`].
pub struct StreamingRecorder {
    schema: Arc<Schema>,
    chunk_size: usize,
    timestamp_buf: Vec<i64>,
    value_bufs: Vec<Vec<f64>>,
    /// The report's destination: the retention rule and, for a file, the
    /// publish target.
    destination: Destination,
    /// The unique partial file the rows stream to, decided at construction
    /// and created at the first batch. `None` for a `Memory` destination.
    partial_path: Option<PathBuf>,
    backend: Option<Backend>,
    total_rows: usize,
    flushed_batches: Vec<RecordBatch>,
    /// Whether the partial file has been published (or discarded): a second
    /// close is a no-op, not a second publish.
    published: bool,
    /// The attached batch callback, if any: every built batch is handed to
    /// it in flush order. `None` when nothing is attached, which costs one
    /// branch per flush and nothing else.
    callback: Option<CallbackAttachment>,
    /// Batches built so far, the `batch_index` every delivery carries.
    batches_built: u64,
    /// Whether the recorder's outputs are published (the final flush and
    /// close has run). After this, no batch reaches the callback: the
    /// buffered rows still record, the callback does not see them.
    closed: bool,
}

impl StreamingRecorder {
    /// Create a new streaming recorder for one report's destination.
    ///
    /// # Arguments
    /// - `schema` -- Arrow schema defining the report's output columns.
    /// - `chunk_size` -- Maximum rows to buffer before flushing. Must be > 0.
    /// - `destination` -- Where the report's batches go.
    ///
    /// # Errors
    ///
    /// [`OutputError::InvalidChunkSize`] for a zero chunk size; for a file
    /// destination, the freeze-time checks run here, before the first step:
    /// [`OutputError::TargetExists`] when the target exists and the report
    /// does not overwrite it, and [`OutputError::PathIo`] when the target's
    /// parent directory is missing. The file itself is not created until
    /// the first batch.
    pub fn new(
        schema: Schema,
        chunk_size: usize,
        destination: Destination,
    ) -> Result<Self, OutputError> {
        if chunk_size == 0 {
            return Err(OutputError::InvalidChunkSize);
        }

        let schema = Arc::new(schema);
        // Number of f64 value columns (everything except the timestamp).
        let n_value_cols = schema.fields().len() - 1;

        let partial_path = match &destination {
            Destination::Memory | Destination::Callback => None,
            Destination::Parquet { path, overwrite } | Destination::Csv { path, overwrite } => {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                match std::fs::metadata(parent) {
                    Ok(meta) if meta.is_dir() => {}
                    _ => {
                        return Err(OutputError::PathIo {
                            path: path.clone(),
                            source: std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "the destination's parent directory does not exist",
                            ),
                        });
                    }
                }
                if path.exists() && !overwrite {
                    return Err(OutputError::TargetExists {
                        target: path.clone(),
                    });
                }
                // The unique partial name: a dot file in the target's own
                // directory, named after it. The pid namespaces concurrent
                // processes, and the creation nanos keep the name unique
                // across a pid's reuse; the process-global counter separates
                // recorders created within one process.
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "output".to_string());
                let n = PARTIAL_COUNTER.fetch_add(1, Ordering::Relaxed);
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                Some(parent.join(format!(
                    ".{file_name}.partial-{}-{}-{n}",
                    std::process::id(),
                    nanos
                )))
            }
        };

        Ok(Self {
            schema,
            chunk_size,
            timestamp_buf: Vec::with_capacity(chunk_size),
            value_bufs: (0..n_value_cols)
                .map(|_| Vec::with_capacity(chunk_size))
                .collect(),
            destination,
            partial_path,
            backend: None,
            total_rows: 0,
            flushed_batches: Vec::new(),
            published: false,
            callback: None,
            batches_built: 0,
            closed: false,
        })
    }

    /// Whether this recorder's destination is [`Destination::Memory`].
    #[must_use]
    pub fn is_memory(&self) -> bool {
        matches!(self.destination, Destination::Memory)
    }

    /// The report's destination.
    #[must_use]
    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    /// Attaches a batch callback: every built batch is handed to it in
    /// flush order, with the context naming the report, the building and
    /// the batch index. A later attach replaces the earlier one.
    pub fn attach_callback(&mut self, attachment: CallbackAttachment) {
        self.callback = Some(attachment);
    }

    /// Creates the partial file's backend writer at the first batch: the
    /// lazy creation the destinations model runs on.
    fn ensure_backend(&mut self) -> Result<(), OutputError> {
        if self.backend.is_some() || self.partial_path.is_none() {
            return Ok(());
        }
        let Some(partial) = &self.partial_path else {
            return Ok(());
        };
        let file = File::create(partial).map_err(|source| OutputError::PathIo {
            path: partial.clone(),
            source,
        })?;
        let buf_writer = BufWriter::new(file);
        self.backend = Some(match self.destination {
            Destination::Parquet { .. } => {
                let props = WriterProperties::builder()
                    .set_compression(Compression::SNAPPY)
                    .build();
                let writer =
                    ArrowWriter::try_new(buf_writer, Arc::clone(&self.schema), Some(props))?;
                Backend::Parquet(Box::new(writer))
            }
            Destination::Csv { .. } => {
                let writer = arrow::csv::WriterBuilder::new()
                    .with_header(true)
                    .build(buf_writer);
                Backend::Csv(Box::new(writer))
            }
            Destination::Memory => unreachable!("a memory destination has no partial file"),
            Destination::Callback => unreachable!("a callback destination has no partial file"),
        });
        tracing::info!(
            path = %partial.display(),
            "created partial output file"
        );
        Ok(())
    }

    /// Flush the current buffer to the destination, draining it into a
    /// `RecordBatch`.
    fn flush_inner(&mut self) -> Result<(), OutputError> {
        if self.timestamp_buf.is_empty() {
            return Ok(());
        }

        let batch = self.build_batch()?;
        let batch_index = self.batches_built;
        self.batches_built += 1;

        self.ensure_backend()?;
        match self.backend.as_mut() {
            Some(Backend::Parquet(w)) => {
                w.write(&batch)?;
            }
            Some(Backend::Csv(writer)) => {
                writer.write(&batch)?;
            }
            None => {}
        }

        // The callback sees every built batch exactly once, in flush
        // order, until the outputs are published. Its error fails the
        // flush, naming the report; the batch itself is already written
        // to the destination file, but a failed delivery ends the flush
        // before the retention push, so the batch is not retained.
        if let Some(attachment) = &self.callback
            && !self.closed
        {
            attachment
                .deliver(&batch, batch_index)
                .map_err(|err| OutputError::Callback {
                    report: attachment.report().to_string(),
                    message: err.message,
                })?;
        }

        if self.is_memory() {
            self.flushed_batches.push(batch);
        }

        Ok(())
    }

    /// Close the current backend writer, closing the file.
    fn close_backend_inner(&mut self) -> Result<(), OutputError> {
        if let Some(backend) = self.backend.take() {
            match backend {
                Backend::Parquet(w) => {
                    w.close()?;
                }
                Backend::Csv(writer) => {
                    use std::io::Write;
                    let mut buf_writer = writer.into_inner();
                    buf_writer.flush()?;
                }
            }
        }
        Ok(())
    }

    /// Publishes the partial file at the report's target path: by hard link
    /// plus partial removal with `overwrite: false` (the no-clobber form of
    /// the rename: the final name appears atomically and complete, and
    /// never replaces a file the caller asked to keep), by rename with
    /// `overwrite: true`. A zero-row run has no partial file and publishes
    /// nothing.
    fn publish(&mut self) -> Result<(), OutputError> {
        if self.published {
            return Ok(());
        }
        self.published = true;
        let Some(partial) = self.partial_path.clone() else {
            return Ok(());
        };
        if self.total_rows == 0 {
            // Zero rows: no file was ever created, nothing to publish.
            return Ok(());
        }
        let Some(target) = self.destination.path().cloned() else {
            return Ok(());
        };
        let overwrite = match self.destination {
            Destination::Parquet { overwrite, .. } | Destination::Csv { overwrite, .. } => {
                overwrite
            }
            Destination::Memory => unreachable!("a memory destination publishes nothing"),
            Destination::Callback => unreachable!("a callback destination publishes nothing"),
        };
        if overwrite {
            std::fs::rename(&partial, &target).map_err(|source| OutputError::PathIo {
                path: target.clone(),
                source,
            })?;
        } else {
            if let Err(source) = std::fs::hard_link(&partial, &target) {
                let _ = std::fs::remove_file(&partial);
                return Err(match source.kind() {
                    std::io::ErrorKind::AlreadyExists => OutputError::TargetExists {
                        target: target.clone(),
                    },
                    _ if hard_link_unavailable(&source) => OutputError::HardLinkUnavailable {
                        target: target.clone(),
                    },
                    _ => OutputError::PathIo {
                        path: target.clone(),
                        source,
                    },
                });
            }
            std::fs::remove_file(&partial).map_err(|source| OutputError::PathIo {
                path: partial.clone(),
                source,
            })?;
        }
        Ok(())
    }

    /// Removes the partial file: every failed run calls this for each of
    /// its recorders, so a failed run leaves no partial file.
    pub fn discard_partial(&mut self) {
        self.published = true;
        if let Some(partial) = &self.partial_path {
            let _ = std::fs::remove_file(partial);
        }
    }

    /// Append a single row. The `timestamp` is the row's naive local
    /// wall-clock milliseconds (the `Timestamp(Millisecond, None)` Time
    /// field's value; the schema metadata's `hares:utc_offset_s` recovers
    /// the instant); `values` contains the report's numeric columns in the
    /// report's schema order (excluding the timestamp column).
    ///
    /// Automatically flushes when the buffer reaches `chunk_size`.
    pub fn push_row(&mut self, timestamp: i64, values: &[f64]) -> Result<(), OutputError> {
        let expected = self.value_bufs.len();
        if values.len() != expected {
            return Err(OutputError::ColumnMismatch {
                expected,
                got: values.len(),
            });
        }

        self.timestamp_buf.push(timestamp);
        for (buf, &val) in self.value_bufs.iter_mut().zip(values.iter()) {
            buf.push(val);
        }
        self.total_rows += 1;

        if self.timestamp_buf.len() >= self.chunk_size {
            self.flush_inner()?;
        }
        Ok(())
    }

    /// Flush the current buffer to the destination. No-op if buffer is empty.
    pub fn flush(&mut self) -> Result<(), OutputError> {
        self.flush_inner()
    }

    /// Flush remaining rows, close the backend file writer and publish the
    /// report's file.
    ///
    /// After this call, `push_row()` will still buffer but no further data
    /// will be written. `flushed_batches()` remains available. A second
    /// call is a no-op.
    pub fn flush_and_close(&mut self) -> Result<(), OutputError> {
        self.flush_inner()?;
        self.close_backend_inner()?;
        self.publish()?;
        // The outputs are published: the final partial batch was delivered
        // by the flush above, and nothing reaches the callback after this.
        self.closed = true;
        Ok(())
    }

    /// Returns the batches the report retained: every flushed batch for a
    /// `Memory` destination, empty for a file destination.
    #[must_use]
    pub fn flushed_batches(&self) -> &[RecordBatch] {
        &self.flushed_batches
    }

    /// Flush remaining rows, publish the report's file, and return summary
    /// statistics for it.
    pub fn finish(mut self) -> Result<OutputSummary, OutputError> {
        self.flush_and_close()?;

        let byte_size = self
            .destination
            .path()
            .and_then(|path| std::fs::metadata(path).ok())
            .map_or(0, |meta| meta.len());

        Ok(OutputSummary {
            row_count: self.total_rows,
            byte_size,
            path: self.destination.path().cloned().unwrap_or_default(),
        })
    }

    /// The Arrow schema this recorder was constructed with, including any
    /// field metadata (for example, telemetry-unit provenance) attached
    /// before construction.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        self.schema.as_ref()
    }

    /// Number of rows currently buffered (not yet flushed).
    #[must_use]
    pub fn buffered_rows(&self) -> usize {
        self.timestamp_buf.len()
    }

    /// Total rows written (flushed + buffered).
    #[must_use]
    pub fn total_rows(&self) -> usize {
        self.total_rows
    }

    /// Batches the recorder has built (and delivered) so far, the final
    /// partial batch included.
    #[must_use]
    pub fn batches_built(&self) -> u64 {
        self.batches_built
    }

    /// The chunk size the recorder flushes at: what a rows snapshot
    /// carries, and what a restore refuses a mismatch with.
    #[must_use]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Build a `RecordBatch` from the current buffer, draining buffers to
    /// avoid cloning. After this call, all buffers are empty.
    fn build_batch(&mut self) -> Result<RecordBatch, OutputError> {
        let mut columns: Vec<Arc<dyn arrow::array::Array>> =
            Vec::with_capacity(self.schema.fields().len());

        // Drain timestamp buffer into a TimestampMillisecondArray (avoids
        // clone); the Time field's type pins the array type.
        let timestamps = std::mem::take(&mut self.timestamp_buf);
        columns.push(Arc::new(TimestampMillisecondArray::from(timestamps)));

        // Drain each f64 buffer into a Float64Array.
        for buf in &mut self.value_bufs {
            let data = std::mem::take(buf);
            columns.push(Arc::new(Float64Array::from(data)));
        }

        let batch = RecordBatch::try_new(Arc::clone(&self.schema), columns)?;
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use arrow::array::Float64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    use super::*;
    use crate::output::callback::{BatchCallback, BatchContext, CallbackError, TIMESTEP_REPORT};

    fn test_schema() -> Schema {
        Schema::new(vec![
            Field::new(
                "Time",
                DataType::Timestamp(arrow::datatypes::TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("Total Electric Power (kW)", DataType::Float64, true),
            Field::new("Total Gas Power (therms/hour)", DataType::Float64, true),
        ])
    }

    fn recorder_at(schema: Schema, path: &Path, chunk: usize) -> StreamingRecorder {
        StreamingRecorder::new(
            schema,
            chunk,
            Destination::Parquet {
                path: path.to_path_buf(),
                overwrite: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn parquet_round_trip_preserves_values() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let mut recorder = recorder_at(schema, &path, 100);

        let test_values: Vec<(f64, f64)> = vec![
            (1.5, 0.3),
            (2.7, 0.0),
            (f64::MAX, f64::MIN),
            (0.0, 0.0),
            (std::f64::consts::PI, std::f64::consts::E),
        ];

        for (i, (e, g)) in test_values.iter().enumerate() {
            recorder
                .push_row(i64::try_from(i).expect("row index") * 60_000, &[*e, *g])
                .unwrap();
        }

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 5);
        assert!(summary.byte_size > 0);

        // Read back and verify bit-exact values.
        let file = File::open(&path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();

        let mut all_e = Vec::new();
        let mut all_g = Vec::new();
        for batch in reader {
            let batch = batch.unwrap();
            let e_col = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let g_col = batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                all_e.push(e_col.value(i));
                all_g.push(g_col.value(i));
            }
        }

        for (i, (expected_e, expected_g)) in test_values.iter().enumerate() {
            assert_eq!(
                all_e[i].to_bits(),
                expected_e.to_bits(),
                "electric power mismatch at row {i}"
            );
            assert_eq!(
                all_g[i].to_bits(),
                expected_g.to_bits(),
                "gas power mismatch at row {i}"
            );
        }
    }

    #[test]
    fn csv_round_trip_produces_valid_output() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.csv");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            Destination::Csv {
                path: path.clone(),
                overwrite: false,
            },
        )
        .unwrap();

        recorder.push_row(0, &[1.0, 2.0]).unwrap();
        recorder.push_row(60_000, &[3.0, 4.0]).unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 2);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("Total Electric Power (kW)"));
        assert!(contents.contains("1.0"));
    }

    #[test]
    fn flush_count_matches_chunk_size() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let chunk_size = 10_000;
        let total_rows = 100_000;
        let mut recorder = recorder_at(schema, &path, chunk_size);

        for i in 0..total_rows {
            recorder
                .push_row(i64::try_from(i).expect("row index"), &[i as f64, 0.0])
                .unwrap();
        }

        // After 100k rows with chunk_size=10k, buffer should be empty
        // (last push_row triggered the 10th flush).
        assert_eq!(recorder.buffered_rows(), 0);
        assert_eq!(recorder.total_rows(), total_rows);
        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, total_rows);
    }

    /// `zero_row_run_creates_no_file`: a recorder that closed with zero
    /// rows published no file at all (the declared replacement for
    /// `finish_on_empty_recorder_produces_valid_file`).
    #[test]
    fn zero_row_run_creates_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let recorder = recorder_at(schema, &path, 100);

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 0);
        assert!(
            !path.exists(),
            "a zero-row run creates no file, not even an empty one"
        );
        let files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            files.is_empty(),
            "no partial file survives a zero-row run either"
        );
    }

    #[test]
    fn column_mismatch_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let mut recorder = recorder_at(schema, &path, 100);

        let result = recorder.push_row(0, &[1.0]); // 1 value, expects 2
        assert!(result.is_err());
    }

    #[test]
    fn partial_chunk_flushed_on_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let mut recorder = recorder_at(schema, &path, 1000);

        for i in 0..5 {
            recorder.push_row(i64::from(i), &[i as f64, 0.0]).unwrap();
        }
        assert_eq!(recorder.buffered_rows(), 5);
        assert!(
            !path.exists(),
            "the file is created lazily at the first batch, not at construction"
        );

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 5);

        // Verify Parquet has 5 rows.
        let file = File::open(&path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let total: usize = reader.filter_map(Result::ok).map(|b| b.num_rows()).sum();
        assert_eq!(total, 5);
    }

    /// Structural: a recorder with no callback attached constructs no
    /// adapter, so a run that never attaches one pays a branch per flush
    /// and nothing else.
    #[test]
    fn no_callback_report_constructs_no_adapter() {
        let schema = test_schema();
        let recorder = StreamingRecorder::new(schema, 10, Destination::Memory).unwrap();
        assert!(recorder.callback.is_none());
    }

    #[test]
    fn zero_chunk_size_rejected() {
        let result = StreamingRecorder::new(test_schema(), 0, Destination::Memory);
        assert!(matches!(result, Err(OutputError::InvalidChunkSize)));
    }

    #[test]
    fn memory_destination_retains_batches() {
        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(schema, 10, Destination::Memory).unwrap();

        for i in 0..25 {
            recorder
                .push_row(i64::from(i), &[i as f64, i as f64 * 2.0])
                .unwrap();
        }
        // 25 rows with chunk_size=10 → 2 flushes triggered by push_row.
        assert_eq!(recorder.flushed_batches().len(), 2);

        recorder.finish().unwrap();
    }

    #[test]
    fn file_destination_does_not_retain_batches() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let chunk_size = 50;
        let total_rows = 10 * chunk_size; // exercise 10x chunk_size
        let mut recorder = recorder_at(test_schema(), &path, chunk_size);

        for i in 0..total_rows {
            recorder
                .push_row(i64::try_from(i).expect("row index"), &[i as f64, 0.0])
                .unwrap();
        }
        // A file destination streams: flushed batches are written and
        // dropped, never retained.
        assert!(recorder.flushed_batches().is_empty());
        assert_eq!(recorder.total_rows(), total_rows);

        recorder.finish().unwrap();
    }

    #[test]
    fn existing_target_without_overwrite_fails_at_construction() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");
        std::fs::write(&path, b"existing").unwrap();

        let err = StreamingRecorder::new(
            test_schema(),
            100,
            Destination::Parquet {
                path: path.clone(),
                overwrite: false,
            },
        );
        let err = err
            .err()
            .expect("an existing target without overwrite fails at freeze");
        assert!(
            err.to_string().contains("out.parquet"),
            "the error names the path, got: {err}"
        );
    }

    #[test]
    fn overwrite_true_replaces_target() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");
        std::fs::write(&path, b"existing").unwrap();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            Destination::Parquet {
                path: path.clone(),
                overwrite: true,
            },
        )
        .unwrap();
        recorder.push_row(0, &[1.0, 2.0]).unwrap();
        recorder.finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_ne!(&bytes[..3], b"exi", "the target was replaced");
        // No partial file survives.
        let files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(files.len(), 1, "exactly the published target remains");
    }

    #[test]
    fn missing_parent_dir_is_io_error_naming_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing_dir").join("out.parquet");

        let err = StreamingRecorder::new(
            test_schema(),
            100,
            Destination::Parquet {
                path: path.clone(),
                overwrite: false,
            },
        );
        let err = err
            .err()
            .expect("a missing parent directory fails at freeze");
        assert!(
            err.to_string().contains("out.parquet"),
            "the error names the path, got: {err}"
        );
    }

    #[test]
    fn publish_leaves_no_partial_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.csv");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            2,
            Destination::Csv {
                path: path.clone(),
                overwrite: false,
            },
        )
        .unwrap();
        recorder.push_row(0, &[1.0, 2.0]).unwrap();
        recorder.push_row(1, &[3.0, 4.0]).unwrap();
        // The chunk flushed: the partial file exists mid-run.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
        let partial_is_hidden = std::fs::read_dir(tmp.path())
            .unwrap()
            .all(|e| e.unwrap().file_name().to_string_lossy().starts_with('.'));
        assert!(partial_is_hidden, "the mid-run file is the hidden partial");

        recorder.finish().unwrap();
        assert!(path.exists());
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "the partial is gone; exactly the published target remains"
        );
    }

    #[test]
    fn discard_partial_removes_the_streaming_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let mut recorder = recorder_at(schema, &path, 1);
        recorder.push_row(0, &[1.0, 2.0]).unwrap();
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);

        recorder.discard_partial();
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            0,
            "a failed run leaves no partial file"
        );
    }

    #[test]
    fn second_close_is_a_noop_not_a_second_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.parquet");

        let schema = test_schema();
        let mut recorder = recorder_at(schema, &path, 100);
        recorder.push_row(0, &[1.0, 2.0]).unwrap();
        recorder.flush_and_close().unwrap();
        recorder.flush_and_close().unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn link_capability_errors_classify_as_hard_link_unavailable() {
        // The capability refusals: a filesystem that reports the operation
        // unsupported, and one that refuses the cross-device link.
        for source in [
            std::io::Error::from(std::io::ErrorKind::Unsupported),
            std::io::Error::from(std::io::ErrorKind::CrossesDevices),
        ] {
            assert!(
                super::hard_link_unavailable(&source),
                "{source} must classify as a link-capability failure"
            );
        }
    }

    #[test]
    fn eperm_and_ordinary_errors_classify_as_path_errors() {
        // EPERM, the errno a filesystem that merely refuses links raises:
        // a raw OS error, not an ErrorKind construction.
        let eperm = std::io::Error::from_raw_os_error(1);
        assert_eq!(eperm.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            !super::hard_link_unavailable(&eperm),
            "EPERM is a path error, not a link-capability failure"
        );
        for source in [
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            std::io::Error::from(std::io::ErrorKind::NotFound),
            std::io::Error::other("something else"),
        ] {
            assert!(
                !super::hard_link_unavailable(&source),
                "{source} must classify as a path error"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_link_refusal_from_the_os_is_a_path_error() {
        // The integration probe: the one link refusal a test filesystem
        // can raise portably is link(2) on a directory, EPERM on Linux.
        // The real error must land in the path-error class, the class the
        // publish reports as PathIo.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        let err = std::fs::hard_link(&dir, tmp.path().join("target"))
            .expect_err("directories do not hard link");
        assert!(
            !super::hard_link_unavailable(&err),
            "the OS refused the link with {err} ({:?}); it must classify as a path error",
            err.kind()
        );
    }

    #[test]
    fn two_recorders_never_share_a_partial_name() {
        let tmp = tempfile::tempdir().unwrap();
        let schema = test_schema();
        let first = StreamingRecorder::new(
            schema.clone(),
            10,
            Destination::Csv {
                path: tmp.path().join("a.csv"),
                overwrite: false,
            },
        )
        .unwrap();
        let second = StreamingRecorder::new(
            schema,
            10,
            Destination::Csv {
                path: tmp.path().join("b.csv"),
                overwrite: false,
            },
        )
        .unwrap();
        assert_ne!(
            first.partial_path, second.partial_path,
            "two recorder creations in one process produce distinct partial names"
        );
    }

    fn memory_recorder(schema: &Schema, chunk: usize) -> StreamingRecorder {
        StreamingRecorder::new(schema.clone(), chunk, Destination::Memory).unwrap()
    }

    fn rows_of(batches: &[RecordBatch]) -> Vec<(i64, Vec<u64>)> {
        let mut out = Vec::new();
        for batch in batches {
            let ts = batch
                .column(0)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("timestamp column");
            let values: Vec<&Float64Array> = batch
                .columns()
                .iter()
                .skip(1)
                .map(|c| {
                    c.as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column")
                })
                .collect();
            for r in 0..batch.num_rows() {
                out.push((
                    ts.value(r),
                    values.iter().map(|c| c.value(r).to_bits()).collect(),
                ));
            }
        }
        out
    }

    /// A recorder's rows snapshot restores a fresh recorder to the same
    /// state: the flushed batches bitwise (values, timestamps, order), the
    /// buffered tail, and the counters.
    #[test]
    fn recorder_rows_snapshot_restores_flushed_batches_and_buffers_bitwise() {
        let schema = test_schema();
        let mut original = memory_recorder(&schema, 4);
        for i in 0..9_i64 {
            original
                .push_row(1_000 * i, &[i as f64 * 1.5, i as f64 * -0.25])
                .unwrap();
        }
        // 9 rows at chunk 4: two full flushes, one buffered row.
        assert_eq!(original.buffered_rows(), 1);
        assert_eq!(original.batches_built(), 2);

        let snapshot = original.rows_snapshot();
        assert_eq!(
            snapshot.chunk_size, 4,
            "the section carries the writing run's chunk size"
        );
        let json = serde_json::to_string(&snapshot).expect("serialize the snapshot");
        let decoded: ReportRowsSnapshot = serde_json::from_str(&json).expect("decode");

        let mut restored = memory_recorder(&schema, 4);
        restored.restore_rows(&decoded).expect("restore");
        assert_eq!(restored.total_rows(), 9);
        assert_eq!(restored.buffered_rows(), 1);
        assert_eq!(restored.batches_built(), 2);
        let before = rows_of(original.flushed_batches());
        let after = rows_of(restored.flushed_batches());
        assert_eq!(
            before, after,
            "the flushed batches are bitwise the original's"
        );

        // And the restored recorder keeps recording exactly as the
        // original: the next flush builds the third batch.
        original.push_row(9_000, &[13.5, -2.25]).unwrap();
        restored.push_row(9_000, &[13.5, -2.25]).unwrap();
        assert_eq!(
            original.flushed_batches().len(),
            restored.flushed_batches().len()
        );
        let a = rows_of(original.flushed_batches());
        let b = rows_of(restored.flushed_batches());
        assert_eq!(a, b, "the post-restore batches continue bitwise");
    }

    /// A snapshot written at another chunk size is refused by name: the
    /// resumed batches' shapes would differ from the writing run's, and
    /// the refusal must name the chunk sizes, not blame the rows.
    #[test]
    fn recorder_rows_snapshot_refuses_a_foreign_chunk_size_by_name() {
        let schema = test_schema();
        let mut original = memory_recorder(&schema, 4);
        for i in 0..9_i64 {
            original
                .push_row(1_000 * i, &[i as f64 * 1.5, i as f64 * -0.25])
                .unwrap();
        }
        let snapshot = original.rows_snapshot();

        let mut other = memory_recorder(&schema, 2);
        let err = other
            .restore_rows(&snapshot)
            .expect_err("a foreign chunk size refuses the restore")
            .to_string();
        assert!(
            err.contains("chunk size 4") && err.contains("this run uses 2"),
            "the refusal names both chunk sizes: {err}"
        );
    }

    /// The batch-count check alone lets a chunk-size drift through when
    /// the row count rebuilds the same batch count both ways (100 flushed
    /// rows: one batch at chunk 100, one at chunk 150): the carried chunk
    /// size is what refuses it, so a resumed run's batches cannot be
    /// re-chunked out of the writing run's shapes.
    #[test]
    fn recorder_rows_snapshot_refuses_a_chunk_drift_the_batch_count_misses() {
        let schema = test_schema();
        let mut original = memory_recorder(&schema, 100);
        for i in 0..100_i64 {
            original.push_row(1_000 * i, &[i as f64, 0.0]).unwrap();
        }
        assert_eq!(original.batches_built(), 1);
        let snapshot = original.rows_snapshot();

        let mut other = memory_recorder(&schema, 150);
        let err = other
            .restore_rows(&snapshot)
            .expect_err("the chunk drift must not re-chunk the batches")
            .to_string();
        assert!(
            err.contains("chunk size 100") && err.contains("this run uses 150"),
            "the refusal names both chunk sizes: {err}"
        );
    }

    /// A restored recorder's next callback delivery carries the batch
    /// index the interrupted run reached, so a resumed run's callback
    /// stream continues the original's.
    #[test]
    fn restored_recorder_continues_the_callback_index() {
        use std::sync::atomic::AtomicU64;

        struct Counting(std::sync::Arc<AtomicU64>);
        impl BatchCallback for Counting {
            fn on_batch(
                &mut self,
                ctx: &BatchContext<'_>,
                _batch: &RecordBatch,
            ) -> Result<(), CallbackError> {
                self.0.store(ctx.batch_index, Ordering::SeqCst);
                Ok(())
            }
        }

        let schema = test_schema();
        let mut original = memory_recorder(&schema, 4);
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        original.attach_callback(CallbackAttachment::new(
            TIMESTEP_REPORT,
            1,
            Box::new(Counting(std::sync::Arc::clone(&seen))),
        ));
        for i in 0..9_i64 {
            original.push_row(1_000 * i, &[i as f64, 0.0]).unwrap();
        }
        // Two deliveries: indices 0 and 1.
        assert_eq!(seen.load(Ordering::SeqCst), 1);

        let snapshot = original.rows_snapshot();
        let mut restored = memory_recorder(&schema, 4);
        restored.restore_rows(&snapshot).expect("restore");
        restored.attach_callback(CallbackAttachment::new(
            TIMESTEP_REPORT,
            1,
            Box::new(Counting(std::sync::Arc::clone(&seen))),
        ));
        // The next flush delivers with the continued index 2.
        restored.push_row(9_000, &[9.0, 0.0]).unwrap();
        restored.flush().unwrap();
        assert_eq!(
            seen.load(Ordering::SeqCst),
            2,
            "the restored recorder's deliveries continue the interrupted run's index"
        );
    }
}
