//! Water heater resolution from HPXML into canonical equipment specs.

use serde_json::{Map, Value, json};

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
                json!(conv::power_btu_h_to_kbtu_h(cap)),
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
            Err(err) => {
                tracing::warn!(
                    water_heater_type = %wh_type,
                    %err,
                    "UA calculation failed; equipment model will use default"
                );
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
                json!(conv::r_value_ip_to_si(jacket_r)),
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

fn wh_category(wh_type: &str, fuel: FuelType) -> WhCategory {
    match (wh_type.trim(), fuel) {
        ("heat pump water heater", FuelType::Electric) => WhCategory::HeatPump,
        ("instantaneous water heater", _) => WhCategory::Instantaneous,
        (_, FuelType::Electric) => WhCategory::StorageElectric,
        _ => WhCategory::StorageGas,
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
