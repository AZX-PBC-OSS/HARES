// The replay export: a run's resolved configuration, inputs, seeds and
// build identity, exported so a construction from the export alone
// reproduces the run's frames bitwise.
//
// RED: written against the export surface before it exists, so this file
// does not even compile against the parent commit.

use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use hares_core::replay;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, RunResult, derive_dwelling_rng, rng};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherFormat, WeatherInput};
use hares_types::{HaresError, ReplayBlocker};
use serde_json::json;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The consumer's inputs (`02-measurement.md` M-2 `consumer_shape_900s`)
/// copied into `dir`: the ResStock home.xml, its schedule, the station
/// weather and the replaced envelope materials file. The config the tests
/// run reads the copies, so the originals can be deleted and a replay
/// still has everything it needs inside the export. `initialization_duration`
/// is the run's own (the golden config's zero-length warm-up where the
/// horizon fits a warm-up day, `None` where it does not).
fn consumer_shape_config(
    dir: &Path,
    duration_s: i64,
    initialization_duration: Option<StdDuration>,
) -> DwellingConfig {
    let fixture = repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227");
    let weather = repo_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv");
    let materials = repo_root().join("tests/fixtures/golden/consumer_shape_envelope_materials.csv");
    std::fs::create_dir_all(dir).expect("the input directory creates");
    std::fs::copy(fixture.join("home.xml"), dir.join("home.xml")).expect("the home xml copies");
    std::fs::copy(
        fixture.join("in.schedules.csv"),
        dir.join("in.schedules.csv"),
    )
    .expect("the schedule copies");
    std::fs::copy(&weather, dir.join("G0600770_2018.csv")).expect("the weather copies");
    std::fs::copy(
        &materials,
        dir.join("consumer_shape_envelope_materials.csv"),
    )
    .expect("the materials copy");

    let sim_config = SimulationConfig::from_toml(&format!(
        r#"
start_time = "2018-01-01T00:00:00-08:00"
duration = {duration_s}
time_res = 900
metrics = true
master_seed = 0

[[reports]]
name = "frame"
selection = {{ verbosity = 2 }}
destination = "memory"
"#
    ))
    .expect("the sim config parses");

    DwellingConfig {
        hpxml: InputSource::Path(dir.join("home.xml")),
        schedule: Some(InputSource::Path(dir.join("in.schedules.csv"))),
        weather: WeatherInput::Path(dir.join("G0600770_2018.csv")),
        defaults: DefaultsSource {
            base: Some(repo_root().join("defaults")),
            files: [(
                "envelope/Envelope Materials.csv".to_string(),
                InputSource::Path(dir.join("consumer_shape_envelope_materials.csv")),
            )]
            .into_iter()
            .collect(),
        },
        settings: DwellingSettings::new(
            sim_config,
            Some(json!({ "Gas Water Heater": { "ua_w_per_k": 2.0 } })),
            176_227,
            initialization_duration,
            None,
        ),
    }
}

fn run(config: DwellingConfig) -> std::sync::Arc<RunResult> {
    Dwelling::from_config(config)
        .expect("the dwelling builds")
        .simulate()
        .expect("the run completes")
}

fn memory_batches(result: &RunResult) -> Vec<RecordBatch> {
    result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect()
}

/// Every column of every batch compared bitwise: float64 columns by their
/// bit patterns, every other column by its bytes.
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

fn assert_runs_bitwise_equal(original: &RunResult, replayed: &RunResult, context: &str) {
    assert_batches_bitwise_equal(
        &memory_batches(original),
        &memory_batches(replayed),
        &format!("{context}: the reports on bits"),
    );
    assert_eq!(
        original.metrics, replayed.metrics,
        "{context}: the metrics on bits"
    );
}

/// The export's layout: the files a replay reads are all inside the
/// export directory.
fn exported_file(dir: &Path, relative: &str) -> PathBuf {
    dir.join(relative)
}

/// `replay_export_reproduces_frame_bitwise`: the inputs copied into a
/// temporary directory, run, export, delete that input directory, replay:
/// every report and the metrics equal the original on bits. The replay
/// constructs from the export alone.
#[test]
fn replay_export_reproduces_frame_bitwise() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 48 * 3600, Some(StdDuration::ZERO));
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    drop(inputs);

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    let result = replayed
        .build()
        .expect("the replay builds")
        .simulate()
        .expect("the replay runs");
    assert_runs_bitwise_equal(&original, &result, "replay vs original");
}

