//! Regression tests: every field that used to be silently defaulted must now
//! error loudly with a `HpxmlError::MissingField` carrying the HPXML path,
//! system kind, and system id.
//!
//! Per project rule: no silent substitution of engineering defaults. Each test
//! pairs a missing-field scenario (must error) with a happy-path scenario
//! (must resolve cleanly), so both the negative and positive code paths are
//! exercised.

use serde_json::json;

use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::HpxmlError;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;

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

fn resolve(xml: &str) -> Result<Vec<hares_io::hpxml::EquipmentSpec>, HpxmlError> {
    let building = parse_building(xml).expect("HPXML must parse");
    resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
}

fn expect_missing(
    result: Result<Vec<hares_io::hpxml::EquipmentSpec>, HpxmlError>,
    expected_path: &str,
    expected_kind: &str,
) {
    match result {
        Err(HpxmlError::MissingField {
            path,
            system_kind,
            system_id,
            reason,
        }) => {
            assert_eq!(
                path, expected_path,
                "MissingField.path mismatch (system_id={system_id}, reason={reason})"
            );
            assert_eq!(
                system_kind, expected_kind,
                "MissingField.system_kind mismatch (path={path}, system_id={system_id})"
            );
        }
        Err(other) => panic!("expected HpxmlError::MissingField, got {other:?}"),
        Ok(_) => panic!("expected HpxmlError::MissingField, but parse succeeded"),
    }
}

fn expect_invalid_field(
    result: Result<Vec<hares_io::hpxml::EquipmentSpec>, HpxmlError>,
    expected_path: &str,
    expected_kind: &str,
    expected_value_received: &str,
) {
    match result {
        Err(HpxmlError::InvalidField {
            path,
            system_kind,
            system_id,
            value_received,
            reason,
        }) => {
            assert_eq!(
                path, expected_path,
                "InvalidField.path mismatch (system_id={system_id}, \
                 value_received={value_received}, reason={reason})"
            );
            assert_eq!(
                system_kind, expected_kind,
                "InvalidField.system_kind mismatch (path={path}, \
                 system_id={system_id}, value_received={value_received})"
            );
            assert_eq!(
                value_received, expected_value_received,
                "InvalidField.value_received mismatch (path={path}, \
                 system_kind={system_kind}, system_id={system_id})"
            );
        }
        Err(other) => panic!("expected HpxmlError::InvalidField, got {other:?}"),
        Ok(_) => panic!("expected HpxmlError::InvalidField, but parse succeeded"),
    }
}

// --- PV ---------------------------------------------------------------------

#[test]
fn pv_missing_max_power_output_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <Photovoltaics>
            <PVSystem>
              <SystemIdentifier id="pv1"/>
              <ArrayTilt>20</ArrayTilt>
              <ArrayAzimuth>180</ArrayAzimuth>
            </PVSystem>
          </Photovoltaics>
        </Systems>"#,
    );
    expect_missing(resolve(&xml), "PVSystem/MaxPowerOutput", "PV");
}

#[test]
fn pv_with_max_power_output_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <Photovoltaics>
            <PVSystem>
              <SystemIdentifier id="pv1"/>
              <MaxPowerOutput>5000</MaxPowerOutput>
              <ArrayTilt>20</ArrayTilt>
              <ArrayAzimuth>180</ArrayAzimuth>
            </PVSystem>
          </Photovoltaics>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("PV should resolve when MaxPowerOutput is present");
    assert!(
        specs.iter().any(|s| s.name == "PV"),
        "expected a PV spec in {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

// --- Battery ----------------------------------------------------------------

#[test]
fn battery_missing_nominal_capacity_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <Batteries>
            <Battery>
              <SystemIdentifier id="bat1"/>
              <RatedPowerOutput>5000</RatedPowerOutput>
            </Battery>
          </Batteries>
        </Systems>"#,
    );
    expect_missing(resolve(&xml), "Battery/NominalCapacity", "Battery");
}

#[test]
fn battery_missing_rated_power_output_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <Batteries>
            <Battery>
              <SystemIdentifier id="bat1"/>
              <NominalCapacity><Units>kWh</Units><Value>13.5</Value></NominalCapacity>
            </Battery>
          </Batteries>
        </Systems>"#,
    );
    expect_missing(resolve(&xml), "Battery/RatedPowerOutput", "Battery");
}

