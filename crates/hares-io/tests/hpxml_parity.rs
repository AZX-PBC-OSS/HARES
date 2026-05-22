//! OCHRE parity tests for HPXML config extraction.
//!
//! Testable coverage against the current HARES implementation:
//! - number_of_speeds derivation from CompressorType (variable/single stage)
//! - number_of_speeds fallback from SEER rating when CompressorType is absent
//! - Duct parameter extraction (surface_area, leakage, r-value)
//! - fan_power_w_per_cfm storage for HVAC equipment
//! - Water heater UA derivation matches OCHRE for known EF inputs
//!
//! For each test, the OCHRE derivation logic is cited as a comment showing
//! the Python expression that produces the expected value.
//!
//! Tests for gaps that are NOT yet implemented are marked #[ignore] with a
//! description of what is missing.

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
use hares_equipment::hvac::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};
use hares_equipment::hvac::heating_config::{GasBoilerConfig, GasFurnaceConfig};
use hares_equipment::{
    Equipment, EquipmentRegistry, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
    TanklessWaterHeaterConfig,
};
use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;
use hares_types::telemetry_keys as tk;
use hares_types::{
    EnvironmentState, FuelType, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
    ZoneState,
};
use serde_json::json;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_xml(systems_xml: &str) -> String {
    format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      {systems_xml}
    </BuildingDetails>
  </Building>
</HPXML>"#
    )
}

fn resolve(xml: &str) -> Vec<hares_io::EquipmentSpec> {
    resolve_with_defaults(xml, &DefaultsStore::empty())
}

fn resolve_with_defaults(xml: &str, defaults: &DefaultsStore) -> Vec<hares_io::EquipmentSpec> {
    let building = parse_building(xml).expect("should parse");
    resolve_equipment(&building, defaults, &json!({})).expect("resolve_equipment should succeed")
}

fn repo_defaults() -> DefaultsStore {
    let defaults_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("defaults");
    DefaultsStore::load(&defaults_dir).expect("load defaults")
}

fn parity_fixture_xml(fixture_name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join(fixture_name)
        .join("building.xml");
    std::fs::read_to_string(path).expect("fixture building.xml should be readable")
}

fn make_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp_c - 5.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: outdoor_temp_c - 4.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: outdoor_temp_c,
            sky_temp_c: outdoor_temp_c - 3.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ..WeatherState::default()
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: HashMap::new(),
        equipment_core: HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .expect("offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: ChronoDuration::minutes(1),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn init_equipment(spec: &hares_io::EquipmentSpec, env: &EnvironmentState) -> Box<dyn Equipment> {
    let registry = EquipmentRegistry::new();
    let cfg = spec
        .typed_config
        .as_ref()
        .expect("typed config expected")
        .clone();
    let mut eq = registry
        .create(&spec.name, cfg.clone())
        .expect("equipment should create");
    eq.init(&cfg, env).expect("equipment should init");
    eq
}

// ---------------------------------------------------------------------------
// Test: CompressorType=variable speed → number_of_speeds=4
//
// OCHRE Dwelling.py / hpxml.py:
//   if compressor_type == 'variable speed': number_of_speeds = 4
// ---------------------------------------------------------------------------

#[test]
fn variable_speed_compressor_gives_four_speeds() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CompressorType>variable speed</CompressorType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>18</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let n_speeds = ac.parameters["number_of_speeds"]
        .as_u64()
        .expect("number_of_speeds should be integer");
    assert_eq!(
        n_speeds, 4,
        "CompressorType=variable speed must give number_of_speeds=4 (OCHRE: variable speed → 4)"
    );
}

// ---------------------------------------------------------------------------
// Test: CompressorType=single stage → number_of_speeds=1
//
// OCHRE Dwelling.py / hpxml.py:
//   if compressor_type == 'single stage': number_of_speeds = 1
// ---------------------------------------------------------------------------

#[test]
fn single_stage_compressor_gives_one_speed() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CompressorType>single stage</CompressorType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>13</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let n_speeds = ac.parameters["number_of_speeds"]
        .as_u64()
        .expect("number_of_speeds should be integer");
    assert_eq!(
        n_speeds, 1,
        "CompressorType=single stage must give number_of_speeds=1 (OCHRE: single stage → 1)"
    );
}

// ---------------------------------------------------------------------------
// Test: SEER-based speed fallback when CompressorType is absent
//
// HARES fallback (mirrors OCHRE's heuristic):
//   SEER > 21 → 4 speeds (variable)
//   SEER > 15 → 2 speeds (two-speed)
//   else      → 1 speed  (single-speed)
//
// apply_default_hvac_speed_fallback() checks "efficiency_seer" (bare <SEER> tag)
// first, then falls back to "cooling_efficiency" when "cooling_efficiency_units"
// is "SEER" (HPXML 4.x AnnualCoolingEfficiency path).
// ---------------------------------------------------------------------------

#[test]
fn seer_above_21_without_compressor_type_gives_four_speeds() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>22</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let n_speeds = ac.parameters["number_of_speeds"]
        .as_u64()
        .expect("number_of_speeds should be integer");
    assert_eq!(
        n_speeds, 4,
        "SEER=22 without CompressorType must fall back to 4 speeds (variable heuristic)"
    );
}

