//! Compare and delta acceptance tests: schema rejection with and without
//! column selection, first-mismatch-row reporting from materialized
//! frames, per-column delta statistics, and the materialize pointer in the
//! missing-frames error.

mod common;

use std::collections::BTreeMap;

use frame_golden::compare::{compare_products, delta_products};
use frame_golden::digest::digest_frame;

use arrow::datatypes::{DataType, Field, Schema};

fn schema_two_numeric() -> Schema {
    Schema::new(vec![
        Field::new("Time", DataType::Utf8, false),
        Field::new("A (kW)", DataType::Float64, true),
        Field::new("B (kW)", DataType::Float64, true),
    ])
}

fn frame_values(a: &[Option<f64>], b: &[Option<f64>]) -> arrow::record_batch::RecordBatch {
    let time: Vec<String> = (0..a.len()).map(|row| format!("t{row}")).collect();
    common::batch(
        schema_two_numeric(),
        vec![
            common::utf8_column_owned(time),
            common::f64_column(a),
            common::f64_column(b),
        ],
    )
}

fn digest_of(
    frame: &arrow::record_batch::RecordBatch,
) -> BTreeMap<String, frame_golden::FrameDigests> {
    let mut products = BTreeMap::new();
    products.insert(
        "frame".to_string(),
        digest_frame(std::slice::from_ref(frame)).unwrap(),
    );
    products
}

/// Digests of a product held as several batches, as a fresh multi-batch
/// run produces them.
fn digest_map_of(
    batches: &[arrow::record_batch::RecordBatch],
) -> BTreeMap<String, frame_golden::FrameDigests> {
    let mut products = BTreeMap::new();
    products.insert("frame".to_string(), digest_frame(batches).unwrap());
    products
}

/// A fresh frame product map holding one product named `frame`, built
/// from several batches.
fn fresh_map_of(
    batches: Vec<arrow::record_batch::RecordBatch>,
) -> BTreeMap<String, frame_golden::adapter::FrameProducts> {
    let mut products = BTreeMap::new();
    products.insert(
        "frame".to_string(),
        frame_golden::adapter::FrameProducts::from_batches(batches).expect("non-empty batch list"),
    );
    products
}

#[test]
fn compare_rejects_schema_mismatch_unless_columns_selected() {
    let golden_frame = frame_values(&[Some(1.0), Some(2.0)], &[Some(3.0), Some(4.0)]);
    let golden = common::golden_doc(digest_of(&golden_frame));

    // Same column count, different second numeric column: a schema drift.
    let drifted_schema = Schema::new(vec![
        Field::new("Time", DataType::Utf8, false),
        Field::new("A (kW)", DataType::Float64, true),
        Field::new("C (kW)", DataType::Float64, true),
    ]);
    let drifted = common::batch(
        drifted_schema,
        vec![
            common::utf8_column_owned(vec!["t0".to_string(), "t1".to_string()]),
            common::f64_column(&[Some(1.0), Some(2.0)]),
            common::f64_column(&[Some(3.0), Some(4.0)]),
        ],
    );

    let report = compare_products(
        "schema_drift",
        &golden,
        &common::frame_map(drifted.clone()),
        &common::fresh_metrics(),
        Some(&common::fresh_health()),
        &None,
        None,
    )
    .unwrap();
    assert!(
        !report.is_identical(),
        "a schema mismatch must be rejected: {report:?}"
    );
    assert!(
        report
            .differences
            .iter()
            .any(|difference| matches!(difference, frame_golden::Difference::ProductSchema { .. }))
    );

    // Selecting only the shared columns rejects nothing: the comparison is
    // restricted to columns that exist on both sides.
    let report = compare_products(
        "schema_drift",
        &golden,
        &common::frame_map(drifted),
        &common::fresh_metrics(),
        Some(&common::fresh_health()),
        &Some(vec!["A (kW)".to_string()]),
        None,
    )
    .unwrap();
    assert!(
        report.is_identical(),
        "selected shared columns must pass: {report:?}"
    );
}

