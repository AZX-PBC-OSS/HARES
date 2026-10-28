//! The checkpoint-resume acceptance tests: a run checkpointed mid-year and
//! resumed equals the continuous run bitwise in every output surface —
//! the frames, the metrics, the billing, the health and the warnings.
//!
//! Fixture: the HPXML, schedule and weather of
//! `tests/fixtures/parity/cz4a_ashp_hpwh/`, start
//! `2023-01-25T00:00:00-07:00`, 12 days (crossing the January to February
//! billing boundary), 900 s, `metrics = true`, with the time-of-use tariff
//! `tests/fixtures/urdb/pge_e_tou_c.json` in zone `America/Denver`. The
//! mid-billing checkpoint lands at `2023-01-28T16:30` local: inside the
//! January billing period and inside an afternoon price period, mid-hour.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, Float64Array, TimestampMillisecondArray};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, RunResult};
use hares_io::{
    Destination, Frequency, ReportSpec, Selection, SimulationConfig, SiteLocationOverride,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const PGE_E_TOU_C: &str = include_str!("../../../tests/fixtures/urdb/pge_e_tou_c.json");

fn start() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 25, 0, 0, 0)
        .single()
        .expect("start")
}

fn sim_config() -> SimulationConfig {
    let config = SimulationConfig {
        scenario_seed: None,
        schedule_diversity: Default::default(),
        start_time: start(),
        duration: Duration::days(12),
        time_res: Duration::seconds(900),
        reports: vec![ReportSpec {
            name: "frame".to_string(),
            selection: Selection::Verbosity(2),
            frequency: Frequency::default(),
            destination: Destination::Memory,
        }],
        diagnostics: false,
        metrics: true,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    };
    config.validate().expect("config validates");
    config
}

fn dwelling() -> Dwelling {
    let root = project_root();
    let home = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(), None, 1, None),
    })
    .expect("cz4a dwelling builds");
    // The monthly cycle closes on the start's day anniversary, so a
    // January 25 start's first boundary is February 25, outside a 12-day
    // run. The seven-day cycle puts a boundary at February 1: the entry's
    // January to February crossing, with its checkpoint instants kept
    // exactly.
    let mut tariff = hares_tariff::parse_urdb(PGE_E_TOU_C).expect("parse the URDB tariff");
    tariff.billing_cycle = hares_types::BillingCycle::Custom(7);
    dwelling
        .set_tariff(tariff, chrono_tz::America::Denver)
        .expect("attach the tariff");
    dwelling
}

/// The steps whose interval end is `at`, local wall clock: the checkpoint
/// step counts steps of 900 s from the run's start.
fn steps_until(at: &str) -> u64 {
    let checkpoint = DateTime::parse_from_rfc3339(at).expect("checkpoint time");
    let secs = checkpoint.signed_duration_since(start()).num_seconds();
    debug_assert!(
        secs > 0 && secs % 900 == 0,
        "the checkpoint lands on a step end"
    );
    u64::try_from(secs / 900).expect("positive step count")
}

/// The mid-billing checkpoint instant: 2023-01-28T16:30 local, step 354.
const MID_BILLING_CHECKPOINT: &str = "2023-01-28T16:30:00-07:00";
/// The post-close checkpoint instant: 2023-02-03T12:00 local, step 912,
/// after the January billing period closed.
const AFTER_CLOSE_CHECKPOINT: &str = "2023-02-03T12:00:00-07:00";

/// The metrics as their float bits: the shortest round-trip Debug form is
/// injective on f64 bit patterns (every finite value prints its own
/// shortest form, -0.0 prints apart from 0.0), so equal renderings pin
/// equal bits.
fn metrics_digest(result: &RunResult) -> String {
    let Some(metrics) = result.metrics.as_ref() else {
        panic!("the run requested metrics");
    };
    format!("{metrics:?}")
}

/// The billing summaries as a bit-level digest: the bounds and every
/// charge's bits.
fn summaries_digest(dwelling: &Dwelling) -> String {
    let mut digest = String::new();
    for summary in dwelling.billing_summaries() {
        digest.push_str(&format!(
            "{}..{}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x};",
            summary.period_start,
            summary.period_end,
            summary.energy_charge_usd.to_bits(),
            summary.demand_charge_usd.to_bits(),
            summary.fixed_charge_usd.to_bits(),
            summary.export_credit_usd.to_bits(),
            summary.net_bill_usd.to_bits(),
            summary.peak_demand_kw.to_bits(),
            summary.total_import_kwh.to_bits(),
            summary.total_export_kwh.to_bits(),
        ));
    }
    digest
}

/// The frames as bit-level digests: every batch's timestamp and value
/// column, row by row, on bits.
fn frames_digest(result: &RunResult) -> String {
    let mut digest = String::new();
    for report in &result.reports {
        digest.push_str(&report.name);
        digest.push('|');
        for batch in report.batches() {
            digest.push_str(&batch_digest(batch));
        }
    }
    digest
}

