//! Each stochastic event load in a dwelling draws from its own random
//! stream, keyed by the dwelling seed and the load's name.
//!
//! The probes are two raw `EventBasedLoad`-class specs with identical
//! parameters (a constant start probability strictly inside (0, 1), so every
//! idle step draws) and distinct classes, so the assembly's instance-name
//! pass leaves their names alone. Each load's per-step on/off pattern is a
//! direct read of its draw sequence; identical configs make any difference
//! between the two patterns attributable to the streams alone.

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::{FuelType, telemetry_keys as tk};
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
        schedule_path: fixture_dir().join("in.schedules.csv"),
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

fn build(probes: &[&str]) -> Dwelling {
    let hpxml = fixture_dir().join("home.xml");
    let mut blueprint = DwellingBlueprint::from_config(dwelling_config(&hpxml)).expect("blueprint");
    for class in probes {
        blueprint
            .add_equipment_spec(probe_spec(class))
            .expect("add probe spec");
    }
    blueprint.build().expect("build dwelling")
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
