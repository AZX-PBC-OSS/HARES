//! Golden-document acceptance tests: one capture's bytes are stable across
//! captures, and a one-day manifest captured into a temporary directory
//! compares identical against a fresh run.

mod common;

use std::path::PathBuf;

use frame_golden::adapter::{RunOutput, RunRequest};
use frame_golden::capture;
use frame_golden::compare::compare_products;
use frame_golden::manifest::{GoldenManifest, repo_root};
use hares_core::{RunHealth, WarmupOutcome};

/// A one-day, hourly manifest against the cz4a parity fixture: 24 rows,
/// master seed 0, no initialization (`initialization_duration_s = 0` runs
/// no warm-up, as the Python binding's rule has it).
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

/// A one-day, hourly manifest against the cz2a parity fixture, with the
/// warm-up setting passed through: the acceptance tests for the manifest's
/// `initialization_duration_s` rule run the same inputs with `0` and with
/// a positive value.
fn cz2a_one_day_manifest(initialization_duration_s: u64) -> String {
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
hpxml = "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/building.xml"
schedule = "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/schedule.csv"
weather = "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/weather.epw"
initialization_duration_s = {initialization_duration_s}
overrides = {{}}
"#
    )
}

fn run_cz2a_one_day(initialization_duration_s: u64) -> RunHealth {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("cz2a_one_day.toml");
    std::fs::write(
        &manifest_path,
        cz2a_one_day_manifest(initialization_duration_s),
    )
    .unwrap();

    let products = run_products(&manifest_path);
    serde_json::from_value(products.health.expect("dwelling run records health")).unwrap()
}

#[test]
fn manifest_zero_initialization_duration_runs_no_warmup() {
    let health = run_cz2a_one_day(0);
    assert_eq!(
        health.warmup,
        WarmupOutcome::Disabled,
        "initialization_duration_s = 0 must run no warm-up"
    );
}

#[test]
fn manifest_positive_initialization_duration_runs_warmup() {
    let health = run_cz2a_one_day(86400);
    assert!(
        matches!(health.warmup, WarmupOutcome::Ran { .. }),
        "a positive initialization_duration_s must run the warm-up: {:?}",
        health.warmup
    );
}
