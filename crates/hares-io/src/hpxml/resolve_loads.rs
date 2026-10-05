//! Scheduled loads, lighting, appliances, ventilation, and miscellaneous load resolution.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use hares_equipment::hvac::cooling_config::DehumidifierConfig;
use hares_equipment::{EquipmentConfig, EvConfig, VentilationConfig};
use hares_types::{FuelType, parse_trimmed_f64};

use super::HpxmlError;
use super::building::{Building, XmlNode, ZoneType};
use super::equipment::{EquipmentSpec, build_spec, build_typed_spec, canonical_instance_namer};
use super::resolve_pool::resolve_pool_and_spa_loads;
use super::xml_helpers::{
    capitalize, child_bool, child_f64, child_load_kwh, child_load_therms, child_text, element_id,
    parse_fuel, parse_schedule_extension_params,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

/// OCHRE EV fuel economy: 1/325 * 1000 miles per kWh (for sedans).
const EV_FUEL_ECONOMY: f64 = 1000.0 / 325.0;
/// Fraction of dryer energy exhausted to outdoors when the dryer is vented,
/// and the sensible share of the rest, for every fuel and for the electric
/// and fuel input alike: OpenStudio-HPXML v1.12.0
/// `hotwater_appliances.rb` `calc_clothes_dryer_energy` (771-777).
const DRYER_EXHAUST_FRACTION_VENTED: f64 = 0.85;
const DRYER_SENSIBLE_SHARE: f64 = 0.90;
/// BTU per kWh conversion factor (1 kWh = 3412.141... BTU; rounded to 3412).
/// Used to convert kWh to BTU for gas dryer therm calculation.
/// Source: OCHRE hpxml.py parse_clothes_dryer.
const BTU_PER_KWH: f64 = 3412.0;

/// ANSI/RESNET 301-2014 §4.2.2.5.2: microwave oven default annual electric energy (kWh).
pub(crate) const MICROWAVE_DEFAULT_ANNUAL_KWH: f64 = 100.0;

/// Resolve a bedroom count for appliance energy calculations.
///
/// Reads `NumberofBedrooms` from HPXML first. When absent, derives from
/// `NumberofResidents` using `max(1, n_occ - 1)` — a house-type-agnostic
/// approximation. HARES diverges from OCHRE `hpxml.py:791-800`, which uses
/// house-type-specific regression formulas (-1.47+1.69*n_occ for detached,
/// -0.68+1.09*n_occ for attached). HARES cannot access the house type at
/// this point in the parse (it is resolved later in building construction).
///
/// Falls back to ANSI/RESNET 301-2014 Table 4.2.2(1) Reference Home default
/// of 3 bedrooms when neither `NumberofBedrooms` nor `NumberofResidents`
/// is available in HPXML.
fn resolve_bedroom_count_for_appliances(details: &XmlNode) -> f64 {
    if let Some(n) = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofBedrooms",
        ])
        .and_then(|n| parse_trimmed_f64(&n.text))
    {
        #[cfg(feature = "observe")]
        tracing::info!(
            bedroom_source = "HPXML NumberofBedrooms",
            n_bedrooms = n,
            "bedroom count read directly from HPXML"
        );
        return n;
    }
    if let Some(n_occ) = details
        .path(&["BuildingSummary", "BuildingOccupancy", "NumberofResidents"])
        .and_then(|n| parse_trimmed_f64(&n.text))
    {
        // Diverges from OCHRE hpxml.py:791-800 which uses house-type-specific
        // regression formulas (-1.47+1.69*n_occ for detached, -0.68+1.09*n_occ
        // for attached). HARES uses max(1, n_occ - 1) as a house-type-agnostic
        // approximation because house type is not yet resolved at this point.
        let derived = (n_occ - 1.0).max(1.0);
        tracing::warn!(
            derived_bedrooms = derived,
            n_occupants = n_occ,
            "NumberofBedrooms absent from HPXML; derived = max(1, NumberofResidents - 1)"
        );
        #[cfg(feature = "observe")]
        tracing::info!(
            bedroom_source = "derived from occupants",
            n_bedrooms = derived,
            n_occupants = n_occ,
            "bedroom count imputed from occupant count"
        );
        return derived;
    }
    // ANSI/RESNET 301-2014 Table 4.2.2(1) Reference Home: 3 bedrooms.
    3.0
}

