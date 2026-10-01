//! Errors raised by the golden capture and compare tool.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrameGoldenError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest {path}: {detail}")]
    Manifest { path: PathBuf, detail: String },

    #[error("simulation config: {0}")]
    SimulationConfig(String),

    #[error("digest: {0}")]
    Digest(String),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("engine run failed: {0}")]
    Engine(String),

    #[error(
        "feature set mismatch: manifest {manifest:?} requires {required:?}, running binary has {running:?}"
    )]
    FeatureSetMismatch {
        manifest: PathBuf,
        required: Vec<String>,
        running: Vec<String>,
    },

    #[error("materialized frames unavailable: {0}")]
    MaterializedFrames(String),

    #[error("usage: {0}")]
    Usage(String),
}

pub type FrameGoldenResult<T> = Result<T, FrameGoldenError>;
