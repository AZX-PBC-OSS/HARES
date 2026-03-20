//! HPXML equipment resolution into canonical OCHRE-style equipment specs.

use std::collections::HashMap;

use hares_types::FuelType;
use serde_json::{Map, Value, json};

use super::HpxmlError;
use super::building::{Building, DuctLocation, XmlNode, ZoneType};
use super::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};
use crate::defaults::{DefaultsStore, ZipParameters};
use crate::draw_profile::{DistributionSystem, FixtureEfficiency, combined_daily_hot_water_l};

const BTU_PER_HOUR_TO_KBTU_PER_HOUR: f64 = 0.001;
const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;
const M2_TO_FT2: f64 = 10.763_910_416_709_722;
/// OCHRE EV fuel economy: 1/325 * 1000 miles per kWh (for sedans).
const EV_FUEL_ECONOMY: f64 = 1000.0 / 325.0;

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentSpec {
    pub name: String,
    pub fuel_type: FuelType,
    pub parameters: Map<String, Value>,
    pub zip_params: Option<ZipParameters>,
}

pub fn resolve_equipment(
    building: &Building,
    defaults: &DefaultsStore,
    overrides: &Value,
) -> std::result::Result<Vec<EquipmentSpec>, HpxmlError> {
    let mut specs = Vec::new();
    let details = &building.details_xml;

    resolve_hvac(building, defaults, &mut specs)?;
    resolve_water_heaters(details, defaults, &mut specs);
    resolve_pv(details, defaults, &mut specs);
    resolve_batteries(details, defaults, &mut specs);
    resolve_ev(details, defaults, &mut specs);
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

/// Compute simplified ASHRAE 152 Distribution System Efficiency from parsed duct data.
///
/// Scans `building.zones` for the first non-conditioned zone containing duct systems
/// and returns `(duct_dse, duct_zone_index)` params for injection into HVAC equipment
/// configs. The zone index is 1-based (matching `ZoneId(u16)`).
///
/// Simplified DSE per OCHRE:
///   DSE = 1 - leakage_loss - conduction_loss
///
/// - leakage_loss: `leakage_fraction * 0.5` (half supply, half return averaging)
/// - conduction_loss: `surface_area_m2 / (r_value_m2_k_w * conditioned_volume_m3)`
///   scaled by an assumed delta-T ratio of 0.1 (10% of conditioned-to-duct temp difference).
///   Only applied when both surface area and R-value are available.
fn compute_duct_dse_params(building: &Building) -> Map<String, Value> {
    let mut params = Map::new();

    let conditioned_volume_m3 = building.conditioned_volume_m3.unwrap_or(0.0);

    for (zone_idx, zone) in building.zones.iter().enumerate() {
        if matches!(zone.zone_type, ZoneType::Conditioned) {
            continue;
        }

        for duct in &zone.duct_systems {
            if matches!(duct.location, DuctLocation::InsideConditionedSpace) {
                continue;
            }

            let mut dse = 1.0_f64;

            // Leakage loss: half-split between supply and return
            if let Some(leak) = duct.leakage_fraction {
                dse -= leak * 0.5;
            }

            // Conduction loss from duct surface area and R-value
            if let Some(area_m2) = duct.surface_area_m2 {
                if let Some(r_val) = duct.insulation_r_value_m2_k_w {
                    if r_val > 0.0 && conditioned_volume_m3 > 0.0 {
                        // UA = area / R-value [W/K]. Normalize by a reference capacity
                        // derived from house volume (rough proxy for system size).
                        // Factor 0.1 accounts for seasonal avg delta-T fraction.
                        let ua_w_k = area_m2 / r_val;
                        let ref_capacity_w = conditioned_volume_m3 * 40.0; // ~40 W/m3 sizing rule
                        let conduction_loss = (ua_w_k * 10.0) / ref_capacity_w; // delta-T ~10K
                        dse -= conduction_loss;
                    }
                }
            }

            dse = dse.clamp(0.3, 1.0);

            if dse < 1.0 {
                params.insert("duct_dse".to_string(), json!(dse));
                // zone_idx is 0-based; ZoneId is 1-based
                let zone_id = (zone_idx as u16) + 1;
                params.insert("duct_zone_id".to_string(), json!(zone_id));
                return params;
            }
        }
    }

    params
}

fn resolve_hvac(building: &Building, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) -> std::result::Result<(), HpxmlError> {
    let details = &building.details_xml;
    let Some(hvac) = details.path(&["Systems", "HVAC"]) else {
        return Ok(());
    };

    // Parse thermostat setpoints from HVACControl for injection into HVAC equipment configs.
    // Each HVAC equipment self-manages its setpoint schedule using these 24h arrays.
    let setpoint_params = parse_hvac_setpoint_params(details);
    let duct_params = compute_duct_dse_params(building);

    for heating in descendants_named(hvac, "HeatingSystem") {
        let fuel = parse_fuel(
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
        let fuel = parse_fuel(
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
                json!(cap_btu * 0.293_071_07),
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
                    params.insert(param_key.to_string(), json!((f_val - 32.0) / 1.8));
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

fn resolve_water_heaters(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    // Shared draw parameters parsed once from the WaterHeating section.
    let avg_water_draw_l_per_day = parse_avg_water_draw_l_per_day(details);

    for wh in descendants_named(details, "WaterHeatingSystem") {
        let fuel = parse_fuel(child_text(wh, "FuelType").as_deref());
        let wh_type = child_text(wh, "WaterHeaterType").unwrap_or_default();
        let name = canonical_water_heater_name(&wh_type, fuel);

        let energy_factor = child_f64(wh, "EnergyFactor");
        let uniform_energy_factor = child_f64(wh, "UniformEnergyFactor");
        let tank_volume_rated_gal = child_f64(wh, "TankVolume");
        let first_hour_rating_gal = child_f64(wh, "FirstHourRating");
        let heating_capacity_btu_hr = child_f64(wh, "HeatingCapacity");
        let recovery_efficiency = child_f64(wh, "RecoveryEfficiency");

        let mut params = Map::new();
        if let Some(gal) = tank_volume_rated_gal {
            params.insert("tank_volume_gal".to_string(), json!(gal));
        }
        if let Some(gal) = first_hour_rating_gal {
            params.insert("first_hour_rating_gal".to_string(), json!(gal));
        }
        if let Some(temp_c) = child_temperature_c(wh) {
            params.insert("setpoint_c".to_string(), json!(temp_c));
        }
        if let Some(ef) = energy_factor {
            params.insert("energy_factor".to_string(), json!(ef));
        }
        if let Some(uef) = uniform_energy_factor {
            params.insert("uniform_energy_factor".to_string(), json!(uef));
        }
        if let Some(cap) = heating_capacity_btu_hr {
            params.insert(
                "heating_capacity_kbtu_h".to_string(),
                json!(cap * BTU_PER_HOUR_TO_KBTU_PER_HOUR),
            );
        }
        params.insert(
            "water_heater_type".to_string(),
            Value::String(wh_type.clone()),
        );

        // Average daily hot water draw: used to normalize fractional draw schedules.
        if let Some(avg_l) = avg_water_draw_l_per_day {
            params.insert("avg_water_draw_l_per_day".to_string(), json!(avg_l));
        }

        // Derive tank UA from EF/UEF using the DOE 10 CFR 430 standby-loss test.
        let category = wh_category(&wh_type, fuel);
        let ua_inputs = UaInputs {
            category,
            energy_factor,
            uniform_energy_factor,
            tank_volume_rated_gal,
            recovery_efficiency,
            heating_capacity_btu_hr,
            first_hour_rating_gal,
        };
        match ua_from_energy_factor(&ua_inputs) {
            Ok(Some(ua_result)) => {
                params.insert("ua_w_per_k".to_string(), json!(ua_result.ua_w_per_k));
            }
            Ok(None) => {
                // Instantaneous or other type with no tank: leave ua_w_per_k absent.
            }
            Err(_) => {
                // Inputs are inconsistent (e.g. missing capacity for gas, negative UA).
                // Do not insert ua_w_per_k; the equipment model will fall back to its
                // default. The simulation log would surface this at a higher layer.
            }
        }

        // HPWH COP from UEF
        if wh_type.contains("heat pump") {
            if let Some(uef) = uniform_energy_factor {
                params.insert("cop".to_string(), json!(1.174_536_058 * uef));
            }
            // HPWH tempering valve: storage at 60°C (140°F), delivery at 51.67°C (125°F)
            let storage_setpoint_c = params
                .get("setpoint_c")
                .and_then(|v| v.as_f64())
                .unwrap_or(60.0);
            if storage_setpoint_c > 51.67 {
                params.insert("tempering_valve_setpoint_c".to_string(), json!(51.67));
            }
        }

        // Tank jacket R-value
        if let Some(jacket_r) = wh
            .path(&["WaterHeaterInsulation", "Jacket", "JacketRValue"])
            .and_then(|n| n.text.trim().parse::<f64>().ok())
        {
            // Convert hr·ft²·°F/BTU → m²·K/W
            params.insert(
                "jacket_r_value_m2_k_w".to_string(),
                json!(jacket_r * 0.176_110_184),
            );
        }

        // Tankless performance adjustment
        if wh_type.contains("instantaneous") {
            let perf_adj = child_f64(wh, "PerformanceAdjustment").unwrap_or(0.92);
            params.insert("performance_adjustment".to_string(), json!(perf_adj));
        }

        // Water heater location
        if let Some(location) = child_text(wh, "Location") {
            params.insert("location".to_string(), Value::String(location));
        }

        specs.push(build_spec(name, fuel, params, defaults));
    }
}

/// Parse the combined average daily hot water draw [L/day] from the `<BuildingDetails>` node.
///
/// Reads `<BuildingSummary>/<BuildingConstruction>/<NumberofBedrooms>`,
/// `<WaterHeating>/<WaterFixture>/<LowFlow>`, the `<WaterFixturesUsageMultiplier>` extension,
/// and `<HotWaterDistribution>` to compute the OCHRE/ANSI-RESNET 301 draw estimate.
///
/// Returns `None` when bedroom count is absent (required for the formula).
fn parse_avg_water_draw_l_per_day(details: &XmlNode) -> Option<f64> {
    // Bedroom count is required; without it the formula cannot be evaluated.
    let n_bedrooms = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofBedrooms",
        ])
        .and_then(|n| n.text.trim().parse::<f64>().ok())?;

    // Fixture efficiency: low-flow if any WaterFixture has <LowFlow>true</LowFlow>.
    let fixture_efficiency = if details
        .path(&["WaterHeating"])
        .map(|wh_section| {
            descendants_named(wh_section, "WaterFixture")
                .iter()
                .any(|f| child_text(f, "LowFlow").is_some_and(|v| v.eq_ignore_ascii_case("true")))
        })
        .unwrap_or(false)
    {
        FixtureEfficiency::LowFlow
    } else {
        FixtureEfficiency::Standard
    };

    // Usage multiplier from the WaterHeating extension (OCHRE: WaterFixturesUsageMultiplier).
    let usage_multiplier = details
        .path(&["WaterHeating", "extension"])
        .and_then(|ext| child_f64(ext, "WaterFixturesUsageMultiplier"))
        .unwrap_or(1.0);

    // Distribution system.
    let distribution = parse_distribution_system(details, n_bedrooms);

    Some(combined_daily_hot_water_l(
        n_bedrooms,
        fixture_efficiency,
        usage_multiplier,
        &distribution,
    ))
}

/// Parse `<HotWaterDistribution>` from the `<WaterHeating>` section.
///
/// Falls back to `DistributionSystem::Unknown` when the section is absent or the type
/// is unrecognised.  Default piping length for Standard distribution is derived from
/// conditioned floor area and number of floors following the OCHRE formula.
fn parse_distribution_system(details: &XmlNode, n_bedrooms: f64) -> DistributionSystem {
    let Some(dist_node) = details.path(&["WaterHeating", "HotWaterDistribution"]) else {
        return DistributionSystem::Unknown;
    };

    let pipe_r_value = dist_node
        .path(&["PipeInsulation"])
        .and_then(|n| child_f64(n, "PipeRValue"))
        .unwrap_or(0.0);

    let system_type = dist_node.child("SystemType");

    if let Some(standard) = system_type.and_then(|n| n.child("Standard")) {
        // Default piping length: OCHRE formula using floor area and floors.
        // 2 * sqrt(floor_area_ft2 / floors) + 10 * floors + 5 * has_unfinished_basement
        // We use a bedroom-count proxy when floor area data is not available.
        let default_piping_length_m = derive_default_piping_length_m(details, n_bedrooms);
        let piping_length_m = child_f64(standard, "PipingLength").map(|ft| ft * 0.3048); // HPXML PipingLength is in feet
        DistributionSystem::Standard {
            pipe_r_value,
            piping_length_m,
            default_piping_length_m,
        }
    } else if let Some(recirc) = system_type.and_then(|n| n.child("Recirculation")) {
        // BranchPipingLoopLength is in feet in HPXML.
        let branch_loop_length_m =
            child_f64(recirc, "BranchPipingLoopLength").map(|ft| ft * 0.3048);
        DistributionSystem::Recirculation {
            pipe_r_value,
            branch_loop_length_m,
        }
    } else {
        DistributionSystem::Unknown
    }
}

/// Derive the OCHRE default piping length [m] from building geometry.
///
/// OCHRE formula (converted to SI):
/// `default_length_ft = 2 * sqrt(floor_area_ft2 / floors) + 10 * floors + 5 * has_unfinished_basement`
///
/// Falls back to a bedroom-count proxy (25 + 5 * n_bedrooms ft ≈ 7.6 + 1.5 * n_bedrooms m)
/// when floor area data is unavailable.
fn derive_default_piping_length_m(details: &XmlNode, n_bedrooms: f64) -> f64 {
    const FT_TO_M: f64 = 0.3048;
    const FT2_TO_M2: f64 = 0.092_903_04;

    let floor_area_m2 = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "ConditionedFloorArea",
        ])
        .and_then(|n| {
            let val = n.text.trim().parse::<f64>().ok()?;
            // HPXML ConditionedFloorArea may carry a `units` attribute; default is ft2.
            let units = n.attrs.get("units").map(String::as_str).unwrap_or("ft2");
            if units.eq_ignore_ascii_case("m2") {
                Some(val)
            } else {
                Some(val * FT2_TO_M2)
            }
        });

    let n_floors = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofConditionedFloors",
        ])
        .or_else(|| {
            details.path(&[
                "BuildingSummary",
                "BuildingConstruction",
                "NumberofConditionedFloorsAboveGrade",
            ])
        })
        .and_then(|n| n.text.trim().parse::<f64>().ok())
        .unwrap_or(1.0)
        .max(1.0);

    let has_unfinished_bsmt = details
        .path(&["BuildingSummary", "BuildingConstruction", "FoundationType"])
        .map(|n| n.text.trim().eq_ignore_ascii_case("UnfinishedBasement"))
        .unwrap_or(false);

    if let Some(area_m2) = floor_area_m2 {
        let area_ft2 = area_m2 / FT2_TO_M2;
        let ft_per_floor = area_ft2 / n_floors;
        let default_ft = 2.0 * ft_per_floor.sqrt()
            + 10.0 * n_floors
            + if has_unfinished_bsmt { 5.0 } else { 0.0 };
        default_ft * FT_TO_M
    } else {
        // Bedroom-count proxy when floor area is absent.
        (25.0 + 5.0 * n_bedrooms) * FT_TO_M
    }
}

