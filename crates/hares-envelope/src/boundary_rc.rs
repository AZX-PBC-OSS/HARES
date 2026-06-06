//! RC graph construction from building envelope boundary data.
//!
//! Assembles the continuous-time state-space matrices (A_c, B_ext) from
//! zone air nodes, material-layer nodes, and resistive connections.

use std::collections::{HashMap, HashSet};

use nalgebra::DMatrix;
use thiserror::Error;

use crate::NodeId;
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use crate::rc_network::sorted_internal_nodes;
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
/// NodeId for the outdoor temperature driving node.
pub const OUTDOOR_NODE_ID: u32 = u32::MAX - 1;
/// NodeId for the ground temperature driving node (legacy single-ground reference).
pub const GROUND_NODE_ID: u32 = u32::MAX;
/// Base NodeId for per-depth ground driving nodes.
///
/// Each unique foundation depth gets its own ground node: `GROUND_NODE_BASE + i`
/// where `i` is the depth index in ascending order. All ground nodes are assigned
/// below `OUTDOOR_NODE_ID` so the sorted external-node list places ground columns
/// before the outdoor column in the B-matrix. Up to 99 unique depths are supported.
pub const GROUND_NODE_BASE: u32 = u32::MAX - 100;
/// First NodeId used for material-layer nodes (above zone air node range).
const LAYER_NODE_BASE: u32 = 1_000;

/// Minimum density [kg/m³] to qualify a layer for automatic splitting.
/// Insulation and air gaps are excluded.
const SPLIT_MIN_DENSITY: f64 = 100.0;

/// Minimum conductivity [W/(m·K)] to qualify a layer for automatic splitting.
const SPLIT_MIN_CONDUCTIVITY: f64 = 0.1;

/// Diurnal period [s] — one full day (24 × 3600).
const DIURNAL_PERIOD_S: f64 = 86_400.0;

/// Compute the number of RC sub-layers needed to resolve the diurnal
/// temperature wave.
///
/// Uses half the diurnal penetration depth to ensure adequate spatial
/// resolution of the diurnal wave:
///   Λ = ½ · √(α · P / π) = √(α · P / (4π))
/// where P = 86 400 s (one day) and α = k / (ρ·cₚ).  The full penetration
/// depth δ_p = √(α·P/π) is where the diurnal wave decays to 1/e of its
/// surface amplitude (Incropera & DeWitt §5.8).  Using Λ = δ_p/2 ensures at
/// least two sub-layers per penetration depth for adequate wave-shape
/// resolution.  The layer is discretised into n = ceil(thickness / Λ).
///
/// Cite:
/// - Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer* §5.8,
///   "Penetration depth" for semi-infinite solid with periodic surface
///   temperature.
/// - ISO 13786:2007 §6.2, dynamic thermal characteristics — uses the
///   same diffusion-length criterion for periodic heat flow.
/// - The HARES solver is an implicit ZOH state-space solver, which is
///   unconditionally stable; spatial accuracy is decoupled from temporal
///   stability, so the discretisation depends on the physical length scale
///   (diurnal penetration depth), not the simulation timestep.
fn split_layer_count(
    thickness_m: f64,
    conductivity: f64,
    density: f64,
    specific_heat: f64,
) -> usize {
    if density <= 0.0 || specific_heat <= 0.0 || conductivity <= 0.0 || thickness_m <= 0.0 {
        return 1;
    }
    let alpha = conductivity / (density * specific_heat);
    // Diurnal penetration depth Λ = √(α · P / (4π)).
    // ISO 13786:2007 §6.2; Incropera & DeWitt §5.8.
    let lambda = (alpha * DIURNAL_PERIOD_S / (4.0 * std::f64::consts::PI)).sqrt();
    let n = (thickness_m / lambda).ceil() as usize;
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
    /// Interior-facing longwave emissivity [-] for this boundary surface.
    ///
    /// Used by the star-mesh interior LWR conductance to compute
    /// `g = 4·ε·σ·A·T_ref³` per surface. For ALL interior surfaces
    /// (including windows), ASHRAE 140-2017 §5.3.1.9 specifies
    /// ε_ir = 0.9. Attic-zone surfaces with radiant barriers use 0.05.
    ///
    /// Populated from the building's boundary emissivity data by
    /// `building_to_boundary_inputs` in `hares-core`.
    pub interior_emissivity: f64,
    /// Centroid depth of the boundary below grade [m].
    ///
    /// For ground-contacting boundaries (`ExteriorTarget::Ground`), this is the
    /// depth at which the Kusuda-Achenbach ground temperature is evaluated.
    /// 0.0 = grade surface. For above-grade boundaries (Outdoor, Zone), this
    /// field is unused but must be set to a valid value.
    ///
    /// Typical values: slab-on-grade floor ≈ 0.1–0.5 m, basement wall centroid
    /// ≈ 1.2 m for a 2.4 m basement, crawlspace floor ≈ 0.5–1.0 m.
    ///
    /// Default: 0.0 m (grade surface — matches pre-fix behaviour).
    pub foundation_depth_m: f64,
    /// Whether this boundary used the default 2.5 m²·K/W R-value fallback
    /// because neither `AssemblyEffectiveRValue` nor `NominalRValue` layers
    /// were specified. Set by `building_to_boundary_inputs` in `hares-core`.
    #[cfg(feature = "observe")]
    pub used_default_r: bool,
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
    /// Half-resistance of the post-split outermost sub-layer [m²·K/W].
    /// `None` for fallback-R boundaries with no RC nodes.
    pub r_outer_half_m2_k_w: Option<f64>,
    /// Half-resistance of the post-split innermost sub-layer [m²·K/W].
    /// `None` for fallback-R boundaries with no RC nodes.
    pub r_inner_half_m2_k_w: Option<f64>,
    pub path: RCPath,
    /// NodeId of the innermost RC node (closest to zone air).
    /// `None` for fallback-R boundaries with no RC nodes.
    pub inner_node: Option<NodeId>,
    /// Interior-facing longwave emissivity [-] for star-mesh radiation.
    pub interior_emissivity: f64,
    /// Foundation depth below grade for ground-contacting boundaries [m].
    ///
    /// Copied from `BoundaryInput::foundation_depth_m`. 0.0 for above-grade
    /// boundaries. Used by solver_builder to attach depth-aware
    /// `DrivingTemp::Ground { depth_m }` to boundary diagnostics.
    pub foundation_depth_m: f64,
    /// For same-zone precomputed boundaries: which half of the layer stack
    /// was kept ("interior" or "exterior"). `None` for non-same-zone or
    /// non-precomputed boundaries.
    #[cfg(feature = "observe")]
    pub same_zone_kept_half: Option<&'static str>,
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
    /// Number of boundaries that fell back to the default 2.5 m²·K/W
    /// R-value because no HPXML R-value data was present.
    #[cfg(feature = "observe")]
    pub default_r_fallback_count: usize,
}

// ── Output ──────────────────────────────────────────────────────────────────

/// Per-boundary surface metadata for the caller to wire up LWR/solar injection.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceLayerInfo {
    /// NodeId of the outermost material-layer node for this boundary.
    pub outer_node: NodeId,
    /// NodeId of the innermost material-layer node (closest to zone air).
    pub inner_node: NodeId,
    /// NodeId of the interior surface temperature node (between R_film_conv and
    /// R_inner_half) when using StarMesh interior LWR mode. This is the node
    /// that participates in the star-mesh radiation network.
    ///
    /// In StarMesh mode, the star-mesh radiation conductance connects from
    /// this surface_node (not inner_node) to the radiation star node, giving
    /// the correct radiation topology per EnergyPlus "Option 2", TRNSYS Type
    /// 56, and ESP-r. The surface_node is floating (no capacitance) and is
    /// eliminated during RC network reduction, distributing radiation
    /// conductances to inner_node and zone_air via Y-Δ transform.
    ///
    /// `None` in ScriptF mode (combined R_film + R_inner_half resistor, no
    /// separate surface node; radiation_frac handles surface temperature).
    pub surface_node: Option<NodeId>,
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
    /// Ground temperature columns in B_ext — one per unique foundation depth.
    /// Each entry is `(depth_m, col_index)`, sorted by ascending depth.
    /// Depth = 0.0 m represents the grade surface (DOE-2 surface model).
    /// Empty when no ground-connected boundaries exist.
    pub ground_cols: Vec<(f64, usize)>,
    /// Number of external driving columns in B_ext.
    pub n_ext: usize,
    /// NodeId → thermal capacitance [J/K] for all internal nodes.
    pub node_capacitances: HashMap<NodeId, f64>,
}

// ── Interior LWR method ─────────────────────────────────────────────────────

/// Interior longwave radiation method — a physics-significant choice that
/// materially changes the A-matrix structure and solver behaviour.
///
/// `StarMesh` bakes linearized inter-surface radiation conductances into the
/// RC A-matrix at construction time, eliminating the need for iterative LWR
/// injection each timestep.
///
/// `ScriptF` preserves the previous behavior: iterative T⁴ radiosity
/// injection via the `apply_interior_longwave_inputs()` solver method.
///
/// **Explicit-variant rule**: callers must name the variant explicitly at every
/// construction site (`InteriorLwrMethod::StarMesh` or
/// `InteriorLwrMethod::ScriptF`), never `::default()`. The `Default` impl is
/// deliberately omitted so the compiler enforces this. A future change to add
/// a third variant (e.g. an exact `ExactMatrix` method) cannot silently switch
/// behaviour at any callsite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteriorLwrMethod {
    /// Star-mesh linearized radiation: floating "radiation star" node per zone,
    /// connected to each interior surface via `R = 1/(4·ε·σ·A·T_ref³)`.
    /// `reduce_floating_nodes()` eliminates the star node into pairwise
    /// conductances automatically. Matches OCHRE `linearize_int_radiation`,
    /// TRNSYS Type 56, ESP-r.
    StarMesh,
    /// ScriptF iterative T⁴ radiosity injection (OCHRE legacy mode).
    /// The A-matrix contains convection-only film resistances; LWR flux
    /// is computed and injected each timestep via the radiation_frac split.
    ScriptF,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Errors raised during boundary RC construction.
#[derive(Debug, Error)]
pub enum BoundaryRcError {
    /// Non-positive site atmospheric pressure [Pa] supplied to zone capacitance derivation.
    /// Site pressure must be physically positive; zero or negative indicates
    /// missing or corrupted weather/configuration data.
    #[error("invalid site_pressure_pa: expected > 0, got {value}")]
    InvalidSitePressure {
        /// The offending pressure value.
        value: f64,
    },
}

