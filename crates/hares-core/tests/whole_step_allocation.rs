#![cfg(not(feature = "observe"))]
//! The whole-step allocation assertion (the contributor platform's E2
//! spec): a fully loaded dwelling steps between two allocation-counter
//! reads and the window's delta is pinned at its exact named bound,
//! with the observe feature off (the observe build's per-step port
//! snapshots and thermal-update clones are their own recorded class,
//! not this assertion's subject).
//!
//! The counter is the workspace's shared counting allocator (the
//! per-thread call counter, deallocation does not decrement), installed
//! by this test binary alone; the library never installs one. The delta
//! between two reads is therefore the exact number of allocations the
//! thread made across the window's steps, and the assertion is exact:
//! not a threshold, a named number.
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
//! the count they removed. The lazy capacity fills the steady state
//! still paid were moved to construction in the same series: the
//! dispatcher's priority queues and the actor dispatch buffer (one
//! step's queued-request bound), the EV composer's action label (the
//! composed label's bound), the rainflow counter's reversal residual
//! and daily DOD list (the measured working sets), and the HVAC speed
//! selections' capacity-fraction cache (config-frozen data, computed at
//! the first selection). What the windows still count are the named
//! event classes below, each a real state change or a real event
//! record, never a per-step cost.
//!
//! The three windows partition the row's five census sites across the
//! fixture set (the coverage claim, checked by each window's own
//! precondition reads):
//!
//! - the January window (the cz6b parity dwelling: heat pump both
//!   sides, battery, EV, PV, ventilation, water heater, the driver
//!   actor) pins zero: the fluid solver's scratch and the humidity
//!   alpha telemetry are per-step sites every window exercises, and
//!   this window's day carries the EV arrival, the driver's schedule
//!   events and the battery, all now zero;
//! - the June SelfConsumption window (the same dwelling, the battery's
//!   BMS in `SelfConsumption`, three warm-in days then the fourth
//!   day asserted, the SHR registration and the first cooling step
//!   landed in the warm-in) pins the BMS's decision transitions and
//!   the cooling run's first-step clamp record: the cooling equipment's
//!   SHR write runs in the window's steady state (twenty cooling
//!   steps, zero each);
//! - the June gas-water-heater window (the cz5a minisplit with its gas
//!   water heater, three warm-in days then the fourth day asserted)
//!   pins the gas water heater's steady state (the burner and pilot
//!   fire every window step, zero each) and the same cooling-start
//!   clamp record (the minisplit cools through the window).
//!
//! The census (the profiling instrument at this commit, three runs per
//! fixture, identical per run): consumer_shape_900s carries 315
//! allocations over its 35,040-step year and 3 violating steps — step 0
//! the maps' and buffers' first inserts (263), step 1 the snapshot
//! maps' capacity settle (1), step 12,563 the first cooling step's SHR
//! registration and the snapshot's copy of the new key (51) — and the
//! three sum. der_pv_battery_ev_900s carries 441 over its 960 steps and
//! 53 violating steps, the BMS's decision transitions and the EV
//! driver's day events, one to two each. A deliberately introduced
//! per-step allocation adds 96 to the January window and fails it (the
//! mutation check).
//!
//! The assertion runs in the default PR profile: each window is two to
//! four days of its parity dwelling at 900 s, and the whole file is a
//! few seconds.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings, StepResult};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// A window's dwelling: the parity fixture's directory at 900 s steps,
/// with the window's start time (the fixture's own when `None`) and the
/// settings' battery override (the fixture's own when `None`). No
/// reports: the tests read in-memory state, nothing is recorded.
fn window_dwelling(
    fixture: &str,
    start_time: Option<&str>,
    overrides: Option<serde_json::Value>,
    steps: usize,
) -> Dwelling {
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
    let overrides = overrides.or_else(|| {
        config_value
            .get("overrides")
            .map(|value| serde_json::to_value(value).expect("the fixture overrides must serialize"))
    });

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

/// The cz6b parity dwelling as the first window loads it: the fixture's
/// own January start and its `Manual` battery declaration, two days.
fn january_dwelling() -> Dwelling {
    window_dwelling("cz6b_pv_battery_ev", None, None, 192)
}

/// The June `SelfConsumption` window's dwelling: the same fixture with
/// the battery's BMS in `SelfConsumption`, four days (three warm-in,
/// the fourth asserted).
fn june_selfconsumption_dwelling() -> Dwelling {
    let overrides = serde_json::json!({
        "Battery": {"bms_mode": "{\"SelfConsumption\": {\"min_soc\": 0.1, \"max_soc\": 0.95, \"solar_only_charging\": false}}"}
    });
    window_dwelling(
        "cz6b_pv_battery_ev",
        Some("2023-06-01T00:00:00-07:00"),
        Some(overrides),
        384,
    )
}

/// The June gas-water-heater window's dwelling: the cz5a minisplit
/// fixture (the gas water heater, the draft-inducer fan), four days
/// (three warm-in, the fourth asserted).
fn june_gas_water_heater_dwelling() -> Dwelling {
    window_dwelling(
        "cz5a_minisplit_gas_wh",
        Some("2023-06-01T00:00:00-07:00"),
        None,
        384,
    )
}

/// Steps `count` steps, asserting each succeeds.
fn step_n(dwelling: &mut Dwelling, step_result: &mut StepResult, count: usize, label: &str) {
    for step in 0..count {
        dwelling
            .step_into(step_result)
            .unwrap_or_else(|err| panic!("{label} step {step} must succeed: {err:?}"));
    }
}

/// The whole-step allocation assertion, the January window: after the
/// one-day warm-in, 96 steps between two allocation-counter reads
/// allocate exactly zero.
///
/// The window's day carries the row's named events in their removed or
/// pre-sized forms: the EV driver's 18:00 arrival (step 168 — the
/// dispatch buffers, the composer's label and the rainflow buffers are
/// construction-sized), the battery's held decision, the schedule
/// events. Every step is exactly zero, so the bound is zero; a new
/// per-step site (96) or a new event class moves this number and fails
/// the assertion.
#[test]
fn the_whole_step_allocates_nothing_between_the_two_counter_reads() {
    let mut dwelling = january_dwelling();
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
        "the whole step allocated {} times over the 96 steps after the \
         warm-in: the step loop's steady state must allocate nothing; the \
         named events in this window's day (the EV driver's 18:00 arrival, \
         the battery's held decision) are construction-sized or removed",
        after - before,
    );
}

