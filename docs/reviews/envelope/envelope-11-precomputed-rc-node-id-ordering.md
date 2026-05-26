# Precomputed RC node ID ordering fragile across solver configurations
**Review ID**: envelope-11
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/rc_network.rs`
- `crates/hares-envelope/src/boundary_rc.rs`
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` (lines 336–410, 847–899, 972–1033, 1048–1061, 1104–1150)
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc` (lines 493–540, 5232–5350, 8654)
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalFiniteDiffManager.cc` (lines 843–893)

## Findings

### Finding 1: [Severity: critical] `node_index` and matrix row ordering derived from two independently-sorted code paths with zero cross-validation

**Description**: The `node_index: HashMap<NodeId, usize>` (the authoritative mapping from physical node to state-vector row consumed by the solver, energy-balance checker, and surface wiring) is built by independently re-sorting `rc.capacitances.keys()` in `assemble_building_rc()` — while the A_c and B_c matrices are populated by a separate sort in `build_matrices()`. These two sorts use **different predicates** and are not validated against each other. The correspondence is coincidental, guaranteed only by the invariant that external nodes never appear in the capacitance map.

**Code Location**:
- Matrix construction: `rc_network.rs:139–183` — `build_matrices()` calls `sorted_internal_nodes()` which filters `external_nodes` from the key set before sorting
- `node_index` construction: `boundary_rc.rs:1082–1090` — raw `rc.capacitances.keys()` collected and sorted with no external-node filter
- `sorted_internal_nodes`: `rc_network.rs:206–218` — filters via `!external.contains(node)` then sorts
- No cross-validation exists anywhere in the codebase between the two orderings

**Root Cause**: The two paths accidentally agree because `RCNetwork::from_elements()` (rc_network.rs:59–137) validates that external nodes have no capacitances (line 75–77), so external nodes are absent from `rc.capacitances`. But this invariant is enforced only at construction time, via a different error path (`NodeInInternalAndExternal`), not by a shared sort predicate. If a future modification to the NodeId allocation scheme introduces an ID range that could overlap external nodes into capacitances (e.g., if `LAYER_NODE_BASE` were changed or a new floating-node reduction path created nodes with external-range IDs), `sorted_internal_nodes` would silently drop those nodes from the state vector while `node_index` would still map them. The resulting cardinality mismatch (`node_index.len() > a_c.nrows()`) would cause the energy-balance check in `stepping.rs:676–686` to silently skip nodes whose index exceeds `self.x.len()`.

**Impact**: Silent wrong results. If the row ordering between `node_index` and A_c/B_c ever diverges, the solver would:
- Apply solar/LWR excitation to the wrong thermal node (via `ExteriorSurfaceInfo::state_index` and `input_index` in `solver_builder.rs:695–704`)
- Route interior LWR fluxes to the wrong surface nodes (via `InteriorSurfaceInfo::state_index` in `solver_builder.rs:779–788`)
- Compute energy balance on wrong capacitances (via `stepping.rs:678–685`)

The existing test `state_row_order_is_deterministic_across_hashmap_insertion_order` (rc_network.rs:445–467) validates that different HashMap insertion orders produce identical matrices — but it only tests consistency of `build_matrices()`, not consistency between `build_matrices()` row ordering and the independently-computed `node_index`.

### Finding 2: [Severity: high] Missing cross-validation of `node_index` ↔ `a_c.nrows()` cardinality

**Description**: There is no assertion anywhere that `node_index.len() == a_c.nrows()`. The `node_index` HashMap is the source of truth for NodeId→row mappings consumed in `stepping.rs` (energy balance), while `a_c.nrows()` determines the actual state vector dimension. If these cardinalities diverge, the energy balance check silently skips mismatched nodes via the bounds-guarded `if idx < self.x.len()` check at `stepping.rs:680`.

