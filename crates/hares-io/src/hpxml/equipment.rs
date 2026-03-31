//! HPXML equipment resolution into canonical OCHRE-style equipment specs.

use serde_json::{Map, Value, json};

use hares_types::FuelType;

use super::HpxmlError;
use super::building::Building;

use crate::defaults::{DefaultsStore, ZipParameters};

use super::resolve_der::{resolve_batteries, resolve_ev, resolve_generators, resolve_pv};
use super::resolve_hvac::resolve_hvac;
use super::resolve_loads::{default_gain_fractions, resolve_scheduled_loads, resolve_ventilation};
use super::resolve_water_heater::resolve_water_heaters;

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentSpec {
    pub name: String,
    pub fuel_type: FuelType,
    pub parameters: Map<String, Value>,
    pub zip_params: Option<ZipParameters>,
    /// Typed config, populated for HVAC equipment types that have been migrated.
    /// When present, consumers should prefer this over raw `parameters`.
    pub typed_config: Option<hares_equipment::EquipmentConfig>,
}

pub fn resolve_equipment(
    building: &Building,
    defaults: &DefaultsStore,
    overrides: &Value,
) -> std::result::Result<Vec<EquipmentSpec>, HpxmlError> {
    let mut specs = Vec::new();
    let details = &building.details_xml;

    resolve_hvac(building, defaults, &mut specs)?;
    resolve_water_heaters(details, defaults, &mut specs)?;
    resolve_pv(details, defaults, &mut specs);
    resolve_batteries(details, defaults, &mut specs);
    resolve_ev(details, defaults, &mut specs);
    resolve_generators(details, defaults, &mut specs);
    resolve_scheduled_loads(building, defaults, &mut specs);
    resolve_ventilation(details, defaults, &mut specs);

    apply_overrides(&mut specs, overrides);
    Ok(specs)
}

pub fn nested_update(base: &mut Map<String, Value>, overrides: &Map<String, Value>) {
    for (key, override_value) in overrides {
        match (base.get_mut(key), override_value) {
            (Some(Value::Object(base_obj)), Value::Object(override_obj)) => {
                nested_update(base_obj, override_obj);
            }
            _ => {
                base.insert(key.clone(), override_value.clone());
            }
        }
    }
}

fn apply_overrides(specs: &mut [EquipmentSpec], overrides: &Value) {
    let Value::Object(root) = overrides else {
        return;
    };

    let global = root
        .get("all")
        .or_else(|| root.get("*"))
        .and_then(Value::as_object);

    for spec in specs {
        if let Some(global_obj) = global {
            nested_update(&mut spec.parameters, global_obj);
        }

        if let Some(Value::Object(eq_obj)) = root.get(&spec.name) {
            nested_update(&mut spec.parameters, eq_obj);
        }
    }
}

