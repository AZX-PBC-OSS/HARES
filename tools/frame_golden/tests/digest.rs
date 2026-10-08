//! Digest acceptance tests: identical content is equal, one flipped bit is
//! detected with its column and block, NaN payloads and validity count,
//! and the digest is independent of batch chunking.

mod common;

use frame_golden::{BLOCK_ROWS, FrameDigests, digest_frame, first_digest_difference};

#[test]
fn digest_of_identical_batches_is_equal() {
    let rows = common::rows(300, 7);
    let once = digest_frame(&[common::frame(&rows)]).unwrap();
    let again = digest_frame(&[common::frame(&rows)]).unwrap();
    assert_eq!(once, again);

    // Independently chunked into two halves is still one logical column
    // stream; equality here is the chunking test's job, so only sanity
    // check the digest is stable per batch set.
    let batches = common::chunked(&rows, 150);
    let split = digest_frame(&batches).unwrap();
    assert_eq!(once, split);
}

#[test]
fn digest_detects_one_bit_of_one_f64() {
    let mut values = Vec::new();
    for row in 0..300 {
        values.push(Some(row as f64 * 0.5));
    }
    let baseline = digest_frame(&[common::batch(
        arrow_schema_power(),
        vec![common::f64_column(&values)],
    )])
    .unwrap();

    // Flip the low bit of one payload far into block 2.
    let row = 2 * BLOCK_ROWS + 40;
    let original = values[row].unwrap();
    values[row] = Some(f64::from_bits(original.to_bits() ^ 1));
    let changed = digest_frame(&[common::batch(
        arrow_schema_power(),
        vec![common::f64_column(&values)],
    )])
    .unwrap();

    let location = first_digest_difference(&baseline, &changed)
        .expect("a one-bit payload flip must change the digest");
    assert_eq!(location.column, "Power (kW)");
    assert_eq!(location.block, 2);

    // Reversed sides report the same location.
    let reversed = first_digest_difference(&changed, &baseline).expect("symmetric");
    assert_eq!(reversed, location);
}

#[test]
fn digest_distinguishes_nan_payloads() {
    let nan_a = f64::from_bits(0x7ff8_0000_0000_0001);
    let nan_b = f64::from_bits(0x7ff8_0000_0000_0002);

    let a = power_frame(&[Some(nan_a)]);
    let b = power_frame(&[Some(nan_b)]);
    let a_again = power_frame(&[Some(nan_a)]);

    assert_ne!(
        digest_frame(&[a]).unwrap(),
        digest_frame(&[b]).unwrap(),
        "two NaNs with different payloads must digest differently"
    );
    assert_eq!(
        digest_frame(std::slice::from_ref(&a_again)).unwrap(),
        digest_frame(std::slice::from_ref(&a_again)).unwrap(),
        "identical NaN payloads must digest identically"
    );
}

#[test]
fn digest_distinguishes_validity() {
    let with_null = power_frame(&[Some(1.0), None, Some(3.0)]);
    let with_zero = power_frame(&[Some(1.0), Some(0.0), Some(3.0)]);
    assert_ne!(
        digest_frame(&[with_null]).unwrap(),
        digest_frame(&[with_zero]).unwrap(),
        "a null cell and a zero-valued cell must digest differently even \
         when the null slot's raw bytes are zero"
    );
}

#[test]
fn digest_is_independent_of_batch_chunking() {
    let rows = common::rows(300, 5);
    let one_batch = digest_of(&[common::frame(&rows)]);
    let by_one = digest_of(&common::chunked(&rows, 1));
    let by_seven = digest_of(&common::chunked(&rows, 7));
    let by_block = digest_of(&common::chunked(&rows, BLOCK_ROWS));

    assert_eq!(one_batch, by_one, "batches of 1 row");
    assert_eq!(one_batch, by_seven, "batches of 7 rows");
    assert_eq!(one_batch, by_block, "batches of 96 rows");
}

/// Timestamps and dates digest through their physical buffers: their
/// distinct array types do not downcast to the integer array types they
/// share storage with, so a per-type downcast would error here.
#[test]
fn digest_reads_timestamp_and_date32_physical_values() {
    use std::sync::Arc;

    use arrow::array::{Date32Array, TimestampMillisecondArray};
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

    let timestamp_values: Vec<Option<i64>> = (0..10)
        .map(|row| Some(1_700_000_000_000 + row as i64))
        .collect();
    let timestamp_schema = Schema::new(vec![Field::new(
        "ts",
        DataType::Timestamp(TimeUnit::Millisecond, None),
        true,
    )]);
    let baseline = digest_frame(&[common::batch(
        timestamp_schema.clone(),
        vec![Arc::new(TimestampMillisecondArray::from(
            timestamp_values.clone(),
        ))],
    )])
    .unwrap();

    let mut changed_values = timestamp_values;
    changed_values[3] = Some(1_700_000_000_001);
    let changed = digest_frame(&[common::batch(
        timestamp_schema,
        vec![Arc::new(TimestampMillisecondArray::from(changed_values))],
    )])
    .unwrap();

    let location = first_digest_difference(&baseline, &changed)
        .expect("a one-millisecond timestamp change must change the digest");
    assert_eq!(location.column, "ts");

    let date_values: Vec<Option<i32>> = (0..10i32).map(|row| Some(19_000 + row)).collect();
    let date_schema = Schema::new(vec![Field::new("day", DataType::Date32, true)]);
    let date_baseline = digest_frame(&[common::batch(
        date_schema.clone(),
        vec![Arc::new(Date32Array::from(date_values.clone()))],
    )])
    .unwrap();

    let mut changed_date = date_values;
    changed_date[3] = Some(19_001);
    let date_changed = digest_frame(&[common::batch(
        date_schema,
        vec![Arc::new(Date32Array::from(changed_date))],
    )])
    .unwrap();

    let date_location = first_digest_difference(&date_baseline, &date_changed)
        .expect("a one-day date change must change the digest");
    assert_eq!(date_location.column, "day");
}

fn digest_of(batches: &[arrow::record_batch::RecordBatch]) -> FrameDigests {
    digest_frame(batches).unwrap()
}

fn arrow_schema_power() -> arrow::datatypes::Schema {
    arrow::datatypes::Schema::new(vec![arrow::datatypes::Field::new(
        "Power (kW)",
        arrow::datatypes::DataType::Float64,
        true,
    )])
}

fn power_frame(values: &[Option<f64>]) -> arrow::record_batch::RecordBatch {
    common::batch(arrow_schema_power(), vec![common::f64_column(values)])
}

/// A zero-row frame product (num_rows = 0 in a committed golden) is stored
/// as a schema-only parquet: the reader yields no row groups. The read-back
/// must carry the schema as one empty batch, so the digest is the empty
/// input's and a later `delta` can compare the product instead of erroring
/// on it (the defect the fleet aggregate's zero-row product hit).
#[test]
fn zero_row_product_reads_back_as_its_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aggregate.parquet");

    let schema = arrow_schema_power();
    let empty = arrow::record_batch::RecordBatch::new_empty(std::sync::Arc::new(schema.clone()));
    frame_golden::frames::write_frame(&path, &[empty]).unwrap();

    let (read_schema, batches) = frame_golden::frames::read_frame(&path).unwrap();
    assert_eq!(*read_schema, schema);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_rows(), 0);

    // Zero rows feed the block loop nothing: the column digest is SHA-256
    // of the empty input, the digest a captured zero-row product records.
    let digests = digest_of(&batches);
    let empty_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(digests.columns[0].digest, empty_sha);
}
