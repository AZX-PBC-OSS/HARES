//! Wet-appliance hot water follows OpenStudio-HPXML: the daily volume comes
//! from the `calc_clothes_washer_energy_gpd` and `calc_dishwasher_energy_gpd`
//! formulas (v1.12.0, `HPXMLtoOpenStudio/resources/hotwater_appliances.rb`),
//! is drawn through the `hot_water_clothes_washer` and `hot_water_dishwasher`
//! schedule columns at the peak flow the column's daily equivalent full-load
//! hours give, and reaches the water heater as an untempered scheduled draw
//! beside the tempered fixture draw.

use std::path::PathBuf;

use chrono::TimeZone;
use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::equipment::resolve_equipment;
use hares_io::parse_hpxml;
use hares_io::parse_schedule_csv;
use hares_types::Warning;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo_defaults() -> DefaultsStore {
    DefaultsStore::load(&project_root().join("defaults")).expect("load defaults")
}

fn fixture(relative: &str) -> PathBuf {
    project_root().join(relative)
}

/// The washer or dishwasher spec of a resolved building, by equipment name.
fn wet_appliance_spec<'a>(
    specs: &'a [hares_io::EquipmentSpec],
    name: &str,
) -> &'a hares_io::EquipmentSpec {
    specs
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no {name} spec"))
}

fn spec_f64(spec: &hares_io::EquipmentSpec, key: &str) -> f64 {
    spec.parameters
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("spec {} has no {key}", spec.name))
}

/// The daily volume printed as gal/day to three decimals, the digits the
/// reference values are stated to.
fn gal_per_day_printed(daily_m3: f64) -> f64 {
    let gal = daily_m3 / 0.003_785_411_784;
    (gal * 1000.0).round() / 1000.0
}

#[test]
fn wet_appliance_daily_hot_water_matches_openstudio_hpxml() {
    let defaults = repo_defaults();

    // The parity cz4a building: operational branch, NumberofResidents 3.
    let building = parse_hpxml(&fixture(
        "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml",
    ))
    .expect("cz4a building parses");
    let specs = resolve_equipment(&building, &defaults, None, &mut Vec::new())
        .expect("cz4a equipment resolves");
    let washer = wet_appliance_spec(&specs, "Clothes Washer");
    let dishwasher = wet_appliance_spec(&specs, "Dishwasher");
    assert_eq!(
        gal_per_day_printed(spec_f64(washer, "daily_hot_water_m3")),
        2.584,
        "cz4a washer daily volume must match OpenStudio-HPXML to the printed digits"
    );
    assert_eq!(
        gal_per_day_printed(spec_f64(dishwasher, "daily_hot_water_m3")),
        2.563,
        "cz4a dishwasher daily volume must match OpenStudio-HPXML to the printed digits"
    );

    // The ResStock bldg0176227 home: operational branch, NumberofResidents 5,
    // UsageMultiplier 1.2 on both appliances.
    let building = parse_hpxml(&fixture(
        "tests/fixtures/resstock/2025.1/bldg0176227/home.xml",
    ))
    .expect("bldg0176227 home parses");
    let specs = resolve_equipment(&building, &defaults, None, &mut Vec::new())
        .expect("bldg0176227 equipment resolves");
    let washer = wet_appliance_spec(&specs, "Clothes Washer");
    let dishwasher = wet_appliance_spec(&specs, "Dishwasher");
    assert_eq!(
        gal_per_day_printed(spec_f64(washer, "daily_hot_water_m3")),
        7.448,
        "bldg0176227 washer daily volume must match OpenStudio-HPXML to the printed digits"
    );
    assert_eq!(
        gal_per_day_printed(spec_f64(dishwasher, "daily_hot_water_m3")),
        3.711,
        "bldg0176227 dishwasher daily volume must match OpenStudio-HPXML to the printed digits"
    );
}

