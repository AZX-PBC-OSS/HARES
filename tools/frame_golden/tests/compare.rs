//! Compare and delta acceptance tests: schema rejection with and without
//! column selection, first-mismatch-row reporting from materialized
//! frames, per-column delta statistics, the materialize pointer in the
//! missing-frames error, and the defaults-digest difference reported by
//! compare, materialize and delta.

mod common;

use std::collections::BTreeMap;

use frame_golden::compare::{compare_products, compare_run, delta_products, delta_run};
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

/// A one-day, hourly cz4a manifest whose `[defaults_files]` replacement
/// source is the absolute path of a tempfile copy of a committed defaults
/// file: `repo_path` joins an absolute path as given, so the manifest
/// points outside the tree and the test writes nothing into it.
fn one_day_defaults_replacement_manifest(replacement: &std::path::Path) -> String {
    let features = frame_golden::manifest::running_features()
        .into_iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
kind = "dwelling"
features = [{features}]
defaults = "defaults"

[defaults_files]
"zip_parameters.toml" = "{}"

[simulation]
start_time = "2023-01-01T00:00:00-07:00"
duration = 86400
time_res = 3600
output_verbosity = 2
master_seed = 0

[[home]]
bldg_id = 1
hpxml = "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml"
schedule = "tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv"
weather = "tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw"
initialization_duration_s = 0
overrides = {{}}
"#,
        replacement.display()
    )
}

/// Sets up the defaults-digest fixture: a tempfile copy of the committed
/// `defaults/zip_parameters.toml` wired into the manifest's
/// `[defaults_files]`, captured into the fixture tempdir. The tempdirs are
/// dropped by the caller when the test ends.
fn captured_defaults_replacement(
    fixture_dir: &tempfile::TempDir,
    replacement_dir: &tempfile::TempDir,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    frame_golden::GoldenDoc,
) {
    let manifest_path = fixture_dir.path().join("defaults_replacement.toml");
    let root = frame_golden::manifest::repo_root(&manifest_path);
    let replacement = replacement_dir.path().join("zip_parameters.toml");
    std::fs::copy(
        root.join("defaults").join("zip_parameters.toml"),
        &replacement,
    )
    .unwrap();
    std::fs::write(
        &manifest_path,
        one_day_defaults_replacement_manifest(&replacement),
    )
    .unwrap();
    let captured = frame_golden::capture::capture(&manifest_path, &root, None).unwrap();
    (root, manifest_path, replacement, captured.doc)
}

/// Changes one byte of the replacement file. The file's first line must
/// be a comment, so the substituted byte sits outside every parsed value
/// and every product stays identical while the tree digest differs.
fn change_one_comment_byte(replacement: &std::path::Path) {
    let mut bytes = std::fs::read(replacement).unwrap();
    let first_line = bytes
        .iter()
        .position(|b| *b == b'\n')
        .expect("the defaults file has a first line");
    assert_eq!(
        bytes[0], b'#',
        "the first line must stay a comment for the substitution to be inert"
    );
    let position = bytes[..first_line]
        .iter()
        .position(|b| *b == b'Z')
        .expect("a comment byte to change");
    bytes[position] = b'Y';
    std::fs::write(replacement, &bytes).unwrap();
}

fn run_manifest_products(manifest_path: &std::path::Path) -> frame_golden::RunProducts {
    let manifest = frame_golden::manifest::GoldenManifest::load(manifest_path).unwrap();
    frame_golden::adapter::run(frame_golden::RunRequest {
        repo_root: &frame_golden::manifest::repo_root(manifest_path),
        manifest: &manifest,
        output: frame_golden::RunOutput::Full,
        duration_override_s: None,
    })
    .unwrap()
}

