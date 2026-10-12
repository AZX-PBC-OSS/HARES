//! The resolved-parameter read-back: the model the physics actually uses,
//! computed at assembly, after defaults, autosizing and overrides and
//! before warm-up, and read back through [`crate::Dwelling::resolved_model`]
//! and `DwellingBlueprint::validate` (the runtime-memory ledger's read-back
//! entry). SI units throughout.

use chrono::NaiveDateTime;
use hares_envelope::boundary_rc::{BoundaryDiagnostic, BoundaryInput, ExteriorTarget, RCPath};
use hares_io::hpxml::BoundaryType;
use hares_io::{SetpointSource, SiteLocation};
use hares_types::{EndUse, EquipmentId, ResolvedParameter};

/// The envelope half of the read-back, computed where the RC network is
/// assembled (`build_default_solvers`) and carried to the dwelling's
/// assembly, which adds the equipment's half.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEnvelope {
    pub boundaries: Vec<ResolvedBoundary>,
    pub windows: Vec<ResolvedWindow>,
}

/// The parameters the physics actually uses, per equipment: the built
/// equipment's own resolved values (the trait method), the active
/// setpoint sources the schedule resolution stamped on its spec, and the
/// end use the equipment declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModel {
    pub equipment: Vec<ResolvedEquipment>,
    pub boundaries: Vec<ResolvedBoundary>,
    pub windows: Vec<ResolvedWindow>,
    pub infiltration: ResolvedInfiltration,
    pub site: SiteLocation,
    /// The run's horizon and clock, so a caller can check a table's row
    /// count against the engine.
    pub clock: ResolvedClock,
}

/// The run's clock at assembly: the first step's local standard time, the
/// site offset it is labelled at, the step, and the simulated step count
/// of the configured horizon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedClock {
    /// The first step's local standard time (the fixed-offset clock's own
    /// wall clock; DST never moves it).
    pub start: NaiveDateTime,
    /// The site's standard-time offset from UTC, in seconds.
    pub utc_offset_s: i32,
    /// The step, in seconds.
    pub time_res_s: u32,
    /// The simulated step count: the configured duration's whole steps.
    pub steps: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEquipment {
    pub name: String,
    pub id: EquipmentId,
    pub end_use: EndUse,
    pub parameters: Vec<ResolvedParameter>,
    /// The active heating-side setpoint source, copied from the spec the
    /// schedule resolution stamped (`SetpointSource`, the inputs ledger's
    /// setpoint-source rule).
    pub heating_setpoint: Option<SetpointSource>,
    /// The active cooling-side setpoint source.
    pub cooling_setpoint: Option<SetpointSource>,
}

/// One assembled boundary: the effective resistance the RC network
/// conducts through (films included), the films that make up part of it,
/// the network's capacitance, and the layers' own resistances and
/// capacitances (per area), outside to inside. Layers are empty for
/// slab (F-factor) boundaries: the F-factor perimeter conductance is not
/// a layer stack.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBoundary {
    pub id: String,
    pub kind: BoundaryType,
    pub area_m2: f64,
    /// Assembly R, films included: what the network conducts through.
    /// For a same-zone fin the network's fin chain (see `same_zone`).
    pub r_effective_m2k_w: f64,
    /// Interior plus exterior film resistance (the exterior film zero
    /// where the assembly applies none).
    pub films_m2k_w: f64,
    /// The boundary's assembled thermal capacitance.
    pub c_j_k: f64,
    /// Whether the boundary faces no outside: the parser's same-zone
    /// surfaces (the interior partition wall, the furniture mass), whose
    /// construction the network keeps as a dead-end fin (the interior
    /// half of the stack). The effective R's convention follows it.
    pub same_zone: bool,
    pub layers: Vec<ResolvedLayer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLayer {
    pub r_m2k_w: f64,
    /// Per-area capacitance.
    pub c_j_m2k: f64,
}

/// One window's resolved glazing properties: the effective SHGC and
/// U-factor the solar and conduction models use (the shading coefficients
/// applied), not the raw HPXML declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWindow {
    pub id: String,
    pub area_m2: f64,
    pub u_w_m2k: f64,
    pub shgc: f64,
}

/// The dwelling's infiltration inputs as the assembly resolved them: the
/// blower-door ACH50, the effective leakage area (SI), or the constant
/// rate the synthetic inputs fix.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedInfiltration {
    pub ach50: Option<f64>,
    pub ela_m2: Option<f64>,
    pub constant_ach: Option<f64>,
}

