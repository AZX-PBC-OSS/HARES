# StarMesh cascading elimination edge case for zero-capacitance layers
**Review ID**: envelope-09
**Category**: envelope
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-envelope/src/rc_network.rs` (907 lines)
- `crates/hares-envelope/src/boundary_rc.rs` (3326 lines)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` (1103+ lines)
- `vendors/OCHRE/ochre/Models/RCModel.py` (417 lines) — `transform_floating_node` and `reduce` logic
- `vendors/OCHRE/ochre/utils/envelope.py` (line 294: `create_rc_data`) — zero-capacitance layer fusion algorithm
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalance*.cc` — CTF/FiniteDiff conduction methods (no star-mesh)
- `vendors/EnergyPlus/src/EnergyPlus/Construction.cc`, `Material.cc` — layer property handling

## Findings

### Finding 1: [Severity: medium]
**Description**: The `reduce_floating_nodes` function in `rc_network.rs:220-276` is numerically safe against division-by-zero in the star-mesh (Y-Δ) transform denominator. The denominator `sum_g` (line 257) is the sum of conductances `1/r` for all resistances incident on the floating node. Since `RCNetwork::from_elements` rejects non-positive resistances at construction time (`rc_network.rs:86`), and `RcGraphState::add_resistance` in `boundary_rc.rs:1152` applies `r.max(1e-6)`, all resistances are strictly positive and finite. No division-by-zero path exists.

**Code Location**: `rc_network.rs:256-262` (star-mesh denominator)

**Root Cause**: The review question presupposes the denominator involves a capacitance value, but the star-mesh formula uses the sum of admittances (`Σ 1/R_ij`), not a capacitance. Zero-capacitance nodes never appear in the floating-node pool in the first place because they are either (a) fused before node creation (precomputed path), (b) clamped to a minimum capacitance (material-layer path), or (c) intentionally created as floating surface/star nodes with zero capacitance for correct elimination (StarMesh LWR mode).

**Impact**: No runtime failure. StarMesh cascading elimination produces numerically well-conditioned results for all valid construction inputs.

**Comparison with OCHRE**: OCHRE's `transform_floating_node` in `RCModel.py:15-50` uses `r_parallel = RCModel.par(*adj_resistors.values())` (parallel resistance) as denominator, which is mathematically identical to HARES's `sum_g` approach. OCHRE also checks for zero-resistance branches before the star-mesh transform (line 21-34). Both implementations are equivalent and safe.

---

### Finding 2: [Severity: medium]
**Description**: The precomputed-RC path (`build_precomputed_boundary` in `boundary_rc.rs:1392-1407`) guards against zero-capacitance layers by fusing them with adjacent resistors *before* RC network construction. This is Step 3 of the OCHRE `create_rc_data` algorithm. However, the guard uses exact floating-point equality: `if cap_list[i] == 0.0` (line 1396). While precomputed LUT values are constants and should compare exactly, this pattern may silently fail to fuse a layer whose capacitance is exactly representable as a float but originates from a calculation path (e.g., `0.1 - 0.1` may produce `2.78e-17` rather than exactly `0.0` due to floating-point error). If a near-zero but non-zero capacitance value slips through, a physically negligible capacitance (~1e-17 kJ/m²-K) would create a node with `MIN_CAPACITANCE_J_K` (1000 J/K) minimum clamping in the subsequent scaling step (line 1424). This would not crash but would add a spurious capacitance node.

**Code Location**: `boundary_rc.rs:1396`

**Root Cause**: Exact equality comparison on floating-point values from a computation chain, rather than an epsilon-gated check.

**Impact**: Low. With current LUT data (constant values read from a database), exact zero comparisons are sufficient. Risk manifests only if a future derived-value path feeds non-exact-zero capacitances into precomputed layers. The resulting error would be a single spurious 1000 J/K node in an otherwise correct network — a small capacitance error, not a crash.

**Comparison with OCHRE**: OCHRE uses the same exact-equality pattern in `create_rc_data` at `envelope.py:326`: `while any([c == 0 for c in cap_list])`. Both HARES and OCHRE share this fragility.

---

### Finding 3: [Severity: low]
**Description**: The raw material-layer path (`build_layered_boundary` in `boundary_rc.rs:1201-1335`) creates capacitor nodes for every layer in the construction, including layers that physically have no thermal mass (e.g., air gaps with `density_kg_m3 ≈ 0`, `specific_heat_j_kg_k ≈ 0`). The node capacitance is clamped to `MIN_CAPACITANCE_J_K = 1000 J/K` (line 1260). This prevents floating-node classification (the node has capacitance > 0) and avoids star-mesh elimination altogether, but introduces a physically inaccurate thermal capacitance for resistance-only layers.

**Code Location**: `boundary_rc.rs:1253-1261` (capacitance computation and clamping)

**Root Cause**: The material-layer path does not have a zero-capacitance fusion step analogous to Step 3 of `build_precomputed_boundary`. Layers are filtered only by `conductivity > 0` and `thickness > 0` at `boundary_rc.rs:647`, with no density or specific-heat check.

**Impact**: Low. Air gaps in typical residential construction have negligible thermal mass compared to the zone air node (500,000+ J/K). A 1000 J/K spurious node represents <0.2% of total zone capacitance and has no measurable effect on annual heating/cooling loads. The node's existence does not cause numerical instability because it has non-zero capacitance and participates in the A-matrix normally.

**Comparison with OCHRE**: OCHRE eliminates zero-capacitance layers *before* node creation via `create_rc_data` (envelope.py:326-332). HARES matches this for precomputed layers but diverges for raw material layers, which have no pre-processing step. Adding a density or specific-heat guard to the `valid_layers` filter at `boundary_rc.rs:644-648` would align the paths.

---

### Finding 4: [Severity: low]
**Description**: The `reduce_floating_nodes` function at `rc_network.rs:269` computes `r_new = 1.0 / g_new` where `g_new = (1.0 / r_if) * (1.0 / r_jf) / sum_g`. If `sum_g` is extremely small (all adjacent resistances are very large, e.g., a floating node connecting only through high-R radiation resistances in the GΩ range), `g_new` becomes large and `r_new` drops toward zero. Resistance values approaching zero could, in an extreme edge case, violate the implicit invariant that all resistances are > 0 after elimination. The current code does not re-validate resistance positivity after the star-mesh transform.

**Code Location**: `rc_network.rs:258-270`

**Root Cause**: The star-mesh transform does not apply a minimum resistance guard to the newly computed pairwise resistances. When `sum_g` approaches zero, the new pairwise conductance approaches infinity.

**Impact**: Very low (theoretical only). For this to occur, a floating node would need adjacent resistances in the teraohm range, which requires physically impossible constructions (e.g., 10 m of aerogel with 1 m² area). All practical building materials produce resistances in the 0.001-100 K/W range (absolute), ensuring `sum_g` stays above machine epsilon. The existing `add_resistance` guard at `boundary_rc.rs:1153` applies `r.max(1e-6)` to resistances added to the graph, but the star-mesh computed `1.0 / g_new` at `rc_network.rs:268` bypasses this guard before insertion into the HashMap.

**Comparison with OCHRE**: OCHRE's `transform_floating_node` computes `r_new = r1 * r2 / r_parallel`. If `r_parallel` approaches zero (all adjacent resistances approach zero), `r_new` grows without bound (infinite resistance, not zero). This is the inverse of HARES's formulation — OCHRE's edge case produces impractically large resistances while HARES's produces impractically small ones. Neither causes a crash in practice but OCHRE's formulation is technically safer because an open circuit (R → ∞) is physically harmless while a short circuit (R → 0) in an RC network can cause the solver to produce extreme temperature swings in the affected node.

**Recommendation**: Apply `r_new.max(1e-6)` after computing `1.0 / g_new` at `rc_network.rs:268`, consistent with the guard at `boundary_rc.rs:1153`.

---

## Summary
- **Total findings**: 4
- **Critical**: 0
- **High**: 0
- **Medium**: 2
- **Low**: 2

## Recommendations

1. **Apply minimum resistance guard after star-mesh transform** (Finding 4). Change `rc_network.rs:268` from `resistances.insert(edge, 1.0 / g_new)` to `resistances.insert(edge, (1.0 / g_new).max(1e-6))` for defence-in-depth.

2. **Use epsilon-gated zero-capacitance check in precomputed path** (Finding 2). Change `boundary_rc.rs:1396` from `cap_list[i] == 0.0` to `cap_list[i].abs() < f64::EPSILON` (or equivalent) to guard against floating-point artifacts.

3. **Add zero-capacitance fusion for raw material layers** (Finding 3). In `build_layered_boundary`, add a pre-pass that merges layers where `density * specific_heat <= 0` into adjacent resistive layers before node creation, analogous to Step 3 of the precomputed path. Alternately, filter such layers from `valid_layers` and add their thermal resistance to adjacent layers.

4. **Add a test for raw material air-gap layers** (Finding 3). A test case with a layer where `density = 0, specific_heat = 0, conductivity > 0, thickness > 0` should verify that no capacitor node is created and the layer's resistance is correctly folded into adjacent connections.

## References / Citations

- OCHRE `transform_floating_node`: `vendors/OCHRE/ochre/Models/RCModel.py:15-50`
- OCHRE `create_rc_data` zero-capacitance fusion: `vendors/OCHRE/ochre/utils/envelope.py:326-332`
- OCHRE `linearize_int_radiation` star-mesh mode: `vendors/OCHRE/ochre/Models/Envelope.py:1048-1061`
- EnergyPlus CTF method (no star-mesh): `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:2617-2619`, `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc:5282-5293`
- Star-mesh (Y-Δ) transform reference: Wikipedia "Star-mesh transform"; TRNSYS 18 Vol.5 §5.8.2.3
- ASHRAE 140-2017 BESTEST: §5.3.1.9 (ε_ir = 0.9 for interior surfaces)
- ISO 13786:2007 §6.2 (dynamic thermal characteristics, diurnal penetration depth)
- Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer*, §5.8
