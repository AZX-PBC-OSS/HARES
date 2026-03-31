//! Conversion helpers between I/O types and envelope/equipment types.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset};
use hares_envelope::{BoundaryInput, ExteriorTarget, LayerInput, ZoneInput};
use hares_equipment::{EquipmentConfig, config::ConfigValue};
use hares_io::{Building, DefaultsStore, SimulationConfig};
use hares_types::{DomainUpdate, EnvironmentState, ExecutionStage, HaresError, ZoneId};
use serde_json::{Map, Value};

use super::{DwellingConfig, Result};

const DEFAULT_R_M2_K_W: f64 = hares_envelope::boundary_rc::DEFAULT_R_M2_K_W;

/// Convert building zones to envelope-crate ZoneInput.
pub fn building_to_zone_inputs(building: &Building, n_zones: usize) -> Vec<ZoneInput> {
    (0..n_zones)
        .map(|idx| ZoneInput {
            floor_area_m2: building.zones.get(idx).and_then(|z| z.floor_area_m2),
            volume_m3: building.zones.get(idx).and_then(|z| z.volume_m3),
        })
        .collect()
}

/// Convert building boundaries to envelope-crate BoundaryInput with pre-resolved zone indices.
///
/// When the defaults store contains an envelope LUT, attempts to resolve each
/// boundary to OCHRE pre-computed RC layers. Falls through to raw material
/// layers on LUT miss.
pub fn building_to_boundary_inputs(
    building: &Building,
    n_zones: usize,
    defaults: &DefaultsStore,
    avg_wind_m_s: f64,
    avg_ambient_c: f64,
    avg_ground_c: f64,
) -> Vec<BoundaryInput> {
    use hares_envelope::PrecomputedRCLayer;
    use hares_io::envelope_lut::resolve_boundary_name;
    use hares_io::hpxml::BoundaryType;
    use hares_physics::film_coefficients::{SurfaceRoughness, film_resistances};
    use hares_physics::solar::window_u_factor_decomposition;

    let envelope_lut = defaults.envelope_lut();

    building
        .boundaries
        .iter()
        .map(|bd| {
            let interior_zone_idx = find_zone_idx(building, bd.interior_zone.as_ref(), n_zones);
            let exterior = resolve_exterior(building, bd, n_zones);

            // Film resistances first — needed to strip from assembly R-value.
            let tilt_deg = bd.tilt_deg.unwrap_or(90.0);
            let interior_label = zone_type_to_label(bd.interior_zone.as_ref());
            let exterior_label = zone_type_to_label(bd.exterior_zone.as_ref());
            let (r_film_int, r_film_ext) = film_resistances(
                tilt_deg,
                interior_label,
                exterior_label,
                avg_wind_m_s,
                avg_ground_c,
                avg_ambient_c,
                SurfaceRoughness::Rough,
            );

            // fallback_r is material-only R (no film). HPXML AssemblyEffectiveRValue
            // includes film per spec, so subtract computed film R to get material-only.
            // NominalRValue layers are already material-only. Film R is added back in
            // boundary_rc.rs for all three construction paths.
            let fallback_r = bd
                .assembly_r_value_m2_k_w
                .map(|r| (r - r_film_int - r_film_ext).max(1e-6))
                .or_else(|| {
                    let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                    if sum > 0.0 { Some(sum) } else { None }
                })
                .unwrap_or(DEFAULT_R_M2_K_W)
                .max(1e-6);

            // Try LUT lookup for precomputed RC layers — skip when the boundary
            // has explicit material layers (BESTEST synthetic configs define their own
            // layer stack which must not be overridden by the LUT).
            // Skip LUT when the boundary has explicit material layers (BESTEST
            // synthetic configs define their own layer stack).
            let precomputed_rc = if !bd.material_layers.is_empty() {
                Vec::new()
            } else {
                envelope_lut
                    .and_then(|lut| {
                        let boundary_name = bd.lut_boundary_name.as_deref().or_else(|| {
                            resolve_boundary_name(
                                &bd.boundary_type,
                                bd.interior_zone.as_ref(),
                                bd.exterior_zone.as_ref(),
                            )
                        })?;
                        let r_value = bd.assembly_r_value_m2_k_w.or_else(|| {
                            let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                            if sum > 0.0 { Some(sum) } else { None }
                        });
                        let result = lut.lookup(
                            boundary_name,
                            bd.construction_type.as_deref(),
                            bd.finish_type.as_deref(),
                            bd.insulation_details.as_deref(),
                            r_value,
                        );
                        if result.is_none() {
                            tracing::debug!(
                                boundary = boundary_name,
                                construction = ?bd.construction_type,
                                finish = ?bd.finish_type,
                                insulation = ?bd.insulation_details,
                                r_value = ?r_value,
                                "envelope LUT lookup miss"
                            );
                        }
                        let result = result?;
                        tracing::debug!(
                            boundary = boundary_name,
                            layers = result.layers.len(),
                            matched_type = %result.matched_boundary_type,
                            "envelope LUT match"
                        );
                        // LUT layers are exterior→interior (OCHRE CSV convention).
                        // RC builder expects interior→exterior, so reverse.
                        let mut layers: Vec<_> = result
                            .layers
                            .into_iter()
                            .map(|l| PrecomputedRCLayer {
                                resistance_m2_k_w: l.resistance_m2_k_w,
                                capacitance_kj_m2_k: l.capacitance_kj_m2_k,
                            })
                            .collect();
                        layers.reverse();
                        Some(layers)
                    })
                    .unwrap_or_default()
            };

            // Window U-factor decomposition: EnergyPlus Simple Window Model Step 1.
            // Overrides fallback_r and film resistances for window boundaries.
            let (fallback_r, r_film_int, r_film_ext) = if bd.boundary_type == BoundaryType::Window {
                let u_factor = building
                    .windows
                    .iter()
                    .find(|w| w.id == bd.id)
                    .and_then(|w| w.u_factor_w_m2_k);
                if let Some(u) = u_factor.filter(|&u| u > 0.0) {
                    let (r_glass, r_int) = window_u_factor_decomposition(u);
                    (r_glass, r_int, 0.0)
                } else {
                    tracing::warn!(
                        boundary = %bd.id,
                        "window boundary has no U-factor — using generic film resistances"
                    );
                    (fallback_r, r_film_int, r_film_ext)
                }
            } else {
                (fallback_r, r_film_int, r_film_ext)
            };

            BoundaryInput {
                area_m2: bd.area_m2,
                interior_zone_idx,
                exterior,
                material_layers: bd
                    .material_layers
                    .iter()
                    .map(|l| LayerInput {
                        thickness_m: l.thickness_m,
                        conductivity_w_m_k: l.conductivity_w_m_k,
                        density_kg_m3: l.density_kg_m3,
                        specific_heat_j_kg_k: l.specific_heat_j_kg_k,
                        area_m2: l.area_m2,
                    })
                    .collect(),
                precomputed_rc,
                fallback_r_m2_k_w: fallback_r,
                r_film_interior_m2_k_w: r_film_int,
                r_film_exterior_m2_k_w: r_film_ext,
                framing_factor: bd.framing_factor,
            }
        })
        .collect()
}