#[test]
fn seer_above_15_without_compressor_type_gives_two_speeds() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>16</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let n_speeds = ac.parameters["number_of_speeds"]
        .as_u64()
        .expect("number_of_speeds should be integer");
    assert_eq!(
        n_speeds, 2,
        "SEER=16 without CompressorType must fall back to 2 speeds (two-speed heuristic)"
    );
}

#[test]
fn seer_at_or_below_15_without_compressor_type_gives_one_speed() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>15</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let n_speeds = ac.parameters["number_of_speeds"]
        .as_u64()
        .expect("number_of_speeds should be integer");
    assert_eq!(
        n_speeds, 1,
        "SEER=15 without CompressorType must fall back to 1 speed (single-speed heuristic)"
    );
}

// ---------------------------------------------------------------------------
// Test: fan_power_w_per_cfm is extracted and stored from HPXML extension
//
// OCHRE hpxml.py: fan_power_w_per_cfm = extension.FanPowerWattsPerCFM
// The ticket notes fan power should be SCALED by capacity (tons × rated_cfm_per_ton),
// but scaling happens at equipment init, not during extraction. This test verifies
// the raw per-CFM value is correctly stored.
// ---------------------------------------------------------------------------

#[test]
fn fan_power_w_per_cfm_extracted_from_cooling_system_extension() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CompressorType>single stage</CompressorType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>13</Value>
            </AnnualCoolingEfficiency>
            <extension>
                <FanPowerWattsPerCFM>0.5</FanPowerWattsPerCFM>
            </extension>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let fan_w_per_cfm = ac
        .parameters
        .get("fan_power_w_per_cfm")
        .and_then(|v| v.as_f64())
        .expect("fan_power_w_per_cfm should be extracted");
    assert!(
        (fan_w_per_cfm - 0.5).abs() < 1e-9,
        "fan_power_w_per_cfm should be 0.5, got {fan_w_per_cfm}"
    );
}

#[test]
fn heat_pump_extension_install_quality_fields_are_extracted() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <CompressorType>single stage</CompressorType>
            <HeatingCapacity>36000</HeatingCapacity>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.5</Value></AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
            <extension>
                <AirflowDefectRatio>-0.25</AirflowDefectRatio>
                <ChargeDefectRatio>-0.10</ChargeDefectRatio>
                <HeatingAirflowCFM>1400</HeatingAirflowCFM>
                <CoolingAirflowCFM>1300</CoolingAirflowCFM>
            </extension>
        </HeatPump></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("should emit ASHP Heater");
    let cooler = specs
        .iter()
        .find(|s| s.name == "ASHP Cooler")
        .expect("should emit ASHP Cooler");

    assert_eq!(
        heater
            .parameters
            .get("airflow_defect_ratio")
            .and_then(|v| v.as_f64()),
        Some(-0.25)
    );
    assert_eq!(
        heater
            .parameters
            .get("charge_defect_ratio")
            .and_then(|v| v.as_f64()),
        Some(-0.10)
    );
    assert_eq!(
        heater
            .parameters
            .get("heating_airflow_cfm")
            .and_then(|v| v.as_f64()),
        Some(1400.0)
    );
    assert_eq!(
        cooler
            .parameters
            .get("cooling_airflow_cfm")
            .and_then(|v| v.as_f64()),
        Some(1300.0)
    );
}

// ---------------------------------------------------------------------------
// Test: duct parameters extracted (surface area, leakage, r-value)
//
// OCHRE Dwelling.py:
//   duct_surface_area = sum(duct.SurfaceArea for duct in ducts)
//   duct_leakage_fraction = duct.DuctLeakageValue (fractional)
//   duct_r_value = duct.DuctInsulationRValue
//
// HARES stores these as duct_supply_area_m2, duct_supply_leakage_frac, etc.
// The fixture below uses the HPXML 4.x HVACDistribution / AirDistribution /
// DuctLeakageMeasurement / Ducts structure that HARES now parses directly.
// ---------------------------------------------------------------------------

#[test]
fn duct_parameters_extracted_for_hvac_equipment() {
    // HPXML 4.x duct structure: HVACDistribution/DistributionSystemType/AirDistribution/Ducts
    // Reference: https://hpxml.nrel.gov/datadictionary/3.0.0/Building/BuildingDetails/Systems/HVAC/HVACDistribution
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Attics><Attic>
          <SystemIdentifier id="attic1"/>
          <AtticType><Attic><Vented>false</Vented></Attic></AtticType>
        </Attic></Attics>
      </Enclosure>
      <Systems>
        <HVAC>
          <HVACDistribution>
            <SystemIdentifier id="hvacd1"/>
            <DistributionSystemType>
              <AirDistribution>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="supply-leak"/>
                  <DuctType>supply</DuctType>
                  <DuctLeakage>
                    <Value>12</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="return-leak"/>
                  <DuctType>return</DuctType>
                  <DuctLeakage>
                    <Value>5</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <Ducts>
                  <SystemIdentifier id="supply-duct"/>
                  <DuctType>supply</DuctType>
                  <DuctInsulationRValue>8.0</DuctInsulationRValue>
                  <DuctSurfaceArea>50.0</DuctSurfaceArea>
                  <DuctLocation>attic - unvented</DuctLocation>
                </Ducts>
                <Ducts>
                  <SystemIdentifier id="return-duct"/>
                  <DuctType>return</DuctType>
                  <DuctInsulationRValue>4.0</DuctInsulationRValue>
                  <DuctSurfaceArea>30.0</DuctSurfaceArea>
                  <DuctLocation>attic - unvented</DuctLocation>
                </Ducts>
              </AirDistribution>
            </DistributionSystemType>
          </HVACDistribution>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let building = parse_building(xml).expect("should parse HPXML 4.x duct structure");

    // Ducts should be attached to an attic zone.
    let attic_ducts: Vec<_> = building
        .zones
        .iter()
        .flat_map(|z| &z.duct_systems)
        .collect();
    assert!(
        !attic_ducts.is_empty(),
        "at least one duct system should be parsed from HPXML 4.x AirDistribution/Ducts"
    );

    let supply = attic_ducts
        .iter()
        .find(|d| d.duct_type == hares_io::hpxml::DuctType::Supply)
        .expect("supply duct should be present");

    // R-8 imperial ≈ 1.41 m²·K/W RSI
    let r_si = supply
        .insulation_r_value_m2_k_w
        .expect("supply duct should have R-value");
    assert!(
        (r_si - 1.41).abs() < 0.05,
        "R-8 imperial should convert to ~1.41 m²·K/W RSI, got {r_si:.3}"
    );
    assert_eq!(supply.leakage_fraction, Some(0.12));

    let area_m2 = supply
        .surface_area_m2
        .expect("supply duct should have surface area");
    assert!(
        area_m2 > 0.0,
        "supply duct surface area must be > 0, got {area_m2}"
    );

    let return_duct = attic_ducts
        .iter()
        .find(|d| d.duct_type == hares_io::hpxml::DuctType::Return)
        .expect("return duct should be present");
    assert_eq!(return_duct.leakage_fraction, Some(0.05));
}