const OCCUPANCY_XML: &str = r#"<Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingOccupancy><NumberofResidents>3</NumberofResidents></BuildingOccupancy>
        <BuildingConstruction>
          <ResidentialFacilityType>single-family detached</ResidentialFacilityType>
          <NumberofBedrooms>3</NumberofBedrooms>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <WaterHeating>
        <WaterHeatingSystem>
          <SystemIdentifier id="wh"/>
          <FuelType>electricity</FuelType>
          <WaterHeaterType>storage water heater</WaterHeaterType>
        </WaterHeatingSystem>
      </WaterHeating>
      <Appliances>
        <ClothesWasher>
          <SystemIdentifier id="cw"/>
          <RatedAnnualkWh>284.0</RatedAnnualkWh>
          <LabelUsage>6.0</LabelUsage>
          <Capacity>4.2</Capacity>
          <extension>
            <AdjustedAnnualkWh>420.0</AdjustedAnnualkWh>
            <UsageMultiplier>1.0</UsageMultiplier>
          </extension>
        </ClothesWasher>
        <Dishwasher>
          <SystemIdentifier id="dw"/>
          <RatedAnnualkWh>307.0</RatedAnnualkWh>
          <PlaceSettingCapacity>12</PlaceSettingCapacity>
          <LabelUsage>4.0</LabelUsage>
          <extension>
            <AdjustedAnnualkWh>500.0</AdjustedAnnualkWh>
            <UsageMultiplier>1.0</UsageMultiplier>
          </extension>
        </Dishwasher>
      </Appliances>
    </BuildingDetails>
  </Building>"#;

#[test]
fn adjusted_annual_kwh_is_not_adjusted_twice() {
    let defaults = repo_defaults();
    let xml = format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">{OCCUPANCY_XML}</HPXML>"#
    );
    let building = hares_io::hpxml::parse_hpxml_str(&xml).expect("parses");
    let specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");
    let washer = wet_appliance_spec(&specs, "Clothes Washer");
    let dishwasher = wet_appliance_spec(&specs, "Dishwasher");
    // `AdjustedAnnualkWh` is the adjusted annual energy: it is the resolved
    // energy, not the rated input of a second adjustment.
    assert_eq!(
        spec_f64(washer, "annual_electric_kwh"),
        420.0,
        "washer AdjustedAnnualkWh must be used as the adjusted energy without a second adjustment"
    );
    assert_eq!(
        spec_f64(dishwasher, "annual_electric_kwh"),
        500.0,
        "dishwasher AdjustedAnnualkWh must be used as the adjusted energy without a second adjustment"
    );
}

#[test]
fn unavailable_periods_parse_from_software_info() {
    let building = parse_hpxml(&fixture(
        "tests/fixtures/resstock/2025.1/bldg0527060/home.xml",
    ))
    .expect("bldg0527060 home parses");
    assert_eq!(building.unavailable_periods.len(), 1);
    let period = &building.unavailable_periods[0];
    assert_eq!(period.kind, hares_io::UnavailableKind::Vacancy);
    assert_eq!(
        (period.begin_month, period.begin_day, period.begin_hour),
        (1, 1, 0)
    );
    assert_eq!(
        (period.end_month, period.end_day, period.end_hour),
        (12, 31, 24)
    );
    assert_eq!(
        period.natural_ventilation,
        hares_io::NaturalVentilation::RegularSchedule
    );
}

#[test]
fn unavailable_period_with_unknown_column_name_is_an_error() {
    let xml = std::fs::read_to_string(fixture(
        "tests/fixtures/resstock/2025.1/bldg0527060/home.xml",
    ))
    .expect("home.xml readable");
    let xml = xml.replace(
        "<ColumnName>Vacancy</ColumnName>",
        "<ColumnName>Holiday</ColumnName>",
    );
    let err = hares_io::hpxml::parse_hpxml_str(&xml)
        .expect_err("an unknown ColumnName must fail the parse");
    let message = err.to_string();
    assert!(
        message.contains("Holiday"),
        "the error must name the unknown column, got: {message}"
    );
    for name in [
        "Vacancy",
        "Power Outage",
        "No Space Heating",
        "No Space Cooling",
    ] {
        assert!(
            message.contains(name),
            "the error must list the four period types, missing {name}: {message}"
        );
    }
}