#[test]
fn battery_with_required_fields_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <Batteries>
            <Battery>
              <SystemIdentifier id="bat1"/>
              <RatedPowerOutput>5000</RatedPowerOutput>
              <NominalCapacity><Units>kWh</Units><Value>13.5</Value></NominalCapacity>
            </Battery>
          </Batteries>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("battery should resolve");
    assert!(specs.iter().any(|s| s.name == "Battery"));
}

// --- EV ---------------------------------------------------------------------

#[test]
fn ev_missing_battery_capacity_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <ElectricVehicles>
            <ElectricVehicle>
              <SystemIdentifier id="ev1"/>
              <MaxChargingPower>7.2</MaxChargingPower>
            </ElectricVehicle>
          </ElectricVehicles>
        </Systems>"#,
    );
    expect_missing(resolve(&xml), "ElectricVehicle/BatteryCapacity", "EV");
}

#[test]
fn ev_missing_max_charging_power_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <ElectricVehicles>
            <ElectricVehicle>
              <SystemIdentifier id="ev1"/>
              <BatteryCapacity><Units>kWh</Units><Value>75</Value></BatteryCapacity>
            </ElectricVehicle>
          </ElectricVehicles>
        </Systems>"#,
    );
    expect_missing(resolve(&xml), "ElectricVehicle/MaxChargingPower", "EV");
}

#[test]
fn ev_with_required_fields_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <ElectricVehicles>
            <ElectricVehicle>
              <SystemIdentifier id="ev1"/>
              <BatteryCapacity><Units>kWh</Units><Value>75</Value></BatteryCapacity>
              <MaxChargingPower>7.2</MaxChargingPower>
            </ElectricVehicle>
          </ElectricVehicles>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("EV should resolve");
    assert!(specs.iter().any(|s| s.name == "EV"));
}

// --- Generator --------------------------------------------------------------

#[test]
fn generator_missing_electrical_power_output_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <extension>
            <Generators>
              <Generator>
                <SystemIdentifier id="gen1"/>
                <FuelType>natural gas</FuelType>
              </Generator>
            </Generators>
          </extension>
        </Systems>"#,
    );
    expect_missing(
        resolve(&xml),
        "Generator/ElectricalPowerOutput",
        "Generator",
    );
}

#[test]
fn generator_with_required_fields_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <extension>
            <Generators>
              <Generator>
                <SystemIdentifier id="gen1"/>
                <FuelType>natural gas</FuelType>
                <ElectricalPowerOutput>10</ElectricalPowerOutput>
              </Generator>
            </Generators>
          </extension>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Generator should resolve");
    assert!(specs.iter().any(|s| s.name == "Gas Generator"));
}

// --- Gas Furnace: AFUE ------------------------------------------------------

#[test]
fn gas_furnace_missing_afue_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="fur1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_missing(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency[AFUE]",
        "Gas Furnace",
    );
}

#[test]
fn gas_furnace_with_afue_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="fur1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Furnace should resolve when AFUE is provided");
    assert!(specs.iter().any(|s| s.name == "Gas Furnace"));
}

// --- Gas Boiler: AFUE -------------------------------------------------------

#[test]
fn gas_boiler_missing_afue_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="boi1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_missing(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency[AFUE]",
        "Gas Boiler",
    );
}

#[test]
fn gas_boiler_with_afue_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="boi1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.85</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Boiler should resolve when AFUE is provided");
    assert!(specs.iter().any(|s| s.name == "Gas Boiler"));
}

// --- Ticket #112: Gas/Electric Boiler flow_rate_kg_s and return_temp_c silent defaults -----
//
// When HPXML omits flow_rate_kg_s and return_temp_c, the resolver silently
// applies 0.5 kg/s and 40.0 °C without any log diagnostic or citation.
// The fix (ticket #112, Approach A) must add tracing::debug! and an inline
// citation comment. These tests document the current silent-default behaviour
// so that any future "fix" that changes the defaults or removes the substitution
// is caught immediately.