pub(super) fn resolve_scheduled_loads(
    building: &Building,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let details = &building.details_xml;

    // Occupancy
    if let Some(occupancy) = details.path(&["BuildingSummary", "BuildingOccupancy"]) {
        let mut params = Map::new();
        if let Some(n) = child_f64(occupancy, "NumberofResidents") {
            params.insert("number_of_occupants".to_string(), json!(n));
            #[cfg(feature = "observe")]
            tracing::info!(
                occupant_source = "HPXML NumberofResidents",
                n_occupants = n,
                "occupant count read directly from HPXML"
            );
        } else if let Some(n_bedrooms) = details
            .path(&[
                "BuildingSummary",
                "BuildingConstruction",
                "NumberofBedrooms",
            ])
            .and_then(|n| parse_trimmed_f64(&n.text))
        {
            // ANSI/RESNET 301-2014 §4.2.2.2.1: occupant count from bedrooms
            // (2 occupants for the first bedroom + 1 for each additional).
            // For n bedrooms this is 2 + (n - 1) = n + 1.
            let derived = n_bedrooms + 1.0;
            tracing::warn!(
                derived_occupants = derived,
                n_bedrooms = n_bedrooms,
                "NumberofResidents absent from HPXML; derived occupant count = \
                 NumberofBedrooms + 1 per ANSI/RESNET 301-2014 §4.2.2.2.1"
            );
            params.insert("number_of_occupants".to_string(), json!(derived));
            #[cfg(feature = "observe")]
            tracing::info!(
                occupant_source = "derived from bedrooms",
                n_occupants = derived,
                n_bedrooms = n_bedrooms,
                "occupant count imputed from bedroom count"
            );
        }
        for (k, v) in parse_schedule_extension_params(occupancy, "") {
            params.insert(k, v);
        }
        if !params.is_empty() {
            if !params.contains_key("number_of_occupants") {
                // Extension schedule params exist but neither NumberofResidents nor
                // NumberofBedrooms is present in HPXML. Default to 3 occupants —
                // ANSI/RESNET 301-2014 Table 4.2.2(1) Reference Home occupant count.
                tracing::error!(
                    "BuildingOccupancy has extension schedule fractions but neither \
                     NumberofResidents nor NumberofBedrooms present in HPXML; \
                     defaulting to 3 occupants (ANSI/RESNET 301-2014 Table 4.2.2(1))"
                );
                params.insert("number_of_occupants".to_string(), json!(3.0));
                #[cfg(feature = "observe")]
                tracing::info!(
                    occupant_source = "default 3 occupants",
                    "occupant count defaulted; neither HPXML field nor bedroom proxy available"
                );
            }
            specs.push(build_spec(
                "Occupancy".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        } else {
            tracing::error!(
                "BuildingOccupancy present but NumberofResidents, NumberofBedrooms, \
                 and schedule extension params are all absent; skipping Occupancy spec"
            );
        }
    }

    if let Some(appliances) = details.child("Appliances") {
        let n_bedrooms = resolve_bedroom_count_for_appliances(details);

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
            ("Microwave", "Microwave"),
            ("Dehumidifier", "Dehumidifier"),
        ] {
            require_unique_appliance_ids(appliances, tag)?;
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
                    fuel = parse_fuel(Some(&fuel_name))?;
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

                        // Multi-phase cycle: fill, wash, rinse, spin.
                        // Fill and wash/rinse phases draw hot water; spin does not.
                        params.insert("phase_len".to_string(), json!(4));
                        params.insert("phase_0_power_kw".to_string(), json!(0.02));
                        params.insert("phase_0_duration_s".to_string(), json!(300.0));
                        params.insert("phase_0_has_water_draw".to_string(), json!(true));
                        params.insert("phase_1_power_kw".to_string(), json!(0.4));
                        params.insert("phase_1_duration_s".to_string(), json!(900.0));
                        params.insert("phase_1_has_water_draw".to_string(), json!(true));
                        params.insert("phase_2_power_kw".to_string(), json!(0.02));
                        params.insert("phase_2_duration_s".to_string(), json!(300.0));
                        params.insert("phase_2_has_water_draw".to_string(), json!(true));
                        params.insert("phase_3_power_kw".to_string(), json!(0.8));
                        params.insert("phase_3_duration_s".to_string(), json!(600.0));
                        params.insert("phase_3_has_water_draw".to_string(), json!(false));
                    }
                    "ClothesDryer" => {
                        if let Some(cef) = child_f64(node, "CombinedEnergyFactor") {
                            params.insert("combined_energy_factor".to_string(), json!(cef));
                        } else if let Some(ef) = child_f64(node, "EnergyFactor") {
                            params.insert("energy_factor".to_string(), json!(ef));
                        }
                        let vented =
                            child_bool(node, "Vented", "ClothesDryer/Vented", "Clothes Dryer")?
                                .unwrap_or(true);
                        // A fuel-fired dryer conveys its moisture and combustion
                        // products outside the building (IFGC 614.1).
                        if !vented && fuel != FuelType::Electric {
                            return Err(HpxmlError::InvalidField {
                                path: "ClothesDryer/Vented",
                                system_kind: "Clothes Dryer",
                                system_id: element_id(node)
                                    .unwrap_or_else(|| "unknown".to_string()),
                                value_received: "false".to_string(),
                                reason: "a fuel-fired clothes dryer must be vented to outdoors",
                            });
                        }
                        let frac_lost = if vented {
                            DRYER_EXHAUST_FRACTION_VENTED
                        } else {
                            0.0
                        };
                        let frac_sens = (1.0 - frac_lost) * DRYER_SENSIBLE_SHARE;
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

                        // Multi-phase cycle: fill, wash, drain, dry.
                        // Fill and wash phases draw hot water; drain and dry do not.
                        params.insert("phase_len".to_string(), json!(4));
                        params.insert("phase_0_power_kw".to_string(), json!(0.05));
                        params.insert("phase_0_duration_s".to_string(), json!(120.0));
                        params.insert("phase_0_has_water_draw".to_string(), json!(true));
                        params.insert("phase_1_power_kw".to_string(), json!(0.6));
                        params.insert("phase_1_duration_s".to_string(), json!(1200.0));
                        params.insert("phase_1_has_water_draw".to_string(), json!(true));
                        params.insert("phase_2_power_kw".to_string(), json!(0.3));
                        params.insert("phase_2_duration_s".to_string(), json!(60.0));
                        params.insert("phase_2_has_water_draw".to_string(), json!(false));
                        params.insert("phase_3_power_kw".to_string(), json!(0.8));
                        params.insert("phase_3_duration_s".to_string(), json!(900.0));
                        params.insert("phase_3_has_water_draw".to_string(), json!(false));
                    }
                    "CookingRange"
                        if !params.contains_key("annual_electric_kwh")
                            && !params.contains_key("annual_gas_therms") =>
                    {
                        // OCHRE hpxml.py parse_cooking_range:1451-1461.
                        let is_induction = child_bool(
                            node,
                            "IsInduction",
                            "CookingRange/IsInduction",
                            "Cooking Range",
                        )?
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
                    "Microwave" if !params.contains_key("annual_electric_kwh") => {
                        // ANSI/RESNET 301-2014 §4.2.2.5.2: microwave oven default.
                        // HPXML Microwave element typically carries RatedAnnualkWh or
                        // extension/AdjustedAnnualkWh; this arm applies the default only
                        // when neither is present.
                        let multiplier = node
                            .child("extension")
                            .and_then(|e| child_f64(e, "UsageMultiplier"))
                            .unwrap_or(1.0);
                        let annual_kwh = MICROWAVE_DEFAULT_ANNUAL_KWH * multiplier;
                        params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
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
                    "Refrigerator" => {
                        let multiplier = node
                            .child("extension")
                            .and_then(|e| child_f64(e, "UsageMultiplier"))
                            .unwrap_or(1.0);
                        if let Some(kwh) =
                            params.get("annual_electric_kwh").and_then(|v| v.as_f64())
                        {
                            params
                                .insert("annual_electric_kwh".to_string(), json!(kwh * multiplier));
                        }
                    }
                    "Freezer" => {
                        let multiplier = node
                            .child("extension")
                            .and_then(|e| child_f64(e, "UsageMultiplier"))
                            .unwrap_or(1.0);
                        if let Some(kwh) =
                            params.get("annual_electric_kwh").and_then(|v| v.as_f64())
                        {
                            params
                                .insert("annual_electric_kwh".to_string(), json!(kwh * multiplier));
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

                #[cfg(debug_assertions)]
                {
                    let multiplier = node
                        .child("extension")
                        .and_then(|e| child_f64(e, "UsageMultiplier"))
                        .unwrap_or(1.0);
                    if (multiplier - 1.0).abs() > f64::EPSILON
                        && let Some(kwh) =
                            params.get("annual_electric_kwh").and_then(|v| v.as_f64())
                    {
                        let unscaled_default = match tag {
                            "Refrigerator" => Some(637.0 + 18.0 * n_bedrooms),
                            "Freezer" => Some(319.8),
                            _ => None,
                        };
                        if let Some(default) = unscaled_default
                            && (kwh - default).abs() < 1e-6
                        {
                            tracing::warn!(
                                appliance = tag,
                                multiplier = %multiplier,
                                unscaled_default_kwh = default,
                                "appliance default energy was not scaled by UsageMultiplier"
                            );
                        }
                    }
                }

                for (k, v) in parse_schedule_extension_params(node, "") {
                    params.insert(k, v);
                }

                let is_non_primary_fridge = tag == "Refrigerator"
                    && !child_bool(
                        node,
                        "PrimaryIndicator",
                        "Refrigerator/PrimaryIndicator",
                        "Refrigerator",
                    )?
                    .unwrap_or(true);
                if tag != "Dehumidifier" {
                    let location = child_text(node, "Location").unwrap_or_else(|| {
                        default_appliance_location(
                            building,
                            tag == "Freezer" || is_non_primary_fridge,
                        )
                        .to_string()
                    });
                    match appliance_site(building, tag, node, &location)? {
                        ApplianceSite::ConditionedSpace => {}
                        ApplianceSite::Zone(zone_id) => {
                            params.insert("zone_id".to_string(), json!(zone_id));
                        }
                        ApplianceSite::OutsideUnit => {
                            params.insert("sensible_gain_fraction".to_string(), json!(0.0));
                            params.insert("latent_gain_fraction".to_string(), json!(0.0));
                        }
                    }
                }

                let mut spec = build_spec(name.to_string(), fuel, params, defaults);
                spec.system_id = element_id(node);
                if counter > 1 {
                    spec.instance_name = Some(canonical_instance_namer(name, counter as usize));
                }
                if is_non_primary_fridge {
                    spec.instance_name = Some(format!("{name} (Secondary)"));
                }
                if tag == "Dehumidifier" {
                    // HPXML's Dehumidifier schema has no Location element;
                    // OS-HPXML and HARES both model every dehumidifier in the
                    // one conditioned zone. A building with no conditioned
                    // zone cannot host one, so the spec is rejected instead of
                    // silently landing on a guessed zone.
                    let zone_id = super::resolve_hvac::conditioned_zone_id(building, name)?;
                    let cfg = DehumidifierConfig {
                        equipment_id: None,
                        zone_id: Some(zone_id),
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
                        part_load_curve_coeffs: None,
                        plf_min: None,
                        off_cycle_parasitic_load_w: None,
                        min_operating_temp_c: None,
                        max_operating_temp_c: None,
                    };
                    spec.typed_config = Some(EquipmentConfig::from_typed(
                        "Dehumidifier".to_string(),
                        "Dehumidifier".to_string(),
                        cfg,
                    )?);
                }
                specs.push(spec);
            }
        }
    }

    if let Some(lighting) = details.child("Lighting") {
        let indoor_area_ft2 = conv::area_m2_to_ft2(conditioned_floor_area_m2(building));
        let foundation_area_ft2 = conv::area_m2_to_ft2(foundation_floor_area_m2(building));
        let garage_area_ft2 = conv::area_m2_to_ft2(garage_floor_area_m2(building));

        // BTreeMap (not HashMap): iteration below pushes one EquipmentSpec per
        // location, and spec order fixes the equipment step order — which in
        // turn fixes the float-summation order behind "Total Electric Power".
        // Sorted-key iteration keeps run-to-run output bit-identical.
        let mut by_location: BTreeMap<String, LightingFractions> = BTreeMap::new();
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

            // OCHRE hpxml.py:1695-1698: Basement Lighting is only created when
            // Foundation Type == "Finished Basement". Unconditioned foundations
            // (unfinished basements, crawlspaces, vented basements, slab-on-grade)
            // have no thermally-modeled basement zone, so routing lighting heat
            // gains to them incorrectly distributes zone-level HVAC loads.
            if location == "basement"
                && building.foundation_name.as_deref() != Some("Finished Basement")
            {
                #[cfg(feature = "observe")]
                tracing::info!(
                    target: "observe",
                    foundation_name = building.foundation_name,
                    basement_lighting_created = false,
                    "basement lighting skipped — foundation is not Finished Basement"
                );
                continue;
            }
            #[cfg(feature = "observe")]
            if location == "basement" {
                tracing::info!(
                    target: "observe",
                    foundation_name = building.foundation_name,
                    basement_lighting_created = true,
                    "basement lighting created — foundation is Finished Basement"
                );
            }

            // OCHRE hpxml.py:1703-1709: garage lighting is only created when a
            // garage is modeled. A garage is modeled when a Garage zone exists —
            // either from an `Enclosure/Garages` element or from walls
            // referencing garage adjacency (ResStock style, where the zone
            // carries no floor area but OCHRE's wall-geometry-derived garage
            // area is positive, so the floor area alone must not gate this).
            // HPXML files commonly carry garage lighting groups even for
            // houses without any garage.
            if location == "garage" && !has_garage_zone(building) {
                #[cfg(feature = "observe")]
                tracing::info!(
                    target: "observe",
                    garage_lighting_created = false,
                    "garage lighting skipped — no garage is modeled"
                );
                continue;
            }
            #[cfg(feature = "observe")]
            if location == "garage" {
                tracing::info!(
                    target: "observe",
                    garage_lighting_created = true,
                    "garage lighting created — garage is modeled"
                );
            }

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
            // OCHRE hpxml.py `add_simple_schedule_params(extension, prefix)`:
            // HPXML-provided weekday/weekend fractions and month multipliers
            // (e.g. `InteriorWeekdayScheduleFractions`) take precedence over
            // the Default Schedule Parameters profiles; the schedule injector
            // prefers them via `resolve_hpxml_profile`. The usage multiplier
            // is not a schedule parameter — OCHRE applies it to the annual
            // kWh inside `parse_lighting`, so fold it in here instead of
            // leaving an unconsumed spec parameter.
            let mut usage_multiplier = 1.0;
            let mut schedule_params = Vec::new();
            for (key, value) in parse_schedule_extension_params(lighting, &capitalize(&location)) {
                if key == "usage_multiplier" {
                    usage_multiplier = value.as_f64().unwrap_or(1.0);
                } else {
                    schedule_params.push((key, value));
                }
            }
            let annual_kwh = fractions
                .explicit_kwh
                .unwrap_or_else(|| derive_lighting_annual_kwh(&location, area_ft2, &fractions))
                * usage_multiplier;

            let mut params = Map::new();
            params.insert("annual_electric_kwh".to_string(), json!(annual_kwh));
            for (key, value) in schedule_params {
                params.insert(key, value);
            }
            specs.push(build_spec(name, FuelType::Electric, params, defaults));
        }

        if let Some(ceiling_fan) = lighting.child("CeilingFan") {
            let mut params = Map::new();
            let n_bedrooms = resolve_bedroom_count_for_appliances(details);
            let n_fans = child_f64(ceiling_fan, "Count").unwrap_or(n_bedrooms + 1.0);
            let efficiency_cfm_per_w = ceiling_fan
                .path(&["Airflow", "Efficiency"])
                .and_then(|n| parse_trimmed_f64(&n.text))
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
                            n_series: None,
                            n_parallel: None,
                            cell_resistance_ohm: None,
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
                            power_factor: None,
                            charger_capacity_kva: None,
                            cc_cv_transition_soc: None,
                            charging_priority: None,
                            discharge_respects_deadline: true,
                        };
                        specs.push(build_typed_spec(
                            "Electric Vehicle".to_string(),
                            FuelType::Electric,
                            cfg,
                            defaults,
                        )?);
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
    Ok(())
}

pub(super) fn resolve_ventilation(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), super::HpxmlError> {
    let Some(vent_fans) = details.path(&["Systems", "MechanicalVentilation", "VentilationFans"])
    else {
        return Ok(());
    };

    for fan in vent_fans.children_named("VentilationFan") {
        // Only include fans used for whole-building or seasonal cooling ventilation.
        let is_whole_building = child_bool(
            fan,
            "UsedForWholeBuildingVentilation",
            "VentilationFan/UsedForWholeBuildingVentilation",
            "Ventilation Fan",
        )?
        .unwrap_or(false);
        let is_seasonal_cooling = child_bool(
            fan,
            "UsedForSeasonalCoolingLoadReduction",
            "VentilationFan/UsedForSeasonalCoolingLoadReduction",
            "Ventilation Fan",
        )?
        .unwrap_or(false);
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
            supply_fan_power_w: None,
            exhaust_fan_power_w: None,
            sensible_effectiveness: (sensible_re > 0.0).then_some(sensible_re),
            latent_effectiveness: (latent_re > 0.0).then_some(latent_re),
            bypass_temp_min_c: None,
            bypass_temp_max_c: None,
            defrost_temp_c: None,
            defrost_initial_time_fraction: None,
            defrost_time_increase_rate_per_k: None,
            ventilation_type: Some(ventilation_type.to_string()),
            balanced: Some(balanced),
            hours_in_operation: child_f64(fan, "HoursInOperation"),
        };
        specs.push(build_typed_spec(
            "Ventilation Fan".to_string(),
            FuelType::Electric,
            cfg,
            defaults,
        )?);
    }
    Ok(())
}

/// Same-type appliances are told apart by their SystemIdentifier ids (their
/// instance names number them by order), so when an HPXML lists more than
/// one of a type, each must carry a unique id.
fn require_unique_appliance_ids(appliances: &XmlNode, tag: &str) -> Result<(), HpxmlError> {
    let ids: Vec<Option<String>> = appliances.children_named(tag).map(element_id).collect();
    if ids.len() < 2 {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for id in &ids {
        let Some(id) = id else {
            return Err(HpxmlError::Parse(
                format!(
                    "{} {tag} elements need a SystemIdentifier id each to be told \
                     apart, but one has none",
                    ids.len()
                )
                .into(),
            ));
        };
        if !seen.insert(id) {
            return Err(HpxmlError::Parse(
                format!("duplicate SystemIdentifier id '{id}' among {tag} elements").into(),
            ));
        }
    }
    Ok(())
}

/// Where an HPXML appliance gives its heat.
#[derive(Debug, PartialEq)]
enum ApplianceSite {
    /// The conditioned zone, which the load finds in the dwelling's zone map.
    ConditionedSpace,
    /// A modelled zone outside conditioned space, by its 1-based id.
    Zone(u16),
    /// Outside the dwelling unit: no gain to any zone.
    OutsideUnit,
}

/// The default location of an appliance with no `Location`, as
/// OpenStudio-HPXML v1.12.0 `defaults.rb` sets it: conditioned space for
/// the washer, dryer, dishwasher, range and primary refrigerator (4204,
/// 4252, 4302, 4384, 4462); for a freezer or another refrigerator the first
/// of garage, unconditioned basement, conditioned basement and conditioned
/// space the building has (`get_freezer_or_extra_fridge_location`,
/// 5683-5697).
fn default_appliance_location(building: &Building, freezer_or_extra_fridge: bool) -> &'static str {
    if !freezer_or_extra_fridge {
        return "conditioned space";
    }
    if has_garage_zone(building) {
        return "garage";
    }
    match building.foundation_name.as_deref() {
        Some("Unfinished Basement") => "basement - unconditioned",
        Some("Finished Basement") => "basement - conditioned",
        _ => "conditioned space",
    }
}

/// The site of an appliance at an HPXML `location`. OpenStudio-HPXML
/// v1.12.0 places the conditioned locations in conditioned space and the
/// other modelled locations in their own space, and gives an appliance
/// outside the unit no space and no zone gain (`geometry.rb`
/// `get_space_from_location`, 1704-1716; `hpxml.rb` `conditioned_locations`,
/// 12311-12316; `hotwater_appliances.rb` zeroes the fractions when
/// `is_outside`). "living space" is the pre-v4 spelling of conditioned
/// space.
///
/// # Errors
///
/// A location that is not an HPXML appliance location, or one whose zone
/// the building does not model.
fn appliance_site(
    building: &Building,
    tag: &'static str,
    node: &XmlNode,
    location: &str,
) -> Result<ApplianceSite, HpxmlError> {
    let invalid = |reason: &'static str| HpxmlError::InvalidField {
        path: "Location",
        system_kind: tag,
        system_id: element_id(node).unwrap_or_else(|| "unknown".to_string()),
        value_received: location.to_string(),
        reason,
    };
    let zone_type = match location.trim().to_ascii_lowercase().as_str() {
        "conditioned space"
        | "living space"
        | "basement - conditioned"
        | "crawlspace - conditioned" => return Ok(ApplianceSite::ConditionedSpace),
        "outside"
        | "other housing unit"
        | "other heated space"
        | "other multifamily buffer space"
        | "other non-freezing space" => return Ok(ApplianceSite::OutsideUnit),
        "garage" => ZoneType::Garage,
        "basement - unconditioned" | "crawlspace - vented" | "crawlspace - unvented" => {
            ZoneType::Foundation
        }
        "attic - vented" | "attic - unvented" => ZoneType::Attic,
        _ => return Err(invalid("not an HPXML appliance location")),
    };
    // The last zone of the type wins, the dwelling's zone map rule: the
    // zone map inserts per role while it walks the zone list, so its id for
    // a role is the last zone of the role's type. Taking the first here
    // would put an appliance in a different zone than the zone map names
    // when a building ever carries two zones of one type.
    let idx = building
        .zones
        .iter()
        .rposition(|zone| zone.zone_type == zone_type)
        .ok_or_else(|| invalid("the building models no zone at this location"))?;
    u16::try_from(idx + 1)
        .map(ApplianceSite::Zone)
        .map_err(|_| invalid("zone index exceeds the zone id range"))
}

/// Default sensible and latent gain fractions per equipment name.
/// Returns (sensible_fraction, latent_fraction) of equipment power entering the zone.
///
/// Appliance, plug-load and ceiling-fan values are OpenStudio-HPXML v1.12.0's
/// for equipment inside the dwelling unit (`HPXMLtoOpenStudio/resources/`):
/// `hotwater_appliances.rb` gives the range `frac_lost` 0.20 with 0.90 of the
/// rest sensible when electric and 0.80 for any other fuel (577-584), the
/// washer `frac_lost` 0.70 and 0.90 sensible (871-874), the dishwasher
/// `frac_lost` 0.40 and 0.50 sensible (658-661), and refrigerators and
/// freezers all sensible (924-926); `defaults.rb` gives other plug loads
/// `frac_lost` 0.10 and 0.95 sensible (7524-7526) and televisions all
/// sensible (4797-4803); `hvac.rb` puts all ceiling fan power into
/// conditioned space as sensible heat (1629-1638). The dryer's split
/// depends on its venting and is set where the dryer is resolved.
pub(super) fn default_gain_fractions(name: &str, fuel_type: FuelType) -> Option<(f64, f64)> {
    match name {
        "Cooking Range" => {
            if fuel_type == FuelType::Electric {
                Some((0.72, 0.08))
            } else {
                Some((0.64, 0.16))
            }
        }
        "Clothes Washer" => Some((0.27, 0.03)),
        "Dishwasher" => Some((0.30, 0.30)),
        "Refrigerator" | "Freezer" => Some((1.00, 0.00)),
        "MELs" | "Plug Loads" => Some((0.855, 0.045)),
        "TV" => Some((1.00, 0.00)),
        // All lighting power is sensible heat (`model.rb` `add_lights`: no
        // latent or lost fraction); its radiant and visible parts are set
        // from `default_radiant_share` and `default_visible_share`.
        "Indoor Lighting" | "Exterior Lighting" | "Basement Lighting" | "Garage Lighting"
        | "Lighting" => Some((1.00, 0.00)),
        // OCHRE: gas lighting and outdoor equipment → 0 zone gain.
        "Gas Lighting" => Some((0.00, 0.00)),
        "Ceiling Fan" => Some((1.00, 0.00)),
        "Ventilation Fan" => Some((1.00, 0.00)),
        // OCHRE hpxml.py:1461-1472: electric cooking range sensible ~0.72, latent ~0.08.
        // Microwave ovens have a similar fraction of input power entering the zone
        // as sensible heat; use the same split as an electric cooking range.
        "Microwave" => Some((0.72, 0.08)),
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

/// The long-wave radiant share of an equipment's sensible heat, the rest
/// being convective: OpenStudio-HPXML v1.12.0 (`HPXMLtoOpenStudio/resources/`)
/// gives appliances `frac_radiant: 0.6 *` their sensible fraction
/// (`hotwater_appliances.rb` washer 80, dryer 121 and 132, dishwasher 172,
/// refrigerator 220, freezer 268, range 309 and 320), plug loads
/// (`misc_loads.rb` 89) and fuel loads (`misc_loads.rb` 186) the same, and
/// ceiling fans, all of whose power is sensible, `frac_radiant: 0.558`
/// (`hvac.rb` 1635). Lights in a space are `FractionRadiant` 0.6 of their
/// power (`model.rb` 249), all of it sensible.
pub(super) fn default_radiant_share(name: &str) -> Option<f64> {
    match name {
        "Cooking Range" | "Clothes Washer" | "Clothes Dryer" | "Dishwasher" | "Refrigerator"
        | "Freezer" | "MELs" | "Plug Loads" | "TV" | "Gas Fireplace" | "Gas Grill"
        | "Gas Lighting" => Some(0.6),
        "Ceiling Fan" => Some(0.558),
        "Indoor Lighting" | "Basement Lighting" | "Garage Lighting" | "Lighting" => Some(0.6),
        _ => None,
    }
}

/// The short-wave (visible) share of an equipment's sensible heat: lights
/// in a space are `FractionVisible` 0.2 of their power in OpenStudio-HPXML
/// v1.12.0 (`model.rb` 250), the rest of the 1.0 being 0.6 radiant and 0.2
/// convective. Exterior lights have no space and give no zone heat.
pub(super) fn default_visible_share(name: &str) -> Option<f64> {
    match name {
        "Indoor Lighting" | "Basement Lighting" | "Garage Lighting" | "Lighting" => Some(0.2),
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
    let vals: Vec<f64> = raw.split(',').filter_map(parse_trimmed_f64).collect();
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

/// A garage is modeled when a Garage zone exists, regardless of whether its
/// floor area is populated (ResStock-style wall-referenced garages create the
/// zone without an area). Mirrors OCHRE's positive garage floor area gate in
/// hpxml.py:1703-1709, which is derived from garage wall geometry.
fn has_garage_zone(building: &Building) -> bool {
    building.models_garage()
}

#[cfg(test)]
mod tests {
    use super::super::building::{Site, Zone, ZoneType, parse_building, parse_xml_document};
    use super::super::xml_helpers::assert_reads_xs_boolean;
    use super::*;
    use crate::defaults::DefaultsStore;

    fn conditioned_zone() -> Zone {
        Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }
    }

    fn foundation_zone() -> Zone {
        Zone {
            zone_type: ZoneType::Foundation,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }
    }

    fn garage_zone() -> Zone {
        Zone {
            zone_type: ZoneType::Garage,
            ..conditioned_zone()
        }
    }

    #[test]
    fn appliance_sites_follow_openstudio_hpxml_locations() {
        let building = appliance_test_building("", vec![conditioned_zone(), garage_zone()]);
        let node = parse_xml_document("<Freezer/>").expect("parse freezer");
        let site = |location: &str| appliance_site(&building, "Freezer", &node, location);
        for location in [
            "conditioned space",
            "living space",
            "basement - conditioned",
            "crawlspace - conditioned",
        ] {
            assert_eq!(site(location).unwrap(), ApplianceSite::ConditionedSpace);
        }
        for location in [
            "outside",
            "other housing unit",
            "other heated space",
            "other multifamily buffer space",
            "other non-freezing space",
        ] {
            assert_eq!(site(location).unwrap(), ApplianceSite::OutsideUnit);
        }
        assert_eq!(site("garage").unwrap(), ApplianceSite::Zone(2));
        assert!(
            site("basement - unconditioned").is_err(),
            "a location whose zone the building does not model is an error"
        );
        assert!(site("kitchen").is_err(), "not an HPXML appliance location");
    }

    /// Two zones of one type are not constructible from HPXML (the zone map
    /// keys each type once), but `appliance_site` must still follow the
    /// dwelling zone map's last-wins rule so the two can never disagree.
    #[test]
    fn a_second_zone_of_a_type_takes_the_zone_maps_last_wins_rule() {
        let mut first_garage = garage_zone();
        first_garage.floor_area_m2 = Some(20.0);
        let mut second_garage = garage_zone();
        second_garage.floor_area_m2 = Some(40.0);
        let building =
            appliance_test_building("", vec![conditioned_zone(), first_garage, second_garage]);
        let node = parse_xml_document("<Freezer/>").expect("parse freezer");
        let site = appliance_site(&building, "Freezer", &node, "garage").unwrap();
        assert_eq!(
            site,
            ApplianceSite::Zone(3),
            "the appliance takes the last zone of the type, the zone map's rule"
        );
    }

    fn try_resolve_appliances(
        appliances: &str,
        zones: Vec<Zone>,
    ) -> Result<Vec<EquipmentSpec>, HpxmlError> {
        let building = appliance_test_building(appliances, zones);
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)?;
        Ok(specs)
    }

    fn resolve_appliances(appliances: &str, zones: Vec<Zone>) -> Vec<EquipmentSpec> {
        try_resolve_appliances(appliances, zones).expect("appliances resolve")
    }

    fn appliance_param(
        appliances: &str,
        zones: Vec<Zone>,
        name: &str,
        key: &str,
    ) -> Result<Option<f64>, HpxmlError> {
        let specs = try_resolve_appliances(appliances, zones)?;
        let spec = specs.iter().find(|s| s.name == name).expect(name);
        Ok(param(spec, key))
    }

    fn param(spec: &EquipmentSpec, key: &str) -> Option<f64> {
        spec.parameters.get(key).and_then(Value::as_f64)
    }

    /// A garage appliance gives its heat to the garage zone; one outside the
    /// unit gives none; a freezer with no location goes to the garage when
    /// the building has one and to conditioned space otherwise, all sensible.
    #[test]
    fn appliances_give_their_heat_where_they_stand() {
        let specs = resolve_appliances(
            r#"<ClothesWasher><Location>garage</Location></ClothesWasher>
               <Dishwasher><Location>other housing unit</Location></Dishwasher>
               <Freezer/>"#,
            vec![conditioned_zone(), garage_zone()],
        );
        let find = |name: &str| specs.iter().find(|s| s.name == name).expect(name);
        let washer = find("Clothes Washer");
        assert_eq!(param(washer, "zone_id"), Some(2.0));
        assert_eq!(param(washer, "sensible_gain_fraction"), Some(0.27));
        let dishwasher = find("Dishwasher");
        assert_eq!(param(dishwasher, "sensible_gain_fraction"), Some(0.0));
        assert_eq!(param(dishwasher, "latent_gain_fraction"), Some(0.0));
        let freezer = find("Freezer");
        assert_eq!(param(freezer, "zone_id"), Some(2.0));
        assert_eq!(param(freezer, "sensible_gain_fraction"), Some(1.0));

        let specs = resolve_appliances("<Freezer/>", vec![conditioned_zone()]);
        let freezer = specs.iter().find(|s| s.name == "Freezer").expect("freezer");
        assert_eq!(param(freezer, "zone_id"), None);
        assert_eq!(param(freezer, "sensible_gain_fraction"), Some(1.0));
    }

    /// The HPXML extension's FracSensible and FracLatent land under the one
    /// parameter each fraction has, and the resolver's defaults leave them.
    #[test]
    fn extension_gain_fractions_take_the_parameter_names() {
        let specs = resolve_appliances(
            r"<Refrigerator><extension>
                <FracSensible>0.4</FracSensible><FracLatent>0.1</FracLatent>
              </extension></Refrigerator>",
            vec![conditioned_zone()],
        );
        let fridge = specs
            .iter()
            .find(|s| s.name == "Refrigerator")
            .expect("fridge");
        assert_eq!(param(fridge, "sensible_gain_fraction"), Some(0.4));
        assert_eq!(param(fridge, "latent_gain_fraction"), Some(0.1));
        assert!(!fridge.parameters.contains_key("frac_sensible"));
        assert!(!fridge.parameters.contains_key("frac_latent"));
    }

    #[test]
    fn non_electric_range_takes_the_fuel_split() {
        for fuel in [FuelType::Gas, FuelType::Propane, FuelType::Oil] {
            assert_eq!(
                default_gain_fractions("Cooking Range", fuel),
                Some((0.64, 0.16)),
                "{fuel:?}"
            );
        }
        assert_eq!(
            default_gain_fractions("Cooking Range", FuelType::Electric),
            Some((0.72, 0.08))
        );
    }

    #[test]
    fn television_and_ceiling_fan_heat_the_room() {
        for name in ["TV", "Ceiling Fan", "Freezer"] {
            assert_eq!(
                default_gain_fractions(name, FuelType::Electric),
                Some((1.0, 0.0)),
                "{name}"
            );
        }
    }

    /// The resolver carries the OpenStudio-HPXML radiant share of sensible
    /// heat, which the load applies to its final sensible fraction; a share
    /// already given is kept.
    #[test]
    fn resolver_carries_the_radiant_share() {
        let radiant = |name: &str, params: Map<String, Value>| {
            param(
                &build_spec(
                    name.to_string(),
                    FuelType::Electric,
                    params,
                    &DefaultsStore::empty(),
                ),
                "radiant_share_of_sensible",
            )
        };
        for name in ["Clothes Washer", "TV", "Refrigerator", "Indoor Lighting"] {
            assert_eq!(radiant(name, Map::new()), Some(0.6), "{name}");
        }
        assert_eq!(radiant("Ceiling Fan", Map::new()), Some(0.558));
        let mut explicit = Map::new();
        explicit.insert("radiant_share_of_sensible".to_string(), json!(0.1));
        assert_eq!(radiant("Dishwasher", explicit), Some(0.1));
        assert_eq!(radiant("Exterior Lighting", Map::new()), None);
    }

    /// Lights in a space give 0.2 of their power as visible short-wave
    /// radiation; nothing else does.
    #[test]
    fn lighting_carries_a_visible_part() {
        let visible = |name: &str| {
            param(
                &build_spec(
                    name.to_string(),
                    FuelType::Electric,
                    Map::new(),
                    &DefaultsStore::empty(),
                ),
                "visible_share_of_sensible",
            )
        };
        for name in ["Indoor Lighting", "Basement Lighting", "Garage Lighting"] {
            assert_eq!(visible(name), Some(0.2), "{name}");
        }
        for name in ["Exterior Lighting", "Cooking Range", "MELs"] {
            assert_eq!(visible(name), None, "{name}");
        }
    }

    #[test]
    fn unvented_fuel_fired_dryer_is_rejected() {
        let building = appliance_test_building(
            "<ClothesDryer><FuelType>natural gas</FuelType><Vented>false</Vented></ClothesDryer>",
            vec![conditioned_zone()],
        );
        let mut specs = Vec::new();
        let err = resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect_err("an unvented gas dryer has no flue");
        assert!(
            matches!(
                err,
                HpxmlError::InvalidField {
                    path: "ClothesDryer/Vented",
                    ..
                }
            ),
            "got: {err:?}"
        );
    }

    #[test]
    fn dryer_vented_is_an_xs_boolean() {
        let dryer = |fuel: &str, vented: &str| {
            appliance_param(
                &format!(
                    "<ClothesDryer><FuelType>{fuel}</FuelType><Vented>{vented}</Vented></ClothesDryer>"
                ),
                vec![conditioned_zone()],
                "Clothes Dryer",
                "sensible_gain_fraction",
            )
        };
        assert_reads_xs_boolean("ClothesDryer/Vented", |v| dryer("electricity", v));
        assert_eq!(
            dryer("natural gas", "1").expect("a vented gas dryer"),
            dryer("electricity", "true").expect("a vented electric dryer")
        );
    }

    #[test]
    fn range_induction_is_an_xs_boolean() {
        assert_reads_xs_boolean("CookingRange/IsInduction", |v| {
            appliance_param(
                &format!(
                    "<CookingRange><FuelType>electricity</FuelType><IsInduction>{v}</IsInduction></CookingRange>"
                ),
                vec![conditioned_zone()],
                "Cooking Range",
                "annual_electric_kwh",
            )
        });
    }

    #[test]
    fn refrigerator_primary_indicator_is_an_xs_boolean() {
        assert_reads_xs_boolean("Refrigerator/PrimaryIndicator", |v| {
            appliance_param(
                &format!("<Refrigerator><PrimaryIndicator>{v}</PrimaryIndicator></Refrigerator>"),
                vec![conditioned_zone(), garage_zone()],
                "Refrigerator",
                "zone_id",
            )
        });
    }

    #[test]
    fn ventilation_fan_uses_are_xs_booleans() {
        for (element, path) in [
            (
                "UsedForWholeBuildingVentilation",
                "VentilationFan/UsedForWholeBuildingVentilation",
            ),
            (
                "UsedForSeasonalCoolingLoadReduction",
                "VentilationFan/UsedForSeasonalCoolingLoadReduction",
            ),
        ] {
            assert_reads_xs_boolean(path, |v| {
                let details = parse_xml_document(&format!(
                    "<BuildingDetails><Systems><MechanicalVentilation><VentilationFans>\
                     <VentilationFan><{element}>{v}</{element}><FanType>exhaust only</FanType>\
                     <RatedFlowRate>50</RatedFlowRate><FanPower>10</FanPower></VentilationFan>\
                     </VentilationFans></MechanicalVentilation></Systems></BuildingDetails>"
                ))
                .expect("xml");
                let mut specs = Vec::new();
                resolve_ventilation(&details, &DefaultsStore::empty(), &mut specs)?;
                Ok(specs.len())
            });
        }
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

    #[test]
    fn bedroom_count_from_hpxml_numberofbedrooms() {
        let details = parse_xml_document(
            r#"<BuildingDetails>
                <BuildingSummary>
                    <BuildingConstruction>
                        <NumberofBedrooms>4</NumberofBedrooms>
                    </BuildingConstruction>
                </BuildingSummary>
            </BuildingDetails>"#,
        )
        .expect("parse");
        assert_eq!(resolve_bedroom_count_for_appliances(&details), 4.0);
    }

    #[test]
    fn bedroom_count_derived_from_residents() {
        let details = parse_xml_document(
            r#"<BuildingDetails>
                <BuildingSummary>
                    <BuildingOccupancy>
                        <NumberofResidents>3</NumberofResidents>
                    </BuildingOccupancy>
                </BuildingSummary>
            </BuildingDetails>"#,
        )
        .expect("parse");
        assert_eq!(resolve_bedroom_count_for_appliances(&details), 2.0);
    }

    #[test]
    fn bedroom_count_defaults_to_3_when_no_fields() {
        let details = parse_xml_document(
            r#"<BuildingDetails>
                <BuildingSummary>
                </BuildingSummary>
            </BuildingDetails>"#,
        )
        .expect("parse");
        assert_eq!(resolve_bedroom_count_for_appliances(&details), 3.0);
    }

    #[test]
    fn bedroom_count_derived_minimum_is_1() {
        // NumberofResidents=1 → max(1, 1-1) → max(1, 0) → 1
        let details = parse_xml_document(
            r#"<BuildingDetails>
                <BuildingSummary>
                    <BuildingOccupancy>
                        <NumberofResidents>1</NumberofResidents>
                    </BuildingOccupancy>
                </BuildingSummary>
            </BuildingDetails>"#,
        )
        .expect("parse");
        assert_eq!(resolve_bedroom_count_for_appliances(&details), 1.0);
    }

    #[test]
    fn occupancy_derived_from_bedrooms_when_residents_absent() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingOccupancy>
                      <extension>
                        <WeekdayScheduleFractions>0.1,0.1,0.1,0.1,0.1,0.1,0.4,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.4,0.4,0.2,0.1</WeekdayScheduleFractions>
                      </extension>
                    </BuildingOccupancy>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");

        let occ = specs
            .iter()
            .find(|s| s.name == "Occupancy")
            .expect("Occupancy spec should exist");
        let n_occ = occ
            .parameters
            .get("number_of_occupants")
            .and_then(|v| v.as_f64())
            .expect("number_of_occupants should be present");
        // NumberofBedrooms=3 → derived = 3 + 1 = 4
        assert_eq!(n_occ, 4.0);
    }

    #[test]
    fn occupancy_uses_residents_when_both_fields_present() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingOccupancy>
                      <NumberofResidents>2</NumberofResidents>
                    </BuildingOccupancy>
                    <BuildingConstruction>
                      <NumberofBedrooms>5</NumberofBedrooms>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");

        let occ = specs
            .iter()
            .find(|s| s.name == "Occupancy")
            .expect("Occupancy spec should exist");
        let n_occ = occ
            .parameters
            .get("number_of_occupants")
            .and_then(|v| v.as_f64())
            .expect("number_of_occupants should be present");
        // Both fields present → use NumberofResidents=2 directly, not bedrooms+1=6
        assert_eq!(n_occ, 2.0);
    }

    #[test]
    fn occupancy_spec_skipped_when_no_fields_and_no_extensions() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingOccupancy>
                    </BuildingOccupancy>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");

        assert!(
            !specs.iter().any(|s| s.name == "Occupancy"),
            "Occupancy spec should not be created when no occupant fields and no extension params"
        );
    }

    /// Regression: lighting specs must resolve in a deterministic order.
    ///
    /// Lighting groups are aggregated per location before one spec is pushed
    /// per location. That aggregation previously used a `HashMap`, whose
    /// iteration order varies per process/instance (`RandomState`), so the
    /// equipment spec order — and with it output column order and the
    /// float-summation order behind "Total Electric Power" — wobbled between
    /// identical runs. The aggregation now uses a `BTreeMap`, so lighting
    /// specs appear in sorted-location order on every run.
    #[test]
    fn lighting_specs_resolve_in_deterministic_sorted_location_order() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Garages>
                      <Garage>
                        <FloorArea units="ft2">400</FloorArea>
                      </Garage>
                    </Garages>
                    <Foundations>
                      <Foundation>
                        <FoundationType><Basement><Conditioned>true</Conditioned></Basement></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>garage</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <LightingGroup>
                      <Location>interior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <LightingGroup>
                      <Location>exterior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <LightingGroup>
                      <Location>basement</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");

        let resolve_names = || {
            let mut specs = Vec::new();
            resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
                .expect("resolve_scheduled_loads");
            specs
                .into_iter()
                .filter(|s| s.name.ends_with("Lighting"))
                .map(|s| s.name)
                .collect::<Vec<_>>()
        };

        let first = resolve_names();
        assert_eq!(
            first,
            vec![
                "Basement Lighting",
                "Exterior Lighting",
                "Garage Lighting",
                "Indoor Lighting",
            ],
            "lighting specs must appear in sorted-location order"
        );
        // Re-resolving must yield the identical order (run-to-run determinism).
        assert_eq!(first, resolve_names());
    }

    /// Resolves the lighting specs from an XML document and returns them
    /// keyed by name, for lighting-specific assertions.
    fn resolve_lighting_specs(xml: &str) -> Vec<EquipmentSpec> {
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");
        specs
            .into_iter()
            .filter(|s| s.name.ends_with("Lighting"))
            .collect()
    }

    /// Reads a spec parameter as a `Vec<f64>` (JSON array).
    fn param_f64s(spec: &EquipmentSpec, key: &str) -> Option<Vec<f64>> {
        spec.parameters
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
    }

    /// OCHRE hpxml.py `add_simple_schedule_params(extension, prefix)`: HPXML
    /// extension weekday/weekend fractions and month multipliers
    /// (e.g. `InteriorWeekdayScheduleFractions`) take precedence over the
    /// Default Schedule Parameters profiles, selected per location prefix.
    #[test]
    fn lighting_extension_schedule_fractions_prefered_over_defaults() {
        let weekday: String = std::iter::repeat_n("0.04", 24)
            .collect::<Vec<_>>()
            .join(",");
        let weekend: String = std::iter::repeat_n("0.02", 24)
            .collect::<Vec<_>>()
            .join(",");
        let ext_weekday: String = std::iter::repeat_n("0.07", 24)
            .collect::<Vec<_>>()
            .join(",");
        let ext_weekend: String = std::iter::repeat_n("0.03", 24)
            .collect::<Vec<_>>()
            .join(",");
        let months = "1.5,1.5,1.5,1.5,1.5,1.5,1.5,1.5,1.5,1.5,1.5,1.5";
        let ext_months = "0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5";
        let xml = format!(
            r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>interior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <LightingGroup>
                      <Location>exterior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <extension>
                      <InteriorWeekdayScheduleFractions>{weekday}</InteriorWeekdayScheduleFractions>
                      <InteriorWeekendScheduleFractions>{weekend}</InteriorWeekendScheduleFractions>
                      <InteriorMonthlyScheduleMultipliers>{months}</InteriorMonthlyScheduleMultipliers>
                      <ExteriorWeekdayScheduleFractions>{ext_weekday}</ExteriorWeekdayScheduleFractions>
                      <ExteriorWeekendScheduleFractions>{ext_weekend}</ExteriorWeekendScheduleFractions>
                      <ExteriorMonthlyScheduleMultipliers>{ext_months}</ExteriorMonthlyScheduleMultipliers>
                    </extension>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let specs = resolve_lighting_specs(&xml);

        let indoor = specs
            .iter()
            .find(|s| s.name == "Indoor Lighting")
            .expect("Indoor Lighting spec");
        assert_eq!(
            param_f64s(indoor, "weekday_schedule_fractions").as_deref(),
            Some(&[0.04; 24][..]),
            "Indoor Lighting must use InteriorWeekdayScheduleFractions"
        );
        assert_eq!(
            param_f64s(indoor, "weekend_schedule_fractions").as_deref(),
            Some(&[0.02; 24][..]),
            "Indoor Lighting must use InteriorWeekendScheduleFractions"
        );
        assert_eq!(
            param_f64s(indoor, "month_multipliers").as_deref(),
            Some(&[1.5; 12][..]),
            "Indoor Lighting must use InteriorMonthlyScheduleMultipliers"
        );

        let exterior = specs
            .iter()
            .find(|s| s.name == "Exterior Lighting")
            .expect("Exterior Lighting spec");
        assert_eq!(
            param_f64s(exterior, "weekday_schedule_fractions").as_deref(),
            Some(&[0.07; 24][..]),
            "Exterior Lighting must use ExteriorWeekdayScheduleFractions"
        );
        assert_eq!(
            param_f64s(exterior, "weekend_schedule_fractions").as_deref(),
            Some(&[0.03; 24][..]),
            "Exterior Lighting must use ExteriorWeekendScheduleFractions"
        );
        assert_eq!(
            param_f64s(exterior, "month_multipliers").as_deref(),
            Some(&[0.5; 12][..]),
            "Exterior Lighting must use ExteriorMonthlyScheduleMultipliers"
        );
    }

    /// OCHRE gates extension schedule params on the weekday key alone
    /// (`add_simple_schedule_params` returns `{}` without it); a weekend-only
    /// extension must not produce fractions. With only the weekday key, the
    /// weekend param stays absent so the schedule layer falls back to
    /// weekday-equals-weekend, matching OCHRE.
    #[test]
    fn lighting_extension_weekday_only_leaves_weekend_unset() {
        let weekday: String = std::iter::repeat_n("0.04", 24)
            .collect::<Vec<_>>()
            .join(",");
        let xml = format!(
            r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>interior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <extension>
                      <InteriorWeekdayScheduleFractions>{weekday}</InteriorWeekdayScheduleFractions>
                    </extension>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let specs = resolve_lighting_specs(&xml);
        let indoor = specs
            .iter()
            .find(|s| s.name == "Indoor Lighting")
            .expect("Indoor Lighting spec");
        assert_eq!(
            param_f64s(indoor, "weekday_schedule_fractions").as_deref(),
            Some(&[0.04; 24][..])
        );
        assert!(
            !indoor.parameters.contains_key("weekend_schedule_fractions"),
            "weekend fractions must fall back to weekday downstream, not be set here"
        );
    }

    /// Malformed extension fractions (wrong count) must be ignored with a
    /// warning rather than inserted, so the defaults profile is used.
    #[test]
    fn lighting_malformed_extension_fractions_are_ignored() {
        let short = "0.04,0.04,0.04,0.04,0.04,0.04,0.04,0.04,0.04,0.04,0.04,0.04";
        let xml = format!(
            r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>interior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                    <extension>
                      <InteriorWeekdayScheduleFractions>{short}</InteriorWeekdayScheduleFractions>
                    </extension>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let specs = resolve_lighting_specs(&xml);
        let indoor = specs
            .iter()
            .find(|s| s.name == "Indoor Lighting")
            .expect("Indoor Lighting spec");
        assert!(
            !indoor.parameters.contains_key("weekday_schedule_fractions"),
            "12-value fractions must be ignored (expected 24)"
        );
    }

    /// `UsageMultiplier` scales the annual kWh (OCHRE parse_lighting), and
    /// must not be left on the spec as an unconsumed parameter.
    #[test]
    fn lighting_extension_usage_multiplier_scales_annual_kwh() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>interior</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                      <Load>
                        <Units>kWh/year</Units>
                        <Value>1000</Value>
                      </Load>
                    </LightingGroup>
                    <extension>
                      <InteriorUsageMultiplier>1.25</InteriorUsageMultiplier>
                    </extension>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let specs = resolve_lighting_specs(xml);
        let indoor = specs
            .iter()
            .find(|s| s.name == "Indoor Lighting")
            .expect("Indoor Lighting spec");
        let annual = indoor
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh");
        assert!((annual - 1250.0).abs() < 1e-9, "annual: {annual}");
        assert!(
            !indoor.parameters.contains_key("usage_multiplier"),
            "usage multiplier must be folded into annual kWh, not left as a param"
        );
    }

    /// OCHRE hpxml.py:1703-1709: garage lighting is only created when a
    /// garage is modeled; HPXML files commonly carry garage lighting groups
    /// for houses without a garage zone.
    #[test]
    fn garage_lighting_skipped_when_no_garage_modeled() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>garage</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let specs = resolve_lighting_specs(xml);
        assert!(
            !specs.iter().any(|s| s.name == "Garage Lighting"),
            "Garage Lighting must not be created when no garage is modeled"
        );
    }

    /// Positive counterpart: a modeled garage (Enclosure/Garages) creates
    /// Garage Lighting.
    #[test]
    fn garage_lighting_created_for_modeled_garage() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Garages>
                      <Garage>
                        <FloorArea units="ft2">400</FloorArea>
                      </Garage>
                    </Garages>
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>garage</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let specs = resolve_lighting_specs(xml);
        assert!(
            specs.iter().any(|s| s.name == "Garage Lighting"),
            "Garage Lighting must be created for a modeled garage"
        );
    }

    /// ResStock-style buildings model the garage through walls with
    /// `InteriorAdjacentTo=garage` (no `Enclosure/Garages` element, so the
    /// auto-created Garage zone has no floor area). A garage referenced by
    /// walls is a modeled garage: OCHRE keeps Garage Lighting for these
    /// buildings (hpxml.py:1703-1709 gates on wall-geometry-derived garage
    /// area, which is positive), so HARES must not skip on missing floor
    /// area alone.
    #[test]
    fn garage_lighting_created_for_garage_referenced_by_walls() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls>
                      <Wall>
                        <SystemIdentifier id='GarageWall'/>
                        <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
                        <InteriorAdjacentTo>garage</InteriorAdjacentTo>
                        <WallType>
                          <WoodStud/>
                        </WallType>
                        <Area>192.0</Area>
                        <Azimuth>135</Azimuth>
                        <Insulation>
                          <SystemIdentifier id='GarageWallInsulation'/>
                          <AssemblyEffectiveRValue>4.0</AssemblyEffectiveRValue>
                        </Insulation>
                      </Wall>
                    </Walls>
                    <Foundations>
                      <Foundation>
                        <FoundationType><SlabOnGrade/></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>garage</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let specs = resolve_lighting_specs(xml);
        assert!(
            specs.iter().any(|s| s.name == "Garage Lighting"),
            "Garage Lighting must be created when walls reference a garage, \
             even without a garage floor area"
        );
    }

    /// Basement lighting must NOT be created when foundation is unconditioned.
    #[test]
    fn basement_lighting_skipped_for_unfinished_basement() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><Basement><Conditioned>false</Conditioned></Basement></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>basement</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");
        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be created for Unfinished Basement"
        );
    }

    /// Basement lighting must be created when foundation is conditioned
    /// (Finished Basement).
    #[test]
    fn basement_lighting_created_for_finished_basement() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Foundations>
                      <Foundation>
                        <FoundationType><Basement><Conditioned>true</Conditioned></Basement></FoundationType>
                      </Foundation>
                    </Foundations>
                  </Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>basement</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");
        assert!(
            specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must be created for Finished Basement"
        );
    }

    #[test]
    fn refrigerator_default_scaled_by_usage_multiplier() {
        // Refrigerator missing RatedAnnualkWh and AdjustedAnnualkWh,
        // with UsageMultiplier=2.0. Default = 637.0 + 18.0 * n_bedrooms.
        // With 3 bedrooms: default = 691.0, scaled = 1382.0.
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                  <Appliances>
                    <Refrigerator>
                      <extension>
                        <UsageMultiplier>2.0</UsageMultiplier>
                      </extension>
                    </Refrigerator>
                  </Appliances>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");

        let fridge = specs
            .iter()
            .find(|s| s.name == "Refrigerator")
            .expect("Refrigerator spec should exist");
        let kwh = fridge
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh should be present");
        // Default for 3 bedrooms: 637.0 + 18.0 * 3.0 = 691.0. Scaled by 2.0 = 1382.0.
        assert!((kwh - 1382.0).abs() < 0.01, "expected ~1382.0, got {kwh}");
    }

    #[test]
    fn freezer_default_scaled_by_usage_multiplier() {
        // Freezer missing RatedAnnualkWh and AdjustedAnnualkWh,
        // with UsageMultiplier=1.5. Default = 319.8, scaled = 479.7.
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                  <Appliances>
                    <Freezer>
                      <extension>
                        <UsageMultiplier>1.5</UsageMultiplier>
                      </extension>
                    </Freezer>
                  </Appliances>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");

        let freezer = specs
            .iter()
            .find(|s| s.name == "Freezer")
            .expect("Freezer spec should exist");
        let kwh = freezer
            .parameters
            .get("annual_electric_kwh")
            .and_then(|v| v.as_f64())
            .expect("annual_electric_kwh should be present");
        // Default = 319.8, scaled by 1.5 = 479.7.
        assert!((kwh - 479.7).abs() < 0.01, "expected ~479.7, got {kwh}");
    }

    #[test]
    fn all_appliance_energies_doubled_by_usage_multiplier() {
        // Run two identical HPXML buildings, one with UsageMultiplier=2.0
        // and one with UsageMultiplier=1.0 (baseline), and verify that
        // all six appliance annual_electric_kwh values are doubled.
        let building_xml = |multiplier: f64| -> String {
            format!(
                r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                  <Appliances>
                    <ClothesWasher>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </ClothesWasher>
                    <ClothesDryer>
                      <FuelType>electricity</FuelType>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </ClothesDryer>
                    <Dishwasher>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </Dishwasher>
                    <Refrigerator>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </Refrigerator>
                    <Freezer>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </Freezer>
                    <CookingRange>
                      <FuelType>electricity</FuelType>
                      <extension>
                        <UsageMultiplier>{multiplier}</UsageMultiplier>
                      </extension>
                    </CookingRange>
                  </Appliances>
                </BuildingDetails>
              </Building>
            </HPXML>
            "#
            )
        };

        let resolve = |xml: &str| -> BTreeMap<String, f64> {
            let building = parse_building(xml).expect("building should parse");
            let mut specs = Vec::new();
            resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
                .expect("resolve_scheduled_loads");
            specs
                .into_iter()
                .filter_map(|s| {
                    s.parameters
                        .get("annual_electric_kwh")
                        .and_then(|v| v.as_f64())
                        .map(|kwh| (s.name, kwh))
                })
                .collect()
        };

        let baseline_xml = building_xml(1.0);
        let doubled_xml = building_xml(2.0);

        let baseline = resolve(&baseline_xml);
        let doubled = resolve(&doubled_xml);

        let target_appliances = [
            "Clothes Washer",
            "Clothes Dryer",
            "Dishwasher",
            "Refrigerator",
            "Freezer",
            "Cooking Range",
        ];

        for name in target_appliances {
            let base_kwh = baseline
                .get(name)
                .unwrap_or_else(|| panic!("{name} missing from baseline"));
            let dbl_kwh = doubled
                .get(name)
                .unwrap_or_else(|| panic!("{name} missing from doubled run"));
            let expected = base_kwh * 2.0;
            assert!(
                (dbl_kwh - expected).abs() < 0.1,
                "{name}: baseline={base_kwh}, doubled={dbl_kwh}, expected 2x={expected}"
            );
        }
    }

    /// Basement lighting must NOT be created when no foundation type
    /// is specified (slab-on-grade default: foundation_name = None).
    #[test]
    fn basement_lighting_skipped_when_no_foundation() {
        let xml = r#"
            <HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                   xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site><SiteType>suburban</SiteType></Site>
                    <BuildingConstruction>
                      <ConditionedFloorArea>1000</ConditionedFloorArea>
                      <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure><Walls /></Enclosure>
                  <Lighting>
                    <LightingGroup>
                      <Location>basement</Location>
                      <LightingType><LightEmittingDiode/></LightingType>
                      <FractionofUnitsInLocation>1.0</FractionofUnitsInLocation>
                    </LightingGroup>
                  </Lighting>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = parse_building(xml).expect("building should parse");
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");
        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be created when no foundation type is specified"
        );
    }

    fn dehumidifier_test_building(zones: Vec<Zone>) -> Building {
        appliance_test_building(
            r#"<Dehumidifier>
                 <SystemIdentifier id="Dehumidifier1"/>
                 <Capacity>70</Capacity>
               </Dehumidifier>"#,
            zones,
        )
    }

    fn appliance_test_building(appliances: &str, zones: Vec<Zone>) -> Building {
        let details = parse_xml_document(&format!(
            "<BuildingDetails><Appliances>{appliances}</Appliances></BuildingDetails>"
        ))
        .expect("parse appliance details");
        Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones,
            boundaries: vec![],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: 400.0,
            ceiling_height_m: 2.5,
            infiltration_height_m: None,
            floors_above_grade: 1.0,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            climate_zone_iecc: None,
            details_xml: details,
            parse_warnings: Vec::new(),
        }
    }

    fn dehumidifier_zone_id_from_spec(spec: &EquipmentSpec) -> Option<u16> {
        spec.typed_config
            .as_ref()
            .expect("dehumidifier spec carries a typed config")
            .typed::<DehumidifierConfig>()
            .expect("typed config deserializes to DehumidifierConfig")
            .zone_id
    }

    #[test]
    fn dehumidifier_zone_id_resolves_to_conditioned_zone_not_first_zone() {
        let building = dehumidifier_test_building(vec![foundation_zone(), conditioned_zone()]);
        let mut specs = Vec::new();
        resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs)
            .expect("resolve_scheduled_loads");
        let spec = specs
            .iter()
            .find(|s| s.name == "Dehumidifier")
            .expect("dehumidifier spec resolved");
        assert_eq!(
            dehumidifier_zone_id_from_spec(spec),
            Some(2),
            "the dehumidifier must be wired to the conditioned zone's id, not the first zone"
        );
    }

    #[test]
    fn dehumidifier_errors_when_no_conditioned_zone_exists() {
        let building = dehumidifier_test_building(vec![foundation_zone()]);
        let mut specs = Vec::new();
        let result = resolve_scheduled_loads(&building, &DefaultsStore::empty(), &mut specs);
        let err = result.expect_err(
            "a dehumidifier in a building with no conditioned zone must fail resolution",
        );
        assert!(
            matches!(err, HpxmlError::NoConditionedZone { ref equipment } if equipment == "Dehumidifier"),
            "expected NoConditionedZone naming the dehumidifier, got: {err:?}"
        );
        assert!(
            specs.iter().all(|s| s.name != "Dehumidifier"),
            "the rejected dehumidifier spec must not be pushed"
        );
    }
}
