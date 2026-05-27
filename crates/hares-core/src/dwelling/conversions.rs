//! Conversion helpers between I/O types and envelope/equipment types.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset};
use hares_envelope::longwave_radiation::{
    EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, EMISSIVITY_WINDOW,
};
use hares_envelope::{BoundaryInput, ExteriorTarget, LayerInput, ZoneInput};
use hares_equipment::{ConfigPayload, EquipmentConfig, config::ConfigValue};
use hares_io::hpxml::ZoneType;
use hares_io::{Building, DefaultsStore, SimulationConfig};
use hares_types::{DomainUpdate, EnvironmentState, ExecutionStage, HaresError, ZoneId};
use serde_json::{Map, Value};

use super::{DwellingConfig, Result};

const DEFAULT_R_M2_K_W: f64 = hares_envelope::boundary_rc::DEFAULT_R_M2_K_W;

/// Interior mass multiplier by zone type.
///
/// Conditioned space has furniture, partition walls, etc. that store heat;
/// the 7.0 multiplier captures this implicitly. All other zone types use
/// 1.0 (air capacitance only). OCHRE uses 7.0 for all zones, which
/// overstates foundation thermal mass.
///
/// **IMPORTANT**: EnergyPlus uses EITHER `ZoneCapacitanceMultiplier` (default
/// 1.0) OR explicit `InternalMass` objects — never both. When furniture RC
/// boundaries are present for a zone, `building_to_zone_inputs` overrides
/// this multiplier to 1.0 so the furniture thermal mass is counted only once
/// (via the explicit RC nodes). See E+ InputOutputRef, ZoneCapacitanceMultiplier.
pub fn mass_multiplier_for_zone(zone_type: &ZoneType) -> f64 {
    match zone_type {
        ZoneType::Conditioned => 7.0,
        ZoneType::Foundation
        | ZoneType::Attic
        | ZoneType::Garage
        | ZoneType::Outdoor
        | ZoneType::Ground
        | ZoneType::Adjacent
        | ZoneType::Other(_) => 1.0,
    }
}

/// Check if the building has auto-generated furniture boundaries for the given zone type.
///
/// Furniture boundaries are same-zone boundaries (interior == exterior) whose `id`
/// contains "furniture", e.g. "conditioned_furniture", "garage_furniture".
/// When these exist, they provide explicit RC thermal-mass nodes that replace the
/// implicit mass captured by `mass_multiplier > 1.0`.
///
/// Per EnergyPlus convention, `ZoneCapacitanceMultiplier` (default 1.0) and
/// `InternalMass` objects are mutually exclusive; the furniture boundary is the
/// HARES equivalent of an E+ InternalMass object.
fn zone_has_furniture_boundaries(building: &Building, zone_type: &ZoneType) -> bool {
    building.boundaries.iter().any(|bd| {
        bd.id.contains("furniture")
            && bd.interior_zone.as_ref() == Some(zone_type)
            && bd.exterior_zone.as_ref() == Some(zone_type)
    })
}

