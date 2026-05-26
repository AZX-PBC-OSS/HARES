//! Water heater resolution from HPXML into canonical equipment specs.

use serde::Serialize;

use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, TanklessWaterHeaterConfig,
};
use hares_types::{FuelType, normalize_ascii};

use super::building::XmlNode;
use super::equipment::EquipmentSpec;
use super::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};
use super::xml_helpers::{child_f64, child_temperature_c, child_text, descendants_named};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;
use crate::draw_profile::{DistributionSystem, FixtureEfficiency, combined_daily_hot_water_l};

pub(super) fn resolve_water_heaters(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), super::HpxmlError> {
    // Shared draw parameters parsed once from the WaterHeating section.
    let (avg_water_draw_l_per_day, n_bedrooms) = parse_avg_water_draw_and_bedrooms(details);

    for wh in descendants_named(details, "WaterHeatingSystem") {
        let fuel = parse_water_heater_fuel(wh)?;
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
        // Tank height is optional in HPXML; propagate None so the equipment
        // model falls back to its documented stratified-tank geometry defaults
        // rather than having the IO layer silently substitute 4 ft here.
        let tank_height_m = child_f64(wh, "TankHeight").map(conv::length_ft_to_m);
        let first_hour_rating_m3 = first_hour_rating_gal.map(conv::volume_gal_to_m3);
        let heating_capacity_btu_hr = heating_capacity_input;
        let heating_capacity_w = heating_capacity_input.map(conv::power_btu_h_to_w);

        // HPXML JacketRValue is in hr·ft²·°F/Btu; convert to SI m²·K/W.
        let jacket_r_value_m2_k_w = wh
            .path(&["WaterHeaterInsulation", "Jacket"])
            .and_then(|j| child_f64(j, "JacketRValue"))
            .map(conv::r_value_ip_to_si);

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
        let ua_result = match ua_from_energy_factor(&ua_inputs) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    water_heater_type = %wh_type,
                    %err,
                    "UA calculation failed; equipment model will use default"
                );
                None
            }
        };
        let ua_w_per_k = ua_result.map(|r| r.ua_w_per_k);

        let spec = match name.as_str() {
            "Gas Water Heater" => {
                let conversion_efficiency = ua_result.map(|r| r.conversion_efficiency);
                let gas_flue_loss_fraction = child_f64(wh, "FlueLossFraction")
                    .or_else(|| conversion_efficiency.map(|_| 0.0));
                let cfg = GasWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: fuel,
                    tank_volume_m3,
                    tank_height_m,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: child_f64(wh, "PilotPower").map(conv::power_btu_h_to_w),
                    flue_loss_fraction: gas_flue_loss_fraction,
                    skin_loss_fraction: None,
                    ignition_type: child_text(wh, "IgnitionType"),
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    conversion_efficiency,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Electric Resistance Water Heater" => {
                let cfg = ElectricResistanceWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    element_power_w: heating_capacity_w,
                    element_priority_mode: None,
                    max_setpoint_ramp_rate_c_per_min: None,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Tankless Water Heater" | "Gas Tankless Water Heater" => {
                // UEF-only inputs (no EF) use 0.94 default per RESNET 301;
                // EF-sourced inputs use the legacy 0.92 default.
                let default_perf_adj = if energy_factor.is_none() { 0.94 } else { 0.92 };
                let perf_adj = child_f64(wh, "PerformanceAdjustment").unwrap_or(default_perf_adj);
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
                    inlet_temp_c: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    avg_water_draw_l_per_day,
                    number_of_bedrooms: n_bedrooms,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Heat Pump Water Heater" => {
                // UEF→EF conversion coefficients for HPWH.
                // Source: ResStock waterheater.rb; NREL ResStock calibration (Maguire & Roberts 2020).
                const HPWH_UEF_TO_EF_SLOPE: f64 = 0.60522;
                const HPWH_UEF_TO_EF_DENOM: f64 = 1.2101;
                // Scale factor mapping UEF to rated COP for HPWH.
                // Source: OCHRE WaterHeater.py; derived from GE GeoSpring calibration.
                const HPWH_UEF_TO_COP: f64 = 1.174_536_058;
                let uef = uniform_energy_factor.or_else(|| {
                    energy_factor.map(|ef| (HPWH_UEF_TO_EF_SLOPE + ef) / HPWH_UEF_TO_EF_DENOM)
                });
                let low_power = uef.is_some_and(|u| (u - 4.9).abs() < 1e-9);
                let (cop, setpoint_c, tempering_valve_setpoint_c) = if low_power {
                    // Low-power OCHRE preset (UEF ≈ 4.9): fixed 60 °C storage,
                    // 51.67 °C (125 °F) tempering valve per manufacturer spec.
                    (Some(4.2), Some(60.0), Some(51.67))
                } else {
                    let cop = uef.map(|uef_val| HPWH_UEF_TO_COP * uef_val);
                    // Tempering valve setpoint mirrors the HPXML-declared
                    // storage setpoint. If HPXML omits HotWaterTemperature we
                    // cannot guess safely -- error loudly.
                    let storage_setpoint_c = setpoint_c.ok_or_else(|| {
                        super::HpxmlError::MissingField {
                            path: "WaterHeatingSystem/HotWaterTemperature",
                            system_kind: "Heat Pump Water Heater",
                            system_id: super::xml_helpers::element_id(wh)
                                .unwrap_or_else(|| "unknown".to_string()),
                            reason:
                                "hot water setpoint (°C) is required to size the tempering valve; no silent default permitted",
                        }
                    })?;
                    (cop, setpoint_c, Some(storage_setpoint_c))
                };
                let cfg = HeatPumpWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    cop,
                    backup_element_power_w: heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    tempering_valve_setpoint_c,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    compressor_power_w: if low_power { Some(1_499.4) } else { None },
                    backup_enable_offset_c: None,
                    min_ambient_temp_c: None,
                    max_ambient_temp_c: None,
                    min_on_time_s: None,
                    min_off_time_s: None,
                    hp_only_mode: Some(low_power),
                    element_hp_control_mode: None,
                    fan_power_w: None,
                    parasitic_power_w: None,
                    backup_efficiency: None,
                    shr: None,
                    lost_heat_fraction: None,
                    wall_heat_fraction: None,
                    capacity_biquadratic_coeffs: None,
                    cop_biquadratic_coeffs: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            // Unreachable: canonical_water_heater_name (called at the top of
            // this loop) generates exactly the five names matched above and
            // rejects all others with Err, so execution never reaches this arm.
            _ => unreachable!(
                "canonical_water_heater_name validated '{}' but match did not cover it",
                name,
            ),
        };

        specs.push(spec);
    }
    Ok(())
}

