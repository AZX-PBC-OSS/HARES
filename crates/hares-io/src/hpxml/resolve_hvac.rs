//! HVAC equipment resolution from HPXML into canonical equipment specs.

use serde_json::{Map, Value, json};

use hares_types::FuelType;

use super::HpxmlError;
use super::building::{Building, DuctLocation, XmlNode, ZoneType};
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{child_f64, child_text, descendants_named};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;

/// Extract raw duct parameters for ASHRAE 152 DSE calculation.
///
/// Scans `building.zones` for non-conditioned duct systems and passes through
/// the raw parameters needed by `hares_physics::ashrae152::calculate_dse()`.
/// The HVAC equipment `init` function computes DSE from these at init time,
/// when capacity and fan flow are known.
///
/// Falls back to `AnnualDistributionSystemEfficiency` from HPXML if present.
fn compute_duct_dse_params(building: &Building) -> Map<String, Value> {
    use super::building::DuctType;

    let mut params = Map::new();
    let house_volume_m3 = building.conditioned_volume_m3.unwrap_or(400.0);

    // Check for direct DSE override from HPXML first.
    // (AnnualDistributionSystemEfficiency would be set on a per-equipment basis
    //  by the caller if available; this function handles the duct-based path.)

    // Aggregate supply vs return duct data from unconditioned zones.
    let mut supply_leakage = 0.0_f64;
    let mut supply_area_m2 = 0.0_f64;
    let mut supply_r_m2_k_w = 0.0_f64;
    let mut supply_count = 0u32;
    let mut return_leakage = 0.0_f64;
    let mut return_area_m2 = 0.0_f64;
    let mut return_r_m2_k_w = 0.0_f64;
    let mut return_count = 0u32;
    let mut duct_zone_idx: Option<usize> = None;
    let mut duct_zone_type_str: Option<String> = None;

    for (zone_idx, zone) in building.zones.iter().enumerate() {
        if matches!(zone.zone_type, ZoneType::Conditioned) {
            continue;
        }

        for duct in &zone.duct_systems {
            if matches!(duct.location, DuctLocation::InsideConditionedSpace) {
                continue;
            }

            if duct_zone_idx.is_none() {
                duct_zone_idx = Some(zone_idx);
                duct_zone_type_str = Some(zone_type_to_ashrae152_str(&zone.zone_type, zone.vented));
            }

            let leak = duct.leakage_fraction.unwrap_or(0.0);
            let area = duct.surface_area_m2.unwrap_or(0.0);
            let r_val = duct.insulation_r_value_m2_k_w.unwrap_or(0.0);

            match duct.duct_type {
                DuctType::Supply => {
                    supply_leakage += leak;
                    supply_area_m2 += area;
                    supply_r_m2_k_w = supply_r_m2_k_w.max(r_val);
                    supply_count += 1;
                }
                DuctType::Return => {
                    return_leakage += leak;
                    return_area_m2 += area;
                    return_r_m2_k_w = return_r_m2_k_w.max(r_val);
                    return_count += 1;
                }
                DuctType::Unknown => {
                    // Unknown type: split evenly between supply and return
                    supply_leakage += leak * 0.5;
                    supply_area_m2 += area * 0.5;
                    supply_r_m2_k_w = supply_r_m2_k_w.max(r_val);
                    supply_count += 1;
                    return_leakage += leak * 0.5;
                    return_area_m2 += area * 0.5;
                    return_r_m2_k_w = return_r_m2_k_w.max(r_val);
                    return_count += 1;
                }
            }
        }
    }

    if supply_count == 0 && return_count == 0 {
        return params;
    }

    let zone_idx = duct_zone_idx.unwrap_or(0);
    let zone_id = (zone_idx as u16) + 1;
    params.insert("duct_zone_id".to_string(), json!(zone_id));
    params.insert("duct_house_volume_m3".to_string(), json!(house_volume_m3));
    params.insert("duct_supply_leakage_frac".to_string(), json!(supply_leakage));
    params.insert("duct_supply_area_m2".to_string(), json!(supply_area_m2));
    params.insert("duct_supply_r_m2_k_w".to_string(), json!(supply_r_m2_k_w));
    params.insert("duct_return_leakage_frac".to_string(), json!(return_leakage));
    params.insert("duct_return_area_m2".to_string(), json!(return_area_m2));
    params.insert("duct_return_r_m2_k_w".to_string(), json!(return_r_m2_k_w));
    params.insert("duct_latitude_deg".to_string(), json!(building.site.latitude_deg));
    params.insert("duct_longitude_deg".to_string(), json!(building.site.longitude_deg));

    if let Some(zt) = duct_zone_type_str {
        params.insert("duct_zone_type".to_string(), Value::String(zt));
    }

    params
}

