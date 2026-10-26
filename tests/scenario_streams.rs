//! The scenario seed algebra's dwelling-level invariants: two scenarios'
//! streams are keystream-disjoint under one master seed, one scenario's
//! streams reproduce bitwise run to run and interleaved with other
//! scenarios' runs, a declared scenario 0 is not the absent scenario, and
//! the gym's per-episode seed composes with the scenario seed through
//! `reseed` (the reseed's effective key derivation is the same
//! scenario-mixed derivation construction uses, with the episode seed in
//! the master-seed position).
//!
//! Fixture: the synthetic single-zone TOML dwelling the rl_step bench
//! builds, at 900 s steps, warm-up disabled, extended with the two
//! stochastic classes the synthetic fixture alone omits: the `[event_load]`
//! table's Cooking Range (an event-load stream keyed on the load's
//! identity) and a built-in EV driver (the EV equipment's actor seed, the
//! driver keyed on the dwelling's reserved EV-driver stream). Every stream
//! construction derives: the dwelling's own stream 0, the event load's
//! tagged stream, the built-in driver's reserved stream. The scenario
//! reaches the build through the `[simulation]` table's `scenario_seed`
//! key.
//!
//! RED as the unthreaded scenario: before the scenario key exists, the
//! config boundary rejects the key (`SimulationConfig` carries
//! `deny_unknown_fields`), and every test here fails at that parse, not
//! at an assertion.

use std::path::Path;

use hares_core::Dwelling;

#[path = "../benches/common.rs"]
mod bench_common;

/// The synthetic dwelling's TOML with the master seed and the optional
/// scenario key set, and the `[event_load]` table's Cooking Range: the
/// event-load stream the per-component claims must cover. Warm-up
/// disabled: the streams a fresh build holds at construction are the
/// streams the reseed composition compares.
fn scenario_toml(scenario: Option<u64>, master_seed: u64) -> String {
    let scenario_line = scenario
        .map(|id| format!("scenario_seed = {id}\n"))
        .unwrap_or_default();
    format!(
        r#"building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res = 900
duration = 86400
output_chunk_size = 1000
master_seed = {master_seed}
warmup = "disabled"
{scenario_line}
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[infiltration]
ach = 3.0

[schedule]
occupancy = 1.0

[event_load]
active_power_w = 1500.0
event_probability = 0.5
"#
    )
}

/// The fixture's built-in EV driver: the EV equipment's actor seed builds
/// `EvDriver:EV1` at the add, keyed on the dwelling's scenario-mixed
/// stream range exactly as a fresh construction's driver is.
fn add_fixture_ev(dwelling: &mut Dwelling) {
    use hares_equipment::{EquipmentConfig, EquipmentRegistry};

    let config: hares_equipment::EvConfig = serde_json::from_value(serde_json::json!({
        "capacity_j": 216000000.0,
        "charging_level": "L2",
        "max_charging_power_w": 7200.0,
    }))
    .expect("the fixture's EvConfig parses");
    let config =
        EquipmentConfig::from_typed("EV1".to_string(), "EV".to_string(), config).expect("typed");
    let mut ev = EquipmentRegistry::new()
        .create("EV", config.clone())
        .expect("the fixture's EV creates");
    ev.init(&config, dwelling.latest_env())
        .expect("the fixture's EV inits");
    dwelling
        .add_equipment(ev)
        .expect("the fixture's EV joins with its built-in driver");
}

fn scenario_dwelling(dir: &Path, scenario: Option<u64>, master_seed: u64) -> Dwelling {
    let path = bench_common::numbered_path(dir, "hares-scenario-dwelling", "toml");
    std::fs::write(&path, scenario_toml(scenario, master_seed)).expect("the scenario TOML writes");
    let mut dwelling = Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the scenario dwelling builds");
    add_fixture_ev(&mut dwelling);
    dwelling
}

