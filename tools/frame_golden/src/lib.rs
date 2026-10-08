//! Golden capture and bitwise compare tool for simulation frame products.

#[cfg(feature = "profiling")]
use hares_types::alloc_count::CountingAllocator;

pub mod adapter;
pub mod capture;
pub mod cli;
pub mod compare;
pub mod defaults;
pub mod digest;
pub mod error;
pub mod frames;
pub mod golden;
pub mod manifest;

// Installed in the library so every binary linking it (the tool's binary
// and its test binaries) counts allocations. Only under `-F profiling`:
// the plain release binary whose wall times the measurement log recorded
// keeps the system allocator unchanged.
#[cfg(feature = "profiling")]
#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

pub use adapter::{FrameProducts, RunOutput, RunProducts, RunRequest, RunTiming};
pub use capture::{Captured, capture as capture_manifest, materialize as materialize_manifest};
pub use compare::{
    ColumnDelta, ColumnSelection, CompareReport, DeltaReport, Difference, RowMismatch,
};
pub use digest::{
    BLOCK_ROWS, ColumnDigest, FrameDigests, SchemaFieldDigest, digest_frame,
    first_digest_difference,
};
pub use error::{FrameGoldenError, FrameGoldenResult};
pub use golden::{GoldenDoc, MetricField, MetricValue, MetricsRow};
pub use manifest::{Feature, GoldenManifest, ManifestKind, running_features};
