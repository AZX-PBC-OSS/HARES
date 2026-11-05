//! The stochastic lifecycle's end-to-end reproducibility pins.
//!
//! Three invariants over one EV-archetype dwelling whose episode crosses a
//! day boundary (the EV driver's daily miles are a LogNormal draw that
//! re-rolls at the day roll):
//!
//! 1. The gym's `reset(seed)` replays an episode bit-identically when the
//!    checkpoint and the reseed land BEFORE the day boundary and the roll
//!    happens inside the replayed window: the day-roll state (the driver's
//!    day counter and stream position) travels in the checkpoint.
//! 2. A mid-episode checkpoint roundtrip (save, restore, continue, no
//!    reseed) continues bit-identically across a LogNormal day-roll draw:
//!    the stream cursors the draw consumes are checkpoint state.
//! 3. The event load's start rate follows the per-step probability
//!    normalization (`p_step = 1 - (1 - p_hour)^(dt/h)`) at every
//!    resolution: the dwell's step semantics count partial powered
//!    dwell-time (the sub-step duty), so the integrated duty is the
//!    successful-draw count the normalization's closed form pins at 60,
//!    300, 900 and 3600 s.
//!
//! Fixtures: the synthetic single-zone TOML dwelling the
//! `warmup_stochastic_neutrality` tests build carrying the EV equipment
//! and its canonical driver (the catalogue archetype's stochastic driving
//! sources; warm-up disabled, so the lifecycle pins hold from the
//! construction state); the event-rate arm rides the committed ResStock
//! sample with a raw-spec EventBasedLoad instead, because the synthetic
//! `[event_load]` path's rate is modulated by the resolver's
//! annual-energy reconciliation and cannot observe the raw normalization.

use hares_core::dwelling::DwellingBlueprintBuilder;
use hares_core::{
    Dwelling, DwellingCheckpoint, DwellingConfig, DwellingSettings, SimulationConfig,
};

use hares_equipment::Equipment;
use hares_equipment::config::EquipmentConfig;
use hares_equipment::ev::Ev;
use hares_equipment::ev::catalog::{EvArchetypeId, archetype_by_id};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng as EngineRng;

use hares_core::actors::{EvDriverActor, EvDriverParams};
use hares_core::rng;

