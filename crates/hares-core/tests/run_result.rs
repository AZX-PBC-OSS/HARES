// One RunResult, finish() for every run.
//
// These are the RED tests: they are written against the RunResult and
// Dwelling::finish() surface before it exists, so this file does not even
// compile against the parent commit, and every test here fails while the
// surface is missing.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use hares_control::DispatchRequest;
use hares_control::PriorityTier;
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Actor, Dwelling, DwellingConfig, RunResult};
use hares_types::{EnvironmentState, HaresError, Telemetry, TelemetryField, Warning};
// The engine-run test is the import's only user, and it exists only under
// debug assertions (the invariant failure it exercises panics in release).
#[cfg(debug_assertions)]
use hares_core::SimulationEngine;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `leap_year_february_900s` inputs: the cz4a_ashp_hpwh HPXML, schedule
/// and weather, start `2024-02-27T00:00:00-07:00`, no warm-up, at 900 s.
fn sim_toml(reports: &str) -> String {
    sim_toml_for(reports, 86400)
}

/// The same inputs over a custom horizon (seconds).
fn sim_toml_for(reports: &str, duration_s: u64) -> String {
    format!(
        r#"
start_time = "2024-02-27T00:00:00-07:00"
duration = {duration_s}
time_res = 900
output_chunk_size = 40
{reports}
"#
    )
}

fn memory_report(name: &str, verbosity: u8) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\ndestination = \"memory\"\n"
    )
}

fn dwelling_config(sim_toml: &str) -> DwellingConfig {
    let home = repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(sim_toml).expect("sim config parses"),
            None,
            1,
            None,
        ),
    }
}

fn memory_batches(result: &RunResult) -> Vec<RecordBatch> {
    result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect()
}

/// Bitwise comparison of two batch lists: same count, same columns, and
/// every f64 value equal on bits.
fn assert_batches_bitwise_equal(a: &[RecordBatch], b: &[RecordBatch], context: &str) {
    assert_eq!(a.len(), b.len(), "{context}: the same batch count");
    for (index, (left, right)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            left.schema().fields(),
            right.schema().fields(),
            "{context}: batch {index} schemas"
        );
        for (column, (l, r)) in left.columns().iter().zip(right.columns()).enumerate() {
            let name = left.schema().field(column).name().to_string();
            match (
                l.as_any().downcast_ref::<arrow::array::Float64Array>(),
                r.as_any().downcast_ref::<arrow::array::Float64Array>(),
            ) {
                (Some(l), Some(r)) => {
                    assert_eq!(l.len(), r.len(), "{context}: {name} length");
                    for row in 0..l.len() {
                        let lv = if l.is_null(row) {
                            None
                        } else {
                            Some(l.value(row).to_bits())
                        };
                        let rv = if r.is_null(row) {
                            None
                        } else {
                            Some(r.value(row).to_bits())
                        };
                        assert_eq!(lv, rv, "{context}: {name} row {row}");
                    }
                }
                _ => {
                    assert_eq!(l, r, "{context}: {name} values");
                }
            }
        }
    }
}

/// `stepped_run_finish_equals_simulate`: 24 h (96 steps) with
/// `output_chunk_size = 40`, so the last batch is partial, stepped to its
/// end then `finish()`, equals `simulate()` on an identical dwelling field
/// by field (reports on bits, metrics on bits, health, warnings).
#[test]
fn stepped_run_finish_equals_simulate() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut simulated = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let simulated = simulated.simulate().expect("simulate runs");

    let mut stepped = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    for _ in 0..96 {
        stepped.step().expect("step runs");
    }
    let finished = stepped.finish().expect("finish runs");

    assert_batches_bitwise_equal(
        &memory_batches(&simulated),
        &memory_batches(&finished),
        "reports on bits",
    );
    assert_eq!(
        simulated.metrics, finished.metrics,
        "metrics on bits: the same Option<RunMetrics>"
    );
    assert_eq!(simulated.health, finished.health, "health equal");
    assert_eq!(simulated.warnings, finished.warnings, "warnings equal");
    assert_eq!(
        simulated.warnings_dropped, finished.warnings_dropped,
        "dropped counts equal"
    );
}