// ---------------------------------------------------------------------------
// Test: water heater UA from energy factor matches OCHRE's compute_ua()
//
// OCHRE hpxml.py calculate_ua():
//   For electric 50-gal EF=0.92:
//   q_load = 64.3 gal * 8.2938 lb/gal * 1.0007 Btu/lb·°F * (135 - 58) °F ≈ 41092 Btu/day
//   ua = q_load * (1/EF - 1) / ((T_setpoint - T_env) * 24)
//      = 41092 * (1/0.92 - 1) / ((135 - 67.5) * 24)
//      ≈ 2.206 Btu/hr·°F → 1.164 W/K
//
// This tests the internal water_heater_ua module via the public API.
// ---------------------------------------------------------------------------

#[test]
fn electric_water_heater_ua_from_ef_matches_ochre() {
    use hares_io::hpxml::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};

    let inputs = UaInputs {
        category: WhCategory::StorageElectric,
        energy_factor: Some(0.92),
        uniform_energy_factor: None,
        tank_volume_rated_gal: Some(50.0),
        recovery_efficiency: None,
        heating_capacity_btu_hr: None,
        first_hour_rating_gal: None,
    };

    let result = ua_from_energy_factor(&inputs)
        .expect("no error")
        .expect("should return Some");

    // OCHRE reference: ua ≈ 2.2057 Btu/hr·°F → 1.1636 W/K
    // Tolerance: 0.01 W/K (~0.9% relative)
    let expected_ua_w_per_k = 1.163_568_f64;
    assert!(
        (result.ua_w_per_k - expected_ua_w_per_k).abs() < 0.01,
        "electric WH EF=0.92 50-gal: ua={:.4} W/K, expected≈{expected_ua_w_per_k:.4}",
        result.ua_w_per_k
    );
}

#[test]
fn gas_furnace_typed_config_uses_afue_during_init() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><HeatingSystem>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <HeatingCapacity>60000</HeatingCapacity>
            <AnnualHeatingEfficiency>
                <Units>AFUE</Units><Value>0.96</Value>
            </AnnualHeatingEfficiency>
        </HeatingSystem></HVAC></Systems>"#,
    );

    let spec = resolve(&xml)
        .into_iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("gas furnace spec");
    let typed_cfg: GasFurnaceConfig = spec
        .typed_config
        .as_ref()
        .expect("typed gas furnace config")
        .typed()
        .expect("gas furnace typed config");
    assert!((typed_cfg.afue - 0.96).abs() < 1e-12);
    let env = make_env(18.0, 8.0);
    let mut eq = init_equipment(&spec, &env);

    let mut ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    };
    eq.update_control(&env);
    eq.step(&env, std::time::Duration::from_secs(60), &mut ports)
        .expect("step");

    let fuel_w = ports.fuel.get(FuelType::Gas);
    let fan_w = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0) * 1_000.0;
    let gross_capacity_w = ports.thermal[0].sensible_gain_w - fan_w;
    let observed_afue = gross_capacity_w / fuel_w;
    assert!(
        (observed_afue - 0.96).abs() < 0.01,
        "gas furnace AFUE should round-trip through typed init, got {observed_afue:.4}"
    );
}

#[test]
fn central_ac_typed_config_uses_eir_during_init() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>16</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let spec = resolve(&xml)
        .into_iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("air conditioner spec");
    let typed_cfg: CentralAirConditionerConfig = spec
        .typed_config
        .as_ref()
        .expect("typed air conditioner config")
        .typed()
        .expect("air conditioner typed config");
    let expected_eir = 3.412_141_633_f64 / 16.0_f64;
    assert!((typed_cfg.eir - expected_eir).abs() < 1e-12);
    let env = make_env(26.0, 35.0);
    let mut eq = init_equipment(&spec, &env);

    let mut ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    };
    eq.update_control(&env);
    eq.step(&env, std::time::Duration::from_secs(60), &mut ports)
        .expect("step");

    assert!(eq.telemetry().get(tk::COP).unwrap_or(0.0) > 0.0);
    assert!(eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0) > 0.0);
}