/// Convert building zones to envelope-crate ZoneInput.
///
/// When furniture RC boundaries exist for a zone (same-zone boundaries whose `id`
/// contains "furniture"), the mass multiplier is set to 1.0 (air capacitance only)
/// because the furniture thermal mass is already modeled via explicit RC nodes.
/// This avoids double-counting: E+ uses EITHER ZoneCapacitanceMultiplier (default
/// 1.0) OR InternalMass objects, never both.
pub fn building_to_zone_inputs(building: &Building, n_zones: usize) -> Vec<ZoneInput> {
    (0..n_zones)
        .map(|idx| {
            let zone = building.zones.get(idx);
            let mass_multiplier = building.mass_multiplier_override.unwrap_or_else(|| {
                let zone_type_mult = zone
                    .map(|z| mass_multiplier_for_zone(&z.zone_type))
                    .unwrap_or(1.0);
                // When furniture RC boundaries exist for this zone, the furniture
                // thermal mass is modeled explicitly via RC nodes (equivalent to
                // EnergyPlus InternalMass objects). The ZoneCapacitanceMultiplier
                // must be 1.0 (air capacitance only) to avoid double-counting.
                // Ref: E+ InputOutputRef ZoneCapacitanceMultiplier default=1.0;
                // E+ InternalMass and ZoneCapacitanceMultiplier are mutually exclusive.
                if zone.is_some_and(|z| zone_has_furniture_boundaries(building, &z.zone_type)) {
                    1.0
                } else {
                    zone_type_mult
                }
            });
            ZoneInput {
                floor_area_m2: zone.and_then(|z| z.floor_area_m2),
                volume_m3: zone.and_then(|z| z.volume_m3),
                mass_multiplier,
            }
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
) -> Result<Vec<BoundaryInput>> {
    use hares_envelope::PrecomputedRCLayer;
    use hares_io::envelope_lut::resolve_boundary_name;
    use hares_io::hpxml::{BoundaryType, ZoneType};
    use hares_physics::film_coefficients::{film_resistances, surface_roughness_from_finish_type};
    use hares_physics::ground::f2_coefficient;
    use hares_physics::solar::window_u_factor_decomposition;

    let envelope_lut = defaults.envelope_lut();

    building
        .boundaries
        .iter()
        .map(|bd| {
            let interior_zone_idx = find_zone_idx(building, bd.interior_zone.as_ref(), n_zones);
            let exterior = resolve_exterior(building, bd, n_zones);

            // Film resistances first -- needed to strip from assembly R-value.
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
                surface_roughness_from_finish_type(bd.finish_type.as_deref()),
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

            // ASHRAE F-factor perimeter method for slab-on-grade boundaries.
            // Replaces area-UA conduction with F2 × P × ΔT per ASHRAE HoF 2021
            // Ch. 18.31. The F-factor method accounts for 3-D edge heat flow
            // around the slab perimeter rather than 1-D conduction through the
            // full floor area, which can overstate loss by 2–2.5×.
            // Ref: ANSI/ASHRAE 90.1-2022 Table A6.3.1;
            // EnergyPlus Eng.Ref "Slab-on-grade and Underground Floors Defined
            // with F-factors".
            let precomputed_rc = if bd.boundary_type == BoundaryType::Slab {
                let perimeter_m = bd.perimeter_m.unwrap_or_else(|| {
                    // Square-plan approximation: P ≈ 4 × √(area).
                    // ASHRAE 90.1-2022 §A6.3 permits this as a fallback when
                    // exposed perimeter is not explicitly documented.
                    let derived = 4.0 * bd.area_m2.sqrt();
                    tracing::warn!(
                        slab_id = %bd.id,
                        area_m2 = bd.area_m2,
                        derived_perimeter_m = derived,
                        "slab <Perimeter> element not provided; using square-plan approximation P ≈ 4 × √(area)"
                    );
                    derived
                });
                let insulation_r = bd.perimeter_insulation_r_m2_k_w.unwrap_or(0.0);
                let f2 = f2_coefficient(insulation_r, false); // unheated residential slab
                let g_w_per_k = f2 * perimeter_m;

                // Q = F2 × P × (T_indoor - T_ground) → G = F2 × P [W/K].
                // build_precomputed_boundary adds film_int to the interior-side
                // resistor, so the layer resistance must be reduced by film_int
                // to keep total R from interior → ground = 1/(F2×P).
                let r_slab_m2_k_w = if g_w_per_k > 1e-9 && bd.area_m2 > 1e-9 {
                    (bd.area_m2 / g_w_per_k - r_film_int).max(1e-6)
                } else {
                    DEFAULT_R_M2_K_W
                };

                // Concrete slab thermal mass capacitance per unit area [kJ/(m²·K)].
                // Density 2400 kg/m³, Cp 880 J/(kg·K), thickness 0.1 m for typical
                // 4-inch residential slab. Ref: ASHRAE HoF 2021 Ch. 33, Table 1.
                // C = ρ × Cp × t / 1000 = 2400 × 880 × 0.1 / 1000 ≈ 211.2 kJ/(m²·K).
                const CONCRETE_DENSITY_KG_M3: f64 = 2400.0;
                const CONCRETE_CP_J_KG_K: f64 = 880.0;
                const TYPICAL_SLAB_THICKNESS_M: f64 = 0.1;
                let slab_cap_kj_m2_k = CONCRETE_DENSITY_KG_M3
                    * CONCRETE_CP_J_KG_K
                    * TYPICAL_SLAB_THICKNESS_M
                    / 1000.0;

                vec![PrecomputedRCLayer {
                    resistance_m2_k_w: r_slab_m2_k_w,
                    capacitance_kj_m2_k: slab_cap_kj_m2_k,
                }]
            } else if !bd.material_layers.is_empty() {
                // Skip LUT when the boundary has explicit material layers (BESTEST
                // synthetic configs define their own layer stack).
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
                        // LUT layers are exterior→interior (OCHRE CSV convention),
                        // matching boundary_rc's expected ordering.
                        let layers: Vec<_> = result
                            .layers
                            .into_iter()
                            .map(|l| PrecomputedRCLayer {
                                resistance_m2_k_w: l.resistance_m2_k_w,
                                capacitance_kj_m2_k: l.capacitance_kj_m2_k,
                            })
                            .collect();
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
                    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(u);
                    (r_glass, r_int, r_ext)
                } else {
                    tracing::warn!(
                        boundary = %bd.id,
                        "window boundary has no U-factor -- using generic film resistances"
                    );
                    (fallback_r, r_film_int, r_film_ext)
                }
            } else {
                (fallback_r, r_film_int, r_film_ext)
            };

            // For slab-on-grade boundaries, zero the exterior film resistance.
            // Ground is a fixed-temperature node — no convective exterior film
            // applies. The F-factor perimeter conductance G = F2 × P captures
            // the entire slab-to-ground pathway; adding an exterior film would
            // over-resist it. Per ASHRAE HoF 2021 Ch. 18.31.
            let r_film_ext = if bd.boundary_type == BoundaryType::Slab {
                0.0
            } else {
                r_film_ext
            };

            // Interior-facing longwave emissivity for star-mesh LWR conductance.
            // The E+ Simple Window Model Step 1 polynomial (E+ Eng.Ref
            // §Window Heat Transfer Calculations) was derived at glass thermal
            // emissivity ε = 0.84 (NFRC rating value). For total interior
            // coupling = h_si exactly, both the R_conv decomposition
            // (boundary_rc.rs) and the star-mesh radiation conductance
            // must use the same ε that was implicit in h_si. Using ε = 0.9
            // for the star-mesh would over-couple the window by
            // h_rad(0.9) − h_rad(0.84) ≈ 0.34 W/(m²·K).
            //
            // ASHRAE 140-2017 §5.3.1.9 specifies ε_ir = 0.9 for ALL interior
            // surfaces, but this applies to opaque surfaces with ε = 0.9.
            // Glass has a physical infrared emissivity of 0.84; specifying
            // 0.9 for glass LWR exchange is inconsistent with the U-factor
            // model that underlies the window boundary.
            // Reference: E+ Eng.Ref §Window Heat Transfer Calculations;
            // OCHRE Envelope.py uses ε = 0.84 for window radiation_frac.
            let interior_emissivity = if bd.boundary_type == BoundaryType::Window {
                EMISSIVITY_WINDOW // 0.84 glass thermal emissivity (NFRC)
            } else if bd.has_radiant_barrier && bd.interior_zone.as_ref() == Some(&ZoneType::Attic)
            {
                EMISSIVITY_RADIANT_BARRIER
            } else {
                bd.emittance.unwrap_or(EMISSIVITY_DEFAULT)
            };

            // Guard: SteelFrame without geometry on LUT-miss path.
            // ASHRAE HoF 2021 Ch. 27 requires the zone method for metal
            // framing. Without explicit <StudSpacing>, <StudWidth>, or
            // <FramingFactor>, and when the pre-computed construction LUT
            // does not match, the parallel-path method (which uses softwood
            // conductivity) cannot be applied correctly. Fail loudly rather
            // than silently understate the steel thermal bridge.
            if bd.construction_type.as_deref() == Some("SteelFrame")
                && bd.framing_factor.is_none()
                && precomputed_rc.is_empty()
            {
                return Err(HaresError::Io(format!(
                    "SteelFrame boundary '{}' requires explicit <StudSpacing> and <StudWidth> \
                     for the ASHRAE zone method (HoF 2021 Ch. 27); no stud geometry provided, \
                     no explicit <FramingFactor> found, and the pre-computed construction LUT \
                     did not match this assembly",
                    bd.id,
                )));
            }

            Ok(BoundaryInput {
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
                interior_emissivity,
                foundation_depth_m: bd.foundation_depth_m.unwrap_or(0.0),
            })
        })
        .collect::<std::result::Result<Vec<_>, HaresError>>()
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
        // Adjacent zones are rewritten to the non-Adjacent side's type during
        // HPXML parsing (building.rs:rewrite_adjacent_zone_pair). If an Adjacent
        // zone reaches this function, it indicates a bug in that rewrite.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                !matches!(target, hares_io::hpxml::ZoneType::Adjacent),
                "Adjacent zone type reached find_zone_idx — the rewrite in building.rs was not applied"
            );
        }
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
    let mut alpha_telemetry = env
        .equipment_telemetry
        .remove(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY)
        .unwrap_or_default();
    for chunk in payload.chunks_exact(5) {
        let zone_raw = chunk[0];
        let humidity_ratio = chunk[1];
        let relative_humidity = chunk[2];
        let wet_bulb_c = chunk[3];
        let alpha = chunk[4];

        if !zone_raw.is_finite() || zone_raw < 0.0 || zone_raw > f64::from(u16::MAX) {
            continue;
        }
        let zone_id = ZoneId(zone_raw as u16);
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.humidity_ratio = humidity_ratio;
            zone.relative_humidity = relative_humidity;
            zone.wet_bulb_c = wet_bulb_c;
        }
        let alpha_key = format!(
            "{}_zone_{}",
            hares_types::telemetry_keys::HUMIDITY_SEMI_IMPLICIT_ALPHA,
            zone_id.0
        );
        alpha_telemetry.insert(alpha_key, alpha);
    }
    env.equipment_telemetry.insert(
        hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY.to_string(),
        alpha_telemetry,
    );
}