/// `finish_before_end_flushes_partial_batch_and_billing`: the same inputs
/// over 72 h with the flat tariff `tests/fixtures/urdb/flat_rate.json`
/// (`America/Denver`), finished after 50 steps: the `Memory` report holds
/// 50 rows and the billing summary covers those 50 steps.
#[test]
fn finish_before_end_flushes_partial_batch_and_billing() {
    let toml = sim_toml_for(&memory_report("frame", 2), 259200);
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let tariff_json =
        std::fs::read_to_string(repo_root().join("tests/fixtures/urdb/flat_rate.json"))
            .expect("tariff file reads");
    let tariff = hares_tariff::parse_urdb(&tariff_json).expect("tariff parses");
    dwelling
        .set_tariff(tariff, chrono_tz::Tz::America__Denver)
        .expect("tariff attaches");

    for _ in 0..50 {
        dwelling.step().expect("step runs");
    }
    let result = dwelling.finish().expect("finish runs");

    let batches = memory_batches(&result);
    let rows: usize = batches.iter().map(|batch| batch.num_rows()).sum();
    assert_eq!(rows, 50, "the Memory report holds the 50 stepped rows");

    let summaries = dwelling.billing_summaries();
    assert!(
        !summaries.is_empty(),
        "the billing summary covers the 50 steps"
    );
    assert!(
        summaries
            .iter()
            .any(|summary| summary.total_import_kwh > 0.0),
        "the finalised periods carry the 50 steps' energy"
    );
}

/// `finish_is_idempotent_and_returns_the_same_arc` (`Arc::ptr_eq` on the
/// two results).
#[test]
fn finish_is_idempotent_and_returns_the_same_arc() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.step().expect("step runs");
    let first = dwelling.finish().expect("finish runs");
    let second = dwelling.finish().expect("second finish runs");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "finish() returns the same Arc"
    );
}

/// `step_after_finish_is_a_typed_error`.
#[test]
fn step_after_finish_is_a_typed_error() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.step().expect("step runs");
    dwelling.finish().expect("finish runs");
    let err = dwelling.step().expect_err("step after finish is an error");
    assert!(
        matches!(err, HaresError::Simulation(_)),
        "the error is typed, got: {err:?}"
    );
}

/// `finish_before_first_step_is_a_typed_error`.
#[test]
fn finish_before_first_step_is_a_typed_error() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let err = dwelling
        .finish()
        .expect_err("finish before the first step is an error");
    assert!(
        matches!(err, HaresError::Simulation(_)),
        "the error is typed, got: {err:?}"
    );
}

/// `take_warnings_returns_only_new_warnings_and_run_result_keeps_all`: a
/// drain returns only the warnings since the previous drain, and the
/// finished run's record holds every warning of the run.
#[test]
fn take_warnings_returns_only_new_warnings_and_run_result_keeps_all() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");

    // The fixture's construction warnings are not the step stream's.
    dwelling.take_warnings();

    // A control signal routed at a target that does not exist rejects the
    // signal and warns, once per step.
    use hares_types::ControlSignal;
    dwelling.step().expect("the first step runs");
    dwelling.apply_control(
        "no such equipment",
        ControlSignal::PowerSetpoint {
            active_power_w: 1.0000,
            reactive_power_var: None,
            min_soc: None,
            max_soc: None,
        },
    );
    dwelling.step().expect("the second step runs");
    let first = dwelling.take_warnings();
    assert_eq!(first.len(), 1, "the first drain holds the one warning");
    dwelling.step().expect("the third step runs");
    let second = dwelling.take_warnings();
    assert!(
        second.is_empty(),
        "the second drain holds only new warnings, got {second:?}"
    );

    let result = dwelling.finish().expect("finish runs");
    // The record keeps every warning of the run: the fixture's
    // construction-time warning (drained above, never un-recorded) and the
    // one rejected-signal warning.
    assert_eq!(
        result
            .warnings
            .iter()
            .filter(|warning| warning.message.contains("no such equipment"))
            .count(),
        1,
        "the run record keeps every warning, not only the undrained ones"
    );
}

