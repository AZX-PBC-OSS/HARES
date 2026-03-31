//! Water heater resolution from HPXML into canonical equipment specs.

use serde_json::{Map, Value, json};

use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, TanklessWaterHeaterConfig,
};
use hares_types::FuelType;

use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_spec};
use super::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};
use super::xml_helpers::{
    child_f64, child_temperature_c, child_text, descendants_named, parse_fuel,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;
use crate::draw_profile::{DistributionSystem, FixtureEfficiency, combined_daily_hot_water_l};

pub(super) fn resolve_water_heaters(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), super::HpxmlError> {
    // Shared draw parameters parsed once from the WaterHeating section.
    let avg_water_draw_l_per_day = parse_avg_water_draw_l_per_day(details);

    for wh in descendants_named(details, "WaterHeatingSystem") {
        let fuel = parse_fuel(child_text(wh, "FuelType").as_deref());
        let wh_type = child_text(wh, "WaterHeaterType").unwrap_or_default();
        let name = canonical_water_heater_name(&wh_type, fuel)?;

        let energy_factor = child_f64(wh, "EnergyFactor");
        let uniform_energy_factor = child_f64(wh, "UniformEnergyFactor");
        // Raw HPXML values (IP) — kept for DOE test procedure UA calculation.
        let tank_volume_rated_gal = child_f64(wh, "TankVolume");
        let first_hour_rating_gal = child_f64(wh, "FirstHourRating");
        let heating_capacity_btu_hr = child_f64(wh, "HeatingCapacity");
        let recovery_efficiency = child_f64(wh, "RecoveryEfficiency");

        // Convert to SI at the parse boundary. All downstream params are SI.
        let volume_correction = if fuel == FuelType::Electric {
            0.9
        } else {
            0.95
        };
        let tank_volume_m3 =
            tank_volume_rated_gal.map(|gal| conv::volume_gal_to_m3(gal * volume_correction));
        let first_hour_rating_m3 = first_hour_rating_gal.map(conv::volume_gal_to_m3);
        let heating_capacity_w = heating_capacity_btu_hr.map(conv::power_btu_h_to_w);
        let tank_height_m = child_f64(wh, "TankHeight")
            .map(conv::length_ft_to_m)
            .unwrap_or(conv::length_ft_to_m(4.0));

        let mut params = Map::new();
        if let Some(v) = tank_volume_m3 {
            params.insert("tank_volume_m3".to_string(), json!(v));
        }
        params.insert("tank_height_m".to_string(), json!(tank_height_m));
        if let Some(v) = first_hour_rating_m3 {
            params.insert("first_hour_rating_m3".to_string(), json!(v));
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
        if let Some(cap_w) = heating_capacity_w {
            params.insert("heating_capacity_w".to_string(), json!(cap_w));
        }
        params.insert(
            "water_heater_type".to_string(),
            Value::String(wh_type.clone()),
        );
        params.insert("fuel_type".to_string(), json!(format!("{fuel:?}")));

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
            Err(err) => {
                tracing::warn!(
                    water_heater_type = %wh_type,
                    %err,
                    "UA calculation failed; equipment model will use default"
                );
            }
        }

        // HPWH-specific parameters
        if wh_type.contains("heat pump") {
            // COP from UEF; fall back to EF→UEF conversion when UEF is absent.
            // Conversion formula derived from DOE 10 CFR Part 430 test-procedure correlation.
            let uef =
                uniform_energy_factor.or_else(|| energy_factor.map(|ef| (0.60522 + ef) / 1.2101));

            let storage_setpoint_c = params
                .get("setpoint_c")
                .and_then(|v| v.as_f64())
                .unwrap_or(51.67);

            if uef.is_some_and(|u| (u - 4.9).abs() < 1e-9) {
                // Low-power HPWH (UEF == 4.9): fixed COP and compressor power per OCHRE
                // reference data; storage setpoint raised to 60°C (140°F) with tempering
                // valve to deliver 51.67°C (125°F) at the fixture.
                params.insert("cop".to_string(), json!(4.2));
                params.insert("compressor_power_w".to_string(), json!(1499.4_f64));
                params.insert("setpoint_c".to_string(), json!(60.0_f64));
                params.insert("tempering_valve_setpoint_c".to_string(), json!(51.67_f64));
                params.insert("hp_only_mode".to_string(), json!(true));
                params.insert("low_power_hpwh".to_string(), json!(true));
            } else {
                if let Some(uef_val) = uef {
                    params.insert("cop".to_string(), json!(1.174_536_058 * uef_val));
                }
                // Tempering valve setpoint matches the tank storage setpoint unless a
                // separate delivery temperature is configured later.
                params.insert(
                    "tempering_valve_setpoint_c".to_string(),
                    json!(storage_setpoint_c),
                );
            }

            // HPXML HeatingCapacity maps to the backup resistance element in a HPWH.
            if let Some(cap_w) = heating_capacity_w {
                params.insert("backup_element_power_w".to_string(), json!(cap_w));
            }

            // Conditioned-space HPWHs reject waste heat back into the zone; unconditioned
            // locations lose the heat to the ambient without a zone-heat benefit.
            let location = child_text(wh, "Location").unwrap_or_default();
            let zone_type = super::building::parse_zone_label(&location);
            let zone_name = super::building::zone_key(&zone_type);
            if zone_name == "conditioned" {
                params.insert("lost_heat_fraction".to_string(), json!(0.25_f64));
                params.insert("wall_heat_fraction".to_string(), json!(0.5_f64));
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
                json!(conv::r_value_ip_to_si(jacket_r)),
            );
        }

        // Tankless performance adjustment
        if wh_type.contains("instantaneous") {
            let perf_adj = child_f64(wh, "PerformanceAdjustment").unwrap_or(0.92);
            params.insert("performance_adjustment".to_string(), json!(perf_adj));
        }

        // Water heater location → canonical zone name.
        if let Some(location) = child_text(wh, "Location") {
            let zone_type = super::building::parse_zone_label(&location);
            let zone_name = super::building::zone_key(&zone_type);
            params.insert("zone_type".to_string(), Value::String(zone_name));
        }

        let mut spec = build_spec(name.clone(), fuel, params.clone(), defaults);
        spec.typed_config = match name.as_str() {
            "Gas Water Heater" => try_build_gas_wh_config(&name, &params, avg_water_draw_l_per_day),
            "Electric Resistance Water Heater" => {
                try_build_resistance_wh_config(&name, &params, avg_water_draw_l_per_day)
            }
            "Tankless Water Heater" | "Gas Tankless Water Heater" => {
                try_build_tankless_wh_config(&name, &params, fuel, avg_water_draw_l_per_day)
            }
            "Heat Pump Water Heater" => {
                try_build_hpwh_config(&name, &params, avg_water_draw_l_per_day)
            }
            _ => None,
        };
        specs.push(spec);
    }
    Ok(())
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
    let n_bedrooms_raw = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofBedrooms",
        ])
        .and_then(|n| n.text.trim().parse::<f64>().ok())?;

    // Adjust bedroom count by occupancy and house type (OCHRE hpxml.py:789-797).
    let n_occupants = details
        .path(&["BuildingSummary", "BuildingOccupancy", "NumberofResidents"])
        .and_then(|n| n.text.trim().parse::<f64>().ok());
    let house_type = details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "ResidentialFacilityType",
        ])
        .map(|n| n.text.trim().to_ascii_lowercase());
    let n_bedrooms = match (n_occupants, house_type.as_deref()) {
        (Some(occ), Some("single-family attached" | "apartment unit")) => {
            (-0.68 + 1.09 * occ).max(0.0)
        }
        (Some(occ), _) => (-1.47 + 1.69 * occ).max(0.0),
        (None, _) => n_bedrooms_raw,
    };

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
        let piping_length_m = child_f64(standard, "PipingLength").map(conv::length_ft_to_m); // HPXML PipingLength is in feet
        DistributionSystem::Standard {
            pipe_r_value,
            piping_length_m,
            default_piping_length_m,
        }
    } else if let Some(recirc) = system_type.and_then(|n| n.child("Recirculation")) {
        // BranchPipingLoopLength is in feet in HPXML.
        let branch_loop_length_m =
            child_f64(recirc, "BranchPipingLoopLength").map(conv::length_ft_to_m);
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
                Some(conv::area_ft2_to_m2(val))
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
        let area_ft2 = conv::area_m2_to_ft2(area_m2);
        let ft_per_floor = area_ft2 / n_floors;
        let default_ft = 2.0 * ft_per_floor.sqrt()
            + 10.0 * n_floors
            + if has_unfinished_bsmt { 5.0 } else { 0.0 };
        conv::length_ft_to_m(default_ft)
    } else {
        // Bedroom-count proxy when floor area is absent.
        conv::length_ft_to_m(25.0 + 5.0 * n_bedrooms)
    }
}

