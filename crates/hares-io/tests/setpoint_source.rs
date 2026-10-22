//! The thermostat setpoints resolve by the OpenStudio-HPXML rule and the
//! active source is recorded where they resolve.
//!
//! The rule (OpenStudio-HPXML `HPXMLtoOpenStudio/resources/hvac.rb`
//! `apply_setpoints`, master `57bd1b88`, lines 1641-1741): per side, a
//! schedule-file column when present ("permit mixing detailed schedules
//! with simple schedules", hvac.rb:1665), else the HPXML setpoints
//! (`get_hvac_setpoints`, hvac.rb:1768-1829: the hourly weekday and
//! weekend extension arrays when both are declared, else the constant
//! `SetpointTemp*Season` element with its setback, hvac.rb:1785-1798),
//! else the default profile (OS-HPXML's manual-thermostat defaults,
//! 68 F heating and 78 F cooling, `defaults.rb:2827-2848`
//! `apply_hvac_control`). When a column and HPXML setpoints are both
//! given, OS-HPXML warns "Both 'heating_setpoint' schedule file and
//! heating setpoint temperature provided; the latter will be ignored."
//! (hvac.rb:1669-1675); HARES warns for every ignored input, so no
//! choice is silent.
//!
//! The fixture is `tests/fixtures/parity/cz4a_ashp_hpwh`: its HPXML
//! declares hourly `Weekday/WeekendSetpointTempsHeatingSeason` arrays
//! (building.xml:794-795) and a simple `SetpointTempCoolingSeason`
//! (building.xml:780), and its schedule.csv carries no setpoint column;
//! each test appends one constant setpoint column to a copy of the file
//! and reads it the way the dwelling reads a schedule file, through the
//! parser, which reads the setpoint columns in degrees Fahrenheit and
//! converts every value to Celsius at the parse.

use std::path::PathBuf;