/// `gym_drain_sees_warnings_past_run_capacity`: a test actor sends one
/// out-of-bounds control signal per step (one rejected-signal warning each)
/// for 1 100 steps of BESTEST 600 at 900 s, with `take_warnings()` called
/// after every step: each drain returns exactly that step's warning, the
/// 1 001st included, and `RunResult.warnings` holds the first 1 000 with
/// `warnings_dropped` 100.
#[test]
fn gym_drain_sees_warnings_past_run_capacity() {
    use hares_types::ControlSignal;

    /// An actor that queues one control signal at a target that exists in
    /// no dwelling every `decide()`: one rejected-signal warning per step.
    struct RejectingActor;

    impl Actor for RejectingActor {
        fn name(&self) -> &str {
            "rejecting-actor"
        }

        fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
            out.push(DispatchRequest {
                target: hares_control::DispatchTarget::ByName(std::sync::Arc::from(
                    "no such equipment",
                )),
                signal: ControlSignal::PowerSetpoint {
                    active_power_w: 1.0000,
                    reactive_power_var: None,
                    min_soc: None,
                    max_soc: None,
                },
                priority: PriorityTier::Schedule,
            });
        }

        fn output_variables(&self) -> &[TelemetryField] {
            &[]
        }
    }

    let bestest = repo_root().join("tests/fixtures/bestest/600.toml");
    let mut dwelling = Dwelling::from_toml_config(&bestest).expect("the bestest dwelling builds");
    dwelling
        .add_actor(Box::new(RejectingActor))
        .expect("the actor registers");

    // The fixture's two construction-time envelope warnings are not the
    // step stream's: one drain before the loop leaves every drain inside it
    // exactly that step's rejected-signal warning.
    dwelling.take_warnings();

    for step in 0..1_100 {
        dwelling.step().expect("the step runs");
        let drained = dwelling.take_warnings();
        assert_eq!(
            drained.len(),
            1,
            "step {step}: the drain returns exactly that step's warning, got {:?}",
            drained
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            drained[0].step_index,
            Some(step),
            "step {step}: the warning carries the step it was raised in"
        );
    }

    let result = dwelling.finish().expect("finish runs");
    // The fixture's one construction-time envelope warning (the site
    // type's; the shielding's warning carries the resolved default's
    // class and the synthetic building names no facility type for the
    // default to read) precedes the actor's 1 100 rejected-signal
    // warnings: the record keeps its first 1 000 and counts 101 dropped
    // past it. The per-step drains above are the pinned semantics: every
    // step's warning reached its step's drain, the 1 001st included.
    assert_eq!(
        result.warnings.len(),
        1_000,
        "the run record holds its first 1 000 warnings"
    );
    assert_eq!(result.warnings_dropped, 101, "the drop count is the rest");
}

/// `engine_run_preserves_invariant_violation_type` (through the engine
/// seam): an
/// `InvariantViolation { check_name: "thermal_balance" }` from the run
/// reaches the engine's caller as that value.
#[test]
#[cfg(debug_assertions)]
fn engine_run_preserves_invariant_violation_type() {
    use std::fs;

    // The thermal-invariant wiring test's synthetic RC dwelling.
    const TOML: &str = r#"building_id = 999

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res = 60
duration = 3600
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 129.6

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 30.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.5

internal_gains_w = 200.0
internal_gains_constant = true
internal_gains_sensible_fraction = 1.0
internal_gains_radiant_fraction = 0.3

[[boundaries]]
id = "test-wall"
boundary_type = "Wall"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[[boundaries]]
id = "test-floor"
boundary_type = "Floor"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[schedule]
occupancy = 0.0
occupants_present = false
"#;

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dwelling.toml");
    fs::write(&path, TOML).expect("write toml");
    let mut dwelling = Dwelling::from_toml_config(&path).expect("dwelling builds");
    dwelling.set_thermal_invariant_failure_for_test();

    let sim_config = dwelling.sim_config().clone();
    let err = SimulationEngine::new()
        .run_dwelling(&mut dwelling, &sim_config)
        .expect_err("the invariant violation reaches the engine's caller");
    match &err {
        HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance" => {}
        other => panic!("expected InvariantViolation(thermal_balance), got {other:?}"),
    }
}

