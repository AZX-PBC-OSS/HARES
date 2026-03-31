//! HVAC equipment resolution from HPXML into canonical equipment specs.
// Invariant: HVAC typed configs are built from typed/defaults-derived fields
// (including curve metadata), not from ad-hoc string-key translation logic.

use serde_json::{Map, Value, json};

use hares_equipment::EquipmentConfig;
use hares_equipment::hvac::cooling_config::{
    CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig,
};
use hares_equipment::hvac::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};
use hares_equipment::hvac::heating_config::{
    DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig,
    GasBoilerConfig, GasFurnaceConfig, IdealHvacConfig,
};
use hares_types::{BoundaryPolicy, FuelType, ScheduleSourceConfig};

use super::HpxmlError;
use super::building::{Boundary, BoundaryType, Building, DuctLocation, XmlNode, Zone, ZoneType};
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{child_f64, child_text, descendants_named};
use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.95;

fn airflow_defect_multiplier(params: &Map<String, Value>) -> f64 {
    params
        .get("airflow_defect_ratio")
        .or_else(|| params.get("AirflowDefectRatio"))
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
}

#[derive(Debug, Clone, Default)]
struct DuctDseParams {
    zone_id: Option<u16>,
    zone_type: Option<String>,
    house_volume_m3: f64,
    supply_leakage_frac: f64,
    supply_area_m2: f64,
    supply_r_m2_k_w: f64,
    return_leakage_frac: f64,
    return_area_m2: f64,
    return_r_m2_k_w: f64,
    latitude_deg: f64,
    longitude_deg: f64,
}

impl DuctDseParams {
    fn insert_into_map(&self, params: &mut Map<String, Value>) {
        if self.zone_id.is_none() || self.zone_type.is_none() {
            return;
        }
        if let Some(zone_id) = self.zone_id {
            params.insert("duct_zone_id".to_string(), json!(zone_id));
        }
        if let Some(zone_type) = &self.zone_type {
            params.insert(
                "duct_zone_type".to_string(),
                Value::String(zone_type.clone()),
            );
        }
        params.insert(
            "duct_house_volume_m3".to_string(),
            json!(self.house_volume_m3),
        );
        params.insert(
            "duct_supply_leakage_frac".to_string(),
            json!(self.supply_leakage_frac),
        );
        params.insert(
            "duct_supply_area_m2".to_string(),
            json!(self.supply_area_m2),
        );
        params.insert(
            "duct_supply_r_m2_k_w".to_string(),
            json!(self.supply_r_m2_k_w),
        );
        params.insert(
            "duct_return_leakage_frac".to_string(),
            json!(self.return_leakage_frac),
        );
        params.insert(
            "duct_return_area_m2".to_string(),
            json!(self.return_area_m2),
        );
        params.insert(
            "duct_return_r_m2_k_w".to_string(),
            json!(self.return_r_m2_k_w),
        );
        params.insert("duct_latitude_deg".to_string(), json!(self.latitude_deg));
        params.insert("duct_longitude_deg".to_string(), json!(self.longitude_deg));
    }
}

/// Extract raw duct parameters for ASHRAE 152 DSE calculation.
///
/// Scans `building.zones` for non-conditioned duct systems and passes through
/// the raw parameters needed by `hares_physics::ashrae152::calculate_dse()`.
/// The HVAC equipment `init` function computes DSE from these at init time,
/// when capacity and fan flow are known.
///
/// Falls back to `AnnualDistributionSystemEfficiency` from HPXML if present.
fn compute_duct_dse_params(building: &Building) -> DuctDseParams {
    use super::building::DuctType;

    let house_volume_m3 = building.conditioned_volume_m3.unwrap_or(400.0);

    // Check for direct DSE override from HPXML first.
    // (AnnualDistributionSystemEfficiency would be set on a per-equipment basis
    //  by the caller if available; this function handles the duct-based path.)

    // Aggregate supply vs return duct data from unconditioned zones.
    let mut supply_leakage = 0.0_f64;
    let mut supply_area_m2 = 0.0_f64;
    let mut supply_r_area_product = 0.0_f64;
    let mut supply_count = 0u32;
    let mut return_leakage = 0.0_f64;
    let mut return_area_m2 = 0.0_f64;
    let mut return_r_area_product = 0.0_f64;
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
                duct_zone_type_str = Some(zone_type_to_ashrae152_str(zone, building));
            }

            let leak = duct.leakage_fraction.unwrap_or(0.0);
            let area = duct.surface_area_m2.unwrap_or(0.0);
            let r_val = duct.insulation_r_value_m2_k_w.unwrap_or(0.0);

            match duct.duct_type {
                DuctType::Supply => {
                    supply_leakage += leak;
                    supply_area_m2 += area;
                    supply_r_area_product += area * r_val;
                    supply_count += 1;
                }
                DuctType::Return => {
                    return_leakage += leak;
                    return_area_m2 += area;
                    return_r_area_product += area * r_val;
                    return_count += 1;
                }
                DuctType::Unknown => {
                    // Unknown type: split evenly between supply and return
                    supply_leakage += leak * 0.5;
                    supply_area_m2 += area * 0.5;
                    supply_r_area_product += area * 0.5 * r_val;
                    supply_count += 1;
                    return_leakage += leak * 0.5;
                    return_area_m2 += area * 0.5;
                    return_r_area_product += area * 0.5 * r_val;
                    return_count += 1;
                }
            }
        }
    }

    if supply_count == 0 && return_count == 0 {
        return DuctDseParams::default();
    }

    let zone_idx = duct_zone_idx.unwrap_or(0);
    let zone_id = (zone_idx as u16) + 1;
    let supply_r_m2_k_w = if supply_area_m2 > 0.0 {
        supply_r_area_product / supply_area_m2
    } else {
        0.0
    };
    let return_r_m2_k_w = if return_area_m2 > 0.0 {
        return_r_area_product / return_area_m2
    } else {
        0.0
    };
    DuctDseParams {
        zone_id: Some(zone_id),
        zone_type: duct_zone_type_str,
        house_volume_m3,
        supply_leakage_frac: supply_leakage,
        supply_area_m2,
        supply_r_m2_k_w,
        return_leakage_frac: return_leakage,
        return_area_m2,
        return_r_m2_k_w,
        latitude_deg: building.site.latitude_deg.unwrap_or(40.0),
        longitude_deg: building.site.longitude_deg.unwrap_or(-100.0),
    }
}

/// Compute a `DuctConfig` from the duct parameter bundle produced by
/// `compute_duct_dse_params`.
///
/// DSE is pre-computed here using ASHRAE 152 so that typed-config equipment
/// init paths (which only see `DuctConfig.dse_heat/dse_cool`) apply duct
/// losses correctly. Uses a default airflow based on equipment type conventions.
///
/// Returns `DuctConfig { dse_heat: None, dse_cool: None }` when no duct zone
/// type was recorded (i.e., ducts are in conditioned space or absent).
fn compute_duct_config(
    duct_params: &DuctDseParams,
    capacity_w: f64,
    is_heating: bool,
    n_speeds: u8,
    is_heat_pump: bool,
) -> DuctConfig {
    use hares_physics::ashrae152::{Ashrae152ZoneType, DuctDseInput, calculate_dse};
    use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};

    let Some(zone_type_str) = duct_params.zone_type.as_deref() else {
        return DuctConfig::default();
    };

    let zone_type = match zone_type_str {
        "attic_vented" => Ashrae152ZoneType::AtticVented,
        "attic_vented_radiant_barrier" => Ashrae152ZoneType::AtticVentedRadiantBarrier,
        "attic_unvented" => Ashrae152ZoneType::AtticUnvented,
        "attic_unvented_radiant_barrier" => Ashrae152ZoneType::AtticUnventedRadiantBarrier,
        "garage" => Ashrae152ZoneType::Garage,
        "unvent_unins_crawlspace" => Ashrae152ZoneType::UnventUninsulatedCrawlspace,
        "unvent_crawlspace_ins_floor_wall" => Ashrae152ZoneType::UnventCrawlspaceInsFloorWall,
        "unvent_crawlspace_ins_floor" => Ashrae152ZoneType::UnventCrawlspaceInsFloor,
        "vent_unins_crawlspace" => Ashrae152ZoneType::VentUninsulatedCrawlspace,
        "vent_crawlspace_ins_floor_wall" => Ashrae152ZoneType::VentCrawlspaceInsFloorWall,
        "vent_crawlspace_ins_floor" => Ashrae152ZoneType::VentCrawlspaceInsFloor,
        "unins_basement" => Ashrae152ZoneType::UninsulatedBasement,
        "basement_ins_walls" => Ashrae152ZoneType::BasementInsWalls,
        "basement_ins_ceiling" => Ashrae152ZoneType::BasementInsCeiling,
        "under_slab" => Ashrae152ZoneType::UnderSlab,
        "ext_walls" => Ashrae152ZoneType::ExteriorWalls,
        _ => return DuctConfig::default(),
    };

    let lat = duct_params.latitude_deg;
    let lon = duct_params.longitude_deg;
    let house_vol = duct_params.house_volume_m3;
    let supply_leak = duct_params.supply_leakage_frac.clamp(0.0, 1.0);
    let supply_area = duct_params.supply_area_m2;
    let supply_r = duct_params.supply_r_m2_k_w;
    let return_leak = duct_params.return_leakage_frac.clamp(0.0, 1.0);
    let return_area = duct_params.return_area_m2;
    let return_r = duct_params.return_r_m2_k_w;

    if capacity_w <= 0.0 {
        return DuctConfig::default();
    }

    let cfm_per_ton = if is_heating { 350.0_f64 } else { 400.0_f64 };
    let fan_flow_m3_s = capacity_w * (cfm_per_ton * CFM_TO_M3_S / W_PER_TON);

    let input = DuctDseInput {
        zone_type,
        latitude_deg: lat,
        longitude_deg: lon,
        house_volume_m3: house_vol,
        supply_leakage_frac: supply_leak,
        supply_area_m2: supply_area,
        supply_r_nominal_m2_k_w: supply_r,
        return_leakage_frac: return_leak,
        return_area_m2: return_area,
        return_r_nominal_m2_k_w: return_r,
        is_heating,
        capacity_w,
        fan_flow_m3_s,
        n_speeds,
        capacity_low_w: None,
        fan_flow_low_m3_s: None,
        is_heat_pump,
    };

    let dse = calculate_dse(&input).clamp(0.0, 1.0);

    if is_heating {
        DuctConfig {
            dse_heat: Some(dse),
            ..DuctConfig::default()
        }
    } else {
        DuctConfig {
            dse_cool: Some(dse),
            ..DuctConfig::default()
        }
    }
}