/// `replay_export_writes_dwelling_settings`: `replay.toml` carries
/// `bldg_id` 176227 and `overrides.json` carries the `Gas Water Heater`
/// override; a run with `HpxmlDataPatches { number_of_bedrooms: Some(3.0) }`
/// round-trips its patch.
#[test]
fn replay_export_writes_dwelling_settings() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let mut config = consumer_shape_config(inputs.path(), 4 * 3600, None);
    config.settings.patches = Some(hares_io::HpxmlDataPatches {
        number_of_bedrooms: Some(3.0),
    });
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");

    let toml_text =
        std::fs::read_to_string(exported_file(export.path(), "replay.toml")).expect("replay.toml");
    assert!(
        toml_text.contains("176227"),
        "replay.toml carries the bldg_id: {toml_text}"
    );
    let overrides_text = std::fs::read_to_string(exported_file(export.path(), "overrides.json"))
        .expect("overrides.json");
    let overrides: serde_json::Value = serde_json::from_str(&overrides_text).expect("the json");
    assert_eq!(
        overrides["Gas Water Heater"]["ua_w_per_k"],
        json!(2.0),
        "overrides.json carries the override"
    );

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    assert_eq!(
        replayed.config.settings.bldg_id, 176_227,
        "the bldg_id round trips"
    );
    assert_eq!(
        replayed.config.settings.patches,
        Some(hares_io::HpxmlDataPatches {
            number_of_bedrooms: Some(3.0)
        }),
        "the patches round trip"
    );
}

/// `replay_overrides_null_round_trips`: an override map holding a JSON
/// `null` value replays equal (TOML cannot encode it, `overrides.json`
/// carries it).
#[test]
fn replay_overrides_null_round_trips() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let mut config = consumer_shape_config(inputs.path(), 4 * 3600, None);
    config.settings.overrides = Some(json!({
        "Gas Water Heater": { "ua_w_per_k": 2.0, "pilot_power_w": null },
    }));
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    let result = replayed
        .build()
        .expect("the replay builds")
        .simulate()
        .expect("the replay runs");
    assert_eq!(
        replayed.config.settings.overrides, original.provenance.settings.overrides,
        "the null-valued override entry round trips"
    );
    assert_runs_bitwise_equal(&original, &result, "null-override replay vs original");
}

/// `replay_of_text_weather_round_trips`: the cz4a EPW passed as
/// `InputSource::Text` with format EPW exports `weather_format = "epw"`,
/// and the replay's frame equals the original on bits.
#[test]
fn replay_of_text_weather_round_trips() {
    let fixture = repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let weather_text = std::fs::read_to_string(fixture.join("weather.epw")).expect("the epw reads");
    let sim_config = SimulationConfig::from_toml(
        r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = true
master_seed = 0

[[reports]]
name = "frame"
selection = { verbosity = 2 }
destination = "memory"
"#,
    )
    .expect("the sim config parses");
    let config = DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Text {
            content: weather_text.into(),
            format: WeatherFormat::Epw,
        },
        defaults: DefaultsSource::directory(repo_root().join("defaults")),
        settings: DwellingSettings::new(sim_config, None, 1, Some(StdDuration::ZERO), None),
    };
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    let toml_text =
        std::fs::read_to_string(exported_file(export.path(), "replay.toml")).expect("replay.toml");
    assert!(
        toml_text.contains("weather_format = \"epw\""),
        "the text weather exports in the format it was parsed as: {toml_text}"
    );

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    let result = replayed
        .build()
        .expect("the replay builds")
        .simulate()
        .expect("the replay runs");
    assert_runs_bitwise_equal(&original, &result, "text-weather replay vs original");
}

