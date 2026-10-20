//! The `resstock_fleet` kind: the manifest keys are required and
//! kind-policed, a run reports the fleet's construction beside the
//! simulation products, and the capture round trip fails a corrupted
//! frame and passes the restored one.

mod common;

use std::sync::Arc;

use frame_golden::adapter::{RunOutput, RunRequest};
use frame_golden::capture;
use frame_golden::compare::compare_run;
use frame_golden::manifest::{GoldenManifest, ManifestKind, repo_root};

fn resstock_fleet_manifest(duration_s: u64) -> String {
    let features = frame_golden::manifest::running_features()
        .into_iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
kind = "resstock_fleet"
features = [{features}]
defaults = "python/hares/defaults"
metadata = "tests/fixtures/resstock/2025.1/metadata_fleet3.parquet"
buildings_dir = "tests/fixtures/resstock/2025.1"
weather_dir = "tests/fixtures/resstock/2025.1/weather"
version = "2025.1"
duration = {duration_s}

[fleet]
frequency = "hourly"
selection = "verbosity-2"
"#
    )
}

fn write_manifest(dir: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
    let path = dir.path().join("resstock_fleet.toml");
    std::fs::write(&path, text).unwrap();
    path
}

fn run_products(manifest_path: &std::path::Path) -> frame_golden::RunProducts {
    let manifest = GoldenManifest::load(manifest_path).unwrap();
    frame_golden::adapter::run(RunRequest {
        repo_root: &repo_root(manifest_path),
        manifest: &manifest,
        output: RunOutput::Full,
        duration_override_s: None,
    })
    .unwrap()
}

fn int_column(batch: &arrow::record_batch::RecordBatch, name: &str) -> arrow::array::Int64Array {
    let index = batch
        .schema()
        .fields()
        .iter()
        .position(|field| field.name() == name)
        .expect("the column exists");
    batch
        .column(index)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .expect("the column is Int64")
        .clone()
}

#[test]
fn resstock_fleet_manifest_parses_with_its_keys() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir, &resstock_fleet_manifest(172800));
    let manifest = GoldenManifest::load(&manifest_path).unwrap();

    assert_eq!(manifest.kind, ManifestKind::ResstockFleet);
    assert_eq!(
        manifest.metadata.as_deref(),
        Some("tests/fixtures/resstock/2025.1/metadata_fleet3.parquet")
    );
    assert_eq!(
        manifest.buildings_dir.as_deref(),
        Some("tests/fixtures/resstock/2025.1")
    );
    assert_eq!(
        manifest.weather_dir.as_deref(),
        Some("tests/fixtures/resstock/2025.1/weather")
    );
    assert_eq!(manifest.version, Some(hares_io::ResStockVersion::V2025_1));
    assert_eq!(manifest.duration, Some(172800));
    let fleet = manifest.fleet.as_ref().expect("the [fleet] table parses");
    assert_eq!(fleet.frequency, hares_io::Frequency::Hourly);
    assert_eq!(fleet.selection.verbosity(), 2);
    assert!(
        manifest.home.is_empty(),
        "the kind takes no [[home]] entries"
    );
}

#[test]
fn resstock_fleet_manifest_requires_every_key() {
    for key in [
        "metadata",
        "buildings_dir",
        "weather_dir",
        "version",
        "duration",
    ] {
        let text = resstock_fleet_manifest(172800)
            .lines()
            .filter(|line| !line.starts_with(key))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = write_manifest(&dir, &text);
        let error = GoldenManifest::load(&manifest_path)
            .expect_err("a manifest missing a required key must be rejected");
        assert!(
            error.to_string().contains(key),
            "the error must name the missing key {key}: {error}"
        );
    }

    // A missing [fleet] table: the error names the table.
    let text = resstock_fleet_manifest(172800);
    let without_fleet = text[..text.find("[fleet]").unwrap()].to_string();
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir, &without_fleet);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a manifest without a [fleet] table must be rejected");
    assert!(
        error.to_string().contains("fleet"),
        "the error must name the table: {error}"
    );
}