/// Extract AFUE from the params map, trying both the bare AFUE tag path
/// and the AnnualHeatingEfficiency path.
fn afue_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("efficiency_afue")
        .and_then(Value::as_f64)
        .or_else(|| {
            let units = params
                .get("heating_efficiency_units")
                .and_then(Value::as_str)?;
            if units.eq_ignore_ascii_case("AFUE") {
                params.get("heating_efficiency").and_then(Value::as_f64)
            } else {
                None
            }
        })
}

/// Extract COP/efficiency for resistance heaters from the params map.
fn resistance_efficiency_from_params(params: &Map<String, Value>) -> f64 {
    params
        .get("heating_efficiency")
        .and_then(Value::as_f64)
        .or_else(|| params.get("efficiency_cop").and_then(Value::as_f64))
        .unwrap_or(1.0)
}

/// Extract SEER from the params map.
fn seer_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("efficiency_seer")
        .and_then(Value::as_f64)
        .or_else(|| {
            let units = params
                .get("cooling_efficiency_units")
                .and_then(Value::as_str)?;
            if units.eq_ignore_ascii_case("SEER") {
                params.get("cooling_efficiency").and_then(Value::as_f64)
            } else {
                None
            }
        })
}

/// Extract EER from the params map.
fn eer_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("efficiency_eer")
        .and_then(Value::as_f64)
        .or_else(|| {
            let units = params
                .get("cooling_efficiency_units")
                .and_then(Value::as_str)?;
            if units.eq_ignore_ascii_case("EER") || units.eq_ignore_ascii_case("EER2") {
                params.get("cooling_efficiency").and_then(Value::as_f64)
            } else {
                None
            }
        })
}

/// Extract HSPF from the params map.
fn hspf_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("efficiency_hspf")
        .and_then(Value::as_f64)
        .or_else(|| {
            let units = params
                .get("heating_efficiency_units")
                .and_then(Value::as_str)?;
            if units.eq_ignore_ascii_case("HSPF") {
                params.get("heating_efficiency").and_then(Value::as_f64)
            } else {
                None
            }
        })
}

/// Extract number_of_speeds from params (defaults to 1).
fn n_speeds_from_params(params: &Map<String, Value>) -> u8 {
    params
        .get("number_of_speeds")
        .and_then(Value::as_u64)
        .unwrap_or(1) as u8
}

fn array24_from_params(params: &Map<String, Value>, key: &str) -> Option<[f64; 24]> {
    let values = params.get(key)?.as_array()?;
    if values.is_empty() {
        return None;
    }
    let mut out = [0.0; 24];
    let mut count = 0usize;
    for (idx, value) in values.iter().take(24).enumerate() {
        out[idx] = value.as_f64()?;
        count = idx + 1;
    }
    let fill = *out.get(count.saturating_sub(1))?;
    for slot in out.iter_mut().skip(count) {
        *slot = fill;
    }
    Some(out)
}

fn schedule_source_from_params(
    params: &Map<String, Value>,
    prefix: &str,
) -> Option<ScheduleSourceConfig> {
    let source_key = format!("{prefix}_setpoint_source");
    if let Some(source) = params.get(&source_key) {
        return serde_json::from_value::<ScheduleSourceConfig>(source.clone()).ok();
    }

    let col_key = format!("{prefix}_setpoint_schedule_col");
    if let Some(col) = params.get(&col_key).and_then(Value::as_u64) {
        return Some(ScheduleSourceConfig::ColumnRef {
            col_idx: col as usize,
            boundary: BoundaryPolicy::Clamp,
        });
    }

    let weekday_key = format!("{prefix}_weekday_setpoints_c");
    let weekend_key = format!("{prefix}_weekend_setpoints_c");
    let weekday = array24_from_params(params, &weekday_key);
    let weekend = array24_from_params(params, &weekend_key);
    if let Some(wd) = weekday {
        return Some(ScheduleSourceConfig::DailyProfile {
            weekday: wd,
            weekend: weekend.unwrap_or(wd),
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
    }

    let setpoint_key = format!("{prefix}_setpoint_c");
    params
        .get(&setpoint_key)
        .and_then(Value::as_f64)
        .map(ScheduleSourceConfig::Constant)
}

fn static_setpoint_from_source(source: &Option<ScheduleSourceConfig>) -> Option<f64> {
    match source {
        Some(ScheduleSourceConfig::Constant(v)) => Some(*v),
        Some(ScheduleSourceConfig::DailyProfile { weekday, .. }) => Some(weekday[0]),
        _ => None,
    }
}

fn extract_stage_values(params: &Map<String, Value>, prefix: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for i in 0..32 {
        let key = format!("{prefix}_{i}");
        let Some(v) = params.get(&key).and_then(Value::as_f64) else {
            break;
        };
        out.push(v);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Extract fan_power_w from params (optional).
fn fan_power_from_params(params: &Map<String, Value>) -> Option<f64> {
    params.get("fan_power_w").and_then(Value::as_f64)
}

#[derive(Debug, Clone, Copy, Default)]
struct CurveBounds {
    x1_min: Option<f64>,
    x1_max: Option<f64>,
    x2_min: Option<f64>,
    x2_max: Option<f64>,
    ff_min: Option<f64>,
    ff_max: Option<f64>,
    plf_min: Option<f64>,
    plf_max: Option<f64>,
}

fn extract_curve_bounds(params: &Map<String, Value>) -> CurveBounds {
    CurveBounds {
        x1_min: params.get("biquadratic_x1_min").and_then(Value::as_f64),
        x1_max: params.get("biquadratic_x1_max").and_then(Value::as_f64),
        x2_min: params.get("biquadratic_x2_min").and_then(Value::as_f64),
        x2_max: params.get("biquadratic_x2_max").and_then(Value::as_f64),
        ff_min: params.get("ff_min").and_then(Value::as_f64),
        ff_max: params.get("ff_max").and_then(Value::as_f64),
        plf_min: params.get("plf_min").and_then(Value::as_f64),
        plf_max: params.get("plf_max").and_then(Value::as_f64),
    }
}

/// Build a `GasFurnaceConfig` typed config from the resolved params map.
/// Returns `None` when required fields are missing (capacity_w).
/// AFUE defaults to 0.80 when not specified in the HPXML.
fn try_build_gas_furnace_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    let afue = afue_from_params(params).unwrap_or(0.80);
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    let ducts = compute_duct_config(duct_params, capacity_w, true, n_speeds, false);

    let cfg = GasFurnaceConfig {
        equipment_id: None,
        zone_id: None,
        afue,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        ducts,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Gas Furnace".to_string(),
        cfg,
    ))
}

/// Build an `ElectricFurnaceConfig` typed config.
fn try_build_electric_furnace_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;
    let heating_efficiency = resistance_efficiency_from_params(params);
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    let ducts = compute_duct_config(duct_params, capacity_w, true, n_speeds, false);

    let cfg = ElectricFurnaceConfig {
        equipment_id: None,
        zone_id: None,
        eir: heating_efficiency,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        ducts,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Furnace".to_string(),
        cfg,
    ))
}

/// Build a `GasBoilerConfig` typed config.
/// AFUE defaults to 0.80 when not specified in the HPXML.
fn try_build_gas_boiler_config(name: &str, params: &Map<String, Value>) -> Option<EquipmentConfig> {
    let afue = afue_from_params(params).unwrap_or(0.80);
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);

    let flow_rate_kg_s = params
        .get("flow_rate_kg_s")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let return_temp_c = params
        .get("return_temp_c")
        .and_then(Value::as_f64)
        .unwrap_or(40.0);

    let cfg = GasBoilerConfig {
        equipment_id: None,
        zone_id: None,
        loop_id: None,
        afue,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        flow_rate_kg_s,
        return_temp_c,
        fluid_type: hares_types::FluidType::Water,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Gas Boiler".to_string(),
        cfg,
    ))
}

/// Build an `ElectricBoilerConfig` typed config.
fn try_build_electric_boiler_config(
    name: &str,
    params: &Map<String, Value>,
) -> Option<EquipmentConfig> {
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;
    let heating_efficiency = resistance_efficiency_from_params(params);
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);

    let flow_rate_kg_s = params
        .get("flow_rate_kg_s")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let return_temp_c = params
        .get("return_temp_c")
        .and_then(Value::as_f64)
        .unwrap_or(40.0);

    let cfg = ElectricBoilerConfig {
        equipment_id: None,
        zone_id: None,
        loop_id: None,
        eir: heating_efficiency,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        flow_rate_kg_s,
        return_temp_c,
        fluid_type: hares_types::FluidType::Water,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Boiler".to_string(),
        cfg,
    ))
}

/// Build an `ElectricBaseboardConfig` typed config.
fn try_build_electric_baseboard_config(
    name: &str,
    params: &Map<String, Value>,
) -> Option<EquipmentConfig> {
    let capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64)?;

    let cfg = ElectricBaseboardConfig {
        equipment_id: None,
        zone_id: None,
        capacity_w,
        eir: 1.0,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Baseboard".to_string(),
        cfg,
    ))
}

