RC Discretization Shortcut Analysis — HARES split_layer_count()
1. Current split_layer_count() Logic
File: crates/hares-envelope/src/boundary_rc.rs
Key Constants (lines 44–51)
Constant	Value
DEFAULT_DT_S	3600.0
SPLIT_MIN_DENSITY	100.0
SPLIT_MIN_CONDUCTIVITY	0.1
Function (lines 57–78)
fn split_layer_count(
    thickness_m: f64,
    conductivity: f64,
    density: f64,
    specific_heat: f64,
    dt_s: f64,           // ← always passed as DEFAULT_DT_S = 3600
) -> usize {
    if density <= 0.0 || specific_heat <= 0.0 || conductivity <= 0.0 || thickness_m <= 0.0 {
        return 1;
    }
    let alpha = conductivity / (density * specific_heat);
    // C=3 → Fo = 1/C ≈ 0.33 (EnergyPlus CondFD default)
    let c_discretization = 3.0;
    let dx_max = (c_discretization * alpha * dt_s).sqrt();
    let n = (thickness_m / dx_max).ceil() as usize;
    n.max(1)
}
Call Site (lines 729–753)
The function is called from build_layered_boundary() with two guard conditions:
let needs_split = layer.density_kg_m3 > SPLIT_MIN_DENSITY      // ρ > 100
    && layer.conductivity_w_m_k > SPLIT_MIN_CONDUCTIVITY;       // k > 0.1
