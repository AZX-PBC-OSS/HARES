//! The skylight's interior zone follows the OS-HPXML skylight rule: the
//! skylight lives in the conditioned space (geometry.rb apply_skylights
//! seats its parent surface unconditionally in the conditioned space,
//! :687-688) and fails when its parent roof is unconditioned (:639-641).
//! The schema names no AttachedToWall and no adjacency child on a Skylight,
//! so a schema-conformant skylight reaches the build through its
//! AttachedToRoof reference alone.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Duration;
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{OutputFormat, SimulationConfig};

#[path = "../../../tests/support/fixture_start.rs"]
mod fixture_start;

fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).expect("failed to write input file");
    path
}

fn build_minimal_epw() -> String {
    let mut lines = vec![
        "LOCATION,Test Site,CO,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
        "COMMENTS 1,synthetic".to_string(),
        "COMMENTS 2,synthetic".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = chrono::NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + Duration::hours(i as i64);
        use chrono::{Datelike, Timelike};
        let row = [
            timestamp.year().to_string(),
            timestamp.month().to_string(),
            timestamp.day().to_string(),
            (timestamp.hour() + 1).to_string(),
            "0".to_string(),
            "A0A0A0A0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0*0".to_string(),
            "20.0".to_string(),
            "10.0".to_string(),
            "50".to_string(),
            "101325".to_string(),
            "0".to_string(),
            "0".to_string(),
            "300".to_string(),
            "100".to_string(),
            "200".to_string(),
            "50".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "180".to_string(),
            "3.5".to_string(),
            "4".to_string(),
            "4".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ];
        lines.push(row.join(","));
    }
    lines.join("\n")
}

fn build_minimal_schedule() -> String {
    let start = fixture_start::fixture_start();
    let mut lines = vec!["Time,Dummy".to_string()];
    for i in 0..60 {
        let ts = start + Duration::minutes(i as i64);
        lines.push(format!("{},0.0", ts.format("%Y-%m-%dT%H:%M:%S-07:00")));
    }
    lines.join("\n")
}

/// A single-story home whose roof is a cathedral ceiling (the roof's
/// interior adjacency is the conditioned space), with one skylight on it.
fn cathedral_skylight_hpxml(skylight_extra: &str, roof_interior: &str) -> String {
    format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.99</Longitude>
        </Site>
        <BuildingConstruction>
          <NumberofBedrooms>3</NumberofBedrooms>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <AirInfiltrationMeasurement>
          <BuildingAirLeakage>
            <UnitofMeasure>ACH</UnitofMeasure>
            <AirLeakage>0</AirLeakage>
          </BuildingAirLeakage>
        </AirInfiltrationMeasurement>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">1200.0</Area>
            <Azimuth>180</Azimuth>
            <WallType><WoodStud/></WallType>
            <Insulation>
              <AssemblyEffectiveRValue>13.0</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>{roof_interior}</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">2000.0</Area>
            <RoofType>asphalt or fiberglass shingles</RoofType>
            <Pitch>6</Pitch>
            <Insulation>
              <AssemblyEffectiveRValue>30.0</AssemblyEffectiveRValue>
            </Insulation>
          </Roof>
        </Roofs>
        <Skylights>
          <Skylight>
            <SystemIdentifier id="Skylight1"/>
            <Area units="ft2">15.0</Area>
            <Azimuth>180</Azimuth>
            <UFactor units="Btu/(h-ft2-F)">0.35</UFactor>
            <SHGC>0.40</SHGC>
            {skylight_extra}
          </Skylight>
        </Skylights>
      </Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.80</Value>
            </AnnualHeatingEfficiency>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
    )
}

fn dwelling_config(hpxml_xml: &str) -> (DwellingConfig, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = write_file(dir.path(), "home.xml", hpxml_xml);
    let schedule_path = write_file(dir.path(), "schedule.csv", &build_minimal_schedule());
    let weather_path = write_file(dir.path(), "weather.epw", &build_minimal_epw());
    let output_path = dir.path().join("output.csv");

    let config = DwellingConfig {
        hpxml_path,
        schedule_path: Some(schedule_path),
        weather_path,
        defaults_path: Some(hares_core::shipped_defaults_dir()),
        sim_config: SimulationConfig {
            start_time: fixture_start::fixture_start(),
            duration: Duration::hours(1),
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(output_path),
            write_output: false,
            output_format: OutputFormat::Csv,
            output_chunk_size: 128,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 7,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    (config, dir)
}

/// A schema-conformant skylight (AttachedToRoof, the cathedral roof's
/// interior adjacency is the conditioned space) builds and its interior
/// zone is the conditioned space. Before the skylight rule, the parse read
/// only AttachedToWall and no adjacency child, so the skylight reached the
/// build with no interior zone and failed it.
#[test]
fn a_cathedral_roof_skylight_builds_and_seats_in_the_conditioned_space() {
    let hpxml = cathedral_skylight_hpxml("<AttachedToRoof idref=\"Roof1\"/>", "living space");
    let (config, _dir) = dwelling_config(&hpxml);
    let mut dwelling =
        Dwelling::from_config(config).expect("the dwelling builds with the skylight seated");
    let step = dwelling
        .step()
        .expect("the first step runs with the skylight seated in the conditioned space");
    assert!(step.net_electric_power_kw.is_finite());
}

/// A skylight whose parent roof's interior adjacency is not conditioned
/// fails naming the rule (geometry.rb apply_skylights :639-641: "Skylight
/// '...' not connected to conditioned space; if it's a skylight with a
/// shaft, use AttachedToFloor to connect it to conditioned space.").
#[test]
fn an_unconditioned_roof_skylight_fails_naming_the_rule() {
    let hpxml = cathedral_skylight_hpxml("<AttachedToRoof idref=\"Roof1\"/>", "attic - unvented");
    let (config, _dir) = dwelling_config(&hpxml);
    let err = Dwelling::from_config(config)
        .err()
        .expect("a skylight on an unconditioned roof must fail the build");
    let shown = err.to_string();
    assert!(
        shown.contains("Skylight1") && shown.contains("not connected to conditioned space"),
        "the error names the skylight and the OS-HPXML rule: {shown}"
    );
}