/// `provenance_carries_equipment_zone_mapping`: for the leap-year inputs,
/// `Provenance.equipment` holds one entry per equipment whose `zone`,
/// `zone_type`, `ocv_source` and `notes` equal the preamble lines the
/// pinned `diagnostics_cz4a.csv` header recorded.
#[test]
fn provenance_carries_equipment_zone_mapping() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.step().expect("step runs");
    let result = dwelling.finish().expect("finish runs");

    let pin = repo_root().join("crates/hares-core/tests/fixtures/diagnostics_cz4a.csv");
    let pin = std::fs::read_to_string(pin).expect("the pinned diagnostics CSV reads");
    let pinned_zone_lines: Vec<&str> = pin
        .lines()
        .filter(|line| line.starts_with("# eq zone_id: "))
        .collect();
    let pinned_cop_lines: Vec<&str> = pin
        .lines()
        .filter(|line| line.starts_with("# eq cop_"))
        .collect();

    assert_eq!(
        result.provenance.equipment.len(),
        dwelling.equipment().len(),
        "one entry per equipment"
    );
    // The pinned preamble carried one zone line per equipment that names a
    // zone (zoneless equipment had no line to write).
    let zoned = result
        .provenance
        .equipment
        .iter()
        .filter(|entry| entry.zone.is_some())
        .count();
    assert_eq!(
        zoned,
        pinned_zone_lines.len(),
        "one zone line per zoned equipment, matching the pinned preamble"
    );
    for entry in &result.provenance.equipment {
        let Some(zone) = entry.zone else {
            continue;
        };
        let zone_line = format!("# eq zone_id: {} -> ZoneId({})", entry.name, zone.0);
        let zone_line = match &entry.zone_type {
            Some(zone_type) => format!("{zone_line}, zone_type={zone_type}"),
            None => zone_line,
        };
        assert!(
            pinned_zone_lines.iter().any(|line| *line == zone_line),
            "the entry's zone line '{zone_line}' is in the pinned preamble"
        );
    }

    // The pinned cop lines are the heat pump water heater's notes.
    let hpwh = result
        .provenance
        .equipment
        .iter()
        .find(|entry| entry.name == "Heat Pump Water Heater")
        .expect("the heat pump water heater is in the provenance");
    assert_eq!(
        hpwh.zone_type.as_deref(),
        Some("living space"),
        "the zone type equals the pinned preamble's"
    );
    let note_lines: Vec<String> = hpwh.notes.clone();
    for pinned in &pinned_cop_lines {
        assert!(
            note_lines.iter().any(|line| line == pinned),
            "the pinned line '{pinned}' is among the HPWH's notes"
        );
    }
}