/// Map HPXML zone type to ASHRAE 152 zone type string.
fn zone_type_to_ashrae152_str(zt: &ZoneType, vented: bool) -> String {
    match zt {
        ZoneType::Attic => {
            if vented { "attic_vented" } else { "attic_unvented" }
        }
        ZoneType::Garage => "garage",
        ZoneType::Foundation => {
            if vented { "vent_unins_crawlspace" } else { "unvent_unins_crawlspace" }
        }
        _ => "attic_vented",
    }
    .to_string()
}

pub(super) fn resolve_hvac(building: &Building, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) -> std::result::Result<(), HpxmlError> {
    let details = &building.details_xml;
    let Some(hvac) = details.path(&["Systems", "HVAC"]) else {
        return Ok(());
    };

    // Parse thermostat setpoints from HVACControl for injection into HVAC equipment configs.
    // Each HVAC equipment self-manages its setpoint schedule using these 24h arrays.
    let setpoint_params = parse_hvac_setpoint_params(details);
    let duct_params = compute_duct_dse_params(building);

    for heating in descendants_named(hvac, "HeatingSystem") {
        let fuel = super::xml_helpers::parse_fuel(
            child_text(heating, "HeatingSystemFuel")
                .as_deref()
                .or(child_text(heating, "FuelType").as_deref()),
        );
        let system_type = parse_named_type(heating, "HeatingSystemType")
            .ok_or_else(|| HpxmlError::Parse(
                "HeatingSystem is missing required HeatingSystemType element".into(),
            ))?;
        let name = canonical_hvac_heating_name(&system_type, fuel);
        let mut params = Map::new();
        insert_capacity_kbtu_h(&mut params, heating, "HeatingCapacity");
        insert_annual_efficiency(&mut params, heating, true);
        params.insert("system_type".to_string(), Value::String(system_type));

        if let Some(frac) = child_f64(heating, "FractionHeatLoadServed")
            .or_else(|| child_f64(heating, "FractionHeatingLoadServed"))
        {
            params.insert("fraction_load_served".to_string(), json!(frac));
        }
        if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
            // OCHRE: aux_power = kwh/year / 2080 * 1000 = W
            params.insert(
                "auxiliary_power_w".to_string(),
                json!(aux_kwh / 2080.0 * 1000.0),
            );
        }
        if let Some(ext) = heating.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
        }
        for (k, v) in &setpoint_params {
            params.insert(k.clone(), v.clone());
        }
        for (k, v) in &duct_params {
            params.insert(k.clone(), v.clone());
        }
        specs.push(build_spec(name, fuel, params, defaults));
    }

    for cooling in descendants_named(hvac, "CoolingSystem") {
        let fuel = super::xml_helpers::parse_fuel(
            child_text(cooling, "CoolingSystemFuel")
                .as_deref()
                .or(Some("electricity")),
        );
        let system_type = child_text(cooling, "CoolingSystemType")
            .ok_or_else(|| HpxmlError::Parse(
                "CoolingSystem is missing required CoolingSystemType element".into(),
            ))?;
        let name = canonical_hvac_cooling_name(&system_type, fuel);
        let mut params = Map::new();
        insert_capacity_kbtu_h(&mut params, cooling, "CoolingCapacity");
        insert_capacity_w(
            &mut params,
            cooling,
            "CoolingCapacity",
            "cooling_capacity_w",
        );
        insert_annual_efficiency(&mut params, cooling, false);
        params.insert("system_type".to_string(), Value::String(system_type));
        insert_mode_and_speed_metadata(
            &mut params,
            child_text(cooling, "CompressorType").as_deref(),
        );
        apply_default_hvac_speed_fallback(&mut params);
        apply_multispeed_cooling_parameters(&mut params, defaults, &name);

        if let Some(shr) = child_f64(cooling, "SensibleHeatFraction") {
            params.insert("shr".to_string(), json!(shr));
        }
        if let Some(frac) = child_f64(cooling, "FractionCoolLoadServed")
            .or_else(|| child_f64(cooling, "FractionCoolingLoadServed"))
        {
            params.insert("fraction_load_served".to_string(), json!(frac));
        }
        if let Some(ext) = cooling.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
        }
        for (k, v) in &setpoint_params {
            params.insert(k.clone(), v.clone());
        }
        if name != "Room Air Conditioner" {
            for (k, v) in &duct_params {
                params.insert(k.clone(), v.clone());
            }
        }
        specs.push(build_spec(name, fuel, params, defaults));
    }

    for heat_pump in descendants_named(hvac, "HeatPump") {
        let heat_pump_type = child_text(heat_pump, "HeatPumpType")
            .ok_or_else(|| HpxmlError::Parse(
                "HeatPump is missing required HeatPumpType element".into(),
            ))?
            .to_ascii_lowercase();

        let split = match heat_pump_type.as_str() {
            "air-to-air" => Some(("ASHP Heater", "ASHP Cooler")),
            "mini-split" => Some(("MSHP Heater", "MSHP Cooler")),
            _ => None,
        };

        let mut params = Map::new();
        insert_capacity_kbtu_h(&mut params, heat_pump, "HeatingCapacity");
        insert_capacity_kbtu_h(&mut params, heat_pump, "CoolingCapacity");
        insert_capacity_w(
            &mut params,
            heat_pump,
            "HeatingCapacity",
            "heating_capacity_w",
        );
        insert_capacity_w(
            &mut params,
            heat_pump,
            "CoolingCapacity",
            "cooling_capacity_w",
        );
        insert_annual_efficiency(&mut params, heat_pump, true);
        insert_annual_efficiency(&mut params, heat_pump, false);
        params.insert(
            "heat_pump_type".to_string(),
            Value::String(heat_pump_type.clone()),
        );

        // Backup heating parameters
        if let Some(cap_btu) = child_f64(heat_pump, "BackupHeatingCapacity") {
            params.insert(
                "backup_capacity_w".to_string(),
                json!(conv::power_btu_h_to_w(cap_btu)),
            );
        }
        if let Some(eff_node) = heat_pump.child("BackupAnnualHeatingEfficiency") {
            if let Some(val) = child_f64(eff_node, "Value") {
                // EIR = 1/efficiency for resistance backup
                params.insert("backup_eir".to_string(), json!(1.0 / val.max(0.01)));
            }
        }
        if let Some(fuel) = child_text(heat_pump, "BackupSystemFuel") {
            params.insert("backup_fuel".to_string(), Value::String(fuel));
        }

        // Lockout temperatures (°F → °C)
        for (xml_keys, param_key) in [
            (
                &[
                    "CompressorLockoutTemperature",
                    "BackupHeatingSwitchoverTemperature",
                ][..],
                "hp_lockout_temp_c",
            ),
            (
                &[
                    "BackupHeatingLockoutTemperature",
                    "BackupHeatingSwitchoverTemperature",
                ][..],
                "er_lockout_temp_c",
            ),
        ] {
            for xml_key in xml_keys {
                if let Some(f_val) = child_f64(heat_pump, xml_key) {
                    params.insert(param_key.to_string(), json!(conv::temperature_f_to_c(f_val)));
                    break;
                }
            }
        }

        insert_mode_and_speed_metadata(
            &mut params,
            child_text(heat_pump, "CompressorType").as_deref(),
        );
        apply_default_hvac_speed_fallback(&mut params);

        if let Some(shr) = child_f64(heat_pump, "CoolingSensibleHeatFraction") {
            params.insert("shr".to_string(), json!(shr));
        }
        if let Some(frac) = child_f64(heat_pump, "FractionHeatingLoadServed") {
            params.insert("fraction_heating_load_served".to_string(), json!(frac));
        }
        if let Some(frac) = child_f64(heat_pump, "FractionCoolingLoadServed") {
            params.insert("fraction_cooling_load_served".to_string(), json!(frac));
        }
        if let Some(ext) = heat_pump.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
        }
        for (k, v) in &setpoint_params {
            params.insert(k.clone(), v.clone());
        }
        if heat_pump_type != "mini-split" {
            for (k, v) in &duct_params {
                params.insert(k.clone(), v.clone());
            }
        }

        if let Some((heater_name, cooler_name)) = split {
            let mut heater_params = params.clone();
            let mut cooler_params = params;
            apply_multispeed_heating_parameters(&mut heater_params, defaults, heater_name);
            apply_multispeed_cooling_parameters(&mut cooler_params, defaults, cooler_name);

            specs.push(build_spec(
                heater_name.to_string(),
                FuelType::Electric,
                heater_params,
                defaults,
            ));
            specs.push(build_spec(
                cooler_name.to_string(),
                FuelType::Electric,
                cooler_params,
                defaults,
            ));
        }
    }

    inject_setpoint_profiles(building, specs);
    Ok(())
}

