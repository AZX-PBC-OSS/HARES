HARES A-Matrix & Inter-Surface Radiation Conductance Analysis
1. A-Matrix Data Structure and How Conductances Are Added
Core Data Structure (rc_network.rs:52-56)
pub struct RCNetwork {
    pub capacitances: HashMap<NodeId, f64>,       // internal nodes with thermal mass
    pub resistances: HashMap<(NodeId, NodeId), f64>, // edge → resistance [K/W]
    pub external_nodes: Vec<NodeId>,               // driving nodes (outdoor, ground)
}
How conductances are added — via RcGraphState::add_resistance() in boundary_rc.rs:688-696:
- Key is the canonical edge (min(a,b), max(a,b))
- If an edge already exists, the new resistor is combined in parallel: R_new = (R1 × R2) / (R1 + R2)
- Otherwise, the edge is simply inserted
- Resistance is clamped to ≥ 1e-6 K/W
How the A-matrix is built — RCNetwork::build_matrices() in rc_network.rs:139-183:
1. Internal nodes are sorted by NodeId (ascending) → this defines row/column ordering
2. For each internal node i with capacitance C_i:
   - For each neighbor j connected by resistance R_ij:
     - A[i,i] -= 1 / (R_ij × C_i)  (self-coupling, always negative)
     - A[i,j] += 1 / (R_ij × C_i)  (off-diagonal, if j is internal)
     - B[i,k] += 1 / (R_ij × C_i)  (if j is external, column k)
Star-mesh elimination — reduce_floating_nodes() in rc_network.rs:220-276:
- Any node that is neither in capacitances (internal) nor external_nodes is a "floating" node
- Floating nodes are automatically eliminated via the star-mesh transform:
  - For floating node f with N neighbors: G_new(ni, nj) = G(ni,f) × G(nj,f) / Σ G(ni,f) for all i≠j
  - This produces direct pairwise conductances between all neighbors of f
This is the mechanism OCHRE uses for inter-surface radiation — add a star node per zone and let the transform eliminate it into pairwise conductances. HARES already has the machinery.
NodeId Allocation Scheme
Node ID Range	Meaning
1..=n_zones	Zone air nodes (NodeId((zone_idx + 1) as u32))
1_000+	Material-layer RC nodes (allocated sequentially via next_layer_id)
u32::MAX - 1	Outdoor driving node (OUTDOOR_NODE_ID)
u32::MAX	Ground driving node (GROUND_NODE_ID)
State-Vector Row Ordering
Internal nodes are sorted by NodeId ascending (rc_network.rs:206-218):
- Zone air nodes (1..=n_zones) appear first (lowest IDs)
- Layer nodes (1_000+) appear after all zone air nodes
- External nodes (outdoor, ground) are not in the state vector — they drive via B_ext
The BuildingRC output provides:
- zone_state_rows: Vec<usize> — state-vector row index for each zone air node (one per zone)
- node_index: HashMap<NodeId, usize> — NodeId → state row for all internal nodes
- layer_info: HashMap<usize, SurfaceLayerInfo> — boundary index → {outer_node, inner_node, interior_zone_idx}
2. State Variable Ordering — Exact Index Mapping
For a building with n_zones zones and M total material-layer nodes:
State row 0:   NodeId(1)  → Zone 0 air
State row 1:   NodeId(2)  → Zone 1 air
...
State row n_zones-1: NodeId(n_zones) → Zone (n_zones-1) air
State row n_zones:   NodeId(1000) → 1st layer node (boundary 0, outermost)
State row n_zones+1: NodeId(1001) → 2nd layer node
...
State row n_zones+M-1: NodeId(1000+M-1) → last layer node
Interior-facing node for boundary bd_idx: layer_info[bd_idx].inner_node gives the NodeId, then node_index[inner_node] gives the state-vector row.
Zone air node for zone zone_idx: zone_state_rows[zone_idx] gives the state-vector row directly.
3. How solver_builder.rs Wires Up Boundaries
The SolverBoundary struct (solver_builder.rs:49-76)
This is the intermediate representation that captures all per-boundary metadata:
Field	Source	Notes
surface_idx	building.boundaries index	Boundary index
area_m2	boundary.area_m2	✅ Available
zone_idx	boundary_zone_index()	✅ Available
exterior_emissivity	exterior_emissivity()	Exterior side
attic_emissivity	attic_interior_emissivity()	Interior-facing emissivity
inner_wiring	layer_info → node_index	✅ NodeWiring { state_row, b_col }
interior_rad_frac	r_film_int / (r_film_int + r_inner_half)	✅
r_film_int_m2_k_w	From BoundaryInput	✅
diagnostic_r_zone_to_inner	From envelope diagnostics	✅
exterior_rad_frac	r_film_ext / (r_film_ext + r_outermost_half)	Exterior side
exterior_rad_res_k_w	r_film_ext / area_m2	Exterior side
boundary_category	Derived from boundary type	Wall/Floor/Roof/Window/InternalMass
Where radiation_frac Is Computed
Interior radiation_frac (solver_builder.rs:241-252):
let interior_rad_frac = if r_inner_half > 0.0 {
    r_film_int / (r_film_int + r_inner_half)   // OCHRE "full" mode
} else {
    1.0
};
This is the convection-only fraction. r_film_int = 1/h_conv from TARP, no parallel R_rad.
Exterior rad_frac (solver_builder.rs:227-239):
let (exterior_rad_frac, exterior_rad_res_k_w) = if r_outermost_half > 0.0 && area_m2 > 0.0 {
    (r_film_ext / (r_film_ext + r_outermost_half), r_film_ext / area_m2)
} else { (0.0, 0.0) };
Where radiation_frac Is Consumed
1. Exterior LWR iterative solver (longwave.rs:123-209):
   - rad_frac controls the interpolation t_surf = rad_frac × t_node + (1-rad_frac) × t_ext
   - The injected flux is (solar_w + q_lw) × rad_frac