fn try_build_gas_wh_config(
    name: &str,
    params: &Map<String, Value>,
    avg_water_draw_l_per_day: Option<f64>,
) -> Option<EquipmentConfig> {
    let cfg = GasWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        tank_volume_m3: params.get("tank_volume_m3").and_then(Value::as_f64),
        tank_height_m: params.get("tank_height_m").and_then(Value::as_f64),
        tank_diameter_m: None,
        ua_w_per_k: params.get("ua_w_per_k").and_then(Value::as_f64),
        jacket_r_value_m2_k_w: params.get("jacket_r_value_m2_k_w").and_then(Value::as_f64),
        tank_nodes: None,
        burner_node: None,
        setpoint_c: params.get("setpoint_c").and_then(Value::as_f64),
        deadband_c: None,
        max_tank_temp_c: None,
        heating_capacity_w: params.get("heating_capacity_w").and_then(Value::as_f64),
        // EF/UEF are whole-appliance metrics, not burner thermal efficiency.
        // Leave burner_efficiency as None; the model uses DEFAULT_BURNER_EFFICIENCY.
        burner_efficiency: None,
        flue_loss_fraction: None,
        ignition_type: None,
        pilot_power_w: None,
        fan_power_w: None,
        skin_loss_fraction: None,
        fuel_type: params
            .get("fuel_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        mains_temp_c: None,
        avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_schedule_col: None,
        mains_temp_schedule_col: None,
        zip_z: None,
        zip_i: None,
        zip_p: None,
        zip_zq: None,
        zip_iq: None,
        zip_pq: None,
        zip_pf: None,
        zip_v0: None,
    };
    cfg.validate().ok()?;
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Gas Water Heater".to_string(),
        cfg,
    ))
}