/// `replay_with_attached_tariff_reproduces_billing`: the run with
/// `tests/fixtures/urdb/flat_rate.json` attached in `America/Denver`
/// exports `tariff.json`, and the replay's billing summaries equal the
/// original's on bits.
#[test]
fn replay_with_attached_tariff_reproduces_billing() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 4 * 3600, None);
    let mut original_dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    let tariff_json =
        std::fs::read_to_string(repo_root().join("tests/fixtures/urdb/flat_rate.json"))
            .expect("the tariff file reads");
    let tariff = hares_tariff::parse_urdb(&tariff_json).expect("the tariff parses");
    original_dwelling
        .set_tariff(tariff, chrono_tz::Tz::America__Denver)
        .expect("the tariff attaches");
    let original = original_dwelling.simulate().expect("the run completes");

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    assert!(
        exported_file(export.path(), "tariff.json").is_file(),
        "tariff.json is written"
    );

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    let mut replay_dwelling = replayed.build().expect("the replay builds");
    let result = replay_dwelling.simulate().expect("the replay runs");

    assert_billing_summaries_bitwise_equal(
        &original_dwelling,
        &replay_dwelling,
        "tariff replay vs original",
    );
    assert_runs_bitwise_equal(&original, &result, "tariff replay vs original");
}

/// The billing summaries of two dwellings, period by period, on bits:
/// the net bill, the energy, fixed and demand charges, and both energies.
fn assert_billing_summaries_bitwise_equal(original: &Dwelling, replayed: &Dwelling, context: &str) {
    let original_summaries = original.billing_summaries();
    let replay_summaries = replayed.billing_summaries();
    assert_eq!(
        original_summaries.len(),
        replay_summaries.len(),
        "{context}: the same period count"
    );
    for (period, (left, right)) in original_summaries.iter().zip(replay_summaries).enumerate() {
        assert_eq!(
            left.net_bill_usd.to_bits(),
            right.net_bill_usd.to_bits(),
            "{context}: period {period}: the net bill on bits"
        );
        assert_eq!(
            left.energy_charge_usd.to_bits(),
            right.energy_charge_usd.to_bits(),
            "{context}: period {period}: the energy charge on bits"
        );
        assert_eq!(
            left.fixed_charge_usd.to_bits(),
            right.fixed_charge_usd.to_bits(),
            "{context}: period {period}: the fixed charge on bits"
        );
        assert_eq!(
            left.demand_charge_usd.to_bits(),
            right.demand_charge_usd.to_bits(),
            "{context}: period {period}: the demand charge on bits"
        );
        assert_eq!(
            left.total_import_kwh.to_bits(),
            right.total_import_kwh.to_bits(),
            "{context}: period {period}: the import energy on bits"
        );
        assert_eq!(
            left.total_export_kwh.to_bits(),
            right.total_export_kwh.to_bits(),
            "{context}: period {period}: the export energy on bits"
        );
    }
}

/// `replay_of_a_mid_run_tariff_attach_reproduces_billing`: a tariff
/// attached after step 0 exports with the step it attached at, and the
/// replay attaches at that recorded step (the steps before it replay
/// tariff-less): the billing summaries equal the original's on bits, the
/// prorated first period included.
#[test]
fn replay_of_a_mid_run_tariff_attach_reproduces_billing() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 4 * 3600, None);
    let mut original_dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    for _ in 0..4 {
        original_dwelling.step().expect("the step runs");
    }
    let tariff_json =
        std::fs::read_to_string(repo_root().join("tests/fixtures/urdb/flat_rate.json"))
            .expect("the tariff file reads");
    let tariff = hares_tariff::parse_urdb(&tariff_json).expect("the tariff parses");
    original_dwelling
        .set_tariff(tariff, chrono_tz::Tz::America__Denver)
        .expect("the tariff attaches");
    let original = original_dwelling.simulate().expect("the run completes");

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    let toml_text =
        std::fs::read_to_string(exported_file(export.path(), "replay.toml")).expect("replay.toml");
    assert!(
        toml_text.contains("tariff_attach_step"),
        "the manifest carries the attach step: {toml_text}"
    );

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    assert_eq!(
        replayed
            .tariff
            .as_ref()
            .expect("the tariff round trips")
            .attach_step,
        4,
        "the recorded attach step round trips"
    );
    let mut replay_dwelling = replayed.build().expect("the replay builds");
    let result = replay_dwelling.simulate().expect("the replay runs");

    assert_billing_summaries_bitwise_equal(
        &original_dwelling,
        &replay_dwelling,
        "mid-run attach replay vs original",
    );
    assert_runs_bitwise_equal(&original, &result, "mid-run attach replay vs original");
}