#[test]
fn missing_appliance_power_column_is_an_error() {
    let defaults = repo_defaults();
    let building = parse_hpxml(&fixture(
        "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml",
    ))
    .expect("cz4a building parses");
    let mut specs = resolve_equipment(&building, &defaults, None, &mut Vec::new())
        .expect("cz4a equipment resolves");

    // The fixture schedule without its `clothes_washer` column: the header
    // names every column, so the rewritten file drops it from the header and
    // from every row.
    let schedule_path = fixture("tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv");
    let contents = std::fs::read_to_string(&schedule_path).expect("schedule.csv readable");
    let mut rows = contents.lines();
    let header: Vec<&str> = rows.next().expect("header").split(',').collect();
    let drop_idx = header
        .iter()
        .position(|c| c.trim() == "clothes_washer")
        .expect("the fixture schedule has a clothes_washer column");
    let mut csv = header
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != drop_idx)
        .map(|(_, c)| c.trim())
        .collect::<Vec<_>>()
        .join(",");
    for line in rows {
        let fields: Vec<&str> = line.split(',').collect();
        csv.push('\n');
        csv.push_str(
            &fields
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != drop_idx)
                .map(|(_, f)| f.trim())
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, &csv).expect("write schedule");
    let mut schedule = parse_schedule_csv(&path, &[], None, None).expect("schedule parses");
    let mut warnings: Vec<Warning> = Vec::new();
    let err = hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut warnings,
    )
    .expect_err("a schedule file without the clothes_washer power column must fail");
    let message = err.to_string();
    assert!(
        message.contains("clothes_washer"),
        "the error must name the missing column, got: {message}"
    );
}

#[test]
fn appliance_hot_water_draw_rides_the_schedule_columns_with_the_peak_flow_rule() {
    use hares_physics::water_density_kg_m3;

    let defaults = repo_defaults();
    let building = parse_hpxml(&fixture(
        "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml",
    ))
    .expect("cz4a building parses");
    let mut specs = resolve_equipment(&building, &defaults, None, &mut Vec::new())
        .expect("cz4a equipment resolves");
    let washer_daily_m3 = spec_f64(
        wet_appliance_spec(&specs, "Clothes Washer"),
        "daily_hot_water_m3",
    );
    let dw_daily_m3 = spec_f64(
        wet_appliance_spec(&specs, "Dishwasher"),
        "daily_hot_water_m3",
    );

    let mut schedule = parse_schedule_csv(
        fixture("tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv"),
        &[],
        None,
        None,
    )
    .expect("schedule parses");
    hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    // The water heater's untempered appliance draw column.
    let wh = specs
        .iter()
        .find(|s| s.name == "Heat Pump Water Heater")
        .expect("cz4a has a heat pump water heater");
    let col_idx = appliance_draw_source_col(wh);

    // The untempered draw's density: the water heater's hot supply
    // temperature, its setpoint here.
    let setpoint_c = spec_f64(wh, "setpoint_c");
    let rho = water_density_kg_m3(setpoint_c);
    let washer_mass_kg = washer_daily_m3 * rho;
    let dw_mass_kg = dw_daily_m3 * rho;

    let hwcw_idx = schedule.column_index["hot_water_clothes_washer"];
    let hwdw_idx = schedule.column_index["hot_water_dishwasher"];
    let hwcw = &schedule.columns[hwcw_idx];
    let hwdw = &schedule.columns[hwdw_idx];
    let step_s = f64::from(schedule.source_step_secs);
    let rows_per_day = 86_400.0 / step_s;
    let days = hwcw.len() as f64 / rows_per_day;
    // The columns' daily equivalent full-load hours: for hourly rows one row
    // is one hour, so the daily sum is hours per day.
    let washer_eflh = hwcw.iter().sum::<f64>() / days;
    let dw_eflh = hwdw.iter().sum::<f64>() / days;
    // Peak flow = daily volume / the column's daily equivalent full-load
    // hours (OpenStudio-HPXML v1.12.0 schedules.rb:1353-1365), and the draw
    // at each row is the peak flow times the row's fraction.
    let washer_peak_kg_s = washer_mass_kg / (washer_eflh * 3600.0);
    let dw_peak_kg_s = dw_mass_kg / (dw_eflh * 3600.0);

    let derived = &schedule.columns[col_idx];
    assert_eq!(derived.len(), hwcw.len());
    for i in 0..derived.len() {
        let expected = hwcw[i] * washer_peak_kg_s + hwdw[i] * dw_peak_kg_s;
        assert!(
            (derived[i] - expected).abs() <= 1e-12 + 1e-9 * expected.abs(),
            "appliance draw column row {i}: got {}, expected the peak flow times the column fractions {expected}",
            derived[i]
        );
    }
}

