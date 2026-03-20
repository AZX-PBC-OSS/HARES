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
pub const AIR_DENSITY_KG_M3: f64 = 1.2;
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

/// Where the exterior side of a boundary connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Material layers (interior → exterior order).
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
}

/// Zone input: floor area and volume for capacitance derivation.
#[derive(Debug, Clone)]
pub struct ZoneInput {
    pub floor_area_m2: Option<f64>,
    pub volume_m3: Option<f64>,
}

// ── Output ──────────────────────────────────────────────────────────────────

/// Per-boundary surface metadata for the caller to wire up LWR/solar injection.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceLayerInfo {
    /// NodeId of the outermost material-layer node for this boundary.
    pub outer_node: NodeId,
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
    /// Number of external driving columns in B_ext.
    pub n_ext: usize,
    /// NodeId → thermal capacitance [J/K] for all internal nodes.
    pub node_capacitances: HashMap<NodeId, f64>,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Derive zone air-node capacitances [J/K] from zone floor areas.
pub fn derive_zone_capacitances(zones: &[ZoneInput]) -> Vec<f64> {
    zones
        .iter()
        .map(|z| {
            let volume = z
                .volume_m3
                .or_else(|| z.floor_area_m2.map(|a| a * DEFAULT_HEIGHT_M))
                .unwrap_or(DEFAULT_VOLUME_M3);
            (AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * volume * INTERIOR_MASS_MULTIPLIER)
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
) -> Result<BuildingRC, String> {
    let outdoor_node = NodeId(OUTDOOR_NODE_ID);
    let ground_node = NodeId(GROUND_NODE_ID);

    // Pre-size from boundary data.
    let est_nodes = n_zones
        + boundaries
            .iter()
            .map(|b| b.material_layers.len())
            .sum::<usize>();
    let est_edges = est_nodes + boundaries.len();

    let mut capacitances: HashMap<NodeId, f64> = HashMap::with_capacity(est_nodes);
    let mut resistances: HashMap<(NodeId, NodeId), f64> = HashMap::with_capacity(est_edges);

    // Insert zone air nodes (IDs 1..=n_zones).
    for (zone_idx, cap) in zone_capacitances.iter().enumerate().take(n_zones) {
        let node = NodeId((zone_idx + 1) as u32);
        capacitances.insert(node, cap.max(MIN_CAPACITANCE_J_K));
    }

    let mut layer_info: HashMap<usize, SurfaceLayerInfo> = HashMap::new();
    let mut outdoor_connected = false;
    let mut ground_connected = false;
    // Monotonically increasing counter for layer node IDs, starting above zone range.
    let mut next_layer_id: u32 = LAYER_NODE_BASE;

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

        // Precomputed RC path (OCHRE LUT) takes priority over raw material layers.
        if !bd.precomputed_rc.is_empty() {
            let outer_node = build_precomputed_boundary(
                &bd.precomputed_rc,
                bd.area_m2,
                interior_node,
                exterior_node,
                same_zone,
                &mut next_layer_id,
                &mut capacitances,
                &mut resistances,
                bd.r_film_interior_m2_k_w,
                bd.r_film_exterior_m2_k_w,
            );
            if let Some(outer) = outer_node {
                layer_info.insert(
                    bd_idx,
                    SurfaceLayerInfo {
                        outer_node: outer,
                        interior_zone_idx: bd.interior_zone_idx,
                    },
                );
            }
            continue;
        }

        let valid_layers: Vec<&LayerInput> = bd
            .material_layers
            .iter()
            .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
            .collect();

        if !valid_layers.is_empty() {
            let outer_node = build_layered_boundary(
                &valid_layers,
                bd.area_m2,
                interior_node,
                exterior_node,
                same_zone,
                &mut next_layer_id,
                &mut capacitances,
                &mut resistances,
                bd.r_film_interior_m2_k_w,
                bd.r_film_exterior_m2_k_w,
            );
            if let Some(outer) = outer_node {
                layer_info.insert(
                    bd_idx,
                    SurfaceLayerInfo {
                        outer_node: outer,
                        interior_zone_idx: bd.interior_zone_idx,
                    },
                );
            }
        } else if !same_zone {
            // No valid material layers: single-resistance connection.
            // Same-zone pure-resistance boundaries have no thermal mass — skip.
            let r_ohm = bd.fallback_r_m2_k_w.max(1e-6) / bd.area_m2;
            add_resistance(&mut resistances, interior_node, exterior_node, r_ohm);
        }
    }

    // Build set of connected nodes for O(1) membership checks.
    let connected: HashSet<NodeId> = resistances.keys().flat_map(|&(a, b)| [a, b]).collect();

    // Ensure every zone air node participates in at least one resistance.
    for zone_idx in 0..n_zones {
        let zone_node = NodeId((zone_idx + 1) as u32);
        if !connected.contains(&zone_node) {
            let fallback_r = DEFAULT_R_M2_K_W * 100.0;
            add_resistance(&mut resistances, zone_node, outdoor_node, fallback_r);
            outdoor_connected = true;
        }
    }

    // If nothing connected to outdoor/ground, derive UAs as fallback.
    if !outdoor_connected && !ground_connected {
        let zone_uas = derive_zone_uas(boundaries, n_zones);
        for (i, &ua) in zone_uas.iter().enumerate() {
            let zone_node = NodeId((i + 1) as u32);
            let r = 1.0 / ua.max(1e-6);
            add_resistance(&mut resistances, zone_node, outdoor_node, r);
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

    let rc = RCNetwork::from_elements(capacitances, resistances, external_nodes)
        .map_err(|err| format!("RC network build failed: {err}"))?;

    // Look up outdoor column by node ID in the sorted external_nodes list.
    let outdoor_col = rc.external_nodes.iter().position(|&n| n == outdoor_node);

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

    Ok(BuildingRC {
        a_c,
        b_ext,
        internal_node_order,
        node_index,
        zone_state_rows,
        layer_info,
        outdoor_col,
        n_ext,
        node_capacitances,
    })
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Build RC nodes and resistances for a boundary with valid material layers.
///
/// When `same_zone` is true (adjacent/party wall), keep only the inner half
/// of layers as a dead-end "fin" of thermal mass (matches OCHRE's halving).
/// For odd layer counts, the middle layer is kept with halved capacitance.
/// Returns `None` if no capacitor nodes remain.
fn build_layered_boundary(
    layers: &[&LayerInput],
    boundary_area: f64,
    interior_node: NodeId,
    exterior_node: NodeId,
    same_zone: bool,
    next_layer_id: &mut u32,
    capacitances: &mut HashMap<NodeId, f64>,
    resistances: &mut HashMap<(NodeId, NodeId), f64>,
    r_film_interior: f64,
    r_film_exterior: f64,
) -> Option<NodeId> {
    let mut effective_layers: Vec<&LayerInput> = layers.to_vec();
    // Track whether the outermost retained layer needs halved capacitance (odd count).
    let mut halve_last_cap = false;

    // Same-zone: keep only the inner half of layers (matching OCHRE halving).
    // Odd counts keep n/2+1 layers with the middle layer's capacitance halved.
    if same_zone {
        let n = effective_layers.len();
        let keep = if n % 2 == 0 { n / 2 } else { n / 2 + 1 };
        if n % 2 != 0 {
            halve_last_cap = true;
        }
        effective_layers.truncate(keep);
        if effective_layers.is_empty() {
            return None;
        }
    }

    let n_layers = effective_layers.len();
    let mut layer_nodes: Vec<NodeId> = Vec::with_capacity(n_layers);

    for (i, layer) in effective_layers.iter().enumerate() {
        let node = NodeId(*next_layer_id);
        *next_layer_id += 1;
        let layer_area = if layer.area_m2 > 0.0 {
            layer.area_m2
        } else {
            boundary_area
        };
        let mut cap =
            (layer.density_kg_m3 * layer.specific_heat_j_kg_k * layer.thickness_m * layer_area)
                .max(MIN_CAPACITANCE_J_K);
        if halve_last_cap && i == n_layers - 1 {
            cap /= 2.0;
        }
        capacitances.insert(node, cap);
        layer_nodes.push(node);
    }

    // Interior zone → innermost layer.
    let inner = effective_layers[0];
    let inner_area = if inner.area_m2 > 0.0 {
        inner.area_m2
    } else {
        boundary_area
    };
    let r_int = inner.thickness_m / (2.0 * inner.conductivity_w_m_k * inner_area)
        + r_film_interior / inner_area;
    add_resistance(resistances, interior_node, layer_nodes[0], r_int);

    // Adjacent layer connections.
    for i in 0..(n_layers - 1) {
        let li = effective_layers[i];
        let lj = effective_layers[i + 1];
        let ai = if li.area_m2 > 0.0 {
            li.area_m2
        } else {
            boundary_area
        };
        let aj = if lj.area_m2 > 0.0 {
            lj.area_m2
        } else {
            boundary_area
        };
        let r = li.thickness_m / (2.0 * li.conductivity_w_m_k * ai)
            + lj.thickness_m / (2.0 * lj.conductivity_w_m_k * aj);
        add_resistance(resistances, layer_nodes[i], layer_nodes[i + 1], r);
    }

    // Outermost layer → exterior node (skip for same-zone dead-end fin).
    if !same_zone {
        let outer = effective_layers[n_layers - 1];
        let outer_area = if outer.area_m2 > 0.0 {
            outer.area_m2
        } else {
            boundary_area
        };
        let r_ext = outer.thickness_m / (2.0 * outer.conductivity_w_m_k * outer_area)
            + r_film_exterior / outer_area;
        add_resistance(resistances, layer_nodes[n_layers - 1], exterior_node, r_ext);
    }

    Some(layer_nodes[n_layers - 1])
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
/// Returns the outermost layer NodeId, or None if no capacitor nodes remain.
#[allow(clippy::too_many_arguments)]
fn build_precomputed_boundary(
    layers: &[PrecomputedRCLayer],
    boundary_area: f64,
    interior_node: NodeId,
    exterior_node: NodeId,
    same_zone: bool,
    next_layer_id: &mut u32,
    capacitances: &mut HashMap<NodeId, f64>,
    resistances: &mut HashMap<(NodeId, NodeId), f64>,
    r_film_interior: f64,
    r_film_exterior: f64,
) -> Option<NodeId> {
    if layers.is_empty() {
        return None;
    }

    let mut cap_list: Vec<f64> = layers.iter().map(|l| l.capacitance_kj_m2_k).collect();
    let mut res_list: Vec<f64> = layers.iter().map(|l| l.resistance_m2_k_w).collect();
    let mut nodes = cap_list.len();

    // Step 1: same-zone boundaries — cut in half
    if same_zone {
        let new_nodes = nodes / 2;
        if nodes.is_multiple_of(2) {
            cap_list.truncate(new_nodes);
            res_list.truncate(new_nodes);
        } else {
            cap_list[new_nodes] /= 2.0;
            cap_list.truncate(new_nodes + 1);
            res_list.truncate(new_nodes + 1);
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

    // Step 4: remove last resistor if same zones
    if same_zone && !res_list.is_empty() {
        res_list.pop();
    }

    if nodes == 0 {
        let total_r: f64 = res_list.iter().sum();
        let r_abs = total_r.max(1e-6) / boundary_area;
        add_resistance(resistances, interior_node, exterior_node, r_abs);
        return None;
    }

    // Step 5: scale to absolute values
    let cap_abs: Vec<f64> = cap_list
        .iter()
        .map(|c| (c * 1000.0 * boundary_area).max(MIN_CAPACITANCE_J_K))
        .collect();
    let mut res_abs: Vec<f64> = res_list
        .iter()
        .map(|r| (r / boundary_area).max(1e-6))
        .collect();

    // Step 6: add film resistances
    if !res_abs.is_empty() {
        res_abs[0] += r_film_interior / boundary_area;
    }
    if !same_zone && !res_abs.is_empty() {
        let last = res_abs.len() - 1;
        res_abs[last] += r_film_exterior / boundary_area;
    }

    // Step 7: create nodes and wire
    let n_caps = cap_abs.len();
    let mut layer_nodes: Vec<NodeId> = Vec::with_capacity(n_caps);
    for cap in &cap_abs {
        let node = NodeId(*next_layer_id);
        *next_layer_id += 1;
        capacitances.insert(node, *cap);
        layer_nodes.push(node);
    }

    // Wire: interior_node --R[0]--> layer[0] --R[1]--> ... --R[n]--> exterior_node
    if !res_abs.is_empty() {
        add_resistance(resistances, interior_node, layer_nodes[0], res_abs[0]);
    }
    for i in 1..n_caps {
        if i < res_abs.len() {
            add_resistance(resistances, layer_nodes[i - 1], layer_nodes[i], res_abs[i]);
        }
    }
    if !same_zone && res_abs.len() > n_caps {
        add_resistance(
            resistances,
            layer_nodes[n_caps - 1],
            exterior_node,
            res_abs[n_caps],
        );
    }

    Some(layer_nodes[n_caps - 1])
}

/// Add a resistance edge, combining in parallel if one already exists.
fn add_resistance(resistances: &mut HashMap<(NodeId, NodeId), f64>, a: NodeId, b: NodeId, r: f64) {
    let r = r.max(1e-6);
    let edge = if a <= b { (a, b) } else { (b, a) };
    if let Some(existing) = resistances.get_mut(&edge) {
        *existing = parallel_resistance(*existing, r);
    } else {
        resistances.insert(edge, r);
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
        }
    }

    // ── derive_zone_capacitances ────────────────────────────────────────

    #[test]
    fn zone_capacitance_with_known_area() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        }];
        let caps = derive_zone_capacitances(&zones);
        // Should use 300 m³ (explicit), not 100 × 2.5 = 250 m³ (derived from area)
        let expected =
            AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * 300.0 * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_defaults_when_area_unknown() {
        let zones = vec![ZoneInput {
            floor_area_m2: None,
            volume_m3: None,
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
        }];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        assert_eq!(rc.a_c.nrows(), 1);
        assert_eq!(rc.a_c.ncols(), 1);
        assert_eq!(rc.zone_state_rows, vec![0]);
        assert_eq!(rc.outdoor_col, Some(0));
        assert_eq!(rc.n_ext, 1);
        assert!(rc.layer_info.is_empty());
    }

    // ── Single zone with material layers ────────────────────────────────

    #[test]
    fn single_zone_with_layers_creates_layer_nodes() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 2 layer nodes = 3 states.
        assert_eq!(rc.a_c.nrows(), 3);
        assert_eq!(rc.a_c.ncols(), 3);
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
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5),
            make_boundary(30.0, 1, ExteriorTarget::Ground, vec![], 3.0),
        ];
        let rc = assemble_building_rc(&boundaries, 2, &caps).unwrap();

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
        }];
        let caps = derive_zone_capacitances(&zones);
        // Only a slab boundary connecting zone 0 to ground.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Ground, vec![], 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

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
        }];
        let caps = derive_zone_capacitances(&zones);
        // Same-zone boundary with no material layers — no thermal mass to model.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), vec![], 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // Only the zone air node; pure-resistance self-loop is skipped.
        assert_eq!(rc.a_c.nrows(), 1);
    }

    // ── Same-zone boundary with layers becomes internal mass ─────────

    #[test]
    fn same_zone_with_layers_creates_internal_mass() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
        ];
        // Same-zone boundary with 4 layers → halved to 2 internal mass nodes.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone air node + 2 layer nodes (inner half of 4 layers).
        assert_eq!(rc.a_c.nrows(), 3);
    }

    // ── Same-zone boundary with odd layers halves middle cap ──────────

    #[test]
    fn same_zone_odd_layers_halves_middle_capacitance() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
        ];
        // 3 layers → keep 2 (n/2+1), with layer[1]'s cap halved.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone air node + 2 layer nodes.
        assert_eq!(rc.a_c.nrows(), 3);
    }

    #[test]
    fn same_zone_single_layer_keeps_one_node_with_halved_cap() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![make_layer(0.10, 1.0, 2000.0, 900.0, 0.0)];
        // 1 layer → keep 1 (n/2+1=1), with halved capacitance.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone air node + 1 layer node.
        assert_eq!(rc.a_c.nrows(), 2);
    }

    // ── Layer node IDs don't collide with external nodes ────────────────

    #[test]
    fn many_boundaries_no_node_id_collision() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);

        // 20 boundaries, each with 3 layers = 60 layer nodes.
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.1, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.02, 0.3, 800.0, 700.0, 0.0),
        ];
        let boundaries: Vec<BoundaryInput> = (0..20)
            .map(|_| make_boundary(10.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5))
            .collect();

        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone + 60 layer nodes = 61 internal nodes.
        assert_eq!(rc.a_c.nrows(), 61);
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
            },
            ZoneInput {
                floor_area_m2: Some(50.0),
                volume_m3: None,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        // Only zone 0 has a boundary; zone 1 is disconnected.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let rc = assemble_building_rc(&boundaries, 2, &caps).unwrap();

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
        }];
        let caps = derive_zone_capacitances(&zones);
        let rc = assemble_building_rc(&[], 1, &caps).unwrap();

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
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        // Zone 0 ↔ Zone 1 internal boundary (no outdoor/ground).
        let boundaries = vec![make_boundary(30.0, 0, ExteriorTarget::Zone(1), vec![], 2.5)];
        let rc = assemble_building_rc(&boundaries, 2, &caps).unwrap();

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
            },
            ZoneInput {
                floor_area_m2: Some(80.0),
                volume_m3: None,
            },
        ];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 0.0)];
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5),
            make_boundary(40.0, 1, ExteriorTarget::Outdoor, layers, 2.5),
        ];
        let rc = assemble_building_rc(&boundaries, 2, &caps).unwrap();

        assert_eq!(rc.layer_info[&0].interior_zone_idx, 0);
        assert_eq!(rc.layer_info[&1].interior_zone_idx, 1);
    }

    // ── Matrix symmetry: A_c diagonal should be negative ────────────────

    #[test]
    fn a_c_diagonal_is_negative() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
        }];
        let caps = derive_zone_capacitances(&zones);
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 50.0)];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

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
        }];
        let caps = derive_zone_capacitances(&zones);
        let boundaries = vec![make_boundary(0.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        // Zone gets fallback; zero-area boundary is ignored.
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
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
        }
    }

    #[test]
    fn precomputed_single_layer_creates_one_node() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

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
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 3 precomputed layers = 4 states.
        assert_eq!(rc.a_c.nrows(), 4);
        assert!(rc.layer_info.contains_key(&0));
    }

    #[test]
    fn precomputed_zero_capacitance_layer_is_pruned() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // 1 zone air + 2 remaining layers (one pruned) = 3 states.
        assert_eq!(rc.a_c.nrows(), 3);
    }

    #[test]
    fn precomputed_outdoor_boundary_keeps_all_layers() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();
        // 1 zone + 4 layers = 5 states.
        assert_eq!(rc.a_c.nrows(), 5);
    }

    #[test]
    fn precomputed_inter_zone_boundary_keeps_all_layers() {
        let zones = vec![
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
            },
            ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: None,
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
        let rc = assemble_building_rc(&boundaries, 2, &caps).unwrap();
        // 2 zones + 4 layers = 6 states.
        assert_eq!(rc.a_c.nrows(), 6);
    }

    #[test]
    fn precomputed_all_zero_capacitance_falls_back_to_resistance() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        let rc = assemble_building_rc(&boundaries, 1, &caps).unwrap();

        // No layer nodes created; just zone air node.
        assert_eq!(rc.a_c.nrows(), 1);
        assert!(!rc.layer_info.contains_key(&0));
    }

    #[test]
    fn precomputed_takes_priority_over_material_layers() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
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
        };
        let rc = assemble_building_rc(&[bd], 1, &caps).unwrap();

        // 1 zone + 1 precomputed layer (not 2 raw layers).
        assert_eq!(rc.a_c.nrows(), 2);
    }
}
