//! Each stochastic event load in a dwelling draws from its own random
//! stream, keyed by the dwelling seed and the load's stable identity.
//!
//! The probes are raw event-load specs with identical parameters (a
//! constant start probability strictly inside (0, 1), so every idle step
//! draws). Each load's per-step on/off pattern is a direct read of its draw
//! sequence; identical configs make any difference between two patterns
//! attributable to the streams alone.

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::{FuelType, HaresError, telemetry_keys as tk};
use serde_json::json;

const PROBE_A: &str = "EventBasedLoad";
const PROBE_B: &str = "Microwave";
const STEPS: usize = 96;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

fn dwelling_config(hpxml: &Path) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        schedule_path: Some(fixture_dir().join("in.schedules.csv")),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(10 * 3600)
                .expect("UTC-10 offset is valid")
                .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::days(2),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 7,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn probe_spec(class: &str) -> hares_io::EquipmentSpec {
    let parameters = json!({
        "active_power_kw": 1.0,
        "active_duration_s": 1800.0,
        "cooldown_duration_s": 900.0,
        "event_window_source": "constant",
        "event_probability_source": "constant",
        "event_probability_constant": 0.3,
        "sensible_gain_fraction": 0.0,
    });
    hares_io::EquipmentSpec {
        name: class.to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: parameters
            .as_object()
            .expect("probe parameters are an object")
            .clone(),
        zip_params: None,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

fn try_build_from(
    hpxml: &Path,
    specs: Vec<hares_io::EquipmentSpec>,
) -> Result<Dwelling, HaresError> {
    let mut blueprint = DwellingBlueprint::from_config(dwelling_config(hpxml)).expect("blueprint");
    for spec in specs {
        blueprint.add_equipment_spec(spec).expect("add probe spec");
    }
    blueprint.build()
}

fn try_build(specs: Vec<hares_io::EquipmentSpec>) -> Result<Dwelling, HaresError> {
    try_build_from(&fixture_dir().join("home.xml"), specs)
}

fn build(classes: &[&str]) -> Dwelling {
    try_build(classes.iter().map(|class| probe_spec(class)).collect()).expect("build dwelling")
}

fn named_probe(class: &str, instance_name: &str) -> hares_io::EquipmentSpec {
    hares_io::EquipmentSpec {
        instance_name: Some(instance_name.to_string()),
        ..probe_spec(class)
    }
}

/// An HPXML microwave carrying `id` as its SystemIdentifier and the probe's
/// event parameters in its extension.
fn hpxml_microwave(id: &str) -> String {
    format!(
        "<Microwave><SystemIdentifier id='{id}'/><extension>\
         <active_power_kw>1.0</active_power_kw>\
         <active_duration_s>1800</active_duration_s>\
         <cooldown_duration_s>900</cooldown_duration_s>\
         <event_window_source>constant</event_window_source>\
         <event_probability_source>constant</event_probability_source>\
         <event_probability_constant>0.3</event_probability_constant>\
         <sensible_gain_fraction>0.0</sensible_gain_fraction>\
         <latent_gain_fraction>0.0</latent_gain_fraction>\
         </extension></Microwave>"
    )
}

/// The fixture HPXML with `microwave_ids` added to its appliances.
fn write_hpxml_with_microwaves(dir: &Path, microwave_ids: &[&str]) -> PathBuf {
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let marker = "</Appliances>";
    assert!(src.contains(marker), "fixture must contain {marker}");
    let microwaves: String = microwave_ids.iter().map(|id| hpxml_microwave(id)).collect();
    let path = dir.join(format!("home_{}.xml", microwave_ids.join("_")));
    std::fs::write(
        &path,
        src.replacen(marker, &format!("{microwaves}{marker}"), 1),
    )
    .expect("write HPXML");
    path
}

fn is_on(dwelling: &Dwelling, name: &str) -> bool {
    let eq = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == name)
        .unwrap_or_else(|| panic!("probe '{name}' missing from the dwelling"));
    eq.telemetry()
        .get(tk::ACTIVE_POWER_KW)
        .expect("event load publishes active power")
        > 0.0
}

/// Steps `steps` times and returns each named probe's on/off pattern.
fn record(dwelling: &mut Dwelling, names: &[&str], steps: usize) -> Vec<Vec<bool>> {
    let mut patterns = vec![Vec::with_capacity(steps); names.len()];
    for _ in 0..steps {
        dwelling.step().expect("step");
        for (pattern, name) in patterns.iter_mut().zip(names) {
            pattern.push(is_on(dwelling, name));
        }
    }
    patterns
}

#[test]
fn event_loads_draw_from_independent_streams() {
    let both = record(&mut build(&[PROBE_A, PROBE_B]), &[PROBE_A, PROBE_B], STEPS);
    let [a, b] = [&both[0], &both[1]];
    assert!(
        a.contains(&true) && a.contains(&false),
        "probe A must both start and idle within {STEPS} steps"
    );
    assert_ne!(
        a, b,
        "two identically configured event loads must not share a draw sequence"
    );

    let reordered = record(&mut build(&[PROBE_B, PROBE_A]), &[PROBE_A, PROBE_B], STEPS);
    assert_eq!(
        reordered, both,
        "reordering the loads must not change either load's draws"
    );

    let a_alone = record(&mut build(&[PROBE_A]), &[PROBE_A], STEPS);
    assert_eq!(&a_alone[0], a, "removing B must not change A's draws");
    let b_alone = record(&mut build(&[PROBE_B]), &[PROBE_B], STEPS);
    assert_eq!(&b_alone[0], b, "removing A must not change B's draws");
}

#[test]
fn event_load_streams_survive_checkpoint() {
    let names = [PROBE_A, PROBE_B];
    let mut continuous = build(&names);
    record(&mut continuous, &names, STEPS / 2);
    let checkpoint = continuous.save_checkpoint().expect("save checkpoint");
    let expected = record(&mut continuous, &names, STEPS);

    let mut resumed = build(&names);
    resumed
        .load_checkpoint(checkpoint)
        .expect("load checkpoint");
    let actual = record(&mut resumed, &names, STEPS);

    assert!(
        expected.iter().all(|p| p.contains(&true)),
        "both probes must start after the checkpoint"
    );
    assert_eq!(
        actual, expected,
        "a resumed dwelling must continue each event load's stream exactly"
    );
}

/// Instance naming numbers same-class loads by order ("Microwave #1",
/// "#2"), so removing the first renames the second to "Microwave". Its
/// stream is keyed by its HPXML SystemIdentifier id, so its draws stay.
#[test]
fn same_class_hpxml_load_keeps_its_draws_when_a_sibling_is_removed() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let both = write_hpxml_with_microwaves(tmp.path(), &["Microwave-Kitchen", "Microwave-Bar"]);
    let alone = write_hpxml_with_microwaves(tmp.path(), &["Microwave-Bar"]);

    let mut both = try_build_from(&both, Vec::new()).expect("build both microwaves");
    let both = record(&mut both, &["Microwave #1", "Microwave #2"], STEPS);
    assert_ne!(both[0], both[1], "same-class siblings must not share draws");

    let mut alone = try_build_from(&alone, Vec::new()).expect("build the second alone");
    let alone = record(&mut alone, &["Microwave"], STEPS);
    assert_eq!(
        alone[0], both[1],
        "removing a same-class sibling must not change the survivor's draws"
    );
}

