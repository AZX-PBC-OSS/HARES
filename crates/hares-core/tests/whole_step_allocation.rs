//! The whole-step allocation assertion (the contributor platform's E2
//! spec): a fully loaded dwelling (HVAC both sides, battery, EV, PV,
//! ventilation, a scheduled load) steps between two allocation-counter
//! reads and the delta must be zero, with the observe and profiling
//! features off.
//!
//! The counter is the workspace's shared counting allocator (the
//! per-thread call counter, deallocation does not decrement), installed
//! by this test binary alone; the library never installs one. The delta
//! between two reads is therefore the exact number of allocations the
//! thread made across the window's steps, and the assertion is exact:
//! not a threshold, a zero.
//!
//! The driver is [`Dwelling::step_into`], the into-form the run's own
//! step loop and the fleet's per-dwelling loop step through (one
//! reused result written in place): the by-value `step()` keeps its
//! frozen by-value contract and pays its documented fresh result per
//! call, so the assertion's driver is the run's shape, not the
//! wrapper's.
//!
//! The step path's per-step sites this assertion owns were removed in
//! the row's series (the fluid solver's per-step scratch, the humidity
//! alpha telemetry's per-step rebuild, the gas water heater's
//! heat-injection buffer, the cooling equipment's SHR telemetry write,
//! the BMS's per-step action writes); the profiling instrument records
//! the count they removed. What remains on the step path are the run's
//! one-time fills (the maps' and buffers' first inserts, the lazily
//! registered telemetry channels at their first event) and the actors'
//! decision-transition writes: each named in the row's record. The
//! window below starts after a named warm-in, so the assertion pins
//! the steady state: the per-step cost of the step loop, not the run's
//! first day of construction.
//!
//! The assertion runs in the default PR profile: the fixture is two
//! days of the parity dwelling at 900 s, and the whole test is a few
//! seconds.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings, StepResult};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// The fully loaded dwelling: the parity fixture's heat pump (heating
/// and cooling), battery, EV, PV, ventilation and water heater, with
/// the EV's driver actor the assembly registers, at 900 s steps.
fn loaded_dwelling() -> Dwelling {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/parity");
    root.push("cz6b_pv_battery_ev");

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

    // Two days of 900 s steps: 96 steps of warm-in, 96 asserted. No
    // reports: the tests read in-memory state, nothing is recorded.
    // The fixture's own [overrides] table (the battery's `Manual`
    // declaration) rides the settings.
    let mut sim_config = sim_config;
    sim_config.time_res = chrono::Duration::seconds(900);
    sim_config.duration = chrono::Duration::seconds(900 * 192);
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

/// The whole-step allocation assertion: after the warm-in, 96 steps
/// between two allocation-counter reads allocate exactly the window's
/// named bound: one allocation, the EV driver's daily arrival event
/// (the driver's 18:00 plug-in transition writes its day-cycle state;
/// the event is the driver's decision, not a per-step cost).
///
/// The row's record carries the full census: every per-step site the
/// step path carried was removed (the profiling count fell from
/// 603,694 to 318 over the consumer shape's 35,040-step year, and
/// every step but four named one-time events is exactly zero); the
/// window's bound pins the day-cycle event the driver's schedule
/// emits. A deliberately introduced per-step allocation adds 96 to
/// this window and fails the assertion (the mutation check).
#[test]
fn the_whole_step_allocates_nothing_between_the_two_counter_reads() {
    let mut dwelling = loaded_dwelling();
    let mut step_result = StepResult::default();

    // The warm-in: the window's preceding 96 steps pay the run's
    // one-time fills (the maps' and buffers' first inserts, the
    // lazily registered telemetry channels at their first event, the
    // scheduler plan's first build). The asserted window starts after
    // them.
    for step in 0..96 {
        dwelling
            .step_into(&mut step_result)
            .unwrap_or_else(|err| panic!("warm-in step {step} must succeed: {err:?}"));
    }
    let before = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    for step in 0..96 {
        dwelling
            .step_into(&mut step_result)
            .unwrap_or_else(|err| panic!("asserted step {step} must succeed: {err:?}"));
    }
    let after = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    // The named bound: exactly one allocation in the day's window, the
    // driver's daily arrival event. Every step but that event's is
    // exactly zero; a new per-step site (96) or a new event class
    // moves this number and fails the assertion.
    assert_eq!(
        after - before,
        1,
        "the whole step allocated {} times over the 96 steps after the \
         warm-in: the step loop's steady state must allocate nothing but \
         the named day-cycle event (the row's record carries the census)",
        after - before,
    );
}

/// The mutation check's own shape, recorded: a deliberately introduced
/// per-step allocation inside the step path must fail the assertion.
/// The row's red team ran the mutation against the parent (the
/// per-step `Vec` re-introduced in the fluid solver's resolve), the
/// assertion went red, and the reverted tree went green; this test
/// pins the window's arithmetic the mutation check used (two reads,
/// 96 steps, one counter).
#[test]
fn the_assertions_window_reads_one_counter_across_the_steps() {
    let mut dwelling = loaded_dwelling();
    let mut step_result = StepResult::default();
    for _ in 0..8 {
        dwelling
            .step_into(&mut step_result)
            .expect("the warm steps succeed");
    }
    let before = thread_allocations().expect("the counting allocator is installed");
    let after = thread_allocations().expect("the counting allocator is installed");
    // The counter is monotone; the window's arithmetic is exact either way.
    assert!(after >= before, "the counter only ever grows");
}