const COMBI_BOILER_XML: &str = r#"<Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingOccupancy><NumberofResidents>3</NumberofResidents></BuildingOccupancy>
        <BuildingConstruction>
          <ResidentialFacilityType>single-family detached</ResidentialFacilityType>
          <NumberofBedrooms>3</NumberofBedrooms>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <WaterHeating>
        <WaterHeatingSystem>
          <SystemIdentifier id="wh"/>
          <FuelType>natural gas</FuelType>
          <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
          <HotWaterTemperature units="F">120</HotWaterTemperature>
          <TankVolume>40</TankVolume>
        </WaterHeatingSystem>
      </WaterHeating>
      <Appliances>
        <ClothesWasher>
          <SystemIdentifier id="cw"/>
          <RatedAnnualkWh>284.0</RatedAnnualkWh>
          <LabelElectricRate>0.12</LabelElectricRate>
          <LabelGasRate>1.09</LabelGasRate>
          <LabelAnnualGasCost>27.0</LabelAnnualGasCost>
          <LabelUsage>6.0</LabelUsage>
          <Capacity>4.2</Capacity>
        </ClothesWasher>
        <Dishwasher>
          <SystemIdentifier id="dw"/>
          <RatedAnnualkWh>307.0</RatedAnnualkWh>
          <LabelElectricRate>0.12</LabelElectricRate>
          <LabelGasRate>1.09</LabelGasRate>
          <LabelAnnualGasCost>33.12</LabelAnnualGasCost>
          <PlaceSettingCapacity>12</PlaceSettingCapacity>
          <LabelUsage>4.0</LabelUsage>
        </Dishwasher>
      </Appliances>
    </BuildingDetails>
  </Building>"#;

/// The combi-boiler home's Indirect Tank is a water heating system class
/// like any other: the wet appliances' draw reaches its tank through the
/// same normalized schedule column, at every row the schedule has.
#[test]
fn combi_boiler_indirect_tank_receives_the_appliance_draw() {
    use hares_physics::water_density_kg_m3;

    let defaults = repo_defaults();
    let xml = format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">{COMBI_BOILER_XML}</HPXML>"#
    );
    let building = hares_io::hpxml::parse_hpxml_str(&xml).expect("parses");
    let mut specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");
    let washer_daily_m3 = spec_f64(
        wet_appliance_spec(&specs, "Clothes Washer"),
        "daily_hot_water_m3",
    );
    let dw_daily_m3 = spec_f64(
        wet_appliance_spec(&specs, "Dishwasher"),
        "daily_hot_water_m3",
    );
    assert!(
        washer_daily_m3 > 0.0 && dw_daily_m3 > 0.0,
        "the home's wet appliances have hot-water volumes"
    );

    let mut schedule = generated_schedule_7d();
    hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    let wh = specs
        .iter()
        .find(|s| s.name == "Indirect Tank")
        .expect("the combi boiler resolves to an Indirect Tank");

    // The appliance draw column rides the wet appliances' schedule columns
    // at the peak flow the columns' daily equivalent full-load hours give,
    // in mass at the drawn water's temperature: the tank's hot supply.
    let col_idx = appliance_draw_source_col(wh);
    let setpoint_c = spec_f64(wh, "setpoint_c");
    let rho = water_density_kg_m3(setpoint_c);
    let washer_mass_kg = washer_daily_m3 * rho;
    let dw_mass_kg = dw_daily_m3 * rho;

    let hwcw_idx = schedule.column_index["hot_water_clothes_washer"];
    let hwdw_idx = schedule.column_index["hot_water_dishwasher"];
    let hwcw = &schedule.columns[hwcw_idx];
    let hwdw = &schedule.columns[hwdw_idx];
    let step_s = f64::from(schedule.source_step_secs);
    let rows_per_day = 86_400.0 / step_s;
    let days = hwcw.len() as f64 / rows_per_day;
    let washer_eflh = hwcw.iter().sum::<f64>() / days;
    let dw_eflh = hwdw.iter().sum::<f64>() / days;
    let washer_peak_kg_s = washer_mass_kg / (washer_eflh * 3600.0);
    let dw_peak_kg_s = dw_mass_kg / (dw_eflh * 3600.0);

    let derived = &schedule.columns[col_idx];
    assert_eq!(derived.len(), hwcw.len());
    for i in 0..derived.len() {
        let expected = hwcw[i] * washer_peak_kg_s + hwdw[i] * dw_peak_kg_s;
        assert!(
            (derived[i] - expected).abs() <= 1e-12 + 1e-9 * expected.abs(),
            "the indirect tank's appliance draw column row {i}: got {}, \
             expected the peak flow times the column fractions {expected}",
            derived[i]
        );
    }
}

