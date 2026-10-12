//! The batch callback: a run streams every flushed batch of its output
//! report to an attached sink, in order, and a callback error ends the run
//! the way the failure budget ends it.
//!
//! The sink is minimal on purpose: `on_batch` with the batch and a context
//! naming the report, the building and the batch's index. The destination
//! model lands later and turns the sink into one destination among
//! several; the notes in `crates/hares-io/src/output/callback.rs` mark
//! where each piece plugs in.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arrow::array::{Float64Array, StringArray, TimestampMillisecondArray};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, Duration};
use hares_control::DispatchRequest;
use hares_core::actor::Actor;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::output::callback::{BatchCallback, BatchContext, CallbackError};
use hares_io::{Destination, ReportSpec, Selection, SiteLocationOverride};
use hares_types::{EnvironmentState, TelemetryField};

/// The one report the engine builds today: the run's timestep table.
const REPORT: &str = "timestep";

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Collected {
    report: String,
    bldg_id: i64,
    batch_index: u64,
    batch: RecordBatch,
}

/// A test sink: collects the batches it is handed, and fails the named
/// batch index with a message the run must carry through.
struct Collecting {
    sink: Arc<Mutex<Vec<Collected>>>,
    fail_on: Option<u64>,
}

impl BatchCallback for Collecting {
    fn on_batch(
        &mut self,
        ctx: &BatchContext<'_>,
        batch: &RecordBatch,
    ) -> Result<(), CallbackError> {
        if self.fail_on == Some(ctx.batch_index) {
            return Err(CallbackError::new("the store is full"));
        }
        self.sink.lock().expect("sink lock").push(Collected {
            report: ctx.report.to_string(),
            bldg_id: ctx.bldg_id,
            batch_index: ctx.batch_index,
            batch: batch.clone(),
        });
        Ok(())
    }
}

fn config(retain_batches: bool, output_path: &std::path::Path) -> SimulationConfig {
    SimulationConfig {
        start_time: DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00").expect("start time"),
        duration: Duration::hours(24),
        time_res: Duration::seconds(900),
        reports: vec![ReportSpec {
            name: REPORT.to_string(),
            selection: Selection::Verbosity(2),
            frequency: hares_io::Frequency::default(),
            destination: if retain_batches {
                Destination::Memory
            } else {
                Destination::Parquet {
                    path: output_path.to_path_buf(),
                    overwrite: false,
                }
            },
        }],
        diagnostics: false,
        metrics: false,
        output_chunk_size: 40,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

fn dwelling(sim_config: SimulationConfig, bldg_id: i64) -> Dwelling {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    Dwelling::from_config(DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(
            sim_config,
            None,
            bldg_id,
            Some(std::time::Duration::from_secs(7 * 24 * 3600)),
            None,
        ),
    })
    .expect("the fixture dwelling builds")
}

fn assert_batches_bitwise_equal(a: &[RecordBatch], b: &[RecordBatch], label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: batch count");
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(x.schema(), y.schema(), "{label}: batch {i} schema");
        assert_eq!(x.num_rows(), y.num_rows(), "{label}: batch {i} row count");
        for (j, (xc, yc)) in x.columns().iter().zip(y.columns()).enumerate() {
            if let (Some(xs), Some(ys)) = (
                xc.as_any().downcast_ref::<TimestampMillisecondArray>(),
                yc.as_any().downcast_ref::<TimestampMillisecondArray>(),
            ) {
                for k in 0..x.num_rows() {
                    assert_eq!(
                        xs.value(k),
                        ys.value(k),
                        "{label}: batch {i} column {j} row {k}"
                    );
                }
            } else if let (Some(xs), Some(ys)) = (
                xc.as_any().downcast_ref::<StringArray>(),
                yc.as_any().downcast_ref::<StringArray>(),
            ) {
                for k in 0..x.num_rows() {
                    assert_eq!(
                        xs.value(k),
                        ys.value(k),
                        "{label}: batch {i} column {j} row {k}"
                    );
                }
            } else {
                let xs = xc
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap_or_else(|| {
                        panic!("{label}: batch {i} column {j} is not a float column")
                    });
                let ys = yc
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap_or_else(|| {
                        panic!("{label}: batch {i} column {j} is not a float column")
                    });
                for k in 0..x.num_rows() {
                    assert_eq!(
                        xs.value(k).to_bits(),
                        ys.value(k).to_bits(),
                        "{label}: batch {i} column {j} row {k}"
                    );
                }
            }
        }
    }
}

