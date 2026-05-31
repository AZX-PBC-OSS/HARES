//! Water heater resolution from HPXML into canonical equipment specs.

use serde::Serialize;

use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, IndirectTankConfig, TanklessWaterHeaterConfig,
};
use hares_types::{FuelType, normalize_ascii};

use super::building::{Building, XmlNode};
use super::data_patches::HpxmlDataPatches;
use super::equipment::EquipmentSpec;
use super::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};
use super::xml_helpers::{
    child_f64, child_temperature_c, child_text, descendants_named, element_id,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;
use crate::draw_profile::{DistributionSystem, FixtureEfficiency, combined_daily_hot_water_l};

/// ANSI/RESNET 301 on-time fractions for gas tankless parasitic power, indexed by 1–5 bedrooms.
const TANKLESS_ON_TIME_FRACS: [f64; 5] = [0.0269, 0.0333, 0.0397, 0.0462, 0.0529];

/// Compute gas tankless parasitic power (W) from bedroom count.
///
/// Formula: `5 + 60 * on_time_frac` where `on_time_frac` is from the RESNET 301 table.
/// Bedroom count rounded to nearest integer, clamped to [1, 5].
fn tankless_parasitic_power_w(n_bedrooms: f64) -> f64 {
    let idx = (n_bedrooms.round() as usize).clamp(1, 5) - 1;
    5.0 + 60.0 * TANKLESS_ON_TIME_FRACS[idx]
}

/// Resolve a bedroom count, trying multiple sources in priority order:
/// 1. The provided `n_bedrooms` (from HPXML)
/// 2. The `data_patches.number_of_bedrooms` field
/// 3. Default of 2.0 (RESNET 301 implicit)
fn resolve_bedroom_count(n_bedrooms: Option<f64>, data_patches: Option<&HpxmlDataPatches>) -> f64 {
    n_bedrooms
        .or_else(|| data_patches.and_then(|p| p.number_of_bedrooms))
        .unwrap_or(2.0)
}

/// Resolve the numeric `zone_id` (1-indexed) corresponding to an HPXML
/// `<Location>` string (e.g. "garage", "conditioned space"), by matching
/// against the zone types in the parsed `Building`.
///
/// Returns `None` when the location string does not match any zone in the
/// building (should not happen for well-formed HPXML but is not a hard error).
fn zone_id_for_location(building: &Building, location_text: &str) -> Option<u16> {
    let zone_type = super::building::parse_zone_label(location_text);
    let key = super::building::zone_key(&zone_type);
    building
        .zones
        .iter()
        .position(|z| super::building::zone_key(&z.zone_type) == key)
        .map(|idx| (idx as u16) + 1)
}

pub(super) fn resolve_water_heaters(
    building: &Building,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    data_patches: Option<&HpxmlDataPatches>,
) -> std::result::Result<(), super::HpxmlError> {
    let details = &building.details_xml;
    let (avg_water_draw_l_per_day, n_bedrooms) =
        parse_avg_water_draw_and_bedrooms(details, data_patches);

    for wh in descendants_named(details, "WaterHeatingSystem") {
        let wh_type = child_text(wh, "WaterHeaterType").unwrap_or_default();
        let fuel = parse_water_heater_fuel(wh, &wh_type)?;
        let name = canonical_water_heater_name(&wh_type, fuel)?;
        let setpoint_c = child_temperature_c(wh);
        let performance_adjustment = child_f64(wh, "PerformanceAdjustment");
        let location = child_text(wh, "Location");
        let zone_name = location.as_deref().map(|location| {
            let zone_type = super::building::parse_zone_label(location);
            super::building::zone_key(&zone_type)
        });
        let zone_id = location
            .as_deref()
            .and_then(|loc| zone_id_for_location(building, loc));

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

        // RelatedHVACSystem cross-reference for combi boiler / indirect tank.
        let related_hvac_idref = wh
            .child("RelatedHVACSystem")
            .and_then(|n| n.attrs.get("idref").cloned());

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

        let mut spec = match name.as_str() {
            "Gas Water Heater" => {
                let conversion_efficiency = ua_result.map(|r| r.conversion_efficiency);
                let gas_flue_loss_fraction = child_f64(wh, "FlueLossFraction")
                    .or_else(|| conversion_efficiency.map(|_| 0.0));
                let cfg = GasWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
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
                    pilot_fraction_to_tank: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            "Electric Resistance Water Heater" => {
                let cfg = ElectricResistanceWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
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
                    max_combined_power_w: None,
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
                    zone_id,
                    loop_id: None,
                    fuel_type: fuel,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    setpoint_c,
                    parasitic_power_w: (fuel == FuelType::Gas)
                        .then(|| tankless_parasitic_power_w(n_bedrooms.expect("n_bedrooms always Some; parse_avg_water_draw_and_bedrooms resolves via HPXML/data_patches/default"))),
                    performance_adjustment: Some(perf_adj),
                    inlet_temp_c: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    avg_water_draw_l_per_day,
                    zone_type: zone_name.clone(),
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
                    zone_id,
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
            "Indirect Tank" => {
                let cfg = IndirectTankConfig {
                    equipment_id: None,
                    zone_id,
                    boiler_loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    ua_w_per_k,
                    hx_ua_w_per_k: None,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    draw_flow_rate_kg_s: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    boiler_loop_flow_rate_kg_s: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)
            }
            // Unreachable: canonical_water_heater_name (called at the top of
            // this loop) generates exactly the six names matched above and
            // rejects all others with Err, so execution never reaches this arm.
            _ => unreachable!(
                "canonical_water_heater_name validated '{}' but match did not cover it",
                name,
            ),
        };

        if name == "Indirect Tank" {
            spec.related_hvac_idref = related_hvac_idref;
        }
        spec.system_id = element_id(wh);
        specs.push(spec);
    }
    // Invariant: when multiple WaterHeatingSystem elements are present, each
    // must carry a unique SystemIdentifier/@id so downstream code can
    // cross-reference individual water heaters without ambiguity.
    let wh_count = descendants_named(details, "WaterHeatingSystem").len();
    if wh_count > 1 {
        let wh_specs_start = specs.len().saturating_sub(wh_count);
        let ids: Vec<_> = specs[wh_specs_start..]
            .iter()
            .filter_map(|s| s.system_id.as_deref())
            .collect();
        if ids.len() != wh_count {
            return Err(super::HpxmlError::Parse(format!(
                "expected {wh_count} unique system identifiers for {wh_count} WaterHeatingSystem \
                 elements but only {found} have an id attribute",
                found = ids.len(),
            ).into()));
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != ids.len() {
            return Err(super::HpxmlError::Parse(
                format!(
                    "duplicate SystemIdentifier/@id found among {wh_count} WaterHeatingSystem \
                 elements: {ids:?}"
                )
                .into(),
            ));
        }
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
fn parse_avg_water_draw_and_bedrooms(
    details: &XmlNode,
    data_patches: Option<&HpxmlDataPatches>,
) -> (Option<f64>, Option<f64>) {
    let n_bedrooms_raw = match details
        .path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "NumberofBedrooms",
        ])
        .and_then(|n| n.text.trim().parse::<f64>().ok())
    {
        Some(v) => v,
        None => resolve_bedroom_count(None, data_patches),
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
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

/// Parse a FuelType string value from HPXML into a [`FuelType`].
///
/// Accepts the HPXML fuel type vocabulary and maps it to the canonical
/// [`FuelType`] enum. Returns `Err` for unrecognised fuel type strings.
fn parse_water_heater_fuel_from_str(raw: &str) -> Result<FuelType, super::HpxmlError> {
    match normalize_ascii(raw).as_str() {
        "electricity" | "electric" => Ok(FuelType::Electric),
        "natural gas" | "natural_gas" | "gas" => Ok(FuelType::Gas),
        "propane" => Ok(FuelType::Propane),
        "oil" | "fuel oil" | "fuel_oil" | "fuel oil 1" | "fuel oil 2" | "fuel oil 4"
        | "fuel oil 5/6" | "kerosene" | "diesel" => Ok(FuelType::Oil),
        "wood" => Ok(FuelType::Wood),
        "wood pellets" | "wood_pellets" => Ok(FuelType::WoodPellet),
        "coal" | "anthracite coal" | "anthracite_coal" | "bituminous coal" | "bituminous_coal"
        | "coke" => Ok(FuelType::Coal),
        other => Err(super::HpxmlError::Parse(
            format!("unsupported water-heater FuelType '{other}'").into(),
        )),
    }
}

/// Resolve the fuel type for a `WaterHeatingSystem` element.
///
/// For space-heating boiler types (indirect tanks), `FuelType` is optional in
/// HPXML — the fuel comes from the linked heating system via `RelatedHVACSystem`.
/// When absent and the type is a boiler-linked system, we default to `Gas` since
/// the linked boiler's fuel will be resolved independently.
/// For all other water heater types, `FuelType` is required.
fn parse_water_heater_fuel(
    wh: &XmlNode,
    wh_type: &str,
) -> std::result::Result<FuelType, super::HpxmlError> {
    match child_text(wh, "FuelType") {
        Some(raw) => parse_water_heater_fuel_from_str(&raw),
        None => {
            let ty = wh_type.trim();
            if ty == "space-heating boiler with storage tank"
                || ty == "space-heating boiler with tankless coil"
            {
                Ok(FuelType::Gas)
            } else {
                Err(super::HpxmlError::Parse(
                    format!("WaterHeatingSystem type '{ty}' is missing required FuelType").into(),
                ))
            }
        }
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
        ("space-heating boiler with storage tank", _) => "Indirect Tank",
        ("space-heating boiler with tankless coil", _) => "Indirect Tank",
        _ => {
            return Err(super::HpxmlError::Parse(
                format!(
                    "unsupported HPXML water heater type/fuel combination: \
                 WaterHeaterType='{ty}', fuel='{fuel:?}'"
                )
                .into(),
            ));
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

    use super::super::building::{Building, Site, SiteType, Zone, ZoneType};

    /// Build a minimal [`Building`] wrapping the details node from a parsed HPXML.
    /// Used by tests that only exercise water-heater parsing and do not provide
    /// a complete building envelope. Zones are empty so `zone_id_for_location`
    /// returns `None`, preserving the pre-fix behaviour for tests that don't set
    /// zone topology.
    fn building_for_test(root: &XmlNode) -> Building {
        let details = root
            .path(&["Building", "BuildingDetails"])
            .cloned()
            .unwrap_or_else(|| root.clone());
        Building {
            site: Site {
                elevation_m: None,
                site_type: Some(SiteType::Suburban),
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
            },
            zones: vec![],
            boundaries: vec![],
            windows: vec![],
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
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: details,
        }
    }

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
            pilot_fraction_to_tank: None,
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
            max_combined_power_w: None,
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
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
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
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
    fn gas_tankless_parasitic_power_computed_from_hpxml_bedrooms() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>4</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic for 4 bedrooms must be {expected_w} W"
        );
    }

    #[test]
    fn gas_tankless_parasitic_power_falls_back_to_data_patches() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut patches = HpxmlDataPatches::default();
        patches.number_of_bedrooms = Some(5.0);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            &DefaultsStore::empty(),
            &mut specs,
            Some(&patches),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0529_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic for 5 bedrooms from patches must be {expected_w} W"
        );
    }

    #[test]
    fn gas_tankless_parasitic_power_defaults_to_two_bedrooms() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0333_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic default (2 bedrooms) must be {expected_w} W"
        );
    }

    #[test]
    fn electric_tankless_parasitic_power_is_none() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
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
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        assert!(
            cfg.parasitic_power_w.is_none(),
            "electric tankless must not have parasitic_power_w"
        );
    }

    #[test]
    fn hpxml_bedrooms_take_priority_over_data_patches() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>4</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut patches = HpxmlDataPatches::default();
        patches.number_of_bedrooms = Some(1.0);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            &DefaultsStore::empty(),
            &mut specs,
            Some(&patches),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "HPXML bedrooms=4 must beat patches bedrooms=1; expected {expected_w} W for 4 beds"
        );
    }

    #[test]
    fn fractional_bedrooms_rounded_to_nearest_integer() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3.7</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "fractional 3.7 bedrooms must round to 4; expected {expected_w} W"
        );
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

        let err = parse_water_heater_fuel(&wh, "storage water heater")
            .expect_err("invalid fuel must be rejected");
        assert!(
            err.to_string()
                .contains("unsupported water-heater FuelType")
        );
    }

    #[test]
    fn parse_water_heater_fuel_none_is_rejected() {
        let result = parse_water_heater_fuel_from_str("none");
        assert!(
            result.is_err(),
            "\"none\" is not a valid HPXML fuel type and must be rejected"
        );
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("unsupported water-heater FuelType"), "got: {err}");
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
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
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

    // Combi boiler types (HPXML v4 §8.5) resolve to the Indirect Tank
    // equipment model for combined boiler + indirect-tank DHW configurations.
    // Full IndirectTank equipment model is implemented in T-0134.

    #[test]
    fn combi_boiler_with_storage_tank_type_resolves_to_indirect_tank() {
        let name =
            canonical_water_heater_name("space-heating boiler with storage tank", FuelType::Gas)
                .expect("combi boiler with storage tank must resolve to Indirect Tank");
        assert_eq!(name, "Indirect Tank");
    }

    #[test]
    fn combi_boiler_with_tankless_coil_type_resolves_to_indirect_tank() {
        let name =
            canonical_water_heater_name("space-heating boiler with tankless coil", FuelType::Gas)
                .expect("combi boiler with tankless coil must resolve to Indirect Tank");
        assert_eq!(name, "Indirect Tank");
    }

    #[test]
    fn combi_boiler_full_xml_round_trip_resolves_to_indirect_tank() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
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
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("combi boiler with storage tank must resolve successfully");

        let spec = specs
            .iter()
            .find(|s| s.name == "Indirect Tank")
            .expect("Indirect Tank spec must be emitted");
        let cfg: IndirectTankConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed IndirectTankConfig");
        assert_eq!(cfg.setpoint_c, Some(conv::temperature_f_to_c(120.0)));
        let expected_vol_m3 = conv::volume_gal_to_m3(40.0 * 0.95);
        assert!(
            (cfg.tank_volume_m3.expect("tank volume must be set") - expected_vol_m3).abs() < 1e-6
        );
        assert!(
            cfg.avg_water_draw_l_per_day
                .expect("avg draw should be derived")
                > 0.0
        );
    }

    #[test]
    fn zone_id_for_location_maps_each_zone_type() {
        let building = Building {
            site: Site {
                elevation_m: None,
                site_type: Some(SiteType::Suburban),
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
            },
            zones: vec![
                Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
                Zone {
                    zone_type: ZoneType::Garage,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
                Zone {
                    zone_type: ZoneType::Attic,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
            ],
            boundaries: vec![],
            windows: vec![],
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
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: XmlNode {
                name: String::new(),
                attrs: std::collections::HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        };

        // Conditioned zone is at index 0 → zone_id 1
        assert_eq!(
            zone_id_for_location(&building, "conditioned space"),
            Some(1)
        );
        // Garage zone is at index 1 → zone_id 2
        assert_eq!(zone_id_for_location(&building, "garage"), Some(2));
        // Attic zone is at index 2 → zone_id 3
        assert_eq!(zone_id_for_location(&building, "attic vented"), Some(3));
        // Foundation not in building → None
        assert_eq!(zone_id_for_location(&building, "unfinished basement"), None);
        // Unknown location → None
        assert_eq!(zone_id_for_location(&building, "rooftop"), None);
    }

    #[test]
    fn water_heater_in_garage_receives_correct_zone_id() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site>
                      <SiteType>suburban</SiteType>
                    </Site>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Garages>
                      <Garage>
                        <SystemIdentifier id="g1"/>
                      </Garage>
                    </Garages>
                  </Enclosure>
                  <WaterHeating>
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
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.92</EnergyFactor>
                      <TankVolume>50</TankVolume>
                      <PerformanceAdjustment>0.93</PerformanceAdjustment>
                      <Location>garage</Location>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = crate::hpxml::building::parse_building(xml).expect("building must parse");
        let mut specs = Vec::new();
        resolve_water_heaters(&building, &DefaultsStore::empty(), &mut specs, None)
            .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("WH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed resistance config");

        // Garage is zone 2 (conditioned is zone 1).
        assert_eq!(
            cfg.zone_id,
            Some(2),
            "water heater in garage must have zone_id=2"
        );
        assert_eq!(cfg.zone_type.as_deref(), Some("garage"));
    }
}
