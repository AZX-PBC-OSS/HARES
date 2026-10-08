//! Bit-level digests over Arrow columns.
//!
//! The canonical digest input for one column over a row range is, in order:
//! the validity bitmap (one bit per row, LSB-first within each byte, the
//! same packing Arrow uses), then the value bytes. Null rows contribute a
//! zero validity bit and zero value bytes so the digest depends only on
//! logical content: the raw bytes behind a null slot are unspecified and
//! change with batch chunking, which would otherwise break
//! `digest_is_independent_of_batch_chunking`.
//!
//! Value bytes by type: floats as `f64::to_bits()` little-endian, so NaN
//! payloads count; integers, timestamps and durations as their
//! little-endian physical bytes; booleans as one byte per value, the value
//! in the byte's lowest bit; strings as a little-endian `u32` (u64 for
//! `LargeUtf8`) length followed by the bytes.

use std::sync::Arc;

use arrow::array::{
    Array, ArrayData, ArrayRef, BooleanArray, Float64Array, LargeStringArray, StringArray,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{FrameGoldenError, FrameGoldenResult};

/// Row count of one digest block. A first difference is located to a block
/// of this many rows without keeping the full frame around.
pub const BLOCK_ROWS: usize = 96;

/// One column of the frame schema: name, Arrow type, nullability, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaFieldDigest {
    pub name: String,
    pub arrow_type: String,
    pub nullable: bool,
}

/// Digests for one column: the whole-column digest plus one digest per
/// 96-row block. Block k covers rows `96k` to `min(96(k+1), num_rows)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnDigest {
    pub name: String,
    pub digest: String,
    pub blocks: Vec<String>,
}

/// The committed form of one frame product: schema, row count, per-column
/// digests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameDigests {
    pub num_rows: usize,
    pub schema: Vec<SchemaFieldDigest>,
    pub columns: Vec<ColumnDigest>,
}

/// Where two digest sets first disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestLocation {
    pub column: String,
    pub block: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    F64,
    I64,
    I32,
    U32,
    U64,
    Bool,
    Str,
    LargeStr,
}