/// Map HPXML ZoneType to film-coefficient ZoneLabel.
pub(crate) fn zone_type_to_label(
    zt: Option<&hares_io::hpxml::ZoneType>,
) -> hares_physics::film_coefficients::ZoneLabel {
    use hares_io::hpxml::ZoneType;
    use hares_physics::film_coefficients::ZoneLabel;
    match zt {
        Some(ZoneType::Conditioned) => ZoneLabel::Conditioned,
        Some(ZoneType::Attic) => ZoneLabel::Attic,
        Some(ZoneType::Garage) => ZoneLabel::Garage,
        Some(ZoneType::Foundation) => ZoneLabel::Foundation,
        Some(ZoneType::Ground) => ZoneLabel::Ground,
        Some(ZoneType::Adjacent) => ZoneLabel::Conditioned,
        Some(ZoneType::Outdoor) | None => ZoneLabel::Outdoor,
        Some(ZoneType::Other(_)) => ZoneLabel::Outdoor,
    }
}

/// Find zone index by ZoneType equality (exact match including Other payload).
pub(crate) fn find_zone_idx(
    building: &Building,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> usize {
    if n_zones == 0 {
        return 0;
    }
    if let Some(target) = zone_type {
        building
            .zones
            .iter()
            .position(|z| z.zone_type == *target)
            .unwrap_or(0)
            .min(n_zones - 1)
    } else {
        0
    }
}

pub(crate) fn resolve_exterior(
    building: &Building,
    boundary: &hares_io::hpxml::Boundary,
    n_zones: usize,
) -> ExteriorTarget {
    match boundary.exterior_zone.as_ref() {
        Some(hares_io::hpxml::ZoneType::Outdoor) => ExteriorTarget::Outdoor,
        Some(hares_io::hpxml::ZoneType::Ground) => ExteriorTarget::Ground,
        Some(zt) => {
            let idx = find_zone_idx(building, Some(zt), n_zones);
            ExteriorTarget::Zone(idx)
        }
        None => ExteriorTarget::Outdoor,
    }
}

pub(crate) fn build_output_column_index(
    schema: &arrow::datatypes::Schema,
) -> HashMap<String, usize> {
    // Recorder rows exclude the timestamp column.
    schema
        .fields()
        .iter()
        .skip(1)
        .enumerate()
        .map(|(idx, field)| (field.name().to_string(), idx))
        .collect()
}

pub(crate) fn default_output_path(config: &DwellingConfig) -> PathBuf {
    let ext = match config.sim_config.output_format {
        hares_io::OutputFormat::Csv => "csv",
        hares_io::OutputFormat::Parquet => "parquet",
    };
    PathBuf::from(format!("dwelling_{}.{}", config.bldg_id, ext))
}

pub(crate) fn chrono_to_std_duration(duration: Duration) -> Result<StdDuration> {
    let millis = duration.num_milliseconds();
    if millis <= 0 {
        return Err(HaresError::Physics(format!(
            "time resolution must be positive, got {millis} ms"
        )));
    }
    let ms_u64 = u64::try_from(millis)
        .map_err(|_| HaresError::Physics("failed converting duration to u64 ms".to_string()))?;
    Ok(StdDuration::from_millis(ms_u64))
}

pub fn stage_rank(stage: ExecutionStage) -> u8 {
    match stage {
        ExecutionStage::Independent => 0,
        ExecutionStage::Electrical => 1,
        ExecutionStage::Thermal => 2,
        ExecutionStage::EnvelopeResolution => 3,
    }
}

pub(crate) fn apply_thermal_update_to_zones(env: &mut EnvironmentState, update: &DomainUpdate) {
    for &(zone_id, temp_c) in &update.zone_temperatures_c {
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.temperature_c = temp_c;
        }
    }
}

