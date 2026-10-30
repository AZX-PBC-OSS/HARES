//! The thread-safety and per-run-state proofs.
//!
//! The sharing model is the `Arc`-shared parsed inputs behind one
//! blueprint plus the per-dwelling mutable state: N dwellings
//! built from one blueprint and stepped on N threads give the serial
//! runs' results bitwise, two concurrent runs report health counts that
//! never cross runs, and whole frames are identical across spawned
//! processes that draw their own hash seeds.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::DataType;
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_core::actor::{Actor, ActorInterest};
use hares_core::{
    Dwelling, DwellingBlueprint, DwellingConfig, DwellingSettings, RunHealth, RunMetrics,
};
use hares_io::{
    DefaultsSource, Destination, Frequency, InputSource, ReportSpec, Selection, SimulationConfig,
    SiteLocationOverride, WeatherInput,
};
use hares_types::{ControlSignal, EndUse, EnvironmentState, TelemetryField, Warning};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(start: DateTime<FixedOffset>, duration_s: i64, step_s: i64) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        schedule_diversity: Default::default(),
        start_time: start,
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(step_s),
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
    }
}

/// One shared blueprint of the entry's inputs: `base.xml`, the BEopt
/// example schedule, the Denver TMY3 weather, the repo's defaults, 24 h
/// at 900 s.
fn base_blueprint() -> Arc<DwellingBlueprint> {
    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    DwellingBlueprint::from_config(DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(start, 24 * 3600, 900),
            overrides: None,
            bldg_id: 1,
            patches: None,
        },
    })
    .expect("the blueprint builds")
}

/// One run's comparable products: the frame batches, the metrics, the
/// health totals and the warning record.
struct RunRecord {
    batches: Vec<RecordBatch>,
    metrics: Option<RunMetrics>,
    health: RunHealth,
    warnings: Vec<Warning>,
}

fn run_one(dwelling: &mut Dwelling) -> RunRecord {
    let result = dwelling.simulate().expect("the run completes");
    let batches = dwelling.flushed_batches().to_vec();
    RunRecord {
        batches,
        metrics: result.metrics.clone(),
        health: result.health.clone(),
        warnings: result.warnings.clone(),
    }
}

/// Every column of every batch compared bitwise: float64 columns by their
/// bit patterns (so a sign flip or a payload difference cannot hide), Utf8
/// columns by their bytes.
fn assert_frames_bitwise_equal(a: &[RecordBatch], b: &[RecordBatch], context: &str) {
    assert_eq!(a.len(), b.len(), "{context}: batch count");
    for (batch_index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            x.num_rows(),
            y.num_rows(),
            "{context}: batch {batch_index} rows"
        );
        assert_eq!(
            x.num_columns(),
            y.num_columns(),
            "{context}: batch {batch_index} columns"
        );
        for (col, (cx, cy)) in x.columns().iter().zip(y.columns()).enumerate() {
            assert_eq!(
                cx.data_type(),
                cy.data_type(),
                "{context}: batch {batch_index} column {col} type"
            );
            match cx.data_type() {
                DataType::Float64 => {
                    let xa = cx
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column");
                    let ya = cy
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .expect("f64 column");
                    for row in 0..x.num_rows() {
                        assert_eq!(
                            xa.is_null(row),
                            ya.is_null(row),
                            "{context}: batch {batch_index} column {col} row {row} null"
                        );
                        if !xa.is_null(row) {
                            assert_eq!(
                                xa.value(row).to_bits(),
                                ya.value(row).to_bits(),
                                "{context}: batch {batch_index} column {col} row {row} bits"
                            );
                        }
                    }
                }
                DataType::Utf8 | DataType::LargeUtf8 => {
                    let xa = cx
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8 column");
                    let ya = cy
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("utf8 column");
                    for row in 0..x.num_rows() {
                        assert_eq!(
                            xa.value(row),
                            ya.value(row),
                            "{context}: batch {batch_index} column {col} row {row}"
                        );
                    }
                }
                _ => assert!(cx == cy, "{context}: batch {batch_index} column {col}"),
            }
        }
    }
}

/// The metrics, health and warnings compared bitwise: the postcard bytes
/// of the serializable triple, every f64 by its IEEE-754 bytes (BTreeMaps
/// inside, so the encoding is deterministic across processes too).
fn assert_results_bitwise_equal(a: &RunRecord, b: &RunRecord, context: &str) {
    let a_bytes = postcard::to_allocvec(&(&a.metrics, &a.health, &a.warnings))
        .expect("the result triple serializes");
    let b_bytes = postcard::to_allocvec(&(&b.metrics, &b.health, &b.warnings))
        .expect("the result triple serializes");
    assert_eq!(
        a_bytes, b_bytes,
        "{context}: metrics, health and warnings bitwise"
    );
    assert_frames_bitwise_equal(&a.batches, &b.batches, context);
}

/// Four dwellings built from one shared blueprint, stepped on four
/// threads (one dwelling per thread, the blueprint's `Arc` the only
/// shared object), give the same four run one at a time bitwise: frames,
/// metrics, health and warnings.
#[test]
fn concurrent_dwellings_match_sequential() {
    let blueprint = base_blueprint();

    let mut serial = Vec::new();
    for _ in 0..4 {
        let mut dwelling = blueprint.build().expect("the serial build");
        serial.push(run_one(&mut dwelling));
    }

    let concurrent: Vec<RunRecord> = (0..4)
        .map(|_| {
            let bp = Arc::clone(&blueprint);
            std::thread::spawn(move || {
                let mut dwelling = bp.build().expect("the concurrent build");
                run_one(&mut dwelling)
            })
        })
        .map(|handle| handle.join().expect("the concurrent thread"))
        .collect();

    for (index, record) in concurrent.iter().enumerate() {
        assert_results_bitwise_equal(&serial[index], record, &format!("dwelling {index}"));
    }
}