/// The callback sees every flushed batch exactly once, in flush order, and
/// the concatenation equals the same run's retained batches on bits: 96
/// steps at 900 s with a 40-row chunk are batches of 40, 40 and 16 rows.
#[test]
fn callback_receives_every_batch_once_in_order() {
    let reference_dir = tempfile::tempdir().expect("tempdir");
    let mut reference = dwelling(
        config(true, &reference_dir.path().join("reference.parquet")),
        7,
    );
    reference.simulate().expect("the reference run completes");
    let expected: Vec<RecordBatch> = reference.flushed_batches().to_vec();
    assert_eq!(expected.len(), 3, "the reference run flushes three batches");
    assert_eq!(
        expected
            .iter()
            .map(RecordBatch::num_rows)
            .collect::<Vec<_>>(),
        vec![40, 40, 16],
        "the batch boundaries follow the configured chunk size"
    );

    let run_dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.attach_callback(
        REPORT,
        Box::new(Collecting {
            sink: Arc::clone(&sink),
            fail_on: None,
        }),
    )
    .expect("the callback attaches");
    run.simulate().expect("the callback run completes");

    let got = sink.lock().expect("sink lock");
    assert_eq!(got.len(), 3, "the callback saw three batches, once each");
    let indices: Vec<u64> = got.iter().map(|c| c.batch_index).collect();
    assert_eq!(indices, vec![0, 1, 2], "batch_index counts from 0 in order");

    let got_batches: Vec<RecordBatch> = got.iter().map(|c| c.batch.clone()).collect();
    assert_batches_bitwise_equal(
        &got_batches,
        &expected,
        "the concatenated batches equal the retained batches",
    );
}

/// The context names the report and the building the batch belongs to.
#[test]
fn callback_context_carries_bldg_id_and_report_name() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 42);
    run.attach_callback(
        REPORT,
        Box::new(Collecting {
            sink: Arc::clone(&sink),
            fail_on: None,
        }),
    )
    .expect("the callback attaches");
    run.simulate().expect("the run completes");

    let got = sink.lock().expect("sink lock");
    assert!(!got.is_empty(), "the callback saw at least one batch");
    for collected in got.iter() {
        assert_eq!(collected.report, REPORT);
        assert_eq!(collected.bldg_id, 42);
    }
    let indices: Vec<u64> = got.iter().map(|c| c.batch_index).collect();
    assert_eq!(
        indices,
        (0..got.len() as u64).collect::<Vec<_>>(),
        "the indices are the flush order"
    );
}

/// A callback error fails the run at that step, naming the report and
/// carrying the cause; the run stays ended (every later step returns the
/// error without stepping anything) and the callback is never called
/// again.
#[test]
fn callback_error_fails_the_run_naming_the_report() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.attach_callback(
        REPORT,
        Box::new(Collecting {
            sink: Arc::clone(&sink),
            fail_on: Some(1),
        }),
    )
    .expect("the callback attaches");

    let mut steps_ok = 0;
    let err = loop {
        match run.step() {
            Ok(_) => steps_ok += 1,
            Err(err) => break err,
        }
    };
    assert!(
        steps_ok < 96,
        "the run must fail at the flush, not run to the end: {steps_ok} steps ran"
    );
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("the store is full"),
                "the error must carry the callback's cause, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }

    // The budget semantics: the run stays ended, and the failed callback
    // is not called again.
    let calls_after_failure = sink.lock().expect("sink lock").len();
    for _ in 0..3 {
        let later = run.step().expect_err("the run stays ended");
        assert!(
            later.to_string().contains("the store is full"),
            "the later step's error must carry the original cause, got: {later}"
        );
    }
    assert_eq!(
        sink.lock().expect("sink lock").len(),
        calls_after_failure,
        "the callback is not called after the run ended"
    );
}

