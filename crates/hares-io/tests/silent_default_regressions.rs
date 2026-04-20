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
