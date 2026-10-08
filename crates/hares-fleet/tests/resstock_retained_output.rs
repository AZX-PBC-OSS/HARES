//! The output-mode builder: a ResStock fleet's homes run with their
//! recorders off (no destination exists at construction), so the
//! recorder-on run mode and the retained batches it produces are the
//! builder's own acceptance.

use std::path::PathBuf;

use hares_fleet::Fleet;
use hares_io::ResStockVersion;

fn fleet() -> hares_fleet::Fleet {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = root.join("tests/fixtures/resstock/2025.1");
    Fleet::from_resstock(
        &fixtures.join("metadata_fleet3.parquet"),
        &fixtures,
        &fixtures.join("weather"),
        Some(ResStockVersion::V2025_1),
        None,
        Some(chrono::Duration::seconds(3600)),
        Some(&root.join("defaults")),
    )
    .expect("fleet construction")
}

/// Without the builder the homes run with the constructor's own sim
/// config: the recorder off, no retained batches, run metrics empty.
#[test]
fn resstock_fleet_runs_without_a_recorder_by_default() {
    let outcomes = fleet().simulate(1);
    for outcome in outcomes {
        let outcome = outcome.expect("the fixture homes run");
        assert!(
            !matches!(outcome.status, hares_fleet::SimStatus::Failed(_)),
            "a missing recorder is not a run failure: {:?}",
            outcome.status
        );
        assert!(
            outcome
                .result
                .timeseries
                .as_ref()
                .is_none_or(|batches| batches.is_empty()),
            "without a recorder no batches are flushed"
        );
        assert_eq!(
            outcome.result.metrics.total_energy_kwh.net_energy_kwh, 0.0,
            "without a recorder the run metrics stay empty"
        );
    }
}

/// The builder turns every home's recorder on: the run retains batches
/// (a Time column and the numeric product columns) and the run metrics
/// are real.
#[test]
fn with_retained_output_runs_the_recorder() {
    let dir = tempfile::tempdir().expect("temp output dir");
    let outcomes = fleet()
        .with_retained_output(dir.path())
        .with_output_verbosity(2)
        .expect("verbosity 2 is valid")
        .simulate(1);
    for outcome in outcomes {
        let outcome = outcome.expect("the fixture homes run");
        assert!(
            !matches!(outcome.status, hares_fleet::SimStatus::Failed(_)),
            "with the recorder on the run completes: {:?}",
            outcome.status
        );
        let batches = outcome
            .result
            .timeseries
            .expect("the retained batches exist");
        let first = batches.first().expect("at least one batch");
        assert_eq!(
            first.schema().field(0).name(),
            "Time",
            "the timeseries schema leads with the timestamp column"
        );
        assert!(first.num_columns() > 1, "the product columns exist");
        assert!(
            outcome.result.metrics.total_energy_kwh.net_energy_kwh > 0.0,
            "the run metrics are computed from the recorded rows"
        );
        assert_eq!(first.num_rows(), 60, "one hour at the constructor's step");
    }
}