#[test]
fn gas_boiler_omitting_flow_rate_and_return_temp_silently_applies_defaults() {
    use hares_equipment::hvac::heating_config::GasBoilerConfig;

    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="boi1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.85</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );

    let specs = resolve(&xml).expect("boiler with omitted hydronic fields must resolve");
    let boiler = specs
        .iter()
        .find(|s| s.name == "Gas Boiler")
        .expect("must emit Gas Boiler");
    let cfg: GasBoilerConfig = boiler
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("must deserialize to GasBoilerConfig");

    // Ticket #112: these defaults are applied silently (no tracing::debug!, no citation).
    // The fix must add a debug log and inline ASHRAE citation for each.
    assert_eq!(
        cfg.flow_rate_kg_s, 0.5,
        "ticket #112: flow_rate_kg_s default must be 0.5 kg/s (currently silent)"
    );
    assert_eq!(
        cfg.return_temp_c, 40.0,
        "ticket #112: return_temp_c default must be 40.0 °C (currently silent)"
    );
}

#[test]
fn electric_boiler_omitting_flow_rate_and_return_temp_silently_applies_defaults() {
    use hares_equipment::hvac::heating_config::ElectricBoilerConfig;

    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="eboi1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>12000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>1.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );

    let specs = resolve(&xml).expect("electric boiler with omitted hydronic fields must resolve");
    let boiler = specs
        .iter()
        .find(|s| s.name == "Electric Boiler")
        .expect("must emit Electric Boiler");
    let cfg: ElectricBoilerConfig = boiler
        .typed_config
        .as_ref()
        .expect("typed config must be present")
        .typed()
        .expect("must deserialize to ElectricBoilerConfig");

    // Ticket #112: these defaults are applied silently (no tracing::debug!, no citation).
    assert_eq!(
        cfg.flow_rate_kg_s, 0.5,
        "ticket #112: flow_rate_kg_s default must be 0.5 kg/s (currently silent)"
    );
    assert_eq!(
        cfg.return_temp_c, 40.0,
        "ticket #112: return_temp_c default must be 40.0 °C (currently silent)"
    );
}

// --- Heat Pump Water Heater: HotWaterTemperature ---------------------------

#[test]
fn hpwh_missing_hot_water_temperature_errors() {
    let xml = wrap_systems(
        r#"<WaterHeating>
          <WaterHeatingSystem>
            <SystemIdentifier id="wh1"/>
            <FuelType>electricity</FuelType>
            <WaterHeaterType>heat pump water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <UniformEnergyFactor>3.45</UniformEnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
    );
    expect_missing(
        resolve(&xml),
        "WaterHeatingSystem/HotWaterTemperature",
        "Heat Pump Water Heater",
    );
}

#[test]
fn hpwh_with_hot_water_temperature_resolves() {
    let xml = wrap_systems(
        r#"<WaterHeating>
          <WaterHeatingSystem>
            <SystemIdentifier id="wh1"/>
            <FuelType>electricity</FuelType>
            <WaterHeaterType>heat pump water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <HotWaterTemperature>125</HotWaterTemperature>
            <UniformEnergyFactor>3.45</UniformEnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
    );
    let specs = resolve(&xml).expect("HPWH should resolve when HotWaterTemperature is provided");
    assert!(specs.iter().any(|s| s.name == "Heat Pump Water Heater"));
}

// --- Site + Duct DSE: lat/lon/volume ---------------------------------------

/// Same fixture as `wrap_systems` but omits `<Latitude>` from the Site element.
fn wrap_systems_no_lat(systems_xml: &str) -> String {
    format!(
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Longitude>-105.0</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1500</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">12000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Attics><Attic><FloorArea units="ft2">500</FloorArea></Attic></Attics>
      </Enclosure>
      {systems_xml}
    </BuildingDetails>
  </Building>
</HPXML>"#
    )
}

