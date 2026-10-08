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
use hares_control::DispatchRequest;
use hares_core::{Actor, Dwelling, DwellingConfig};
use hares_io::SimulationConfig;
use hares_types::EnvironmentState;

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
        schedule_path: Some(root.join("schedule.csv")),
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

/// A probe actor that declares no interests (the filter calls `decide()`
/// every step) and dispatches nothing.
struct ProbeActor {
    name: &'static str,
}

impl Actor for ProbeActor {
    fn name(&self) -> &str {
        self.name
    }

    fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {}
}

/// Per-actor totals accumulate over the run, not per step, and sum exactly
/// to the `actors` phase: each entry's total is the same phase-clock
/// `Instant` difference the phase gets (`Duration` equality, no tolerance,
/// no timing dependence). The assertions name the two probe entries rather
/// than count the list, because `auto_register_actors` may register
/// built-in actors first.
#[test]
fn actor_timings_accumulate_over_the_run() {
    let mut dwelling = load_cz2a_fixture();
    dwelling
        .add_actor(Box::new(ProbeActor { name: "probe_a" }))
        .expect("probe_a must register");
    dwelling
        .add_actor(Box::new(ProbeActor { name: "probe_b" }))
        .expect("probe_b must register");
    for _ in 0..96 {
        dwelling.step().expect("cz2a fixture step must succeed");
    }

    let summary = dwelling.profiling_summary();
    let position = |name: &str| {
        summary
            .per_actor
            .iter()
            .position(|timing| timing.name.as_ref() == name)
            .unwrap_or_else(|| panic!("per_actor must hold a '{name}' entry"))
    };
    let probe_a = position("probe_a");
    let probe_b = position("probe_b");
    assert!(
        probe_a < probe_b,
        "probe_a must be listed before probe_b (registration order): {:?}",
        summary.per_actor
    );

    let timing = |name: &str| {
        let entry = &summary.per_actor[position(name)];
        assert_eq!(
            entry.calls, 96,
            "'{name}' declares no interests, so the filter must call decide() every step"
        );
        entry.total
    };
    let probe_a_total = timing("probe_a");
    let probe_b_total = timing("probe_b");
    assert!(
        probe_a_total > StdDuration::ZERO || probe_b_total > StdDuration::ZERO,
        "the probes' decide() must take measurable time across 96 steps"
    );

    let per_actor_sum: StdDuration = summary.per_actor.iter().map(|t| t.total).sum();
    assert_eq!(
        per_actor_sum, summary.actors,
        "the per-actor totals must sum to the actors phase exactly: both are \
         differences of the same phase-clock Instant readings"
    );
}

/// Reads `VmHWM` (KiB) straight from the test process's `/proc/self/status`.
#[cfg(target_os = "linux")]
fn vm_hwm_kb_now() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        })
}

/// The high-water mark is read once per summary, not per step: after 96 steps
/// it must be a real measurement (`Some`, `> 0`) no greater than the
/// process's `VmHWM` read immediately afterwards (the peak can only grow).
#[test]
#[cfg(target_os = "linux")]
fn profiling_summary_reports_process_high_water_mark() {
    let mut dwelling = load_cz2a_fixture();
    for _ in 0..96 {
        dwelling.step().expect("cz2a fixture step must succeed");
    }

    let summary = dwelling.profiling_summary();
    let kb = summary
        .memory_high_water_kb
        .expect("the summary must carry a high-water mark on Linux");
    assert!(kb > 0, "the high-water mark must be a measurement, never 0");

    let later = vm_hwm_kb_now().expect("a VmHWM read must succeed in the test process");
    assert!(
        kb <= later,
        "the summary's high-water mark {kb} KiB must not exceed the later VmHWM read {later} KiB"
    );
}