pub(super) fn build_spec(
    name: String,
    fuel_type: FuelType,
    mut parameters: Map<String, Value>,
    defaults: &DefaultsStore,
) -> EquipmentSpec {
    parameters.insert(
        "fuel_type".to_string(),
        Value::String(format!("{fuel_type:?}")),
    );

    // Inject OCHRE-compatible default gain fractions when HPXML did not provide them.
    if !parameters.contains_key("frac_sensible")
        && !parameters.contains_key("sensible_gain_fraction")
    {
        if let Some((sensible, latent)) = default_gain_fractions(&name, fuel_type) {
            parameters.insert("sensible_gain_fraction".to_string(), json!(sensible));
            if !parameters.contains_key("frac_latent")
                && !parameters.contains_key("latent_gain_fraction")
            {
                parameters.insert("latent_gain_fraction".to_string(), json!(latent));
            }
        }
    }

    let zip_params = defaults.zip_params(&name).cloned();

    EquipmentSpec {
        name,
        fuel_type,
        parameters,
        zip_params,
        typed_config: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use hares_physics::units as conv;

    use super::{nested_update, resolve_equipment};
    use crate::defaults::DefaultsStore;
    use crate::hpxml::building::parse_building;

    #[test]
    fn nested_update_merges_objects_without_clobbering_siblings() {
        let mut base = Map::new();
        base.insert(
            "a".to_string(),
            json!({"b": 1, "c": 2, "deep": {"x": 3, "y": 4}}),
        );
        base.insert("z".to_string(), json!(10));

        let mut over = Map::new();
        over.insert("a".to_string(), json!({"b": 99, "deep": {"x": 42}}));
        over.insert("z".to_string(), json!(15));

        nested_update(&mut base, &over);

        assert_eq!(
            base.get("a")
                .and_then(Value::as_object)
                .and_then(|o| o.get("c")),
            Some(&json!(2))
        );
        assert_eq!(
            base.get("a")
                .and_then(Value::as_object)
                .and_then(|o| o.get("b")),
            Some(&json!(99))
        );
        assert_eq!(
            base.get("a")
                .and_then(Value::as_object)
                .and_then(|o| o.get("deep"))
                .and_then(Value::as_object)
                .and_then(|o| o.get("y")),
            Some(&json!(4))
        );
        assert_eq!(base.get("z"), Some(&json!(15)));
    }

    #[test]
    fn heat_pump_air_to_air_splits_to_ashp_heater_and_cooler() {
        let xml = r#"
<HPXML xmlns=\"http://hpxmlonline.com/2019/10\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://hpxmlonline.com/2019/10\" schemaVersion=\"4.0\">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <HeatingCapacity>24000</HeatingCapacity>
            <CoolingCapacity>24000</CoolingCapacity>
          </HeatPump>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>
"#;

        let building = parse_building(xml).expect("building should parse");
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");

        assert!(resolved.iter().any(|s| s.name == "ASHP Heater"));
        assert!(resolved.iter().any(|s| s.name == "ASHP Cooler"));
    }

    #[test]
    fn heat_pump_mini_split_splits_to_mshp_heater_and_cooler() {
        let xml = r#"
<HPXML xmlns=\"http://hpxmlonline.com/2019/10\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://hpxmlonline.com/2019/10\" schemaVersion=\"4.0\">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatPump>
            <HeatPumpType>mini-split</HeatPumpType>
            <HeatingCapacity>24000</HeatingCapacity>
            <CoolingCapacity>24000</CoolingCapacity>
          </HeatPump>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>
"#;

        let building = parse_building(xml).expect("building should parse");
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");

        assert!(resolved.iter().any(|s| s.name == "MSHP Heater"));
        assert!(resolved.iter().any(|s| s.name == "MSHP Cooler"));
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

    fn minimal_hvac_xml(hvac_inner: &str) -> String {
        format!(
            r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems><HVAC>{hvac_inner}</HVAC></Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
        )
    }

    #[test]
    fn heat_pump_variable_speed_sets_number_of_speeds_four() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>air-to-air</HeatPumpType>
          <CompressorType>variable speed</CompressorType>
          <CoolingCapacity>24000</CoolingCapacity>
          <HeatingCapacity>24000</HeatingCapacity>
          <SEER>22</SEER>
          <HSPF>10</HSPF>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");
        assert_eq!(cooler.parameters.get("number_of_speeds"), Some(&json!(4)));
    }

    #[test]
    fn heat_pump_single_stage_sets_number_of_speeds_one() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>air-to-air</HeatPumpType>
          <CompressorType>single stage</CompressorType>
          <CoolingCapacity>24000</CoolingCapacity>
          <HeatingCapacity>24000</HeatingCapacity>
          <SEER>14</SEER>
          <HSPF>8</HSPF>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");
        assert_eq!(cooler.parameters.get("number_of_speeds"), Some(&json!(1)));
    }

    #[test]
    fn heat_pump_two_stage_sets_number_of_speeds_two() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>air-to-air</HeatPumpType>
          <CompressorType>two stage</CompressorType>
          <CoolingCapacity>24000</CoolingCapacity>
          <HeatingCapacity>24000</HeatingCapacity>
          <SEER>16</SEER>
          <HSPF>9</HSPF>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");
        assert_eq!(cooler.parameters.get("number_of_speeds"), Some(&json!(2)));
        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater");
        assert_eq!(heater.parameters.get("number_of_speeds"), Some(&json!(2)));
    }

    #[test]
    fn cooling_system_seer_fallback_sets_two_speeds_at_seer_18() {
        let xml = minimal_hvac_xml(
            r#"<CoolingSystem>
          <CoolingSystemType>central air conditioner</CoolingSystemType>
          <CoolingCapacity>24000</CoolingCapacity>
          <SEER>18</SEER>
        </CoolingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let ac = specs
            .iter()
            .find(|s| s.name == "Air Conditioner")
            .expect("Air Conditioner");
        assert_eq!(ac.parameters.get("number_of_speeds"), Some(&json!(2)));
    }

    #[test]
    fn cooling_system_seer_fallback_sets_four_speeds_above_21() {
        let xml = minimal_hvac_xml(
            r#"<CoolingSystem>
          <CoolingSystemType>central air conditioner</CoolingSystemType>
          <CoolingCapacity>24000</CoolingCapacity>
          <SEER>22</SEER>
        </CoolingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let ac = specs
            .iter()
            .find(|s| s.name == "Air Conditioner")
            .expect("Air Conditioner");
        assert_eq!(ac.parameters.get("number_of_speeds"), Some(&json!(4)));
    }

    #[test]
    fn cooling_system_seer_fallback_sets_one_speed_at_or_below_15() {
        let xml = minimal_hvac_xml(
            r#"<CoolingSystem>
          <CoolingSystemType>central air conditioner</CoolingSystemType>
          <CoolingCapacity>24000</CoolingCapacity>
          <SEER>14</SEER>
        </CoolingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let ac = specs
            .iter()
            .find(|s| s.name == "Air Conditioner")
            .expect("Air Conditioner");
        assert_eq!(ac.parameters.get("number_of_speeds"), Some(&json!(1)));
    }

    #[test]
    fn multispeed_rows_inject_stage_capacities_and_eir_for_ashp_cooler() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>air-to-air</HeatPumpType>
          <CompressorType>variable speed</CompressorType>
          <CoolingCapacity>24000</CoolingCapacity>
          <HeatingCapacity>24000</HeatingCapacity>
          <SEER>22</SEER>
          <HSPF>10</HSPF>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs =
            resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");

        for idx in 0..4 {
            assert!(
                cooler
                    .parameters
                    .contains_key(&format!("cooling_capacity_w_stage_{idx}")),
                "missing stage capacity key for stage {idx}"
            );
            assert!(
                cooler
                    .parameters
                    .contains_key(&format!("cooling_eir_stage_{idx}")),
                "missing stage EIR key for stage {idx}"
            );
        }
    }

    fn minimal_wh_xml(systems_inner: &str) -> String {
        format!(
            r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>{systems_inner}</Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
        )
    }

    /// Round-trip: HPXML with EF=0.92 electric 50-gal → ua_w_per_k ≈ 1.1636 W/K.
    #[test]
    fn electric_wh_ef_produces_ua_w_per_k_in_params() {
        let xml = minimal_wh_xml(
            r#"<WaterHeating>
          <WaterHeatingSystem>
            <FuelType>electricity</FuelType>
            <WaterHeaterType>storage water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <EnergyFactor>0.92</EnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let wh = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("WH spec must be present");

        let ua = wh
            .parameters
            .get("ua_w_per_k")
            .and_then(Value::as_f64)
            .expect("ua_w_per_k must be present in params");

        // Expected: ~1.1636 W/K (verified against OCHRE output)
        assert!(
            (ua - 1.163_568).abs() < 0.01,
            "ua_w_per_k={ua:.4}, expected≈1.1636"
        );
    }

    /// Round-trip: HPXML with EF=0.59 gas 50-gal, 40 kBtu/hr → ua_w_per_k ≈ 4.646 W/K.
    #[test]
    fn gas_wh_ef_produces_ua_w_per_k_in_params() {
        let xml = minimal_wh_xml(
            r#"<WaterHeating>
          <WaterHeatingSystem>
            <FuelType>natural gas</FuelType>
            <WaterHeaterType>storage water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <EnergyFactor>0.59</EnergyFactor>
            <RecoveryEfficiency>0.78</RecoveryEfficiency>
            <HeatingCapacity>40000</HeatingCapacity>
          </WaterHeatingSystem>
        </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let wh = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("WH spec must be present");

        let ua = wh
            .parameters
            .get("ua_w_per_k")
            .and_then(Value::as_f64)
            .expect("ua_w_per_k must be present for gas WH with EF and HeatingCapacity");

        // Expected: ~4.646 W/K (verified against OCHRE output)
        assert!(
            (ua - 4.646_228).abs() < 0.01,
            "ua_w_per_k={ua:.4}, expected≈4.646"
        );
    }

    /// Round-trip: HPXML HPWH with UEF=3.45, 50-gal → ua_w_per_k from volume bin.
    #[test]
    fn hpwh_uef_produces_ua_w_per_k_from_volume_bin() {
        let xml = minimal_wh_xml(
            r#"<WaterHeating>
          <WaterHeatingSystem>
            <FuelType>electricity</FuelType>
            <WaterHeaterType>heat pump water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <UniformEnergyFactor>3.45</UniformEnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let wh = specs
            .iter()
            .find(|s| s.name == "Heat Pump Water Heater")
            .expect("WH spec must be present");

        let ua = wh
            .parameters
            .get("ua_w_per_k")
            .and_then(Value::as_f64)
            .expect("ua_w_per_k must be present for HPWH with TankVolume");

        // 50 gal × 0.9 = 45 gal → bin ≤ 58 gal → 3.6 Btu/hr·°F → 1.899 W/K
        let expected = conv::btu_hr_per_f_to_w_per_k(3.6);
        assert!(
            (ua - expected).abs() < 1e-6,
            "HPWH ua_w_per_k={ua:.4}, expected {expected:.4}"
        );
    }

    /// HPXML Battery: RatedPowerOutput maps to both max_charge_kw and max_discharge_kw.
    #[test]
    fn hpxml_battery_rated_power_maps_to_charge_discharge_keys() {
        let xml = minimal_wh_xml(
            r#"<Batteries>
          <Battery>
            <NominalCapacity><Value>10</Value><Units>kWh</Units></NominalCapacity>
            <RatedPowerOutput>5</RatedPowerOutput>
          </Battery>
        </Batteries>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let battery = specs
            .iter()
            .find(|s| s.name == "Battery")
            .expect("Battery spec must be present");

        let charge_kw = battery
            .parameters
            .get("max_charge_kw")
            .and_then(Value::as_f64)
            .expect("max_charge_kw must be present");
        let discharge_kw = battery
            .parameters
            .get("max_discharge_kw")
            .and_then(Value::as_f64)
            .expect("max_discharge_kw must be present");

        assert!(
            (charge_kw - 5.0).abs() < 1e-9,
            "max_charge_kw={charge_kw}, expected 5.0"
        );
        assert!(
            (discharge_kw - 5.0).abs() < 1e-9,
            "max_discharge_kw={discharge_kw}, expected 5.0"
        );
    }

    /// HPXML Battery: RoundTripEfficiency of 0.90 → inverter_efficiency stored as raw RTE.
    /// Battery::init applies sqrt() per direction internally; the resolver must NOT pre-apply it.
    #[test]
    fn hpxml_battery_rte_converts_to_inverter_efficiency() {
        let rte = 0.90_f64;
        let xml = minimal_wh_xml(&format!(
            r#"<Batteries>
          <Battery>
            <NominalCapacity><Value>10</Value><Units>kWh</Units></NominalCapacity>
            <RoundTripEfficiency>{rte}</RoundTripEfficiency>
          </Battery>
        </Batteries>"#
        ));
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let battery = specs
            .iter()
            .find(|s| s.name == "Battery")
            .expect("Battery spec must be present");

        let inv_eff = battery
            .parameters
            .get("inverter_efficiency")
            .and_then(Value::as_f64)
            .expect("inverter_efficiency must be present");

        // Resolver stores raw RTE; battery applies sqrt() per direction, yielding sqrt(rte) each way.
        assert!(
            (inv_eff - rte).abs() < 1e-9,
            "inverter_efficiency={inv_eff:.6}, expected raw rte={rte:.6}"
        );
    }

    /// HPXML ElectricVehicle: resolve_ev emits ChargingLevel, MaxChargingPower, BatteryCapacity.
    #[test]
    fn hpxml_ev_keys_emitted_with_correct_names() {
        let xml = minimal_wh_xml(
            r#"<ElectricVehicles>
          <ElectricVehicle>
            <ChargingLevel>Level 2</ChargingLevel>
            <MaxChargingPower>7.2</MaxChargingPower>
            <BatteryCapacity><Value>60</Value><Units>kWh</Units></BatteryCapacity>
          </ElectricVehicle>
        </ElectricVehicles>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let ev = specs
            .iter()
            .find(|s| s.name == "Electric Vehicle")
            .expect("Electric Vehicle spec must be present");

        assert_eq!(
            ev.parameters.get("ChargingLevel").and_then(Value::as_str),
            Some("Level 2"),
            "ChargingLevel key must be present with correct value"
        );
        let max_power = ev
            .parameters
            .get("MaxChargingPower")
            .and_then(Value::as_f64)
            .expect("MaxChargingPower must be present");
        assert!(
            (max_power - 7.2).abs() < 1e-9,
            "MaxChargingPower={max_power}, expected 7.2"
        );
        let capacity = ev
            .parameters
            .get("BatteryCapacity")
            .and_then(Value::as_f64)
            .expect("BatteryCapacity must be present");
        assert!(
            (capacity - 60.0).abs() < 1e-9,
            "BatteryCapacity={capacity}, expected 60.0"
        );
    }

    /// Gas WH without HeatingCapacity: ua_w_per_k must not be inserted
    /// (the UA formula requires capacity; without it we leave the default).
    #[test]
    fn gas_wh_without_capacity_omits_ua_w_per_k() {
        let xml = minimal_wh_xml(
            r#"<WaterHeating>
          <WaterHeatingSystem>
            <FuelType>natural gas</FuelType>
            <WaterHeaterType>storage water heater</WaterHeaterType>
            <TankVolume>50</TankVolume>
            <EnergyFactor>0.59</EnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let wh = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("WH spec must be present");

        // Without HeatingCapacity the gas EF→UA formula cannot be evaluated;
        // ua_w_per_k must be absent so the equipment model uses its built-in default.
        assert!(
            !wh.parameters.contains_key("ua_w_per_k"),
            "gas WH without capacity must not produce ua_w_per_k"
        );
    }

    fn hvac_with_ducts_xml(hvac_inner: &str) -> String {
        format!(
            r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1500</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">12000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Attics><Attic><FloorArea units="ft2">500</FloorArea></Attic></Attics>
      </Enclosure>
      <Systems>
        <HVAC>
          <HVACDistribution>
            <DuctSystem>
              <SystemIdentifier id="Duct1"/>
              <LeakageFraction>0.10</LeakageFraction>
              <DuctInsulationRValue units="hr-ft2-F/BTU">6</DuctInsulationRValue>
              <DuctSurfaceArea units="ft2">150</DuctSurfaceArea>
              <DuctLocation>attic vented</DuctLocation>
            </DuctSystem>
          </HVACDistribution>
          {hvac_inner}
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
        )
    }

    #[test]
    fn duct_raw_params_injected_into_furnace_from_attic_ducts() {
        let xml = hvac_with_ducts_xml(
            r#"<HeatingSystem>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace spec must be present");

        assert!(
            furnace.parameters.contains_key("duct_zone_id"),
            "duct_zone_id must be set for non-conditioned duct location"
        );
        assert!(
            furnace.parameters.contains_key("duct_zone_type"),
            "duct_zone_type must be set for ASHRAE 152 computation"
        );
        assert!(
            furnace.parameters.contains_key("duct_supply_leakage_frac"),
            "raw duct supply leakage must be passed through"
        );
    }

    #[test]
    fn duct_raw_params_injected_into_ashp_from_attic_ducts() {
        let xml = hvac_with_ducts_xml(
            r#"<HeatPump>
              <HeatPumpType>air-to-air</HeatPumpType>
              <HeatingCapacity>36000</HeatingCapacity>
              <CoolingCapacity>36000</CoolingCapacity>
            </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");

        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater");
        assert!(
            heater.parameters.contains_key("duct_zone_type"),
            "ASHP Heater must have duct_zone_type for ASHRAE 152"
        );

        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");
        assert!(
            cooler.parameters.contains_key("duct_zone_type"),
            "ASHP Cooler must also have duct_zone_type"
        );
    }

    #[test]
    fn mshp_does_not_get_duct_dse() {
        let xml = hvac_with_ducts_xml(
            r#"<HeatPump>
              <HeatPumpType>mini-split</HeatPumpType>
              <HeatingCapacity>24000</HeatingCapacity>
              <CoolingCapacity>24000</CoolingCapacity>
            </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");

        let heater = specs
            .iter()
            .find(|s| s.name == "MSHP Heater")
            .expect("MSHP Heater");
        assert!(
            !heater.parameters.contains_key("duct_dse"),
            "mini-split must not have duct_dse"
        );
    }

    fn minimal_appliance_xml(appliance_inner: &str) -> String {
        format!(
            r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Appliances>{appliance_inner}</Appliances>
    </BuildingDetails>
  </Building>
</HPXML>"#
        )
    }

    fn find_spec<'a>(specs: &'a [super::EquipmentSpec], name: &str) -> &'a super::EquipmentSpec {
        specs
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("spec '{name}' not found"))
    }

    #[test]
    fn clothes_washer_gain_fractions_match_ochre() {
        let xml = minimal_appliance_xml("<ClothesWasher />");
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let cw = find_spec(&specs, "Clothes Washer");
        let sens = cw.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = cw.parameters["latent_gain_fraction"].as_f64().unwrap();
        assert!((sens - 0.27).abs() < 1e-9, "sensible={sens}, expected 0.27");
        assert!((lat - 0.03).abs() < 1e-9, "latent={lat}, expected 0.03");
    }

    #[test]
    fn dishwasher_gain_fractions_match_ochre() {
        let xml = minimal_appliance_xml("<Dishwasher />");
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dw = find_spec(&specs, "Dishwasher");
        let sens = dw.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = dw.parameters["latent_gain_fraction"].as_f64().unwrap();
        assert!((sens - 0.30).abs() < 1e-9, "sensible={sens}, expected 0.30");
        assert!((lat - 0.30).abs() < 1e-9, "latent={lat}, expected 0.30");
    }

    #[test]
    fn vented_electric_dryer_gain_fractions() {
        let xml = minimal_appliance_xml(
            "<ClothesDryer><FuelType>electricity</FuelType><Vented>true</Vented></ClothesDryer>",
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dryer = find_spec(&specs, "Clothes Dryer");
        let sens = dryer.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = dryer.parameters["latent_gain_fraction"].as_f64().unwrap();
        // frac_lost=0.85, gain_factor=0.90 → sens=0.135, lat=0.015
        assert!(
            (sens - 0.135).abs() < 1e-9,
            "sensible={sens}, expected 0.135"
        );
        assert!((lat - 0.015).abs() < 1e-9, "latent={lat}, expected 0.015");
    }

    #[test]
    fn unvented_electric_dryer_gain_fractions() {
        let xml = minimal_appliance_xml(
            "<ClothesDryer><FuelType>electricity</FuelType><Vented>false</Vented></ClothesDryer>",
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dryer = find_spec(&specs, "Clothes Dryer");
        let sens = dryer.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = dryer.parameters["latent_gain_fraction"].as_f64().unwrap();
        // frac_lost=0.0, gain_factor=0.90 → sens=0.90, lat=0.10
        assert!((sens - 0.90).abs() < 1e-9, "sensible={sens}, expected 0.90");
        assert!((lat - 0.10).abs() < 1e-9, "latent={lat}, expected 0.10");
    }

    #[test]
    fn vented_gas_dryer_gain_fractions() {
        let xml = minimal_appliance_xml(
            "<ClothesDryer><FuelType>natural gas</FuelType><Vented>true</Vented></ClothesDryer>",
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dryer = find_spec(&specs, "Clothes Dryer");
        let sens = dryer.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = dryer.parameters["latent_gain_fraction"].as_f64().unwrap();
        // frac_lost=0.85, gain_factor=0.89 → sens=0.1335, lat=0.0165
        assert!(
            (sens - 0.1335).abs() < 1e-9,
            "sensible={sens}, expected 0.1335"
        );
        assert!((lat - 0.0165).abs() < 1e-9, "latent={lat}, expected 0.0165");
    }

    #[test]
    fn unvented_gas_dryer_gain_fractions() {
        let xml = minimal_appliance_xml(
            "<ClothesDryer><FuelType>natural gas</FuelType><Vented>false</Vented></ClothesDryer>",
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dryer = find_spec(&specs, "Clothes Dryer");
        let sens = dryer.parameters["sensible_gain_fraction"].as_f64().unwrap();
        let lat = dryer.parameters["latent_gain_fraction"].as_f64().unwrap();
        // frac_lost=0.0, gain_factor=0.89 → sens=0.89, lat=0.11
        assert!((sens - 0.89).abs() < 1e-9, "sensible={sens}, expected 0.89");
        assert!((lat - 0.11).abs() < 1e-9, "latent={lat}, expected 0.11");
    }

    #[test]
    fn dryer_defaults_to_vented_when_element_absent() {
        let xml =
            minimal_appliance_xml("<ClothesDryer><FuelType>electricity</FuelType></ClothesDryer>");
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).unwrap();
        let dryer = find_spec(&specs, "Clothes Dryer");
        let sens = dryer.parameters["sensible_gain_fraction"].as_f64().unwrap();
        // Defaults to vented (frac_lost=0.85) → sens=0.135
        assert!(
            (sens - 0.135).abs() < 1e-9,
            "should default to vented: sensible={sens}"
        );
    }

    #[test]
    fn no_duct_data_means_no_duct_dse_in_params() {
        let xml = minimal_hvac_xml(
            r#"<HeatingSystem>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace");
        assert!(
            !furnace.parameters.contains_key("duct_dse"),
            "without duct data, duct_dse must not be injected"
        );
    }

    #[test]
    fn room_ac_gets_setpoints_and_no_duct_dse() {
        let xml = hvac_with_ducts_xml(
            r#"<CoolingSystem>
              <CoolingSystemType>room air conditioner</CoolingSystemType>
              <CoolingCapacity>12000</CoolingCapacity>
              <SEER>10</SEER>
            </CoolingSystem>
            <HVACControl>
              <SetpointTempCoolingSeason>75</SetpointTempCoolingSeason>
            </HVACControl>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");

        let rac = specs
            .iter()
            .find(|s| s.name == "Room AC")
            .expect("Room Air Conditioner spec must be present");

        // Room AC must NOT get duct DSE params (no ducts for window units).
        assert!(
            !rac.parameters.contains_key("duct_zone_id"),
            "Room Air Conditioner must not have duct_zone_id"
        );
        assert!(
            !rac.parameters.contains_key("duct_zone_type"),
            "Room Air Conditioner must not have duct_zone_type"
        );

        // Room AC must get cooling setpoints injected.
        assert!(
            rac.parameters.contains_key("cooling_weekday_setpoints_c"),
            "Room Air Conditioner must receive cooling setpoints"
        );
        let setpoints = rac
            .parameters
            .get("cooling_weekday_setpoints_c")
            .and_then(Value::as_array)
            .expect("setpoints array");
        assert_eq!(setpoints.len(), 24, "setpoints must have 24 values");
        let expected_c = conv::temperature_f_to_c(75.0);
        assert!(
            (setpoints[0].as_f64().unwrap() - expected_c).abs() < 1e-6,
            "setpoint should be 75F converted to C"
        );
    }

    /// Both `building.rs::parse_hvac_setpoints` and `resolve_hvac.rs::parse_hvac_setpoint_params`
    /// use the same shared helper. Verify they produce identical output for the same input.
    #[test]
    fn both_setpoint_parsers_produce_same_output() {
        let xml = minimal_hvac_xml(
            r#"<HVACControl>
              <SetpointTempHeatingSeason>68</SetpointTempHeatingSeason>
              <SetpointTempCoolingSeason>75</SetpointTempCoolingSeason>
            </HVACControl>
            <HeatingSystem>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");

        // Path 1: setpoints parsed into the Building struct by building.rs
        let bldg_heating_wd = building
            .heating_weekday_setpoints_c
            .as_ref()
            .expect("building heating weekday setpoints");
        let bldg_cooling_wd = building
            .cooling_weekday_setpoints_c
            .as_ref()
            .expect("building cooling weekday setpoints");

        // Path 2: setpoints parsed by resolve_equipment → resolve_hvac → parse_hvac_setpoint_params
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace");

        let spec_heating_wd = furnace
            .parameters
            .get("heating_weekday_setpoints_c")
            .and_then(Value::as_array)
            .expect("spec heating setpoints");

        // Both paths should produce the same 24-element array.
        assert_eq!(bldg_heating_wd.len(), 24);
        assert_eq!(spec_heating_wd.len(), 24);
        for (i, (bldg_val, spec_val)) in bldg_heating_wd
            .iter()
            .zip(spec_heating_wd.iter())
            .enumerate()
        {
            let sv = spec_val.as_f64().unwrap();
            assert!(
                (bldg_val - sv).abs() < 1e-10,
                "heating setpoint mismatch at index {i}: building={bldg_val}, spec={sv}"
            );
        }

        // Verify the actual value: 68°F → °C
        let expected_c = conv::temperature_f_to_c(68.0);
        assert!(
            (bldg_heating_wd[0] - expected_c).abs() < 1e-6,
            "heating setpoint should be 68F converted to C"
        );
        let expected_cool = conv::temperature_f_to_c(75.0);
        assert!(
            (bldg_cooling_wd[0] - expected_cool).abs() < 1e-6,
            "cooling setpoint should be 75F converted to C"
        );
    }

    #[test]
    fn gas_furnace_hpxml_parse_produces_typed_config_with_correct_afue() {
        let xml = minimal_hvac_xml(
            r#"<HeatingSystem>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency>
                <Units>AFUE</Units>
                <Value>0.96</Value>
              </AnnualHeatingEfficiency>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let spec = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace spec must be present");

        let typed = spec
            .typed_config
            .as_ref()
            .expect("Gas Furnace must have a typed config after migration");

        assert!(
            typed.is_typed(),
            "typed_config payload must be Typed variant"
        );

        let cfg: hares_equipment::hvac::heating_config::GasFurnaceConfig =
            typed.typed().expect("must deserialize as GasFurnaceConfig");

        assert!(
            (cfg.afue - 0.96).abs() < 1e-12,
            "afue must be 0.96, got {}",
            cfg.afue
        );
        let expected_w = conv::power_btu_h_to_w(60_000.0);
        assert!(
            (cfg.capacity_w - expected_w).abs() < 1.0,
            "capacity_w must be ~{expected_w:.1} W, got {}",
            cfg.capacity_w
        );
    }

    #[test]
    fn central_ac_hpxml_parse_produces_typed_config_with_correct_seer() {
        let xml = minimal_hvac_xml(
            r#"<CoolingSystem>
              <CoolingSystemFuel>electricity</CoolingSystemFuel>
              <CoolingSystemType>central air conditioner</CoolingSystemType>
              <CoolingCapacity>36000</CoolingCapacity>
              <AnnualCoolingEfficiency>
                <Units>SEER</Units>
                <Value>16</Value>
              </AnnualCoolingEfficiency>
            </CoolingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let spec = specs
            .iter()
            .find(|s| s.name == "Air Conditioner")
            .expect("Air Conditioner spec must be present");

        let typed = spec
            .typed_config
            .as_ref()
            .expect("Air Conditioner must have a typed config after migration");

        assert!(
            typed.is_typed(),
            "typed_config payload must be Typed variant"
        );

        let cfg: hares_equipment::hvac::cooling_config::CentralAirConditionerConfig = typed
            .typed()
            .expect("must deserialize as CentralAirConditionerConfig");

        assert!(
            (cfg.seer - 16.0).abs() < 1e-12,
            "seer must be 16.0, got {}",
            cfg.seer
        );
        let expected_w = conv::power_btu_h_to_w(36_000.0);
        assert!(
            (cfg.capacity_w - expected_w).abs() < 1.0,
            "capacity_w must be ~{expected_w:.1} W, got {}",
            cfg.capacity_w
        );
    }

    /// HPXML Generator: gas generator with ElectricalPowerOutput and annual energy figures
    /// → rated_power_kw and eta_electric are emitted with correct values.
    #[test]
    fn hpxml_gas_generator_emits_capacity_and_efficiency() {
        // 10 kW output, 120 kBtu/h consumption at rated load.
        // Annual figures: 8760 kWh out, 105_120 kBtu in (≈ 8760 * 12 kBtu/h assumed)
        // eta = 8760 / (105_120 * 0.29307107) ≈ 0.2844
        let annual_out_kwh = 8_760.0_f64;
        let annual_cons_kbtu = 105_120.0_f64;
        let expected_eta = annual_out_kwh / (annual_cons_kbtu * 0.293_071_07);

        let xml = minimal_wh_xml(&format!(
            r#"<extension>
          <Generators>
            <Generator>
              <SystemIdentifier id="gen-1"/>
              <FuelType>natural gas</FuelType>
              <ElectricalPowerOutput>10.0</ElectricalPowerOutput>
              <AnnualOutputkWh>{annual_out_kwh}</AnnualOutputkWh>
              <AnnualConsumptionkBtu>{annual_cons_kbtu}</AnnualConsumptionkBtu>
            </Generator>
          </Generators>
        </extension>"#
        ));
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("resolve_equipment");
        let generator = specs
            .iter()
            .find(|s| s.name == "Gas Generator")
            .expect("Gas Generator spec must be present");

        let rated_power = generator
            .parameters
            .get("rated_power_kw")
            .and_then(Value::as_f64)
            .expect("rated_power_kw must be present");
        assert!(
            (rated_power - 10.0).abs() < 1e-9,
            "rated_power_kw={rated_power}, expected 10.0"
        );

        let eta = generator
            .parameters
            .get("eta_electric")
            .and_then(Value::as_f64)
            .expect("eta_electric must be present");
        assert!(
            (eta - expected_eta).abs() < 1e-6,
            "eta_electric={eta:.6}, expected {expected_eta:.6}"
        );
    }
}