fn is_heating_equipment(name: &str) -> bool {
    matches!(
        name,
        "ASHP Heater"
            | "MSHP Heater"
            | "Gas Furnace"
            | "Electric Furnace"
            | "Oil Furnace"
            | "Electric Baseboard"
            | "Gas Boiler"
            | "Electric Boiler"
            | "Oil Boiler"
    )
}

fn is_cooling_equipment(name: &str) -> bool {
    matches!(
        name,
        "ASHP Cooler" | "MSHP Cooler" | "Air Conditioner" | "Room Air Conditioner"
    )
}

/// Inject weekday/weekend setpoint profiles from the Building into HVAC
/// equipment specs so each equipment owns its setpoint schedule.
fn inject_setpoint_profiles(building: &Building, specs: &mut [EquipmentSpec]) {
    for spec in specs.iter_mut() {
        if is_heating_equipment(&spec.name) {
            if let Some(ref wd) = building.heating_weekday_setpoints_c {
                spec.parameters
                    .insert("heating_weekday_setpoints_c".to_string(), json!(wd));
            }
            if let Some(ref we) = building.heating_weekend_setpoints_c {
                spec.parameters
                    .insert("heating_weekend_setpoints_c".to_string(), json!(we));
            }
        }
        if is_cooling_equipment(&spec.name) {
            if let Some(ref wd) = building.cooling_weekday_setpoints_c {
                spec.parameters
                    .insert("cooling_weekday_setpoints_c".to_string(), json!(wd));
            }
            if let Some(ref we) = building.cooling_weekend_setpoints_c {
                spec.parameters
                    .insert("cooling_weekend_setpoints_c".to_string(), json!(we));
            }
        }
    }
}

