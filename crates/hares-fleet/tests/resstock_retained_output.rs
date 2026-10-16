//! The ResStock fleet's run on the streaming fold: the homes run their
//! constructor's own sim config (the recorder off, no destination exists
//! at construction), the fleet appends its own `fleet` report, and the
//! result carries the aggregate, the homes table and the run metrics when
//! the homes requested them.

use std::path::PathBuf;

use hares_fleet::{Fleet, FleetReport};
use hares_io::{Frequency, ResStockVersion, Selection};

fn fleet() -> hares_fleet::Fleet {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = root.join("tests/fixtures/resstock/2025.1");
    Fleet::from_resstock(
        &fixtures.join("metadata_fleet3.parquet"),
        &fixtures,
        &fixtures.join("weather"),
        Some(ResStockVersion::V2025_1),
        None,
        None,
        Some(chrono::Duration::seconds(3600)),
        Some(&root.join("defaults")),
    )
    .expect("fleet construction")
}

fn report(selection: Selection, frequency: Frequency) -> FleetReport {
    FleetReport {
        selection,
        frequency,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

fn chunk(size: usize) -> std::num::NonZeroUsize {
    std::num::NonZeroUsize::new(size).expect("nonzero chunk")
}

/// The homes run with the constructor's own sim config: no recorder, no
/// run metrics, and the fleet's own report folds every home.
#[test]
fn resstock_fleet_runs_without_a_recorder_by_default() {
    let result = fleet()
        .simulate(
            &report(Selection::Verbosity(0), Frequency::Hourly),
            1,
            chunk(2),
        )
        .expect("the fixture homes run");

    assert_eq!(result.homes.len(), 3);
    for home in &result.homes {
        assert!(
            !matches!(home.status, hares_fleet::HomeStatus::Failed(_)),
            "a missing recorder is not a run failure: {:?}",
            home.status
        );
        assert!(
            home.metrics.is_none(),
            "without the metrics request the run's result carries none"
        );
        assert_eq!(home.steps, 60, "one hour at the constructor's step");
    }
    assert_eq!(
        result.peak_live_home_results, 2,
        "the chunk bounds the fold"
    );
    assert_eq!(
        result.coverage.weight_total,
        result.coverage.weight_ok + result.coverage.weight_degraded + result.coverage.weight_failed,
        "the coverage weights partition the fleet's total"
    );
    assert_eq!(
        result.coverage.weight_failed, 0.0,
        "no fixture home fails: a warning is not a failure"
    );
}

/// The fold reduces the homes' steps onto the frequency: one hour at the
/// constructor's one-minute step is one hourly row per home.
#[test]
fn resstock_fleet_folds_hourly() {
    let result = fleet()
        .simulate(
            &report(Selection::Verbosity(0), Frequency::Hourly),
            1,
            chunk(2),
        )
        .expect("the fixture homes run");

    let aggregate = &result.aggregate;
    assert_eq!(aggregate.num_rows(), 1, "one hour is one row");
    assert_eq!(aggregate.schema().field(0).name(), "Time");
    assert!(
        aggregate.num_columns() > 1,
        "the product columns exist: the homes' tier 0 variables folded"
    );
    // The homes table: one row per home, the fleet's own columns.
    let homes = result.homes_table();
    assert_eq!(homes.num_rows(), 3);
    let names: Vec<String> = homes
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();
    assert_eq!(
        names,
        vec![
            "bldg_id",
            "weight",
            "status",
            "reason",
            "steps",
            "periods_folded",
        ],
        "no home requested metrics, so the homes table carries none"
    );
}