/// Attaching to a report the engine does not build is a typed error naming
/// the report and the one report that exists.
#[test]
fn attach_to_non_callback_report_is_an_error() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    let err = run
        .attach_callback(
            "hourly",
            Box::new(Collecting {
                sink: Arc::new(Mutex::new(Vec::new())),
                fail_on: None,
            }),
        )
        .expect_err("attaching to an unknown report fails");
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, "hourly");
            assert!(
                message.contains(REPORT),
                "the error must name the report that exists, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }
}

/// A callback attached after rows were recorded is refused: the
/// destinations freeze with the first recorded row.
#[test]
fn attach_after_the_first_recorded_row_is_an_error() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.step().expect("the first step runs");
    let err = run
        .attach_callback(
            REPORT,
            Box::new(Collecting {
                sink: Arc::new(Mutex::new(Vec::new())),
                fail_on: None,
            }),
        )
        .expect_err("attaching after the first recorded row fails");
    assert!(
        matches!(err, hares_types::HaresError::Callback { .. }),
        "expected a callback error, got: {err:?}"
    );
}

/// Attaching without a report to ride is refused: the callback sink rides
/// a report's recorder, so a config that declares no reports has nothing
/// to attach to.
#[test]
fn attach_without_output_recording_is_refused() {
    let mut sim = config(false, std::path::Path::new("unused.parquet"));
    sim.reports.clear();
    let mut run = dwelling(sim, 7);
    let err = run
        .attach_callback(
            REPORT,
            Box::new(Collecting {
                sink: Arc::new(Mutex::new(Vec::new())),
                fail_on: None,
            }),
        )
        .expect_err("attaching without a report to ride fails");
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("no reports"),
                "the refusal must name the missing report, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }
}

/// A callback-destination report with no callback attached fails the run
/// at the first step, naming the report: the batches would go nowhere,
/// and the run refuses instead of recording rows nothing reads.
#[test]
fn callback_report_without_attachment_fails_before_first_step() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let mut sim = config(false, &run_dir.path().join("run.parquet"));
    sim.reports[0].destination = Destination::Callback;
    let mut run = dwelling(sim, 7);
    let err = run
        .step()
        .expect_err("the unattached callback report fails the first step");
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("no callback is attached"),
                "the refusal must name the missing attachment, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }
    // The refusal is the run's end: a later step carries it.
    let later = run.step().expect_err("the run stays ended");
    assert!(
        later.to_string().contains("no callback is attached"),
        "the later step's error must carry the refusal, got: {later}"
    );
}

/// A callback attached before the first recorded row survives a roster
/// rebuild: the dwelling holds the attachment and hands a clone to the
/// fresh recorder, so the callback still receives every batch.
#[test]
fn the_callback_survives_a_roster_rebuild() {
    struct Idle;
    impl Actor for Idle {
        fn name(&self) -> &str {
            "idle"
        }
        fn output_variables(&self) -> &[TelemetryField] {
            &[]
        }
        fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {}
    }

    let run_dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.attach_callback(
        REPORT,
        Box::new(Collecting {
            sink: Arc::clone(&sink),
            fail_on: None,
        }),
    )
    .expect("the callback attaches");
    run.add_actor(Box::new(Idle)).expect("the roster rebuilds");
    run.simulate().expect("the run completes");

    let got = sink.lock().expect("sink lock");
    assert_eq!(
        got.len(),
        3,
        "the rebuilt recorder still delivers every batch"
    );
    let indices: Vec<u64> = got.iter().map(|c| c.batch_index).collect();
    assert_eq!(indices, vec![0, 1, 2], "the flush order is unchanged");
}