pub(crate) fn apply_humidity_update_to_zones(env: &mut EnvironmentState, update: &DomainUpdate) {
    let Some(payload) = &update.custom_payload else {
        return;
    };
    for chunk in payload.chunks_exact(4) {
        let zone_raw = chunk[0];
        let humidity_ratio = chunk[1];
        let relative_humidity = chunk[2];
        let wet_bulb_c = chunk[3];

        if !zone_raw.is_finite() || zone_raw < 0.0 || zone_raw > f64::from(u16::MAX) {
            continue;
        }
        let zone_id = ZoneId(zone_raw as u16);
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.humidity_ratio = humidity_ratio;
            zone.relative_humidity = relative_humidity;
            zone.wet_bulb_c = wet_bulb_c;
        }
    }
}

pub(crate) fn equipment_config_from_spec(spec: &hares_io::EquipmentSpec) -> EquipmentConfig {
    let mut raw_config: HashMap<String, ConfigValue> = spec
        .parameters
        .iter()
        .filter_map(|(k, v)| json_value_to_config_value(v).map(|cv| (k.clone(), cv)))
        .collect();
    if let Some(zip) = &spec.zip_params {
        raw_config.insert("zip_z".to_string(), ConfigValue::Float(zip.zp));
        raw_config.insert("zip_i".to_string(), ConfigValue::Float(zip.ip));
        raw_config.insert("zip_p".to_string(), ConfigValue::Float(zip.pp));
        raw_config.insert("zip_v0".to_string(), ConfigValue::Float(1.0));
        raw_config.insert("zip_zq".to_string(), ConfigValue::Float(zip.zq));
        raw_config.insert("zip_iq".to_string(), ConfigValue::Float(zip.iq));
        raw_config.insert("zip_pq".to_string(), ConfigValue::Float(zip.pq));
        raw_config.insert("zip_pf".to_string(), ConfigValue::Float(zip.pf));
    }

    EquipmentConfig {
        name: spec.name.clone(),
        ochre_class: spec.name.clone(),
        payload: hares_equipment::ConfigPayload::Raw { data: raw_config },
    }
}

