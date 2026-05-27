//! Scheduled loads, lighting, appliances, ventilation, and miscellaneous load resolution.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_equipment::hvac::cooling_config::DehumidifierConfig;
use hares_equipment::{EquipmentConfig, EvConfig, VentilationConfig};
use hares_types::FuelType;

use super::building::{Building, XmlNode, ZoneType};
use super::equipment::{EquipmentSpec, build_spec, build_typed_spec};
use super::resolve_pool::resolve_pool_and_spa_loads;
use super::xml_helpers::{
    capitalize, child_f64, child_load_kwh, child_load_therms, child_text, parse_fuel,
    parse_schedule_extension_params,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

/// OCHRE EV fuel economy: 1/325 * 1000 miles per kWh (for sedans).
const EV_FUEL_ECONOMY: f64 = 1000.0 / 325.0;
/// Fraction of dryer energy exhausted to outdoors when the dryer is vented.
/// Source: OCHRE hpxml.py parse_clothes_dryer; ANSI/RESNET 301-2014 §4.2.2.5.2.7.
const DRYER_EXHAUST_FRACTION_VENTED: f64 = 0.85;
/// Sensible heat gain factor for gas clothes dryers.
/// Approximates OCHRE's BTU-weighted blend of electric parasitic (0.90) and gas combustion (0.8894).
/// Source: OCHRE hpxml.py parse_clothes_dryer.
const DRYER_GAS_SENSIBLE_GAIN: f64 = 0.89;
/// Sensible heat gain factor for electric clothes dryers.
/// Source: OCHRE hpxml.py parse_clothes_dryer.
const DRYER_ELECTRIC_SENSIBLE_GAIN: f64 = 0.90;
/// BTU per kWh conversion factor (1 kWh = 3412.141... BTU; rounded to 3412).
/// Used to convert kWh to BTU for gas dryer therm calculation.
/// Source: OCHRE hpxml.py parse_clothes_dryer.
const BTU_PER_KWH: f64 = 3412.0;

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
        let n_bedrooms = details
            .path(&[
                "BuildingSummary",
                "BuildingConstruction",
                "NumberofBedrooms",
            ])
            .and_then(|n| n.text.parse::<f64>().ok())
            .unwrap_or(3.0);

        // Pre-extract washer params for dryer energy calculation (OCHRE passes
        // the actual ClothesWasher to parse_clothes_dryer).
        let washer_node = appliances.children_named("ClothesWasher").next();
        let washer_rated_kwh = washer_node
            .and_then(|n| child_f64(n, "RatedAnnualkWh"))
            .unwrap_or(400.0);
        let washer_imef = washer_node
            .and_then(|n| child_f64(n, "IntegratedModifiedEnergyFactor"))
            .unwrap_or(1.0);
        let washer_capacity_ft3 = washer_node
            .and_then(|n| child_f64(n, "Capacity"))
            .unwrap_or(3.0);

        for (tag, name) in [
            ("ClothesWasher", "Clothes Washer"),
            ("ClothesDryer", "Clothes Dryer"),
            ("Dishwasher", "Dishwasher"),
            ("Refrigerator", "Refrigerator"),
            ("Freezer", "Freezer"),
            ("CookingRange", "Cooking Range"),
            ("Dehumidifier", "Dehumidifier"),
        ] {
            let mut counter = 0u32;
            for node in appliances.children_named(tag) {
                counter += 1;
                let mut params = Map::new();
                let mut fuel = FuelType::Electric;
                // Default RatedAnnualkWh: washer=400, dishwasher=467, fridge=637+18*beds, freezer=319.8.
                // OCHRE falls back to these when HPXML omits the element.
                let default_kwh = match tag {
                    "ClothesWasher" => Some(400.0),
                    "Dishwasher" => Some(467.0),
                    "Refrigerator" => Some(637.0 + 18.0 * n_bedrooms),
                    "Freezer" => Some(319.8),
                    other => {
                        tracing::warn!(
                            appliance_tag = other,
                            "No default RatedAnnualkWh for appliance type; energy will be zero if \
                             HPXML omits the element"
                        );
                        None
                    }
                };
                let rated_kwh = node
                    .child("extension")
                    .and_then(|ext| child_f64(ext, "AdjustedAnnualkWh"))
                    .or_else(|| child_f64(node, "RatedAnnualkWh"))
                    .or(default_kwh);
                if let Some(kwh) = rated_kwh {
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
                        if let Some(capacity_ft3) = child_f64(node, "Capacity") {
                            params.insert(
                                "capacity_m3".to_string(),
                                json!(conv::volume_ft3_to_m3(capacity_ft3)),
                            );
                        }
                        if let Some(usage) = child_f64(node, "LabelUsage") {
                            params.insert("label_usage_cycles_per_week".to_string(), json!(usage));
                        }
                        if let Some(imef) = child_f64(node, "IntegratedModifiedEnergyFactor") {
                            params.insert("imef".to_string(), json!(imef));
                        }
                        params.insert("hot_water_draw_volume_l".to_string(), json!(15.0));

                        // Convert RatedAnnualkWh to actual annual energy (OCHRE hpxml.py:1243-1262).
                        // RatedAnnualkWh is a test-cycle rating; actual usage depends on
                        // household size, appliance capacity, and label usage cycles.
                        if let Some(rated_kwh) =
                            params.get("annual_electric_kwh").and_then(|v| v.as_f64())
                        {
                            let capacity_ft3 = params
                                .get("capacity_m3")
                                .and_then(|v| v.as_f64())
                                .map(|m3| m3 / 0.028_316_8)
                                .unwrap_or(3.0);
                            let label_usage = params
                                .get("label_usage_cycles_per_week")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(6.0);
                            let multiplier = node
                                .child("extension")
                                .and_then(|e| child_f64(e, "UsageMultiplier"))
                                .unwrap_or(1.0);

                            const GAS_H20: f64 = 0.3914;
                            const ELEC_H20: f64 = 0.0178;
                            // Read per-appliance label rates from HPXML, fall back
                            // to OCHRE defaults when absent.
                            let gas_rate = child_f64(node, "LabelGasRate").unwrap_or(1.09);
                            let gas_cost = child_f64(node, "LabelAnnualGasCost").unwrap_or(27.0);
                            let electric_rate =
                                child_f64(node, "LabelElectricRate").unwrap_or(0.12);

                            let lcy = label_usage * 52.0;
                            let scy = 164.0 + n_bedrooms * 46.5;
                            let acy = scy * ((3.0 * 2.08 + 1.59) / (capacity_ft3 * 2.08 + 1.59));
                            let cw_appl = (gas_cost * GAS_H20 / gas_rate
                                - rated_kwh * electric_rate * ELEC_H20 / electric_rate)
                                / (electric_rate * GAS_H20 / gas_rate - ELEC_H20);
                            let actual_kwh = cw_appl / lcy * acy * multiplier;
                            params.insert("annual_electric_kwh".to_string(), json!(actual_kwh));
                        }
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
                        let frac_lost = if vented {
                            DRYER_EXHAUST_FRACTION_VENTED
                        } else {
                            0.0
                        };
                        let gain_factor = if fuel == FuelType::Gas {
                            DRYER_GAS_SENSIBLE_GAIN
                        } else {
                            DRYER_ELECTRIC_SENSIBLE_GAIN
                        };
                        let frac_sens = (1.0 - frac_lost) * gain_factor;
                        let frac_lat = 1.0 - frac_sens - frac_lost;
                        params.insert("sensible_gain_fraction".to_string(), json!(frac_sens));
                        params.insert("latent_gain_fraction".to_string(), json!(frac_lat));

                        // Default annual energy when HPXML doesn't provide it
                        // (OCHRE hpxml.py parse_clothes_dryer). Uses default washer values
                        // (RatedAnnualkWh=400, IMEF=1.0, Capacity=3.0 ft³) since washer params
                        // are not accessible here.
                        if !params.contains_key("annual_electric_kwh")
                            && !params.contains_key("annual_gas_therms")
                        {
                            let cef = params
                                .get("combined_energy_factor")
                                .and_then(|v| v.as_f64())
                                .or_else(|| {
                                    params
                                        .get("energy_factor")
                                        .and_then(|v| v.as_f64())
                                        .map(|ef| ef / 1.15)
                                })
                                .unwrap_or(3.01);
                            let multiplier = node
                                .child("extension")
                                .and_then(|e| child_f64(e, "UsageMultiplier"))
                                .unwrap_or(1.0);
                            let washer_capacity = washer_capacity_ft3;
                            let rmc = (0.97 * (washer_capacity / washer_imef)
                                - washer_rated_kwh / 312.0)
                                / ((2.0104 * washer_capacity + 1.4242) * 0.455)
                                + 0.04;
                            let acy = (164.0 + 46.5 * n_bedrooms)
                                * ((3.0 * 2.08 + 1.59) / (washer_capacity * 2.08 + 1.59));
                            let base_kwh = (((rmc - 0.04) * 100.0) / 55.5) * (8.45 / cef) * acy;
                            if fuel == FuelType::Electric {
                                params.insert(
                                    "annual_electric_kwh".to_string(),
                                    json!(base_kwh * multiplier),
                                );
                            } else {
                                // Gas dryer: ~7% electric parasitic, ~93% gas combustion
                                // OCHRE hpxml.py:1312-1313
                                let annual_kwh = base_kwh * 0.07 * (3.73 / 3.30) * multiplier;
                                let annual_therm =
                                    base_kwh * BTU_PER_KWH * (1.0 - 0.07) * (3.73 / 3.30)
                                        / 100_000.0
                                        * multiplier;
                                params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
                                params.insert("annual_gas_therms".to_string(), json!(annual_therm));
                            }
                        }
                    }
                    "Dishwasher" => {
                        if let Some(cap) = child_f64(node, "PlaceSettingCapacity") {
                            params.insert("place_setting_capacity".to_string(), json!(cap));
                        }
                        if let Some(usage) = child_f64(node, "LabelUsage") {
                            params.insert("label_usage_cycles_per_week".to_string(), json!(usage));
                        }
                        params.insert("hot_water_draw_volume_l".to_string(), json!(6.0));

                        // Convert RatedAnnualkWh to actual annual energy (OCHRE hpxml.py:1336-1354).
                        // RatedAnnualkWh is a test-cycle rating; actual usage depends on
                        // household size, place-setting capacity, and label usage cycles.
                        if let Some(rated_kwh) =
                            params.get("annual_electric_kwh").and_then(|v| v.as_f64())
                        {
                            let capacity = params
                                .get("place_setting_capacity")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(12.0);
                            let label_usage = params
                                .get("label_usage_cycles_per_week")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(4.0);
                            let multiplier = node
                                .child("extension")
                                .and_then(|e| child_f64(e, "UsageMultiplier"))
                                .unwrap_or(1.0);

                            let gas_rate = child_f64(node, "LabelGasRate").unwrap_or(1.09);
                            let gas_cost = child_f64(node, "LabelAnnualGasCost").unwrap_or(33.12);
                            let electric_rate =
                                child_f64(node, "LabelElectricRate").unwrap_or(0.12);

                            let usage_annual = label_usage * 52.0;
                            let kwh_per_cyc = ((gas_cost * 0.5497 / gas_rate
                                - rated_kwh * electric_rate * 0.02504 / electric_rate)
                                / (electric_rate * 0.5497 / gas_rate - 0.02504))
                                / usage_annual;
                            let dwcpy = (88.4 + 34.9 * n_bedrooms) * (12.0 / capacity);
                            let actual_kwh = kwh_per_cyc * dwcpy * multiplier;
                            params.insert("annual_electric_kwh".to_string(), json!(actual_kwh));
                        }
                    }
                    "CookingRange"
                        if !params.contains_key("annual_electric_kwh")
                            && !params.contains_key("annual_gas_therms") =>
                    {
                        // OCHRE hpxml.py parse_cooking_range:1451-1461.
                        let is_induction = child_text(node, "IsInduction")
                            .map(|v| v.eq_ignore_ascii_case("true"))
                            .unwrap_or(false);
                        let multiplier = node
                            .child("extension")
                            .and_then(|e| child_f64(e, "UsageMultiplier"))
                            .unwrap_or(1.0);
                        let burner_ef = if is_induction { 0.91_f64 } else { 1.0_f64 };
                        let oven_ef = 1.0_f64;
                        if fuel == FuelType::Electric {
                            let annual_kwh =
                                burner_ef * oven_ef * (331.0 + 39.0 * n_bedrooms) * multiplier;
                            params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
                        } else {
                            let annual_kwh = (22.6 + 2.7 * n_bedrooms) * multiplier;
                            let annual_therm = oven_ef * (22.6 + 2.7 * n_bedrooms) * multiplier;
                            params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
                            params.insert("annual_gas_therms".to_string(), json!(annual_therm));
                        }
                    }
                    "Dehumidifier" => {
                        if let Some(cap_pints_day) = child_f64(node, "Capacity") {
                            // HPXML §Dehumidifier/Capacity is in US liquid pints/day per the HPXML 4.x schema
                            // annotation. 1 US liquid pint = 0.473176473 L (exact, NIST Handbook 44).
                            params.insert(
                                "capacity_liters_per_day".to_string(),
                                json!(cap_pints_day * 0.473_176_473),
                            );
                        }
                        if let Some(ef) = child_f64(node, "EnergyFactor") {
                            params.insert("energy_factor".to_string(), json!(ef));
                        }
                        if let Some(ief) = child_f64(node, "IntegratedEnergyFactor") {
                            params.insert("integrated_energy_factor".to_string(), json!(ief));
                        }
                        if let Some(frac) = child_f64(node, "FractionDehumidificationLoadServed")
                            .or_else(|| child_f64(node, "FractionLoadServed"))
                        {
                            params.insert("fraction_served".to_string(), json!(frac));
                        }
                        if let Some(setpoint) = child_f64(node, "DehumidistatSetpoint") {
                            let target_rh = if setpoint > 1.0 {
                                setpoint / 100.0
                            } else {
                                setpoint
                            };
                            params.insert("target_rh".to_string(), json!(target_rh));
                        }
                    }
                    other => {
                        tracing::warn!(
                            appliance_tag = other,
                            "No appliance-specific parameter extraction for this tag; \
                             only generic fields will be parsed"
                        );
                    }
                }

                for (k, v) in parse_schedule_extension_params(node, "") {
                    params.insert(k, v);
                }

                // OCHRE hpxml.py:1397-1406: refrigerators in non-conditioned space
                // (garage, attic, unconditioned basement, etc.) contribute zero
                // zone gain. Location classification uses substring-keyword
                // matching via is_conditioned_location() — same pattern as
                // parse_zone_label / parse_duct_location. Primary refrigerators
                // default to "conditioned space"; non-primary default to "".
                let is_non_primary_fridge = if tag == "Refrigerator" {
                    let is_primary = child_text(node, "PrimaryIndicator")
                        .map(|v| v.eq_ignore_ascii_case("true"))
                        .unwrap_or(true);
                    let default_loc = if is_primary { "conditioned space" } else { "" };
                    let location =
                        child_text(node, "Location").unwrap_or_else(|| default_loc.to_string());
                    if !is_conditioned_location(&location) {
                        params.insert("sensible_gain_fraction".to_string(), json!(0.0));
                        params.insert("latent_gain_fraction".to_string(), json!(0.0));
                    }
                    !is_primary
                } else {
                    false
                };

                let mut spec = build_spec(name.to_string(), fuel, params, defaults);
                if counter > 1 {
                    spec.instance_name = Some(format!("{name} {counter}"));
                }
                if is_non_primary_fridge {
                    spec.instance_name = Some(format!("{name} (Secondary)"));
                }
                if tag == "Dehumidifier" {
                    let cfg = DehumidifierConfig {
                        equipment_id: None,
                        zone_id: None,
                        capacity_liters_per_day: spec
                            .parameters
                            .get("capacity_liters_per_day")
                            .and_then(Value::as_f64),
                        energy_factor: spec.parameters.get("energy_factor").and_then(Value::as_f64),
                        integrated_energy_factor: spec
                            .parameters
                            .get("integrated_energy_factor")
                            .and_then(Value::as_f64),
                        fraction_served: spec
                            .parameters
                            .get("fraction_served")
                            .and_then(Value::as_f64),
                        target_rh: spec.parameters.get("target_rh").and_then(Value::as_f64),
                    };
                    spec.typed_config = Some(EquipmentConfig::from_typed(
                        "Dehumidifier".to_string(),
                        "Dehumidifier".to_string(),
                        cfg,
                    ));
                }
                specs.push(spec);
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
                other => {
                    tracing::warn!(
                        lighting_type = other,
                        "Unrecognized lighting type; light fraction will not be accounted"
                    );
                }
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
                other => {
                    tracing::warn!(
                        lighting_location = other,
                        "Unrecognized lighting location; assuming Indoor Lighting"
                    );
                    "Indoor Lighting"
                }
            }
            .to_string();

            let area_ft2 = match location.as_str() {
                "garage" => garage_area_ft2,
                "basement" => foundation_area_ft2,
                other => {
                    tracing::warn!(
                        lighting_location = other,
                        "Unrecognized lighting location; using indoor floor area"
                    );
                    indoor_area_ft2
                }
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
                    if let Some(kwh) = child_load_kwh(plug) {
                        // Splits the two EV size options from ResStock (matches OCHRE parse_ev)
                        let range_miles = if kwh < 1500.0 { 100.0 } else { 250.0 };
                        // OCHRE: capacity = range / EV_FUEL_ECONOMY where
                        // EV_FUEL_ECONOMY = 1/325 * 1000 miles/kWh
                        let battery_capacity_kwh = range_miles / EV_FUEL_ECONOMY;
                        let max_charging_power_kw = if range_miles < 175.0 { 7.2 } else { 11.5 };
                        let cfg = EvConfig {
                            equipment_id: None,
                            capacity_kwh: battery_capacity_kwh,
                            charging_level: Some("Level 2".to_string()),
                            max_charging_power_kw,
                            charging_efficiency: None,
                            l1_current_a: None,
                            l1_voltage_v: None,
                            soc_max: None,
                            initial_soc: None,
                            battery_temp_c: None,
                            min_charge_temp_c: None,
                            full_power_temp_c: None,
                            heater_power_w: None,
                            heater_threshold_c: None,
                            thermal_mass_j_per_k: None,
                            ua_w_per_k: None,
                            v2l_enabled: None,
                            v2l_soc_reserve: None,
                            v2l_max_discharge_kw: None,
                            v2g_enabled: None,
                            v2g_soc_reserve: None,
                            v2g_max_discharge_kw: None,
                            chemistry: None,
                            fuel_economy_kwh_per_mi: None,
                            ready_soc: None,
                            charging_strategy: None,
                            plug_in_policy: None,
                            power_limit_kw: None,
                            initial_connection_state: None,
                        };
                        specs.push(build_typed_spec(
                            "Electric Vehicle".to_string(),
                            FuelType::Electric,
                            cfg,
                            defaults,
                        ));
                    }
                    continue;
                }
                other => {
                    tracing::warn!(
                        plug_load_type = other,
                        "Unrecognized PlugLoadType; treating as miscellaneous electric load (MELs)"
                    );
                    ("MELs", FuelType::Electric)
                }
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
                other => {
                    tracing::warn!(
                        fuel_load_type = other,
                        "Unrecognized FuelLoadType; defaulting to 'Other'"
                    );
                    "Other"
                }
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

    // Pools, HotTubs, and Spas — delegated to resolve_pool module.
    resolve_pool_and_spa_loads(details, defaults, specs);
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
        // Only include fans used for whole-building or seasonal cooling ventilation.
        let is_whole_building = child_text(fan, "UsedForWholeBuildingVentilation")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let is_seasonal_cooling = child_text(fan, "UsedForSeasonalCoolingLoadReduction")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if !is_whole_building && !is_seasonal_cooling {
            continue;
        }

        let flow_cfm = child_f64(fan, "RatedFlowRate");
        let fan_type_str = child_text(fan, "FanType");
        let fan_type_lower = fan_type_str.as_deref().unwrap_or("").to_ascii_lowercase();
        let flow_m3_s = flow_cfm.map(|cfm| cfm * hares_physics::constants::CFM_TO_M3_S);
        let fan_power_w = if let Some(power_w) = child_f64(fan, "FanPower") {
            Some(power_w)
        } else if let Some(cfm) = flow_cfm {
            // OCHRE default W/CFM by fan type when FanPower is absent.
            let w_per_cfm = match fan_type_lower.as_str() {
                "energy recovery ventilator" | "heat recovery ventilator" | "balanced" => 1.0,
                "exhaust only" | "supply only" => 0.35,
                "whole house fan" => 0.1,
                other => {
                    tracing::warn!(
                        fan_type = other,
                        "Unrecognized FanType; defaulting to 0.35 W/CFM (exhaust/supply default)"
                    );
                    0.35
                }
            };
            Some(cfm * w_per_cfm)
        } else {
            None
        };
        let balanced = matches!(
            fan_type_lower.as_str(),
            "energy recovery ventilator" | "heat recovery ventilator" | "balanced"
        );
        let ventilation_type = match fan_type_lower.as_str() {
            "exhaust only" | "supply only" | "whole house fan" => "exhaust_fan",
            "energy recovery ventilator" => "erv",
            "heat recovery ventilator" | "balanced" => "hrv",
            other => {
                tracing::warn!(
                    fan_type = other,
                    "Unrecognized FanType; defaulting ventilation type to 'hrv'"
                );
                "hrv"
            }
        };
        let sensible_re = child_f64(fan, "SensibleRecoveryEfficiency").unwrap_or(0.0);
        let total_re = child_f64(fan, "TotalRecoveryEfficiency").unwrap_or(0.0);
        let latent_re = (total_re - sensible_re).max(0.0);
        let cfg = VentilationConfig {
            equipment_id: None,
            zone_id: None,
            flow_rate_m3_s: flow_m3_s.unwrap_or(0.0),
            fan_power_w,
            sensible_effectiveness: (sensible_re > 0.0).then_some(sensible_re),
            latent_effectiveness: (latent_re > 0.0).then_some(latent_re),
            bypass_temp_min_c: None,
            bypass_temp_max_c: None,
            defrost_temp_c: None,
            defrost_effectiveness_fraction: None,
            ventilation_type: Some(ventilation_type.to_string()),
            balanced: Some(balanced),
            hours_in_operation: child_f64(fan, "HoursInOperation"),
        };
        specs.push(build_typed_spec(
            "Ventilation Fan".to_string(),
            FuelType::Electric,
            cfg,
            defaults,
        ));
    }
}

