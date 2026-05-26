//! HVAC equipment resolution from HPXML into canonical equipment specs.
// Invariant: HVAC typed configs are built from typed/defaults-derived fields
// (including curve metadata), not from ad-hoc string-key translation logic.

use serde_json::{Map, Value, json};

use hares_equipment::hvac::cooling_config::{
    CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig,
};
use hares_equipment::hvac::heat_pump::defrost::{DefrostConfig, DefrostControl, DefrostStrategy};
use hares_equipment::hvac::heat_pump_config::{
    HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
use hares_equipment::hvac::heating_config::{
    DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig,
    GasBoilerConfig, GasFurnaceConfig, IdealHvacConfig,
};
use hares_equipment::{EquipmentConfig, SetpointReconciliation};
use hares_types::{FuelType, ScheduleSourceConfig};

use super::HpxmlError;
use super::building::{Boundary, BoundaryType, Building, DuctLocation, XmlNode, Zone, ZoneType};
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{child_f64, child_text, descendants_named, element_id};
use hares_physics::constants::{
    BOILER_AUXILIARY_HOURS_PER_YEAR, BTU_PER_HR_PER_W, CFM_TO_M3_S, HOURS_PER_YEAR, KW_TO_W,
    W_PER_TON,
};
use hares_physics::units as conv;

use crate::defaults::DefaultsStore;

/// SEER2→SEER factor for ducted split/package systems.
/// RESNET MINHERS Addendum 71f / ANSI/RESNET 301-2022:
/// ducted SEER2/SEER ratio ≈ 0.95 (≈5% reduction).
/// Ductless (mini-split): factor = 1.0 (SEER2 = SEER, no conversion needed).
const SEER2_TO_SEER_FACTOR: f64 = 1.0 / 0.95;
/// HSPF2→HSPF factor for ducted split/package systems.
/// DOE 87 FR 74364 (Dec 2022) / AHRI 210/240-2023 / RESNET MINHERS Addendum 71f:
/// ducted split-system heat pump HSPF2/HSPF ratio ≈ 0.85 (≈15% reduction).
/// Distinct from SEER2→SEER (0.95). For ductless (mini-split) use
/// HSPF2_TO_HSPF_FACTOR_DUCTLESS (1/0.90 ≈ 10% reduction).
const HSPF2_TO_HSPF_FACTOR: f64 = 1.0 / 0.85;
/// HSPF2→HSPF factor for ductless (mini-split) heat pumps.
/// RESNET MINHERS Addendum 71f / ANSI/RESNET 301-2022:
/// ductless/mini-split HSPF2/HSPF ratio ≈ 0.90 (≈10% reduction).
/// The test-procedure change for ductless units is milder than for ducted
/// units because ductless units have no external static pressure duct penalty.
const HSPF2_TO_HSPF_FACTOR_DUCTLESS: f64 = 1.0 / 0.90;
/// EER2→EER factor for central air conditioners and heat pumps.
/// DOE 10 CFR Part 430 Appendix M1 (2023 revision) / AHRI 210/240-2023:
/// EER2/EER ratio ≈ 0.96 (≈4% reduction). Per CEC conversion table:
/// split-system AC: EER = EER2 × 1.043 (≈ 1/0.9588); packaged AC: EER = EER2 × 1.038.
/// Using 1/0.96 as the central residential estimate — the exact factor varies by
/// equipment type (split vs. packaged) and capacity bin, which HPXML cannot distinguish.
/// Note: room ACs rated under 10 CFR Part 430 Appendix F use CEER, not EER2. An EER2
/// value for a room AC in HPXML is likely a central-AC metric misapplied; the conversion
/// is still applied rather than silently discarding the value.
const EER2_TO_EER_FACTOR: f64 = 1.0 / 0.96;

fn airflow_defect_multiplier(params: &Map<String, Value>) -> f64 {
    params
        .get("airflow_defect_ratio")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .map(|v| 1.0 + v)
        .unwrap_or(1.0)
}

fn airflow_m3_s_per_w_from_explicit_cfm(
    params: &Map<String, Value>,
    key: &str,
    capacity_w: f64,
) -> Option<f64> {
    if !capacity_w.is_finite() || capacity_w <= 0.0 {
        return None;
    }
    let cfm = params.get(key).and_then(Value::as_f64)?;
    if !cfm.is_finite() || cfm <= 0.0 {
        return None;
    }
    Some(cfm * CFM_TO_M3_S / capacity_w)
}

#[derive(Debug, Clone, Default)]
pub struct DuctDseParams {
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
/// Returns `Ok(DuctDseParams::default())` when there are no ducts outside
/// conditioned space. Returns `Err(HpxmlError::MissingField)` when ducts do
/// exist but the building lacks any of the three inputs that drive ASHRAE
/// 152 DSE: conditioned volume, site latitude, site longitude.
pub fn compute_duct_dse_params(
    building: &Building,
) -> std::result::Result<DuctDseParams, HpxmlError> {
    use super::building::DuctType;

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
                duct_zone_type_str = Some(zone_type_to_ashrae152_str(zone, building)?);
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
        return Ok(DuctDseParams::default());
    }

    let house_volume_m3 = building.conditioned_volume_m3.ok_or_else(|| {
        HpxmlError::MissingField {
            path: "BuildingSummary/BuildingConstruction/ConditionedBuildingVolume",
            system_kind: "Building",
            system_id: "conditioned".to_string(),
            reason:
                "conditioned volume (m³) is required for ASHRAE 152 duct DSE when ducts are outside conditioned space; no silent default permitted",
        }
    })?;
    let latitude_deg = building.site.latitude_deg.ok_or_else(|| {
        HpxmlError::MissingField {
            path: "Site/Latitude",
            system_kind: "Building",
            system_id: "site".to_string(),
            reason:
                "site latitude (°) is required for ASHRAE 152 duct DSE; no silent default permitted",
        }
    })?;
    let longitude_deg = building.site.longitude_deg.ok_or_else(|| {
        HpxmlError::MissingField {
            path: "Site/Longitude",
            system_kind: "Building",
            system_id: "site".to_string(),
            reason:
                "site longitude (°) is required for ASHRAE 152 duct DSE; no silent default permitted",
        }
    })?;

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
    Ok(DuctDseParams {
        zone_id: Some(zone_id),
        zone_type: duct_zone_type_str,
        house_volume_m3,
        supply_leakage_frac: supply_leakage,
        supply_area_m2,
        supply_r_m2_k_w,
        return_leakage_frac: return_leakage,
        return_area_m2,
        return_r_m2_k_w,
        latitude_deg,
        longitude_deg,
    })
}

/// Rebuild an HVAC equipment typed config using updated parameters.
///
/// Called after autosizing to replace placeholder capacities with computed
/// values. Matches on the canonical equipment name to select the correct
/// config builder.
pub fn rebuild_hvac_typed_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    match name {
        "Gas Furnace" => try_build_gas_furnace_config(name, params, duct_params)
            .ok()
            .flatten(),
        "Electric Furnace" => try_build_electric_furnace_config(name, params, duct_params)
            .ok()
            .flatten(),
        "Gas Boiler" => try_build_gas_boiler_config(name, params).ok().flatten(),
        "Electric Boiler" => try_build_electric_boiler_config(name, params)
            .ok()
            .flatten(),
        "Electric Baseboard" => try_build_electric_baseboard_config(name, params)
            .ok()
            .flatten(),
        "Ideal HVAC" => try_build_ideal_hvac_config(name, params),
        "Air Conditioner" => try_build_central_ac_config(name, params, duct_params),
        "Room AC" => try_build_room_ac_config(name, params),
        "ASHP Heater" | "MSHP Heater" => {
            let is_mini_split = name.starts_with("MSHP");
            try_build_heat_pump_heater_config(name, params, duct_params, is_mini_split)
        }
        "ASHP Cooler" | "MSHP Cooler" => {
            let is_mini_split = name.starts_with("MSHP");
            try_build_heat_pump_cooler_config(name, params, duct_params, is_mini_split)
        }
        "Dehumidifier" => try_build_dehumidifier_config(name, params),
        other => {
            tracing::warn!(
                canonical_name = other,
                "rebuild_hvac_typed_config called with unrecognized equipment name; \
                 returning None"
            );
            None
        }
    }
}

/// Compute a `DuctConfig` from the duct parameter bundle produced by
/// `compute_duct_dse_params`.
///
/// DSE is pre-computed here using ASHRAE 152 so that typed-config equipment
/// init paths (which only see `DuctConfig.dse_heat/dse_cool`) apply duct
/// losses correctly. Uses explicit HPXML airflow when present, otherwise a
/// nominal airflow based on equipment type conventions.
///
/// Returns `DuctConfig { dse_heat: None, dse_cool: None }` when no duct zone
/// type was recorded (i.e., ducts are in conditioned space or absent).
fn compute_duct_config(
    duct_params: &DuctDseParams,
    capacity_w: f64,
    is_heating: bool,
    n_speeds: u8,
    is_heat_pump: bool,
    explicit_airflow_m3_s_per_w: Option<f64>,
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
        other => {
            tracing::warn!(
                duct_zone_type = other,
                "Unrecognized duct zone type; skipping DSE calculation"
            );
            return DuctConfig::default();
        }
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
    let fan_flow_m3_s = explicit_airflow_m3_s_per_w
        .map(|airflow| capacity_w * airflow)
        .unwrap_or_else(|| capacity_w * (cfm_per_ton * CFM_TO_M3_S / W_PER_TON));

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
///
/// Electric resistance heating is by definition 100% efficient at the appliance
/// (all electrical input becomes heat). This conventional value is documented in
/// ASHRAE Handbook — HVAC Systems and Equipment 2020, Ch. 33 "Furnaces".
/// HARES requires the input file to state the efficiency explicitly rather than
/// silently defaulting — a misspelled key or absent field must fail loudly.
fn resistance_efficiency_from_params(
    params: &Map<String, Value>,
    name: &str,
    system_kind: &'static str,
) -> std::result::Result<f64, HpxmlError> {
    let value = params
        .get("heating_efficiency")
        .and_then(Value::as_f64)
        .or_else(|| params.get("efficiency_cop").and_then(Value::as_f64))
        .ok_or_else(|| HpxmlError::MissingField {
            path: "HeatingSystem/AnnualHeatingEfficiency",
            system_kind,
            system_id: name.to_string(),
            reason: "Electric resistance efficiency must be specified explicitly; \
                     value 1.0 (100% appliance efficiency) per ASHRAE HVAC Systems \
                     & Equipment 2020 Ch. 33 is the conventional choice",
        })?;
    if value <= 0.0 || value > 1.0 {
        return Err(HpxmlError::InvalidField {
            path: "HeatingSystem/AnnualHeatingEfficiency",
            system_kind,
            system_id: name.to_string(),
            value_received: format!("{value}"),
            reason: "Electric resistance efficiency must be in range (0.0, 1.0]; \
                     value 1.0 (100%) is the conventional choice",
        });
    }
    Ok(value)
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
            if units.eq_ignore_ascii_case("EER") {
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

fn schedule_source_from_params(
    params: &Map<String, Value>,
    prefix: &str,
) -> Option<ScheduleSourceConfig> {
    let source_key = format!("{prefix}_setpoint_source");
    if let Some(source) = params.get(&source_key) {
        return serde_json::from_value::<ScheduleSourceConfig>(source.clone()).ok();
    }
    None
}

fn setpoint_source_value(source: ScheduleSourceConfig) -> Value {
    serde_json::to_value(source).expect("ScheduleSourceConfig serialization must succeed")
}

fn daily_profile_source(weekday: [f64; 24], weekend: [f64; 24]) -> Value {
    setpoint_source_value(ScheduleSourceConfig::DailyProfile {
        weekday,
        weekend,
        month_multipliers: [1.0; 12],
        max_value: 1.0,
    })
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

/// Extract fan power from params. Preference order within this function:
/// 1. `fan_power_w` (from HPXML `<extension>/<FanPowerWatts>`)
/// 2. `auxiliary_power_w` (from `ElectricAuxiliaryEnergy` divided by an
///    equipment-specific hours divisor — an average-watts approximation;
///    see conversion site for details)
///
/// Note: `FanPowerWattsPerCFM` (highest priority overall) is handled separately
/// as a `fan_power_w_per_cfm` struct field in each equipment builder, because it
/// requires airflow data to convert to watts and cannot be resolved here.
fn fan_power_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("fan_power_w")
        .and_then(Value::as_f64)
        .or_else(|| params.get("auxiliary_power_w").and_then(Value::as_f64))
}

/// Extract `SetpointReconciliation` records from the resolved params map,
/// if any setpoint hours were widened during HPXML parsing.
fn extract_setpoints_reconciled(
    params: &Map<String, Value>,
) -> Option<Vec<SetpointReconciliation>> {
    params
        .get("setpoints_reconciled")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
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
///
/// Returns `Err(HpxmlError::MissingField)` when `heating_capacity_w` is absent
/// (caller catches this when `autosize_heating` is set and assigns `None`
/// typed_config until autosizing computes the real capacity).
/// Returns `Err(HpxmlError::MissingField)` when AFUE is absent — efficiency
/// must be specified explicitly and never silently defaulted.
fn try_build_gas_furnace_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> std::result::Result<Option<EquipmentConfig>, HpxmlError> {
    let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return Err(HpxmlError::MissingField {
            path: "HeatingSystem/HeatingCapacity",
            system_kind: "Gas Furnace",
            system_id: name.to_string(),
            reason: "HeatingCapacity is required; provide it in HPXML or let the dwelling builder autosize it",
        });
    };
    let afue = afue_from_params(params).ok_or_else(|| HpxmlError::MissingField {
        path: "HeatingSystem/AnnualHeatingEfficiency[AFUE]",
        system_kind: "Gas Furnace",
        system_id: name.to_string(),
        reason: "AFUE is required to model combustion efficiency; no silent default permitted",
    })?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "heating_airflow_cfm", capacity_w)
            .unwrap_or_else(|| {
                350.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
            });
    let mut ducts = compute_duct_config(
        duct_params,
        capacity_w,
        true,
        n_speeds,
        false,
        Some(airflow_m3_s_per_w),
    );
    ducts.airflow_m3_s_per_w = Some(airflow_m3_s_per_w);

    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let setpoints = extract_setpoints_reconciled(params);
    let cfg = GasFurnaceConfig {
        equipment_id: None,
        zone_id: None,
        afue,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        ducts,
        stage_heating_capacities_w: extract_stage_values(params, "heating_capacity_w_stage"),
        stage_heating_eirs: extract_stage_values(params, "heating_eir_stage"),
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        heating_setpoint_source,
    };
    Ok(Some(
        EquipmentConfig::from_typed(name.to_string(), "Gas Furnace".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    ))
}

/// Build an `ElectricFurnaceConfig` typed config.
fn try_build_electric_furnace_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> std::result::Result<Option<EquipmentConfig>, HpxmlError> {
    let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return Ok(None);
    };
    let heating_efficiency = resistance_efficiency_from_params(params, name, "Electric Furnace")?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "heating_airflow_cfm", capacity_w)
            .unwrap_or_else(|| {
                350.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
            });
    let mut ducts = compute_duct_config(
        duct_params,
        capacity_w,
        true,
        n_speeds,
        false,
        Some(airflow_m3_s_per_w),
    );
    ducts.airflow_m3_s_per_w = Some(airflow_m3_s_per_w);

    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let setpoints = extract_setpoints_reconciled(params);
    let cfg = ElectricFurnaceConfig {
        equipment_id: None,
        zone_id: None,
        eir: heating_efficiency,
        capacity_w,
        number_of_speeds: n_speeds,
        fan_power_w,
        ducts,
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        heating_setpoint_source,
    };
    Ok(Some(
        EquipmentConfig::from_typed(name.to_string(), "Electric Furnace".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    ))
}

/// Build a `GasBoilerConfig` typed config.
///
/// Returns `Err(HpxmlError::MissingField)` when `heating_capacity_w` is absent
/// or when the boiler has a capacity but no AFUE.
fn try_build_gas_boiler_config(
    name: &str,
    params: &Map<String, Value>,
) -> std::result::Result<Option<EquipmentConfig>, HpxmlError> {
    let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return Err(HpxmlError::MissingField {
            path: "HeatingSystem/HeatingCapacity",
            system_kind: "Gas Boiler",
            system_id: name.to_string(),
            reason: "HeatingCapacity is required; provide it in HPXML or let the dwelling builder autosize it",
        });
    };
    let afue = afue_from_params(params).ok_or_else(|| HpxmlError::MissingField {
        path: "HeatingSystem/AnnualHeatingEfficiency[AFUE]",
        system_kind: "Gas Boiler",
        system_id: name.to_string(),
        reason: "AFUE is required to model combustion efficiency; no silent default permitted",
    })?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);

    // Hydronic loop flow rate and return-temperature are configuration defaults
    // applied by the boiler equipment model when not explicitly specified.
    // These are water-loop sizing parameters derived from ASHRAE HVAC Systems
    // and Equipment, Chapter 13 "Hydronic Heating and Cooling" for residential
    // flow-rate sizing conventions, and Chapter 32 "Boilers" for return-water
    // temperature in condensing-mode operation. Neither the HPXML data model
    // nor a single primary source prescribes canonical defaults for these fields;
    // 0.5 kg/s (~8 gpm for a typical 60 000 Btu/h residential boiler, from the
    // historical US standard of 1 gpm per 10 000 Btu/h) and 40.0 °C (typical
    // condensing-boiler return temperature, well below the ~55 °C flue-gas
    // dewpoint) are engineering estimates pending calibration data.
    let flow_rate_kg_s = params
        .get("flow_rate_kg_s")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            tracing::debug!(
                equipment_id = name,
                field = "flow_rate_kg_s",
                value = 0.5_f64,
                "boiler hydronic flow rate not specified; using engineering default \
                 (ASHRAE SE Ch.13 residential sizing convention)"
            );
            0.5
        });
    let return_temp_c = params
        .get("return_temp_c")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            tracing::debug!(
                equipment_id = name,
                field = "return_temp_c",
                value = 40.0_f64,
                "boiler return water temperature not specified; using engineering default \
                 (40 °C, typical condensing-boiler return per ASHRAE SE Ch.32)"
            );
            40.0
        });

    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let setpoints = extract_setpoints_reconciled(params);
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
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        heating_setpoint_source,
        // Condensing mode inferred from AFUE > 0.90 (OCHRE convention).
        // OCHRE HVAC.py GasBoiler class: `condensing = eir_max < 1 / 0.9`.
        // Condensing boilers operate at lower return water temperatures (~150 °F)
        // with a 6-coefficient efficiency curve vs 10 coefficients for non-condensing.
        condensing: afue > 0.90,
    };
    Ok(Some(
        EquipmentConfig::from_typed(name.to_string(), "Gas Boiler".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    ))
}

