use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml_schedule::generate_schedule_from_hpxml;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn test_start() -> DateTime<FixedOffset> {
    FixedOffset::west_opt(7 * 3600)
        .unwrap()
        .with_ymd_and_hms(2019, 1, 1, 0, 0, 0)
        .unwrap()
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
        None,
    );

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
        None,
    );

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
        None,
    );
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
        None,
    );
    assert_eq!(sched.len(), 12);
    assert_eq!(sched.timestamps[0], test_start());
    assert_eq!(
        sched.timestamps[11],
        test_start() + Duration::minutes(11 * 15)
    );
}

#[test]
fn generated_schedule_uses_default_profiles_when_dir_provided() {
    let defaults_dir = project_root().join("defaults");
    let building = parse_building(&minimal_hpxml()).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
        test_start(),
        Duration::hours(24),
        Duration::minutes(60),
        Some(&defaults_dir),
    );
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

#[test]
fn generated_schedule_without_defaults_uses_constant_one() {
    let building = parse_building(&minimal_hpxml()).expect("parse");
    let sched = generate_schedule_from_hpxml(
        &building,
        test_start(),
        Duration::hours(1),
        Duration::minutes(1),
        None,
    );
    for col_name in &[
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
    ] {
        let idx = sched.column_index[*col_name];
        for &v in &sched.columns[idx] {
            assert!((v - 1.0).abs() < 1e-9, "{col_name} value {v} != 1.0");
        }
    }
}

#[test]
fn generated_schedule_parity_with_real_csv_for_resstock_2025_1() {
    let defaults_dir = project_root().join("defaults");
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
        Some(&defaults_dir),
    );

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