fn try_build_resistance_wh_config(
    name: &str,
    params: &Map<String, Value>,
    avg_water_draw_l_per_day: Option<f64>,
) -> Option<EquipmentConfig> {
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64);
    let cfg = ElectricResistanceWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        tank_volume_m3: params.get("tank_volume_m3").and_then(Value::as_f64),
        tank_height_m: params.get("tank_height_m").and_then(Value::as_f64),
        tank_diameter_m: None,
        ua_w_per_k: params.get("ua_w_per_k").and_then(Value::as_f64),
        jacket_r_value_m2_k_w: params.get("jacket_r_value_m2_k_w").and_then(Value::as_f64),
        tank_nodes: None,
        upper_element_node: None,
        lower_element_node: None,
        setpoint_c: params.get("setpoint_c").and_then(Value::as_f64),
        deadband_c: None,
        max_tank_temp_c: None,
        heating_capacity_w: capacity_w,
        upper_element_power_w: None,
        lower_element_power_w: None,
        element_priority_mode: None,
        max_setpoint_ramp_rate_c_per_min: None,
        mains_temp_c: None,
        avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_schedule_col: None,
        mains_temp_schedule_col: None,
        zip_z: None,
        zip_i: None,
        zip_p: None,
        zip_zq: None,
        zip_iq: None,
        zip_pq: None,
        zip_pf: None,
        zip_v0: None,
    };
    cfg.validate().ok()?;
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Resistance Water Heater".to_string(),
        cfg,
    ))
}

fn try_build_tankless_wh_config(
    name: &str,
    params: &Map<String, Value>,
    fuel: FuelType,
    avg_water_draw_l_per_day: Option<f64>,
) -> Option<EquipmentConfig> {
    let fuel_str = format!("{fuel:?}");
    let uef = params
        .get("uniform_energy_factor")
        .or_else(|| params.get("energy_factor"))
        .and_then(Value::as_f64);
    let perf_adj = params.get("performance_adjustment").and_then(Value::as_f64);
    let ochre_class = if fuel == FuelType::Gas {
        "Gas Tankless Water Heater"
    } else {
        "Tankless Water Heater"
    };
    let cfg = TanklessWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        fuel_type: Some(fuel_str),
        setpoint_c: params.get("setpoint_c").and_then(Value::as_f64),
        efficiency_factor: uef,
        performance_adjustment: perf_adj,
        max_thermal_power_w: params.get("heating_capacity_w").and_then(Value::as_f64),
        parasitic_power_w: None,
        inlet_temp_c: None,
        avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        zip_z: None,
        zip_i: None,
        zip_p: None,
        zip_zq: None,
        zip_iq: None,
        zip_pq: None,
        zip_pf: None,
        zip_v0: None,
    };
    cfg.validate().ok()?;
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        ochre_class.to_string(),
        cfg,
    ))
}