let n = if needs_split {
    split_layer_count(layer.thickness_m, ..., DEFAULT_DT_S)     // ← hardcoded 3600s
} else {
    1
};
What this means: Insulation (ρ < 100), air gaps, and very low-conductivity layers are never split. Dense, conductive layers (concrete, wood, gypsum) may be split based on the Fourier criterion at dt=3600s.
Numerical Results for BESTEST 900FF Materials
Layer	α (m²/s)
9mm wood siding	2.93e-7
61.5mm insulation	N/A
100mm concrete	3.64e-7
80mm floor concrete	8.07e-7
12mm gypsum	2.0e-7
The 900FF wall gets 4 RC nodes total: 1 (wood) + 1 (insulation) + 2 (concrete).
---
2. What EnergyPlus Does
Two Conduction Methods in EnergyPlus
1. Conduction Transfer Functions (CTF) — the default. This is an analytical method that computes heat flux from temperature histories using pre-computed transfer function coefficients. No spatial discretization is involved — the wall's frequency response is exact at all frequencies. This is what EnergyPlus uses for most surfaces in BESTEST.
2. Conduction Finite Difference (CondFD) — opt-in, for phase-change materials or variable-conductivity layers. Uses the Fourier criterion with SpaceDiscretizationConstant (default C=3) and the zone timestep (typically 600–1200s, not 3600s).
EnergyPlus CondFD Discretization (verified from source)
File: src/EnergyPlus/HeatBalFiniteDiffManager.cc, line 3735:
dxn = std::sqrt(Alpha * Delt * s_hbfd->SpaceDescritConstant);
Ipts1 = int(mat->Thickness / dxn);
if (Ipts1 <= 1) { Ipts1 = 1; }  // minimum 1 full-size node
Where Delt = TimeStepZoneSec (the zone timestep, typically 600s), and SpaceDescritConstant = 3 (default).
Key difference from HARES: EnergyPlus CondFD uses the zone timestep (600s), not a hardcoded 3600s. This produces 4 nodes for the 100mm concrete at dt=600s:
dt (s)	dx_max (m)
60	0.0081
600	0.0256
1200	0.0362
3600	0.0627
Important Caveat: EnergyPlus Uses CTF for BESTEST
EnergyPlus passes BESTEST using the CTF method (not CondFD). The CTF method has zero spatial discretization error — it's an exact solution of the 1D heat equation. This is why EnergyPlus can achieve high accuracy with no RC nodes at all: it doesn't use RC networks for standard surfaces.
HARES uses RC networks for all surfaces, so spatial discretization error is inherent in the method.
---
3. The Physically Correct Approach
Why the Current Criterion is a Shortcut
The current split_layer_count() has three issues:
Issue 1: Fourier stability criterion used for implicit solver accuracy.
The C=3 Fourier criterion (Fo = α·dt/Δx² ≤ 1/3) was designed for explicit finite difference stability. HARES uses an implicit ZOH state-space solver that is unconditionally stable. The comment at line 72–73 acknowledges this: "Our ZOH state-space solver is implicit and unconditionally stable, so this is a spatial accuracy criterion, not a stability requirement." But the formula itself remains the stability-oriented one.
Issue 2: Timestep-dependent spatial resolution.
The node count depends on dt_s, meaning the spatial resolution of the RC network changes with the simulation timestep. For an implicit solver, spatial accuracy should be independent of the timestep — the timestep affects temporal accuracy (how well the time evolution is tracked), while spatial accuracy depends on how well the temperature profile within each layer is represented by the lumped RC nodes.
Issue 3: Hardcoded DEFAULT_DT_S = 3600s doesn't match actual timestep.
The default SimulationConfig.time_res is 60s (see crates/hares-io/src/config.rs:51). But split_layer_count always uses 3600s. The BESTEST fixture uses time_res_s = 3600 (matching), but most other simulations use 60s. The RC network is assembled once at construction time and cannot adapt to the actual timestep.
Recommended Criterion: Diurnal Diffusion Length
For building energy simulation, the dominant forcing frequency is the diurnal cycle (P = 86400s). The physically correct spatial resolution target is to resolve the thermal wave at this frequency within each material layer.
The thermal diffusion length for a periodic excitation of period P is:
$$\Lambda = \sqrt{\frac{\alpha \cdot P}{4\pi}}$$
This represents the distance over which the diurnal thermal wave amplitude decays by ~60% (one e-folding of the spatially decaying envelope). To accurately resolve the thermal gradient, each node should span at most one diffusion length.
Proposed formula:
/// Reference period for spatial accuracy [s] — diurnal cycle.
const DIURNAL_PERIOD_S: f64 = 86400.0;
fn split_layer_count(
    thickness_m: f64,
    conductivity: f64,
    density: f64,
    specific_heat: f64,
    // dt_s parameter REMOVED — spatial resolution is timestep-independent
) -> usize {
    if density <= 0.0 || specific_heat <= 0.0 || conductivity <= 0.0 || thickness_m <= 0.0 {
        return 1;
    }
    let alpha = conductivity / (density * specific_heat);
    // Diffusion length at diurnal frequency:
    //   Λ = √(α × P / (4π))
    // Resolves the spatial thermal gradient at the dominant building forcing frequency.
    let diff_length = (alpha * DIURNAL_PERIOD_S / (4.0 * std::f64::consts::PI)).sqrt();
    let n = (thickness_m / diff_length).ceil() as usize;
    n.max(1)
}
Numerical Comparison: Current vs. Proposed
Layer	α (m²/s)
9mm wood siding	2.93e-7
61.5mm insulation	N/A
100mm concrete (wall)	3.64e-7
80mm concrete (floor)	8.07e-7
12mm gypsum	2.0e-7
200mm concrete (thick slab)	8.07e-7
300mm concrete (foundation)	8.07e-7
Key observation: For the BESTEST 900FF wall, the proposed criterion gives identical node counts (4 total). The only change for this case is the 80mm floor concrete, which goes from 1→2 nodes.
Why Λ = √(α·P/(4π)) instead of δ = √(α·P/π)
The standard penetration depth δ = √(α·P/π) is the distance for one full amplitude decay. The diffusion length Λ = √(α·P/(4π)) is shorter by a factor of 2, making the criterion 2× more conservative. This matches the current C=3/dt=3600s behavior for typical materials and provides adequate resolution for thick layers. Using δ directly would under-discretize thick concrete slabs.
---
4. Code Changes Required
Change 1: Replace split_layer_count() function
File: crates/hares-envelope/src/boundary_rc.rs, lines 43–78
- Remove DEFAULT_DT_S constant (line 44)
- Add DIURNAL_PERIOD_S constant
- Modify split_layer_count(): remove dt_s parameter, replace Fourier criterion with diffusion-length criterion
// REMOVE: const DEFAULT_DT_S: f64 = 3600.0;
/// Reference period for spatial accuracy [s] — diurnal cycle.
/// The RC spatial discretization resolves the thermal wave at this frequency,
/// which is the dominant forcing in building energy simulation.
const DIURNAL_PERIOD_S: f64 = 86400.0;
/// Compute the number of RC sub-layers needed for spatial accuracy.
///
/// Uses the thermal diffusion length at the diurnal frequency:
///   Λ = √(α × P / (4π))
/// Each node spans at most Λ, ensuring the thermal gradient is resolved.
/// This criterion is timestep-independent, which is correct for the
/// implicit ZOH state-space solver (spatial and temporal accuracy are decoupled).
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
    let diff_length = (alpha * DIURNAL_PERIOD_S / (4.0 * std::f64::consts::PI)).sqrt();
    let n = (thickness_m / diff_length).ceil() as usize;
    n.max(1)
}
Change 2: Update call site
File: crates/hares-envelope/src/boundary_rc.rs, lines 735–741
Remove DEFAULT_DT_S argument from the call:
// BEFORE:
split_layer_count(
    layer.thickness_m,
    layer.conductivity_w_m_k,
    layer.density_kg_m3,
    layer.specific_heat_j_kg_k,
    DEFAULT_DT_S,
)
// AFTER:
split_layer_count(
    layer.thickness_m,
    layer.conductivity_w_m_k,
    layer.density_kg_m3,
    layer.specific_heat_j_kg_k,
)
Change 3: Update unit tests
File: crates/hares-envelope/src/boundary_rc.rs, lines 1857–1891
Remove dt_s argument from all split_layer_count calls in tests:
// BEFORE:
let n = split_layer_count(0.100, 0.51, 1400.0, 1000.0, 3600.0);
// AFTER:
let n = split_layer_count(0.100, 0.51, 1400.0, 1000.0);
Similarly for lines 1870, 1882, 1889, 1951, 2032, 2069.
Change 4: Update root cause test assertions
File: crates/hares-envelope/tests/bestest_900ff_root_cause.rs
The heavyweight_concrete_wall_produces_only_four_rc_nodes test (line 166) asserts n_nodes == 4. With the proposed criterion, this remains 4 (1 wood + 1 insulation + 2 concrete). No change needed.
The lightweight_wall_produces_three_rc_nodes test (line 226) asserts n_nodes == 3. With the proposed criterion, this remains 3 (1 wood + 1 insulation + 1 gypsum). No change needed.
---
5. Impact on Other Simulation Cases Beyond BESTEST 900FF
Cases Where the Change Improves Accuracy
Scenario	Current
Thick slabs (200–300mm concrete)	3–4 nodes
Floor concrete (80mm, k=1.13)	1 node
Simulations at dt ≠ 3600s	Spatial resolution based on 3600s regardless
Foundation walls with thick concrete	Under-resolved
Cases Where the Change Has No Effect
Scenario	Current
BESTEST 900FF walls	2 concrete nodes
BESTEST 600FF walls	1 gypsum node
Thin layers (< diffusion length)	1 node
Insulation / air gaps	1 node (guard)
Precomputed RC path (OCHRE LUT)	Unaffected
Cases Where the Change Could Affect Performance
Scenario
Very thick concrete (400mm+ foundation)
Large buildings with many concrete boundaries
Overall
Performance Estimate
For a typical residential building:
- Current: ~15–25 state variables (1 zone + ~15–24 layer nodes)
- Proposed: ~16–26 state variables (1 extra node for floor concrete)
- State vector increase: ~4%
- ZOH matrix exponential cost: O(n³) → ~12% increase in matrix operations
- This is within noise for hourly simulation of a single building
---
6. How to Implement Without Breaking Existing Tests
Step-by-Step Implementation Plan
1. Modify split_layer_count() function: Remove dt_s parameter, replace criterion with diffusion-length formula. This is a private function, so no public API change.
2. Remove DEFAULT_DT_S constant: No longer referenced.
3. Update call site: Remove DEFAULT_DT_S argument from line 740.
4. Update unit tests: Remove dt_s / DEFAULT_DT_S arguments from all test calls. Verify assertions still pass:
   - split_layer_count_concrete_100mm_hourly: n=2 (unchanged) ✓
   - split_layer_count_thick_concrete_200mm: n=3 (unchanged) ✓
   - split_layer_count_insulation_no_split: n=1 (unchanged) ✓
   - split_layer_count_thin_wood_no_split: n=1 (unchanged) ✓