/// Build an `IdealHvacConfig` typed config.
fn try_build_ideal_hvac_config(name: &str, params: &Map<String, Value>) -> Option<EquipmentConfig> {
    let heating_capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64);
    let cooling_capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64);
    let heating_eir = params
        .get("heating_efficiency")
        .and_then(Value::as_f64)
        .map(|v| 1.0 / v.max(1e-6));
    let cooling_eir = seer_from_params(params).map(|seer| 3.412_141_633_f64 / seer.max(1e-6));
    let fraction = params.get("fraction_load_served").and_then(Value::as_f64);
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");

    let cfg = IdealHvacConfig {
        equipment_id: None,
        zone_id: None,
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        deadband_c: None,
        n_speeds: Some(n_speeds_from_params(params)),
        ideal_capacity_mode: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        heating_capacity_w,
        cooling_capacity_w,
        heating_eir,
        cooling_eir,
        shr: params.get("shr").and_then(Value::as_f64),
        fraction_heating_load_served: fraction,
        fraction_cooling_load_served: fraction,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Ideal HVAC".to_string(),
        cfg,
    ))
}

/// Build a `CentralAirConditionerConfig` typed config.
fn try_build_central_ac_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    let seer = seer_from_params(params)?;
    let eir = 3.412_141_633 / seer.max(1e-6);
    let capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64)?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    let shr = params.get("shr").and_then(Value::as_f64);
    let system_type = params
        .get("system_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let startup_cd = params.get("startup_cd").and_then(Value::as_f64);
    let fraction_load_served = params.get("fraction_load_served").and_then(Value::as_f64);
    let duct = compute_duct_config(duct_params, capacity_w, false, n_speeds, false);
    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w =
        400.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params);
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");

    let cfg = CentralAirConditionerConfig {
        equipment_id: None,
        zone_id: None,
        capacity_w,
        eir,
        shr,
        number_of_speeds: n_speeds,
        stage_capacities_w: extract_stage_values(params, "cooling_capacity_w_stage"),
        stage_eirs: extract_stage_values(params, "cooling_eir_stage"),
        stage_shrs: extract_stage_values(params, "shr"),
        fan_power_w,
        fan_power_w_per_cfm: params.get("fan_power_w_per_cfm").and_then(Value::as_f64),
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        hysteresis_c: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        airflow_m3_s_per_w: Some(airflow_m3_s_per_w),
        fraction_load_served,
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
        duct,
        system_type,
        startup_cd,
        biquadratic_x1_min: curve_bounds.x1_min,
        biquadratic_x1_max: curve_bounds.x1_max,
        biquadratic_x2_min: curve_bounds.x2_min,
        biquadratic_x2_max: curve_bounds.x2_max,
        ff_min: curve_bounds.ff_min,
        ff_max: curve_bounds.ff_max,
        plf_min: curve_bounds.plf_min,
        plf_max: curve_bounds.plf_max,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Air Conditioner".to_string(),
        cfg,
    ))
}

/// Build a `RoomAcConfig` typed config.
fn try_build_room_ac_config(name: &str, params: &Map<String, Value>) -> Option<EquipmentConfig> {
    let capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64)?;
    // Room ACs are rated with EER, not SEER. SEER cannot be substituted for EER
    // because the test conditions and cycling correction factors differ; treating
    // SEER as EER would overestimate efficiency by ~10–15%.
    let eer = eer_from_params(params)?;
    let eir = 3.412_141_633 / eer.max(1e-6);

    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w =
        320.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params);
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let cfg = RoomAcConfig {
        equipment_id: None,
        zone_id: None,
        capacity_w,
        eir,
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        hysteresis_c: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        airflow_m3_s_per_w: Some(airflow_m3_s_per_w),
        biquadratic_x1_min: curve_bounds.x1_min,
        biquadratic_x1_max: curve_bounds.x1_max,
        biquadratic_x2_min: curve_bounds.x2_min,
        biquadratic_x2_max: curve_bounds.x2_max,
        ff_min: curve_bounds.ff_min,
        ff_max: curve_bounds.ff_max,
        plf_min: curve_bounds.plf_min,
        plf_max: curve_bounds.plf_max,
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Room AC".to_string(),
        cfg,
    ))
}

/// Build a `DehumidifierConfig` typed config.
fn try_build_dehumidifier_config(
    name: &str,
    params: &Map<String, Value>,
) -> Option<EquipmentConfig> {
    let cfg = DehumidifierConfig {
        equipment_id: None,
        zone_id: None,
        capacity_liters_per_day: params
            .get("capacity_liters_per_day")
            .and_then(Value::as_f64),
        energy_factor: params.get("energy_factor").and_then(Value::as_f64),
        integrated_energy_factor: params
            .get("integrated_energy_factor")
            .and_then(Value::as_f64),
        fraction_served: params.get("fraction_served").and_then(Value::as_f64),
        target_rh: params.get("target_rh").and_then(Value::as_f64),
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        "Dehumidifier".to_string(),
        cfg,
    ))
}

/// Build a `HeatPumpHeaterConfig` typed config from combined heat-pump params.
fn try_build_heat_pump_heater_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
    is_mini_split: bool,
) -> Option<EquipmentConfig> {
    let heating_capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64);
    let cooling_capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64);
    let n_speeds = if is_mini_split {
        4
    } else {
        n_speeds_from_params(params)
    };
    let heating_eir = hspf_from_params(params).map(|hspf| 3.412_141_633 / hspf.max(1e-6));
    let cooling_eir = seer_from_params(params).map(|seer| 3.412_141_633 / seer.max(1e-6));
    let shr = params.get("shr").and_then(Value::as_f64);
    let fan_power_w = fan_power_from_params(params);
    let backup_capacity_w = params.get("backup_capacity_w").and_then(Value::as_f64);
    let backup_eir = params.get("backup_eir").and_then(Value::as_f64);
    let backup_fuel = params
        .get("backup_fuel")
        .and_then(Value::as_str)
        .map(str::to_string);
    let fraction_heating_load_served = params
        .get("fraction_heating_load_served")
        .and_then(Value::as_f64);
    let fraction_cooling_load_served = params
        .get("fraction_cooling_load_served")
        .and_then(Value::as_f64);
    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w = if is_mini_split {
        312.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
    } else {
        400.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
    };
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");

    let ref_cap = heating_capacity_w.or(cooling_capacity_w).unwrap_or(0.0);
    let duct = if is_mini_split {
        DuctConfig::default()
    } else {
        compute_duct_config(duct_params, ref_cap, true, n_speeds, true)
    };

    let ochre_class = if is_mini_split {
        "MSHP Heater"
    } else {
        "ASHP Heater"
    };

    let cfg = HeatPumpHeaterConfig {
        equipment_id: None,
        zone_id: None,
        heating_capacity_w,
        heating_eir,
        stage_heating_capacities_w: extract_stage_values(params, "heating_capacity_w_stage"),
        stage_heating_eirs: extract_stage_values(params, "heating_eir_stage"),
        backup_fuel,
        backup_capacity_w,
        backup_eir,
        fraction_heating_load_served,
        cooling_capacity_w,
        cooling_eir,
        stage_cooling_capacities_w: extract_stage_values(params, "cooling_capacity_w_stage"),
        stage_cooling_eirs: extract_stage_values(params, "cooling_eir_stage"),
        stage_shrs: extract_stage_values(params, "shr"),
        fraction_cooling_load_served,
        number_of_speeds: n_speeds,
        is_mini_split,
        shr,
        fan_power_w,
        fan_power_w_per_cfm: params.get("fan_power_w_per_cfm").and_then(Value::as_f64),
        airflow_m3_s_per_w: Some(airflow_m3_s_per_w),
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        hysteresis_c: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        hp_lockout_temp_c: None,
        er_lockout_temp_c: None,
        max_oat_supplemental_c: None,
        er_setpoint_offset_c: None,
        er_hard_lockout_time_s: None,
        duct,
        biquadratic_x1_min: curve_bounds.x1_min,
        biquadratic_x1_max: curve_bounds.x1_max,
        biquadratic_x2_min: curve_bounds.x2_min,
        biquadratic_x2_max: curve_bounds.x2_max,
        ff_min: curve_bounds.ff_min,
        ff_max: curve_bounds.ff_max,
        plf_min: curve_bounds.plf_min,
        plf_max: curve_bounds.plf_max,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        ochre_class.to_string(),
        cfg,
    ))
}

