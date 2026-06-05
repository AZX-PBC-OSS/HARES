//! Streaming output writer (Parquet / CSV).
//!
//! Rows are buffered up to `chunk_size` and flushed incrementally, keeping
//! peak memory proportional to `chunk_size` rather than simulation length.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Float64Array, RecordBatch, StringArray};
use arrow::datatypes::Schema;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use thiserror::Error;
use tracing;

use super::OutputSummary;
use crate::config::{OutputFormat, RotationPolicy};

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("row has {got} values but schema has {expected} columns")]
    ColumnMismatch { expected: usize, got: usize },
    #[error("chunk_size must be > 0")]
    InvalidChunkSize,
}

/// Inner writer abstraction over Parquet and CSV backends.
enum Backend {
    Parquet(Box<ArrowWriter<BufWriter<File>>>),
    Csv(Box<arrow::csv::Writer<BufWriter<File>>>),
}

/// Streaming recorder that buffers rows and flushes to disk in chunks.
///
/// When `retain_batches` is `false`, peak memory is bounded at
/// `O(chunk_size)`. When `true`, all flushed batches are retained in
/// memory and peak memory is `O(total_rows)`.
///
/// When `rotation_policy` is not [`RotationPolicy::None`], output is
/// split across multiple files at time-interval boundaries. Each file
/// gets a timestamp suffix derived from the first row written to it.
pub struct StreamingRecorder {
    schema: Arc<Schema>,
    chunk_size: usize,
    timestamp_buf: Vec<String>,
    value_bufs: Vec<Vec<f64>>,
    backend: Option<Backend>,
    total_rows: usize,
    output_path: PathBuf,
    flushed_batches: Vec<RecordBatch>,
    retain_batches: bool,
    /// Number of batches retained since construction
    /// (available when feature `observe` is enabled).
    #[cfg(feature = "observe")]
    retained_batch_count: usize,
    /// File rotation policy. `None` means a single output file.
    rotation_policy: RotationPolicy,
    /// Output format, retained so rotated files are created with the
    /// same backend type.
    format: OutputFormat,
    /// Prefix of the current rotation boundary (e.g. `2024-01-01` for daily).
    /// Used to detect boundary crossings without parsing timestamps on every row.
    current_boundary: String,
    /// All output file paths written so far (first file + rotated successors).
    rotated_paths: Vec<PathBuf>,
    /// Number of files created (1 for single file, N for N rotations).
    #[cfg(feature = "observe")]
    rotated_file_count: usize,
}