/// `provenance_records_callback_destination_marker`: a run with a `Callback`
/// report (the test callback attached) records `Destination::Callback`
/// for that report in `Provenance.config.reports`.
#[test]
fn provenance_records_callback_destination_marker() {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct CountingCallback {
        deliveries: StdArc<AtomicU64>,
    }

    impl hares_io::output::callback::BatchCallback for CountingCallback {
        fn on_batch(
            &mut self,
            _context: &hares_io::output::callback::BatchContext,
            _batch: &RecordBatch,
        ) -> Result<(), hares_io::output::callback::CallbackError> {
            self.deliveries.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling
        .attach_callback(
            "frame",
            Box::new(CountingCallback {
                deliveries: StdArc::new(AtomicU64::new(0)),
            }),
        )
        .expect("the callback attaches");
    dwelling.step().expect("step runs");
    let result = dwelling.finish().expect("finish runs");

    let frame = result
        .provenance
        .settings
        .sim_config
        .reports
        .iter()
        .find(|report| report.name == "frame")
        .expect("the report is in the recorded config");
    assert!(
        matches!(frame.destination, hares_io::Destination::Callback),
        "the recorded config's report carries the callback marker, got: {:?}",
        frame.destination
    );
}

/// A construction-time warning (the tariff parse warning) is in the run
/// record with its source, and the run's `warnings_dropped` starts at 0.
#[test]
fn run_result_warnings_carry_sources() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    use hares_tariff::ElectricTariff;
    let tariff = ElectricTariff {
        parse_warnings: vec!["test tariff parse warning".to_string()],
        ..ElectricTariff::default()
    };
    dwelling
        .set_tariff(tariff, chrono_tz::Tz::UTC)
        .expect("set_tariff with a parse warning succeeds");
    dwelling.step().expect("step runs");
    let result = dwelling.finish().expect("finish runs");
    assert!(
        result
            .warnings
            .iter()
            .any(|warning: &Warning| warning.message.contains("test tariff parse warning")),
        "the run record carries the construction-time warning: {:?}",
        result.warnings
    );
    assert_eq!(result.warnings_dropped, 0);
    let _ = StdDuration::ZERO;
}

/// A finished dwelling that loads a mid-run checkpoint is not stuck: the
/// restore replaces all of a run's state, the finished-run latch included,
/// so the dwelling steps again and its `finish()` returns the restored
/// run's own result, never the pre-restore Arc.
#[test]
fn finished_dwelling_loads_a_mid_run_checkpoint_and_finishes_anew() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    for _ in 0..10 {
        dwelling.step().expect("step runs");
    }
    let checkpoint = dwelling.save_checkpoint().expect("save checkpoint");
    for _ in 10..96 {
        dwelling.step().expect("step runs");
    }
    let stale = dwelling.finish().expect("finish runs");
    assert!(
        dwelling.step().is_err(),
        "precondition: the finished latch holds before the restore"
    );

    dwelling
        .load_checkpoint(checkpoint)
        .expect("a finished dwelling loads a mid-run checkpoint");
    dwelling
        .step()
        .expect("the restored dwelling steps: the latch is gone");
    for _ in 11..96 {
        dwelling.step().expect("step runs");
    }
    let fresh = dwelling.finish().expect("finish after the restore runs");
    assert!(
        !std::sync::Arc::ptr_eq(&stale, &fresh),
        "finish() after the restore returns the restored run's own result, not the stale Arc"
    );
}

/// The same rule for the partial prior-segment restore: a finished
/// dwelling accepts `restore_building_state`, steps again from the
/// segment start and finalises its own result.
#[test]
fn finished_dwelling_restores_building_state_and_finishes_anew() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    for _ in 0..10 {
        dwelling.step().expect("step runs");
    }
    let checkpoint = dwelling.save_checkpoint().expect("save checkpoint");
    for _ in 10..96 {
        dwelling.step().expect("step runs");
    }
    let stale = dwelling.finish().expect("finish runs");

    dwelling
        .restore_building_state(&checkpoint)
        .expect("a finished dwelling accepts the building-state restore");
    dwelling
        .step()
        .expect("the restored dwelling steps: the latch is gone");
    for _ in 1..96 {
        dwelling.step().expect("step runs");
    }
    let fresh = dwelling.finish().expect("finish after the restore runs");
    assert!(
        !std::sync::Arc::ptr_eq(&stale, &fresh),
        "finish() after the restore returns the re-run's own result, not the stale Arc"
    );
}

/// The build identity's digest probe: the one-variable actor the
/// catalogue digest test registers. It declares a single state flag,
/// exposes the telemetry map built from that declaration and dispatches
/// nothing.
struct SingleVariableActor {
    telemetry: Telemetry,
}

const DIGEST_PROBE_VARIABLES: &[TelemetryField] = &[TelemetryField::state_flag(
    "digest_probe_flag",
    "one declared variable for the catalogue digest probe",
)];

impl SingleVariableActor {
    fn new() -> Self {
        Self {
            telemetry: Telemetry::from_fields(DIGEST_PROBE_VARIABLES),
        }
    }
}

impl Actor for SingleVariableActor {
    fn name(&self) -> &str {
        "digest_probe"
    }

