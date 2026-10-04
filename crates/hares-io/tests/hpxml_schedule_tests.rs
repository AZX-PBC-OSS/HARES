use std::path::PathBuf;

use chrono::{DateTime, Datelike, Duration, FixedOffset, TimeZone, Timelike};
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml_schedule::generate_default_schedule;
use hares_io::load_default_profiles;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn test_start() -> DateTime<FixedOffset> {
    denver_offset::denver_offset()
        .with_ymd_and_hms(2019, 1, 1, 0, 0, 0)
        .unwrap()
}

fn shipped_profiles() -> hares_io::DefaultProfiles {
    load_default_profiles(&project_root().join("defaults"))
        .expect("the shipped defaults CSV must load")
}

#[test]
fn generated_schedule_has_all_required_columns() {
    let sched = generate_default_schedule(
        test_start(),
        Duration::hours(1),
        Duration::minutes(1),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");

    let required = &[
        "occupants",
        "plug_loads_other",
        "plug_loads_tv",
        "lighting_interior",
        "dishwasher",
        "clothes_washer",
        "clothes_dryer",
        "cooking_range",
        "hot_water_dishwasher",
        "hot_water_clothes_washer",
        "hot_water_fixtures",
    ];
    for col in required {
        assert!(sched.column_index.contains_key(*col), "missing: {col}");
    }
    for col in ["heating_setpoint", "cooling_setpoint"] {
        assert!(
            !sched.column_index.contains_key(col),
            "a generated {col} column would override the HVAC's own setpoints"
        );
    }
}

#[test]
fn generated_schedule_timestamps_are_correct() {
    let sched = generate_default_schedule(
        test_start(),
        Duration::hours(3),
        Duration::minutes(15),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");
    assert_eq!(sched.len(), 12);
    assert_eq!(sched.timestamps[0], test_start());
    assert_eq!(
        sched.timestamps[11],
        test_start() + Duration::minutes(11 * 15)
    );
}

#[test]
fn generated_schedule_uses_default_profiles_when_dir_provided() {
    let sched = generate_default_schedule(
        test_start(),
        Duration::hours(24),
        Duration::minutes(60),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");
    // Occupancy varies over the day when using defaults profile
    let occ_idx = sched.column_index["occupants"];
    let midnight = sched.columns[occ_idx][0];
    let noon = sched.columns[occ_idx][12];
    assert!(midnight > 0.0 && noon > 0.0);
    assert!(
        (midnight - noon).abs() > 1e-9,
        "occupancy should vary: midnight={midnight}, noon={noon}"
    );
}

/// The generated schedule's columns, names, index, and aggregations agree on
/// one column count, the names are unique, and `occupants` is exactly the
/// `Occupancy` profile series: no constant stand-in column, no length drift
/// between a column vector and its aggregation.
#[test]
fn generated_schedule_has_one_occupants_column() {
    let profiles = shipped_profiles();
    let sched = generate_default_schedule(
        test_start(),
        Duration::hours(24),
        Duration::minutes(60),
        &profiles,
    )
    .expect("the shipped profiles generate the schedule");

    let mut names: Vec<&str> = sched.column_names.iter().map(String::as_str).collect();
    names.sort_unstable();
    let unique = names.len();
    names.dedup();
    assert_eq!(
        unique,
        names.len(),
        "generated column names must be unique, got: {names:?}"
    );
    assert_eq!(sched.columns.len(), sched.column_names.len());
    assert_eq!(sched.column_aggregations.len(), sched.column_names.len());
    assert_eq!(sched.column_index.len(), sched.column_names.len());

    // occupants equals the Occupancy profile series: for every timestamp the
    // weekday or weekend fraction times the month multiplier.
    let occupancy = profiles
        .get("Occupancy")
        .expect("the shipped defaults CSV has an Occupancy profile");
    let occ_idx = sched.column_index["occupants"];
    assert_eq!(sched.column_names[occ_idx], "occupants");
    for (i, ts) in sched.timestamps.iter().enumerate() {
        let hour = ts.hour() as usize;
        let month = ts.month0() as usize;
        let is_weekend = ts.weekday().num_days_from_monday() >= 5;
        let expected = if is_weekend {
            occupancy.weekend_fractions[hour]
        } else {
            occupancy.weekday_fractions[hour]
        } * occupancy.month_multipliers[month];
        assert_eq!(
            sched.columns[occ_idx][i], expected,
            "occupants[{i}] must be the Occupancy profile value"
        );
    }
}

/// With no schedule file, the HVAC keeps the HPXML's hourly weekday and
/// weekend setpoints: the generated schedule carries no setpoint column to
/// override them.
#[test]
fn hvac_without_a_schedule_file_keeps_the_hpxml_hourly_setpoints() {
    let sample = project_root()
        .join("vendors/OCHRE/test/OS-HPXML Sample Files/base-hvac-setpoints-daily-schedules.xml");
    let xml = std::fs::read_to_string(&sample)
        .expect("sample readable")
        .replacen(
            "</StateCode>",
            "</StateCode><Latitude>39.7</Latitude><Longitude>-105.0</Longitude>",
            1,
        );
    let building = parse_building(&xml).expect("parse");
    let defaults_dir = project_root().join("defaults");
    let defaults = hares_io::defaults::DefaultsStore::load(&defaults_dir).expect("defaults load");
    let mut specs = hares_io::resolve_equipment(
        &building,
        &defaults,
        &serde_json::json!({}),
        None,
        &mut Vec::new(),
    )
    .expect("resolve");
    let mut schedule = generate_default_schedule(
        test_start(),
        Duration::hours(24),
        Duration::minutes(60),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");
    hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&defaults_dir),
        &defaults,
        None,
        false,
        &mut Vec::new(),
    )
    .expect("inject");
    let furnace = specs
        .iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("the sample has a gas furnace");
    let hares_equipment::ConfigPayload::Typed { data, .. } = &furnace
        .typed_config
        .as_ref()
        .expect("typed furnace config")
        .payload
    else {
        panic!("typed payload");
    };
    let source: hares_types::ScheduleSourceConfig =
        serde_json::from_value(data["setpoint"]["heating_setpoint_source"].clone())
            .expect("the furnace carries a heating setpoint source");
    let hares_types::ScheduleSourceConfig::DailyProfile {
        weekday, weekend, ..
    } = source
    else {
        panic!("the HPXML hourly setpoints must reach the furnace, got {source:?}");
    };
    let f_to_c = |f: f64| (f - 32.0) * 5.0 / 9.0;
    assert!((weekday[0] - f_to_c(64.0)).abs() < 1e-9, "{weekday:?}");
    assert!((weekday[7] - f_to_c(70.0)).abs() < 1e-9, "{weekday:?}");
    assert!((weekend[0] - f_to_c(68.0)).abs() < 1e-9, "{weekend:?}");
}