/// Parse the combined average daily hot water draw [L/day] and bedroom count from `<BuildingDetails>`.
///
/// Reads `<BuildingSummary>/<BuildingConstruction>/<NumberofBedrooms>`,
/// `<WaterHeating>/<WaterFixture>/<LowFlow>`, the `<WaterFixturesUsageMultiplier>` extension,
/// and `<HotWaterDistribution>` to compute the OCHRE/ANSI-RESNET 301 draw estimate.
///
/// Returns `(None, None)` when bedroom count is absent (required for both outputs).
fn parse_avg_water_draw_and_bedrooms(details: &XmlNode) -> (Option<f64>, Option<f64>) {
    // Bedroom count is required; without it the formula cannot be evaluated.
    let n_bedrooms_raw = match details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofBedrooms",
        ])
        .and_then(|n| n.text.trim().parse::<f64>().ok())
    {
        Some(v) => v,
        None => return (None, None),
    };

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

    let draw_l_per_day = combined_daily_hot_water_l(
        n_bedrooms,
        fixture_efficiency,
        usage_multiplier,
        &distribution,
    );

    (Some(draw_l_per_day), Some(n_bedrooms))
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
    let typed_config =
        EquipmentConfig::from_typed(name.clone(), T::equipment_type_name().to_string(), cfg);
    EquipmentSpec {
        name: name.clone(),
        instance_name: None,
        fuel_type,
        parameters,
        zip_params: defaults.zip_params(&name).cloned(),
        typed_config: Some(typed_config),
    }
}