2. Interior LWR injection (longwave.rs:362-398):
   - Opaque surfaces: u[input_index] += q × radiation_frac (to RC node) and u[air_idx] += q × (1 - radiation_frac) (to zone air)
   - Window surfaces: only u[air_idx] += q × (1 - radiation_frac) (no RC node)
3. Interior solar distribution (solar.rs:280-287):
   - u[input_index] += q × radiation_frac (to surface node)
   - air_total += q × (1 - radiation_frac) (returned for zone air)
4. Diagnostics (solver_builder.rs:762-779):
   - BoundaryDiagnosticInfo::RCNode { radiation_frac, ... } for surface temperature calculation
4. Current Interior LWR Injection Code Path
solver_builder.rs:build_default_solvers()
  ├── build_solver_boundaries() → Vec<SolverBoundary>
  │     └── computes interior_rad_frac = r_film_int / (r_film_int + r_inner_half)
  ├── surfaces_by_zone: HashMap<<ZoneId, Vec<<InteriorSurfaceInfo>>
  │     └── each InteriorSurfaceInfo has:
  │           state_index, input_index, area_m2, emissivity, radiation_frac,
  │           rad_res_k_w, solar_absorptance, is_floor, driving_temp
  └── For each zone with ≥ 2 surfaces:
        InteriorLwrZoneConfig { zone_id, surfaces, scriptf }
          └── compute_scriptf() → pre-computes ScriptFCoefficients
At runtime (each timestep):
  ThermalSolver::apply_interior_longwave_inputs()
    ├── For each InteriorLwrZoneConfig:
    │     ├── Initialize surface temps: t_surf = radiation_frac × T_node + (1-radiation_frac) × T_zone
    │     ├── Iterate n_iter times:
    │     │     ├── scriptf.net_flux_w_into(buf, &mut lwr_net_flux_buf)   [exact T⁴]
    │     │     └── t_new = t_base + q_lwr × rad_res_k_w  → update surface temp
    │     ├── Final flux at converged temps
    │     └── Inject into input vector u:
    │           ├── Opaque: u[input_index] += q × radiation_frac
    │           │          u[air_idx]    += q × (1 - radiation_frac)
    │           └── Window: u[air_idx]  += q × (1 - radiation_frac)
    └── Returns per-zone net interior LWR gains for diagnostics
Key insight: Interior LWR is currently handled as a runtime perturbation injected into u (the B-matrix input vector), NOT as a structural conductance in the A-matrix. The ScriptF exact T⁴ radiosity approach already computes inter-surface fluxes, but the result is split and injected into each surface's input_index independently.
5. OCHRE's add_radiation_resistances() — Reference Implementation
Source: vendors/OCHRE/ochre/Models/Envelope.py:1048-1061
def add_radiation_resistances(self):
    # add resistors to RC network to approximate internal radiation
    # linearizes radiation equation: H_ab = e_factor * (Ta^4 - Tb^4)
    #   -> dH/dT = e_factor * 4 * T^3 = 1/Rab
    # assumes operating point of T=20C for all boundary/zone temperatures
    # Note: node <label>-rad is removed from envelope model using star-mesh transform
    t_ref = 20 + degC_to_K
    radiation_res = {}
    for zone in self.zones.values():
        for surface in zone.surfaces:
            res_name = (surface.node, f"{zone.label}-rad")
            radiation_res[res_name] = 1 / (4 * surface.e_factor * t_ref**3)
    return radiation_res