/// Two scenarios under one master seed: every stream the dwellings hold
/// after the same 24 steps is keyed by its own scenario's dwelling key
/// (the keys differ at the scenario bytes), rides the same nonce the
/// scenario-less partition assigns (the mix changes no nonce value), and
/// the two runs' whole states differ, byte for byte.
#[test]
fn scenario_streams_are_keystream_disjoint() {
    let dir = tempfile::tempdir().expect("test temp dir");
    let dir = dir.path();
    let mut scenario_a = scenario_dwelling(dir, Some(1), 42);
    let mut scenario_b = scenario_dwelling(dir, Some(2), 42);
    bench_common::step_n(&mut scenario_a, 24, "scenario a");
    bench_common::step_n(&mut scenario_b, 24, "scenario b");

    let streams_a = scenario_a.seeded_streams();
    let streams_b = scenario_b.seeded_streams();
    assert!(!streams_a.is_empty(), "the dwelling holds seeded streams");
    assert_eq!(streams_a.len(), streams_b.len(), "the same components");
    for (a, b) in streams_a.iter().zip(&streams_b) {
        assert_eq!(a.component, b.component, "component order");
        assert_ne!(
            a.state.seed, b.state.seed,
            "component {}: scenario a and scenario b must not share a dwelling key",
            a.component
        );
        assert_eq!(
            a.state.stream, b.state.stream,
            "component {}: the scenario mix changes no stream nonce",
            a.component
        );
    }

    let a_end = scenario_a
        .save_checkpoint()
        .expect("scenario a's state saves");
    let b_end = scenario_b
        .save_checkpoint()
        .expect("scenario b's state saves");
    assert_ne!(
        bench_common::canonical_checkpoint_bytes(&a_end),
        bench_common::canonical_checkpoint_bytes(&b_end),
        "two scenarios' runs must not hold the same state, byte for byte"
    );
}

/// One scenario's run reproduces itself bitwise, run to run and
/// interleaved with another scenario's run between the two constructions.
#[test]
fn same_scenario_reproduces_bitwise() {
    let dir = tempfile::tempdir().expect("test temp dir");
    let dir = dir.path();
    let mut first = scenario_dwelling(dir, Some(7), 42);
    bench_common::step_n(&mut first, 24, "the first scenario 7 run");
    // The interleaved run: another scenario's dwelling constructs and
    // steps between the two scenario 7 runs.
    let mut other = scenario_dwelling(dir, Some(8), 42);
    bench_common::step_n(&mut other, 24, "the interleaved scenario 8 run");
    let mut second = scenario_dwelling(dir, Some(7), 42);
    bench_common::step_n(&mut second, 24, "the second scenario 7 run");

    let first_end = first.save_checkpoint().expect("the first run saves");
    let second_end = second.save_checkpoint().expect("the second run saves");
    assert_eq!(
        bench_common::canonical_checkpoint_bytes(&first_end),
        bench_common::canonical_checkpoint_bytes(&second_end),
        "one scenario's run must reproduce itself, byte for byte"
    );
    let other_end = other.save_checkpoint().expect("the interleaved run saves");
    assert_ne!(
        bench_common::canonical_checkpoint_bytes(&first_end),
        bench_common::canonical_checkpoint_bytes(&other_end),
        "scenario 7's run must not equal scenario 8's"
    );
}

/// A declared scenario 0 is not the absent scenario: the two runs' keys
/// differ (the presence byte), so "scenario 0" and "no scenario" never
/// share a stream.
#[test]
fn scenario_zero_is_not_the_absent_scenario() {
    let dir = tempfile::tempdir().expect("test temp dir");
    let dir = dir.path();
    let mut declared = scenario_dwelling(dir, Some(0), 42);
    let mut absent = scenario_dwelling(dir, None, 42);
    bench_common::step_n(&mut declared, 24, "the declared scenario 0 run");
    bench_common::step_n(&mut absent, 24, "the absent-scenario run");

    let declared_end = declared.save_checkpoint().expect("the declared run saves");
    let absent_end = absent.save_checkpoint().expect("the absent run saves");
    assert_ne!(
        declared_end.rng_state, absent_end.rng_state,
        "scenario 0 and no scenario must not derive the same dwelling key"
    );
    assert_ne!(
        bench_common::canonical_checkpoint_bytes(&declared_end),
        bench_common::canonical_checkpoint_bytes(&absent_end),
        "scenario 0 and no scenario must not hold the same state"
    );
}