/// An actor that sends an out-of-bounds control signal every step: a
/// sub-band thermostat deadband, rejected at dispatch, counted in the
/// run's `rejected_control_signals`.
struct RejectingActor {
    name: String,
    sent: Arc<AtomicU64>,
}

impl Actor for RejectingActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn output_variables(&self) -> &[TelemetryField] {
        &[]
    }

    fn interests(&self) -> &[ActorInterest] {
        &[ActorInterest::EveryStep]
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        out.push(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: None,
                deadband_c: Some(1e-17),
            },
            priority: PriorityTier::Schedule,
        });
    }
}

/// Two concurrent runs of the same inputs (two builds of one blueprint,
/// two threads), one with the rejecting actor attached: the actor run
/// reports `rejected_control_signals` of exactly the number it sent and
/// the plain run reports 0. A count shared across runs would report the
/// sum to both.
#[test]
fn concurrent_runs_report_independent_counts() {
    let blueprint = base_blueprint();

    let plain = {
        let bp = Arc::clone(&blueprint);
        std::thread::spawn(move || {
            let mut dwelling = bp.build().expect("the plain build");
            run_one(&mut dwelling).health.rejected_control_signals
        })
    };

    let sent = Arc::new(AtomicU64::new(0));
    let actor_runs = {
        let bp = Arc::clone(&blueprint);
        let sent = Arc::clone(&sent);
        std::thread::spawn(move || {
            let mut dwelling = bp.build().expect("the actor build");
            dwelling
                .add_actor(Box::new(RejectingActor {
                    name: "rejector".to_string(),
                    sent,
                }))
                .expect("the actor registers");
            run_one(&mut dwelling)
        })
    };

    let plain_rejections = plain.join().expect("the plain thread");
    let actor_record = actor_runs.join().expect("the actor thread");

    let sent = sent.load(Ordering::Relaxed);
    assert!(sent > 0, "the actor ran and sent its signals");
    assert_eq!(
        actor_record.health.rejected_control_signals, sent,
        "the actor run counts exactly its own rejections"
    );
    assert_eq!(
        plain_rejections, 0,
        "the concurrent plain run's count stays 0: no leak across runs"
    );
}

// --- spawned-process reproducibility ----------------------------------------

/// The child marker: with `HARES_RT5_CHILD` set, runs one fixture's 48 h
/// at 900 s and prints the frame digest. Ignored, so the parent's
/// `--exact --ignored` spawn is the only thing that runs it.
#[test]
#[ignore]
fn rt5_spawned_process_child() {
    let Ok(fixture) = std::env::var("HARES_RT5_CHILD") else {
        return;
    };

    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let blueprint = DwellingBlueprint::from_config(DwellingConfig {
        hpxml: InputSource::Path(project_root().join(fixture)),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(start, 48 * 3600, 900),
            overrides: None,
            bldg_id: 1,
            patches: None,
        },
    })
    .expect("the blueprint builds");
    let mut dwelling = blueprint.build().expect("the build");
    let record = run_one(&mut dwelling);

    // The frame digest: every column's type and every buffer's raw bytes
    // (values and validity masks alike) into a deterministic hasher
    // (DefaultHasher::new is unkeyed). Two processes with equal digests
    // ran frames equal bit for bit.
    let mut hasher = std::hash::DefaultHasher::new();
    for batch in &record.batches {
        std::hash::Hash::hash(&batch.num_columns(), &mut hasher);
        std::hash::Hash::hash(&batch.num_rows(), &mut hasher);
        for column in batch.columns() {
            std::hash::Hash::hash(&column.data_type().to_string(), &mut hasher);
            let data = column.into_data();
            std::hash::Hash::hash(&data.offset(), &mut hasher);
            for buffer in data.buffers() {
                std::hash::Hash::hash(&buffer.len(), &mut hasher);
                std::hash::Hasher::write(&mut hasher, buffer.as_slice());
            }
            if let Some(nulls) = data.nulls() {
                std::hash::Hasher::write(&mut hasher, nulls.validity());
            }
        }
    }
    println!("HASH {:016x}", std::hash::Hasher::finish(&hasher));
}

/// The spawned-process reproducibility proof: the test binary spawns
/// itself twenty times across the two fixtures (each child a fresh
/// process drawing its own hash seed, so a hashed-collection iteration
/// that reached the numbers could not hide), and every child's frame
/// digest is equal.
#[test]
fn whole_frame_identical_across_spawned_processes() {
    let fixtures = [
        "tests/fixtures/hpxml/ochre_samples/base-foundation-basement-garage.xml",
        "tests/fixtures/hpxml/ochre_samples/base-hvac-multiple.xml",
    ];
    for fixture in fixtures {
        let mut digests = Vec::new();
        for _ in 0..10 {
            let output = std::process::Command::new(std::env::current_exe().expect("test exe"))
                .args([
                    "--exact",
                    "rt5_spawned_process_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HARES_RT5_CHILD", fixture)
                .output()
                .expect("the child spawns");
            assert!(
                output.status.success(),
                "the child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            let digest = stdout
                .lines()
                .find_map(|line| line.strip_prefix("HASH "))
                .unwrap_or_else(|| panic!("the child printed no hash: {stdout}"))
                .to_string();
            digests.push(digest);
        }
        let first = digests.first().expect("children ran").clone();
        for (index, digest) in digests.iter().enumerate() {
            assert_eq!(
                *digest, first,
                "child {index} of {fixture}: frame digest differs across spawned processes"
            );
        }
    }
}