fn wh_category(wh_type: &str, fuel: FuelType) -> WhCategory {
    match (wh_type.trim(), fuel) {
        ("heat pump water heater", FuelType::Electric) => WhCategory::HeatPump,
        ("instantaneous water heater", _) => WhCategory::Instantaneous,
        (_, FuelType::Electric) => WhCategory::StorageElectric,
        _ => WhCategory::StorageGas,
    }
}

fn resolve_pv(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    let Some(photovoltaics) = details.path(&["Systems", "Photovoltaics"]) else {
        return;
    };

    let mut inverter_eff_by_id: HashMap<String, f64> = HashMap::new();
    for inverter in children_named(photovoltaics, "Inverter") {
        if let (Some(id), Some(eff)) = (
            element_id(inverter),
            child_f64(inverter, "InverterEfficiency"),
        ) {
            inverter_eff_by_id.insert(id, eff);
        }
    }

    for pv in children_named(photovoltaics, "PVSystem") {
        let mut params = Map::new();
        if let Some(kw) = child_f64(pv, "MaxPowerOutput") {
            params.insert("system_capacity_kw".to_string(), json!(kw));
        }
        if let Some(tilt) = child_f64(pv, "ArrayTilt") {
            params.insert("tilt_deg".to_string(), json!(tilt));
        }
        if let Some(az) = child_f64(pv, "ArrayAzimuth") {
            params.insert("azimuth_deg".to_string(), json!(az));
        }
        if let Some(module_type) = child_text(pv, "ModuleType") {
            params.insert("module_type".to_string(), Value::String(module_type));
        }

        let inverter_eff = pv
            .child("AttachedToInverter")
            .and_then(|n| n.attrs.get("idref"))
            .and_then(|id| inverter_eff_by_id.get(id))
            .copied();
        if let Some(eff) = inverter_eff {
            params.insert("inverter_efficiency".to_string(), json!(eff));
        }

        specs.push(build_spec(
            "PV".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
    }
}

fn resolve_batteries(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    let Some(batteries) = details.path(&["Systems", "Batteries"]) else {
        return;
    };

    for battery in children_named(batteries, "Battery") {
        let mut params = Map::new();
        if let Some(kwh) = child_energy_kwh(battery, "NominalCapacity") {
            params.insert("capacity_kwh".to_string(), json!(kwh));
        }
        if let Some(kw) = child_f64(battery, "RatedPowerOutput") {
            params.insert("max_charge_kw".to_string(), json!(kw));
            params.insert("max_discharge_kw".to_string(), json!(kw));
        }
        if let Some(rte) = child_f64(battery, "RoundTripEfficiency") {
            // Round-trip efficiency is the product of charge and discharge
            // efficiencies; each one-way efficiency is the square root.
            params.insert("inverter_efficiency".to_string(), json!(rte.sqrt()));
        }
        specs.push(build_spec(
            "Battery".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
    }
}

fn resolve_ev(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    if let Some(evs) = details.path(&["Systems", "ElectricVehicles"]) {
        for ev in children_named(evs, "ElectricVehicle") {
            let mut params = Map::new();
            if let Some(level) = child_text(ev, "ChargingLevel") {
                params.insert("ChargingLevel".to_string(), Value::String(level));
            }
            if let Some(power_kw) = child_f64(ev, "MaxChargingPower") {
                params.insert("MaxChargingPower".to_string(), json!(power_kw));
            }
            if let Some(kwh) = child_energy_kwh(ev, "BatteryCapacity") {
                params.insert("BatteryCapacity".to_string(), json!(kwh));
            }
            specs.push(build_spec(
                "Electric Vehicle".to_string(),
                FuelType::Electric,
                params,
                defaults,
            ));
        }
    }
}

fn resolve_scheduled_loads(
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
            for node in children_named(appliances, tag) {
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
        let indoor_area_ft2 = conditioned_floor_area_m2(building) * M2_TO_FT2;
        let foundation_area_ft2 = foundation_floor_area_m2(building) * M2_TO_FT2;
        let garage_area_ft2 = garage_floor_area_m2(building) * M2_TO_FT2;

        let mut by_location: HashMap<String, LightingFractions> = HashMap::new();
        for group in children_named(lighting, "LightingGroup") {
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
        for plug in children_named(misc_loads, "PlugLoad") {
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

        for fuel_load in children_named(misc_loads, "FuelLoad") {
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
            for entry in children_named(group, item) {
                for (pump_container, pump_item, schedule_name) in [
                    ("PoolPumps", "PoolPump", pump_name),
                    ("SpaPumps", "SpaPump", pump_name),
                ] {
                    if let Some(pumps) = entry.child(pump_container) {
                        for pump in children_named(pumps, pump_item) {
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

fn resolve_ventilation(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(vent_fans) = details.path(&["Systems", "MechanicalVentilation", "VentilationFans"])
    else {
        return;
    };

    for fan in children_named(vent_fans, "VentilationFan") {
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
fn default_gain_fractions(name: &str, fuel_type: FuelType) -> Option<(f64, f64)> {
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
        "Clothes Dryer" => Some((0.15, 0.05)),
        "Clothes Washer" => Some((0.80, 0.00)),
        "Dishwasher" => Some((0.60, 0.15)),
        "Refrigerator" | "Freezer" => Some((1.00, 0.00)),
        "MELs" | "Plug Loads" | "TV" => Some((0.73, 0.02)),
        "Indoor Lighting" | "Exterior Lighting" | "Basement Lighting" | "Garage Lighting"
        | "Lighting" | "Gas Lighting" => Some((0.70, 0.00)),
        "Ceiling Fan" | "Ventilation Fan" => Some((1.00, 0.00)),
        _ => None,
    }
}

fn build_spec(
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
        "room air conditioner" => "Room AC".to_string(),
        _ => "Generic Cooler".to_string(),
    }
}

fn canonical_water_heater_name(wh_type: &str, fuel: FuelType) -> String {
    match (wh_type.trim(), fuel) {
        ("storage water heater", FuelType::Electric) => {
            "Electric Resistance Water Heater".to_string()
        }
        ("instantaneous water heater", FuelType::Electric) => "Tankless Water Heater".to_string(),
        ("heat pump water heater", FuelType::Electric) => "Heat Pump Water Heater".to_string(),
        ("storage water heater", FuelType::Gas) => "Gas Water Heater".to_string(),
        ("instantaneous water heater", FuelType::Gas) => "Gas Tankless Water Heater".to_string(),
        _ => "Water Heating".to_string(),
    }
}

fn parse_fuel(raw: Option<&str>) -> FuelType {
    match raw
        .unwrap_or("electricity")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "electricity" | "electric" | "none" => FuelType::Electric,
        "natural gas" | "natural_gas" | "gas" => FuelType::Gas,
        "propane" => FuelType::Propane,
        "oil" | "fuel oil" | "fuel_oil" => FuelType::Oil,
        _ => FuelType::Electric,
    }
}

fn insert_capacity_kbtu_h(params: &mut Map<String, Value>, node: &XmlNode, tag: &str) {
    if let Some(cap) = child_f64(node, tag) {
        params.insert(
            format!("{}_kbtu_h", tag.to_ascii_lowercase()),
            json!(cap * BTU_PER_HOUR_TO_KBTU_PER_HOUR),
        );
    }
}

fn insert_capacity_w(params: &mut Map<String, Value>, node: &XmlNode, tag: &str, key: &str) {
    if let Some(cap_btu_h) = child_f64(node, tag) {
        params.insert(key.to_string(), json!(cap_btu_h * 0.293_071_07));
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
    let seer = params
        .get("efficiency_seer")
        .and_then(Value::as_f64)
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

fn element_id(node: &XmlNode) -> Option<String> {
    node.child("SystemIdentifier")
        .and_then(|id_node| id_node.attrs.get("id"))
        .cloned()
}

fn child_text(node: &XmlNode, child_name: &str) -> Option<String> {
    node.child(child_name).map(|n| n.text.trim().to_string())
}

fn child_f64(node: &XmlNode, child_name: &str) -> Option<f64> {
    node.child(child_name)
        .and_then(|n| n.text.trim().parse::<f64>().ok())
}

fn child_temperature_c(node: &XmlNode) -> Option<f64> {
    let temp = node
        .child("HotWaterTemperature")
        .or_else(|| node.child("Temperature"))?;
    let value = temp.text.trim().parse::<f64>().ok()?;
    let units = temp
        .attrs
        .get("units")
        .map(String::as_str)
        .unwrap_or("F")
        .to_ascii_lowercase();
    Some(
        if units == "f" || units == "degf" || units == "fahrenheit" {
            (value - 32.0) * (5.0 / 9.0)
        } else {
            value
        },
    )
}

fn child_energy_kwh(node: &XmlNode, child_name: &str) -> Option<f64> {
    let target = node.child(child_name)?;
    let value = child_f64(target, "Value")?;
    let units = child_text(target, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "kwh" | "kwh/year" | "kwh/yr" => Some(value),
        "wh" | "wh/year" | "wh/yr" => Some(value / 1000.0),
        _ => None,
    }
}

fn child_load_kwh(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units = child_text(load, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "kwh/year" | "kwh/yr" | "kwh" => Some(value),
        _ => None,
    }
}

fn child_load_therms(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units = child_text(load, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "therm/year" | "therm/yr" | "therm" => Some(value),
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
            eprintln!(
                "[WARN] {key} has {} values (expected 12); ignoring",
                vals.len()
            );
        }
        None
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.push_str(chars.as_str());
    out
}

fn conditioned_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, super::building::ZoneType::Conditioned))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

fn foundation_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, super::building::ZoneType::Foundation))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

fn garage_floor_area_m2(building: &Building) -> f64 {
    building
        .zones
        .iter()
        .find(|z| matches!(z.zone_type, super::building::ZoneType::Garage))
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0)
}

fn children_named<'a>(node: &'a XmlNode, name: &'a str) -> impl Iterator<Item = &'a XmlNode> {
    node.children.iter().filter(move |child| child.name == name)
}

fn descendants_named<'a>(node: &'a XmlNode, name: &'a str) -> Vec<&'a XmlNode> {
    let mut out = Vec::new();
    collect_descendants(node, name, &mut out);
    out
}

fn collect_descendants<'a>(node: &'a XmlNode, name: &str, out: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        out.push(node);
    }
    for child in &node.children {
        collect_descendants(child, name, out);
    }
}

/// Parse HVACControl setpoints and return them as JSON key-value pairs
/// ready for injection into HVAC equipment config.
fn parse_hvac_setpoint_params(details: &XmlNode) -> Vec<(String, Value)> {
    let mut out = Vec::new();

    // Locate HVACControl: prefer one nested under HVACPlant or HVAC, else any descendant.
    let control: Option<&XmlNode> = descendants_named(details, "HVACPlant")
        .into_iter()
        .find_map(|p| p.child("HVACControl"))
        .or_else(|| {
            descendants_named(details, "HVAC")
                .into_iter()
                .find_map(|p| p.child("HVACControl"))
        })
        .or_else(|| descendants_named(details, "HVACControl").into_iter().next());

    let Some(control) = control else {
        return out;
    };

    for (hvac_type, param_prefix) in [("Heating", "heating"), ("Cooling", "cooling")] {
        for (day_type, day_suffix) in [("Weekday", "weekday"), ("Weekend", "weekend")] {
            let ext_key = format!("{day_type}SetpointTemps{hvac_type}Season");
            let param_key = format!("{param_prefix}_{day_suffix}_setpoints_c");

            if let Some(ext) = control.child("extension") {
                if let Some(node) = ext.child(&ext_key) {
                    let vals: Vec<f64> = node
                        .text
                        .trim()
                        .split(',')
                        .filter_map(|s: &str| s.trim().parse::<f64>().ok())
                        .map(|f| (f - 32.0) / 1.8)
                        .collect();
                    if vals.len() == 24 {
                        out.push((param_key, json!(vals)));
                        continue;
                    }
                }
            }

            // Fallback: constant setpoint for the season
            let const_key = format!("SetpointTemp{hvac_type}Season");
            if let Some(node) = control.child(&const_key) {
                if let Ok(f_val) = node.text.trim().parse::<f64>() {
                    let c_val = (f_val - 32.0) / 1.8;
                    out.push((param_key, json!(vec![c_val; 24])));
                }
            }
        }
    }

    out
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
            eprintln!(
                "[WARN] {weekday_key} has {} values (expected 24); ignoring",
                vals.len()
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
            eprintln!(
                "[WARN] {weekend_key} has {} values (expected 24); ignoring",
                vals.len()
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
            eprintln!(
                "[WARN] {month_key} has {} values (expected 12); ignoring",
                vals.len()
            );
        }
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
            eprintln!(
                "[WARN] {month_key} has {} values (expected 12); ignoring",
                vals.len()
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
    use serde_json::{Map, Value, json};

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
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");

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
        let resolved = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");

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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &repo_defaults(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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
        let expected = 3.6 * (0.293_071_07 * 9.0 / 5.0);
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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

    /// HPXML Battery: RoundTripEfficiency of 0.90 → inverter_efficiency ≈ sqrt(0.90).
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
        let battery = specs
            .iter()
            .find(|s| s.name == "Battery")
            .expect("Battery spec must be present");

        let inv_eff = battery
            .parameters
            .get("inverter_efficiency")
            .and_then(Value::as_f64)
            .expect("inverter_efficiency must be present");

        let expected = rte.sqrt();
        assert!(
            (inv_eff - expected).abs() < 1e-9,
            "inverter_efficiency={inv_eff:.6}, expected sqrt(0.90)≈{expected:.6}"
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
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
    fn duct_dse_injected_into_furnace_from_attic_ducts() {
        let xml = hvac_with_ducts_xml(
            r#"<HeatingSystem>
              <HeatingSystemFuel>natural gas</HeatingSystemFuel>
              <HeatingSystemType><Furnace/></HeatingSystemType>
              <HeatingCapacity>60000</HeatingCapacity>
              <AnnualHeatingEfficiency><Units>AFUE</Units><Value>0.92</Value></AnnualHeatingEfficiency>
            </HeatingSystem>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace spec must be present");

        let dse = furnace
            .parameters
            .get("duct_dse")
            .and_then(Value::as_f64)
            .expect("duct_dse must be set");
        assert!(dse < 1.0, "DSE must be less than 1.0 with attic ducts, got {dse}");
        assert!(dse > 0.3, "DSE must be reasonable, got {dse}");

        assert!(
            furnace.parameters.contains_key("duct_zone_id"),
            "duct_zone_id must be set for non-conditioned duct location"
        );
    }

    #[test]
    fn duct_dse_injected_into_ashp_from_attic_ducts() {
        let xml = hvac_with_ducts_xml(
            r#"<HeatPump>
              <HeatPumpType>air-to-air</HeatPumpType>
              <HeatingCapacity>36000</HeatingCapacity>
              <CoolingCapacity>36000</CoolingCapacity>
            </HeatPump>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");

        let heater = specs
            .iter()
            .find(|s| s.name == "ASHP Heater")
            .expect("ASHP Heater");
        let dse = heater
            .parameters
            .get("duct_dse")
            .and_then(Value::as_f64)
            .expect("duct_dse must be set on ASHP Heater");
        assert!(dse < 1.0 && dse > 0.3, "DSE {dse} out of expected range");

        let cooler = specs
            .iter()
            .find(|s| s.name == "ASHP Cooler")
            .expect("ASHP Cooler");
        assert!(
            cooler.parameters.contains_key("duct_dse"),
            "ASHP Cooler must also have duct_dse"
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");

        let heater = specs
            .iter()
            .find(|s| s.name == "MSHP Heater")
            .expect("MSHP Heater");
        assert!(
            !heater.parameters.contains_key("duct_dse"),
            "mini-split must not have duct_dse"
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
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({})).expect("resolve_equipment");
        let furnace = specs
            .iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace");
        assert!(
            !furnace.parameters.contains_key("duct_dse"),
            "without duct data, duct_dse must not be injected"
        );
    }
}