/// `replay_of_a_replaced_tariff_is_refused`: a tariff replaced mid-run
/// closes the outgoing period with the outgoing tariff's accruals through
/// the run's own steps, which no export carries: the export returns
/// `NotReplayable` naming the replacement and writes nothing.
#[test]
fn replay_of_a_replaced_tariff_is_refused() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 4 * 3600, None);
    let mut dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    let flat = hares_tariff::parse_urdb(
        &std::fs::read_to_string(repo_root().join("tests/fixtures/urdb/flat_rate.json"))
            .expect("the flat rate reads"),
    )
    .expect("the flat rate parses");
    dwelling
        .set_tariff(flat, chrono_tz::Tz::America__Denver)
        .expect("the first tariff attaches");
    for _ in 0..4 {
        dwelling.step().expect("the step runs");
    }
    let tou = hares_tariff::parse_urdb(
        &std::fs::read_to_string(repo_root().join("tests/fixtures/urdb/pge_e_tou_c.json"))
            .expect("the tou reads"),
    )
    .expect("the tou parses");
    dwelling
        .set_tariff(tou, chrono_tz::Tz::America__Denver)
        .expect("the replacement attaches");
    let result = dwelling.simulate().expect("the run completes");
    reject_returns(&result, &[ReplayBlocker::TariffReplaced]);
}

/// `replay_records_the_exporting_build`: `Replay.recorded_build` equals
/// the exporting run's `BuildInfo` field for field.
#[test]
fn replay_records_the_exporting_build() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");

    let build = &original.provenance.build;
    let recorded = &replayed.recorded_build;
    assert_eq!(build.version, recorded.version, "the version");
    assert_eq!(build.commit, recorded.commit.as_deref(), "the commit");
    assert_eq!(build.dirty, recorded.dirty, "the dirty flag");
    assert_eq!(build.profile, recorded.profile, "the profile");
    assert_eq!(build.features, recorded.features.as_slice(), "the features");
    assert_eq!(build.rustc, recorded.rustc, "the rustc line");
}

/// `replay_export_fails_if_input_file_changed_since_parse`: the same
/// inputs copied into a temporary directory and run 1 h; one byte is
/// appended to the copied HPXML before `export_replay`, which fails
/// naming that file.
#[test]
fn replay_export_fails_if_input_file_changed_since_parse() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let original = run(config);

    let mut hpxml = std::fs::read(inputs.path().join("home.xml")).expect("the copy reads");
    hpxml.push(b' ');
    std::fs::write(inputs.path().join("home.xml"), &hpxml).expect("the byte appends");

    let export = tempfile::tempdir().expect("the export directory");
    let err = original
        .export_replay(export.path())
        .expect_err("the export of a changed input fails");
    let message = err.to_string();
    assert!(
        message.contains("home.xml"),
        "the error names the changed file: {message}"
    );
}

/// `replay_load_fails_on_defaults_digest_mismatch`: an export of the same
/// 1 h run loaded with `defaults_base` a temporary copy of the defaults
/// tree with one byte of a file the load reads changed fails naming the
/// digest mismatch. (The replaced file's own log entry carries the
/// replacement's bytes, so the base tree's copy of it is not what the
/// digest composes; the edited file here is one the digest reads.)
#[test]
fn replay_load_fails_on_defaults_digest_mismatch() {
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let original = run(config);

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");

    let copied = tempfile::tempdir().expect("the copied defaults directory");
    copy_tree(&repo_root().join("defaults"), copied.path());
    let profiles = copied.path().join("Default Schedule Parameters.csv");
    let mut bytes = std::fs::read(&profiles).expect("the profiles read");
    bytes[0] = bytes[0].wrapping_add(1);
    std::fs::write(&profiles, &bytes).expect("the byte changes");

    let err = replay::load(export.path(), copied.path())
        .expect_err("the load over a changed defaults tree fails");
    let message = err.to_string();
    assert!(
        message.contains("digest"),
        "the error names the digest mismatch: {message}"
    );
}