/// Build an `ElectricBoilerConfig` typed config.
fn try_build_electric_boiler_config(
    name: &str,
    params: &Map<String, Value>,
) -> std::result::Result<Option<EquipmentConfig>, HpxmlError> {
    let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return Ok(None);
    };
    let heating_efficiency = resistance_efficiency_from_params(params, name, "Electric Boiler")?;
    let n_speeds = n_speeds_from_params(params);
    let fan_power_w = fan_power_from_params(params);

    // Hydronic loop flow rate and return-temperature are configuration defaults
    // applied by the boiler equipment model when not explicitly specified.
    // These are water-loop sizing parameters derived from ASHRAE HVAC Systems
    // and Equipment, Chapter 13 "Hydronic Heating and Cooling" for residential
    // flow-rate sizing conventions, and Chapter 32 "Boilers" for return-water
    // temperature in condensing-mode operation. See Gas Boiler path above for
    // the full citation rationale.
    let flow_rate_kg_s = params
        .get("flow_rate_kg_s")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            tracing::debug!(
                equipment_id = name,
                field = "flow_rate_kg_s",
                value = 0.5_f64,
                "electric boiler hydronic flow rate not specified; using engineering default \
                 (ASHRAE SE Ch.13 residential sizing convention)"
            );
            0.5
        });
    let return_temp_c = params
        .get("return_temp_c")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            tracing::debug!(
                equipment_id = name,
                field = "return_temp_c",
                value = 40.0_f64,
                "electric boiler return water temperature not specified; using engineering default \
                 (40 °C, typical condensing-boiler return per ASHRAE SE Ch.32)"
            );
            40.0
        });

    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let setpoints = extract_setpoints_reconciled(params);
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
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        heating_setpoint_source,
    };
    Ok(Some(
        EquipmentConfig::from_typed(name.to_string(), "Electric Boiler".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    ))
}