#[test]
fn resstock_fleet_manifest_rejects_homes_and_fleet_keys() {
    let dir = tempfile::tempdir().unwrap();

    // A [[home]] entry: the homes come from the metadata.
    let with_home = format!(
        "{}\n[[home]]\nbldg_id = 1\nhpxml = \"a.xml\"\nschedule = \"a.csv\"\nweather = \"a.epw\"\nweight = 1.0\noverrides = {{}}\n",
        resstock_fleet_manifest(172800)
    );
    let manifest_path = write_manifest(&dir, &with_home);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a resstock fleet manifest with a [[home]] must be rejected");
    assert!(
        error.to_string().contains("[[home]]"),
        "the error must state the rule: {error}"
    );

    // The top-level resolution key: the [fleet] frequency carries it.
    let with_resolution = resstock_fleet_manifest(172800).replace(
        "duration = 172800",
        "duration = 172800\nresolution = \"hourly\"",
    );
    let manifest_path = write_manifest(&dir, &with_resolution);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a resstock fleet manifest with a resolution must be rejected");
    assert!(
        error.to_string().contains("resolution"),
        "the error must name the key: {error}"
    );

    // A zero duration: positive.
    let manifest_path = write_manifest(&dir, &resstock_fleet_manifest(0));
    let error = GoldenManifest::load(&manifest_path).expect_err("a zero duration must be rejected");
    assert!(
        error.to_string().contains("positive"),
        "the error must state the rule: {error}"
    );
}

#[test]
fn dwelling_manifest_rejects_resstock_keys() {
    let dir = tempfile::tempdir().unwrap();

    let with_metadata = format!(
        "{}\nmetadata = \"tests/fixtures/resstock/2025.1/metadata_fleet3.parquet\"\n",
        common::dwelling_manifest()
    );
    let manifest_path = write_manifest(&dir, &with_metadata);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a dwelling manifest with a metadata key must be rejected");
    assert!(
        error.to_string().contains("resstock_fleet"),
        "the error must name the accepting kind: {error}"
    );

    let with_fleet_table = format!(
        "{}\n[fleet]\nfrequency = \"hourly\"\nselection = \"verbosity-2\"\n",
        common::dwelling_manifest()
    );
    let manifest_path = write_manifest(&dir, &with_fleet_table);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a dwelling manifest with a [fleet] table must be rejected");
    assert!(
        error.to_string().contains("resstock_fleet"),
        "the error must name the accepting kind: {error}"
    );
}

#[test]
fn resstock_fleet_manifest_rejects_unknown_spellings() {
    let dir = tempfile::tempdir().unwrap();

    let unknown_version =
        resstock_fleet_manifest(172800).replace("version = \"2025.1\"", "version = \"2024.99\"");
    let manifest_path = write_manifest(&dir, &unknown_version);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("an unknown release spelling must be rejected");
    assert!(
        error.to_string().contains("2024.99"),
        "the error must name the value: {error}"
    );

    let unknown_selection = resstock_fleet_manifest(172800)
        .replace("selection = \"verbosity-2\"", "selection = \"named\"");
    let manifest_path = write_manifest(&dir, &unknown_selection);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("an unknown selection spelling must be rejected");
    assert!(
        error.to_string().contains("verbosity-<level>"),
        "the error must name the accepted form: {error}"
    );

    let excessive_selection = resstock_fleet_manifest(172800)
        .replace("selection = \"verbosity-2\"", "selection = \"verbosity-9\"");
    let manifest_path = write_manifest(&dir, &excessive_selection);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a selection above the maximum must be rejected");
    assert!(
        error.to_string().contains("exceeds the maximum"),
        "the error must state the bound: {error}"
    );

    let unknown_frequency = resstock_fleet_manifest(172800)
        .replace("frequency = \"hourly\"", "frequency = \"fortnightly\"");
    let manifest_path = write_manifest(&dir, &unknown_frequency);
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("an unknown frequency spelling must be rejected");
    assert!(
        error.to_string().contains("fortnightly"),
        "the error must name the value: {error}"
    );
}

