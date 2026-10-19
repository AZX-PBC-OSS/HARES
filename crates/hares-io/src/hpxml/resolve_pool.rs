//! HPXML pool, hot tub, and spa equipment load resolution.
//!
//! Parses `<Pools>/<Pool>`, `<HotTubs>/<HotTub>`, and `<Spas>` elements
//! from `<BuildingDetails>` and creates `EquipmentSpec` entries for
//! pool pumps, pool heaters, spa pumps, and spa heaters.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_physics::units as conv;
use hares_types::FuelType;

use super::HpxmlError;
use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{
    child_annual_energy_j, child_annual_fuel_energy_j, child_f64, child_text, element_id,
    parse_schedule_extension_params, resolve_local_ref,
};

use super::elements::*;
use crate::defaults::DefaultsStore;

/// HPXML PoolType enumeration values (also used for HotTub/Type).
const VALID_POOL_TYPES: [&str; 6] = [
    "in ground",
    "on ground",
    "above ground",
    "other",
    "unknown",
    "none",
];

// ---------------------------------------------------------------------------
// Parsed pool equipment load data
// ---------------------------------------------------------------------------

/// Extracted pool equipment loads used for LocalReference resolution.
///
/// Pool elements are keyed by `SystemIdentifier/@id`. `Spas/PermanentSpa`
/// elements reference a Pool via `AttachedToPool/@idref` and inherit these
/// loads as "Spa Pump" / "Spa Heater" specs.
#[derive(Default)]
pub(super) struct PoolEquipmentLoads {
    pub(super) pump_params: Option<Map<String, Value>>,
    pub(super) heater_params: Option<Map<String, Value>>,
    pub(super) heater_fuel: FuelType,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Resolve pool, hot tub, and spa equipment from `<BuildingDetails>`.
///
/// Returns a lookup of Pool `SystemIdentifier/@id → PoolEquipmentLoads` so
/// that `resolve_spas` can resolve `PermanentSpa` LocalReferences against
/// the already-parsed Pool data.
pub(super) fn resolve_pool_and_spa_loads(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> Result<(), HpxmlError> {
    let pool_equipment = resolve_pools(details, defaults, specs);
    resolve_hot_tubs(details, defaults, specs);
    resolve_spas(details, defaults, specs, &pool_equipment)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pools
// ---------------------------------------------------------------------------

fn resolve_pools(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> HashMap<String, PoolEquipmentLoads> {
    let mut lookup: HashMap<String, PoolEquipmentLoads> = HashMap::new();
    let Some(pools) = details.child(&POOLS) else {
        return lookup;
    };
    for pool in pools.children_named(&POOL) {
        let pool_id = element_id(pool).unwrap_or_default();
        let pool_type = validate_pool_type(child_text(pool, &TYPE), "Pool");
        tracing::info!(
            pool_type = %pool_type,
            pool_id = %if pool_id.is_empty() { "none" } else { pool_id.as_str() },
            "Parsing pool equipment from HPXML <Pools>/<Pool>"
        );
        let mut loads = PoolEquipmentLoads::default();
        resolve_pool_pumps(pool, &pool_type, defaults, specs, &mut loads);
        resolve_pool_heater(pool, &pool_type, defaults, specs, &mut loads);
        if !pool_id.is_empty() && (loads.pump_params.is_some() || loads.heater_params.is_some()) {
            lookup.insert(pool_id, loads);
        }
    }
    lookup
}

fn resolve_pool_pumps(
    pool: &XmlNode,
    pool_type: &str,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    loads: &mut PoolEquipmentLoads,
) {
    let Some(pumps) = pool.child(&POOL_PUMPS) else {
        return;
    };
    for pump in pumps.children_named(&POOL_PUMP) {
        let Some(kwh) = child_annual_energy_j(pump) else {
            continue;
        };
        let mut params = Map::new();
        params.insert("annual_electric_j".to_string(), json!(kwh));
        for (k, v) in parse_schedule_extension_params(pump, "") {
            params.insert(k, v);
        }
        loads.pump_params = Some(params.clone());
        specs.push(build_spec(
            "Pool Pump".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
        tracing::info!(
            load_kwh = kwh,
            pool_type = %pool_type,
            "Pool pump load from HPXML <Pools>/<Pool> (annual_electric_kWh)"
        );
    }
}

fn resolve_pool_heater(
    pool: &XmlNode,
    pool_type: &str,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    loads: &mut PoolEquipmentLoads,
) {
    let Some(heater) = pool.child(&HEATER) else {
        return;
    };
    let mut params = Map::new();
    if let Some(kwh) = child_annual_energy_j(heater) {
        params.insert("annual_electric_j".to_string(), json!(kwh));
        for (k, v) in parse_schedule_extension_params(heater, "") {
            params.insert(k, v);
        }
        loads.heater_params = Some(params.clone());
        loads.heater_fuel = FuelType::Electric;
        specs.push(build_spec(
            "Pool Heater".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
        tracing::info!(
            load_kwh = kwh,
            pool_type = %pool_type,
            "Pool heater load (electric) from HPXML <Pools>/<Pool> (annual_electric_kWh)"
        );
    } else if let Some(therms) = child_annual_fuel_energy_j(heater) {
        params.insert("annual_gas_j".to_string(), json!(therms));
        for (k, v) in parse_schedule_extension_params(heater, "") {
            params.insert(k, v);
        }
        loads.heater_params = Some(params.clone());
        loads.heater_fuel = FuelType::Gas;
        specs.push(build_spec(
            "Pool Heater".to_string(),
            FuelType::Gas,
            params,
            defaults,
        ));
        tracing::info!(
            load_therms = therms,
            pool_type = %pool_type,
            "Pool heater load (gas) from HPXML <Pools>/<Pool> (annual_gas_therms)"
        );
    }
}

// ---------------------------------------------------------------------------
// HotTubs
// ---------------------------------------------------------------------------

fn resolve_hot_tubs(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    let Some(tubs) = details.child(&HOT_TUBS) else {
        return;
    };
    for tub in tubs.children_named(&HOT_TUB) {
        let tub_type = validate_pool_type(child_text(tub, &TYPE), "HotTub");
        tracing::info!(
            tub_type = %tub_type,
            "Parsing hot tub equipment from HPXML <HotTubs>/<HotTub>"
        );
        resolve_hot_tub_pumps(tub, &tub_type, defaults, specs);
        resolve_hot_tub_heater(tub, &tub_type, defaults, specs);
    }
}

fn resolve_hot_tub_pumps(
    tub: &XmlNode,
    tub_type: &str,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(pumps) = tub.child(&HOT_TUB_PUMPS) else {
        return;
    };
    for pump in pumps.children_named(&HOT_TUB_PUMP) {
        let Some(kwh) = child_annual_energy_j(pump) else {
            continue;
        };
        let mut params = Map::new();
        params.insert("annual_electric_j".to_string(), json!(kwh));
        for (k, v) in parse_schedule_extension_params(pump, "") {
            params.insert(k, v);
        }
        specs.push(build_spec(
            "Spa Pump".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
        tracing::info!(
            load_kwh = kwh,
            tub_type = %tub_type,
            "Spa pump load from HPXML <HotTubs>/<HotTub> (annual_electric_kWh)"
        );
    }
}

fn resolve_hot_tub_heater(
    tub: &XmlNode,
    tub_type: &str,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(heater) = tub.child(&HEATER) else {
        return;
    };
    let mut params = Map::new();
    if let Some(kwh) = child_annual_energy_j(heater) {
        params.insert("annual_electric_j".to_string(), json!(kwh));
        for (k, v) in parse_schedule_extension_params(heater, "") {
            params.insert(k, v);
        }
        specs.push(build_spec(
            "Spa Heater".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
        tracing::info!(
            load_kwh = kwh,
            tub_type = %tub_type,
            "Spa heater load (electric) from HPXML <HotTubs>/<HotTub> (annual_electric_kWh)"
        );
    } else if let Some(therms) = child_annual_fuel_energy_j(heater) {
        params.insert("annual_gas_j".to_string(), json!(therms));
        for (k, v) in parse_schedule_extension_params(heater, "") {
            params.insert(k, v);
        }
        specs.push(build_spec(
            "Spa Heater".to_string(),
            FuelType::Gas,
            params,
            defaults,
        ));
        tracing::info!(
            load_therms = therms,
            tub_type = %tub_type,
            "Spa heater load (gas) from HPXML <HotTubs>/<HotTub> (annual_gas_therms)"
        );
    }
}

// ---------------------------------------------------------------------------
// Spas
// ---------------------------------------------------------------------------

fn resolve_spas(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    pool_equipment: &HashMap<String, PoolEquipmentLoads>,
) -> Result<(), HpxmlError> {
    let Some(spas) = details.child(&SPAS) else {
        return Ok(());
    };
    resolve_permanent_spas(spas, defaults, specs, pool_equipment)?;
    resolve_portable_electric_spas(spas, defaults, specs);
    Ok(())
}

fn resolve_permanent_spas(
    spas: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    pool_equipment: &HashMap<String, PoolEquipmentLoads>,
) -> Result<(), HpxmlError> {
    for spa in spas.children_named(&PERMANENT_SPA) {
        let Some(loads) = resolve_local_ref(spa, &ATTACHED_TO_POOL, pool_equipment) else {
            let idref = spa
                .child(&ATTACHED_TO_POOL)
                .and_then(|a| a.attrs.get("idref"))
                .map(|s| s.as_str())
                .unwrap_or("<absent>")
                .to_string();
            return Err(HpxmlError::InvalidField {
                path: "PermanentSpa/AttachedToPool",
                system_kind: "PermanentSpa",
                system_id: element_id(spa).unwrap_or_else(|| "unknown".to_string()),
                value_received: idref,
                reason: "the AttachedToPool idref does not resolve to a resolved pool \
                         equipment load; a spa whose pump and heater loads cannot be \
                         found is rejected, not skipped",
            });
        };
        tracing::info!("PermanentSpa resolved pool equipment via AttachedToPool/@idref");
        if let Some(ref pump_params) = loads.pump_params {
            specs.push(build_spec(
                "Spa Pump".to_string(),
                FuelType::Electric,
                pump_params.clone(),
                defaults,
            ));
        }
        if let Some(ref heater_params) = loads.heater_params {
            specs.push(build_spec(
                "Spa Heater".to_string(),
                loads.heater_fuel,
                heater_params.clone(),
                defaults,
            ));
        }
    }
    Ok(())
}

fn resolve_portable_electric_spas(
    spas: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    for spa in spas.children_named(&PORTABLE_ELECTRIC_SPA) {
        let mut params = Map::new();
        if let Some(kwh) = child_f64(spa, &TOTAL_ANNUAL_POWER_CONSUMPTION_IN_STANDBY_MODE) {
            // The HPXML element's annual consumption is kWh; the raw key is
            // J: one conversion at the write.
            let j = kwh * 3.6e6;
            params.insert("annual_electric_j".to_string(), json!(j));
            tracing::info!(
                load_kwh = kwh,
                "PortableElectricSpa standby load from \
                 TotalAnnualPowerConsumptionInStandbyMode (annual_electric_kWh), \
                 written to annual_electric_j"
            );
        } else if let Some(watts) = child_f64(spa, &STANDBY_POWER) {
            let j = conv::power_watt_to_j_per_year(watts);
            params.insert("annual_electric_j".to_string(), json!(j));
            tracing::info!(
                standby_power_w = watts,
                load_j = j,
                "PortableElectricSpa standby load from StandbyPower \
                 converted to annual_electric_j (W * 8760 h * 3600 s)"
            );
        }
        if params.contains_key("annual_electric_j") {
            specs.push(build_spec(
                "Spa Pump".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Validate a PoolType / HotTub Type value against the HPXML enumeration.
///
/// Returns the validated type string or `"unknown"` for absent/unrecognized values.
/// Unrecognized values emit a `tracing::warn!` naming the element and valid values.
fn validate_pool_type(raw: Option<String>, element: &str) -> String {
    match raw {
        Some(ref t) if VALID_POOL_TYPES.contains(&t.as_str()) => t.clone(),
        Some(other) => {
            tracing::warn!(
                pool_type = %other,
                element,
                valid = ?VALID_POOL_TYPES,
                "Unrecognized <{element}>/Type value; treating as 'unknown'",
            );
            "unknown".to_string()
        }
        None => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hpxml::building::parse_building;

    #[test]
    fn pools_parsed_into_pool_pump_and_heater_specs() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <Type>unknown</Type>
                  <PoolPumps>
                    <PoolPump>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>2700.0</Value>
                      </Load>
                      <extension>
                        <UsageMultiplier>0.9</UsageMultiplier>
                        <WeekdayScheduleFractions>0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1</WeekdayScheduleFractions>
                        <MonthlyScheduleMultipliers>1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0</MonthlyScheduleMultipliers>
                      </extension>
                    </PoolPump>
                  </PoolPumps>
                  <Heater>
                    <Load>
                      <Units>therm/year</Units>
                      <Value>500.0</Value>
                    </Load>
                    <extension>
                      <UsageMultiplier>0.9</UsageMultiplier>
                      <WeekdayScheduleFractions>0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2</WeekdayScheduleFractions>
                      <MonthlyScheduleMultipliers>2.0,2.0,2.0,2.0,2.0,2.0,2.0,2.0,2.0,2.0,2.0,2.0</MonthlyScheduleMultipliers>
                    </extension>
                  </Heater>
                </Pool>
              </Pools>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        assert_eq!(
            pump.parameters
                .get("annual_electric_j")
                .and_then(|v| v.as_f64()),
            Some(2700.0 * 3.6e6)
        );
        assert_eq!(
            pump.parameters
                .get("usage_multiplier")
                .and_then(|v| v.as_f64()),
            Some(0.9)
        );
        assert!(pump.parameters.contains_key("weekday_schedule_fractions"));

        let heater = specs
            .iter()
            .find(|s| s.name == "Pool Heater")
            .expect("Pool Heater spec should exist");
        assert_eq!(heater.fuel_type, FuelType::Gas);
        assert_eq!(
            heater
                .parameters
                .get("annual_gas_j")
                .and_then(|v| v.as_f64()),
            Some(500.0 * 105505600.0)
        );
        assert_eq!(
            heater
                .parameters
                .get("usage_multiplier")
                .and_then(|v| v.as_f64()),
            Some(0.9)
        );
        assert!(heater.parameters.contains_key("weekday_schedule_fractions"));
    }

    #[test]
    fn hot_tubs_parsed_into_spa_pump_and_heater_specs() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <HotTubs>
                <HotTub>
                  <Type>unknown</Type>
                  <HotTubPumps>
                    <HotTubPump>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>1000.0</Value>
                      </Load>
                    </HotTubPump>
                  </HotTubPumps>
                  <Heater>
                    <Load>
                      <Units>kWh/year</Units>
                      <Value>1300.0</Value>
                    </Load>
                  </Heater>
                </HotTub>
              </HotTubs>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (from HotTubs)");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        assert_eq!(
            pump.parameters
                .get("annual_electric_j")
                .and_then(|v| v.as_f64()),
            Some(1000.0 * 3.6e6)
        );

        let heater = specs
            .iter()
            .find(|s| s.name == "Spa Heater")
            .expect("Spa Heater spec should exist (from HotTubs)");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(
            heater
                .parameters
                .get("annual_electric_j")
                .and_then(|v| v.as_f64()),
            Some(1300.0 * 3.6e6)
        );
    }

    #[test]
    fn no_pools_element_resolves_without_error() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        assert!(
            specs.iter().all(|s| s.name != "Pool Pump"
                && s.name != "Pool Heater"
                && s.name != "Spa Pump"
                && s.name != "Spa Heater"),
            "no pool/spa specs should be created when XML has no <Pools>, <HotTubs>, or <Spas> elements"
        );
    }

    #[test]
    fn portable_electric_spa_parsed_into_spa_pump_spec() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Spas>
                <PortableElectricSpa>
                  <StandbyPower>500</StandbyPower>
                </PortableElectricSpa>
              </Spas>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (from PortableElectricSpa)");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        let annual_j = pump
            .parameters
            .get("annual_electric_j")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_j should be present");
        assert!((annual_j - 4380.0 * 3.6e6).abs() < 0.01);
    }

    #[test]
    fn portable_electric_spa_prefers_total_annual_over_standby_power() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Spas>
                <PortableElectricSpa>
                  <StandbyPower>500</StandbyPower>
                  <TotalAnnualPowerConsumptionInStandbyMode>8760</TotalAnnualPowerConsumptionInStandbyMode>
                </PortableElectricSpa>
              </Spas>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist");
        let annual_j = pump
            .parameters
            .get("annual_electric_j")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_j should be present");
        assert!((annual_j - 8760.0 * 3.6e6).abs() < 0.01);
    }

    #[test]
    fn permanent_spa_resolves_pool_equipment_via_idref() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <SystemIdentifier id="Pool1"/>
                  <Type>in ground</Type>
                  <PoolPumps>
                    <PoolPump>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>3000.0</Value>
                      </Load>
                    </PoolPump>
                  </PoolPumps>
                  <Heater>
                    <Load>
                      <Units>therm/year</Units>
                      <Value>400.0</Value>
                    </Load>
                  </Heater>
                </Pool>
              </Pools>
              <Spas>
                <PermanentSpa>
                  <AttachedToPool idref="Pool1"/>
                </PermanentSpa>
              </Spas>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pool_pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        let pool_pump_j = pool_pump
            .parameters
            .get("annual_electric_j")
            .and_then(|v| v.as_f64());
        assert_eq!(pool_pump_j, Some(3000.0 * 3.6e6));

        let pool_heater = specs
            .iter()
            .find(|s| s.name == "Pool Heater")
            .expect("Pool Heater spec should exist");
        assert_eq!(pool_heater.fuel_type, FuelType::Gas);
        let pool_heater_j = pool_heater
            .parameters
            .get("annual_gas_j")
            .and_then(|v| v.as_f64());
        assert_eq!(pool_heater_j, Some(400.0 * 105505600.0));

        let spa_pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (resolved from Pool via idref)");
        assert_eq!(spa_pump.fuel_type, FuelType::Electric);
        let spa_pump_j = spa_pump
            .parameters
            .get("annual_electric_j")
            .and_then(|v| v.as_f64());
        assert_eq!(spa_pump_j, Some(3000.0 * 3.6e6));

        let spa_heater = specs
            .iter()
            .find(|s| s.name == "Spa Heater")
            .expect("Spa Heater spec should exist (resolved from Pool via idref)");
        assert_eq!(spa_heater.fuel_type, FuelType::Gas);
        let spa_heater_j = spa_heater
            .parameters
            .get("annual_gas_j")
            .and_then(|v| v.as_f64());
        assert_eq!(spa_heater_j, Some(400.0 * 105505600.0));
    }

    #[test]
    fn permanent_spa_unresolved_idref_errors() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Spas>
                <PermanentSpa>
                  <AttachedToPool idref="MissingPool"/>
                </PermanentSpa>
              </Spas>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        // An idref that resolves to nothing is an input defect naming the
        // spa and the idref: the pre-fix code skipped the spa
        // silently and the home lost its spa loads.
        let err =
            resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
                .expect_err("an unresolvable AttachedToPool idref must fail the resolve");
        let message = err.to_string();
        assert!(
            message.contains("MissingPool"),
            "the error must name the idref, got: {message}"
        );
        assert!(
            message.contains("AttachedToPool"),
            "the error must name the field, got: {message}"
        );
    }

    #[test]
    fn no_spas_element_resolves_without_error() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        assert!(
            specs.iter().all(|s| s.name != "Pool Pump"
                && s.name != "Pool Heater"
                && s.name != "Spa Pump"
                && s.name != "Spa Heater"),
            "no pool/spa specs should be created when XML has no pool/spa elements"
        );
    }

    #[test]
    fn unrecognized_pool_type_is_warned_and_treated_as_unknown() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <Type>volcanic</Type>
                  <PoolPumps>
                    <PoolPump>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>100.0</Value>
                      </Load>
                    </PoolPump>
                  </PoolPumps>
                </Pool>
              </Pools>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should still be created for unrecognized Type");
        assert_eq!(pump.fuel_type, FuelType::Electric);
    }

    #[test]
    fn valid_pool_type_in_ground_is_accepted() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <Type>in ground</Type>
                  <PoolPumps>
                    <PoolPump>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>100.0</Value>
                      </Load>
                    </PoolPump>
                  </PoolPumps>
                </Pool>
              </Pools>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        assert!(specs.iter().any(|s| s.name == "Pool Pump"));
    }

    #[test]
    fn pool_pump_with_watt_units_converts_to_kwh_year() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <Type>unknown</Type>
                  <PoolPumps>
                    <PoolPump>
                      <Load>
                        <Units>W</Units>
                        <Value>1000.0</Value>
                      </Load>
                    </PoolPump>
                  </PoolPumps>
                </Pool>
              </Pools>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        let annual_j = pump
            .parameters
            .get("annual_electric_j")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_j should be present");
        // 1000 W * 8760 / 1000 = 8760 kWh/year, held in joules
        assert!((annual_j - 8760.0 * 3.6e6).abs() < 0.01);
    }

    #[test]
    fn pool_heater_with_btuh_units_converts_to_therm_year() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
                  <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
              <Pools>
                <Pool>
                  <Type>unknown</Type>
                  <Heater>
                    <Load>
                      <Units>Btuh</Units>
                      <Value>100000.0</Value>
                    </Load>
                  </Heater>
                </Pool>
              </Pools>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs)
            .expect("the pool and spa loads must resolve");

        let heater = specs
            .iter()
            .find(|s| s.name == "Pool Heater")
            .expect("Pool Heater spec should exist");
        assert_eq!(heater.fuel_type, FuelType::Gas);
        let annual_gas_j = heater
            .parameters
            .get("annual_gas_j")
            .and_then(|v| v.as_f64())
            .expect("annual_gas_j should be present");
        // 100000 Btuh * 8760 / 100000 = 8760 therms/year, held in joules
        // through the HARES therm.
        let therms = uom::si::f64::Energy::new::<uom::si::energy::joule>(annual_gas_j)
            .get::<hares_types::si_units::energy::therm>();
        assert!((therms - 8760.0).abs() < 0.01);
    }
}