**Code Location**:
- `node_index` construction: `boundary_rc.rs:1086`
- A_c construction: `rc_network.rs:162` — `DMatrix::zeros(internal_nodes.len(), ...)`
- Energy balance consumption: `stepping.rs:676–686`
- No `debug_assert_eq!(node_index.len(), a_c.nrows())` exists at any join point

**Root Cause**: The two data structures are produced by different functions and never reconciled. `build_matrices()` returns `(A_c, B_c)` with `nrows = internal_nodes.len()`, while `assemble_building_rc()` separately recomputes the sorted node list and `node_index`. The compiler cannot enforce parity between the dimension of a nalgebra matrix and the length of a HashMap.

**Impact**: If the cardinalities diverge, the solver silently degrades without any error, log message, or detectable symptom other than physically incorrect results. The energy balance closure in `stepping.rs:682` would compute `c_j_k * ΔT / dt` for the wrong node's state row or skip nodes entirely, yielding a plausible-looking but wrong stored energy total.

**Comparison with vendor references**:
- OCHRE (`Envelope.py:847–870`) uses string-based lookups (`state_names.index("T_" + surface.node)`) and leaves `t_idx = None` when a node name is not found. It then asserts `t_idx >= 0` at every use site (e.g., lines 1104–1105), failing fast at runtime.
- EnergyPlus uses direct 1:1 surface-number indexing (e.g., `SurfInsideTempHist(Term)(SurfNum)` in `DataHeatBalSurface.hh:211–220`), so the mapping is structurally guaranteed by the array layout — no validation needed.

### Finding 3: [Severity: high] Surface-to-node wiring silently skips surfaces on lookup failure

**Description**: When `build_solver_boundaries()` resolves the `outer_wiring` and `inner_wiring` for each exterior/interior surface, it uses `.and_then()` chains that silently produce `None` when a NodeId is missing from either `layer_info` or `node_index`. A surface without `outer_wiring` receives zero exterior solar/LWR injection with no diagnostic. This is the "silently incorrect results" failure mode described in the review's problem statement.

**Code Location**:
- `solver_builder.rs:156–163` — `outer_wiring` construction chain: `rc.layer_info.get(&surface_idx).and_then(|info| rc.node_index.get(&info.outer_node))`
- `solver_builder.rs:169–176` — `inner_wiring` construction chain: same pattern for `info.inner_node`
- Fallback when wiring is `None`: `solver_builder.rs:695–704` — falls back to zone air state/index, meaning the surface injection goes to the wrong node entirely
- No warning, error, or `debug_assert!` is emitted on missing lookup

**Root Cause**: The `.and_then()` combinators produce `Option::None` as a valid "this surface has no material-layer RC node" signal (e.g., for fallback-R boundaries). But they cannot distinguish between "intentionally no node" and "node exists but mapping is broken." A broken mapping silently collapses into the same `None` branch, treating the surface as if it has no RC representation.

**Impact**: In the scenario described in the review prompt — "different zone ordering, different surface enumeration causes stale node ID mapping" — a surface could receive the wrong boundary condition. Exterior solar absorbed by the wrong node would heat the wrong thermal mass; LWR exchange would flow between the wrong node pairs. The results would be physically plausible but wrong, detectable only by comparison to a golden reference.

**Comparison with vendor references**:
- OCHRE (`Envelope.py:1104–1105`): `for i, t_idx in enumerate(self._ext_t_idxs): assert t_idx >= 0` — fails fast.
- OCHRE (`Envelope.py:1150`): `assert zone.h_idx == surface.h_idx` — validates the zone-to-surface index correspondence.
- HARES: silently skips, no assertion, no log.

### Finding 4: [Severity: medium] `SurfaceLayerInfo` NodeIds never validated against the internal-node set

**Description**: The `SurfaceLayerInfo` struct (boundary_rc.rs:284–306) stores `inner_node` and `outer_node` NodeIds that are used downstream via `node_index` to resolve state-vector rows. While `inner_node` has an assertion guarding against collision with the reserved external-node ID range (boundary_rc.rs:597–602, 703–708), neither `inner_node` nor `outer_node` is validated to be present in the `capacitances` map or the `node_index` set. If node allocation logic changes and a `SurfaceLayerInfo` NodeId refers to a node that was eliminated during floating-node reduction, the lookup chain `node_index.get(&info.inner_node)` silently produces `None`.