5. Run full test suite:
      cargo test -p hares-envelope
   cargo test -p hares-core -- bestest
   cargo test -p hares-core -- oracle
   
6. Verify BESTEST results unchanged: The 900FF min temperature should remain at ~0.90°C (the wall discretization is unchanged). The floor concrete going from 1→2 nodes will produce a negligible change (<0.05°C) because the floor is heavily insulated (R=25.2 m²·K/W).
7. Add a new unit test for the diffusion-length criterion:
      #[test]
   fn split_layer_count_diurnal_criterion_thick_concrete() {
       // 300mm concrete foundation wall should get 5 nodes
       let n = split_layer_count(0.300, 1.13, 1400.0, 1000.0);
       assert!(n >= 4, "300mm concrete should get >=4 nodes, got {n}");
   }
   
   #[test]
   fn split_layer_count_timestep_independent() {
       // Same material should give same node count regardless of how it's called
       // (the function no longer takes dt_s, so this is enforced by the type system)
       let n = split_layer_count(0.100, 0.51, 1400.0, 1000.0);
       assert_eq!(n, 2, "100mm concrete always gets 2 nodes");
   }
   
Test Modifications Required
The following existing tests need their split_layer_count call signatures updated (removing the dt_s argument):
Test
split_layer_count_concrete_100mm_hourly
split_layer_count_thick_concrete_200mm
split_layer_count_insulation_no_split
split_layer_count_thin_wood_no_split
r_zone_to_inner_uses_post_split_thickness
split_outer_layer_half_r_reflects_post_split_thickness
split_inner_layer_half_r_reflects_post_split_thickness
All assertion values remain unchanged because the proposed criterion produces identical node counts for these materials.
---
7. Classification: Shortcut or Bug?
Classification: Shortcut (not a bug)
Reasoning:
1. Not a bug: The code deliberately implements the EnergyPlus CondFD formula. The comment at line 68–73 explicitly references the EnergyPlus Engineering Reference and acknowledges that the criterion is for spatial accuracy, not stability. The implementation produces correct results for the BESTEST timestep (3600s).
2. It is a shortcut: The implementation copies the explicit-FD formula without adapting it to the fundamentally different solver approach:
   - EnergyPlus CondFD uses an explicit or semi-implicit finite difference scheme where the Fourier criterion controls stability → timestep-dependent node count is physically necessary
   - HARES uses a ZOH state-space solver that is unconditionally stable → spatial accuracy should be timestep-independent
   - The hardcoded DEFAULT_DT_S = 3600s is an engineering convenience, not a physics requirement
   - The formula's dependency on dt_s means the RC network's spatial resolution varies with the simulation timestep, which is physically incorrect for an implicit solver