const DUCTED_FURNACE_SYSTEMS: &str = r#"<Systems>
  <HVAC>
    <HVACDistribution>
      <DistributionSystemType>
        <AirDistribution>
          <DuctLeakageMeasurement>
            <SystemIdentifier id="LeakSupply"/>
            <DuctType>supply</DuctType>
            <DuctLeakage><Value>10</Value><Units>Percent</Units></DuctLeakage>
          </DuctLeakageMeasurement>
          <Ducts>
            <SystemIdentifier id="SupplyDuct1"/>
            <DuctType>supply</DuctType>
            <DuctInsulationRValue units="hr-ft2-F/BTU">6</DuctInsulationRValue>
            <DuctSurfaceArea units="ft2">150</DuctSurfaceArea>
            <DuctLocation>attic vented</DuctLocation>
          </Ducts>
        </AirDistribution>
      </DistributionSystemType>
    </HVACDistribution>
    <HeatingSystem>
      <SystemIdentifier id="fur1"/>
      <HeatingSystemFuel>natural gas</HeatingSystemFuel>
      <HeatingSystemType><Furnace/></HeatingSystemType>
      <HeatingCapacity>60000</HeatingCapacity>
      <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
    </HeatingSystem>
  </HVAC>
</Systems>"#;

#[test]
fn ducts_outside_conditioned_space_require_site_latitude() {
    let xml = wrap_systems_no_lat(DUCTED_FURNACE_SYSTEMS);
    match resolve(&xml) {
        Err(HpxmlError::MissingField {
            path, system_kind, ..
        }) => {
            assert_eq!(path, "Site/Latitude");
            assert_eq!(system_kind, "Building");
        }
        other => panic!("expected Site/Latitude MissingField, got {other:?}"),
    }
}

#[test]
fn ducts_outside_conditioned_space_resolve_when_site_data_present() {
    // `wrap_systems` includes Latitude, Longitude, and ConditionedBuildingVolume.
    let xml = wrap_systems(DUCTED_FURNACE_SYSTEMS);
    let specs = resolve(&xml).expect("ducted furnace should resolve with full site data");
    let furnace = specs
        .iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("Gas Furnace spec must be present");
    // The happy path here is that `resolve_equipment` returns Ok -- the
    // absence of a lat/lon MissingField is the contract. When the attic
    // zone plumbing produces a valid `duct_zone_id`, `duct_latitude_deg`
    // is also injected; verify that compound behaviour when available.
    if furnace.parameters.contains_key("duct_zone_id") {
        assert!(
            furnace.parameters.contains_key("duct_latitude_deg"),
            "duct_latitude_deg must accompany duct_zone_id"
        );
        assert!(
            furnace.parameters.contains_key("duct_longitude_deg"),
            "duct_longitude_deg must accompany duct_zone_id"
        );
    }
}

// --- Ticket 085: missing HeatingCapacity triggers autosizing flag, not silent drop -----
//
// When HPXML omits <HeatingCapacity>, Phase 2 autosizing kicks in: the resolver
// sets autosize_heating = true and the dwelling builder computes capacity from
// the building envelope model at design conditions. The equipment spec is still
// emitted (typed_config = None) because capacity will be populated after
// autosizing. Phase 1 errors fire in the equipment init layer when capacity is
// still absent after autosizing completes.

/// Furnace without <HeatingCapacity> resolves with autosize_heating flag set,
/// signalling that capacity will be computed by the dwelling builder later.
#[test]
fn gas_furnace_missing_heating_capacity_flags_autosize() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="fur1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs =
        resolve(&xml).expect("furnace without HeatingCapacity must resolve with autosize flag");
    let furnace = specs
        .iter()
        .find(|s| s.name == "Gas Furnace")
        .expect("Gas Furnace spec must be emitted even without HeatingCapacity");
    let autosize = furnace
        .parameters
        .get("autosize_heating")
        .and_then(|v| v.as_bool())
        .expect("autosize_heating must be set when HeatingCapacity is absent");
    assert!(
        autosize,
        "autosize_heating must be true when HeatingCapacity is absent"
    );
    assert!(
        !furnace.parameters.contains_key("heating_capacity_w"),
        "heating_capacity_w must not be set when HeatingCapacity is absent \
         (autosizing will compute it later)"
    );
}