**Code Location**:
- `SurfaceLayerInfo` definition: `boundary_rc.rs:284–306`
- `inner_node` range assertion (only guard): `boundary_rc.rs:597–602`
- Consumption in solver builder: `solver_builder.rs:157–176`
- No assertion that `layer_info[bd_idx].inner_node ∈ node_index.keys()` exists

**Root Cause**: Node allocation is sequential (`next_layer_id` counter), and the assertion only guards against overflow into the external-node reserved range. There is no post-construction validation that the nodes referenced by `SurfaceLayerInfo` survived the floating-node reduction pass. (Currently they always survive because surface nodes are floating and inner/outer nodes have capacitance, but this is architectural coupling, not a validated invariant.)

**Impact**: If a future code change causes an inner or outer node to be eliminated (e.g., a material layer with zero capacitance), the surface would silently lose its wiring, degrading to zone-air injection for both exterior and interior paths. This would produce physically incorrect results.

## Summary
- Total findings: 4
- Critical / High / Medium / Low: 1 / 2 / 1 / 0

## Recommendations

1. **Add a `debug_assert_eq!(node_index.len(), a_c.nrows())` in `assemble_building_rc()`** immediately after building `node_index` (boundary_rc.rs:1090). This catches the cardinality mismatch at development time with zero runtime cost.

2. **Unify the sort predicate** between `sorted_internal_nodes()` and the `node_index` construction, or better, have `build_matrices()` return the sorted internal node list alongside the matrices so there is a single source of truth:
   ```rust
   pub fn build_matrices(&self) -> Result<(DMatrix<f64>, DMatrix<f64>, Vec<NodeId>)>
   ```
   Then `assemble_building_rc()` should consume this returned list rather than re-computing it.

3. **Add `debug_assert!` in `build_solver_boundaries()`** (solver_builder.rs) that surfaces with `layer_info` entries successfully resolve through `node_index`:
   ```rust
   debug_assert!(outer_wiring.is_some(), "surface {surface_idx}: outer_node missing from node_index");
   ```

4. **Validate `SurfaceLayerInfo` completeness** at the end of `assemble_building_rc()`. For every `SurfaceLayerInfo` entry, assert that `inner_node` and `outer_node` are present in `rc.capacitances`:
   ```rust
   debug_assert!(rc.capacitances.contains_key(&info.inner_node));
   debug_assert!(rc.capacitances.contains_key(&info.outer_node));
   ```

5. **Consider a strong-type approach**: Replace `NodeId(u32)` with a newtype that encodes whether the node is internal (capacitance-bearing) or external (driving-temperature), and have `node_index` use a type that guarantees the contained NodeIds are internal. This makes the invariant compiler-enforced rather than comment-enforced.

## References / Citations
- OCHRE Envelope.py:847–870 — string-based `state_names.index()` lookup for surface-to-state mapping, followed by `assert t_idx >= 0` at each use site
- OCHRE Envelope.py:336–410 — `Boundary.__init__` node naming: sequential labels per boundary, exterior-to-interior ordering
- OCHRE Envelope.py:1104–1105 — `for i, t_idx in enumerate(self._ext_t_idxs): assert t_idx >= 0`
- OCHRE Envelope.py:1150 — `assert zone.h_idx == surface.h_idx`
- EnergyPlus DataHeatBalSurface.hh:211–220 — history term arrays indexed by `(HistTerm, SurfNum)` — structurally guaranteed mapping
- EnergyPlus HeatBalFiniteDiffManager.cc:843–893 — per-surface independent node arrays, no cross-surface ordering dependency
- EnergyPlus HBSurfaceManager.cc:493–540 — CTF constant part calculation using `SurfNum` as direct index
