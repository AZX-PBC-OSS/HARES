//! The warm-up's stochastic-state neutrality and the gym's seed contract.
//!
//! Two invariants over the stochastic lifecycle:
//!
//! 1. The warm-up day count never reaches production: a dwelling whose
//!    warm-up runs two days and one whose warm-up runs four produce
//!    bit-identical production telemetry. The warm-up draws from every
//!    stochastic schedule source and event-start stream; production starts
//!    with each at its construction position, whatever the warm-up drew.
//! 2. The gym's `reset(seed)` is exact: save the dwelling at step 0, reseed,
//!    run an episode; restore the step-0 state, reseed with the same seed,
//!    run again; the two episodes are bit-identical. Every stochastic stream
//!    the episode draws from is carried in the checkpoint, the EV driver's
//!    driving sources included.

use hares_core::{Dwelling, DwellingCheckpoint, WarmupOutcome};
use hares_equipment::ev::catalog::EvArchetypeId;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// A minimal synthetic dwelling with a stochastic event load: no HVAC, no
/// internal gains, constant weather with the clear-sky solar off. The
/// event load's gain fractions are zero (its starts move no zone
/// temperature), so the stochastic state drives nothing but its own
/// power: a difference in the stochastic telemetry channels is a
/// stochastic-state difference, nothing else. The zone temperature is
/// not compared bitwise: it carries warm-up's thermal initial condition,
/// whose day-count dependence the warm-up convergence criteria bound -
/// it is warm-up's product, not stochastic carryover - and in this
/// configuration it feeds back into nothing the stochastic state reads.
fn neutrality_toml(min_days: u32, max_days: u32) -> String {
    format!(
        r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 900
duration = 172800
output_chunk_size = 1000
master_seed = 0
warmup = {{ min_days = {min_days}, max_days = {max_days} }}

[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325
clear_sky_solar = false

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0

[event_load]
active_power_w = 1500.0
active_duration_s = 900.0
event_probability = 0.5
sensible_gain_fraction = 0.0
latent_gain_fraction = 0.0
"#
    )
}

fn write_toml(content: &str) -> tempfile::NamedTempFile {
    let mut tmp = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("tempfile must be createable");
    std::fs::write(&mut tmp, content).expect("write to tempfile must succeed");
    tmp
}

fn warmup_days_run(dwelling: &Dwelling) -> u32 {
    match &dwelling.health().warmup {
        WarmupOutcome::Ran { days_run, .. } => *days_run,
        other => panic!("expected a ran warm-up, got {other:?}"),
    }
}

/// The per-step production telemetry, bitwise: the net electric power,
/// the HVAC delivery, the gas burn, and every equipment and actor
/// telemetry channel - the surfaces the stochastic state drives. The
/// zone temperatures are read for nothing here: they carry warm-up's
/// thermal initial condition (the convergence criteria's to bound), and
/// in this configuration they feed back into nothing the stochastic
/// state reads.
fn step_bits(dwelling: &mut Dwelling) -> Vec<u64> {
    let step = dwelling.step().expect("the production step runs");
    let mut bits: Vec<u64> = vec![
        step.net_electric_power_w.to_bits(),
        step.hvac_heating_w.to_bits(),
        step.hvac_cooling_w.to_bits(),
        step.gas_power_w.to_bits(),
    ];
    let telemetry = dwelling.telemetry().expect("telemetry");
    for power in &telemetry.equipment_power_w {
        bits.push(power.to_bits());
    }
    for soc in &telemetry.equipment_soc {
        bits.push(soc.to_bits());
    }
    // The actor telemetry's channels, in the deterministic BTreeMap order.
    for (actor, channels) in &telemetry.actor_telemetry {
        bits.push(actor.len() as u64);
        for (channel, value) in channels {
            bits.push(channel.len() as u64);
            bits.push(value.to_bits());
        }
    }
    bits
}

/// Warm-up ran `days_run` days, production ran two simulated days, and the
/// two dwellings' production telemetry is bit-identical: the warm-up day
/// count never reaches production.
#[test]
fn production_telemetry_is_invariant_to_the_warmup_day_count() {
    let run = |min_days: u32| {
        let tmp = write_toml(&neutrality_toml(min_days, min_days));
        let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");
        let days = warmup_days_run(&dwelling);
        assert_eq!(
            days, min_days,
            "the forced warm-up day count must be exact for this test to mean anything"
        );
        let steps = (86_400 / 900) * 2;
        let bits: Vec<Vec<u64>> = (0..steps).map(|_| step_bits(&mut dwelling)).collect();
        bits
    };

    let at_two_days = run(2);
    let at_four_days = run(4);
    assert_eq!(
        at_two_days, at_four_days,
        "production telemetry must be bit-identical whatever the warm-up \
         day count was: a difference is stochastic state the warm-up \
         carried into production"
    );
}

/// The EV-archetype dwelling: the EV equipment with the canonical driver
/// actor carrying a catalogue archetype's stochastic driving sources (the
/// Python `add_ev_with_driver` construction), beside a stochastic event
/// load. The driver's day-roll rides the reserved EV-driver stream nonce
/// the way the registry and Python paths derive it, and its four driving
/// sources carry the archetype's tagged keys.
fn ev_archetype_dwelling(archetype: EvArchetypeId, min_days: u32, max_days: u32) -> Dwelling {
    use hares_core::actors::{EvDriverActor, EvDriverParams};
    use hares_core::rng;
    use hares_equipment::Equipment;
    use hares_equipment::config::EquipmentConfig;
    use hares_equipment::ev::Ev;
    use hares_equipment::ev::catalog::archetype_by_id;

    let tmp = write_toml(&neutrality_toml(min_days, max_days));
    let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");

    // The EV equipment, the way the Python add-ev-with-driver path builds
    // it: typed config, init against the dwelling's environment.
    let ev_config = EquipmentConfig::from_typed(
        "EV1".to_string(),
        "EV".to_string(),
        hares_equipment::ev::EvConfig {
            equipment_id: None,
            capacity_j: 60.0 * 3.6e6,
            charging_level: None,
            max_charging_power_w: 7200.0,
            charging_efficiency: None,
            l1_current_a: None,
            l1_voltage_v: None,
            soc_max: None,
            initial_soc: Some(0.5),
            battery_temp_c: None,
            min_charge_temp_c: None,
            full_power_temp_c: None,
            heater_power_w: None,
            heater_threshold_c: None,
            thermal_mass_j_per_k: None,
            ua_w_per_k: None,
            n_series: None,
            n_parallel: None,
            cell_resistance_ohm: None,
            v2l_enabled: None,
            v2l_soc_reserve: None,
            v2l_max_discharge_w: None,
            v2g_enabled: None,
            v2g_soc_reserve: None,
            v2g_max_discharge_w: None,
            chemistry: None,
            fuel_economy_j_per_m: None,
            ready_soc: None,
            charging_strategy: None,
            plug_in_policy: None,
            power_limit_w: None,
            initial_connection_state: None,
            power_factor: None,
            charger_capacity_va: None,
            cc_cv_transition_soc: None,
            charging_priority: None,
            discharge_respects_deadline: true,
            der_1547: None,
        },
    )
    .expect("typed EV config");
    let mut ev = Ev::new(ev_config.clone());
    ev.init(&ev_config, dwelling.latest_env()).expect("EV init");

    // The driver: the archetype's own stochastic sources (miles, departure,
    // duration, arrival), the caller's seed mixed with the dwelling's
    // identity the way the Python add-ev-with-driver path derives it.
    let preset = archetype_by_id(archetype.to_string().as_str()).expect("the archetype exists");
    let seed_bytes = hares_core::actors::ev_driver::driver_key_material(7, dwelling.bldg_id);
    let params = EvDriverParams {
        strategy: preset.strategy.clone(),
        plug_in_policy: preset.plug_in_policy.clone(),
        daily_drive_distance_m: preset.build_distance_schedule(seed_bytes),
        departure_time: preset.build_departure_schedule(seed_bytes),
        trip_duration: preset.build_duration_schedule(seed_bytes),
        arrival_time: preset.build_arrival_schedule(seed_bytes),
        event_day_ratio: preset.event_day_ratio,
        fuel_economy_j_per_m: 0.3 * 3.6e6 / 1609.344,
        capacity_j: 60.0 * 3.6e6,
        max_charge_w: 7200.0,
        average_speed_m_per_s: hares_core::actors::ev_driver::defaults::AVERAGE_SPEED_M_PER_S,
        range_anxiety_distance_m: hares_core::actors::ev_driver::defaults::RANGE_ANXIETY_DISTANCE_M,
        away_charge_fraction: 0.0,
        away_charge_power_w: 0.0,
    };
    let driver = EvDriverActor::new(
        "EvDriver:EV1",
        "EV1",
        params,
        rng::derive_sub_rng(
            &ChaCha8Rng::from_seed(seed_bytes),
            rng::RNG_STREAM_EV_DRIVER_BASE,
        ),
    );

    dwelling
        .add_equipment_with_actors(Box::new(ev), vec![Box::new(driver)])
        .expect("the EV and its driver join");
    dwelling
}

/// The gym's `reset(seed)` on the EV-archetype dwelling: the step-0
/// checkpoint restored, the streams reseeded, the episode run again -
/// bit-identical the second time. The checkpoint carries the four driving
/// sources' positions beside the day-roll's (seed, stream, word) triple,
/// so the tagged source keys ride the snapshot unchanged.
#[test]
fn gym_style_reset_seed_twice_is_bit_identical_on_an_ev_archetype_dwelling() {
    let mut dwelling = ev_archetype_dwelling(EvArchetypeId::DailyCommuterL2, 2, 2);

    // Episode one: the step-0 state, the streams reseeded, one episode run.
    let initial: DwellingCheckpoint = dwelling.save_checkpoint().expect("the step-0 checkpoint");
    dwelling.reseed(7).expect("the first episode reseeds");
    let steps = (86_400 / 900) * 2;
    let episode_one: Vec<Vec<u64>> = (0..steps).map(|_| step_bits(&mut dwelling)).collect();

    // Episode two: the gym's reset - the warmed-up state restored, the
    // streams reseeded with the same seed, no rebuild, no second warm-up.
    dwelling
        .load_checkpoint(initial)
        .expect("the step-0 state restores");
    dwelling.reseed(7).expect("the second episode reseeds");
    let episode_two: Vec<Vec<u64>> = (0..steps).map(|_| step_bits(&mut dwelling)).collect();

    assert_eq!(
        episode_one, episode_two,
        "reset(seed) twice must replay the episode bit-identically: a \
         difference is stochastic state the checkpoint did not carry or \
         the reseed did not re-derive"
    );
}

/// The warm-up day count never reaches production when the dwelling's
/// production phase carries the EV driver's noisy sources (the noisy
/// TimeWindows miles beside the Gaussian departure, duration and arrival)
/// drawing over the event load's own stream: two noisy sources across
/// perturbed warm-up lengths give bit-identical production telemetry.
#[test]
fn warmup_day_count_neutrality_holds_with_the_ev_drivers_noisy_sources() {
    let run = |min_days: u32| {
        let mut dwelling = ev_archetype_dwelling(EvArchetypeId::WfhOccasional, min_days, min_days);
        let steps = (86_400 / 900) * 2;
        (0..steps)
            .map(|_| step_bits(&mut dwelling))
            .collect::<Vec<_>>()
    };

    let at_two_days = run(2);
    let at_four_days = run(4);
    assert_eq!(
        at_two_days, at_four_days,
        "production telemetry must be bit-identical whatever the warm-up \
         day count was: a difference is stochastic state the warm-up \
         carried into production"
    );
}
