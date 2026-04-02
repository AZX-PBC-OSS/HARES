//! RC graph construction from building envelope boundary data.
//!
//! Assembles the continuous-time state-space matrices (A_c, B_ext) from
//! zone air nodes, material-layer nodes, and resistive connections.

use std::collections::{HashMap, HashSet};

use nalgebra::DMatrix;

use crate::NodeId;
use crate::rc_network::{RCNetwork, parallel_resistance};

// ── Physical constants ──────────────────────────────────────────────────────

/// Dry air density at ~20 °C, 101.325 kPa [kg/m³].
/// Matches OCHRE's 1.2041 for parity.
pub const AIR_DENSITY_KG_M3: f64 = 1.2041;
/// Specific heat of dry air [J/(kg·K)].
pub const AIR_CP_J_KG_K: f64 = 1006.0;
/// Default zone volume when floor area is unknown [m³].
pub const DEFAULT_VOLUME_M3: f64 = 200.0;
/// Default storey height [m].
pub const DEFAULT_HEIGHT_M: f64 = 2.5;
/// Interior mass multiplier applied to zone air capacitance.
pub const INTERIOR_MASS_MULTIPLIER: f64 = 7.0;
/// Floor capacitance for any RC node [J/K].
pub const MIN_CAPACITANCE_J_K: f64 = 1_000.0;
/// Default aggregate UA when no boundary data is available [W/K].
pub const DEFAULT_UA_W_PER_K: f64 = 120.0;
/// Default assembly R-value when nothing else is known [m²·K/W].
pub const DEFAULT_R_M2_K_W: f64 = 2.5;
/// Exterior air-film resistance [m²·K/W].
pub const R_FILM_EXTERIOR_M2_K_W: f64 = 0.03;
/// Interior air-film resistance [m²·K/W].
pub const R_FILM_INTERIOR_M2_K_W: f64 = 0.12;
/// NodeId for the outdoor temperature driving node.
pub const OUTDOOR_NODE_ID: u32 = u32::MAX - 1;
/// NodeId for the ground temperature driving node.
pub const GROUND_NODE_ID: u32 = u32::MAX;
/// First NodeId used for material-layer nodes (above zone air node range).
const LAYER_NODE_BASE: u32 = 1_000;

/// Conservative default timestep [s] for Fourier stability splitting.
const DEFAULT_DT_S: f64 = 3600.0;

/// Minimum density [kg/m³] to qualify a layer for automatic splitting.
/// Insulation and air gaps are excluded.
const SPLIT_MIN_DENSITY: f64 = 100.0;

/// Minimum conductivity [W/(m·K)] to qualify a layer for automatic splitting.
const SPLIT_MIN_CONDUCTIVITY: f64 = 0.1;

/// Compute the number of RC sub-layers needed for numerical stability.
///
/// Ensures the Fourier number Fo = alpha * dt / dx² <= 0.5 by choosing
/// dx_max = sqrt(2 * alpha * dt) and splitting accordingly.
fn split_layer_count(thickness_m: f64, conductivity: f64, density: f64, specific_heat: f64, dt_s: f64) -> usize {
    if density <= 0.0 || specific_heat <= 0.0 || conductivity <= 0.0 || thickness_m <= 0.0 {
        return 1;
    }
    let alpha = conductivity / (density * specific_heat);
    // EnergyPlus CondFD uses space discretization constant C=3 (Fo = 1/C ≈ 0.33):
    //   dx = sqrt(C × α × Δt)
    // Reference: EnergyPlus Engineering Reference §3.3.10 "Conduction Finite
    // Difference Solution Algorithm" — default C=3, inverse of Fourier number.
    // Our ZOH state-space solver is implicit and unconditionally stable, so this
    // is a spatial accuracy criterion, not a stability requirement.
    let c_discretization = 3.0;
    let dx_max = (c_discretization * alpha * dt_s).sqrt();
    let n = (thickness_m / dx_max).ceil() as usize;
    n.max(1)
}

// ── Input types ─────────────────────────────────────────────────────────────

/// Pre-computed RC layer values from OCHRE's material database.
/// Used as an alternative to raw material layer calculations.
#[derive(Debug, Clone)]
pub struct PrecomputedRCLayer {
    pub resistance_m2_k_w: f64,
    pub capacitance_kj_m2_k: f64,
}

/// Material layer data needed for RC construction.
#[derive(Debug, Clone)]
pub struct LayerInput {
    pub thickness_m: f64,
    pub conductivity_w_m_k: f64,
    pub density_kg_m3: f64,
    pub specific_heat_j_kg_k: f64,
    pub area_m2: f64,
}

impl LayerInput {
    fn effective_area(&self, fallback: f64) -> f64 {
        if self.area_m2 > 0.0 {
            self.area_m2
        } else {
            fallback
        }
    }
}

/// Where the exterior side of a boundary connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    any(debug_assertions, feature = "observe_detailed"),
    derive(serde::Serialize)
)]
pub enum ExteriorTarget {
    /// Another zone (by index, 0-based).
    Zone(usize),
    /// Outdoor air.
    Outdoor,
    /// Ground temperature.
    Ground,
}

/// Pre-resolved boundary data for RC construction.
#[derive(Debug, Clone)]
pub struct BoundaryInput {
    pub area_m2: f64,
    /// Interior zone index (0-based).
    pub interior_zone_idx: usize,
    /// Where the exterior side connects.
    pub exterior: ExteriorTarget,
    /// Material layers (exterior → interior order).
    pub material_layers: Vec<LayerInput>,
    /// Pre-computed RC layers from OCHRE LUT. When non-empty, these take
    /// priority over `material_layers`.
    pub precomputed_rc: Vec<PrecomputedRCLayer>,
    /// Assembly R-value fallback [m²·K/W], used when no valid material layers.
    pub fallback_r_m2_k_w: f64,
    /// Interior air-film resistance [m²·K/W] for this boundary.
    pub r_film_interior_m2_k_w: f64,
    /// Exterior air-film resistance [m²·K/W] for this boundary.
    pub r_film_exterior_m2_k_w: f64,
    /// Framing factor [-] for ASHRAE parallel-path conductivity correction.
    ///
    /// When `Some(ff)`, insulation layer conductivities are corrected:
    /// `k_eff = ff * k_wood + (1 - ff) * k_cavity` where k_wood = 0.144 W/(m·K).
    /// `None` = no correction (use raw layer conductivities uniformly).
    ///
    /// Only applied to raw material layers (`build_layered_boundary`). Precomputed RC
    /// layers from the OCHRE LUT already bake framing effects into their resistance values.
    pub framing_factor: Option<f64>,
}

/// Zone input: floor area, volume, and mass multiplier for capacitance derivation.
#[derive(Debug, Clone)]
pub struct ZoneInput {
    pub floor_area_m2: Option<f64>,
    pub volume_m3: Option<f64>,
    /// Effective thermal mass multiplier applied to zone air capacitance.
    /// Accounts for furniture and interior mass. Typical values:
    /// - Conditioned: 7.0 (standard furnished living space)
    /// - Foundation / Attic / Garage: 1.0 (air capacitance only)
    pub mass_multiplier: f64,
}

// ── Diagnostics ─────────────────────────────────────────────────────────────

/// Which RC construction path was used for a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    any(debug_assertions, feature = "observe_detailed"),
    derive(serde::Serialize)
)]
pub enum RCPath {
    /// Pre-computed layers from OCHRE LUT.
    Precomputed,
    /// Raw material layers from HPXML.
    MaterialLayer,
    /// Single-resistance fallback from assembly R-value.
    FallbackR,
}