/// The blueprint counterpart: callers name same-class loads, instance
/// naming renumbers them by order, and the caller's name keys the stream.
#[test]
fn same_class_named_load_keeps_its_draws_when_a_sibling_is_removed() {
    let first = named_probe(PROBE_A, "Probe One");
    let second = named_probe(PROBE_A, "Probe Two");

    let mut both = try_build(vec![first, second.clone()]).expect("build both probes");
    let both = record(
        &mut both,
        &[&format!("{PROBE_A} #1"), &format!("{PROBE_A} #2")],
        STEPS,
    );
    assert_ne!(both[0], both[1], "same-class siblings must not share draws");

    let mut alone = try_build(vec![second]).expect("build the second probe alone");
    let alone = record(&mut alone, &["Probe Two"], STEPS);
    assert_eq!(
        alone[0], both[1],
        "removing a same-class sibling must not change the survivor's draws"
    );
}

/// An HPXML SystemIdentifier id and a caller's instance name can name the
/// same identity; the two loads would then share a stream, so assembly
/// rejects them instead of telling them apart by order.
#[test]
fn event_loads_sharing_an_identity_are_rejected() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_hpxml_with_microwaves(tmp.path(), &["Shared"]);
    let err = try_build_from(&hpxml, vec![named_probe(PROBE_A, "Shared")])
        .err()
        .expect("event loads sharing an identity must not build");
    let msg = err.to_string();
    assert!(
        msg.contains("'Microwave'")
            && msg.contains("'Shared'")
            && msg.contains("same random stream"),
        "the rejection must name both loads and the violation, got: {msg}"
    );
}