fn try_build_hpwh_config(
    name: &str,
    params: &Map<String, Value>,
    avg_water_draw_l_per_day: Option<f64>,
) -> Option<EquipmentConfig> {
    let cop = params.get("cop").and_then(Value::as_f64);
    let uef = params.get("uniform_energy_factor").and_then(Value::as_f64);
    let hp_only_mode = params
        .get("hp_only_mode")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let low_power = params
        .get("low_power_hpwh")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cfg = HeatPumpWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        tank_volume_m3: params.get("tank_volume_m3").and_then(Value::as_f64),
        tank_height_m: params.get("tank_height_m").and_then(Value::as_f64),
        tank_diameter_m: None,
        ua_w_per_k: params.get("ua_w_per_k").and_then(Value::as_f64),
        jacket_r_value_m2_k_w: params.get("jacket_r_value_m2_k_w").and_then(Value::as_f64),
        tank_nodes: None,
        thermostat_node: None,
        thermostat_upper_node: None,
        condenser_node: None,
        setpoint_c: params.get("setpoint_c").and_then(Value::as_f64),
        deadband_c: None,
        max_tank_temp_c: None,
        max_setpoint_ramp_rate_c_per_min: None,
        compressor_power_w: params.get("compressor_power_w").and_then(Value::as_f64),
        backup_element_power_w: params.get("backup_element_power_w").and_then(Value::as_f64),
        backup_enable_offset_c: None,
        backup_efficiency: None,
        hp_only_mode: if hp_only_mode { Some(true) } else { None },
        cop,
        uniform_energy_factor: uef,
        cop_curve_coeffs: None,
        capacity_curve_coeffs: None,
        min_ambient_temp_c: None,
        max_ambient_temp_c: None,
        low_power_hpwh: if low_power { Some(true) } else { None },
        shr: None,
        lost_heat_fraction: params.get("lost_heat_fraction").and_then(Value::as_f64),
        wall_heat_fraction: params.get("wall_heat_fraction").and_then(Value::as_f64),
        fan_power_w: None,
        parasitic_power_w: None,
        min_on_time_s: None,
        min_off_time_s: None,
        tempering_valve_setpoint_c: params
            .get("tempering_valve_setpoint_c")
            .and_then(Value::as_f64),
        element_hp_control_mode: None,
        mains_temp_c: None,
        avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        zip_z: None,
        zip_i: None,
        zip_p: None,
        zip_zq: None,
        zip_iq: None,
        zip_pq: None,
        zip_pf: None,
        zip_v0: None,
    };
    cfg.validate().ok()?;
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Heat Pump Water Heater".to_string(),
        cfg,
    ))
}

fn wh_category(wh_type: &str, fuel: FuelType) -> WhCategory {
    match (wh_type.trim(), fuel) {
        ("heat pump water heater", FuelType::Electric) => WhCategory::HeatPump,
        ("instantaneous water heater", _) => WhCategory::Instantaneous,
        (_, FuelType::Electric) => WhCategory::StorageElectric,
        _ => WhCategory::StorageGas,
    }
}

