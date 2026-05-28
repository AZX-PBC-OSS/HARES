//! HPXML equipment resolution into canonical OCHRE-style equipment specs.

use serde_json::{Map, Value, json};

use hares_equipment::{EquipmentConfig, EquipmentTypedConfig};
use hares_types::FuelType;

use super::HpxmlError;
use super::data_patches::HpxmlDataPatches;

use super::building::Building;

use crate::defaults::{DefaultsStore, ZipParameters};

use super::resolve_der::{resolve_batteries, resolve_ev, resolve_generators, resolve_pv};
use super::resolve_hvac::resolve_hvac;
use super::resolve_loads::{default_gain_fractions, resolve_scheduled_loads, resolve_ventilation};
use super::resolve_water_heater::resolve_water_heaters;

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentSpec {
    /// Equipment class name — identifies the equipment type for defaults lookup
    /// and registry instantiation.
    pub name: String,
    /// Per-instance display name. When `None`, `name` is used for display.
    pub instance_name: Option<String>,
    pub fuel_type: FuelType,
    pub parameters: Map<String, Value>,
    pub zip_params: Option<ZipParameters>,
    /// Typed config, populated for equipment types that have been migrated.
    /// When present, consumers should prefer this over raw `parameters`.
    pub typed_config: Option<hares_equipment::EquipmentConfig>,
    /// HPXML SystemIdentifier/@id from the source element, for cross-referencing.
    pub system_id: Option<String>,
    /// HPXML RelatedHVACSystem/@idref from WaterHeatingSystem elements.
    pub related_hvac_idref: Option<String>,
    /// HPXML `<PrimarySystems>` designation: `"heating"` or `"cooling"` for the
    /// equipment designated as primary by `<PrimaryHeatingSystem>` or
    /// `<PrimaryCoolingSystem>`. `None` for non-primary equipment and for
    /// equipment types that do not participate in the primary-system mechanism
    /// (heat pumps, water heaters, PV, etc.).
    pub primary_role: Option<String>,
}