impl StreamingRecorder {
    /// Create a new streaming recorder.
    ///
    /// # Arguments
    /// - `schema` -- Arrow schema defining all output columns.
    /// - `chunk_size` -- Maximum rows to buffer before flushing. Must be > 0.
    /// - `format` -- Output format (CSV or Parquet).
    /// - `output_path` -- Destination file path. When `rotation_policy` is not
    ///   [`RotationPolicy::None`], rotated files derive their paths from this
    ///   base path by inserting a timestamp suffix before the extension.
    /// - `retain_batches` -- When `false`, flushed batches are not retained
    ///   in memory after writing; `flushed_batches()` returns an empty slice.
    ///   Set `true` only when post-hoc batch access is required (e.g. Python
    ///   bindings, metrics computation from in-memory batches).
    /// - `rotation_policy` -- File rotation interval.
    pub fn new(
        schema: Schema,
        chunk_size: usize,
        format: OutputFormat,
        output_path: &Path,
        retain_batches: bool,
        rotation_policy: RotationPolicy,
    ) -> Result<Self, OutputError> {
        if chunk_size == 0 {
            return Err(OutputError::InvalidChunkSize);
        }

        let schema = Arc::new(schema);
        // Number of f64 value columns (everything except the timestamp).
        let n_value_cols = schema.fields().len() - 1;

        // When rotation is enabled, defer file creation until the first row
        // is pushed (we need a timestamp to derive the boundary suffix).
        let (backend, rotated_paths, current_boundary) = if rotation_policy == RotationPolicy::None
        {
            let file = File::create(output_path)?;
            let buf_writer = BufWriter::new(file);
            let backend = match format {
                OutputFormat::Parquet => {
                    let props = WriterProperties::builder()
                        .set_compression(Compression::SNAPPY)
                        .build();
                    let writer =
                        ArrowWriter::try_new(buf_writer, Arc::clone(&schema), Some(props))?;
                    Backend::Parquet(Box::new(writer))
                }
                OutputFormat::Csv => {
                    let writer = arrow::csv::WriterBuilder::new()
                        .with_header(true)
                        .build(buf_writer);
                    Backend::Csv(Box::new(writer))
                }
            };
            (
                Some(backend),
                vec![output_path.to_path_buf()],
                String::new(),
            )
        } else {
            (None, Vec::new(), String::new())
        };

        if retain_batches {
            // When enabled, every flush accumulates an in-memory RecordBatch
            // that is never freed until the recorder is dropped, unbounded
            // memory growth for long simulations.
            tracing::warn!(
                "StreamingRecorder: retain_batches=true — flushed batches \
                 accumulate in memory (unbounded growth risk for long simulations)"
            );
        }

        Ok(Self {
            schema,
            chunk_size,
            timestamp_buf: Vec::with_capacity(chunk_size),
            value_bufs: (0..n_value_cols)
                .map(|_| Vec::with_capacity(chunk_size))
                .collect(),
            backend,
            total_rows: 0,
            output_path: output_path.to_path_buf(),
            flushed_batches: Vec::new(),
            retain_batches,
            #[cfg(feature = "observe")]
            retained_batch_count: 0,
            rotation_policy,
            format,
            current_boundary,
            rotated_paths,
            #[cfg(feature = "observe")]
            rotated_file_count: if rotation_policy == RotationPolicy::None {
                1
            } else {
                0
            },
        })
    }

    /// Check whether the given timestamp has crossed a rotation boundary
    /// and create a new output file if so.
    fn check_rotation(&mut self, timestamp: &str) -> Result<(), OutputError> {
        if self.rotation_policy == RotationPolicy::None {
            return Ok(());
        }

        let boundary = Self::extract_boundary(timestamp, self.rotation_policy);

        // First row: open the initial file.
        if self.backend.is_none() {
            let path = Self::derive_rotated_path(&self.output_path, &boundary);
            self.backend = Some(Self::create_backend_from_path(
                &path,
                self.schema.clone(),
                self.format,
            )?);
            self.current_boundary = boundary;
            self.rotated_paths.push(path.clone());
            #[cfg(feature = "observe")]
            {
                self.rotated_file_count = 1;
            }
            tracing::info!(
                path = %path.display(),
                "created output file"
            );
            return Ok(());
        }

        // Boundary unchanged — continue writing to current file.
        if boundary == self.current_boundary {
            return Ok(());
        }

        // Boundary crossed: flush and close current file, open new one.
        self.flush_inner()?;
        self.close_backend_inner()?;

        let path = Self::derive_rotated_path(&self.output_path, &boundary);
        self.backend = Some(Self::create_backend_from_path(
            &path,
            self.schema.clone(),
            self.format,
        )?);
        self.current_boundary = boundary;
        self.rotated_paths.push(path.clone());
        #[cfg(feature = "observe")]
        {
            self.rotated_file_count += 1;
        }
        tracing::info!(
            path = %path.display(),
            "rotating output file"
        );
        Ok(())
    }

    /// Extract the rotation boundary prefix from an RFC 3339 timestamp.
    /// For example, `"2024-01-01T12:30:00Z"` with [`RotationPolicy::Daily`]
    /// returns `"2024-01-01"`.
    fn extract_boundary(timestamp: &str, policy: RotationPolicy) -> String {
        let len = match policy {
            RotationPolicy::None => unreachable!("extract_boundary not called for None"),
            RotationPolicy::Hourly => 13, // "2024-01-01T12"
            RotationPolicy::Daily => 10,  // "2024-01-01"
            RotationPolicy::Monthly => 7, // "2024-01"
            RotationPolicy::Yearly => 4,  // "2024"
        };
        if timestamp.len() < len {
            timestamp.to_string()
        } else {
            timestamp[..len].to_string()
        }
    }

