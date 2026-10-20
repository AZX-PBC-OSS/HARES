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

/// Where a boundary's resistance came from: the winner of the opaque-R
/// precedence (the numeric-fixes ledger's opaque-R precedence entry). A
/// declared assembly R-value (`AssemblyEffectiveRValue` or `RValue`) sets
/// the matched LUT row's layer resistances when the boundary takes the
/// LUT path, or the fallback's single resistor when it does not; the LUT
/// row's own construction R wins only where nothing is declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RValueSource {
    /// The declared assembly R-value set the resistance: over the matched
    /// LUT row's layer distribution (scaled by `k`, the row's capacitances
    /// kept), or as the fallback's single resistor on a LUT miss.
    DeclaredAssemblyR,
    /// The matched LUT row's own construction R: nothing declared, the
    /// row's layers as they stand (OCHRE's behavior).
    LutRowR,
    /// The boundary's explicit material layers.
    MaterialLayers,
    /// The single-resistor fallback with nothing declared: the
    /// `NominalRValue` layer sum, else the default R, after a LUT miss.
    FallbackR,
    /// The F-factor perimeter method (a slab): no declared R drives it.
    SlabFfactor,
}

impl RValueSource {
    /// The provenance label: the winning source, in the form the Python
    /// read-back records.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::DeclaredAssemblyR => "declared assembly R",
            Self::LutRowR => "LUT row",
            Self::MaterialLayers => "material layers",
            Self::FallbackR => "fallback R",
            Self::SlabFfactor => "F-factor slab",
        }
    }
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
    /// The winning source of the boundary's resistance (the opaque-R
    /// precedence): the declared assembly R-value, the LUT row's own
    /// construction R, the explicit material layers, the fallback, or
    /// the F-factor slab method.
    pub r_source: RValueSource,
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
    /// The infiltration height the AIM-2 model runs at, in metres, when the
    /// model runs: the declared `<InfiltrationHeight>`, else the
    /// OpenStudio-HPXML estimate (the defaults code the infiltration
    /// default entry ports). `None` when the run takes no AIM-2 path (a
    /// constant ACH, or no infiltration measurement).
    pub height_m: Option<f64>,
    /// The flue-or-chimney flag the AIM-2 model runs at: the declared
    /// `<HasFlueOrChimneyInConditionedSpace>`, else the OpenStudio-HPXML
    /// rule over the resolved heating systems and water heaters. `None`
    /// when the run takes no AIM-2 path.
    pub has_flue: Option<bool>,
    /// The shielding class the infiltration methods read: the declared
    /// `<ShieldingofHome>`, else the facility type's default; `None` when
    /// the resolution never had what it needs (no method read one).
    pub shielding: Option<hares_physics::infiltration::ShieldingClass>,
}

/// The AIM-2 model's evaluated defaults, recorded as the solver built them:
/// the height the coefficients ran at and the flue flag they ran with, the
/// declared inputs when given and the OpenStudio-HPXML rules' outcomes
/// otherwise, with the shielding class every infiltration method read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aim2Defaults {
    pub height_m: f64,
    pub has_flue: bool,
    pub shielding: hares_physics::infiltration::ShieldingClass,
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
        let (layers, r_source) = resolved_layers(
            input,
            same_zone,
            &bd.boundary_type,
            bd.assembly_r_value_m2_k_w.is_some(),
        );
        // The read-back's effective resistance, films included, per the
        // boundary kind:
        //
        // - An outside-facing boundary: the full chain, the layers'
        //   series plus both films. The diagnostic's total is that same
        //   series, and the network's discrete chain sums to it.
        // - A same-zone fin (no outside): the network's actual discrete
        //   chain, which drops one half-resistor from the kept layers'
        //   series: the cut layer's exterior-facing half. Both build
        //   paths drop the same half: the precomputed path's pad-and-
        //   average makes the cut stub the first resistor and removes it;
        //   the material path never wires the cut layer's exterior-facing
        //   half. The one interior film rides the zone-facing edge. This
        //   is the number the physics conducts through.
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
            r_source,
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