/// Build a `HeatPumpCoolerConfig` typed config from combined heat-pump params.
fn try_build_heat_pump_cooler_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
    is_mini_split: bool,
) -> Option<EquipmentConfig> {
    let heating_capacity_w = params.get("heating_capacity_w").and_then(Value::as_f64);
    let cooling_capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64);
    let n_speeds = if is_mini_split {
        4
    } else {
        n_speeds_from_params(params)
    };
    let heating_eir = hspf_from_params(params).map(|hspf| 3.412_141_633 / hspf.max(1e-6));
    let cooling_eir = seer_from_params(params).map(|seer| 3.412_141_633 / seer.max(1e-6));
    let shr = params.get("shr").and_then(Value::as_f64);
    let fan_power_w = fan_power_from_params(params);
    let backup_capacity_w = params.get("backup_capacity_w").and_then(Value::as_f64);
    let backup_eir = params.get("backup_eir").and_then(Value::as_f64);
    let backup_fuel = params
        .get("backup_fuel")
        .and_then(Value::as_str)
        .map(str::to_string);
    let fraction_heating_load_served = params
        .get("fraction_heating_load_served")
        .and_then(Value::as_f64);
    let fraction_cooling_load_served = params
        .get("fraction_cooling_load_served")
        .and_then(Value::as_f64);
    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w = if is_mini_split {
        312.0_f64 * CFM_TO_M3_S / W_PER_TON
    } else {
        400.0_f64 * CFM_TO_M3_S / W_PER_TON
    } * airflow_defect_multiplier(params);
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");

    let ref_cap = cooling_capacity_w.or(heating_capacity_w).unwrap_or(0.0);
    let duct = if is_mini_split {
        DuctConfig::default()
    } else {
        compute_duct_config(duct_params, ref_cap, false, n_speeds, true)
    };

    let ochre_class = if is_mini_split {
        "MSHP Cooler"
    } else {
        "ASHP Cooler"
    };

    let cfg = HeatPumpCoolerConfig {
        equipment_id: None,
        zone_id: None,
        heating_capacity_w,
        heating_eir,
        stage_heating_capacities_w: extract_stage_values(params, "heating_capacity_w_stage"),
        stage_heating_eirs: extract_stage_values(params, "heating_eir_stage"),
        backup_fuel,
        backup_capacity_w,
        backup_eir,
        fraction_heating_load_served,
        cooling_capacity_w,
        cooling_eir,
        stage_cooling_capacities_w: extract_stage_values(params, "cooling_capacity_w_stage"),
        stage_cooling_eirs: extract_stage_values(params, "cooling_eir_stage"),
        stage_shrs: extract_stage_values(params, "shr"),
        fraction_cooling_load_served,
        number_of_speeds: n_speeds,
        is_mini_split,
        shr,
        fan_power_w,
        fan_power_w_per_cfm: params.get("fan_power_w_per_cfm").and_then(Value::as_f64),
        airflow_m3_s_per_w: Some(airflow_m3_s_per_w),
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        hysteresis_c: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        duct,
        biquadratic_x1_min: curve_bounds.x1_min,
        biquadratic_x1_max: curve_bounds.x1_max,
        biquadratic_x2_min: curve_bounds.x2_min,
        biquadratic_x2_max: curve_bounds.x2_max,
        ff_min: curve_bounds.ff_min,
        ff_max: curve_bounds.ff_max,
        plf_min: curve_bounds.plf_min,
        plf_max: curve_bounds.plf_max,
    };
    Some(EquipmentConfig::from_typed(
        name.to_string(),
        ochre_class.to_string(),
        cfg,
    ))
}

/// OCHRE `get_duct_info` threshold for "insulated floor" (IP R-value 5.3 → SI).
///
/// OCHRE uses `fnd_ceil_ins > 5.3` (ft²·h·°F/Btu). Converting: 5.3 / 5.678 ≈ 0.934 m²·K/W.
const FLOOR_INS_THRESHOLD_M2_K_W: f64 = 5.3 / 5.678;

/// Return true if any boundary is a floor between the conditioned zone and the
/// foundation zone with sufficient insulation to count as an "insulated floor".
///
/// Mirrors OCHRE `fnd_ceil_ins = boundaries["Foundation Ceiling"]["Boundary R Value"]`.
fn foundation_floor_is_insulated(boundaries: &[Boundary]) -> bool {
    boundaries.iter().any(|b| {
        b.boundary_type == BoundaryType::Floor
            && matches!(
                (&b.interior_zone, &b.exterior_zone),
                (Some(ZoneType::Conditioned), Some(ZoneType::Foundation))
                    | (Some(ZoneType::Foundation), Some(ZoneType::Conditioned))
            )
            && b.assembly_r_value_m2_k_w
                .is_some_and(|r| r > FLOOR_INS_THRESHOLD_M2_K_W)
    })
}

/// Return true if any FoundationWall boundary adjacent to the foundation zone
/// has insulation (insulation_details is not "Uninsulated" and not None).
///
/// Mirrors OCHRE `fnd_wall_ins = boundaries["Foundation Wall"]["Insulation Details"]`.
fn foundation_wall_is_insulated(boundaries: &[Boundary]) -> bool {
    boundaries.iter().any(|b| {
        b.boundary_type == BoundaryType::FoundationWall
            && matches!(
                b.insulation_details.as_deref(),
                Some(s) if s != "Uninsulated"
            )
    })
}

/// Map HPXML zone to ASHRAE 152 zone type string.
///
/// Mirrors OCHRE `get_duct_info()` (ochre/utils/equipment.py:48-78).
/// For Foundation zones, uses `building.foundation_name` and boundary insulation
/// data to distinguish all ASHRAE 152 crawlspace/basement subtypes.
fn zone_type_to_ashrae152_str(zone: &Zone, building: &Building) -> String {
    match zone.zone_type {
        ZoneType::Attic => {
            if zone.vented {
                "attic_vented".into()
            } else {
                "attic_unvented".into()
            }
        }
        ZoneType::Garage => "garage".into(),
        ZoneType::Foundation => {
            let fnd_name = building.foundation_name.as_deref().unwrap_or("");
            let wall_ins = foundation_wall_is_insulated(&building.boundaries);
            let floor_ins = foundation_floor_is_insulated(&building.boundaries);
            if fnd_name == "Crawlspace" {
                let v = if zone.vented { "vent" } else { "unvent" };
                if wall_ins && floor_ins {
                    format!("{v}_crawlspace_ins_floor_wall")
                } else if floor_ins {
                    format!("{v}_crawlspace_ins_floor")
                } else {
                    format!("{v}_unins_crawlspace")
                }
            } else if fnd_name.contains("Basement") {
                if wall_ins {
                    "basement_ins_walls".into()
                } else if floor_ins {
                    "basement_ins_ceiling".into()
                } else {
                    "unins_basement".into()
                }
            } else {
                // Unknown foundation sub-type: fall back to uninsulated crawlspace.
                if zone.vented {
                    "vent_unins_crawlspace".into()
                } else {
                    "unvent_unins_crawlspace".into()
                }
            }
        }
        _ => "attic_vented".into(),
    }
}

/// Compute basement heat distribution params for heating equipment.
///
/// OCHRE HVAC.py lines 181-184: for finished basements, 20% of DSE-adjusted
/// capacity is routed to the Foundation zone. Returns empty map when the
/// building does not have a finished basement.
fn compute_basement_params(building: &Building) -> Map<String, Value> {
    let mut params = Map::new();
    if building.foundation_name.as_deref() != Some("Finished Basement") {
        return params;
    }
    let Some(zone_idx) = building
        .zones
        .iter()
        .position(|z| matches!(z.zone_type, ZoneType::Foundation))
    else {
        return params;
    };
    let zone_id = (zone_idx as u16) + 1;
    params.insert("basement_zone_id".to_string(), json!(zone_id));
    params.insert("basement_airflow_ratio".to_string(), json!(0.2_f64));
    params
}

