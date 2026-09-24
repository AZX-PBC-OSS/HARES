//! HPXML pool, hot tub, and spa equipment load resolution.
//!
//! Parses `<Pools>/<Pool>`, `<HotTubs>/<HotTub>`, and `<Spas>` elements
//! from `<BuildingDetails>` and creates `EquipmentSpec` entries for
//! pool pumps, pool heaters, spa pumps, and spa heaters.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_physics::units as conv;
use hares_types::FuelType;

use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{
    child_f64, child_load_kwh, child_load_therms, child_text, element_id,
    parse_schedule_extension_params, resolve_local_ref,
};

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
) {
    let pool_equipment = resolve_pools(details, defaults, specs);
    resolve_hot_tubs(details, defaults, specs);
    resolve_spas(details, defaults, specs, &pool_equipment);
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
    let Some(pools) = details.child("Pools") else {
        return lookup;
    };
    for pool in pools.children_named("Pool") {
        let pool_id = element_id(pool).unwrap_or_default();
        let pool_type = validate_pool_type(child_text(pool, "Type"), "Pool");
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
    let Some(pumps) = pool.child("PoolPumps") else {
        return;
    };
    for pump in pumps.children_named("PoolPump") {
        let Some(kwh) = child_load_kwh(pump) else {
            continue;
        };
        let mut params = Map::new();
        params.insert("annual_electric_kwh".to_string(), json!(kwh));
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
    let Some(heater) = pool.child("Heater") else {
        return;
    };
    let mut params = Map::new();
    if let Some(kwh) = child_load_kwh(heater) {
        params.insert("annual_electric_kwh".to_string(), json!(kwh));
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
    } else if let Some(therms) = child_load_therms(heater) {
        params.insert("annual_gas_therms".to_string(), json!(therms));
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
    let Some(tubs) = details.child("HotTubs") else {
        return;
    };
    for tub in tubs.children_named("HotTub") {
        let tub_type = validate_pool_type(child_text(tub, "Type"), "HotTub");
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
    let Some(pumps) = tub.child("HotTubPumps") else {
        return;
    };
    for pump in pumps.children_named("HotTubPump") {
        let Some(kwh) = child_load_kwh(pump) else {
            continue;
        };
        let mut params = Map::new();
        params.insert("annual_electric_kwh".to_string(), json!(kwh));
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
    let Some(heater) = tub.child("Heater") else {
        return;
    };
    let mut params = Map::new();
    if let Some(kwh) = child_load_kwh(heater) {
        params.insert("annual_electric_kwh".to_string(), json!(kwh));
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
    } else if let Some(therms) = child_load_therms(heater) {
        params.insert("annual_gas_therms".to_string(), json!(therms));
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
) {
    let Some(spas) = details.child("Spas") else {
        return;
    };
    resolve_permanent_spas(spas, defaults, specs, pool_equipment);
    resolve_portable_electric_spas(spas, defaults, specs);
}

fn resolve_permanent_spas(
    spas: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    pool_equipment: &HashMap<String, PoolEquipmentLoads>,
) {
    for spa in spas.children_named("PermanentSpa") {
        let Some(loads) = resolve_local_ref(spa, "AttachedToPool", pool_equipment) else {
            let idref = spa
                .child("AttachedToPool")
                .and_then(|a| a.attrs.get("idref"))
                .map(|s| s.as_str())
                .unwrap_or("<absent>");
            tracing::warn!(
                attached_to_pool = %idref,
                "PermanentSpa AttachedToPool idref could not be resolved; skipping"
            );
            continue;
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
}

fn resolve_portable_electric_spas(
    spas: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    for spa in spas.children_named("PortableElectricSpa") {
        let mut params = Map::new();
        if let Some(kwh) = child_f64(spa, "TotalAnnualPowerConsumptionInStandbyMode") {
            params.insert("annual_electric_kwh".to_string(), json!(kwh));
            tracing::info!(
                load_kwh = kwh,
                "PortableElectricSpa standby load from \
                 TotalAnnualPowerConsumptionInStandbyMode (annual_electric_kWh)"
            );
        } else if let Some(watts) = child_f64(spa, "StandbyPower") {
            let kwh = conv::power_watt_to_kwh_per_year(watts);
            params.insert("annual_electric_kwh".to_string(), json!(kwh));
            tracing::info!(
                standby_power_w = watts,
                load_kwh = kwh,
                "PortableElectricSpa standby load from StandbyPower \
                 converted to annual_electric_kWh (W * 8760 / 1000)"
            );
        }
        if params.contains_key("annual_electric_kwh") {
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        assert_eq!(
            pump.parameters
                .get("annual_electric_kwh")
                .and_then(|v| v.as_f64()),
            Some(2700.0)
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
                .get("annual_gas_therms")
                .and_then(|v| v.as_f64()),
            Some(500.0)
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (from HotTubs)");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        assert_eq!(
            pump.parameters
                .get("annual_electric_kwh")
                .and_then(|v| v.as_f64()),
            Some(1000.0)
        );

        let heater = specs
            .iter()
            .find(|s| s.name == "Spa Heater")
            .expect("Spa Heater spec should exist (from HotTubs)");
        assert_eq!(heater.fuel_type, FuelType::Electric);
        assert_eq!(
            heater
                .parameters
                .get("annual_electric_kwh")
                .and_then(|v| v.as_f64()),
            Some(1300.0)
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
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (from PortableElectricSpa)");
        assert_eq!(pump.fuel_type, FuelType::Electric);
        let kwh = pump
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh should be present");
        assert!((kwh - 4380.0).abs() < 0.01);
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist");
        let kwh = pump
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh should be present");
        assert!((kwh - 8760.0).abs() < 0.01);
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pool_pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        let pool_pump_kwh = pool_pump
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64());
        assert_eq!(pool_pump_kwh, Some(3000.0));

        let pool_heater = specs
            .iter()
            .find(|s| s.name == "Pool Heater")
            .expect("Pool Heater spec should exist");
        assert_eq!(pool_heater.fuel_type, FuelType::Gas);
        let pool_heater_therms = pool_heater
            .parameters
            .get("annual_gas_therms")
            .and_then(|v| v.as_f64());
        assert_eq!(pool_heater_therms, Some(400.0));

        let spa_pump = specs
            .iter()
            .find(|s| s.name == "Spa Pump")
            .expect("Spa Pump spec should exist (resolved from Pool via idref)");
        assert_eq!(spa_pump.fuel_type, FuelType::Electric);
        let spa_pump_kwh = spa_pump
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64());
        assert_eq!(spa_pump_kwh, Some(3000.0));

        let spa_heater = specs
            .iter()
            .find(|s| s.name == "Spa Heater")
            .expect("Spa Heater spec should exist (resolved from Pool via idref)");
        assert_eq!(spa_heater.fuel_type, FuelType::Gas);
        let spa_heater_therms = spa_heater
            .parameters
            .get("annual_gas_therms")
            .and_then(|v| v.as_f64());
        assert_eq!(spa_heater_therms, Some(400.0));
    }

    #[test]
    fn permanent_spa_unresolved_idref_produces_no_spec() {
        let xml = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
          <Building>
            <BuildingDetails>
              <BuildingSummary>
                <Site><SiteType>suburban</SiteType></Site>
                <BuildingConstruction>
                  <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
                  <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        assert!(
            specs.iter().all(|s| s.name != "Pool Pump"
                && s.name != "Pool Heater"
                && s.name != "Spa Pump"
                && s.name != "Spa Heater"),
            "no pool/spa specs when PermanentSpa references missing pool"
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
                </BuildingConstruction>
              </BuildingSummary>
              <Enclosure><Walls /></Enclosure>
            </BuildingDetails>
          </Building>
        </HPXML>"#;

        let building = parse_building(xml).expect("parse building");
        let mut specs = Vec::new();
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let pump = specs
            .iter()
            .find(|s| s.name == "Pool Pump")
            .expect("Pool Pump spec should exist");
        let kwh = pump
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh should be present");
        // 1000 W * 8760 / 1000 = 8760 kWh/year
        assert!((kwh - 8760.0).abs() < 0.01);
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
        resolve_pool_and_spa_loads(&building.details_xml, &DefaultsStore::empty(), &mut specs);

        let heater = specs
            .iter()
            .find(|s| s.name == "Pool Heater")
            .expect("Pool Heater spec should exist");
        assert_eq!(heater.fuel_type, FuelType::Gas);
        let therms = heater
            .parameters
            .get("annual_gas_therms")
            .and_then(|v| v.as_f64())
            .expect("annual_gas_therms should be present");
        // 100000 Btuh * 8760 / 100000 = 8760 therms/year
        assert!((therms - 8760.0).abs() < 0.01);
    }
}