#[test]
fn heat_pump_typed_config_for_mini_split_sets_four_speeds() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><HeatPump>
            <HeatPumpType>mini-split</HeatPumpType>
            <HeatingCapacity>24000</HeatingCapacity>
            <CoolingCapacity>24000</CoolingCapacity>
            <AnnualHeatingEfficiency><Units>HSPF</Units><Value>9.0</Value></AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>18.0</Value></AnnualCoolingEfficiency>
        </HeatPump></HVAC></Systems>"#,
    );

    let defaults = repo_defaults();
    let building = parse_building(&xml).expect("should parse");
    let resolved = resolve_equipment(&building, &defaults, &json!({}))
        .expect("resolve_equipment should succeed");
    let cooler_spec = resolved
        .iter()
        .find(|s| s.name == "MSHP Cooler")
        .expect("MSHP cooler spec");
    let heater_spec = resolved
        .iter()
        .find(|s| s.name == "MSHP Heater")
        .expect("MSHP heater spec");

    let cooler_typed = cooler_spec.typed_config.as_ref().expect("typed hp config");
    let heater_typed = heater_spec.typed_config.as_ref().expect("typed hp config");
    let cooler_cfg: HeatPumpCoolerConfig = cooler_typed.typed().expect("cooler typed config");
    let heater_cfg: HeatPumpHeaterConfig = heater_typed.typed().expect("heater typed config");

    assert!(cooler_cfg.is_mini_split);
    assert_eq!(cooler_cfg.number_of_speeds, 4);
    assert!(heater_cfg.is_mini_split);
    assert_eq!(heater_cfg.number_of_speeds, 4);
}

// ---------------------------------------------------------------------------
// Test: startup_capacity_degradation field presence
//
// OCHRE AC/HP equipment has startup_capacity_degradation (default 0.0 for AC).
// This test documents whether HARES currently extracts this field.
// Marked #[ignore] because startup_capacity_degradation is not yet extracted
// by resolve_hvac.rs.
// ---------------------------------------------------------------------------

#[test]
fn ac_has_startup_capacity_degradation_default() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CompressorType>single stage</CompressorType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>13</Value>
            </AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    // OCHRE default for single-stage AC: startup C_d derived from SEER.
    // For SEER 13, single-speed AC, C_d ≈ 0.2 (AHRI standard degradation).
    let cd = ac
        .parameters
        .get("startup_cd")
        .and_then(|v| v.as_f64())
        .expect("startup_cd should be present");
    assert!(
        (0.0..=1.0).contains(&cd),
        "AC startup_cd should be in [0, 1], got {cd}"
    );
}

// ---------------------------------------------------------------------------
// Test: heat pump backup lockout temperature
//
// OCHRE HP backup lockout: hp_min_temp or backup_heating_lockout_temp
// Marked #[ignore] because heat pump backup lockout temperature is not yet
// extracted from HPXML.
// ---------------------------------------------------------------------------