fn parse_water_heater_fuel(wh: &XmlNode) -> std::result::Result<FuelType, super::HpxmlError> {
    let raw = child_text(wh, "FuelType").ok_or_else(|| {
        super::HpxmlError::Parse("WaterHeatingSystem is missing required FuelType".to_string())
    })?;
    match normalize_ascii(&raw).as_str() {
        "electricity" | "electric" | "none" => Ok(FuelType::Electric),
        "natural gas" | "natural_gas" | "gas" => Ok(FuelType::Gas),
        "propane" => Ok(FuelType::Propane),
        "oil" | "fuel oil" | "fuel_oil" | "fuel oil 1" | "fuel oil 2" | "fuel oil 4"
        | "fuel oil 5/6" | "kerosene" | "diesel" => Ok(FuelType::Oil),
        "wood" => Ok(FuelType::Wood),
        "wood pellets" | "wood_pellets" => Ok(FuelType::WoodPellet),
        "coal" | "anthracite coal" | "anthracite_coal" | "bituminous coal" | "bituminous_coal"
        | "coke" => Ok(FuelType::Coal),
        other => Err(super::HpxmlError::Parse(format!(
            "unsupported water-heater FuelType '{other}'"
        ))),
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

    // HPXML PipeRValue is in hr·ft²·°F/Btu (IP R-value). Convert to SI [m²·K/W] at parse time.
    let pipe_r_value = dist_node
        .path(&["PipeInsulation"])
        .and_then(|n| child_f64(n, "PipeRValue"))
        .map(conv::r_value_ip_to_si)
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
        (other_type, other_fuel) => {
            tracing::warn!(
                wh_type = other_type,
                fuel = ?other_fuel,
                "Unrecognized water heater type/fuel combination; \
                 defaulting UA category to StorageGas"
            );
            WhCategory::StorageGas
        }
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
        (
            "storage water heater",
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Water Heater",
        (
            "instantaneous water heater",
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Tankless Water Heater",
        ("space-heating boiler with storage tank", _) => {
            return Err(super::HpxmlError::Parse(
                "space-heating boiler with storage tank (combi boiler with indirect tank) \
                 is not yet implemented. Workaround: configure a separate boiler and \
                 storage water heater as independent equipment in the HPXML input."
                    .to_string(),
            ));
        }
        ("space-heating boiler with tankless coil", _) => {
            return Err(super::HpxmlError::Parse(
                "space-heating boiler with tankless coil is not yet implemented. \
                 Workaround: configure a separate boiler and storage water heater as \
                 independent equipment in the HPXML input."
                    .to_string(),
            ));
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
    use crate::hpxml::parse_xml_document;
    use serde_json::Value;

    use hares_physics::units as conv;
    use hares_types::FuelType;

    use crate::defaults::DefaultsStore;

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
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let spec = typed_spec(
            "Gas Water Heater".to_string(),
            FuelType::Propane,
            cfg,
            &DefaultsStore::empty(),
        );
        assert!(
            spec.typed_config
                .as_ref()
                .expect("typed payload")
                .is_typed()
        );
        assert_eq!(
            spec.parameters.get("fuel_type").and_then(Value::as_str),
            Some("Propane")
        );
        assert_eq!(
            spec.parameters
                .get("avg_water_draw_l_per_day")
                .and_then(Value::as_f64),
            Some(227.0)
        );
    }

    #[test]
    fn typed_spec_preserves_explicit_none_schedule_sources() {
        let cfg = ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let spec = typed_spec(
            "Electric Resistance Water Heater".to_string(),
            FuelType::Electric,
            cfg,
            &DefaultsStore::empty(),
        );
        assert_eq!(
            spec.parameters.get("draw_flow_rate_source"),
            Some(&Value::Null)
        );
        assert_eq!(
            spec.parameters.get("mains_temp_c_source"),
            Some(&Value::Null)
        );
        let typed_cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed resistance config")
            .typed()
            .expect("typed resistance decode");
        assert!(typed_cfg.draw_flow_rate_source.is_none());
        assert!(typed_cfg.mains_temp_c_source.is_none());
    }

    #[test]
    fn resolve_water_heaters_parses_realistic_tankless_xml() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <extension>
                      <WaterFixturesUsageMultiplier>1.05</WaterFixturesUsageMultiplier>
                    </extension>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                      <PerformanceAdjustment>0.93</PerformanceAdjustment>
                      <Location>attic</Location>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");

        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Tankless Water Heater")
            .expect("tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed tankless config");

        assert_eq!(cfg.setpoint_c, Some(conv::temperature_f_to_c(120.0)));
        assert_eq!(cfg.performance_adjustment, Some(0.93));
        assert!(
            cfg.avg_water_draw_l_per_day
                .expect("avg draw should be derived")
                > 0.0
        );
        assert!(cfg.draw_flow_rate_source.is_none());
        assert!(cfg.mains_temp_c_source.is_none());
    }

    #[test]
    fn invalid_water_heater_fuel_is_rejected() {
        let wh = XmlNode {
            name: "WaterHeatingSystem".to_string(),
            attrs: Default::default(),
            text: String::new(),
            children: vec![
                XmlNode {
                    name: "FuelType".to_string(),
                    attrs: Default::default(),
                    text: "mystery-fuel".to_string(),
                    children: vec![],
                },
                XmlNode {
                    name: "WaterHeaterType".to_string(),
                    attrs: Default::default(),
                    text: "storage water heater".to_string(),
                    children: vec![],
                },
            ],
        };

        let err = parse_water_heater_fuel(&wh).expect_err("invalid fuel must be rejected");
        assert!(
            err.to_string()
                .contains("unsupported water-heater FuelType")
        );
    }

    #[test]
    fn jacket_r_value_parsed_and_converted_to_si() {
        let r_ip = 5.0_f64;
        let expected_si = conv::r_value_ip_to_si(r_ip);

        let xml = format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <EnergyFactor>0.92</EnergyFactor>
                      <WaterHeaterInsulation>
                        <Jacket>
                          <JacketRValue>{r_ip}</JacketRValue>
                        </Jacket>
                      </WaterHeaterInsulation>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let root = parse_xml_document(&xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");

        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("ERWH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed ERWH config");

        let jacket_si = cfg
            .jacket_r_value_m2_k_w
            .expect("jacket_r_value_m2_k_w must be populated");
        assert!(
            (jacket_si - expected_si).abs() < 1e-9,
            "expected {expected_si} m²·K/W, got {jacket_si}"
        );
    }

    #[test]
    fn jacket_r_value_absent_yields_none() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <EnergyFactor>0.92</EnergyFactor>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");

        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("ERWH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed ERWH config");

        assert!(
            cfg.jacket_r_value_m2_k_w.is_none(),
            "jacket_r_value_m2_k_w must be None when HPXML element is absent"
        );
    }

    fn gas_wh_xml_with_ignition(ignition_type: Option<&str>) -> String {
        let ignition_elem = ignition_type
            .map(|v| format!("                      <IgnitionType>{v}</IgnitionType>\n"))
            .unwrap_or_default();
        format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
{ignition_elem}
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        )
    }

    fn parse_gas_wh_config(xml: &str) -> GasWaterHeaterConfig {
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");
        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("Gas Water Heater spec must be emitted");
        spec.typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed GasWaterHeaterConfig")
    }

    #[test]
    fn hpxml_electronic_ignition_type_is_parsed() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(Some("electronic ignition")));
        assert_eq!(
            cfg.ignition_type.as_deref(),
            Some("electronic ignition"),
            "IgnitionType 'electronic ignition' must be forwarded to config"
        );
    }

    #[test]
    fn hpxml_standing_pilot_ignition_type_is_parsed() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(Some("standing pilot")));
        assert_eq!(
            cfg.ignition_type.as_deref(),
            Some("standing pilot"),
            "IgnitionType 'standing pilot' must be forwarded to config"
        );
    }

    #[test]
    fn hpxml_absent_ignition_type_yields_none() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(None));
        assert!(
            cfg.ignition_type.is_none(),
            "absent IgnitionType must yield None in config"
        );
    }

    #[test]
    fn pilot_power_btu_h_converted_to_watts() {
        let ignition_elem = "                      <PilotPower>600</PilotPower>\n";
        let xml = format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
{ignition_elem}
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let cfg = parse_gas_wh_config(&xml);
        let expected_w = 600.0 * 0.293_071_07;
        let pw = cfg
            .pilot_power_w
            .expect("pilot_power_w must be set when PilotPower is in HPXML");
        assert!(
            (pw - expected_w).abs() < 0.01,
            "PilotPower=600 BTU/h must convert to ~{expected_w:.2} W, got {pw:.2}"
        );
    }

    #[test]
    fn pilot_power_absent_yields_none() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(None));
        assert!(
            cfg.pilot_power_w.is_none(),
            "absent PilotPower must yield None in config"
        );
    }

    #[test]
    fn hpwh_heating_capacity_converted_from_btu_h_to_watts() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>heat pump water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <HotWaterTemperature>125</HotWaterTemperature>
                      <UniformEnergyFactor>3.75</UniformEnergyFactor>
                      <HeatingCapacity>4500</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");
        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Heat Pump Water Heater")
            .expect("HPWH spec must be emitted");
        let cfg: HeatPumpWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed HeatPumpWaterHeaterConfig");

        let expected_w = conv::power_btu_h_to_w(4500.0);
        let actual_w = cfg
            .backup_element_power_w
            .expect("backup_element_power_w must be populated");
        assert!(
            (actual_w - expected_w).abs() < 1e-6,
            "expected {expected_w} W, got {actual_w} W (HeatingCapacity must be converted from Btu/h)"
        );
    }

    /// Gas WH resolver must populate conversion_efficiency (eta_c) from the UA derivation
    /// and set flue_loss_fraction=0 when deriving from EF/RE/capacity.
    /// EF=0.59, RE=0.76, 36 kBtu/hr => eta_c ~ 0.782.
    #[test]
    fn gas_wh_resolver_emits_eta_c_as_conversion_efficiency() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
                      <RecoveryEfficiency>0.76</RecoveryEfficiency>
                      <HeatingCapacity>36000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");
        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("Gas Water Heater spec must be emitted");
        let cfg: GasWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed GasWaterHeaterConfig");

        let eta_c = cfg
            .conversion_efficiency
            .expect("conversion_efficiency must be populated");
        assert!(
            (eta_c - 0.782).abs() < 0.01,
            "expected eta_c ~ 0.782, got {eta_c:.4}"
        );
        assert!(
            eta_c > 0.76,
            "eta_c must be greater than RE (0.76), got {eta_c:.4}"
        );
        assert_eq!(
            cfg.energy_factor,
            Some(0.59),
            "energy_factor must remain the original EF, not eta_c"
        );
        assert_eq!(
            cfg.flue_loss_fraction,
            Some(0.0),
            "flue_loss_fraction must be 0.0 when eta_c is derived from UA calc"
        );
    }

    /// Tankless WH with UEF-only (no EF) should default perf_adj to 0.94.
    #[test]
    fn tankless_uef_only_default_performance_adjustment() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <UniformEnergyFactor>0.87</UniformEnergyFactor>
                      <HeatingCapacity>199000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");
        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("Tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed TanklessWaterHeaterConfig");

        assert_eq!(
            cfg.performance_adjustment,
            Some(0.94),
            "UEF-only tankless must default perf_adj to 0.94, got {:?}",
            cfg.performance_adjustment
        );
    }

    /// Tankless WH with EF (not UEF-only) should default perf_adj to 0.92.
    #[test]
    fn tankless_ef_default_performance_adjustment() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <EnergyFactor>0.82</EnergyFactor>
                      <HeatingCapacity>199000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");
        let mut specs = Vec::new();
        resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs)
            .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("Tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed TanklessWaterHeaterConfig");

        assert_eq!(
            cfg.performance_adjustment,
            Some(0.92),
            "EF-sourced tankless must default perf_adj to 0.92, got {:?}",
            cfg.performance_adjustment
        );
    }

    // Combi boiler types (HPXML v4 §8.5) are valid HPXML enumerations that
    // HARES does not yet model. canonical_water_heater_name() returns a clear
    // error with a workaround rather than a generic "unsupported type" message,
    // so users can configure separate boiler + tank equipment in the HPXML input
    // as a stopgap. Full IndirectTank equipment model is tracked in T-0134.

    #[test]
    fn combi_boiler_with_storage_tank_type_rejected_with_clear_error() {
        let err =
            canonical_water_heater_name("space-heating boiler with storage tank", FuelType::Gas)
                .expect_err("combi boiler with storage tank must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("is not yet implemented"),
            "error must identify the type as unimplemented, got: {err}"
        );
        assert!(
            msg.contains("Workaround"),
            "error must provide a workaround, got: {err}"
        );
    }

    #[test]
    fn combi_boiler_with_tankless_coil_type_rejected_with_clear_error() {
        let err =
            canonical_water_heater_name("space-heating boiler with tankless coil", FuelType::Gas)
                .expect_err("combi boiler with tankless coil must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("is not yet implemented"),
            "error must identify the type as unimplemented, got: {err}"
        );
        assert!(
            msg.contains("Workaround"),
            "error must provide a workaround, got: {err}"
        );
    }

    #[test]
    fn combi_boiler_full_xml_round_trip_rejected_with_clear_error() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <TankVolume>40</TankVolume>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("building details must exist");

        let mut specs = Vec::new();
        let result = resolve_water_heaters(details, &DefaultsStore::empty(), &mut specs);
        assert!(
            result.is_err(),
            "resolve_water_heaters must return Err for unsupported combi boiler type"
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("is not yet implemented"),
            "the error must identify the type as unimplemented, got: {err}"
        );
        assert!(
            msg.contains("Workaround"),
            "the error must provide a workaround, got: {err}"
        );
    }
}