pub(crate) fn merged_equipment_config(
    spec: &hares_io::EquipmentSpec,
    overrides: &Value,
) -> EquipmentConfig {
    let mut merged = spec.parameters.clone();
    apply_equipment_overrides(&mut merged, overrides, &spec.name);
    let merged_spec = hares_io::EquipmentSpec {
        name: spec.name.clone(),
        fuel_type: spec.fuel_type,
        parameters: merged,
        zip_params: spec.zip_params.clone(),
    };
    equipment_config_from_spec(&merged_spec)
}

fn apply_equipment_overrides(base: &mut Map<String, Value>, overrides: &Value, name: &str) {
    let Value::Object(root) = overrides else {
        return;
    };
    if let Some(Value::Object(all)) = root.get("all").or_else(|| root.get("*")) {
        hares_io::hpxml::nested_update(base, all);
    }
    if let Some(Value::Object(eq)) = root.get(name) {
        hares_io::hpxml::nested_update(base, eq);
    }
}

pub(crate) fn boundary_zone_index(
    building: &Building,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> usize {
    if n_zones == 0 {
        return 0;
    }
    if let Some(target) = zone_type
        && let Some(idx) = building.zones.iter().position(|z| z.zone_type == *target)
    {
        return idx.min(n_zones - 1);
    }
    0
}

pub(crate) fn duration_to_u32_secs(duration: Duration) -> Result<u32> {
    let secs = duration.num_seconds();
    if secs <= 0 {
        return Err(HaresError::Physics(format!(
            "duration must be positive seconds, got {secs}"
        )));
    }
    u32::try_from(secs)
        .map_err(|_| HaresError::Physics(format!("duration seconds exceed u32 range: {secs}")))
}

pub(crate) fn required_path(kwargs: &HashMap<String, Value>, key: &str) -> Result<PathBuf> {
    let value = kwargs
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    Ok(PathBuf::from(value))
}

pub(crate) fn required_datetime(
    kwargs: &HashMap<String, Value>,
    key: &str,
) -> Result<DateTime<FixedOffset>> {
    let value = kwargs
        .get(key)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    if let Some(text) = value.as_str() {
        let dt = DateTime::parse_from_rfc3339(text).map_err(|err| {
            HaresError::Io(format!("failed parsing `{key}` as RFC3339 datetime: {err}"))
        })?;
        return Ok(dt);
    }
    if let Some(epoch) = value.as_i64() {
        let dt = DateTime::from_timestamp(epoch, 0)
            .map(|dt| dt.fixed_offset())
            .ok_or_else(|| HaresError::Io(format!("invalid epoch timestamp for `{key}`")))?;
        return Ok(dt);
    }
    Err(HaresError::Io(format!(
        "unsupported datetime format for `{key}`"
    )))
}

pub(crate) fn required_duration(kwargs: &HashMap<String, Value>, key: &str) -> Result<Duration> {
    let value = kwargs
        .get(key)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    if let Some(secs) = value.as_i64() {
        if secs <= 0 {
            return Err(HaresError::Io(format!(
                "`{key}` must be positive seconds, got {secs}"
            )));
        }
        return Ok(Duration::seconds(secs));
    }
    if let Some(text) = value.as_str() {
        let secs: i64 = text.parse().map_err(|err| {
            HaresError::Io(format!("failed parsing `{key}` duration seconds: {err}"))
        })?;
        if secs <= 0 {
            return Err(HaresError::Io(format!(
                "`{key}` must be positive seconds, got {secs}"
            )));
        }
        return Ok(Duration::seconds(secs));
    }
    Err(HaresError::Io(format!(
        "unsupported duration format for `{key}`"
    )))
}

pub(crate) fn validate_sim_config(sim_config: &SimulationConfig) -> Result<()> {
    if sim_config.duration.num_seconds() <= 0 {
        return Err(HaresError::Io("duration must be > 0".to_string()));
    }
    if sim_config.time_res.num_seconds() <= 0 {
        return Err(HaresError::Io("time_res must be > 0".to_string()));
    }
    if sim_config.duration.num_seconds() % sim_config.time_res.num_seconds() != 0 {
        return Err(HaresError::Io(
            "duration must be evenly divisible by time_res".to_string(),
        ));
    }
    Ok(())
}

/// Map HPXML `<SiteType>` to [`TerrainClass`] for AIM-2 wind correction.
pub(crate) fn site_type_to_terrain(
    site_type: &Option<hares_io::hpxml::SiteType>,
) -> hares_physics::infiltration::TerrainClass {
    use hares_io::hpxml::SiteType;
    use hares_physics::infiltration::TerrainClass;
    match site_type {
        Some(SiteType::Rural) => TerrainClass::Rural,
        Some(SiteType::Urban) => TerrainClass::Urban,
        _ => TerrainClass::Suburban,
    }
}

/// Map HPXML `<ShieldingOfHome>` string to [`ShieldingClass`].
///
/// HPXML values: "normal", "exposed", "well-shielded".
/// Walker & Wilson (1998) Table 3; ResStock `airflow.get_aim2_shelter_coefficient`.
pub(crate) fn shielding_str_to_class(
    s: Option<&str>,
) -> hares_physics::infiltration::ShieldingClass {
    use hares_physics::infiltration::ShieldingClass;
    match s {
        Some("exposed") => ShieldingClass::Exposed,
        Some("well-shielded") => ShieldingClass::WellShielded,
        _ => ShieldingClass::Normal,
    }
}

/// Check if any Foundation zone has `vented == true` (vented crawlspace).
///
/// Used to select [`FoundationLeakageClass`] for AIM-2 leakage distribution.
pub(crate) fn has_vented_crawlspace(building: &Building) -> bool {
    use hares_io::hpxml::ZoneType;
    building
        .zones
        .iter()
        .any(|z| z.zone_type == ZoneType::Foundation && z.vented)
}

pub(crate) fn json_value_to_config_value(value: &serde_json::Value) -> Option<ConfigValue> {
    match value {
        serde_json::Value::Number(n) => n.as_f64().map(ConfigValue::Float),
        serde_json::Value::String(s) => Some(ConfigValue::Text(s.clone())),
        serde_json::Value::Bool(b) => Some(ConfigValue::Bool(*b)),
        serde_json::Value::Array(arr) => {
            let floats: Vec<f64> = arr.iter().filter_map(|v| v.as_f64()).collect();
            if floats.len() == arr.len() {
                Some(ConfigValue::FloatArray(floats))
            } else {
                None
            }
        }
        _ => None,
    }
}
