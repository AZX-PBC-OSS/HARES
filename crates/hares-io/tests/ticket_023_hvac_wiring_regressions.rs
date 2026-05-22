//! Regression tests for ticket 023 — HPXML HVAC configuration wiring gaps.
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
#[should_panic(expected = "BUG (ticket 023 G1): CrankcaseHeaterWatts=75 must wire to")]
fn g1_crankcase_heater_watts_wired_from_hpxml() {
    // BUG (ticket 023 G1): the resolver sets crankcase_heater_kw = None
    // regardless of the <CrankcaseHeaterWatts> HPXML value.
    // This test must FAIL until the wiring is added.
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
        "BUG (ticket 023 G1): CrankcaseHeaterWatts=75 must wire to \
         crankcase_heater_kw=Some(0.075 kW); got {:?}",
        cfg.crankcase_heater_kw
    );
}

#[test]
#[should_panic(expected = "BUG (ticket 023 G1): absent CrankcaseHeaterWatts must apply OCHRE")]
fn g1_crankcase_heater_absent_uses_ochre_default() {
    // BUG (ticket 023 G1): when CrankcaseHeaterWatts is absent, the resolver
    // must apply OCHRE-compatible defaults (50 W, 12.78 °C for central AC).
    // Currently it leaves both fields as None.
    // This test must FAIL until the default application is added.
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
        "BUG (ticket 023 G1): absent CrankcaseHeaterWatts must apply OCHRE \
         default of 0.050 kW (50 W) for central AC; got {:?}",
        cfg.crankcase_heater_kw
    );
    let threshold = cfg.crankcase_heater_threshold_c.unwrap_or(f64::NAN);
    assert!(
        (threshold - 12.78).abs() < 0.01,
        "BUG (ticket 023 G1): absent threshold must default to 12.78 °C (55 °F); \
         got {threshold:.2}"
    );
}

// ---------------------------------------------------------------------------
// G1 presence check — documents the current None state (passes today)
// ---------------------------------------------------------------------------

#[test]
fn g1_crankcase_heater_kw_is_currently_none_documents_gap() {
    // Documents the current (broken) state: the resolver always emits None.
    // This test PASSES today. When the fix is applied it will correctly fail,
    // signaling that g1_crankcase_heater_watts_wired_from_hpxml should pass.
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <CoolingSystem>
            <SystemIdentifier id="AC3"/>
            <CoolingSystemType>central air conditioner</CoolingSystemType>
            <CoolingCapacity>36000</CoolingCapacity>
            <AnnualCoolingEfficiency>
              <Units>SEER</Units><Value>16.0</Value>
            </AnnualCoolingEfficiency>
            <CrankcaseHeaterWatts>50</CrankcaseHeaterWatts>
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

    // Today this is None (the bug). Remove this assertion when G1 is fixed.
    assert_eq!(
        cfg.crankcase_heater_kw, None,
        "Gap still present (expected today): crankcase_heater_kw should be \
         Some(0.050) after fix but is currently None"
    );
}

// ---------------------------------------------------------------------------
// G3: MinimumCapacity — NOT WIRED, 25% hardcode in heater.rs
// ---------------------------------------------------------------------------
//
// Documents that the resolver does not populate a min_compressor_fraction
// param.  The 25% hardcode lives in heater.rs:517-524 (init-time, not
// resolver-time), so it is checked separately.

#[test]
fn g3_resolver_does_not_wire_min_compressor_fraction() {
    // Documents the gap: MinimumCapacity in HPXML is not wired to any
    // resolver output parameter.  This test PASSES today (gap confirmed).
    let xml = wrap_systems(
        r#"<Systems><HVAC>
          <HeatPump>
            <SystemIdentifier id="MSHP1"/>
            <HeatPumpType>mini-split</HeatPumpType>
            <HeatingCapacity>36000</HeatingCapacity>
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

    assert!(
        heater.parameters.get("min_compressor_fraction").is_none(),
        "Gap confirmed (ticket 023 G3): min_compressor_fraction must be absent \
         from resolver output until tickets 013+023-G3 are implemented; \
         if this assertion fires, the gap has been closed"
    );
}

// ---------------------------------------------------------------------------
// G4: Condensing boiler — NOT WIRED
// ---------------------------------------------------------------------------
//
// GasBoilerConfig has no `condensing: bool` field.  OCHRE infers condensing
// from AFUE > 0.90 and applies different 6-coeff vs 10-coeff efficiency curves.

#[test]
fn g4_gas_boiler_config_has_no_condensing_field() {
    // Documents the gap: GasBoilerConfig lacks a `condensing` field, so no
    // resolver can ever set it.  This test PASSES today (gap confirmed).
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

    // No `condensing` key in the raw params — the field doesn't exist yet.
    assert!(
        boiler.parameters.get("condensing").is_none(),
        "Gap confirmed (ticket 023 G4): `condensing` field absent from \
         GasBoilerConfig/params; AFUE=0.95 should be treated as condensing \
         (OCHRE: AFUE>0.90) once the field is added. \
         If this fires, the field has been added."
    );

    // Verify AFUE is correctly carried (the resolver does read AFUE).
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
}

// ---------------------------------------------------------------------------
// G6: SupplementalHeatingLockoutTemperature — NOT FULLY WIRED
// ---------------------------------------------------------------------------
//
// max_oat_supplemental_c in HeatPumpHeaterConfig is only populatable via the
// extension params map; the standard HPXML element is not read.

#[test]
fn g6_supplemental_heating_lockout_temperature_not_read_from_standard_element() {
    // Documents the gap: SupplementalHeatingLockoutTemperature in the
    // standard HPXML element is not parsed.  The resolver's output must
    // therefore not carry max_oat_supplemental_c.
    // This test PASSES today (gap confirmed).
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

    // The element is in the HPXML but the resolver does not read it.
    // max_oat_supplemental_c must be absent from raw params.
    let supplemental_oat = heater
        .parameters
        .get("max_oat_supplemental_c")
        .and_then(|v| v.as_f64());

    assert!(
        supplemental_oat.is_none(),
        "Gap confirmed (ticket 023 G6): SupplementalHeatingLockoutTemperature \
         is not parsed from the standard HPXML element; max_oat_supplemental_c \
         must be None. Got {supplemental_oat:?}. \
         If this fires, the standard-element wiring has been added."
    );
}
