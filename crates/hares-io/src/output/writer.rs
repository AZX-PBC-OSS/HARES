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

use super::OutputSummary;
use crate::config::OutputFormat;

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
/// Peak memory is bounded: the buffer holds at most `chunk_size` rows.
pub struct StreamingRecorder {
    schema: Arc<Schema>,
    chunk_size: usize,
    timestamp_buf: Vec<String>,
    value_bufs: Vec<Vec<f64>>,
    backend: Backend,
    total_rows: usize,
    output_path: PathBuf,
    flushed_batches: Vec<RecordBatch>,
}

impl StreamingRecorder {
    /// Create a new streaming recorder.
    ///
    /// # Arguments
    /// - `schema` — Arrow schema defining all output columns.
    /// - `chunk_size` — Maximum rows to buffer before flushing. Must be > 0.
    /// - `format` — Output format (CSV or Parquet).
    /// - `output_path` — Destination file path.
    pub fn new(
        schema: Schema,
        chunk_size: usize,
        format: OutputFormat,
        output_path: &Path,
    ) -> Result<Self, OutputError> {
        if chunk_size == 0 {
            return Err(OutputError::InvalidChunkSize);
        }

        let schema = Arc::new(schema);
        // Number of f64 value columns (everything except the timestamp).
        let n_value_cols = schema.fields().len() - 1;

        let file = File::create(output_path)?;
        let buf_writer = BufWriter::new(file);

        let backend = match format {
            OutputFormat::Parquet => {
                let props = WriterProperties::builder()
                    .set_compression(Compression::SNAPPY)
                    .build();
                let writer = ArrowWriter::try_new(buf_writer, Arc::clone(&schema), Some(props))?;
                Backend::Parquet(Box::new(writer))
            }
            OutputFormat::Csv => {
                let writer = arrow::csv::WriterBuilder::new()
                    .with_header(true)
                    .build(buf_writer);
                Backend::Csv(Box::new(writer))
            }
        };

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
        })
    }

    /// Append a single row. The first element of `timestamp` is the time
    /// string; `values` contains all numeric columns in schema order
    /// (excluding the timestamp column).
    ///
    /// Automatically flushes when the buffer reaches `chunk_size`.
    pub fn push_row(&mut self, timestamp: &str, values: &[f64]) -> Result<(), OutputError> {
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
        if self.timestamp_buf.is_empty() {
            return Ok(());
        }

        let batch = self.build_batch()?;

        match &mut self.backend {
            Backend::Parquet(w) => {
                w.write(&batch)?;
            }
            Backend::Csv(writer) => {
                writer.write(&batch)?;
            }
        }

        self.flushed_batches.push(batch);

        Ok(())
    }

    /// Returns all batches that have been flushed so far.
    #[must_use]
    pub fn flushed_batches(&self) -> &[RecordBatch] {
        &self.flushed_batches
    }

    /// Flush remaining rows, close the file, and return summary statistics.
    pub fn finish(mut self) -> Result<OutputSummary, OutputError> {
        self.flush()?;

        match self.backend {
            Backend::Parquet(w) => {
                w.close()?;
            }
            Backend::Csv(writer) => {
                // into_inner() flushes the CSV writer internals, then we
                // explicitly flush the BufWriter to surface IO errors
                // before drop, rather than silently losing data.
                use std::io::Write;
                let mut buf_writer = writer.into_inner();
                buf_writer.flush()?;
            }
        }

        let byte_size = std::fs::metadata(&self.output_path)?.len();

        Ok(OutputSummary {
            row_count: self.total_rows,
            byte_size,
            path: self.output_path,
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
    use arrow::array::Float64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::config::OutputFormat;

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
        let mut recorder =
            StreamingRecorder::new(schema, 100, OutputFormat::Parquet, &path).unwrap();

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
        let mut recorder = StreamingRecorder::new(schema, 100, OutputFormat::Csv, &path).unwrap();

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
        let mut recorder =
            StreamingRecorder::new(schema, chunk_size, OutputFormat::Parquet, &path).unwrap();

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
        let recorder = StreamingRecorder::new(schema, 100, OutputFormat::Parquet, &path).unwrap();

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
    fn column_mismatch_returns_error() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder =
            StreamingRecorder::new(schema, 100, OutputFormat::Parquet, &path).unwrap();

        let result = recorder.push_row("T0", &[1.0]); // 1 value, expects 2
        assert!(result.is_err());
    }

    #[test]
    fn partial_chunk_flushed_on_finish() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let schema = test_schema();
        let mut recorder =
            StreamingRecorder::new(schema, 1000, OutputFormat::Parquet, &path).unwrap();

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
        let result = StreamingRecorder::new(schema, 0, OutputFormat::Parquet, &path);
        assert!(matches!(result, Err(OutputError::InvalidChunkSize)));
    }
}
