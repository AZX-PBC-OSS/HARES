use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate, Timelike};
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

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + Duration::hours(i as i64);
        let year = timestamp.year();
        let month = timestamp.month();
        let day = timestamp.day();
        let hour = timestamp.hour() + 1;

        let row = [
            year.to_string(),
            month.to_string(),
            day.to_string(),
            hour.to_string(),
            "0".to_string(),
            "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
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

fn furnace_without_heating_capacity_hpxml() -> &'static str {
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
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
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
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">2000.0</Area>
            <RoofType>asphalt or fiberglass shingles</RoofType>
            <Pitch>4</Pitch>
            <Insulation>
              <AssemblyEffectiveRValue>30.0</AssemblyEffectiveRValue>
            </Insulation>
          </Roof>
        </Roofs>
        <Windows>
          <Window>
            <SystemIdentifier id="Win1"/>
            <Area units="ft2">40.0</Area>
            <Azimuth>180</Azimuth>
            <UFactor units="Btu/(h-ft2-F)">0.30</UFactor>
            <SHGC>0.40</SHGC>
            <InteriorShading>
              <SummerShadingCoefficient>0.70</SummerShadingCoefficient>
              <WinterShadingCoefficient>0.85</WinterShadingCoefficient>
            </InteriorShading>
            <AttachedToWall idref="Wall1"/>
          </Window>
        </Windows>
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
}

fn build_minimal_dwelling(hpxml_xml: &str) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = write_file(dir.path(), "home.xml", hpxml_xml);
    let schedule_path = write_file(dir.path(), "schedule.csv", &build_minimal_schedule());
    let weather_path = write_file(dir.path(), "weather.epw", &build_minimal_epw());
    let output_path = dir.path().join("output.csv");

    let start_time = fixture_start::fixture_start();

    let config = DwellingConfig {
        hpxml_path,
        schedule_path: Some(schedule_path),
        weather_path,
        defaults_path: None,
        sim_config: SimulationConfig {
            start_time,
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
        },
        overrides: None,
        bldg_id: 42,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    Dwelling::from_config(config).expect(
        "Dwelling::from_config must succeed when HPXML omits HeatingCapacity \
         and autosizing computes the required capacity from the building envelope",
    )
}

#[test]
fn from_config_autosizing_populates_heating_capacity() {
    let hpxml = furnace_without_heating_capacity_hpxml();
    let mut dwelling = build_minimal_dwelling(hpxml);

    let equipment_names: Vec<&str> = dwelling
        .equipment()
        .iter()
        .map(|e| e.descriptor().name.as_str())
        .collect();

    assert!(
        equipment_names.contains(&"Gas Furnace"),
        "Dwelling equipment should include Gas Furnace; got: {equipment_names:?}"
    );

    // Autosizing correctness is verified by construction: `from_config` succeeds
    // iff autosizing produced a non-zero heating capacity, because a zero-capacity
    // result leaves `heating_capacity_w` absent from the equipment spec, causing
    // `GasFurnace::init` → `require_typed()` to return `Err`. Since `EndUse::HVAC_HEATING`
    // is critical, `from_config` returns `Err` and the `expect()` in
    // `build_minimal_dwelling` panics. Autosizing failures emit `tracing::error!()`,
    // not `dwelling.warnings` — the construction path is the gate.

    let step_result = dwelling
        .step()
        .expect("first step must succeed after autosizing");

    assert!(
        step_result.net_electric_power_kw.is_finite(),
        "Step result net electric power should be finite"
    );
}