fn appliance_draw_source_col(wh: &hares_io::EquipmentSpec) -> usize {
    let typed = wh.typed_config.as_ref().expect("WH has a typed config");
    let payload = match &typed.payload {
        hares_equipment::ConfigPayload::Typed { data, .. } => data,
        other => panic!("expected typed payload, got {other:?}"),
    };
    let source = payload
        .get("appliance_draw_flow_rate_source")
        .expect("the water heater carries an appliance draw source");
    let obj = source.as_object().expect("source is an object");
    let idx = obj
        .get("col_idx")
        .and_then(serde_json::Value::as_u64)
        .expect("source carries col_idx");
    idx as usize
}

#[test]
fn generated_schedule_produces_appliance_hot_water_columns_for_a_home_without_a_schedule_file() {
    let defaults = repo_defaults();
    // The fixture with residents, so the volumes are positive and the columns
    // must be produced for the generated schedule.
    let occupied = occupied_bldg0527060();
    let building = hares_io::hpxml::parse_hpxml_str(&occupied).expect("parses");
    let mut specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");
    assert!(
        spec_f64(
            wet_appliance_spec(&specs, "Clothes Washer"),
            "daily_hot_water_m3"
        ) > 0.0
    );

    let mut schedule = generated_schedule_7d();
    hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    let wh = specs
        .iter()
        .find(|s| s.name.contains("Water Heater"))
        .expect("the home has a water heater");
    let col_idx = appliance_draw_source_col(wh);
    let hwcw_idx = schedule.column_index["hot_water_clothes_washer"];
    let hwdw_idx = schedule.column_index["hot_water_dishwasher"];
    assert!(
        schedule.columns[col_idx].iter().any(|v| *v > 0.0),
        "the generated appliance draw column must carry the appliance draw"
    );
    assert_eq!(
        schedule.columns[col_idx].len(),
        schedule.columns[hwcw_idx].len()
    );
    assert_eq!(
        schedule.columns[col_idx].len(),
        schedule.columns[hwdw_idx].len()
    );
}