How OCHRE consumes this (Envelope.py:960-961):
if self.linearize_int_radiation:
    resistances = update_with_par(resistances, self.add_radiation_resistances())
The resistances are merged into the main resistances dict before the RC model is created. The {zone.label}-rad node has no capacitance — it's a floating node. When OCHRE builds its RC model (which uses the same star-mesh transform as HARES), the floating *-rad node is automatically eliminated, producing direct pairwise conductances between all surface nodes in the same zone:
G_ij = G_i,star × G_j,star / Σ_k G_k,star
where G_i,star = 4 × ε_i × σ × A_i × T_ref³   (conductance from surface i to star)
The resulting equivalent conductance between surfaces i and j is:
G_ij = (4 × ε_i × σ × A_i × T_ref³) × (4 × ε_j × σ × A_j × T_ref³)
       ────────────────────────────────────────────────────────────────
                          Σ_k (4 × ε_k × σ × A_k × T_ref³)
Simplifying: G_ij = 16 × σ² × T_ref⁶ × (ε_i × A_i) × (ε_j × A_j) / Σ_k(ε_k × A_k)
This is the linearized radiation conductance at T_ref = 293.15 K (20°C).
6. Boundary Metadata Available at Construction Time
In solver_builder.rs, after build_solver_boundaries() returns, every SolverBoundary has:
Data	Field
Area [m²]	sb.area_m2
Interior emissivity	sb.attic_emissivity (for attic-zone surfaces) or needs to be derived
Zone label / index	sb.zone_idx + sb.zone_id
Inner node state row	sb.inner_wiring.state_row
Inner node B-column	sb.inner_wiring.b_col
radiation_frac	sb.interior_rad_frac
r_film_int	sb.r_film_int_m2_k_w
r_zone_to_inner	sb.diagnostic_r_zone_to_inner
Interior node NodeId	layer_info[surface_idx].inner_node
What's missing for the star-mesh approach:
- Interior emissivity for conditioned-zone opaque surfaces: Currently sb.attic_emissivity is used for attic surfaces, but for conditioned-zone surfaces the emissivity defaults to EMISSIVITY_DEFAULT = 0.90. There is no per-surface interior_emissivity field on SolverBoundary — it's only populated in InteriorSurfaceInfo later. You'd need to either:
  (a) add an interior_emissivity field to SolverBoundary, or
  (b) compute the star-mesh conductances at the point where InteriorSurfaceInfo is already populated (the surfaces_by_zone HashMap in build_default_solvers).