/// The acceptance case: a defaults edit that leaves every column of the
/// fixture unchanged must still fail compare, naming both digests.
#[test]
fn compare_fails_on_defaults_digest_mismatch() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let (_root, manifest_path, replacement, doc) =
        captured_defaults_replacement(&fixture_dir, &replacement_dir);

    // Unchanged: the replacement is a byte-identical copy of the
    // committed file, the fresh run read the same tree the capture did.
    let products = run_manifest_products(&manifest_path);
    let report = compare_run("defaults_replacement", &doc, &products, &None, None).unwrap();
    assert!(
        report.is_identical(),
        "an unchanged defaults tree must compare identical: {report:?}"
    );

    change_one_comment_byte(&replacement);
    let products = run_manifest_products(&manifest_path);
    let report = compare_run("defaults_replacement", &doc, &products, &None, None).unwrap();
    assert!(
        !report.is_identical(),
        "a defaults-digest mismatch must fail the compare: {report:?}"
    );
    // The substituted byte is inert: the digest difference is the only
    // one, which is the case nothing else catches.
    assert_eq!(
        report.differences.len(),
        1,
        "a digest-only change must produce only the digest difference: {report:?}"
    );
    assert!(
        report.row_mismatch.is_none(),
        "a digest-only change must produce no row-level report: {report:?}"
    );
    let (defaults_dir, expected, actual) = report
        .differences
        .iter()
        .find_map(|difference| match difference {
            frame_golden::Difference::DefaultsDigest {
                defaults_dir,
                expected,
                actual,
            } => Some((defaults_dir, expected, actual)),
            _ => None,
        })
        .expect("the report must carry the defaults-digest difference");
    assert_eq!(defaults_dir, "defaults");
    assert_eq!(expected, &doc.defaults_digest);
    assert_eq!(actual, &products.defaults_digest);
    assert_ne!(expected, actual, "the two digests must differ");
}

/// The same changed replacement makes materialize refuse with the
/// defaults-digest message and write no full product.
#[test]
fn materialize_refuses_defaults_digest_mismatch() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let (root, manifest_path, replacement, doc) =
        captured_defaults_replacement(&fixture_dir, &replacement_dir);
    let frames_dir = fixture_dir.path().join("frames");

    // Unchanged: materialize writes the full products.
    frame_golden::capture::materialize(&manifest_path, &root, &frames_dir)
        .expect("an unchanged defaults tree must materialize");
    assert!(
        frames_dir.exists(),
        "the unchanged materialize must write the full products"
    );

    std::fs::remove_dir_all(&frames_dir).unwrap();
    change_one_comment_byte(&replacement);
    let materialized = frame_golden::capture::materialize(&manifest_path, &root, &frames_dir);
    let Err(error) = materialized else {
        panic!("a defaults-digest mismatch must refuse to materialize");
    };
    let message = error.to_string();
    let fresh = run_manifest_products(&manifest_path);
    assert!(
        message.contains("digest mismatch"),
        "the refusal must be the defaults-digest message: {message}"
    );
    assert!(
        message.contains(&format!("defaults {:?}", doc.defaults_dir)),
        "the refusal must name the defaults directory: {message}"
    );
    assert!(
        message.contains(&doc.defaults_digest),
        "the refusal must name the golden's digest: {message}"
    );
    assert!(
        message.contains(&fresh.defaults_digest),
        "the refusal must name the run's digest: {message}"
    );
    assert!(
        !frames_dir.exists(),
        "no full product may be written on refusal"
    );
}

/// Delta reports a defaults-digest change in its notes and succeeds: the
/// change is pointed at, not failed on, and an unchanged tree adds no
/// note.
#[test]
fn delta_reports_defaults_digest_change_without_failing() {
    let fixture_dir = tempfile::tempdir().unwrap();
    let replacement_dir = tempfile::tempdir().unwrap();
    let (root, manifest_path, replacement, doc) =
        captured_defaults_replacement(&fixture_dir, &replacement_dir);
    let frames_dir = fixture_dir.path().join("frames");

    // The capture's full products, for delta to compare against.
    frame_golden::capture::materialize(&manifest_path, &root, &frames_dir).unwrap();

    // Unchanged: no note.
    let fresh = run_manifest_products(&manifest_path);
    let report = delta_run("delta_digest", &doc, &frames_dir, &fresh, &None)
        .expect("an unchanged defaults tree must not fail the delta");
    assert!(
        report.notes.is_empty(),
        "an unchanged defaults tree must not be reported: {report:?}"
    );

    change_one_comment_byte(&replacement);
    let fresh = run_manifest_products(&manifest_path);
    let report = delta_run("delta_digest", &doc, &frames_dir, &fresh, &None)
        .expect("a defaults-digest change must not fail the delta");
    // The substituted byte is inert: the delta's value level is empty and
    // the change is reported as the digest note only.
    assert!(
        report.is_identical(),
        "a digest-only change must leave every value identical: {report:?}"
    );
    assert!(
        report.notes.iter().any(
            |note| note.contains(&doc.defaults_digest) && note.contains(&fresh.defaults_digest)
        ),
        "the delta must report the defaults-digest change with both digests: {report:?}"
    );
}