pub(super) fn resolve_hvac(
    building: &Building,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let details = &building.details_xml;
    let Some(hvac) = details.path(&["Systems", "HVAC"]) else {
        return Ok(());
    };

    let setpoint_params = parse_hvac_setpoint_params(details);
    let duct_params = compute_duct_dse_params(building);
    let basement_params = compute_basement_params(building);

    for heating in descendants_named(hvac, "HeatingSystem") {
        let fuel = super::xml_helpers::parse_fuel(
            child_text(heating, "HeatingSystemFuel")
                .as_deref()
                .or(child_text(heating, "FuelType").as_deref()),
        );
        let system_type = parse_named_type(heating, "HeatingSystemType").ok_or_else(|| {
            HpxmlError::Parse("HeatingSystem is missing required HeatingSystemType element".into())
        })?;
        let name = canonical_hvac_heating_name(&system_type, fuel)?;
        let mut params = Map::new();
        insert_capacity_kbtu_h(&mut params, heating, "HeatingCapacity");
        insert_capacity_w(
            &mut params,
            heating,
            "HeatingCapacity",
            "heating_capacity_w",
        );
        insert_capacity_w(
            &mut params,
            heating,
            "CoolingCapacity",
            "cooling_capacity_w",
        );
        insert_annual_efficiency(&mut params, heating, true);
        if name == "Ideal HVAC" {
            insert_annual_efficiency(&mut params, heating, false);
        }
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
        apply_building_setpoint_profiles(building, &mut params, true, name == "Ideal HVAC");
        duct_params.insert_into_map(&mut params);
        for (k, v) in &basement_params {
            params.insert(k.clone(), v.clone());
        }
        // OCHRE only applies startup_cd for heat pump heaters (ASHP/MSHP),
        // not for furnaces, boilers, or baseboard.
        if matches!(name.as_str(), "ASHP Heater" | "MSHP Heater") {
            insert_startup_degradation(&mut params, &name, true);
        }
        let typed_config = match name.as_str() {
            "Gas Furnace" => try_build_gas_furnace_config(&name, &params, &duct_params),
            "Electric Furnace" => try_build_electric_furnace_config(&name, &params, &duct_params),
            "Gas Boiler" => try_build_gas_boiler_config(&name, &params),
            "Electric Boiler" => try_build_electric_boiler_config(&name, &params),
            "Electric Baseboard" => try_build_electric_baseboard_config(&name, &params),
            "Ideal HVAC" => try_build_ideal_hvac_config(&name, &params),
            _ => None,
        };
        let mut spec = build_spec(name, fuel, params, defaults);
        spec.typed_config = typed_config;
        specs.push(spec);
    }

    for cooling in descendants_named(hvac, "CoolingSystem") {
        let fuel = super::xml_helpers::parse_fuel(
            child_text(cooling, "CoolingSystemFuel")
                .as_deref()
                .or(Some("electricity")),
        );
        let system_type = child_text(cooling, "CoolingSystemType").ok_or_else(|| {
            HpxmlError::Parse("CoolingSystem is missing required CoolingSystemType element".into())
        })?;
        let name = canonical_hvac_cooling_name(&system_type, fuel)?;
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
        insert_startup_degradation(&mut params, &name, false);

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
        apply_building_setpoint_profiles(building, &mut params, false, true);
        if name != "Room AC" {
            duct_params.insert_into_map(&mut params);
        }
        let typed_config = match name.as_str() {
            "Air Conditioner" => try_build_central_ac_config(&name, &params, &duct_params),
            "Room AC" => try_build_room_ac_config(&name, &params),
            _ => None,
        };
        let mut spec = build_spec(name, fuel, params, defaults);
        spec.typed_config = typed_config;
        specs.push(spec);
    }

    for heat_pump in descendants_named(hvac, "HeatPump") {
        let heat_pump_type = child_text(heat_pump, "HeatPumpType")
            .ok_or_else(|| {
                HpxmlError::Parse("HeatPump is missing required HeatPumpType element".into())
            })?
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
                    params.insert(
                        param_key.to_string(),
                        json!(conv::temperature_f_to_c(f_val)),
                    );
                    break;
                }
            }
        }

        insert_mode_and_speed_metadata(
            &mut params,
            child_text(heat_pump, "CompressorType").as_deref(),
        );
        apply_default_hvac_speed_fallback(&mut params);
        if heat_pump_type == "mini-split" {
            params.insert("number_of_speeds".to_string(), json!(4));
            params.insert(
                "speed_control_mode".to_string(),
                Value::String("variable_speed".to_string()),
            );
        }

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
        apply_building_setpoint_profiles(building, &mut params, true, true);
        if heat_pump_type != "mini-split" {
            duct_params.insert_into_map(&mut params);
        }

        if let Some((heater_name, cooler_name)) = split {
            let is_mini_split = heat_pump_type == "mini-split";
            let mut heater_params = params.clone();
            let mut cooler_params = params;
            for (k, v) in &basement_params {
                heater_params.insert(k.clone(), v.clone());
            }
            apply_multispeed_heating_parameters(&mut heater_params, defaults, heater_name);
            apply_multispeed_cooling_parameters(&mut cooler_params, defaults, cooler_name);
            insert_startup_degradation(&mut heater_params, heater_name, true);
            insert_startup_degradation(&mut cooler_params, cooler_name, false);

            let heater_typed = try_build_heat_pump_heater_config(
                heater_name,
                &heater_params,
                &duct_params,
                is_mini_split,
            );
            let cooler_typed = try_build_heat_pump_cooler_config(
                cooler_name,
                &cooler_params,
                &duct_params,
                is_mini_split,
            );

            let mut heater_spec = build_spec(
                heater_name.to_string(),
                FuelType::Electric,
                heater_params,
                defaults,
            );
            heater_spec.typed_config = heater_typed;
            specs.push(heater_spec);

            let mut cooler_spec = build_spec(
                cooler_name.to_string(),
                FuelType::Electric,
                cooler_params,
                defaults,
            );
            cooler_spec.typed_config = cooler_typed;
            specs.push(cooler_spec);
        }
    }

    for dehumidifier in descendants_named(hvac, "Dehumidifier") {
        let mut params = Map::new();
        if let Some(cap_pints_day) = child_f64(dehumidifier, "Capacity") {
            params.insert(
                "capacity_liters_per_day".to_string(),
                json!(cap_pints_day * 0.473_176_473),
            );
        }
        if let Some(ef) = child_f64(dehumidifier, "EnergyFactor") {
            params.insert("energy_factor".to_string(), json!(ef));
        }
        if let Some(ief) = child_f64(dehumidifier, "IntegratedEnergyFactor") {
            params.insert("integrated_energy_factor".to_string(), json!(ief));
        }
        if let Some(frac) = child_f64(dehumidifier, "FractionDehumidificationLoadServed")
            .or_else(|| child_f64(dehumidifier, "FractionLoadServed"))
        {
            params.insert("fraction_served".to_string(), json!(frac));
        }
        if let Some(setpoint) = child_f64(dehumidifier, "DehumidistatSetpoint") {
            params.insert("target_rh".to_string(), json!(setpoint));
        }
        let typed_config = try_build_dehumidifier_config("Dehumidifier", &params);
        let mut spec = build_spec(
            "Dehumidifier".to_string(),
            FuelType::Electric,
            params,
            defaults,
        );
        spec.typed_config = typed_config;
        specs.push(spec);
    }

    Ok(())
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

fn canonical_hvac_heating_name(
    system_type: &str,
    fuel: FuelType,
) -> std::result::Result<String, HpxmlError> {
    let ty = system_type.trim();
    if matches!(ty, "IdealHVAC" | "Ideal HVAC" | "IdealHvac") {
        return Ok("Ideal HVAC".to_string());
    }
    let name = match (ty, fuel) {
        ("ElectricResistance", FuelType::Electric) => "Electric Baseboard",
        ("Furnace", FuelType::Electric)
        | ("WallFurnace", FuelType::Electric)
        | ("FloorFurnace", FuelType::Electric) => "Electric Furnace",
        ("Boiler", FuelType::Electric) => "Electric Boiler",
        ("Furnace", FuelType::Gas)
        | ("WallFurnace", FuelType::Gas)
        | ("FloorFurnace", FuelType::Gas) => "Gas Furnace",
        ("Boiler", FuelType::Gas) => "Gas Boiler",
        _ => {
            return Err(HpxmlError::Parse(format!(
                "unsupported HPXML heating system type/fuel combination: \
                 HeatingSystemType='{ty}', fuel='{fuel:?}'"
            )));
        }
    };
    Ok(name.to_string())
}