/// Per-boundary diagnostic data captured during RC construction.
#[derive(Debug, Clone)]
#[cfg_attr(
    any(debug_assertions, feature = "observe_detailed"),
    derive(serde::Serialize)
)]
pub struct BoundaryDiagnostic {
    pub boundary_idx: usize,
    /// Effective steady-state UA [W/K]: area / R_total.
    pub ua_w_per_k: f64,
    /// Total thermal resistance including film [m²·K/W].
    pub r_total_m2_k_w: f64,
    /// Total thermal capacitance of all layer nodes [J/K].
    pub capacitance_j_k: f64,
    /// Number of RC nodes created for this boundary.
    pub n_rc_nodes: usize,
    pub interior_zone_idx: usize,
    pub exterior_target: ExteriorTarget,
    pub area_m2: f64,
    pub r_film_int_m2_k_w: f64,
    pub r_film_ext_m2_k_w: f64,
    /// Total resistance from zone air to innermost RC node [m²·K/W].
    /// Includes film resistance plus half the innermost layer's conduction.
    /// `None` for fallback-R boundaries with no RC nodes.
    pub r_zone_to_inner_m2_k_w: Option<f64>,
    pub path: RCPath,
    /// NodeId of the innermost RC node (closest to zone air).
    /// `None` for fallback-R boundaries with no RC nodes.
    pub inner_node: Option<NodeId>,
}

/// Diagnostics captured during RC network construction.
#[derive(Debug, Clone)]
#[cfg_attr(
    any(debug_assertions, feature = "observe_detailed"),
    derive(serde::Serialize)
)]
pub struct EnvelopeDiagnostics {
    pub boundaries: Vec<BoundaryDiagnostic>,
    pub zone_capacitances_j_k: Vec<f64>,
    pub total_ua_w_per_k: f64,
}

// ── Output ──────────────────────────────────────────────────────────────────

/// Per-boundary surface metadata for the caller to wire up LWR/solar injection.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceLayerInfo {
    /// NodeId of the outermost material-layer node for this boundary.
    pub outer_node: NodeId,
    /// NodeId of the innermost material-layer node (closest to zone air).
    pub inner_node: NodeId,
    /// Interior zone index (0-based) that this boundary belongs to.
    pub interior_zone_idx: usize,
}