use hares_io::defaults::DefaultsStore;
use hares_io::{
    ColumnAggregation, EquipmentSpec, ScheduleTimeSeries, inject_schedule_into_specs, parse_hpxml,
    parse_schedule_csv, resolve_equipment,
};
use hares_physics::units::temperature_f_to_c;
use hares_types::{ScheduleSourceConfig, Warning};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn fixture() -> PathBuf {
    repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

fn repo_defaults() -> DefaultsStore {
    DefaultsStore::load(&repo_root().join("python/hares/defaults"))
        .expect("the shipped defaults load")
}

/// The fixture's HPXML resolved into specs the way the parity run resolves
/// it, before any schedule injection.
fn fixture_specs() -> Vec<EquipmentSpec> {
    let building =
        parse_hpxml(&fixture().join("building.xml")).expect("the fixture's HPXML parses");
    resolve_equipment(
        &building,
        None,
        None,
        &repo_defaults(),
        None,
        &mut Vec::new(),
    )
    .expect("the fixture's equipment resolves")
}

/// The fixture's schedule with one constant column appended, written to a
/// temporary file and read the way the dwelling reads a schedule file:
/// through the parser, where the setpoint columns' Fahrenheit reading
/// applies.
fn schedule_with_appended_column(
    dir: &tempfile::TempDir,
    name: &str,
    value: f64,
) -> ScheduleTimeSeries {
    let original = std::fs::read_to_string(fixture().join("schedule.csv"))
        .expect("the fixture's schedule reads");
    let mut out = String::with_capacity(original.len() + 32);
    let mut lines = original.lines();
    let header = lines.next().expect("the schedule has a header");
    assert!(
        !header.contains(name),
        "the fixture's schedule has no {name} column to start from"
    );
    out.push_str(&format!("{header},{name}\n"));
    for line in lines {
        if line.is_empty() {
            continue;
        }
        out.push_str(&format!("{line},{value}\n"));
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, out).expect("the appended schedule writes");
    parse_schedule_csv(&path, &[], None, None).expect("the appended schedule parses")
}

fn fixture_schedule() -> ScheduleTimeSeries {
    parse_schedule_csv(fixture().join("schedule.csv"), &[], None, None)
        .expect("the fixture's schedule reads")
}

/// The serialized `ScheduleSourceConfig` the spec's typed config carries
/// under `setpoint.<prefix>_setpoint_source`, the source the engine reads
/// its setpoints from.
fn config_setpoint_source(spec: &EquipmentSpec, prefix: &str) -> ScheduleSourceConfig {
    let data = &spec
        .typed_config
        .as_ref()
        .expect("the heat pump carries a typed config")
        .payload;
    let hares_equipment::ConfigPayload::Typed { data, .. } = data else {
        panic!("the heat pump's config payload is typed");
    };
    let source = data
        .get("setpoint")
        .and_then(|sp| sp.get(format!("{prefix}_setpoint_source")))
        .expect("the config carries the setpoint source the spec records");
    serde_json::from_value(source.clone()).expect("the setpoint source deserializes")
}

fn heat_pump(specs: &[EquipmentSpec]) -> (&EquipmentSpec, &EquipmentSpec) {
    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("the fixture's heat pump heats");
    let cooler = specs
        .iter()
        .find(|s| s.name == "ASHP Cooler")
        .expect("the fixture's heat pump cools");
    (heater, cooler)
}

/// The cz4a heating profile: 65 F for hours 0 to 5 and hour 23, 71 F for
/// hours 6 to 22, the same on weekends (building.xml:794-795).
fn expected_cz4a_heating_c() -> Vec<f64> {
    (0..24)
        .map(|h| if (6..=22).contains(&h) { 71.0 } else { 65.0 })
        .map(temperature_f_to_c)
        .collect()
}

/// The cz4a cooling profile: the constant 76 F element (building.xml:780).
fn expected_cz4a_cooling_c() -> Vec<f64> {
    vec![temperature_f_to_c(76.0); 24]
}

/// With a schedule heating column and HPXML heating setpoints both given,
/// the column wins and the ignored HPXML heating elements are named in one
/// warning: the OS-HPXML rule's record (hvac.rb:1669-1670), which HARES
/// raises for every ignored input. The column is read in degrees
/// Fahrenheit: the schedule's column holds the Celsius conversion of the
/// 70 F values, on bits, and the config reads that converted column each
/// step.
#[test]
fn schedule_column_wins_over_hpxml_and_warns() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut specs = fixture_specs();
    let mut schedule = schedule_with_appended_column(&dir, "heating_setpoint", 70.0);
    let col_idx = schedule.column_index["heating_setpoint"];
    for value in &schedule.columns[col_idx] {
        assert_eq!(
            value.to_bits(),
            temperature_f_to_c(70.0).to_bits(),
            "the heating column is read in degrees Fahrenheit, got {value}"
        );
    }
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect("the injection succeeds");

    let (heater, _cooler) = heat_pump(&specs);
    assert_eq!(
        heater.heating_setpoint_source,
        Some(hares_io::SetpointSource::ScheduleColumn {
            column: "heating_setpoint".to_string(),
        }),
        "the heating side's active source is the schedule column"
    );
    assert!(
        matches!(
            config_setpoint_source(heater, "heating"),
            ScheduleSourceConfig::ColumnRef { col_idx: idx, .. } if idx == col_idx
        ),
        "the config reads the appended column each step"
    );

    assert_eq!(
        warnings
            .iter()
            .filter(|w| w.message.contains("heating_setpoint") && w.message.contains("ignored"))
            .count(),
        1,
        "one warning names the column and the ignored HPXML heating elements, got {warnings:?}"
    );
    let warning = warnings
        .iter()
        .find(|w| w.message.contains("heating_setpoint") && w.message.contains("ignored"))
        .expect("the overlap warning is present");
    assert!(
        warning
            .message
            .contains("WeekdaySetpointTempsHeatingSeason")
            && warning
                .message
                .contains("WeekendSetpointTempsHeatingSeason"),
        "the warning names the ignored HPXML heating elements: {}",
        warning.message
    );
    assert_eq!(
        warning.source.as_ref(),
        "schedule",
        "the warning comes from the schedule source"
    );
    assert!(
        warning.step_index.is_none(),
        "the warning is a construction-time warning"
    );
}

/// The sides resolve independently: with the heating column only, cooling
/// still resolves to the HPXML setpoints and raises no warning. The
/// heating column is read in degrees Fahrenheit, on bits.
#[test]
fn sides_resolve_independently() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut specs = fixture_specs();
    let mut schedule = schedule_with_appended_column(&dir, "heating_setpoint", 70.0);
    let col_idx = schedule.column_index["heating_setpoint"];
    for value in &schedule.columns[col_idx] {
        assert_eq!(
            value.to_bits(),
            temperature_f_to_c(70.0).to_bits(),
            "the heating column is read in degrees Fahrenheit, got {value}"
        );
    }
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect("the injection succeeds");

    let (heater, cooler) = heat_pump(&specs);
    assert_eq!(
        heater.heating_setpoint_source,
        Some(hares_io::SetpointSource::ScheduleColumn {
            column: "heating_setpoint".to_string(),
        }),
    );
    assert_eq!(
        cooler.cooling_setpoint_source,
        Some(hares_io::SetpointSource::Hpxml {
            weekday_c: expected_cz4a_cooling_c(),
            weekend_c: expected_cz4a_cooling_c(),
        }),
        "the cooling side resolves to the HPXML setpoints"
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.message.contains("cooling_setpoint")),
        "the cooling side raises no warning, got {warnings:?}"
    );
}