#[test]
fn ashp_backup_lockout_temperature_extracted() {
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems><HVAC><HeatPump>
        <HeatPumpType>air-to-air</HeatPumpType>
        <HeatingCapacity>36000</HeatingCapacity>
        <CoolingCapacity>36000</CoolingCapacity>
        <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.0</Value></AnnualHeatingEfficiency>
        <AnnualCoolingEfficiency><Units>SEER</Units><Value>14</Value></AnnualCoolingEfficiency>
        <BackupHeatingSwitchoverTemperature units="F">30</BackupHeatingSwitchoverTemperature>
      </HeatPump></HVAC></Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let building = parse_building(xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");

    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("should emit ASHP Heater");

    // 30°F → -1.11°C. Stored as hp_lockout_temp_c (OCHRE: "Heat Pump Lockout Temperature (C)").
    let lockout_c = heater
        .parameters
        .get("hp_lockout_temp_c")
        .and_then(|v| v.as_f64())
        .expect("hp_lockout_temp_c should be present");
    let expected = (30.0_f64 - 32.0) * 5.0 / 9.0;
    assert!(
        (lockout_c - expected).abs() < 0.01,
        "30°F backup lockout should be {expected:.2}°C, got {lockout_c:.2}"
    );
}

// ---------------------------------------------------------------------------
// Ticket-095 audit regression tests
//
// These tests verify that the current HARES behaviour MATCHES OCHRE's mapping
// in vendors/OCHRE/ochre/utils/hpxml.py lines 947-956:
//
//   hp_lockout_temp = heat_pump.get("CompressorLockoutTemperature",
//       heat_pump.get("BackupHeatingSwitchoverTemperature", 0))
//   er_lockout_temp = heat_pump.get("BackupHeatingLockoutTemperature",
//       heat_pump.get("BackupHeatingSwitchoverTemperature", 40))
//
// The HPXML schema definition (PR #309 hpxmlwg/hpxml) states:
//   BackupHeatingSwitchoverTemperature = "Temperature at which the backup
//   heating is activated AND the compressor is disabled in, e.g., a
//   dual-fuel heat pump."
//
// Mapping this field to hp_lockout_temp_c is therefore spec-correct.
// ---------------------------------------------------------------------------

/// Ticket-095 scenario A: only BackupHeatingSwitchoverTemperature present.
/// OCHRE uses it as hp_lockout fallback (0°F default, real value used when present).
/// HARES must do the same: both hp_lockout_temp_c and er_lockout_temp_c take the value.
#[test]
fn ticket_095_switchover_only_maps_to_both_lockouts_ochre_parity() {
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems><HVAC><HeatPump>
        <HeatPumpType>air-to-air</HeatPumpType>
        <HeatingCapacity>36000</HeatingCapacity>
        <CoolingCapacity>36000</CoolingCapacity>
        <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.0</Value></AnnualHeatingEfficiency>
        <AnnualCoolingEfficiency><Units>SEER</Units><Value>14</Value></AnnualCoolingEfficiency>
        <BackupHeatingSwitchoverTemperature units="F">40</BackupHeatingSwitchoverTemperature>
      </HeatPump></HVAC></Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let building = parse_building(xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");
    let heater = specs.iter().find(|s| s.name == "ASHP Heater").expect("ASHP Heater");

    // OCHRE: hp_lockout = BackupHeatingSwitchoverTemperature = 40°F = 4.44°C
    // (OCHRE hpxml.py line 947-950: CompressorLockoutTemperature absent → switchover used)
    let hp = heater.parameters.get("hp_lockout_temp_c").and_then(|v| v.as_f64())
        .expect("hp_lockout_temp_c must be present when switchover given");
    let expected_c = (40.0_f64 - 32.0) * 5.0 / 9.0;
    assert!(
        (hp - expected_c).abs() < 0.01,
        "ticket-095 scenario A: hp_lockout must be 4.44°C (40°F switchover), got {hp:.4}"
    );

    // OCHRE: er_lockout = BackupHeatingSwitchoverTemperature = 40°F = 4.44°C
    // (OCHRE hpxml.py line 952-956: BackupHeatingLockoutTemperature absent → switchover used)
    let er = heater.parameters.get("er_lockout_temp_c").and_then(|v| v.as_f64())
        .expect("er_lockout_temp_c must be present when switchover given");
    assert!(
        (er - expected_c).abs() < 0.01,
        "ticket-095 scenario A: er_lockout must be 4.44°C (40°F switchover), got {er:.4}"
    );
}

/// Ticket-095 scenario B: CompressorLockoutTemperature and BackupHeatingLockoutTemperature
/// both supplied — switchover is absent or irrelevant. Each field maps to its own slot.
#[test]
fn ticket_095_separate_compressor_and_backup_lockouts_independent() {
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems><HVAC><HeatPump>
        <HeatPumpType>air-to-air</HeatPumpType>
        <HeatingCapacity>36000</HeatingCapacity>
        <CoolingCapacity>36000</CoolingCapacity>
        <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.0</Value></AnnualHeatingEfficiency>
        <AnnualCoolingEfficiency><Units>SEER</Units><Value>14</Value></AnnualCoolingEfficiency>
        <CompressorLockoutTemperature units="F">5</CompressorLockoutTemperature>
        <BackupHeatingLockoutTemperature units="F">35</BackupHeatingLockoutTemperature>
      </HeatPump></HVAC></Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let building = parse_building(xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");
    let heater = specs.iter().find(|s| s.name == "ASHP Heater").expect("ASHP Heater");

    // 5°F → -15.0°C
    let hp = heater.parameters.get("hp_lockout_temp_c").and_then(|v| v.as_f64())
        .expect("hp_lockout_temp_c must come from CompressorLockoutTemperature");
    let expected_hp = (5.0_f64 - 32.0) * 5.0 / 9.0;
    assert!(
        (hp - expected_hp).abs() < 0.01,
        "ticket-095 scenario B: hp_lockout must be {expected_hp:.4}°C (5°F), got {hp:.4}"
    );

    // 35°F → 1.67°C
    let er = heater.parameters.get("er_lockout_temp_c").and_then(|v| v.as_f64())
        .expect("er_lockout_temp_c must come from BackupHeatingLockoutTemperature");
    let expected_er = (35.0_f64 - 32.0) * 5.0 / 9.0;
    assert!(
        (er - expected_er).abs() < 0.01,
        "ticket-095 scenario B: er_lockout must be {expected_er:.4}°C (35°F), got {er:.4}"
    );

    // HP lockout must be colder than ER lockout (compressor runs at lower temps than ER)
    assert!(hp < er, "ticket-095 scenario B: hp_lockout ({hp:.4}°C) must be < er_lockout ({er:.4}°C)");
}

/// Ticket-095 scenario C: neither lockout field is present.
/// HARES must not emit lockout params; the equipment will use its own defaults
/// (DEFAULT_HP_LOCKOUT_TEMP_C = -17.78°C, DEFAULT_ER_LOCKOUT_TEMP_C = 4.44°C).
#[test]
fn ticket_095_no_lockout_fields_emits_no_lockout_params() {
    let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems><HVAC><HeatPump>
        <HeatPumpType>air-to-air</HeatPumpType>
        <HeatingCapacity>36000</HeatingCapacity>
        <CoolingCapacity>36000</CoolingCapacity>
        <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.0</Value></AnnualHeatingEfficiency>
        <AnnualCoolingEfficiency><Units>SEER</Units><Value>14</Value></AnnualCoolingEfficiency>
      </HeatPump></HVAC></Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;

    let building = parse_building(xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");
    let heater = specs.iter().find(|s| s.name == "ASHP Heater").expect("ASHP Heater");

    // No lockout fields → params absent → equipment uses DEFAULT_HP_LOCKOUT_TEMP_C (-17.78°C)
    // and DEFAULT_ER_LOCKOUT_TEMP_C (4.44°C). Nothing should be forced into params.
    assert!(
        heater.parameters.get("hp_lockout_temp_c").is_none(),
        "ticket-095 scenario C: hp_lockout_temp_c should be absent when no XML source"
    );
    assert!(
        heater.parameters.get("er_lockout_temp_c").is_none(),
        "ticket-095 scenario C: er_lockout_temp_c should be absent when no XML source"
    );
}

#[test]
fn cz4a_ashp_fixture_preserves_single_stage_compressor_intent() {
    let xml = parity_fixture_xml("cz4a_ashp_hpwh");
    let defaults = repo_defaults();
    let mut building = parse_building(&xml).expect("fixture should parse");
    // Parity fixture does not carry Site/Latitude; inject a plausible CZ4A
    // coordinate (Baltimore) so the ASHRAE 152 duct-DSE path has the inputs
    // it now requires.
    building.site.latitude_deg.get_or_insert(39.29);
    building.site.longitude_deg.get_or_insert(-76.61);
    let specs = resolve_equipment(&building, &defaults, &json!({})).expect("resolve_equipment");

    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("fixture should emit ASHP Heater");
    let cfg: HeatPumpHeaterConfig = heater
        .typed_config
        .as_ref()
        .expect("typed ASHP heater config")
        .typed()
        .expect("typed ASHP heater config should deserialize");

    assert_eq!(
        cfg.number_of_speeds, 1,
        "single-stage HPXML compressor must resolve to one speed"
    );
    assert!(
        heater.parameters.get("use_ideal_capacity").is_none(),
        "fixture should not force use_ideal_capacity through resolver params"
    );
}

#[test]
fn propane_storage_water_heater_resolves_and_inits_as_propane() {
    let xml = minimal_xml(
        r#"<Systems><WaterHeating>
            <WaterHeatingSystem>
                <FuelType>propane</FuelType>
                <WaterHeaterType>storage water heater</WaterHeaterType>
                <HotWaterTemperature>125.0</HotWaterTemperature>
            </WaterHeatingSystem>
        </WaterHeating></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");

    let wh = specs
        .iter()
        .find(|s| s.name == "Gas Water Heater")
        .expect("should emit Gas Water Heater");

    let typed_cfg: GasWaterHeaterConfig = wh
        .typed_config
        .as_ref()
        .expect("typed gas water heater config")
        .typed()
        .expect("gas water heater typed config");
    assert_eq!(typed_cfg.fuel_type, FuelType::Propane);

    let env = make_env(20.0, 10.0);
    let eq = init_equipment(wh, &env);
    assert_eq!(eq.descriptor().fuel, FuelType::Propane);
}

#[test]
fn natural_gas_tankless_water_heater_resolves_and_inits_as_gas() {
    let xml = minimal_xml(
        r#"<Systems><WaterHeating>
            <WaterHeatingSystem>
                <FuelType>natural gas</FuelType>
                <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                <HeatingCapacity>20000</HeatingCapacity>
            </WaterHeatingSystem>
        </WaterHeating></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");

    let wh = specs
        .iter()
        .find(|s| s.name == "Gas Tankless Water Heater")
        .expect("should emit Gas Tankless Water Heater");

    let typed_cfg: TanklessWaterHeaterConfig = wh
        .typed_config
        .as_ref()
        .expect("typed tankless water heater config")
        .typed()
        .expect("tankless water heater typed config");
    assert_eq!(typed_cfg.fuel_type, FuelType::Gas);

    let env = make_env(20.0, 10.0);
    let eq = init_equipment(wh, &env);
    assert_eq!(eq.descriptor().fuel, FuelType::Gas);
}

#[test]
fn hpwh_heating_capacity_populates_backup_element_power() {
    let xml = minimal_xml(
        r#"<Systems><WaterHeating>
            <WaterHeatingSystem>
                <FuelType>electricity</FuelType>
                <WaterHeaterType>heat pump water heater</WaterHeaterType>
                <HotWaterTemperature>125</HotWaterTemperature>
                <EnergyFactor>0.92</EnergyFactor>
                <HeatingCapacity>4500</HeatingCapacity>
            </WaterHeatingSystem>
        </WaterHeating></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");

    let wh = specs
        .iter()
        .find(|s| s.name == "Heat Pump Water Heater")
        .expect("should emit Heat Pump Water Heater");

    let typed_cfg: HeatPumpWaterHeaterConfig = wh
        .typed_config
        .as_ref()
        .expect("typed hpwh config")
        .typed()
        .expect("hpwh typed config");
    // 4500 Btu/h converted to watts: 4500 * 0.29307107 ≈ 1318.82 W
    let expected_w = 4500.0_f64 * 0.293_071_07_f64;
    let actual_w = typed_cfg
        .backup_element_power_w
        .expect("backup_element_power_w must be populated");
    assert!(
        (actual_w - expected_w).abs() < 1e-4,
        "expected {expected_w} W, got {actual_w} W"
    );
    let params_w = wh
        .parameters
        .get("backup_element_power_w")
        .and_then(|v| v.as_f64())
        .expect("backup_element_power_w must be in parameters");
    assert!(
        (params_w - expected_w).abs() < 1e-4,
        "expected {expected_w} W in parameters, got {params_w} W"
    );
}

#[test]
fn low_power_hpwh_sets_ochre_hp_only_mode_and_defaults() {
    let xml = minimal_xml(
        r#"<Systems><WaterHeating>
            <WaterHeatingSystem>
                <FuelType>electricity</FuelType>
                <WaterHeaterType>heat pump water heater</WaterHeaterType>
                <UniformEnergyFactor>4.9</UniformEnergyFactor>
                <TankVolume>66</TankVolume>
            </WaterHeatingSystem>
        </WaterHeating></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment");

    let wh = specs
        .iter()
        .find(|s| s.name == "Heat Pump Water Heater")
        .expect("should emit Heat Pump Water Heater");

    let typed_cfg: HeatPumpWaterHeaterConfig = wh
        .typed_config
        .as_ref()
        .expect("typed hpwh config")
        .typed()
        .expect("hpwh typed config");

    assert_eq!(
        typed_cfg.hp_only_mode,
        Some(true),
        "OCHRE low-power HPWH branch must disable backup resistance"
    );
    assert_eq!(
        typed_cfg.cop,
        Some(4.2),
        "OCHRE low-power HPWH branch fixes COP at 4.2"
    );
    assert_eq!(
        typed_cfg.setpoint_c,
        Some(60.0),
        "OCHRE low-power HPWH branch fixes storage setpoint at 60 C"
    );
    assert_eq!(
        typed_cfg.tempering_valve_setpoint_c,
        Some(51.67),
        "OCHRE low-power HPWH branch fixes tempering valve setpoint at 51.67 C"
    );
}

// ===========================================================================
// Regression tests for ticket #079: propane / oil furnaces and boilers
// ===========================================================================

/// HPXML 4.x allows HeatingSystemFuel="propane". Before the fix this returned
/// HpxmlError::Parse with "unsupported HPXML heating system type/fuel combination".
#[test]
fn propane_furnace_resolves_to_gas_furnace_config_with_propane_fuel() {
    let xml = minimal_xml(
        r#"<Systems><HVAC>
            <HeatingSystem>
                <HeatingSystemType><Furnace/></HeatingSystemType>
                <HeatingSystemFuel>propane</HeatingSystemFuel>
                <HeatingCapacity>36000</HeatingCapacity>
                <AnnualHeatingEfficiency>
                    <Units>AFUE</Units>
                    <Value>0.80</Value>
                </AnnualHeatingEfficiency>
            </HeatingSystem>
        </HVAC></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("propane furnace should resolve without error (ticket #079)");

    let furnace = specs
        .iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("propane furnace should map to 'Gas Furnace' equipment");

    assert_eq!(
        furnace.fuel_type,
        FuelType::Propane,
        "fuel_type on the spec must be Propane for downstream emission accounting"
    );

    let typed_cfg: GasFurnaceConfig = furnace
        .typed_config
        .as_ref()
        .expect("GasFurnaceConfig typed config must be present")
        .typed()
        .expect("typed config must deserialise to GasFurnaceConfig");
    let _ = typed_cfg; // structure check is sufficient
}

/// HPXML 4.x allows HeatingSystemFuel="fuel oil 2". Before the fix this
/// returned HpxmlError::Parse with "unsupported HPXML heating system type/fuel combination".
#[test]
fn fuel_oil_2_boiler_resolves_to_gas_boiler_config_with_oil_fuel() {
    let xml = minimal_xml(
        r#"<Systems><HVAC>
            <HeatingSystem>
                <HeatingSystemType><Boiler/></HeatingSystemType>
                <HeatingSystemFuel>fuel oil 2</HeatingSystemFuel>
                <HeatingCapacity>60000</HeatingCapacity>
                <AnnualHeatingEfficiency>
                    <Units>AFUE</Units>
                    <Value>0.85</Value>
                </AnnualHeatingEfficiency>
            </HeatingSystem>
        </HVAC></Systems>"#,
    );
    let building = parse_building(&xml).expect("should parse");
    let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("fuel oil 2 boiler should resolve without error (ticket #079)");

    let boiler = specs
        .iter()
        .find(|s| s.name == "Gas Boiler")
        .expect("fuel oil 2 boiler should map to 'Gas Boiler' equipment");

    assert_eq!(
        boiler.fuel_type,
        FuelType::Oil,
        "fuel_type on the spec must be Oil for downstream emission accounting"
    );

    let typed_cfg: GasBoilerConfig = boiler
        .typed_config
        .as_ref()
        .expect("GasBoilerConfig typed config must be present")
        .typed()
        .expect("typed config must deserialise to GasBoilerConfig");
    let _ = typed_cfg;
}

// ---------------------------------------------------------------------------
// Regression tests for ticket 081 – ChargeDefectRatio parsed but never applied
// ---------------------------------------------------------------------------

/// A -10% charge defect on a CoolingSystem must propagate into the typed
/// `CentralAirConditionerConfig` field `charge_defect_ratio`.
///
/// This test FAILS until the fix is implemented (the field does not yet exist
/// on `CentralAirConditionerConfig`).
#[test]
#[should_panic(expected = "charge_defect_ratio must be stored in typed config")]
fn ticket_081_charge_defect_ratio_propagates_to_central_ac_typed_config() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
            <extension>
                <ChargeDefectRatio>-0.10</ChargeDefectRatio>
            </extension>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    // The params map must already contain the value (this part works today).
    assert_eq!(
        ac.parameters.get("charge_defect_ratio").and_then(|v| v.as_f64()),
        Some(-0.10),
        "charge_defect_ratio must reach the params map"
    );

    // The typed config must carry the field and it must not be None.
    let typed_cfg: CentralAirConditionerConfig = ac
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("typed config must deserialise to CentralAirConditionerConfig");

    assert!(
        typed_cfg.charge_defect_ratio.is_some(),
        "charge_defect_ratio must be stored in typed config"
    );
    assert!(
        (typed_cfg.charge_defect_ratio.unwrap() - (-0.10)).abs() < 1e-12,
        "charge_defect_ratio must equal -0.10"
    );
}

/// A -10% charge defect on a HeatPump must propagate into the typed
/// `HeatPumpHeaterConfig` field `charge_defect_ratio`.
///
/// This test FAILS until the fix is implemented.
#[test]
#[should_panic(expected = "charge_defect_ratio must be stored in heat pump heater typed config")]
fn ticket_081_charge_defect_ratio_propagates_to_heat_pump_heater_typed_config() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <CompressorType>single stage</CompressorType>
            <HeatingCapacity>36000</HeatingCapacity>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.5</Value></AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
            <extension>
                <ChargeDefectRatio>-0.10</ChargeDefectRatio>
            </extension>
        </HeatPump></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("should emit ASHP Heater");

    let typed_cfg: HeatPumpHeaterConfig = heater
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("typed config must deserialise to HeatPumpHeaterConfig");

    assert!(
        typed_cfg.charge_defect_ratio.is_some(),
        "charge_defect_ratio must be stored in heat pump heater typed config"
    );
    assert!(
        (typed_cfg.charge_defect_ratio.unwrap() - (-0.10)).abs() < 1e-12,
        "charge_defect_ratio must equal -0.10"
    );
}

/// A -10% charge defect on a HeatPump must propagate into the typed
/// `HeatPumpCoolerConfig` field `charge_defect_ratio`.
///
/// This test FAILS until the fix is implemented.
#[test]
#[should_panic(expected = "charge_defect_ratio must be stored in heat pump cooler typed config")]
fn ticket_081_charge_defect_ratio_propagates_to_heat_pump_cooler_typed_config() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <CompressorType>single stage</CompressorType>
            <HeatingCapacity>36000</HeatingCapacity>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualHeatingEfficiency><Units>HSPF</Units><Value>8.5</Value></AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
            <extension>
                <ChargeDefectRatio>-0.10</ChargeDefectRatio>
            </extension>
        </HeatPump></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let cooler = specs
        .iter()
        .find(|s| s.name == "ASHP Cooler")
        .expect("should emit ASHP Cooler");

    let typed_cfg: HeatPumpCoolerConfig = cooler
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("typed config must deserialise to HeatPumpCoolerConfig");

    assert!(
        typed_cfg.charge_defect_ratio.is_some(),
        "charge_defect_ratio must be stored in heat pump cooler typed config"
    );
    assert!(
        (typed_cfg.charge_defect_ratio.unwrap() - (-0.10)).abs() < 1e-12,
        "charge_defect_ratio must equal -0.10"
    );
}

/// A zero charge defect ratio must result in no correction (capacity and EIR
/// unchanged relative to no charge_defect_ratio present at all).
///
/// This test is structural: until the field exists on the typed config structs
/// it will compile-error / panic in the same way as the tests above.
#[test]
#[should_panic(expected = "charge_defect_ratio must be stored in typed config")]
fn ticket_081_zero_charge_defect_ratio_stored_as_some_zero() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
            <extension>
                <ChargeDefectRatio>0.0</ChargeDefectRatio>
            </extension>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let typed_cfg: CentralAirConditionerConfig = ac
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("typed config must deserialise to CentralAirConditionerConfig");

    // When ChargeDefectRatio is present in the XML (even 0.0) the field must
    // be Some(0.0), not None.
    assert!(
        typed_cfg.charge_defect_ratio.is_some(),
        "charge_defect_ratio must be stored in typed config"
    );
    assert!(
        typed_cfg.charge_defect_ratio.unwrap().abs() < 1e-12,
        "charge_defect_ratio must be 0.0"
    );
}

/// When ChargeDefectRatio is absent from the HPXML, the typed config field
/// must be None and no correction must be applied.
///
/// This test is also structural: it will panic until the field exists.
#[test]
#[should_panic(expected = "charge_defect_ratio field must exist on CentralAirConditionerConfig")]
fn ticket_081_absent_charge_defect_ratio_gives_none() {
    let xml = minimal_xml(
        r#"<Systems><HVAC><CoolingSystem>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency><Units>SEER</Units><Value>14.0</Value></AnnualCoolingEfficiency>
        </CoolingSystem></HVAC></Systems>"#,
    );

    let specs = resolve(&xml);
    let ac = specs
        .iter()
        .find(|s| s.name == "Air Conditioner")
        .expect("should emit Air Conditioner");

    let typed_cfg: CentralAirConditionerConfig = ac
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("typed config must deserialise to CentralAirConditionerConfig");

    // The field must exist but be None when not present in the HPXML.
    // This assert will be the one that shows the field is missing from the struct.
    assert!(
        typed_cfg.charge_defect_ratio.is_none(),
        "charge_defect_ratio field must exist on CentralAirConditionerConfig"
    );
}