/// The envelope half of the read-back, from the assembled RC network: the
/// boundary inputs (the layers as fed to assembly), the RC diagnostics
/// (the effective R and C the network reports) and the thermal config's
/// resolved window properties keyed by surface id (the boundary index)
/// with the ids in the config's sorted order.
pub(crate) fn resolved_envelope(
    building: &hares_io::hpxml::Building,
    boundary_inputs: &[BoundaryInput],
    diagnostics: &[BoundaryDiagnostic],
    window_properties: &std::collections::HashMap<
        u32,
        hares_envelope::thermal_solver::WindowSolarProperties,
    >,
    window_ids_sorted: &[u32],
) -> ResolvedEnvelope {
    let mut boundaries = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics {
        let idx = diagnostic.boundary_idx;
        let Some(input) = boundary_inputs.get(idx) else {
            continue;
        };
        let Some(bd) = building.boundaries.get(idx) else {
            continue;
        };
        let same_zone = matches!(diagnostic.exterior_target, ExteriorTarget::Zone(zone) if zone == diagnostic.interior_zone_idx);
        let films = diagnostic.r_film_int_m2_k_w
            + if same_zone {
                0.0
            } else {
                diagnostic.r_film_ext_m2_k_w
            };
        let layers = resolved_layers(input, same_zone, &bd.boundary_type);
        // The read-back's effective resistance, films included, per the
        // boundary kind:
        //
        // - An outside-facing boundary: the full chain, the layers'
        //   series plus both films. The diagnostic's total is that same
        //   series, and the network's discrete chain sums to it.
        // - A same-zone fin (no outside): the network's actual discrete
        //   chain, which drops one half-resistor from the kept layers'
        //   series. The precomputed path pops the innermost kept layer's
        //   inner half (the pad-average's last resistor); the material
        //   path never wires the cut layer's exterior-facing resistor;
        //   the one interior film rides the zone-facing edge. This is
        //   the number the physics conducts through.
        // - The diagnostic's same-zone total is a different record with
        //   a different convention (the full stack halved plus the one
        //   film, the per-component heat-gain tracking's UA): in general
        //   it equals neither the kept layers' series nor the network's
        //   chain, and the read-back does not report it.
        let r_effective_m2k_w = if same_zone {
            films + same_zone_fin_chain(diagnostic.path, &layers)
        } else {
            diagnostic.r_total_m2_k_w
        };
        boundaries.push(ResolvedBoundary {
            id: bd.id.clone(),
            kind: bd.boundary_type.clone(),
            area_m2: diagnostic.area_m2,
            r_effective_m2k_w,
            films_m2k_w: films,
            c_j_k: diagnostic.capacitance_j_k,
            same_zone,
            layers,
        });
    }
    // The resolved windows, in the thermal config's sorted surface-id
    // order: surface id is the boundary index (the wiring's rule), so the
    // id and area come from the boundary the surface id names.
    let mut resolved_windows = Vec::new();
    for surface_id in window_ids_sorted {
        let Some(properties) = window_properties.get(surface_id) else {
            continue;
        };
        let Some(bd) = building.boundaries.get(*surface_id as usize) else {
            continue;
        };
        resolved_windows.push(ResolvedWindow {
            id: bd.id.clone(),
            area_m2: properties.area_m2,
            u_w_m2k: properties.u_factor_w_m2_k,
            shgc: properties.shgc,
        });
    }
    ResolvedEnvelope {
        boundaries,
        windows: resolved_windows,
    }
}