/// Returns `true` when an HPXML `Location` string represents a conditioned space
/// suitable for internal heat gains. Follows the same substring-keyword matching
/// pattern as `parse_zone_label` and `parse_duct_location` in `building.rs`.
///
/// HPXML 4.2 RefrigeratorLocation enumeration defines the valid location
/// values. This function handles standard HPXML values plus common non-standard
/// strings encountered in field data (e.g. "Indoor", "finished basement").
///
/// Diverges from OCHRE's `parse_zone_name` (ochre/utils/hpxml.py) which
/// classifies `"basement - conditioned"` as `Foundation`, silently zeroing
/// refrigerator gains for conditioned basements. HARES intentionally classifies
/// it as conditioned using substring-keyword matching with correct priority
/// ordering.
fn is_conditioned_location(location: &str) -> bool {
    let s = location.to_ascii_lowercase();
    let s = s.trim();
    // Explicitly unconditioned or unvented spaces — never conditioned.
    if s.contains("uncondition") || s.contains("unvent") {
        return false;
    }
    // Bare garages (but not "garage - conditioned" — "condition" check below
    // catches those).
    if s.contains("garage") && !s.contains("condition") {
        return false;
    }
    // Attics are unconditioned buffer zones.
    if s.contains("attic") {
        return false;
    }
    // Conditioned-space keywords. Handles all HPXML RefrigeratorLocation values
    // that imply a heated/cooled indoor space plus common non-standard strings.
    // HPXML 4.2 data dictionary §RefrigeratorLocation_simple:
    //   "conditioned space", "living space", "kitchen", "other heated space",
    //   "other housing unit", "other non-freezing space", "basement - conditioned",
    //   "garage - conditioned".
    if s.contains("condition")
        || s == "living space"
        || s == "kitchen"
        || s == "indoor"
        || s.contains("heated")
        || s.contains("housing unit")
        || s.contains("non-freezing")
    {
        return true;
    }
    // Finished basements, foundations, and crawlspaces: "finished" implies
    // conditioned in residential building practice. Guard against "unfinished"
    // (which would not reach here because the "uncondition"/"unvent" checks
    // above cover most forms, but bare "unfinished basement" could slip past).
    if (s.contains("basement") || s.contains("foundation") || s.contains("crawl"))
        && s.contains("finished")
        && !s.contains("unfinished")
    {
        return true;
    }
    // HPXML 4.2 RefrigeratorLocation_simple enumeration values that are not
    // conditioned.
    //
    // "other multifamily buffer space": per HPXML 4.2, a semi-conditioned
    // corridor or common area — not a fully conditioned dwelling unit.
    // Treated as non-conditioned (gains zeroed).
    if s == "other multifamily buffer space" {
        return false;
    }
    false
}