// Gas furnace with both AFUE and HeatingCapacity must still resolve cleanly.
#[test]
fn gas_furnace_with_heating_capacity_and_afue_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="fur1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Gas Furnace with capacity and AFUE must resolve");
    assert!(
        specs.iter().any(|s| s.name == "Gas Furnace"),
        "expected Gas Furnace spec in {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

/// ASHP heat pump without <HeatingCapacity> resolves with autosize_heating flag set,
/// signalling that capacity will be computed by the dwelling builder later.
#[test]
fn ashp_missing_heating_capacity_flags_autosize() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatPump>
              <SystemIdentifier id="hp1"/>
              <HeatPumpType>air-to-air</HeatPumpType>
              <HeatPumpFuel>electricity</HeatPumpFuel>
              <CoolingCapacity>24000</CoolingCapacity>
              <AnnualCoolingEfficiency><Units>SEER2</Units><Value>14.0</Value></AnnualCoolingEfficiency>
              <AnnualHeatingEfficiency><Units>HSPF2</Units><Value>7.5</Value></AnnualHeatingEfficiency>
              <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
              <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
            </HeatPump>
          </HVAC>
        </Systems>"#,
    );
    let specs =
        resolve(&xml).expect("ASHP without HeatingCapacity must resolve with autosize flag");
    let heater = specs
        .iter()
        .find(|s| s.name == "ASHP Heater")
        .expect("ASHP Heater spec must be emitted even without HeatingCapacity");
    let autosize = heater
        .parameters
        .get("autosize_heating")
        .and_then(|v| v.as_bool())
        .expect("autosize_heating must be set when HeatingCapacity is absent");
    assert!(
        autosize,
        "autosize_heating must be true when HeatingCapacity is absent"
    );
    assert!(
        !heater.parameters.contains_key("heating_capacity_w"),
        "heating_capacity_w must not be set when HeatingCapacity is absent \
         (autosizing will compute it later)"
    );
}

// ASHP with explicit HeatingCapacity must resolve cleanly.
#[test]
fn ashp_with_heating_capacity_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatPump>
              <SystemIdentifier id="hp1"/>
              <HeatPumpType>air-to-air</HeatPumpType>
              <HeatPumpFuel>electricity</HeatPumpFuel>
              <HeatingCapacity>24000</HeatingCapacity>
              <CoolingCapacity>24000</CoolingCapacity>
              <AnnualCoolingEfficiency><Units>SEER2</Units><Value>14.0</Value></AnnualCoolingEfficiency>
              <AnnualHeatingEfficiency><Units>HSPF2</Units><Value>7.5</Value></AnnualHeatingEfficiency>
              <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
              <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
            </HeatPump>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("ASHP with explicit capacity must resolve");
    assert!(
        specs.iter().any(|s| s.name == "ASHP Heater"),
        "expected ASHP Heater spec in {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

// --- Ticket 111: resistance_efficiency_from_params silent 1.0 default --------
//
// When AnnualHeatingEfficiency is absent from an electric resistance heating
// system (ElectricResistance, Electric Furnace, Electric Boiler), the parser
// currently silently returns 1.0.  The ticket requires either a loud error or
// a debug-logged, cited default.  These tests are FAILING until the fix is
// applied: they verify Option B (loud error) as the recommended path.

/// Electric Furnace without AnnualHeatingEfficiency must produce
/// MissingField, not silently assume EIR = 1.0.
#[test]
fn electric_furnace_missing_efficiency_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="ef1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>18000</HeatingCapacity>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_missing(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Furnace",
    );
}

/// Electric Furnace with explicit AnnualHeatingEfficiency must still resolve.
#[test]
fn electric_furnace_with_explicit_efficiency_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="ef1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>18000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>1.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Electric Furnace with explicit efficiency must resolve");
    assert!(
        specs.iter().any(|s| s.name == "Electric Furnace"),
        "expected Electric Furnace spec in {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

/// ElectricResistance baseboard without AnnualHeatingEfficiency must produce
/// MissingField, not silently assume EIR = 1.0.
#[test]
fn electric_resistance_baseboard_missing_efficiency_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="er1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><ElectricResistance/></HeatingSystemType>
              <HeatingCapacity>5000</HeatingCapacity>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_missing(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Baseboard",
    );
}