#[test]
fn compare_reports_first_mismatch_row_from_materialized_frames() {
    // 200 rows so the mismatch lands past the first block.
    let mut reference_a = Vec::new();
    let mut reference_b = Vec::new();
    for row in 0..200 {
        reference_a.push(Some(row as f64));
        reference_b.push(Some(10.0 + row as f64));
    }
    let reference_frame = frame_values(&reference_a, &reference_b);
    let golden = common::golden_doc(digest_of(&reference_frame));

    let dir = tempfile::tempdir().unwrap();
    frame_golden::frames::write_frame(
        &dir.path().join("frame.parquet"),
        std::slice::from_ref(&reference_frame),
    )
    .unwrap();

    let mut actual_a = reference_a.clone();
    actual_a[150] = Some(f64::from_bits(actual_a[150].unwrap().to_bits() ^ 0x10));
    let fresh = frame_values(&actual_a, &reference_b);

    let report = compare_products(
        "first_row",
        &golden,
        &common::frame_map(fresh),
        &common::fresh_metrics(),
        Some(&common::fresh_health()),
        &None,
        Some(dir.path()),
    )
    .unwrap();

    let mismatch = report
        .row_mismatch
        .as_ref()
        .expect("materialized frames must produce a first-mismatch-row report");
    assert_eq!(mismatch.column, "frame.A (kW)");
    assert_eq!(mismatch.row, 150);
    assert!(
        mismatch.expected.contains("0x"),
        "both values are reported with bit patterns: {}",
        mismatch.expected
    );
    let cells = &mismatch.per_column_cells;
    assert!(cells.contains(&("A (kW)".to_string(), 1)));
    assert!(cells.contains(&("B (kW)".to_string(), 0)));
}

#[test]
fn delta_reports_per_column_max_abs_difference() {
    let reference_frame = frame_values(
        &[Some(1.0), Some(2.0), Some(3.0)],
        &[Some(10.0), None, Some(30.0)],
    );
    let golden = common::golden_doc(digest_of(&reference_frame));

    let dir = tempfile::tempdir().unwrap();
    frame_golden::frames::write_frame(
        &dir.path().join("frame.parquet"),
        std::slice::from_ref(&reference_frame),
    )
    .unwrap();

    let fresh = frame_values(
        &[Some(1.0), Some(2.5), Some(4.0)],
        &[Some(10.0), Some(20.0), Some(33.0)],
    );

    let report = delta_products(
        "delta_stats",
        &golden,
        dir.path(),
        &common::frame_map(fresh),
        &common::fresh_metrics(),
        &None,
    )
    .unwrap();

    assert!(!report.is_identical());
    let a = report
        .columns
        .iter()
        .find(|column| column.column == "A (kW)")
        .expect("column A differs");
    assert_eq!(a.differing_cells, 2);
    assert_eq!(a.max_abs_difference, Some(1.0));
    assert_eq!(a.max_relative_difference, Some(0.25));
    let b = report
        .columns
        .iter()
        .find(|column| column.column == "B (kW)")
        .expect("column B differs");
    assert_eq!(b.differing_cells, 2);
    assert_eq!(
        b.validity_changes, 1,
        "row 1 gains a value the reference lacks"
    );
    assert_eq!(b.max_abs_difference, Some(3.0));
}

#[test]
fn delta_without_materialized_frames_names_materialize() {
    let reference_frame = frame_values(&[Some(1.0)], &[Some(2.0)]);
    let golden = common::golden_doc(digest_of(&reference_frame));

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent");
    let error = delta_products(
        "delta_missing",
        &golden,
        &missing,
        &common::frame_map(reference_frame),
        &common::fresh_metrics(),
        &None,
    )
    .expect_err("delta without materialized frames must fail");

    let message = error.to_string();
    assert!(
        message.contains("materialize"),
        "the error must name materialize: {message}"
    );
}

