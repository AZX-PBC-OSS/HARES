//! Scheduled loads, lighting, appliances, ventilation, and miscellaneous load resolution.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_types::FuelType;

use super::building::{Building, XmlNode, ZoneType};
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{
    capitalize, child_f64, child_load_kwh, child_load_therms, child_text,
    parse_fuel,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

/// OCHRE EV fuel economy: 1/325 * 1000 miles per kWh (for sedans).
const EV_FUEL_ECONOMY: f64 = 1000.0 / 325.0;

pub(super) fn resolve_scheduled_loads(
    building: &Building,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let details = &building.details_xml;

    // Occupancy
    if let Some(occupancy) = details.path(&["BuildingSummary", "BuildingOccupancy"]) {
        let mut params = Map::new();
        if let Some(n) = child_f64(occupancy, "NumberofResidents") {
            params.insert("number_of_occupants".to_string(), json!(n));
        }
        for (k, v) in parse_schedule_extension_params(occupancy, "") {
            params.insert(k, v);
        }
        if !params.is_empty() {
            specs.push(build_spec(
                "Occupancy".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        }
    }

    if let Some(appliances) = details.child("Appliances") {
        for (tag, name) in [
            ("ClothesWasher", "Clothes Washer"),
            ("ClothesDryer", "Clothes Dryer"),
            ("Dishwasher", "Dishwasher"),
            ("Refrigerator", "Refrigerator"),
            ("Freezer", "Freezer"),
            ("CookingRange", "Cooking Range"),
        ] {
            for node in appliances.children_named(tag) {
                let mut params = Map::new();
                let mut fuel = FuelType::Electric;
                if let Some(kwh) = child_f64(node, "RatedAnnualkWh") {
                    params.insert("annual_electric_kwh".to_string(), json!(kwh));
                }
                if let Some(load_kwh) = child_load_kwh(node) {
                    params.insert("annual_electric_kwh".to_string(), json!(load_kwh));
                } else if let Some(load_therms) = child_load_therms(node) {
                    fuel = FuelType::Gas;
                    params.insert("annual_gas_therms".to_string(), json!(load_therms));
                }
                if let Some(fuel_name) = child_text(node, "FuelType") {
                    fuel = parse_fuel(Some(&fuel_name));
                }

                // Appliance-specific parameters
                match tag {
                    "ClothesWasher" => {
                        if let Some(cap) = child_f64(node, "Capacity") {
                            params.insert("capacity_ft3".to_string(), json!(cap));
                        }
                        if let Some(usage) = child_f64(node, "LabelUsage") {
                            params.insert("label_usage_cycles_per_week".to_string(), json!(usage));
                        }
                        if let Some(imef) = child_f64(node, "IntegratedModifiedEnergyFactor") {
                            params.insert("imef".to_string(), json!(imef));
                        }
                        params.insert("hot_water_draw_volume_l".to_string(), json!(15.0));
                    }
                    "ClothesDryer" => {
                        if let Some(cef) = child_f64(node, "CombinedEnergyFactor") {
                            params.insert("combined_energy_factor".to_string(), json!(cef));
                        } else if let Some(ef) = child_f64(node, "EnergyFactor") {
                            params.insert("energy_factor".to_string(), json!(ef));
                        }
                        let vented = child_text(node, "Vented")
                            .map(|v| v.eq_ignore_ascii_case("true"))
                            .unwrap_or(true);
                        params.insert("vented".to_string(), json!(vented));

                        // Vented dryers exhaust 85% of energy; unvented keep all.
                        // Gas combustion has a lower sensible fraction than electric.
                        // The 0.89 gas factor approximates OCHRE's BTU-weighted blend of
                        // 0.90 (electric parasitic) and 0.8894 (gas combustion), stable
                        // across CEF values (~7%/93% electric/gas split).
                        let frac_lost = if vented { 0.85 } else { 0.0 };
                        let gain_factor = if fuel == FuelType::Gas { 0.89 } else { 0.90 };
                        let frac_sens = (1.0 - frac_lost) * gain_factor;
                        let frac_lat = 1.0 - frac_sens - frac_lost;
                        params.insert("sensible_gain_fraction".to_string(), json!(frac_sens));
                        params.insert("latent_gain_fraction".to_string(), json!(frac_lat));
                    }
                    "Dishwasher" => {
                        if let Some(cap) = child_f64(node, "PlaceSettingCapacity") {
                            params.insert("place_setting_capacity".to_string(), json!(cap));
                        }
                        if let Some(usage) = child_f64(node, "LabelUsage") {
                            params.insert("label_usage_cycles_per_week".to_string(), json!(usage));
                        }
                        params.insert("hot_water_draw_volume_l".to_string(), json!(6.0));
                    }
                    _ => {}
                }

                for (k, v) in parse_schedule_extension_params(node, "") {
                    params.insert(k, v);
                }
                specs.push(build_spec(name.to_string(), fuel, params, defaults));
            }
        }
    }

    if let Some(lighting) = details.child("Lighting") {
        let indoor_area_ft2 = conv::area_m2_to_ft2(conditioned_floor_area_m2(building));
        let foundation_area_ft2 = conv::area_m2_to_ft2(foundation_floor_area_m2(building));
        let garage_area_ft2 = conv::area_m2_to_ft2(garage_floor_area_m2(building));

        let mut by_location: HashMap<String, LightingFractions> = HashMap::new();
        for group in lighting.children_named("LightingGroup") {
            let location = child_text(group, "Location")
                .unwrap_or_else(|| "interior".to_string())
                .to_ascii_lowercase();
            let ty = lighting_group_type(group).unwrap_or_else(|| "incandescent".to_string());
            let frac = child_f64(group, "FractionofUnitsInLocation").unwrap_or(0.0);

            let entry = by_location.entry(location).or_default();
            match ty.as_str() {
                "lightemittingdiode" => entry.led += frac,
                "compactfluorescent" => entry.compact_fluorescent += frac,
                "fluorescenttube" => entry.fluorescent_tube += frac,
                _ => {}
            }
            if let Some(kwh) = child_load_kwh(group) {
                entry.explicit_kwh = Some(kwh);
            }
        }

        let ext = lighting.child("extension");
        for (location, fractions) in by_location {
            let name = match location.as_str() {
                "interior" => "Indoor Lighting",
                "exterior" => "Exterior Lighting",
                "garage" => "Garage Lighting",
                "basement" => "Basement Lighting",
                _ => "Indoor Lighting",
            }
            .to_string();

            let area_ft2 = match location.as_str() {
                "garage" => garage_area_ft2,
                "basement" => foundation_area_ft2,
                _ => indoor_area_ft2,
            };
            let usage_multiplier = ext
                .and_then(|n| child_f64(n, &format!("{}UsageMultiplier", capitalize(&location))))
                .unwrap_or(1.0);
            let annual_kwh = fractions
                .explicit_kwh
                .unwrap_or_else(|| derive_lighting_annual_kwh(&location, area_ft2, &fractions))
                * usage_multiplier;

            let mut params = Map::new();
            params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
            if let Some(multipliers) = read_extension_month_multipliers(ext, &capitalize(&location))
            {
                params.insert(
                    "month_multipliers".to_string(),
                    Value::Array(multipliers.into_iter().map(Value::from).collect()),
                );
            }
            specs.push(build_spec(name, FuelType::Electric, params, defaults));
        }

        if let Some(ceiling_fan) = lighting.child("CeilingFan") {
            let mut params = Map::new();
            let n_bedrooms = details
                .path(&[
                    "BuildingSummary",
                    "BuildingConstruction",
                    "NumberofBedrooms",
                ])
                .and_then(|n| n.text.parse::<f64>().ok());
            let n_fans =
                child_f64(ceiling_fan, "Count").unwrap_or_else(|| n_bedrooms.unwrap_or(3.0) + 1.0);
            let efficiency_cfm_per_w = ceiling_fan
                .path(&["Airflow", "Efficiency"])
                .and_then(|n| n.text.parse::<f64>().ok())
                .unwrap_or(3000.0 / 42.6);
            let annual_kwh = n_fans * 3000.0 / efficiency_cfm_per_w * 10.5 * 365.0 / 1000.0;
            params.insert("count".to_string(), json!(n_fans));
            params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
            params.insert(
                "month_multipliers".to_string(),
                Value::Array(
                    read_extension_month_multipliers(ceiling_fan.child("extension"), "")
                        .or_else(|| {
                            read_extension_month_multipliers(
                                lighting.child("extension"),
                                "CeilingFan",
                            )
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .map(Value::from)
                        .collect(),
                ),
            );
            specs.push(build_spec(
                "Ceiling Fan".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        }
    }

    if let Some(misc_loads) = details.child("MiscLoads") {
        for plug in misc_loads.children_named("PlugLoad") {
            let load_type = child_text(plug, "PlugLoadType")
                .unwrap_or_else(|| "other".to_string())
                .to_ascii_lowercase();
            let (name, fuel) = match load_type.as_str() {
                "tv other" => ("TV", FuelType::Electric),
                "well pump" => ("Well Pump", FuelType::Electric),
                "electric vehicle charging" => {
                    let mut ev_params = Map::new();
                    if let Some(kwh) = child_load_kwh(plug) {
                        ev_params
                            .insert("vehicle_type".to_string(), Value::String("BEV".to_string()));
                        ev_params.insert(
                            "charging_level".to_string(),
                            Value::String("Level 2".to_string()),
                        );
                        // Splits the two EV size options from ResStock (matches OCHRE parse_ev)
                        let range_miles = if kwh < 1500.0 { 100 } else { 250 };
                        ev_params.insert("range_miles".to_string(), json!(range_miles));
                        // OCHRE: capacity = range / EV_FUEL_ECONOMY where
                        // EV_FUEL_ECONOMY = 1/325 * 1000 miles/kWh
                        let battery_capacity_kwh = f64::from(range_miles) / EV_FUEL_ECONOMY;
                        ev_params.insert(
                            "battery_capacity_kwh".to_string(),
                            json!(battery_capacity_kwh),
                        );
                    }
                    specs.push(build_spec(
                        "Electric Vehicle".to_string(),
                        FuelType::Electric,
                        ev_params,
                        defaults,
                    ));
                    continue;
                }
                _ => ("MELs", FuelType::Electric),
            };

            let mut params = Map::new();
            if let Some(kwh) = child_load_kwh(plug) {
                params.insert("annual_electric_kwh".to_string(), json!(kwh));
            }
            for (k, v) in parse_schedule_extension_params(plug, "") {
                params.insert(k, v);
            }
            specs.push(build_spec(name.to_string(), fuel, params, defaults));
        }

        for fuel_load in misc_loads.children_named("FuelLoad") {
            let load_type = child_text(fuel_load, "FuelLoadType")
                .unwrap_or_else(|| "other".to_string())
                .to_ascii_lowercase();
            let name = match load_type.as_str() {
                "grill" => "Gas Grill",
                "fireplace" => "Gas Fireplace",
                "lighting" => "Gas Lighting",
                _ => "Other",
            }
            .to_string();

            let mut params = Map::new();
            if let Some(therms) = child_load_therms(fuel_load) {
                params.insert("annual_gas_therms".to_string(), json!(therms));
            }
            for (k, v) in parse_schedule_extension_params(fuel_load, "") {
                params.insert(k, v);
            }
            specs.push(build_spec(name, FuelType::Gas, params, defaults));
        }
    }

    for (container, item, pump_name, heater_name) in [
        ("Pools", "Pool", "Pool Pump", "Pool Heater"),
        ("Spas", "Spa", "Spa Pump", "Spa Heater"),
    ] {
        if let Some(group) = details.child(container) {
            for entry in group.children_named(item) {
                for (pump_container, pump_item, schedule_name) in [
                    ("PoolPumps", "PoolPump", pump_name),
                    ("SpaPumps", "SpaPump", pump_name),
                ] {
                    if let Some(pumps) = entry.child(pump_container) {
                        for pump in pumps.children_named(pump_item) {
                            if let Some(kwh) = child_load_kwh(pump) {
                                let mut params = Map::new();
                                params.insert("annual_electric_kwh".to_string(), json!(kwh));
                                specs.push(build_spec(
                                    schedule_name.to_string(),
                                    FuelType::Electric,
                                    params,
                                    defaults,
                                ));
                            }
                        }
                    }
                }

                for (heater_container, heater_item, schedule_name) in [
                    ("Heater", "Heater", heater_name),
                    ("SpaHeater", "SpaHeater", heater_name),
                ] {
                    if let Some(heater) = entry
                        .child(heater_container)
                        .or_else(|| entry.child(heater_item))
                    {
                        let mut params = Map::new();
                        if let Some(kwh) = child_load_kwh(heater) {
                            params.insert("annual_electric_kwh".to_string(), json!(kwh));
                            specs.push(build_spec(
                                schedule_name.to_string(),
                                FuelType::Electric,
                                params,
                                defaults,
                            ));
                        } else if let Some(therms) = child_load_therms(heater) {
                            params.insert("annual_gas_therms".to_string(), json!(therms));
                            specs.push(build_spec(
                                schedule_name.to_string(),
                                FuelType::Gas,
                                params,
                                defaults,
                            ));
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn resolve_ventilation(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(vent_fans) = details.path(&["Systems", "MechanicalVentilation", "VentilationFans"])
    else {
        return;
    };

    for fan in vent_fans.children_named("VentilationFan") {
        // Only include whole-building ventilation fans, matching OCHRE's filter logic
        let is_whole_building = child_text(fan, "UsedForWholeBuildingVentilation")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let is_seasonal_cooling = child_text(fan, "UsedForSeasonalCoolingLoadReduction")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if !is_whole_building && !is_seasonal_cooling {
            continue;
        }

        let mut params = Map::new();
        if let Some(flow_cfm) = child_f64(fan, "RatedFlowRate") {
            params.insert("ventilation_rate_cfm".to_string(), json!(flow_cfm));
        }
        if let Some(power_w) = child_f64(fan, "FanPower") {
            params.insert("power_w".to_string(), json!(power_w));
        }
        if let Some(fan_type) = child_text(fan, "FanType") {
            // OCHRE hpxml.py:556: balanced = fan_type in ["energy recovery ventilator",
            // "heat recovery ventilator", "balanced"]
            let ft_lower = fan_type.to_ascii_lowercase();
            let balanced = matches!(
                ft_lower.as_str(),
                "energy recovery ventilator" | "heat recovery ventilator" | "balanced"
            );
            params.insert("fan_type".to_string(), Value::String(fan_type));
            params.insert("balanced".to_string(), json!(balanced));
        }
        let sensible_re = child_f64(fan, "SensibleRecoveryEfficiency").unwrap_or(0.0);
        let total_re = child_f64(fan, "TotalRecoveryEfficiency").unwrap_or(0.0);
        if sensible_re > 0.0 {
            params.insert("sensible_recovery_efficiency".to_string(), json!(sensible_re));
        }
        // OCHRE hpxml.py:560: latent_recovery = total_recovery - sensible_recovery
        let latent_re = (total_re - sensible_re).max(0.0);
        if latent_re > 0.0 {
            params.insert("latent_recovery_efficiency".to_string(), json!(latent_re));
        }
        if !params.is_empty() {
            specs.push(build_spec(
                "Ventilation Fan".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        }
    }
}

/// Default sensible and latent gain fractions per equipment name, matching OCHRE defaults.
/// Returns `(sensible, latent)`.
pub(super) fn default_gain_fractions(name: &str, fuel_type: FuelType) -> Option<(f64, f64)> {
    match name {
        // OCHRE hpxml.py:1461-1472: gas range sensible ~0.64 (0.80 × 0.7942),
        // electric range sensible ~0.72 (0.80 × 0.90). Latent ~0.16.
        "Cooking Range" => {
            if fuel_type == FuelType::Gas {
                Some((0.64, 0.16))
            } else {
                Some((0.72, 0.08))
            }
        }
        // Dryer fractions are computed inline with venting awareness; see ClothesDryer arm.
        "Clothes Washer" => Some((0.27, 0.03)),
        "Dishwasher" => Some((0.30, 0.30)),
        "Refrigerator" | "Freezer" => Some((1.00, 0.00)),
        "MELs" | "Plug Loads" | "TV" => Some((0.73, 0.02)),
        "Indoor Lighting" | "Exterior Lighting" | "Basement Lighting" | "Garage Lighting"
        | "Lighting" | "Gas Lighting" => Some((0.70, 0.00)),
        "Ceiling Fan" | "Ventilation Fan" => Some((1.00, 0.00)),
        _ => None,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct LightingFractions {
    led: f64,
    compact_fluorescent: f64,
    fluorescent_tube: f64,
    explicit_kwh: Option<f64>,
}

fn derive_lighting_annual_kwh(location: &str, area_ft2: f64, fractions: &LightingFractions) -> f64 {
    let f_led = fractions.led;
    let f_flr = fractions.compact_fluorescent + fractions.fluorescent_tube;
    let f_inc = (1.0 - f_led - f_flr).max(0.0);

    let e_led = 15.0 / 90.0;
    let e_flr = 15.0 / 60.0;
    let e_inc = 1.0;
    let adj = f_inc * e_inc + f_flr * e_flr + f_led * e_led;

    match location {
        "interior" | "basement" => {
            let base = 455.0 + 0.8 * area_ft2;
            (0.9 / 0.925 * base * adj) + (0.1 * base)
        }
        "exterior" => (100.0 + 0.05 * area_ft2) * adj,
        "garage" => 100.0 * adj,
        _ => (0.9 / 0.925 * (455.0 + 0.8 * area_ft2) * adj) + (0.1 * (455.0 + 0.8 * area_ft2)),
    }
}

fn lighting_group_type(group: &XmlNode) -> Option<String> {
    let ty = group.child("LightingType")?;
    ty.children.first().map(|n| n.name.to_ascii_lowercase())
}

fn read_extension_month_multipliers(ext: Option<&XmlNode>, prefix: &str) -> Option<Vec<f64>> {
    let ext = ext?;
    let key = if prefix.is_empty() {
        "MonthlyScheduleMultipliers".to_string()
    } else {
        format!("{prefix}MonthlyScheduleMultipliers")
    };
    let raw = ext.child(&key)?.text.trim();
    let vals: Vec<f64> = raw
        .split(',')
        .filter_map(|x| x.trim().parse::<f64>().ok())
        .collect();
    if vals.len() == 12 {
        Some(vals)
    } else {
        if !vals.is_empty() {
            tracing::warn!(
                key = %key,
                count = vals.len(),
                "MonthlyScheduleMultipliers has unexpected number of values (expected 12); ignoring"
            );
        }
        None
    }
}

fn conditioned_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, ZoneType::Conditioned))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

fn foundation_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, ZoneType::Foundation))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

fn garage_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, ZoneType::Garage))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

/// Parse weekday/weekend schedule fractions and usage multiplier from an extension node.
fn parse_schedule_extension_params(node: &XmlNode, prefix: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let Some(ext) = node.child("extension") else {
        return out;
    };

    let weekday_key = if prefix.is_empty() {
        "WeekdayScheduleFractions".to_string()
    } else {
        format!("{prefix}WeekdayScheduleFractions")
    };
    let weekend_key = if prefix.is_empty() {
        "WeekendScheduleFractions".to_string()
    } else {
        format!("{prefix}WeekendScheduleFractions")
    };
    let multiplier_key = if prefix.is_empty() {
        "UsageMultiplier".to_string()
    } else {
        format!("{prefix}UsageMultiplier")
    };

    if let Some(frac_node) = ext.child(&weekday_key) {
        let vals: Vec<f64> = frac_node
            .text
            .trim()
            .split(',')
            .filter_map(|s| s.trim().parse::<f64>().ok())
            .collect();
        if vals.len() == 24 {
            out.push(("weekday_schedule_fractions".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %weekday_key,
                count = vals.len(),
                "WeekdayScheduleFractions has unexpected number of values (expected 24); ignoring"
            );
        }
    }
    if let Some(frac_node) = ext.child(&weekend_key) {
        let vals: Vec<f64> = frac_node
            .text
            .trim()
            .split(',')
            .filter_map(|s| s.trim().parse::<f64>().ok())
            .collect();
        if vals.len() == 24 {
            out.push(("weekend_schedule_fractions".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %weekend_key,
                count = vals.len(),
                "WeekendScheduleFractions has unexpected number of values (expected 24); ignoring"
            );
        }
    }
    if let Some(mult) = child_f64(ext, &multiplier_key) {
        out.push(("usage_multiplier".to_string(), json!(mult)));
    }

    // Monthly schedule multipliers (same pattern as lighting)
    let month_key = if prefix.is_empty() {
        "MonthlyScheduleMultipliers".to_string()
    } else {
        format!("{prefix}MonthlyScheduleMultipliers")
    };
    if let Some(node) = ext.child(&month_key) {
        let vals: Vec<f64> = node
            .text
            .trim()
            .split(',')
            .filter_map(|s| s.trim().parse::<f64>().ok())
            .collect();
        if vals.len() == 12 {
            out.push(("month_multipliers".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %month_key,
                count = vals.len(),
                "MonthlyScheduleMultipliers has unexpected number of values (expected 12); ignoring"
            );
        }
    }

    // Thermal gain fractions
    if let Some(frac) = child_f64(ext, "FracSensible") {
        out.push(("frac_sensible".to_string(), json!(frac)));
    }
    if let Some(frac) = child_f64(ext, "FracLatent") {
        out.push(("frac_latent".to_string(), json!(frac)));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::building::{XmlNode, parse_xml_document};

    /// Verify that `parse_schedule_extension_params` emits `month_multipliers` only once.
    /// Before the fix, a duplicated code block would emit it twice.
    #[test]
    fn month_multipliers_emitted_only_once() {
        let xml = r#"<node>
          <extension>
            <MonthlyScheduleMultipliers>1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0</MonthlyScheduleMultipliers>
            <WeekdayScheduleFractions>0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1</WeekdayScheduleFractions>
          </extension>
        </node>"#;
        let root = parse_xml_document(xml).expect("parse test XML");

        let result = parse_schedule_extension_params(&root, "");
        let month_count = result
            .iter()
            .filter(|(k, _)| k == "month_multipliers")
            .count();
        assert_eq!(
            month_count, 1,
            "month_multipliers should appear exactly once in output, found {month_count}"
        );
    }
}