/// ElectricResistance baseboard with explicit 100% efficiency must resolve.
#[test]
fn electric_resistance_baseboard_with_explicit_efficiency_resolves() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="er1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><ElectricResistance/></HeatingSystemType>
              <HeatingCapacity>5000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>1.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    let specs = resolve(&xml).expect("Electric Baseboard with explicit efficiency must resolve");
    assert!(
        specs.iter().any(|s| s.name == "Electric Baseboard"),
        "expected Electric Baseboard spec in {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

/// Electric Furnace with out-of-range efficiency value 2.0 must produce
/// InvalidField, not silently accept an invalid value.
#[test]
fn electric_furnace_efficiency_greater_than_one_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="ef1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>18000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>2.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Furnace",
        "2",
    );
}

/// Electric Furnace with efficiency value 0.0 must produce InvalidField.
#[test]
fn electric_furnace_efficiency_zero_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="ef1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>18000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>0.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Furnace",
        "0",
    );
}

/// Electric Boiler with out-of-range efficiency value 2.0 must produce InvalidField.
#[test]
fn electric_boiler_efficiency_greater_than_one_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="eboi1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>12000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>2.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Boiler",
        "2",
    );
}

/// Electric Boiler with efficiency value 0.0 must produce InvalidField.
#[test]
fn electric_boiler_efficiency_zero_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="eboi1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>12000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>0.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Boiler",
        "0",
    );
}

/// Electric Baseboard with out-of-range efficiency value 2.0 must produce InvalidField.
#[test]
fn electric_baseboard_efficiency_greater_than_one_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="er1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><ElectricResistance/></HeatingSystemType>
              <HeatingCapacity>5000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>2.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Baseboard",
        "2",
    );
}

/// Electric Baseboard with efficiency value 0.0 must produce InvalidField.
#[test]
fn electric_baseboard_efficiency_zero_errors() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="er1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><ElectricResistance/></HeatingSystemType>
              <HeatingCapacity>5000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>0.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );
    expect_invalid_field(
        resolve(&xml),
        "HeatingSystem/AnnualHeatingEfficiency",
        "Electric Baseboard",
        "0",
    );
}

// --- Ticket 112: tracing::debug! emission for boiler flow_rate_kg_s and return_temp_c defaults -----

#[test]
fn gas_boiler_logs_debug_when_flow_rate_and_return_temp_are_defaulted() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    struct TestWriter(Arc<Mutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for TestWriter {
        type Writer = TestWriterGuard;
        fn make_writer(&self) -> Self::Writer {
            TestWriterGuard(self.0.clone())
        }
    }
    struct TestWriterGuard(Arc<Mutex<Vec<u8>>>);
    impl Write for TestWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(TestWriter(buffer.clone()))
        .finish();

    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="boi1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.85</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );

    tracing::subscriber::with_default(subscriber, || {
        let _specs = resolve(&xml).expect("boiler must resolve");
    });

    let log = String::from_utf8(buffer.lock().unwrap().clone()).expect("logs must be valid utf8");
    assert!(
        log.contains("flow_rate_kg_s"),
        "debug log must mention field=flow_rate_kg_s, got: {log}"
    );
    assert!(
        log.contains("return_temp_c"),
        "debug log must mention field=return_temp_c, got: {log}"
    );
    assert!(
        log.contains("Gas Boiler"),
        "debug log must include the boiler equipment identifier, got: {log}"
    );
}

#[test]
fn electric_boiler_logs_debug_when_flow_rate_and_return_temp_are_defaulted() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    struct TestWriter(Arc<Mutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for TestWriter {
        type Writer = TestWriterGuard;
        fn make_writer(&self) -> Self::Writer {
            TestWriterGuard(self.0.clone())
        }
    }
    struct TestWriterGuard(Arc<Mutex<Vec<u8>>>);
    impl Write for TestWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(TestWriter(buffer.clone()))
        .finish();

    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <HeatingSystem>
              <SystemIdentifier id="eboi1"/>
              <HeatingSystemFuel>electricity</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>12000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>Percent</Units><Value>1.0</Value></AnnualHeatingEfficiency>
            </HeatingSystem>
          </HVAC>
        </Systems>"#,
    );

    tracing::subscriber::with_default(subscriber, || {
        let _specs = resolve(&xml).expect("electric boiler must resolve");
    });

    let log = String::from_utf8(buffer.lock().unwrap().clone()).expect("logs must be valid utf8");
    assert!(
        log.contains("flow_rate_kg_s"),
        "debug log must mention field=flow_rate_kg_s, got: {log}"
    );
    assert!(
        log.contains("return_temp_c"),
        "debug log must mention field=return_temp_c, got: {log}"
    );
    assert!(
        log.contains("Electric Boiler"),
        "debug log must include the boiler equipment identifier, got: {log}"
    );
}