/// The fleet products carry Int64 and Boolean columns: a differing cell in
/// either must produce the row-level report naming the column and row, not
/// an error about the column's type.
#[test]
fn row_level_report_covers_int64_and_boolean_columns() {
    use std::sync::Arc;

    use arrow::array::{ArrayRef, BooleanArray, Int64Array};

    let schema = Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, false),
        Field::new("failed", DataType::Boolean, false),
    ]);
    let batch_of = |ids: &[i64], failed: &[bool]| -> arrow::record_batch::RecordBatch {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(ids.to_vec())),
            Arc::new(BooleanArray::from(failed.to_vec())),
        ];
        common::batch(schema.clone(), columns)
    };
    let reference_batches = vec![
        batch_of(&[1, 2], &[false, false]),
        batch_of(&[3, 4], &[false, false]),
    ];
    let golden = common::golden_doc(digest_map_of(&reference_batches));

    let dir = tempfile::tempdir().unwrap();
    frame_golden::frames::write_frame(&dir.path().join("frame.parquet"), &reference_batches)
        .unwrap();

    // One Int64 cell differs (row 2, in the second batch).
    let fresh_batches = vec![
        reference_batches[0].clone(),
        batch_of(&[30, 4], &[false, false]),
    ];
    let report = compare_products(
        "int64_boolean",
        &golden,
        &fresh_map_of(fresh_batches),
        &common::fresh_metrics(),
        Some(&common::fresh_health()),
        &None,
        Some(dir.path()),
    )
    .expect("an Int64 difference must compare at the value level, not error");
    let mismatch = report
        .row_mismatch
        .as_ref()
        .expect("the Int64 difference must produce a row-level report");
    assert_eq!(mismatch.column, "frame.bldg_id");
    assert_eq!(mismatch.row, 2);

    // One Boolean cell differs (row 1, in the first batch): the same
    // report path.
    let fresh_batches = vec![
        batch_of(&[1, 2], &[false, true]),
        reference_batches[1].clone(),
    ];
    let report = compare_products(
        "int64_boolean",
        &golden,
        &fresh_map_of(fresh_batches),
        &common::fresh_metrics(),
        Some(&common::fresh_health()),
        &None,
        Some(dir.path()),
    )
    .expect("a Boolean difference must compare at the value level, not error");
    let mismatch = report
        .row_mismatch
        .as_ref()
        .expect("the Boolean difference must produce a row-level report");
    assert_eq!(mismatch.column, "frame.failed");
    assert_eq!(mismatch.row, 1);
}

/// A fresh run carrying a nested metrics key the golden lacks (one
/// per-end-use entry) must fail compare, naming the key's full path: the
/// extra-key sweep runs at every recursion level of the metrics maps.
#[test]
fn metrics_extra_nested_key_is_rejected() {
    use frame_golden::golden::{MetricField, MetricValue, MetricsRow};

    let field = |value: f64| {
        MetricValue::Field(MetricField {
            bits: Some(format!("0x{:016x}", value.to_bits())),
            value: Some(value),
        })
    };
    let per_end_use = |keys: &[&str]| {
        let mut map = MetricsRow::new();
        for key in keys {
            map.insert(key.to_string(), field(10.0));
        }
        map
    };

    let frame = frame_values(&[Some(1.0)], &[Some(2.0)]);
    let mut golden = common::golden_doc(digest_of(&frame));
    let mut golden_row = MetricsRow::new();
    golden_row.insert(
        "total_energy_kwh.per_end_use".to_string(),
        MetricValue::Map(per_end_use(&["hvac"])),
    );
    golden.metrics = vec![golden_row];

    let mut fresh_row = MetricsRow::new();
    let mut fresh_end_use = per_end_use(&["hvac"]);
    fresh_end_use.insert("lighting".to_string(), field(2.0));
    fresh_row.insert(
        "total_energy_kwh.per_end_use".to_string(),
        MetricValue::Map(fresh_end_use),
    );

    let report = compare_products(
        "nested_extra",
        &golden,
        &common::frame_map(frame),
        &[fresh_row],
        Some(&common::fresh_health()),
        &None,
        None,
    )
    .unwrap();

    assert!(
        !report.is_identical(),
        "a fresh run carrying a nested key the golden lacks must fail compare: {report:?}"
    );
    assert!(
        report.differences.iter().any(|difference| matches!(
            difference,
            frame_golden::Difference::MetricsField { field, .. }
                if field.contains("total_energy_kwh.per_end_use.lighting")
        )),
        "the difference must name the nested path: {report:?}"
    );
}
