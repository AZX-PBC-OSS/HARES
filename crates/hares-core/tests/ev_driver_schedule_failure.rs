//! The EV driver's schedule-read failures propagate as step failures.
//!
//! A driver whose driving schedule cannot be read must fail the step
//! naming the actor and the source, riding the post-decide health check's
//! step-failure path. It must never fabricate the day from defaults: the
//! fabricated-default fallback drove and charged the vehicle on invented
//! times whenever a source failed to resolve.

use hares_core::{Dwelling, actors, actors::EvDriverActor, actors::EvDriverParams};
use hares_equipment::Equipment;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// A minimal synthetic dwelling: no HVAC, no gains, constant weather. The
/// EV and its driver join after construction, the way the Python
/// `add_ev_with_driver` surface adds them.
fn failure_toml() -> String {
    r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 900
duration = 86400
output_chunk_size = 1000
master_seed = 0
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
"#
    .to_string()
}

fn write_toml(content: &str) -> tempfile::NamedTempFile {
    let mut tmp = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("tempfile must be createable");
    std::fs::write(&mut tmp, content).expect("write to tempfile must succeed");
    tmp
}

/// The EV equipment, the way the Python add-ev-with-driver path builds it.
fn ev_equipment(dwelling: &Dwelling) -> hares_equipment::ev::Ev {
    use hares_equipment::config::EquipmentConfig;
    use hares_equipment::ev::Ev;

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
    ev
}

/// A driver whose departure source cannot resolve a value: its only
/// window matches Mondays and the run steps a Saturday (2024-06-15), and
/// no default is set. Every other source stays a constant.
fn failing_departure_driver() -> EvDriverActor {
    use chrono::Weekday;
    use hares_types::{DayFilter, TimeWindow};

    let params = EvDriverParams {
        strategy: hares_types::ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: hares_types::PlugInPolicy::Always,
        daily_drive_distance_m: hares_types::ScheduleSource::Constant(48_280.32),
        departure_time: hares_types::ScheduleSource::TimeWindows {
            windows: vec![TimeWindow::new(
                DayFilter::Day(Weekday::Mon),
                0,
                1440,
                480.0,
            )],
            default: None,
            rng_state: None,
        },
        trip_duration: hares_types::ScheduleSource::Constant(600.0),
        arrival_time: None,
        event_day_ratio: 1.0,
        fuel_economy_j_per_m: 0.3 * 3.6e6 / 1609.344,
        capacity_j: 60.0 * 3.6e6,
        max_charge_w: 7200.0,
        average_speed_m_per_s: actors::ev_driver::defaults::AVERAGE_SPEED_M_PER_S,
        range_anxiety_distance_m: actors::ev_driver::defaults::RANGE_ANXIETY_DISTANCE_M,
        away_charge_fraction: 0.0,
        away_charge_power_w: 0.0,
    };
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&7u64.to_le_bytes());
    EvDriverActor::new("EvDriver:EV1", "EV1", params, ChaCha8Rng::from_seed(bytes))
}

/// A driver whose departure source cannot be read fails the step naming
/// the actor and the source. The fabricated-default fallback instead
/// produced a full invented day: the vehicle departed at the default
/// 8:00, drove the default 30 miles and charged on the invented arrival.
#[test]
fn an_unreadable_driving_schedule_fails_the_step_naming_the_actor() {
    let tmp = write_toml(&failure_toml());
    let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");

    dwelling
        .add_equipment_with_actors(
            Box::new(ev_equipment(&dwelling)),
            vec![Box::new(failing_departure_driver())],
        )
        .expect("the EV and its driver join the dwelling");

    let err = dwelling
        .step()
        .expect_err("the step must fail: the driver cannot read its departure schedule");
    let msg = err.to_string();
    assert!(
        msg.contains("EvDriver:EV1"),
        "the step error must name the actor, got: {msg}"
    );
    assert!(
        msg.contains("departure"),
        "the step error must name the source that could not be read, got: {msg}"
    );
}

/// The same dwelling with a readable schedule steps: the failure is the
/// schedule's, not the driver's presence.
#[test]
fn a_readable_driving_schedule_steps() {
    use hares_types::ScheduleSource;

    let tmp = write_toml(&failure_toml());
    let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");

    // A driver whose sources all resolve on any day.
    let params = EvDriverParams {
        strategy: hares_types::ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: hares_types::PlugInPolicy::Always,
        daily_drive_distance_m: ScheduleSource::Constant(48_280.32),
        departure_time: ScheduleSource::Constant(480.0),
        trip_duration: ScheduleSource::Constant(600.0),
        arrival_time: None,
        event_day_ratio: 1.0,
        fuel_economy_j_per_m: 0.3 * 3.6e6 / 1609.344,
        capacity_j: 60.0 * 3.6e6,
        max_charge_w: 7200.0,
        average_speed_m_per_s: actors::ev_driver::defaults::AVERAGE_SPEED_M_PER_S,
        range_anxiety_distance_m: actors::ev_driver::defaults::RANGE_ANXIETY_DISTANCE_M,
        away_charge_fraction: 0.0,
        away_charge_power_w: 0.0,
    };
    let readable = EvDriverActor::new("EvDriver:EV1", "EV1", params, ChaCha8Rng::seed_from_u64(7));
    dwelling
        .add_equipment_with_actors(Box::new(ev_equipment(&dwelling)), vec![Box::new(readable)])
        .expect("the EV and its driver join the dwelling");

    dwelling
        .step()
        .expect("a readable schedule steps: the failure is the schedule's");
}

/// The driver's failure names the actor in the post-decide health check's
/// step error, whose text rides the batch boundary unchanged: the message
/// must be self-sufficient (the typed step-failure struct across the
/// Python boundary is a later change; this path is its text source).
#[test]
fn the_step_failure_text_is_self_sufficient() {
    let tmp = write_toml(&failure_toml());
    let mut dwelling = Dwelling::from_toml_config(tmp.path()).expect("the dwelling builds");

    dwelling
        .add_equipment_with_actors(
            Box::new(ev_equipment(&dwelling)),
            vec![Box::new(failing_departure_driver())],
        )
        .expect("the EV and its driver join the dwelling");

    let err = dwelling.step().expect_err("the step must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("EvDriver:EV1")
            && msg.contains("departure")
            && msg.contains("no time window matches"),
        "the step error must carry the actor, the source and the read's own \
         cause without further context, got: {msg}"
    );
}
