#![cfg(feature = "observe")]
//! The observe build's never-observing step (the allocation census's
//! class A): the observe feature alone added four allocations per step
//! on the census's consumer fixture (the `scheduled_phases` string
//! list's Vec growth and label String, and the thermal-update buffer's
//! observer capture clone), paid by every observe build whether or not
//! the ring was ever enabled, about 140,160 dead allocations over a
//! 35,040-step year. Both sites are gated on the observer's actual
//! enablement now: the phase labels feed only the observing path's
//! debug line (compiled out in release) and the thermal clone's sole
//! consumer sits behind `observer_buf.is_some()`.
//!
//! The window is the whole-step assertion's January window (the cz6b
//! parity dwelling: heat pump both sides, battery, EV, PV, ventilation,
//! water heater, the driver actor): the same fixture's 96 asserted steps
//! pin at zero in the default build, and this file pins the observe
//! build's never-observing step at the same zero. The counter is the
//! workspace's shared counting allocator (the per-thread call counter,
//! deallocation does not decrement), installed by this test binary
//! alone; the driver is the into-form the run's own step loop steps
//! through.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings, StepResult};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// A window's dwelling: the parity fixture's directory at 900 s steps,
/// with the window's start time (the fixture's own when `None`), the
/// fixture's own overrides when none are given, no reports, no metrics.
fn window_dwelling(fixture: &str, start_time: Option<&str>, steps: usize) -> Dwelling {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/parity");
    root.push(fixture);

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
    let mut sim_config =
        SimulationConfig::from_toml(&sim_toml).expect("simulation config must parse");
    let bldg_id = config_value
        .get("bldg_id")
        .and_then(|value| value.as_integer())
        .unwrap_or(1);

    if let Some(start) = start_time {
        sim_config.start_time =
            chrono::DateTime::parse_from_rfc3339(start).expect("window start time parses");
    }
    sim_config.time_res = chrono::Duration::seconds(900);
    sim_config.duration = chrono::Duration::seconds(900 * steps as i64);
    sim_config.reports = Vec::new();
    let overrides = config_value
        .get("overrides")
        .map(|value| serde_json::to_value(value).expect("the fixture overrides must serialize"));

    let config = DwellingConfig {
        hpxml: InputSource::Path(root.join("building.xml")),
        schedule: Some(InputSource::Path(root.join("schedule.csv"))),
        weather: WeatherInput::Path(root.join("weather.epw")),
        defaults: DefaultsSource::directory(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults"),
        ),
        settings: DwellingSettings::new(sim_config, overrides, bldg_id, None),
    };

    Dwelling::from_config(config).expect("fixture dwelling must load")
}

/// Steps `count` steps, asserting each succeeds.
fn step_n(dwelling: &mut Dwelling, step_result: &mut StepResult, count: usize, label: &str) {
    for step in 0..count {
        dwelling
            .step_into(step_result)
            .unwrap_or_else(|err| panic!("{label} step {step} must succeed: {err:?}"));
    }
}

/// The observe build's never-observing step: after the one-day warm-in,
/// 96 steps between two allocation-counter reads allocate exactly zero.
/// RED at the parent: the observe feature's own four (here eight: the
/// driver actor's phase label joins the list) per step. GREEN at the
/// commit: the default build's zero.
#[test]
fn the_never_observing_step_pays_nothing() {
    let mut dwelling = window_dwelling("cz6b_pv_battery_ev", None, 192);
    let mut step_result = StepResult::default();

    // The warm-in: the window's preceding 96 steps pay the run's
    // one-time fills (the maps' and buffers' first inserts, the lazily
    // registered telemetry channels at their first event, the scheduler
    // plan's first build). The asserted window starts after them.
    step_n(&mut dwelling, &mut step_result, 96, "warm-in");
    let before = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    step_n(&mut dwelling, &mut step_result, 96, "asserted");
    let after = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    assert_eq!(
        after - before,
        0,
        "the observe build's never-observing step allocated {} times over \
         the 96 steps after the warm-in: the observer is never enabled, so \
         the step must pay nothing for the observe feature",
        after - before,
    );
}

/// The single-step form: one step between the two reads, allocating
/// exactly zero. The 96-step window above could in principle hide a
/// negative site against a positive one; the single-step window cannot.
#[test]
fn one_never_observing_step_between_the_reads_allocates_exactly_zero() {
    let mut dwelling = window_dwelling("cz6b_pv_battery_ev", None, 192);
    let mut step_result = StepResult::default();
    step_n(&mut dwelling, &mut step_result, 96, "warm-in");
    let before = thread_allocations().expect("the counting allocator is installed");
    step_n(&mut dwelling, &mut step_result, 1, "asserted single");
    let after = thread_allocations().expect("the counting allocator is installed");
    assert_eq!(
        after - before,
        0,
        "one never-observing step allocated {} times: the step must pay \
         nothing for the observe feature",
        after - before,
    );
}