fn parse_named_type(node: &XmlNode, tag: &str) -> Option<String> {
    let ty = node.child(tag)?;
    if !ty.text.trim().is_empty() {
        return Some(ty.text.trim().to_string());
    }
    ty.children
        .iter()
        .find(|child| !child.name.is_empty())
        .map(|child| child.name.clone())
}

fn canonical_hvac_heating_name(system_type: &str, fuel: FuelType) -> String {
    let ty = system_type.trim();
    match (ty, fuel) {
        ("ElectricResistance", FuelType::Electric) => "Electric Baseboard".to_string(),
        ("Furnace", FuelType::Electric)
        | ("WallFurnace", FuelType::Electric)
        | ("FloorFurnace", FuelType::Electric) => "Electric Furnace".to_string(),
        ("Boiler", FuelType::Electric) => "Electric Boiler".to_string(),
        ("Furnace", FuelType::Gas)
        | ("WallFurnace", FuelType::Gas)
        | ("FloorFurnace", FuelType::Gas) => "Gas Furnace".to_string(),
        ("Boiler", FuelType::Gas) => "Gas Boiler".to_string(),
        _ => "Generic Heater".to_string(),
    }
}

fn canonical_hvac_cooling_name(system_type: &str, _fuel: FuelType) -> String {
    match system_type.trim() {
        "central air conditioner" => "Air Conditioner".to_string(),
        "room air conditioner" => "Room Air Conditioner".to_string(),
        _ => "Generic Cooler".to_string(),
    }
}

fn insert_capacity_kbtu_h(params: &mut Map<String, Value>, node: &XmlNode, tag: &str) {
    if let Some(cap) = child_f64(node, tag) {
        params.insert(
            format!("{}_kbtu_h", tag.to_ascii_lowercase()),
            json!(conv::power_btu_h_to_kbtu_h(cap)),
        );
    }
}

fn insert_capacity_w(params: &mut Map<String, Value>, node: &XmlNode, tag: &str, key: &str) {
    if let Some(cap_btu_h) = child_f64(node, tag) {
        params.insert(key.to_string(), json!(conv::power_btu_h_to_w(cap_btu_h)));
    }
}