/// The gym composition: the per-episode reseed seed enters the
/// scenario-mixed derivation in the master-seed position. Two episodes
/// sharing an episode seed under different scenarios draw disjoint
/// streams (each reseeded dwelling holds the streams a fresh construction
/// under the episode seed and its own scenario holds, every component,
/// bitwise); one episode seed under one scenario reproduces bitwise.
#[test]
fn reseed_composes_the_episode_seed_with_the_scenario() {
    const EPISODE_SEED: u64 = 7;
    let dir = tempfile::tempdir().expect("test temp dir");
    let dir = dir.path();
    let mut episode_a = scenario_dwelling(dir, Some(1), 42);
    let mut episode_b = scenario_dwelling(dir, Some(2), 42);
    bench_common::step_n(&mut episode_a, 24, "scenario a's first episode");
    bench_common::step_n(&mut episode_b, 24, "scenario b's first episode");

    // The same episode seed into both scenarios' dwellings.
    episode_a.reseed(EPISODE_SEED).expect("scenario a reseeds");
    episode_b.reseed(EPISODE_SEED).expect("scenario b reseeds");
    let streams_a = episode_a.seeded_streams();
    let streams_b = episode_b.seeded_streams();
    assert_eq!(streams_a.len(), streams_b.len(), "the same components");
    for (a, b) in streams_a.iter().zip(&streams_b) {
        assert_ne!(
            a.state.seed, b.state.seed,
            "component {}: the same episode seed under two scenarios must not \
             share a dwelling key",
            a.component
        );
        assert_eq!(a.state.stream, b.state.stream, "component {}", a.component);
        assert_eq!(
            a.state.word_pos, b.state.word_pos,
            "component {}: both reseeds start every stream at its own start",
            a.component
        );
    }

    // The composition's exactness: each reseeded dwelling's streams are
    // the streams a fresh construction under (the episode seed, its own
    // scenario) holds at build, every component, key, nonce and word
    // position. The zips assert equal lengths first: a truncated zip
    // would pass the composition with a component missing.
    let fresh_a = scenario_dwelling(dir, Some(1), EPISODE_SEED);
    let fresh_b = scenario_dwelling(dir, Some(2), EPISODE_SEED);
    let fresh_streams_a = fresh_a.seeded_streams();
    let fresh_streams_b = fresh_b.seeded_streams();
    assert_eq!(
        streams_a.len(),
        fresh_streams_a.len(),
        "the reseeded scenario a holds the components a fresh build does"
    );
    assert_eq!(
        streams_b.len(),
        fresh_streams_b.len(),
        "the reseeded scenario b holds the components a fresh build does"
    );
    for (reseeded, fresh) in streams_a
        .iter()
        .zip(&fresh_streams_a)
        .chain(streams_b.iter().zip(&fresh_streams_b))
    {
        assert_eq!(
            reseeded.state, fresh.state,
            "component {}: the reseed's effective derivation is the \
             scenario-mixed construction derivation with the episode seed \
             in the master-seed position",
            reseeded.component
        );
    }

    // The class-specific composition: the fixture's two stochastic
    // classes compose by their own derivations, not as a global
    // swap-and-drop. The event-load component keys on the scenario-mixed
    // episode key and rides the tagged event-load nonce partition; the
    // built-in EV driver keys on the same key and rides the reserved
    // EV-driver stream the construction assigned it.
    let episode_key = hares_types::rng::dwelling_seed_scenario(EPISODE_SEED, Some(1), 1);
    let event_load = streams_a
        .iter()
        .find(|stream| stream.state.stream & hares_types::rng::EVENT_LOAD_STREAM_TAG != 0)
        .expect("the fixture's event-load stream is in the seeded set");
    assert_eq!(
        event_load.state.seed, episode_key,
        "the event load's stream keys on the scenario-mixed episode key"
    );
    assert_eq!(
        event_load.state.word_pos, 0,
        "the reseeded event load draws from its start"
    );
    let driver = streams_a
        .iter()
        .find(|stream| stream.component.starts_with("actor 'EvDriver:"))
        .expect("the fixture's built-in EV driver is in the seeded set");
    assert_eq!(
        driver.state.stream,
        hares_core::rng::RNG_STREAM_EV_DRIVER_BASE,
        "the built-in driver's stream nonce is the reserved base the \
         construction assigns the first driver"
    );
    assert_eq!(
        driver.state.seed, episode_key,
        "the built-in driver's stream keys on the scenario-mixed episode key"
    );
    assert_eq!(
        driver.state.word_pos, 0,
        "the reseeded driver draws from its start"
    );
}