    /// Derive a rotated file path from the base output path and a boundary
    /// suffix.
    ///
    /// `base_output_path` is, e.g., `dwelling_42.parquet`.
    /// The result is, e.g., `dwelling_42_2024-01-01.parquet` for daily rotation.
    fn derive_rotated_path(base_path: &Path, boundary: &str) -> PathBuf {
        let stem = base_path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|| std::borrow::Cow::Borrowed("output"));
        let ext = base_path
            .extension()
            .map(|e| e.to_string_lossy())
            .unwrap_or_else(|| std::borrow::Cow::Borrowed("parquet"));
        let parent = base_path.parent().unwrap_or_else(|| Path::new("."));

        let rotated_stem = format!("{}_{}", stem, boundary);
        parent.join(rotated_stem).with_extension(ext.as_ref())
    }

    /// Create a backend writer for a given file path, using the specified format.
    fn create_backend_from_path(
        path: &Path,
        schema: Arc<Schema>,
        format: OutputFormat,
    ) -> Result<Backend, OutputError> {
        let file = File::create(path)?;
        let buf_writer = BufWriter::new(file);
        match format {
            OutputFormat::Csv => {
                let writer = arrow::csv::WriterBuilder::new()
                    .with_header(true)
                    .build(buf_writer);
                Ok(Backend::Csv(Box::new(writer)))
            }
            OutputFormat::Parquet => {
                let props = WriterProperties::builder()
                    .set_compression(Compression::SNAPPY)
                    .build();
                let writer = ArrowWriter::try_new(buf_writer, schema, Some(props))?;
                Ok(Backend::Parquet(Box::new(writer)))
            }
        }
    }

    /// Flush the current buffer to disk, draining it into a `RecordBatch`.
    fn flush_inner(&mut self) -> Result<(), OutputError> {
        if self.timestamp_buf.is_empty() {
            return Ok(());
        }

        let batch = self.build_batch()?;

        match self.backend.as_mut() {
            Some(Backend::Parquet(w)) => {
                w.write(&batch)?;
            }
            Some(Backend::Csv(writer)) => {
                writer.write(&batch)?;
            }
            None => {}
        }

        if self.retain_batches {
            self.flushed_batches.push(batch);
            #[cfg(feature = "observe")]
            {
                self.retained_batch_count += 1;
            }
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

    /// Append a single row. The first element of `timestamp` is the time
    /// string; `values` contains all numeric columns in schema order
    /// (excluding the timestamp column).
    ///
    /// Automatically flushes when the buffer reaches `chunk_size`.
    ///
    /// When `rotation_policy` is not [`RotationPolicy::None`], this method
    /// checks the timestamp against the current rotation boundary and
    /// creates a new file if the boundary has been crossed.
    pub fn push_row(&mut self, timestamp: &str, values: &[f64]) -> Result<(), OutputError> {
        // Check rotation before buffering so the row lands in the correct file.
        self.check_rotation(timestamp)?;

        let expected = self.value_bufs.len();
        if values.len() != expected {
            return Err(OutputError::ColumnMismatch {
                expected,
                got: values.len(),
            });
        }

        self.timestamp_buf.push(timestamp.to_string());
        for (buf, &val) in self.value_bufs.iter_mut().zip(values.iter()) {
            buf.push(val);
        }
        self.total_rows += 1;

        if self.timestamp_buf.len() >= self.chunk_size {
            self.flush()?;
        }
        Ok(())
    }

    /// Flush the current buffer to disk. No-op if buffer is empty.
    pub fn flush(&mut self) -> Result<(), OutputError> {
        self.flush_inner()
    }

    /// Flush remaining rows and close the backend file writer.
    ///
    /// After this call, `push_row()` will still buffer but no further data
    /// will be written to disk. `flushed_batches()` remains available.
    ///
    /// When rotation is enabled, logs the total number of files produced.
    pub fn flush_and_close(&mut self) -> Result<(), OutputError> {
        self.flush_inner()?;
        self.close_backend_inner()?;

        #[cfg(feature = "observe")]
        {
            if self.rotated_file_count > 1 {
                let file_count = self.rotated_file_count;
                tracing::info!(file_count, "output rotation produced {file_count} files");
            }
        }

        Ok(())
    }

    /// Returns all batches that have been flushed so far.
    ///
    /// When `retain_batches` is `false` (the default), this always
    /// returns an empty slice — flushed batches are written to disk
    /// and dropped. When `true`, returns all batches flushed since
    /// construction.
    #[must_use]
    pub fn flushed_batches(&self) -> &[RecordBatch] {
        &self.flushed_batches
    }

    /// Number of batches retained since construction.
    ///
    /// Available when feature `observe` is enabled; allows an external
    /// observer to verify that no batches are retained when
    /// `retain_batches` is `false`.
    #[cfg(feature = "observe")]
    #[must_use]
    pub fn retained_batch_count(&self) -> usize {
        self.retained_batch_count
    }

    /// Flush remaining rows, close all files, and return summary statistics
    /// covering every rotated file.
    pub fn finish(mut self) -> Result<OutputSummary, OutputError> {
        self.flush_and_close()?;

        // Aggregate byte size across all files (rotated + primary).
        let mut byte_size: u64 = 0;
        for path in &self.rotated_paths {
            if let Ok(meta) = std::fs::metadata(path) {
                byte_size += meta.len();
            }
        }
        if self.rotated_paths.is_empty() {
            // No rows written under rotation — no files were created.
            byte_size = 0;
        }

        let primary_path = self
            .rotated_paths
            .first()
            .cloned()
            .unwrap_or_else(|| self.output_path.clone());

        Ok(OutputSummary {
            row_count: self.total_rows,
            byte_size,
            path: primary_path,
            rotated_paths: self.rotated_paths,
        })
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

    /// Build a `RecordBatch` from the current buffer, draining buffers to
    /// avoid cloning. After this call, all buffers are empty.
    fn build_batch(&mut self) -> Result<RecordBatch, OutputError> {
        let mut columns: Vec<Arc<dyn arrow::array::Array>> =
            Vec::with_capacity(self.schema.fields().len());

        // Drain timestamp buffer into a StringArray (avoids clone).
        let timestamps = std::mem::take(&mut self.timestamp_buf);
        columns.push(Arc::new(StringArray::from(timestamps)));

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
    use arrow::array::{Float64Array, RecordBatchReader};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use tempfile::NamedTempFile;

    use super::*;
    use tempfile::tempdir;

    fn test_schema() -> Schema {
        Schema::new(vec![
            Field::new("Time", DataType::Utf8, false),
            Field::new("Total Electric Power (kW)", DataType::Float64, true),
            Field::new("Total Gas Power (therms/hour)", DataType::Float64, true),
        ])
    }

    #[test]
    fn parquet_round_trip_preserves_values() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        let test_values: Vec<(f64, f64)> = vec![
            (1.5, 0.3),
            (2.7, 0.0),
            (f64::MAX, f64::MIN),
            (0.0, 0.0),
            (std::f64::consts::PI, std::f64::consts::E),
        ];

        for (i, (e, g)) in test_values.iter().enumerate() {
            recorder
                .push_row(&format!("2024-01-01T00:{i:02}:00Z"), &[*e, *g])
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
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Csv,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        recorder
            .push_row("2024-01-01T00:00:00Z", &[1.0, 2.0])
            .unwrap();
        recorder
            .push_row("2024-01-01T00:01:00Z", &[3.0, 4.0])
            .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 2);

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("Total Electric Power (kW)"));
        assert!(contents.contains("1.0"));
    }

    #[test]
    fn flush_count_matches_chunk_size() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let chunk_size = 10_000;
        let total_rows = 100_000;
        let mut recorder = StreamingRecorder::new(
            schema,
            chunk_size,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        for i in 0..total_rows {
            recorder
                .push_row(&format!("T{i}"), &[i as f64, 0.0])
                .unwrap();
        }

        // After 100k rows with chunk_size=10k, buffer should be empty
        // (last push_row triggered the 10th flush).
        assert_eq!(recorder.buffered_rows(), 0);
        assert_eq!(recorder.total_rows(), total_rows);

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, total_rows);
    }

    #[test]
    fn finish_on_empty_recorder_produces_valid_file() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 0);
        assert!(summary.byte_size > 0); // Parquet writes metadata even for 0 rows.

        // Verify the file is valid Parquet.
        let file = File::open(&path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.collect();
        let total: usize = batches
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(total, 0);
    }

    #[test]
    fn finish_on_empty_recorder_with_rotation_produces_valid_summary() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().join("output.parquet");

        let schema = test_schema();
        let recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::Daily,
        )
        .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 0);
        assert_eq!(summary.byte_size, 0);
        assert!(summary.rotated_paths.is_empty());
    }

    #[test]
    fn column_mismatch_returns_error() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        let result = recorder.push_row("T0", &[1.0]); // 1 value, expects 2
        assert!(result.is_err());
    }

    #[test]
    fn partial_chunk_flushed_on_finish() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            1000,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        for i in 0..5 {
            recorder
                .push_row(&format!("T{i}"), &[i as f64, 0.0])
                .unwrap();
        }
        assert_eq!(recorder.buffered_rows(), 5);

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

    #[test]
    fn zero_chunk_size_rejected() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let result = StreamingRecorder::new(
            schema,
            0,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        );
        assert!(matches!(result, Err(OutputError::InvalidChunkSize)));
    }

    #[test]
    fn batch_retention_disabled() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            10,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        for i in 0..25 {
            recorder
                .push_row(&format!("T{i}"), &[i as f64, i as f64 * 2.0])
                .unwrap();
        }
        // 25 rows with chunk_size=10 → 2 full flushes triggered by push_row,
        // 5 rows remain buffered. When retain_batches=false, flushed_batches
        // is always empty.
        assert!(recorder.flushed_batches().is_empty());

        recorder.finish().unwrap();
    }

    #[test]
    fn batch_retention_enabled() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            10,
            OutputFormat::Parquet,
            &path,
            true,
            RotationPolicy::None,
        )
        .unwrap();

        for i in 0..25 {
            recorder
                .push_row(&format!("T{i}"), &[i as f64, i as f64 * 2.0])
                .unwrap();
        }
        // 25 rows with chunk_size=10 → 2 flushes triggered by push_row.
        assert_eq!(recorder.flushed_batches().len(), 2);

        recorder.finish().unwrap();
    }

    #[test]
    fn retention_disabled_memory_bounded() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let chunk_size = 50;
        let total_rows = 10 * chunk_size; // exercise 10x chunk_size
        let mut recorder = StreamingRecorder::new(
            schema,
            chunk_size,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        for i in 0..total_rows {
            recorder
                .push_row(&format!("T{i}"), &[i as f64, 0.0])
                .unwrap();
        }
        // With retain_batches=false, flushed_batches is always empty.
        assert!(recorder.flushed_batches().is_empty());
        assert_eq!(recorder.total_rows(), total_rows);

        recorder.finish().unwrap();
    }

    // ---------------------------------------------------------------------------
    // Rotation tests
    // ---------------------------------------------------------------------------

    #[test]
    fn rotation_none_produces_single_file_with_original_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dwelling_42.parquet");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        recorder
            .push_row("2024-01-01T00:00:00Z", &[1.0, 2.0])
            .unwrap();
        recorder
            .push_row("2024-01-02T00:00:00Z", &[3.0, 4.0])
            .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 2);
        // Exactly one file, at the original path.
        assert_eq!(summary.rotated_paths.len(), 1);
        assert_eq!(summary.path, path);
        assert!(path.exists());
        assert!(summary.byte_size > 0);

        // No extra files created in the directory.
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn rotation_none_csv_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.csv");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Csv,
            &path,
            false,
            RotationPolicy::None,
        )
        .unwrap();

        recorder
            .push_row("2024-01-01T00:00:00Z", &[1.0, 2.0])
            .unwrap();
        recorder
            .push_row("2024-01-02T00:00:00Z", &[3.0, 4.0])
            .unwrap();
        recorder.flush_and_close().unwrap();

        assert!(path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn rotation_daily_boundary_crossing_two_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dwelling_42.parquet");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema.clone(),
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::Daily,
        )
        .unwrap();

        // Day 1 rows
        recorder
            .push_row("2024-01-01T12:00:00Z", &[1.0, 10.0])
            .unwrap();
        recorder
            .push_row("2024-01-01T23:59:00Z", &[2.0, 20.0])
            .unwrap();

        // Day 2 row — crosses boundary
        recorder
            .push_row("2024-01-02T00:01:00Z", &[3.0, 30.0])
            .unwrap();
        recorder
            .push_row("2024-01-02T12:00:00Z", &[4.0, 40.0])
            .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 4);

        // Two files with correct timestamp suffixes.
        assert_eq!(summary.rotated_paths.len(), 2);

        let day1_path = dir.path().join("dwelling_42_2024-01-01.parquet");
        let day2_path = dir.path().join("dwelling_42_2024-01-02.parquet");
        assert!(day1_path.exists(), "day 1 file missing");
        assert!(day2_path.exists(), "day 2 file missing");

        // Read back and verify row counts per file.
        let file1 = File::open(&day1_path).unwrap();
        let reader1 = ParquetRecordBatchReaderBuilder::try_new(file1)
            .unwrap()
            .build()
            .unwrap();
        let rows1: usize = reader1.filter_map(Result::ok).map(|b| b.num_rows()).sum();
        assert_eq!(rows1, 2, "day 1 should have 2 rows");

        let file2 = File::open(&day2_path).unwrap();
        let reader2 = ParquetRecordBatchReaderBuilder::try_new(file2)
            .unwrap()
            .build()
            .unwrap();
        let rows2: usize = reader2.filter_map(Result::ok).map(|b| b.num_rows()).sum();
        assert_eq!(rows2, 2, "day 2 should have 2 rows");
    }

    #[test]
    fn rotation_monthly_schema_consistency() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dwelling.parquet");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &path,
            false,
            RotationPolicy::Monthly,
        )
        .unwrap();

        // January row
        recorder
            .push_row("2024-01-15T00:00:00Z", &[1.0, 10.0])
            .unwrap();

        // February row — crosses month boundary
        recorder
            .push_row("2024-02-01T00:00:00Z", &[2.0, 20.0])
            .unwrap();

        recorder.flush_and_close().unwrap();

        let jan = dir.path().join("dwelling_2024-01.parquet");
        let feb = dir.path().join("dwelling_2024-02.parquet");
        assert!(jan.exists());
        assert!(feb.exists());

        // Read schemas from both files.
        let schema1 = ParquetRecordBatchReaderBuilder::try_new(File::open(&jan).unwrap())
            .unwrap()
            .build()
            .unwrap()
            .schema();
        let schema2 = ParquetRecordBatchReaderBuilder::try_new(File::open(&feb).unwrap())
            .unwrap()
            .build()
            .unwrap()
            .schema();
        assert_eq!(
            schema1, schema2,
            "schemas must be identical across rotated files"
        );
    }

    #[test]
    fn rotation_no_data_loss_parquet() {
        let dir = tempfile::tempdir().unwrap();
        let rotated_path = dir.path().join("dwelling_42.parquet");
        let single_path = dir.path().join("dwelling_single.parquet");

        let test_rows: Vec<(&str, f64, f64)> = vec![
            ("2024-01-01T00:00:00Z", 1.0, 10.0),
            ("2024-01-01T12:00:00Z", 2.0, 20.0),
            ("2024-01-02T00:00:00Z", 3.0, 30.0), // day boundary
            ("2024-01-02T12:00:00Z", 4.0, 40.0),
            ("2024-01-03T06:00:00Z", 5.0, 50.0), // another day
        ];

        // Run with daily rotation.
        let schema = test_schema();
        let mut rot_recorder = StreamingRecorder::new(
            schema.clone(),
            100,
            OutputFormat::Parquet,
            &rotated_path,
            false,
            RotationPolicy::Daily,
        )
        .unwrap();
        for (ts, a, b) in &test_rows {
            rot_recorder.push_row(ts, &[*a, *b]).unwrap();
        }
        let rot_summary = rot_recorder.finish().unwrap();

        // Run without rotation (single file).
        let mut single_recorder = StreamingRecorder::new(
            schema.clone(),
            100,
            OutputFormat::Parquet,
            &single_path,
            false,
            RotationPolicy::None,
        )
        .unwrap();
        for (ts, a, b) in &test_rows {
            single_recorder.push_row(ts, &[*a, *b]).unwrap();
        }
        single_recorder.finish().unwrap();

        // Row counts must match.
        assert_eq!(rot_summary.row_count, test_rows.len());

        // Read back all rows from all rotated files and compare with single file.
        let mut rot_electric: Vec<f64> = Vec::new();
        let mut rot_gas: Vec<f64> = Vec::new();
        for path in &rot_summary.rotated_paths {
            let file = File::open(path).unwrap();
            let reader = ParquetRecordBatchReaderBuilder::try_new(file)
                .unwrap()
                .build()
                .unwrap();
            for batch in reader {
                let batch = batch.unwrap();
                let e = batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                let g = batch
                    .column(2)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap();
                for i in 0..batch.num_rows() {
                    rot_electric.push(e.value(i));
                    rot_gas.push(g.value(i));
                }
            }
        }

        let file = File::open(&single_path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let mut single_electric: Vec<f64> = Vec::new();
        let mut single_gas: Vec<f64> = Vec::new();
        for batch in reader {
            let batch = batch.unwrap();
            let e = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let g = batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                single_electric.push(e.value(i));
                single_gas.push(g.value(i));
            }
        }

        assert_eq!(
            rot_electric.len(),
            test_rows.len(),
            "rotated file row count mismatch"
        );
        assert_eq!(rot_electric, single_electric, "electric power mismatch");
        assert_eq!(rot_gas, single_gas, "gas power mismatch");
    }

    #[test]
    fn rotation_csv_daily_produces_two_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.csv");

        let schema = test_schema();
        let mut recorder = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Csv,
            &path,
            false,
            RotationPolicy::Daily,
        )
        .unwrap();

        recorder
            .push_row("2024-01-01T00:00:00Z", &[1.0, 2.0])
            .unwrap();
        recorder
            .push_row("2024-01-02T00:00:00Z", &[3.0, 4.0])
            .unwrap();

        let summary = recorder.finish().unwrap();
        assert_eq!(summary.row_count, 2);
        assert_eq!(summary.rotated_paths.len(), 2);

        let day1 = dir.path().join("output_2024-01-01.csv");
        let day2 = dir.path().join("output_2024-01-02.csv");
        assert!(day1.exists());
        assert!(day2.exists());

        // Both files should have headers.
        let d1 = std::fs::read_to_string(&day1).unwrap();
        let d2 = std::fs::read_to_string(&day2).unwrap();
        assert!(d1.contains("Total Electric Power (kW)"));
        assert!(d2.contains("Total Electric Power (kW)"));
    }

    #[test]
    fn rotation_hourly_and_yearly() {
        let dir = tempfile::tempdir().unwrap();

        // Hourly
        let h_path = dir.path().join("h.parquet");
        let schema = test_schema();
        let mut hr = StreamingRecorder::new(
            schema.clone(),
            100,
            OutputFormat::Parquet,
            &h_path,
            false,
            RotationPolicy::Hourly,
        )
        .unwrap();
        hr.push_row("2024-01-01T00:30:00Z", &[1.0, 1.0]).unwrap();
        hr.push_row("2024-01-01T01:00:00Z", &[2.0, 2.0]).unwrap();
        let hs = hr.finish().unwrap();
        assert_eq!(hs.rotated_paths.len(), 2);
        assert!(dir.path().join("h_2024-01-01T00.parquet").exists());
        assert!(dir.path().join("h_2024-01-01T01.parquet").exists());

        // Yearly
        let y_path = dir.path().join("y.parquet");
        let mut yr = StreamingRecorder::new(
            schema,
            100,
            OutputFormat::Parquet,
            &y_path,
            false,
            RotationPolicy::Yearly,
        )
        .unwrap();
        yr.push_row("2024-06-15T00:00:00Z", &[1.0, 1.0]).unwrap();
        yr.push_row("2025-01-01T00:00:00Z", &[2.0, 2.0]).unwrap();
        let ys = yr.finish().unwrap();
        assert_eq!(ys.rotated_paths.len(), 2);
        assert!(dir.path().join("y_2024.parquet").exists());
        assert!(dir.path().join("y_2025.parquet").exists());
    }
}