7. Concrete Plan for Adding Star-Mesh Conductances
Approach: Two Options
Option A — Structural (in A-matrix, before state-space discretization):
Add linearized radiation conductances to the RC network at construction time (OCHRE's linearize_int_radiation approach). The conductances become permanent entries in the A-matrix, producing direct thermal coupling between interior-facing RC nodes.
Option B — Runtime perturbation (current ScriptF approach, enhanced):
Keep the current ScriptF T⁴ injection at runtime but remove the radiation_frac split that distributes LWR through the zone-air intermediate. Instead, inject each surface's net flux directly into its own input channel.
Recommendation: Option A — it matches OCHRE's proven approach and leverages the existing reduce_floating_nodes() star-mesh machinery.
Detailed Implementation Plan for Option A
Step 1: Add star-mesh radiation conductances in assemble_building_rc()
Where: In boundary_rc.rs, after all boundaries are built but before RCNetwork::from_elements() is called (around line 604).
What:
1. For each zone, identify the set of interior-facing boundary nodes (from layer_info) and their emissivities/areas.
2. Create one floating "radiation star" node per zone (NodeId allocated from next_layer_id, but not added to capacitances — no capacitance = floating node).
3. For each interior-facing surface i in zone z, add a resistance:
      R_i,star = 1 / (4 × ε_i × σ × A_i × T_ref³)
      where T_ref = 293.15 K (20°C, matching OCHRE).
4. The existing reduce_floating_nodes() in rc_network.rs will automatically eliminate the star node, producing pairwise conductances between all interior-facing nodes in the same zone.
New data needed:
- Interior emissivity per boundary (currently not in BoundaryInput). Need to add interior_emissivity: f64 to BoundaryInput.
- The set of boundaries belonging to each interior zone and their inner_node NodeIds.
Step 2: Modify BoundaryInput to carry interior emissivity
File: boundary_rc.rs (and conversions.rs where building_to_boundary_inputs populates it).
Add:
pub struct BoundaryInput {
    // ... existing fields ...
    /// Interior-facing longwave emissivity [-]. Default 0.90 for opaque surfaces.
    pub interior_emissivity: f64,
}
This is populated from the same logic that currently determines attic_emissivity in solver_builder.rs.
Step 3: Add the star-mesh wiring in assemble_building_rc()
After the boundary loop and before calling RCNetwork::from_elements(), add:
// Inter-surface radiation star-mesh: one floating star node per zone.
const T_REF_K: f64 = 293.15;  // 20°C operating point (matches OCHRE)
for (zone_idx, zone_boundaries) in zone_boundary_groups.iter().enumerate() {
    // Allocate a star node (no capacitance → floating → eliminated by star-mesh)
    let star_node = graph.alloc_node_no_cap();  // new method needed
    for &bd_idx in zone_boundaries {
        if let Some(info) = layer_info.get(&bd_idx) {
            let bd = &boundaries[bd_idx];
            let e = bd.interior_emissivity;
            let a = bd.area_m2;
            if e > 0.0 && a > 0.0 {
                let g = 4.0 * e * STEFAN_BOLTZMANN * a * T_REF_K.powi(3);
                let r_star = 1.0 / g;  // K/W
                graph.add_resistance(info.inner_node, star_node, r_star);
            }
        }
    }
}
New method needed on RcGraphState: alloc_node_no_cap() that creates a NodeId but does NOT insert it into capacitances. This makes it a floating node that reduce_floating_nodes() will eliminate.
Step 4: Remove or reduce the runtime ScriptF LWR injection
When star-mesh conductances are present in the A-matrix, the conduction path already carries the linearized radiation between interior-facing nodes. The runtime ScriptF T⁴ injection would double-count this.
Options:
- (a) Disable ScriptF entirely when star-mesh conductances are present (simplest, matches OCHRE's linearize_int_radiation=True mode)
- (b) Keep ScriptF but subtract the linearized baseline — compute the linearized flux from the A-matrix conductances and inject only the T⁴ correction
Recommendation: Start with (a) for parity with OCHRE. The A-matrix conductances at T_ref = 20°C are a reasonable linearization for typical residential temperatures.
Step 5: Remove radiation_frac splitting for interior LWR
Currently the LWR flux is split:
q × radiation_frac → surface RC node
q × (1 - radiation_frac) → zone air
With star-mesh conductances in the A-matrix, radiation between interior surfaces flows directly through the A-matrix conductances, not through the zone-air intermediate. The zone air only participates via convective film resistance (which is already in the A-matrix as the edge from inner_node to zone_air_node).
So:
- Remove the interior LWR radiation_frac split entirely for opaque surfaces
- Keep the radiation_frac for solar distribution (that's a separate physical mechanism)
- Windows continue to inject q × (1 - radiation_frac) to zone air only
Step 6: Update InteriorSurfaceInfo and InteriorLwrZoneConfig
If going with option (a) (disable ScriptF when star-mesh present), add a flag:
pub struct InteriorLwrZoneConfig {
    pub zone_id: ZoneId,
    pub surfaces: Vec<<InteriorSurfaceInfo>,
    pub scriptf: Option<<ScriptFCoefficients>,
    pub star_mesh_active: bool,  // NEW: when true, skip ScriptF injection
}
Or simply: don't populate interior_lwr_zones when star-mesh conductances are present, since the A-matrix handles it structurally.
Summary of Changes by File
File	Change
boundary_rc.rs	Add interior_emissivity to BoundaryInput; add star-mesh wiring logic after boundary loop; add alloc_node_no_cap() to RcGraphState
rc_network.rs	No changes needed — reduce_floating_nodes() already implements the star-mesh transform
solver_builder.rs	Pass interior_emissivity into BoundaryInput; skip InteriorSurfaceInfo construction for radiation when star-mesh is active
longwave.rs	Gate apply_interior_longwave_inputs() when star-mesh handles interior LWR structurally
config.rs	Optionally add star_mesh_active flag to InteriorLwrZoneConfig
conversions.rs	Populate interior_emissivity in building_to_boundary_inputs()
Verification Criteria
1. Energy conservation: Σ Q_interior_lwr = 0 per zone (the A-matrix conductances guarantee this by construction — the star-mesh transform preserves it)
2. OCHRE parity: At T_ref = 20°C, the linearized conductances must match OCHRE's add_radiation_resistances() output exactly
3. Thermal response: The A-matrix eigenvalues should show faster interior-surface equilibration than the current ScriptF injection approach (since radiation is now a structural coupling, not a perturbation)
4. Backward compatibility: When interior_emissivity is not provided, default to 0.90 (same as current behavior for opaque surfaces)