fn insert_annual_efficiency(params: &mut Map<String, Value>, node: &XmlNode, is_heating: bool) {
    let annual_tag = if is_heating {
        "AnnualHeatingEfficiency"
    } else {
        "AnnualCoolingEfficiency"
    };

    if let Some(annual) = node.child(annual_tag)
        && let (Some(units), Some(value)) =
            (child_text(annual, "Units"), child_f64(annual, "Value"))
    {
        let (normalized_units, normalized_value) = normalize_efficiency_units(&units, value);
        params.insert(
            if is_heating {
                "heating_efficiency_units".to_string()
            } else {
                "cooling_efficiency_units".to_string()
            },
            Value::String(normalized_units),
        );
        params.insert(
            if is_heating {
                "heating_efficiency".to_string()
            } else {
                "cooling_efficiency".to_string()
            },
            json!(normalized_value),
        );
    }

    for tag in [
        "SEER", "SEER2", "EER", "EER2", "HSPF", "HSPF2", "AFUE", "COP",
    ] {
        if let Some(value) = child_f64(node, tag) {
            let (units, normalized) = normalize_efficiency_units(tag, value);
            params.insert(
                format!("efficiency_{}", units.to_ascii_lowercase()),
                json!(normalized),
            );
        }
    }
}

fn normalize_efficiency_units(units: &str, value: f64) -> (String, f64) {
    match units.trim().to_ascii_uppercase().as_str() {
        "SEER2" => ("SEER".to_string(), value * SEER2_TO_SEER_FACTOR),
        "HSPF2" => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR),
        "SEER" | "EER" | "EER2" | "HSPF" | "AFUE" | "PERCENT" | "COP" => {
            (units.trim().to_ascii_uppercase(), value)
        }
        other => (other.to_string(), value),
    }
}

fn compressor_type_to_mode(compressor_type: &str) -> &'static str {
    match compressor_type.trim().to_ascii_lowercase().as_str() {
        "single stage" => "single_speed",
        "two stage" => "two_speed",
        "variable speed" => "variable_speed",
        _ => "single_speed",
    }
}

fn number_of_speeds_from_mode(mode: &str) -> usize {
    match mode {
        "two_speed" => 2,
        "variable_speed" => 4,
        _ => 1,
    }
}

fn mode_from_number_of_speeds(n: usize) -> &'static str {
    match n {
        2 => "two_speed",
        4 => "variable_speed",
        _ => "single_speed",
    }
}

fn insert_mode_and_speed_metadata(params: &mut Map<String, Value>, compressor_type: Option<&str>) {
    if let Some(raw) = compressor_type {
        let mode = compressor_type_to_mode(raw);
        let n_speeds = number_of_speeds_from_mode(mode);
        params.insert(
            "speed_control_mode".to_string(),
            Value::String(mode.to_string()),
        );
        params.insert("number_of_speeds".to_string(), json!(n_speeds));
    }
}

fn apply_default_hvac_speed_fallback(params: &mut Map<String, Value>) {
    if params.contains_key("number_of_speeds") {
        return;
    }
    // Prefer the bare <SEER> tag path ("efficiency_seer").
    // Fall back to HPXML 4.x AnnualCoolingEfficiency when units are SEER
    // ("cooling_efficiency" with "cooling_efficiency_units" == "SEER").
    let seer = params
        .get("efficiency_seer")
        .and_then(Value::as_f64)
        .or_else(|| {
            let units = params
                .get("cooling_efficiency_units")
                .and_then(Value::as_str)?;
            if units == "SEER" {
                params.get("cooling_efficiency").and_then(Value::as_f64)
            } else {
                None
            }
        })
        .unwrap_or(0.0);
    let n_speeds = if seer > 21.0 {
        4
    } else if seer > 15.0 {
        2
    } else {
        1
    };
    params.insert("number_of_speeds".to_string(), json!(n_speeds));
    params.insert(
        "speed_control_mode".to_string(),
        Value::String(mode_from_number_of_speeds(n_speeds).to_string()),
    );
}

fn apply_multispeed_cooling_parameters(
    params: &mut Map<String, Value>,
    defaults: &DefaultsStore,
    equipment_name: &str,
) {
    apply_multispeed_parameters(params, defaults, equipment_name, false);
}

fn apply_multispeed_heating_parameters(
    params: &mut Map<String, Value>,
    defaults: &DefaultsStore,
    equipment_name: &str,
) {
    apply_multispeed_parameters(params, defaults, equipment_name, true);
}

