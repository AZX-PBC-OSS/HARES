//! Hot-path allocation counting through the profiling summary (`profiling`
//! feature).
//!
//! This test binary installs the workspace's shared counting allocator: the
//! summary's hot-path fields must report real counts for the cz2a fixture's
//! steps (which allocate today) without panicking the run.

#![cfg(feature = "profiling")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::Duration;
use hares_core::{ActorTimings, Dwelling, DwellingConfig};
use hares_io::SimulationConfig;
use hares_types::alloc_count::{CountingAllocator, thread_allocations};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

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

/// With the allocator installed, 96 debug-build steps of the cz2a fixture
/// complete, and both counters are `Some`: every step's delta is summed into
/// `hot_path_allocations`, and every step that allocated counts one
/// violation, so `hot_path_allocations` is at least
/// `hot_path_alloc_violations`. Steps allocating is expected today; the
/// count must never panic the run.
#[test]
fn profiled_steps_count_allocations_without_panicking() {
    let mut dwelling = load_cz2a_fixture();
    for _ in 0..96 {
        dwelling.step().expect("cz2a fixture step must succeed");
    }

    let summary = dwelling.profiling_summary();
    let allocations = summary
        .hot_path_allocations
        .expect("the test binary installs the counting allocator");
    let violations = summary
        .hot_path_alloc_violations
        .expect("the test binary installs the counting allocator");
    assert!(
        allocations >= violations,
        "every violating step allocates at least once: allocations {allocations} \
         must be >= violations {violations}"
    );
}

/// Accumulating per-actor timings allocates nothing: 96 x 2 `record` calls
/// (one per slot per step) leave the stepping thread's allocation count
/// unchanged. Only construction allocates (one slot per actor, off the hot
/// path), so the baseline is read after it. Whole-step `Some(0)` is not
/// asserted here: steps still allocate at this row (the telemetry map), and
/// the runtime-memory ledger rows own that assertion.
#[test]
fn actor_timings_record_without_allocating() {
    let mut timings = ActorTimings::new(["probe_a", "probe_b"].map(Arc::<str>::from));
    let before = thread_allocations();

    for _ in 0..96 {
        timings.record(0, StdDuration::from_nanos(1), true);
        timings.record(1, StdDuration::from_nanos(2), true);
    }

    let delta = thread_allocations().and_then(|after| before.map(|baseline| after - baseline));
    assert_eq!(
        delta,
        Some(0),
        "record must add to the slots in place without allocating (the test \
         binary installs the counting allocator, so the delta is a measurement)"
    );
}