/// The run's products: the construction report (the fixture's three
/// buildings, all weather files resolved), the metadata's weights on the
/// homes, and real per-home energies.
#[test]
fn resstock_fleet_run_reports_the_construction() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir, &resstock_fleet_manifest(3600));

    let products = run_products(&manifest_path);
    let report = products
        .frames
        .get("fleet_report")
        .expect("a resstock fleet run carries its construction report");
    let batch = report.batches.first().expect("one report batch");
    assert_eq!(batch.num_rows(), 1, "one report row");
    assert_eq!(int_column(batch, "buildings_parsed").value(0), 3);
    assert_eq!(int_column(batch, "dwellings_built").value(0), 3);
    assert_eq!(int_column(batch, "weather_resolved").value(0), 3);
    assert_eq!(int_column(batch, "weather_failed").value(0), 0);

    // The homes table: one row per home, in metadata order, each home's
    // metadata weight.
    let homes = products.frames.get("homes").expect("the homes product");
    let batch = homes.batches.first().unwrap();
    let ids = int_column(batch, "bldg_id");
    assert_eq!(ids.value(0), 19694, "rows in metadata order");
    assert_eq!(ids.value(1), 162486);
    assert_eq!(ids.value(2), 352714);
    let weights = batch
        .column(1)
        .as_any()
        .downcast_ref::<arrow::array::Float64Array>()
        .unwrap();
    assert_eq!(
        weights.value(0),
        253.90367272727272,
        "the weight comes from the metadata's weight column"
    );
    let statuses = batch
        .column(2)
        .as_any()
        .downcast_ref::<arrow::array::StringArray>()
        .unwrap();
    for row in 0..3 {
        assert_ne!(
            statuses.value(row),
            "failed",
            "a warning is not a failure: the fixture homes run"
        );
    }
}

/// The capture round trip: two captures write identical bytes, a fresh run
/// compares identical, a corrupted frame fails the compare and the
/// restored frame passes it.
#[test]
fn resstock_fleet_capture_compares_and_detects_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir, &resstock_fleet_manifest(3600));
    let root = repo_root(&manifest_path);
    let frames_dir = dir.path().join("frames");

    let first = capture::capture(&manifest_path, &root, Some(&frames_dir)).unwrap();
    let first_bytes = std::fs::read(&first.golden_path).unwrap();
    let second = capture::capture(&manifest_path, &root, None).unwrap();
    let second_bytes = std::fs::read(&second.golden_path).unwrap();
    assert_eq!(
        first_bytes, second_bytes,
        "two captures of one run must write identical bytes"
    );
    assert_eq!(first.doc.kind, "resstock_fleet");

    // A fresh run against the captured golden: identical.
    let products = run_products(&manifest_path);
    let report = compare_run("resstock_fleet", &first.doc, &products, &None, None).unwrap();
    assert!(
        report.is_identical(),
        "the round trip must be identical: {report:?}"
    );

    // The mutation: one corrupted cell in the homes product must fail the
    // compare, naming the product and column.
    let mut corrupted_frames = std::collections::BTreeMap::new();
    for (name, frame) in &products.frames {
        if name != "homes" {
            corrupted_frames.insert(name.clone(), frame.clone());
        }
    }
    let homes = products.frames.get("homes").unwrap();
    let batch = homes.batches.first().unwrap();
    let weight: arrow::array::Float64Array = batch
        .column(1)
        .as_any()
        .downcast_ref::<arrow::array::Float64Array>()
        .unwrap()
        .clone();
    let flipped = f64::from_bits(weight.value(0).to_bits() ^ 0x10);
    let mutated: arrow::array::ArrayRef = Arc::new({
        let mut builder = arrow::array::Float64Builder::new();
        for row in 0..weight.len() {
            if row == 0 {
                builder.append_value(flipped);
            } else {
                builder.append_value(weight.value(row));
            }
        }
        builder.finish()
    });
    let mut columns: Vec<arrow::array::ArrayRef> = batch.columns().to_vec();
    columns[1] = mutated;
    let mutated_batch = arrow::record_batch::RecordBatch::try_new(batch.schema(), columns).unwrap();
    corrupted_frames.insert(
        "homes".to_string(),
        frame_golden::adapter::FrameProducts::from_batches(vec![mutated_batch]).unwrap(),
    );
    let corrupted = frame_golden::RunProducts {
        frames: corrupted_frames,
        ..products.clone()
    };
    let report = compare_run("resstock_fleet", &first.doc, &corrupted, &None, None).unwrap();
    assert!(
        !report.is_identical(),
        "a corrupted frame must fail the compare: {report:?}"
    );
    assert!(
        report.differences.iter().any(|difference| matches!(
            difference,
            frame_golden::Difference::ColumnDigest { product, column, .. }
                if product == "homes" && column == "weight"
        )),
        "the difference must name the corrupted product and column: {report:?}"
    );

    // Restored: the untouched products compare identical again.
    let report = compare_run("resstock_fleet", &first.doc, &products, &None, None).unwrap();
    assert!(
        report.is_identical(),
        "the restored run must compare identical: {report:?}"
    );
}
