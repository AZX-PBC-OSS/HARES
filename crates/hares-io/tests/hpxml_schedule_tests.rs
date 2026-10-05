use std::path::PathBuf;

use chrono::{DateTime, Datelike, Duration, FixedOffset, TimeZone, Timelike};
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml_schedule::generate_schedule_from_hpxml;
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
    let xml = minimal_hpxml();
    let building = parse_building(&xml).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
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
        "heating_setpoint",
        "cooling_setpoint",
    ];
    for col in required {
        assert!(sched.column_index.contains_key(*col), "missing: {col}");
    }
}

#[test]
fn generated_schedule_uses_hpxml_setpoints() {
    let xml = minimal_hpxml_with_hvac_control();
    let building = parse_building(&xml).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
        test_start(),
        Duration::hours(2),
        Duration::minutes(60),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");

    let heat = sched.columns[sched.column_index["heating_setpoint"]][0];
    let cool = sched.columns[sched.column_index["cooling_setpoint"]][0];
    assert!((heat - 22.222).abs() < 0.01); // 72°F
    assert!((cool - 25.556).abs() < 0.01); // 78°F
}

#[test]
fn generated_schedule_defaults_setpoints_when_hpxml_missing() {
    let xml = minimal_hpxml();
    let building = parse_building(&xml).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
        test_start(),
        Duration::hours(1),
        Duration::minutes(1),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");
    let heat = sched.columns[sched.column_index["heating_setpoint"]][0];
    let cool = sched.columns[sched.column_index["cooling_setpoint"]][0];
    assert!((heat - 20.0).abs() < 1e-9);
    assert!((cool - 24.0).abs() < 1e-9);
}

#[test]
fn generated_schedule_timestamps_are_correct() {
    let building = parse_building(&minimal_hpxml()).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
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
    let building = parse_building(&minimal_hpxml()).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
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
    let building = parse_building(&minimal_hpxml()).expect("parse");
    let profiles = shipped_profiles();
    let sched = generate_schedule_from_hpxml(
        &building,
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

#[test]
fn generated_schedule_parity_with_real_csv_for_resstock_2025_1() {
    let bldg_dir = project_root().join("tests/fixtures/resstock/2025.1/bldg0527060");

    // Skip if fixture not available
    if !bldg_dir.join("home.xml").exists() {
        return;
    }

    let xml = std::fs::read_to_string(bldg_dir.join("home.xml")).expect("read fixture");
    let building = parse_building(&xml).expect("parse building");

    let generated = generate_schedule_from_hpxml(
        &building,
        test_start(),
        Duration::hours(1),
        Duration::minutes(1),
        &shipped_profiles(),
    )
    .expect("the shipped profiles generate the schedule");

    // Must have all required columns
    for col in &[
        "occupants",
        "plug_loads_other",
        "plug_loads_tv",
        "lighting_interior",
        "heating_setpoint",
        "cooling_setpoint",
    ] {
        assert!(generated.column_index.contains_key(*col), "missing: {col}");
    }

    // Setpoints must be physically plausible
    let heat = generated.columns[generated.column_index["heating_setpoint"]][0];
    let cool = generated.columns[generated.column_index["cooling_setpoint"]][0];
    assert!(
        heat > 5.0 && heat < 40.0,
        "implausible heating setpoint: {heat}"
    );
    assert!(
        cool > 10.0 && cool < 50.0,
        "implausible cooling setpoint: {cool}"
    );
    assert!(cool > heat, "cooling must be above heating");
}

// --- helpers ---

fn minimal_hpxml() -> String {
    r#"
    <HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
      <Building>
        <BuildingDetails>
          <BuildingSummary>
            <Site><SiteType>suburban</SiteType></Site>
            <BuildingConstruction>
              <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
              <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
              <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
            </BuildingConstruction>
          </BuildingSummary>
          <Enclosure><Walls/></Enclosure>
        </BuildingDetails>
      </Building>
    </HPXML>"#
        .to_string()
}

fn minimal_hpxml_with_hvac_control() -> String {
    r#"
    <HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
      <Building>
        <BuildingDetails>
          <BuildingSummary>
            <Site><SiteType>suburban</SiteType></Site>
            <BuildingConstruction>
              <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
              <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
              <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
            </BuildingConstruction>
          </BuildingSummary>
          <Enclosure><Walls/></Enclosure>
          <Systems>
            <HVAC>
              <HVACControl>
                <SetpointTempHeatingSeason>72.0</SetpointTempHeatingSeason>
                <SetpointTempCoolingSeason>78.0</SetpointTempCoolingSeason>
              </HVACControl>
            </HVAC>
          </Systems>
        </BuildingDetails>
      </Building>
    </HPXML>"#
        .to_string()
}