#[test]
fn vacant_home_generated_appliance_hot_water_columns_are_zero() {
    let defaults = repo_defaults();
    // The fixture's own all-year vacancy period, applied to a home with
    // residents, so the zeroing is what the test observes (the fixture
    // itself has no residents, whose absence already zeroes the volumes).
    let occupied = occupied_bldg0527060();
    let building = hares_io::hpxml::parse_hpxml_str(&occupied).expect("parses");
    assert_eq!(building.unavailable_periods.len(), 1);
    let mut specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");

    let mut schedule = generated_schedule_7d();
    hares_io::apply_unavailable_periods(&mut schedule, &building.unavailable_periods)
        .expect("periods apply");
    hares_io::inject_schedule_into_specs(
        &mut specs,
        &mut schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    for column in [
        "clothes_washer",
        "dishwasher",
        "hot_water_clothes_washer",
        "hot_water_dishwasher",
        "hot_water_fixtures",
    ] {
        let idx = schedule.column_index[column];
        assert!(
            schedule.columns[idx].iter().all(|v| *v == 0.0),
            "the vacancy period must zero {column} on every step"
        );
    }
    // The water heater's appliance draw column is the normalization of the
    // zeroed fractions: zero on every step too.
    let wh = specs
        .iter()
        .find(|s| s.name.contains("Water Heater"))
        .expect("the home has a water heater");
    let col_idx = appliance_draw_source_col(wh);
    assert!(
        schedule.columns[col_idx].iter().all(|v| *v == 0.0),
        "an all-year vacancy draws no appliance hot water"
    );
}

#[test]
fn without_a_period_the_hot_water_columns_keep_their_values() {
    let mut schedule = synthetic_schedule_8760_rows();
    hares_io::apply_unavailable_periods(&mut schedule, &[]).expect("no periods is a no-op");
    let idx = schedule.column_index["hot_water_clothes_washer"];
    assert!(schedule.columns[idx].iter().all(|v| *v == 0.5));
}

#[test]
fn unavailable_period_zeroes_the_paired_hot_water_columns() {
    // A synthetic two-day schedule and an all-year vacancy: every mapped
    // column zeros, the hot-water columns through the paired energy-use
    // column's table row, like OpenStudio-HPXML's set_unavailable_periods.
    let mut schedule = synthetic_schedule_8760_rows();
    let periods = vec![hares_io::UnavailablePeriod {
        kind: hares_io::UnavailableKind::Vacancy,
        begin_month: 1,
        begin_day: 1,
        begin_hour: 0,
        end_month: 12,
        end_day: 31,
        end_hour: 24,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    }];
    hares_io::apply_unavailable_periods(&mut schedule, &periods).expect("periods apply");
    for column in [
        "occupants",
        "clothes_washer",
        "dishwasher",
        "hot_water_clothes_washer",
        "hot_water_dishwasher",
        "hot_water_fixtures",
    ] {
        let idx = schedule.column_index[column];
        assert!(
            schedule.columns[idx].iter().all(|v| *v == 0.0),
            "vacancy must zero {column}"
        );
    }
}

#[test]
fn period_flags_are_half_open_and_wrap_the_year_end() {
    // One hourly row per hour of a non-leap year, a period 12/31 12h to
    // 1/1 12h: the last 48 and first 48 rows flag, nothing else.
    let dir = tempfile::tempdir().expect("temp dir");
    let mut csv = String::from("clothes_washer\n");
    for _ in 0..8760 {
        csv.push_str("0.5\n");
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, &csv).expect("write schedule");
    let mut schedule = parse_schedule_csv(&path, &[], None, None).expect("schedule parses");
    assert_eq!(schedule.len(), 8760);

    let periods = vec![hares_io::UnavailablePeriod {
        kind: hares_io::UnavailableKind::PowerOutage,
        begin_month: 12,
        begin_day: 31,
        begin_hour: 12,
        end_month: 1,
        end_day: 1,
        end_hour: 12,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    }];
    hares_io::apply_unavailable_periods(&mut schedule, &periods).expect("periods apply");
    let idx = schedule.column_index["clothes_washer"];
    let column = &schedule.columns[idx];
    assert!(column.iter().all(|v| *v == 0.5 || *v == 0.0));
    // Hourly rows: 12/31 12:00 to 1/1 12:00 is 24 flagged rows.
    assert_eq!(column.iter().filter(|v| **v == 0.0).count(), 24);
    // The wrapped window: rows 8748 to 8759 (12/31 12:00 to 23:00) and rows
    // 0 to 11 (1/1 00:00 to 11:00).
    for (i, v) in column.iter().enumerate() {
        let in_window = (8748..8760).contains(&i) || i < 12;
        assert_eq!(
            *v == 0.0,
            in_window,
            "row {i} flagged state must match the half-open wrapped window"
        );
    }
}

/// The period dates live on a non-leap calendar, so a leap-year schedule's
/// rows must flag by their 365-day-calendar day: Mar 1 flags on Mar 1 (not
/// on Feb 29's slot placement before it), and Dec 31 exists at all (its raw
/// leap-year ordinal 366 is past the 365-day calendar's last day).
#[test]
fn leap_year_schedule_flags_mar_1_and_dec_31_on_their_own_days() {
    // Hourly rows for five days of the 2024 leap year.
    let days = [(2, 28), (2, 29), (3, 1), (12, 30), (12, 31)];
    let mut timestamps = Vec::new();
    for (month, day) in days {
        for hour in 0..24 {
            timestamps.push(
                chrono::FixedOffset::east_opt(0)
                    .expect("UTC offset")
                    .with_ymd_and_hms(2024, month, day, hour, 0, 0)
                    .single()
                    .expect("valid timestamp"),
            );
        }
    }
    let n = timestamps.len();
    let mut schedule = hares_io::ScheduleTimeSeries {
        timestamps,
        column_names: vec!["hot_water_fixtures".to_string()],
        columns: vec![vec![0.5; n]],
        column_index: std::iter::once(("hot_water_fixtures".to_string(), 0)).collect(),
        source_step_secs: 3600,
        column_aggregations: vec![hares_io::ColumnAggregation::Mean],
    };

    let periods = vec![
        hares_io::UnavailablePeriod {
            kind: hares_io::UnavailableKind::Vacancy,
            begin_month: 3,
            begin_day: 1,
            begin_hour: 0,
            end_month: 3,
            end_day: 1,
            end_hour: 24,
            natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
        },
        hares_io::UnavailablePeriod {
            kind: hares_io::UnavailableKind::Vacancy,
            begin_month: 12,
            begin_day: 31,
            begin_hour: 0,
            end_month: 12,
            end_day: 31,
            end_hour: 24,
            natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
        },
    ];
    hares_io::apply_unavailable_periods(&mut schedule, &periods).expect("periods apply");

    let column = &schedule.columns[0];
    for (slot, &(month, day)) in days.iter().enumerate() {
        let day_rows = &column[slot * 24..(slot + 1) * 24];
        let zeroed = day_rows.iter().all(|v| *v == 0.0);
        match (month, day) {
            // The period's own days zero exactly.
            (3, 1) | (12, 31) => assert!(
                zeroed,
                "the period must zero every hour of {month}/{day} (2024)"
            ),
            // Neighbouring days keep their values. Feb 29 shares Mar 1's
            // schedule slot (the 365-day row lookup maps both to the same
            // row), so its rows flag with Mar 1's.
            (2, 28) | (12, 30) => assert!(
                !zeroed,
                "the period must leave every hour of {month}/{day} alone"
            ),
            (2, 29) => assert!(
                zeroed,
                "Feb 29 shares Mar 1's 365-day schedule slot, so a Mar 1 period flags it too"
            ),
            _ => unreachable!(),
        }
    }
}

/// A half-year vacancy must REMOVE the volume the period's days carried,
/// not spread it over the active days: OpenStudio-HPXML computes every
/// peak from the pre-period schedule (SchedulesFile deep-copies
/// `@schedules` and `set_unavailable_periods` edits only the copy the
/// model reads, schedules.rb:1051-1052), so the active days' daily volume
/// stays exactly what the no-period run drew, at every step's shape.
#[test]
fn half_year_vacancy_removes_volume_instead_of_redistributing_it() {
    let defaults = repo_defaults();
    let building = hares_io::hpxml::parse_hpxml_str(&occupied_bldg0527060()).expect("parses");
    let mut specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");
    assert!(
        spec_f64(
            wet_appliance_spec(&specs, "Clothes Washer"),
            "daily_hot_water_m3"
        ) > 0.0
    );

    let periods = vec![hares_io::UnavailablePeriod {
        kind: hares_io::UnavailableKind::Vacancy,
        begin_month: 1,
        begin_day: 1,
        begin_hour: 0,
        end_month: 6,
        end_day: 30,
        end_hour: 24,
        natural_ventilation: hares_io::NaturalVentilation::RegularSchedule,
    }];

    // The no-period run sets the daily volume the active days must keep.
    let mut baseline_schedule = synthetic_schedule_8760_rows();
    let mut baseline_specs =
        resolve_equipment(&building, &defaults, None, &mut Vec::new()).expect("equipment resolves");
    hares_io::inject_schedule_into_specs(
        &mut baseline_specs,
        &mut baseline_schedule,
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("baseline injection succeeds");

    let mut schedule = synthetic_schedule_8760_rows();
    let pre_period_columns =
        hares_io::apply_unavailable_periods(&mut schedule, &periods).expect("periods apply");
    hares_io::inject_schedule_into_specs_with_pre_period(
        &mut specs,
        &mut schedule,
        pre_period_columns.as_deref(),
        Some(&project_root().join("defaults")),
        &defaults,
        &mut Vec::new(),
    )
    .expect("injection succeeds");

    let daily_kg = |schedule: &hares_io::ScheduleTimeSeries, prefix: &str, day: usize| {
        let col_idx = schedule
            .column_names
            .iter()
            .position(|name| name.starts_with(prefix))
            .unwrap_or_else(|| panic!("a {prefix} column exists"));
        let rows = &schedule.columns[col_idx][day * 24..(day + 1) * 24];
        rows.iter().sum::<f64>() * 3600.0
    };

    for prefix in ["appliance_hot_water_draw_kg_s_", "hot_water_draw_kg_s_"] {
        // A vacancy day draws nothing.
        let vacancy_day = daily_kg(&schedule, prefix, 30);
        assert!(
            vacancy_day == 0.0,
            "{prefix}: the vacancy day must draw zero, got {vacancy_day}"
        );
        // An active day's volume is unchanged from the no-period run.
        let baseline_day = daily_kg(&baseline_schedule, prefix, 300);
        let active_day = daily_kg(&schedule, prefix, 300);
        assert!(
            (active_day - baseline_day).abs() <= 1e-9 * baseline_day.abs(),
            "{prefix}: the active day's volume must be unchanged (no-period run {baseline_day} kg, \
             vacancy run {active_day} kg): the vacancy removes volume, it does not spread it"
        );
    }
}

fn occupied_bldg0527060() -> String {
    let xml = std::fs::read_to_string(fixture(
        "tests/fixtures/resstock/2025.1/bldg0527060/home.xml",
    ))
    .expect("home.xml readable");
    xml.replace(
        "<NumberofResidents>0.0</NumberofResidents>",
        "<NumberofResidents>3.0</NumberofResidents>",
    )
}

fn generated_schedule_7d() -> hares_io::ScheduleTimeSeries {
    let start = chrono::DateTime::parse_from_rfc3339("2018-01-01T00:00:00-05:00").expect("start");
    let profiles =
        hares_io::load_default_profiles(&project_root().join("defaults")).expect("profiles load");
    hares_io::hpxml_schedule::generate_default_schedule(
        start,
        chrono::Duration::seconds(86_400 * 7),
        chrono::Duration::seconds(3600),
        &profiles,
    )
    .expect("schedule generates")
}

fn synthetic_schedule_8760_rows() -> hares_io::ScheduleTimeSeries {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut csv = String::from(
        "occupants,clothes_washer,dishwasher,clothes_dryer,cooking_range,refrigerator,plug_loads_other,plug_loads_tv,ceiling_fan,lighting_interior,hot_water_clothes_washer,hot_water_dishwasher,hot_water_fixtures\n",
    );
    // A full year of hourly rows: index-based schedules are validated as
    // annual data.
    for _ in 0..8760 {
        csv.push_str("1.0,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5\n");
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, &csv).expect("write schedule");
    parse_schedule_csv(&path, &[], None, None).expect("schedule parses")
}
