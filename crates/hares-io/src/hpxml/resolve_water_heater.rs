//! Water heater resolution from HPXML into canonical equipment specs.

use serde::Serialize;
#[cfg(test)]
use serde_json::Value;

use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, TanklessWaterHeaterConfig,
};
use hares_types::FuelType;

use super::building::XmlNode;
use super::equipment::EquipmentSpec;
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
        let setpoint_c = child_temperature_c(wh);
        let performance_adjustment = child_f64(wh, "PerformanceAdjustment");
        let location = child_text(wh, "Location");
        let zone_name = location.as_deref().map(|location| {
            let zone_type = super::building::parse_zone_label(location);
            super::building::zone_key(&zone_type)
        });

        let energy_factor = child_f64(wh, "EnergyFactor");
        let uniform_energy_factor = child_f64(wh, "UniformEnergyFactor");
        let tank_volume_rated_gal = child_f64(wh, "TankVolume");
        let first_hour_rating_gal = child_f64(wh, "FirstHourRating");
        let heating_capacity_input = child_f64(wh, "HeatingCapacity");
        let recovery_efficiency = child_f64(wh, "RecoveryEfficiency");

        let volume_correction = if fuel == FuelType::Electric {
            0.9
        } else {
            0.95
        };
        let tank_volume_m3 =
            tank_volume_rated_gal.map(|gal| conv::volume_gal_to_m3(gal * volume_correction));
        let tank_height_m = child_f64(wh, "TankHeight")
            .map(conv::length_ft_to_m)
            .unwrap_or(conv::length_ft_to_m(4.0));
        let first_hour_rating_m3 = first_hour_rating_gal.map(conv::volume_gal_to_m3);
        let heating_capacity_btu_hr = heating_capacity_input;
        let heating_capacity_w = heating_capacity_input.map(conv::power_btu_h_to_w);

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
        let ua_w_per_k = match ua_from_energy_factor(&ua_inputs) {
            Ok(Some(ua_result)) => Some(ua_result.ua_w_per_k),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(
                    water_heater_type = %wh_type,
                    %err,
                    "UA calculation failed; equipment model will use default"
                );
                None
            }
        };

        let spec = match name.as_str() {
            "Gas Water Heater" => {
                let cfg = GasWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: fuel,
                    tank_volume_m3,
                    tank_height_m: Some(tank_height_m),
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    avg_water_draw_l_per_day,
                    pilot_power_w: child_f64(wh, "PilotPower"),
                    flue_loss_fraction: child_f64(wh, "FlueLossFraction"),
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Electric Resistance Water Heater" => {
                let cfg = ElectricResistanceWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m: Some(tank_height_m),
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    avg_water_draw_l_per_day,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    element_power_w: heating_capacity_w,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Tankless Water Heater" | "Gas Tankless Water Heater" => {
                let perf_adj = child_f64(wh, "PerformanceAdjustment").unwrap_or(0.92);
                let cfg = TanklessWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: fuel,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    setpoint_c,
                    parasitic_power_w: None,
                    performance_adjustment: Some(perf_adj),
                    avg_water_draw_l_per_day,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Heat Pump Water Heater" => {
                let uef = uniform_energy_factor
                    .or_else(|| energy_factor.map(|ef| (0.60522 + ef) / 1.2101));
                let low_power = uef.is_some_and(|u| (u - 4.9).abs() < 1e-9);
                let (cop, setpoint_c, tempering_valve_setpoint_c) = if low_power {
                    (Some(4.2), Some(60.0), Some(51.67))
                } else {
                    let cop = uef.map(|uef_val| 1.174_536_058 * uef_val);
                    let storage_setpoint_c = setpoint_c.unwrap_or(51.67);
                    (cop, setpoint_c, Some(storage_setpoint_c))
                };
                let cfg = HeatPumpWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m: Some(tank_height_m),
                    cop,
                    // CFG-012: HPWH typed config uses `backup_element_power_w` directly.
                    // The existing HPXML/regression expectations treat this value as the
                    // electric backup element rating already expressed in watts.
                    backup_element_power_w: heating_capacity_input,
                    ua_w_per_k,
                    setpoint_c,
                    tempering_valve_setpoint_c,
                    avg_water_draw_l_per_day,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            _ => {
                return Err(super::HpxmlError::Parse(format!(
                    "unsupported canonical water heater name: {name}"
                )));
            }
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

fn typed_spec<T>(
    name: String,
    fuel_type: FuelType,
    cfg: T,
    defaults: &DefaultsStore,
) -> EquipmentSpec
where
    T: hares_equipment::EquipmentTypedConfig + Serialize,
{
    let parameters = serde_json::to_value(&cfg)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let typed_config = EquipmentConfig::from_typed(
        name.clone(),
        T::equipment_type_name().to_string(),
        cfg,
    );
    EquipmentSpec {
        name: name.clone(),
        fuel_type,
        parameters,
        zip_params: defaults.zip_params(&name).cloned(),
        typed_config: Some(typed_config),
    }
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

fn canonical_water_heater_name(
    wh_type: &str,
    fuel: FuelType,
) -> std::result::Result<String, super::HpxmlError> {
    let ty = wh_type.trim();
    let name = match (ty, fuel) {
        ("storage water heater", FuelType::Electric) => "Electric Resistance Water Heater",
        ("instantaneous water heater", FuelType::Electric) => "Tankless Water Heater",
        ("heat pump water heater", FuelType::Electric) => "Heat Pump Water Heater",
        ("storage water heater", FuelType::Gas | FuelType::Propane | FuelType::Oil) => {
            "Gas Water Heater"
        }
        ("instantaneous water heater", FuelType::Gas | FuelType::Propane | FuelType::Oil) => {
            "Gas Tankless Water Heater"
        }
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
    fn propane_storage_water_heater_maps_to_gas_class() {
        let name = canonical_water_heater_name("storage water heater", FuelType::Propane)
            .expect("propane storage WH should map to gas class");
        assert_eq!(name, "Gas Water Heater");
    }

    #[test]
    fn typed_spec_serializes_canonical_fields() {
        let cfg = GasWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(2),
            loop_id: Some(3),
            fuel_type: FuelType::Propane,
            tank_volume_m3: Some(0.151),
            tank_height_m: Some(1.2),
            energy_factor: Some(0.78),
            uniform_energy_factor: Some(0.81),
            heating_capacity_w: Some(11_000.0),
            ua_w_per_k: Some(2.5),
            setpoint_c: Some(51.67),
            avg_water_draw_l_per_day: Some(227.0),
            pilot_power_w: None,
            flue_loss_fraction: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
        };
        let spec = typed_spec(
            "Gas Water Heater".to_string(),
            FuelType::Propane,
            cfg,
            &DefaultsStore::empty(),
        );
        assert!(spec.typed_config.as_ref().expect("typed payload").is_typed());
        assert_eq!(
            spec.parameters
                .get("fuel_type")
                .and_then(Value::as_str),
            Some("Propane")
        );
        assert_eq!(
            spec.parameters
                .get("avg_water_draw_l_per_day")
                .and_then(Value::as_f64),
            Some(227.0)
        );
    }
}