/// The repo root the committed fixtures resolve from.
fn project_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The EV pack the archetype dwelling carries (the neutrality tests' size).
const EV_CAPACITY_J: f64 = 60.0 * 3.6e6;

/// The archetype dwelling: the synthetic TOML (no HVAC, no gains, the
/// stochastic event load) with the EV equipment and the canonical driver
/// actor built the way the Python `add_ev_with_driver` path builds it.
fn ev_archetype_dwelling(archetype: EvArchetypeId, master_seed: u64) -> Dwelling {
    let toml = format!(
        r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 900
duration = 172800
output_chunk_size = 1000
master_seed = {master_seed}
warmup = "disabled"

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
    );
    let mut tmp = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("tempfile must be createable");
    std::io::Write::write_all(&mut tmp, toml.as_bytes()).expect("write to tempfile must succeed");
    let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");

    let ev_config = EquipmentConfig::from_typed(
        "EV1".to_string(),
        "EV".to_string(),
        hares_equipment::ev::EvConfig {
            equipment_id: None,
            capacity_j: EV_CAPACITY_J,
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
        capacity_j: EV_CAPACITY_J,
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
            &EngineRng::from_seed(seed_bytes),
            rng::RNG_STREAM_EV_DRIVER_BASE,
        ),
    );

    dwelling
        .add_equipment_with_actors(Box::new(ev), vec![Box::new(driver)])
        .expect("the EV and its driver join");
    dwelling
}

/// The per-step stochastic telemetry, bitwise: the EV's power and SOC and
/// the actor telemetry's channels (the surfaces the day roll drives), plus
/// the event load's power (its own stream).
fn step_bits(dwelling: &mut Dwelling) -> Vec<u64> {
    let step = dwelling.step().expect("the step runs");
    let mut bits = vec![
        step.net_electric_power_w.to_bits(),
        step.hvac_heating_w.to_bits(),
        step.hvac_cooling_w.to_bits(),
    ];
    let telem = dwelling.telemetry().expect("telemetry");
    for power in &telem.equipment_power_w {
        bits.push(power.to_bits());
    }
    for soc in &telem.equipment_soc {
        bits.push(soc.to_bits());
    }
    for (actor, channels) in &telem.actor_telemetry {
        bits.push(actor.len() as u64);
        for (channel, value) in channels {
            bits.push(channel.len() as u64);
            bits.push(value.to_bits());
        }
    }
    bits
}

/// The dwelling's current wall-clock hour (the step the day boundary sits
/// before).
fn current_hour(dwelling: &Dwelling) -> u32 {
    dwelling
        .latest_env()
        .current_time
        .format("%H")
        .to_string()
        .parse()
        .expect("the wall clock reads an hour")
}

/// The reset(seed) episode replay, with the checkpoint landing at 23:00 and
/// the day roll (midnight) inside the replayed window: the roll's state
/// travels in the checkpoint and the replay is bit-identical.
#[test]
fn reset_before_the_day_boundary_replays_the_roll_bit_identically() {
    let mut dwelling = ev_archetype_dwelling(EvArchetypeId::DailyCommuterL2, 0);

    // Step to 23:00 (the build's clock starts at 12:00; step until the
    // wall clock reads the hour, the way the DR suite arms its events).
    let mut armed = false;
    for _ in 0..(2 * 86_400 / 900) {
        if current_hour(&dwelling) == 23 {
            armed = true;
            break;
        }
        dwelling.step().expect("the approach to 23:00");
    }
    assert!(armed, "the run never reached the 23:00 hour");

    // The gym's reset: the checkpoint, the reseed, the episode. The
    // replayed window runs 3 h past 23:00 (12 steps at 900 s): the
    // midnight roll happens inside it.
    let episode_steps = 3 * (3_600 / 900);
    let initial: DwellingCheckpoint = dwelling.save_checkpoint().expect("the 23:00 checkpoint");
    dwelling.reseed(7).expect("the first episode reseeds");
    let episode_one: Vec<Vec<u64>> = (0..episode_steps)
        .map(|_| step_bits(&mut dwelling))
        .collect();
    assert_eq!(
        current_hour(&dwelling),
        2,
        "episode one crossed the midnight boundary"
    );

    dwelling
        .load_checkpoint(initial)
        .expect("the 23:00 state restores");
    dwelling.reseed(7).expect("the second episode reseeds");
    let episode_two: Vec<Vec<u64>> = (0..episode_steps)
        .map(|_| step_bits(&mut dwelling))
        .collect();

    assert_eq!(
        episode_one, episode_two,
        "reset(seed) before the day boundary must replay the roll \
         bit-identically: a difference is day-roll state the checkpoint \
         did not carry or the reseed did not re-derive"
    );
}

/// The mid-episode checkpoint roundtrip across a LogNormal draw: the
/// archetype's daily-miles source is LogNormal (the NHTS right-skew the
/// catalogue documents); the checkpoint lands at 23:00, the continuation
/// crosses the midnight roll where the LogNormal draw fires, and the
/// restored run continues bit-identically to the uninterrupted one. No
/// reseed: the streams continue from their checkpoint cursors.
#[test]
fn checkpoint_roundtrip_continues_across_the_lognormal_day_roll() {
    // The uninterrupted reference: step to 23:00, then 3 h past it.
    let continuation_steps = 3 * (3_600 / 900);
    let run_to_boundary = |dwelling: &mut Dwelling, what: &str| {
        for _ in 0..(2 * 86_400 / 900) {
            if current_hour(dwelling) == 23 {
                return;
            }
            dwelling
                .step()
                .unwrap_or_else(|err| panic!("{what}: {err}"));
        }
        panic!("{what}: the run never reached the 23:00 hour");
    };
    let mut reference = ev_archetype_dwelling(EvArchetypeId::DailyCommuterL2, 5);
    run_to_boundary(&mut reference, "the reference's approach");
    let reference_tail: Vec<Vec<u64>> = (0..continuation_steps)
        .map(|_| step_bits(&mut reference))
        .collect();

    // The interrupted run: the same steps, checkpoint at 23:00, restore
    // into a fresh build of the same inputs, continue.
    let mut interrupted = ev_archetype_dwelling(EvArchetypeId::DailyCommuterL2, 5);
    run_to_boundary(&mut interrupted, "the interrupted run's approach");
    let checkpoint: DwellingCheckpoint =
        interrupted.save_checkpoint().expect("the 23:00 checkpoint");
    let mut resumed = ev_archetype_dwelling(EvArchetypeId::DailyCommuterL2, 5);
    resumed
        .load_checkpoint(checkpoint)
        .expect("the checkpoint restores");
    let resumed_tail: Vec<Vec<u64>> = (0..continuation_steps)
        .map(|_| step_bits(&mut resumed))
        .collect();

    assert_eq!(
        reference_tail, resumed_tail,
        "the restored run must continue the uninterrupted one bit-identically \
         across the LogNormal day roll: a difference is stream state the \
         checkpoint did not carry"
    );
}

/// The event load's start rate follows the per-step probability
/// normalization at every resolution: `p_step = 1 - (1 - p_hour)^(dt/h)`,
/// the cycle the geometric idle run plus its own success step
/// (`E[cycle] = (1-p_step)/p_step + 1`, `var(idle) = (1-p_step)/p_step^2`),
/// and the renewal count over N steps with mean `N/E[cycle]` and sd
/// `sqrt(N * var_idle / E[cycle]^3)`.
///
/// The rate's observable at a coarse step is the step's integrated duty,
/// not the power channel's rising edges. The dwell's step semantics count
/// partial powered dwell-time (the sub-step duty): a 30 s dwell inside a
/// 3600 s step carries 1000 W x 30/3600, so the energy integrated over the
/// run, divided by the per-start energy (1000 W x 30 s), is exactly the
/// number of successful draws, the renewal count the closed form pins, at
/// every resolution. All-or-nothing step occupancy inflates that integral
/// by dt/dwell (120x at 3600 s), and read off rising edges the clustered
/// starts merge (two starts in consecutive steps are one run of two
/// powered steps): the pre-fix channel measured 407 starts against 458 at
/// 900 s and 191 against 360 at 3600 s, seed-independent, the drift the
/// coarse arms pin against here.
///
/// The rising edges are pinned separately, against the run structure the
/// draw sequence itself predicts: with every step drawing once (the 30 s
/// dwell exhausts inside any step here and the cooldown is 0) the edges
/// are the maximal consecutive-success blocks, `E[R] = p_step + (N-1) q`
/// and `var(R) = p(1-p) + (N-1) q(1-q) - 2(N-2) q^2 - 2 p q` with
/// `q = p_step (1 - p_step)` (the block-start indicator at step i needs a
/// success at i and a failure before it; adjacent indicators are
/// exclusive, so their covariance is minus the product of their means,
/// and all other pairs are independent). The run pins hold before and
/// after the duty fix: the fix changes no draw and no phase transition,
/// only the power a short dwell carries within its step.
#[test]
fn event_start_rate_follows_the_normalization_at_every_resolution() {
    use hares_input::EquipmentSpec;
    use hares_types::FuelType;

    let days = 30_u64;
    let p_hour = 0.5_f64;

    for time_res_s in [60_u64, 300, 900, 3600] {
        let dt = time_res_s as f64;
        let p_step = 1.0 - (1.0 - p_hour).powf(dt / 3600.0);
        let mean_cycle = (1.0 - p_step) / p_step + 1.0;
        let var_idle = (1.0 - p_step) / (p_step * p_step);
        let n_steps = (days * 86_400 / time_res_s) as f64;
        let expected = n_steps / mean_cycle;
        let sd = (n_steps * var_idle / mean_cycle.powi(3)).sqrt();
        let (low, high) = (expected - 3.0 * sd, expected + 3.0 * sd);

        // The run structure's mean and spread, from the derivation above.
        let q = p_step * (1.0 - p_step);
        let run_mean = p_step + (n_steps - 1.0) * q;
        let run_var = p_step * (1.0 - p_step) + (n_steps - 1.0) * q * (1.0 - q)
            - 2.0 * (n_steps - 2.0) * q * q
            - 2.0 * p_step * q;
        let run_sd = run_var.max(0.0).sqrt();
        let (run_low, run_high) = (run_mean - 3.0 * run_sd, run_mean + 3.0 * run_sd);

        // The probe: an EventBasedLoad with the parameters this test pins,
        // on the committed ResStock sample dwelling (the stream keyed on
        // the load's identity as every event load's is).
        let spec = EquipmentSpec {
            name: "EventBasedLoad".to_string(),
            instance_name: None,
            fuel_type: FuelType::Electric,
            parameters: serde_json::json!({
                "active_power_w": 1000.0,
                "active_duration_s": 30.0,
                "cooldown_duration_s": 0.0,
                "event_window_source": "constant",
                "event_probability_source": "constant",
                "event_probability_constant": p_hour,
                "sensible_gain_fraction": 0.0,
            })
            .as_object()
            .expect("probe parameters are an object")
            .clone(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        };
        let mut blueprint =
            DwellingBlueprintBuilder::from_config(dwelling_config_resstock(time_res_s, days))
                .expect("blueprint");
        blueprint
            .add_equipment_spec(spec)
            .expect("add the probe spec");
        let mut dwelling = blueprint.build().expect("build dwelling");

        // The start count reads off the integrated duty: the step's average
        // power times the step length, summed over the run, divided by the
        // per-start energy (1000 W x 30 s). The dwell completes inside any
        // step here (30 s against a 60 s floor), so each start contributes
        // exactly its own dwell to the integral.
        let per_start_energy_ws = 1000.0 * 30.0;
        let mut duty_energy_ws = 0.0;
        // The rising edges count the runs of consecutive powered steps.
        let mut edges = 0_u64;
        let mut previous_drawing = false;
        for _ in 0..(n_steps as u64) {
            dwelling.step().expect("the step runs");
            let power_w = probe_power(&dwelling);
            duty_energy_ws += power_w * dt;
            let drawing = power_w > 1e-9;
            if drawing && !previous_drawing {
                edges += 1;
            }
            previous_drawing = drawing;
        }
        let duty_starts = duty_energy_ws / per_start_energy_ws;

        assert!(
            duty_starts >= low && duty_starts <= high,
            "time_res {time_res_s} s: {duty_starts:.1} event starts by duty \
             integral in {days} days outside the 3-sigma renewal band \
             [{low:.1}, {high:.1}] (mean {expected:.1}, sd {sd:.2}, p_step \
             {p_step:.6}): the per-step probability normalization drifted",
        );
        assert!(
            edges as f64 >= run_low && edges as f64 <= run_high,
            "time_res {time_res_s} s: {edges} powered runs in {days} days \
             outside the 3-sigma run-structure band [{run_low:.1}, \
             {run_high:.1}] (mean {run_mean:.1}, sd {run_sd:.2}, q {q:.6}): \
             the draw sequence's clustering moved",
        );
        eprintln!(
            "[lifecycle] time_res {time_res_s} s: {duty_starts:.0} starts by \
             duty (band {low:.0}-{high:.0}, mean {expected:.0}), {edges} runs \
             (band {run_low:.0}-{run_high:.0}, mean {run_mean:.0})"
        );
    }
}

/// The ResStock sample dwelling's config the probe rides: the committed
/// 2025.1 bldg0000007 fixtures, the arm's timestep and horizon, warm-up
/// disabled.
fn dwelling_config_resstock(time_res_s: u64, days: u64) -> DwellingConfig {
    use chrono::TimeZone as _;
    let fixture = project_root().join("tests/fixtures/resstock/2025.1/bldg0000007");
    DwellingConfig {
        hpxml: hares_input::InputSource::Path(fixture.join("home.xml")),
        schedule: Some(hares_input::InputSource::Path(
            fixture.join("in.schedules.csv"),
        )),
        weather: hares_input::WeatherInput::Path(
            project_root().join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        ),
        defaults: hares_input::DefaultsSource::directory(
            project_root().join("python/hares/defaults"),
        ),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: chrono::FixedOffset::east_opt(10 * 3600)
                    .expect("UTC+10 offset is valid")
                    .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                    .unwrap(),
                duration: chrono::Duration::seconds((days * 86_400) as i64),
                time_res: chrono::Duration::seconds(time_res_s as i64),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 100000,
                max_memory_rows: None,
                setpoint_deadband_c: None,
                master_seed: 7,
                civil_timezone: None,
                site_location: hares_input::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_input::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_input::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_input::WeatherYear::default(),
                warmup: hares_input::Warmup::Disabled,
            },
            None,
            100,
            None,
        ),
    }
}

/// The probe's own active power channel, in W: the telemetry key keeps its
/// kW reporting spelling and carries W (loads.md), as the equipment's own
/// pins assert.
fn probe_power(dwelling: &Dwelling) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get("EventBasedLoad")
        .and_then(|t| t.get(hares_types::telemetry_keys::ACTIVE_POWER_KW))
        .unwrap_or_else(|| panic!("the probe publishes its power"))
}
