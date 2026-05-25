//! Regression tests for HPXML HVAC configuration wiring gaps.
//!
//! Each test demonstrates a gap: a field that is present in the HPXML input
//! and in the typed config struct, but is **not** wired through the resolver.
//!
//! Several tests currently FAIL (demonstrating the bug). They must pass after
//! the corresponding fix lands. Tests marked "documents the gap" pass today
//! and must remain passing to prove the gap still exists.

use serde_json::json;

use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
use hares_equipment::hvac::heating_config::GasBoilerConfig;
use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wrap_systems(systems_xml: &str) -> String {
    format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>40.0</Latitude>
          <Longitude>-105.0</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1500</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">12000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      {systems_xml}
    </BuildingDetails>
  </Building>
</HPXML>"#
    )
}

fn resolve_ok(xml: &str) -> Vec<hares_io::hpxml::EquipmentSpec> {
    let building = parse_building(xml).expect("HPXML must parse");
    resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
        .expect("resolve_equipment must succeed")
}

fn find_spec<'a>(
    specs: &'a [hares_io::hpxml::EquipmentSpec],
    ochre_class: &str,
) -> &'a hares_io::hpxml::EquipmentSpec {
    specs
        .iter()
        .find(|s| s.name == ochre_class)
        .unwrap_or_else(|| {
            let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
            panic!("spec '{ochre_class}' not found in {names:?}")
        })
}

// ---------------------------------------------------------------------------
// G1: CrankcaseHeaterWatts — NOT WIRED
// ---------------------------------------------------------------------------
//
// The HPXML element <CrankcaseHeaterWatts> should wire to `crankcase_heater_kw`
// in CentralAirConditionerConfig.  Currently the resolver hard-codes
// `crankcase_heater_kw: None` regardless of the HPXML value.
//
// OCHRE default: 50 W (0.050 kW) for central AC/ASHP at 12.78 °C (55 °F).
// EnergyPlus Coil:Heating:DX:SingleSpeed: crankcase heater default is 0 W,
// threshold default is 10 °C.

#[test]
fn g1_crankcase_heater_watts_wired_from_hpxml() {
    // G1: CrankcaseHeaterWatts→crankcase_heater_kw wiring is now fixed.
    // HPXML does not define a standard CrankcaseHeaterWatts element; the
    // resolver reads the direct child element for compatibility with
    // non-standard HPXML files and extension/CrankcaseHeaterPowerWatts
    // for OpenStudio-HPXML extension convention.
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <CoolingSystem>
            <SystemIdentifier id="AC1"/>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
              <Units>SEER</Units><Value>16.0</Value>
            </AnnualCoolingEfficiency>
            <CrankcaseHeaterWatts>75</CrankcaseHeaterWatts>
          </CoolingSystem>
        </HVAC></Systems>"#,
    );
    let specs = resolve_ok(&xml);
    let spec = find_spec(&specs, "Air Conditioner");

    let typed_config = spec
        .typed_config
        .as_ref()
        .expect("Air Conditioner must have a typed_config");
    let cfg: CentralAirConditionerConfig = typed_config
        .typed()
        .expect("must deserialize into CentralAirConditionerConfig");

    assert_eq!(
        cfg.crankcase_heater_kw,
        Some(0.075),
        "CrankcaseHeaterWatts=75 must wire to crankcase_heater_kw=Some(0.075 kW)"
    );
}

#[test]
fn g1_crankcase_heater_absent_uses_ochre_default() {
    // G1: when no crankcase data is provided, the resolver applies
    // OCHRE-compatible defaults: 0.050 kW (50 W) at 12.78 °C (55 °F) for
    // central AC. OCHRE HVAC.py AirConditioner class.
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <CoolingSystem>
            <SystemIdentifier id="AC2"/>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
              <Units>SEER</Units><Value>16.0</Value>
            </AnnualCoolingEfficiency>
          </CoolingSystem>
        </HVAC></Systems>"#,
    );
    let specs = resolve_ok(&xml);
    let spec = find_spec(&specs, "Air Conditioner");

    let typed_config = spec
        .typed_config
        .as_ref()
        .expect("Air Conditioner must have a typed_config");
    let cfg: CentralAirConditionerConfig = typed_config
        .typed()
        .expect("must deserialize into CentralAirConditionerConfig");

    assert_eq!(
        cfg.crankcase_heater_kw,
        Some(0.050),
        "absent crankcase must apply OCHRE default of 0.050 kW (50 W) for central AC"
    );
    let threshold = cfg.crankcase_heater_threshold_c.unwrap_or(f64::NAN);
    assert!(
        (threshold - 12.78).abs() < 0.01,
        "absent threshold must default to 12.78 °C (55 °F)"
    );
}

// ---------------------------------------------------------------------------
// G3: MinimumCapacity — WIRED
// ---------------------------------------------------------------------------
//
// MinimumCapacity in HPXML is wired to min_compressor_fraction via
// MinimumCapacity / HeatingCapacity. The 25% hardcode in heater.rs has been
// replaced with the configurable min_compressor_fraction field.