fn encoding_of(data_type: &DataType) -> FrameGoldenResult<Encoding> {
    match data_type {
        DataType::Float64 => Ok(Encoding::F64),
        // The physical storage of every type on this line is i64.
        DataType::Int64
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Date64
        | DataType::Time64(_) => Ok(Encoding::I64),
        DataType::Int32 | DataType::Date32 => Ok(Encoding::I32),
        DataType::UInt32 => Ok(Encoding::U32),
        DataType::UInt64 => Ok(Encoding::U64),
        DataType::Boolean => Ok(Encoding::Bool),
        DataType::Utf8 => Ok(Encoding::Str),
        DataType::LargeUtf8 => Ok(Encoding::LargeStr),
        other => Err(FrameGoldenError::Digest(format!(
            "unsupported column type {other}: extend the digest encodings when a product grows this type"
        ))),
    }
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Lowercase hex of a digest, for provenance hashes recorded beside the
/// per-column digests.
pub fn hex_digest(bytes: &[u8]) -> String {
    hex_of(bytes)
}

/// Global row offsets of each batch in one column.
struct Pieces {
    offsets: Vec<usize>,
    total_rows: usize,
}

impl Pieces {
    fn build(arrays: &[ArrayRef]) -> Self {
        let mut offsets = Vec::with_capacity(arrays.len());
        let mut total_rows = 0usize;
        for array in arrays {
            offsets.push(total_rows);
            total_rows += array.len();
        }
        Self {
            offsets,
            total_rows,
        }
    }
}

/// Appends the validity bitmap of rows `start..end` (global indices) to
/// `out`, packed LSB-first exactly like Arrow's own bitmaps.
fn feed_validity(
    arrays: &[ArrayRef],
    pieces: &Pieces,
    start: usize,
    end: usize,
    out: &mut Vec<u8>,
) {
    let rows = end - start;
    let mut bitmap = vec![0u8; rows.div_ceil(8)];
    let mut bit = 0usize;
    for (index, array) in arrays.iter().enumerate() {
        let piece_start = pieces.offsets[index];
        let piece_end = piece_start + array.len();
        if piece_end <= start {
            continue;
        }
        let lo = start.max(piece_start);
        let hi = end.min(piece_end);
        for global in lo..hi {
            if !array.is_null(global - piece_start) {
                bitmap[bit >> 3] |= 1 << (bit & 7);
            }
            bit += 1;
        }
    }
    out.extend_from_slice(&bitmap);
}

/// Appends the canonical value bytes of rows `start..end` (global indices) to `out`.
fn feed_values(
    encoding: Encoding,
    arrays: &[ArrayRef],
    pieces: &Pieces,
    start: usize,
    end: usize,
    out: &mut Vec<u8>,
) -> FrameGoldenResult<()> {
    for (index, array) in arrays.iter().enumerate() {
        let piece_start = pieces.offsets[index];
        let piece_end = piece_start + array.len();
        if piece_end <= start {
            continue;
        }
        let lo = start.max(piece_start);
        let hi = end.min(piece_end);
        // A piece entirely past this block has no rows here: the old
        // per-row loop was simply empty, so skip it before the offsets
        // are subtracted.
        if lo >= hi {
            continue;
        }
        feed_piece_values(encoding, array, lo - piece_start, hi - piece_start, out)?;
    }
    Ok(())
}

/// The canonical value bytes of rows `lo..hi` (local indices) of one
/// batch piece. [`value_bytes`] reports exactly these bytes for one cell,
/// so `compare`'s value-level equality agrees with the digests by
/// construction.
fn feed_piece_values(
    encoding: Encoding,
    array: &ArrayRef,
    lo: usize,
    hi: usize,
    out: &mut Vec<u8>,
) -> FrameGoldenResult<()> {
    match encoding {
        // Fixed-width types digest through their physical value buffer:
        // the array types sharing one storage (Timestamp, Duration,
        // Date64 and Time64 all store i64, Date32 stores i32) are
        // distinct types that do not downcast to each other.
        Encoding::F64 | Encoding::I64 | Encoding::U64 => fixed_width_8(array, lo, hi, out)?,
        Encoding::I32 | Encoding::U32 => fixed_width_4(array, lo, hi, out)?,
        Encoding::Bool => {
            let values = as_primitive::<BooleanArray>(array)?;
            for local in lo..hi {
                // Each boolean appends one whole byte, its value in the
                // byte's lowest bit.
                let bit = out.len() * 8;
                let byte = bit >> 3;
                out.resize(byte + 1, 0);
                if !values.is_null(local) && values.value(local) {
                    out[byte] |= 1 << (bit & 7);
                }
            }
        }
        Encoding::Str => {
            let values = as_primitive::<StringArray>(array)?;
            for local in lo..hi {
                if values.is_null(local) {
                    out.extend_from_slice(&0u32.to_le_bytes());
                } else {
                    let value = values.value(local);
                    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    out.extend_from_slice(value.as_bytes());
                }
            }
        }
        Encoding::LargeStr => {
            let values = as_primitive::<LargeStringArray>(array)?;
            for local in lo..hi {
                if values.is_null(local) {
                    out.extend_from_slice(&0u64.to_le_bytes());
                } else {
                    let value = values.value(local);
                    out.extend_from_slice(&(value.len() as u64).to_le_bytes());
                    out.extend_from_slice(value.as_bytes());
                }
            }
        }
    }
    Ok(())
}

/// The canonical value bytes of rows `lo..hi` of one 8-byte fixed-width
/// piece: each value's bytes normalized to little-endian (the canonical
/// order; the buffer is native-endian), zero bytes behind null slots (the
/// raw bytes behind a null slot are unspecified).
fn fixed_width_8(
    array: &ArrayRef,
    lo: usize,
    hi: usize,
    out: &mut Vec<u8>,
) -> FrameGoldenResult<()> {
    let data = array.to_data();
    for local in lo..hi {
        if array.is_null(local) {
            out.extend_from_slice(&[0u8; 8]);
        } else {
            out.extend_from_slice(
                u64::from_ne_bytes(physical_value_8(&data, local)?)
                    .to_le_bytes()
                    .as_slice(),
            );
        }
    }
    Ok(())
}

/// Same as [`fixed_width_8`] for one 4-byte fixed-width piece.
fn fixed_width_4(
    array: &ArrayRef,
    lo: usize,
    hi: usize,
    out: &mut Vec<u8>,
) -> FrameGoldenResult<()> {
    let data = array.to_data();
    for local in lo..hi {
        if array.is_null(local) {
            out.extend_from_slice(&[0u8; 4]);
        } else {
            out.extend_from_slice(
                u32::from_ne_bytes(physical_value_4(&data, local)?)
                    .to_le_bytes()
                    .as_slice(),
            );
        }
    }
    Ok(())
}

/// The canonical value bytes of one row of one column: exactly the bytes
/// [`digest_frame`] hashes for that cell, so `compare`'s value-level
/// equality agrees with the digests by construction.
pub(crate) fn value_bytes(array: &ArrayRef, row: usize) -> FrameGoldenResult<Vec<u8>> {
    let encoding = encoding_of(array.data_type())?;
    let mut out = Vec::new();
    feed_piece_values(encoding, array, row, row + 1, &mut out)?;
    Ok(out)
}

/// One non-null row's value as f64, for delta statistics over numeric
/// columns: floats as themselves, the integer-physical families through
/// their physical buffers.
pub(crate) fn numeric_value(array: &ArrayRef, row: usize) -> FrameGoldenResult<f64> {
    let data = array.to_data();
    Ok(match encoding_of(array.data_type())? {
        Encoding::F64 => f64::from_ne_bytes(physical_value_8(&data, row)?),
        Encoding::I64 => i64::from_ne_bytes(physical_value_8(&data, row)?) as f64,
        Encoding::I32 => i32::from_ne_bytes(physical_value_4(&data, row)?) as f64,
        Encoding::U32 => u32::from_ne_bytes(physical_value_4(&data, row)?) as f64,
        Encoding::U64 => u64::from_ne_bytes(physical_value_8(&data, row)?) as f64,
        _ => {
            return Err(FrameGoldenError::Digest(format!(
                "column type {} has no numeric value for delta statistics",
                array.data_type()
            )));
        }
    })
}

/// The raw value buffer of one flat fixed-width array data.
fn value_buffer(data: &ArrayData) -> FrameGoldenResult<&[u8]> {
    let buffer = data.buffers().first().ok_or_else(|| {
        FrameGoldenError::Digest(format!(
            "column data {:?} has no value buffer",
            data.data_type()
        ))
    })?;
    Ok(buffer.as_slice())
}

/// The native-endian bytes of the physical value at flat row `row` of one
/// 8-byte fixed-width array data. The distinct Arrow array types sharing
/// one physical storage do not downcast to each other, so reads go
/// through the raw buffer.
pub(crate) fn physical_value_8(data: &ArrayData, row: usize) -> FrameGoldenResult<[u8; 8]> {
    let raw = value_buffer(data)?;
    let at = (data.offset() + row) * 8;
    Ok(raw[at..at + 8]
        .try_into()
        .expect("8 bytes per 8-wide physical value"))
}

/// Same as [`physical_value_8`] for one 4-byte fixed-width array data.
pub(crate) fn physical_value_4(data: &ArrayData, row: usize) -> FrameGoldenResult<[u8; 4]> {
    let raw = value_buffer(data)?;
    let at = (data.offset() + row) * 4;
    Ok(raw[at..at + 4]
        .try_into()
        .expect("4 bytes per 4-wide physical value"))
}

fn as_primitive<T: arrow::array::Array + 'static>(array: &ArrayRef) -> FrameGoldenResult<&T> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| downcast_error(array, std::any::type_name::<T>()))
}