/// Build an `ElectricBaseboardConfig` typed config.
///
/// Electric resistance heating is by definition 100% efficient at the appliance
/// (all electrical input becomes heat). This conventional value is documented in
/// ASHRAE Handbook — HVAC Systems and Equipment 2020, Ch. 33 "Furnaces".
fn try_build_electric_baseboard_config(
    name: &str,
    params: &Map<String, Value>,
) -> std::result::Result<Option<EquipmentConfig>, HpxmlError> {
    let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return Ok(None);
    };
    let zone_id = params
        .get("zone_id")
        .and_then(Value::as_u64)
        .map(|v| v as u16);
    let heating_efficiency = resistance_efficiency_from_params(params, name, "Electric Baseboard")?;

    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let setpoints = extract_setpoints_reconciled(params);
    let cfg = ElectricBaseboardConfig {
        equipment_id: None,
        zone_id,
        capacity_w,
        eir: heating_efficiency,
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        heating_setpoint_source,
    };
    Ok(Some(
        EquipmentConfig::from_typed(name.to_string(), "Electric Baseboard".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
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
    let cooling_eir = seer_from_params(params).map(|seer| BTU_PER_HR_PER_W / seer.max(1e-6));
    let rated_eir = heating_eir.or(cooling_eir);
    let fraction = params.get("fraction_load_served").and_then(Value::as_f64);
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let setpoints = extract_setpoints_reconciled(params);

    let cfg = IdealHvacConfig {
        equipment_id: None,
        zone_id: None,
        heating_setpoint_c: static_setpoint_from_source(&heating_setpoint_source),
        cooling_setpoint_c: static_setpoint_from_source(&cooling_setpoint_source),
        deadband_c: params.get("deadband_c").and_then(Value::as_f64),
        n_speeds: Some(n_speeds_from_params(params)),
        ideal_capacity_mode: None,
        heating_setpoint_source,
        cooling_setpoint_source,
        heating_capacity_w,
        cooling_capacity_w,
        shr: params.get("shr").and_then(Value::as_f64),
        fraction_heating_load_served: fraction,
        fraction_cooling_load_served: fraction,
        rated_fan_power_w: None,
        rated_eir,
        capacity_min_w: None,
        fuel_type: None,
        capacity_biquadratic_coeffs: None,
        eir_biquadratic_coeffs: None,
    };
    Some(
        EquipmentConfig::from_typed(name.to_string(), "Ideal HVAC".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
}

/// Build a `CentralAirConditionerConfig` typed config.
fn try_build_central_ac_config(
    name: &str,
    params: &Map<String, Value>,
    duct_params: &DuctDseParams,
) -> Option<EquipmentConfig> {
    let Some(seer) = seer_from_params(params) else {
        tracing::warn!("Skipping AC: AnnualCoolingEfficiency (SEER) not found in HPXML");
        return None;
    };
    let eir = BTU_PER_HR_PER_W / seer.max(1e-6);
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
    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "cooling_airflow_cfm", capacity_w)
            .unwrap_or_else(|| {
                400.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
            });
    let duct = compute_duct_config(
        duct_params,
        capacity_w,
        false,
        n_speeds,
        false,
        Some(airflow_m3_s_per_w),
    );
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let setpoints = extract_setpoints_reconciled(params);

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
        // Crankcase heater: read from HPXML extension (CrankcaseHeaterPowerWatts in W).
        // Central AC/ASHP defaults: 50 W (0.050 kW) at 12.78°C (55°F) per OCHRE.
        // Room ACs typically have no crankcase heater (0.0 kW).
        crankcase_heater_kw: Some(
            params
                .get("crankcase_heater_w")
                .and_then(Value::as_f64)
                .map(|w| w / 1000.0)
                .unwrap_or(0.050),
        ),
        crankcase_heater_threshold_c: Some(12.78),
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
        charge_defect_ratio: params.get("charge_defect_ratio").and_then(Value::as_f64),
    };
    Some(
        EquipmentConfig::from_typed(name.to_string(), "Air Conditioner".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
}

/// Build a `RoomAcConfig` typed config.
fn try_build_room_ac_config(name: &str, params: &Map<String, Value>) -> Option<EquipmentConfig> {
    let capacity_w = params.get("cooling_capacity_w").and_then(Value::as_f64)?;
    // Room ACs are rated with EER, not SEER. SEER cannot be substituted for EER
    // because the test conditions and cycling correction factors differ; treating
    // SEER as EER would overestimate efficiency by ~10–15%.
    let eer = eer_from_params(params)?;
    let eir = BTU_PER_HR_PER_W / eer.max(1e-6);

    let curve_bounds = extract_curve_bounds(params);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "cooling_airflow_cfm", capacity_w)
            .unwrap_or_else(|| {
                320.0_f64 * CFM_TO_M3_S / W_PER_TON * airflow_defect_multiplier(params)
            });
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let setpoints = extract_setpoints_reconciled(params);
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
        shr: params.get("shr").and_then(Value::as_f64),
        startup_cd: None,
        crankcase_heater_kw: params
            .get("crankcase_heater_w")
            .and_then(Value::as_f64)
            .map(|w| Some(w / 1000.0))
            .unwrap_or(Some(0.0)),
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
    };
    Some(
        EquipmentConfig::from_typed(name.to_string(), "Room AC".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
}

/// Build a `DehumidifierConfig` typed config.
fn try_build_dehumidifier_config(
    name: &str,
    params: &Map<String, Value>,
) -> Option<EquipmentConfig> {
    let setpoints = extract_setpoints_reconciled(params);
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
    Some(
        EquipmentConfig::from_typed(name.to_string(), "Dehumidifier".to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
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
    let heating_eir = hspf_from_params(params).map(|hspf| BTU_PER_HR_PER_W / hspf.max(1e-6));
    let cooling_eir = seer_from_params(params).map(|seer| BTU_PER_HR_PER_W / seer.max(1e-6));
    let shr = params.get("shr").and_then(Value::as_f64);
    let fan_power_w = fan_power_from_params(params);
    let backup_capacity_w = params.get("backup_capacity_w").and_then(Value::as_f64);
    let backup_eir = params.get("backup_eir").and_then(Value::as_f64);
    let backup_fuel = params
        .get("backup_fuel")
        .and_then(Value::as_str)
        .map(|s| super::xml_helpers::parse_fuel(Some(s)));
    let fraction_heating_load_served = params
        .get("fraction_heating_load_served")
        .and_then(Value::as_f64);
    let fraction_cooling_load_served = params
        .get("fraction_cooling_load_served")
        .and_then(Value::as_f64);
    let curve_bounds = extract_curve_bounds(params);
    let ref_cap_w = heating_capacity_w.or(cooling_capacity_w).unwrap_or(0.0);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "heating_airflow_cfm", ref_cap_w)
            .unwrap_or_else(|| {
                (if is_mini_split {
                    312.0_f64 * CFM_TO_M3_S / W_PER_TON
                } else {
                    400.0_f64 * CFM_TO_M3_S / W_PER_TON
                }) * airflow_defect_multiplier(params)
            });
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let setpoints = extract_setpoints_reconciled(params);

    let ref_cap = heating_capacity_w.or(cooling_capacity_w).unwrap_or(0.0);
    let duct = if is_mini_split {
        DuctConfig::default()
    } else {
        compute_duct_config(
            duct_params,
            ref_cap,
            true,
            n_speeds,
            true,
            Some(airflow_m3_s_per_w),
        )
    };

    let ochre_class = if is_mini_split {
        "MSHP Heater"
    } else {
        "ASHP Heater"
    };

    let cfg = HeatPumpHeaterConfig {
        common: HeatPumpCommonConfig {
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
            min_compressor_fraction: params
                .get("min_compressor_fraction")
                .and_then(Value::as_f64)
                .unwrap_or(0.25),
            eir_part_load_benefit: params.get("eir_part_load_benefit").and_then(Value::as_f64),
            er_stages: params
                .get("er_stages")
                .and_then(Value::as_u64)
                .map(|v| v as u8)
                .unwrap_or(1),
            charge_defect_ratio: params.get("charge_defect_ratio").and_then(Value::as_f64),
            pump_loop_depth_m: params.get("pump_loop_depth_m").and_then(Value::as_f64),
            pump_pipe_diameter_m: params.get("pump_pipe_diameter_m").and_then(Value::as_f64),
            pump_flow_rate_m3_per_s: params
                .get("pump_flow_rate_m3_per_s")
                .and_then(Value::as_f64),
            pump_efficiency: params.get("pump_efficiency").and_then(Value::as_f64),
            pump_motor_efficiency: params.get("pump_motor_efficiency").and_then(Value::as_f64),
            pump_system_head_loss_m: params
                .get("pump_system_head_loss_m")
                .and_then(Value::as_f64),
            enter_water_temp_c: params.get("enter_water_temp_c").and_then(Value::as_f64),
            borehole_depth_m: params.get("borehole_depth_m").and_then(Value::as_f64),
            borehole_radius_m: params.get("borehole_radius_m").and_then(Value::as_f64),
            borehole_shank_spacing_m: params
                .get("borehole_shank_spacing_m")
                .and_then(Value::as_f64),
            number_of_boreholes: params
                .get("number_of_boreholes")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            borehole_soil_conductivity_w_per_m_k: params
                .get("borehole_soil_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
            borehole_soil_diffusivity_m2_per_day: params
                .get("borehole_soil_diffusivity_m2_per_day")
                .and_then(Value::as_f64),
            borehole_grout_conductivity_w_per_m_k: params
                .get("borehole_grout_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
            borehole_pipe_outer_radius_m: params
                .get("borehole_pipe_outer_radius_m")
                .and_then(Value::as_f64),
            borehole_pipe_inner_radius_m: params
                .get("borehole_pipe_inner_radius_m")
                .and_then(Value::as_f64),
            borehole_pipe_conductivity_w_per_m_k: params
                .get("borehole_pipe_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
        },
        hp_lockout_temp_c: params.get("hp_lockout_temp_c").and_then(Value::as_f64),
        er_lockout_temp_c: params.get("er_lockout_temp_c").and_then(Value::as_f64),
        max_oat_supplemental_c: params.get("max_oat_supplemental_c").and_then(Value::as_f64),
        er_setpoint_offset_c: params.get("er_setpoint_offset_c").and_then(Value::as_f64),
        er_hard_lockout_time_s: params.get("er_hard_lockout_time_s").and_then(Value::as_f64),
        heating_shr: None,
        capacity_ratio_at_17f: params.get("capacity_ratio_at_17f").and_then(Value::as_f64),
        defrost: {
            let mut d = DefrostConfig::default();
            if let Some(s) = params.get("defrost_control").and_then(Value::as_str) {
                if let Ok(c) =
                    serde_json::from_value::<DefrostControl>(Value::String(s.to_string()))
                {
                    d.control = c;
                }
            }
            if let Some(s) = params.get("defrost_strategy").and_then(Value::as_str) {
                if let Ok(st) =
                    serde_json::from_value::<DefrostStrategy>(Value::String(s.to_string()))
                {
                    d.strategy = st;
                }
            }
            if let Some(v) = params.get("defrost_time_fraction").and_then(Value::as_f64) {
                d.defrost_time_fraction = v;
            }
            if let Some(v) = params.get("defrost_max_oat_c").and_then(Value::as_f64) {
                d.max_oat_defrost_c = v;
            }
            if let Some(v) = params
                .get("defrost_capacity_reduction_factor")
                .and_then(Value::as_f64)
            {
                d.capacity_reduction_factor = v;
            }
            if let Some(v) = params.get("defrost_power_w").and_then(Value::as_f64) {
                d.defrost_power_w = v;
            }
            if let Some(v) = params
                .get("resistive_defrost_capacity_w")
                .and_then(Value::as_f64)
            {
                d.resistive_defrost_capacity_w = v;
            }
            d
        },
    };
    Some(
        EquipmentConfig::from_typed(name.to_string(), ochre_class.to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
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
    let heating_eir = hspf_from_params(params).map(|hspf| BTU_PER_HR_PER_W / hspf.max(1e-6));
    let cooling_eir = seer_from_params(params).map(|seer| BTU_PER_HR_PER_W / seer.max(1e-6));
    let shr = params.get("shr").and_then(Value::as_f64);
    let fan_power_w = fan_power_from_params(params);
    let backup_capacity_w = params.get("backup_capacity_w").and_then(Value::as_f64);
    let backup_eir = params.get("backup_eir").and_then(Value::as_f64);
    let backup_fuel = params
        .get("backup_fuel")
        .and_then(Value::as_str)
        .map(|s| super::xml_helpers::parse_fuel(Some(s)));
    let fraction_heating_load_served = params
        .get("fraction_heating_load_served")
        .and_then(Value::as_f64);
    let fraction_cooling_load_served = params
        .get("fraction_cooling_load_served")
        .and_then(Value::as_f64);
    let curve_bounds = extract_curve_bounds(params);
    let ref_cap_w = cooling_capacity_w.or(heating_capacity_w).unwrap_or(0.0);
    let airflow_m3_s_per_w =
        airflow_m3_s_per_w_from_explicit_cfm(params, "cooling_airflow_cfm", ref_cap_w)
            .unwrap_or_else(|| {
                (if is_mini_split {
                    312.0_f64 * CFM_TO_M3_S / W_PER_TON
                } else {
                    400.0_f64 * CFM_TO_M3_S / W_PER_TON
                }) * airflow_defect_multiplier(params)
            });
    let heating_setpoint_source = schedule_source_from_params(params, "heating");
    let cooling_setpoint_source = schedule_source_from_params(params, "cooling");
    let setpoints = extract_setpoints_reconciled(params);

    let ref_cap = cooling_capacity_w.or(heating_capacity_w).unwrap_or(0.0);
    let duct = if is_mini_split {
        DuctConfig::default()
    } else {
        compute_duct_config(
            duct_params,
            ref_cap,
            false,
            n_speeds,
            true,
            Some(airflow_m3_s_per_w),
        )
    };

    let ochre_class = if is_mini_split {
        "MSHP Cooler"
    } else {
        "ASHP Cooler"
    };

    let cfg = HeatPumpCoolerConfig {
        common: HeatPumpCommonConfig {
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
            min_compressor_fraction: params
                .get("min_compressor_fraction")
                .and_then(Value::as_f64)
                .unwrap_or(0.25),
            eir_part_load_benefit: params.get("eir_part_load_benefit").and_then(Value::as_f64),
            er_stages: params
                .get("er_stages")
                .and_then(Value::as_u64)
                .map(|v| v as u8)
                .unwrap_or(1),
            charge_defect_ratio: params.get("charge_defect_ratio").and_then(Value::as_f64),
            pump_loop_depth_m: params.get("pump_loop_depth_m").and_then(Value::as_f64),
            pump_pipe_diameter_m: params.get("pump_pipe_diameter_m").and_then(Value::as_f64),
            pump_flow_rate_m3_per_s: params
                .get("pump_flow_rate_m3_per_s")
                .and_then(Value::as_f64),
            pump_efficiency: params.get("pump_efficiency").and_then(Value::as_f64),
            pump_motor_efficiency: params.get("pump_motor_efficiency").and_then(Value::as_f64),
            pump_system_head_loss_m: params
                .get("pump_system_head_loss_m")
                .and_then(Value::as_f64),
            enter_water_temp_c: params.get("enter_water_temp_c").and_then(Value::as_f64),
            borehole_depth_m: params.get("borehole_depth_m").and_then(Value::as_f64),
            borehole_radius_m: params.get("borehole_radius_m").and_then(Value::as_f64),
            borehole_shank_spacing_m: params
                .get("borehole_shank_spacing_m")
                .and_then(Value::as_f64),
            number_of_boreholes: params
                .get("number_of_boreholes")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            borehole_soil_conductivity_w_per_m_k: params
                .get("borehole_soil_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
            borehole_soil_diffusivity_m2_per_day: params
                .get("borehole_soil_diffusivity_m2_per_day")
                .and_then(Value::as_f64),
            borehole_grout_conductivity_w_per_m_k: params
                .get("borehole_grout_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
            borehole_pipe_outer_radius_m: params
                .get("borehole_pipe_outer_radius_m")
                .and_then(Value::as_f64),
            borehole_pipe_inner_radius_m: params
                .get("borehole_pipe_inner_radius_m")
                .and_then(Value::as_f64),
            borehole_pipe_conductivity_w_per_m_k: params
                .get("borehole_pipe_conductivity_w_per_m_k")
                .and_then(Value::as_f64),
        },
        stage_shrs: extract_stage_values(params, "shr"),
        // Crankcase heater: power (W) from HPXML extension → kW.
        // ASHP defaults: 50 W (0.050 kW) at 12.78°C (55°F); MSHP defaults: 15 W (0.015 kW) at 0°C (32°F).
        // OCHRE HVAC.py AirConditioner / MinisplitAHSPCooler classes.
        crankcase_heater_kw: params
            .get("crankcase_heater_w")
            .and_then(Value::as_f64)
            .map(|w| Some(w / 1000.0))
            .unwrap_or_else(|| {
                if is_mini_split {
                    Some(0.015)
                } else {
                    Some(0.050)
                }
            }),
        crankcase_heater_threshold_c: if is_mini_split {
            Some(0.0)
        } else {
            Some(12.78)
        },
    };
    Some(
        EquipmentConfig::from_typed(name.to_string(), ochre_class.to_string(), cfg)
            .with_setpoints_reconciled(setpoints),
    )
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
fn zone_type_to_ashrae152_str(zone: &Zone, building: &Building) -> super::Result<String> {
    match zone.zone_type {
        ZoneType::Attic => {
            if zone.vented {
                Ok("attic_vented".into())
            } else {
                Ok("attic_unvented".into())
            }
        }
        ZoneType::Garage => Ok("garage".into()),
        ZoneType::Foundation => {
            let fnd_name = building.foundation_name.as_deref().unwrap_or("");
            let wall_ins = foundation_wall_is_insulated(&building.boundaries);
            let floor_ins = foundation_floor_is_insulated(&building.boundaries);
            if fnd_name.is_empty() {
                tracing::warn!(
                    "Foundation zone has no foundation name; falling back to crawlspace \
                     ASHRAE 152 zone type default"
                );
            }
            if fnd_name == "Crawlspace" {
                let v = if zone.vented { "vent" } else { "unvent" };
                if wall_ins && floor_ins {
                    Ok(format!("{v}_crawlspace_ins_floor_wall"))
                } else if floor_ins {
                    Ok(format!("{v}_crawlspace_ins_floor"))
                } else {
                    Ok(format!("{v}_unins_crawlspace"))
                }
            } else if fnd_name.contains("Basement") {
                if wall_ins {
                    Ok("basement_ins_walls".into())
                } else if floor_ins {
                    Ok("basement_ins_ceiling".into())
                } else {
                    Ok("unins_basement".into())
                }
            } else {
                // Unknown foundation sub-type: fall back to uninsulated crawlspace.
                if !fnd_name.is_empty() {
                    tracing::warn!(
                        foundation_name = %fnd_name,
                        "Unknown foundation sub-type; falling back to crawlspace \
                         ASHRAE 152 zone type"
                    );
                }
                if zone.vented {
                    Ok("vent_unins_crawlspace".into())
                } else {
                    Ok("unvent_unins_crawlspace".into())
                }
            }
        }
        ZoneType::Conditioned => {
            // Invariant: the sole call site in compute_duct_dse_params skips
            // Conditioned zones before invoking this function.
            unreachable!(
                "Conditioned zones are filtered by call site before invoking \
                 zone_type_to_ashrae152_str"
            )
        }
        ZoneType::Outdoor => Err(super::HpxmlError::Parse(
            "zone type 'Outdoor' is not supported for ASHRAE 152 duct derating".into(),
        )),
        ZoneType::Ground => Err(super::HpxmlError::Parse(
            "zone type 'Ground' is not supported for ASHRAE 152 duct derating".into(),
        )),
        ZoneType::Adjacent => Err(super::HpxmlError::Parse(
            "zone type 'Adjacent' is not supported for ASHRAE 152 duct derating".into(),
        )),
        ZoneType::Other(ref s) => Err(super::HpxmlError::Parse(format!(
            "unrecognised zone type '{s}' is not supported for ASHRAE 152 duct derating"
        ))),
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

fn conditioned_zone_id(building: &Building) -> Option<u16> {
    let idx = building
        .zones
        .iter()
        .position(|z| matches!(z.zone_type, ZoneType::Conditioned))?;
    Some((idx as u16) + 1)
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
    let duct_params = compute_duct_dse_params(building)?;
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
        insert_annual_efficiency(&mut params, heating, true, false);
        if name == "Ideal HVAC" {
            insert_annual_efficiency(&mut params, heating, false, false);
        }
        params.insert("system_type".to_string(), Value::String(system_type));

        if let Some(frac) = child_f64(heating, "FractionHeatLoadServed")
            .or_else(|| child_f64(heating, "FractionHeatingLoadServed"))
        {
            params.insert("fraction_load_served".to_string(), json!(frac));
        }
        if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
            // ElectricAuxiliaryEnergy is an annual total (kWh/year per HPXML Data
            // Dictionary v4.2).
            //
            // For furnaces: dividing by HOURS_PER_YEAR (8760 h) yields an
            // average-watts value — this is an approximation: real fan power is
            // load-dependent and the per-timestep distribution is distorted, though
            // the annual energy total is preserved. Prefer FanPowerWattsPerCFM or
            // FanPowerWatts extension fields when available.
            //
            // For boilers: dividing by BOILER_AUXILIARY_HOURS_PER_YEAR (2080 h)
            // per ANSI/RESNET/ICC 301-2019 Eq. 4.4-5 and the ResStock convention
            // (OCHRE hvac.rb:1754), reflecting heating-season pump/control operating
            // hours rather than year-round continuous duty.
            let hours = if name.contains("Boiler") {
                BOILER_AUXILIARY_HOURS_PER_YEAR
            } else {
                HOURS_PER_YEAR
            };
            params.insert(
                "auxiliary_power_w".to_string(),
                json!(aux_kwh / hours * KW_TO_W),
            );
        }
        if let Some(ext) = heating.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
            if let Some(v) = child_f64(ext, "AirflowDefectRatio") {
                params.insert("airflow_defect_ratio".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "HeatingAirflowCFM") {
                params.insert("heating_airflow_cfm".to_string(), json!(v));
            }
        }
        insert_autosizing_params(&mut params, heating, true, false, false);
        for (k, v) in &setpoint_params {
            params.insert(k.clone(), v.clone());
        }
        apply_building_setpoint_profiles(building, &mut params, true, name == "Ideal HVAC");
        if name == "Ideal HVAC" {
            if let Some(deadband) = building.hvac_deadband_c {
                params.insert("deadband_c".to_string(), json!(deadband));
            }
        }
        duct_params.insert_into_map(&mut params);
        for (k, v) in &basement_params {
            params.insert(k.clone(), v.clone());
        }
        // OCHRE only applies startup_cd for heat pump heaters (ASHP/MSHP),
        // not for furnaces, boilers, or baseboard.
        if matches!(name.as_str(), "ASHP Heater" | "MSHP Heater") {
            insert_startup_degradation(&mut params, &name, true);
        }
        if name == "Electric Baseboard" {
            if let Some(zone_id) = conditioned_zone_id(building) {
                params.insert("zone_id".to_string(), json!(zone_id));
            }
        }
        if name == "Gas Furnace" {
            apply_multispeed_furnace_parameters(&mut params, defaults, &name);
        }
        let typed_config = match name.as_str() {
            "Gas Furnace" => match try_build_gas_furnace_config(&name, &params, &duct_params) {
                Ok(config) => config,
                Err(HpxmlError::MissingField { .. })
                    if params
                        .get("autosize_heating")
                        .and_then(Value::as_bool)
                        .unwrap_or(false) =>
                {
                    None
                }
                Err(e) => return Err(e),
            },
            "Electric Furnace" => try_build_electric_furnace_config(&name, &params, &duct_params)?,
            "Gas Boiler" => match try_build_gas_boiler_config(&name, &params) {
                Ok(config) => config,
                Err(HpxmlError::MissingField { .. })
                    if params
                        .get("autosize_heating")
                        .and_then(Value::as_bool)
                        .unwrap_or(false) =>
                {
                    None
                }
                Err(e) => return Err(e),
            },
            "Electric Boiler" => try_build_electric_boiler_config(&name, &params)?,
            "Electric Baseboard" => try_build_electric_baseboard_config(&name, &params)?,
            "Ideal HVAC" => try_build_ideal_hvac_config(&name, &params),
            // Unreachable: canonical_hvac_heating_name generates exactly the
            // six names matched above and rejects all others with Err.
            _ => unreachable!(
                "canonical_hvac_heating_name validated '{}' but typed_config match did not cover it",
                name,
            ),
        };
        let mut spec = build_spec(name, fuel, params, defaults);
        spec.typed_config = typed_config;
        spec.system_id = element_id(heating);
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
        insert_annual_efficiency(&mut params, cooling, false, false);
        params.insert("system_type".to_string(), Value::String(system_type));
        insert_mode_and_speed_metadata(
            &mut params,
            child_text(cooling, "CompressorType").as_deref(),
            "CoolingSystem",
            &name,
        )?;
        apply_default_hvac_speed_fallback(
            &mut params,
            "CoolingSystem/AnnualCoolingEfficiency",
            "CoolingSystem",
            &name,
        )?;
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
        // Crankcase heater power: HPXML does not define a standard element.
        // OpenStudio-HPXML uses extension/CrankcaseHeaterPowerWatts (W).
        // Non-standard HPXML files may carry a direct CrankcaseHeaterWatts (W) child.
        let crankcase_w = child_f64(cooling, "CrankcaseHeaterWatts");
        if let Some(ext) = cooling.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
            if let Some(v) = child_f64(ext, "AirflowDefectRatio") {
                params.insert("airflow_defect_ratio".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "ChargeDefectRatio") {
                params.insert("charge_defect_ratio".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "CoolingAirflowCFM") {
                params.insert("cooling_airflow_cfm".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "CrankcaseHeaterPowerWatts") {
                params.insert("crankcase_heater_w".to_string(), json!(v));
            }
        }
        // Non-standard direct CrankcaseHeaterWatts element (used by some HPXML
        // files that pre-date the OpenStudio-HPXML extension convention).
        if let Some(w) = crankcase_w {
            params.insert("crankcase_heater_w".to_string(), json!(w));
        }
        insert_autosizing_params(&mut params, cooling, false, true, false);
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
            // Unreachable: canonical_hvac_cooling_name generates exactly the
            // two names matched above and rejects all others with Err.
            _ => unreachable!(
                "canonical_hvac_cooling_name validated '{}' but typed_config match did not cover it",
                name,
            ),
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

        let (heater_name, cooler_name) = match heat_pump_type.as_str() {
            "air-to-air" => ("ASHP Heater", "ASHP Cooler"),
            "mini-split" => ("MSHP Heater", "MSHP Cooler"),
            "ground-to-air" => ("GSHP Heater", "GSHP Cooler"),
            "water-loop-to-air" => ("WSHP Heater", "WSHP Cooler"),
            "water-to-air" => ("WSHP Heater", "WSHP Cooler"),
            other => {
                return Err(HpxmlError::Parse(format!(
                    "HeatPump: unsupported HeatPumpType '{other}'; \
                     supported types are: air-to-air, mini-split, ground-to-air, \
                     water-loop-to-air, water-to-air"
                )));
            }
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
        // HPXML MinimumCapacity → min_compressor_fraction when HeatingCapacity is available.
        // MinimumCapacity is the lowest compressor output; min_compressor_fraction = MinimumCapacity / HeatingCapacity.
        if let Some(min_btu_h) = child_f64(heat_pump, "MinimumCapacity") {
            if let Some(Value::Number(cap_n)) = params.get("heating_capacity_w") {
                if let Some(heat_w) = cap_n.as_f64() {
                    if heat_w > 0.0 {
                        let min_w = conv::power_btu_h_to_w(min_btu_h);
                        let raw_frac = min_w / heat_w;
                        let frac = raw_frac.clamp(0.1, 0.5);
                        if (frac - raw_frac).abs() > f64::EPSILON {
                            tracing::warn!(
                                raw = raw_frac,
                                clamped = frac,
                                "HPXML MinimumCapacity / HeatingCapacity = {raw_frac:.3} \
                                 is outside [0.1, 0.5]; clamped to {frac:.3}"
                            );
                        }
                        params.insert("min_compressor_fraction".to_string(), json!(frac));
                    }
                }
            }
        }
        let is_mini_split = heat_pump_type == "mini-split";
        insert_annual_efficiency(&mut params, heat_pump, true, is_mini_split);
        insert_annual_efficiency(&mut params, heat_pump, false, is_mini_split);
        params.insert(
            "heat_pump_type".to_string(),
            Value::String(heat_pump_type.clone()),
        );

        // HeatingCapacity17F: AHRI 210/240 H3 low-ambient rating point at 17°F (-8.33°C).
        // Stores capacity_ratio_at_17f = HeatingCapacity17F / HeatingCapacity (both in W)
        // so the biquadratic curve can be validated against the manufacturer spec.
        if let Some(cap_17f_btu) = child_f64(heat_pump, "HeatingCapacity17F") {
            if let Some(cap_w) = params.get("heating_capacity_w").and_then(Value::as_f64) {
                if cap_w > 0.0 {
                    let cap_17f_w = conv::power_btu_h_to_w(cap_17f_btu);
                    params.insert(
                        "capacity_ratio_at_17f".to_string(),
                        json!(cap_17f_w / cap_w),
                    );
                }
            }
        }

        // Backup heating parameters.
        // HPXML does not encode "no backup system" via an explicit element —
        // absence of BackupHeatingCapacity alone does not distinguish between
        // (a) a backup system exists but capacity is omitted for autosizing,
        // and (b) no backup system exists at all.
        // Gate autosizing on evidence that a backup system is declared.
        if let Some(cap_btu) = child_f64(heat_pump, "BackupHeatingCapacity") {
            params.insert(
                "backup_capacity_w".to_string(),
                json!(conv::power_btu_h_to_w(cap_btu)),
            );
        } else if child_text(heat_pump, "BackupSystemFuel").is_some()
            || child_text(heat_pump, "BackupType").is_some()
        {
            // Backup system declared but capacity omitted — autosize to 100%
            // of design heating load (no oversizing) per ACCA Manual S-2017.
            params.insert("autosize_backup".to_string(), json!(true));
        }
        // else: no backup system declared — backup_capacity_w stays absent.
        // For MSHP this means 0 W backup (correct — MSHP typically has no
        // backup). For ASHP, init_from_typed will return Err if a required
        // backup capacity is missing, guarding against a genuinely incomplete
        // HPXML rather than silently inferring an absent backup system.
        if let Some(eff_node) = heat_pump.child("BackupAnnualHeatingEfficiency") {
            if let Some(val) = child_f64(eff_node, "Value") {
                let units_raw = child_text(eff_node, "Units").unwrap_or_default();
                let units = units_raw.to_ascii_uppercase();
                let eir = match units.as_str() {
                    "PERCENT" if val > 1.0 => {
                        // Value expressed as percent-out-of-100 (e.g. 95 → 95%).
                        // Divide by 100 before inverting so 100% efficiency yields EIR = 1.0.
                        // OpenStudio-HPXML (NREL reference implementation) treats Percent
                        // values as fractions 0–1, but HARES guards against the
                        // percent-out-of-100 form to be robust to all valid HPXML inputs.
                        100.0 / val.max(0.01)
                    }
                    "PERCENT" => {
                        // Value is already a fraction (0–1) — the conventional HPXML form.
                        1.0 / val.max(0.01)
                    }
                    // AFUE is always a fraction (0–1). Absent units default to fraction
                    // form per HPXML convention.
                    "AFUE" | "" => 1.0 / val.max(0.01),
                    // COP is already a COP; EIR = 1/COP.
                    "COP" => 1.0 / val.max(0.01),
                    // HSPF and HSPF2 are seasonal metrics valid per the HPXML XSD
                    // HeatingEfficiencyUnits_simple type, but they do not apply to a
                    // backup resistance or gas strip. Reject loudly.
                    "HSPF" | "HSPF2" => {
                        return Err(HpxmlError::Parse(format!(
                            "BackupAnnualHeatingEfficiency: '{units_raw}' is a seasonal metric, \
                             not supported for backup heating"
                        )));
                    }
                    _ => {
                        return Err(HpxmlError::Parse(format!(
                            "BackupAnnualHeatingEfficiency: unrecognized or unsupported units \
                             '{units_raw}'"
                        )));
                    }
                };
                params.insert("backup_eir".to_string(), json!(eir));
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
        // SupplementalHeatingLockoutTemperature (°F → °C).
        // HPXML 4.x schema does not define this as a standard element; it
        // appears in some non-standard HPXML files. The resolver reads it as
        // a direct child of <HeatPump> for compatibility and stores it as
        // max_oat_supplemental_c.
        if let Some(f_val) = child_f64(heat_pump, "SupplementalHeatingLockoutTemperature") {
            params.insert(
                "max_oat_supplemental_c".to_string(),
                json!(conv::temperature_f_to_c(f_val)),
            );
        }

        insert_mode_and_speed_metadata(
            &mut params,
            child_text(heat_pump, "CompressorType").as_deref(),
            "HeatPump",
            cooler_name,
        )?;
        apply_default_hvac_speed_fallback(
            &mut params,
            "HeatPump/AnnualCoolingEfficiency",
            "HeatPump",
            cooler_name,
        )?;
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
        if let Some(frac) = child_f64(heat_pump, "FractionHeatLoadServed")
            .or_else(|| child_f64(heat_pump, "FractionHeatingLoadServed"))
        {
            params.insert("fraction_heating_load_served".to_string(), json!(frac));
        }
        if let Some(frac) = child_f64(heat_pump, "FractionCoolLoadServed")
            .or_else(|| child_f64(heat_pump, "FractionCoolingLoadServed"))
        {
            params.insert("fraction_cooling_load_served".to_string(), json!(frac));
        }
        if let Some(ext) = heat_pump.child("extension") {
            if let Some(w_per_cfm) = child_f64(ext, "FanPowerWattsPerCFM") {
                params.insert("fan_power_w_per_cfm".to_string(), json!(w_per_cfm));
            } else if let Some(w) = child_f64(ext, "FanPowerWatts") {
                params.insert("fan_power_w".to_string(), json!(w));
            }
            if let Some(v) = child_f64(ext, "AirflowDefectRatio") {
                params.insert("airflow_defect_ratio".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "ChargeDefectRatio") {
                params.insert("charge_defect_ratio".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "HeatingAirflowCFM") {
                params.insert("heating_airflow_cfm".to_string(), json!(v));
            }
            if let Some(v) = child_f64(ext, "CoolingAirflowCFM") {
                params.insert("cooling_airflow_cfm".to_string(), json!(v));
            }
            // Crankcase heater power (W): OpenStudio-HPXML extension convention.
            if let Some(v) = child_f64(ext, "CrankcaseHeaterPowerWatts") {
                params.insert("crankcase_heater_w".to_string(), json!(v));
            }
        }
        insert_autosizing_params(&mut params, heat_pump, true, true, true);
        for (k, v) in &setpoint_params {
            params.insert(k.clone(), v.clone());
        }
        apply_building_setpoint_profiles(building, &mut params, true, true);
        if heat_pump_type != "mini-split" {
            duct_params.insert_into_map(&mut params);
        }

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

    for dehumidifier in descendants_named(hvac, "Dehumidifier") {
        let mut params = Map::new();
        if let Some(cap_pints_day) = child_f64(dehumidifier, "Capacity") {
            // HPXML §Dehumidifier/Capacity is in US liquid pints/day per the HPXML 4.x schema
            // annotation. 1 US liquid pint = 0.473176473 L (exact, NIST Handbook 44).
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
        (
            "Furnace",
            FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        )
        | (
            "WallFurnace",
            FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        )
        | (
            "FloorFurnace",
            FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Furnace",
        (
            "Boiler",
            FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Boiler",
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
    } else {
        // Mark for autosizing: capacity was omitted from HPXML.
        // The dwelling builder will back-calculate the required capacity
        // from the building envelope model and design weather conditions.
        // No placeholder is inserted — the absent capacity triggers a loud
        // error in the typed-config builder, which the resolver catches when
        // autosizing is flagged and assigns None typed_config until autosizing
        // computes the real capacity.
        let autosize_key = if key.contains("heating") {
            "autosize_heating"
        } else {
            "autosize_cooling"
        };
        params.insert(autosize_key.to_string(), json!(true));
    }
}

/// Parse HPXML `<HeatingAutosizingFactor>`, `<CoolingAutosizingFactor>`,
/// `<BackupHeatingAutosizingFactor>`, and `<AutosizingLimits>` from a system
/// element. Supports both direct children and `<extension>` children (the
/// convention used by OpenStudio-HPXML / ResStock).
///
/// # Arguments
/// * `has_heating` — `true` for `HeatingSystem` and `HeatPump`.
/// * `has_cooling` — `true` for `CoolingSystem` and `HeatPump`.
/// * `has_backup` — `true` only for `HeatPump`.
fn insert_autosizing_params(
    params: &mut Map<String, Value>,
    node: &XmlNode,
    has_heating: bool,
    has_cooling: bool,
    has_backup: bool,
) {
    // Helper: read a factor from direct child, then extension child.
    // Direct children take precedence (HPXML 4.x schema native location).
    let read_factor = |params: &mut Map<String, Value>, tag: &str, key: &str| {
        if let Some(factor) = child_f64(node, tag) {
            params.insert(key.to_string(), json!(factor));
            return;
        }
        if let Some(ext) = node.child("extension") {
            if let Some(factor) = child_f64(ext, tag) {
                params.insert(key.to_string(), json!(factor));
            }
        }
    };

    if has_heating {
        read_factor(params, "HeatingAutosizingFactor", "autosize_heating_factor");
    }
    if has_cooling {
        read_factor(params, "CoolingAutosizingFactor", "autosize_cooling_factor");
    }
    if has_backup {
        read_factor(
            params,
            "BackupHeatingAutosizingFactor",
            "autosize_backup_factor",
        );
    }

    // Parse `<AutosizingLimits>` specifying min/max capacity bounds.
    // Supports direct child and extension child.
    // Child element names match the OpenStudio-HPXML convention:
    // `<MinCapacity>` and `<MaxCapacity>` (value in Btu/h).
    let limits_node = node.child("AutosizingLimits").or_else(|| {
        node.child("extension")
            .and_then(|ext| ext.child("AutosizingLimits"))
    });

    if let Some(limits) = limits_node {
        let insert_limit = |params: &mut Map<String, Value>, child: &str, key: &str| {
            if let Some(val_btu_h) = child_f64(limits, child) {
                params.insert(key.to_string(), json!(conv::power_btu_h_to_w(val_btu_h)));
            }
        };

        if has_heating {
            insert_limit(params, "MinCapacity", "autosize_heating_min_w");
            insert_limit(params, "MaxCapacity", "autosize_heating_max_w");
        }
        if has_cooling {
            insert_limit(params, "MinCapacity", "autosize_cooling_min_w");
            insert_limit(params, "MaxCapacity", "autosize_cooling_max_w");
        }
    }
}

fn insert_annual_efficiency(
    params: &mut Map<String, Value>,
    node: &XmlNode,
    is_heating: bool,
    is_ductless: bool,
) {
    let annual_tag = if is_heating {
        "AnnualHeatingEfficiency"
    } else {
        "AnnualCoolingEfficiency"
    };

    if let Some(annual) = node.child(annual_tag)
        && let (Some(units), Some(value)) =
            (child_text(annual, "Units"), child_f64(annual, "Value"))
    {
        let (normalized_units, normalized_value) =
            normalize_efficiency_units(&units, value, is_ductless);
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
            let (units, normalized) = normalize_efficiency_units(tag, value, is_ductless);
            params.insert(
                format!("efficiency_{}", units.to_ascii_lowercase()),
                json!(normalized),
            );
        }
    }
}

fn normalize_efficiency_units(units: &str, value: f64, is_ductless: bool) -> (String, f64) {
    match units.trim().to_ascii_uppercase().as_str() {
        // Ductless/mini-split units: the AHRI 210/240-2023 test-procedure change
        // primarily affects external static pressure in ducted systems. Ductless
        // SEER2 = SEER (factor 1.0) per RESNET MINHERS Addendum 71f.
        "SEER2" if is_ductless => ("SEER".to_string(), value),
        "SEER2" => ("SEER".to_string(), value * SEER2_TO_SEER_FACTOR),
        // Ductless HSPF2→HSPF ratio ≈ 0.90 (≈10% reduction) per MINHERS Addendum 71f,
        // milder than the ducted 0.85 because ductless units have no external static
        // pressure duct penalty.
        "HSPF2" if is_ductless => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR_DUCTLESS),
        "HSPF2" => ("HSPF".to_string(), value * HSPF2_TO_HSPF_FACTOR),
        "EER2" => ("EER".to_string(), value * EER2_TO_EER_FACTOR),
        // PERCENT values in HPXML are conventionally fractions (0–1), but some
        // sources supply percent-out-of-100 form. Normalize to fraction 0–1 by
        // dividing values > 1.0 by 100. Values ≤ 1.0 (including exactly 1.0)
        // are already in fraction form — 1.0 is unambiguous (100% = 1.0).
        "PERCENT" if value > 1.0 => ("PERCENT".to_string(), value / 100.0),
        "PERCENT" => ("PERCENT".to_string(), value),
        "SEER" | "EER" | "HSPF" | "AFUE" | "COP" => (units.trim().to_ascii_uppercase(), value),
        other => (other.to_string(), value),
    }
}

/// HPXML v4.x §8.4 CompressorType enumeration values.
/// Three valid values per the HPXML Data Dictionary; any other value is rejected
/// at parse time rather than silently defaulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HpxmlCompressorType {
    SingleStage,
    TwoStage,
    VariableSpeed,
}

fn parse_compressor_type(
    raw: &str,
    system_kind: &'static str,
    system_id: &str,
) -> Result<HpxmlCompressorType, HpxmlError> {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "single stage" => Ok(HpxmlCompressorType::SingleStage),
        "two stage" => Ok(HpxmlCompressorType::TwoStage),
        "variable speed" => Ok(HpxmlCompressorType::VariableSpeed),
        _ => {
            tracing::warn!(
                compressor_type = raw,
                system_kind,
                system_id,
                "Unknown HPXML CompressorType; rejecting rather than silently defaulting",
            );
            Err(HpxmlError::InvalidField {
                path: "CompressorType",
                system_kind,
                system_id: system_id.to_string(),
                value_received: raw.to_string(),
                reason: "CompressorType must be one of: single stage, two stage, variable speed",
            })
        }
    }
}

fn compressor_type_to_mode(ct: HpxmlCompressorType) -> &'static str {
    match ct {
        HpxmlCompressorType::SingleStage => "single_speed",
        HpxmlCompressorType::TwoStage => "two_speed",
        HpxmlCompressorType::VariableSpeed => "variable_speed",
    }
}

fn number_of_speeds_from_mode(mode: &str) -> usize {
    match mode {
        "single_speed" => 1,
        "two_speed" => 2,
        "variable_speed" => 4,
        // Unreachable: mode is produced by compressor_type_to_mode which
        // receives a validated HpxmlCompressorType — only the three arms
        // above can appear.
        _ => unreachable!(
            "compressor_type_to_mode produced unexpected mode '{}'; \
             HpxmlCompressorType enum should prevent this",
            mode,
        ),
    }
}

fn mode_from_number_of_speeds(n: usize) -> &'static str {
    match n {
        1 => "single_speed",
        2 => "two_speed",
        4 => "variable_speed",
        // Unreachable: n_speeds is always 1, 2, or 4 by construction —
        // either from number_of_speeds_from_mode (which returns 1, 2, or 4)
        // or from set_speed_fallback (which only sets 1, 2, or 4 based on
        // SEER/EER thresholds).  Any other integer would originate from
        // manual parameter injection and is a caller error.
        _ => unreachable!(
            "mode_from_number_of_speeds called with unexpected n_speeds={}; \
             only 1, 2, or 4 are valid",
            n,
        ),
    }
}

fn insert_mode_and_speed_metadata(
    params: &mut Map<String, Value>,
    compressor_type: Option<&str>,
    system_kind: &'static str,
    system_id: &str,
) -> Result<(), HpxmlError> {
    if let Some(raw) = compressor_type {
        let ct = parse_compressor_type(raw, system_kind, system_id)?;
        let mode = compressor_type_to_mode(ct);
        let n_speeds = number_of_speeds_from_mode(mode);
        params.insert(
            "speed_control_mode".to_string(),
            Value::String(mode.to_string()),
        );
        params.insert("number_of_speeds".to_string(), json!(n_speeds));
    }
    Ok(())
}

fn apply_default_hvac_speed_fallback(
    params: &mut Map<String, Value>,
    path: &'static str,
    system_kind: &'static str,
    system_id: &str,
) -> std::result::Result<(), HpxmlError> {
    if params.contains_key("number_of_speeds") {
        return Ok(());
    }

    // Try SEER first, then EER as a fallback for EER-only systems (room ACs
    // and some legacy central units).  EER is a valid primary efficiency metric
    // per HPXML v4.x for room air conditioners; SEER requires multi-condition
    // test data (AHRI 210/240) and may legitimately be absent.
    let efficiency = seer_from_params(params).or_else(|| eer_from_params(params));
    let Some(efficiency) = efficiency else {
        // HeatPumps may legitimately lack cooling efficiency when modelling
        // heating-only features (e.g. lockout temperatures, defrost, heating
        // capacity ratios at 17°F).  For standalone CoolingSystems a missing
        // efficiency is a data-quality error.
        if system_kind == "HeatPump" {
            // No CompressorType and no cooling efficiency — the speed cannot
            // be inferred from data.  Default to single-speed and warn so the
            // downstream model still functions.
            tracing::warn!(
                system_kind,
                system_id,
                "HeatPump has no CompressorType and no SEER/EER cooling efficiency; \
                 defaulting to single-speed (1)"
            );
            set_speed_fallback(params, 1);
            return Ok(());
        }
        return Err(HpxmlError::MissingField {
            path,
            system_kind,
            system_id: system_id.to_string(),
            reason: "SEER or EER cooling efficiency is required to infer equipment speed; \
                 provide AnnualCoolingEfficiency with Units='SEER' or 'EER'",
        });
    };

    // Speed thresholds are calibrated to SEER per the reference OCHRE
    // resolver (ochre/utils/hpxml.py:861-876).  Using the same thresholds
    // for EER is a defensible approximation: EER and SEER differ by ≤10-15%
    // for typical single-speed residential equipment.
    let n_speeds = if efficiency > 21.0 {
        4
    } else if efficiency > 15.0 {
        2
    } else {
        1
    };
    set_speed_fallback(params, n_speeds);
    Ok(())
}

/// Write number_of_speeds and speed_control_mode into params (the shared
/// body when a fallback speed is determined or defaulted).
fn set_speed_fallback(params: &mut Map<String, Value>, n_speeds: usize) {
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
            // For variable-speed equipment (n_speeds=4), startup degradation
            // is 0.0: continuous modulation eliminates cycling losses.
            4 => 0.0,
            // Unreachable: n_speeds is always 1, 2, or 4 by construction
            // (validated via compressor_type_to_mode → number_of_speeds_from_mode
            // or set_speed_fallback with SEER/EER thresholds).
            _ => unreachable!(
                "calc_startup_degradation heating called with unexpected n_speeds={}; \
                 only 1, 2, or 4 are valid",
                n_speeds,
            ),
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
            4 => 0.0,
            // Unreachable: same rationale as heating branch above.
            _ => unreachable!(
                "calc_startup_degradation cooling called with unexpected n_speeds={}; \
                 only 1, 2, or 4 are valid",
                n_speeds,
            ),
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

fn apply_multispeed_furnace_parameters(
    params: &mut Map<String, Value>,
    defaults: &DefaultsStore,
    equipment_name: &str,
) {
    let n_speeds = params
        .get("number_of_speeds")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;
    if n_speeds <= 1 {
        return;
    }
    let Some(rated_capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
        return;
    };
    let Some(afue) = params.get("efficiency_afue").and_then(Value::as_f64) else {
        return;
    };
    let Some(multispeed) =
        defaults.hvac_multispeed_parameters(equipment_name, "AFUE", n_speeds, afue * 100.0)
    else {
        return;
    };
    let stage_count = multispeed
        .capacity_ratios
        .len()
        .min(multispeed.cops.len())
        .min(n_speeds);
    for i in 0..stage_count {
        let cap_w = rated_capacity_w * multispeed.capacity_ratios[i];
        params.insert(format!("heating_capacity_w_stage_{i}"), json!(cap_w));
        let cop = multispeed.cops[i].max(1e-6);
        params.insert(format!("heating_eir_stage_{i}"), json!(1.0 / cop));
    }
}

/// OCHRE MinisplitHVAC 10-to-4 speed remap: when the defaults CSV provides
/// exactly 10 entries and the equipment needs 4 speeds, subsample at
/// indices [1, 3, 5, 9] (0-indexed).  Applied to capacity_ratios, COPs,
/// and SHRs identically.
fn remap_minisplit_stages(
    capacity_ratios: &[f64],
    cops: &[f64],
    shrs: &[f64],
    n_speeds: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    const REMAP_INDICES: [usize; 4] = [1, 3, 5, 9];
    if n_speeds == 4 && capacity_ratios.len() == 10 {
        let pick = |src: &[f64]| -> Vec<f64> {
            REMAP_INDICES
                .iter()
                .filter_map(|&i| src.get(i).copied())
                .collect()
        };
        (pick(capacity_ratios), pick(cops), pick(shrs))
    } else {
        (capacity_ratios.to_vec(), cops.to_vec(), shrs.to_vec())
    }
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

    let is_mshp = matches!(equipment_name, "MSHP Heater" | "MSHP Cooler");

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

    let (capacity_ratios, cops, shrs) = if is_mshp {
        remap_minisplit_stages(
            &multispeed.capacity_ratios,
            &multispeed.cops,
            &multispeed.shrs,
            n_speeds,
        )
    } else {
        (
            multispeed.capacity_ratios.clone(),
            multispeed.cops.clone(),
            multispeed.shrs.clone(),
        )
    };

    let stage_count = capacity_ratios.len().min(cops.len()).min(n_speeds);
    if stage_count == 0 {
        return;
    }

    for i in 0..stage_count {
        let cap_w = rated_capacity_w * capacity_ratios[i];
        params.insert(format!("{stage_cap_prefix}_{i}"), json!(cap_w));
        let cop = cops[i].max(1e-6);
        params.insert(format!("{stage_eir_prefix}_{i}"), json!(1.0 / cop));
        if !is_heating && i < shrs.len() {
            params.insert(format!("shr_{i}"), json!(shrs[i]));
        }
    }

    if let Some(curve_set) = curves {
        if let Some(coeff_text) = serialize_stage_plr_coefficients(curve_set, n_speeds) {
            params.insert(
                "eir_plr_coefficients".to_string(),
                Value::String(coeff_text),
            );
        }
        let all_pairs = select_all_curve_pairs(curve_set, n_speeds);
        if let Some(primary) = all_pairs.last() {
            // Emit per-stage biquadratic coefficients as a flat list.
            // The equipment loader splits on 6-element chunks to get per-stage curves.
            // Index 0 in the list = lowest speed stage.
            let cap_flat: Vec<f64> = all_pairs
                .iter()
                .flat_map(|p| p.cap_coeffs.iter().copied())
                .collect();
            let eir_flat: Vec<f64> = all_pairs
                .iter()
                .flat_map(|p| p.eir_coeffs.iter().copied())
                .collect();
            params.insert(
                "capacity_biquadratic_coeffs".to_string(),
                Value::String(format!("{cap_flat:?}")),
            );
            params.insert(
                "eir_biquadratic_coeffs".to_string(),
                Value::String(format!("{eir_flat:?}")),
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
            params.insert(
                "cap_ff_coeffs".to_string(),
                Value::String(format!("{:?}", primary.cap_ff)),
            );
            params.insert(
                "eir_ff_coeffs".to_string(),
                Value::String(format!("{:?}", primary.eir_ff)),
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

struct PrimaryCurvePair {
    cap_coeffs: [f64; 6],
    eir_coeffs: [f64; 6],
    cap_ff: [f64; 3],
    eir_ff: [f64; 3],
    x1_bounds: (f64, f64),
    x2_bounds: (f64, f64),
    ff_bounds: Option<(f64, f64)>,
    plf_bounds: Option<(f64, f64)>,
}

#[cfg(test)]
fn select_primary_curve_pair(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Option<PrimaryCurvePair> {
    select_all_curve_pairs(curve_set, n_speeds)
        .into_iter()
        .last()
}

/// Return all matched curve variants in speed order (lowest speed first).
fn select_all_curve_pairs(
    curve_set: &crate::defaults::HvacCurveSet,
    n_speeds: usize,
) -> Vec<PrimaryCurvePair> {
    let variants = select_variants_for_speed_count(curve_set, n_speeds);
    variants
        .iter()
        .map(|v| PrimaryCurvePair {
            cap_coeffs: v.cap_t.coeffs,
            eir_coeffs: v.eir_t.coeffs,
            cap_ff: v.cap_ff,
            eir_ff: v.eir_ff,
            x1_bounds: v.cap_t.x1_bounds,
            x2_bounds: v.cap_t.x2_bounds,
            ff_bounds: v.ff_bounds,
            plf_bounds: v.plf_bounds,
        })
        .collect()
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
                // Unreachable: n_speeds is always 1, 2, or 4 by construction
                // (see mode_from_number_of_speeds justification above).
                _ => unreachable!(
                    "select_variants_for_speed_count called with unexpected n_speeds={}; \
                     only 1, 2, or 4 are valid",
                    n_speeds,
                ),
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

    let mut heating: Option<([f64; 24], [f64; 24])> = None;
    let mut cooling: Option<([f64; 24], [f64; 24])> = None;
    for (hvac_type, slot) in [("Heating", &mut heating), ("Cooling", &mut cooling)] {
        let weekday = super::xml_helpers::parse_setpoint_from_control(control, hvac_type, true);
        let weekend = super::xml_helpers::parse_setpoint_from_control(control, hvac_type, false);
        if let Some(wd) = weekday {
            let mut weekday_arr = [0.0; 24];
            weekday_arr.copy_from_slice(&wd[..24]);
            let weekend_vec = weekend.unwrap_or_else(|| wd.clone());
            let mut weekend_arr = [0.0; 24];
            weekend_arr.copy_from_slice(&weekend_vec[..24]);
            *slot = Some((weekday_arr, weekend_arr));
        }
    }

    if let (Some((h_wd, h_we)), Some((c_wd, c_we))) = (heating.as_mut(), cooling.as_mut()) {
        let mut reconciliations = Vec::<SetpointReconciliation>::new();
        if let Some(r) = reconcile_setpoint_pair(h_wd, c_wd, "weekday") {
            reconciliations.push(r);
        }
        if let Some(r) = reconcile_setpoint_pair(h_we, c_we, "weekend") {
            reconciliations.push(r);
        }
        if !reconciliations.is_empty() {
            let v = serde_json::to_value(&reconciliations)
                .expect("SetpointReconciliation must serialize");
            out.push(("setpoints_reconciled".to_string(), v));
        }
    }

    if let Some((wd, we)) = heating {
        out.push((
            "heating_setpoint_source".to_string(),
            daily_profile_source(wd, we),
        ));
    }
    if let Some((wd, we)) = cooling {
        out.push((
            "cooling_setpoint_source".to_string(),
            daily_profile_source(wd, we),
        ));
    }

    out
}

/// Clip inverted or too-close heating/cooling setpoint pairs to the daily
/// midpoint with a symmetric offset, matching OCHRE's reconciliation
/// (see `vendors/OCHRE/ochre/utils/schedule.py:617-625`).
///
/// OCHRE enforces a 1 °C minimum separation. HARES's downstream thermostat
/// validator (`ThermalSetpoints::validate_for_deadband`) requires
/// `cooling - heating >= 2 * hysteresis_c`, and the default hysteresis is 1 °C,
/// so the required gap is `max(1.0, 2 * hysteresis) = 2.0 °C`. We use that as
/// the reconciliation gap and clip to `avg ± gap/2` so the post-clip pair
/// always satisfies the thermostat invariant.
const SETPOINT_RECONCILE_GAP_C: f64 = 2.0;

fn reconcile_setpoint_pair(
    heating: &mut [f64; 24],
    cooling: &mut [f64; 24],
    day_label: &str,
) -> Option<SetpointReconciliation> {
    let original_heating = *heating;
    let original_cooling = *cooling;

    let half_gap = 0.5 * SETPOINT_RECONCILE_GAP_C;
    let mut violated_hours = 0_u32;
    let mut worst_inversion_c = 0.0_f64;
    for h in 0..24 {
        let gap = cooling[h] - heating[h];
        if gap < SETPOINT_RECONCILE_GAP_C {
            violated_hours += 1;
            worst_inversion_c = worst_inversion_c.max(-gap);
            let avg = 0.5 * (heating[h] + cooling[h]);
            heating[h] = avg - half_gap;
            cooling[h] = avg + half_gap;
        }
    }
    if violated_hours > 0 {
        tracing::warn!(
            day = day_label,
            violated_hours,
            worst_inversion_c,
            gap_c = SETPOINT_RECONCILE_GAP_C,
            "HPXML heating/cooling setpoints too close or inverted; clipped to midpoint with 2 °C separation"
        );
        Some(SetpointReconciliation {
            day: day_label.to_string(),
            original_heating_c: original_heating,
            original_cooling_c: original_cooling,
            adjusted_heating_c: *heating,
            adjusted_cooling_c: *cooling,
        })
    } else {
        None
    }
}

fn apply_building_setpoint_profiles(
    building: &Building,
    params: &mut Map<String, Value>,
    include_heating: bool,
    include_cooling: bool,
) {
    let mut heating = building.heating_weekday_setpoints_c.as_ref().map(|wd| {
        let mut weekday = [0.0; 24];
        weekday.copy_from_slice(&wd[..24]);
        let weekend = building
            .heating_weekend_setpoints_c
            .as_ref()
            .map(|vals| {
                let mut arr = [0.0; 24];
                arr.copy_from_slice(&vals[..24]);
                arr
            })
            .unwrap_or(weekday);
        (weekday, weekend)
    });
    let mut cooling = building.cooling_weekday_setpoints_c.as_ref().map(|wd| {
        let mut weekday = [0.0; 24];
        weekday.copy_from_slice(&wd[..24]);
        let weekend = building
            .cooling_weekend_setpoints_c
            .as_ref()
            .map(|vals| {
                let mut arr = [0.0; 24];
                arr.copy_from_slice(&vals[..24]);
                arr
            })
            .unwrap_or(weekday);
        (weekday, weekend)
    });

    if let (Some((h_wd, h_we)), Some((c_wd, c_we))) = (heating.as_mut(), cooling.as_mut()) {
        let mut reconciliations = Vec::<SetpointReconciliation>::new();
        if let Some(r) = reconcile_setpoint_pair(h_wd, c_wd, "weekday") {
            reconciliations.push(r);
        }
        if let Some(r) = reconcile_setpoint_pair(h_we, c_we, "weekend") {
            reconciliations.push(r);
        }
        if !reconciliations.is_empty() {
            let v = serde_json::to_value(&reconciliations)
                .expect("SetpointReconciliation must serialize");
            params.insert("setpoints_reconciled".to_string(), v);
        }
    }

    if include_heating {
        if let Some((weekday, weekend)) = heating {
            params.insert(
                "heating_setpoint_source".to_string(),
                daily_profile_source(weekday, weekend),
            );
        }
    }
    if include_cooling {
        if let Some((weekday, weekend)) = cooling {
            params.insert(
                "cooling_setpoint_source".to_string(),
                daily_profile_source(weekday, weekend),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::building::{DuctSystem, DuctType, Site, XmlNode, Zone};
    use super::*;
    use crate::hpxml::parse_xml_document;
    use hares_physics::units as conv;
    use hares_types::{BoundaryPolicy, HumidityAccumulator, ScheduleSourceConfig};
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;
        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    struct SharedWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("writer lock poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_warnings<F>(f: F) -> String
    where
        F: FnOnce(),
    {
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(buffer.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(buffer.lock().expect("log lock poisoned").clone())
            .expect("logs must be valid utf8")
    }

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
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            // Reserved for Phase 2 autosizing: will hold the building-level design
            // heating/cooling load once Manual J/S autosizing is implemented.
            // Currently always None — autosizing computes per-equipment capacity
            // in hares-core::dwelling::autosize rather than at the HPXML parse layer.
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
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        }
    }

    fn building_with_setpoint_profiles(
        heating_weekday: [f64; 24],
        heating_weekend: [f64; 24],
        cooling_weekday: [f64; 24],
        cooling_weekend: [f64; 24],
    ) -> Building {
        let mut building = empty_building(vec![conditioned_zone()]);
        building.heating_weekday_setpoints_c = Some(heating_weekday.to_vec());
        building.heating_weekend_setpoints_c = Some(heating_weekend.to_vec());
        building.cooling_weekday_setpoints_c = Some(cooling_weekday.to_vec());
        building.cooling_weekend_setpoints_c = Some(cooling_weekend.to_vec());
        building
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
        let mut building = empty_building(vec![unconditioned_zone(vec![
            duct(DuctType::Supply, 10.0, 8.0),
            duct(DuctType::Supply, 10.0, 4.0),
        ])]);
        building.site.latitude_deg = Some(40.0);
        building.site.longitude_deg = Some(-105.0);
        building.conditioned_volume_m3 = Some(400.0);
        let params = compute_duct_dse_params(&building)
            .expect("duct DSE params must resolve when lat/lon/volume are set");
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
        let mut building = empty_building(vec![unconditioned_zone(vec![
            duct(DuctType::Return, 20.0, 3.0),
            duct(DuctType::Return, 10.0, 9.0),
        ])]);
        building.site.latitude_deg = Some(40.0);
        building.site.longitude_deg = Some(-105.0);
        building.conditioned_volume_m3 = Some(400.0);
        let params = compute_duct_dse_params(&building)
            .expect("duct DSE params must resolve when lat/lon/volume are set");
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
                    warn_on_clamp: false,
                },
                cap_ff: [1.0, 0.0, 0.0],
                eir_t: hares_physics::biquadratic::BiquadraticCurve {
                    coeffs: [1.1, 0.0, 0.0, 0.0, 0.0, 0.0],
                    x1_bounds: (12.0, 24.0),
                    x2_bounds: (18.0, 50.0),
                    warn_on_clamp: false,
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

    #[test]
    fn four_speed_emits_per_stage_biquadratic_curves() {
        let make_variant = |name: &str, cap0: f64, eir0: f64| crate::defaults::HvacCurveVariant {
            name: name.to_string(),
            cap_t: hares_physics::biquadratic::BiquadraticCurve {
                coeffs: [cap0, 0.0, 0.0, 0.0, 0.0, 0.0],
                x1_bounds: (12.0, 24.0),
                x2_bounds: (18.0, 50.0),
                warn_on_clamp: false,
            },
            cap_ff: [1.0, 0.0, 0.0],
            eir_t: hares_physics::biquadratic::BiquadraticCurve {
                coeffs: [eir0, 0.0, 0.0, 0.0, 0.0, 0.0],
                x1_bounds: (12.0, 24.0),
                x2_bounds: (18.0, 50.0),
                warn_on_clamp: false,
            },
            eir_ff: [1.0, 0.0, 0.0],
            eir_plr: [1.0, 0.0, 0.0],
            ff_bounds: None,
            plf_bounds: None,
        };
        let curve_set = crate::defaults::HvacCurveSet {
            variants: vec![
                make_variant("Variable_1", 0.8, 1.2),
                make_variant("Variable_2", 0.9, 1.1),
                make_variant("Variable_3", 1.0, 1.0),
                make_variant("Variable_4", 1.1, 0.9),
            ],
        };
        let all = select_all_curve_pairs(&curve_set, 4);
        assert_eq!(all.len(), 4, "should return 4 curve pairs for 4-speed unit");
        // Lowest speed first.
        assert!((all[0].cap_coeffs[0] - 0.8).abs() < 1e-12);
        assert!((all[1].cap_coeffs[0] - 0.9).abs() < 1e-12);
        assert!((all[2].cap_coeffs[0] - 1.0).abs() < 1e-12);
        assert!((all[3].cap_coeffs[0] - 1.1).abs() < 1e-12);
        // EIR curves in order too.
        assert!((all[0].eir_coeffs[0] - 1.2).abs() < 1e-12);
        assert!((all[3].eir_coeffs[0] - 0.9).abs() < 1e-12);
    }

    #[test]
    fn select_primary_curve_pair_carries_ff_coefficients() {
        let curve_set = crate::defaults::HvacCurveSet {
            variants: vec![crate::defaults::HvacCurveVariant {
                name: "Variable_1".to_string(),
                cap_t: hares_physics::biquadratic::BiquadraticCurve {
                    coeffs: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    x1_bounds: (12.0, 24.0),
                    x2_bounds: (18.0, 50.0),
                    warn_on_clamp: false,
                },
                cap_ff: [0.7, 0.4, -0.1],
                eir_t: hares_physics::biquadratic::BiquadraticCurve {
                    coeffs: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    x1_bounds: (12.0, 24.0),
                    x2_bounds: (18.0, 50.0),
                    warn_on_clamp: false,
                },
                eir_ff: [1.3, -0.5, 0.2],
                eir_plr: [1.0, 0.0, 0.0],
                ff_bounds: None,
                plf_bounds: None,
            }],
        };
        let selected = select_primary_curve_pair(&curve_set, 4).expect("should find pair");
        assert_eq!(selected.cap_ff, [0.7, 0.4, -0.1]);
        assert_eq!(selected.eir_ff, [1.3, -0.5, 0.2]);
    }

    // -----------------------------------------------------------------------
    // zone_type_to_ashrae152_str -- foundation type granularity
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
            foundation_depth_m: None,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
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
            foundation_depth_m: None,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
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
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
            "attic_vented"
        );
    }

    #[test]
    fn attic_unvented_maps_correctly() {
        let zone = attic_zone(false);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
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
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
            "vent_unins_crawlspace"
        );
    }

    #[test]
    fn foundation_no_name_falls_back_to_vented_uninsulated() {
        let zone = foundation_zone(true);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
            "vent_unins_crawlspace"
        );
    }

    #[test]
    fn foundation_no_name_unvented_falls_back() {
        let zone = foundation_zone(false);
        let building = building_with(None, vec![]);
        assert_eq!(
            zone_type_to_ashrae152_str(&zone, &building).expect("valid zone type"),
            "unvent_unins_crawlspace"
        );
    }

    // Regression test: zone_type_to_ashrae152_str must reject zone types
    // that have no ASHRAE 152 duct-derating key (Outdoor, Ground, Adjacent,
    // and unrecognised Other variants).  Previously these fell through a
    // `_ => "attic_vented".into()` catch-all, silently routing non-attic
    // zones through attic-specific efficiency parameters.
    #[test]
    fn non_attic_zone_types_must_not_return_attic_vented() {
        let building = building_with(None, vec![]);

        for zone_type in [
            ZoneType::Outdoor,
            ZoneType::Ground,
            ZoneType::Adjacent,
            ZoneType::Other("Unknown".to_string()),
        ] {
            let zone = Zone {
                zone_type,
                floor_area_m2: None,
                volume_m3: None,
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            };
            let result = zone_type_to_ashrae152_str(&zone, &building);
            assert!(
                result.is_err(),
                "zone_type {:?} must not succeed; it has no ASHRAE 152 zone-type key",
                zone.zone_type
            );
        }
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
            .expect("gas furnace builder must not error with valid params")
            .expect("gas furnace builder must produce a typed config when capacity is present");
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
    fn gas_furnace_builder_uses_explicit_heating_airflow() {
        let mut params = minimal_furnace_params(0.96, 24_000.0);
        params.insert("airflow_defect_ratio".to_string(), json!(-0.25));
        params.insert("heating_airflow_cfm".to_string(), json!(1_200.0));

        let duct_params = DuctDseParams {
            zone_id: Some(1),
            zone_type: Some("attic_unvented".to_string()),
            house_volume_m3: 400.0,
            supply_leakage_frac: 0.08,
            supply_area_m2: 20.0,
            supply_r_m2_k_w: 0.5,
            return_leakage_frac: 0.05,
            return_area_m2: 12.0,
            return_r_m2_k_w: 0.5,
            latitude_deg: 40.0,
            longitude_deg: -105.0,
        };
        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &duct_params)
            .expect("gas furnace builder must not error with explicit airflow")
            .expect("gas furnace builder must produce a typed config when capacity is present");
        use hares_equipment::hvac::heating_config::GasFurnaceConfig;
        let cfg: GasFurnaceConfig = ec.typed().expect("must deserialize to GasFurnaceConfig");
        let expected = 1_200.0 * CFM_TO_M3_S / 24_000.0;
        assert!(
            (cfg.ducts.airflow_m3_s_per_w.expect("airflow") - expected).abs() < 1e-12,
            "explicit heating airflow must be converted from CFM to SI and ignore defect ratio"
        );
        assert!(
            cfg.ducts.dse_heat.is_some(),
            "duct DSE must still be computed"
        );
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
    fn airflow_defect_ratio_uses_install_quality_delta_semantics() {
        let mut params = minimal_central_ac_params(16.0, 12_000.0);
        params.insert("airflow_defect_ratio".to_string(), json!(0.0));
        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("central AC builder must succeed");
        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        let cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("must deserialize to CentralAirConditionerConfig");
        let nominal = 400.0_f64 * CFM_TO_M3_S / W_PER_TON;
        assert!((cfg.airflow_m3_s_per_w.expect("airflow") - nominal).abs() < 1e-12);

        params.insert("airflow_defect_ratio".to_string(), json!(-0.25));
        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("central AC builder must succeed");
        let cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("must deserialize to CentralAirConditionerConfig");
        let expected = nominal * 0.75;
        assert!((cfg.airflow_m3_s_per_w.expect("airflow") - expected).abs() < 1e-12);
    }

    #[test]
    fn explicit_cooling_airflow_cfm_overrides_defect_ratio_scaling() {
        let mut params = minimal_central_ac_params(16.0, 12_000.0);
        params.insert("airflow_defect_ratio".to_string(), json!(-0.25));
        params.insert("cooling_airflow_cfm".to_string(), json!(1_500.0));
        let ec = try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default())
            .expect("central AC builder must succeed");
        use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
        let cfg: CentralAirConditionerConfig = ec
            .typed()
            .expect("must deserialize to CentralAirConditionerConfig");
        let expected = 1_500.0 * CFM_TO_M3_S / 12_000.0;
        assert!((cfg.airflow_m3_s_per_w.expect("airflow") - expected).abs() < 1e-12);
    }

    #[test]
    fn parse_hvac_setpoint_params_reads_realistic_control_xml() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <HVACPlant>
                    <HVACControl>
                      <extension>
                        <WeekdaySetpointTempsHeatingSeason>
                          66,66,66,66,66,66,68,70,70,70,70,70,70,70,70,70,70,68,66,66,66,66,66,66
                        </WeekdaySetpointTempsHeatingSeason>
                        <WeekendSetpointTempsHeatingSeason>
                          66,66,66,66,66,66,67,67,69,69,69,69,69,69,69,69,69,69,66,66,66,66,66,66
                        </WeekendSetpointTempsHeatingSeason>
                        <WeekdaySetpointTempsCoolingSeason>
                          80,80,80,80,80,80,78,76,76,76,76,76,76,76,76,76,76,76,78,80,80,80,80,80
                        </WeekdaySetpointTempsCoolingSeason>
                        <WeekendSetpointTempsCoolingSeason>
                          80,80,80,80,80,80,79,79,77,77,77,77,77,77,77,77,77,77,80,80,80,80,80,80
                        </WeekendSetpointTempsCoolingSeason>
                      </extension>
                    </HVACControl>
                  </HVACPlant>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details node must exist");

        let params = parse_hvac_setpoint_params(details);
        let mut map = Map::new();
        for (key, value) in params {
            map.insert(key, value);
        }

        let source: ScheduleSourceConfig = serde_json::from_value(
            map.get("heating_setpoint_source")
                .cloned()
                .expect("heating_setpoint_source must be present"),
        )
        .expect("heating_setpoint_source must deserialize");

        match source {
            ScheduleSourceConfig::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => {
                assert!((weekday[0] - conv::temperature_f_to_c(66.0)).abs() < 1e-9);
                assert!((weekday[8] - conv::temperature_f_to_c(70.0)).abs() < 1e-9);
                assert!((weekend[6] - conv::temperature_f_to_c(67.0)).abs() < 1e-9);
                assert_eq!(month_multipliers, [1.0; 12]);
                assert_eq!(max_value, 1.0);
            }
            other => panic!("expected DailyProfile source, got {other:?}"),
        }
    }

    #[test]
    fn parse_hvac_setpoint_params_reconciles_inverted_setpoints() {
        // Inverted setpoints: heating=72°F (22.2 °C), cooling=68°F (20.0 °C).
        // Reconciler must clip to midpoint ± 1 °C so cooling − heating ≥ 2 °C
        // (matches thermostat validate_for_deadband with default 1 °C hysteresis).
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <HVACPlant>
                    <HVACControl>
                      <extension>
                        <WeekdaySetpointTempsHeatingSeason>
                          72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72
                        </WeekdaySetpointTempsHeatingSeason>
                        <WeekendSetpointTempsHeatingSeason>
                          72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72
                        </WeekendSetpointTempsHeatingSeason>
                        <WeekdaySetpointTempsCoolingSeason>
                          68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68
                        </WeekdaySetpointTempsCoolingSeason>
                        <WeekendSetpointTempsCoolingSeason>
                          68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68,68
                        </WeekendSetpointTempsCoolingSeason>
                      </extension>
                    </HVACControl>
                  </HVACPlant>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details node must exist");

        let params = parse_hvac_setpoint_params(details);
        let mut map = Map::new();
        for (key, value) in params {
            map.insert(key, value);
        }

        let heating_source: ScheduleSourceConfig = serde_json::from_value(
            map.get("heating_setpoint_source")
                .cloned()
                .expect("heating_setpoint_source must be present"),
        )
        .expect("heating_setpoint_source must deserialize");
        let cooling_source: ScheduleSourceConfig = serde_json::from_value(
            map.get("cooling_setpoint_source")
                .cloned()
                .expect("cooling_setpoint_source must be present"),
        )
        .expect("cooling_setpoint_source must deserialize");

        let (heating_weekday, heating_weekend) = match heating_source {
            ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            } => (weekday, weekend),
            other => panic!("expected heating DailyProfile source, got {other:?}"),
        };
        let (cooling_weekday, cooling_weekend) = match cooling_source {
            ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            } => (weekday, weekend),
            other => panic!("expected cooling DailyProfile source, got {other:?}"),
        };

        // Midpoint of 72 °F (22.222 °C) and 68 °F (20.0 °C) is 21.111 °C.
        // After reconciliation with a 2 °C gap: heating = midpoint − 1, cooling = midpoint + 1.
        let expected_midpoint_c =
            0.5 * (conv::temperature_f_to_c(72.0) + conv::temperature_f_to_c(68.0));
        let expected_heating_c = expected_midpoint_c - 1.0;
        let expected_cooling_c = expected_midpoint_c + 1.0;

        for h in 0..24 {
            assert!(
                (heating_weekday[h] - expected_heating_c).abs() < 1e-9,
                "weekday hour {h}: heating={} expected={}",
                heating_weekday[h],
                expected_heating_c,
            );
            assert!(
                (cooling_weekday[h] - expected_cooling_c).abs() < 1e-9,
                "weekday hour {h}: cooling={} expected={}",
                cooling_weekday[h],
                expected_cooling_c,
            );
            assert!(
                (heating_weekend[h] - expected_heating_c).abs() < 1e-9,
                "weekend hour {h}: heating={} expected={}",
                heating_weekend[h],
                expected_heating_c,
            );
            assert!(
                (cooling_weekend[h] - expected_cooling_c).abs() < 1e-9,
                "weekend hour {h}: cooling={} expected={}",
                cooling_weekend[h],
                expected_cooling_c,
            );
            assert!(
                cooling_weekday[h] - heating_weekday[h] >= 2.0 - 1e-9,
                "weekday hour {h}: gap < 2 °C",
            );
            assert!(
                cooling_weekend[h] - heating_weekend[h] >= 2.0 - 1e-9,
                "weekend hour {h}: gap < 2 °C",
            );
        }
    }

    #[test]
    fn apply_building_setpoint_profiles_reconciles_inverted_setpoints() {
        // Swapped: heating=22 °C, cooling=20 °C -- must reconcile to midpoint ± 1 °C.
        let heating_weekday = [22.0f64; 24];
        let heating_weekend = [22.0f64; 24];
        let cooling_weekday = [20.0f64; 24];
        let cooling_weekend = [20.0f64; 24];
        let building = building_with_setpoint_profiles(
            heating_weekday,
            heating_weekend,
            cooling_weekday,
            cooling_weekend,
        );

        let mut params = Map::new();
        apply_building_setpoint_profiles(&building, &mut params, true, true);

        let heating_source: ScheduleSourceConfig = serde_json::from_value(
            params
                .get("heating_setpoint_source")
                .cloned()
                .expect("heating_setpoint_source must be present"),
        )
        .expect("heating_setpoint_source must deserialize");
        let cooling_source: ScheduleSourceConfig = serde_json::from_value(
            params
                .get("cooling_setpoint_source")
                .cloned()
                .expect("cooling_setpoint_source must be present"),
        )
        .expect("cooling_setpoint_source must deserialize");

        if let ScheduleSourceConfig::DailyProfile {
            weekday: h_wd,
            weekend: h_we,
            ..
        } = heating_source
        {
            if let ScheduleSourceConfig::DailyProfile {
                weekday: c_wd,
                weekend: c_we,
                ..
            } = cooling_source
            {
                for h in 0..24 {
                    assert!(
                        c_wd[h] - h_wd[h] >= 2.0 - 1e-9,
                        "weekday hour {h}: cooling − heating < 2 °C",
                    );
                    assert!(
                        c_we[h] - h_we[h] >= 2.0 - 1e-9,
                        "weekend hour {h}: cooling − heating < 2 °C",
                    );
                    // Midpoint preserved.
                    assert!((0.5 * (h_wd[h] + c_wd[h]) - 21.0).abs() < 1e-9);
                    assert!((0.5 * (h_we[h] + c_we[h]) - 21.0).abs() < 1e-9);
                    assert!(h_wd[h] <= c_wd[h] - 1.0);
                    assert!(h_we[h] <= c_we[h] - 1.0);
                }
            } else {
                panic!("expected cooling DailyProfile source");
            }
        } else {
            panic!("expected heating DailyProfile source");
        }
    }

    #[test]
    fn building_setpoint_profiles_are_emitted_as_typed_sources() {
        let heating_weekday = [20.0f64; 24];
        let heating_weekend = [19.0f64; 24];
        let cooling_weekday = [26.0f64; 24];
        let cooling_weekend = [25.0f64; 24];
        let building = building_with_setpoint_profiles(
            heating_weekday,
            heating_weekend,
            cooling_weekday,
            cooling_weekend,
        );

        let mut params = Map::new();
        apply_building_setpoint_profiles(&building, &mut params, true, true);

        let heating_source: ScheduleSourceConfig = serde_json::from_value(
            params
                .get("heating_setpoint_source")
                .cloned()
                .expect("heating_setpoint_source must be present"),
        )
        .expect("heating_setpoint_source must deserialize");
        let cooling_source: ScheduleSourceConfig = serde_json::from_value(
            params
                .get("cooling_setpoint_source")
                .cloned()
                .expect("cooling_setpoint_source must be present"),
        )
        .expect("cooling_setpoint_source must deserialize");

        match heating_source {
            ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            } => {
                assert_eq!(weekday[0], 20.0);
                assert_eq!(weekend[0], 19.0);
            }
            other => panic!("expected heating DailyProfile source, got {other:?}"),
        }
        match cooling_source {
            ScheduleSourceConfig::DailyProfile {
                weekday, weekend, ..
            } => {
                assert_eq!(weekday[0], 26.0);
                assert_eq!(weekend[0], 25.0);
            }
            other => panic!("expected cooling DailyProfile source, got {other:?}"),
        }
        assert!(!params.contains_key("heating_weekday_setpoints_c"));
        assert!(!params.contains_key("cooling_weekday_setpoints_c"));
    }

    #[test]
    fn room_ac_builder_returns_none_when_only_seer_available() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(3_500.0));
        params.insert("efficiency_seer".to_string(), json!(12.0));
        // No EER present -- must return None rather than silently substituting SEER.
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
    fn room_ac_builder_propagates_shr_from_params() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(3_500.0));
        params.insert("efficiency_eer".to_string(), json!(10.5));
        params.insert("shr".to_string(), json!(0.82));
        let ec = try_build_room_ac_config("Room AC", &params)
            .expect("room AC builder must succeed when EER and SHR are present");
        use hares_equipment::hvac::cooling_config::RoomAcConfig;
        let cfg: RoomAcConfig = ec.typed().expect("must deserialize to RoomAcConfig");
        assert_eq!(
            cfg.shr,
            Some(0.82),
            "shr should be Some(0.82) when params[\"shr\"] = 0.82, got {:?}",
            cfg.shr
        );
    }

    #[test]
    fn room_ac_builder_shr_is_none_when_not_in_params() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(3_500.0));
        params.insert("efficiency_eer".to_string(), json!(10.5));
        // No "shr" key in params — SensibleHeatFraction absent from HPXML element.
        let ec = try_build_room_ac_config("Room AC", &params)
            .expect("room AC builder must succeed without SHR");
        use hares_equipment::hvac::cooling_config::RoomAcConfig;
        let cfg: RoomAcConfig = ec.typed().expect("must deserialize to RoomAcConfig");
        assert_eq!(
            cfg.shr, None,
            "shr should be None when SensibleHeatFraction is absent"
        );
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
        assert!(cfg.common.is_mini_split);
        assert_eq!(cfg.common.number_of_speeds, 4);
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
        assert_eq!(cfg.common.number_of_speeds, 2);
        assert_eq!(cfg.stage_shrs, Some(vec![0.81, 0.74]));
    }

    #[test]
    fn heat_pump_heater_builder_propagates_lockout_fields() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(10_000.0));
        params.insert("efficiency_hspf".to_string(), json!(8.5));
        params.insert("hp_lockout_temp_c".to_string(), json!(-12.0));
        params.insert("er_lockout_temp_c".to_string(), json!(2.0));
        params.insert("max_oat_supplemental_c".to_string(), json!(18.0));
        params.insert("er_setpoint_offset_c".to_string(), json!(1.4));
        params.insert("er_hard_lockout_time_s".to_string(), json!(600.0));

        let ec = try_build_heat_pump_heater_config(
            "ASHP Heater",
            &params,
            &DuctDseParams::default(),
            false,
        )
        .expect("ASHP heater typed config should be built");

        use hares_equipment::hvac::heat_pump_config::HeatPumpHeaterConfig;
        let cfg: HeatPumpHeaterConfig = ec.typed().expect("typed heater config");
        assert_eq!(cfg.hp_lockout_temp_c, Some(-12.0));
        assert_eq!(cfg.er_lockout_temp_c, Some(2.0));
        assert_eq!(cfg.max_oat_supplemental_c, Some(18.0));
        assert_eq!(cfg.er_setpoint_offset_c, Some(1.4));
        assert_eq!(cfg.er_hard_lockout_time_s, Some(600.0));
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
    fn ideal_hvac_builder_preserves_heating_only_thermostat_source() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(9_000.0));
        params.insert(
            "heating_setpoint_source".to_string(),
            daily_profile_source([19.5; 24], [19.5; 24]),
        );

        let ec = try_build_ideal_hvac_config("Ideal HVAC", &params)
            .expect("Ideal HVAC typed config should be built");

        use hares_equipment::hvac::heating_config::IdealHvacConfig;
        let cfg: IdealHvacConfig = ec.typed().expect("typed ideal config");
        assert_eq!(cfg.heating_setpoint_c, Some(19.5));
        assert_eq!(
            cfg.heating_setpoint_source,
            Some(ScheduleSourceConfig::DailyProfile {
                weekday: [19.5; 24],
                weekend: [19.5; 24],
                month_multipliers: [1.0; 12],
                max_value: 1.0,
            })
        );
        assert_eq!(cfg.cooling_setpoint_c, None);
        assert_eq!(cfg.cooling_setpoint_source, None);
    }

    #[test]
    fn ideal_hvac_builder_parses_daily_profile_setpoint_source() {
        let building =
            building_with_setpoint_profiles([20.0; 24], [19.0; 24], [26.0; 24], [25.0; 24]);
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(9_000.0));
        apply_building_setpoint_profiles(&building, &mut params, true, true);
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
        assert_eq!(cfg.cooling_setpoint_c, Some(26.0));
    }

    #[test]
    fn central_ac_builder_parses_column_ref_setpoint_source() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_seer".to_string(), json!(16.0));
        params.insert(
            "heating_setpoint_source".to_string(),
            setpoint_source_value(ScheduleSourceConfig::ColumnRef {
                col_idx: 3,
                boundary: BoundaryPolicy::Clamp,
            }),
        );
        params.insert(
            "cooling_setpoint_source".to_string(),
            setpoint_source_value(ScheduleSourceConfig::ColumnRef {
                col_idx: 4,
                boundary: BoundaryPolicy::Clamp,
            }),
        );
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
    // try_build_electric_baseboard_config
    // -----------------------------------------------------------------------

    #[test]
    fn electric_baseboard_builder_populates_zone_id_from_params() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(3_000.0));
        params.insert("heating_efficiency".to_string(), json!(1.0));
        params.insert("zone_id".to_string(), json!(1u16));

        let ec = try_build_electric_baseboard_config("Electric Baseboard", &params)
            .expect("builder must succeed with capacity and zone_id")
            .expect("typed config must be present");
        use hares_equipment::hvac::heating_config::ElectricBaseboardConfig;
        let cfg: ElectricBaseboardConfig = ec
            .typed()
            .expect("must deserialize to ElectricBaseboardConfig");
        assert_eq!(
            cfg.zone_id,
            Some(1),
            "zone_id must be populated from params"
        );
        assert!((cfg.capacity_w - 3_000.0).abs() < 1e-9);
        assert!((cfg.eir - 1.0).abs() < 1e-9);
    }

    #[test]
    fn electric_baseboard_builder_zone_id_none_when_param_absent() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(3_000.0));
        params.insert("heating_efficiency".to_string(), json!(1.0));

        let ec = try_build_electric_baseboard_config("Electric Baseboard", &params)
            .expect("builder must succeed with capacity only")
            .expect("typed config must be present");
        use hares_equipment::hvac::heating_config::ElectricBaseboardConfig;
        let cfg: ElectricBaseboardConfig = ec
            .typed()
            .expect("must deserialize to ElectricBaseboardConfig");
        assert_eq!(
            cfg.zone_id, None,
            "zone_id must be None when param is absent"
        );
    }

    #[test]
    fn conditioned_zone_id_returns_correct_zone_id() {
        let b = empty_building(vec![conditioned_zone(), foundation_zone(false)]);
        assert_eq!(
            conditioned_zone_id(&b),
            Some(1),
            "Conditioned zone at index 0 → ZoneId 1"
        );
    }

    #[test]
    fn conditioned_zone_id_returns_none_when_no_conditioned_zone() {
        let b = empty_building(vec![foundation_zone(false)]);
        assert_eq!(conditioned_zone_id(&b), None);
    }

    // -----------------------------------------------------------------------
    // Setpoint reconciliation machine-readable signal
    // -----------------------------------------------------------------------

    /// When heating/cooling setpoints are too close (gap < 2 °C), the
    /// reconciler widens them to satisfy the thermostat deadband invariant and
    /// emits a machine-readable `SetpointReconciliation` record under the
    /// `"setpoints_reconciled"` key in the params map so downstream consumers
    /// can detect and report the mutation.
    #[test]
    fn reconcile_setpoint_pair_narrow_gap_emits_reconciliation_record() {
        // Heating 21 °C, cooling 22 °C — gap is 1 °C, below the 2 °C minimum.
        let building = building_with_setpoint_profiles(
            [21.0f64; 24],
            [21.0f64; 24],
            [22.0f64; 24],
            [22.0f64; 24],
        );

        let mut params = Map::new();
        apply_building_setpoint_profiles(&building, &mut params, true, true);

        let reconciliations = params
            .get("setpoints_reconciled")
            .expect("setpoints_reconciled key must be present after reconciliation");

        let arr = reconciliations
            .as_array()
            .expect("setpoints_reconciled must be an array");

        assert!(!arr.is_empty(), "reconciliations array must be non-empty");

        // Both weekday and weekend had the same 1 °C gap, so both should be
        // reconciled.
        assert_eq!(
            arr.len(),
            2,
            "expected weekday and weekend reconciliation records"
        );

        let rec = &arr[0];
        assert_eq!(rec["day"].as_str().expect("day must be string"), "weekday");
        let original_h: Vec<f64> = rec["original_heating_c"]
            .as_array()
            .expect("original_heating_c must be array")
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let original_c: Vec<f64> = rec["original_cooling_c"]
            .as_array()
            .expect("original_cooling_c must be array")
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let adjusted_h: Vec<f64> = rec["adjusted_heating_c"]
            .as_array()
            .expect("adjusted_heating_c must be array")
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let adjusted_c: Vec<f64> = rec["adjusted_cooling_c"]
            .as_array()
            .expect("adjusted_cooling_c must be array")
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();

        // Original values preserved in record.
        assert!(original_h.iter().all(|&v| (v - 21.0).abs() < 1e-9));
        assert!(original_c.iter().all(|&v| (v - 22.0).abs() < 1e-9));

        // Adjusted values widened to midpoint ± 1 °C.
        let expected_midpoint = 0.5 * (21.0 + 22.0); // 21.5 °C
        let expected_heating = expected_midpoint - 1.0; // 20.5 °C
        let expected_cooling = expected_midpoint + 1.0; // 22.5 °C
        assert!(
            adjusted_h
                .iter()
                .all(|&v| (v - expected_heating).abs() < 1e-9)
        );
        assert!(
            adjusted_c
                .iter()
                .all(|&v| (v - expected_cooling).abs() < 1e-9)
        );

        // Post-reconciliation gap is ≥ 2 °C for every hour.
        for h in 0..24 {
            assert!(adjusted_c[h] - adjusted_h[h] >= 2.0 - 1e-9);
        }
    }

    #[test]
    fn reconcile_setpoint_pair_wide_gap_produces_no_reconciliation_key() {
        // Heating 18 °C, cooling 26 °C — gap is 8 °C, well above 2 °C minimum.
        let building = building_with_setpoint_profiles(
            [18.0f64; 24],
            [18.0f64; 24],
            [26.0f64; 24],
            [26.0f64; 24],
        );

        let mut params = Map::new();
        apply_building_setpoint_profiles(&building, &mut params, true, true);

        assert!(
            !params.contains_key("setpoints_reconciled"),
            "setpoints_reconciled key must NOT be present when no reconciliation occurred"
        );
    }

    #[test]
    fn parse_hvac_setpoint_params_narrow_gap_emits_reconciliation_record() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <HVACPlant>
                    <HVACControl>
                      <extension>
                        <WeekdaySetpointTempsHeatingSeason>
                          70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70
                        </WeekdaySetpointTempsHeatingSeason>
                        <WeekendSetpointTempsHeatingSeason>
                          70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70,70
                        </WeekendSetpointTempsHeatingSeason>
                        <WeekdaySetpointTempsCoolingSeason>
                          72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72
                        </WeekdaySetpointTempsCoolingSeason>
                        <WeekendSetpointTempsCoolingSeason>
                          72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72,72
                        </WeekendSetpointTempsCoolingSeason>
                      </extension>
                    </HVACControl>
                  </HVACPlant>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details node must exist");

        let params = parse_hvac_setpoint_params(details);

        // Find the setpoints_reconciled entry.
        let key_found = params.iter().any(|(k, _)| k == "setpoints_reconciled");
        assert!(
            key_found,
            "setpoints_reconciled key must be present in output"
        );

        for (k, v) in &params {
            if k == "setpoints_reconciled" {
                let arr = v.as_array().expect("setpoints_reconciled must be an array");
                assert!(!arr.is_empty(), "reconciliations array must be non-empty");
                // 70 °F → 21.11 °C, 72 °F → 22.22 °C, gap ≈ 1.11 °C < 2 °C
                // so both weekday and weekend should be reconciled.
                assert_eq!(arr.len(), 2);
                // Verify record fields exist.
                for rec in arr {
                    assert!(rec["original_heating_c"].is_array());
                    assert!(rec["original_cooling_c"].is_array());
                    assert!(rec["adjusted_heating_c"].is_array());
                    assert!(rec["adjusted_cooling_c"].is_array());
                }
                break;
            }
        }
    }

    #[test]
    fn setpoints_reconciled_propagates_to_typed_config() {
        // Build params with setpoints_reconciled recorded by HPXML parsing,
        // then verify the typed EquipmentConfig carries them.
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_afue".to_string(), json!(0.96));
        params.insert("fan_power_w".to_string(), json!(0.0));
        params.insert(
            "setpoints_reconciled".to_string(),
            serde_json::to_value(vec![
                SetpointReconciliation {
                    day: "weekday".to_string(),
                    original_heating_c: [21.0; 24],
                    original_cooling_c: [22.0; 24],
                    adjusted_heating_c: [20.5; 24],
                    adjusted_cooling_c: [22.5; 24],
                },
                SetpointReconciliation {
                    day: "weekend".to_string(),
                    original_heating_c: [20.0; 24],
                    original_cooling_c: [22.0; 24],
                    adjusted_heating_c: [20.0; 24],
                    adjusted_cooling_c: [22.0; 24],
                },
            ])
            .expect("SetpointReconciliation must serialize"),
        );

        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &DuctDseParams::default())
            .expect("builder must not error")
            .expect("builder must produce a typed config");

        let reconciliations = ec
            .setpoints_reconciled
            .as_ref()
            .expect("setpoints_reconciled must be present");
        assert_eq!(reconciliations.len(), 2);
        assert_eq!(reconciliations[0].day, "weekday");
        assert_eq!(reconciliations[0].original_heating_c, [21.0; 24]);
        assert_eq!(reconciliations[0].original_cooling_c, [22.0; 24]);
        assert_eq!(reconciliations[0].adjusted_heating_c, [20.5; 24]);
        assert_eq!(reconciliations[0].adjusted_cooling_c, [22.5; 24]);
        assert_eq!(reconciliations[1].day, "weekend");
    }

    #[test]
    fn setpoints_reconciled_none_when_not_present_in_params() {
        // No setpoints_reconciled key in params -> EquipmentConfig field is None.
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(12_000.0));
        params.insert("efficiency_afue".to_string(), json!(0.96));
        params.insert("fan_power_w".to_string(), json!(0.0));

        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &DuctDseParams::default())
            .expect("builder must not error")
            .expect("builder must produce a typed config");

        assert!(
            ec.setpoints_reconciled.is_none(),
            "setpoints_reconciled must be None when params have no key"
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
            .expect("try_build_gas_furnace_config must not error with AFUE and capacity")
            .expect("try_build_gas_furnace_config must produce a typed config");

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
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
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

    #[test]
    fn fan_power_from_params_falls_back_to_auxiliary_power_w() {
        let mut params = Map::new();
        params.insert("auxiliary_power_w".to_string(), json!(250.0));

        let result = fan_power_from_params(&params);
        assert_eq!(
            result,
            Some(250.0),
            "auxiliary_power_w must be returned when fan_power_w is absent"
        );
    }

    #[test]
    fn fan_power_from_params_prefers_fan_power_w_over_auxiliary() {
        let mut params = Map::new();
        params.insert("fan_power_w".to_string(), json!(300.0));
        params.insert("auxiliary_power_w".to_string(), json!(250.0));

        let result = fan_power_from_params(&params);
        assert_eq!(
            result,
            Some(300.0),
            "fan_power_w must take precedence over auxiliary_power_w"
        );
    }

    #[test]
    fn central_ac_builder_warns_when_seer_absent() {
        let mut params = Map::new();
        params.insert("cooling_capacity_w".to_string(), json!(12_000.0));

        let log = capture_warnings(|| {
            let result =
                try_build_central_ac_config("Air Conditioner", &params, &DuctDseParams::default());
            assert!(
                result.is_none(),
                "builder must return None when SEER is absent"
            );
        });

        assert!(
            log.contains("AnnualCoolingEfficiency (SEER) not found"),
            "expected SEER-missing warning in log output; got: {log:?}"
        );
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

    #[test]
    fn gas_furnace_multispeed_csv_stage_capacities_and_eirs_are_applied() {
        let defaults = repo_defaults();

        // CSV row: Gas Furnace, 90 AFUE, 2 speeds → capacity ratios [0.65, 1.0], COPs [0.88, 0.90]
        let rated_capacity_w = 10_000.0_f64;
        let afue = 0.90_f64;
        let mut params = Map::new();
        params.insert("efficiency_afue".to_string(), json!(afue));
        params.insert("heating_capacity_w".to_string(), json!(rated_capacity_w));
        params.insert("number_of_speeds".to_string(), json!(2u64));

        apply_multispeed_furnace_parameters(&mut params, &defaults, "Gas Furnace");

        let ec = try_build_gas_furnace_config("Gas Furnace", &params, &DuctDseParams::default())
            .expect("must not error for 2-speed gas furnace")
            .expect("must build config for 2-speed gas furnace");

        use hares_equipment::hvac::heating_config::GasFurnaceConfig;
        let cfg: GasFurnaceConfig = ec.typed().expect("must deserialize to GasFurnaceConfig");

        let stage_caps = cfg
            .stage_heating_capacities_w
            .expect("2-speed furnace must have stage_heating_capacities_w");
        let stage_eirs = cfg
            .stage_heating_eirs
            .expect("2-speed furnace must have stage_heating_eirs");

        assert_eq!(stage_caps.len(), 2, "must have exactly 2 stage capacities");
        assert_eq!(stage_eirs.len(), 2, "must have exactly 2 stage EIRs");

        // Stage 0: ratio 0.65 → 6500 W; EIR = 1/0.88
        assert!(
            (stage_caps[0] - rated_capacity_w * 0.65).abs() < 1.0,
            "low stage capacity must be capacity×0.65, got {}",
            stage_caps[0]
        );
        assert!(
            (stage_eirs[0] - 1.0 / 0.88).abs() < 1e-6,
            "low stage EIR must be 1/0.88, got {}",
            stage_eirs[0]
        );

        // Stage 1: ratio 1.0 → 10 000 W; EIR = 1/0.90
        assert!(
            (stage_caps[1] - rated_capacity_w * 1.0).abs() < 1.0,
            "high stage capacity must equal rated capacity, got {}",
            stage_caps[1]
        );
        assert!(
            (stage_eirs[1] - 1.0 / 0.90).abs() < 1e-6,
            "high stage EIR must be 1/0.90, got {}",
            stage_eirs[1]
        );
    }

    #[test]
    fn electric_auxiliary_energy_kwh_per_year_converts_to_watts() {
        let xml = r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatingSystem>
                          <SystemIdentifier id="htg1"/>
                          <HeatingSystemType><ElectricResistance/></HeatingSystemType>
                          <HeatingSystemFuel>electricity</HeatingSystemFuel>
                          <ElectricAuxiliaryEnergy>876</ElectricAuxiliaryEnergy>
                          <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
                        </HeatingSystem>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs).expect("resolve_hvac must succeed");
        assert_eq!(specs.len(), 1, "expected one heating spec");
        let aux_w = specs[0]
            .parameters
            .get("auxiliary_power_w")
            .and_then(|v| v.as_f64())
            .expect("auxiliary_power_w must be present");
        assert!(
            (aux_w - 100.0).abs() < 1e-6,
            "ElectricAuxiliaryEnergy=876 kWh/year must yield 100.0 W, got {aux_w}"
        );
    }

    #[test]
    fn boiler_electric_auxiliary_energy_uses_2080_hour_divisor() {
        // ANSI/RESNET/ICC 301-2019 Eq. 4.4-5: boiler auxiliaries (pumps, controls)
        // operate for 2080 h/yr during heating season, not 8760 h/yr year-round.
        // ElectricAuxiliaryEnergy = 2080 kWh/yr → 2080 / 2080 * 1000 = 1000.0 W.
        let xml = r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatingSystem>
                          <SystemIdentifier id="htg1"/>
                          <HeatingSystemType><Boiler><BoilerType>hot water</BoilerType></Boiler></HeatingSystemType>
                          <HeatingSystemFuel>natural gas</HeatingSystemFuel>
                          <HeatingCapacity>40000</HeatingCapacity>
                          <AnnualHeatingEfficiency>
                            <Units>AFUE</Units>
                            <Value>0.85</Value>
                          </AnnualHeatingEfficiency>
                          <ElectricAuxiliaryEnergy>2080</ElectricAuxiliaryEnergy>
                          <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
                        </HeatingSystem>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs).expect("resolve_hvac must succeed");
        assert_eq!(specs.len(), 1, "expected one heating spec");
        let aux_w = specs[0]
            .parameters
            .get("auxiliary_power_w")
            .and_then(|v| v.as_f64())
            .expect("auxiliary_power_w must be present");
        assert!(
            (aux_w - 1000.0).abs() < 1e-6,
            "Boiler ElectricAuxiliaryEnergy=2080 kWh/yr must yield 1000.0 W \
             (2080 h/yr divisor per RESNET Eq. 4.4-5), got {aux_w}"
        );
    }

    // ---- Task 1: remap_minisplit_stages ----

    #[test]
    fn remap_minisplit_stages_subsamples_10_to_4() {
        let ratios: Vec<f64> = (0..10).map(|i| (i + 1) as f64 * 0.1).collect();
        let cops: Vec<f64> = (0..10).map(|i| 2.0 + i as f64 * 0.5).collect();
        let shrs: Vec<f64> = (0..10).map(|i| 0.70 + i as f64 * 0.01).collect();

        let (r, c, s) = remap_minisplit_stages(&ratios, &cops, &shrs, 4);

        assert_eq!(r, vec![ratios[1], ratios[3], ratios[5], ratios[9]]);
        assert_eq!(c, vec![cops[1], cops[3], cops[5], cops[9]]);
        assert_eq!(s, vec![shrs[1], shrs[3], shrs[5], shrs[9]]);
    }

    #[test]
    fn remap_minisplit_stages_passthrough_when_not_10() {
        let ratios = vec![0.5, 0.75, 1.0, 1.2];
        let cops = vec![3.0, 3.5, 4.0, 4.5];
        let shrs = vec![0.78, 0.76, 0.74, 0.72];

        let (r, c, s) = remap_minisplit_stages(&ratios, &cops, &shrs, 4);
        assert_eq!(r, ratios);
        assert_eq!(c, cops);
        assert_eq!(s, shrs);
    }

    #[test]
    fn remap_minisplit_stages_passthrough_when_not_4_speeds() {
        let ratios: Vec<f64> = (0..10).map(|i| (i + 1) as f64 * 0.1).collect();
        let cops: Vec<f64> = (0..10).map(|i| 2.0 + i as f64 * 0.5).collect();
        let shrs: Vec<f64> = (0..10).map(|i| 0.70 + i as f64 * 0.01).collect();

        let (r, c, s) = remap_minisplit_stages(&ratios, &cops, &shrs, 2);
        assert_eq!(r, ratios);
        assert_eq!(c, cops);
        assert_eq!(s, shrs);
    }

    // ---- Task 2: MSHP resolver-level speed forcing ----

    #[test]
    fn mshp_heater_forces_four_speeds_with_no_compressor_type() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(10_000.0));
        params.insert("efficiency_hspf".to_string(), json!(9.0));
        // No CompressorType → n_speeds from defaults should be 1, but MSHP forces 4.
        let ec = try_build_heat_pump_heater_config(
            "MSHP Heater",
            &params,
            &DuctDseParams::default(),
            true,
        )
        .expect("MSHP heater typed config should be built");

        use hares_equipment::hvac::heat_pump_config::HeatPumpHeaterConfig;
        let cfg: HeatPumpHeaterConfig = ec.typed().expect("typed heater config");
        assert!(cfg.common.is_mini_split);
        assert_eq!(cfg.common.number_of_speeds, 4);
    }

    #[test]
    fn mshp_heater_forces_four_speeds_even_with_single_stage_compressor_type() {
        let mut params = Map::new();
        params.insert("heating_capacity_w".to_string(), json!(10_000.0));
        params.insert("efficiency_hspf".to_string(), json!(9.0));
        params.insert("number_of_speeds".to_string(), json!(1));
        // Even if CompressorType says single_stage → MSHP still forces 4.
        let ec = try_build_heat_pump_heater_config(
            "MSHP Heater",
            &params,
            &DuctDseParams::default(),
            true,
        )
        .expect("MSHP heater typed config should be built");

        use hares_equipment::hvac::heat_pump_config::HeatPumpHeaterConfig;
        let cfg: HeatPumpHeaterConfig = ec.typed().expect("typed heater config");
        assert!(cfg.common.is_mini_split);
        assert_eq!(cfg.common.number_of_speeds, 4);
    }

    // ---- HeatPump FractionHeatLoadServed / FractionCoolLoadServed ----

    /// Regression: HeatPump resolver must read the canonical HPXML 4.x element names
    /// `FractionHeatLoadServed` and `FractionCoolLoadServed`.  Prior to the fix the
    /// resolver only looked for the non-canonical aliases
    /// (`FractionHeatingLoadServed` / `FractionCoolingLoadServed`), so fractions from
    /// real OS-HPXML files were silently dropped and both typed configs defaulted to
    /// `None` (interpreted downstream as 100 % load served regardless of the actual
    /// fraction).
    #[test]
    fn heat_pump_reads_canonical_fraction_element_names() {
        // Canonical names as used in every OS-HPXML sample file.
        let xml = r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatPump>
                          <SystemIdentifier id="hp1"/>
                          <HeatPumpType>air-to-air</HeatPumpType>
                          <HeatPumpFuel>electricity</HeatPumpFuel>
                          <HeatingCapacity>36000.0</HeatingCapacity>
                          <CoolingCapacity>36000.0</CoolingCapacity>
                          <AnnualHeatingEfficiency>
                            <Units>HSPF</Units>
                            <Value>8.5</Value>
                          </AnnualHeatingEfficiency>
                          <AnnualCoolingEfficiency>
                            <Units>SEER</Units>
                            <Value>16.0</Value>
                          </AnnualCoolingEfficiency>
                          <BackupType>integrated</BackupType>
                          <BackupSystemFuel>electricity</BackupSystemFuel>
                          <BackupAnnualHeatingEfficiency>
                            <Units>Percent</Units>
                            <Value>1.0</Value>
                          </BackupAnnualHeatingEfficiency>
                          <BackupHeatingCapacity>10000.0</BackupHeatingCapacity>
                          <FractionHeatLoadServed>0.8</FractionHeatLoadServed>
                          <FractionCoolLoadServed>0.7</FractionCoolLoadServed>
                        </HeatPump>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs).expect("resolve_hvac must succeed");

        // The split produces two specs: ASHP Heater and ASHP Cooler.
        assert_eq!(specs.len(), 2, "expected heater + cooler specs");

        let heater = specs
            .iter()
            .find(|s| s.name.contains("Heater"))
            .expect("heater spec");
        let cooler = specs
            .iter()
            .find(|s| s.name.contains("Cooler"))
            .expect("cooler spec");

        use hares_equipment::hvac::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};
        let heater_cfg: HeatPumpHeaterConfig = heater
            .typed_config
            .clone()
            .expect("heater must have typed config")
            .typed()
            .expect("typed heater config");
        let cooler_cfg: HeatPumpCoolerConfig = cooler
            .typed_config
            .clone()
            .expect("cooler must have typed config")
            .typed()
            .expect("typed cooler config");

        assert_eq!(
            heater_cfg.common.fraction_heating_load_served,
            Some(0.8),
            "FractionHeatLoadServed (canonical) must be parsed for heater"
        );
        assert_eq!(
            cooler_cfg.common.fraction_cooling_load_served,
            Some(0.7),
            "FractionCoolLoadServed (canonical) must be parsed for cooler"
        );
    }

    /// Companion regression: the non-canonical alias names
    /// (`FractionHeatingLoadServed` / `FractionCoolingLoadServed`) must still work
    /// as a fallback so that any existing files using them continue to parse correctly.
    #[test]
    fn heat_pump_reads_alias_fraction_element_names_as_fallback() {
        let xml = r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatPump>
                          <SystemIdentifier id="hp1"/>
                          <HeatPumpType>air-to-air</HeatPumpType>
                          <HeatPumpFuel>electricity</HeatPumpFuel>
                          <HeatingCapacity>36000.0</HeatingCapacity>
                          <CoolingCapacity>36000.0</CoolingCapacity>
                          <AnnualHeatingEfficiency>
                            <Units>HSPF</Units>
                            <Value>8.5</Value>
                          </AnnualHeatingEfficiency>
                          <AnnualCoolingEfficiency>
                            <Units>SEER</Units>
                            <Value>16.0</Value>
                          </AnnualCoolingEfficiency>
                          <BackupType>integrated</BackupType>
                          <BackupSystemFuel>electricity</BackupSystemFuel>
                          <BackupAnnualHeatingEfficiency>
                            <Units>Percent</Units>
                            <Value>1.0</Value>
                          </BackupAnnualHeatingEfficiency>
                          <BackupHeatingCapacity>10000.0</BackupHeatingCapacity>
                          <FractionHeatingLoadServed>0.6</FractionHeatingLoadServed>
                          <FractionCoolingLoadServed>0.5</FractionCoolingLoadServed>
                        </HeatPump>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs).expect("resolve_hvac must succeed");

        assert_eq!(specs.len(), 2, "expected heater + cooler specs");

        let heater = specs
            .iter()
            .find(|s| s.name.contains("Heater"))
            .expect("heater spec");
        let cooler = specs
            .iter()
            .find(|s| s.name.contains("Cooler"))
            .expect("cooler spec");

        use hares_equipment::hvac::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};
        let heater_cfg: HeatPumpHeaterConfig = heater
            .typed_config
            .clone()
            .expect("heater must have typed config")
            .typed()
            .expect("typed heater config");
        let cooler_cfg: HeatPumpCoolerConfig = cooler
            .typed_config
            .clone()
            .expect("cooler must have typed config")
            .typed()
            .expect("typed cooler config");

        assert_eq!(
            heater_cfg.common.fraction_heating_load_served,
            Some(0.6),
            "FractionHeatingLoadServed (alias) must be parsed for heater as fallback"
        );
        assert_eq!(
            cooler_cfg.common.fraction_cooling_load_served,
            Some(0.5),
            "FractionCoolingLoadServed (alias) must be parsed for cooler as fallback"
        );
    }

    // ---- BackupAnnualHeatingEfficiency Units element ignored ----

    /// Helper: build a minimal HeatPump XML fragment with the given backup efficiency
    /// units and value, run resolve_hvac, and return the `backup_eir` from the heater
    /// spec params map.
    fn resolve_backup_eir(units: &str, value: f64) -> Option<f64> {
        let xml = format!(
            r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatPump>
                          <SystemIdentifier id="hp1"/>
                          <HeatPumpType>air-to-air</HeatPumpType>
                          <HeatPumpFuel>electricity</HeatPumpFuel>
                          <HeatingCapacity>36000.0</HeatingCapacity>
                          <CoolingCapacity>36000.0</CoolingCapacity>
                          <AnnualHeatingEfficiency>
                            <Units>HSPF</Units>
                            <Value>8.5</Value>
                          </AnnualHeatingEfficiency>
                          <AnnualCoolingEfficiency>
                            <Units>SEER</Units>
                            <Value>16.0</Value>
                          </AnnualCoolingEfficiency>
                          <BackupType>integrated</BackupType>
                          <BackupSystemFuel>electricity</BackupSystemFuel>
                          <BackupAnnualHeatingEfficiency>
                            <Units>{units}</Units>
                            <Value>{value}</Value>
                          </BackupAnnualHeatingEfficiency>
                          <BackupHeatingCapacity>10000.0</BackupHeatingCapacity>
                          <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
                          <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
                        </HeatPump>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
            "#
        );
        let root = parse_xml_document(&xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs).ok()?;
        let heater = specs.iter().find(|s| s.name.contains("Heater"))?;
        heater.parameters.get("backup_eir").and_then(Value::as_f64)
    }

    /// Percent with value 1.0 (fraction form, 0–1) → EIR = 1.0 (COP = 1).
    /// This is the conventional form used by all OpenStudio-HPXML sample files.
    #[test]
    fn backup_eir_percent_fraction_form_yields_eir_one() {
        // <Units>Percent</Units><Value>1.0</Value> means 100% efficiency as a fraction.
        // EIR = 1 / 1.0 = 1.0 (COP = 1, i.e., electric resistance).
        let eir = resolve_backup_eir("Percent", 1.0).expect("backup_eir must be present");
        assert!(
            (eir - 1.0).abs() < 1e-9,
            "Percent/1.0 must yield EIR=1.0, got {eir}"
        );
    }

    /// Regression: Percent with value 100.0 (percent-out-of-100 form) must be normalized
    /// by dividing by 100 before inverting, yielding EIR = 1.0 (COP = 1).
    #[test]
    fn backup_eir_percent_out_of_100_must_normalize_to_eir_one() {
        // <Units>Percent</Units><Value>100.0</Value> expressing 100% efficiency
        // as percent-out-of-100 must be normalized to fraction before EIR inversion.
        let eir = resolve_backup_eir("Percent", 100.0).expect("backup_eir must be present");
        assert!(
            (eir - 1.0).abs() < 1e-9,
            "Percent/100.0 must yield EIR=1.0 after normalization, got {eir}"
        );
    }

    /// AFUE with value 0.95 → EIR ≈ 1.0526 (gas backup at 95% AFUE).
    #[test]
    fn backup_eir_afue_fraction_yields_correct_eir() {
        let eir = resolve_backup_eir("AFUE", 0.95).expect("backup_eir must be present");
        let expected = 1.0 / 0.95;
        assert!(
            (eir - expected).abs() < 1e-9,
            "AFUE/0.95 must yield EIR≈{expected:.4}, got {eir}"
        );
    }

    /// COP with value 3.5 → EIR ≈ 0.2857.
    #[test]
    fn backup_eir_cop_yields_correct_eir() {
        let eir = resolve_backup_eir("COP", 3.5).expect("backup_eir must be present");
        let expected = 1.0 / 3.5;
        assert!(
            (eir - expected).abs() < 1e-6,
            "COP/3.5 must yield EIR≈{expected:.4}, got {eir}"
        );
    }

    /// Helper: build a minimal HeatPump XML fragment with the given backup efficiency
    /// units and value, run resolve_hvac, and return the `Result<backup_eir>` — propagating
    /// parse errors so error-path tests can assert on the error.
    fn resolve_backup_eir_result(units: &str, value: f64) -> Result<Option<f64>, HpxmlError> {
        let xml = format!(
            r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <HVAC>
                      <HVACPlant>
                        <HeatPump>
                          <SystemIdentifier id="hp1"/>
                          <HeatPumpType>air-to-air</HeatPumpType>
                          <HeatPumpFuel>electricity</HeatPumpFuel>
                          <HeatingCapacity>36000.0</HeatingCapacity>
                          <CoolingCapacity>36000.0</CoolingCapacity>
                          <AnnualHeatingEfficiency>
                            <Units>HSPF</Units>
                            <Value>8.5</Value>
                          </AnnualHeatingEfficiency>
                          <AnnualCoolingEfficiency>
                            <Units>SEER</Units>
                            <Value>16.0</Value>
                          </AnnualCoolingEfficiency>
                          <BackupType>integrated</BackupType>
                          <BackupSystemFuel>electricity</BackupSystemFuel>
                          <BackupAnnualHeatingEfficiency>
                            <Units>{units}</Units>
                            <Value>{value}</Value>
                          </BackupAnnualHeatingEfficiency>
                          <BackupHeatingCapacity>10000.0</BackupHeatingCapacity>
                          <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
                          <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
                        </HeatPump>
                      </HVACPlant>
                    </HVAC>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
            "#
        );
        let root = parse_xml_document(&xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let mut building = empty_building(vec![conditioned_zone()]);
        building.details_xml = details.clone();
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_hvac(&building, &defaults, &mut specs)?;
        let heater = specs
            .iter()
            .find(|s| s.name.contains("Heater"))
            .ok_or_else(|| HpxmlError::Parse("no heater spec found".into()))?;
        Ok(heater.parameters.get("backup_eir").and_then(Value::as_f64))
    }

    /// Unrecognized units (e.g. "Joules") must produce a loud parse error, not a silent default.
    #[test]
    fn backup_eir_unknown_units_produces_parse_error() {
        let err = resolve_backup_eir_result("Joules", 1.0).unwrap_err();
        assert!(
            matches!(err, HpxmlError::Parse(_)),
            "unrecognized units must produce HpxmlError::Parse, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("Joules"),
            "error message must name the unrecognized unit, got: {msg}"
        );
    }

    /// HSPF and HSPF2 are valid HeatingEfficiencyUnits per the HPXML XSD, but are seasonal
    /// metrics that do not apply to a backup resistance or gas strip. They must be rejected
    /// with a loud parse error.
    #[test]
    fn backup_eir_hspf_rejected_for_backup_strip() {
        let err = resolve_backup_eir_result("HSPF", 8.5).unwrap_err();
        assert!(
            matches!(err, HpxmlError::Parse(_)),
            "HSPF for backup must produce HpxmlError::Parse, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("HSPF"),
            "error message must name HSPF, got: {msg}"
        );
    }

    // HSPF2_TO_HSPF_FACTOR must be 1/0.85, not 1/0.95.
    // Per MINHERS Addendum 71f (RESNET, adopted from AHRI), the HSPF2/HSPF ratio for
    // ducted split-system heat pumps is 0.85 (≈15% reduction), whereas the SEER2/SEER
    // ratio is 0.95 (≈5% reduction). Using 0.95 for HSPF2 overstates COP by ~10%.
    #[test]
    fn hspf2_to_hspf_factor_is_one_over_0_85() {
        // normalize_efficiency_units("HSPF2", 9.0, false) must return ("HSPF", 9.0 / 0.85).
        let (units, hspf) = normalize_efficiency_units("HSPF2", 9.0, false);
        assert_eq!(units, "HSPF", "unit label must be HSPF after conversion");

        let expected = 9.0_f64 / 0.85;
        assert!(
            (hspf - expected).abs() < expected * 0.001,
            "HSPF2=9.0 must convert to HSPF≈{expected:.4}, got {hspf:.4}"
        );
    }

    // The EIR derived from the converted HSPF must match
    // 3.412 / (HSPF2 / 0.85), not 3.412 / (HSPF2 / 0.95).
    #[test]
    fn hspf2_conversion_eir_matches_correct_factor() {
        let hspf2 = 9.0_f64;
        let (_, hspf) = normalize_efficiency_units("HSPF2", hspf2, false);

        // EIR = BTU_PER_WH / HSPF  (3.412 Btu/Wh)
        const BTU_PER_WH: f64 = 3.412_141_633;
        let eir = BTU_PER_WH / hspf;

        let correct_eir = BTU_PER_WH / (hspf2 / 0.85);
        let wrong_eir = BTU_PER_WH / (hspf2 / 0.95);

        assert!(
            (eir - correct_eir).abs() < 1e-6,
            "EIR must be {correct_eir:.6} (1/0.85 factor), got {eir:.6}"
        );
        assert!(
            (eir - wrong_eir).abs() > 0.01,
            "EIR must NOT equal the wrong value {wrong_eir:.6} produced by 1/0.95 factor"
        );
    }

    // Sanity check: SEER2→SEER factor for ducted systems must remain 1/0.95, independent of
    // the HSPF2→HSPF correction. The two conversion factors are distinct (0.85 vs. 0.95).
    #[test]
    fn seer2_to_seer_factor_is_one_over_0_95() {
        let (units, seer) = normalize_efficiency_units("SEER2", 14.0, false);
        assert_eq!(units, "SEER");
        let expected = 14.0_f64 / 0.95;
        assert!(
            (seer - expected).abs() < expected * 0.001,
            "SEER2=14.0 must convert to SEER≈{expected:.4}, got {seer:.4}"
        );
    }

    // EER2 must be converted to EER using EER2_TO_EER_FACTOR (1/0.96).
    // DOE 10 CFR Part 430 Appendix M1 (2023) / AHRI 210/240-2023: EER2/EER ratio ≈ 0.96
    // (≈4% reduction). CEC conversion table: split-system EER = EER2 × 1.043, packaged
    // EER = EER2 × 1.038. 1/0.96 is the central residential estimate.
    #[test]
    fn eer2_is_converted_to_eer_with_correct_factor() {
        let (units, eer) = normalize_efficiency_units("EER2", 10.0, false);
        assert_eq!(units, "EER", "EER2 must normalize to label 'EER'");

        let expected = 10.0_f64 / 0.96;
        assert!(
            (eer - expected).abs() < expected * 0.001,
            "EER2=10.0 must convert to EER≈{expected:.4} (factor 1/0.96), got {eer:.4}"
        );
        assert!(
            eer > 10.0,
            "EER2=10.0 must yield EER > 10.0 since EER2 < EER for same unit"
        );
    }

    // Plain EER must pass through normalize_efficiency_units unchanged — no conversion applied.
    #[test]
    fn eer_passthrough_unchanged() {
        let (units, eer) = normalize_efficiency_units("EER", 10.0, false);
        assert_eq!(units, "EER");
        assert!(
            (eer - 10.0).abs() < 1e-9,
            "EER=10.0 must pass through unchanged, got {eer}"
        );
    }

    // Ductless/mini-split heat pumps use HSPF2/HSPF ratio ≈ 0.90 (≈10% reduction)
    // per RESNET MINHERS Addendum 71f. Ductless units have no external static pressure
    // duct penalty, so the test-procedure impact is milder than for ducted units (0.85).
    #[test]
    fn hspf2_to_hspf_factor_is_one_over_0_90_for_ductless() {
        // normalize_efficiency_units("HSPF2", 9.0, true) must return ("HSPF", 9.0 / 0.90).
        let (units, hspf) = normalize_efficiency_units("HSPF2", 9.0, true);
        assert_eq!(units, "HSPF", "unit label must be HSPF after conversion");

        let expected = 9.0_f64 / 0.90;
        assert!(
            (hspf - expected).abs() < expected * 0.001,
            "HSPF2=9.0 ductless must convert to HSPF≈{expected:.4} (1/0.90), got {hspf:.4}"
        );
        // Ductless HSPF (10.0 = 9.0/0.90) must be GREATER than the wrong
        // SEER2 factor (1/0.95 → 9.47) and LESS than the ducted HSPF2 factor
        // (1/0.85 → 10.59), proving the conversion uses the ductless factor.
        assert!(
            hspf > 9.0 / 0.95,
            "ductless HSPF2 must NOT use the SEER2 factor 1/0.95, got {hspf}"
        );
        assert!(
            hspf < 9.0 / 0.85,
            "ductless HSPF2 must NOT use the ducted factor 1/0.85, got {hspf}"
        );
    }

    // Ductless/mini-split SEER2 = SEER (ratio 1.00) per RESNET MINHERS Addendum 71f.
    // The AHRI 210/240-2023 test-procedure change for external static pressure
    // does not affect ductless units, so SEER2 = SEER for mini-splits.
    #[test]
    fn seer2_to_seer_factor_is_one_for_ductless() {
        let (units, seer) = normalize_efficiency_units("SEER2", 14.0, true);
        assert_eq!(units, "SEER", "unit label must be SEER after conversion");
        // SEER2 = SEER for ductless (factor 1.0), so value unchanged.
        assert!(
            (seer - 14.0).abs() < 1e-9,
            "SEER2=14.0 ductless must equal SEER=14.0 (factor 1.0), got {seer}"
        );
        // Must NOT equal the ducted conversion (14.0 / 0.95 ≈ 14.74).
        let ducted = 14.0_f64 / 0.95;
        assert!(
            (seer - ducted).abs() > 0.5,
            "SEER2=14.0 ductless must NOT equal ducted conversion {ducted:.2}, got {seer}"
        );
    }

    // Ductless HSPF2 EIR sanity check: derive EIR from HSPF2=9.0 ductless
    // and verify it matches 3.412 / (9.0 / 0.90), not the ducted 1/0.85 or 1/0.95.
    #[test]
    fn hspf2_ductless_eir_matches_correct_factor() {
        let hspf2 = 9.0_f64;
        let (_, hspf) = normalize_efficiency_units("HSPF2", hspf2, true);

        const BTU_PER_WH: f64 = 3.412_141_633;
        let eir = BTU_PER_WH / hspf;

        let correct_eir = BTU_PER_WH / (hspf2 / 0.90);
        let ducted_eir = BTU_PER_WH / (hspf2 / 0.85);
        let wrong_eir = BTU_PER_WH / (hspf2 / 0.95);

        assert!(
            (eir - correct_eir).abs() < 1e-6,
            "ductless EIR must be {correct_eir:.6} (HSPF2/0.90), got {eir:.6}"
        );
        assert!(
            (eir - ducted_eir).abs() > 1e-6,
            "ductless EIR must NOT equal ducted EIR {ducted_eir:.6}"
        );
        assert!(
            (eir - wrong_eir).abs() > 0.01,
            "ductless EIR must NOT equal wrong (1/0.95) EIR {wrong_eir:.6}"
        );
    }

    // PERCENT values > 1.0 are interpreted as percent-out-of-100 form and
    // divided by 100 to yield a fraction. A HPXML <Value>95</Value> with
    // <Units>Percent</Units> should normalize to 0.95 (fraction form).
    #[test]
    fn percent_value_above_one_divided_by_100() {
        let (units, value) = normalize_efficiency_units("Percent", 95.0, false);
        assert_eq!(
            units, "PERCENT",
            "unit label must be PERCENT after normalization"
        );
        assert!(
            (value - 0.95).abs() < 1e-9,
            "95% must normalize to 0.95 fraction, got {value}"
        );
    }

    // PERCENT values ≤ 1.0 are already in fraction form (the HPXML convention).
    // 1.0 means 100% efficient, 0.95 means 95% efficient — value passes through.
    #[test]
    fn percent_value_at_one_passes_through() {
        let (units, value) = normalize_efficiency_units("Percent", 1.0, false);
        assert_eq!(units, "PERCENT");
        assert!(
            (value - 1.0).abs() < 1e-9,
            "1.0 fraction must pass through unchanged, got {value}"
        );
    }

    // PERCENT value already in fraction form (e.g. 0.95) passes through
    // without modification — no false-positive reinterpretation.
    #[test]
    fn percent_value_below_one_passes_through() {
        let (units, value) = normalize_efficiency_units("Percent", 0.95, false);
        assert_eq!(units, "PERCENT");
        assert!(
            (value - 0.95).abs() < 1e-9,
            "0.95 fraction must pass through unchanged, got {value}"
        );
    }

    // PERCENT values in percent-out-of-100 form that are still ≤ 1.0 after
    // division by 100 (e.g. 0.99 → 0.0099) pass through as-is. The threshold
    // check is on the input, not the output — a value of 0.99 is already in
    // fraction form and represents 99% efficiency.
    #[test]
    fn percent_value_near_zero_passes_through() {
        let (units, value) = normalize_efficiency_units("Percent", 0.01, false);
        assert_eq!(units, "PERCENT");
        assert!(
            (value - 0.01).abs() < 1e-9,
            "0.01 fraction must pass through unchanged, got {value}"
        );
    }
}