#[test]
fn g3_resolver_wires_min_compressor_fraction_from_minimum_capacity() {
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <HeatPump>
            <SystemIdentifier id="MSHP1"/>
            <HeatPumpType>mini-split</HeatPumpType>
            <HeatingCapacity>36000</HeatingCapacity>
            <MinimumCapacity>10800</MinimumCapacity>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualHeatingEfficiency>
              <Units>HSPF</Units><Value>10.0</Value>
            </AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency>
              <Units>SEER</Units><Value>18.0</Value>
            </AnnualCoolingEfficiency>
          </HeatPump>
        </HVAC></Systems>"#,
    );
    let specs = resolve_ok(&xml);
    let heater = find_spec(&specs, "MSHP Heater");

    let frac = heater
        .parameters
        .get("min_compressor_fraction")
        .and_then(|v| v.as_f64());
    assert!(
        frac.is_some(),
        "min_compressor_fraction must be wired from MinimumCapacity / HeatingCapacity"
    );
    // 10800 Btu/h / 36000 Btu/h = 0.30
    let expected = 10_800.0 / 36_000.0;
    assert!(
        (frac.unwrap() - expected).abs() < 1e-6,
        "min_compressor_fraction = {:.4}, expected {:.4}",
        frac.unwrap(),
        expected,
    );
}

// ---------------------------------------------------------------------------
// G4: Condensing boiler — NOT WIRED
// ---------------------------------------------------------------------------
//
// GasBoilerConfig has no `condensing: bool` field.  OCHRE infers condensing
// from AFUE > 0.90 and applies different 6-coeff vs 10-coeff efficiency curves.

#[test]
fn g4_gas_boiler_config_has_condensing_field() {
    // G4: GasBoilerConfig now has a `condensing: bool` field.
    // The resolver sets it from AFUE > 0.90 per OCHRE convention.
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <HeatingSystem>
            <SystemIdentifier id="Boiler1"/>
            <HeatingSystemType><Boiler><BoilerType>hot water</BoilerType></Boiler></HeatingSystemType>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingCapacity>60000</HeatingCapacity>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units><Value>0.95</Value>
            </AnnualHeatingEfficiency>
          </HeatingSystem>
        </HVAC></Systems>"#,
    );
    let specs = resolve_ok(&xml);
    let boiler = find_spec(&specs, "Gas Boiler");

    let typed_config = boiler
        .typed_config
        .as_ref()
        .expect("Gas Boiler must have a typed_config");
    let cfg: GasBoilerConfig = typed_config
        .typed()
        .expect("must deserialize into GasBoilerConfig");
    assert!(
        (cfg.afue - 0.95).abs() < 1e-9,
        "AFUE must be 0.95, got {}",
        cfg.afue
    );
    // AFUE 0.95 > 0.90 → condensing boiler.
    assert!(
        cfg.condensing,
        "AFUE=0.95 must set condensing=true (OCHRE: AFUE > 0.90)"
    );
}

// ---------------------------------------------------------------------------
// G6: SupplementalHeatingLockoutTemperature — NOT FULLY WIRED
// ---------------------------------------------------------------------------
//
// max_oat_supplemental_c in HeatPumpHeaterConfig is only populatable via the
// extension params map; the standard HPXML element is not read.

#[test]
fn g6_supplemental_heating_lockout_temperature_wired() {
    // G6: SupplementalHeatingLockoutTemperature is now read from HPXML
    // and wired to max_oat_supplemental_c in HeatPumpHeaterConfig.
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <HeatPump>
            <SystemIdentifier id="HP1"/>
            <HeatPumpType>air-to-air</HeatPumpType>
            <HeatingCapacity>36000</HeatingCapacity>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualHeatingEfficiency>
              <Units>HSPF</Units><Value>9.0</Value>
            </AnnualHeatingEfficiency>
            <AnnualCoolingEfficiency>
              <Units>SEER</Units><Value>16.0</Value>
            </AnnualCoolingEfficiency>
            <BackupHeatingSystem>
              <SystemIdentifier id="ER1"/>
              <BackupSystemFuel>electricity</BackupSystemFuel>
              <BackupHeatingCapacity>18000</BackupHeatingCapacity>
              <BackupAnnualHeatingEfficiency>
                <Units>Percent</Units><Value>1.0</Value>
              </BackupAnnualHeatingEfficiency>
            </BackupHeatingSystem>
            <SupplementalHeatingLockoutTemperature>65</SupplementalHeatingLockoutTemperature>
          </HeatPump>
        </HVAC></Systems>"#,
    );
    let specs = resolve_ok(&xml);
    let heater = find_spec(&specs, "ASHP Heater");

    let supplemental_oat = heater
        .parameters
        .get("max_oat_supplemental_c")
        .and_then(|v| v.as_f64());

    // 65°F → °C = (65 - 32) * 5/9 ≈ 18.33°C
    let expected_c = (65.0 - 32.0) * 5.0 / 9.0;
    assert!(
        supplemental_oat.is_some(),
        "SupplementalHeatingLockoutTemperature=65°F must wire to max_oat_supplemental_c={expected_c:.2}°C"
    );
    assert!(
        (supplemental_oat.unwrap() - expected_c).abs() < 0.01,
        "max_oat_supplemental_c must be {expected_c:.2}°C, got {:.2}",
        supplemental_oat.unwrap()
    );
}