fn downcast_error(array: &ArrayRef, expected: &str) -> FrameGoldenError {
    FrameGoldenError::Digest(format!(
        "column data {:?} does not downcast to {expected}",
        array.data_type()
    ))
}

/// Digests one logical column given its pieces (one array per batch).
fn digest_column(field: &Field, arrays: &[ArrayRef]) -> FrameGoldenResult<ColumnDigest> {
    let encoding = encoding_of(field.data_type())?;
    let pieces = Pieces::build(arrays);
    let mut master = Sha256::new();
    let mut blocks = Vec::new();
    let mut buffer: Vec<u8> = Vec::with_capacity(BLOCK_ROWS * 16);
    for block_start in (0..pieces.total_rows).step_by(BLOCK_ROWS) {
        let block_end = (block_start + BLOCK_ROWS).min(pieces.total_rows);
        buffer.clear();
        feed_validity(arrays, &pieces, block_start, block_end, &mut buffer);
        feed_values(
            encoding,
            arrays,
            &pieces,
            block_start,
            block_end,
            &mut buffer,
        )?;
        let mut block_hasher = Sha256::new();
        block_hasher.update(&buffer);
        master.update(&buffer);
        blocks.push(hex_of(&block_hasher.finalize()));
    }
    Ok(ColumnDigest {
        name: field.name().clone(),
        digest: hex_of(&master.finalize()),
        blocks,
    })
}

fn schema_digests(schema: &Schema) -> Vec<SchemaFieldDigest> {
    schema
        .fields()
        .iter()
        .map(|field| SchemaFieldDigest {
            name: field.name().clone(),
            arrow_type: field.data_type().to_string(),
            nullable: field.is_nullable(),
        })
        .collect()
}