3. The shortcut is low-impact for current use: The BESTEST fixture uses time_res_s = 3600, so the hardcoded value matches. The measured impact on 900FF is <0.03°C. Most residential buildings have thin enough mass layers that 1–2 nodes suffice.
4. The shortcut is consequential for generalizability: For simulations at non-standard timesteps, or for buildings with very thick thermal mass (foundation walls, thermal storage), the current criterion may under-resolve the spatial temperature profile. The proposed fix makes the criterion physically robust regardless of timestep or building type.
Severity Assessment
Dimension	Rating
Physical correctness	Medium
Numerical impact (BESTEST)	Negligible
Numerical impact (general)	Low–Medium
Code quality	Low
Risk of fix	Very Low
---
## 8. Summary of Recommendations
1. **Replace the timestep-dependent Fourier criterion** with the timestep-independent diurnal diffusion-length criterion: `Λ = √(α × 86400 / (4π))`.
2. **Remove the `DEFAULT_DT_S` constant** and the `dt_s` parameter from `split_layer_count()`. Spatial and temporal accuracy should be decoupled for an implicit solver.
3. **Keep the existing guards** (`SPLIT_MIN_DENSITY`, `SPLIT_MIN_CONDUCTIVITY`) — they correctly prevent splitting of insulation and air gaps.
4. **No changes to the precomputed RC path** (OCHRE LUT) — this is a separate code path with its own node assignment.
5. **Add documentation** explaining why the diurnal diffusion length is the appropriate criterion for an implicit ZOH solver, and how it differs from the EnergyPlus CondFD Fourier criterion.
6. **Run full BESTEST suite** after implementation to confirm no regressions. The expected change is zero for 900FF (wall nodes unchanged) and <0.05°C from the floor concrete gaining a node.
---
References
1. EnergyPlus CondFD source: src/EnergyPlus/HeatBalFiniteDiffManager.cc, line 3735 — dxn = std::sqrt(Alpha * Delt * SpaceDescritConstant) — confirms C=3 default and zone-timestep dependency.
2. EnergyPlus Engineering Reference §3.3.10: "Conduction Finite Difference Solution Algorithm" — documents C=3 as the space discretization constant (inverse of Fourier number).
3. Incropera & DeWitt, "Fundamentals of Heat and Mass Transfer" — defines thermal penetration depth δ = √(αt/π) for semi-infinite solid with periodic boundary condition.
4. ISO 13786:2007 "Thermal performance of building components — Dynamic thermal characteristics" — defines periodic thermal transmittance and the use of RC networks to approximate frequency response.
5. Clarke, J.A. (2001), "Energy Simulation in Building Design" — discusses spatial discretization requirements for RC networks in building simulation, recommending at least 2 nodes per layer for layers with significant thermal mass.
6. BESTEST 900FF root cause analysis (docs/bestest-900ff-root-cause-analysis.md) — empirically rules out RC under-discretization as the cause of the 900FF outlier (+0.03°C wrong direction from 2→4 node refinement).