/// Default sensible and latent gain fractions per equipment name.
/// Returns (sensible_fraction, latent_fraction) of equipment power entering the zone.
pub(super) fn default_gain_fractions(name: &str, fuel_type: FuelType) -> Option<(f64, f64)> {
    match name {
        // OCHRE hpxml.py:1461-1472: gas range sensible ~0.64, electric ~0.72.
        "Cooking Range" => {
            if fuel_type == FuelType::Gas {
                Some((0.64, 0.16))
            } else {
                Some((0.72, 0.08))
            }
        }
        "Clothes Washer" => Some((0.27, 0.03)),
        "Dishwasher" => Some((0.30, 0.30)),
        "Refrigerator" => Some((1.00, 0.00)),
        // 0.00 matches OCHRE (freezers are typically in unconditioned space).
        // ASHRAE would use sensible=1.0 for indoor freezers -- location-based
        // override not yet implemented.
        "Freezer" => Some((0.00, 0.00)),
        // OCHRE parse_mel: "other" plug loads → (0.855, 0.045).
        "MELs" | "Plug Loads" => Some((0.855, 0.045)),
        // OCHRE parse_mel: "TV other" → (0.0, 0.0).
        "TV" => Some((0.00, 0.00)),
        // OCHRE parse_lighting: Convective=1.0 for all lighting types.
        "Indoor Lighting" | "Exterior Lighting" | "Basement Lighting" | "Garage Lighting"
        | "Lighting" => Some((1.00, 0.00)),
        // OCHRE: gas lighting and outdoor equipment → 0 zone gain.
        "Gas Lighting" => Some((0.00, 0.00)),
        // OCHRE: ceiling fan → 0 zone gain (parse_mel default for non-"other").
        "Ceiling Fan" => Some((0.00, 0.00)),
        "Ventilation Fan" => Some((1.00, 0.00)),
        // Outdoor equipment: well pump, grill, pool/hot tub, spa.
        "Well Pump" | "Gas Grill" | "Pool Heater" | "Hot Tub Heater" | "Pool Pump"
        | "Hot Tub Pump" | "Spa Pump" | "Spa Heater" => Some((0.00, 0.00)),
        // OCHRE: gas fireplace → (0.50, 0.10).
        "Gas Fireplace" => Some((0.50, 0.10)),
        other => {
            tracing::warn!(
                equipment_name = other,
                fuel_type = ?fuel_type,
                "No default gain fractions for equipment; gains may be zero"
            );
            None
        }
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
        other => {
            tracing::warn!(
                lighting_location = other,
                "Unrecognized lighting location in derive_lighting_annual_kwh; \
                 falling back to interior formula"
            );
            (0.9 / 0.925 * (455.0 + 0.8 * area_ft2) * adj) + (0.1 * (455.0 + 0.8 * area_ft2))
        }
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

#[cfg(test)]
mod tests {
    use super::super::building::parse_xml_document;
    use super::*;

    #[test]
    fn is_conditioned_location_classifies_hpxml_and_field_strings() {
        // Standard HPXML RefrigeratorLocation conditioned values.
        assert!(is_conditioned_location("conditioned space"));
        assert!(is_conditioned_location("living space"));
        assert!(is_conditioned_location("kitchen"));
        assert!(is_conditioned_location("basement - conditioned"));
        assert!(is_conditioned_location("garage - conditioned"));
        assert!(is_conditioned_location("other heated space"));
        assert!(is_conditioned_location("other housing unit"));
        assert!(is_conditioned_location("other non-freezing space"));
        // Common non-standard strings encountered in field data.
        assert!(is_conditioned_location("Indoor"));
        assert!(is_conditioned_location("conditioned basement"));
        assert!(is_conditioned_location("finished basement"));
        // Explicitly unconditioned spaces.
        assert!(!is_conditioned_location("garage"));
        assert!(!is_conditioned_location("basement - unconditioned"));
        assert!(!is_conditioned_location("garage - unconditioned"));
        assert!(!is_conditioned_location("crawlspace - vented"));
        assert!(!is_conditioned_location("attic - vented"));
        assert!(!is_conditioned_location("unconditioned space"));
        assert!(!is_conditioned_location("other multifamily buffer space"));
        assert!(!is_conditioned_location("other"));
        assert!(!is_conditioned_location(""));
        // Edge cases: bare "basement" is ambiguous — conservative default is
        // not conditioned (caller should use "basement - conditioned" when
        // the space is heated).
        assert!(!is_conditioned_location("basement"));
    }

    #[test]
    fn gain_fractions_spa_pump_and_heater_are_zero() {
        assert_eq!(
            default_gain_fractions("Spa Pump", FuelType::Electric),
            Some((0.0, 0.0))
        );
        assert_eq!(
            default_gain_fractions("Spa Heater", FuelType::Electric),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn refrigerator_adjusted_annual_kwh_preferred() {
        let xml = r#"<Refrigerator>
          <RatedAnnualkWh>500</RatedAnnualkWh>
          <extension>
            <AdjustedAnnualkWh>420</AdjustedAnnualkWh>
          </extension>
        </Refrigerator>"#;
        let node = parse_xml_document(xml).expect("parse");
        let ext = node.child("extension").unwrap();
        let adjusted = child_f64(ext, "AdjustedAnnualkWh");
        assert_eq!(adjusted, Some(420.0));
        let rated = child_f64(&node, "RatedAnnualkWh");
        assert_eq!(rated, Some(500.0));
        // The combined lookup should prefer adjusted.
        let result = node
            .child("extension")
            .and_then(|e| child_f64(e, "AdjustedAnnualkWh"))
            .or_else(|| child_f64(&node, "RatedAnnualkWh"));
        assert_eq!(result, Some(420.0));
    }

    #[test]
    fn refrigerator_falls_back_to_rated_kwh() {
        let xml = r#"<Refrigerator>
          <RatedAnnualkWh>500</RatedAnnualkWh>
        </Refrigerator>"#;
        let node = parse_xml_document(xml).expect("parse");
        let result = node
            .child("extension")
            .and_then(|e| child_f64(e, "AdjustedAnnualkWh"))
            .or_else(|| child_f64(&node, "RatedAnnualkWh"));
        assert_eq!(result, Some(500.0));
    }
}
