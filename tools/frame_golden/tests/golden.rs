//! Golden-document acceptance tests: one capture's bytes are stable across
//! captures, and a one-day manifest captured into a temporary directory
//! compares identical against a fresh run.

mod common;

use std::path::PathBuf;

use frame_golden::adapter::{RunOutput, RunRequest};
use frame_golden::capture;
use frame_golden::compare::compare_products;
use frame_golden::manifest::{GoldenManifest, repo_root};

/// A one-day, hourly manifest against the cz4a parity fixture: 24 rows,
/// master seed 0, no initialization (the warm-up loop still runs, at the
/// hourly resolution, and converges in a few day-replays).
///
/// The feature set names the running binary's own: these tests exercise
/// capture stability and the round trip, not the feature gate, so the
/// manifest must be accepted whatever the build flags.
fn one_day_manifest() -> String {
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
"#
    )
}

fn write_manifest(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("one_day.toml");
    std::fs::write(&path, one_day_manifest()).unwrap();
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

#[test]
fn golden_json_is_byte_stable() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir);

    let first = capture::capture(&manifest_path, &repo_root(&manifest_path), None).unwrap();
    let first_bytes = std::fs::read(&first.golden_path).unwrap();

    let second = capture::capture(&manifest_path, &repo_root(&manifest_path), None).unwrap();
    let second_bytes = std::fs::read(&second.golden_path).unwrap();

    assert_eq!(
        first_bytes, second_bytes,
        "two captures of one run must write identical bytes"
    );
    // Sorted keys, visible provenance: the document parses and pins the
    // feature set and defaults directory it was captured under. The feature
    // set is the running binary's, matching the manifest.
    assert_eq!(
        first.doc.features,
        frame_golden::manifest::running_features()
    );
    assert_eq!(first.doc.defaults_dir, "defaults");
}

#[test]
fn capture_then_compare_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(&dir);
    let frames_dir = dir.path().join("frames");

    let captured = capture::capture(
        &manifest_path,
        &repo_root(&manifest_path),
        Some(&frames_dir),
    )
    .unwrap();

    // A fresh run against the captured golden: identical.
    let products = run_products(&manifest_path);
    let report = compare_products(
        "one_day",
        &captured.doc,
        &products.frames,
        &products.metrics_rows,
        products.health.as_ref(),
        &None,
        Some(&frames_dir),
    )
    .unwrap();
    assert!(
        report.is_identical(),
        "round trip must be identical: {report:?}"
    );

    // The materialized frames from the capture verified against the
    // committed digests, so the row-level report was actually exercised:
    // a note would mean the row-level comparison never ran.
    assert!(report.row_mismatch.is_none());
    assert!(
        report.notes.is_empty(),
        "the row-level path must have run without a note: {report:?}"
    );

    // And the full products are on disk for delta.
    let delta = frame_golden::compare::delta_products(
        "one_day",
        &captured.doc,
        &frames_dir,
        &products.frames,
        &products.metrics_rows,
        &None,
    )
    .unwrap();
    assert!(
        delta.is_identical(),
        "delta against itself must be empty: {delta:?}"
    );
}
