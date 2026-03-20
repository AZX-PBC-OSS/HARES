//! Simulation output writing (Arrow/Parquet/CSV).
//!
//! Output is written incrementally via [`StreamingRecorder`], which buffers
//! rows up to a configurable chunk size and flushes to disk, keeping peak
//! memory proportional to the chunk size rather than simulation length.

pub mod columns;
pub mod metrics;
pub mod writer;

use std::path::PathBuf;

pub use columns::{build_schema, expected_columns_at_verbosity, mode_to_ordinal};
pub use writer::{OutputError, StreamingRecorder};

/// Summary returned by [`StreamingRecorder::finish`].
#[derive(Debug, Clone)]
pub struct OutputSummary {
    /// Total number of rows written.
    pub row_count: usize,
    /// Size of the output file in bytes.
    pub byte_size: u64,
    /// Path to the output file.
    pub path: PathBuf,
}