fn batch_digest(batch: &RecordBatch) -> String {
    let mut digest = format!("{}x{};", batch.num_rows(), batch.num_columns());
    let ts = batch
        .column(0)
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .expect("timestamp column");
    for r in 0..batch.num_rows() {
        digest.push_str(&format!("{},", ts.value(r)));
    }
    digest.push(';');
    for (c, column) in batch.columns().iter().enumerate().skip(1) {
        let values = column
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap_or_else(|| panic!("column {c} is f64"));
        for r in 0..values.len() {
            digest.push_str(&format!("{:x},", values.value(r).to_bits()));
        }
        digest.push(';');
    }
    digest
}

/// The uninterrupted run, and the interrupted-and-resumed twin: the
/// interrupted run steps to `checkpoint_at`, checkpoints, and a freshly
/// built dwelling restores the checkpoint and runs to the end.
fn continuous_and_resumed(
    checkpoint_at: &str,
) -> (Arc<RunResult>, Dwelling, Arc<RunResult>, Dwelling) {
    let mut continuous = dwelling();
    let continuous_result = continuous.simulate().expect("the continuous run completes");

    let checkpoint_step = steps_until(checkpoint_at);
    let mut interrupted = dwelling();
    for _ in 0..checkpoint_step {
        interrupted.step().expect("the interrupted run steps");
    }
    let checkpoint = interrupted.save_checkpoint().expect("checkpoint saves");
    let file = checkpoint_path("resume");
    checkpoint.save(&file).expect("checkpoint writes");

    let mut resumed = dwelling();
    let loaded = hares_core::DwellingCheckpoint::load(&file).expect("checkpoint loads");
    resumed
        .load_checkpoint(loaded)
        .expect("the checkpoint restores");
    let resumed_result = resumed.simulate().expect("the resumed run completes");
    let _ = std::fs::remove_file(&file);
    (continuous_result, continuous, resumed_result, resumed)
}

fn checkpoint_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("hares-r48-or10");
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Nextest runs this binary's tests concurrently as separate processes,
    // and the coverage job's in-process cargo test runs them on shared
    // threads of one process: the pid plus a per-call sequence keeps a
    // parallel test's overwrite (or a half written file, or its remove) out
    // of another test's load.
    static CHECKPOINT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = CHECKPOINT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!("{name}-{}-{seq}.json", std::process::id()))
}

/// A run checkpointed mid-billing-period and resumed equals the
/// uninterrupted run bitwise in every output surface: the metrics, the
/// billing summaries, the health totals, the warning record and the
/// report frames.
#[test]
fn restore_mid_billing_period_then_finish_matches_uninterrupted_run() {
    let (continuous_result, continuous, resumed_result, resumed) =
        continuous_and_resumed(MID_BILLING_CHECKPOINT);

    assert_eq!(
        metrics_digest(&continuous_result),
        metrics_digest(&resumed_result),
        "the resumed run's metrics are bitwise the uninterrupted run's"
    );
    assert_eq!(
        summaries_digest(&continuous),
        summaries_digest(&resumed),
        "the resumed run's billing summaries are bitwise the uninterrupted run's"
    );
    assert_eq!(
        continuous_result.health, resumed_result.health,
        "the resumed run's health totals equal the uninterrupted run's"
    );
    assert_eq!(
        continuous_result.warnings, resumed_result.warnings,
        "the resumed run's warning record equals the uninterrupted run's"
    );
    assert_eq!(
        continuous_result.warnings_dropped, resumed_result.warnings_dropped,
        "the resumed run's dropped-warning count equals the uninterrupted run's"
    );
    assert_eq!(
        frames_digest(&continuous_result),
        frames_digest(&resumed_result),
        "the resumed run's frames are bitwise the uninterrupted run's"
    );
}

/// The same run checkpointed after the January period closed: the restored
/// run keeps the closed summary and finishes to the uninterrupted run's
/// billing summaries, the January one included.
#[test]
fn restore_after_a_closed_billing_period_keeps_its_summary() {
    let (_continuous_result, continuous, _resumed_result, resumed) =
        continuous_and_resumed(AFTER_CLOSE_CHECKPOINT);

    let continuous_summaries = summaries_digest(&continuous);
    let resumed_summaries = summaries_digest(&resumed);
    assert!(
        continuous_summaries.contains("2023-01-"),
        "the uninterrupted run holds the closed period's summary: {continuous_summaries}"
    );
    assert_eq!(
        continuous_summaries, resumed_summaries,
        "the resumed run's billing summaries, the closed January one included, \
         are bitwise the uninterrupted run's"
    );
}