pub fn resolve_equipment(
    building: &Building,
    defaults: &DefaultsStore,
    overrides: &Value,
    data_patches: Option<&HpxmlDataPatches>,
) -> std::result::Result<Vec<EquipmentSpec>, HpxmlError> {
    let mut specs = Vec::new();
    let details = &building.details_xml;

    resolve_hvac(building, defaults, &mut specs)?;
    resolve_water_heaters(details, defaults, &mut specs, data_patches)?;
    resolve_pv(details, defaults, &mut specs)?;
    resolve_batteries(details, defaults, &mut specs)?;
    resolve_ev(details, defaults, &mut specs)?;
    resolve_generators(details, defaults, &mut specs)?;
    resolve_scheduled_loads(building, defaults, &mut specs);
    resolve_ventilation(details, defaults, &mut specs);

    apply_overrides(&mut specs, overrides);
    resolve_loop_wiring(&mut specs);
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
        Value::String(fuel_type_label(fuel_type)),
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
        instance_name: None,
        fuel_type,
        parameters,
        zip_params,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

pub(super) fn build_typed_spec<T>(
    name: String,
    fuel_type: FuelType,
    config: T,
    defaults: &DefaultsStore,
) -> EquipmentSpec
where
    T: EquipmentTypedConfig,
{
    let parameters = serde_json::to_value(&config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let typed_config = EquipmentConfig::from_typed(name.clone(), name.clone(), config);
    EquipmentSpec {
        name: name.clone(),
        instance_name: None,
        fuel_type,
        parameters,
        zip_params: defaults.zip_params(&name).cloned(),
        typed_config: Some(typed_config),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

fn fuel_type_label(fuel_type: FuelType) -> String {
    match fuel_type {
        FuelType::Electric => "electricity",
        FuelType::Gas => "natural gas",
        FuelType::Propane => "propane",
        FuelType::Oil => "fuel oil",
        FuelType::Wood => "wood",
        FuelType::Coal => "coal",
        FuelType::WoodPellet => "wood pellets",
        FuelType::None => "none",
    }
    .to_string()
}

/// Post-resolution wiring pass: assigns consistent fluid loop IDs across
/// cross-referenced equipment (boiler + indirect tank) based on HPXML
/// `RelatedHVACSystem` idrefs.
///
/// Without this pass, both boiler and indirect tank default to `LoopId(1)`
/// independently — the cross-reference is parsed but never applied.
fn resolve_loop_wiring(specs: &mut [EquipmentSpec]) {
    let mut next_loop_id: u16 = 1;

    // Build a lookup: HPXML SystemIdentifier/id → index for boiler specs.
    let boiler_indices: Vec<(String, usize)> = specs
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.name.as_str(), "Gas Boiler" | "Electric Boiler"))
        .filter_map(|(i, s)| s.system_id.clone().map(|id| (id, i)))
        .collect();

    // Collect wiring jobs to avoid borrow-checker friction.
    let mut wiring_jobs: Vec<(usize, usize, u16)> = Vec::new();
    for (tank_idx, tank_spec) in specs.iter().enumerate() {
        if tank_spec.name != "Indirect Tank" {
            continue;
        }
        let Some(ref related_hvac_idref) = tank_spec.related_hvac_idref else {
            continue;
        };
        let Some(&(_, boiler_idx)) = boiler_indices
            .iter()
            .find(|(id, _)| id == related_hvac_idref)
        else {
            tracing::warn!(
                related_hvac_idref = %related_hvac_idref,
                "IndirectTank references HVAC system but no matching boiler found in resolved specs"
            );
            continue;
        };
        let loop_id = next_loop_id;
        next_loop_id += 1;
        wiring_jobs.push((tank_idx, boiler_idx, loop_id));
    }

    // Apply wiring jobs.
    for (tank_idx, boiler_idx, loop_id) in wiring_jobs {
        set_boiler_loop_id(specs, boiler_idx, loop_id);
        set_indirect_tank_boiler_loop_id(specs, tank_idx, loop_id);
    }
}

fn set_boiler_loop_id(specs: &mut [EquipmentSpec], idx: usize, loop_id: u16) {
    use hares_equipment::{ElectricBoilerConfig, GasBoilerConfig};

    let Some(ref mut cfg) = specs[idx].typed_config else {
        return;
    };
    match specs[idx].name.as_str() {
        "Gas Boiler" => {
            if let Ok(mut typed) = cfg.typed::<GasBoilerConfig>() {
                typed.loop_id = Some(loop_id);
                *cfg =
                    EquipmentConfig::from_typed(cfg.name.clone(), cfg.ochre_class.clone(), typed);
            } else {
                tracing::warn!(
                    name = %specs[idx].name,
                    loop_id,
                    "set_boiler_loop_id: failed to deserialize GasBoilerConfig"
                );
            }
        }
        "Electric Boiler" => {
            if let Ok(mut typed) = cfg.typed::<ElectricBoilerConfig>() {
                typed.loop_id = Some(loop_id);
                *cfg =
                    EquipmentConfig::from_typed(cfg.name.clone(), cfg.ochre_class.clone(), typed);
            } else {
                tracing::warn!(
                    name = %specs[idx].name,
                    loop_id,
                    "set_boiler_loop_id: failed to deserialize ElectricBoilerConfig"
                );
            }
        }
        _ => {}
    }
}

fn set_indirect_tank_boiler_loop_id(specs: &mut [EquipmentSpec], idx: usize, loop_id: u16) {
    use hares_equipment::IndirectTankConfig;

    if let Some(ref mut cfg) = specs[idx].typed_config {
        if let Ok(mut typed) = cfg.typed::<IndirectTankConfig>() {
            typed.boiler_loop_id = Some(loop_id);
            *cfg = EquipmentConfig::from_typed(cfg.name.clone(), cfg.ochre_class.clone(), typed);
        } else {
            tracing::warn!(
                name = %specs[idx].name,
                loop_id,
                "set_indirect_tank_boiler_loop_id: failed to deserialize IndirectTankConfig"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use hares_physics::units as conv;
    use hares_types::{FuelType, ScheduleSourceConfig};

    use super::{HpxmlError, nested_update, resolve_equipment};
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
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
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
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
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
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({}), None)
            .expect("resolve_equipment");
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
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
            <HotWaterTemperature>125</HotWaterTemperature>
            <UniformEnergyFactor>3.45</UniformEnergyFactor>
          </WaterHeatingSystem>
        </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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

    /// HPXML Battery: RatedPowerOutput maps to the typed config power fields.
    #[test]
    fn hpxml_battery_rated_power_maps_to_charge_discharge_keys() {
        let xml = minimal_wh_xml(
            r#"<Batteries>
          <Battery>
            <NominalCapacity><Value>10</Value><Units>kWh</Units></NominalCapacity>
            <RatedPowerOutput>5000</RatedPowerOutput>
          </Battery>
        </Batteries>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let battery = specs
            .iter()
            .find(|s| s.name == "Battery")
            .expect("Battery spec must be present");

        let typed: hares_equipment::BatteryConfig = battery
            .typed_config
            .as_ref()
            .expect("Battery spec must carry typed config")
            .typed()
            .expect("Battery typed config");

        assert!(
            (typed.capacity_kwh - 10.0).abs() < 1e-9,
            "capacity_kwh={}, expected 10.0",
            typed.capacity_kwh
        );
        assert!(
            (typed.max_charge_kw - 5.0).abs() < 1e-9,
            "max_charge_kw={}, expected 5.0",
            typed.max_charge_kw
        );
        assert!(
            (typed.max_discharge_kw - 5.0).abs() < 1e-9,
            "max_discharge_kw={}, expected 5.0",
            typed.max_discharge_kw
        );
    }

    /// HPXML Battery: RoundTripEfficiency of 0.90 is converted to one-way efficiency.
    #[test]
    fn hpxml_battery_rte_converts_to_inverter_efficiency() {
        let rte = 0.90_f64;
        let xml = minimal_wh_xml(&format!(
            r#"<Batteries>
          <Battery>
            <NominalCapacity><Value>10</Value><Units>kWh</Units></NominalCapacity>
            <RatedPowerOutput>5000</RatedPowerOutput>
            <RoundTripEfficiency>{rte}</RoundTripEfficiency>
          </Battery>
        </Batteries>"#
        ));
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let battery = specs
            .iter()
            .find(|s| s.name == "Battery")
            .expect("Battery spec must be present");

        let typed: hares_equipment::BatteryConfig = battery
            .typed_config
            .as_ref()
            .expect("Battery spec must carry typed config")
            .typed()
            .expect("Battery typed config");
        let inv_eff = typed
            .inverter_efficiency
            .expect("typed battery inverter_efficiency must be present");

        // Resolver stores one-way efficiency = sqrt(rte).
        assert!(
            (inv_eff - rte.sqrt()).abs() < 1e-9,
            "inverter_efficiency={inv_eff:.6}, expected sqrt(rte)={:.6}",
            rte.sqrt()
        );
    }

    /// HPXML ElectricVehicle: resolve_ev emits a typed EV config with the expected fields.
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let ev = specs
            .iter()
            .find(|s| s.name == "EV")
            .expect("EV spec must be present");

        let typed: hares_equipment::EvConfig = ev
            .typed_config
            .as_ref()
            .expect("EV spec must carry typed config")
            .typed()
            .expect("EV typed config");

        assert_eq!(typed.charging_level.as_deref(), Some("Level 2"));
        assert!(
            (typed.max_charging_power_kw - 7.2).abs() < 1e-9,
            "max_charging_power_kw={}, expected 7.2",
            typed.max_charging_power_kw
        );
        assert!(
            (typed.capacity_kwh - 60.0).abs() < 1e-9,
            "capacity_kwh={}, expected 60.0",
            typed.capacity_kwh
        );
    }

    #[test]
    fn build_spec_uses_parseable_fuel_type_string() {
        let spec = super::build_spec(
            "Test Load".to_string(),
            FuelType::Gas,
            Map::new(),
            &DefaultsStore::empty(),
        );

        assert_eq!(
            spec.parameters.get("fuel_type").and_then(Value::as_str),
            Some("natural gas")
        );
    }

    #[test]
    fn hpxml_pv_produces_typed_config() {
        let xml = minimal_wh_xml(
            r#"<Photovoltaics>
          <PVSystem>
            <Tracking>fixed</Tracking>
            <MaxPowerOutput>5000</MaxPowerOutput>
            <ArrayTilt>30</ArrayTilt>
            <ArrayAzimuth>180</ArrayAzimuth>
            <ModuleType>standard</ModuleType>
            <SystemLossesFraction>0.14</SystemLossesFraction>
          </PVSystem>
        </Photovoltaics>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let pv = specs
            .iter()
            .find(|s| s.name == "PV")
            .expect("PV spec must be present");

        let typed: hares_equipment::PvConfig = pv
            .typed_config
            .as_ref()
            .expect("PV spec must carry typed config")
            .typed()
            .expect("PV typed config");

        assert!((typed.capacity_kw - 5.0).abs() < 1e-9);
        assert_eq!(typed.tilt_deg, Some(30.0));
        assert_eq!(typed.azimuth_deg, Some(180.0));
        assert_eq!(typed.module_type.as_deref(), Some("standard"));
        assert_eq!(typed.system_losses_fraction, Some(0.14));
    }

    #[test]
    fn hpxml_pv_non_fixed_tracking_is_rejected() {
        let xml = minimal_wh_xml(
            r#"<Photovoltaics>
          <PVSystem>
            <Tracking>1-axis</Tracking>
            <MaxPowerOutput>5000</MaxPowerOutput>
            <ArrayTilt>30</ArrayTilt>
            <ArrayAzimuth>180</ArrayAzimuth>
            <ModuleType>standard</ModuleType>
            <SystemLossesFraction>0.14</SystemLossesFraction>
          </PVSystem>
        </Photovoltaics>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let err = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect_err("non-fixed tracking should be rejected");

        let msg = match err {
            HpxmlError::Parse(msg) => msg.to_string(),
            other => panic!("expected parse error, got {other:?}"),
        };
        assert!(
            msg.contains("unsupported tracking mode") && msg.contains("1-axis"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn hpxml_generator_produces_typed_config() {
        let xml = minimal_wh_xml(
            r#"<extension><Generators>
          <Generator>
            <FuelType>natural gas</FuelType>
            <ElectricalPowerOutput>10.0</ElectricalPowerOutput>
            <AnnualOutputkWh>2500</AnnualOutputkWh>
            <AnnualConsumptionkBtu>9000</AnnualConsumptionkBtu>
          </Generator>
        </Generators></extension>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let generator = specs
            .iter()
            .find(|s| s.name == "Gas Generator")
            .expect("Gas Generator spec must be present");

        let typed: hares_equipment::GeneratorConfig = generator
            .typed_config
            .as_ref()
            .expect("generator spec must carry typed config")
            .typed()
            .expect("generator typed config");

        assert_eq!(typed.fuel_type, Some(FuelType::Gas));
        assert!((typed.rated_power_kw - 10.0).abs() < 1e-9);
        let eta = typed.eta_electric.expect("eta_electric must be present");
        let expected = 2500.0 / (9000.0 * 0.293_071_07);
        assert!((eta - expected).abs() < 1e-9);
    }

    #[test]
    fn hpxml_ventilation_produces_typed_config() {
        let xml = minimal_wh_xml(
            r#"<MechanicalVentilation><VentilationFans>
          <VentilationFan>
            <UsedForWholeBuildingVentilation>true</UsedForWholeBuildingVentilation>
            <FanType>energy recovery ventilator</FanType>
            <RatedFlowRate>75</RatedFlowRate>
            <FanPower>30</FanPower>
            <SensibleRecoveryEfficiency>0.75</SensibleRecoveryEfficiency>
            <TotalRecoveryEfficiency>0.85</TotalRecoveryEfficiency>
            <HoursInOperation>8</HoursInOperation>
          </VentilationFan>
        </VentilationFans></MechanicalVentilation>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let fan = specs
            .iter()
            .find(|s| s.name == "Ventilation Fan")
            .expect("Ventilation Fan spec must be present");

        let typed: hares_equipment::VentilationConfig = fan
            .typed_config
            .as_ref()
            .expect("ventilation spec must carry typed config")
            .typed()
            .expect("ventilation typed config");

        assert!((typed.flow_rate_m3_s - 75.0 * hares_physics::constants::CFM_TO_M3_S).abs() < 1e-9);
        assert_eq!(typed.fan_power_w, Some(30.0));
        assert_eq!(typed.sensible_effectiveness, Some(0.75));
        assert!(
            typed
                .latent_effectiveness
                .map(|v| (v - 0.10).abs() < 1e-9)
                .unwrap_or(false)
        );
        assert_eq!(typed.ventilation_type.as_deref(), Some("erv"));
        assert_eq!(typed.balanced, Some(true));
        assert_eq!(typed.hours_in_operation, Some(8.0));
    }

    /// Gas WH without HeatingCapacity: no usable numeric ua_w_per_k override
    /// should be emitted. A serialized `null` is acceptable, but it must not
    /// become a real UA value that overrides the equipment default.
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let wh = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("WH spec must be present");

        // Without HeatingCapacity the gas EF→UA formula cannot be evaluated;
        // ua_w_per_k must be absent so the equipment model uses its built-in default.
        assert!(
            wh.parameters
                .get("ua_w_per_k")
                .and_then(|v| v.as_f64())
                .is_none(),
            "gas WH without capacity must not produce a usable numeric ua_w_per_k override"
        );
    }

    fn hvac_with_ducts_xml(hvac_inner: &str) -> String {
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
      <Enclosure>
        <Walls />
        <Attics><Attic><FloorArea units="ft2">500</FloorArea></Attic></Attics>
      </Enclosure>
      <Systems>
        <HVAC>
          <HVACDistribution>
            <DistributionSystemType>
              <AirDistribution>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="LeakSupply"/>
                  <DuctType>supply</DuctType>
                  <DuctLeakage>
                    <Value>10</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
        let specs =
            resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None).unwrap();
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
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.80</Value></AnnualHeatingEfficiency>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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

        // Room AC must receive cooling setpoints via typed setpoint source.
        let cooling_source: ScheduleSourceConfig = serde_json::from_value(
            rac.parameters
                .get("cooling_setpoint_source")
                .cloned()
                .expect("Room Air Conditioner must receive cooling_setpoint_source"),
        )
        .expect("cooling_setpoint_source must deserialize");
        match cooling_source {
            ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            } => {
                assert_eq!(weekday.len(), 24, "weekday setpoints must have 24 values");
                assert_eq!(weekend.len(), 24, "weekend setpoints must have 24 values");
                let expected_c = conv::temperature_f_to_c(75.0);
                assert!(
                    (weekday[0] - expected_c).abs() < 1e-6,
                    "weekday setpoint should be 75F converted to C"
                );
            }
            other => panic!("expected DailyProfile setpoint source, got {other:?}"),
        }
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
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.80</Value></AnnualHeatingEfficiency>
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace");

        let spec_heating_source: ScheduleSourceConfig = serde_json::from_value(
            furnace
                .parameters
                .get("heating_setpoint_source")
                .cloned()
                .expect("spec heating setpoint source"),
        )
        .expect("heating_setpoint_source must deserialize");
        let spec_heating_wd = match spec_heating_source {
            ScheduleSourceConfig::DailyProfile { weekday, .. } => weekday,
            other => panic!("expected heating DailyProfile setpoint source, got {other:?}"),
        };

        // Both paths should produce the same 24-element array.
        assert_eq!(bldg_heating_wd.len(), 24);
        assert_eq!(spec_heating_wd.len(), 24);
        for (i, (bldg_val, spec_val)) in bldg_heating_wd
            .iter()
            .zip(spec_heating_wd.iter())
            .enumerate()
        {
            assert!(
                (bldg_val - spec_val).abs() < 1e-10,
                "heating setpoint mismatch at index {i}: building={bldg_val}, spec={spec_val}"
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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
    fn central_ac_hpxml_parse_produces_typed_config_with_correct_eir() {
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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

        let expected_eir = 3.412_141_633_f64 / 16.0_f64;
        assert!(
            (cfg.eir - expected_eir).abs() < 1e-12,
            "eir must equal 3.412141633/16={expected_eir:.9}, got {}",
            cfg.eir
        );
        let expected_w = conv::power_btu_h_to_w(36_000.0);
        assert!(
            (cfg.capacity_w - expected_w).abs() < 1.0,
            "capacity_w must be ~{expected_w:.1} W, got {}",
            cfg.capacity_w
        );
    }

    #[test]
    fn dehumidifier_in_appliances_section_produces_typed_config() {
        let xml = minimal_appliance_xml(
            r#"<Dehumidifier>
              <Capacity>30</Capacity>
              <IntegratedEnergyFactor>2.0</IntegratedEnergyFactor>
              <DehumidistatSetpoint>50</DehumidistatSetpoint>
              <FractionDehumidificationLoadServed>1.0</FractionDehumidificationLoadServed>
            </Dehumidifier>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");
        let spec = specs
            .iter()
            .find(|s| s.name == "Dehumidifier")
            .expect("Dehumidifier spec must be present");

        let typed: hares_equipment::hvac::cooling_config::DehumidifierConfig = spec
            .typed_config
            .as_ref()
            .expect("Dehumidifier spec must carry typed config")
            .typed()
            .expect("typed DehumidifierConfig");

        let expected_l_per_day = 30.0 * 0.473_176_473;
        assert!(
            (typed.capacity_liters_per_day.expect("capacity must be set") - expected_l_per_day)
                .abs()
                < 1e-9,
            "capacity_liters_per_day mismatch"
        );
        assert_eq!(typed.integrated_energy_factor, Some(2.0));
        assert!(
            (typed.target_rh.expect("target_rh must be set") - 0.5).abs() < 1e-9,
            "target_rh must be normalized from 50 percent to 0.5"
        );
        assert_eq!(typed.fraction_served, Some(1.0));
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
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

    // --- HeatingCapacity17F ratio ---
    // HeatingCapacity17F (21600 BTU/h) with HeatingCapacity (36000 BTU/h) must
    // produce capacity_ratio_at_17f ≈ 0.600 in the ASHP Heater typed config.
    // This test FAILS until the resolver reads HeatingCapacity17F.
    #[test]
    fn ashp_heating_capacity_17f_ratio_is_parsed_into_typed_config() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1500</ConditionedFloorArea>
          <ConditionedBuildingVolume>12000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <HeatingCapacity>36000.0</HeatingCapacity>
            <HeatingCapacity17F>21600.0</HeatingCapacity17F>
            <CoolingCapacity>36000.0</CoolingCapacity>
          </HeatPump>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");

        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater spec must be present");

        let ratio = heater
            .parameters
            .get("capacity_ratio_at_17f")
            .and_then(Value::as_f64)
            .expect(
                "HeatingCapacity17F must be parsed and stored as capacity_ratio_at_17f in params",
            );

        assert!(
            (ratio - 0.600).abs() < 0.01,
            "capacity_ratio_at_17f = {ratio:.4}, expected ~0.600 (21600/36000)"
        );

        let typed: hares_equipment::HeatPumpHeaterConfig = heater
            .typed_config
            .as_ref()
            .expect("heater spec must carry typed config")
            .typed()
            .expect("heater typed config must deserialise");
        let typed_ratio = typed
            .capacity_ratio_at_17f
            .expect("capacity_ratio_at_17f must be present in typed config");
        assert!(
            (typed_ratio - 0.600).abs() < 0.01,
            "typed capacity_ratio_at_17f = {typed_ratio:.4}, expected ~0.600 (21600/36000)"
        );
    }

    // Absence of HeatingCapacity17F must not produce an error or set the ratio field.
    #[test]
    fn ashp_without_heating_capacity_17f_has_none_ratio() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1500</ConditionedFloorArea>
          <ConditionedBuildingVolume>12000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatPump>
            <HeatPumpType>air-to-air</HeatPumpType>
            <HeatingCapacity>36000.0</HeatingCapacity>
            <CoolingCapacity>36000.0</CoolingCapacity>
          </HeatPump>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment must not error when HeatingCapacity17F is absent");

        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater spec must be present");

        let typed: hares_equipment::HeatPumpHeaterConfig = heater
            .typed_config
            .as_ref()
            .expect("heater spec must carry typed config")
            .typed()
            .expect("heater typed config must deserialise");
        assert!(
            typed.capacity_ratio_at_17f.is_none(),
            "capacity_ratio_at_17f must be None when HeatingCapacity17F is absent"
        );
    }

    // Unsupported HeatPumpType must produce a hard error, not a silent HVAC skip.

    #[test]
    fn heat_pump_ground_to_air_produces_gshp_specs() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>ground-to-air</HeatPumpType>
          <HeatingCapacity>36000</HeatingCapacity>
          <CoolingCapacity>36000</CoolingCapacity>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("xml parses");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("ground-to-air must succeed with GSHP model");
        let heater = specs
            .iter()
            .find(|s| s.name == "GSHP Heater")
            .expect("GSHP Heater spec must be present");
        let cooler = specs
            .iter()
            .find(|s| s.name == "GSHP Cooler")
            .expect("GSHP Cooler spec must be present");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(cooler.fuel_type, FuelType::Electric);
    }

    #[test]
    fn heat_pump_water_loop_to_air_produces_wshp_specs() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>water-loop-to-air</HeatPumpType>
          <HeatingCapacity>36000</HeatingCapacity>
          <CoolingCapacity>36000</CoolingCapacity>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("xml parses");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("water-loop-to-air must resolve to WSHP equipment");
        let heater = specs
            .iter()
            .find(|s| s.name == "WSHP Heater")
            .expect("WSHP Heater spec must be present");
        let cooler = specs
            .iter()
            .find(|s| s.name == "WSHP Cooler")
            .expect("WSHP Cooler spec must be present");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(cooler.fuel_type, FuelType::Electric);
    }

    #[test]
    fn heat_pump_water_to_air_produces_wshp_specs() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>water-to-air</HeatPumpType>
          <HeatingCapacity>36000</HeatingCapacity>
          <CoolingCapacity>36000</CoolingCapacity>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("xml parses");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("water-to-air must resolve to WSHP equipment");
        let heater = specs
            .iter()
            .find(|s| s.name == "WSHP Heater")
            .expect("WSHP Heater spec must be present");
        let cooler = specs
            .iter()
            .find(|s| s.name == "WSHP Cooler")
            .expect("WSHP Cooler spec must be present");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(cooler.fuel_type, FuelType::Electric);
    }

    #[test]
    fn heat_pump_air_to_air_produces_ashp_specs() {
        let xml = minimal_hvac_xml(
            r#"<HeatPump>
          <HeatPumpType>air-to-air</HeatPumpType>
          <HeatingCapacity>36000</HeatingCapacity>
          <CoolingCapacity>36000</CoolingCapacity>
        </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("xml parses");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("air-to-air must succeed");
        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater spec must be present");
        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler spec must be present");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(cooler.fuel_type, FuelType::Electric);
    }

    // ── loop wiring tests (T-0141) ────────────────────────────────────

    fn minimal_combi_xml(systems_inner: &str) -> String {
        format!(
            r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction><ConditionedFloorArea>1000</ConditionedFloorArea><ConditionedBuildingVolume>8000</ConditionedBuildingVolume></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>{systems_inner}</Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
        )
    }

    #[test]
    fn combi_boiler_indirect_tank_get_consistent_loop_id_via_related_hvac_system() {
        let xml = minimal_combi_xml(
            r#"<HVAC>
              <HeatingSystem>
                <SystemIdentifier id="boiler1"/>
                <HeatingSystemFuel>natural gas</HeatingSystemFuel>
                <HeatingSystemType><Boiler/></HeatingSystemType>
                <HeatingCapacity>60000</HeatingCapacity>
                <AnnualHeatingEfficiency>
                  <Units>AFUE</Units><Value>0.95</Value>
                </AnnualHeatingEfficiency>
              </HeatingSystem>
            </HVAC>
            <WaterHeating>
              <WaterHeatingSystem>
                <SystemIdentifier id="wh1"/>
                <FuelType>natural gas</FuelType>
                <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                <RelatedHVACSystem idref="boiler1"/>
                <TankVolume>40</TankVolume>
              </WaterHeatingSystem>
            </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");

        let boiler = specs
            .iter()
            .find(|s| s.name == "Gas Boiler")
            .expect("Gas Boiler spec must be present");
        let indirect_tank = specs
            .iter()
            .find(|s| s.name == "Indirect Tank")
            .expect("Indirect Tank spec must be present");

        // Verify the loop ID was wired to both typed configs.
        let boiler_cfg: hares_equipment::GasBoilerConfig = boiler
            .typed_config
            .as_ref()
            .expect("boiler typed config")
            .typed()
            .expect("GasBoilerConfig");
        let tank_cfg: hares_equipment::IndirectTankConfig = indirect_tank
            .typed_config
            .as_ref()
            .expect("tank typed config")
            .typed()
            .expect("IndirectTankConfig");

        assert_eq!(
            boiler_cfg.loop_id,
            Some(1),
            "boiler loop_id must be 1, got {:?}",
            boiler_cfg.loop_id
        );
        assert_eq!(
            tank_cfg.boiler_loop_id,
            Some(1),
            "indirect tank boiler_loop_id must be 1, got {:?}",
            tank_cfg.boiler_loop_id
        );
    }

    #[test]
    fn indirect_tank_without_related_hvac_system_keeps_none_boiler_loop_id() {
        let xml = minimal_combi_xml(
            r#"<HVAC>
              <HeatingSystem>
                <SystemIdentifier id="boiler1"/>
                <HeatingSystemFuel>natural gas</HeatingSystemFuel>
                <HeatingSystemType><Boiler/></HeatingSystemType>
                <HeatingCapacity>60000</HeatingCapacity>
                <AnnualHeatingEfficiency>
                  <Units>AFUE</Units><Value>0.95</Value>
                </AnnualHeatingEfficiency>
              </HeatingSystem>
            </HVAC>
            <WaterHeating>
              <WaterHeatingSystem>
                <SystemIdentifier id="wh1"/>
                <FuelType>natural gas</FuelType>
                <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                <TankVolume>40</TankVolume>
              </WaterHeatingSystem>
            </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");

        let tank_cfg: hares_equipment::IndirectTankConfig = specs
            .iter()
            .find(|s| s.name == "Indirect Tank")
            .expect("Indirect Tank spec")
            .typed_config
            .as_ref()
            .expect("tank typed config")
            .typed()
            .expect("IndirectTankConfig");

        assert_eq!(
            tank_cfg.boiler_loop_id, None,
            "without RelatedHVACSystem, boiler_loop_id must remain None"
        );
    }

    #[test]
    fn standalone_boiler_without_indirect_tank_keeps_none_loop_id() {
        let xml = minimal_hvac_xml(
            r#"<HeatingSystem>
              <SystemIdentifier id="boiler1"/>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Boiler/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency>
                <Units>AFUE</Units><Value>0.95</Value>
              </AnnualHeatingEfficiency>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");

        let boiler_cfg: hares_equipment::GasBoilerConfig = specs
            .iter()
            .find(|s| s.name == "Gas Boiler")
            .expect("Gas Boiler spec")
            .typed_config
            .as_ref()
            .expect("boiler typed config")
            .typed()
            .expect("GasBoilerConfig");

        assert_eq!(
            boiler_cfg.loop_id, None,
            "standalone boiler without indirect tank must keep loop_id None"
        );
    }

    #[test]
    fn electric_boiler_indirect_tank_pair_wired_correctly() {
        let xml = minimal_combi_xml(
            r#"<HVAC>
              <HeatingSystem>
                <SystemIdentifier id="boiler1"/>
                <HeatingSystemFuel>electricity</HeatingSystemFuel>
                <HeatingSystemType><Boiler/></HeatingSystemType>
                <HeatingCapacity>30000</HeatingCapacity>
                <AnnualHeatingEfficiency>
                  <Units>Percent</Units><Value>100</Value>
                </AnnualHeatingEfficiency>
              </HeatingSystem>
            </HVAC>
            <WaterHeating>
              <WaterHeatingSystem>
                <SystemIdentifier id="wh1"/>
                <FuelType>electricity</FuelType>
                <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                <RelatedHVACSystem idref="boiler1"/>
                <TankVolume>40</TankVolume>
              </WaterHeatingSystem>
            </WaterHeating>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("resolve_equipment");

        let boiler_cfg: hares_equipment::ElectricBoilerConfig = specs
            .iter()
            .find(|s| s.name == "Electric Boiler")
            .expect("Electric Boiler spec")
            .typed_config
            .as_ref()
            .expect("boiler typed config")
            .typed()
            .expect("ElectricBoilerConfig");
        let tank_cfg: hares_equipment::IndirectTankConfig = specs
            .iter()
            .find(|s| s.name == "Indirect Tank")
            .expect("Indirect Tank spec")
            .typed_config
            .as_ref()
            .expect("tank typed config")
            .typed()
            .expect("IndirectTankConfig");

        assert_eq!(boiler_cfg.loop_id, Some(1));
        assert_eq!(tank_cfg.boiler_loop_id, Some(1));
    }
}