fn canonical_water_heater_name(
    wh_type: &str,
    fuel: FuelType,
) -> std::result::Result<String, super::HpxmlError> {
    let ty = wh_type.trim();
    let name = match (ty, fuel) {
        ("storage water heater", FuelType::Electric) => "Electric Resistance Water Heater",
        ("instantaneous water heater", FuelType::Electric) => "Tankless Water Heater",
        ("heat pump water heater", FuelType::Electric) => "Heat Pump Water Heater",
        ("storage water heater", FuelType::Gas) => "Gas Water Heater",
        ("instantaneous water heater", FuelType::Gas) => "Gas Tankless Water Heater",
        _ => {
            return Err(super::HpxmlError::Parse(format!(
                "unsupported HPXML water heater type/fuel combination: \
                 WaterHeaterType='{ty}', fuel='{fuel:?}'"
            )));
        }
    };
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gas_wh_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("tank_volume_m3".to_string(), json!(0.151));
        params.insert("tank_height_m".to_string(), json!(1.2));
        params.insert("ua_w_per_k".to_string(), json!(2.5));
        params.insert("heating_capacity_w".to_string(), json!(11_000.0));
        params.insert("setpoint_c".to_string(), json!(51.67));
        params.insert("fuel_type".to_string(), json!("Gas"));

        let result = try_build_gas_wh_config("Gas Water Heater", &params, Some(227.0));
        assert!(result.is_some(), "gas WH builder must return Some");
        let ec = result.unwrap();
        assert!(ec.is_typed());
        let cfg: GasWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.tank_volume_m3, Some(0.151));
        assert_eq!(cfg.heating_capacity_w, Some(11_000.0));
        assert_eq!(cfg.avg_water_draw_l_per_day, Some(227.0));
        // EF/UEF must not be aliased to burner_efficiency.
        assert!(cfg.burner_efficiency.is_none());
    }

    #[test]
    fn resistance_wh_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("tank_volume_m3".to_string(), json!(0.151));
        params.insert("tank_height_m".to_string(), json!(1.2));
        params.insert("ua_w_per_k".to_string(), json!(2.0));
        params.insert("heating_capacity_w".to_string(), json!(4_500.0));
        params.insert("setpoint_c".to_string(), json!(51.67));

        let result = try_build_resistance_wh_config(
            "Electric Resistance Water Heater",
            &params,
            Some(200.0),
        );
        assert!(result.is_some(), "resistance WH builder must return Some");
        let ec = result.unwrap();
        assert!(ec.is_typed());
        let cfg: ElectricResistanceWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.heating_capacity_w, Some(4_500.0));
        assert_eq!(cfg.avg_water_draw_l_per_day, Some(200.0));
    }

    #[test]
    fn gas_tankless_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("uniform_energy_factor".to_string(), json!(0.87));
        params.insert("performance_adjustment".to_string(), json!(0.92));
        params.insert("setpoint_c".to_string(), json!(51.67));

        let result = try_build_tankless_wh_config(
            "Gas Tankless Water Heater",
            &params,
            FuelType::Gas,
            Some(180.0),
        );
        assert!(result.is_some(), "gas tankless WH builder must return Some");
        let ec = result.unwrap();
        assert!(ec.is_typed());
        let cfg: TanklessWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.efficiency_factor, Some(0.87));
        assert_eq!(cfg.performance_adjustment, Some(0.92));
        assert_eq!(cfg.avg_water_draw_l_per_day, Some(180.0));
    }

    #[test]
    fn hpwh_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("tank_volume_m3".to_string(), json!(0.189));
        params.insert("tank_height_m".to_string(), json!(1.2));
        params.insert("ua_w_per_k".to_string(), json!(2.0));
        params.insert("cop".to_string(), json!(3.5));
        params.insert("uniform_energy_factor".to_string(), json!(3.45));
        params.insert("backup_element_power_w".to_string(), json!(4_500.0));
        params.insert("setpoint_c".to_string(), json!(51.67));
        params.insert("tempering_valve_setpoint_c".to_string(), json!(51.67));
        params.insert("hp_only_mode".to_string(), json!(false));
        params.insert("low_power_hpwh".to_string(), json!(false));

        let result = try_build_hpwh_config("Heat Pump Water Heater", &params, Some(220.0));
        assert!(result.is_some(), "HPWH builder must return Some");
        let ec = result.unwrap();
        assert!(ec.is_typed());
        let cfg: HeatPumpWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.cop, Some(3.5));
        assert_eq!(cfg.uniform_energy_factor, Some(3.45));
        assert_eq!(cfg.backup_element_power_w, Some(4_500.0));
        assert_eq!(cfg.avg_water_draw_l_per_day, Some(220.0));
    }

    #[test]
    fn hpwh_builder_reads_bool_flags_correctly() {
        let mut params = Map::new();
        params.insert("tank_volume_m3".to_string(), json!(0.189));
        params.insert("ua_w_per_k".to_string(), json!(2.0));
        params.insert("cop".to_string(), json!(4.2));
        params.insert("hp_only_mode".to_string(), json!(true));
        params.insert("low_power_hpwh".to_string(), json!(true));

        let result = try_build_hpwh_config("Heat Pump Water Heater", &params, None);
        assert!(result.is_some());
        let cfg: HeatPumpWaterHeaterConfig = result.unwrap().typed().unwrap();
        assert_eq!(cfg.hp_only_mode, Some(true));
        assert_eq!(cfg.low_power_hpwh, Some(true));
    }
}