/// The single-step form of the January assertion: one step between the
/// two reads, allocating exactly zero. The 96-step window above could
/// in principle hide a negative site against a positive one; the
/// single-step window cannot.
#[test]
fn one_whole_step_between_the_reads_allocates_exactly_the_named_bound() {
    let mut dwelling = january_dwelling();
    let mut step_result = StepResult::default();
    step_n(&mut dwelling, &mut step_result, 96, "warm-in");
    let before = thread_allocations().expect("the counting allocator is installed");
    step_n(&mut dwelling, &mut step_result, 1, "asserted single");
    let after = thread_allocations().expect("the counting allocator is installed");
    assert_eq!(
        after - before,
        0,
        "one whole step allocated {} times: the step loop's steady state \
         must allocate nothing",
        after - before,
    );
}

/// The June `SelfConsumption` window: after the three-day warm-in (the
/// first cooling step's SHR registration lands there), the fourth
/// day's 96 steps allocate exactly five, and each one is named.
///
/// The five: the BMS's `SelfConsumption` decision transitions (steps
/// 320, 321 — two, the decide's and the PV re-evaluation's — and 333;
/// each is `assign_action`'s content-change write, a real decision
/// change) and the cooling run's first step (step 356), which carries
/// the curve-index clamp's health record (the clamp source's name into
/// the step's drain). The window's twenty cooling steps — the SHR
/// telemetry write's steady state — and the EV driver's 18:00 arrival
/// (step 360) allocate exactly zero each.
#[test]
fn the_june_selfconsumption_steps_pin_their_named_event_writes() {
    let mut dwelling = june_selfconsumption_dwelling();
    let mut step_result = StepResult::default();
    step_n(&mut dwelling, &mut step_result, 288, "warm-in");
    let before = thread_allocations().expect("the counting allocator is installed");
    step_n(&mut dwelling, &mut step_result, 96, "asserted");
    let after = thread_allocations().expect("the counting allocator is installed");
    assert_eq!(
        after - before,
        5,
        "the June SelfConsumption window allocated {} times over the 96 \
         steps after the three-day warm-in: the named events are the BMS's \
         decision transitions (three decide/re-evaluation steps, four \
         content-change writes) and the cooling run's first-step clamp \
         record; anything else moves this bound and must be named",
        after - before,
    );
}

/// The June gas-water-heater window: after the three-day warm-in, the
/// fourth day's 96 steps allocate exactly two, the cooling run's first
/// step's clamp record (step 347: the step's clamp-source drain — the
/// source list's first fill and the source's name). The gas water
/// heater fires every window step (the pilot's standby draw, the
/// morning burner) and allocates exactly zero each: the heat-injection
/// scratch is the equipment's persistent buffer. The minisplit's
/// cooling steps — the SHR write's steady state, twenty-four of them —
/// allocate exactly zero each.
#[test]
fn the_june_gas_water_heater_steps_pin_their_named_event_writes() {
    let mut dwelling = june_gas_water_heater_dwelling();
    let mut step_result = StepResult::default();
    step_n(&mut dwelling, &mut step_result, 288, "warm-in");
    let before = thread_allocations().expect("the counting allocator is installed");
    step_n(&mut dwelling, &mut step_result, 96, "asserted");
    let after = thread_allocations().expect("the counting allocator is installed");
    assert_eq!(
        after - before,
        2,
        "the June gas-water-heater window allocated {} times over the 96 \
         steps after the three-day warm-in: the named event is the cooling \
         run's first-step clamp record; anything else moves this bound \
         and must be named",
        after - before,
    );
}