/// The layers the network builds for one boundary, outside to inside, and
/// the winning source of the boundary's resistance: the precomputed LUT
/// layers when the boundary took that path (the declared assembly R's when
/// one is declared, else the row's own), the material layers' effective
/// resistances (the framing correction the parallel-path method applies)
/// when it took the material path, and a single layer carrying the
/// fallback assembly R when no layer stack exists. Slabs report none and
/// the F-factor method as their source: the F-factor perimeter conductance
/// is not a layer stack.
fn resolved_layers(
    input: &BoundaryInput,
    same_zone: bool,
    kind: &BoundaryType,
    has_declared_r: bool,
) -> (Vec<ResolvedLayer>, RValueSource) {
    if *kind == BoundaryType::Slab {
        return (Vec::new(), RValueSource::SlabFfactor);
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
        let kept = if same_zone {
            let keep = layers.len().div_ceil(2);
            layers[layers.len() - keep..].to_vec()
        } else {
            layers
        };
        let source = if has_declared_r {
            RValueSource::DeclaredAssemblyR
        } else {
            RValueSource::LutRowR
        };
        return (kept, source);
    }
    let valid: Vec<_> = input
        .material_layers
        .iter()
        .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
        .collect();
    if valid.is_empty() {
        // The fallback-R path: the one lumped resistance the network
        // builds, reported as the single layer it is. The declared
        // assembly R is the lump's source when one is declared.
        let source = if has_declared_r {
            RValueSource::DeclaredAssemblyR
        } else {
            RValueSource::FallbackR
        };
        return (
            vec![ResolvedLayer {
                r_m2k_w: input.fallback_r_m2_k_w,
                c_j_m2k: 0.0,
            }],
            source,
        );
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
        (
            layers[layers.len() - keep..].to_vec(),
            RValueSource::MaterialLayers,
        )
    } else {
        (layers, RValueSource::MaterialLayers)
    }
}

/// The same-zone fin's network chain (per area, films excluded): the kept
/// layers' series minus the cut layer's outer half-resistor, the one
/// resistor the network's discrete build never wires. Both build paths
/// drop the same half: the precomputed path's pad-and-average makes the
/// cut stub the first resistor and removes it (the zone-facing half stays
/// at the zone edge); the material path skips the cut layer's
/// exterior-facing half when it wires. The fallback path builds no
/// same-zone boundary (its diagnostic arm is outside-facing only), so its
/// arm is unreachable for a reported fin and carries nothing to drop.
fn same_zone_fin_chain(path: RCPath, layers: &[ResolvedLayer]) -> f64 {
    let kept = layers.iter().map(|l| l.r_m2k_w).sum::<f64>();
    let dropped = match path {
        RCPath::Precomputed | RCPath::MaterialLayer => {
            layers.first().map(|l| l.r_m2k_w / 2.0).unwrap_or(0.0)
        }
        RCPath::FallbackR => 0.0,
    };
    kept - dropped
}

/// The infiltration inputs as the assembly resolved them, from the parsed
/// building's values (the ACH50 the leakage conversion produced, the ELA
/// converted from cm², the constant rate the synthetic inputs fix).
pub(crate) fn resolved_infiltration(
    building: &hares_io::hpxml::Building,
    aim2: Option<&Aim2Defaults>,
    shielding: Option<hares_physics::infiltration::ShieldingClass>,
) -> ResolvedInfiltration {
    ResolvedInfiltration {
        ach50: building.infiltration_ach50,
        ela_m2: building.infiltration_ela_cm2.map(|cm2| cm2 / 10_000.0),
        constant_ach: building.infiltration_constant_ach,
        height_m: aim2.map(|defaults| defaults.height_m),
        has_flue: aim2.map(|defaults| defaults.has_flue),
        shielding,
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

    /// Both paths' rule, the precomputed path's stated: the cut layer's
    /// outer half-resistor is the pad-average's first resistor and is
    /// removed, so the chain is the kept series minus half the first
    /// kept layer. Dyadic values, so the subtracted form and the direct
    /// sum agree on bits: the other dropped half (half the last layer,
    /// 0.125) moves the chain off these bits.
    #[test]
    fn fin_chain_precomputed_drops_the_cut_layer_outer_half() {
        let kept = layers(&[0.5, 0.25]);
        let chain = same_zone_fin_chain(RCPath::Precomputed, &kept);
        let expected: f64 = 0.5 / 2.0 + 0.25;
        assert_eq!(chain.to_bits(), expected.to_bits());
    }

    /// The material path's rule, the same half-resistor the precomputed
    /// path drops: the cut layer's outer half is never wired, so the
    /// chain is the kept series minus half the first layer.
    #[test]
    fn fin_chain_material_drops_the_cut_layer_outer_half() {
        let kept = layers(&[0.5, 0.25, 0.25]);
        let chain = same_zone_fin_chain(RCPath::MaterialLayer, &kept);
        let expected: f64 = 0.5 / 2.0 + 0.25 + 0.25;
        assert_eq!(chain.to_bits(), expected.to_bits());
    }
}