/// `replay_export_rejects_mutated_run`, one case per `ReplayBlocker` on
/// the same inputs over 1 h: a blueprint with one `remove_equipment`, a
/// dwelling with one `remove_equipment` after build, one
/// `add_actor_by_name`, one `apply_control`, one `load_checkpoint`; each
/// `export_replay` returns `NotReplayable` naming exactly that category
/// and writes nothing into the directory.
#[test]
fn replay_export_rejects_mutated_run() {
    // BlueprintEdited: the builder's mutator ran before the freeze.
    let inputs = tempfile::tempdir().expect("the inputs directory");
    let blueprint = {
        let mut config = consumer_shape_config(inputs.path(), 3600, None);
        config.settings.overrides = None;
        let mut builder =
            hares_core::DwellingBlueprintBuilder::from_config(config).expect("the builder builds");
        builder
            .remove_equipment("Gas Water Heater")
            .expect("the spec removes");
        builder.freeze().expect("the blueprint freezes")
    };
    let mut blueprint_dwelling = blueprint.build().expect("the dwelling builds");
    blueprint_dwelling.step().expect("the step runs");
    let blueprint_result = blueprint_dwelling.finish().expect("the finish runs");
    reject_returns(&blueprint_result, &[ReplayBlocker::BlueprintEdited]);

    // EquipmentChanged: the dwelling's own equipment changes after build.
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let mut equipped = Dwelling::from_config(config).expect("the dwelling builds");
    equipped
        .remove_equipment("Gas Water Heater")
        .expect("the equipment removes");
    equipped.step().expect("the step runs");
    let equipped_result = equipped.finish().expect("the finish runs");
    reject_returns(&equipped_result, &[ReplayBlocker::EquipmentChanged]);

    // CallerActor: an actor registered by the caller.
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let mut acted = Dwelling::from_config(config).expect("the dwelling builds");
    acted
        .add_actor_by_name(
            &hares_core::ActorRegistry::new(),
            hares_core::ActorConfig::new("occupant", "Occupant"),
        )
        .expect("the actor registers");
    acted.step().expect("the step runs");
    let acted_result = acted.finish().expect("the finish runs");
    reject_returns(&acted_result, &[ReplayBlocker::CallerActor]);

    // ExternalInput: a control signal pushed in by the caller.
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let mut controlled = Dwelling::from_config(config).expect("the dwelling builds");
    controlled.step().expect("the step runs");
    controlled.apply_control(
        "Gas Water Heater",
        hares_types::ControlSignal::LoadFraction { fraction: 0.5 },
    );
    let controlled_result = controlled.finish().expect("the finish runs");
    reject_returns(&controlled_result, &[ReplayBlocker::ExternalInput]);

    // StateRestored: a checkpoint loaded.
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let mut saved = Dwelling::from_config(config).expect("the dwelling builds");
    saved.step().expect("the step runs");
    let checkpoint = saved.save_checkpoint().expect("the checkpoint saves");
    let config = consumer_shape_config(inputs.path(), 3600, None);
    let mut restored = Dwelling::from_config(config).expect("the dwelling builds");
    restored
        .load_checkpoint(checkpoint)
        .expect("the checkpoint loads");
    let restored_result = restored.finish().expect("the finish runs");
    reject_returns(&restored_result, &[ReplayBlocker::StateRestored]);
}

