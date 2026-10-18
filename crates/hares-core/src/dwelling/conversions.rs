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
use hares_io::{Building, DefaultsStore, EnvelopeScales};
use hares_types::{DomainUpdate, EnvironmentState, ExecutionStage, HaresError, ZoneId};
use serde_json::{Map, Value};

use super::Result;

const DEFAULT_R_M2_K_W: f64 = hares_envelope::boundary_rc::DEFAULT_R_M2_K_W;

/// Convert building zones to envelope-crate ZoneInput.
///
/// Every zone's air capacitance takes the building's one temperature
/// capacitance multiplier, as OS-HPXML applies its
/// `ZoneCapacitanceMultiplier:ResearchSpecial` to all zones
/// (simcontrols.rb:27-28) alongside the furniture and partition-wall
/// InternalMass it also models (constructions.rb:1817, 1835).
pub fn building_to_zone_inputs(building: &Building, n_zones: usize) -> Vec<ZoneInput> {
    (0..n_zones)
        .map(|idx| {
            let zone = building.zones.get(idx);
            ZoneInput {
                floor_area_m2: zone.and_then(|z| z.floor_area_m2),
                volume_m3: zone.and_then(|z| z.volume_m3),
                temperature_capacitance_multiplier: building.temperature_capacitance_multiplier,
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
    use hares_io::hpxml::{BoundaryType, ZoneType};
    use hares_physics::film_coefficients::film_resistances;
    use hares_physics::ground::f2_coefficient;
    use hares_physics::solar::window_u_factor_decomposition;

    let envelope_lut = defaults.envelope_lut();

    building
        .boundaries
        .iter()
        .map(|bd| {
            let interior_zone_idx = find_zone_idx(building, Some(&bd.id), bd.interior_zone.as_ref(), n_zones)?;
            let exterior = resolve_exterior(building, bd, n_zones)?;

            // Film resistances first -- needed to strip from assembly R-value.
            let tilt_deg = bd.tilt_deg.unwrap_or(90.0);
            let interior_label = zone_type_to_label(&bd.id, bd.interior_zone.as_ref())?;
            let exterior_label = zone_type_to_label(&bd.id, bd.exterior_zone.as_ref())?;
            let (r_film_int, r_film_ext) = film_resistances(
                tilt_deg,
                interior_label,
                exterior_label,
                avg_wind_m_s,
                avg_ground_c,
                avg_ambient_c,
                boundary_outside_roughness(bd, exterior_label)?.0,
            );

            // fallback_r is material-only R (no film). HPXML AssemblyEffectiveRValue
            // includes film per spec, so subtract computed film R to get material-only.
            // NominalRValue layers are already material-only. Film R is added back in
            // boundary_rc.rs for all three construction paths.
            //
            // The declared assembly R is the opaque-R precedence's winner
            // (the numeric-fixes ledger's opaque-R precedence entry): on the
            // LUT path it sets the
            // matched row's layer resistances below, and here it feeds the
            // single-resistor fallback. Its old `.max(1e-6)` clamp is gone:
            // a declared R that leaves no material resistance is the
            // film-sum error checked on the LUT path, not a silent near-zero.
            let material_r = match bd.assembly_r_value_m2_k_w {
                Some(r) => Some(r - r_film_int - r_film_ext),
                None => {
                    let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                    if sum > 0.0 { Some(sum) } else { None }
                }
            };

            #[cfg(feature = "observe")]
            let used_default_r = material_r.is_none();

            let fallback_r = material_r
                .unwrap_or_else(|| {
                    tracing::warn!(
                        boundary_id = %bd.id,
                        "No R-value specified for boundary; applying default 2.5 m²·K/W (R-14 IP). Results may significantly understate heat loss."
                    );
                    DEFAULT_R_M2_K_W
                })
                .max(1e-6);

            // ASHRAE F-factor perimeter method for slab-on-grade boundaries.
            // Replaces area-UA conduction with F2 × P × ΔT per ASHRAE HoF 2021
            // Ch. 17. The F-factor method accounts for 3-D edge heat flow
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

                tracing::trace!(
                    slab_id = %bd.id,
                    area_m2 = bd.area_m2,
                    perimeter_m,
                    insulation_r_m2_k_w = insulation_r,
                    f2_w_per_m_k = f2,
                    conductance_w_per_k = g_w_per_k,
                    r_film_int_m2_k_w = r_film_int,
                    "slab F-factor perimeter method: ASHRAE 90.1-2022 Table A6.3.1"
                );

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
                const TYPICAL_SLAB_THICKNESS_M: f64 = 0.1;
                let slab_cap_kj_m2_k = hares_physics::constants::CONCRETE_DENSITY_KG_M3
                    * hares_physics::constants::CONCRETE_CP_J_KG_K
                    * TYPICAL_SLAB_THICKNESS_M
                    / 1000.0;

                tracing::trace!(
                    slab_id = %bd.id,
                    r_slab_layer_m2_k_w = r_slab_m2_k_w,
                    r_total_m2_k_w = r_slab_m2_k_w + r_film_int,
                    slab_cap_kj_m2_k,
                    "slab precomputed RC layer: single resistor node via F-factor perimeter method"
                );

                vec![PrecomputedRCLayer {
                    resistance_m2_k_w: r_slab_m2_k_w,
                    capacitance_kj_m2_k: slab_cap_kj_m2_k,
                }]
            } else if !bd.material_layers.is_empty() {
                // Skip LUT when the boundary has explicit material layers (BESTEST
                // synthetic configs define their own layer stack).
                Vec::new()
            } else {
                // The opaque-R precedence (the numeric-fixes ledger's
                // opaque-R precedence entry): the row selection is unchanged - by the
                // construction and finish filters, then the row whose
                // assembly R plus the convention films is nearest the
                // declared R - and a declared `AssemblyEffectiveRValue`
                // then sets the matched row's resistance: every layer
                // resistance of the row is multiplied by
                // `k = (declared R - convention films) / sum(row layer
                // resistances)`, so the layers plus the convention films
                // equal the declared R in the convention the declaration
                // and the row selection speak. The row's layer
                // capacitances are kept (they are mass, not resistance).
                // The convention films are the LUT's row-selection films,
                // not the weather's convection-only films: the declared
                // assembly R includes the standard films per HPXML, and
                // HARES's convection-only film sums (the radiative part
                // rides the LWR network) exceed legitimate declared
                // assembly R values - the corpus's low-R roofs and
                // floors among them - so the network's films cannot be
                // the scaling's or the error's measure.
                match envelope_lut {
                    Some(lut) => {
                        let boundary_name = bd
                            .lut_boundary_name
                            .clone()
                            .or_else(|| {
                                lut.resolve_name(
                                    &bd.boundary_type,
                                    bd.interior_zone.as_ref(),
                                    bd.exterior_zone.as_ref(),
                                )
                                .map(str::to_string)
                            });
                        match boundary_name {
                            Some(boundary_name) => {
                                // A declared R at or below the convention film
                                // sum leaves no material resistance: a
                                // construction error naming the boundary, the
                                // declared R and the film sum, where the old
                                // clamp scaled the LUT layers to almost no
                                // resistance without a word.
                                if let Some(declared) = bd.assembly_r_value_m2_k_w {
                                    let films = lut.convention_films(&boundary_name);
                                    if declared <= films {
                                        return Err(HaresError::Dwelling(format!(
                                            "boundary '{}': the declared assembly R-value {declared} m²·K/W \
                                             is at or below the film resistance sum {films} m²·K/W, so the \
                                             films alone leave no material resistance for the construction",
                                            bd.id
                                        )));
                                    }
                                }
                                let r_value = bd.assembly_r_value_m2_k_w.or_else(|| {
                                    let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                                    if sum > 0.0 { Some(sum) } else { None }
                                });
                                let result = lut.lookup(
                                    &boundary_name,
                                    bd.construction_type.as_deref(),
                                    bd.finish_type.as_deref(),
                                    bd.insulation_details.as_deref(),
                                    r_value,
                                );
                                match result {
                                    Some(result) => {
                                        tracing::debug!(
                                            boundary = boundary_name,
                                            layers = result.layers.len(),
                                            matched_type = %result.matched_boundary_type,
                                            "envelope LUT match"
                                        );
                                        // LUT layers are exterior→interior (OCHRE CSV convention),
                                        // matching boundary_rc's expected ordering.
                                        let mut layers: Vec<_> = result
                                            .layers
                                            .into_iter()
                                            .map(|l| PrecomputedRCLayer {
                                                resistance_m2_k_w: l.resistance_m2_k_w,
                                                capacitance_kj_m2_k: l.capacitance_kj_m2_k,
                                            })
                                            .collect();
                                        if let Some(declared) = bd.assembly_r_value_m2_k_w {
                                            let films = lut.convention_films(&boundary_name);
                                            let row_sum: f64 =
                                                layers.iter().map(|l| l.resistance_m2_k_w).sum();
                                            if row_sum > 0.0 {
                                                let k = (declared - films) / row_sum;
                                                for layer in &mut layers {
                                                    layer.resistance_m2_k_w *= k;
                                                }
                                                tracing::debug!(
                                                    boundary = boundary_name,
                                                    row = %result.matched_boundary_type,
                                                    declared_r_m2_k_w = declared,
                                                    k,
                                                    "opaque-R precedence: the declared assembly R \
                                                     scales the matched LUT row's layer resistances"
                                                );
                                            }
                                        }
                                        layers
                                    }
                                    None => {
                                        tracing::debug!(
                                            boundary = boundary_name,
                                            construction = ?bd.construction_type,
                                            finish = ?bd.finish_type,
                                            insulation = ?bd.insulation_details,
                                            r_value = ?r_value,
                                            "envelope LUT lookup miss"
                                        );
                                        Vec::new()
                                    }
                                }
                            }
                            None => Vec::new(),
                        }
                    }
                    None => Vec::new(),
                }
            };

            // Window / Skylight U-factor decomposition: EnergyPlus Simple Window
            // Model Step 1. Overrides fallback_r and film resistances for
            // fenestration boundaries.
            let is_fenestration = matches!(
                bd.boundary_type,
                BoundaryType::Window | BoundaryType::Skylight
            );
            let (fallback_r, r_film_int, r_film_ext) = if is_fenestration {
                let u_factor = building
                    .windows
                    .iter()
                    .chain(building.skylights.iter())
                    .find(|w| w.id == bd.id)
                    .and_then(|w| w.u_factor_w_m2_k);
                if let Some(u) = u_factor.filter(|&u| u > 0.0) {
                    let (r_glass, r_int, r_ext) = window_u_factor_decomposition(u)?;
                    (r_glass, r_int, r_ext)
                } else {
                    return Err(HaresError::Dwelling(format!(
                        "fenestration boundary '{}' has missing or non-positive U-factor; \
                         <UFactor> must be present with a positive value per HPXML §6.5",
                        bd.id
                    )));
                }
            } else {
                (fallback_r, r_film_int, r_film_ext)
            };

            // For slab-on-grade boundaries, zero the exterior film resistance.
            // Ground is a fixed-temperature node — no convective exterior film
            // applies. The F-factor perimeter conductance G = F2 × P captures
            // the entire slab-to-ground pathway; adding an exterior film would
            // over-resist it. Per ASHRAE HoF 2021 Ch. 17.
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
            let interior_emissivity = if matches!(
                bd.boundary_type,
                BoundaryType::Window | BoundaryType::Skylight
            ) {
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

            // Fenestration boundaries (windows, skylights) must not receive
            // pre-computed RC layers from the envelope LUT.  They use the
            // Window struct's U-factor code path (EnergyPlus Simple Window
            // Model Step 1), not the LUT.
            // Fenestration boundaries (windows, skylights) must not receive
            // pre-computed RC layers from the envelope LUT; they use the
            // Window struct's U-factor code path. Debug-build check of the
            // assembler logic.
            #[cfg(debug_assertions)]
            if matches!(
                bd.boundary_type,
                BoundaryType::Window | BoundaryType::Skylight
            ) {
                debug_assert!(
                    precomputed_rc.is_empty(),
                    "{} boundary '{}' received pre-computed RC layers from envelope LUT; \
                     fenestration must use the Window struct U-factor path (EnergyPlus Simple \
                     Window Model Step 1), not the LUT",
                    if bd.boundary_type == BoundaryType::Window { "Window" } else { "Skylight" },
                    bd.id,
                );
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
                #[cfg(feature = "observe")]
                used_default_r,
            })
        })
        .collect::<std::result::Result<Vec<_>, HaresError>>()
}

/// Map HPXML ZoneType to film-coefficient ZoneLabel.
///
/// An unrecognised zone type is an error naming the boundary and the input's
/// text: no label may be substituted for a location the input does not name,
/// the same rule `resolve_exterior` applies to the exterior side. An absent
/// zone type is the outdoors (a boundary the input gives no adjacency for,
/// which no attached wall derives one from).
pub(crate) fn zone_type_to_label(
    boundary_id: &str,
    zt: Option<&hares_io::hpxml::ZoneType>,
) -> Result<hares_physics::film_coefficients::ZoneLabel> {
    use hares_io::hpxml::ZoneType;
    use hares_physics::film_coefficients::ZoneLabel;
    Ok(match zt {
        Some(ZoneType::Conditioned) => ZoneLabel::Conditioned,
        Some(ZoneType::Attic) => ZoneLabel::Attic,
        Some(ZoneType::Garage) => ZoneLabel::Garage,
        Some(ZoneType::Foundation) => ZoneLabel::Foundation,
        Some(ZoneType::Ground) => ZoneLabel::Ground,
        Some(ZoneType::Adjacent) => ZoneLabel::Conditioned,
        Some(ZoneType::Outdoor) | None => ZoneLabel::Outdoor,
        Some(ZoneType::Other(raw)) => {
            return Err(HaresError::Dwelling(format!(
                "boundary '{boundary_id}': unrecognised zone type '{raw}'; no zone label \
                 may be substituted for a location the input does not name"
            )));
        }
    })
}

/// Find zone index by zone identity.
///
/// Primary path: match on boundary ID (in the zone's `attached_wall_ids`) AND
/// zone type. This disambiguates when multiple zones share the same `ZoneType`
/// (e.g. two `Conditioned` zones in a duplex). The boundary ID must be present
/// in the zone's `attached_wall_ids` — a relationship established during HPXML
/// parsing by `assign_walls_to_zones`.
///
/// Fallback: type-only matching via `position()`. Used when the boundary ID is
/// absent or the boundary is not tracked in `attached_wall_ids` (e.g. roofs,
/// floors, auto-generated interior walls).
///
/// An absent zone type is an error naming the boundary: a subsurface's
/// interior zone derives from its attached surface at parse (the OS-HPXML
/// subsurface rule), so a boundary reaching here with none names neither its
/// adjacency nor an attached surface, and no zone may be substituted for it.
pub(crate) fn find_zone_idx(
    building: &Building,
    boundary_id: Option<&str>,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> Result<usize> {
    // The zero-zone edge returns zone 0 without consulting the zone type:
    // every production caller passes at least one zone (the solver build's
    // `n_zones = env.zones.len().max(1)`, the synthetic builder's 1), so
    // the edge is unreachable for real input; it exists for the empty-zone
    // call shape, and zone 0 is the only index an empty zone list can name.
    // Pinned by find_zone_idx_zero_zones_returns_zero.
    if n_zones == 0 {
        return Ok(0);
    }
    let target = zone_type.ok_or_else(|| {
        let bid = boundary_id.unwrap_or("<unknown>");
        HaresError::Dwelling(format!(
            "boundary '{bid}': no interior zone; the boundary names neither its \
             interior adjacency nor a wall to derive it from, and no zone may \
             be substituted for a location the input does not name"
        ))
    })?;

    // Adjacent zones are rewritten to the non-Adjacent side's type during
    // HPXML parsing (building.rs:rewrite_adjacent_zone_pair). If an Adjacent
    // zone reaches this function, it indicates a bug in that rewrite:
    // an unconditional typed error (the function already returns Result).
    if matches!(target, hares_io::hpxml::ZoneType::Adjacent) {
        return Err(HaresError::Dwelling(format!(
            "Adjacent zone type reached find_zone_idx (boundary '{bid}'): the \
             rewrite in building.rs was not applied",
            bid = boundary_id.unwrap_or("<unknown>")
        )));
    }

    // Primary: match by boundary ID + zone type for disambiguation when
    // multiple zones share the same ZoneType. The boundary ID is tracked
    // in `attached_wall_ids` for Wall and FoundationWall boundaries.
    if let Some(bid) = boundary_id
        && let Some(idx) = building.zones.iter().position(|z| {
            z.zone_type == *target && z.attached_wall_ids.iter().any(|wid| wid == bid)
        })
    {
        return Ok(idx.min(n_zones - 1));
    }

    // Fallback: type-only matching for boundaries not tracked in
    // attached_wall_ids (roofs, floors, doors, windows, auto-generated
    // interior walls).
    if let Some(idx) = building.zones.iter().position(|z| z.zone_type == *target) {
        Ok(idx.min(n_zones - 1))
    } else {
        let bid = boundary_id.unwrap_or("<unknown>");
        tracing::warn!(
            boundary_id = %bid,
            zone_type = ?target,
            n_zones = n_zones,
            "No zone matches the boundary's interior zone type; failing build — no valid zone to connect to"
        );
        Err(HaresError::Dwelling(format!(
            "boundary '{bid}': no zone found for interior zone type {target:?} (n_zones={n_zones})"
        )))
    }
}

/// Log an `info` message when multiple zones share the same `ZoneType`,
/// indicating that zone resolution uses boundary ID + zone type matching
/// (via `attached_wall_ids`) rather than type-only matching.
pub(crate) fn check_multi_unit_zones(building: &Building) {
    let has_duplicate_type = building.zones.iter().enumerate().any(|(i, zi)| {
        building
            .zones
            .iter()
            .skip(i + 1)
            .any(|zj| zi.zone_type == zj.zone_type)
    });
    if has_duplicate_type {
        tracing::info!(
            n_zones = building.zones.len(),
            "Multiple zones share the same ZoneType; zone resolution uses boundary ID + \
             zone type matching (attached_wall_ids) rather than type-only matching"
        );
    }
}

pub(crate) fn resolve_exterior(
    building: &Building,
    boundary: &hares_io::hpxml::Boundary,
    n_zones: usize,
) -> Result<ExteriorTarget> {
    match boundary.exterior_zone.as_ref() {
        Some(hares_io::hpxml::ZoneType::Outdoor) => Ok(ExteriorTarget::Outdoor),
        Some(hares_io::hpxml::ZoneType::Ground) => Ok(ExteriorTarget::Ground),
        Some(hares_io::hpxml::ZoneType::Other(raw)) => Err(HaresError::Dwelling(format!(
            "boundary '{}': unrecognised exterior zone text '{raw}'; no zone may be \
             substituted for a location the input does not name",
            boundary.id
        ))),
        Some(zt) => {
            let idx = find_zone_idx(building, Some(&boundary.id), Some(zt), n_zones)?;
            Ok(ExteriorTarget::Zone(idx))
        }
        None => Err(HaresError::Dwelling(format!(
            "boundary '{}': no exterior zone; the boundary names neither its exterior \
             adjacency nor a wall to derive it from",
            boundary.id
        ))),
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

pub(crate) fn chrono_to_std_duration(duration: Duration) -> Result<StdDuration> {
    let millis = duration.num_milliseconds();
    if millis <= 0 {
        return Err(HaresError::Io(format!(
            "time resolution must be positive, got {millis} ms"
        )));
    }
    let ms_u64 = u64::try_from(millis)
        .map_err(|_| HaresError::Io("failed converting duration to u64 ms".to_string()))?;
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
    for chunk in payload.as_chunks::<7>().0 {
        let zone_raw = chunk[0];
        let humidity_ratio = chunk[1];
        // chunk[2] and chunk[3] are relative_humidity and wet_bulb_c — no longer
        // stored in ZoneState; they are derived from humidity_ratio + temperature_c
        // on every read via zone_relative_humidity / zone_wet_bulb_c.
        let alpha = chunk[4];
        // chunk[5] = condensation_mass_kg, chunk[6] = condensation_occurred_flag
        // — consumed by the invariant checker and observer, not needed here.

        if !zone_raw.is_finite() || zone_raw < 0.0 || zone_raw > f64::from(u16::MAX) {
            continue;
        }
        let zone_id = ZoneId(zone_raw as u16);
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.humidity_ratio = humidity_ratio;
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

pub(crate) fn equipment_config_from_spec(
    spec: &hares_io::EquipmentSpec,
) -> Result<EquipmentConfig> {
    if let Some(typed) = &spec.typed_config {
        // Typed payloads are #[serde(deny_unknown_fields)], so ZIP parameters
        // travel in the sidecar instead of the payload. Prefer the spec's
        // zip_params (the defaults/zip_parameters.toml lookup); keep any
        // sidecar already present on the typed config when the spec carries
        // none.
        let zip = spec.zip_params.or(typed.zip);
        return spec_config_from_typed(
            typed,
            spec,
            zip,
            hares_io::hpxml::OverrideLayers::default(),
        );
    }

    // A parameter a load reads must hold the kind it reads it as: any other
    // value (a string for a number, a null, an object) would read as absent
    // and run the load on its default. Other keys are the resolver's own
    // state, which the load does not read.
    let params = hares_equipment::raw_params_for_class(&spec.name);
    let lookup = |name: &str| spec.parameters.get(name);
    let mut raw_config: HashMap<String, ConfigValue> =
        HashMap::with_capacity(spec.parameters.len());
    for (key, value) in &spec.parameters {
        let config_value = json_value_to_config_value(value);
        if let Some(param) = params.and_then(|params| params.param(key, &lookup))
            && !config_value
                .as_ref()
                .is_some_and(|config_value| param.kind.holds(config_value))
        {
            return Err(HaresError::InvalidEquipmentParameter {
                equipment: spec.instance_name.as_ref().unwrap_or(&spec.name).clone(),
                key: key.clone(),
                reason: format!("must be {}, got {value}", param.kind.describe()),
            });
        }
        if let Some(config_value) = config_value {
            raw_config.insert(key.clone(), config_value);
        }
    }

    let display_name = spec.instance_name.as_ref().unwrap_or(&spec.name).clone();
    let mut cfg = EquipmentConfig::raw(display_name, spec.name.clone(), raw_config);
    // ZIP parameters travel exclusively in the sidecar for raw and typed
    // equipment alike; `hares_equipment::resolve_zip` is the single consumer.
    cfg.zip = spec.zip_params;
    Ok(cfg)
}

/// Land a spec's own typed overrides onto its typed payload as an override
/// layer, one key at a time, validating the payload against its typed
/// struct as each key lands.
///
/// This is the blueprint entrance's spec-level override channel: a
/// caller-built spec carries its overrides in
/// [`hares_io::EquipmentSpec::typed_overrides`], deliberately apart from
/// the parameter bag (the bag mixes the resolver's machinery keys with the
/// serialized payload and cannot diagnose a caller's typo), and every
/// override lands on the payload whether or not the payload's serialized
/// data carries the field: an override of an unset optional reaches the
/// schema. The conversion paths then read the landed payload as
/// the one config generation. The bag keeps the one encoding its producer
/// writes (the Python builders' human fuel spellings ("natural gas",
/// "electric"), the HPXML resolvers' raw text), and the landing translates
/// it into the canonical vocabulary the payload's serde deserializer
/// reads: a raw-text value the shared fuel parser can read lands in its
/// canonical form ("Gas"). A value that neither lands nor normalizes fails
/// the build naming the equipment, the field, and the offending value.
///
/// An override key the payload's schema does not read is an error naming
/// the equipment and the field: the bag is never consulted for overrides,
/// so nothing else can be mistaken for one. The reserved `equipment_id`
/// is the identity channel the assembly's assignment pass and the
/// four-way malformed-id classifier own, and a spec-bag id (malformed
/// included) must keep reaching that classifier, whose diagnosis names
/// the channel and the value.
///
/// A raw spec has no payload to land on and is returned unchanged: its
/// bag IS its config.
pub(crate) fn apply_spec_bag_to_typed_config(spec: &mut hares_io::EquipmentSpec) -> Result<()> {
    let Some(typed) = spec.typed_config.as_mut() else {
        return Ok(());
    };
    let (type_name, current) = match &typed.payload {
        ConfigPayload::Typed {
            type_name, data, ..
        } => (type_name.clone(), data.clone()),
        _ => return Ok(()),
    };
    let mut landed = match current {
        Value::Object(obj) => obj,
        _ => {
            return Err(HaresError::Equipment(format!(
                "equipment '{}': its typed payload's data is not a JSON object and \
                 cannot carry the spec's parameter overrides",
                spec.name
            )));
        }
    };
    let before = landed.clone();
    land_spec_parameters(&mut landed, spec, &type_name)?;
    if landed == before {
        // Nothing landed: the payload is the caller's own, unchanged, and
        // keeps the init-time semantics it always had (a payload the init
        // rejects fails there, stopping construction).
        return Ok(());
    }
    if let ConfigPayload::Typed { data, .. } = &mut spec
        .typed_config
        .as_mut()
        .expect("the typed config existed above and cannot have been removed in between")
        .payload
    {
        *data = Value::Object(landed);
    }
    Ok(())
}

/// Merge a spec's own typed overrides onto a typed payload's data as an
/// override layer, one key at a time.
///
/// An override key lands whether or not the payload's serialized data
/// carries it: the schema decides. Three key classes never land:
///
/// - `equipment_id`: the identity channel the assembly's assignment pass
///   and the four-way malformed-id classifier own.
/// - A key the payload's schema does not read: an error naming the
///   equipment and the key. The probe is the schema itself
///   ([`hares_equipment::typed_payload_reads`]), so an unset optional
///   (`Option` skipped in the serialized payload) is overridable while a
///   typo is rejected.
///
/// Each landing is validated against the typed struct before it is kept,
/// so the error for a rejected value names the field that failed (serde's
/// own path when the payload's schema can report one, the bag key when the
/// flattened heat-pump configs' schema cannot).
///
/// A raw-text value the shared fuel parser can read lands in its canonical
/// form: the bags' fuel vocabulary is the human/HPXML spelling the Python
/// builders and HPXML resolvers write (and the raw channel parses), while
/// the payload's serde deserializer reads only the canonical spellings. The
/// schema gates the normalization: a normalized form is kept only when the
/// payload then validates against it.
fn land_spec_parameters(
    landed: &mut Map<String, Value>,
    spec: &hares_io::EquipmentSpec,
    type_name: &str,
) -> Result<()> {
    let display_name = spec.instance_name.as_ref().unwrap_or(&spec.name);
    for (key, value) in &spec.typed_overrides {
        if key.as_str() == hares_equipment::config::KEY_EQUIPMENT_ID {
            continue;
        }
        reject_shadowed_wh_efficiency(type_name, landed, key, display_name)?;
        let reads =
            hares_equipment::typed_payload_reads(type_name, landed, key, value).map_err(|err| {
                HaresError::Equipment(format!(
                    "equipment '{display_name}': its typed config type '{type_name}' \
                     is not registered, so the override '{key}' cannot be checked: {err}"
                ))
            })?;
        if !reads {
            return Err(HaresError::InvalidEquipmentParameter {
                equipment: display_name.clone(),
                key: key.clone(),
                reason: format!(
                    "no field of the equipment's typed config '{type_name}' reads \
                     this key; the parameter bag's machinery keys are not overrides"
                ),
            });
        }
        let mut trial = landed.clone();
        let single: Map<String, Value> = [(key.clone(), value.clone())].into_iter().collect();
        hares_io::hpxml::nested_update(&mut trial, &single);
        if trial == *landed {
            // The override agrees with the payload: nothing to land, and the
            // payload keeps the init-time semantics it always had.
            continue;
        }
        match hares_equipment::validate_typed_payload_detailed(
            type_name,
            &Value::Object(trial.clone()),
        ) {
            Ok(()) => *landed = trial,
            Err(failure) => {
                let normalized_retry = match value
                    .as_str()
                    .and_then(hares_equipment::normalize_enum_text)
                {
                    Some(canonical) if canonical != *value => {
                        let mut retry = landed.clone();
                        let canonical_single: Map<String, Value> =
                            [(key.clone(), canonical)].into_iter().collect();
                        hares_io::hpxml::nested_update(&mut retry, &canonical_single);
                        match hares_equipment::validate_typed_payload_detailed(
                            type_name,
                            &Value::Object(retry.clone()),
                        ) {
                            Ok(()) => Some(retry),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
                match normalized_retry {
                    Some(retry) => *landed = retry,
                    None => {
                        return Err(HaresError::Equipment(format!(
                            "equipment '{display_name}': its typed config rejected the spec's \
                             parameters: '{}': {}",
                            failure.path.unwrap_or_else(|| key.to_string()),
                            failure.message,
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Build the typed [`EquipmentConfig`] for a spec from its payload plus the
/// dwelling-level override layer and one ZIP sidecar value, validating the
/// merged payload against the typed struct the payload's type name
/// registers when the overrides landed something.
///
/// The layers, lowest first: the typed payload's own data, then the
/// dwelling-level override layers that reach this spec (in the canonical
/// vocabulary the payload's serde deserializer reads, applied by
/// [`apply_typed_overrides`]). A merged result that fails its schema is an error naming the
/// equipment and the offending field when serde's path tracking can see it
/// (the flattened heat-pump configs' members cannot be pathed; their
/// failures carry the deserialization problem alone), at the conversion
/// that applied the override; a payload no override touched is returned
/// unvalidated and is checked at init, whose rejection fails the build.
fn spec_config_from_typed(
    typed: &EquipmentConfig,
    spec: &hares_io::EquipmentSpec,
    base_zip: Option<hares_types::zip::ZipLoad>,
    layers: hares_io::hpxml::OverrideLayers<'_>,
) -> Result<EquipmentConfig> {
    let ConfigPayload::Typed {
        type_name,
        version,
        data,
    } = &typed.payload
    else {
        // A spec carrying a non-typed EquipmentConfig is the raw channel;
        // the caller's raw branch handles it.
        return Err(HaresError::Equipment(format!(
            "equipment '{}': the spec's typed config does not carry a typed payload",
            spec.name
        )));
    };
    let Value::Object(mut merged) = data.clone() else {
        return Err(HaresError::Equipment(format!(
            "equipment '{}': its typed payload's data is not a JSON object and \
             cannot be override-merged",
            spec.name
        )));
    };
    let before_overrides = merged.clone();
    apply_typed_overrides(&mut merged, layers, type_name, &spec.name)?;
    // Validation rides the override delta: a value the schema rejects must
    // error at the merge that applied it, but a payload that carried its
    // defect before any override reached it keeps the init-time semantics
    // (the init fails the build): the merge changed nothing, so the merge
    // reports nothing.
    let overrides_landed = merged != before_overrides;
    // Peel the reserved "zip" override object (it travels inside the
    // override map) into the sidecar so deny_unknown_fields payloads never
    // see it; it is merged field-wise into the ZIP base.
    let zip_override = merged.remove("zip");
    let zip = merge_zip_override(
        base_zip,
        zip_override.as_ref(),
        &typed.ochre_class,
        &spec.name,
    )?;
    let merged_data = Value::Object(merged);
    if overrides_landed {
        hares_equipment::validate_typed_payload(type_name, &merged_data).map_err(|err| {
            HaresError::Equipment(format!(
                "equipment '{}': its typed config rejected the merged \
                 parameters: {err}",
                spec.instance_name.as_ref().unwrap_or(&spec.name)
            ))
        })?;
    }
    let display_name = spec
        .instance_name
        .clone()
        .unwrap_or_else(|| typed.name.clone());
    let mut eq_cfg = EquipmentConfig::with_payload(
        display_name,
        typed.ochre_class.clone(),
        ConfigPayload::Typed {
            type_name: type_name.clone(),
            version: *version,
            data: merged_data,
        },
    );
    eq_cfg.setpoints_reconciled = typed.setpoints_reconciled.clone();
    eq_cfg.zip = zip;
    Ok(eq_cfg)
}

/// Valid field names for the reserved `"zip"` override object, matching the
/// `ZipLoad` struct fields.
const ZIP_OVERRIDE_KEYS: &[&str] = &["zp", "ip", "pp", "zq", "iq", "pq", "pf", "v0"];

/// Merge a reserved `"zip"` override object field-wise over the effective
/// base ZIP for one equipment instance.
///
/// The base is, in precedence order: `base` (the spec's `zip_params` /
/// pre-existing sidecar), else the class-table defaults for `ochre_class`,
/// else `ZipLoad::constant_power()`. Partial overrides such as
/// `{"pf": 0.88}` replace only the named fields; all other fields are
/// inherited from the base. Returns `base` unchanged when there is no
/// override object.
///
/// A malformed override is a hard configuration error at dwelling build
/// time (matching the `deny_unknown_fields` ethos of typed configs): an
/// unknown key, a non-numeric value, or a non-object `"zip"` value all
/// fail loudly instead of being silently dropped.
fn merge_zip_override(
    base: Option<hares_types::zip::ZipLoad>,
    zip_override: Option<&Value>,
    ochre_class: &str,
    equipment_name: &str,
) -> Result<Option<hares_types::zip::ZipLoad>> {
    let obj = match zip_override {
        None => return Ok(base),
        Some(Value::Object(obj)) => obj,
        Some(other) => {
            return Err(HaresError::Equipment(format!(
                "equipment '{equipment_name}': \"zip\" override must be an object \
                 like {{\"pf\": 0.9}}, got: {other}"
            )));
        }
    };
    let mut zip = base
        .or_else(|| hares_types::zip::zip_defaults_for_class(ochre_class))
        .unwrap_or_else(hares_types::zip::ZipLoad::constant_power);
    for (key, value) in obj {
        let Some(v) = value.as_f64() else {
            return Err(HaresError::Equipment(format!(
                "equipment '{equipment_name}': non-numeric value for \"{key}\" in \
                 \"zip\" override: {value}"
            )));
        };
        match key.as_str() {
            "zp" => zip.zp = v,
            "ip" => zip.ip = v,
            "pp" => zip.pp = v,
            "zq" => zip.zq = v,
            "iq" => zip.iq = v,
            "pq" => zip.pq = v,
            "pf" => zip.pf = v,
            "v0" => zip.v0 = v,
            unknown => {
                return Err(HaresError::Equipment(format!(
                    "equipment '{equipment_name}': unknown field \"{unknown}\" in \
                     \"zip\" override; valid keys: {}",
                    ZIP_OVERRIDE_KEYS.join(", ")
                )));
            }
        }
    }
    // Magnitude bounds, not a point probe, at the override channel's
    // earliest stage: a cancelling row (e.g. zp = 1.79e308, ip = -1.79e308)
    // passes finiteness, the sum checks, and any single-voltage probe, but
    // NaNs real power across the service band, and an init that reads the
    // row fails the build for the same reason: the typo is rejected at the
    // earliest boundary that sees it. Shared bounds live in
    // `hares_types::zip` (single source of truth with the init backstop).
    hares_types::zip::validate_plausible_magnitudes(&zip).map_err(|err| {
        HaresError::Equipment(format!(
            "equipment '{equipment_name}': \"zip\" \
         override produced an implausible row: {err}"
        ))
    })?;
    Ok(Some(zip))
}

/// The config of `spec` with the dwelling's equipment overrides applied:
/// the wildcard's parameters the equipment reads, then its own entry.
///
/// # Errors
///
/// A malformed override map ([`hares_io::hpxml::override_layers`]), the
/// reserved `equipment_id` in a layer, a parameter the equipment cannot take
/// (a typed config's schema or a load's parameter list rejects it), or a
/// malformed `zip` override.
pub(crate) fn merged_equipment_config(
    spec: &hares_io::EquipmentSpec,
    overrides: &Value,
) -> Result<EquipmentConfig> {
    let Value::Object(root) = overrides else {
        return Err(non_object_overrides(overrides));
    };
    let layers = hares_io::hpxml::override_layers(root, &spec.name)?;
    if let Some(typed) = &spec.typed_config
        && matches!(typed.payload, ConfigPayload::Typed { .. })
    {
        return spec_config_from_typed(typed, spec, spec.zip_params.or(typed.zip), layers);
    }

    let mut merged = raw_load_parameters(spec, layers)?;
    remove_override_shadowed_setpoint_sources(&mut merged, layers, &spec.name);
    // Raw equipment honor the reserved "zip" override object too, for
    // consistency with typed equipment: it is merged field-wise over the
    // spec's zip_params base and folded back into `zip_params`, which
    // `equipment_config_from_spec` stores in the sidecar (the only ZIP
    // channel).
    let zip_override = merged.remove("zip");
    let zip_params = merge_zip_override(
        spec.zip_params,
        zip_override.as_ref(),
        &spec.name,
        &spec.name,
    )?;
    let merged_spec = hares_io::EquipmentSpec {
        instance_name: spec.instance_name.clone(),
        name: spec.name.clone(),
        fuel_type: spec.fuel_type,
        parameters: merged,
        typed_overrides: spec.typed_overrides.clone(),
        zip_params,
        typed_config: spec.typed_config.clone(),
        system_id: spec.system_id.clone(),
        related_hvac_idref: spec.related_hvac_idref.clone(),
        primary_role: spec.primary_role.clone(),
        heating_setpoint_source: spec.heating_setpoint_source.clone(),
        cooling_setpoint_source: spec.cooling_setpoint_source.clone(),
    };
    equipment_config_from_spec(&merged_spec)
}

/// The reserved override object that carries an equipment's ZIP
/// coefficients, which every equipment takes.
const RESERVED_ZIP_OVERRIDE: &str = "zip";

/// The reserved `envelope` override, deserialised and validated: the
/// scales the boundary construction multiplies its selected layers by.
/// Absent from the map (the setting `None` or the key absent), the unit
/// default. A malformed object (a non-object value, a field the struct
/// does not declare) or an out-of-bounds field is an error naming the key
/// and the field, at the assembly boundary before any side effect.
///
/// # Errors
///
/// [`HaresError::Dwelling`] naming the key and the offending field when
/// the object does not deserialise or its fields are out of bounds.
pub(crate) fn extract_envelope_scales(overrides: Option<&Value>) -> Result<EnvelopeScales> {
    let Some(value) = overrides.and_then(|root| root.get(hares_io::ENVELOPE_OVERRIDE_KEY)) else {
        return Ok(EnvelopeScales::default());
    };
    let scales: EnvelopeScales = serde_json::from_value(value.clone()).map_err(|err| {
        HaresError::Dwelling(format!(
            "the '{}' override rejected: {err}",
            hares_io::ENVELOPE_OVERRIDE_KEY
        ))
    })?;
    scales
        .validate()
        .map_err(|err| HaresError::Dwelling(err.to_string()))?;
    Ok(scales)
}

/// Apply the envelope scales to the boundary inputs, after the row
/// selection: the resistance scale multiplies every opaque layer's
/// resistance and the thermal-mass scale every such layer's capacitance,
/// on the LUT row's layers, the input's own material layers, and the
/// single-resistor fallback the no-layer path builds from. The films are
/// unscaled and the F-factor slab and the fenestration are outside both
/// scales: the slab's resistor is the perimeter method's, and a window's
/// resistance is its U-factor decomposition's.
pub(crate) fn apply_envelope_scales(
    building: &Building,
    inputs: &mut [hares_envelope::BoundaryInput],
    scales: &EnvelopeScales,
) {
    use hares_io::hpxml::BoundaryType;
    for (bd, input) in building.boundaries.iter().zip(inputs.iter_mut()) {
        if matches!(
            bd.boundary_type,
            BoundaryType::Slab | BoundaryType::Window | BoundaryType::Skylight
        ) {
            continue;
        }
        for layer in &mut input.precomputed_rc {
            layer.resistance_m2_k_w *= scales.opaque_r_scale;
            layer.capacitance_kj_m2_k *= scales.thermal_mass_scale;
        }
        for layer in &mut input.material_layers {
            // The layer's resistance is its thickness over its conductivity
            // and its capacitance its density by specific heat by thickness:
            // scaling the conductivity's inverse scales the resistance alone,
            // scaling the density the capacitance alone.
            layer.conductivity_w_m_k /= scales.opaque_r_scale;
            layer.density_kg_m3 *= scales.thermal_mass_scale;
        }
        input.fallback_r_m2_k_w *= scales.opaque_r_scale;
    }
}

/// The typed-config type name of the stationary battery.
const BATTERY_TYPE_NAME: &str = "Battery";

/// Every stationary battery's control must be declared where the dwelling
/// is assembled: HPXML has no battery-control element, and OCHRE resolves a
/// battery's control per step from a control signal or its schedule
/// (Equipment/Generator.py:27, Equipment/Battery.py:205), never from
/// HPXML. A battery resolved with no `bms_mode` would otherwise take the
/// `Manual` default silently, seed no management actor, and idle for the
/// whole run while looking configured. The override map is the one route a
/// declared control has into assembly (a `BatteryManagement` actor joins a
/// built dwelling and drives the battery from outside the input), so a
/// battery whose merged config still carries no `bms_mode` fails here,
/// named with its input.
///
/// Runs before any equipment is constructed so the build fails before side
/// effects, not partway through the population.
pub(crate) fn validate_battery_controls(
    specs: &[hares_io::EquipmentSpec],
    overrides: &Value,
    hpxml_display: &str,
) -> Result<()> {
    for spec in specs {
        let Some(typed) = &spec.typed_config else {
            continue;
        };
        let ConfigPayload::Typed { type_name, .. } = &typed.payload else {
            continue;
        };
        if type_name != BATTERY_TYPE_NAME {
            continue;
        }
        let merged = merged_equipment_config(spec, overrides)?;
        let display = spec
            .instance_name
            .as_deref()
            .unwrap_or(&typed.name)
            .to_string();
        let system = spec.system_id.as_deref().unwrap_or("unknown");
        let battery: hares_equipment::BatteryConfig = merged.typed().map_err(|err| {
            HaresError::Equipment(format!(
                "battery '{display}' (SystemIdentifier '{system}', input '{}'): \
                 its typed config cannot be read: {err}",
                hpxml_display
            ))
        })?;
        if battery.bms_mode.is_some() {
            continue;
        }
        return Err(HaresError::Equipment(format!(
            "battery '{display}' (SystemIdentifier '{system}', input '{}'): \
             no control is declared for it and HPXML has none to resolve; \
             built as is, the battery takes the `Manual` default, seeds no \
             management actor and idles for the whole run. Declare \
             `bms_mode` on the battery's override (\"Manual\" follows \
             commands) or add a `BatteryManagement` actor targeting \
             '{display}'",
            hpxml_display
        )));
    }
    Ok(())
}

fn non_object_overrides(overrides: &Value) -> HaresError {
    HaresError::Equipment(format!(
        "equipment overrides payload must be a JSON object mapping \
         equipment names to override fields, got: {overrides}; a \
         non-object payload can never match an equipment name and \
         would silently no-op every override"
    ))
}

/// Rejects the reserved `equipment_id` in the override layer `source`:
/// equipment ids are assigned by the dwelling.
fn reject_reserved_key(source: &str, key: &str, equipment: &str) -> Result<()> {
    if key == hares_equipment::config::KEY_EQUIPMENT_ID {
        return Err(HaresError::InvalidEquipmentParameter {
            equipment: equipment.to_string(),
            key: key.to_string(),
            reason: format!(
                "in the '{source}' override is rejected: equipment ids are \
                 assigned by the dwelling, not configurable; remove the field"
            ),
        });
    }
    Ok(())
}

/// The value a load's parameter `key` has once the override layers that
/// reach it are applied: its own entry's, else the wildcard's, else its
/// resolved value.
fn layered_value<'v>(
    resolved: &'v Map<String, Value>,
    layers: hares_io::hpxml::OverrideLayers<'v>,
    key: &str,
) -> Option<&'v Value> {
    layers
        .own
        .and_then(|own| own.get(key))
        .or_else(|| layers.wildcard.and_then(|(_, wildcard)| wildcard.get(key)))
        .or_else(|| resolved.get(key))
}

/// Whether the typed config `type_name` with payload `data` has the field
/// `key` (given `value`).
fn typed_reads(
    type_name: &str,
    data: &Map<String, Value>,
    key: &str,
    value: &Value,
    equipment: &str,
) -> Result<bool> {
    hares_equipment::typed_payload_reads(type_name, data, key, value)
        .map_err(|err| HaresError::Equipment(format!("equipment '{equipment}': {err}")))
}

/// Whether the equipment `spec` reads the wildcard parameter `key` (given
/// `value`) once the override layers of `root` that reach it are applied: a
/// typed config whose schema has the field, or a raw-parameter load whose
/// list holds it. A raw spec of a class no list declares vouches for no
/// parameter.
fn spec_reads(
    spec: &hares_io::EquipmentSpec,
    root: &Map<String, Value>,
    key: &str,
    value: &Value,
) -> Result<bool> {
    if let Some(EquipmentConfig {
        payload: ConfigPayload::Typed {
            type_name, data, ..
        },
        ..
    }) = &spec.typed_config
    {
        return match data.as_object() {
            Some(data) => typed_reads(type_name, data, key, value, &spec.name),
            None => Ok(false),
        };
    }
    let Some(params) = hares_equipment::raw_params_for_class(&spec.name) else {
        return Ok(false);
    };
    let layers = hares_io::hpxml::override_layers(root, &spec.name)?;
    Ok(params.reads(key, &|name| layered_value(&spec.parameters, layers, name)))
}

/// Checks the wildcard override (`all` or `*`) against the whole
/// population: every parameter it gives is read by some equipment of the
/// dwelling. Each equipment then takes the wildcard parameters it reads and
/// skips the rest, which other equipment read.
///
/// # Errors
///
/// Both wildcard spellings, a wildcard that is not an object, the reserved
/// `equipment_id`, a fraction under both of its spellings, or a parameter no
/// equipment reads (a misspelling).
pub(crate) fn validate_wildcard_override(
    overrides: &Value,
    population: &[&hares_io::EquipmentSpec],
) -> Result<()> {
    let Value::Object(root) = overrides else {
        return Err(non_object_overrides(overrides));
    };
    let Some((source, wildcard)) = hares_io::hpxml::wildcard_override(root)? else {
        return Ok(());
    };
    let every_equipment = format!("every equipment (the '{source}' override)");
    hares_equipment::check_one_gain_spelling(wildcard, &every_equipment)?;
    for (key, value) in wildcard {
        reject_reserved_key(source, key, &every_equipment)?;
        let key = hares_equipment::canonical_gain_key(key);
        if key == RESERVED_ZIP_OVERRIDE {
            continue;
        }
        let mut read = false;
        for spec in population {
            if spec_reads(spec, root, key, value)? {
                read = true;
                break;
            }
        }
        if read {
            continue;
        }
        return Err(HaresError::InvalidEquipmentParameter {
            equipment: every_equipment,
            key: key.to_string(),
            reason: format!("in the '{source}' override is read by no equipment of this dwelling"),
        });
    }
    Ok(())
}

/// The parameters of a raw-parameter load with the overrides that reach it
/// applied: the wildcard's parameters the load reads once every layer is
/// applied (the rest are other equipment's, checked by
/// [`validate_wildcard_override`]), then its own entry, each HPXML gain
/// spelling rewritten to the parameter it stands for so that it replaces
/// the resolved value.
///
/// # Errors
///
/// The reserved `equipment_id`; a fraction under both spellings in one
/// layer; a parameter of its own entry the load does not read (named in the
/// error with the list it reads). A raw spec of a class no list declares
/// takes the whole wildcard and its own entry unchecked, and vouches for no
/// wildcard parameter.
fn raw_load_parameters(
    spec: &hares_io::EquipmentSpec,
    layers: hares_io::hpxml::OverrideLayers<'_>,
) -> Result<Map<String, Value>> {
    use hares_equipment::canonical_gain_key;
    use hares_io::hpxml::nested_insert;

    let params = hares_equipment::raw_params_for_class(&spec.name);
    let mut merged = spec.parameters.clone();
    hares_equipment::canonicalize_gain_params(&mut merged, &spec.name)?;
    if let Some((source, wildcard)) = layers.wildcard {
        hares_equipment::check_one_gain_spelling(wildcard, &spec.name)?;
        let lookup = |name: &str| layered_value(&spec.parameters, layers, name);
        for (key, value) in wildcard {
            reject_reserved_key(source, key, &spec.name)?;
            let key = canonical_gain_key(key);
            if key == RESERVED_ZIP_OVERRIDE
                || params.is_none_or(|params| params.reads(key, &lookup))
            {
                nested_insert(&mut merged, key, value);
            }
        }
    }
    let Some(own) = layers.own else {
        return Ok(merged);
    };
    hares_equipment::check_one_gain_spelling(own, &spec.name)?;
    for (key, value) in own {
        reject_reserved_key(&spec.name, key, &spec.name)?;
        nested_insert(&mut merged, canonical_gain_key(key), value);
    }
    if let Some(params) = params
        && let Some(key) = own.keys().map(|key| canonical_gain_key(key)).find(|key| {
            *key != RESERVED_ZIP_OVERRIDE && !params.reads(key, &|name| merged.get(name))
        })
    {
        return Err(HaresError::InvalidEquipmentParameter {
            equipment: spec.name.clone(),
            key: key.to_string(),
            reason: format!(
                "in the '{}' override is not a parameter a {} reads; it reads: {}",
                spec.name,
                params.kind,
                params.describe()
            ),
        });
    }
    Ok(merged)
}

/// Applies the overrides that reach a typed config to its payload: the
/// wildcard's parameters its schema has (the rest are other equipment's,
/// checked by [`validate_wildcard_override`]), then its own entry, which
/// the schema validates after the merge.
fn apply_typed_overrides(
    merged: &mut Map<String, Value>,
    layers: hares_io::hpxml::OverrideLayers<'_>,
    type_name: &str,
    equipment: &str,
) -> Result<()> {
    use hares_io::hpxml::nested_insert;

    if let Some((source, wildcard)) = layers.wildcard {
        for (key, value) in wildcard {
            reject_reserved_key(source, key, equipment)?;
            reject_shadowed_wh_efficiency(type_name, merged, key, equipment)?;
            if key == RESERVED_ZIP_OVERRIDE
                || typed_reads(type_name, merged, key, value, equipment)?
            {
                nested_insert(merged, key, value);
            }
        }
    }
    for (key, value) in layers.own.into_iter().flatten() {
        reject_reserved_key(equipment, key, equipment)?;
        reject_shadowed_wh_efficiency(type_name, merged, key, equipment)?;
        nested_insert(merged, key, value);
    }
    remove_override_shadowed_setpoint_sources(merged, layers, equipment);
    Ok(())
}

/// The `setpoint` block's sides and the config key each side's schedule
/// source is read from: the thermostat reads the source ahead of the
/// constant setpoint (`hvac_core.rs` init's "Build setpoint sources: CSV
/// column > daily profile > constant"), so a constant setpoint an override
/// layer sets could never apply while the resolution's source stood. The
/// merge removes the matching source when the override sets the constant,
/// making the override the tier the engine reads.
const SETPOINT_SOURCE_SHADOWS: [(&str, &str); 2] = [
    ("heating", "heating_setpoint_source"),
    ("cooling", "cooling_setpoint_source"),
];

/// The constant setpoint, in Celsius, that the override layers set on one
/// thermostat side, when they set one: the equipment's own entry's
/// `setpoint` object first, then the wildcard's. A `null` value does not
/// set a constant (it unsets one), and a non-numeric value fails the
/// merged payload's schema at the validation that follows the merge.
pub(crate) fn override_constant_setpoint_value(
    layers: hares_io::hpxml::OverrideLayers<'_>,
    prefix: &str,
) -> Option<f64> {
    let key = format!("{prefix}_setpoint_c");
    let constant = |layer: &Map<String, Value>| {
        layer
            .get("setpoint")
            .and_then(Value::as_object)
            .and_then(|sp| sp.get(&key))
            .and_then(Value::as_f64)
    };
    layers
        .own
        .and_then(constant)
        .or_else(|| layers.wildcard.and_then(|(_, wildcard)| constant(wildcard)))
}

/// Remove the merged config's per-side schedule sources that a constant
/// setpoint the override layer set shadows, logging each replacement at
/// debug. Runs after the override layers land, so the constant the layer
/// set is the tier the engine's setpoint chain reads.
fn remove_override_shadowed_setpoint_sources(
    merged: &mut Map<String, Value>,
    layers: hares_io::hpxml::OverrideLayers<'_>,
    equipment: &str,
) {
    let Some(sp_obj) = merged.get_mut("setpoint").and_then(Value::as_object_mut) else {
        return;
    };
    for (prefix, source_key) in SETPOINT_SOURCE_SHADOWS {
        if override_constant_setpoint_value(layers, prefix).is_some()
            && sp_obj.remove(source_key).is_some()
        {
            tracing::debug!(
                equipment,
                side = prefix,
                removed_source = source_key,
                "a constant setpoint override replaces the schedule source"
            );
        }
    }
}

/// The coldest and warmest value a surviving schedule source can read,
/// when its year of values is knowable from the config alone: a
/// constant's own value; a daily profile's or a solar-aware profile's
/// extremes over its fractions, monthly multipliers and maximum (the
/// product `value_at` evaluates). `None` for the kinds whose values the
/// runtime reads from data the config does not carry (a schedule-file
/// column and the time windows), which the runtime's cross-tier check
/// catches instead.
fn schedule_source_extremes(source: &hares_types::ScheduleSourceConfig) -> Option<(f64, f64)> {
    use hares_types::ScheduleSourceConfig as S;
    fn extremes<'a>(
        fractions: impl Iterator<Item = &'a f64>,
        multipliers: &'a [f64],
        max_value: f64,
    ) -> Option<(f64, f64)> {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for fraction in fractions {
            for multiplier in multipliers {
                let value = fraction * multiplier * max_value;
                min = min.min(value);
                max = max.max(value);
            }
        }
        Some((min, max))
    }
    match source {
        S::Constant(value) => Some((*value, *value)),
        S::DailyProfile {
            weekday,
            weekend,
            month_multipliers,
            max_value,
        } => extremes(
            weekday.iter().chain(weekend.iter()),
            month_multipliers,
            *max_value,
        ),
        S::SolarAware {
            daytime_fraction,
            evening_fraction,
            overnight_fraction,
            month_multipliers,
            max_value,
            ..
        } => extremes(
            [*daytime_fraction, *evening_fraction, *overnight_fraction].iter(),
            month_multipliers,
            *max_value,
        ),
        S::ColumnRef { .. } | S::TimeWindows { .. } => None,
    }
}

/// The cross-tier deadband warnings a merged config's setpoint block
/// carries: a constant setpoint (an override layer's among them) named on
/// one side against the surviving schedule source on the other, whose
/// knowable coldest or warmest step crosses the constant. The init's
/// deadband reconciliation reads the static pair alone, so this is the
/// construction-time check that can see the surviving tier; the run's
/// cross-tier count is the backstop for the kinds this scan cannot read.
pub(crate) fn cross_tier_setpoint_warnings(
    equipment: &str,
    config: &hares_equipment::EquipmentConfig,
) -> Vec<hares_types::Warning> {
    use hares_types::ScheduleSourceConfig;

    let hares_equipment::ConfigPayload::Typed { data, .. } = &config.payload else {
        return Vec::new();
    };
    let Some(sp) = data.get("setpoint").and_then(Value::as_object) else {
        return Vec::new();
    };
    let named_constant = |side: &str| {
        sp.get(&format!("{side}_setpoint_c"))
            .and_then(Value::as_f64)
    };
    let surviving_source = |side: &str| {
        sp.get(&format!("{side}_setpoint_source"))
            .and_then(|value| serde_json::from_value::<ScheduleSourceConfig>(value.clone()).ok())
    };
    let mut warnings = Vec::new();
    // Heating named against the surviving cooling schedule: the schedule's
    // coldest step below the constant asks heating and cooling at once.
    if let (Some(heating_c), (min_c, _)) = (
        named_constant("heating"),
        surviving_source("cooling")
            .and_then(|source| schedule_source_extremes(&source))
            .unwrap_or((f64::INFINITY, f64::NEG_INFINITY)),
    ) && min_c < heating_c
    {
        warnings.push(hares_types::Warning::new(
            equipment,
            format!(
                "the heating setpoint constant {heating_c} °C crosses the surviving cooling \
                 schedule's coldest step {min_c} °C: the tiers ask heating and cooling at \
                 once, the collision the init's static check cannot see; the run counts \
                 every step both deliver"
            ),
        ));
    }
    // Cooling named against the surviving heating schedule: the schedule's
    // warmest step above the constant asks both at once.
    if let (Some(cooling_c), (_, max_c)) = (
        named_constant("cooling"),
        surviving_source("heating")
            .and_then(|source| schedule_source_extremes(&source))
            .unwrap_or((f64::INFINITY, f64::NEG_INFINITY)),
    ) && max_c > cooling_c
    {
        warnings.push(hares_types::Warning::new(
            equipment,
            format!(
                "the cooling setpoint constant {cooling_c} °C crosses the surviving heating \
                 schedule's warmest step {max_c} °C: the tiers ask heating and cooling at \
                 once, the collision the init's static check cannot see; the run counts \
                 every step both deliver"
            ),
        ));
    }
    warnings
}

/// The gas water heater reads `conversion_efficiency` ahead of the EF/UEF
/// test-procedure fields, so an override of those could never apply on a
/// config that carries the conversion efficiency: it is rejected naming the
/// field that shadows it instead of being accepted and silently ignored.
fn reject_shadowed_wh_efficiency(
    type_name: &str,
    merged: &Map<String, Value>,
    key: &str,
    equipment: &str,
) -> Result<()> {
    if type_name != "Gas Water Heater" || (key != "energy_factor" && key != "uniform_energy_factor")
    {
        return Ok(());
    }
    if merged
        .get("conversion_efficiency")
        .is_some_and(|value| !value.is_null())
    {
        return Err(HaresError::Equipment(format!(
            "equipment '{equipment}': an override of '{key}' cannot apply while the \
             config carries 'conversion_efficiency', which the gas water heater reads \
             ahead of '{key}'; override 'conversion_efficiency' or remove it from the config"
        )));
    }
    Ok(())
}

/// Validate equipment-override keys against the equipment population at
/// the assembly boundary where the population is known.
///
/// Every key must be the `"all"`/`"*"` wildcard or the name of an
/// equipment whose config is built through the override merge. An unknown
/// key — e.g. an HPXML `SystemIdentifier` like `"ReproEV1"`, which never
/// matches a spec name (overrides are matched by `spec.name`, and the EV
/// resolver names every EV spec `"EV"` regardless of its
/// SystemIdentifier) — was previously a silent no-op: the override quietly
/// missed and the equipment ran its defaults, exactly the
/// silent-substitution failure mode the loud-error rule forbids at parse
/// and assembly boundaries. `unhandled_names` are spec names that exist in
/// the population but are consumed outside the registry (no override
/// channel) — an override keyed by one of those is the same silent no-op
/// and is rejected with the reason.
pub(crate) fn validate_equipment_override_keys(
    overrides: &Value,
    overridable_names: &[&str],
    unhandled_names: &[&str],
) -> Result<()> {
    // A non-object payload (a bare string, array, or number — reachable
    // from the Python surface, which converts any Python object into the
    // `overrides` field) can never name an equipment, so it would silently
    // no-op the entire override channel: `overrides = "ReproEV1"` is the
    // same silent miss an unknown key produces, opened by a payload type
    // the per-key loop below never sees keys from. The canonical
    // "no overrides" is `overrides: None` (upstream wraps it into an
    // empty object); `Some(Null)` is a misconfiguration of the same class
    // and is rejected with everything else non-object. The same rule the
    // `zip` override channel already enforces ("must be an object", above).
    let Value::Object(root) = overrides else {
        return Err(non_object_overrides(overrides));
    };
    for key in root.keys() {
        if hares_io::hpxml::WILDCARD_OVERRIDE_KEYS.contains(&key.as_str()) {
            continue;
        }
        // The reserved `envelope` key is not an equipment name: the
        // envelope scales it names are extracted and validated at the
        // assembly boundary, and the boundary construction applies them.
        if key == hares_io::ENVELOPE_OVERRIDE_KEY {
            continue;
        }
        if overridable_names.contains(&key.as_str()) {
            continue;
        }
        if unhandled_names.contains(&key.as_str()) {
            return Err(HaresError::Equipment(format!(
                "equipment override key '{key}' names a spec handled outside \
                 the equipment override path and cannot be overridden; \
                 overridable equipment names: {}",
                overridable_names.join(", ")
            )));
        }
        return Err(HaresError::Equipment(format!(
            "unknown equipment override key '{key}': overrides are matched by \
             equipment name, not SystemIdentifier; overridable equipment \
             names: {}",
            overridable_names.join(", ")
        )));
    }
    Ok(())
}

/// Map a boundary's interior or exterior zone to a solver zone index.
///
/// Primary path: match on boundary ID + zone type via `find_zone_idx`, which
/// uses `attached_wall_ids` for disambiguation when multiple zones share the
/// same `ZoneType`. Fallback: type-only matching for untracked boundaries.
pub(crate) fn boundary_zone_index(
    building: &Building,
    boundary_id: Option<&str>,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> Result<usize> {
    find_zone_idx(building, boundary_id, zone_type, n_zones)
}

pub(crate) fn duration_to_u32_secs(duration: Duration) -> Result<u32> {
    let secs = duration.num_seconds();
    if secs <= 0 {
        return Err(HaresError::Io(format!(
            "duration must be positive seconds, got {secs}"
        )));
    }
    u32::try_from(secs)
        .map_err(|_| HaresError::Io(format!("duration seconds exceed u32 range: {secs}")))
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

/// Map HPXML `<SiteType>` to [`TerrainClass`] for the wind on every leakage
/// path. A file without one is suburban, OS-HPXML v1.12.0's default
/// (defaults.rb:817-818). The parser rejects a value outside HPXML's rural,
/// suburban and urban, so no other case reaches the resolver.
pub(crate) fn site_type_to_terrain(
    site_type: Option<&hares_io::hpxml::SiteType>,
) -> hares_physics::infiltration::TerrainClass {
    use hares_io::hpxml::SiteType;
    use hares_physics::infiltration::TerrainClass;
    match site_type {
        Some(SiteType::Rural) => TerrainClass::Rural,
        Some(SiteType::Urban) => TerrainClass::Urban,
        Some(SiteType::Suburban) | None => TerrainClass::Suburban,
    }
}

/// The convective roughness class of a boundary's outside face, and a
/// warning when its HPXML names no material. Only an outdoor face reads it
/// (any other face has no forced convection, and the class returned for it
/// is never used). Walls and rim joists read their `Siding`, roofs their
/// `RoofType`, foundation walls their `Type`
/// ([`hares_physics::film_coefficients::outside_layer_roughness`]). Glazing
/// is glass, the Engineering Reference's Very Smooth example (its films come
/// from the U-factor decomposition in any case). Doors, floors and slabs
/// have no material element in HPXML and take OS-HPXML v1.12.0's `Rough`
/// (model.rb:49, :95) without a warning, since no input is missing.
fn boundary_outside_roughness(
    bd: &hares_io::hpxml::Boundary,
    exterior: hares_physics::film_coefficients::ZoneLabel,
) -> Result<(
    hares_physics::film_coefficients::SurfaceRoughness,
    Option<String>,
)> {
    use hares_io::hpxml::BoundaryType;
    use hares_physics::film_coefficients::{
        OutsideLayer, SurfaceRoughness, ZoneLabel, outside_layer_roughness,
    };
    if exterior != ZoneLabel::Outdoor {
        return Ok((SurfaceRoughness::Rough, None));
    }
    let layer = match &bd.boundary_type {
        BoundaryType::Wall | BoundaryType::RimJoist => OutsideLayer::Siding,
        BoundaryType::Roof => OutsideLayer::Roof,
        BoundaryType::FoundationWall => OutsideLayer::FoundationWall,
        BoundaryType::Window | BoundaryType::Skylight => {
            return Ok((SurfaceRoughness::VerySmooth, None));
        }
        BoundaryType::Door | BoundaryType::Floor | BoundaryType::Slab => {
            return Ok((SurfaceRoughness::Rough, None));
        }
        BoundaryType::Other(kind) => {
            return Err(HaresError::Dwelling(format!(
                "boundary '{}' of unrecognised type '{kind}' faces outdoors; its outside \
                 convection is undefined",
                bd.id
            )));
        }
    };
    let (class, warning) = outside_layer_roughness(layer, bd.finish_type.as_deref())
        .map_err(|e| HaresError::Physics(format!("boundary '{}': {e}", bd.id)))?;
    Ok((class, warning.map(|w| format!("boundary '{}': {w}", bd.id))))
}

/// The construction warnings the envelope's inputs raise: each outdoor
/// boundary whose HPXML names no outside material, a site with no
/// `SiteType` (suburban, OS-HPXML's default, defaults.rb:817-818) and one
/// with no `ShieldingofHome` (OS-HPXML's default, defaults.rb:822-830).
///
/// # Errors
///
/// An outdoor boundary's material that is not an HPXML value.
pub(crate) fn envelope_input_warnings(building: &Building) -> Result<Vec<hares_types::Warning>> {
    let mut warnings = Vec::new();
    for bd in &building.boundaries {
        let exterior = zone_type_to_label(&bd.id, bd.exterior_zone.as_ref())?;
        if let (_, Some(message)) = boundary_outside_roughness(bd, exterior)? {
            warnings.push(hares_types::Warning::new("envelope", message));
        }
    }
    if building.site.site_type.is_none() {
        warnings.push(hares_types::Warning::new(
            "envelope",
            "the building gives no SiteType; the wind terrain is suburban, OS-HPXML's default \
             (defaults.rb:817-818)",
        ));
    }
    if building.site.shielding_of_home.is_none() {
        // The warning carries the resolved default's class, so it fires
        // only where the default resolves: a building with neither
        // ShieldingofHome nor ResidentialFacilityType is the infiltration
        // methods' error (they name the facility type when one of them
        // reads the shielding); a dwelling whose infiltration reads no
        // shielding needs neither.
        if let Ok(shielding) =
            shielding_to_class(None, building.residential_facility_type.as_deref())
        {
            warnings.push(hares_types::Warning::new(
                "envelope",
                format!(
                    "the building gives no ShieldingofHome; the shielding is {shielding:?}, \
                     OS-HPXML's default for this facility type (defaults.rb:822-830)"
                ),
            ));
        }
    }
    Ok(warnings)
}

/// Map HPXML `<ShieldingofHome>` to [`ShieldingClass`].
///
/// Walker & Wilson (1998) Table 3; ResStock `airflow.get_aim2_shelter_coefficient`.
/// A file without one takes OS-HPXML v1.12.0's default (defaults.rb:822-830):
/// well-shielded for an apartment unit or single-family attached home,
/// normal otherwise; [`envelope_input_warnings`] reports it. The parser
/// rejects any other value. The default reads the residential facility
/// type; a building with no `ShieldingofHome` and no
/// `ResidentialFacilityType` either is a `MissingField` naming the
/// facility type: the element is a required input of the reference
/// (workflow_inputs.rst:622, "Yes" in the required column), and its
/// absence leaves the default unevaluable.
///
/// # Errors
///
/// [`HaresError::Dwelling`] naming `ResidentialFacilityType` when neither
/// element is present.
pub(crate) fn shielding_to_class(
    shielding: Option<&hares_io::hpxml::ShieldingOfHome>,
    residential_facility_type: Option<&str>,
) -> Result<hares_physics::infiltration::ShieldingClass> {
    use hares_io::hpxml::ShieldingOfHome;
    use hares_physics::infiltration::ShieldingClass;
    match shielding {
        Some(ShieldingOfHome::Exposed) => Ok(ShieldingClass::Exposed),
        Some(ShieldingOfHome::WellShielded) => Ok(ShieldingClass::WellShielded),
        Some(ShieldingOfHome::Normal) => Ok(ShieldingClass::Normal),
        None => match residential_facility_type {
            Some("apartment unit" | "single-family attached") => Ok(ShieldingClass::WellShielded),
            Some(_) => Ok(ShieldingClass::Normal),
            None => Err(HaresError::Dwelling(
                "BuildingSummary/BuildingConstruction/ResidentialFacilityType: the building \
                 declares no ShieldingofHome, whose default reads the residential facility \
                 type (OS-HPXML v1.12.0 defaults.rb:822-830: well-shielded for an apartment \
                 unit or single-family attached home, normal otherwise), and declares no \
                 facility type either; name the facility type or declare the shielding"
                    .to_string(),
            )),
        },
    }
}

/// The flue-or-chimney flag OpenStudio-HPXML defaults when the HPXML's air
/// infiltration measurement declares none (v1.12.0
/// `HPXMLtoOpenStudio/resources/defaults.rb:5096-5132`,
/// `get_flue_or_chimney_in_conditioned_space`, applied at
/// defaults.rb:3418-3421): true when a non-electric heating system of the
/// atmospheric types (furnace, boiler, wall furnace, floor furnace, stove,
/// space heater; HARES resolves the wall furnace, floor furnace and stove
/// into its furnace class) sits in conditioned space with an AFUE or
/// percent efficiency below 0.89; true when a non-electric fireplace sits
/// in conditioned space (a type HARES does not resolve, so no spec reaches
/// that arm); true when a non-electric water heater sits in conditioned
/// space whose energy factor, or the equivalent calculated from its
/// uniform energy factor (`resources/waterheater.rb:1557-1577`
/// `calc_ef_from_uef`), is below 0.63; false otherwise. The combi tank
/// types are excluded: their boiler is the heating system checked above
/// (defaults.rb:5119-5120), so HARES's indirect tank is not a water heater
/// arm here either. A non-electric water heater in conditioned space with
/// no efficiency declared falls to true, the reference's own fall-through.
///
/// "In conditioned space" reads the spec's resolved zone: the zone it
/// serves is the conditioned zone (the merged conditioned foundation is
/// part of it); a system serving no declared zone is resolved to the
/// conditioned zone at init and reads conditioned here.
pub(crate) fn flue_or_chimney_in_conditioned_space(
    env: &EnvironmentState,
    building: &Building,
    specs: &[hares_io::EquipmentSpec],
) -> bool {
    use hares_types::FuelType;

    // The spec's zone_id is the environment zone's id (one past the
    // building zone's index, environment.rs's ZoneId(idx + 1)); the
    // conditioned-space test reads the building zone it names. A spec
    // serving no declared zone resolves to the conditioned zone at init
    // and reads conditioned here.
    let zone_is_conditioned = |spec: &hares_io::EquipmentSpec| {
        let Some(zone_id) = spec_zone_id(spec) else {
            return true;
        };
        match env
            .zones
            .iter()
            .position(|zone| zone.id.0 as usize == zone_id)
        {
            Some(index) => building
                .zones
                .get(index)
                .is_some_and(|zone| zone.zone_type == hares_io::hpxml::ZoneType::Conditioned),
            None => true,
        }
    };
    for spec in specs {
        let atmospheric_heating = matches!(spec.name.as_str(), "Gas Furnace" | "Gas Boiler");
        if !atmospheric_heating
            && spec.name != "Gas Water Heater"
            && spec.name != "Tankless Water Heater"
            && spec.name != "Gas Tankless Water Heater"
            && spec.name != "Heat Pump Water Heater"
        {
            continue;
        }
        if spec.fuel_type == FuelType::Electric {
            continue;
        }
        if !zone_is_conditioned(spec) {
            continue;
        }
        if atmospheric_heating {
            // The AFUE or percent efficiency, typed payload first, then
            // the raw parameter bag a spec pending autosizing carries
            // (the units are read beside it and both thresholds are
            // 0.89, the rule's "AFUE/Percent").
            let afue = spec_payload_f64(spec, "afue")
                .or_else(|| spec_payload_f64(spec, "heating_efficiency"));
            if afue.is_none_or(|afue| afue < 0.89) {
                return true;
            }
            continue;
        }
        let energy_factor = spec_payload_f64(spec, "energy_factor");
        let effective = energy_factor.or_else(|| {
            spec_payload_f64(spec, "uniform_energy_factor")
                .map(|uef| calc_ef_from_uef(&spec.name, uef))
        });
        if effective.is_none_or(|ef| ef < 0.63) {
            return true;
        }
    }
    false
}

/// The `zone_id` a spec's typed payload or raw parameters carry.
fn spec_zone_id(spec: &hares_io::EquipmentSpec) -> Option<usize> {
    if let Some(typed) = &spec.typed_config
        && let ConfigPayload::Typed { data, .. } = &typed.payload
        && let Some(zone_id) = data.get("zone_id").and_then(Value::as_u64)
    {
        return usize::try_from(zone_id).ok();
    }
    spec.parameters
        .get("zone_id")
        .and_then(Value::as_u64)
        .map(|zone_id| zone_id as usize)
}

/// The `f64` a spec's typed payload or raw parameters carry at `key`.
fn spec_payload_f64(spec: &hares_io::EquipmentSpec, key: &str) -> Option<f64> {
    if let Some(typed) = &spec.typed_config
        && let ConfigPayload::Typed { data, .. } = &typed.payload
        && let Some(value) = data.get(key).and_then(Value::as_f64)
    {
        return Some(value);
    }
    spec.parameters.get(key).and_then(Value::as_f64)
}

/// The energy factor equivalent of a uniform energy factor
/// (`resources/waterheater.rb:1557-1577`, `calc_ef_from_uef`), by the
/// water heater's class: a fuel storage tank's
/// `0.9066 * uef + 0.0711`, a tankless water heater's UEF itself. (The
/// electric and heat-pump forms are unreachable here: an electric water
/// heater is skipped by fuel before the efficiency reads.)
fn calc_ef_from_uef(class: &str, uef: f64) -> f64 {
    if class.contains("Tankless") {
        uef
    } else {
        0.9066 * uef + 0.0711
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
    use serde_json::{Map, Value, json};

    use hares_equipment::{
        ConfigPayload, SetpointReconciliation,
        hvac::heating_config::{DuctConfig, GasFurnaceConfig, HvacSetpointConfig},
    };
    use hares_io::hpxml::{Boundary, BoundaryType, Window, Zone, ZoneType};
    use hares_types::FuelType;

    use super::{
        apply_spec_bag_to_typed_config, building_to_boundary_inputs, building_to_zone_inputs,
        chrono_to_std_duration, duration_to_u32_secs, equipment_config_from_spec, find_zone_idx,
        merged_equipment_config, resolve_exterior, validate_wildcard_override, zone_type_to_label,
    };
    use hares_types::HaresError;

    // ── zone capacitance multiplier tests ──────────────────────────────

    fn minimal_building(zones: Vec<Zone>, boundaries: Vec<Boundary>) -> hares_io::Building {
        hares_io::Building {
            site: hares_io::hpxml::Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones,
            boundaries,
            windows: Vec::new(),
            skylights: Vec::new(),
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: Some(0.0),
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
            conditioned_volume_m3: 400.0,
            ceiling_height_m: 2.5,
            infiltration_height_m: None,
            floors_above_grade: 1.0,
            has_flue_or_chimney: None,
            foundation_name: None,
            conditioned_foundation_merged: false,
            residential_facility_type: None,
            temperature_capacitance_multiplier: 7.0,
            hvac_deadband_c: None,
            climate_zone_iecc: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: "root".into(),
                attrs: Default::default(),
                text: String::new(),
                children: vec![],
            },
            unavailable_periods: Vec::new(),
            parse_warnings: Vec::new(),
            eri_calculation_version: None,
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

    /// OS-HPXML's one temperature capacitance multiplier applies to every
    /// zone, attic, garage and foundation included, whether or not the zone
    /// also carries furniture mass (simcontrols.rb:27-28, and the furniture
    /// InternalMass of constructions.rb:1835 alongside it).
    #[test]
    fn every_zone_takes_the_building_capacitance_multiplier() {
        let zone_types = [
            ZoneType::Conditioned,
            ZoneType::Attic,
            ZoneType::Garage,
            ZoneType::Foundation,
            ZoneType::Other("Custom".to_string()),
        ];
        let zones = zone_types
            .iter()
            .map(|zone_type| Zone {
                zone_type: zone_type.clone(),
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: Vec::new(),
                duct_systems: Vec::new(),
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
                height_m: None,
                hpxml_location: None,
            })
            .collect();
        let mut building = minimal_building(
            zones,
            vec![
                furniture_boundary("conditioned_furniture", ZoneType::Conditioned),
                furniture_boundary("garage_furniture", ZoneType::Garage),
            ],
        );
        building.temperature_capacitance_multiplier = 5.0;
        let zone_inputs = building_to_zone_inputs(&building, zone_types.len());
        for (zone_type, input) in zone_types.iter().zip(&zone_inputs) {
            assert_eq!(
                input.temperature_capacitance_multiplier, 5.0,
                "{zone_type:?}"
            );
        }
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
            setpoint: HvacSetpointConfig::default(),
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
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "Gas Furnace".to_string(),
                    "Gas Furnace".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("unknown override keys must fail the merge that applies them");
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

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("known override keys must deserialize");

        assert!((cfg.afue - 0.96).abs() < 1e-12);
        assert!((cfg.capacity_w - 12_000.0).abs() < 1e-12);
    }

    /// A 24-hour constant daily profile, the shape the resolver stamps.
    fn constant_profile(value: f64) -> Value {
        Value::Array(vec![Value::from(value); 24])
    }

    #[test]
    fn constant_setpoint_override_removes_the_matching_schedule_source() {
        // The resolver's shape: the payload's setpoint block carries the
        // per-side sources the resolution stamped, and the constant
        // override lands beside them. The merge removes the shadowed
        // side's source, and only that side's.
        let mut spec = gas_furnace_spec();
        if let Some(typed) = spec.typed_config.as_mut()
            && let ConfigPayload::Typed { data, .. } = &mut typed.payload
        {
            let obj = data.as_object_mut().expect("furnace payload object");
            obj.insert(
                "setpoint".to_string(),
                json!({
                    "heating_setpoint_source": {
                        "kind": "daily_profile",
                        "weekday": constant_profile(20.0),
                        "weekend": constant_profile(20.0)
                    },
                    "cooling_setpoint_source": {
                        "kind": "daily_profile",
                        "weekday": constant_profile(25.5556),
                        "weekend": constant_profile(25.5556)
                    }
                }),
            );
        }
        let overrides = json!({
            "Gas Furnace": {
                "setpoint": {"heating_setpoint_c": 25.0}
            }
        });

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("the merged config decodes");

        assert_eq!(cfg.setpoint.heating_setpoint_c, Some(25.0));
        assert!(
            cfg.setpoint.heating_setpoint_source.is_none(),
            "the constant shadows the heating source: it must be removed"
        );
        assert!(
            cfg.setpoint.cooling_setpoint_source.is_some(),
            "the cooling side's source stands: the override named the heating side only"
        );
    }

    #[test]
    fn constant_setpoint_override_from_the_wildcard_layer_shadows_too() {
        let mut spec = gas_furnace_spec();
        if let Some(typed) = spec.typed_config.as_mut()
            && let ConfigPayload::Typed { data, .. } = &mut typed.payload
        {
            let obj = data.as_object_mut().expect("furnace payload object");
            obj.insert(
                "setpoint".to_string(),
                json!({
                    "heating_setpoint_source": {
                        "kind": "daily_profile",
                        "weekday": constant_profile(20.0),
                        "weekend": constant_profile(20.0)
                    }
                }),
            );
        }
        let overrides = json!({
            "all": {"setpoint": {"cooling_setpoint_c": 26.0}}
        });

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("the merged config decodes");

        assert_eq!(cfg.setpoint.cooling_setpoint_c, Some(26.0));
        assert!(cfg.setpoint.cooling_setpoint_source.is_none());
        assert!(cfg.setpoint.heating_setpoint_source.is_some());
    }

    #[test]
    fn a_null_constant_does_not_shadow_the_schedule_source() {
        // An explicit null unsets the constant; it does not set one, so
        // the resolution's source keeps governing.
        let mut spec = gas_furnace_spec();
        if let Some(typed) = spec.typed_config.as_mut()
            && let ConfigPayload::Typed { data, .. } = &mut typed.payload
        {
            let obj = data.as_object_mut().expect("furnace payload object");
            obj.insert(
                "setpoint".to_string(),
                json!({
                    "heating_setpoint_source": {
                        "kind": "daily_profile",
                        "weekday": constant_profile(20.0),
                        "weekend": constant_profile(20.0)
                    }
                }),
            );
        }
        let overrides = json!({
            "Gas Furnace": {"setpoint": {"heating_setpoint_c": null}}
        });

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("the merged config decodes");

        assert_eq!(cfg.setpoint.heating_setpoint_c, None);
        assert!(cfg.setpoint.heating_setpoint_source.is_some());
    }

    // ── the spec's parameter bag as an override layer on typed specs ────

    /// A typed spec whose bag mirrors the payload (the shape every
    /// `build_typed_spec` resolver produces), with one bag key changed the
    /// way a blueprint caller overrides a single field.
    fn pv_spec_with_bag_override(override_kw: f64) -> hares_io::EquipmentSpec {
        let typed_cfg = hares_equipment::PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw: 5.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: None,
            noct_c: None,
            array_type: None,
            system_losses_fraction: None,
            inverter_efficiency: None,
            inverter_capacity_kw: None,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
            soiling: None,
            arrays: None,
        };
        let mut parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable PV config")
            .as_object()
            .cloned()
            .expect("PV config object");
        parameters.insert("capacity_kw".to_string(), json!(override_kw));
        let mut typed_overrides = serde_json::Map::new();
        typed_overrides.insert("capacity_kw".to_string(), json!(override_kw));
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "PV".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            typed_overrides,
            zip_params: None,
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "PV".to_string(),
                    "PV".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    #[test]
    fn spec_parameter_override_on_typed_spec_reaches_the_typed_config() {
        let mut spec = pv_spec_with_bag_override(9.0);
        apply_spec_bag_to_typed_config(&mut spec).expect("the spec's overrides must land");
        let overrides = serde_json::Value::Object(serde_json::Map::new());

        let merged = merged_equipment_config(&spec, &overrides)
            .expect("the spec's own parameter override must merge cleanly");
        let cfg = merged
            .require_typed::<hares_equipment::PvConfig>("PV")
            .expect("the merged payload must deserialize as PvConfig");

        assert!(
            (cfg.capacity_kw - 9.0).abs() < 1e-12,
            "an override the caller set on the spec must reach the typed \
             config instead of being dropped, got capacity_kw {}",
            cfg.capacity_kw
        );
    }

    #[test]
    fn spec_parameter_bag_in_agreement_with_typed_payload_merges_unchanged() {
        let mut spec = pv_spec_with_bag_override(5.0);
        apply_spec_bag_to_typed_config(&mut spec).expect("the spec's overrides must land");
        let overrides = serde_json::Value::Object(serde_json::Map::new());

        let merged = merged_equipment_config(&spec, &overrides)
            .expect("a mirror bag in agreement with the payload must merge cleanly");
        let cfg = merged
            .require_typed::<hares_equipment::PvConfig>("PV")
            .expect("the merged payload must deserialize as PvConfig");

        assert!((cfg.capacity_kw - 5.0).abs() < 1e-12);
        assert!((cfg.tilt_deg.unwrap() - 30.0).abs() < 1e-12);
    }

    /// The bag-encoding disagreement case the Python builders write: the
    /// override carries the human fuel spelling ("natural gas", the same
    /// text the raw channel parses) while the typed payload carries the
    /// canonical serde form ("Gas"). The landing must translate the raw
    /// text into the canonical form, not reject the spec.
    #[test]
    fn spec_bag_raw_text_fuel_lands_the_canonical_form() {
        let typed_cfg: hares_equipment::GasWaterHeaterConfig = serde_json::from_value(json!({
            "fuel_type": "Gas",
            "tank_volume_m3": 0.3,
            "heating_capacity_w": 4000.0,
        }))
        .expect("minimal GasWaterHeaterConfig");
        let mut typed_overrides = serde_json::Map::new();
        typed_overrides.insert("fuel_type".to_string(), json!("natural gas"));
        let mut spec = hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Water Heater".to_string(),
            fuel_type: FuelType::Gas,
            parameters: serde_json::to_value(&typed_cfg)
                .expect("serializable gas water heater config")
                .as_object()
                .cloned()
                .expect("gas water heater config object"),
            typed_overrides,
            zip_params: None,
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "Gas Water Heater".to_string(),
                    "Gas Water Heater".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        };

        apply_spec_bag_to_typed_config(&mut spec).expect("the raw-text fuel must land normalized");

        let merged = merged_equipment_config(&spec, &Value::Object(Map::new()))
            .expect("the normalized payload must merge cleanly");
        let cfg = merged
            .require_typed::<hares_equipment::GasWaterHeaterConfig>("Gas Water Heater")
            .expect("the landed payload must deserialize");
        assert_eq!(
            cfg.fuel_type,
            FuelType::Gas,
            "the override's raw text must land as the canonical enum the payload's \
             deserializer reads"
        );
    }

    /// Same disagreement on a flattened config, where serde's path tracking
    /// cannot see the failing field: the raw-text backup fuel must still
    /// land in its canonical form.
    #[test]
    fn spec_bag_raw_text_backup_fuel_lands_canonical_on_the_flattened_config() {
        for (raw, canonical) in [
            ("natural gas", FuelType::Gas),
            ("gas", FuelType::Gas),
            ("electric", FuelType::Electric),
        ] {
            let mut spec = heat_pump_heater_spec_with_backup_fuel(FuelType::Gas);
            spec.typed_overrides
                .insert("backup_fuel".to_string(), json!(raw));

            apply_spec_bag_to_typed_config(&mut spec)
                .unwrap_or_else(|err| panic!("the raw text '{raw}' must land normalized: {err}"));

            let merged = merged_equipment_config(&spec, &Value::Object(Map::new()))
                .expect("the normalized payload must merge cleanly");
            let cfg = merged
                .require_typed::<hares_equipment::HeatPumpHeaterConfig>("ASHP Heater")
                .expect("the landed payload must deserialize");
            assert_eq!(
                cfg.common.backup_fuel,
                Some(canonical),
                "the bag's raw text '{raw}' must land as the canonical enum"
            );
        }
    }

    /// A bag value no parser can read is a build error naming the
    /// equipment, the field, and the offending value.
    #[test]
    fn spec_bag_unparseable_fuel_text_errors_naming_field_and_value() {
        let typed_cfg: hares_equipment::GasWaterHeaterConfig = serde_json::from_value(json!({
            "fuel_type": "Gas",
            "tank_volume_m3": 0.3,
            "heating_capacity_w": 4000.0,
        }))
        .expect("minimal GasWaterHeaterConfig");
        let mut typed_overrides = serde_json::to_value(&typed_cfg)
            .expect("serializable gas water heater config")
            .as_object()
            .cloned()
            .expect("gas water heater config object");
        typed_overrides.insert("fuel_type".to_string(), json!("ban gas"));
        let mut spec = hares_io::EquipmentSpec {
            instance_name: Some("Hot Water".to_string()),
            name: "Gas Water Heater".to_string(),
            fuel_type: FuelType::Gas,
            parameters: serde_json::to_value(&typed_cfg)
                .expect("serializable gas water heater config")
                .as_object()
                .cloned()
                .expect("gas water heater config object"),
            typed_overrides,
            zip_params: None,
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "Gas Water Heater".to_string(),
                    "Gas Water Heater".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        };

        let err = apply_spec_bag_to_typed_config(&mut spec)
            .expect_err("a bag value the schema rejects must fail the landing");
        let msg = err.to_string();
        assert!(
            msg.contains("Hot Water"),
            "the error must name the equipment instance, got: {msg}"
        );
        assert!(
            msg.contains("'fuel_type'"),
            "the error must name the field, got: {msg}"
        );
        assert!(
            msg.contains("ban gas"),
            "the error must carry the offending value, got: {msg}"
        );
    }

    /// On a flattened config serde's path tracking reports no field, so the
    /// landing must name the field from the bag key it just landed.
    #[test]
    fn spec_bag_unparseable_fuel_on_flattened_config_names_the_bag_key() {
        let mut spec = heat_pump_heater_spec_with_backup_fuel(FuelType::Gas);
        spec.typed_overrides
            .insert("backup_fuel".to_string(), json!("ban gas"));

        let err = apply_spec_bag_to_typed_config(&mut spec)
            .expect_err("a bag value the schema rejects must fail the landing");
        let msg = err.to_string();
        assert!(
            msg.contains("'backup_fuel'"),
            "the flattened config's fieldless serde path must be replaced by \
             the bag key, got: {msg}"
        );
        assert!(
            msg.contains("ban gas"),
            "the error must carry the offending value, got: {msg}"
        );
    }

    /// A spec whose typed payload is the schema-valid JSON `data`, with one
    /// override on the dedicated override field.
    fn spec_with_payload(
        type_name: &str,
        data: serde_json::Value,
        field: &str,
        value: serde_json::Value,
    ) -> hares_io::EquipmentSpec {
        let mut typed_overrides = serde_json::Map::new();
        typed_overrides.insert(field.to_string(), value);
        hares_io::EquipmentSpec {
            instance_name: None,
            name: type_name.to_string(),
            fuel_type: FuelType::Electric,
            parameters: data.as_object().cloned().unwrap_or_default(),
            typed_overrides,
            zip_params: None,
            typed_config: Some(hares_equipment::EquipmentConfig::with_payload(
                type_name.to_string(),
                type_name.to_string(),
                hares_equipment::ConfigPayload::Typed {
                    type_name: type_name.to_string(),
                    version: 1,
                    data,
                },
            )),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    /// Every registered config class takes an override of an unset
    /// optional field: the field is absent from the payload's serialized
    /// data (`skip_serializing_if`), and the schema still reads it. The
    /// pre-fix landing keyed off the payload's carried keys and dropped
    /// every one of these silently.
    #[test]
    fn an_override_of_an_unset_optional_lands_on_every_config_class() {
        let cases: Vec<(&str, &str, serde_json::Value, serde_json::Value)> = vec![
            (
                "Gas Furnace",
                "fan_power_w",
                json!({"afue": 1.0, "capacity_w": 1.0}),
                json!(350.0),
            ),
            (
                "Electric Furnace",
                "fan_power_w",
                json!({"capacity_w": 1.0, "eir": 1.0, "setpoint": {}}),
                json!(350.0),
            ),
            (
                "Gas Boiler",
                "fan_power_w",
                json!({"afue": 1.0, "capacity_w": 1.0}),
                json!(350.0),
            ),
            (
                "Electric Boiler",
                "fan_power_w",
                json!({"capacity_w": 1.0, "eir": 1.0}),
                json!(350.0),
            ),
            (
                "Electric Baseboard",
                "zone_id",
                json!({"capacity_w": 1.0, "eir": 1.0}),
                json!(1),
            ),
            ("Ideal HVAC", "shr", json!({}), json!(0.5)),
            (
                "Central AC",
                "fan_power_w",
                json!({"capacity_w": 1.0, "eir": 1.0}),
                json!(350.0),
            ),
            (
                "Room AC",
                "shr",
                json!({"capacity_w": 1.0, "eir": 1.0}),
                json!(0.5),
            ),
            ("Dehumidifier", "zone_id", json!({}), json!(1)),
            ("ASHP Heater", "backup_fuel", json!({}), json!("Gas")),
            ("ASHP Cooler", "zone_id", json!({}), json!(1)),
            (
                "Gas Water Heater",
                "ua_w_per_k",
                json!({"fuel_type": "Gas"}),
                json!(5.0),
            ),
            (
                "Electric Resistance Water Heater",
                "ua_w_per_k",
                json!({}),
                json!(5.0),
            ),
            (
                "Tankless Water Heater",
                "zone_id",
                json!({"fuel_type": "Gas"}),
                json!(1),
            ),
            ("Indirect Tank", "ua_w_per_k", json!({}), json!(5.0)),
            ("Heat Pump Water Heater", "cop", json!({}), json!(3.0)),
            ("PV", "noct_c", json!({"capacity_kw": 1.0}), json!(45.0)),
            (
                "Battery",
                "self_discharge_pct_per_day",
                json!({"capacity_kwh": 1.0, "max_charge_kw": 1.0, "max_discharge_kw": 1.0}),
                json!(0.1),
            ),
            (
                "EV",
                "charging_level",
                json!({"capacity_kwh": 1.0, "max_charging_power_kw": 1.0}),
                json!("L2"),
            ),
            (
                "Generator",
                "zone_id",
                json!({"rated_power_kw": 1.0}),
                json!(1),
            ),
            (
                "Ventilation Fan",
                "sensible_effectiveness",
                json!({"flow_rate_m3_s": 1.0}),
                json!(0.7),
            ),
        ];
        for (type_name, field, payload, value) in cases {
            let mut spec = spec_with_payload(type_name, payload, field, value.clone());
            apply_spec_bag_to_typed_config(&mut spec).unwrap_or_else(|err| {
                panic!("{type_name}: the unset optional '{field}' must land: {err}")
            });
            let hares_equipment::ConfigPayload::Typed { data, .. } =
                &spec.typed_config.as_ref().expect("typed").payload
            else {
                panic!("{type_name}: typed payload expected");
            };
            assert_eq!(
                data.get(field),
                Some(&value),
                "{type_name}: the override of the unset optional '{field}' must reach the payload"
            );
        }
    }

    /// A typo'd override key on a spec errors naming the equipment and the
    /// field: the dedicated override field has no machinery keys to hide
    /// behind, so the whole channel is strict (the acceptance test).
    #[test]
    fn a_typo_override_key_errors_naming_the_equipment_and_field() {
        let mut spec = spec_with_payload(
            "Gas Furnace",
            json!({"afue": 1.0, "capacity_w": 1.0}),
            "afuee",
            json!(0.96),
        );
        let err = apply_spec_bag_to_typed_config(&mut spec)
            .expect_err("a typo'd override key must fail the landing");
        let msg = err.to_string();
        assert!(
            msg.contains("Gas Furnace"),
            "the error must name the equipment, got: {msg}"
        );
        assert!(
            msg.contains("afuee"),
            "the error must name the field, got: {msg}"
        );
    }

    /// An ASHP heater spec whose payload carries the backup fuel, the shape
    /// the Python `ASHPHeater` builder produces for `autosize=False`.
    fn heat_pump_heater_spec_with_backup_fuel(fuel: FuelType) -> hares_io::EquipmentSpec {
        let typed_cfg = hares_equipment::HeatPumpHeaterConfig {
            common: hares_equipment::HeatPumpCommonConfig {
                backup_fuel: Some(fuel),
                ..hares_equipment::HeatPumpCommonConfig::default()
            },
            ..hares_equipment::HeatPumpHeaterConfig::default()
        };
        let parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable heater config")
            .as_object()
            .cloned()
            .expect("heater config object");
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "ASHP Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "ASHP Heater".to_string(),
                    "ASHP Heater".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    fn heat_pump_heater_spec() -> hares_io::EquipmentSpec {
        let typed_cfg = hares_equipment::HeatPumpHeaterConfig::default();
        let parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable heater config")
            .as_object()
            .cloned()
            .expect("heater config object");
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "ASHP Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "ASHP Heater".to_string(),
                    "ASHP Heater".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    #[test]
    fn unknown_override_field_on_heat_pump_heater_errors_at_the_merge() {
        let spec = heat_pump_heater_spec();
        let overrides = json!({
            "ASHP Heater": {
                "backup_fuell": 1.0
            }
        });

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("the flattened heater config must reject unknown override keys");
        let msg = err.to_string();
        assert!(
            msg.contains("ASHP Heater"),
            "the error must name the equipment, got: {msg}"
        );
        assert!(
            msg.contains("backup_fuell"),
            "the error must name the unknown field, got: {msg}"
        );
    }

    fn dehumidifier_spec() -> hares_io::EquipmentSpec {
        let typed_cfg: hares_equipment::DehumidifierConfig =
            serde_json::from_value(json!({ "zone_id": 1 })).expect("minimal DehumidifierConfig");
        let parameters = serde_json::to_value(&typed_cfg)
            .expect("serializable dehumidifier config")
            .as_object()
            .cloned()
            .expect("dehumidifier config object");
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "Dehumidifier".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    "Dehumidifier".to_string(),
                    "Dehumidifier".to_string(),
                    typed_cfg,
                )
                .unwrap(),
            ),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    #[test]
    fn invalid_override_value_errors_at_the_merge_call() {
        let spec = dehumidifier_spec();
        let overrides = json!({
            "Dehumidifier": {
                "capacity_liters_per_day": "lots"
            }
        });

        let err = merged_equipment_config(&spec, &overrides).expect_err(
            "a value the typed config's schema rejects must fail at the \
                         override application",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("Dehumidifier"),
            "the error must name the equipment, got: {msg}"
        );
        assert!(
            msg.contains("capacity_liters_per_day"),
            "the error must name the field, got: {msg}"
        );
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
            setpoint: HvacSetpointConfig::default(),
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
        )
        .unwrap();
        eq_cfg.setpoints_reconciled = Some(reconciliations);
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Furnace".to_string(),
            fuel_type: FuelType::Gas,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(eq_cfg),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");

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

    /// The merge-time scan fires on a knowable surviving schedule whose
    /// coldest step sits below the constant the config names, and stays
    /// silent on the kinds whose values the config does not carry.
    #[test]
    fn cross_tier_scan_names_both_values_and_skips_unreadable_kinds() {
        use hares_types::ScheduleSourceConfig;
        use hares_types::schedule::BoundaryPolicy;

        let base_cfg = gas_furnace_spec()
            .typed_config
            .expect("the spec carries its typed config")
            .typed::<GasFurnaceConfig>()
            .expect("the furnace's typed config reads");
        let source_config = |cooling_source: ScheduleSourceConfig| {
            let typed_cfg = GasFurnaceConfig {
                setpoint: HvacSetpointConfig {
                    heating_setpoint_c: Some(26.0),
                    cooling_setpoint_source: Some(cooling_source),
                    ..HvacSetpointConfig::default()
                },
                ..base_cfg.clone()
            };
            hares_equipment::EquipmentConfig::from_typed(
                "Gas Furnace".to_string(),
                "Gas Furnace".to_string(),
                typed_cfg,
            )
            .unwrap()
        };

        // A daily profile whose coldest step (18 C) sits below the 26 C
        // constant: the warning names the equipment and both values.
        let profile = source_config(ScheduleSourceConfig::DailyProfile {
            weekday: [18.0; 24],
            weekend: [18.0; 24],
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        let warnings = super::cross_tier_setpoint_warnings("Gas Furnace", &profile);
        assert_eq!(warnings.len(), 1, "the profile's crossing warns");
        assert_eq!(warnings[0].source.as_ref(), "Gas Furnace");
        assert!(
            warnings[0].message.contains("26") && warnings[0].message.contains("18"),
            "the warning names both values: {}",
            warnings[0].message
        );

        // A schedule-file column's values the config does not carry: the
        // scan is silent, the runtime count is the guard.
        let column = source_config(ScheduleSourceConfig::ColumnRef {
            col_idx: 7,
            boundary: BoundaryPolicy::Clamp,
        });
        assert!(super::cross_tier_setpoint_warnings("Gas Furnace", &column).is_empty());

        // A surviving schedule whose every step is above the constant does
        // not warn.
        let high = source_config(ScheduleSourceConfig::DailyProfile {
            weekday: [28.0; 24],
            weekend: [28.0; 24],
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        assert!(super::cross_tier_setpoint_warnings("Gas Furnace", &high).is_empty());
    }

    #[test]
    fn merged_equipment_config_preserves_setpoints_reconciled_none() {
        // Use the existing spec helper which has setpoints_reconciled = None.
        let spec = gas_furnace_spec();
        let overrides = serde_json::Value::Object(serde_json::Map::new());

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");

        assert!(
            merged.setpoints_reconciled.is_none(),
            "setpoints_reconciled must be None when typed config has None"
        );
    }

    // ── ZIP sidecar plumbing (config root fix) ─────────────────────────

    /// Regression: typed specs previously DROPPED `EquipmentSpec::zip_params`
    /// — the typed early-return in `equipment_config_from_spec` and the typed
    /// branch of `merged_equipment_config` never copied it, so typed HVAC/WH
    /// equipment never received ZIP/PF parameters (only the raw branch
    /// injected `zip_*` keys).
    #[test]
    fn regression_typed_spec_zip_params_reach_equipment_config_sidecar() {
        let mut spec = gas_furnace_spec();
        let zip = hares_types::zip::zip_defaults_for_class("Gas Furnace").expect("class row");
        spec.zip_params = Some(zip);

        let cfg = equipment_config_from_spec(&spec).expect("spec conversion must succeed");
        assert_eq!(
            cfg.zip,
            Some(zip),
            "typed early-return must carry zip_params into the sidecar"
        );

        let merged =
            merged_equipment_config(&spec, &serde_json::Value::Object(serde_json::Map::new()))
                .expect("merge must succeed");
        assert_eq!(
            merged.zip,
            Some(zip),
            "typed merged branch must carry zip_params into the sidecar"
        );
        // The sidecar travels outside the #[serde(deny_unknown_fields)]
        // payload, which must still deserialize cleanly.
        merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("typed payload must stay intact");
    }

    #[test]
    fn zip_override_beats_defaults_for_typed_equipment_field_wise() {
        let mut spec = gas_furnace_spec();
        let base = hares_types::zip::zip_defaults_for_class("Gas Furnace").expect("class row");
        spec.zip_params = Some(base);
        let overrides = json!({"Gas Furnace": {"zip": {"pf": 0.9}}});

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let zip = merged.zip.expect("zip sidecar must be populated");
        assert_eq!(zip.pf, 0.9, "override must beat the toml default");
        // Partial-field merge: every other field inherited from the base.
        assert_eq!((zip.zp, zip.ip, zip.pp), (base.zp, base.ip, base.pp));
        assert_eq!((zip.zq, zip.iq, zip.pq), (base.zq, base.iq, base.pq));
        assert_eq!(zip.v0, base.v0);
        // The reserved "zip" key must be peeled before typed
        // deserialization so deny_unknown_fields never sees it.
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("\"zip\" override must not reach the typed payload");
        assert!((cfg.afue - 0.82).abs() < 1e-12);
    }

    /// An unknown key in the reserved `"zip"` override object is a hard
    /// configuration error at build time (deny_unknown_fields ethos), not a
    /// silently dropped warning.
    #[test]
    fn zip_override_unknown_key_is_hard_error() {
        let spec = gas_furnace_spec();
        let overrides = json!({"Gas Furnace": {"zip": {"fp": 0.9}}});

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("unknown \"zip\" override key must fail the build");
        let msg = err.to_string();
        assert!(msg.contains("Gas Furnace"), "missing equipment name: {msg}");
        assert!(msg.contains("fp"), "missing offending key: {msg}");
        assert!(
            msg.contains("zp, ip, pp, zq, iq, pq, pf, v0"),
            "must list the valid keys: {msg}"
        );
    }

    /// A non-numeric value in the `"zip"` override object is a hard
    /// configuration error at build time.
    #[test]
    fn zip_override_non_numeric_value_is_hard_error() {
        let spec = gas_furnace_spec();
        let overrides = json!({"Gas Furnace": {"zip": {"pf": "high"}}});

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("non-numeric \"zip\" override value must fail the build");
        let msg = err.to_string();
        assert!(msg.contains("Gas Furnace"), "missing equipment name: {msg}");
        assert!(msg.contains("non-numeric"), "missing reason: {msg}");
        assert!(msg.contains("pf"), "missing offending key: {msg}");
    }

    /// A non-object `"zip"` override value is a hard configuration error.
    #[test]
    fn zip_override_non_object_is_hard_error() {
        let spec = gas_furnace_spec();
        let overrides = json!({"Gas Furnace": {"zip": 0.9}});

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("non-object \"zip\" override must fail the build");
        let msg = err.to_string();
        assert!(msg.contains("must be an object"), "missing reason: {msg}");
    }

    /// The hard error also applies on the raw-equipment merge path.
    #[test]
    fn zip_override_unknown_key_is_hard_error_for_raw_equipment() {
        let spec = raw_ashp_spec();
        let overrides = json!({"ASHP Heater": {"zip": {"power_factor": 0.9}}});

        let err = merged_equipment_config(&spec, &overrides)
            .expect_err("unknown \"zip\" override key must fail the raw build too");
        let msg = err.to_string();
        assert!(msg.contains("power_factor"), "missing offending key: {msg}");
    }

    #[test]
    fn zip_override_without_spec_zip_params_merges_over_class_defaults() {
        // gas_furnace_spec() has zip_params: None; the override should merge
        // over the class-table defaults for "Gas Furnace" (blower fan row).
        let spec = gas_furnace_spec();
        let class_row = hares_types::zip::zip_defaults_for_class("Gas Furnace").expect("class row");
        let overrides = json!({"Gas Furnace": {"zip": {"pf": 0.9}}});

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        let zip = merged.zip.expect("zip sidecar must be populated");
        assert_eq!(zip.pf, 0.9);
        assert_eq!(
            (zip.zq, zip.iq, zip.pq),
            (class_row.zq, class_row.iq, class_row.pq),
            "unset fields must inherit the class-table defaults"
        );
    }

    fn raw_ashp_spec() -> hares_io::EquipmentSpec {
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "ASHP Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: hares_types::zip::zip_defaults_for_class("ASHP Heater"),
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    #[test]
    fn zip_override_beats_defaults_for_raw_equipment_field_wise() {
        let spec = raw_ashp_spec();
        let base = spec.zip_params.expect("toml base");
        let overrides = json!({"ASHP Heater": {"zip": {"pf": 0.9}}});

        let merged = merged_equipment_config(&spec, &overrides).expect("merge must succeed");
        // Sidecar carries the merged value.
        let zip = merged.zip.expect("zip sidecar must be populated");
        assert_eq!(zip.pf, 0.9, "override must beat the toml default");
        assert_eq!((zip.zq, zip.iq, zip.pq), (base.zq, base.iq, base.pq));
        // The reserved "zip" key must not leak into the raw parameter map.
        assert!(
            !raw_keys(&merged).iter().any(|key| *key == "zip"),
            "reserved \"zip\" key must be peeled from raw parameters"
        );
        // End-to-end through the resolver.
        let resolved = hares_equipment::resolve_zip(&merged);
        assert_eq!(resolved.pf, 0.9);
        assert_eq!(resolved.zq, base.zq);
    }

    /// Raw equipment receive ZIP exclusively through the sidecar — the
    /// legacy raw `zip_*` config-key injection is gone.
    #[test]
    fn raw_equipment_gets_zip_through_sidecar_only() {
        let spec = raw_ashp_spec();
        let base = spec.zip_params.expect("toml base");

        let cfg = equipment_config_from_spec(&spec).expect("spec conversion must succeed");
        // The sidecar is the single ZIP channel.
        assert_eq!(cfg.zip, Some(base));
        assert_eq!(hares_equipment::resolve_zip(&cfg).zip, base);
        // No legacy zip_* keys anywhere in the raw payload.
        let keys = raw_keys(&cfg);
        assert!(
            keys.iter().all(|k| !k.starts_with("zip")),
            "raw payload must carry no zip_* keys, got: {keys:?}"
        );
    }

    // ── apply_humidity_update_to_zones telemetry tests ──────────────────

    fn env_with_humidity_zone(zone_id: u16, humidity_ratio: f64) -> hares_types::EnvironmentState {
        use chrono::{FixedOffset, TimeZone};
        use hares_types::{
            ElectricalSummary, GridState, PriceSignal, SurfaceIrradiance, WeatherState, ZoneState,
        };
        hares_types::EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: hares_types::ZoneId(zone_id),
                temperature_c: 22.0,
                humidity_ratio,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
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
            // 7-float per-zone humidity payload: [zone_id, w_new, rh, wet_bulb_c,
            // alpha, condensation_mass_kg, condensation_occurred_flag]
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, alpha, 0.0, 0.0]),
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
            // 7-float per-zone humidity payload: [zone_id, w_new, rh, wet_bulb_c,
            // alpha, condensation_mass_kg, condensation_occurred_flag]
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, 0.0, 0.0, 0.0]),
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
            // 7-float per-zone humidity payload: [zone_id, w_new, rh, wet_bulb_c,
            // alpha, condensation_mass_kg, condensation_occurred_flag]
            custom_payload: Some(vec![f64::from(zone_id), 0.009, 0.50, 19.0, alpha, 0.0, 0.0]),
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

    /// Only boundaries whose HPXML element carries a material read it: an
    /// outdoor door, floor or window has none to read and takes OS-HPXML's
    /// Rough or glass's Very Smooth with no warning. A wall, rim joist,
    /// roof or foundation wall reads its own element, and one that names
    /// none takes OS-HPXML's default material with a warning naming the
    /// boundary. An outdoor wall's unknown material is an error naming it.
    #[test]
    fn only_material_bearing_boundaries_read_their_material() {
        use hares_physics::film_coefficients::{SurfaceRoughness, ZoneLabel};
        let outdoor = |boundary_type: BoundaryType, finish: Option<&str>| Boundary {
            id: "b-1".to_string(),
            boundary_type,
            exterior_zone: Some(ZoneType::Outdoor),
            finish_type: finish.map(str::to_string),
            ..slab_boundary(None, None)
        };
        for (boundary_type, finish, class, warns) in [
            (
                BoundaryType::Door,
                Some("diamond plate"),
                SurfaceRoughness::Rough,
                false,
            ),
            (
                BoundaryType::Floor,
                Some("diamond plate"),
                SurfaceRoughness::Rough,
                false,
            ),
            (
                BoundaryType::Window,
                Some("diamond plate"),
                SurfaceRoughness::VerySmooth,
                false,
            ),
            (
                BoundaryType::Skylight,
                None,
                SurfaceRoughness::VerySmooth,
                false,
            ),
            (
                BoundaryType::Roof,
                Some("asphalt or fiberglass shingles"),
                SurfaceRoughness::VeryRough,
                false,
            ),
            (BoundaryType::Roof, None, SurfaceRoughness::VeryRough, true),
            (
                BoundaryType::Wall,
                Some("vinyl siding"),
                SurfaceRoughness::Smooth,
                false,
            ),
            (BoundaryType::Wall, None, SurfaceRoughness::Rough, true),
            (BoundaryType::RimJoist, None, SurfaceRoughness::Rough, true),
            (
                BoundaryType::FoundationWall,
                Some("double brick"),
                SurfaceRoughness::MediumRough,
                false,
            ),
            (
                BoundaryType::FoundationWall,
                None,
                SurfaceRoughness::MediumRough,
                true,
            ),
        ] {
            let (got, warning) = super::boundary_outside_roughness(
                &outdoor(boundary_type.clone(), finish),
                ZoneLabel::Outdoor,
            )
            .unwrap();
            assert_eq!(got, class, "{boundary_type:?} {finish:?}");
            assert_eq!(warning.is_some(), warns, "{boundary_type:?} {finish:?}");
            if let Some(warning) = warning {
                assert!(warning.contains("b-1"), "{warning}");
            }
        }
        let err = super::boundary_outside_roughness(
            &outdoor(BoundaryType::Wall, Some("diamond plate")),
            ZoneLabel::Outdoor,
        )
        .expect_err("an unknown wall material must fail");
        assert!(err.to_string().contains("b-1"), "{err}");
        let err = super::boundary_outside_roughness(
            &outdoor(BoundaryType::Other("Canopy".into()), None),
            ZoneLabel::Outdoor,
        )
        .expect_err("an unrecognised outdoor boundary type must fail");
        assert!(err.to_string().contains("Canopy"), "{err}");
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
                    height_m: None,
                    hpxml_location: None,
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
                    height_m: None,
                    hpxml_location: None,
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
                    height_m: None,
                    hpxml_location: None,
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
                    height_m: None,
                    hpxml_location: None,
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
                    height_m: None,
                    hpxml_location: None,
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

    /// A material-layer boundary is unchanged by the opaque-R precedence
    /// (the numeric-fixes ledger's opaque-R precedence entry): a wall with a declared
    /// `AssemblyEffectiveRValue` of 23.0 (IP) and two explicit material
    /// layers takes the material path with the conversion's output the
    /// parent commit's on bits - no precomputed layers, the stated layers
    /// verbatim, and the fallback the declared R's material part minus the
    /// boundary's own films.
    #[test]
    fn material_layer_boundary_unchanged_by_the_declared_r() {
        let plywood = hares_io::hpxml::MaterialLayer {
            thickness_m: 0.02,
            conductivity_w_m_k: 0.13,
            density_kg_m3: 545.0,
            specific_heat_j_kg_k: 1215.0,
            area_m2: 20.0,
        };
        let batt = hares_io::hpxml::MaterialLayer {
            thickness_m: 0.14,
            conductivity_w_m_k: 0.043,
            density_kg_m3: 12.0,
            specific_heat_j_kg_k: 840.0,
            area_m2: 20.0,
        };
        let declared = hares_physics::units::r_value_ip_to_si(23.0);
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "mat-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: Some(declared),
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![plywood.clone(), batt.clone()],
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");
        assert_eq!(inputs.len(), 1);
        let input = &inputs[0];
        assert!(
            input.precomputed_rc.is_empty(),
            "a material-layer boundary takes no LUT layers"
        );
        for (stated, carried) in [&plywood, &batt].iter().zip(&input.material_layers) {
            assert_eq!(carried.thickness_m.to_bits(), stated.thickness_m.to_bits());
            assert_eq!(
                carried.conductivity_w_m_k.to_bits(),
                stated.conductivity_w_m_k.to_bits()
            );
            assert_eq!(
                carried.density_kg_m3.to_bits(),
                stated.density_kg_m3.to_bits()
            );
            assert_eq!(
                carried.specific_heat_j_kg_k.to_bits(),
                stated.specific_heat_j_kg_k.to_bits()
            );
        }
        let films = input.r_film_interior_m2_k_w + input.r_film_exterior_m2_k_w;
        assert_eq!(
            input.fallback_r_m2_k_w.to_bits(),
            (declared - films).to_bits(),
            "the fallback is the declared R's material part, the parent commit's on bits"
        );
        // The read-back's material-path derivation, unchanged by the
        // opaque-R precedence: the
        // layers' resolved resistances and capacitances, on the parent
        // commit's bits (no framing factor: the conductivity passes
        // through; the capacitance is density times specific heat times
        // thickness).
        let expected_r: [u64; 2] = [
            (0.02_f64 / hares_envelope::boundary_rc::parallel_path_conductivity(0.13, None))
                .to_bits(),
            (0.14_f64 / hares_envelope::boundary_rc::parallel_path_conductivity(0.043, None))
                .to_bits(),
        ];
        let expected_c: [u64; 2] = [
            (plywood.density_kg_m3 * plywood.specific_heat_j_kg_k * plywood.thickness_m).to_bits(),
            (batt.density_kg_m3 * batt.specific_heat_j_kg_k * batt.thickness_m).to_bits(),
        ];
        for (index, layer) in input.material_layers.iter().enumerate() {
            let resolved_r = layer.thickness_m
                / hares_envelope::boundary_rc::parallel_path_conductivity(
                    layer.conductivity_w_m_k,
                    input.framing_factor,
                );
            let resolved_c = layer.density_kg_m3 * layer.specific_heat_j_kg_k * layer.thickness_m;
            assert_eq!(resolved_r.to_bits(), expected_r[index]);
            assert_eq!(resolved_c.to_bits(), expected_c[index]);
        }
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
                    height_m: None,
                    hpxml_location: None,
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

    /// The site's HPXML terrain drives the wind on every leakage path, as in
    /// OS-HPXML (airflow.rb:200-221); OCHRE assumes rural terrain whatever
    /// the site (utils/envelope.py:648-649). A file without a site type is
    /// OS-HPXML's default suburban (defaults.rb:817-818); the parser rejects
    /// any other value (`hpxml::building` tests).
    #[test]
    fn the_site_type_sets_the_terrain() {
        use hares_io::hpxml::SiteType;
        use hares_physics::infiltration::TerrainClass;
        for (site, terrain) in [
            (Some(SiteType::Rural), TerrainClass::Rural),
            (Some(SiteType::Suburban), TerrainClass::Suburban),
            (Some(SiteType::Urban), TerrainClass::Urban),
            (None, TerrainClass::Suburban),
        ] {
            assert_eq!(super::site_type_to_terrain(site.as_ref()), terrain);
        }
    }

    /// Exterior film resistance varies with finish_type: fiber cement siding
    /// (Very Rough, the dataset's cement siding record) must produce lower
    /// R_ext than vinyl siding (Smooth, the hollow-backed siding) at the same
    /// wind speed. If the finish type were bypassed, both boundaries would
    /// get identical R_ext and this test would fail.
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
                    id: "wall-fiber-cement".to_string(),
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
                    finish_type: Some("fiber cement siding".to_string()),
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = super::building_to_boundary_inputs(&building, 1, &store, 4.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");
        assert_eq!(inputs.len(), 2);
        let r_vinyl = inputs[0].r_film_exterior_m2_k_w;
        let r_cement = inputs[1].r_film_exterior_m2_k_w;
        assert!(
            r_cement < r_vinyl,
            "fiber cement (VeryRough, Rf=2.17) R_ext={r_cement:.5} must be < vinyl siding \
             (Smooth, Rf=1.11) R_ext={r_vinyl:.5}; one roughness for both would give identical values"
        );
    }

    /// `find_zone_idx` must reject an Adjacent zone type with a typed error:
    /// the rewrite in `building.rs` should eliminate all Adjacent references
    /// before this function is called.
    #[test]
    fn find_zone_idx_errors_on_adjacent_input() {
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
                height_m: None,
                hpxml_location: None,
            }],
            Vec::new(),
        );
        // Adjacent is filtered from the zones vec in building.rs and should
        // never reach find_zone_idx after the rewrite. This call asserts
        // that invariant.
        let err = find_zone_idx(&building, None, Some(&ZoneType::Adjacent), 1)
            .expect_err("an Adjacent zone input must be a typed error");
        assert!(
            err.to_string()
                .contains("Adjacent zone type reached find_zone_idx"),
            "got: {err}"
        );
    }

    /// `find_zone_idx` with boundary ID resolves the correct zone when two
    /// zones share the same `ZoneType` but have different `attached_wall_ids`.
    /// The boundary belonging to zone B (index 1) must map to index 1, not 0.
    #[test]
    fn find_zone_idx_disambiguates_shared_zone_type_by_boundary_id() {
        let zone_a = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec!["WallA".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let zone_b = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec!["WallB".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone_a, zone_b], Vec::new());

        // WallA belongs to zone A (index 0).
        assert_eq!(
            find_zone_idx(&building, Some("WallA"), Some(&ZoneType::Conditioned), 2).unwrap(),
            0
        );
        // WallB belongs to zone B (index 1) — must NOT map to zone 0.
        assert_eq!(
            find_zone_idx(&building, Some("WallB"), Some(&ZoneType::Conditioned), 2).unwrap(),
            1
        );
    }

    /// `boundary_zone_index` with a zone type that appears twice: the second
    /// occurrence must not be displaced. Delegates to `find_zone_idx`.
    #[test]
    fn boundary_zone_index_disambiguates_duplicate_zone_type() {
        let zone_a = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec!["WallA".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let zone_b = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec!["WallB".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone_a, zone_b], Vec::new());

        assert_eq!(
            super::boundary_zone_index(&building, Some("WallA"), Some(&ZoneType::Conditioned), 2,)
                .unwrap(),
            0
        );
        assert_eq!(
            super::boundary_zone_index(&building, Some("WallB"), Some(&ZoneType::Conditioned), 2,)
                .unwrap(),
            1
        );
    }

    /// `find_zone_idx` falls back to type-only matching when boundary ID is
    /// absent (e.g. for roofs, floors, doors — not tracked in attached_wall_ids).
    #[test]
    fn find_zone_idx_falls_back_to_type_matching_without_boundary_id() {
        let zone = Zone {
            zone_type: ZoneType::Attic,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: Vec::new(),
            vented: true,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone], Vec::new());
        // No boundary ID provided — uses type-only fallback.
        assert_eq!(
            find_zone_idx(&building, None, Some(&ZoneType::Attic), 1).unwrap(),
            0
        );
    }

    /// `find_zone_idx` returns Ok(0) when n_zones is 0 (edge case).
    #[test]
    fn find_zone_idx_zero_zones_returns_zero() {
        let building = minimal_building(Vec::new(), Vec::new());
        assert_eq!(
            find_zone_idx(&building, Some("W1"), Some(&ZoneType::Conditioned), 0).unwrap(),
            0
        );
    }

    /// `find_zone_idx` errors when zone_type is None: a boundary that names
    /// no interior zone (and no attached surface to derive one from) has no
    /// zone to belong to, and zone 0 must not be substituted for it. Doors
    /// and windows in HPXML legitimately lack explicit interior adjacency;
    /// the parser derives theirs from their attached surface.
    #[test]
    fn find_zone_idx_none_zone_type_errors() {
        let zone = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec![],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone], Vec::new());
        let err = find_zone_idx(&building, Some("Door1"), None, 1)
            .expect_err("a boundary with no interior zone type must fail the build");
        let text = err.to_string();
        assert!(
            text.contains("Door1") && text.contains("no interior zone"),
            "the error names the boundary and the missing adjacency: {text}"
        );
    }

    /// `find_zone_idx` returns Err when the zone type exists nowhere in the
    /// building's zone list.
    #[test]
    fn find_zone_idx_unmatched_zone_type_returns_error() {
        let zone = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec![],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone], Vec::new());
        let result = find_zone_idx(&building, Some("Roof1"), Some(&ZoneType::Attic), 1);
        assert!(
            result.is_err(),
            "expected Err for unmatched Attic zone type, got {result:?}"
        );
    }

    /// `boundary_zone_index` (the wrapper the solver wiring calls) errors
    /// when zone_type is None, the same rule `find_zone_idx` applies.
    #[test]
    fn boundary_zone_index_none_zone_type_errors() {
        let zone = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec![],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone], Vec::new());
        let err = super::boundary_zone_index(&building, Some("Door1"), None, 1)
            .expect_err("a boundary with no interior zone type must fail the build");
        let text = err.to_string();
        assert!(
            text.contains("Door1") && text.contains("no interior zone"),
            "the error names the boundary and the missing adjacency: {text}"
        );
    }

    /// `boundary_zone_index` returns Err when the zone type has zero matches in
    /// the building's zone list.
    #[test]
    fn boundary_zone_index_unmatched_zone_type_returns_error() {
        let zone = Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(50.0),
            volume_m3: Some(100.0),
            attached_wall_ids: vec![],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        };
        let building = minimal_building(vec![zone], Vec::new());
        let result =
            super::boundary_zone_index(&building, Some("Roof1"), Some(&ZoneType::Attic), 1);
        assert!(
            result.is_err(),
            "expected Err for unmatched Attic zone type, got {result:?}"
        );
    }

    /// Build a minimal DefaultsStore for tests that need one.
    fn load_defaults_store() -> hares_io::DefaultsStore {
        let defaults_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../python/hares/defaults");
        hares_io::DefaultsStore::load(&defaults_path)
            .expect("DefaultsStore must be loadable from project defaults/ directory")
    }

    /// Window U-factor >= ~10 produces negative r_glass in
    /// `window_u_factor_decomposition` because the EnergyPlus film-resistance
    /// correlations yield r_int + r_ext > 1/U. The error must propagate through
    /// `building_to_boundary_inputs` as `HaresError::Physics`, not silently
    /// clamp to zero.
    ///
    /// Regression: T-0145 — negative r_glass was previously clamped with
    /// `.max(0.0)` before the `< 0.0` check, making the error path dead.
    #[test]
    fn window_u_factor_too_high_propagates_physics_error() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "Win1".to_string(),
                boundary_type: BoundaryType::Window,
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(90.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![Window {
                id: "Win1".to_string(),
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                u_factor_w_m2_k: Some(10.0),
                shgc: Some(0.6),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let result = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0);
        let err = result.expect_err("U=10 window must produce physics error");
        assert!(
            matches!(err, HaresError::Physics(_)),
            "error must be HaresError::Physics, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("r_glass"),
            "error message must mention r_glass, got: {msg}"
        );
        assert!(
            msg.contains("EnergyPlus"),
            "error message must mention EnergyPlus correlation range, got: {msg}"
        );
    }

    /// Fenestration boundary with U-factor = 0.0 is rejected by the
    /// boundary-construction validation in `building_to_boundary_inputs`
    /// before any physics computation occurs.
    ///
    /// Regression: T-0571 round 2 — `u_factor_w_m2_k: Some(0.0)` reached the
    /// invariant comparison in solver_builder with a TARP-derived fallback,
    /// producing a false "r_glass divergence" panic. The fix (round 3) rejects
    /// non-positive U-factors here with a named `HaresError::Dwelling`.
    #[test]
    fn window_zero_u_factor_rejected_in_boundary_construction() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "Win1".to_string(),
                boundary_type: BoundaryType::Window,
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(90.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![Window {
                id: "Win1".to_string(),
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                u_factor_w_m2_k: Some(0.0),
                shgc: Some(0.6),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let result = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0);
        let err = result.expect_err("U=0 window must produce Dwelling error");
        assert!(
            matches!(err, HaresError::Dwelling(_)),
            "error must be HaresError::Dwelling, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("missing or non-positive"),
            "error message must mention 'missing or non-positive', got: {msg}"
        );
        assert!(
            msg.contains("HPXML"),
            "error message must mention HPXML, got: {msg}"
        );
    }

    /// Negative U-factor follows the same validation path as
    /// `Some(0.0)`. The `filter(|&u| u > 0.0)` guard at
    /// conversions.rs:308 excludes both zero and negative values,
    /// routing them to the `HaresError::Dwelling` return.
    #[test]
    fn window_negative_u_factor_rejected_in_boundary_construction() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "Win1".to_string(),
                boundary_type: BoundaryType::Window,
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(90.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![Window {
                id: "Win1".to_string(),
                area_m2: 4.0,
                azimuth_deg: Some(180.0),
                u_factor_w_m2_k: Some(-0.5),
                shgc: Some(0.6),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let result = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0);
        let err = result.expect_err("U=-0.5 window must produce Dwelling error");
        assert!(
            matches!(err, HaresError::Dwelling(_)),
            "error must be HaresError::Dwelling, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("missing or non-positive"),
            "error message must mention 'missing or non-positive', got: {msg}"
        );
    }

    /// Skylight U-factor decomposition follows the same EnergyPlus Simple
    /// Window Model Step 1 path as a Window, verifying that the fenestration
    /// code path is wired for the new `BoundaryType::Skylight` variant.
    #[test]
    fn skylight_u_factor_decomposition_uses_fenestration_path() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "SK1".to_string(),
                boundary_type: BoundaryType::Skylight,
                area_m2: 2.0,
                azimuth_deg: Some(0.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(0.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            skylights: vec![Window {
                id: "SK1".to_string(),
                area_m2: 2.0,
                azimuth_deg: Some(0.0),
                u_factor_w_m2_k: Some(0.55),
                shgc: Some(0.30),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let result = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0);
        let inputs = result.expect("skylight with U=0.55 should produce valid boundary inputs");
        assert_eq!(inputs.len(), 1, "building has one boundary (the skylight)");
        let sk_input = &inputs[0];
        // Verify the skylight uses the fenestration U-factor path (r_glass > 0)
        // rather than the opaque fallback-R path.
        assert!(
            sk_input.fallback_r_m2_k_w > 0.0,
            "skylight fallback_r should be derived from U-factor, got {}",
            sk_input.fallback_r_m2_k_w
        );
        // Interior emissivity should be 0.84 (EMISSIVITY_WINDOW = NFRC glass)
        assert!(
            (sk_input.interior_emissivity - 0.84).abs() < 1e-9,
            "skylight interior emissivity should be 0.84 (NFRC), got {}",
            sk_input.interior_emissivity
        );
    }

    /// Integration test: full window boundary construction pipeline.
    ///
    /// Verifies that for fenestration boundaries with valid U-factor, the E+
    /// Step 1 polynomial `r_glass` flows through to `fallback_r_m2_k_w` and
    /// the film resistances match the E+ polynomial (not TARP), even at
    /// non-NFRC conditions (high wind speed).
    #[test]
    fn window_boundary_input_carries_eplus_r_glass_and_film_resistances() {
        // NFRC-typical U=1.8 double-pane low-e.
        let u_factor = 1.8;
        let window_id = "Wint";

        let n_film_resistances = [
            // NFRC conditions: moderate wind (2 m/s), moderate ΔT.
            (2.0, 10.0, "NFRC"),
            // Non-NFRC: high wind (15 m/s — storm conditions), extreme ΔT.
            (15.0, 35.0, "high-wind"),
        ];

        for &(wind_m_s, _delta_t_k, label) in &n_film_resistances {
            let building = hares_io::Building {
                boundaries: vec![Boundary {
                    id: window_id.to_string(),
                    boundary_type: BoundaryType::Window,
                    area_m2: 4.0,
                    azimuth_deg: Some(180.0),
                    assembly_r_value_m2_k_w: None,
                    r_value_layers_m2_k_w: vec![],
                    interior_zone: Some(ZoneType::Conditioned),
                    exterior_zone: Some(ZoneType::Outdoor),
                    material_layers: vec![],
                    construction_type: None,
                    finish_type: None,
                    insulation_details: None,
                    has_radiant_barrier: false,
                    solar_absorptance: None,
                    emittance: None,
                    lut_boundary_name: None,
                    floor_or_ceiling: None,
                    tilt_deg: Some(90.0),
                    framing_factor: None,
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
                }],
                windows: vec![Window {
                    id: window_id.to_string(),
                    area_m2: 4.0,
                    azimuth_deg: Some(180.0),
                    u_factor_w_m2_k: Some(u_factor),
                    shgc: Some(0.40),
                    interior_shading_fraction: 1.0,
                    winter_shading_fraction: 1.0,
                    fraction_operable: 0.0,
                    exterior_shading_summer: 1.0,
                    exterior_shading_winter: 1.0,
                    attached_to_wall_id: None,
                }],
                ..minimal_building(
                    vec![Zone {
                        zone_type: ZoneType::Conditioned,
                        floor_area_m2: Some(100.0),
                        volume_m3: Some(250.0),
                        attached_wall_ids: vec![],
                        duct_systems: vec![],
                        vented: false,
                        ventilation_ach: None,
                        ventilation_sla: None,
                        height_m: None,
                        hpxml_location: None,
                    }],
                    vec![],
                )
            };
            let store = load_defaults_store();
            let inputs = building_to_boundary_inputs(&building, 1, &store, wind_m_s, 10.0, 10.0)
                .unwrap_or_else(|e| panic!("building_to_boundary_inputs failed at {label}: {e}"));
            assert_eq!(inputs.len(), 1, "{label}: expected 1 boundary input");
            let bi = &inputs[0];

            // The E+ Step 1 polynomial values for U=1.8:
            // r_int = 1/(0.359073·ln(1.8) + 6.949915) ≈ 0.139646
            // r_ext = 1/(0.025342·1.8 + 29.163853)    ≈ 0.034236
            // r_glass = 1/1.8 − r_int − r_ext          ≈ 0.381674
            let (expected_r_glass, expected_r_int, expected_r_ext) =
                hares_physics::solar::window_u_factor_decomposition(u_factor)
                    .expect("valid U-factor");

            assert!(
                (bi.r_film_interior_m2_k_w - expected_r_int).abs() < 1e-5,
                "{label}: r_film_int must be E+ polynomial value {expected_r_int:.6}, \
                 got {:.6}",
                bi.r_film_interior_m2_k_w
            );
            assert!(
                (bi.r_film_exterior_m2_k_w - expected_r_ext).abs() < 1e-5,
                "{label}: r_film_ext must be E+ polynomial value {expected_r_ext:.6}, \
                 got {:.6}",
                bi.r_film_exterior_m2_k_w
            );
            assert!(
                (bi.fallback_r_m2_k_w - expected_r_glass).abs() < 1e-5,
                "{label}: fallback_r_m2_k_w must be E+ polynomial r_glass \
                 {expected_r_glass:.6}, got {:.6}",
                bi.fallback_r_m2_k_w
            );

            // At high wind speed, the TARP film_resistances() would compute
            // a much lower exterior film R. Verify that fenestration
            // boundaries use the E+ polynomial (≈0.034), NOT TARP which
            // would be ≈0.01–0.02 at 15 m/s.
            if label == "high-wind" {
                assert!(
                    bi.r_film_exterior_m2_k_w > 0.03,
                    "{label}: at high wind, r_film_ext ({:.6}) should still be \
                     E+ polynomial (~0.034), not TARP wind-dependent value",
                    bi.r_film_exterior_m2_k_w
                );
            }
        }
    }

    /// Boundary with no R-value sources (no AssemblyEffectiveRValue, no
    /// NominalRValue layers) falls back to DEFAULT_R_M2_K_W in the output
    /// `fallback_r_m2_k_w` field.
    #[test]
    fn no_r_value_sources_falls_back_to_default_r() {
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "no-r-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: Some("__test_no_match__".to_string()),
                floor_or_ceiling: None,
                framing_factor: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");
        assert_eq!(inputs.len(), 1);
        // The default R-value fallback is 2.5 m²·K/W.
        const EXPECTED: f64 = 2.5;
        assert!(
            (inputs[0].fallback_r_m2_k_w - EXPECTED).abs() < 1e-9,
            "boundary with no R-value sources should get default fallback_r_m2_k_w={EXPECTED}, got {}",
            inputs[0].fallback_r_m2_k_w
        );
    }

    /// Boundary with a valid AssemblyEffectiveRValue computes correct
    /// material-only fallback R-value (assembly R minus film resistances)
    /// without falling back to the default.
    #[test]
    fn valid_assembly_r_value_produces_correct_fallback_r() {
        let assembly_r = 3.5; // m²·K/W
        let building = hares_io::Building {
            boundaries: vec![Boundary {
                id: "r35-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: Some(assembly_r),
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: Vec::new(),
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: Some("__test_no_match__".to_string()),
                floor_or_ceiling: None,
                framing_factor: None,
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
                    height_m: None,
                    hpxml_location: None,
                }],
                Vec::new(),
            )
        };
        let store = load_defaults_store();
        let inputs = building_to_boundary_inputs(&building, 1, &store, 2.0, 10.0, 10.0)
            .expect("building_to_boundary_inputs");
        assert_eq!(inputs.len(), 1);
        // Assembly R-value 3.5 m²·K/W includes film; material-only R is lower.
        // The fallback_r_m2_k_w must be less than the assembly R and greater
        // than zero (film subtracted, clamped to 1e-6).
        let fallback = inputs[0].fallback_r_m2_k_w;
        assert!(
            fallback < assembly_r,
            "material-only fallback_r_m2_k_w ({fallback}) must be less than assembly R ({assembly_r}) because film resistances are subtracted"
        );
        assert!(
            fallback > 0.0,
            "material-only fallback_r_m2_k_w ({fallback}) must be positive"
        );
    }

    // ── chrono_to_std_duration tests ──────────────────────────────────

    #[test]
    fn chrono_to_std_duration_positive_round_trips() {
        let d = chrono::Duration::seconds(60);
        let result = chrono_to_std_duration(d).expect("positive duration should convert");
        assert_eq!(result, std::time::Duration::from_millis(60_000));
    }

    #[test]
    fn chrono_to_std_duration_zero_returns_io_error() {
        let d = chrono::Duration::zero();
        let err = chrono_to_std_duration(d).expect_err("zero duration must error");
        assert!(
            matches!(err, HaresError::Io(_)),
            "error must be HaresError::Io, got {:?}",
            err
        );
    }

    #[test]
    fn chrono_to_std_duration_negative_returns_io_error() {
        let d = chrono::Duration::seconds(-1);
        let err = chrono_to_std_duration(d).expect_err("negative duration must error");
        assert!(
            matches!(err, HaresError::Io(_)),
            "error must be HaresError::Io, got {:?}",
            err
        );
    }

    // ── duration_to_u32_secs tests ────────────────────────────────────

    #[test]
    fn duration_to_u32_secs_positive_round_trips() {
        let d = chrono::Duration::seconds(3600);
        let result = duration_to_u32_secs(d).expect("positive duration should convert");
        assert_eq!(result, 3600);
    }

    #[test]
    fn duration_to_u32_secs_zero_returns_io_error() {
        let d = chrono::Duration::zero();
        let err = duration_to_u32_secs(d).expect_err("zero duration must error");
        assert!(
            matches!(err, HaresError::Io(_)),
            "error must be HaresError::Io, got {:?}",
            err
        );
    }

    #[test]
    fn duration_to_u32_secs_negative_returns_io_error() {
        let d = chrono::Duration::seconds(-1);
        let err = duration_to_u32_secs(d).expect_err("negative duration must error");
        assert!(
            matches!(err, HaresError::Io(_)),
            "error must be HaresError::Io, got {:?}",
            err
        );
    }

    #[test]
    fn duration_to_u32_secs_exceeds_u32_range_returns_io_error() {
        let d = chrono::Duration::seconds(u32::MAX as i64 + 1);
        let err = duration_to_u32_secs(d).expect_err("duration exceeding u32 range must error");
        assert!(
            matches!(err, HaresError::Io(_)),
            "error must be HaresError::Io, got {:?}",
            err
        );
    }

    // ── zone_type_to_label tests ──────────────────────────────────────

    #[test]
    fn zone_type_to_label_other_is_an_error() {
        let err = zone_type_to_label("wall_1", Some(&ZoneType::Other("Bogus".to_string())))
            .expect_err("an unrecognised zone type must be an error, not a substituted label");
        let message = err.to_string();
        assert!(
            message.contains("Bogus"),
            "the error must name the unrecognised text, got: {message}"
        );
        assert!(
            message.contains("wall_1"),
            "the error must name the boundary, got: {message}"
        );
    }

    // ── resolve_exterior tests ────────────────────────────────────────

    #[test]
    fn resolve_exterior_none_maps_to_outdoor() {
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
                height_m: None,
                hpxml_location: None,
            }],
            Vec::new(),
        );
        let boundary = Boundary {
            id: "none_ext".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 10.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: None,
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
        };
        let result = resolve_exterior(&building, &boundary, 1);
        // An exterior adjacency the boundary neither states nor can derive
        // from a wall is an error: the pre-fix code defaulted to
        // Outdoor and the boundary's true adjacency was lost.
        let err = result.expect_err("a boundary with no exterior zone is an error");
        assert!(
            err.to_string().contains("none_ext"),
            "the error must name the boundary, got: {err}"
        );
    }

    #[test]
    fn resolve_exterior_other_maps_to_outdoor() {
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
                height_m: None,
                hpxml_location: None,
            }],
            Vec::new(),
        );
        let boundary = Boundary {
            id: "other_ext".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 10.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Other("Foobar".to_string())),
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
        };
        let result = resolve_exterior(&building, &boundary, 1);
        // Unrecognised exterior zone text is an error naming it:
        // the pre-fix code defaulted to Outdoor and the input's text was
        // silently discarded.
        let err = result.expect_err("unrecognised exterior zone text is an error");
        let message = err.to_string();
        assert!(
            message.contains("Foobar"),
            "the error must name the unrecognised text, got: {message}"
        );
        assert!(
            message.contains("other_ext"),
            "the error must name the boundary, got: {message}"
        );
    }

    // ── overrides of raw-parameter loads ────────────────────────────────

    /// A load of each kind, by a class registered as that kind, resolved
    /// with a four-phase cycle so the wet appliance's phase keys exist.
    const LOAD_OF_EACH_KIND: [&str; 3] = ["Plug Loads", "Cooking Range", "Dishwasher"];

    fn raw_load_spec(class: &str) -> hares_io::EquipmentSpec {
        let Value::Object(parameters) = json!({ "sensible_gain_fraction": 0.5, "phase_len": 4 })
        else {
            unreachable!()
        };
        hares_io::EquipmentSpec {
            instance_name: None,
            name: class.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    /// A key of each form on `params`'s list, a numbered one by its first
    /// member, with the kind the load reads it as.
    fn listed_keys(
        params: &hares_equipment::RawParams,
    ) -> Vec<(String, hares_equipment::ParamKind)> {
        params
            .params()
            .map(|param| {
                let key = match param.form {
                    hares_equipment::ParamForm::Key(name) => name.to_string(),
                    hares_equipment::ParamForm::Indexed { prefix, suffix, .. } => {
                        format!("{prefix}0{suffix}")
                    }
                };
                (key, param.kind)
            })
            .collect()
    }

    /// A value of `kind`, and one of another kind.
    fn values_of(kind: hares_equipment::ParamKind) -> (Value, Value) {
        use hares_equipment::ParamKind;
        match kind {
            ParamKind::Number => (json!(1.0), json!("1")),
            ParamKind::Text => (json!("constant"), json!(1.0)),
            ParamKind::Bool => (json!(true), json!(1.0)),
            ParamKind::NumberList => (json!([1.0]), json!(1.0)),
        }
    }

    /// The keys of a raw config's parameters.
    fn raw_keys(cfg: &hares_equipment::EquipmentConfig) -> Vec<&String> {
        match &cfg.payload {
            hares_equipment::ConfigPayload::Raw { data } => data.keys().collect(),
            hares_equipment::ConfigPayload::Typed { .. } => panic!("a raw payload"),
        }
    }

    fn rejected_key(spec: &hares_io::EquipmentSpec, overrides: &Value) -> Option<String> {
        match merged_equipment_config(spec, overrides) {
            Ok(_) => None,
            Err(HaresError::InvalidEquipmentParameter { key, .. }) => Some(key),
            Err(other) => panic!("{}: not a parameter error: {other}", spec.name),
        }
    }

    /// Derived from the lists, so a key added to a load's list is covered:
    /// every key a load kind reads takes an override of the kind it reads
    /// (`equipment_id` aside, which ids reserve) and rejects one of another
    /// kind, and every other key is rejected by name: the other kinds' keys
    /// this kind does not read, a misspelling of each of its own, and a
    /// numbered key past its family's count.
    #[test]
    fn a_load_takes_an_override_of_exactly_the_parameters_it_reads() {
        for class in LOAD_OF_EACH_KIND {
            let spec = raw_load_spec(class);
            let params = hares_equipment::raw_params_for_class(class).expect("a load class");
            let own = listed_keys(params);
            for (key, kind) in &own {
                let (value, wrong) = values_of(*kind);
                let rejected = rejected_key(&spec, &json!({ class: { key.as_str(): value } }));
                if key == hares_equipment::config::KEY_EQUIPMENT_ID {
                    assert_eq!(rejected.as_deref(), Some(key.as_str()), "{class}");
                    continue;
                }
                assert_eq!(rejected, None, "{class} must take an override of {key}");
                assert_eq!(
                    rejected_key(&spec, &json!({ class: { key.as_str(): wrong } })).as_deref(),
                    Some(key.as_str()),
                    "{class} must reject {key} given {wrong}"
                );
            }
            let mut others: Vec<String> = hares_equipment::raw_params::ALL
                .iter()
                .flat_map(|other| listed_keys(other))
                .map(|(key, _)| key)
                .filter(|key| !params.names(key))
                .collect();
            others.extend(own.iter().map(|(key, _)| format!("{key}x")));
            others.extend([
                "month_multiplier_12".to_string(),
                "phase_4_power_kw".to_string(),
            ]);
            others.push("fuel_type".to_string());
            for key in others
                .iter()
                .filter(|key| !params.reads(key, &|name| spec.parameters.get(name)))
            {
                let overrides = json!({ class: { key.as_str(): 1.0 } });
                assert_eq!(
                    rejected_key(&spec, &overrides).as_deref(),
                    Some(key.as_str()),
                    "{class} must reject an override of {key}"
                );
            }
        }
    }

    /// The rejection names the parameters the load does read.
    #[test]
    fn a_rejected_override_lists_what_the_load_reads() {
        let err = merged_equipment_config(
            &raw_load_spec("Plug Loads"),
            &json!({ "Plug Loads": { "usage_multiplir": 2.0 } }),
        )
        .expect_err("a misspelt parameter");
        let message = err.to_string();
        for key in [
            "usage_multiplier",
            "power_constant_kw",
            "month_multiplier_<n>",
        ] {
            assert!(message.contains(key), "{key} missing from: {message}");
        }
    }

    /// A wildcard parameter a load does not read is skipped for that load;
    /// one it reads reaches it, under either wildcard spelling.
    #[test]
    fn a_wildcard_reaches_the_loads_that_read_its_parameters() {
        for wildcard in ["all", "*"] {
            let overrides = json!({ wildcard: { "usage_multiplier": 2.0, "n_units": 3.0 } });
            let plug = merged_equipment_config(&raw_load_spec("Plug Loads"), &overrides)
                .expect("the scheduled load takes the usage multiplier");
            assert_eq!(plug.get_f64("usage_multiplier"), Some(2.0), "{wildcard}");
            assert!(!raw_keys(&plug).iter().any(|key| *key == "n_units"));
            let washer = merged_equipment_config(&raw_load_spec("Dishwasher"), &overrides)
                .expect("the wet appliance takes the unit count");
            assert_eq!(washer.get_f64("n_units"), Some(3.0), "{wildcard}");
            assert!(
                !raw_keys(&washer)
                    .iter()
                    .any(|key| *key == "usage_multiplier")
            );
        }
    }

    /// A numbered wildcard key reaches a load only within its family's
    /// count as every layer leaves it: a four-phase dishwasher skips a
    /// fifth phase unless the wildcard also lengthens its cycle.
    #[test]
    fn a_wildcard_numbered_key_reaches_a_load_within_its_count() {
        let washer = raw_load_spec("Dishwasher");
        let short =
            merged_equipment_config(&washer, &json!({ "all": { "phase_4_power_kw": 1.0 } }))
                .expect("skipped for a four-phase cycle");
        assert!(
            !raw_keys(&short)
                .iter()
                .any(|key| *key == "phase_4_power_kw")
        );
        let long = merged_equipment_config(
            &washer,
            &json!({ "all": { "phase_len": 5, "phase_4_power_kw": 1.0 } }),
        )
        .expect("the fifth phase exists");
        assert_eq!(long.get_f64("phase_4_power_kw"), Some(1.0));
    }

    /// Both wildcard spellings are an error, never one shadowing the other.
    #[test]
    fn both_wildcard_spellings_are_an_error() {
        let overrides = json!({ "all": { "usage_multiplier": 2.0 }, "*": { "zone_id": 1 } });
        let err = merged_equipment_config(&raw_load_spec("Plug Loads"), &overrides)
            .expect_err("two wildcards");
        assert!(err.to_string().contains("both 'all' and '*'"), "{err}");
        let err = validate_wildcard_override(&overrides, &[&raw_load_spec("Plug Loads")])
            .expect_err("two wildcards");
        assert!(err.to_string().contains("both 'all' and '*'"), "{err}");
    }

    /// A wildcard parameter no equipment of the dwelling reads is a
    /// misspelling, rejected by name; one some equipment reads is not.
    #[test]
    fn a_wildcard_parameter_must_be_read_by_some_equipment() {
        let plug = raw_load_spec("Plug Loads");
        let washer = raw_load_spec("Dishwasher");
        let furnace = gas_furnace_spec();
        let population = [&plug, &washer, &furnace];
        for overrides in [
            json!({ "all": { "usage_multiplier": 2.0 } }),
            json!({ "*": { "n_units": 2.0 } }),
            json!({ "all": { "afue": 0.9 } }),
            json!({ "all": { "frac_sensible": 0.4 } }),
            json!({ "all": { "zip": { "pf": 0.9 } } }),
        ] {
            validate_wildcard_override(&overrides, &population)
                .unwrap_or_else(|err| panic!("{overrides}: {err}"));
        }
        for (wildcard, key) in [
            ("all", "usage_multiplir"),
            ("*", "afuee"),
            ("all", "month_multiplier_12"),
            ("*", "phase_4_power_kw"),
        ] {
            let err = validate_wildcard_override(&json!({ wildcard: { key: 2.0 } }), &population)
                .expect_err("read by no equipment");
            assert!(
                matches!(&err, HaresError::InvalidEquipmentParameter { key: k, .. } if k == key),
                "{err}"
            );
            assert!(err.to_string().contains(wildcard), "{err}");
        }
        let err = validate_wildcard_override(&json!({ "all": { "n_units": 2.0 } }), &[&plug])
            .expect_err("no wet appliance reads it here");
        assert!(err.to_string().contains("n_units"), "{err}");
        validate_wildcard_override(
            &json!({ "all": { "phase_4_power_kw": 1.0 }, "Dishwasher": { "phase_len": 5 } }),
            &population,
        )
        .expect("the dishwasher's own entry gives it a fifth phase");
        let raw_ev = raw_load_spec("EV");
        let err =
            validate_wildcard_override(&json!({ "all": { "soc_max": 0.9 } }), &[&plug, &raw_ev])
                .expect_err("a raw spec no list declares vouches for no parameter");
        assert!(err.to_string().contains("soc_max"), "{err}");
    }

    /// The typed furnace takes the wildcard parameters its schema has and
    /// skips the loads' ones.
    #[test]
    fn a_typed_config_takes_the_wildcard_parameters_its_schema_has() {
        let overrides = json!({ "all": { "afue": 0.9, "usage_multiplier": 2.0 } });
        let merged = merged_equipment_config(&gas_furnace_spec(), &overrides)
            .expect("the furnace skips the load parameter");
        let cfg = merged
            .require_typed::<GasFurnaceConfig>("Gas Furnace")
            .expect("typed");
        assert!((cfg.afue - 0.9).abs() < 1e-12);
    }

    /// The reserved `zip` object reaches a load from its own entry and from
    /// a wildcard; `equipment_id` is rejected from either.
    #[test]
    fn reserved_override_keys() {
        for overrides in [
            json!({ "Plug Loads": { "zip": { "pf": 0.9 } } }),
            json!({ "all": { "zip": { "pf": 0.9 } } }),
        ] {
            let merged = merged_equipment_config(&raw_load_spec("Plug Loads"), &overrides)
                .unwrap_or_else(|err| panic!("{overrides}: {err}"));
            assert_eq!(merged.zip.map(|zip| zip.pf), Some(0.9), "{overrides}");
        }
        for (source, overrides) in [
            ("Plug Loads", json!({ "Plug Loads": { "equipment_id": 3 } })),
            ("all", json!({ "all": { "equipment_id": 3 } })),
        ] {
            let err = merged_equipment_config(&raw_load_spec("Plug Loads"), &overrides)
                .expect_err("ids are the dwelling's");
            assert!(
                matches!(&err, HaresError::InvalidEquipmentParameter { key, .. } if key == "equipment_id")
                    && err.to_string().contains(&format!("'{source}' override")),
                "{err}"
            );
        }
    }

    /// An override may add a parameter the resolver left out: the usage
    /// multiplier and the zone.
    #[test]
    fn an_override_adds_a_parameter_the_resolver_left_out() {
        let spec = raw_load_spec("Plug Loads");
        assert!(!spec.parameters.contains_key("usage_multiplier"));
        assert!(!spec.parameters.contains_key("zone_id"));
        let merged = merged_equipment_config(
            &spec,
            &json!({ "Plug Loads": { "usage_multiplier": 1.5, "zone_id": 2 } }),
        )
        .expect("both are parameters the load reads");
        assert_eq!(merged.get_f64("usage_multiplier"), Some(1.5));
        assert_eq!(merged.get_f64("zone_id"), Some(2.0));
    }

    /// A parameter a load reads, given a value no config value holds (an
    /// object, a null, a list with a non-number), is rejected by name rather
    /// than read as absent.
    #[test]
    fn a_parameter_value_no_config_holds_is_rejected() {
        for value in [json!({ "nested": 0.5 }), Value::Null, json!([1.0, "two"])] {
            for key in [
                "radiant_share_of_sensible",
                "month_multiplier_3",
                "usage_multiplier",
            ] {
                let overrides = json!({ "Plug Loads": { key: value.clone() } });
                assert_eq!(
                    rejected_key(&raw_load_spec("Plug Loads"), &overrides).as_deref(),
                    Some(key),
                    "{key} = {value}"
                );
            }
        }
    }

    /// The gas water heater reads `conversion_efficiency` ahead of the
    /// EF/UEF test-procedure fields, so an override of those could never
    /// apply on a config that carries the conversion efficiency: it is
    /// rejected naming `conversion_efficiency` instead of being accepted and
    /// silently ignored.
    #[test]
    fn shadowed_energy_factor_override_is_rejected() {
        let spec = typed_gas_water_heater_spec(Some(0.78));
        for (layer, overrides) in [
            (
                "own",
                json!({ "Gas Water Heater": { "energy_factor": 0.95 } }),
            ),
            ("wildcard", json!({ "all": { "energy_factor": 0.95 } })),
            (
                "own uef",
                json!({ "Gas Water Heater": { "uniform_energy_factor": 0.95 } }),
            ),
        ] {
            let err = merged_equipment_config(&spec, &overrides)
                .expect_err("the shadowed override is rejected");
            assert!(
                err.to_string().contains("conversion_efficiency"),
                "{layer}: the error names the field that shadows it, got: {err}"
            );
        }

        // Without the conversion efficiency the same override applies: the
        // EF field is the config's own then.
        let spec = typed_gas_water_heater_spec(None);
        let merged = merged_equipment_config(
            &spec,
            &json!({ "Gas Water Heater": { "energy_factor": 0.95 } }),
        )
        .expect("an override of a field the config reads without a shadow applies");
        let merged_data = match &merged.payload {
            hares_equipment::ConfigPayload::Typed { data, .. } => data.clone(),
            other => panic!("typed payload expected, got {other:?}"),
        };
        assert_eq!(
            merged_data.get("energy_factor"),
            Some(&json!(0.95)),
            "the override landed on the payload"
        );

        // The spec-level channel (a caller-built spec's typed_overrides)
        // rejects the same shadow the same way.
        let mut spec = typed_gas_water_heater_spec(Some(0.78));
        spec.typed_overrides
            .insert("energy_factor".to_string(), json!(0.95));
        let err = apply_spec_bag_to_typed_config(&mut spec)
            .expect_err("the spec-level shadowed override is rejected");
        assert!(
            err.to_string().contains("conversion_efficiency"),
            "the error names the field that shadows it, got: {err}"
        );
    }

    fn typed_gas_water_heater_spec(conversion_efficiency: Option<f64>) -> hares_io::EquipmentSpec {
        let cfg = hares_equipment::GasWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            fuel_type: FuelType::Gas,
            tank_volume_m3: Some(0.151),
            tank_height_m: None,
            energy_factor: Some(0.6),
            uniform_energy_factor: None,
            heating_capacity_w: Some(4_500.0),
            ua_w_per_k: Some(2.5),
            setpoint_c: Some(52.0),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
            fan_power_w: None,
        };
        let typed = hares_equipment::EquipmentConfig::from_typed(
            "Gas Water Heater".to_string(),
            "Gas Water Heater".to_string(),
            cfg,
        )
        .expect("the config is well-formed");
        hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Water Heater".to_string(),
            fuel_type: FuelType::Gas,
            parameters: serde_json::Map::new(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(typed),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        }
    }

    /// The HPXML spelling of a fraction in an override replaces the resolved
    /// fraction; both spellings in one layer are an error.
    #[test]
    fn an_hpxml_gain_spelling_replaces_the_resolved_fraction() {
        let merged = merged_equipment_config(
            &raw_load_spec("Plug Loads"),
            &json!({ "all": { "frac_latent": 0.1 }, "Plug Loads": { "frac_sensible": 0.3 } }),
        )
        .expect("both spellings are read");
        assert_eq!(merged.get_f64("sensible_gain_fraction"), Some(0.3));
        assert_eq!(merged.get_f64("latent_gain_fraction"), Some(0.1));
        let err = merged_equipment_config(
            &raw_load_spec("Plug Loads"),
            &json!({ "Plug Loads": { "frac_sensible": 0.3, "sensible_gain_fraction": 0.2 } }),
        )
        .expect_err("one fraction twice");
        assert!(
            matches!(&err, HaresError::InvalidEquipmentParameter { key, .. } if key == "frac_sensible"),
            "{err}"
        );
    }
}