/// Result of assembling the building RC network.
pub struct BuildingRC {
    /// Continuous-time state matrix.
    pub a_c: DMatrix<f64>,
    /// B matrix columns for external driving nodes (outdoor, ground).
    pub b_ext: DMatrix<f64>,
    /// Sorted internal NodeIds (row index → NodeId).
    pub internal_node_order: Vec<NodeId>,
    /// NodeId → state-vector row index (precomputed from `internal_node_order`).
    pub node_index: HashMap<NodeId, usize>,
    /// State-vector row for each zone air node (len = n_zones).
    pub zone_state_rows: Vec<usize>,
    /// Per-boundary outermost layer info: bd_idx → SurfaceLayerInfo.
    pub layer_info: HashMap<usize, SurfaceLayerInfo>,
    /// Column index of outdoor temperature in B_ext (if present).
    pub outdoor_col: Option<usize>,
    /// Column index of ground temperature in B_ext (if present).
    pub ground_col: Option<usize>,
    /// Number of external driving columns in B_ext.
    pub n_ext: usize,
    /// NodeId → thermal capacitance [J/K] for all internal nodes.
    pub node_capacitances: HashMap<NodeId, f64>,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Derive zone air-node capacitances [J/K] from zone volumes and mass multipliers.
pub fn derive_zone_capacitances(zones: &[ZoneInput]) -> Vec<f64> {
    zones
        .iter()
        .map(|z| {
            let volume = z
                .volume_m3
                .or_else(|| z.floor_area_m2.map(|a| a * DEFAULT_HEIGHT_M))
                .unwrap_or(DEFAULT_VOLUME_M3);
            (AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * volume * z.mass_multiplier)
                .max(MIN_CAPACITANCE_J_K)
        })
        .collect()
}

/// Derive per-zone aggregate UA [W/K] from boundary R-values.
pub fn derive_zone_uas(boundaries: &[BoundaryInput], n_zones: usize) -> Vec<f64> {
    let mut uas = vec![0.0; n_zones];
    for bd in boundaries {
        let area = bd.area_m2.max(0.0);
        if area <= 0.0 {
            continue;
        }
        let r_total = bd.fallback_r_m2_k_w.max(1e-6);
        uas[bd.interior_zone_idx] += area / r_total;
    }
    for ua in &mut uas {
        if *ua <= 0.0 {
            *ua = DEFAULT_UA_W_PER_K;
        }
    }
    uas
}

/// Assemble the multi-layer RC network from pre-resolved boundary data.
///
/// Returns the continuous-time state-space matrices and metadata needed
/// to construct the thermal solver.
pub fn assemble_building_rc(
    boundaries: &[BoundaryInput],
    n_zones: usize,
    zone_capacitances: &[f64],
) -> Result<(BuildingRC, EnvelopeDiagnostics), String> {
    let outdoor_node = NodeId(OUTDOOR_NODE_ID);
    let ground_node = NodeId(GROUND_NODE_ID);

    // Pre-size from boundary data.
    let est_nodes = n_zones
        + boundaries
            .iter()
            .map(|b| b.material_layers.len())
            .sum::<usize>();
    let est_edges = est_nodes + boundaries.len();

    let mut initial_capacitances: HashMap<NodeId, f64> = HashMap::with_capacity(est_nodes);

    // Insert zone air nodes (IDs 1..=n_zones).
    for (zone_idx, cap) in zone_capacitances.iter().enumerate().take(n_zones) {
        let node = NodeId((zone_idx + 1) as u32);
        initial_capacitances.insert(node, cap.max(MIN_CAPACITANCE_J_K));
    }

    let mut graph = RcGraphState::with_capacity(initial_capacitances, est_edges);
    let mut layer_info: HashMap<usize, SurfaceLayerInfo> = HashMap::new();
    let mut outdoor_connected = false;
    let mut ground_connected = false;
    let mut boundary_diagnostics: Vec<BoundaryDiagnostic> = Vec::with_capacity(boundaries.len());

    for (bd_idx, bd) in boundaries.iter().enumerate() {
        if bd.area_m2 <= 0.0 {
            continue;
        }

        let interior_node = NodeId((bd.interior_zone_idx + 1) as u32);
        let exterior_node = match bd.exterior {
            ExteriorTarget::Zone(idx) => NodeId((idx + 1) as u32),
            ExteriorTarget::Outdoor => outdoor_node,
            ExteriorTarget::Ground => ground_node,
        };

        let same_zone = interior_node == exterior_node;

        match bd.exterior {
            ExteriorTarget::Outdoor => outdoor_connected = true,
            ExteriorTarget::Ground => ground_connected = true,
            ExteriorTarget::Zone(_) => {}
        }

        let bp = BoundaryParams {
            boundary_area: bd.area_m2,
            interior_node,
            exterior_node,
            same_zone,
            r_film_interior: bd.r_film_interior_m2_k_w,
            r_film_exterior: bd.r_film_exterior_m2_k_w,
            framing_factor: bd.framing_factor,
        };

        // Precomputed RC path (OCHRE LUT) takes priority over raw material layers.
        if !bd.precomputed_rc.is_empty() {
            let nodes_before = graph.next_layer_id;
            if let Some((inner, outer)) = graph.build_precomputed_boundary(&bd.precomputed_rc, &bp)
            {
                layer_info.insert(
                    bd_idx,
                    SurfaceLayerInfo {
                        outer_node: outer,
                        inner_node: inner,
                        interior_zone_idx: bd.interior_zone_idx,
                    },
                );
            }
            let n_nodes = (graph.next_layer_id - nodes_before) as usize;
            let r_layers: f64 = bd.precomputed_rc.iter().map(|l| l.resistance_m2_k_w).sum();
            // Same-zone boundaries use only inner half of layers and no exterior film.
            let r_effective = if same_zone { r_layers / 2.0 } else { r_layers };
            let r_total = r_effective
                + bd.r_film_interior_m2_k_w
                + if same_zone {
                    0.0
                } else {
                    bd.r_film_exterior_m2_k_w
                };
            debug_assert!(
                r_total > 0.0,
                "boundary {bd_idx}: precomputed R_total must be > 0"
            );
            let cap_total: f64 = bd
                .precomputed_rc
                .iter()
                .map(|l| l.capacitance_kj_m2_k * 1000.0 * bd.area_m2)
                .sum();
            debug_assert!(
                cap_total >= 0.0,
                "boundary {bd_idx}: capacitance must be >= 0"
            );
            let inner_node = if n_nodes > 0 {
                Some(NodeId(nodes_before + n_nodes as u32 - 1))
            } else {
                None
            };
            let r_zone_to_inner = if n_nodes > 0 {
                let r_inner_half = bd
                    .precomputed_rc
                    .last()
                    .map(|l| l.resistance_m2_k_w / 2.0)
                    .unwrap_or(0.0);
                Some(bd.r_film_interior_m2_k_w + r_inner_half)
            } else {
                None
            };
            boundary_diagnostics.push(BoundaryDiagnostic {
                boundary_idx: bd_idx,
                ua_w_per_k: bd.area_m2 / r_total.max(1e-6),
                r_total_m2_k_w: r_total,
                capacitance_j_k: cap_total,
                n_rc_nodes: n_nodes,
                interior_zone_idx: bd.interior_zone_idx,
                exterior_target: bd.exterior,
                area_m2: bd.area_m2,
                r_film_int_m2_k_w: bd.r_film_interior_m2_k_w,
                r_film_ext_m2_k_w: bd.r_film_exterior_m2_k_w,
                r_zone_to_inner_m2_k_w: r_zone_to_inner,
                path: RCPath::Precomputed,
                inner_node,
            });
            continue;
        }

        let valid_layers: Vec<&LayerInput> = bd
            .material_layers
            .iter()
            .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
            .collect();

        if !valid_layers.is_empty() {
            let nodes_before = graph.next_layer_id;
            if let Some((inner, outer)) = graph.build_layered_boundary(&valid_layers, &bp) {
                layer_info.insert(
                    bd_idx,
                    SurfaceLayerInfo {
                        outer_node: outer,
                        inner_node: inner,
                        interior_zone_idx: bd.interior_zone_idx,
                    },
                );
            }
            let n_nodes = (graph.next_layer_id - nodes_before) as usize;
            let r_layers: f64 = valid_layers
                .iter()
                .map(|l| l.thickness_m / l.conductivity_w_m_k)
                .sum();
            // Same-zone boundaries use only inner half of layers and no exterior film.
            let r_effective = if same_zone { r_layers / 2.0 } else { r_layers };
            let r_total = r_effective
                + bd.r_film_interior_m2_k_w
                + if same_zone {
                    0.0
                } else {
                    bd.r_film_exterior_m2_k_w
                };
            debug_assert!(
                r_total > 0.0,
                "boundary {bd_idx}: material-layer R_total must be > 0"
            );
            let cap_total: f64 = valid_layers
                .iter()
                .map(|l| {
                    let a = l.effective_area(bd.area_m2);
                    l.density_kg_m3 * l.specific_heat_j_kg_k * l.thickness_m * a
                })
                .sum();
            debug_assert!(
                cap_total >= 0.0,
                "boundary {bd_idx}: capacitance must be >= 0"
            );
            let inner_node = if n_nodes > 0 {
                Some(NodeId(nodes_before + n_nodes as u32 - 1))
            } else {
                None
            };
            let r_zone_to_inner = if n_nodes > 0 {
                let inner_layer = valid_layers.last().unwrap();
                let k =
                    parallel_path_conductivity(inner_layer.conductivity_w_m_k, bd.framing_factor);
                Some(bd.r_film_interior_m2_k_w + inner_layer.thickness_m / (2.0 * k))
            } else {
                None
            };
            boundary_diagnostics.push(BoundaryDiagnostic {
                boundary_idx: bd_idx,
                ua_w_per_k: bd.area_m2 / r_total.max(1e-6),
                r_total_m2_k_w: r_total,
                capacitance_j_k: cap_total,
                n_rc_nodes: n_nodes,
                interior_zone_idx: bd.interior_zone_idx,
                exterior_target: bd.exterior,
                area_m2: bd.area_m2,
                r_film_int_m2_k_w: bd.r_film_interior_m2_k_w,
                r_film_ext_m2_k_w: bd.r_film_exterior_m2_k_w,
                r_zone_to_inner_m2_k_w: r_zone_to_inner,
                path: RCPath::MaterialLayer,
                inner_node,
            });
        } else if !same_zone {
            // Fallback: single lumped resistance. fallback_r_m2_k_w is typically
            // from HPXML AssemblyEffectiveRValue which includes film resistances,
            // but we add them explicitly for consistency with the other paths
            // (the RC graph also doesn't add films separately here).
            let r_total =
                bd.fallback_r_m2_k_w + bd.r_film_interior_m2_k_w + bd.r_film_exterior_m2_k_w;
            debug_assert!(
                r_total > 0.0,
                "boundary {bd_idx}: fallback R_total must be > 0"
            );
            let r_ohm = r_total.max(1e-6) / bd.area_m2;
            graph.add_resistance(interior_node, exterior_node, r_ohm);
            boundary_diagnostics.push(BoundaryDiagnostic {
                boundary_idx: bd_idx,
                ua_w_per_k: bd.area_m2 / r_total.max(1e-6),
                r_total_m2_k_w: r_total,
                capacitance_j_k: 0.0,
                n_rc_nodes: 0,
                interior_zone_idx: bd.interior_zone_idx,
                exterior_target: bd.exterior,
                area_m2: bd.area_m2,
                r_film_int_m2_k_w: bd.r_film_interior_m2_k_w,
                r_film_ext_m2_k_w: bd.r_film_exterior_m2_k_w,
                r_zone_to_inner_m2_k_w: None,
                path: RCPath::FallbackR,
                inner_node: None,
            });
        }
    }

    // Build set of connected nodes for O(1) membership checks.
    let connected: HashSet<NodeId> = graph
        .resistances
        .keys()
        .flat_map(|&(a, b)| [a, b])
        .collect();

    // Ensure every zone air node participates in at least one resistance.
    let mut fallback_ua = 0.0_f64;
    for zone_idx in 0..n_zones {
        let zone_node = NodeId((zone_idx + 1) as u32);
        if !connected.contains(&zone_node) {
            let fallback_r = DEFAULT_R_M2_K_W * 100.0;
            graph.add_resistance(zone_node, outdoor_node, fallback_r);
            outdoor_connected = true;
            fallback_ua += 1.0 / fallback_r;
        }
    }

    // If nothing connected to outdoor/ground, derive UAs as fallback.
    if !outdoor_connected && !ground_connected {
        let zone_uas = derive_zone_uas(boundaries, n_zones);
        for (i, &ua) in zone_uas.iter().enumerate() {
            let zone_node = NodeId((i + 1) as u32);
            let r = 1.0 / ua.max(1e-6);
            graph.add_resistance(zone_node, outdoor_node, r);
            fallback_ua += ua;
        }
        outdoor_connected = true;
    }

    // Build external nodes list — at most 2 entries, already sorted by ID.
    let mut external_nodes = Vec::with_capacity(2);
    if outdoor_connected {
        external_nodes.push(outdoor_node);
    }
    if ground_connected {
        external_nodes.push(ground_node);
    }
    // OUTDOOR_NODE_ID < GROUND_NODE_ID, so already sorted.

    let (capacitances, resistances) = graph.into_elements();

    let rc = RCNetwork::from_elements(capacitances, resistances, external_nodes)
        .map_err(|err| format!("RC network build failed: {err}"))?;

    // Look up outdoor column by node ID in the sorted external_nodes list.
    let outdoor_col = rc.external_nodes.iter().position(|&n| n == outdoor_node);
    let ground_col = rc.external_nodes.iter().position(|&n| n == ground_node);

    let (a_c, b_ext) = rc
        .build_matrices()
        .map_err(|err| format!("RC matrix assembly failed: {err}"))?;

    // Sorted internal node order (matches build_matrices row ordering).
    let mut internal_node_order: Vec<NodeId> = rc.capacitances.keys().copied().collect();
    internal_node_order.sort_unstable();

    // Precomputed NodeId → row index map.
    let node_index: HashMap<NodeId, usize> = internal_node_order
        .iter()
        .enumerate()
        .map(|(idx, &nid)| (nid, idx))
        .collect();

    // Zone air node → state-vector row.
    let zone_state_rows: Vec<usize> = (0..n_zones)
        .map(|zone_idx| {
            let node = NodeId((zone_idx + 1) as u32);
            *node_index.get(&node).unwrap_or_else(|| {
                panic!(
                    "zone air node {:?} missing from RC network internal nodes",
                    node
                )
            })
        })
        .collect();

    let n_ext = b_ext.ncols();

    let node_capacitances = rc.capacitances.clone();

    let boundary_ua: f64 = boundary_diagnostics.iter().map(|d| d.ua_w_per_k).sum();
    let diagnostics = EnvelopeDiagnostics {
        boundaries: boundary_diagnostics,
        zone_capacitances_j_k: zone_capacitances.to_vec(),
        total_ua_w_per_k: boundary_ua + fallback_ua,
    };

    Ok((
        BuildingRC {
            a_c,
            b_ext,
            internal_node_order,
            node_index,
            zone_state_rows,
            layer_info,
            outdoor_col,
            ground_col,
            n_ext,
            node_capacitances,
        },
        diagnostics,
    ))
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Mutable graph-building state shared across boundary construction calls.
struct RcGraphState {
    next_layer_id: u32,
    capacitances: HashMap<NodeId, f64>,
    resistances: HashMap<(NodeId, NodeId), f64>,
}

impl RcGraphState {
    fn with_capacity(initial_capacitances: HashMap<NodeId, f64>, est_edges: usize) -> Self {
        Self {
            next_layer_id: LAYER_NODE_BASE,
            capacitances: initial_capacitances,
            resistances: HashMap::with_capacity(est_edges),
        }
    }

    /// Add a resistance edge, combining in parallel if one already exists.
    fn add_resistance(&mut self, a: NodeId, b: NodeId, r: f64) {
        let r = r.max(1e-6);
        let edge = if a <= b { (a, b) } else { (b, a) };
        if let Some(existing) = self.resistances.get_mut(&edge) {
            *existing = parallel_resistance(*existing, r);
        } else {
            self.resistances.insert(edge, r);
        }
    }

    /// Allocate a new layer node with the given capacitance.
    fn alloc_node(&mut self, capacitance: f64) -> NodeId {
        let node = NodeId(self.next_layer_id);
        self.next_layer_id += 1;
        self.capacitances.insert(node, capacitance);
        node
    }

    /// Consume the builder, returning capacitances and resistances.
    fn into_elements(self) -> (HashMap<NodeId, f64>, HashMap<(NodeId, NodeId), f64>) {
        (self.capacitances, self.resistances)
    }

    /// Build RC nodes and resistances for a boundary with valid material layers.
    ///
    /// When `same_zone` is true (adjacent/party wall), keep only the inner half
    /// of layers as a dead-end "fin" of thermal mass (matches OCHRE's halving).
    /// For odd layer counts, the middle layer is kept with halved capacitance.
    /// Returns `None` if no capacitor nodes remain, or `Some((inner, outer))`
    /// where inner is closest to zone air and outer is closest to exterior.
    fn build_layered_boundary(
        &mut self,
        layers: &[&LayerInput],
        params: &BoundaryParams,
    ) -> Option<(NodeId, NodeId)> {
        // Split thick dense layers into sub-layers for Fourier stability.
        let split_layers: Vec<LayerInput> = layers
            .iter()
            .flat_map(|layer| {
                let needs_split = layer.density_kg_m3 > SPLIT_MIN_DENSITY
                    && layer.conductivity_w_m_k > SPLIT_MIN_CONDUCTIVITY;
                let n = if needs_split {
                    split_layer_count(
                        layer.thickness_m,
                        layer.conductivity_w_m_k,
                        layer.density_kg_m3,
                        layer.specific_heat_j_kg_k,
                        DEFAULT_DT_S,
                    )
                } else {
                    1
                };
                (0..n).map(move |_| LayerInput {
                    thickness_m: layer.thickness_m / n as f64,
                    conductivity_w_m_k: layer.conductivity_w_m_k,
                    density_kg_m3: layer.density_kg_m3,
                    specific_heat_j_kg_k: layer.specific_heat_j_kg_k,
                    area_m2: layer.area_m2,
                })
            })
            .collect();

        let effective_refs: Vec<&LayerInput> = split_layers.iter().collect();
        let mut effective_layers = effective_refs;
        let mut halve_last_cap = false;

        if params.same_zone {
            let n = effective_layers.len();
            let even = n.is_multiple_of(2);
            let keep = if even { n / 2 } else { n / 2 + 1 };
            halve_last_cap = !even;
            let start = n - keep;
            effective_layers = effective_layers[start..].to_vec();
            if effective_layers.is_empty() {
                return None;
            }
        }

        let n_layers = effective_layers.len();
        let mut layer_nodes: Vec<NodeId> = Vec::with_capacity(n_layers);

        for (i, layer) in effective_layers.iter().enumerate() {
            let layer_area = layer.effective_area(params.boundary_area);
            let raw_cap =
                layer.density_kg_m3 * layer.specific_heat_j_kg_k * layer.thickness_m * layer_area;
            let halved = if halve_last_cap && i == 0 {
                raw_cap / 2.0
            } else {
                raw_cap
            };
            let cap = halved.max(MIN_CAPACITANCE_J_K);
            layer_nodes.push(self.alloc_node(cap));
        }

        // Outermost layer (exterior-facing) → exterior node (film R + half-layer R).
        // Film R folded into the edge, matching OCHRE's Boundary.__init__ which
        // prepends/appends film R to the resistance list.
        let ff = params.framing_factor;
        let outer = effective_layers[0];
        let outer_area = outer.effective_area(params.boundary_area);
        let k_outer = parallel_path_conductivity(outer.conductivity_w_m_k, ff);
        let r_ext =
            params.r_film_exterior / outer_area + outer.thickness_m / (2.0 * k_outer * outer_area);
        self.add_resistance(params.exterior_node, layer_nodes[0], r_ext);

        // Adjacent layer connections.
        for i in 0..(n_layers - 1) {
            let li = effective_layers[i];
            let lj = effective_layers[i + 1];
            let ai = li.effective_area(params.boundary_area);
            let aj = lj.effective_area(params.boundary_area);
            let ki = parallel_path_conductivity(li.conductivity_w_m_k, ff);
            let kj = parallel_path_conductivity(lj.conductivity_w_m_k, ff);
            let r = li.thickness_m / (2.0 * ki * ai) + lj.thickness_m / (2.0 * kj * aj);
            self.add_resistance(layer_nodes[i], layer_nodes[i + 1], r);
        }

        // Innermost layer (interior-facing) → interior zone (film R + half-layer R).
        // Skip for same-zone dead-end fin (no separate exterior node to connect to).
        if !params.same_zone {
            let inner = effective_layers[n_layers - 1];
            let inner_area = inner.effective_area(params.boundary_area);
            let k_inner = parallel_path_conductivity(inner.conductivity_w_m_k, ff);
            let r_int = params.r_film_interior / inner_area
                + inner.thickness_m / (2.0 * k_inner * inner_area);
            self.add_resistance(layer_nodes[n_layers - 1], params.interior_node, r_int);
        }

        Some((layer_nodes[n_layers - 1], layer_nodes[0]))
    }

    /// Build RC nodes from pre-computed OCHRE layer data.
    ///
    /// Implements OCHRE's `create_rc_data` algorithm:
    /// 1. If same-zone boundary, cut in half
    /// 2. Split resistances: pad with 0 at start/end, average adjacent pairs
    /// 3. Remove zero-capacitance layers by merging R into next resistor
    /// 4. Scale to absolute values using boundary area
    /// 5. Add film resistances, create nodes, wire resistances
    ///
    /// Returns `None` if no capacitor nodes remain, or `Some((inner, outer))`
    /// where inner is closest to zone air and outer is closest to exterior.
    fn build_precomputed_boundary(
        &mut self,
        layers: &[PrecomputedRCLayer],
        params: &BoundaryParams,
    ) -> Option<(NodeId, NodeId)> {
        if layers.is_empty() {
            return None;
        }

        let mut cap_list: Vec<f64> = layers.iter().map(|l| l.capacitance_kj_m2_k).collect();
        let mut res_list: Vec<f64> = layers.iter().map(|l| l.resistance_m2_k_w).collect();
        let mut nodes = cap_list.len();

        // Step 1: same-zone boundaries — cut in half, keeping the interior (last) half
        if params.same_zone {
            let new_nodes = nodes / 2;
            if nodes.is_multiple_of(2) {
                let start = nodes - new_nodes;
                cap_list = cap_list.split_off(start);
                res_list = res_list.split_off(start);
            } else {
                let keep = new_nodes + 1;
                let start = nodes - keep;
                cap_list = cap_list.split_off(start);
                res_list = res_list.split_off(start);
                cap_list[0] /= 2.0;
            }
            nodes = cap_list.len();
        }

        if nodes == 0 {
            return None;
        }

        // Step 2: split resistances — pad [0, r0, ..., rN, 0], average adjacent pairs → N+1 resistors
        let mut padded = Vec::with_capacity(nodes + 2);
        padded.push(0.0);
        padded.extend_from_slice(&res_list);
        padded.push(0.0);
        res_list = (0..=nodes)
            .map(|i| (padded[i] + padded[i + 1]) / 2.0)
            .collect();

        // Step 3: remove zero-capacitance nodes by merging R into next resistor
        {
            let mut i = 0;
            while i < cap_list.len() {
                if cap_list[i] == 0.0 {
                    cap_list.remove(i);
                    let r_to_move = res_list.remove(i);
                    if i < res_list.len() {
                        res_list[i] += r_to_move;
                    }
                    nodes -= 1;
                } else {
                    i += 1;
                }
            }
        }

        // Step 4: remove first resistor if same zones (dead-end exterior side)
        if params.same_zone && !res_list.is_empty() {
            res_list.remove(0);
        }

        if nodes == 0 {
            let total_r: f64 = res_list.iter().sum();
            let r_abs = total_r.max(1e-6) / params.boundary_area;
            self.add_resistance(params.interior_node, params.exterior_node, r_abs);
            return None;
        }

        // Step 5: scale to absolute values
        let cap_abs: Vec<f64> = cap_list
            .iter()
            .map(|c| (c * 1000.0 * params.boundary_area).max(MIN_CAPACITANCE_J_K))
            .collect();
        let mut res_abs: Vec<f64> = res_list
            .iter()
            .map(|r| (r / params.boundary_area).max(1e-6))
            .collect();

        // Step 6: fold film resistances into first/last layer resistors.
        // Exterior film folds into the first resistor (exterior-facing edge).
        // Interior film folds into the last resistor (interior-facing edge).
        if !params.same_zone && !res_abs.is_empty() {
            res_abs[0] += params.r_film_exterior / params.boundary_area;
        }
        if !res_abs.is_empty() {
            let last = res_abs.len() - 1;
            res_abs[last] += params.r_film_interior / params.boundary_area;
        }

        // Step 7: create nodes and wire
        let n_caps = cap_abs.len();
        let mut layer_nodes: Vec<NodeId> = Vec::with_capacity(n_caps);
        for &cap in &cap_abs {
            layer_nodes.push(self.alloc_node(cap));
        }

        // Wire: exterior_node --R[0]--> layer[0] --R[1]--> ... --R[n]--> interior_node
        // For same-zone, exterior wiring is skipped and res_abs has n_caps entries
        // (one fewer than non-same-zone). In that case res_abs[0..n_caps-1] are
        // inter-layer resistors and res_abs[n_caps-1] connects to interior_node.
        let res_offset = if params.same_zone { 0 } else { 1 };
        if !params.same_zone && !res_abs.is_empty() {
            self.add_resistance(params.exterior_node, layer_nodes[0], res_abs[0]);
        }
        for i in 0..(n_caps - 1) {
            let r_idx = i + res_offset;
            if r_idx < res_abs.len() {
                self.add_resistance(layer_nodes[i], layer_nodes[i + 1], res_abs[r_idx]);
            }
        }
        let last_r_idx = n_caps - 1 + res_offset;
        if last_r_idx < res_abs.len() {
            self.add_resistance(
                layer_nodes[n_caps - 1],
                params.interior_node,
                res_abs[last_r_idx],
            );
        }

        Some((layer_nodes[n_caps - 1], layer_nodes[0]))
    }
}

/// Immutable parameters for a single boundary build call.
struct BoundaryParams {
    boundary_area: f64,
    interior_node: NodeId,
    exterior_node: NodeId,
    same_zone: bool,
    r_film_interior: f64,
    r_film_exterior: f64,
    framing_factor: Option<f64>,
}

/// Softwood thermal conductivity [W/(m·K)] for framing studs.
/// ASHRAE Handbook of Fundamentals, Table 1, Chapter 26.
const SOFTWOOD_CONDUCTIVITY_W_M_K: f64 = 0.144;

/// Compute effective conductivity using ASHRAE parallel-path method.
///
/// `k_eff = ff * k_wood + (1 - ff) * k_cavity`
///
/// This is the area-weighted conductivity for a layer with fraction `ff` of wood studs
/// and `(1 - ff)` of insulation cavity. Reference: ASHRAE Handbook of Fundamentals Ch. 27.3.
pub fn parallel_path_conductivity(k_cavity_w_m_k: f64, framing_factor: Option<f64>) -> f64 {
    match framing_factor {
        Some(ff) if ff > 0.0 && ff < 1.0 => {
            ff * SOFTWOOD_CONDUCTIVITY_W_M_K + (1.0 - ff) * k_cavity_w_m_k
        }
        _ => k_cavity_w_m_k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_layer(
        thickness: f64,
        conductivity: f64,
        density: f64,
        cp: f64,
        area: f64,
    ) -> LayerInput {
        LayerInput {
            thickness_m: thickness,
            conductivity_w_m_k: conductivity,
            density_kg_m3: density,
            specific_heat_j_kg_k: cp,
            area_m2: area,
        }
    }

    fn make_boundary(
        area: f64,
        interior_zone_idx: usize,
        exterior: ExteriorTarget,
        layers: Vec<LayerInput>,
        fallback_r: f64,
    ) -> BoundaryInput {
        BoundaryInput {
            area_m2: area,
            interior_zone_idx,
            exterior,
            material_layers: layers,
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: fallback_r,
            r_film_interior_m2_k_w: R_FILM_INTERIOR_M2_K_W,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
        }
    }

    // ── derive_zone_capacitances ────────────────────────────────────────

    #[test]
    fn zone_capacitance_with_known_area() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        assert_eq!(caps.len(), 1);
        let expected = AIR_DENSITY_KG_M3
            * AIR_CP_J_KG_K
            * (100.0 * DEFAULT_HEIGHT_M)
            * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_prefers_explicit_volume() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: Some(300.0),
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Should use 300 m³ (explicit), not 100 × 2.5 = 250 m³ (derived from area)
        let expected = AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * 300.0 * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_defaults_when_area_unknown() {
        let zones = vec![ZoneInput {
            floor_area_m2: None,
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let expected =
            AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * DEFAULT_VOLUME_M3 * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_respects_minimum() {
        // Tiny area → capacitance should be clamped to MIN_CAPACITANCE_J_K.
        let zones = vec![ZoneInput {
            floor_area_m2: Some(1e-12),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        assert!((caps[0] - MIN_CAPACITANCE_J_K).abs() < 1e-6);
    }

    // ── Single zone, single boundary (no layers) ───────────────────────

    #[test]
    fn single_zone_no_layers_produces_valid_network() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let (rc, diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        assert_eq!(rc.a_c.nrows(), 1);
        assert_eq!(rc.a_c.ncols(), 1);
        assert_eq!(rc.zone_state_rows, vec![0]);
        assert_eq!(rc.outdoor_col, Some(0));
        assert_eq!(rc.n_ext, 1);
        assert!(rc.layer_info.is_empty());

        // Diagnostics: single fallback boundary.
        assert_eq!(diag.boundaries.len(), 1);
        assert_eq!(diag.boundaries[0].path, RCPath::FallbackR);
        assert_eq!(diag.boundaries[0].n_rc_nodes, 0);
        assert!(diag.total_ua_w_per_k > 0.0);
    }

    // ── Single zone with material layers ────────────────────────────────

    #[test]
    fn single_zone_with_layers_creates_layer_nodes() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 3 layer nodes (layer0 splits into 2, layer1 stays 1) = 4 states.
        assert_eq!(rc.a_c.nrows(), 4);

        // Diagnostics: single material-layer boundary with 3 RC nodes after splitting.
        assert_eq!(diag.boundaries.len(), 1);
        assert_eq!(diag.boundaries[0].path, RCPath::MaterialLayer);
        assert_eq!(diag.boundaries[0].n_rc_nodes, 3);
        assert!(diag.boundaries[0].capacitance_j_k > 0.0);
        assert_eq!(rc.a_c.ncols(), 4);
        // Layer info present for boundary 0.
        assert!(rc.layer_info.contains_key(&0));
        let info = &rc.layer_info[&0];
        assert_eq!(info.interior_zone_idx, 0);
        // Outer node should be in the node_index.
        assert!(rc.node_index.contains_key(&info.outer_node));
    }

    // ── Multi-zone with outdoor and ground ──────────────────────────────

    #[test]
    fn multi_zone_outdoor_and_ground() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5),
            make_boundary(30.0, 1, ExteriorTarget::Ground, vec![], 3.0),
        ];
        let (rc, _diag) = assemble_building_rc(&boundaries, 2, &caps).unwrap();

        assert_eq!(rc.zone_state_rows.len(), 2);
        assert_eq!(rc.n_ext, 2); // outdoor + ground
        assert_eq!(rc.outdoor_col, Some(0)); // outdoor sorted first
    }

    // ── Ground-only produces no outdoor column ──────────────────────────

    #[test]
    fn ground_only_has_no_outdoor_col() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Only a slab boundary connecting zone 0 to ground.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Ground, vec![], 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // Ground is the only external node; outdoor_col should be None.
        assert_eq!(rc.outdoor_col, None);
        assert_eq!(rc.n_ext, 1);
    }

    // ── Same-zone boundary without layers is a no-op ───────────────────

    #[test]
    fn same_zone_no_layers_is_noop() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Same-zone boundary with no material layers — no thermal mass to model.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), vec![], 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // Only the zone air node; pure-resistance self-loop is skipped.
        assert_eq!(rc.a_c.nrows(), 1);
    }

    // ── Same-zone boundary with layers becomes internal mass ─────────

    #[test]
    fn same_zone_with_layers_creates_internal_mass() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
        ];
        // Same-zone boundary with 4 layers → after splitting: 1+2+1+2=6 sub-layers → halved to 3 internal mass nodes.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone air node + 3 layer nodes (inner half of 6 split sub-layers).
        assert_eq!(rc.a_c.nrows(), 4);
    }

    // ── Same-zone boundary with odd layers halves middle cap ──────────

    #[test]
    fn same_zone_odd_layers_halves_middle_capacitance() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Use low-density materials (density=50 < SPLIT_MIN_DENSITY=100) to avoid auto-splitting,
        // so we can test the same-zone halving logic directly.
        // layers are exterior→interior: [thin, thick-middle, thin]
        // With 3 layers, keep last 2 (n/2+1=2): [thick-middle, interior-thin].
        // The cut-point (thick-middle) is layer[0] of the kept slice → halved cap.
        // thick-middle: density=50, cp=900, thickness=0.10, area=50 → full cap = 225 J/K
        let layers = vec![
            make_layer(0.05, 0.5, 50.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 50.0, 900.0, 0.0),
            make_layer(0.05, 0.5, 50.0, 800.0, 0.0),
        ];
        // 3 layers (no splitting) → keep last 2 (n/2+1), with middle cap halved.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        assert_eq!(rc.a_c.nrows(), 3);

        // Verify middle layer (first kept, NodeId 1000) has halved capacitance.
        let middle_cap = rc.node_capacitances[&NodeId(LAYER_NODE_BASE)];
        let expected = 50.0 * 900.0 * 0.10 * 50.0 / 2.0; // 112.5 J/K
        assert!(
            (middle_cap - expected).abs() < 1e-6,
            "middle cap={middle_cap}, expected {expected} (halved)"
        );
    }

    #[test]
    fn same_zone_single_layer_keeps_one_node_with_halved_cap() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Use low-density material (density=50 < SPLIT_MIN_DENSITY=100) to avoid auto-splitting.
        // density=50, cp=900, thickness=0.10, area=50 → full cap = 225 J/K
        let layers = vec![make_layer(0.10, 1.0, 50.0, 900.0, 0.0)];
        // 1 layer (no splitting) → keep 1 (n/2+1=1), with halved capacitance.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        assert_eq!(rc.a_c.nrows(), 2);

        // Verify layer node (NodeId 1000) has halved capacitance.
        let layer_cap = rc.node_capacitances[&NodeId(LAYER_NODE_BASE)];
        let expected = 50.0 * 900.0 * 0.10 * 50.0 / 2.0; // 112.5 J/K
        assert!(
            (layer_cap - expected).abs() < 1e-6,
            "layer cap={layer_cap}, expected {expected} (halved)"
        );
    }

    // ── Layer node IDs don't collide with external nodes ────────────────

    #[test]
    fn many_boundaries_no_node_id_collision() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);

        // 20 boundaries, each with 3 layers. After auto-splitting (C=3, dt=3600):
        // layer0 (0.05m, k=0.5, ρ=1000, cp=800): α=6.25e-7, dx=0.082 → 1
        // layer1 (0.10m, k=1.0, ρ=2000, cp=900): α=5.56e-7, dx=0.077 → 2
        // layer2 (0.02m, k=0.3, ρ=800,  cp=700): α=5.36e-7, dx=0.076 → 1
        // = 4 sub-layers per boundary = 80 layer nodes total.
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.1, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.02, 0.3, 800.0, 700.0, 0.0),
        ];
        let boundaries: Vec<BoundaryInput> = (0..20)
            .map(|_| make_boundary(10.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5))
            .collect();

        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone + 80 layer nodes = 81 internal nodes.
        assert_eq!(rc.a_c.nrows(), 81);
        // All node IDs should be distinct from OUTDOOR_NODE_ID and GROUND_NODE_ID.
        for &nid in rc.node_index.keys() {
            assert_ne!(nid, NodeId(OUTDOOR_NODE_ID));
            assert_ne!(nid, NodeId(GROUND_NODE_ID));
        }
    }

    // ── Disconnected zone gets fallback to outdoor ──────────────────────

    #[test]
    fn disconnected_zone_gets_fallback() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
            ZoneInput {
                floor_area_m2: Some(50.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        // Only zone 0 has a boundary; zone 1 is disconnected.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 2, &caps).unwrap();

        assert_eq!(rc.zone_state_rows.len(), 2);
        // Both zones should appear in the network (zone 1 via fallback).
        assert!(rc.node_index.contains_key(&NodeId(1)));
        assert!(rc.node_index.contains_key(&NodeId(2)));
    }

    // ── No boundaries at all triggers UA fallback ───────────────────────

    #[test]
    fn no_boundaries_triggers_ua_fallback() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let (rc, _diag) = assemble_building_rc(&[], 1, &caps).unwrap();

        assert_eq!(rc.a_c.nrows(), 1);
        assert_eq!(rc.outdoor_col, Some(0));
    }

    // ── Zone-to-zone boundary ───────────────────────────────────────────

    #[test]
    fn zone_to_zone_boundary_no_external_node() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        // Zone 0 ↔ Zone 1 internal boundary (no outdoor/ground).
        let boundaries = vec![make_boundary(30.0, 0, ExteriorTarget::Zone(1), vec![], 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 2, &caps).unwrap();

        // Should still succeed (zones get fallback to outdoor since
        // !outdoor_connected && !ground_connected triggers UA fallback).
        assert_eq!(rc.zone_state_rows.len(), 2);
        assert!(rc.outdoor_col.is_some());
    }

    // ── SurfaceLayerInfo carries correct zone index ─────────────────────

    #[test]
    fn surface_layer_info_has_correct_zone_idx() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 0.0)];
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5),
            make_boundary(40.0, 1, ExteriorTarget::Outdoor, layers, 2.5),
        ];
        let (rc, _diag) = assemble_building_rc(&boundaries, 2, &caps).unwrap();

        assert_eq!(rc.layer_info[&0].interior_zone_idx, 0);
        assert_eq!(rc.layer_info[&1].interior_zone_idx, 1);
    }

    // ── Matrix symmetry: A_c diagonal should be negative ────────────────

    #[test]
    fn a_c_diagonal_is_negative() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 50.0)];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        for i in 0..rc.a_c.nrows() {
            assert!(
                rc.a_c[(i, i)] < 0.0,
                "A_c diagonal at ({i},{i}) should be negative, got {}",
                rc.a_c[(i, i)]
            );
        }
    }

    // ── Zero-area boundary is skipped ───────────────────────────────────

    #[test]
    fn zero_area_boundary_is_skipped() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![make_boundary(0.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        // Zone gets fallback; zero-area boundary is ignored.
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        assert_eq!(rc.a_c.nrows(), 1);
    }

    // ── Precomputed RC path ─────────────────────────────────────────────

    fn make_precomputed_boundary(
        area: f64,
        interior_zone_idx: usize,
        exterior: ExteriorTarget,
        precomputed: Vec<PrecomputedRCLayer>,
        fallback_r: f64,
    ) -> BoundaryInput {
        BoundaryInput {
            area_m2: area,
            interior_zone_idx,
            exterior,
            material_layers: Vec::new(),
            precomputed_rc: precomputed,
            fallback_r_m2_k_w: fallback_r,
            r_film_interior_m2_k_w: R_FILM_INTERIOR_M2_K_W,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
        }
    }

    #[test]
    fn precomputed_single_layer_creates_one_node() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let precomputed = vec![PrecomputedRCLayer {
            resistance_m2_k_w: 2.0,
            capacitance_kj_m2_k: 50.0,
        }];
        let boundaries = vec![make_precomputed_boundary(
            20.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 1 precomputed layer = 2 states.
        assert_eq!(rc.a_c.nrows(), 2);
        assert!(rc.layer_info.contains_key(&0));
        assert_eq!(rc.layer_info[&0].interior_zone_idx, 0);
        // A_c diagonal should be negative for both nodes.
        for i in 0..rc.a_c.nrows() {
            assert!(rc.a_c[(i, i)] < 0.0);
        }
    }

    #[test]
    fn precomputed_multi_layer_creates_correct_node_count() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.5,
                capacitance_kj_m2_k: 80.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.5,
                capacitance_kj_m2_k: 20.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            25.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 3 precomputed layers = 4 states.
        assert_eq!(rc.a_c.nrows(), 4);
        assert!(rc.layer_info.contains_key(&0));
    }

    #[test]
    fn precomputed_zero_capacitance_layer_is_pruned() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Middle layer has zero capacitance — should be merged out.
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.5,
                capacitance_kj_m2_k: 0.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.5,
                capacitance_kj_m2_k: 20.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            25.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 2 remaining layers (one pruned) = 3 states.
        assert_eq!(rc.a_c.nrows(), 3);
    }

    #[test]
    fn precomputed_outdoor_boundary_keeps_all_layers() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // 4 layers, outdoor exterior → not same-zone, all layers kept.
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            25.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone + 4 layers = 5 states.
        assert_eq!(rc.a_c.nrows(), 5);
    }

    #[test]
    fn precomputed_inter_zone_boundary_keeps_all_layers() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        // Zone 0 ↔ Zone 1: different zones, so same_zone=false, all layers kept.
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            25.0,
            0,
            ExteriorTarget::Zone(1),
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 2, &caps).unwrap();
        // 2 zones + 4 layers = 6 states.
        assert_eq!(rc.a_c.nrows(), 6);
    }

    #[test]
    fn precomputed_all_zero_capacitance_falls_back_to_resistance() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // All layers have zero capacitance → no layer nodes, just a single resistance.
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 2.0,
                capacitance_kj_m2_k: 0.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 0.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            20.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, _diag) = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // No layer nodes created; just zone air node.
        assert_eq!(rc.a_c.nrows(), 1);
        assert!(!rc.layer_info.contains_key(&0));
    }

    #[test]
    fn precomputed_takes_priority_over_material_layers() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Boundary has both material layers AND precomputed — precomputed wins.
        let precomputed = vec![PrecomputedRCLayer {
            resistance_m2_k_w: 2.0,
            capacitance_kj_m2_k: 50.0,
        }];
        let raw_layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 0.0),
        ];
        let bd = BoundaryInput {
            area_m2: 20.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: raw_layers,
            precomputed_rc: precomputed,
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: R_FILM_INTERIOR_M2_K_W,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
        };
        let (rc, _diag) = assemble_building_rc(&[bd], 1, &caps).unwrap();

        // 1 zone + 1 precomputed layer (not 2 raw layers).
        assert_eq!(rc.a_c.nrows(), 2);
    }

    // ── Framing factor parallel-path tests ─────────────────────────

    #[test]
    fn parallel_path_conductivity_no_framing_returns_cavity() {
        let k = parallel_path_conductivity(0.04, None);
        assert!((k - 0.04).abs() < 1e-12);
    }

    #[test]
    fn parallel_path_conductivity_with_framing_increases_k() {
        // ff=0.25, k_cavity=0.04 (R-13 fiberglass), k_wood=0.144
        let k_eff = parallel_path_conductivity(0.04, Some(0.25));
        let expected = 0.25 * SOFTWOOD_CONDUCTIVITY_W_M_K + 0.75 * 0.04;
        assert!((k_eff - expected).abs() < 1e-12);
        assert!(
            k_eff > 0.04,
            "framing should increase effective conductivity"
        );
    }

    #[test]
    fn framing_factor_reduces_wall_effective_r_value() {
        // 2x4 R-13 wall: insulation layer 0.089m thick, k=0.04 W/(m·K)
        // Without framing: R_layer = 0.089 / 0.04 = 2.225 m²·K/W
        // With 25% framing: k_eff = 0.25*0.144 + 0.75*0.04 = 0.066
        //                   R_layer = 0.089 / 0.066 = 1.348 m²·K/W
        // Effective R reduced by ~39%
        let layer = make_layer(0.089, 0.04, 50.0, 840.0, 10.0);
        let caps = vec![
            derive_zone_capacitances(&[ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            }])[0],
        ];

        // Without framing
        let bd_no_ff = BoundaryInput {
            area_m2: 10.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![layer.clone()],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: R_FILM_INTERIOR_M2_K_W,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
        };
        let (rc_no_ff, _) = assemble_building_rc(&[bd_no_ff], 1, &caps).expect("no ff");

        // With 25% framing
        let bd_ff = BoundaryInput {
            area_m2: 10.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![layer],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: R_FILM_INTERIOR_M2_K_W,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: Some(0.25),
        };
        let (rc_ff, _) = assemble_building_rc(&[bd_ff], 1, &caps).expect("with ff");

        // The A matrix diagonal for the zone node should be more negative with framing
        // (higher conductance → faster heat loss → more negative diagonal).
        let zone_diag_no_ff = rc_no_ff.a_c[(0, 0)];
        let zone_diag_ff = rc_ff.a_c[(0, 0)];
        assert!(
            zone_diag_ff < zone_diag_no_ff,
            "framing should increase heat loss (more negative A diagonal): no_ff={zone_diag_no_ff}, ff={zone_diag_ff}"
        );
    }

    // ── Per-zone mass multiplier ────────────────────────────────────────

    #[test]
    fn zone_capacitance_uses_per_zone_multiplier() {
        let conditioned = ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: 7.0,
        };
        let attic = ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: 1.0,
        };
        let foundation = ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: 1.5,
        };
        let caps = derive_zone_capacitances(&[conditioned, attic, foundation]);
        let vol = 100.0 * DEFAULT_HEIGHT_M;
        let base = AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * vol;
        assert!((caps[0] - base * 7.0).abs() < 1e-6, "conditioned: 7x");
        assert!((caps[1] - base * 1.0).abs() < 1e-6, "attic: 1x");
        assert!((caps[2] - base * 1.5).abs() < 1e-6, "foundation: 1.5x");
    }

    #[test]
    fn split_layer_count_concrete_100mm_hourly() {
        // 100mm concrete: k=0.51, rho=1400, cp=1000 → alpha=3.64e-7 m²/s
        // dx_max = sqrt(2 * 3.64e-7 * 3600) ≈ 0.0512m → n = ceil(0.1/0.0512) = 2
        let n = split_layer_count(0.100, 0.51, 1400.0, 1000.0, 3600.0);
        assert!(n >= 2, "100mm concrete at dt=3600s should need >=2 nodes, got {n}");
    }

    #[test]
    fn split_layer_count_thick_concrete_200mm() {
        // 200mm concrete slab should need more splits
        let n = split_layer_count(0.200, 1.13, 1400.0, 1000.0, 3600.0);
        assert!(n >= 3, "200mm concrete at dt=3600s should need >=3 nodes, got {n}");
    }

    #[test]
    fn split_layer_count_insulation_no_split() {
        // Fiberglass insulation: k=0.04, rho=12, cp=840
        // Low density and low conductivity -- should NOT be split even by the function
        // (caller guards on density/conductivity thresholds, but function itself returns 1).
        let n = split_layer_count(0.066, 0.04, 12.0, 840.0, 3600.0);
        assert_eq!(n, 1, "insulation should not be split");
    }

    #[test]
    fn split_layer_count_thin_wood_no_split() {
        // 9mm wood: k=0.14, rho=530, cp=900
        let n = split_layer_count(0.009, 0.14, 530.0, 900.0, 3600.0);
        assert_eq!(n, 1, "thin wood should not need splitting");
    }

    // ── r_zone_to_inner picks correct (last) layer ──────────────────────
    //
    // BESTEST 900FF floor: exterior→interior = [insulation, concrete]
    // r_zone_to_inner = r_film_interior + concrete_thickness / (2 * k_concrete)
    //                 = 0.16 + 0.080 / (2 * 1.130) ≈ 0.195 m²·K/W
    // radiation_frac  = r_film_interior / r_zone_to_inner ≈ 0.82

    #[test]
    fn r_zone_to_inner_uses_innermost_layer() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps = derive_zone_capacitances(&zones);
        // Exterior→interior: insulation first, concrete (interior-facing) last.
        let layers = vec![
            make_layer(1.007, 0.040, 0.0, 0.0, 48.0),  // insulation (exterior)
            make_layer(0.080, 1.130, 1400.0, 1000.0, 48.0), // concrete (interior)
        ];
        let r_film_int = 0.16_f64;
        let bd = BoundaryInput {
            area_m2: 48.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Ground,
            material_layers: layers,
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: r_film_int,
            r_film_exterior_m2_k_w: 0.0,
            framing_factor: None,
        };
        let (_rc, diag) = assemble_building_rc(&[bd], 1, &caps).unwrap();

        let bd_diag = &diag.boundaries[0];
        let r_zone_to_inner = bd_diag
            .r_zone_to_inner_m2_k_w
            .expect("floor boundary should have r_zone_to_inner");

        let expected_r = r_film_int + 0.080 / (2.0 * 1.130);
        assert!(
            (r_zone_to_inner - expected_r).abs() < 1e-4,
            "r_zone_to_inner={r_zone_to_inner:.4}, expected {expected_r:.4}"
        );

        let radiation_frac = r_film_int / r_zone_to_inner;
        let expected_rad_frac = r_film_int / expected_r;
        assert!(
            (radiation_frac - expected_rad_frac).abs() < 1e-4,
            "radiation_frac={radiation_frac:.4}, expected {expected_rad_frac:.4}"
        );
        assert!(
            (radiation_frac - 0.82).abs() < 0.02,
            "radiation_frac={radiation_frac:.4} should be ≈ 0.82"
        );
    }
}