    fn output_variables(&self) -> &[TelemetryField] {
        DIGEST_PROBE_VARIABLES
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {}
}

/// Decodes the 64 lowercase hex chars of a catalogue digest into the 32
/// bytes the provenance field records.
fn hex_digest_bytes(hex: &str) -> [u8; 32] {
    assert_eq!(hex.len(), 64, "the digest is 64 hex chars: {hex}");
    let value = |c: u8| -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            other => panic!("the digest is lowercase hex: {other}"),
        }
    };
    let bytes = hex.as_bytes();
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = (value(bytes[index * 2]) << 4) | value(bytes[index * 2 + 1]);
    }
    digest
}

/// `provenance_carries_build_identity`: a finished run's provenance
/// records the build identity (equal field for field to the compile-time
/// record) and the catalogue's digest (the hex the catalogue's digest
/// prints, decoded).
#[test]
fn provenance_carries_build_identity() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let result = dwelling.simulate().expect("the run finishes");

    let info = hares_core::BuildInfo::get();
    let build = &result.provenance.build;
    assert_eq!(build.version, info.version);
    assert_eq!(build.commit, info.commit);
    assert_eq!(build.dirty, info.dirty);
    assert_eq!(build.profile, info.profile);
    assert_eq!(build.features, info.features);
    assert_eq!(build.rustc, info.rustc);

    assert_eq!(
        result.provenance.catalogue_digest,
        hex_digest_bytes(&dwelling.catalogue().digest()),
        "the provenance digest is the catalogue's digest"
    );
}

/// `provenance_digest_equals_schema_metadata`: the provenance digest,
/// hex-encoded, equals the report schema's `hares:catalogue_digest`
/// metadata.
#[test]
fn provenance_digest_equals_schema_metadata() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let result = dwelling.simulate().expect("the run finishes");

    let batch = result.reports[0]
        .batches()
        .first()
        .expect("the memory report holds batches");
    let schema = batch.schema();
    let metadata = schema
        .metadata
        .get("hares:catalogue_digest")
        .expect("the schema carries the catalogue digest metadata");
    let hex: String = result
        .provenance
        .catalogue_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hex, *metadata);
}

/// `two_runs_carry_the_same_build_identity`: two runs of one build record
/// the same identity, field for field.
#[test]
fn two_runs_carry_the_same_build_identity() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut first = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let mut second = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let first = first.simulate().expect("the run finishes");
    let second = second.simulate().expect("the run finishes");
    assert_eq!(first.provenance.build, second.provenance.build);
    assert_eq!(
        first.provenance.catalogue_digest, second.provenance.catalogue_digest,
        "one catalogue per input set: the digests equal too"
    );
}

/// `catalogue_digest_changes_when_a_variable_is_added`: a test actor
/// declaring one variable moves the run's catalogue digest.
#[test]
fn catalogue_digest_changes_when_a_variable_is_added() {
    let toml = sim_toml(&memory_report("frame", 2));
    let mut plain = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let plain = plain.simulate().expect("the run finishes");

    let mut extended = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    extended
        .add_actor(Box::new(SingleVariableActor::new()))
        .expect("the actor registers");
    let extended = extended.simulate().expect("the run finishes");

    assert_ne!(
        plain.provenance.catalogue_digest, extended.provenance.catalogue_digest,
        "the declared variable is a catalogue change"
    );
}

/// `catalogue_digest_equal_for_two_builds_of_one_blueprint`: two builds of
/// one blueprint record the same catalogue digest.
#[test]
fn catalogue_digest_equal_for_two_builds_of_one_blueprint() {
    let toml = sim_toml(&memory_report("frame", 2));
    let first = DwellingBlueprint::from_config(dwelling_config(&toml))
        .expect("blueprint")
        .build()
        .expect("the dwelling builds");
    let second = DwellingBlueprint::from_config(dwelling_config(&toml))
        .expect("blueprint")
        .build()
        .expect("the dwelling builds");
    assert_eq!(first.catalogue().digest(), second.catalogue().digest());
}