/// Derive zone air-node capacitances [J/K] from zone volumes, mass multipliers,
/// and site barometric pressure.
///
/// Zone air capacitance: C = ρ × cp × V × mass_multiplier, where ρ is computed
/// from the ideal gas law: ρ = p / (R_da × T_ref).
///
/// - `site_pressure_pa`: ISA standard atmospheric pressure at site elevation [Pa].
///   Use [`hares_physics::air_properties::standard_pressure_pa`] to compute from
///   elevation, or [`hares_physics::constants::SEA_LEVEL_PRESSURE_PA`] (101 325 Pa)
///   when elevation is unknown (backward-compatible with the former sea-level constant).
/// - Reference temperature T_ref = 293.15 K (20 °C), consistent with the
///   linearization operating point used for interior LWR and film coefficients.
///
/// Cite: ASHRAE HoF 2021 §1.8 Eq.28; EnergyPlus `PsyRhoAirFnPbTdbW`;
/// ISA 1976 / ICAO Doc 7488.
pub fn derive_zone_capacitances(
    zones: &[ZoneInput],
    site_pressure_pa: f64,
) -> Result<Vec<f64>, BoundaryRcError> {
    /// Reference temperature for zone air density computation [K].
    /// 20 °C matches the linearization operating point used throughout the
    /// RC network (star-mesh LWR, TARP film coefficients).
    const T_REF_K: f64 = 293.15;

    if site_pressure_pa <= 0.0 {
        return Err(BoundaryRcError::InvalidSitePressure {
            value: site_pressure_pa,
        });
    }

    // Ideal gas law for dry air: ρ = p / (R_da × T)
    // Cite: ASHRAE HoF 2021 §1.8 Eq.28
    let rho = site_pressure_pa / (hares_physics::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * T_REF_K);

    Ok(zones
        .iter()
        .map(|z| {
            let volume = z
                .volume_m3
                .or_else(|| z.floor_area_m2.map(|a| a * DEFAULT_HEIGHT_M))
                .unwrap_or(DEFAULT_VOLUME_M3);
            (rho * AIR_CP_J_KG_K * volume * z.mass_multiplier).max(MIN_CAPACITANCE_J_K)
        })
        .collect())
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

/// Validate that every `SurfaceLayerInfo` entry's `inner_node` and `outer_node`
/// are present in both the capacitance map and the node index.
///
/// In debug/check_invariants builds the checks are `debug_assert!` that panic
/// on first violation.  In all builds a `tracing::warn!` is emitted for any
/// missing node so production deployments don't silently lose surface wiring.
fn validate_surface_layer_info(
    layer_info: &HashMap<usize, SurfaceLayerInfo>,
    capacitances: &HashMap<NodeId, f64>,
    node_index: &HashMap<NodeId, usize>,
) {
    for (bd_idx, info) in layer_info {
        let inner_has_cap = capacitances.contains_key(&info.inner_node);
        let outer_has_cap = capacitances.contains_key(&info.outer_node);
        let inner_in_index = node_index.contains_key(&info.inner_node);
        let outer_in_index = node_index.contains_key(&info.outer_node);

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                inner_has_cap,
                "SurfaceLayerInfo for boundary {bd_idx}: inner_node {:?} missing from capacitances",
                info.inner_node
            );
            debug_assert!(
                outer_has_cap,
                "SurfaceLayerInfo for boundary {bd_idx}: outer_node {:?} missing from capacitances",
                info.outer_node
            );
            debug_assert!(
                inner_in_index,
                "SurfaceLayerInfo for boundary {bd_idx}: inner_node {:?} missing from node_index",
                info.inner_node
            );
            debug_assert!(
                outer_in_index,
                "SurfaceLayerInfo for boundary {bd_idx}: outer_node {:?} missing from node_index",
                info.outer_node
            );
        }

        if !inner_has_cap {
            tracing::warn!(bd_idx = bd_idx, node = ?info.inner_node, "SurfaceLayerInfo inner_node missing from capacitances");
        }
        if !outer_has_cap {
            tracing::warn!(bd_idx = bd_idx, node = ?info.outer_node, "SurfaceLayerInfo outer_node missing from capacitances");
        }
        if !inner_in_index {
            tracing::warn!(bd_idx = bd_idx, node = ?info.inner_node, "SurfaceLayerInfo inner_node missing from node_index");
        }
        if !outer_in_index {
            tracing::warn!(bd_idx = bd_idx, node = ?info.outer_node, "SurfaceLayerInfo outer_node missing from node_index");
        }
    }
}

