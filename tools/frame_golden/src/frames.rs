//! Full-frame Parquet IO: ZSTD-compressed products under `target/golden/`.
//!
//! Full frames are never committed; they exist locally so `delta` and
//! `compare` can point at exact rows and values, and so a numeric entry can
//! record what a change moved.

use std::path::Path;
use std::sync::Arc;

use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::error::{FrameGoldenError, FrameGoldenResult};

/// Writes one frame product (schema plus batches) as a ZSTD Parquet file.
pub fn write_frame(path: &Path, batches: &[RecordBatch]) -> FrameGoldenResult<()> {
    let Some(first) = batches.first() else {
        return Err(FrameGoldenError::Digest(
            "cannot write a frame product with no batches".to_string(),
        ));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .build();
    let mut writer = ArrowWriter::try_new(file, first.schema(), Some(properties))?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.close()?;
    Ok(())
}

/// Reads one frame product back: the schema and every row group's batches.
pub fn read_frame(
    path: &Path,
) -> FrameGoldenResult<(Arc<arrow::datatypes::Schema>, Vec<RecordBatch>)> {
    let file = std::fs::File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();
    let reader = builder.build()?;
    let batches: Vec<RecordBatch> =
        reader.collect::<Result<Vec<RecordBatch>, arrow::error::ArrowError>>()?;
    Ok((schema, batches))
}
