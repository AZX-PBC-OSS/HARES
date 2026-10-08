//! Golden-document acceptance tests: one capture's bytes are stable across
//! captures, and a one-day manifest captured into a temporary directory
//! compares identical against a fresh run.

mod common;

use std::path::PathBuf;

use arrow::array::{Float64Array, Int64Array};

use frame_golden::adapter::{RunOutput, RunRequest};
use frame_golden::capture;
use frame_golden::compare::{compare_run, delta_run};
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

    // A fresh run against the captured golden: identical, through the
    // command layer's own composition so the digest check is exercised on
    // a matching tree too.
    let products = run_products(&manifest_path);
    let report = compare_run(
        "one_day",
        &captured.doc,
        &products,
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
    let delta = delta_run("one_day", &captured.doc, &frames_dir, &products, &None).unwrap();
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

/// A one-day, hourly fleet manifest with two homes built from the same
/// resstock fixture and weighted 1.0 and 3.0: the manifest's weights must
/// be the weights the run reports, in home order, so the same building
/// twice is told apart by its weight rather than its identity.
fn weighted_fleet_manifest() -> String {
    let features = frame_golden::manifest::running_features()
        .into_iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let home = |weight: f64| {
        format!(
            r#"
[[home]]
bldg_id = 2
hpxml = "tests/fixtures/resstock/2025.1/bldg0000002/home.xml"
schedule = "tests/fixtures/resstock/2025.1/bldg0000002/in.schedules.csv"
weather = "tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv"
initialization_duration_s = 0
weight = {weight:.1}
overrides = {{}}
"#
        )
    };
    format!(
        r#"
kind = "fleet"
features = [{features}]
defaults = "defaults"
resolution = "hourly"

[simulation]
start_time = "2018-01-01T00:00:00-05:00"
duration = 86400
time_res = 3600
output_verbosity = 2
master_seed = 0
{}
{}
"#,
        home(1.0),
        home(3.0)
    )
}

#[test]
fn fleet_manifest_weights_reach_the_weights_product() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("fleet_weights.toml");
    std::fs::write(&manifest_path, weighted_fleet_manifest()).unwrap();

    let products = run_products(&manifest_path);
    let weights = products
        .frames
        .get("weights")
        .expect("a fleet run produces a weights product");
    let batch = weights
        .batches
        .first()
        .expect("the weights product has a batch");
    let bldg_ids = batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("bldg_id is an Int64 column");
    let sample_weights = batch
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("sample_weight is a Float64 column");
    assert_eq!(bldg_ids.len(), 2, "one weights row per home");
    assert_eq!(bldg_ids.value(0), 2, "rows are in home order");
    assert_eq!(bldg_ids.value(1), 2, "rows are in home order");
    assert_eq!(sample_weights.value(0), 1.0, "the first home's weight");
    assert_eq!(sample_weights.value(1), 3.0, "the second home's weight");
}

/// Runs git in the fixture repository `dir` with an empty configuration of
/// its own: the host's global and system configuration (a commit signer, a
/// hooks path) never reach the fixture, so its setup cannot fail on them.
fn git(dir: &std::path::Path, args: &[&str]) {
    let empty_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", empty_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// A capture's provenance describes the engine inputs: golden documents
/// another capture in the same session rewrote do not make the tree dirty,
/// while any other change does.
#[test]
fn golden_documents_do_not_dirty_the_capture_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    let golden_dir = root.join("tests/fixtures/golden");
    std::fs::create_dir_all(&golden_dir).unwrap();
    std::fs::write(golden_dir.join("a.golden.json"), "{}\n").unwrap();
    std::fs::write(golden_dir.join("a.toml"), "kind = \"dwelling\"\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    );

    std::fs::write(golden_dir.join("a.golden.json"), "{\"recaptured\": true}\n").unwrap();
    assert_eq!(frame_golden::golden::git_info(root).1, Some(false));

    std::fs::write(golden_dir.join("a.toml"), "kind = \"fleet\"\n").unwrap();
    assert_eq!(frame_golden::golden::git_info(root).1, Some(true));
}