fn canonical_hvac_cooling_name(
    system_type: &str,
    _fuel: FuelType,
) -> std::result::Result<String, HpxmlError> {
    let ty = system_type.trim();
    let name = match ty {
        "central air conditioner" => "Air Conditioner",
        "room air conditioner" => "Room AC",
        _ => {
            return Err(HpxmlError::Parse(format!(
                "unsupported HPXML cooling system type: CoolingSystemType='{ty}'"
            )));
        }
    };
    Ok(name.to_string())
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

/// Startup capacity degradation coefficient per Winkler (2011) / EnergyPlus.
/// OCHRE `utils/equipment.py:470-500`.
fn calc_startup_degradation(
    is_heating: bool,
    equipment_name: &str,
    efficiency_ip: f64,
    n_speeds: usize,
) -> f64 {
    if is_heating {
        match n_speeds {
            1 => {
                if efficiency_ip < 7.0 {
                    0.20
                } else {
                    0.11
                }
            }
            2 => 0.11,
            _ => 0.0,
        }
    } else if equipment_name == "room ac" {
        0.22
    } else {
        match n_speeds {
            1 => {
                if efficiency_ip < 13.0 {
                    0.20
                } else {
                    0.07
                }
            }
            2 => 0.11,
            _ => 0.0,
        }
    }
}

/// Insert startup capacity degradation for compressor-driven equipment.
fn insert_startup_degradation(
    params: &mut Map<String, Value>,
    equipment_name: &str,
    is_heating: bool,
) {
    let n_speeds = params
        .get("number_of_speeds")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;

    // Get efficiency in IP units (HSPF for heating, SEER for cooling).
    let efficiency_ip = if is_heating {
        params
            .get("efficiency_hspf")
            .and_then(Value::as_f64)
            .unwrap_or(8.0)
    } else {
        params
            .get("efficiency_seer")
            .and_then(Value::as_f64)
            .or_else(|| params.get("cooling_efficiency").and_then(Value::as_f64))
            .unwrap_or(13.0)
    };

    let c_d = calc_startup_degradation(is_heating, equipment_name, efficiency_ip, n_speeds);
    params.insert("startup_cd".to_string(), json!(c_d));
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
        if !is_heating && i < multispeed.shrs.len() {
            params.insert(format!("shr_{i}"), json!(multispeed.shrs[i]));
        }
    }

    if let Some(curve_set) = curves {
        if let Some(coeff_text) = serialize_stage_plr_coefficients(curve_set, n_speeds) {
            params.insert(
                "eir_plr_coefficients".to_string(),
                Value::String(coeff_text),
            );
        }
        if let Some(primary) = select_primary_curve_pair(curve_set, n_speeds) {
            params.insert(
                "capacity_biquadratic_coeffs".to_string(),
                Value::String(format!("{:?}", primary.cap_coeffs)),
            );
            params.insert(
                "eir_biquadratic_coeffs".to_string(),
                Value::String(format!("{:?}", primary.eir_coeffs)),
            );
            params.insert("biquadratic_x1_min".to_string(), json!(primary.x1_bounds.0));
            params.insert("biquadratic_x1_max".to_string(), json!(primary.x1_bounds.1));
            params.insert("biquadratic_x2_min".to_string(), json!(primary.x2_bounds.0));
            params.insert("biquadratic_x2_max".to_string(), json!(primary.x2_bounds.1));
            if let Some((ff_min, ff_max)) = primary.ff_bounds {
                params.insert("ff_min".to_string(), json!(ff_min));
                params.insert("ff_max".to_string(), json!(ff_max));
            }
            if let Some((plf_min, plf_max)) = primary.plf_bounds {
                params.insert("plf_min".to_string(), json!(plf_min));
                params.insert("plf_max".to_string(), json!(plf_max));
            }
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

struct PrimaryCurvePair {
    cap_coeffs: [f64; 6],
    eir_coeffs: [f64; 6],
    x1_bounds: (f64, f64),
    x2_bounds: (f64, f64),
    ff_bounds: Option<(f64, f64)>,
    plf_bounds: Option<(f64, f64)>,
}

fn select_primary_curve_pair(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Option<PrimaryCurvePair> {
    let variants = select_variants_for_speed_count(curve_set, n_speeds);
    let selected = variants.last().copied()?;
    Some(PrimaryCurvePair {
        cap_coeffs: selected.cap_t.coeffs,
        eir_coeffs: selected.eir_t.coeffs,
        x1_bounds: selected.cap_t.x1_bounds,
        x2_bounds: selected.cap_t.x2_bounds,
        ff_bounds: selected.ff_bounds,
        plf_bounds: selected.plf_bounds,
    })
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
            if let Some(vals) =
                super::xml_helpers::parse_setpoint_from_control(control, hvac_type, weekday)
            {
                out.push((param_key, json!(vals)));
            }
        }
    }

    out
}

fn apply_building_setpoint_profiles(
    building: &Building,
    params: &mut Map<String, Value>,
    include_heating: bool,
    include_cooling: bool,
) {
    if include_heating {
        if let Some(ref wd) = building.heating_weekday_setpoints_c {
            params.insert("heating_weekday_setpoints_c".to_string(), json!(wd));
        }
        if let Some(ref we) = building.heating_weekend_setpoints_c {
            params.insert("heating_weekend_setpoints_c".to_string(), json!(we));
        }
    }
    if include_cooling {
        if let Some(ref wd) = building.cooling_weekday_setpoints_c {
            params.insert("cooling_weekday_setpoints_c".to_string(), json!(wd));
        }
        if let Some(ref we) = building.cooling_weekend_setpoints_c {
            params.insert("cooling_weekend_setpoints_c".to_string(), json!(we));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::building::{DuctSystem, DuctType, Site, XmlNode, Zone};
    use super::*;
    use std::collections::HashMap;

    fn empty_building(zones: Vec<Zone>) -> Building {
        Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
            },
            zones,
            boundaries: vec![],
            windows: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ela_cm2: None,
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
            details_xml: XmlNode {
                name: String::new(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        }
    }

    fn duct(duct_type: DuctType, area_m2: f64, r_val: f64) -> DuctSystem {
        DuctSystem {
            id: String::new(),
            leakage_fraction: Some(0.0),
            insulation_r_value_m2_k_w: Some(r_val),
            surface_area_m2: Some(area_m2),
            location: DuctLocation::OutsideConditionedSpace,
            duct_type,
        }
    }

    fn unconditioned_zone(ducts: Vec<DuctSystem>) -> Zone {
        Zone {
            zone_type: ZoneType::Attic,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: ducts,
            vented: true,
            ventilation_ach: None,
            ventilation_sla: None,
        }
    }

    /// Regression test for bug: duct R-value used `.max()` across ducts instead
    /// of area-weighted average.  With one R-8 duct (10 m²) and one R-4 duct
    /// (10 m²) the correct result is R-6, not R-8.
    #[test]
    fn duct_r_value_is_area_weighted_average_not_max() {
        let building = empty_building(vec![unconditioned_zone(vec![
            duct(DuctType::Supply, 10.0, 8.0),
            duct(DuctType::Supply, 10.0, 4.0),
        ])]);
        let params = compute_duct_dse_params(&building);
        let supply_r = params.supply_r_m2_k_w;
        assert!(
            (supply_r - 6.0).abs() < 1e-9,
            "expected area-weighted average R-6, got {supply_r}"
        );
    }

    /// Regression test: unequal-area ducts should weight the larger duct more.
    #[test]
    fn duct_r_value_area_weighted_unequal_areas() {
        // 20 m² at R-3, 10 m² at R-9 → weighted avg = (20*3 + 10*9) / 30 = 150/30 = 5.0
        let building = empty_building(vec![unconditioned_zone(vec![
            duct(DuctType::Return, 20.0, 3.0),
            duct(DuctType::Return, 10.0, 9.0),
        ])]);
        let params = compute_duct_dse_params(&building);
        let return_r = params.return_r_m2_k_w;
        assert!(
            (return_r - 5.0).abs() < 1e-9,
            "expected area-weighted average R-5, got {return_r}"
        );
    }

    #[test]
    fn c_d_heating_single_speed_low_hspf() {
        assert!((calc_startup_degradation(true, "ashp", 6.5, 1) - 0.20).abs() < f64::EPSILON);
    }

    #[test]
    fn c_d_heating_single_speed_high_hspf() {
        assert!((calc_startup_degradation(true, "ashp", 8.0, 1) - 0.11).abs() < f64::EPSILON);
    }

    #[test]
    fn c_d_heating_two_speed() {
        assert!((calc_startup_degradation(true, "ashp", 10.0, 2) - 0.11).abs() < f64::EPSILON);
    }

    #[test]
    fn c_d_heating_variable_speed() {
        assert!((calc_startup_degradation(true, "mini-split", 12.0, 4) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn c_d_cooling_room_ac() {
        assert!((calc_startup_degradation(false, "room ac", 10.0, 1) - 0.22).abs() < f64::EPSILON);
    }

    #[test]
    fn c_d_cooling_single_speed_low_seer() {
        assert!(
            (calc_startup_degradation(false, "central ac", 12.0, 1) - 0.20).abs() < f64::EPSILON
        );
    }

    #[test]
    fn c_d_cooling_single_speed_high_seer() {
        assert!(
            (calc_startup_degradation(false, "central ac", 14.0, 1) - 0.07).abs() < f64::EPSILON
        );
    }

    #[test]
    fn c_d_cooling_two_speed() {
        assert!(
            (calc_startup_degradation(false, "central ac", 18.0, 2) - 0.11).abs() < f64::EPSILON
        );
    }

    #[test]
    fn c_d_cooling_variable_speed() {
        assert!(
            (calc_startup_degradation(false, "central ac", 22.0, 4) - 0.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn select_primary_curve_pair_carries_bounds_and_ff_plf_limits() {
        let curve_set = crate::defaults::HvacCurveSet {
            variants: vec![crate::defaults::HvacCurveVariant {
                name: "Variable_1".to_string(),
                cap_t: hares_physics::biquadratic::BiquadraticCurve {
                    coeffs: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    x1_bounds: (12.0, 24.0),
                    x2_bounds: (18.0, 50.0),
                },
                cap_ff: [1.0, 0.0, 0.0],
                eir_t: hares_physics::biquadratic::BiquadraticCurve {
                    coeffs: [1.1, 0.0, 0.0, 0.0, 0.0, 0.0],
                    x1_bounds: (12.0, 24.0),
                    x2_bounds: (18.0, 50.0),
                },
                eir_ff: [1.0, 0.0, 0.0],
                eir_plr: [1.0, 0.0, 0.0],
                ff_bounds: Some((0.4, 1.0)),
                plf_bounds: Some((0.5, 1.0)),
            }],
        };
        let selected = select_primary_curve_pair(&curve_set, 4).expect("variable speed pair");
        assert_eq!(selected.x1_bounds, (12.0, 24.0));
        assert_eq!(selected.x2_bounds, (18.0, 50.0));
        assert_eq!(selected.ff_bounds, Some((0.4, 1.0)));
        assert_eq!(selected.plf_bounds, Some((0.5, 1.0)));
    }

    // -----------------------------------------------------------------------
    // zone_type_to_ashrae152_str — foundation type granularity
    // -----------------------------------------------------------------------

    fn foundation_zone(vented: bool) -> Zone {
        Zone {
            zone_type: ZoneType::Foundation,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented,
            ventilation_ach: None,
            ventilation_sla: None,
        }
    }

    fn attic_zone(vented: bool) -> Zone {
        Zone {
            zone_type: ZoneType::Attic,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented,
            ventilation_ach: None,
            ventilation_sla: None,
        }
    }

    /// Build a minimal `Boundary` with just the fields relevant to insulation detection.
    fn floor_boundary(r_si: Option<f64>) -> Boundary {
        Boundary {
            id: "floor1".into(),
            boundary_type: BoundaryType::Floor,
            area_m2: 50.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: r_si,
            r_value_layers_m2_k_w: vec![],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Foundation),
            material_layers: vec![],
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
        }
    }

    fn foundation_wall_boundary(insulation_details: Option<&str>) -> Boundary {
        Boundary {
            id: "fndwall1".into(),
            boundary_type: BoundaryType::FoundationWall,
            area_m2: 30.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: vec![],
            interior_zone: Some(ZoneType::Foundation),
            exterior_zone: Some(ZoneType::Ground),
            material_layers: vec![],
            construction_type: None,
            finish_type: None,
            insulation_details: insulation_details.map(String::from),
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
        }
    }

    fn building_with(foundation_name: Option<&str>, boundaries: Vec<Boundary>) -> Building {
        let mut b = empty_building(vec![]);
        b.foundation_name = foundation_name.map(String::from);
        b.boundaries = boundaries;
        b
    }

    // Above OCHRE's 5.3 IP R threshold (0.934 m²·K/W): use 1.5 m²·K/W.
    const INSULATED_FLOOR_R: f64 = 1.5;
    // Below threshold: 0.3 m²·K/W.
    const UNINSULATED_FLOOR_R: f64 = 0.3;

    #[test]
    fn attic_vented_maps_correctly() {
        let zone = attic_zone(true);
        let building = building_with(None, vec![]);
        assert_eq!(zone_type_to_ashrae152_str(&zone, &building), "attic_vented");
    }

    #[test]
    fn attic_unvented_maps_correctly() {
        let zone = attic_zone(false);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "attic_unvented"
        );
    }

    #[test]
    fn vented_crawlspace_uninsulated() {
        let zone = foundation_zone(true);
        let building = building_with(
            Some("Crawlspace"),
            vec![foundation_wall_boundary(Some("Uninsulated"))],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "vent_unins_crawlspace"
        );
    }

    #[test]
    fn vented_crawlspace_insulated_floor_only() {
        let zone = foundation_zone(true);
        let building = building_with(
            Some("Crawlspace"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("Uninsulated")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "vent_crawlspace_ins_floor"
        );
    }

    #[test]
    fn vented_crawlspace_insulated_floor_and_wall() {
        let zone = foundation_zone(true);
        let building = building_with(
            Some("Crawlspace"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("R-10")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "vent_crawlspace_ins_floor_wall"
        );
    }

    #[test]
    fn unvented_crawlspace_uninsulated() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Crawlspace"),
            vec![foundation_wall_boundary(Some("Uninsulated"))],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "unvent_unins_crawlspace"
        );
    }

    #[test]
    fn unvented_crawlspace_insulated_floor_only() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Crawlspace"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("Uninsulated")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "unvent_crawlspace_ins_floor"
        );
    }

    #[test]
    fn unvented_crawlspace_insulated_floor_and_wall() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Crawlspace"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("R-10")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "unvent_crawlspace_ins_floor_wall"
        );
    }

    #[test]
    fn basement_uninsulated() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Unfinished Basement"),
            vec![foundation_wall_boundary(Some("Uninsulated"))],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "unins_basement"
        );
    }

    #[test]
    fn basement_insulated_walls() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Unfinished Basement"),
            vec![foundation_wall_boundary(Some("R-15"))],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "basement_ins_walls"
        );
    }

    #[test]
    fn basement_insulated_ceiling() {
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Unfinished Basement"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("Uninsulated")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "basement_ins_ceiling"
        );
    }

    #[test]
    fn basement_walls_take_priority_over_ceiling() {
        // OCHRE checks wall insulation first (lines 71-76 of equipment.py).
        let zone = foundation_zone(false);
        let building = building_with(
            Some("Unfinished Basement"),
            vec![
                floor_boundary(Some(INSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("R-10")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "basement_ins_walls"
        );
    }

    #[test]
    fn floor_below_threshold_is_not_insulated() {
        let zone = foundation_zone(true);
        let building = building_with(
            Some("Crawlspace"),
            vec![
                floor_boundary(Some(UNINSULATED_FLOOR_R)),
                foundation_wall_boundary(Some("Uninsulated")),
            ],
        );
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "vent_unins_crawlspace"
        );
    }

    #[test]
    fn foundation_no_name_falls_back_to_vented_uninsulated() {
        let zone = foundation_zone(true);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "vent_unins_crawlspace"
        );
    }

    #[test]
    fn foundation_no_name_unvented_falls_back() {
        let zone = foundation_zone(false);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building),
            "unvent_unins_crawlspace"
        );
    }

    // -----------------------------------------------------------------------
    // compute_basement_params
    // -----------------------------------------------------------------------

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

    #[test]
    fn basement_params_empty_when_no_finished_basement() {
        let mut b = empty_building(vec![conditioned_zone(), foundation_zone(false)]);
        b.foundation_name = Some("Unfinished Basement".into());
        let params = compute_basement_params(&b);
        assert!(
            params.is_empty(),
            "unfinished basement must not produce basement params"
        );
    }

    #[test]
    fn basement_params_empty_when_no_foundation_zone() {
        let mut b = empty_building(vec![conditioned_zone()]);
        b.foundation_name = Some("Finished Basement".into());
        let params = compute_basement_params(&b);
        assert!(
            params.is_empty(),
            "finished basement name without Foundation zone must not produce params"
        );
    }

    // -----------------------------------------------------------------------
    // Builder function integration tests
    // -----------------------------------------------------------------------

    fn minimal_furnace_params(afue: f64, capacity_w: f64) -> Map<String, Value> {
        let mut p = Map::new();
        p.insert("efficiency_afue".to_string(), json!(afue));
        p.insert("heating_capacity_w".to_string(), json!(capacity_w));
        p
    }

    fn minimal_central_ac_params(seer: f64, capacity_w: f64) -> Map<String, Value> {
        let mut p = Map::new();
        p.insert("efficiency_seer".to_string(), json!(seer));
        p.insert("cooling_capacity_w".to_string(), json!(capacity_w));
        p
    }

    #[test]
    fn gas_furnace_builder_produces_typed_config_with_correct_afue() {
        let params = minimal_furnace_params(0.96, 10_000.0);
        let duct_params = DuctDseParams::default();
        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &duct_params)
            .expect("gas furnace builder must succeed with valid params");
        assert!(ec.is_typed(), "EquipmentConfig must be typed");
        use hares_equipment::hvac::heating_config::GasFurnaceConfig;
        let cfg: GasFurnaceConfig = ec.typed().expect("must deserialize to GasFurnaceConfig");
        assert!(
            (cfg.afue - 0.96).abs() < 1e-12,
            "afue must be 0.96, got {}",
            cfg.afue
        );
        assert!((cfg.capacity_w - 10_000.0).abs() < 1e-9);
    }

    #[test]
    fn central_ac_builder_produces_typed_config_with_correct_eir() {
        let params = minimal_central_ac_params(16.0, 12_000.0);
        let duct_params = DuctDseParams::default();
        let ec = try_build_central_ac_config("Air Conditioner", &params, &duct_params)
            .expect("central AC builder must succeed with valid params");
        assert!(ec.is_typed(), "EquipmentConfig must be typed");
        assert_eq!(
            ec.ochre_class, "Air Conditioner",
            "ochre_class must match registry key"
        );
        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        let cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("must deserialize to CentralAirConditionerConfig");
        let expected_eir = 3.412_141_633_f64 / 16.0_f64;
        assert!(
            (cfg.eir - expected_eir).abs() < 1e-12,
            "eir must equal 3.412141633/16={expected_eir:.9}, got {}",
            cfg.eir
        );
        assert!((cfg.capacity_w - 12_000.0).abs() < 1e-9);
    }

    #[test]
    fn central_ac_builder_propagates_curve_bounds() {
        let mut params = minimal_central_ac_params(16.0, 12_000.0);
        params.insert("biquadratic_x1_min".to_string(), json!(12.0));
        params.insert("biquadratic_x1_max".to_string(), json!(24.0));
        params.insert("biquadratic_x2_min".to_string(), json!(18.0));
        params.insert("biquadratic_x2_max".to_string(), json!(50.0));
        params.insert("ff_min".to_string(), json!(0.5));
        params.insert("ff_max".to_string(), json!(1.0));
        params.insert("plf_min".to_string(), json!(0.6));
        params.insert("plf_max".to_string(), json!(1.0));
        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("central AC builder must succeed");
        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        let cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("must deserialize to CentralAirConditionerConfig");
        assert_eq!(cfg.biquadratic_x1_min, Some(12.0));
        assert_eq!(cfg.biquadratic_x1_max, Some(24.0));
        assert_eq!(cfg.biquadratic_x2_min, Some(18.0));
        assert_eq!(cfg.biquadratic_x2_max, Some(50.0));
        assert_eq!(cfg.ff_min, Some(0.5));
        assert_eq!(cfg.ff_max, Some(1.0));
        assert_eq!(cfg.plf_min, Some(0.6));
        assert_eq!(cfg.plf_max, Some(1.0));
    }

    #[test]
    fn room_ac_builder_returns_none_when_only_seer_available() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(3_500.0));
        params.insert("efficiency_seer".to_string(), json!(12.0));
        // No EER present — must return None rather than silently substituting SEER.
        let result = try_build_room_ac_config("Room AC", &params);
        assert!(
            result.is_none(),
            "room AC builder must not accept SEER as EER substitute"
        );
    }

    #[test]
    fn room_ac_builder_succeeds_with_eer() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(3_500.0));
        params.insert("efficiency_eer".to_string(), json!(10.5));
        let ec = try_build_room_ac_config("Room AC", &params)
            .expect("room AC builder must succeed when EER is present");
        assert!(ec.is_typed());
        use hares_equipment::hvac::cooling_config::RoomAcConfig;
        let cfg: RoomAcConfig = ec.typed().expect("must deserialize to RoomAcConfig");
        let expected_eir = 3.412_141_633_f64 / 10.5_f64;
        assert!((cfg.eir - expected_eir).abs() < 1e-12);
    }

    #[test]
    fn heat_pump_cooler_builder_forces_four_speeds_for_minisplit() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_seer".to_string(), json!(18.0));
        params.insert("number_of_speeds".to_string(), json!(1));
        let ec = try_build_heat_pump_cooler_config(
            "MSHP Cooler",
            &params,
            &DuctDseParams::default(),
            true,
        )
        .expect("MSHP cooler typed config should be built");

        use hares_equipment::hvac::heat_pump_config::HeatPumpCoolerConfig;
        let cfg: HeatPumpCoolerConfig = ec.typed().expect("typed cooler config");
        assert!(cfg.is_mini_split);
        assert_eq!(cfg.number_of_speeds, 4);
    }

    #[test]
    fn heat_pump_cooler_builder_propagates_stage_shrs() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_seer".to_string(), json!(18.0));
        params.insert("number_of_speeds".to_string(), json!(2));
        params.insert("shr_0".to_string(), json!(0.81));
        params.insert("shr_1".to_string(), json!(0.74));
        let ec = try_build_heat_pump_cooler_config(
            "ASHP Cooler",
            &params,
            &DuctDseParams::default(),
            false,
        )
        .expect("ASHP cooler typed config should be built");

        use hares_equipment::hvac::heat_pump_config::HeatPumpCoolerConfig;
        let cfg: HeatPumpCoolerConfig = ec.typed().expect("typed cooler config");
        assert_eq!(cfg.number_of_speeds, 2);
        assert_eq!(cfg.stage_shrs, Some(vec![0.81, 0.74]));
    }

    #[test]
    fn ideal_hvac_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(9_000.0));
        params.insert("cooling_capacity_w".to_string(), json!(8_000.0));
        params.insert("heating_efficiency".to_string(), json!(0.95));
        params.insert("efficiency_seer".to_string(), json!(16.0));
        params.insert("fraction_load_served".to_string(), json!(0.9));
        let ec = try_build_ideal_hvac_config("Ideal HVAC", &params)
            .expect("Ideal HVAC typed config should be built");

        use hares_equipment::hvac::heating_config::IdealHvacConfig;
        let cfg: IdealHvacConfig = ec.typed().expect("typed ideal config");
        assert_eq!(cfg.heating_capacity_w, Some(9_000.0));
        assert_eq!(cfg.cooling_capacity_w, Some(8_000.0));
        assert_eq!(cfg.fraction_heating_load_served, Some(0.9));
        assert_eq!(cfg.fraction_cooling_load_served, Some(0.9));
    }

    #[test]
    fn ideal_hvac_builder_parses_daily_profile_setpoint_source() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(9_000.0));
        params.insert(
            "heating_weekday_setpoints_c".to_string(),
            json!(vec![20.0; 24]),
        );
        params.insert(
            "heating_weekend_setpoints_c".to_string(),
            json!(vec![19.0; 24]),
        );
        let ec = try_build_ideal_hvac_config("Ideal HVAC", &params)
            .expect("Ideal HVAC typed config should be built");

        use hares_equipment::hvac::heating_config::IdealHvacConfig;
        let cfg: IdealHvacConfig = ec.typed().expect("typed ideal config");
        match cfg.heating_setpoint_source {
            Some(ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            }) => {
                assert_eq!(weekday[0], 20.0);
                assert_eq!(weekend[0], 19.0);
            }
            other => panic!("expected DailyProfile setpoint source, got {other:?}"),
        }
        assert_eq!(cfg.heating_setpoint_c, Some(20.0));
    }

    #[test]
    fn central_ac_builder_parses_column_ref_setpoint_source() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_seer".to_string(), json!(16.0));
        params.insert("heating_setpoint_schedule_col".to_string(), json!(3));
        params.insert("cooling_setpoint_schedule_col".to_string(), json!(4));
        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("AC typed config should be built");

        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        let cfg: CentralAirConditionerConfig = ec.typed().expect("typed AC config");
        assert_eq!(
            cfg.heating_setpoint_source,
            Some(ScheduleSourceConfig::ColumnRef {
                col_idx: 3,
                boundary: BoundaryPolicy::Clamp
            })
        );
        assert_eq!(
            cfg.cooling_setpoint_source,
            Some(ScheduleSourceConfig::ColumnRef {
                col_idx: 4,
                boundary: BoundaryPolicy::Clamp
            })
        );
    }

    #[test]
    fn dehumidifier_builder_produces_typed_config() {
        let mut params = Map::new();
        params.insert("capacity_liters_per_day".to_string(), json!(25.0));
        params.insert("integrated_energy_factor".to_string(), json!(2.0));
        params.insert("fraction_served".to_string(), json!(0.8));
        params.insert("target_rh".to_string(), json!(0.5));
        let ec = try_build_dehumidifier_config("Dehumidifier", &params)
            .expect("dehumidifier typed config should be built");

        use hares_equipment::hvac::cooling_config::DehumidifierConfig;
        let cfg: DehumidifierConfig = ec.typed().expect("typed dehumidifier config");
        assert_eq!(cfg.capacity_liters_per_day, Some(25.0));
        assert_eq!(cfg.integrated_energy_factor, Some(2.0));
        assert_eq!(cfg.fraction_served, Some(0.8));
        assert_eq!(cfg.target_rh, Some(0.5));
    }

    #[test]
    fn basement_params_returns_zone_id_and_ratio_for_finished_basement() {
        // Zones sorted: Conditioned(idx=0) → ZoneId(1), Foundation(idx=1) → ZoneId(2).
        let mut b = empty_building(vec![conditioned_zone(), foundation_zone(false)]);
        b.foundation_name = Some("Finished Basement".into());
        let params = compute_basement_params(&b);
        assert_eq!(
            params.get("basement_zone_id").and_then(|v| v.as_f64()),
            Some(2.0),
            "Foundation zone at index 1 → ZoneId 2"
        );
        let ratio = params
            .get("basement_airflow_ratio")
            .and_then(|v| v.as_f64())
            .expect("basement_airflow_ratio must be present");
        assert!(
            (ratio - 0.2).abs() < 1e-12,
            "OCHRE default basement airflow ratio is 0.2, got {ratio}"
        );
    }

    // -----------------------------------------------------------------------
    // End-to-end AFUE / SEER propagation tests
    // -----------------------------------------------------------------------

    fn make_env() -> hares_types::EnvironmentState {
        use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
        use hares_types::{GridState, WeatherState, ZoneId, ZoneState};
        hares_types::EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 18.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 5.0,
                outdoor_humidity_ratio: 0.003,
                pressure_kpa: 101.325,
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn gas_furnace_afue_propagates_end_to_end() {
        use hares_equipment::Equipment;
        use hares_equipment::GasFurnaceConfig;
        use hares_equipment::hvac::furnace::GasFurnace;
        use hares_types::{
            ControlSignal, PortSlots, ThermalAccumulator, ZoneId, telemetry_keys as tk,
        };

        let mut params = Map::new();
        params.insert("efficiency_afue".to_string(), json!(0.96));
        params.insert("heating_capacity_w".to_string(), json!(12_000.0));
        // Zero fan power so the fuel/thermal ratio equals exactly 1/AFUE.
        params.insert("fan_power_w".to_string(), json!(0.0));

        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &DuctDseParams::default())
            .expect("try_build_gas_furnace_config must succeed with AFUE and capacity");

        let typed_cfg: GasFurnaceConfig = ec
            .typed()
            .expect("config must deserialize as GasFurnaceConfig");
        assert!(
            (typed_cfg.afue - 0.96).abs() < 1e-12,
            "AFUE must round-trip: expected 0.96, got {}",
            typed_cfg.afue
        );

        // Verify the equipment's step produces the correct fuel/thermal ratio.
        let env = make_env();
        let mut eq = GasFurnace::new(ec.clone());
        eq.init(&ec, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let _ = eq.apply_control_unchecked(&ControlSignal::IdealCapacity {
            capacity_w: f64::MAX,
        });
        eq.update_control(&env);
        eq.step(&env, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        let thermal_w = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);
        let fuel_w = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(0.0);
        assert!(thermal_w > 0.0, "thermal output must be positive");
        assert!(fuel_w > 0.0, "fuel input must be positive");
        let ratio = fuel_w / thermal_w;
        assert!(
            (ratio - 1.0 / 0.96).abs() < 0.01,
            "fuel/thermal ratio must equal 1/AFUE=1/0.96≈{:.4}, got {ratio:.4}",
            1.0 / 0.96
        );
    }

    #[test]
    fn central_ac_seer16_eir_propagates_end_to_end() {
        use hares_equipment::Equipment;
        use hares_equipment::hvac::air_conditioner::AirConditioner;
        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        use hares_types::{
            ControlSignal, PortSlots, ThermalAccumulator, ZoneId, telemetry_keys as tk,
        };

        let mut params = Map::new();
        params.insert("efficiency_seer".to_string(), json!(16.0));
        params.insert("cooling_capacity_w".to_string(), json!(10_000.0));

        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("try_build_central_ac_config must succeed with SEER and capacity");

        let typed_cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("config must deserialize as CentralAirConditionerConfig");
        let expected_eir = 3.412_141_633_f64 / 16.0_f64;
        assert!(
            (typed_cfg.eir - expected_eir).abs() < 1e-12,
            "EIR must equal 3.412141633/16={expected_eir:.9}, got {}",
            typed_cfg.eir
        );

        // Build and init the equipment object.
        let mut eq = AirConditioner::new(ec.clone());
        // AHRI 210/240 A-test rated conditions: indoor DB=26.67°C WB=19.44°C,
        // outdoor DB=35°C. At these conditions the EIR biquadratic ratio ≈ 1.0.
        use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
        use hares_types::{GridState, WeatherState, ZoneState};
        let env = hares_types::EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 26.67,
                humidity_ratio: 0.011_16,
                relative_humidity: 0.50,
                wet_bulb_c: 19.44,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.010,
                pressure_kpa: 101.325,
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 7, 1, 14, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        };
        eq.init(&ec, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let _ = eq.apply_control_unchecked(&ControlSignal::IdealCapacity {
            capacity_w: f64::MAX,
        });
        eq.update_control(&env);
        eq.step(&env, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let sensible_w = eq.telemetry().get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0);
        let latent_w = eq.telemetry().get(tk::LATENT_COOLING_W).unwrap_or(0.0);
        let total_cooling_w = sensible_w + latent_w;
        // COP = total_cooling / compressor_only (AHRI convention: excludes fan).
        // 1/COP = EIR_effective ≈ EIR_nominal * biquadratic_correction.
        let cop = eq.telemetry().get(tk::COP).unwrap_or(0.0);
        assert!(electric_kw > 0.0, "electric_kw must be positive");
        assert!(
            total_cooling_w > 0.0,
            "total cooling (sensible+latent) must be positive"
        );
        assert!(cop > 0.0, "COP must be positive when cooling");

        // EIR_nominal = 3.412141633 / SEER. The AHRI COP telemetry excludes fan power so
        // 1/COP ≈ EIR. At AHRI rated conditions (WB=19.44, ODB=35) the biquadratic EIR
        // correction evaluates to ≈ 1.02, so 1/COP ≈ EIR * 1.02 ≈ 0.217. A 10% tolerance
        // confirms the SEER value round-tripped through the config and was applied by init.
        let expected_eir = 3.412_141_633_f64 / 16.0;
        let actual_eir = 1.0 / cop;
        assert!(
            (actual_eir - expected_eir).abs() / expected_eir < 0.10,
            "1/COP at AHRI conditions must be within 10% of nominal EIR=3.412/SEER={expected_eir:.4}, got {actual_eir:.4}"
        );
    }
}