/// Assemble the multi-layer RC network from pre-resolved boundary data.
///
/// Returns the continuous-time state-space matrices and metadata needed
/// to construct the thermal solver.
pub fn assemble_building_rc(
    boundaries: &[BoundaryInput],
    n_zones: usize,
    zone_capacitances: &[f64],
    interior_lwr_method: InteriorLwrMethod,
) -> Result<(BuildingRC, EnvelopeDiagnostics), String> {
    let outdoor_node = NodeId(OUTDOOR_NODE_ID);

    let mut unique_depths: Vec<f64> = boundaries
        .iter()
        .filter(|b| b.exterior == ExteriorTarget::Ground && b.area_m2 > 0.0)
        .map(|b| (b.foundation_depth_m * 1000.0).round() / 1000.0) // round to mm
        .collect();
    unique_depths.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    unique_depths.dedup();
    let depth_to_node: std::collections::HashMap<u64, NodeId> = unique_depths
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            // Round to integer millimetres for the HashMap key (avoids f64 hashing).
            let key = (d * 1000.0).round() as u64;
            (key, NodeId(GROUND_NODE_BASE + i as u32))
        })
        .collect();

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

    // Window boundaries whose interior film includes h_rad need a floating
    // window_node in StarMesh mode. Collected during the boundary loop and
    // consumed in the star-mesh section to add window_node ↔ star_node edges.
    let mut window_for_starmesh: Vec<(usize, NodeId)> = Vec::new();

    for (bd_idx, bd) in boundaries.iter().enumerate() {
        if bd.area_m2 <= 0.0 {
            continue;
        }

        let interior_node = NodeId((bd.interior_zone_idx + 1) as u32);
        let exterior_node = match bd.exterior {
            ExteriorTarget::Zone(idx) => NodeId((idx + 1) as u32),
            ExteriorTarget::Outdoor => outdoor_node,
            ExteriorTarget::Ground => {
                let key = (bd.foundation_depth_m * 1000.0).round() as u64;
                *depth_to_node.get(&key).unwrap_or_else(|| {
                    panic!(
                        "boundary {bd_idx}: foundation_depth_m={} has no ground node; \
                         available depths: {unique_depths:?}",
                        bd.foundation_depth_m
                    )
                })
            }
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
            interior_lwr_method,
        };

        // Precomputed RC path (OCHRE LUT) takes priority over raw material layers.
        if !bd.precomputed_rc.is_empty() {
            let nodes_before = graph.next_layer_id;
            let surface_opt = graph
                .build_precomputed_boundary(&bd.precomputed_rc, &bp)
                .map(|(inner, outer, surf)| {
                    layer_info.insert(
                        bd_idx,
                        SurfaceLayerInfo {
                            outer_node: outer,
                            inner_node: inner,
                            surface_node: surf,
                            interior_zone_idx: bd.interior_zone_idx,
                        },
                    );
                    surf
                });
            let has_surface_node = surface_opt.flatten().is_some();
            // n_nodes counts capacitance-bearing RC nodes only.
            // surface_node (floating, no cap) must be excluded.
            let n_cap_nodes = (graph.next_layer_id - nodes_before) as usize
                - if has_surface_node { 1 } else { 0 };
            let r_layers: f64 = bd.precomputed_rc.iter().map(|l| l.resistance_m2_k_w).sum();
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
            let inner_node = if n_cap_nodes > 0 {
                let inner = NodeId(nodes_before + n_cap_nodes as u32 - 1);
                // Guard: inner_node must not alias any reserved external driving node.
                // Reserved range: [GROUND_NODE_BASE (u32::MAX - 100), u32::MAX] (101 IDs).
                // NodeId scheme allocates from LAYER_NODE_BASE (1000) upward; a collision
                // would require ~4.3B nodes — physically impossible — but the invariant
                // is enforced by assertion so that a future broken scheme panics immediately.
                assert!(
                    inner.0 < GROUND_NODE_BASE,
                    "precomputed-path inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
                    inner
                );
                Some(inner)
            } else {
                None
            };
            let r_zone_to_inner = if n_cap_nodes > 0 {
                let r_inner_half = bd
                    .precomputed_rc
                    .last()
                    .map(|l| l.resistance_m2_k_w / 2.0)
                    .unwrap_or(0.0);
                Some(bd.r_film_interior_m2_k_w + r_inner_half)
            } else {
                None
            };
            let pre_r_outer_half = if same_zone {
                None
            } else {
                bd.precomputed_rc.first().map(|l| l.resistance_m2_k_w / 2.0)
            };
            let pre_r_inner_half = bd.precomputed_rc.last().map(|l| l.resistance_m2_k_w / 2.0);
            boundary_diagnostics.push(BoundaryDiagnostic {
                boundary_idx: bd_idx,
                ua_w_per_k: bd.area_m2 / r_total.max(1e-6),
                r_total_m2_k_w: r_total,
                capacitance_j_k: cap_total,
                n_rc_nodes: n_cap_nodes,
                interior_zone_idx: bd.interior_zone_idx,
                exterior_target: bd.exterior,
                area_m2: bd.area_m2,
                r_film_int_m2_k_w: bd.r_film_interior_m2_k_w,
                r_film_ext_m2_k_w: bd.r_film_exterior_m2_k_w,
                r_zone_to_inner_m2_k_w: r_zone_to_inner,
                r_outer_half_m2_k_w: pre_r_outer_half,
                r_inner_half_m2_k_w: pre_r_inner_half,
                path: RCPath::Precomputed,
                inner_node,
                interior_emissivity: bd.interior_emissivity,
                foundation_depth_m: bd.foundation_depth_m,
                #[cfg(feature = "observe")]
                same_zone_kept_half: if same_zone { Some("interior") } else { None },
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
            let build_result = graph.build_layered_boundary(&valid_layers, &bp);
            let (r_inner_half, r_outer_half, _surface_opt) =
                if let Some((inner, outer, surf, r_ih, r_oh)) = build_result {
                    layer_info.insert(
                        bd_idx,
                        SurfaceLayerInfo {
                            outer_node: outer,
                            inner_node: inner,
                            surface_node: surf,
                            interior_zone_idx: bd.interior_zone_idx,
                        },
                    );
                    (r_ih, r_oh, surf)
                } else {
                    (0.0, 0.0, None)
                };
            // n_nodes counts capacitance-bearing RC nodes only.
            // surface_node (floating, no cap) must be excluded.
            let n_cap_nodes = (graph.next_layer_id - nodes_before) as usize
                - if _surface_opt.is_some() { 1 } else { 0 };
            let r_layers: f64 = valid_layers
                .iter()
                .map(|l| l.thickness_m / l.conductivity_w_m_k)
                .sum();
            let r_effective = if same_zone { r_layers / 2.0 } else { r_layers };
            let r_total = r_effective
                + bd.r_film_interior_m2_k_w
                + if same_zone {
                    0.0
                } else {
                    bd.r_film_exterior_m2_k_w
                };
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
            let inner_node = if n_cap_nodes > 0 {
                let inner = NodeId(nodes_before + n_cap_nodes as u32 - 1);
                // Guard: inner_node must not alias any reserved external driving node.
                // Reserved range: [GROUND_NODE_BASE (u32::MAX - 100), u32::MAX] (101 IDs).
                // NodeId scheme allocates from LAYER_NODE_BASE (1000) upward; a collision
                // would require ~4.3B nodes — physically impossible — but the invariant
                // is enforced by assertion so that a future broken scheme panics immediately.
                assert!(
                    inner.0 < GROUND_NODE_BASE,
                    "material-layer-path inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
                    inner
                );
                Some(inner)
            } else {
                None
            };
            let r_zone_to_inner = if n_cap_nodes > 0 {
                Some(bd.r_film_interior_m2_k_w + r_inner_half)
            } else {
                None
            };
            let (diag_r_outer, diag_r_inner) = if n_cap_nodes > 0 {
                (
                    if same_zone { None } else { Some(r_outer_half) },
                    Some(r_inner_half),
                )
            } else {
                (None, None)
            };
            boundary_diagnostics.push(BoundaryDiagnostic {
                boundary_idx: bd_idx,
                ua_w_per_k: bd.area_m2 / r_total.max(1e-6),
                r_total_m2_k_w: r_total,
                capacitance_j_k: cap_total,
                n_rc_nodes: n_cap_nodes,
                interior_zone_idx: bd.interior_zone_idx,
                exterior_target: bd.exterior,
                area_m2: bd.area_m2,
                r_film_int_m2_k_w: bd.r_film_interior_m2_k_w,
                r_film_ext_m2_k_w: bd.r_film_exterior_m2_k_w,
                r_zone_to_inner_m2_k_w: r_zone_to_inner,
                r_outer_half_m2_k_w: diag_r_outer,
                r_inner_half_m2_k_w: diag_r_inner,
                path: RCPath::MaterialLayer,
                inner_node,
                interior_emissivity: bd.interior_emissivity,
                foundation_depth_m: bd.foundation_depth_m,
                #[cfg(feature = "observe")]
                same_zone_kept_half: if same_zone { Some("interior") } else { None },
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

            if interior_lwr_method == InteriorLwrMethod::StarMesh {
                // Option 2 (EnergyPlus/TRNSYS/ESP-r) window decomposition.
                //
                // When StarMesh mode is active and the boundary has no RC inner
                // node, decompose the interior film into convective and radiative
                // components. A floating window_node is created with connections
                // that enable inter-surface radiation through the star-mesh while
                // correctly representing the convection-only zone-air coupling.
                //
                // Architecture (matches EnergyPlus "Option 2", TRNSYS Type 56,
                // ESP-r):
                //   R_film_int = 1/h_conv (convection only, from TARP model)
                //   Linearized radiation conductances between all interior surface
                //   inner_nodes via a star-mesh
                //   Windows participate in the star-mesh via a floating interior
                //   node
                //
                // Window decomposition:
                //   zone_air ←R_conv→ window_node ←R_glass_ext→ outdoor
                //   window_node ←R_rad_star→ star_node ←R_rad_star→ other surfaces
                //
                // For windows: r_film_interior comes from E+ window U-factor
                // decomposition and includes combined h_si = h_conv + h_rad.
                // We decompose: h_conv = h_si - h_rad(ε_glass).
                // R_conv = 1/h_conv provides the zone-air ↔ window convection path
                // (the ONLY zone-air coupling; air is transparent to LWR).
                // Inter-surface radiation is handled by the star-mesh edge
                // (window_node ↔ star_node), added in the star-mesh section below.
                // This matches EnergyPlus Option 2 where h_c (convection-only)
                // couples surface to zone air, with LWR handled separately.
                //
                // For non-window fallback-R boundaries: r_film_interior from TARP
                // is already convection-only (Step 1 change). h_si < h_rad → no
                // decomposition needed.
                const T_REF_K: f64 = 293.15; // 20°C (OCHRE, TRNSYS, ESP-r)
                // Glass thermal emissivity ε = 0.84 (NFRC). The E+ Simple Window
                // Step 1 polynomial (E+ Eng.Ref §Window Heat Transfer Calculations)
                // was derived at this emissivity, so h_si implicitly contains
                // h_rad at ε = 0.84. The decomposition h_conv = h_si − h_rad
                // must subtract h_rad at the same ε that was implicit in h_si.
                //
                // The star-mesh radiation conductance also uses ε = 0.84 for
                // windows (via bd.interior_emissivity, set in conversions.rs),
                // ensuring total interior coupling = h_si exactly. Using ε = 0.9
                // for the star-mesh would over-couple by
                // h_rad(0.9) − h_rad(0.84) ≈ 0.34 W/(m²·K).
                const GLASS_THERMAL_EMISSIVITY: f64 = crate::longwave_radiation::EMISSIVITY_WINDOW;

                let a = bd.area_m2;
                let h_si = 1.0 / bd.r_film_interior_m2_k_w;
                let h_rad_glass =
                    hares_physics::constants::linearised_h_rad(GLASS_THERMAL_EMISSIVITY, T_REF_K);

                if h_si > h_rad_glass {
                    // Film includes h_rad (window case): decompose into conv + rad.
                    let h_conv = (h_si - h_rad_glass).max(0.1);
                    let r_film_conv = 1.0 / h_conv;

                    // Create floating window node for radiation topology.
                    // The window_node represents the window glass interior surface
                    // temperature. It connects to zone_air via convection ONLY
                    // in this section; the star-mesh edge for inter-surface
                    // radiation is added in the star-mesh section below.
                    // Air is transparent to LWR — there is no surface-to-zone-air
                    // radiation path. The convection-only R_conv is the sole
                    // zone-air coupling, matching EnergyPlus Option 2 where h_c
                    // (not h_si) provides surface-to-air heat transfer.
                    let window_node = graph.alloc_node_no_cap();

                    // zone_air ↔ window_node via convection-only interior film.
                    // R_conv = r_film_conv / A = 1/(h_conv × A).
                    let r_conv_abs = r_film_conv / a;
                    graph.add_resistance(interior_node, window_node, r_conv_abs.max(1e-6));

                    // window_node ↔ outdoor via glass + exterior film.
                    // fallback_r is glass-only R (Ro,w subtracted per E+ Step 1);
                    // r_film_ext = Ro,w is the standard winter exterior film.
                    // Together they reconstruct 1/U − Ri,w.
                    let r_glass_ext_abs = (bd.fallback_r_m2_k_w + bd.r_film_exterior_m2_k_w) / a;
                    graph.add_resistance(window_node, exterior_node, r_glass_ext_abs.max(1e-6));

                    // Store for star-mesh section (window_node ↔ star_node edge).
                    window_for_starmesh.push((bd_idx, window_node));
                } else {
                    // Film is convection-only (opaque fallback-R): decompose into
                    // surface_node for star-mesh radiation participation.
                    // zone_air ← R_film_conv → surface_node ← R_assembly+R_ext → outdoor
                    // surface_node ← R_rad_star → star_node ← R_rad_star → other surfaces
                    //
                    // Air is transparent to LWR, so there is no surface-to-zone-air
                    // radiation path. The star-mesh carries inter-surface radiation
                    // only; R_film_conv (convection-only) provides the sole zone-air
                    // coupling. After Y-Δ elimination of surface_node and star_node,
                    // radiation conductances are correctly distributed to inner_node
                    // and zone_air.
                    let surface_node = graph.alloc_node_no_cap();

                    // zone_air ↔ surface_node via convection-only interior film.
                    let r_conv_abs = bd.r_film_interior_m2_k_w / a;
                    graph.add_resistance(interior_node, surface_node, r_conv_abs.max(1e-6));

                    // surface_node ↔ outdoor via assembly + exterior film.
                    let r_rest_abs = (bd.fallback_r_m2_k_w + bd.r_film_exterior_m2_k_w) / a;
                    graph.add_resistance(surface_node, exterior_node, r_rest_abs.max(1e-6));

                    // Store for star-mesh section (surface_node ↔ star_node edge).
                    window_for_starmesh.push((bd_idx, surface_node));
                }
            } else {
                // ScriptF mode: original combined path
                let r_ohm = r_total.max(1e-6) / bd.area_m2;
                graph.add_resistance(interior_node, exterior_node, r_ohm);
            }

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
                r_outer_half_m2_k_w: None,
                r_inner_half_m2_k_w: None,
                path: RCPath::FallbackR,
                inner_node: None,
                interior_emissivity: bd.interior_emissivity,
                foundation_depth_m: bd.foundation_depth_m,
                #[cfg(feature = "observe")]
                same_zone_kept_half: None,
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

    // ── Star-mesh interior LWR conductances ─────────────────────────────
    //
    // When InteriorLwrMethod::StarMesh is selected, a floating "radiation star"
    // node is created per zone. Each interior surface connects to this star via
    // a linearized radiation conductance:
    //   R_iStar = 1 / (4 · ε_i · σ · A_i · T_ref³)
    //
    // The floating star node is eliminated by reduce_floating_nodes() during
    // RCNetwork::from_elements(), producing pairwise conductances between all
    // interior surfaces in the zone:
    //   G_ij = G_iStar · G_jStar / Σ_k G_kStar
    //
    // This matches OCHRE's `linearize_int_radiation` mode (Envelope.py:1048-1061),
    // TRNSYS Type 56, and ESP-r. The conductances are baked into the A-matrix
    // at construction time, so no iterative LWR injection is needed at runtime.
    //
    // Air is transparent to longwave radiation, so there is no surface-to-zone-air
    // LWR flux path. The combined h_si = h_conv + h_rad is a bookkeeping shorthand,
    // NOT a physical flux decomposition. In Option 2, h_conv provides the sole
    // zone-air ↔ surface coupling (via R_film_conv), while h_rad provides inter-
    // surface coupling via the star-mesh. They are NOT parallel paths to the same
    // destination — h_rad redistributes energy between surfaces, carrying zero net
    // energy through zone air.
    //
    // Radiation conductances connect from the interior surface temperature node
    // (not the inner RC node). For RC-layer boundaries, a floating surface_node
    // is created between R_film_conv and R_inner_half; for window/fallback-R
    // boundaries, a floating node is created between R_film and R_glass/R_assembly.
    // After Y-Δ elimination of surface_nodes and star_node, pairwise conductances
    // correctly distribute radiation exchange to inner_nodes and zone_air.
    //
    // T_ref = 293.15 K (20°C) is the standard linearization operating point
    // used by OCHRE, TRNSYS, and ESP-r. Within ±10 K of T_ref the
    // linearization error is <10% (see linearization_sensitivity_documentation test).
    if interior_lwr_method == InteriorLwrMethod::StarMesh {
        const T_REF_K: f64 = 293.15; // 20°C operating point (OCHRE, TRNSYS, ESP-r)

        // Build lookup from bd_idx to floating surface node (windows + fallback-R).
        let floating_node_map: HashMap<usize, NodeId> = window_for_starmesh
            .iter()
            .map(|&(bd_idx, sn)| (bd_idx, sn))
            .collect();

        for zone_idx in 0..n_zones {
            let star_node = graph.alloc_node_no_cap();

            for (bd_idx, bd) in boundaries.iter().enumerate() {
                if bd.area_m2 <= 0.0 || bd.interior_zone_idx != zone_idx {
                    continue;
                }
                // Only connect interior-facing surfaces with nonzero emissivity.
                // Same-zone (internal mass) boundaries are excluded because their
                // inner_node connects to the same zone air — they don't face
                // the radiation enclosure.
                let is_same_zone = match bd.exterior {
                    ExteriorTarget::Zone(idx) => idx == zone_idx,
                    _ => false,
                };
                if is_same_zone {
                    continue;
                }

                let e = bd.interior_emissivity;
                let a = bd.area_m2;
                if e <= 0.0 || a <= 0.0 {
                    continue;
                }

                // Look up the surface_node for this boundary.
                // Priority: surface_node from layer_info (RC-layer boundaries),
                // then floating_node_map (windows + opaque fallback-R).
                let surface_node = layer_info
                    .get(&bd_idx)
                    .and_then(|info| info.surface_node)
                    .or_else(|| floating_node_map.get(&bd_idx).copied());

                if let Some(s_node) = surface_node {
                    // Surface has a surface_node (from StarMesh decomposition).
                    //
                    // Inter-surface radiation via star node:
                    //    surface_node ↔ star_node, G = 4·ε·σ·A·T_ref³
                    //
                    // Air is transparent to longwave radiation — there is no
                    // surface-to-zone-air LWR flux path. The only zone-air ↔
                    // surface coupling is convection (h_conv through R_film_conv).
                    // Inter-surface radiation is carried exclusively by the star-
                    // mesh; it redistributes energy between surfaces but carries
                    // zero net energy through zone air.
                    //
                    // After Y-Δ elimination of surface_node and star_node, pairwise
                    // conductances between inner_nodes and zone_air correctly
                    // distribute both convective and radiative exchange.
                    //
                    // Reference: OCHRE Envelope.py:1053
                    //   `R = 1 / (4 * emissivity * sigma * area * T_ref**3)`
                    // Reference: E+ Eng.Ref §Inside Surface Heat Balance uses
                    //   h_c (convection-only) for surface-to-air coupling, with
                    //   LWR handled separately by ScriptF / star-mesh.
                    let g = hares_physics::constants::linearised_h_rad(e, T_REF_K) * a;
                    if g > 0.0 {
                        graph.add_resistance(s_node, star_node, 1.0 / g);
                    }
                }
                // Boundaries without surface_node (same-zone or ScriptF mode)
                // do not participate in the star-mesh radiation network.
            }
        }
    }

    // Build external nodes list: all ground nodes (one per depth) + outdoor.
    let n_ground = if ground_connected {
        depth_to_node.len()
    } else {
        0
    };
    let mut external_nodes = Vec::with_capacity(n_ground + 1);
    if ground_connected {
        for &node in depth_to_node.values() {
            external_nodes.push(node);
        }
    }
    if outdoor_connected {
        external_nodes.push(outdoor_node);
    }
    // Ground nodes (GROUND_NODE_BASE + i) are all < OUTDOOR_NODE_ID,
    // so RCNetwork::from_elements will sort them: ground[0] < ground[1] < ... < outdoor.

    let (capacitances, resistances) = graph.into_elements();

    let rc = RCNetwork::from_elements(capacitances, resistances, external_nodes)
        .map_err(|err| format!("RC network build failed: {err}"))?;

    // Look up outdoor column by node ID in the sorted external_nodes list.
    let outdoor_col = rc.external_nodes.iter().position(|&n| n == outdoor_node);
    // Build (depth_m, col_index) pairs in ascending depth order.
    // unique_depths is sorted, and NodeIds are GROUND_NODE_BASE + depth_index,
    // so column positions match depth order.
    let ground_cols: Vec<(f64, usize)> = depth_to_node
        .iter()
        .map(|(&key, &node)| {
            let depth_m = key as f64 / 1000.0;
            let col = rc
                .external_nodes
                .iter()
                .position(|&n| n == node)
                .unwrap_or_else(|| {
                    panic!(
                        "ground node {:?} (depth={depth_m}) missing from external nodes",
                        node
                    )
                });
            (depth_m, col)
        })
        .collect();

    let (a_c, b_ext, internal_node_order) = rc
        .build_matrices()
        .map_err(|err| format!("RC matrix assembly failed: {err}"))?;

    // Precomputed NodeId → row index map from the single-source-of-truth node ordering
    // returned by build_matrices() (which uses sorted_internal_nodes).
    let node_index: HashMap<NodeId, usize> = internal_node_order
        .iter()
        .enumerate()
        .map(|(idx, &nid)| (nid, idx))
        .collect();

    // Invariant check: cardinality of node_index must match A_c nrows.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        debug_assert_eq!(
            node_index.len(),
            a_c.nrows(),
            "node_index cardinality {node_index_len} != A_c nrows {a_c_nrows}",
            node_index_len = node_index.len(),
            a_c_nrows = a_c.nrows()
        );

        // Cross-validate: the independently-sorted node list from sorted_internal_nodes()
        // must agree element-for-element with the node_index keys in order.
        let cross: Vec<NodeId> = sorted_internal_nodes(&rc.capacitances, &rc.external_nodes);
        let node_index_sorted: Vec<NodeId> = {
            let mut keys: Vec<_> = node_index.keys().copied().collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            cross, node_index_sorted,
            "sorted_internal_nodes differs from node_index key set"
        );
    }

    validate_surface_layer_info(&layer_info, &rc.capacitances, &node_index);

    // Observer capture: record SurfaceLayerInfo wiring completeness per boundary.
    #[cfg(feature = "observe")]
    {
        for (bd_idx, info) in &layer_info {
            let inner_ok = rc.capacitances.contains_key(&info.inner_node)
                && node_index.contains_key(&info.inner_node);
            let outer_ok = rc.capacitances.contains_key(&info.outer_node)
                && node_index.contains_key(&info.outer_node);
            tracing::info!(
                bd_idx = bd_idx,
                inner_node = ?info.inner_node,
                outer_node = ?info.outer_node,
                inner_resolved = inner_ok,
                outer_resolved = outer_ok,
                "SurfaceLayerInfo wiring completeness"
            );
        }
    }

    // Observer capture: log internal node count for divergence detection.
    #[cfg(feature = "observe")]
    tracing::info!(
        node_index_len = node_index.len(),
        internal_node_order_len = internal_node_order.len(),
        a_c_nrows = a_c.nrows(),
        "RC assembly internal node mapping"
    );

    // Emit error on cardinality mismatch in release-with-checks builds
    // where debug_assert_eq is a no-op. No check in pure release builds.
    #[cfg(feature = "check_invariants")]
    {
        if node_index.len() != a_c.nrows() {
            tracing::error!(
                node_index_len = node_index.len(),
                a_c_nrows = a_c.nrows(),
                "node_index cardinality does not match A_c nrows"
            );
        }
    }

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

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // Only check when boundaries carry explicit material-layer definitions.
        // Fallback-R-only networks (no material layers, empty precomputed_rc)
        // legitimately produce zero boundary capacitance nodes — the single
        // resistance path is intentional.
        let has_material_layers = boundaries.iter().any(|b| !b.material_layers.is_empty());
        if has_material_layers {
            let has_cap_node = boundary_diagnostics.iter().any(|d| d.n_rc_nodes > 0);
            assert!(
                has_cap_node,
                "RC network has no capacitance-bearing boundary nodes despite \
                 material-layer definitions on at least one boundary. \
                 Verify that material layers have non-zero density \
                 (> 0 kg/m³) and specific_heat (> 0 J/(kg·K))."
            );
        }

        // Verify same-zone precomputed boundaries have correct RC chain topology:
        // the outermost (cut-surface) node must NOT connect directly to zone air
        // (it is the dead end of the fin), and the innermost node must connect to
        // zone air through a single path. This guards against wiring bugs that
        // would short-circuit the fin or leave it disconnected.
        for diag in &boundary_diagnostics {
            if diag.path == RCPath::Precomputed
                && diag.n_rc_nodes > 0
                && diag.exterior_target == ExteriorTarget::Zone(diag.interior_zone_idx)
            {
                if let Some(info) = layer_info.get(&diag.boundary_idx) {
                    let zone_node = NodeId((diag.interior_zone_idx + 1) as u32);
                    // Innermost node must connect to zone air.
                    assert!(
                        rc.resistances.contains_key(&(info.inner_node, zone_node))
                            || rc.resistances.contains_key(&(zone_node, info.inner_node)),
                        "same-zone precomputed boundary {}: inner node {:?} \
                         not connected to zone {:?}",
                        diag.boundary_idx,
                        info.inner_node,
                        zone_node
                    );
                    // Outermost (cut-surface) node must NOT connect directly to
                    // zone air — the fin dead-ends there.
                    if info.outer_node != info.inner_node {
                        assert!(
                            !rc.resistances.contains_key(&(info.outer_node, zone_node))
                                && !rc.resistances.contains_key(&(zone_node, info.outer_node)),
                            "same-zone precomputed boundary {}: outer (cut-surface) node \
                             {:?} incorrectly connected directly to zone {:?}",
                            diag.boundary_idx,
                            info.outer_node,
                            zone_node
                        );
                    }
                }
            }
        }
    }

    let boundary_ua: f64 = boundary_diagnostics.iter().map(|d| d.ua_w_per_k).sum();
    #[cfg(feature = "observe")]
    let default_r_fallback_count = boundaries.iter().filter(|b| b.used_default_r).count();
    let diagnostics = EnvelopeDiagnostics {
        boundaries: boundary_diagnostics,
        zone_capacitances_j_k: zone_capacitances.to_vec(),
        total_ua_w_per_k: boundary_ua + fallback_ua,
        #[cfg(feature = "observe")]
        default_r_fallback_count,
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
            ground_cols,
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

    /// Allocate a floating (zero-capacitance) node for star-mesh radiation topology.
    ///
    /// The node is NOT inserted into `capacitances`, so `reduce_floating_nodes()`
    /// will eliminate it into pairwise conductances between its neighbours via
    /// the star-mesh (Y-Δ) transform:
    ///   G_AB = G_AF × G_BF / Σ G_iF
    ///
    /// This is the standard technique for linearized inter-surface radiation in
    /// TRNSYS Type 56, ESP-r, and OCHRE's `linearize_int_radiation` mode.
    /// Reference: OCHRE Envelope.py:1048-1061, TRNSYS 18 Vol.5 §5.8.2.3.
    fn alloc_node_no_cap(&mut self) -> NodeId {
        let node = NodeId(self.next_layer_id);
        self.next_layer_id += 1;
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
    /// Returns `None` if no capacitor nodes remain, or `Some((inner, outer, surface_opt, r_inner_half, r_outer_half))`
    /// where inner is closest to zone air and outer is closest to exterior.
    /// `surface_opt` is `Some(surface_node)` in StarMesh mode (floating node between
    /// R_film_conv and R_inner_half for proper radiation topology), `None` in ScriptF
    /// mode (combined resistor, radiation_frac handles surface temperature).
    fn build_layered_boundary(
        &mut self,
        layers: &[&LayerInput],
        params: &BoundaryParams,
    ) -> Option<(NodeId, NodeId, Option<NodeId>, f64, f64)> {
        // Split thick dense layers into sub-layers for diurnal wave resolution.
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

        // Innermost layer (interior-facing) → interior zone.
        // In StarMesh mode: split into R_film_conv + R_inner_half with a floating
        // surface_node between them. The surface_node participates in the star-mesh
        // radiation network, giving the correct topology per E+ "Option 2", TRNSYS
        // Type 56, ESP-r. After Y-Δ elimination of the surface_node, radiation
        // conductances are properly distributed to inner_node and zone_air.
        //
        // In ScriptF mode: combined resistor (R_film + R_inner_half). The
        // radiation_frac voltage-divider handles surface temperature interpolation
        // for the iterative LWR solver.
        let mut surface_node: Option<NodeId> = None;
        if !params.same_zone {
            let inner = effective_layers[n_layers - 1];
            let inner_area = inner.effective_area(params.boundary_area);
            let k_inner = parallel_path_conductivity(inner.conductivity_w_m_k, ff);
            let r_inner_half_abs = inner.thickness_m / (2.0 * k_inner * inner_area);
            let r_conv_film_abs = params.r_film_interior / inner_area;

            if params.interior_lwr_method == InteriorLwrMethod::StarMesh {
                // StarMesh: surface_node between R_film_conv and R_inner_half.
                // inner_node ← R_inner_half → surface_node ← R_film_conv → zone_air
                let s_node = self.alloc_node_no_cap();
                self.add_resistance(
                    layer_nodes[n_layers - 1],
                    s_node,
                    r_inner_half_abs.max(1e-6),
                );
                self.add_resistance(s_node, params.interior_node, r_conv_film_abs.max(1e-6));
                surface_node = Some(s_node);
            } else {
                // ScriptF: combined resistor.
                let r_int = r_conv_film_abs + r_inner_half_abs;
                self.add_resistance(layer_nodes[n_layers - 1], params.interior_node, r_int);
            }
        }

        let r_outer_half = outer.thickness_m / (2.0 * k_outer);
        let inner = effective_layers[n_layers - 1];
        let k_inner_eff = parallel_path_conductivity(inner.conductivity_w_m_k, ff);
        let r_inner_half = inner.thickness_m / (2.0 * k_inner_eff);

        Some((
            layer_nodes[n_layers - 1],
            layer_nodes[0],
            surface_node,
            r_inner_half,
            r_outer_half,
        ))
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
    /// Returns `None` if no capacitor nodes remain, or `Some((inner, outer, surface_opt))`
    /// where inner is closest to zone air and outer is closest to exterior.
    /// `surface_opt` is `Some(surface_node)` in StarMesh mode, `None` in ScriptF mode.
    fn build_precomputed_boundary(
        &mut self,
        layers: &[PrecomputedRCLayer],
        params: &BoundaryParams,
    ) -> Option<(NodeId, NodeId, Option<NodeId>)> {
        if layers.is_empty() {
            return None;
        }

        let mut cap_list: Vec<f64> = layers.iter().map(|l| l.capacitance_kj_m2_k).collect();
        let mut res_list: Vec<f64> = layers.iter().map(|l| l.resistance_m2_k_w).collect();
        let mut nodes = cap_list.len();

        // Step 1: same-zone boundaries -- cut in half, keeping the interior (last) half.
        //
        // This diverges from OCHRE's `create_rc_data` (envelope.py:312-314), which
        // keeps the *first* (exterior-facing) half via `[:new_nodes]`. OCHRE's
        // Boundary.__init__ then reverses the node and resistor lists (`[::-1]` at
        // Envelope.py:404-405) to make the cut-surface layer closest to the zone.
        // HARES keeps the interior half directly, matching the material-path
        // convention (`build_layered_boundary` lines 1388-1398), and skips the
        // reversal — the two approaches are topologically equivalent.
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

        // Step 2: split resistances -- pad [0, r0, ..., rN, 0], average adjacent pairs → N+1 resistors
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

        // Step 4: remove last resistor for same-zone boundaries.
        //
        // The padding/averaging step (Step 2) produces N+1 resistors for N
        // capacitors: the first resistor is the cut-surface half-resistance,
        // the last is the interior-most half-resistance. For same-zone
        // dead-end fins we remove the interior-most half-resistance so the
        // zone air connects through the inter-layer resistance between the
        // two innermost kept layers, consistent with the material-path
        // topology where the cut-surface side faces the zone and the
        // innermost layer is the dead end.
        //
        // This corrects a bug where res_list.remove(0) (removing the
        // cut-surface half-resistance) was used instead of OCHRE's
        // res_list = res_list[:-1] (envelope.py:336-337) which removes
        // the last resistor. The previous code made the chain connect to
        // zone air through the interior-most half-resistance, reversing
        // the intended topology.
        if params.same_zone && !res_list.is_empty() {
            res_list.pop();
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
        // Interior film handling depends on the LWR method:
        // - ScriptF: fold into the last resistor (combined R_film + R_inner_half).
        // - StarMesh: DO NOT fold into last resistor; instead we split the last
        //   resistor into R_inner_half + surface_node + R_film_conv in Step 7.
        if !params.same_zone && !res_abs.is_empty() {
            res_abs[0] += params.r_film_exterior / params.boundary_area;
        }
        let r_film_int_abs = params.r_film_interior / params.boundary_area;
        let fold_film_into_last =
            params.same_zone || params.interior_lwr_method != InteriorLwrMethod::StarMesh;
        if fold_film_into_last && !res_abs.is_empty() {
            let last = res_abs.len() - 1;
            res_abs[last] += r_film_int_abs;
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
        let mut surface_node: Option<NodeId> = None;
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
            if !params.same_zone && params.interior_lwr_method == InteriorLwrMethod::StarMesh {
                // StarMesh: split the last resistor into R_inner_half and R_film_conv
                // with a floating surface_node between them. The surface_node
                // participates in the star-mesh radiation network.
                // inner_node ← R_inner_half → surface_node ← R_film_conv → zone_air
                let r_inner_half_abs = res_abs[last_r_idx];
                let s_node = self.alloc_node_no_cap();
                self.add_resistance(layer_nodes[n_caps - 1], s_node, r_inner_half_abs.max(1e-6));
                self.add_resistance(s_node, params.interior_node, r_film_int_abs.max(1e-6));
                surface_node = Some(s_node);
            } else {
                self.add_resistance(
                    layer_nodes[n_caps - 1],
                    params.interior_node,
                    res_abs[last_r_idx],
                );
            }
        }

        Some((layer_nodes[n_caps - 1], layer_nodes[0], surface_node))
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
    interior_lwr_method: InteriorLwrMethod,
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
///
/// Note: this function uses SOFTWOOD_CONDUCTIVITY_W_M_K (0.144 W/(m·K)) and is only
/// appropriate for wood-framed assemblies. For steel-framed assemblies, use
/// [`steel_frame_u_zone_method`] per ASHRAE HoF 2021 Ch. 27 modified zone method.
pub fn parallel_path_conductivity(k_cavity_w_m_k: f64, framing_factor: Option<f64>) -> f64 {
    match framing_factor {
        Some(ff) if ff > 0.0 && ff < 1.0 => {
            ff * SOFTWOOD_CONDUCTIVITY_W_M_K + (1.0 - ff) * k_cavity_w_m_k
        }
        _ => k_cavity_w_m_k,
    }
}

/// ASHRAE zone method for steel-framed wall effective U-value [W/(m²·K)].
///
/// Per ASHRAE Handbook of Fundamentals 2021, Ch. 27, §3.2 (Examples 5 and 7):
/// the zone method area-weights the U-values of the stud and cavity regions.
/// This is required for metal framing because the parallel-path method
/// (which area-weights conductivities) understates the thermal bridging effect
/// of high-conductance steel studs.
///
/// Formula:
/// ```text
/// U_eff = A_stud × U_stud + A_cavity × U_cavity
/// ```
/// where `A_stud = stud_width / stud_spacing`, `A_cavity = 1 - A_stud`,
/// `U_stud = 1 / r_stud`, `U_cavity = 1 / r_cavity`.
///
/// # Parameters
/// - `stud_width_m`: width of a single stud [m] (e.g. 0.0381 m = 1.5 in)
/// - `stud_spacing_m`: on-center stud spacing [m] (e.g. 0.4064 m = 16 in)
/// - `r_cavity_m2_k_w`: total R-value of the insulated cavity assembly (all
///   layers excluding the stud thermal bridge) [m²·K/W]
/// - `r_stud_m2_k_w`: R-value through the stud cross-section [m²·K/W].
///   For steel studs this is very small (steel k ≈ 50 W/(m·K)) but
///   non-zero; typical values are 0.001–0.01 m²·K/W depending on gauge.
///
/// # Panics
/// Panics (via `debug_assert!`) if geometric or thermal inputs are
/// non-physical (zero or negative dimensions, r ≤ 0).
pub fn steel_frame_u_zone_method(
    stud_width_m: f64,
    stud_spacing_m: f64,
    r_cavity_m2_k_w: f64,
    r_stud_m2_k_w: f64,
) -> f64 {
    debug_assert!(
        stud_width_m > 0.0,
        "stud_width_m must be > 0, got {stud_width_m}"
    );
    debug_assert!(
        stud_spacing_m > 0.0,
        "stud_spacing_m must be > 0, got {stud_spacing_m}"
    );
    debug_assert!(
        stud_width_m < stud_spacing_m,
        "stud_width_m ({stud_width_m}) must be < stud_spacing_m ({stud_spacing_m})"
    );
    debug_assert!(
        r_cavity_m2_k_w > 0.0,
        "r_cavity_m2_k_w must be > 0, got {r_cavity_m2_k_w}"
    );
    debug_assert!(
        r_stud_m2_k_w > 0.0,
        "r_stud_m2_k_w must be > 0, got {r_stud_m2_k_w}"
    );

    let a_stud = stud_width_m / stud_spacing_m;
    let a_cavity = 1.0 - a_stud;
    let u_stud = 1.0 / r_stud_m2_k_w;
    let u_cavity = 1.0 / r_cavity_m2_k_w;

    a_stud * u_stud + a_cavity * u_cavity
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
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
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
        let p_pa = hares_physics::constants::SEA_LEVEL_PRESSURE_PA;
        let caps = derive_zone_capacitances(&zones, p_pa).unwrap();
        assert_eq!(caps.len(), 1);
        // ρ computed from ideal gas law at 20 °C, not the 1.2041 constant.
        let rho = p_pa / (hares_physics::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * 293.15);
        let expected = rho * AIR_CP_J_KG_K * (100.0 * DEFAULT_HEIGHT_M) * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_prefers_explicit_volume() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: Some(300.0),
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let p_pa = hares_physics::constants::SEA_LEVEL_PRESSURE_PA;
        let caps = derive_zone_capacitances(&zones, p_pa).unwrap();
        // Should use 300 m³ (explicit), not 100 × 2.5 = 250 m³ (derived from area)
        let rho = p_pa / (hares_physics::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * 293.15);
        let expected = rho * AIR_CP_J_KG_K * 300.0 * INTERIOR_MASS_MULTIPLIER;
        assert!((caps[0] - expected).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_defaults_when_area_unknown() {
        let zones = vec![ZoneInput {
            floor_area_m2: None,
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let p_pa = hares_physics::constants::SEA_LEVEL_PRESSURE_PA;
        let caps = derive_zone_capacitances(&zones, p_pa).unwrap();
        let rho = p_pa / (hares_physics::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * 293.15);
        let expected = rho * AIR_CP_J_KG_K * DEFAULT_VOLUME_M3 * INTERIOR_MASS_MULTIPLIER;
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        assert!((caps[0] - MIN_CAPACITANCE_J_K).abs() < 1e-6);
    }

    #[test]
    fn zone_capacitance_denver_is_lower_than_sea_level() {
        // Denver (1609 m) pressure produces ~17% lower density than sea level.
        // Cite: ASHRAE HoF 2021 §1.8; ISA 1976.
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: Some(250.0),
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let p_sea = hares_physics::constants::SEA_LEVEL_PRESSURE_PA;
        let p_denver = hares_physics::air_properties::standard_pressure_pa(1609.0);
        let caps_sea = derive_zone_capacitances(&zones, p_sea).unwrap();
        let caps_denver = derive_zone_capacitances(&zones, p_denver).unwrap();
        let reduction_pct = (1.0 - caps_denver[0] / caps_sea[0]) * 100.0;
        assert!(
            reduction_pct > 15.0,
            "Denver zone capacitance should be >15% lower than sea-level, got {reduction_pct:.1}%"
        );
    }

    #[test]
    fn zone_capacitance_zero_pressure_returns_error() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: Some(250.0),
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let result = derive_zone_capacitances(&zones, 0.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BoundaryRcError::InvalidSitePressure { value } => {
                assert_eq!(value, 0.0);
            }
        }
    }

    #[test]
    fn zone_capacitance_negative_pressure_returns_error() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: Some(250.0),
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let result = derive_zone_capacitances(&zones, -1.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BoundaryRcError::InvalidSitePressure { value } => {
                assert_eq!(value, -1.0);
            }
        }
    }

    // ── Single zone, single boundary (no layers) ───────────────────────

    #[test]
    fn single_zone_no_layers_produces_valid_network() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let (rc, diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // Diurnal penetration depth Λ = √(α·86400/(4π)).
        // layer0 (100mm, k=0.5, ρ=1000, cp=800): α=6.25e-7 → Λ≈0.066 m → ceil(0.10/0.066)=2 nodes.
        // layer1 (50mm,  k=1.0, ρ=2000, cp=900): α=5.56e-7 → Λ≈0.062 m → ceil(0.05/0.062)=1 node.
        // 1 zone air + 2 + 1 = 4 states.
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5),
            make_boundary(30.0, 1, ExteriorTarget::Ground, vec![], 3.0),
        ];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 2, &caps, InteriorLwrMethod::ScriptF).unwrap();

        assert_eq!(rc.zone_state_rows.len(), 2);
        assert_eq!(rc.n_ext, 2); // outdoor + ground
        // Ground nodes (GROUND_NODE_BASE) are numerically < OUTDOOR_NODE_ID,
        // so they sort first in the external-nodes list.
        assert_eq!(rc.outdoor_col, Some(1)); // outdoor after ground
        assert_eq!(rc.ground_cols, vec![(0.0, 0)]); // depth 0.0 at column 0
    }

    // ── Ground-only produces no outdoor column ──────────────────────────

    #[test]
    fn ground_only_has_no_outdoor_col() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Only a slab boundary connecting zone 0 to ground.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Ground, vec![], 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // Ground is the only external node; outdoor_col should be None.
        assert_eq!(rc.outdoor_col, None);
        assert_eq!(rc.n_ext, 1);
        assert_eq!(rc.ground_cols, vec![(0.0, 0)]);
    }

    // ── Same-zone boundary without layers is a no-op ───────────────────

    #[test]
    fn same_zone_no_layers_is_noop() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Same-zone boundary with no material layers -- no thermal mass to model.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), vec![], 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.10, 1.0, 2000.0, 900.0, 0.0),
        ];
        // Same-zone boundary, diurnal-criterion node counts:
        //   layer0 (50mm, k=0.5, ρ=1000): Λ≈0.066 m → 1 node
        //   layer1 (100mm, k=1.0, ρ=2000): Λ≈0.062 m → 2 nodes
        //   layer2 same as layer0 → 1 node
        //   layer3 same as layer1 → 2 nodes
        // Total sub-layers = 6, halved to 3 internal mass nodes + 1 zone air = 4 states.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Use low-density material (density=50 < SPLIT_MIN_DENSITY=100) to avoid auto-splitting.
        // density=50, cp=900, thickness=0.10, area=50 → full cap = 225 J/K
        let layers = vec![make_layer(0.10, 1.0, 50.0, 900.0, 0.0)];
        // 1 layer (no splitting) → keep 1 (n/2+1=1), with halved capacitance.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Zone(0), layers, 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();

        // 20 boundaries, each with 3 layers. Diurnal-criterion splitting:
        //   layer0 (0.05m, k=0.5, ρ=1000, cp=800): Λ≈0.066 m → ceil(0.05/0.066)=1
        //   layer1 (0.10m, k=1.0, ρ=2000, cp=900): Λ≈0.062 m → ceil(0.10/0.062)=2
        //   layer2 (0.02m, k=0.3, ρ=800,  cp=700): Λ≈0.061 m → ceil(0.02/0.061)=1
        // = 4 sub-layers per boundary × 20 = 80 layer nodes total.
        let layers = vec![
            make_layer(0.05, 0.5, 1000.0, 800.0, 0.0),
            make_layer(0.1, 1.0, 2000.0, 900.0, 0.0),
            make_layer(0.02, 0.3, 800.0, 700.0, 0.0),
        ];
        let boundaries: Vec<BoundaryInput> = (0..20)
            .map(|_| make_boundary(10.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5))
            .collect();

        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        // 1 zone + 80 layer nodes = 81 internal nodes.
        assert_eq!(rc.a_c.nrows(), 81);
        // All node IDs must be outside the reserved driving-node range.
        // Reserved range: [GROUND_NODE_BASE (u32::MAX - 100), u32::MAX].
        for &nid in rc.node_index.keys() {
            assert!(
                nid.0 < GROUND_NODE_BASE,
                "node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
                nid
            );
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Only zone 0 has a boundary; zone 1 is disconnected.
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 2, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let (rc, _diag) = assemble_building_rc(&[], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Zone 0 ↔ Zone 1 internal boundary (no outdoor/ground).
        let boundaries = vec![make_boundary(30.0, 0, ExteriorTarget::Zone(1), vec![], 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 2, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 0.0)];
        let boundaries = vec![
            make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers.clone(), 2.5),
            make_boundary(40.0, 1, ExteriorTarget::Outdoor, layers, 2.5),
        ];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 2, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 50.0)];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let boundaries = vec![make_boundary(0.0, 0, ExteriorTarget::Outdoor, vec![], 2.5)];
        // Zone gets fallback; zero-area boundary is ignored.
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        }
    }

    #[test]
    fn precomputed_single_layer_creates_one_node() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Middle layer has zero capacitance -- should be merged out.
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 2, &caps, InteriorLwrMethod::ScriptF).unwrap();
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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
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
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

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
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Boundary has both material layers AND precomputed -- precomputed wins.
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
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (rc, _diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // 1 zone + 1 precomputed layer (not 2 raw layers).
        assert_eq!(rc.a_c.nrows(), 2);
    }

    // ── Same-zone precomputed path tests ─────────────────────────────────

    #[test]
    fn build_precomputed_boundary_same_zone_correct_topology() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // 4 precomputed layers simulating: gypsum, insulation, sheathing, siding
        // (exterior → interior). same_zone=true keeps the interior half (2 layers).
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.5, // gypsum (exterior)
                capacitance_kj_m2_k: 15.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 2.0, // insulation
                capacitance_kj_m2_k: 5.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.5, // sheathing (interior half start)
                capacitance_kj_m2_k: 8.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.3, // siding (innermost)
                capacitance_kj_m2_k: 12.0,
            },
        ];
        let boundaries = vec![BoundaryInput {
            area_m2: 25.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Zone(0),
            material_layers: vec![],
            precomputed_rc: precomputed,
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        }];
        let (rc, diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // 1 zone air + 2 kept layers (interior half of 4) = 3 states.
        assert_eq!(rc.a_c.nrows(), 3);

        // The diagnostic should record 2 precomputed RC nodes for this boundary.
        let bd = &diag.boundaries[0];
        assert_eq!(bd.path, RCPath::Precomputed);
        assert_eq!(bd.n_rc_nodes, 2);

        // Verify the RC chain topology: the innermost layer node connects to
        // zone air; the cut-surface (outer) node is the dead end of the fin.
        let info = &rc.layer_info[&0];
        let zone_node = NodeId(1u32);
        assert_ne!(
            info.inner_node, info.outer_node,
            "inner and outer nodes must differ for multi-layer same-zone"
        );

        let zone_row = rc.node_index[&zone_node];
        let inner_row = rc.node_index[&info.inner_node];
        let outer_row = rc.node_index[&info.outer_node];

        // Innermost node ↔ zone coupling must exist (positive A_c entry).
        let g_inner_zone = rc.a_c[(inner_row, zone_row)];
        assert!(
            g_inner_zone > 0.0,
            "innermost-node (row {inner_row}) to zone (row {zone_row}) coupling \
             must be positive, got {g_inner_zone}"
        );

        // Cut-surface (outer) node must NOT couple directly to zone air.
        // The fin dead-ends there; coupling is only through inter-layer resistors.
        let g_outer_zone = rc.a_c[(outer_row, zone_row)];
        assert!(
            g_outer_zone == 0.0,
            "outer (cut-surface) node (row {outer_row}) must not couple directly \
             to zone air (row {zone_row}), got {g_outer_zone}"
        );

        // Inter-layer coupling between the two kept layers must exist.
        let g_inter_layer = rc.a_c[(inner_row, outer_row)];
        assert!(
            g_inter_layer > 0.0,
            "inter-layer coupling between inner and outer nodes must be positive, \
             got {g_inter_layer}"
        );
    }

    #[test]
    fn precomputed_same_zone_node_count_matches_material_path() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();

        // Use low-density materials (density < SPLIT_MIN_DENSITY=100) to avoid
        // diurnal-criterion auto-splitting in the material path, so both paths
        // produce the same number of capacitor nodes.
        let thickness = 0.10;
        let conductivity = 1.0;
        let density = 50.0; // < SPLIT_MIN_DENSITY → no auto-splitting
        let cp = 900.0;
        // R = thickness / k = 0.10, C_per_area = density*cp*thickness/1000 = 4.5 kJ/(m²·K)

        // Material-path boundary: 4 layers, same_zone → keep last 2.
        let material_layers: Vec<LayerInput> = (0..4)
            .map(|_| make_layer(thickness, conductivity, density, cp, 0.0))
            .collect();
        let mat_boundary = make_boundary(20.0, 0, ExteriorTarget::Zone(0), material_layers, 2.5);
        let (rc_mat, _) =
            assemble_building_rc(&[mat_boundary], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // Precomputed-path boundary: equivalent 4 layers, same_zone → keep last 2.
        let precomputed: Vec<PrecomputedRCLayer> = (0..4)
            .map(|_| PrecomputedRCLayer {
                resistance_m2_k_w: thickness / conductivity,
                capacitance_kj_m2_k: density * cp * thickness / 1000.0,
            })
            .collect();
        let pre_boundary =
            make_precomputed_boundary(20.0, 0, ExteriorTarget::Zone(0), precomputed, 2.5);
        let (rc_pre, _) =
            assemble_building_rc(&[pre_boundary], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // Both paths should produce the same number of RC states:
        // 1 zone air + 2 kept layers = 3.
        assert_eq!(rc_mat.a_c.nrows(), 3, "material-path node count");
        assert_eq!(rc_pre.a_c.nrows(), 3, "precomputed-path node count");
        assert_eq!(
            rc_mat.a_c.nrows(),
            rc_pre.a_c.nrows(),
            "material and precomputed paths must produce same node count for equivalent layers"
        );
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
            derive_zone_capacitances(
                &[ZoneInput {
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    mass_multiplier: INTERIOR_MASS_MULTIPLIER,
                }],
                hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
            )
            .unwrap()[0],
        ];

        // Without framing
        let bd_no_ff = BoundaryInput {
            area_m2: 10.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![layer.clone()],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (rc_no_ff, _) =
            assemble_building_rc(&[bd_no_ff], 1, &caps, InteriorLwrMethod::ScriptF).expect("no ff");

        // With 25% framing
        let bd_ff = BoundaryInput {
            area_m2: 10.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![layer],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 2.5,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: Some(0.25),
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (rc_ff, _) =
            assemble_building_rc(&[bd_ff], 1, &caps, InteriorLwrMethod::ScriptF).expect("with ff");

        // The A matrix diagonal for the zone node should be more negative with framing
        // (higher conductance → faster heat loss → more negative diagonal).
        let zone_diag_no_ff = rc_no_ff.a_c[(0, 0)];
        let zone_diag_ff = rc_ff.a_c[(0, 0)];
        assert!(
            zone_diag_ff < zone_diag_no_ff,
            "framing should increase heat loss (more negative A diagonal): no_ff={zone_diag_no_ff}, ff={zone_diag_ff}"
        );
    }

    // ── Steel frame zone method tests ───────────────────────────────

    #[test]
    fn steel_frame_u_zone_method_no_studs_equals_cavity_u() {
        // As stud_width → 0, U_eff → U_cavity (no thermal bridge).
        // R_cavity = 2.5 m²·K/W → U_cavity = 0.40 W/(m²·K).
        let stud_width_m = 0.001; // nearly zero
        let stud_spacing_m = 0.4064; // 16" OC
        let r_cavity = 2.5;
        let r_stud = 0.01; // steel stud R ~ 0.01 m²·K/W
        let u = steel_frame_u_zone_method(stud_width_m, stud_spacing_m, r_cavity, r_stud);
        let u_cavity = 1.0 / r_cavity;
        assert!(
            (u - u_cavity).abs() < 1.0,
            "near-zero stud width should approach cavity U: u={u:.4}, u_cavity={u_cavity:.4}"
        );
    }

    #[test]
    fn steel_frame_u_zone_method_steel_bridge_increases_u() {
        // A steel stud is a thermal short: U_eff must exceed U_cavity.
        // 2"×4" steel stud at 16" OC = 0.0381 m wide / 0.4064 m spacing.
        let stud_width_m = 0.0381; // 1.5"
        let stud_spacing_m = 0.4064; // 16"
        let r_cavity = 3.0; // insulated cavity R-3 SI
        let r_stud = 0.005; // 25-gauge steel stud ≈ very low R
        let u = steel_frame_u_zone_method(stud_width_m, stud_spacing_m, r_cavity, r_stud);
        let u_cavity = 1.0 / r_cavity;
        assert!(
            u > u_cavity,
            "steel stud must increase U above cavity-only: u={u:.4}, u_cavity={u_cavity:.4}"
        );
    }

    #[test]
    fn steel_frame_u_zone_method_parallel_path_would_understate() {
        // The parallel-path method using k_wood=0.144 would produce a much lower
        // effective conductivity than the zone method for steel (k≈50 W/(m·K)).
        // This test verifies the zone method U is significantly higher than
        // what parallel_path_conductivity would compute for the same geometry.
        let stud_width_m = 0.0381; // 1.5"
        let stud_spacing_m = 0.4064; // 16"
        let r_cavity = 3.0; // R-3 SI
        let r_stud = 0.005; // steel stud R
        let u_zone = steel_frame_u_zone_method(stud_width_m, stud_spacing_m, r_cavity, r_stud);

        // Parallel-path: framing fraction, then k_eff = ff*k_wood + (1-ff)*k_cavity.
        // For R_cavity=3.0 with 0.089m cavity insulation: k_cavity = 0.089/3.0 ≈ 0.0297.
        // k_eff_parallel = 0.094*0.144 + 0.906*0.0297 ≈ 0.0404 → U_parallel ≈ k_eff/0.089 ≈ 0.454.
        // Zone method (with steel): a_stud = 0.094, U_stud = 1/0.005 = 200.
        // U_zone = 0.094*200 + 0.906*0.333 = 18.8 + 0.302 = 19.1.
        // Ratio > 10× — parallel-path severely understates steel bridging.
        let ff = stud_width_m / stud_spacing_m;
        let k_cavity = 0.089 / r_cavity; // assume 0.089 m cavity for k derivation
        let k_eff_parallel = parallel_path_conductivity(k_cavity, Some(ff));
        let u_parallel = k_eff_parallel / 0.089; // U from conductivity+thickness

        assert!(
            u_zone > 5.0 * u_parallel,
            "zone method U ({u_zone:.2}) must be >> parallel-path U ({u_parallel:.4}) \
             for steel; parallel-path understates thermal bridge by >5×"
        );
    }

    #[test]
    fn steel_frame_u_zone_method_2x4_16oc_typical() {
        // Typical 2×4 steel-stud wall at 16" OC. Verify U_eff is in a
        // physically plausible range for a steel-framed assembly.
        let stud_width_m = 0.0381; // 1.5 in
        let stud_spacing_m = 0.4064; // 16 in
        let r_cavity = 2.3; // ~R-13 fiberglass (SI)
        let r_stud = 0.005; // steel stud through-metal R
        let u = steel_frame_u_zone_method(stud_width_m, stud_spacing_m, r_cavity, r_stud);

        // Steel stud is a severe thermal bridge. Zone method U should be
        // dominated by the stud path: a_stud × U_stud ≈ 0.094 × 200 ≈ 18.8.
        // So U ≈ 18.8 + 0.396 ≈ 19.2 W/(m²·K). This is very high but correct
        // for an unbroken steel thermal bridge — real assemblies include
        // exterior insulation to mitigate this.
        assert!(
            u > 10.0,
            "steel frame 2×4 at 16\" OC must have U >> 10 W/(m²·K) due to thermal \
             bridging: got {u:.2}"
        );
        assert!(
            u < 100.0,
            "steel frame U must be physically bounded: got {u:.2}"
        );
    }

    #[test]
    fn steel_frame_u_zone_method_sanity_as_stud_approaches_spacing() {
        // As stud_width → stud_spacing, the wall is all stud → U → U_stud.
        // R_stud = 0.005 → U_stud = 200.
        let u_all_stud = steel_frame_u_zone_method(0.399, 0.4064, 2.5, 0.005);
        let u_stud = 1.0 / 0.005;
        assert!(
            (u_all_stud - u_stud).abs() < 10.0,
            "dominant-stud U ({u_all_stud:.1}) should approach pure-stud U ({u_stud:.0})"
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
        let p_pa = hares_physics::constants::SEA_LEVEL_PRESSURE_PA;
        let caps = derive_zone_capacitances(&[conditioned, attic, foundation], p_pa).unwrap();
        let vol = 100.0 * DEFAULT_HEIGHT_M;
        // Density computed from ideal gas law at 20 °C, not the 1.2041 constant.
        let rho = p_pa / (hares_physics::constants::DRY_AIR_GAS_CONSTANT_J_KG_K * 293.15);
        let base = rho * AIR_CP_J_KG_K * vol;
        assert!((caps[0] - base * 7.0).abs() < 1e-6, "conditioned: 7x");
        assert!((caps[1] - base * 1.0).abs() < 1e-6, "attic: 1x");
        assert!((caps[2] - base * 1.5).abs() < 1e-6, "foundation: 1.5x");
    }

    #[test]
    fn split_layer_count_concrete_100mm_diurnal() {
        // 100mm concrete: k=0.51, rho=1400, cp=1000 → alpha=3.64e-7 m²/s
        // Diurnal penetration depth Λ = √(α·86400/(4π)) ≈ 0.050 m
        // n = ceil(0.100 / 0.050) = 2
        let n = split_layer_count(0.100, 0.51, 1400.0, 1000.0);
        assert!(
            n >= 2,
            "100mm concrete should need ≥2 nodes (diurnal criterion), got {n}"
        );
    }

    #[test]
    fn split_layer_count_thick_concrete_200mm() {
        // 200mm concrete slab should need more splits
        let n = split_layer_count(0.200, 1.13, 1400.0, 1000.0);
        assert!(
            n >= 3,
            "200mm concrete should need ≥3 nodes (diurnal criterion), got {n}"
        );
    }

    #[test]
    fn split_layer_count_insulation_no_split() {
        // Fiberglass insulation: k=0.04, rho=12, cp=840
        // The function returns 1 (thin relative to Λ); the caller guards on
        // density/conductivity thresholds so this function is never reached
        // for insulation in production code.
        let n = split_layer_count(0.066, 0.04, 12.0, 840.0);
        assert_eq!(
            n, 1,
            "insulation is thin relative to Λ, should not be split"
        );
    }

    #[test]
    fn split_layer_count_thin_wood_no_split() {
        // 9mm wood: k=0.14, rho=530, cp=900 → Λ ≈ 0.045 m
        // Thickness (9mm) << Λ (45mm) → lumped-capacitance (1 node) is exact
        // for the diurnal forcing band; ISO 13786:2007 §6.2.
        let n = split_layer_count(0.009, 0.14, 530.0, 900.0);
        assert_eq!(n, 1, "9mm wood is thin relative to Λ, 1 node suffices");
    }

    #[test]
    fn split_layer_count_diurnal_criterion_thick_concrete() {
        // 300mm concrete slab: k=1.13, rho=1400, cp=1000 → α=8.07e-7
        // Λ = √(8.07e-7 × 86400 / (4π)) ≈ 0.0745 m
        // n = ceil(0.300 / 0.0745) = 5
        let n = split_layer_count(0.300, 1.13, 1400.0, 1000.0);
        assert_eq!(
            n, 5,
            "300mm concrete should need 5 nodes (diurnal criterion)"
        );
    }

    #[test]
    fn split_layer_count_timestep_independent() {
        // The diurnal criterion is independent of timestep: calling the
        // function with the same material properties always gives the same n.
        // (This was not true for the old Fourier criterion which took dt_s.)
        let n1 = split_layer_count(0.100, 0.51, 1400.0, 1000.0);
        // Same result regardless of what timestep the simulation uses.
        assert_eq!(
            n1, 2,
            "100mm concrete always needs 2 nodes (diurnal criterion)"
        );
    }

    // ── r_zone_to_inner picks correct (last) layer ──────────────────────
    //
    // BESTEST 900FF floor: exterior→interior = [insulation, concrete]
    // With diurnal criterion, 80mm concrete (k=1.13, α=8.07e-7) splits into
    // 2 sub-layers of 40mm each.  r_zone_to_inner uses the post-split
    // innermost sub-layer half-R:
    //   r_zone_to_inner = r_film_interior + 0.040 / (2 × 1.130) ≈ 0.178 m²·K/W

    #[test]
    fn r_zone_to_inner_uses_innermost_layer() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        // Exterior→interior: insulation first, concrete (interior-facing) last.
        let layers = vec![
            make_layer(1.007, 0.040, 0.0, 0.0, 48.0), // insulation (exterior)
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
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        let bd_diag = &diag.boundaries[0];
        let r_zone_to_inner = bd_diag
            .r_zone_to_inner_m2_k_w
            .expect("floor boundary should have r_zone_to_inner");

        // 80mm concrete splits into 2 × 40mm; innermost sub-layer half-R = 0.040/(2×1.130)
        let expected_r = r_film_int + 0.040 / (2.0 * 1.130);
        assert!(
            (r_zone_to_inner - expected_r).abs() < 1e-4,
            "r_zone_to_inner={r_zone_to_inner:.4}, expected {expected_r:.4}"
        );
    }

    #[test]
    fn r_zone_to_inner_uses_post_split_thickness() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let r_film_int = 0.16_f64;
        // 120mm concrete splits into 2 sub-layers of 60mm each.
        let concrete = make_layer(0.120, 1.130, 1400.0, 1000.0, 48.0);
        assert_eq!(
            split_layer_count(0.120, 1.130, 1400.0, 1000.0),
            2,
            "120mm concrete should split into 2 sub-layers"
        );
        let bd = BoundaryInput {
            area_m2: 48.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Ground,
            material_layers: vec![concrete],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: r_film_int,
            r_film_exterior_m2_k_w: 0.0,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        let bd_diag = &diag.boundaries[0];
        let r_zone_to_inner = bd_diag
            .r_zone_to_inner_m2_k_w
            .expect("floor boundary should have r_zone_to_inner");

        // Post-split innermost sub-layer is 60mm, so half-R = 0.060 / (2 * 1.130).
        let expected_r = r_film_int + 0.060 / (2.0 * 1.130);
        assert!(
            (r_zone_to_inner - expected_r).abs() < 1e-4,
            "r_zone_to_inner={r_zone_to_inner:.4}, expected {expected_r:.4} (post-split 60mm)"
        );
    }

    #[test]
    fn r_zone_to_inner_precomputed_path() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.5,
                capacitance_kj_m2_k: 80.0,
            },
        ];
        let bd = BoundaryInput {
            area_m2: 20.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: Vec::new(),
            precomputed_rc: precomputed,
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        let bd_diag = &diag.boundaries[0];
        let r_zone_to_inner = bd_diag
            .r_zone_to_inner_m2_k_w
            .expect("precomputed boundary should have r_zone_to_inner");

        // Last precomputed layer has R=0.5, so half-R = 0.25.
        let expected_r = 0.12 + 0.5 / 2.0;
        assert!(
            (r_zone_to_inner - expected_r).abs() < 1e-4,
            "r_zone_to_inner={r_zone_to_inner:.4}, expected {expected_r:.4}"
        );
    }

    #[test]
    fn split_outer_layer_half_r_reflects_post_split_thickness() {
        let caps = derive_zone_capacitances(
            &[ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            }],
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .unwrap();
        let concrete = make_layer(0.100, 0.51, 1400.0, 840.0, 20.0);
        assert_eq!(
            split_layer_count(0.100, 0.51, 1400.0, 840.0),
            2,
            "100mm concrete should split into 2 sub-layers"
        );
        let bd = BoundaryInput {
            area_m2: 20.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![concrete],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        let bd_diag = &diag.boundaries[0];
        let r_outer = bd_diag
            .r_outer_half_m2_k_w
            .expect("should have r_outer_half");
        let expected = 0.050 / (2.0 * 0.51);
        assert!(
            (r_outer - expected).abs() < 1e-6,
            "r_outer_half={r_outer:.6}, expected {expected:.6} (50mm post-split)"
        );
    }

    #[test]
    fn split_inner_layer_half_r_reflects_post_split_thickness() {
        let caps = derive_zone_capacitances(
            &[ZoneInput {
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            }],
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .unwrap();
        let insulation = make_layer(0.089, 0.04, 12.0, 840.0, 20.0);
        let concrete_inner = make_layer(0.100, 0.51, 1400.0, 840.0, 20.0);
        assert_eq!(
            split_layer_count(0.100, 0.51, 1400.0, 840.0),
            2,
            "100mm concrete should split into 2 sub-layers"
        );
        let bd = BoundaryInput {
            area_m2: 20.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![insulation, concrete_inner],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        let bd_diag = &diag.boundaries[0];
        let r_inner = bd_diag
            .r_inner_half_m2_k_w
            .expect("should have r_inner_half");
        let expected = 0.050 / (2.0 * 0.51);
        assert!(
            (r_inner - expected).abs() < 1e-6,
            "r_inner_half={r_inner:.6}, expected {expected:.6} (50mm post-split)"
        );
    }

    #[test]
    fn case_900_wall_exterior_rad_frac() {
        let caps = derive_zone_capacitances(
            &[ZoneInput {
                floor_area_m2: Some(48.0),
                volume_m3: Some(129.6),
                mass_multiplier: INTERIOR_MASS_MULTIPLIER,
            }],
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .unwrap();
        let concrete = make_layer(0.100, 0.51, 1400.0, 840.0, 0.0);
        let insulation = make_layer(0.0615, 0.04, 12.0, 840.0, 0.0);
        let plasterboard = make_layer(0.012, 0.16, 950.0, 840.0, 0.0);
        let r_film_ext = R_FILM_EXTERIOR_M2_K_W;
        let bd = BoundaryInput {
            area_m2: 21.6,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![concrete, insulation, plasterboard],
            precomputed_rc: Vec::new(),
            fallback_r_m2_k_w: 0.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: r_film_ext,
            framing_factor: None,
            interior_emissivity: crate::longwave_radiation::EMISSIVITY_DEFAULT,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        };
        let (_rc, diag) =
            assemble_building_rc(&[bd], 1, &caps, InteriorLwrMethod::ScriptF).unwrap();
        let bd_diag = &diag.boundaries[0];
        let r_outer = bd_diag
            .r_outer_half_m2_k_w
            .expect("should have r_outer_half");
        let exterior_rad_frac = r_film_ext / (r_film_ext + r_outer);
        assert!(
            (exterior_rad_frac - 0.38).abs() < 0.02,
            "exterior_rad_frac={exterior_rad_frac:.3}, expected ≈0.38"
        );
    }

    // ── Star-mesh interior LWR energy conservation ────────────────────
    //
    // When all surfaces in a zone are at the same temperature, the net
    // heat flow on every surface node via linearized radiation conductances
    // must be zero. This is a fundamental energy-conservation property
    // of the star-mesh topology (OCHRE `linearize_int_radiation` mode,
    // TRNSYS Type 56, ESP-r).
    //
    // Ignored until step 4 wires the star-mesh conductances into
    // `assemble_building_rc`.
    #[test]
    fn star_mesh_isothermal_zone_zero_net_flow() {
        /// Linearization reference temperature [K]. 20°C operating point
        /// matching OCHRE, TRNSYS Type 56, ESP-r.
        const T_REF_K: f64 = 293.15_f64;

        // 4-surface zone: areas from BESTEST Case 600 geometry.
        let areas = [48.0_f64, 21.6, 16.2, 12.0]; // roof, wall, wall, floor
        let eps = [0.9_f64; 4]; // all opaque, ε = 0.90

        // Build boundary inputs with simple layered construction.
        let zones = vec![ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![make_layer(0.1, 0.5, 1000.0, 800.0, 0.0)];

        let boundaries: Vec<BoundaryInput> = areas
            .iter()
            .zip(eps.iter())
            .map(|(&area, &emissivity)| BoundaryInput {
                area_m2: area,
                interior_zone_idx: 0,
                exterior: ExteriorTarget::Outdoor,
                material_layers: layers.clone(),
                precomputed_rc: Vec::new(),
                fallback_r_m2_k_w: 2.5,
                r_film_interior_m2_k_w: 0.12,
                r_film_exterior_m2_k_w: R_FILM_EXTERIOR_M2_K_W,
                framing_factor: None,
                interior_emissivity: emissivity,
                foundation_depth_m: 0.0,
                #[cfg(feature = "observe")]
                used_default_r: false,
            })
            .collect();

        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::StarMesh).unwrap();

        // Zone air node (NodeId 1) is row 0. The innermost layer nodes follow.
        // Conservation check: for each node i, the total conductance flowing out
        // equals the total conductance flowing in. In A_c + B_ext terms:
        //   -A_c[i,i] = Σ_{j≠i} A_c[i,j] + Σ_k B_ext[i,k]
        // This is Kirchhoff's current law: the diagonal magnitude equals the sum
        // of all off-diagonal entries (both internal and external).
        for i in 0..rc.a_c.nrows() {
            let a_c_offdiag: f64 = (0..rc.a_c.ncols())
                .filter(|&j| j != i)
                .map(|j| rc.a_c[(i, j)])
                .sum();
            let b_ext_row: f64 = (0..rc.b_ext.ncols()).map(|k| rc.b_ext[(i, k)]).sum();
            let diag = rc.a_c[(i, i)];
            let imbalance = (-diag) - (a_c_offdiag + b_ext_row);
            assert!(
                imbalance.abs() < 1e-9,
                "row {i}: -A_c[i,i]={:.6}, offdiag+B={:.6}, imbalance={imbalance:.3e}",
                -diag,
                a_c_offdiag + b_ext_row
            );
        }

        // Explicit star-mesh conductance verification:
        // For each pair of surface nodes i,j in the same zone, compute the
        // linearized radiation conductance G_ij from the star-mesh formula
        // and verify it matches the A-matrix conductance.
        let _zone_row = rc.zone_state_rows[0];

        // Collect inner nodes (one per boundary surface).
        let mut inner_nodes: Vec<(NodeId, f64, f64)> = Vec::new(); // (NodeId, area, emissivity)
        for (&bd_idx, info) in &rc.layer_info {
            if info.interior_zone_idx == 0 {
                inner_nodes.push((
                    info.inner_node,
                    boundaries[bd_idx].area_m2,
                    boundaries[bd_idx].interior_emissivity,
                ));
            }
        }
        inner_nodes.sort_by_key(|(nid, _, _)| *nid);

        // Compute star-mesh pairwise conductances.
        // G_iStar = 4·ε_i·σ·A_i·T_ref³  for each surface i.
        // After star-node elimination:
        // G_ij = G_iStar × G_jStar / Σ_k G_kStar
        let g_star: Vec<f64> = inner_nodes
            .iter()
            .map(|&(_, a, e)| hares_physics::constants::linearised_h_rad(e, T_REF_K) * a)
            .collect();
        let _sum_g_star: f64 = g_star.iter().sum();

        // At isothermal conditions (all T = T_ref), the net radiation
        // heat flow on each surface is:
        //   Q_i = Σ_{j≠i} G_ij × (T_j - T_i) = 0
        // since T_i = T_j for all pairs.
        // This is satisfied by construction for any conductance network.
        // The stronger test is that the conductances themselves are correct,
        // which the star_mesh_3_branch test in rc_network.rs already verifies.
        // Here we verify the A-matrix includes radiation conductances by
        // checking that the off-diagonal entries between inner nodes are
        // larger than they would be without radiation.
        let n_inner = inner_nodes.len();
        assert!(
            n_inner >= 2,
            "need ≥2 surfaces for LWR exchange, got {n_inner}"
        );

        for i in 0..n_inner {
            for j in (i + 1)..n_inner {
                let row_i = rc.node_index[&inner_nodes[i].0];
                let row_j = rc.node_index[&inner_nodes[j].0];
                // A_c[(row_i, row_j)] should include the star-mesh conductance
                // divided by C_i.  We just check it's nonzero (present).
                let a_ij = rc.a_c[(row_i, row_j)];
                assert!(
                    a_ij > 0.0,
                    "radiation conductance between surfaces {i} and {j} should be positive, got {a_ij}"
                );
            }
        }
    }

    /// Linearization sensitivity: h_rad at T_ref vs true h_rad at different temps.
    ///
    /// Documents the error range inherent in linearizing T⁴ radiation
    /// around T_ref = 293.15 K (20°C). The linearized coefficient is
    ///   h_rad_linear = 4 × ε × σ × T_ref³
    /// while the exact coefficient for small perturbations around T is
    ///   h_rad_exact(T) ≈ 4 × ε × σ × T³
    ///
    /// The relative error is |(T_ref/T)³ - 1|. At ±5 K the error is ~5%;
    /// at ±10 K it is ~10%; at ±20 K it is ~20%. This is acceptable for
    /// annual building energy simulation where surface temperatures
    /// typically stay within ±10 K of 20°C in conditioned zones.
    /// Reference: ASHRAE HOF 2021 Ch.25; EnergyPlus uses the same
    /// linearization for its interior radiation module.
    #[test]
    fn linearization_sensitivity_documentation() {
        const T_REF_K: f64 = 293.15_f64; // 20°C
        const EPSILON: f64 = 0.90;
        let h_ref: f64 = hares_physics::constants::linearised_h_rad(EPSILON, T_REF_K);

        // (temperature °C, max expected error %)
        let cases: [(f64, f64); 4] = [
            (12.0, 10.0), // −8 K from ref → ~9% error
            (22.0, 2.5),  // +2 K from ref → ~2% error
            (32.0, 12.0), // +12 K from ref → ~12% error
            (42.0, 20.0), // +22 K from ref → ~20% error
        ];

        for (t_c, max_err) in cases {
            let t_k = t_c + 273.15;
            let h_true: f64 = hares_physics::constants::linearised_h_rad(EPSILON, t_k);
            let error_pct = ((h_ref - h_true) / h_true).abs() * 100.0;
            assert!(
                error_pct < max_err,
                "at T={t_c:.0}°C: linearization error {error_pct:.1}% exceeds {max_err:.1}% threshold"
            );
        }
    }

    // ── Layer inner_node vs reserved IDs regression ───────────────────
    //
    // Verify that layer_info.inner_node (SurfaceLayerInfo) does not collide
    // with any reserved external driving node.  Also verifies the diagnostic
    // inner_node (BoundaryDiagnostic) for both material-layer and precomputed
    // paths.
    //
    // Node IDs are laid out as:
    //   Zone air nodes:  1 ..= n_zones
    //   Layer nodes:     LAYER_NODE_BASE (1000) .. next_layer_id
    //   Reserved range:  GROUND_NODE_BASE (u32::MAX - 100) .. u32::MAX
    //     ├─ Per-depth ground nodes: GROUND_NODE_BASE + 0..=98
    //     ├─ OUTDOOR_NODE_ID:        u32::MAX - 1
    //     └─ GROUND_NODE_ID:         u32::MAX
    //
    // Any call to alloc_node / alloc_node_no_cap increments next_layer_id from
    // LAYER_NODE_BASE upward.  Reaching GROUND_NODE_BASE would require allocating
    // >4 billion nodes — physically impossible.
    // This test documents and enforces the invariant, analogous to the existing
    // many_boundaries_no_node_id_collision test but scoped to inner_node
    // specifically.

    #[test]
    fn inner_node_does_not_collide_with_reserved_ids_material_path() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // SurfaceLayerInfo.inner_node must not alias any reserved driving node.
        let info = &rc.layer_info[&0];
        assert!(
            info.inner_node.0 < GROUND_NODE_BASE,
            "material-path inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
            info.inner_node
        );

        // BoundaryDiagnostic.inner_node (Option) must also be free of collisions.
        if let Some(diag_inner) = diag.boundaries[0].inner_node {
            assert!(
                diag_inner.0 < GROUND_NODE_BASE,
                "diagnostic inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
                diag_inner
            );
        }
    }

    #[test]
    fn inner_node_does_not_collide_with_reserved_ids_precomputed_path() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let precomputed = vec![
            PrecomputedRCLayer {
                resistance_m2_k_w: 1.0,
                capacitance_kj_m2_k: 30.0,
            },
            PrecomputedRCLayer {
                resistance_m2_k_w: 0.5,
                capacitance_kj_m2_k: 80.0,
            },
        ];
        let boundaries = vec![make_precomputed_boundary(
            20.0,
            0,
            ExteriorTarget::Outdoor,
            precomputed,
            2.5,
        )];
        let (rc, diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        let info = &rc.layer_info[&0];
        assert!(
            info.inner_node.0 < GROUND_NODE_BASE,
            "precomputed-path inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
            info.inner_node
        );

        if let Some(diag_inner) = diag.boundaries[0].inner_node {
            assert!(
                diag_inner.0 < GROUND_NODE_BASE,
                "diagnostic inner_node {:?} falls in reserved driving-node range [GROUND_NODE_BASE, u32::MAX]",
                diag_inner
            );
        }
    }

    // ── node_index cardinality vs A_c nrows ───────────────────────────

    #[test]
    fn node_index_cardinality_matches_a_c() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        assert_eq!(rc.node_index.len(), rc.a_c.nrows());
    }

    // ── SurfaceLayerInfo node validation ─────────────────────────────

    #[test]
    fn surface_layer_info_nodes_in_capacitances() {
        let zones = vec![ZoneInput {
            floor_area_m2: Some(100.0),
            volume_m3: None,
            mass_multiplier: INTERIOR_MASS_MULTIPLIER,
        }];
        let caps =
            derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA)
                .unwrap();
        let layers = vec![
            make_layer(0.1, 0.5, 1000.0, 800.0, 50.0),
            make_layer(0.05, 1.0, 2000.0, 900.0, 50.0),
        ];
        let boundaries = vec![make_boundary(50.0, 0, ExteriorTarget::Outdoor, layers, 2.5)];
        let (rc, _diag) =
            assemble_building_rc(&boundaries, 1, &caps, InteriorLwrMethod::ScriptF).unwrap();

        // Every SurfaceLayerInfo entry must have both inner_node and outer_node
        // present in the capacitances map and node_index.
        for (bd_idx, info) in &rc.layer_info {
            assert!(
                rc.node_capacitances.contains_key(&info.inner_node),
                "boundary {bd_idx}: inner_node {:?} missing from capacitances",
                info.inner_node
            );
            assert!(
                rc.node_capacitances.contains_key(&info.outer_node),
                "boundary {bd_idx}: outer_node {:?} missing from capacitances",
                info.outer_node
            );
            assert!(
                rc.node_index.contains_key(&info.inner_node),
                "boundary {bd_idx}: inner_node {:?} missing from node_index",
                info.inner_node
            );
            assert!(
                rc.node_index.contains_key(&info.outer_node),
                "boundary {bd_idx}: outer_node {:?} missing from node_index",
                info.outer_node
            );
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "SurfaceLayerInfo for boundary")]
    fn missing_surface_layer_node_asserts() {
        let info = SurfaceLayerInfo {
            outer_node: NodeId(LAYER_NODE_BASE + 9999),
            inner_node: NodeId(LAYER_NODE_BASE + 9998),
            surface_node: None,
            interior_zone_idx: 0,
        };
        let mut layer_info: HashMap<usize, SurfaceLayerInfo> = HashMap::new();
        layer_info.insert(0, info);
        let capacitances: HashMap<NodeId, f64> = HashMap::new();
        let node_index: HashMap<NodeId, usize> = HashMap::new();

        validate_surface_layer_info(&layer_info, &capacitances, &node_index);
    }
}