/// The restored evaluator's price at the first post-restore step is the
/// uninterrupted run's price at that step: the price-index position
/// restored, not restarted.
#[test]
fn restore_resumes_tariff_price_index() {
    let price_of = |dwelling: &Dwelling| {
        dwelling
            .tariff_evaluator()
            .expect("a tariff is attached")
            .current_price()
    };
    let checkpoint_step = steps_until(MID_BILLING_CHECKPOINT);
    let mut continuous = dwelling();
    for _ in 0..checkpoint_step {
        continuous.step().expect("step");
    }
    let price_before = price_of(&continuous);

    let mut interrupted = dwelling();
    for _ in 0..checkpoint_step {
        interrupted.step().expect("step");
    }
    let checkpoint = interrupted.save_checkpoint().expect("checkpoint saves");
    let mut resumed = dwelling();
    resumed.load_checkpoint(checkpoint).expect("restore");
    assert_eq!(
        price_before.to_bits(),
        price_of(&resumed).to_bits(),
        "the first post-restore step's price is the uninterrupted run's"
    );

    // And the two run on: the next step's price still agrees.
    continuous.step().expect("step");
    resumed.step().expect("step");
    assert_eq!(
        price_of(&continuous).to_bits(),
        price_of(&resumed).to_bits(),
        "the second post-restore step's price is the uninterrupted run's"
    );
}

/// A run checkpointed mid-hour and resumed reduces to the uninterrupted
/// run's hourly rows: the checkpoint's open hour's partial steps are
/// carried, so the hour row at the checkpoint's period start
/// (2023-01-28T16:00) and every later one match on bits, step counts
/// included.
#[test]
fn restore_mid_hour_matches_uninterrupted_hourly_report() {
    use hares_io::output::reduce::{PeriodCalendar, PeriodReducer, TimeRule};
    use hares_types::Aggregation;

    let (continuous_result, _continuous, resumed_result, _resumed) =
        continuous_and_resumed(MID_BILLING_CHECKPOINT);

    let reduce_hourly = |result: &RunResult| {
        let report = &result.reports[0];
        let rules: Vec<TimeRule> = (0..report.batches()[0].num_columns() - 1)
            .map(|_| TimeRule {
                aggregation: Aggregation::Mean,
                enum_codes: None,
            })
            .collect();
        let mut reducer = PeriodReducer::new(Frequency::Hourly, PeriodCalendar::default(), rules)
            .expect("reducer");
        let mut rows: Vec<(i64, u32, Vec<u64>, Vec<u32>)> = Vec::new();
        for batch in report.batches() {
            let ts = batch
                .column(0)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("timestamp column");
            let values: Vec<&Float64Array> = batch
                .columns()
                .iter()
                .skip(1)
                .map(|c| c.as_any().downcast_ref::<Float64Array>().expect("f64"))
                .collect();
            for r in 0..batch.num_rows() {
                let row: Vec<f64> = values.iter().map(|c| c.value(r)).collect();
                let valid: Vec<bool> = values.iter().map(|c| c.is_valid(r)).collect();
                reducer
                    .push_row(ts.value(r), &row, &valid, &mut |period| {
                        rows.push((
                            period.start_local_ms,
                            period.step_count,
                            period.values.iter().map(|v| v.to_bits()).collect(),
                            period.valid_steps.to_vec(),
                        ));
                    })
                    .expect("the batch reduces");
            }
        }
        reducer.finish(&mut |period| {
            rows.push((
                period.start_local_ms,
                period.step_count,
                period.values.iter().map(|v| v.to_bits()).collect(),
                period.valid_steps.to_vec(),
            ));
        });
        rows
    };

    let continuous_rows = reduce_hourly(&continuous_result);
    let resumed_rows = reduce_hourly(&resumed_result);
    assert_eq!(
        continuous_rows.len(),
        resumed_rows.len(),
        "the resumed run reduces to the uninterrupted run's hourly row count"
    );
    let checkpoint_hour = local_ms_of("2023-01-28T16:00:00-07:00");
    let mut mid_hour_checked = false;
    for (continuous_row, resumed_row) in continuous_rows.iter().zip(&resumed_rows) {
        assert_eq!(continuous_row, resumed_row, "an hourly row differs on bits");
        if continuous_row.0 == checkpoint_hour {
            mid_hour_checked = true;
            assert_eq!(
                continuous_row.1, 4,
                "the mid-hour period holds all four 900 s steps of the hour"
            );
        }
    }
    assert!(
        mid_hour_checked,
        "the checkpoint's own hour is among the compared rows"
    );
    // The rows at and after the checkpoint's hour exist on both runs; the
    // resumed run's reduction covers them bitwise.
    let after = |rows: &[(i64, u32, Vec<u64>, Vec<u32>)]| {
        rows.iter()
            .filter(|(start, _, _, _)| *start >= checkpoint_hour)
            .count()
    };
    assert_eq!(after(&continuous_rows), after(&resumed_rows));
    // The run is 12 days of 900 s steps: 288 hourly periods, 200 of them
    // from the checkpoint's hour to the horizon's end.
    assert_eq!(after(&continuous_rows), 200);
}

fn local_ms_of(at: &str) -> i64 {
    let t = DateTime::parse_from_rfc3339(at).expect("time");
    t.timestamp_millis() + i64::from(t.offset().local_minus_utc()) * 1000
}