/// The shared rejection pin: the export returns `NotReplayable` naming
/// exactly the given categories, and the directory holds nothing.
fn reject_returns(result: &RunResult, expected: &[ReplayBlocker]) {
    let export = tempfile::tempdir().expect("the export directory");
    let err = result
        .export_replay(export.path())
        .expect_err("a mutated run's export fails");
    match &err {
        HaresError::NotReplayable { blockers } => {
            assert_eq!(blockers, expected, "exactly the recorded categories");
        }
        other => panic!("expected NotReplayable, got: {other:?}"),
    }
    let written: Vec<String> = std::fs::read_dir(export.path())
        .expect("the directory reads")
        .map(|entry| {
            entry
                .expect("the entry reads")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        written.is_empty(),
        "a rejected export writes nothing: {written:?}"
    );
}

/// `replay_derivation_chain_reproduces_every_stream`: the r43 review's
/// derivation claim verified end to end. The export carries the master
/// seed (in the config) and the building id; the replay rebuilds every
/// random stream from them the construction derives them (the dwelling's
/// own stream 0, the EV driver's streams from it), and the cz5a EV
/// fixture's replayed frames are bitwise the original's: every derived
/// stream drew the same words.
#[test]
fn replay_derivation_chain_reproduces_every_stream() {
    let fixture = repo_root().join("tests/fixtures/parity/cz5a_ev_charging");
    let inputs = tempfile::tempdir().expect("the inputs directory");
    std::fs::create_dir_all(&inputs).expect("the input directory creates");
    std::fs::copy(
        fixture.join("building.xml"),
        inputs.path().join("building.xml"),
    )
    .expect("the hpxml copies");
    std::fs::copy(
        fixture.join("schedule.csv"),
        inputs.path().join("schedule.csv"),
    )
    .expect("the schedule copies");
    std::fs::copy(
        fixture.join("weather.epw"),
        inputs.path().join("weather.epw"),
    )
    .expect("the weather copies");

    let sim_config = SimulationConfig::from_toml(
        r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 86400
time_res = 900
metrics = true
master_seed = 7

[[reports]]
name = "frame"
selection = { verbosity = 2 }
destination = "memory"
"#,
    )
    .expect("the sim config parses");
    let config = DwellingConfig {
        hpxml: InputSource::Path(inputs.path().join("building.xml")),
        schedule: Some(InputSource::Path(inputs.path().join("schedule.csv"))),
        weather: WeatherInput::Path(inputs.path().join("weather.epw")),
        defaults: DefaultsSource::directory(repo_root().join("defaults")),
        settings: DwellingSettings::new(sim_config, None, 7, Some(StdDuration::ZERO), None),
    };
    let mut original_dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    let original_seed = original_dwelling.rng.get_seed();
    let original_stream = original_dwelling.rng.get_stream();
    let original_words = original_dwelling.rng.get_word_pos();
    let original = original_dwelling.simulate().expect("the run completes");

    let export = tempfile::tempdir().expect("the export directory");
    original
        .export_replay(export.path())
        .expect("the export writes");
    drop(inputs);

    let replayed =
        replay::load(export.path(), &repo_root().join("defaults")).expect("the export loads");
    // The export alone carries the seed derivation's inputs: the master
    // seed and the building id derive the dwelling's stream the
    // construction does, and the replayed dwelling holds it.
    let master = replayed.config.settings.sim_config.master_seed;
    let bldg_id = replayed.config.settings.bldg_id;
    let derived = derive_dwelling_rng(master, bldg_id);
    assert_eq!(
        original_seed,
        derived.get_seed(),
        "the original dwelling's stream is the (master, bldg_id) derivation"
    );
    assert_eq!(
        original_stream,
        derived.get_stream(),
        "the dwelling's own stream is stream 0"
    );

    let mut replay_dwelling = replayed.build().expect("the replay builds");
    assert_eq!(
        replay_dwelling.rng.get_seed(),
        derived.get_seed(),
        "the replayed dwelling's stream derives from the export's seed alone"
    );
    assert_eq!(
        replay_dwelling.rng.get_word_pos(),
        original_words,
        "the replayed dwelling drew the same words through its build"
    );
    let result = replay_dwelling.simulate().expect("the replay runs");
    assert_runs_bitwise_equal(&original, &result, "ev replay vs original");
}

/// Every defaults file under `dir`, copied recursively (the tree is 138
/// MB; the copy is what the fleet tests' shared-tree pins do).
fn copy_tree(source: &Path, target: &Path) {
    for entry in std::fs::read_dir(source).expect("the source reads") {
        let entry = entry.expect("the entry reads");
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&destination).expect("the directory creates");
            copy_tree(&path, &destination);
        } else {
            std::fs::copy(&path, &destination).expect("the file copies");
        }
    }
}

#[cfg(test)]
mod construction_pins {
    use super::*;

    /// The rng module's stream constants the derivation notes name: the
    /// dwelling's own stream 0, the EV drivers' streams from 1.
    #[test]
    fn the_derivation_constants_are_the_notes_ones() {
        assert_eq!(rng::RNG_STREAM_DWELLING, 0);
        assert_eq!(rng::RNG_STREAM_EV_DRIVER_BASE, 1);
    }
}