fn apply_multispeed_parameters(
    params: &mut Map<String, Value>,
    defaults: &DefaultsStore,
    equipment_name: &str,
    is_heating: bool,
) {
    let n_speeds = params
        .get("number_of_speeds")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;
    if n_speeds <= 1 {
        return;
    }

    let (eff_key, eff_kind, cap_key, stage_cap_prefix, stage_eir_prefix, curves) = if is_heating {
        (
            "efficiency_hspf",
            "HSPF",
            "heating_capacity_w",
            "heating_capacity_w_stage",
            "heating_eir_stage",
            defaults.hvac_heating_curves(equipment_name),
        )
    } else {
        (
            "efficiency_seer",
            "SEER",
            "cooling_capacity_w",
            "cooling_capacity_w_stage",
            "cooling_eir_stage",
            defaults.hvac_cooling_curves(equipment_name),
        )
    };

    let Some(rated_capacity_w) = params.get(cap_key).and_then(Value::as_f64) else {
        return;
    };
    let Some(efficiency_value) = params.get(eff_key).and_then(Value::as_f64) else {
        return;
    };

    let Some(multispeed) =
        defaults.hvac_multispeed_parameters(equipment_name, eff_kind, n_speeds, efficiency_value)
    else {
        return;
    };

    let stage_count = multispeed
        .capacity_ratios
        .len()
        .min(multispeed.cops.len())
        .min(n_speeds);
    if stage_count == 0 {
        return;
    }

    for i in 0..stage_count {
        let cap_w = rated_capacity_w * multispeed.capacity_ratios[i];
        params.insert(format!("{stage_cap_prefix}_{i}"), json!(cap_w));
        let cop = multispeed.cops[i].max(1e-6);
        params.insert(format!("{stage_eir_prefix}_{i}"), json!(1.0 / cop));
    }

    if let Some(curve_set) = curves {
        if let Some(coeff_text) = serialize_stage_plr_coefficients(curve_set, n_speeds) {
            params.insert(
                "eir_plr_coefficients".to_string(),
                Value::String(coeff_text),
            );
        }
        if let Some((cap, eir)) = select_primary_curve_pair(curve_set, n_speeds) {
            params.insert(
                "capacity_biquadratic_coeffs".to_string(),
                Value::String(format!("{:?}", cap)),
            );
            params.insert(
                "eir_biquadratic_coeffs".to_string(),
                Value::String(format!("{:?}", eir)),
            );
        }
    }
}

fn serialize_stage_plr_coefficients(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Option<String> {
    let variants = select_variants_for_speed_count(curve_set, n_speeds);
    if variants.is_empty() {
        return None;
    }
    let coeffs: Vec<String> = variants
        .iter()
        .flat_map(|v| v.eir_plr.iter())
        .map(|x| x.to_string())
        .collect();
    Some(format!("[{}]", coeffs.join(", ")))
}

fn select_primary_curve_pair(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Option<([f64; 6], [f64; 6])> {
    let variants = select_variants_for_speed_count(curve_set, n_speeds);
    let selected = variants.last().copied()?;
    Some((selected.cap_t.coeffs, selected.eir_t.coeffs))
}

fn select_variants_for_speed_count(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Vec<&crate::defaults::HvacCurveVariant> {
    let mut matches: Vec<&crate::defaults::HvacCurveVariant> = curve_set
        .variants
        .iter()
        .filter(|v| {
            let name = v.name.to_ascii_lowercase();
            match n_speeds {
                1 => name.starts_with("single_"),
                2 => name.starts_with("double_") || name.starts_with("two_"),
                4 => name.starts_with("variable_"),
                _ => false,
            }
        })
        .collect();

    matches.sort_by_key(|variant| {
        let lower = variant.name.to_ascii_lowercase();
        lower
            .split('_')
            .next_back()
            .and_then(|p| p.parse::<usize>().ok())
            .unwrap_or(usize::MAX)
    });
    matches
}

/// Parse HVACControl setpoints and return them as JSON key-value pairs
/// ready for injection into HVAC equipment config.
///
/// Delegates to the shared `xml_helpers::parse_setpoint_from_control` for
/// the actual XML parsing logic.
fn parse_hvac_setpoint_params(details: &XmlNode) -> Vec<(String, Value)> {
    let mut out = Vec::new();

    let Some(control) = super::xml_helpers::find_hvac_control(details) else {
        return out;
    };

    for (hvac_type, param_prefix) in [("Heating", "heating"), ("Cooling", "cooling")] {
        for (weekday, day_suffix) in [(true, "weekday"), (false, "weekend")] {
            let param_key = format!("{param_prefix}_{day_suffix}_setpoints_c");
            if let Some(vals) = super::xml_helpers::parse_setpoint_from_control(control, hvac_type, weekday) {
                out.push((param_key, json!(vals)));
            }
        }
    }

    out
}
