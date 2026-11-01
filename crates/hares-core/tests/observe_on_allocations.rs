#![cfg(feature = "observe")]
//! The observing step's allocation census, the ring-enabled build's
//! second row: after the recycled-slot machinery filled the capture
//! buffers once, the observing step's remaining per-step allocations
//! are the capture sites' owned Vecs and the conflict scan's own
//! transients. This file pins the observing step at an exact named
//! bound on the same January window the default build's whole-step
//! assertion owns (the cz6b parity dwelling: heat pump both sides,
//! battery, EV, PV, ventilation, water heater, the driver actor; the
//! observer ring at 4).
//!
//! The counter is the workspace's shared counting allocator (the
//! per-thread call counter, deallocation does not decrement), installed
//! by this test binary alone; the driver is the into-form the run's own
//! step loop steps through. The window's bound is exact, not a
//! threshold: a new per-step allocation site moves it and fails.
//!
//! The per-step census this bound pins (the observing path's steady
//! state): the per-equipment pre-step port captures' three owned Vecs
//! each step per equipment, the two phase captures' port Vecs, the
//! environment capture's three Vecs, the solver capture's four
//! domain-update payloads and three envelope-gain Vecs, the zone-update
//! capture's two Vecs, and the dispatch conflict scan's transients (the
//! per-tier requests collect, the recorded-from ledger, the conflict's
//! inner signals Vec and the throwaway's first push). The recycled slot
//! carries the capture buffers; the captures must write through them.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings, StepResult};
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// A window's dwelling: the parity fixture's directory at 900 s steps,
/// with the window's start time (the fixture's own when `None`), the
/// fixture's own overrides, no reports, no metrics.
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

/// The observing step's steady state: after the warm-in filled the
/// ring's slots and the capture buffers, 96 observed steps between two
/// allocation-counter reads allocate exactly the named bound.
///
/// The bound's derivation. The row removed the observing capture sites'
/// per-step allocations: the per-equipment pre-step port captures'
/// owned Vecs (three per equipment per step on this 16-equipment
/// fixture), the two equipment-phase captures' port Vecs, the
/// environment capture's three Vecs, the solver capture's four
/// domain-update payloads and the envelope gains (with the pass-through
/// thermal-update clone the capture no longer pays), the zone-update
/// capture's two Vecs, and the dispatch conflict scan's transients (the
/// per-tier requests collect, the recorded-from ledger, the conflict's
/// inner signals Vec and the throwaway's first push). The parent's
/// window measured 12,937; the tip measures 5,736, the row's removal
/// 7,201 over the window (75 per step: the census's consumer-fixture
/// estimate for the same sites was 71, this fixture's roster carries
/// the rest). Every removed site is proven zero by its own bracket
/// (the per-site pins in the dwelling's test module); what remains is
/// the census's own-row remainder this row does not name: the identity
/// Strings (3 per equipment per step), the diff's Vecs and the
/// observation Vecs' growths. A new per-step allocation moves this
/// number.
#[test]
fn the_observing_step_allocates_exactly_the_named_bound() {
    let mut dwelling = window_dwelling("cz6b_pv_battery_ev", None, 192);
    let mut step_result = StepResult::default();
    dwelling.enable_observer(4);

    // The warm-in: the ring's first fills (the capture buffers' first
    // allocations, the actor keys' cache) land here. The asserted
    // window starts after them.
    step_n(&mut dwelling, &mut step_result, 96, "warm-in");
    let before = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    step_n(&mut dwelling, &mut step_result, 96, "asserted");
    let after = thread_allocations()
        .expect("this test binary installs the counting allocator; the read is Some");

    assert_eq!(
        after - before,
        5_736,
        "the observing step allocated {} times over the 96 steps after \
         the warm-in: the row's capture sites must contribute zero (the \
         per-site pins prove each) and the remainder is the census's \
         identity-and-diff row's own sites; a new per-step allocation \
         moves this bound",
        after - before,
    );
}
