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
use hares_equipment::hvac::heating_config::GasFurnaceConfig;
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

// ---------------------------------------------------------------------------
// Test: duct parameters extracted (surface area, leakage, r-value)
//
// OCHRE Dwelling.py:
//   duct_surface_area = sum(duct.SurfaceArea for duct in ducts)
//   duct_leakage_fraction = duct.DuctLeakageValue (fractional)
//   duct_r_value = duct.DuctInsulationRValue
//
// HARES stores these as duct_supply_area_m2, duct_supply_leakage_frac, etc.
//
// NOTE: HARES building.rs parse_duct_systems() searches for <DuctSystem>
// elements (not HPXML 4.x <HVACDistribution/AirDistribution/Ducts>). The
// test below uses the format the parser actually reads; a full HPXML 4.x
// test requires DuctSystem adapter logic not yet implemented.
//
// This test documents the current duct extraction behavior using the
// existing DuctSystem element format that HARES parses.
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

    let area_m2 = supply
        .surface_area_m2
        .expect("supply duct should have surface area");
    assert!(
        area_m2 > 0.0,
        "supply duct surface area must be > 0, got {area_m2}"
    );
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
fn central_ac_typed_config_uses_seer_during_init() {
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
    assert!((typed_cfg.seer - 16.0).abs() < 1e-12);
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
        cd >= 0.0 && cd <= 1.0,
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
    assert_eq!(typed_cfg.backup_element_power_w, Some(4500.0));
    assert_eq!(
        wh.parameters
            .get("backup_element_power_w")
            .and_then(|v| v.as_f64()),
        Some(4500.0)
    );
}