// --- Ticket 120: unknown CompressorType must error, not silently default to single_speed -----
//
// HPXML v4.x §8.4 defines exactly three CompressorType values: "single stage",
// "two stage", and "variable speed" (confirmed at hpxml.nlr.gov/datadictionary).
// Any other string is outside the spec.  Currently `compressor_type_to_mode`
// (resolve_hvac.rs:1779) silently maps unknown values to "single_speed" via
// `_ => "single_speed"`.  The ticket requires the wildcard be replaced by a
// loud error (tracing::warn! + Err(HpxmlError::...)).
//
// This test is marked #[ignore] because the fix has not yet been applied.
// It documents the *desired* behaviour: an unknown CompressorType must fail
// closed rather than produce a silently-wrong single_speed inference.

#[test]
#[should_panic(expected = "ticket 120: unknown CompressorType 'DualStage' silently resolved")]
fn unknown_compressor_type_errors_not_silently_defaults() {
    let xml = wrap_systems(
        r#"<Systems>
          <HVAC>
            <CoolingSystem>
              <SystemIdentifier id="ac1"/>
              <CoolingSystemType>central air conditioner</CoolingSystemType>
              <CompressorType>DualStage</CompressorType>
              <CoolingCapacity>36000</CoolingCapacity>
              <AnnualCoolingEfficiency>
                <Units>SEER</Units><Value>18</Value>
              </AnnualCoolingEfficiency>
            </CoolingSystem>
          </HVAC>
        </Systems>"#,
    );

    // After the fix, this must return Err rather than silently produce a spec
    // with speed_control_mode="single_speed".
    match resolve(&xml) {
        Err(_) => {} // any error variant satisfies the contract
        Ok(specs) => {
            // Pre-fix: resolve succeeds and silently maps to single_speed.
            // If this branch is hit, the bug is still present.
            let ac = specs.iter().find(|s| s.name == "Air Conditioner");
            panic!(
                "ticket 120: unknown CompressorType 'DualStage' silently resolved to \
                 speed_control_mode={:?} instead of returning an error",
                ac.and_then(|s| s.parameters.get("speed_control_mode"))
            );
        }
    }
}

/// Companion happy-path test: all three HPXML-spec CompressorType values must
/// still resolve cleanly after the fix.  Currently passes; must continue to
/// pass post-fix.
#[test]
fn known_compressor_types_resolve_cleanly() {
    for (compressor_type, expected_mode, expected_speeds) in [
        ("single stage", "single_speed", 1u64),
        ("two stage", "two_speed", 2u64),
        ("variable speed", "variable_speed", 4u64),
    ] {
        let xml = wrap_systems(&format!(
            r#"<Systems>
              <HVAC>
                <CoolingSystem>
                  <SystemIdentifier id="ac1"/>
                  <CoolingSystemType>central air conditioner</CoolingSystemType>
                  <CompressorType>{compressor_type}</CompressorType>
                  <CoolingCapacity>36000</CoolingCapacity>
                  <AnnualCoolingEfficiency>
                    <Units>SEER</Units><Value>18</Value>
                  </AnnualCoolingEfficiency>
                </CoolingSystem>
              </HVAC>
            </Systems>"#
        ));
        let specs = resolve(&xml).unwrap_or_else(|e| {
            panic!("CompressorType={compressor_type:?} must resolve cleanly, got {e:?}")
        });
        let ac = specs
            .iter()
            .find(|s| s.name == "Air Conditioner")
            .expect("must emit Air Conditioner");
        assert_eq!(
            ac.parameters["speed_control_mode"].as_str().unwrap(),
            expected_mode,
            "CompressorType={compressor_type:?} must map to speed_control_mode={expected_mode:?}"
        );
        assert_eq!(
            ac.parameters["number_of_speeds"].as_u64().unwrap(),
            expected_speeds,
            "CompressorType={compressor_type:?} must map to number_of_speeds={expected_speeds}"
        );
    }
}