/// Digests a frame held as one or more batches. Batches are concatenated
/// logically (row order preserved) before digesting, so the result does not
/// depend on how the frame is chunked.
pub fn digest_frame(batches: &[RecordBatch]) -> FrameGoldenResult<FrameDigests> {
    let Some(first) = batches.first() else {
        return Err(FrameGoldenError::Digest(
            "cannot digest an empty batch list: a frame product must carry a schema".to_string(),
        ));
    };
    let schema = first.schema();
    for (index, batch) in batches.iter().enumerate().skip(1) {
        if batch.schema() != schema {
            return Err(FrameGoldenError::Digest(format!(
                "batch {index} schema drifts from batch 0: a frame product must share one schema"
            )));
        }
    }
    let num_rows = batches.iter().map(RecordBatch::num_rows).sum();
    let mut columns = Vec::with_capacity(schema.fields().len());
    for index in 0..schema.fields().len() {
        let arrays: Vec<ArrayRef> = batches.iter().map(|b| b.column(index).clone()).collect();
        columns.push(digest_column(schema.field(index), &arrays)?);
    }
    Ok(FrameDigests {
        num_rows,
        schema: schema_digests(&schema),
        columns,
    })
}

/// Locates the first differing column and, within it, the first differing
/// 96-row block. Both sides must have equal schemas and row counts; callers
/// check those before calling.
pub fn first_digest_difference(
    expected: &FrameDigests,
    actual: &FrameDigests,
) -> Option<DigestLocation> {
    for (expected_column, actual_column) in expected.columns.iter().zip(&actual.columns) {
        if expected_column.digest == actual_column.digest {
            continue;
        }
        let block = expected_column
            .blocks
            .iter()
            .zip(&actual_column.blocks)
            .position(|(e, a)| e != a)
            .unwrap_or(0);
        return Some(DigestLocation {
            column: expected_column.name.clone(),
            block,
        });
    }
    None
}

/// Renders one value of a frame column for a mismatch report: null, the f64
/// bit pattern plus decimal, a physical integer value plus its bit pattern
/// (timestamps render physically: a civil-time rendering would need the
/// engine's timezone context), or the string contents.
pub fn format_column_value(array: &ArrayRef, row: usize) -> String {
    let data_type = array.data_type();
    if array.is_null(row) {
        return "null".to_string();
    }
    let data = array.to_data();
    match data_type {
        DataType::Float64 => {
            let values = array
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("schema checked before report");
            let value = values.value(row);
            format!("{value:?} (0x{:016x})", value.to_bits())
        }
        DataType::Int64
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Date64
        | DataType::Time64(_) => {
            let value = i64::from_ne_bytes(
                physical_value_8(&data, row).expect("fixed-width arrays carry one value buffer"),
            );
            format!("{value} (0x{value:016x})")
        }
        DataType::Int32 | DataType::Date32 => {
            let value = i32::from_ne_bytes(
                physical_value_4(&data, row).expect("fixed-width arrays carry one value buffer"),
            );
            format!("{value} (0x{value:08x})")
        }
        DataType::UInt32 => {
            let value = u32::from_ne_bytes(
                physical_value_4(&data, row).expect("fixed-width arrays carry one value buffer"),
            );
            format!("{value} (0x{value:08x})")
        }
        DataType::UInt64 => {
            let value = u64::from_ne_bytes(
                physical_value_8(&data, row).expect("fixed-width arrays carry one value buffer"),
            );
            format!("{value} (0x{value:016x})")
        }
        DataType::Utf8 => {
            let values = array
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("schema checked before report");
            format!("{:?}", values.value(row))
        }
        DataType::Boolean => {
            let values = array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .expect("schema checked before report");
            format!("{}", values.value(row))
        }
        _ => format!("{}", array.data_type()),
    }
}

/// The schema of `batches[0]`, for product frames held in memory.
pub fn schema_of(batches: &[RecordBatch]) -> FrameGoldenResult<Arc<Schema>> {
    let Some(first) = batches.first() else {
        return Err(FrameGoldenError::Digest(
            "frame product has no batches".to_string(),
        ));
    };
    Ok(first.schema())
}

/// The schema facts a golden pins: names, Arrow types, nullability, order.
/// Schema-level metadata is deliberately excluded: it is informational,
/// and the Parquet round-trip a full-frame comparison takes does not carry
/// it.
pub fn schema_fields_of(batches: &[RecordBatch]) -> FrameGoldenResult<Vec<SchemaFieldDigest>> {
    if batches.is_empty() {
        return Err(FrameGoldenError::Digest(
            "frame product has no batches".to_string(),
        ));
    }
    Ok(schema_digests(&batches[0].schema()))
}