/// A Rust sink that panics ends the run as a typed callback error, the
/// way a sink that returns an error does: the step delivered nothing, the
/// run latches (a later step returns the error without stepping) and the
/// telemetry is consistent with the steps that completed.
#[test]
fn a_panicking_rust_callback_ends_the_run_as_a_callback_error() {
    let sink: Arc<Mutex<Vec<Collected>>> = Arc::new(Mutex::new(Vec::new()));
    struct PanickingThenCollecting {
        sink: Arc<Mutex<Vec<Collected>>>,
    }
    impl BatchCallback for PanickingThenCollecting {
        fn on_batch(
            &mut self,
            ctx: &BatchContext<'_>,
            batch: &RecordBatch,
        ) -> Result<(), CallbackError> {
            self.sink.lock().expect("sink lock").push(Collected {
                report: ctx.report.to_string(),
                bldg_id: ctx.bldg_id,
                batch_index: ctx.batch_index,
                batch: batch.clone(),
            });
            panic!("the sink exploded");
        }
    }

    let run_dir = tempfile::tempdir().expect("tempdir");
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.attach_callback(
        REPORT,
        Box::new(PanickingThenCollecting {
            sink: Arc::clone(&sink),
        }),
    )
    .expect("the callback attaches");

    let mut steps_ok = 0;
    let err = loop {
        match run.step() {
            Ok(_) => steps_ok += 1,
            Err(err) => break err,
        }
    };
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("the sink exploded"),
                "the error must carry the panic's cause, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }

    // The step the delivery failed in delivered nothing further to the
    // sink: one delivery (the first batch, whose flush panicked), then the
    // run stays ended and the sink sees nothing more.
    let got = sink.lock().expect("sink lock");
    assert_eq!(got.len(), 1, "one delivery, then the sink exploded");
    assert_eq!(got[0].batch_index, 0);
    assert!(steps_ok >= 1, "the steps before the failed flush completed");

    // The budget semantics: the run stays ended.
    let later = run.step().expect_err("the run stays ended");
    assert!(
        later.to_string().contains("the sink exploded"),
        "the later step's error must carry the original cause, got: {later}"
    );
}

/// A callback error at the close (the final partial batch delivered inside
/// `flush_and_close`) latches the run ended the way the step path does.
#[test]
fn a_callback_error_at_close_latches_the_run_ended() {
    let run_dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(Mutex::new(Vec::new()));
    let mut run = dwelling(config(false, &run_dir.path().join("run.parquet")), 7);
    run.attach_callback(
        REPORT,
        Box::new(Collecting {
            sink: Arc::clone(&sink),
            fail_on: Some(2),
        }),
    )
    .expect("the callback attaches");

    // Batches 0 and 1 deliver at the full flushes; the 16-row partial
    // stays buffered until the close.
    for _ in 0..80 {
        run.step().expect("the steps before the close record");
    }

    let err = run
        .simulate()
        .expect_err("the close fails on the final batch");
    match &err {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("the store is full"),
                "the error must carry the callback's cause, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }

    // The latch: a later step returns the close's error without stepping,
    // not the horizon's end.
    let later = run.step().expect_err("the run stays ended");
    assert!(
        later.to_string().contains("the store is full"),
        "the later step's error must carry the close's cause, got: {later}"
    );
    assert_eq!(
        sink.lock().expect("sink lock").len(),
        2,
        "the callback saw only the batches before the close"
    );

    // The failed-batch raise semantics: the latched run's finish() raises
    // the latched error too. It never returns a result: there is no
    // finished run to return, and the close's failure is the cause.
    let finished = run.finish().expect_err("a latched run's finish() raises");
    match &finished {
        hares_types::HaresError::Callback { report, message } => {
            assert_eq!(report, REPORT);
            assert!(
                message.contains("the store is full"),
                "finish()'s error must carry the latched cause, got: {message}"
            );
        }
        other => panic!("expected a callback error, got: {other:?}"),
    }
    assert_eq!(
        sink.lock().expect("sink lock").len(),
        2,
        "the latched finish() re-delivers nothing"
    );
}