/// Without a setpoint column the HPXML setpoints are the active source,
/// carrying the 24 hourly values the HPXML declares converted from
/// Fahrenheit, and no setpoint warning is raised.
#[test]
fn hpxml_setpoints_used_without_a_column() {
    let mut specs = fixture_specs();
    let mut schedule = fixture_schedule();
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect("the injection succeeds");

    let (heater, cooler) = heat_pump(&specs);
    let expected_heating = expected_cz4a_heating_c();
    assert_eq!(
        heater.heating_setpoint_source,
        Some(hares_io::SetpointSource::Hpxml {
            weekday_c: expected_heating.clone(),
            weekend_c: expected_heating,
        }),
        "the heating side carries the HPXML's 24 hourly values in Celsius"
    );
    assert_eq!(
        cooler.cooling_setpoint_source,
        Some(hares_io::SetpointSource::Hpxml {
            weekday_c: expected_cz4a_cooling_c(),
            weekend_c: expected_cz4a_cooling_c(),
        }),
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.message.contains("setpoint") && w.message.contains("ignored")),
        "no setpoint warning without an overlap, got {warnings:?}"
    );
    for spec in &specs {
        if spec.name == "ASHP Heater" || spec.name == "ASHP Cooler" {
            continue;
        }
        assert_eq!(
            spec.heating_setpoint_source, None,
            "a non-HVAC spec records no setpoint source"
        );
    }
}

/// A home whose HPXML declares no thermostat at all resolves both sides to
/// OS-HPXML's manual-thermostat default profile, recorded as the active
/// source (the cited 68/78 F manual-thermostat defaults,
/// `defaults.rb:2827-2848` `apply_hvac_control`).
#[test]
fn missing_setpoints_resolve_to_the_default_profile_source() {
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
          <NumberofBedrooms>3</NumberofBedrooms>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HVACPlant>
            <HeatingSystem>
              <SystemIdentifier id='Furnace'/>
              <DistributionSystem idref='AirDistributionSystem'/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>24000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.8</Value></AnnualHeatingEfficiency>
              <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
            </HeatingSystem>
            <CoolingSystem>
              <SystemIdentifier id='AC'/>
              <DistributionSystem idref='AirDistributionSystem'/>
              <CoolingSystemType>central air conditioner</CoolingSystemType>
              <CoolingSystemFuel>electricity</CoolingSystemFuel>
              <CoolingCapacity>6000</CoolingCapacity>
              <AnnualCoolingEfficiency><Units>SEER</Units><Value>13.0</Value></AnnualCoolingEfficiency>
              <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
            </CoolingSystem>
          </HVACPlant>
          <HVACDistribution>
            <SystemIdentifier id='AirDistributionSystem'/>
            <DistributionSystemType><LeakageToOutside><DuctLeakageMeasurement><Units>CFM25</Units><Value>0.0</Value></DuctLeakageMeasurement></LeakageToOutside></DistributionSystemType>
          </HVACDistribution>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;
    let building = hares_io::hpxml::building::parse_building(xml).expect("the HPXML parses");
    let mut specs = resolve_equipment(
        &building,
        None,
        None,
        &repo_defaults(),
        None,
        &mut Vec::new(),
    )
    .expect("the equipment resolves");
    let start =
        chrono::DateTime::parse_from_rfc3339("2026-03-18T00:00:00+00:00").expect("valid timestamp");
    let mut schedule = ScheduleTimeSeries {
        timestamps: vec![start],
        column_names: vec!["occupants".to_string()],
        columns: vec![vec![1.0]],
        column_index: [("occupants".to_string(), 0_usize)].into_iter().collect(),
        source_step_secs: 60,
        column_aggregations: vec![ColumnAggregation::Mean],
    };
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect("the injection succeeds");

    let heater = specs
        .iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("the synthetic home heats");
    let cooler = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("the synthetic home cools");
    let expected_heating_c = temperature_f_to_c(68.0);
    let expected_cooling_c = temperature_f_to_c(78.0);
    assert_eq!(
        heater.heating_setpoint_source,
        Some(hares_io::SetpointSource::DefaultProfile {
            profile: hares_io::SetpointSource::MANUAL_THERMOSTAT.to_string(),
        }),
    );
    assert_eq!(
        cooler.cooling_setpoint_source,
        Some(hares_io::SetpointSource::DefaultProfile {
            profile: hares_io::SetpointSource::MANUAL_THERMOSTAT.to_string(),
        }),
    );
    assert!(
        matches!(
            config_setpoint_source(heater, "heating"),
            ScheduleSourceConfig::DailyProfile { weekday, .. }
                if (weekday[0] - expected_heating_c).abs() < 1e-12
        ),
        "the default profile's value is the cited 68 F heating default"
    );
    assert!(
        matches!(
            config_setpoint_source(cooler, "cooling"),
            ScheduleSourceConfig::DailyProfile { weekday, .. }
                if (weekday[0] - expected_cooling_c).abs() < 1e-12
        ),
        "the default profile's value is the cited 78 F cooling default"
    );
    assert!(
        warnings.iter().any(|w| w.message.contains("68")),
        "the default's substitution is warned: {warnings:?}"
    );
}

