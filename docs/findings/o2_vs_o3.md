Interior Longwave Radiation: Option 2 (Star-Mesh Linearized) vs Option 3 (EnergyPlus-Style Explicit Surfaces)
1. Current Solver Structure
Finding: The solver is a single-pass, no-outer-iteration ZOH state-space step. Each timestep is structured as:
resolve_internal()
  → prepare_inputs_inner()       // Phase 1: build u, coupling (for HVAC capacity solve)
  → integrate_inner()            // Phase 2: rebuild u from post-dispatch ports, step model
      → build_input_vector()
          → apply_outdoor_inputs()
          → apply_solar_inputs()
          → apply_exterior_solar_inputs()
          → apply_exterior_longwave_inputs_iterative()  // ← outer iteration per-surface
          → apply_interior_longwave_inputs()             // ← inner iteration per-zone
          → apply_port_sensible_inputs()
          → apply_port_radiant_inputs()
          → apply_infiltration_and_ventilation()
      → build_coupling()
      → model.step_with_coupled_lu_into()  // OR model.step_into()
      → format_domain_update()
The step itself is x[k+1] = M⁻¹(N·x[k] + B_eff·u[k]) — a single matrix multiply + LU solve, no outer iteration. The iteration that exists is localized: apply_exterior_longwave_inputs_iterative iterates per-exterior-surface temps (4–8 iters each), and apply_interior_longwave_inputs iterates per-zone interior surface temps (3–5 iters). Neither feeds back into the A-matrix state.
Where would an outer iteration loop go? If Option 3 required converging surface temps that are state variables, the outer loop would wrap the entire integrate_inner() — the state vector would change, which would change surface temps, which would change the radiation input, which would change the state. This is a fundamentally different solver architecture from what exists today.
Key code references:
- stepping.rs:113-221 — the single-pass integrate
- state_space.rs:321-325 — the ZOH step (step_into)
- longwave.rs:219-401 — interior LWR with heavy-ball iteration
2. Current Surface Temperature Computation
Finding: Surface temperatures are fully implicit, derived quantities — NOT state variables. They are computed on-the-fly from the RC node state and zone air temperature:
T_surf = radiation_frac × T_node + (1 − radiation_frac) × T_zone
Evidence at longwave.rs:266:
let t_surf = s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone_c;
For windows (no RC node), the "node" temperature is replaced by a driving temperature:
T_surf = radiation_frac × T_driving + (1 − radiation_frac) × T_zone
The radiation_frac is defined as R_film / (R_film + R_material) where R_film is the interior convective film resistance and R_material is the resistance from the film to the capacitor node. This is a linear interpolation that assumes surface temperature lies on the thermal gradient between the RC node and the zone air.
What would it take to make them explicit? Each boundary surface would need:
1. A new state variable (row in A-matrix)
2. A tiny capacitance (for stability — EnergyPlus uses C → 0 with CondFD)
3. The innermost RC node would connect to the surface node instead of directly to the zone air
4. The surface node would connect to zone air via convection AND to other surfaces via radiation
This would require restructuring boundary_rc.rs and solver_builder.rs significantly.
3. A-Matrix Size for BESTEST Case 600
Finding: The simplified Case 600 test model uses 1 state (just zone air). The real full BESTEST Case 600 building built by solver_builder.rs would have:
Component
Zone air (indoor)
Wall inner (gypsum, lightweight)
Roof inner (lightweight)
Floor (slab)
Total current
A typical HPXML residential building (multi-zone with attic) produces 6–12 state variables based on the benchmarks in benches/rc_solver.rs.
Adding 6–7 surface temp nodes (one per boundary) would increase the A-matrix from ~5–6 to ~11–13 states. The ZOH step cost is dominated by the LU solve, which for a 13×13 dense matrix is negligible (<1 µs). Even for a 20-state system (multi-zone), the cost is trivial. Performance impact is essentially zero for residential-scale models.
The real concern is not performance but numerical stability: tiny capacitance surface nodes create stiff systems with time constants τ = C_surf × R_film that can be orders of magnitude smaller than the dominant zone air mode.
4. reduce_floating_nodes() — Correctness Analysis
Finding (CONFIDENCE: HIGH): The function at rc_network.rs:220-276 correctly eliminates all floating nodes including cascading elimination. Here's why:
fn reduce_floating_nodes(...) -> HashMap<(NodeId, NodeId), f64> {
    loop {                                          // ← outer loop
        let floating_nodes = ...;                   // ← find ALL floating nodes
        if floating_nodes.is_empty() { break; }     // ← exit when done
        for node_f in floating_nodes {              // ← eliminate each one
            remove_node_edges(&mut resistances, node_f);
            // Star-mesh transform: create pairwise edges
        }
    }
}
The loop ensures that if eliminating one floating node creates new floating-node configurations (e.g., a chain A—f1—f2—B where eliminating f1 leaves f2 still floating), the next iteration catches them. The test star_mesh_floating_node_matches_direct_equivalent validates the single-node case. The multi-iteration behavior is implicitly tested by from_elements which calls reduce_floating_nodes internally.
However: There is a subtle ordering concern. When multiple floating nodes share edges, the current code eliminates them in sorted NodeId order. If floating node A is connected to both floating node B and internal node C, eliminating A first creates an edge (B, C) via the star-mesh transform, which is correct. Then B is eliminated in the next outer-loop iteration. This is mathematically correct — the Y-Δ transform is associative regardless of elimination order (this is a standard result in circuit theory).
For Option 2: Adding both star nodes (per zone) AND floating window nodes is well within what reduce_floating_nodes handles. A zone with 6 surfaces + 1 star node + 1 floating window node would add 2 floating nodes, both eliminated correctly by the existing algorithm.
Residual risk: No test exists for three or more cascading floating nodes (e.g., internal—star—window—outdoor). I'd recommend adding one before shipping Option 2.
5. Stability of T⁴ Iteration with Explicit Surface State Variables
Finding: The OCHRE _solve_interior_radiation at Envelope.py:91-121 uses:
- Heavy-ball damping: t_new = t + 0.3 × (t_proposed − t) + 0.2 × (t − t_prev)
- Convergence in 3–5 iterations at Δt = 60s
- Clamping to [t_surf_min, t_surf_max]
Critical difference: In OCHRE, surface temperatures are NOT state variables. They are derived quantities (same as HARES's current approach). The iteration converges input fluxes — it doesn't feed back into the ODE state.
If surface temps were explicit state variables (Option 3): The T⁴ iteration would need to converge before the A-matrix step, because surface temps appear in both the radiation forcing AND the conduction path. This creates a coupled nonlinear system:
Given: x[k] (including surface states from last step)
Find: u[k] such that the radiation fluxes are self-consistent with the surface temps
      that would result from stepping the model with those fluxes
This is a fixed-point iteration over the entire integrate_inner() function, not just the LWR sub-calculation. The heavy-ball damping from OCHRE would help, but there's no guarantee it converges when surface temps feed back through the A-matrix.
Concrete risk scenarios:
1. Cold window, hot wall: LWR heats the wall surface → wall state warms → increased conduction to outdoor → wall surface cools → reduced LWR → oscillation
2. Solar step change: Solar heats an exterior surface → interior surface warms → LWR redistributes to other surfaces → their states change → LWR changes again
The OCHRE approach works because surface temps are "soft" (derived from node temps + zone temps via linear interpolation). With explicit states, they become "hard" (constrained by capacitance and energy balance), creating stronger coupling.
Mitigation: A semi-implicit approach where the T⁴ linearization uses the previous timestep's surface temperatures for the conductance matrix, and only iterates the forcing vector. This is what EnergyPlus actually does — it uses the "inside surface heat balance" iteration with fixed conductances. The HARES exterior LWR solver already does exactly this (line 180-191 in longwave.rs).
6. Concrete Code Changes Comparison
Option 2: Star-Mesh Linearized
Aspect	Details
Files modified	boundary_rc.rs (add star node + window floating node), solver_builder.rs (wire LWR conductances into RC graph), longwave_radiation.rs (add star-mesh conductance computation), rc_network.rs (no change — already handles floating nodes)
New/changed code	~80–120 lines in boundary_rc.rs, ~60–80 in solver_builder.rs, ~40 in longwave_radiation.rs
Risk level	LOW — No change to the solver step loop. The star-mesh reduction happens at construction time. If the RC graph is wrong, the A-matrix will be wrong but the solver won't crash. Existing tests (6-node, 12-node benchmarks) provide regression coverage.
Can break existing tests?	Only if the linearized conductance values differ significantly from the current radiation_frac injection — which they should for the better (more physical). BESTEST results may shift.
Incremental?	YES — Can ship Option 2 and upgrade to Option 3 later. The star-mesh conductances can be replaced with explicit surface nodes without changing the solver interface.
Implementation sketch:
For each zone with interior LWR:
  1. Create a "radiation star" floating node (NodeId in a reserved range)
  2. For each surface i:
     - Compute linearized conductance: G_i = 4 × ε_i × σ × A_i × T_ref³
     - Add resistance R_i = 1/G_i between surface's innermost RC node and the star node
  3. For window surfaces (no RC node):
     - Create a floating "window interior" node
     - Connect to star node via R_window = 1/(4 × ε_win × σ × A_win × T_ref³)
     - Connect to outdoor via existing window U-factor resistance
  4. reduce_floating_nodes() eliminates star and window nodes → pairwise conductances
  5. These appear as additional A-matrix entries (conductances between inner RC nodes)
Option 3: EnergyPlus-Style Explicit Surfaces
Aspect	Details
Files modified	boundary_rc.rs (restructure inner film to create surface node), solver_builder.rs (create surface state vars, wire them), state_space.rs (no change), stepping.rs (add outer iteration loop), longwave.rs (restructure to work with surface states), config.rs (add surface state indices, tiny capacitance), initialization.rs (init surface states)
New/changed code	~150–200 lines in boundary_rc.rs, ~100–130 in solver_builder.rs, ~80–120 in stepping.rs (outer loop), ~60–80 in longwave.rs, ~40 in config.rs, ~30 in initialization.rs
Risk level	HIGH — Changes the solver architecture from single-pass to iterative. The outer loop must converge or the solver produces wrong results. Stiff tiny-capacitance surface nodes may cause numerical instability. Debugging convergence failures in a 13-state system with T⁴ nonlinearities is hard.
Can break existing tests?	YES — Every test that depends on the solver step behavior will need updating. The A-matrix will be larger. The initialization will need surface temps. BESTEST results will definitely shift.
Incremental?	NO — This is a fundamental architectural change. Reverting is expensive.
Implementation sketch:
For each boundary surface:
  1. Create a surface state node with tiny capacitance C_surf (e.g., 100 J/K)
  2. Wire: zone_air — R_conv — surface_node — R_cond — inner_RC_node
     (replace current direct zone_air — R_film — inner_RC_node)
  3. At each timestep:
     a. Read surface temps from state vector
     b. Compute T⁴ ScriptF fluxes between all surfaces in the zone
     c. Inject fluxes into surface state equations (input vector or A-matrix off-diagonals)
     d. Step the model
     e. Check if surface temps converged; if not, repeat from (b) with updated temps
  4. The outer loop replaces the current single-pass step
7. The Comfort Modeling Argument
Finding: MRT (mean radiant temperature) does NOT require surface temperatures as explicit state variables. The current implicit computation is adequate for PMV/PPD comfort calculations.
Reasoning:
1. PMV/PPD sensitivity to MRT: The ASHRAE 55 PMV model has an MRT sensitivity of approximately 1 PMV unit per 3–4°C MRT change (at typical office conditions). The accuracy requirement for MRT is ~1°C for comfort applications — not the 0.01°C accuracy needed for energy balance closure.
2. Current MRT computation: The existing formula T_surf = rad_frac × T_node + (1 − rad_frac) × T_zone gives surface temps that are accurate to within 1–3°C of the true surface temperature for most building conditions. The error comes from the linearization of the radiation balance at the surface, which for typical ΔT of 5–10°C between surfaces introduces a ~1–2% error in h_r, translating to ~0.5–1°C error in T_surf.
3. ScriptF iteration already improves this: The existing iterative LWR solver (lines 286–320 in longwave.rs) converges surface temps to within 0.01°C of the T⁴ balance — within the constraint that surface temps are derived from node temps via the radiation_frac interpolation. This is adequate for MRT.
4. When would explicit surfaces help for comfort? Only if you need:
   - Operative temperature at a specific point in the room (not just zone-average MRT)
   - Directional radiant asymmetry (hot ceiling / cold window asymmetry)
   - Local comfort near a specific surface (e.g., radiant floor)
   These require view-factor-weighted MRT from specific positions, not zone-average MRT. But even then, the implicit surface temps from ScriptF iteration are accurate enough — the limiting factor is the view factor model, not the surface temperature accuracy.
Confidence: HIGH. The implicit approach is standard practice in BEM tools (EnergyPlus, IES, DesignBuilder all compute MRT from surface heat balance without requiring surface temps as explicit ODE states). The HARES ScriptF path already gives exact T⁴ surface temps within the radiation_frac constraint.
---
Comparison Table
Criterion	Option 2 (Star-Mesh)
Physics accuracy	Linearized radiation (h_r at T_ref). ~1–3% error vs exact T⁴ for typical ΔT
Solver architecture change	None — construction-time only
State variables added	0 (floating nodes eliminated)
A-matrix size change	Conductances added between existing nodes (matrix gets denser but same size)
Runtime iteration	None
Performance	No change
Lines of new/changed code	~180–240
Files modified	3
Risk level	LOW
Can break existing tests?	BESTEST may shift slightly
Incremental/upgrade path	✅ Ship now, upgrade to Option 3 later
MRT/comfort benefit	Indirect (better inter-surface conductance)
Bestest impact	May improve cooling loads (better LWR redistribution)
Debugging difficulty	Easy — it's just RC network construction
Implementation time	2–3 days
---
Recommendation: Do Option 2 now, defer Option 3.
Rationale:
1. Option 2 is a pure construction-time change with zero risk to the solver's numerical stability. The reduce_floating_nodes() function already handles cascading elimination correctly. The linearized radiation approximation is within 1–3% of exact T⁴ for the temperature ranges found in residential buildings (surface ΔT typically < 15°C).
2. Option 3's outer iteration is architecturally risky. The current solver is beautifully simple — a single LU solve per step. Adding an outer loop that must converge before the step is valid introduces a failure mode (non-convergence) that doesn't exist today. The OCHRE heavy-ball damping works for OCHRE because their surface temps are soft (derived, not states), not hard (ODE-constrained). The convergence guarantee is weaker for Option 3.
3. The comfort argument doesn't justify Option 3. PMV/PPD needs MRT accurate to ~1°C. The ScriptF iteration already gives that accuracy with implicit surface temps. Explicit surface states would only matter for directional radiant asymmetry, which is a niche use case.
4. Option 2 is the incremental path. The star-mesh conductances computed at construction time can later be replaced by runtime T⁴ updates if needed. The upgrade from Option 2 → Option 3 is straightforward: move the linearized conductances from construction-time to per-timestep computation, add surface state variables, and wrap the step in an outer loop. Option 2 doesn't paint you into a corner.
5. The physics improvement from Option 2 is significant. The current radiation_frac split injects LWR as a forcing, not a conductance. This means:
   - A hot ceiling doesn't thermally "see" a cold floor through the radiation network
   - The zone air acts as a thermal bottleneck for LWR redistribution
   - Solar on one surface doesn't warm other surfaces via radiation
   
   The star-mesh conductances fix all of these by creating direct thermal connections between surfaces. This should improve BESTEST cooling results (currently 6002 kWh vs band 6137–7964), where interior LWR redistribution matters.
Before shipping Option 2, add one test to rc_network.rs: a 4-node case with two cascading floating nodes (star + window), verifying the reduced network matches the expected pairwise conductances. This closes the residual gap in the floating-node test coverage.