/// The layers the network builds for one boundary, outside to inside:
/// the precomputed LUT layers when the boundary took that path, the
/// material layers' effective resistances (the framing correction the
/// parallel-path method applies) when it took the material path, and a
/// single layer carrying the fallback assembly R when no layer stack
/// exists. Slabs report none: the F-factor perimeter conductance is the
/// read-back's one excluded path.
fn resolved_layers(
    input: &BoundaryInput,
    same_zone: bool,
    kind: &BoundaryType,
) -> Vec<ResolvedLayer> {
    if *kind == BoundaryType::Slab {
        return Vec::new();
    }
    if !input.precomputed_rc.is_empty() {
        let layers = input
            .precomputed_rc
            .iter()
            .map(|l| ResolvedLayer {
                r_m2k_w: l.resistance_m2_k_w,
                c_j_m2k: l.capacitance_kj_m2_k * 1000.0,
            })
            .collect::<Vec<_>>();
        // A same-zone boundary's network keeps the interior half of the
        // stack; the read-back reports the layers it keeps.
        return if same_zone {
            let keep = layers.len().div_ceil(2);
            layers[layers.len() - keep..].to_vec()
        } else {
            layers
        };
    }
    let valid: Vec<_> = input
        .material_layers
        .iter()
        .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
        .collect();
    if valid.is_empty() {
        // The fallback-R path: the one lumped resistance the network
        // builds, reported as the single layer it is.
        return vec![ResolvedLayer {
            r_m2k_w: input.fallback_r_m2_k_w,
            c_j_m2k: 0.0,
        }];
    }
    let layers = valid
        .iter()
        .map(|l| {
            let k = hares_envelope::boundary_rc::parallel_path_conductivity(
                l.conductivity_w_m_k,
                input.framing_factor,
            );
            ResolvedLayer {
                r_m2k_w: l.thickness_m / k,
                c_j_m2k: l.density_kg_m3 * l.specific_heat_j_kg_k * l.thickness_m,
            }
        })
        .collect::<Vec<_>>();
    if same_zone {
        let keep = layers.len().div_ceil(2);
        layers[layers.len() - keep..].to_vec()
    } else {
        layers
    }
}

/// The same-zone fin's network chain (per area, films excluded): the kept
/// layers' series minus the half-resistor the network's discrete build
/// drops. The precomputed path's pad-and-average makes the innermost kept
/// layer's inner half the last resistor and pops it; the material path
/// skips the cut layer's outer half, leaving its inner half in the last
/// inter-layer resistor. The fallback path builds no same-zone boundary
/// (its diagnostic arm is outside-facing only), so its arm is unreachable
/// for a reported fin and carries nothing to drop.
fn same_zone_fin_chain(path: RCPath, layers: &[ResolvedLayer]) -> f64 {
    let kept = layers.iter().map(|l| l.r_m2k_w).sum::<f64>();
    let dropped = match path {
        RCPath::Precomputed => layers.last().map(|l| l.r_m2k_w / 2.0).unwrap_or(0.0),
        RCPath::MaterialLayer => layers.first().map(|l| l.r_m2k_w / 2.0).unwrap_or(0.0),
        RCPath::FallbackR => 0.0,
    };
    kept - dropped
}

/// The infiltration inputs as the assembly resolved them, from the parsed
/// building's values (the ACH50 the leakage conversion produced, the ELA
/// converted from cm², the constant rate the synthetic inputs fix).
pub(crate) fn resolved_infiltration(building: &hares_io::hpxml::Building) -> ResolvedInfiltration {
    ResolvedInfiltration {
        ach50: building.infiltration_ach50,
        ela_m2: building.infiltration_ela_cm2.map(|cm2| cm2 / 10_000.0),
        constant_ach: building.infiltration_constant_ach,
    }
}

#[cfg(test)]
mod tests {
    use super::{ResolvedLayer, same_zone_fin_chain};
    use hares_envelope::boundary_rc::RCPath;

    fn layers(rs: &[f64]) -> Vec<ResolvedLayer> {
        rs.iter()
            .map(|&r| ResolvedLayer {
                r_m2k_w: r,
                c_j_m2k: 0.0,
            })
            .collect()
    }

    /// The precomputed path's rule: the innermost kept layer's inner half
    /// is the popped resistor, so the chain is the kept series minus half
    /// the last layer.
    #[test]
    fn fin_chain_precomputed_drops_the_innermost_kept_half() {
        let kept = layers(&[0.2269, 0.07923]);
        let chain = same_zone_fin_chain(RCPath::Precomputed, &kept);
        let expected: f64 = 0.2269 + 0.07923 / 2.0;
        assert_eq!(chain.to_bits(), expected.to_bits());
    }

    /// The material path's rule: the cut layer's outer half is never
    /// wired, so the chain is the kept series minus half the first layer.
    #[test]
    fn fin_chain_material_drops_the_cut_layer_outer_half() {
        let kept = layers(&[0.5, 0.25, 0.25]);
        let chain = same_zone_fin_chain(RCPath::MaterialLayer, &kept);
        let expected: f64 = 0.5 / 2.0 + 0.25 + 0.25;
        assert_eq!(chain.to_bits(), expected.to_bits());
    }
}
