# RC network discretization: node count, time constants, stability
**Review ID**: rc-mat-02
**Category**: envelope-rc-mat
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/rc_network.rs` (907 lines)
- `crates/hares-envelope/src/state_space.rs` (1758 lines)
- `crates/hares-envelope/src/boundary_rc.rs` (3326 lines) — contains `split_layer_count()` and `build_layered_boundary()`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/RCModel.py` (417 lines) — star-mesh floating node elimination, `create_rc_matrices`, same-zone boundary halving
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc` (937 lines read) — CTF/ConductionFD heat balance architecture

## Findings

### Finding 1: Same-zone boundary orientation reversed in `build_layered_boundary` [Severity: high]
**Description**: For same-zone (interior partition) boundaries, the material-layer construction path (`build_layered_boundary`) connects the cut (symmetry-plane) side of the construction to zone air via the exterior film resistance, while the actual interior-facing side is left dead-ended. The precomputed (OCHRE LUT) path (`build_precomputed_boundary`) correctly connects the interior-facing side to zone air and dead-ends the cut side. Both paths correctly clip the construction to the inner half and halve the middle layer capacitance for odd layer counts, but the wiring orientation in the material-layer path is reversed.

**Code Location**:
- `boundary_rc.rs:1236–1246` — same-zone clipping keeps interior half (`effective_layers = effective_layers[start..]` where `start = n - keep`). The slice retains exterior→interior ordering: `effective_layers[0]` is the cut point, `effective_layers[n-1]` is the original interior-most layer.
- `boundary_rc.rs:1271–1273` — cut-side wiring: `self.add_resistance(params.exterior_node, layer_nodes[0], r_ext)` connects zone air (via `exterior_node == interior_node` for same-zone) to `layer_nodes[0]` using `r_ext = r_film_exterior/area + thickness/(2*k*area)`. The exterior film resistance is spurious for an interior partition.
- `boundary_rc.rs:1297–1298` — interior-side wiring: `if !params.same_zone { ... }` skips the innermost-to-zone-air connection entirely, leaving `layer_nodes[n_layers-1]` (the original interior-facing layer) dead-ended.

**Root Cause**: The `build_layered_boundary` method always connects the exteriormost layer node to `exterior_node` (line 1273) and the innermost layer node to `interior_node` (line 1298–1320). For same-zone boundaries these two nodes are identical (same zone air), but the wiring assigns different film resistances to each side. The `!params.same_zone` guard on the interior wiring block disables the correct path, while the exterior wiring block (unguarded) uses the wrong film resistance.

The precomputed path avoids this by:
- Removing the first (exterior-side) resistor for same-zone (`res_list.remove(0)` at line 1410–1412)
- Skipping the exterior_node → layer[0] wiring (`!params.same_zone` guard at line 1461)
- Keeping the interior-side wiring intact with film folded in (lines 1470–1489)

**Impact**: The thermal mass of same-zone interior partitions couples to zone air through an incorrect resistance (exterior air-film of ~0.03 m²·K/W instead of interior film of ~0.12 m²·K/W). The stored energy discharge time constant is thus too small (~4×), causing internal mass to respond too quickly to zone air temperature changes. The actual interior-facing surface of the partition is thermally disconnected from the room, which is physically wrong — both sides of an interior partition should exchange heat with the room air.

### Finding 2: No upper bound on `split_layer_count` node count [Severity: medium]
**Description**: `split_layer_count()` at `boundary_rc.rs:81–96` computes `n = ceil(thickness / Λ)` using the half-penetration-depth criterion, but applies no maximum cap. While typical residential constructions produce 2–5 nodes (verified by tests at lines 2707–2760), extreme constructions could produce excessive node counts. For example, a 1.5 m thick earth-bermed concrete wall (k=1.13, ρ=1400, cp=1000, α=8.07e-7) would produce `ceil(1.5/0.0745) = 21` nodes, and 2 m would produce 27. The state-space dimension grows with each node, and the matrix exponential in `discretize_zoh` has O(n³) cost.

**Code Location**: `boundary_rc.rs:81–96` — `split_layer_count()` has no `n.max(1)` upper bound beyond the implicit `n.max(1)` floor.

**Root Cause**: The function was designed around the diurnal criterion with typical residential wall thicknesses in mind. A practical cap (e.g., 20 nodes, consistent with EN ISO 13786:2007 guidance that beyond ~6 nodes the benefit diminishes for hourly simulations) is missing.

**Impact**: For unusually thick walls (earth-sheltered, mass concrete), the state-space dimension could grow unexpectedly large, increasing memory (O(n²) for A_c, O(n³) for matrix exponential) and simulation time. No crash is expected, but performance could degrade noticeably. The review instructions specifically mention a target of "≤ 20 nodes for even the thickest concrete wall."

### Finding 3: Minimum RC time constant is sub-second at ~1 millisecond [Severity: low]
**Description**: The minimum possible RC time constant in the HARES RC network is τ_min = 1e-6 K/W × 1000 J/K = 0.001 s (1 ms). This arises from the clamped minimum resistance (`r.max(1e-6)` at `boundary_rc.rs:1153`) and minimum capacitance (`MIN_CAPACITANCE_J_K = 1000.0` at line 28). While this is far below typical simulation timesteps (60–3600 s), the ZOH discretization method (`discretize_zoh` at `state_space.rs:873`) is exact for linear systems and does not suffer from explicit Euler instability.

**Code Location**:
- `boundary_rc.rs:1153` — `let r = r.max(1e-6)` in `add_resistance` clamps minimum resistance
- `boundary_rc.rs:28` — `pub const MIN_CAPACITANCE_J_K: f64 = 1_000.0`
- `boundary_rc.rs:1260` — `let cap = halved.max(MIN_CAPACITANCE_J_K)` applies the floor before inserting capacitance

**Root Cause**: Design choice to prevent division-by-zero in the A-matrix assembly (`1/(R*C)`) and to bound the smallest eigenvalue of the continuous system. The resulting stiffness is absorbed by the unconditionally stable ZOH discretization.

**Impact**: None in terms of stability. The ZOH method's exact matrix exponential correctly handles arbitrarily stiff systems by producing A_d entries near 0 for fast modes. This is documented by the test at `state_space.rs:1449` (`zoh_no_overshoot_where_explicit_overshoots`) which shows ZOH correctly decaying a stiff (dt/τ = 600) system while explicit Euler overshoots. However, the near-zero A_d entries for sub-second modes mean those modes are effectively frozen — their temperatures stay nearly constant between timesteps because they decay to ambient in << 1 timestep. This is physically correct but may surprise users who expect all RC nodes to exhibit visible dynamics at the simulation timestep.

### Finding 4: Observability confirmed — all thermal mass connects to zone air [Severity: low — non-issue]
**Description**: Every thermal capacitance node in the RC network is connected to the zone air node through a chain of conductances. This is enforced by the network construction topology: `build_layered_boundary` creates chain-connected layer nodes and always connects at least one end to the zone air node (even for same-zone boundaries, the cut side connects to zone air via line 1273). `build_precomputed_boundary` similarly wires either the interior side or the exterior side to the zone. Additionally, `RCNetwork::from_elements` (line 100–109) validates that every classified node (capacitance or external) has at least one incident resistance, throwing `RCNetworkError::DisconnectedNode` if any capacitance-bearing node is orphaned.

**Code Location**:
- `boundary_rc.rs:1273` — same-zone: cut-side → zone air connection via `r_ext`
- `boundary_rc.rs:1298–1320` — non-same-zone: interior-side → zone air connection via `R_film + R_inner_half`
- `rc_network.rs:100–109` — disconnected node validation
- `rc_network.rs:220–276` — `reduce_floating_nodes` eliminates zero-capacitance nodes, merging their connections

**Impact**: No orphaned thermal mass exists. The solver tracks only nodes that affect zone air temperature. However, due to Finding 1, the coupling path for same-zone mass is through the wrong side of the construction.

### Finding 5: `split_layer_count` is deterministic and timestep-independent [Severity: low — non-issue]
**Description**: The `split_layer_count` function computes the number of RC sub-layers using only material properties (thermal diffusivity α = k/(ρ·cp)), the diurnal period (86,400 s), and the layer thickness. The function signature takes no timestep argument. This is appropriate for the ZOH solver, which is unconditionally stable and does not require timestep-dependent spatial discretization. The test at line 2763 confirms this property explicitly.

**Code Location**: `boundary_rc.rs:81–96`

**Root Cause**: By design. The Fourier criterion (`Fo = α·dt/thickness²`) was deliberately removed in favor of the diffusion-length criterion to decouple spatial accuracy from temporal stability (see docs at `docs/findings/rc_discretization.md`).

**Impact**: Same construction always produces the same number of RC nodes regardless of simulation timestep. This simplifies testing and ensures reproducibility.

## Summary
- Total findings: 5
- High: 1 (same-zone boundary orientation reversal)
- Medium: 1 (no upper bound on split_layer_count)
- Low: 3 (sub-second RC time constant, confirmed observability, deterministic splitting)

## Recommendations

1. **Fix same-zone wiring in `build_layered_boundary`** (Finding 1): Mirror the precomputed path's behavior — skip the exterior-side wiring and only connect the interior-facing side to zone air via the interior film resistance. Specifically:
   - Guard line 1273 (`self.add_resistance(params.exterior_node, layer_nodes[0], r_ext)`) with `!params.same_zone`
   - Remove the `!params.same_zone` guard on the interior wiring block (lines 1298–1321) so same-zone boundaries properly connect layer_nodes[n_layers-1] to the interior node via the interior film

2. **Add a practical cap to `split_layer_count`** (Finding 2): `n.min(20)` or a configurable maximum. Reference: EN ISO 13786:2007 notes that beyond ~6 nodes per construction the benefit in hourly/annual energy prediction is negligible; OCHRE caps at the number of material layers (no sub-layer splitting).

3. **Document stiffness expectations** (Finding 3): Add a comment near `MIN_CAPACITANCE_J_K` and the `r.max(1e-6)` clamping noting that τ_min ≈ 1 ms is intentional and absorbed by the ZOH exact discretization.

4. **Add topology-verification tests for same-zone boundaries** (Findings 1, 4): Existing tests (`same_zone_with_layers_creates_internal_mass`, `same_zone_odd_layers_halves_middle_capacitance`) verify node counts and capacitance values but not the resistance topology. Add a test that asserts the innermost layer node connects directly to zone air via the interior film resistance for same-zone boundaries.

## References / Citations
- Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer* §5.8 — penetration depth for semi-infinite solid with periodic surface temperature
- ISO 13786:2007 §6.2 — dynamic thermal characteristics using diffusion-length criterion
- OCHRE `RCModel.py:84–109` — `create_rc_matrices`, star-mesh floating node elimination, same-zone boundary halving
- OCHRE `RCModel.py:15–50` — `transform_floating_node` star-mesh transform
- EN ISO 52016-1:2017 Annex E — detailed RC method
- TRNSYS Type 56 — star network for inter-surface radiation
- EnergyPlus Engineering Reference §Inside Surface Heat Balance — Option 2 convective/radiative decomposition
- HARES `docs/findings/rc_discretization.md` — design rationale for diffusion-length criterion vs. Fourier criterion