pub(crate) fn equipment_config_from_spec(spec: &hares_io::EquipmentSpec) -> EquipmentConfig {
    if let Some(typed) = &spec.typed_config {
        return typed.clone();
    }

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

    let display_name = spec.instance_name.as_ref().unwrap_or(&spec.name).clone();
    EquipmentConfig::raw(display_name, spec.name.clone(), raw_config)
}

pub(crate) fn merged_equipment_config(
    spec: &hares_io::EquipmentSpec,
    overrides: &Value,
) -> EquipmentConfig {
    if let Some(typed) = &spec.typed_config
        && let ConfigPayload::Typed {
            type_name,
            version,
            data,
        } = &typed.payload
        && let Value::Object(base) = data
    {
        let mut merged = base.clone();
        apply_equipment_overrides(&mut merged, overrides, &spec.name);
        let mut eq_cfg = EquipmentConfig::with_payload(
            typed.name.clone(),
            typed.ochre_class.clone(),
            ConfigPayload::Typed {
                type_name: type_name.clone(),
                version: *version,
                data: Value::Object(merged),
            },
        );
        eq_cfg.setpoints_reconciled = typed.setpoints_reconciled.clone();
        return eq_cfg;
    }

    let mut merged = spec.parameters.clone();
    apply_equipment_overrides(&mut merged, overrides, &spec.name);
    let merged_spec = hares_io::EquipmentSpec {
        instance_name: spec.instance_name.clone(),
        name: spec.name.clone(),
        fuel_type: spec.fuel_type,
        parameters: merged,
        zip_params: spec.zip_params.clone(),
        typed_config: spec.typed_config.clone(),
        system_id: spec.system_id.clone(),
        related_hvac_idref: spec.related_hvac_idref.clone(),
        primary_role: spec.primary_role.clone(),
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
#[cfg(test)]
mod tests {
    use serde_json::json;

    use hares_equipment::{
        SetpointReconciliation,
        hvac::heating_config::{DuctConfig, GasFurnaceConfig},
    };
    use hares_io::hpxml::{Boundary, BoundaryType, Zone, ZoneType};
    use hares_types::FuelType;

    use super::{
        building_to_zone_inputs, find_zone_idx, mass_multiplier_for_zone, merged_equipment_config,
        zone_has_furniture_boundaries,
    };

    // ── mass_multiplier_for_zone tests ─────────────────────────────────

    #[test]
    fn mass_multiplier_conditioned_is_7() {
        assert!((mass_multiplier_for_zone(&ZoneType::Conditioned) - 7.0).abs() < 1e-12);
    }

    #[test]
    fn mass_multiplier_non_conditioned_is_1() {
        for zt in [
            ZoneType::Attic,
            ZoneType::Garage,
            ZoneType::Foundation,
            ZoneType::Outdoor,
            ZoneType::Ground,
            ZoneType::Adjacent,
            ZoneType::Other("Custom".to_string()),
        ] {
            assert!(
                (mass_multiplier_for_zone(&zt) - 1.0).abs() < 1e-12,
                "expected 1.0 for {:?}, got {}",
                zt,
                mass_multiplier_for_zone(&zt)
            );
        }
    }

    // ── zone_has_furniture_boundaries tests ────────────────────────────

    fn minimal_building(zones: Vec<Zone>, boundaries: Vec<Boundary>) -> hares_io::Building {
        hares_io::Building {
            site: hares_io::hpxml::Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
            },
            zones,
            boundaries,
            windows: Vec::new(),
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
            details_xml: hares_io::hpxml::building::XmlNode {
                name: "root".into(),
                attrs: Default::default(),
                text: String::new(),
                children: Vec::new(),
            },
        }
    }

    fn furniture_boundary(id: &str, zone_type: ZoneType) -> Boundary {
        Boundary {
            id: id.to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 10.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(zone_type.clone()),
            exterior_zone: Some(zone_type.clone()),
            material_layers: Vec::new(),
            framing_factor: None,
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(90.0),
            lut_boundary_name: None,
            floor_or_ceiling: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        }
    }

    #[test]
    fn furniture_boundaries_detected_for_conditioned() {
        let building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            vec![furniture_boundary(
                "conditioned_furniture",
                ZoneType::Conditioned,
            )],
        );
        assert!(zone_has_furniture_boundaries(
            &building,
            &ZoneType::Conditioned
        ));
        assert!(!zone_has_furniture_boundaries(&building, &ZoneType::Attic));
    }

    #[test]
    fn no_furniture_boundaries_returns_false() {
        let building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            Vec::new(),
        );
        assert!(!zone_has_furniture_boundaries(
            &building,
            &ZoneType::Conditioned
        ));
    }

    // ── building_to_zone_inputs furniture override tests ──────────────

    #[test]
    fn furniture_override_reduces_conditioned_multiplier_to_1() {
        let building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            vec![furniture_boundary(
                "conditioned_furniture",
                ZoneType::Conditioned,
            )],
        );
        let zone_inputs = building_to_zone_inputs(&building, 1);
        assert!(
            (zone_inputs[0].mass_multiplier - 1.0).abs() < 1e-12,
            "expected 1.0 (furniture override), got {}",
            zone_inputs[0].mass_multiplier
        );
    }

    #[test]
    fn no_furniture_uses_default_conditioned_multiplier() {
        let building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            Vec::new(),
        );
        let zone_inputs = building_to_zone_inputs(&building, 1);
        assert!(
            (zone_inputs[0].mass_multiplier - 7.0).abs() < 1e-12,
            "expected 7.0 (no furniture), got {}",
            zone_inputs[0].mass_multiplier
        );
    }

    #[test]
    fn mass_multiplier_override_takes_precedence_over_furniture() {
        let mut building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            vec![furniture_boundary(
                "conditioned_furniture",
                ZoneType::Conditioned,
            )],
        );
        building.mass_multiplier_override = Some(5.0);
        let zone_inputs = building_to_zone_inputs(&building, 1);
        assert!(
            (zone_inputs[0].mass_multiplier - 5.0).abs() < 1e-12,
            "expected 5.0 (explicit override), got {}",
            zone_inputs[0].mass_multiplier
        );
    }

    #[test]
    fn missing_zone_defaults_to_multiplier_1() {
        let building = minimal_building(Vec::new(), Vec::new());
        let zone_inputs = building_to_zone_inputs(&building, 1);
        assert!(
            (zone_inputs[0].mass_multiplier - 1.0).abs() < 1e-12,
            "expected 1.0 (no zone → air only), got {}",
            zone_inputs[0].mass_multiplier
        );
    }

    // ── equipment override tests ───────────────────────────────────────

    fn gas_furnace_spec() -> hares_io::EquipmentSpec {
        let typed_cfg = GasFurnaceConfig {
            equipment_id: Some(7),
            zone_id: Some(1),
            capacity_w: 12_000.0,
            afue: 0.82,
            fan_power_w: Some(350.0),
            number_of_speeds: 1,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        };
        let parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable furnace config")
            .as_object()
            .cloned()
            .expect("furnace config object");
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Furnace".to_string(),
            fuel_type: FuelType::Gas,
            parameters,
            zip_params: None,
            typed_config: Some(hares_equipment::EquipmentConfig::from_typed(
                "Gas Furnace".to_string(),
                "Gas Furnace".to_string(),
                typed_cfg,
            )),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn typed_equipment_override_rejects_unknown_keys() {
        let spec = gas_furnace_spec();
        let overrides = json!({
            "Gas Furnace": {
                "afuee": 0.96
            }
        });

        let merged = merged_equipment_config(&spec, &overrides);
        let err = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect_err("unknown override keys must fail");
        let msg = err.to_string();

        assert!(msg.contains("Gas Furnace"), "missing equipment name: {msg}");
        assert!(msg.contains("afuee"), "missing unknown key: {msg}");
        assert!(msg.contains("unknown field"), "missing serde error: {msg}");
    }

    #[test]
    fn typed_equipment_override_applies_known_keys() {
        let spec = gas_furnace_spec();
        let overrides = json!({
            "Gas Furnace": {
                "afue": 0.96
            }
        });

        let merged = merged_equipment_config(&spec, &overrides);
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("known override keys must deserialize");

        assert!((cfg.afue - 0.96).abs() < 1e-12);
        assert!((cfg.capacity_w - 12_000.0).abs() < 1e-12);
    }

    // ── setpoints_reconciled propagation through merged_equipment_config ─

    fn gas_furnace_spec_with_reconciliation(
        reconciliations: Vec<SetpointReconciliation>,
    ) -> hares_io::EquipmentSpec {
        let typed_cfg = GasFurnaceConfig {
            equipment_id: Some(7),
            zone_id: Some(1),
            capacity_w: 12_000.0,
            afue: 0.82,
            fan_power_w: Some(350.0),
            number_of_speeds: 1,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        };
        let parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable furnace config")
            .as_object()
            .cloned()
            .expect("furnace config object");
        let mut eq_cfg = hares_equipment::EquipmentConfig::from_typed(
            "Gas Furnace".to_string(),
            "Gas Furnace".to_string(),
            typed_cfg,
        );
        eq_cfg.setpoints_reconciled = Some(reconciliations);
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Furnace".to_string(),
            fuel_type: FuelType::Gas,
            parameters,
            zip_params: None,
            typed_config: Some(eq_cfg),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn merged_equipment_config_preserves_setpoints_reconciled() {
        let reconciliations = vec![
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
                original_cooling_c: [23.0; 24],
                adjusted_heating_c: [20.5; 24],
                adjusted_cooling_c: [22.5; 24],
            },
        ];
        let spec = gas_furnace_spec_with_reconciliation(reconciliations);
        let overrides = serde_json::Value::Object(serde_json::Map::new());

        let merged = merged_equipment_config(&spec, &overrides);

        let sr = merged.setpoints_reconciled.as_ref().expect(
            "setpoints_reconciled must propagate from typed config through merged_equipment_config",
        );
        assert_eq!(sr.len(), 2);
        assert_eq!(sr[0].day, "weekday");
        assert_eq!(
            sr[0].original_heating_c, [21.0; 24],
            "original heating setpoints must be preserved"
        );
        assert_eq!(sr[1].day, "weekend");
        assert_eq!(
            sr[1].adjusted_cooling_c, [22.5; 24],
            "adjusted cooling setpoints must be preserved"
        );
    }

    #[test]
    fn merged_equipment_config_preserves_setpoints_reconciled_none() {
        // Use the existing spec helper which has setpoints_reconciled = None.
        let spec = gas_furnace_spec();
        let overrides = serde_json::Value::Object(serde_json::Map::new());

        let merged = merged_equipment_config(&spec, &overrides);

        assert!(
            merged.setpoints_reconciled.is_none(),
            "setpoints_reconciled must be None when typed config has None"
        );
    }

    // ── apply_humidity_update_to_zones telemetry tests ──────────────────

    fn env_with_humidity_zone(zone_id: u16, humidity_ratio: f64) -> hares_types::EnvironmentState {
        use chrono::{FixedOffset, TimeZone};
        use hares_types::{
            ElectricalSummary, GridState, PriceSignal, SurfaceIrradiance, WeatherState, ZoneState,
        };
        hares_types::EnvironmentState {
            zones: vec![ZoneState {
                id: hares_types::ZoneId(zone_id),
                temperature_c: 22.0,
                humidity_ratio,
                relative_humidity: 0.45,
                wet_bulb_c: 19.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 180.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: PriceSignal::default(),
            electrical: ElectricalSummary::default(),
        }
    }

    #[test]
    fn humidity_update_emits_semi_implicit_alpha_to_telemetry() {
        let zone_id: u16 = 1;
        let alpha = 0.333;
        let mut env = env_with_humidity_zone(zone_id, 0.008);
        let update = hares_types::DomainUpdate {
            domain_id: hares_types::HUMIDITY,
            zone_temperatures_c: vec![(hares_types::ZoneId(zone_id), 22.0)],
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, alpha]),
        };

        super::apply_humidity_update_to_zones(&mut env, &update);

        let telem = env
            .equipment_telemetry
            .get(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY)
            .expect("HumiditySolver entry must exist in equipment_telemetry");
        let expected_key = format!(
            "{}_zone_{}",
            hares_types::telemetry_keys::HUMIDITY_SEMI_IMPLICIT_ALPHA,
            zone_id
        );
        let actual_alpha = telem.get(&expected_key).unwrap_or_else(|| {
            panic!("key '{expected_key}' must be present in HumiditySolver telemetry")
        });
        assert!(
            (actual_alpha - alpha).abs() < 1e-12,
            "alpha: expected {alpha}, got {actual_alpha}"
        );
    }

    #[test]
    fn humidity_update_emits_zero_alpha_when_no_infiltration() {
        let zone_id: u16 = 1;
        let mut env = env_with_humidity_zone(zone_id, 0.008);
        let update = hares_types::DomainUpdate {
            domain_id: hares_types::HUMIDITY,
            zone_temperatures_c: vec![(hares_types::ZoneId(zone_id), 22.0)],
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, 0.0]),
        };

        super::apply_humidity_update_to_zones(&mut env, &update);

        let telem = env
            .equipment_telemetry
            .get(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY)
            .expect("HumiditySolver entry must exist in equipment_telemetry");
        let expected_key = format!(
            "{}_zone_{}",
            hares_types::telemetry_keys::HUMIDITY_SEMI_IMPLICIT_ALPHA,
            zone_id
        );
        let actual_alpha = telem.get(&expected_key).unwrap_or_else(|| {
            panic!("key '{expected_key}' must be present in HumiditySolver telemetry")
        });
        assert!(
            actual_alpha.abs() < 1e-12,
            "alpha must be 0.0 when no infiltration, got {actual_alpha}"
        );
    }

    #[test]
    fn humidity_update_preserves_existing_humiditysolver_telemetry() {
        let zone_id: u16 = 1;
        let mut env = env_with_humidity_zone(zone_id, 0.008);
        let mut existing = hares_types::Telemetry::new();
        existing.insert("humidity_semi_implicit_alpha_zone_1".to_string(), 0.1);
        env.equipment_telemetry.insert(
            hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY.to_string(),
            existing,
        );

        let alpha = 0.5;
        let update = hares_types::DomainUpdate {
            domain_id: hares_types::HUMIDITY,
            zone_temperatures_c: vec![(hares_types::ZoneId(zone_id), 22.0)],
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, alpha]),
        };

        super::apply_humidity_update_to_zones(&mut env, &update);

        let telem = env
            .equipment_telemetry
            .get(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY)
            .unwrap();
        let updated_alpha = telem
            .get("humidity_semi_implicit_alpha_zone_1")
            .expect("existing key must still be present");
        assert!(
            (updated_alpha - alpha).abs() < 1e-12,
            "alpha should be updated from 0.1 to {alpha}, got {updated_alpha}"
        );
    }

    // ── slab F-factor integration tests ───────────────────────────────

    fn slab_boundary(perimeter_m: Option<f64>, insulation_r: Option<f64>) -> Boundary {
        Boundary {
            id: "slab-1".to_string(),
            boundary_type: BoundaryType::Slab,
            area_m2: 100.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Ground),
            material_layers: Vec::new(),
            framing_factor: None,
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(180.0),
            lut_boundary_name: None,
            floor_or_ceiling: None,
            perimeter_m,
            perimeter_insulation_r_m2_k_w: insulation_r,
            foundation_depth_m: None,
        }
    }

    /// A slab boundary without perimeter derives P ≈ 4 × √(area)
    /// and produces a precomputed RC layer with resistance matching F2 × P.
    #[test]
    fn slab_f_factor_derives_perimeter_from_area() {
        let building = hares_io::Building {
            boundaries: vec![slab_boundary(None, None)],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");

        assert_eq!(inputs.len(), 1);
        let slab_input = &inputs[0];
        // Should have exactly one precomputed RC layer (F-factor based)
        assert_eq!(
            slab_input.precomputed_rc.len(),
            1,
            "slab boundary should produce exactly one precomputed RC layer"
        );
        // Exterior film must be zero — ground is a fixed-temperature boundary
        assert_eq!(
            slab_input.r_film_exterior_m2_k_w, 0.0,
            "slab boundary must have zero exterior film"
        );
        // The RC layer capacitance should be non-zero (concrete thermal mass)
        assert!(
            slab_input.precomputed_rc[0].capacitance_kj_m2_k > 0.0,
            "slab should retain concrete thermal mass capacitance"
        );
        // The resistance should combine with film_int to give total R ≈ 1/(F2 × P)
        let expected_p = 4.0 * 100.0_f64.sqrt(); // derived perimeter ≈ 40 m
        let f2 = 1.263; // uninsulated unheated F2 per ASHRAE 90.1-2022 Table A6.3.1
        let g = f2 * expected_p;
        let expected_r_total = 1.0 / g; // total K/W from interior to ground
        let r_int_abs = slab_input.r_film_interior_m2_k_w / slab_input.area_m2;
        let r_layer_abs = slab_input.precomputed_rc[0].resistance_m2_k_w / slab_input.area_m2;
        let actual_r_total = r_layer_abs + r_int_abs;
        let tolerance = expected_r_total * 0.02; // 2% tolerance for rounding
        assert!(
            (actual_r_total - expected_r_total).abs() <= tolerance,
            "slab total R from indoor to ground should be ~{:.6} K/W (±2%), got {:.6}",
            expected_r_total,
            actual_r_total
        );
    }

    /// A slab boundary with explicit perimeter uses that value directly.
    #[test]
    fn slab_f_factor_uses_explicit_perimeter() {
        let perimeter_m = 30.0;
        let building = hares_io::Building {
            boundaries: vec![slab_boundary(Some(perimeter_m), None)],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");

        assert_eq!(inputs.len(), 1);
        let slab_input = &inputs[0];
        assert_eq!(slab_input.r_film_exterior_m2_k_w, 0.0);

        // G = F2 × P = 1.263 × 30 = 37.89 W/K per ASHRAE 90.1-2022 Table A6.3.1
        let f2 = 1.263;
        let g = f2 * perimeter_m;
        let expected_r_total = 1.0 / g;
        let r_int_abs = slab_input.r_film_interior_m2_k_w / slab_input.area_m2;
        let r_layer_abs = slab_input.precomputed_rc[0].resistance_m2_k_w / slab_input.area_m2;
        let actual_r_total = r_layer_abs + r_int_abs;
        let tolerance = expected_r_total * 0.02;
        assert!(
            (actual_r_total - expected_r_total).abs() <= tolerance,
            "slab total R with P={perimeter_m}m should be ~{:.6} K/W (±2%), got {:.6}",
            expected_r_total,
            actual_r_total
        );
    }

    /// Perimeter insulation reduces F2 coefficient and increases total resistance.
    #[test]
    fn slab_insulation_increases_resistance() {
        let perimeter_m = 30.0;

        let building_unins = hares_io::Building {
            boundaries: vec![slab_boundary(Some(perimeter_m), None)],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };
        let building_r5 = hares_io::Building {
            boundaries: vec![slab_boundary(
                Some(perimeter_m),
                Some(0.88), // R-5 perimeter insulation (SI)
            )],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };

        let store = load_defaults_store();
        let unins = super::building_to_boundary_inputs(&building_unins, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs unins");
        let r5 = super::building_to_boundary_inputs(&building_r5, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs r5");

        // Insulated slab should have higher layer resistance
        let unins_r = unins[0].precomputed_rc[0].resistance_m2_k_w;
        let r5_r = r5[0].precomputed_rc[0].resistance_m2_k_w;
        assert!(
            r5_r > unins_r,
            "R-5 perimeter insulation should increase resistance: unins={unins_r:.4}, R5={r5_r:.4}"
        );
    }

    /// Non-slab boundaries are unaffected by the slab F-factor path.
    #[test]
    fn wall_boundary_unaffected_by_slab_f_factor() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "wall-1".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: Some(3.0),
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: Some("Uninsulated".to_string()),
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };

        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");

        assert_eq!(inputs.len(), 1);
        // Wall should have a non-zero exterior film (outdoor convection)
        assert!(
            inputs[0].r_film_exterior_m2_k_w > 0.0,
            "wall boundary must have non-zero exterior film"
        );
    }

    /// SteelFrame without framing factor on the LUT-miss path must error.
    /// Regression for ticket 051 Defect 2: ASHRAE HoF 2021 Ch. 27 requires
    /// the zone method for metal framing. When no <StudSpacing>, <StudWidth>,
    /// <FramingFactor>, or LUT match is available, the boundary must fail
    /// loudly rather than silently applying the softwood parallel-path formula.
    #[test]
    fn steel_frame_lut_miss_without_framing_factor_errors() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "steel-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: Some(3.0),
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: Some("SteelFrame".to_string()),
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: Some("__test_no_match__".to_string()),
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let result = super::building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0);

        let err =
            result.expect_err("SteelFrame without framing factor on LUT-miss path must return Err");
        let msg = err.to_string();
        assert!(
            msg.contains("<StudSpacing>"),
            "error message must cite <StudSpacing>: {msg}",
        );
        assert!(
            msg.contains("<StudWidth>"),
            "error message must cite <StudWidth>: {msg}",
        );
    }

    /// Exterior film resistance varies with finish_type: stucco (VeryRough)
    /// must produce lower R_ext than vinyl siding (Smooth) at the same wind speed.
    /// This is the regression test for the hardcoded `SurfaceRoughness::Rough` bug.
    /// If `surface_roughness_from_finish_type` were bypassed, both boundaries
    /// would get identical R_ext and this test would fail.
    #[test]
    fn finish_type_roughness_changes_exterior_film_resistance() {
        let building = hares_io::Building {
            boundaries: vec![
                Boundary {
                    id: "wall-vinyl".to_string(),
                    boundary_type: BoundaryType::Wall,
                    area_m2: 20.0,
                    azimuth_deg: Some(180.0),
                    assembly_r_value_m2_k_w: Some(3.0),
                    r_value_layers_m2_k_w: Vec::new(),
                    interior_zone: Some(ZoneType::Conditioned),
                    exterior_zone: Some(ZoneType::Outdoor),
                    material_layers: Vec::new(),
                    framing_factor: None,
                    construction_type: None,
                    finish_type: Some("vinyl siding".to_string()),
                    insulation_details: None,
                    has_radiant_barrier: false,
                    solar_absorptance: None,
                    emittance: None,
                    tilt_deg: Some(90.0),
                    lut_boundary_name: None,
                    floor_or_ceiling: None,
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
                },
                Boundary {
                    id: "wall-stucco".to_string(),
                    boundary_type: BoundaryType::Wall,
                    area_m2: 20.0,
                    azimuth_deg: Some(180.0),
                    assembly_r_value_m2_k_w: Some(3.0),
                    r_value_layers_m2_k_w: Vec::new(),
                    interior_zone: Some(ZoneType::Conditioned),
                    exterior_zone: Some(ZoneType::Outdoor),
                    material_layers: Vec::new(),
                    framing_factor: None,
                    construction_type: None,
                    finish_type: Some("stucco".to_string()),
                    insulation_details: None,
                    has_radiant_barrier: false,
                    solar_absorptance: None,
                    emittance: None,
                    tilt_deg: Some(90.0),
                    lut_boundary_name: None,
                    floor_or_ceiling: None,
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
                },
            ],
            ..minimal_building(
                vec![Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 4.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");
        assert_eq!(inputs.len(), 2);
        let r_vinyl = inputs[0].r_film_exterior_m2_k_w;
        let r_stucco = inputs[1].r_film_exterior_m2_k_w;
        assert!(
            r_stucco < r_vinyl,
            "stucco (VeryRough, Rf=2.17) R_ext={r_stucco:.5} must be < vinyl siding (Smooth, Rf=1.11) R_ext={r_vinyl:.5}; \
             hardcoded Rough would give identical values"
        );
    }

    /// `find_zone_idx` must assert when passed an Adjacent zone type: the rewrite
    /// in `building.rs` should eliminate all Adjacent references before this
    /// function is called.
    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[should_panic(expected = "Adjacent zone type reached find_zone_idx")]
    fn find_zone_idx_panics_on_adjacent_input() {
        let building = minimal_building(
            vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            Vec::new(),
        );
        // Adjacent is filtered from the zones vec in building.rs and should
        // never reach find_zone_idx after the rewrite. This call asserts
        // that invariant.
        let _ = find_zone_idx(&building, Some(&ZoneType::Adjacent), 1);
    }

    /// Build a minimal DefaultsStore for tests that need one.
    fn load_defaults_store() -> hares_io::DefaultsStore {
        let defaults_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        hares_io::DefaultsStore::load(&defaults_path)
            .expect("DefaultsStore must be loadable from project defaults/ directory")
    }
}
