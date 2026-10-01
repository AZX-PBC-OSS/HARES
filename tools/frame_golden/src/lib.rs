//! Golden capture and bitwise compare tool for simulation frame products.

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
