//! OCHRE parity tests for HPXML config extraction.
//!
//! Covers gaps identified in HARES-073 that are testable against the current
//! HARES implementation:
//! - number_of_speeds derivation from CompressorType (variable/single stage)
//! - number_of_speeds fallback from SEER rating when CompressorType is absent
//! - Duct parameter extraction (surface_area, leakage, r-value)
//! - fan_power_w_per_cfm storage for HVAC equipment
//! - Water heater UA derivation matches OCHRE for known EF inputs
//!
//! For each test, the OCHRE derivation logic is cited as a comment showing
//! the Python expression that produces the expected value.
//!
//! Tests for gaps that are NOT yet implemented (noted in HARES-073 audit) are
//! marked #[ignore] with a description of what is missing.

use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;
use serde_json::json;

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
    let building = parse_building(xml).expect("should parse");
    resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment should succeed")
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
#[ignore = "HARES gap: HPXML 4.x AirDistribution/Ducts elements are not yet parsed by building.rs parse_duct_systems() which only finds <DuctSystem> elements. Duct extraction is not currently working for standard HPXML 4.x input. Fix building.rs to also parse AirDistribution/Ducts."]
fn duct_parameters_extracted_for_hvac_equipment() {
    // This test is a placeholder for when duct parsing is implemented for HPXML 4.x format.
    // The DuctSystem element format currently expected by the parser is non-standard.
    // Once parse_duct_systems is fixed to handle AirDistribution/Ducts, use this HPXML:
    //
    // <Ducts>
    //   <DuctType>supply</DuctType>
    //   <DuctInsulationRValue>8.0</DuctInsulationRValue>
    //   <DuctSurfaceArea>50.0</DuctSurfaceArea>
    //   <DuctLocation>attic - unvented</DuctLocation>
    // </Ducts>
    //
    // Expected: duct_supply_area_m2 > 0, duct_supply_leakage_frac = 0.10,
    //           duct_supply_r_m2_k_w corresponds to RSI of R-8 (imperial) = ~1.41 m²·K/W
    unimplemented!("implement duct extraction in building.rs first")
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

// ---------------------------------------------------------------------------
// Test: startup_capacity_degradation field presence
//
// OCHRE AC/HP equipment has startup_capacity_degradation (default 0.0 for AC).
// This test documents whether HARES currently extracts this field.
// Marked #[ignore] because startup_capacity_degradation is listed as a gap in
// HARES-073 and is not yet extracted by resolve_hvac.rs.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "HARES-073 gap: startup_capacity_degradation is not yet extracted from HPXML (OCHRE default: 0.0 for AC). Implement in resolve_hvac.rs and update this test."]
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

    // OCHRE default: startup_capacity_degradation = 0.0 for AC
    let degradation = ac
        .parameters
        .get("startup_capacity_degradation")
        .and_then(|v| v.as_f64())
        .expect("startup_capacity_degradation should be present");
    assert!(
        (degradation - 0.0).abs() < 1e-9,
        "AC startup_capacity_degradation default should be 0.0, got {degradation}"
    );
}

// ---------------------------------------------------------------------------
// Test: heat pump backup lockout temperature
//
// OCHRE HP backup lockout: hp_min_temp or backup_heating_lockout_temp
// Marked #[ignore] because this is listed as a gap in HARES-073.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "HARES-073 gap: heat pump backup lockout temperature is not yet extracted from HPXML. OCHRE reads BackupHeatingSwitchoverTemperature or BackupHeatControlTemperature. Implement in resolve_hvac.rs."]
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

    // 30°F → -1.11°C
    let lockout_c = heater
        .parameters
        .get("backup_heating_lockout_c")
        .and_then(|v| v.as_f64())
        .expect("backup_heating_lockout_c should be present");
    let expected = (30.0_f64 - 32.0) * 5.0 / 9.0;
    assert!(
        (lockout_c - expected).abs() < 0.01,
        "30°F backup lockout should be {expected:.2}°C, got {lockout_c:.2}"
    );
}
