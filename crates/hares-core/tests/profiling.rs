//! Profiling-summary integration tests (`profiling` feature).
//!
//! Gated out entirely without the feature: the summary fields and the phase
//! clock they assert on only exist under `-F profiling`.
//!
//! Every fixture-driven test here steps "the cz2a fixture": the
//! `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/` inputs (`building.xml`,
//! `schedule.csv`, `weather.epw`) at their config start
//! `2023-01-01T00:00:00-07:00`, no warm-up, `write_output: false`.
#![cfg(feature = "profiling")]

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::Duration;
use hares_core::{Dwelling, DwellingConfig};
use hares_io::SimulationConfig;

/// Loads the cz2a fixture at hourly resolution for 96 steps.
fn load_cz2a_fixture() -> Dwelling {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/parity");
    root.push("cz2a_gas_furnace_ac_res_wh");

    let config_path = root.join("config.toml");
    let config_contents =
        std::fs::read_to_string(&config_path).expect("fixture config.toml must be readable");
    let config_value: toml::Value =
        toml::from_str(&config_contents).expect("fixture config.toml must parse");
    let sim_table = config_value
        .get("simulation")
        .and_then(|value| value.as_table())
        .expect("fixture config.toml must contain [simulation]");
    let sim_toml = toml::to_string(sim_table).expect("simulation table must serialize");
    let sim_config = SimulationConfig::from_toml(&sim_toml).expect("simulation config must parse");
    let bldg_id = config_value
        .get("bldg_id")
        .and_then(|value| value.as_integer())
        .unwrap_or(1);

    // 96 hourly steps; no warm-up; the tests read in-memory state, so the
    // recorder is off.
    let mut sim_config = sim_config;
    sim_config.time_res = Duration::hours(1);
    sim_config.duration = Duration::hours(96);
    sim_config.write_output = false;

    let config = DwellingConfig {
        hpxml_path: root.join("building.xml"),
        schedule_path: root.join("schedule.csv"),
        weather_path: root.join("weather.epw"),
        defaults_path: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")),
        sim_config,
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    Dwelling::from_config(config).expect("fixture dwelling must load")
}

/// The phases are differences of the same `Instant` readings that bound the
/// step, so their sum must equal `step_total` exactly (`Duration` equality,
/// no tolerance, no timing dependence).
#[test]
fn profiling_phases_partition_the_step() {
    let mut dwelling = load_cz2a_fixture();
    for _ in 0..96 {
        dwelling.step().expect("cz2a fixture step must succeed");
    }

    let summary = dwelling.profiling_summary();
    let phases = [
        summary.environment,
        summary.control,
        summary.ideal_capacity,
        summary.actors,
        summary.dispatch,
        summary.equipment,
        summary.envelope,
        summary.invariants,
        summary.state_snapshot,
        summary.output,
        summary.accounting,
    ];
    let phase_sum: StdDuration = phases.into_iter().sum();
    assert_eq!(
        phase_sum, summary.step_total,
        "the eleven phase fields must partition step_total exactly after 96 steps"
    );
}