/// A schedule file carrying a `water_heater_setpoint` column fails the
/// construction naming the column and stating that HARES does not apply
/// it: accepted means applied.
#[test]
fn water_heater_setpoint_column_is_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut specs = fixture_specs();
    let mut schedule = schedule_with_appended_column(&dir, "water_heater_setpoint", 50.0);
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    let error = inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect_err("the water-heater setpoint column is a construction error");
    assert!(
        error.to_string().contains("water_heater_setpoint"),
        "the error names the column: {error}"
    );
    assert!(
        error.to_string().contains("does not apply"),
        "the error states that HARES does not apply the column: {error}"
    );
}

/// The fixture's own schedule builds with no setpoint column present and no
/// water-heater column: the census behind the water-heater error.
#[test]
fn fixture_schedule_has_no_setpoint_columns() {
    let schedule = fixture_schedule();
    assert!(
        !schedule
            .column_names
            .contains(&"heating_setpoint".to_string())
            && !schedule
                .column_names
                .contains(&"cooling_setpoint".to_string())
            && !schedule
                .column_names
                .contains(&"water_heater_setpoint".to_string()),
        "the fixture's schedule carries no setpoint column"
    );
}

/// The appended column's values reach the engine at the schedule's own
/// resolution, as the parser read them: an appended heating column's 70.0
/// is a Fahrenheit value, so the column the config indexes holds the
/// Celsius conversion of 70 F, on bits.
#[test]
fn appended_column_values_are_the_injected_ones() {
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule = schedule_with_appended_column(&dir, "heating_setpoint", 70.0);
    let idx = schedule.column_index["heating_setpoint"];
    for value in &schedule.columns[idx] {
        assert_eq!(
            value.to_bits(),
            temperature_f_to_c(70.0).to_bits(),
            "the appended column is the 70 F values as the parser read them, got {value}"
        );
    }
}

/// A resolved setpoint outside the plausible band fails the construction
/// naming the column and the value: an applied heating column holding
/// 150 (F, the convention) resolves to 65.56 C, which no thermostat
/// should ever be handed, and a Celsius file read under the Fahrenheit
/// convention lands exactly there.
#[test]
fn an_implausible_setpoint_column_fails_construction_naming_the_column_and_value() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut specs = fixture_specs();
    let mut schedule = schedule_with_appended_column(&dir, "heating_setpoint", 150.0);
    let defaults_dir = repo_root().join("python/hares/defaults");
    let mut warnings: Vec<Warning> = Vec::new();
    let error = inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
        &repo_defaults(),
        &mut warnings,
    )
    .expect_err("the implausible column is a construction error");
    let message = error.to_string();
    assert!(
        message.contains("heating_setpoint"),
        "the error names the column: {message}"
    );
    assert!(
        message.contains("65.56") || message.contains("65.6"),
        "the error names the resolved value: {message}"
    );
}

/// The band's edges are inclusive: the converted 41 F (5.0 C) and 104 F
/// (40.0 C) columns build, the 40.5 F (4.72 C) and 110 F (43.33 C)
/// columns fail construction.
#[test]
fn the_setpoint_plausibility_band_is_inclusive_at_its_edges() {
    let build_with = |value: f64| {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut specs = fixture_specs();
        let mut schedule = schedule_with_appended_column(&dir, "heating_setpoint", value);
        let defaults_dir = repo_root().join("python/hares/defaults");
        let mut warnings: Vec<Warning> = Vec::new();
        (
            inject_schedule_into_specs(
                &mut specs,
                &mut schedule,
                Some(&hares_io::load_default_profiles(&defaults_dir).expect("the profiles load")),
                &repo_defaults(),
                &mut warnings,
            ),
            dir,
        )
    };
    for plausible in [41.0, 70.0, 104.0] {
        let (result, _dir) = build_with(plausible);
        result.expect("the plausible column builds");
    }
    for implausible in [40.5, 110.0] {
        let (result, _dir) = build_with(implausible);
        let error = result.expect_err("the implausible column fails construction");
        assert!(
            error.to_string().contains("heating_setpoint"),
            "the error names the column: {error}"
        );
    }
}

/// The HPXML path's setpoints validate the same way: a heating season
/// constant that resolves outside the plausible band fails the
/// construction naming the value, at the equipment resolution where the
/// profile derives -- before any column could be read.
#[test]
fn an_implausible_hpxml_setpoint_fails_construction() {
    let xml = format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
          <NumberofBedrooms>3</NumberofBedrooms>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HVACControl>
            <SetpointTempHeatingSeason>{offending_f}</SetpointTempHeatingSeason>
          </HVACControl>
          <HVACPlant>
            <HeatingSystem>
              <SystemIdentifier id='Furnace'/>
              <DistributionSystem idref='AirDistributionSystem'/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>24000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.8</Value></AnnualHeatingEfficiency>
              <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
            </HeatingSystem>
            <CoolingSystem>
              <SystemIdentifier id='AC'/>
              <DistributionSystem idref='AirDistributionSystem'/>
              <CoolingSystemType>central air conditioner</CoolingSystemType>
              <CoolingSystemFuel>electricity</CoolingSystemFuel>
              <CoolingCapacity>6000</CoolingCapacity>
              <AnnualCoolingEfficiency><Units>SEER</Units><Value>13.0</Value></AnnualCoolingEfficiency>
              <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
            </CoolingSystem>
          </HVACPlant>
          <HVACDistribution>
            <SystemIdentifier id='AirDistributionSystem'/>
            <DistributionSystemType><LeakageToOutside><DuctLeakageMeasurement><Units>CFM25</Units><Value>0.0</Value></DuctLeakageMeasurement></LeakageToOutside></DistributionSystemType>
          </HVACDistribution>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#,
        offending_f = 160.0,
    );
    let building = hares_io::hpxml::building::parse_building(&xml).expect("the HPXML parses");
    let error = resolve_equipment(
        &building,
        None,
        None,
        &repo_defaults(),
        None,
        &mut Vec::new(),
    )
    .expect_err("the implausible HPXML setpoint is a construction error");
    let message = error.to_string();
    assert!(
        message.contains("heating") && (message.contains("71.11") || message.contains("71.1")),
        "the error names the side and the resolved value: {message}"
    );
}

/// The Ideal HVAC path takes the same building setpoint profiles every
/// heating system takes (`apply_building_setpoint_profiles` hands the
/// Ideal HVAC its heating and cooling sources from the HVACControl), so
/// the plausibility band holds there too: a 160 F heating setpoint that a
/// gas-furnace home refuses at construction an IdealHVAC home refuses
/// identically, naming the side, the elements and the value. The ideal
/// solver would otherwise back-calculate whatever load holds 71.11 C and
/// run the year silently at it.
#[test]
fn an_implausible_hpxml_setpoint_fails_the_ideal_hvac_construction_too() {
    let xml = format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
          <NumberofBedrooms>3</NumberofBedrooms>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HVACControl>
            <SetpointTempHeatingSeason>{offending_f}</SetpointTempHeatingSeason>
          </HVACControl>
          <HVACPlant>
            <HeatingSystem>
              <SystemIdentifier id='Ideal'/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><IdealHVAC/></HeatingSystemType>
              <HeatingCapacity>24000</HeatingCapacity>
              <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
            </HeatingSystem>
          </HVACPlant>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#,
        offending_f = 160.0,
    );
    let building = hares_io::hpxml::building::parse_building(&xml).expect("the HPXML parses");
    let error = resolve_equipment(
        &building,
        None,
        None,
        &repo_defaults(),
        None,
        &mut Vec::new(),
    )
    .expect_err("the Ideal HVAC home's implausible setpoint is a construction error");
    let message = error.to_string();
    assert!(
        message.contains("heating") && (message.contains("71.11") || message.contains("71.1")),
        "the error names the side and the resolved value, identically to the \
         gas-furnace case: {message}"
    );
}
