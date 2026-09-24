# Defensive `assert_ne!` for `inner_node` NodeId Construction

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-envelope/boundary_rc

## Problem

The `inner_node` NodeId formula in `crates/hares-envelope/src/boundary_rc.rs` constructs node IDs without a defensive assertion against the reserved IDs `OUTDOOR_NODE_ID` and `GROUND_NODE_ID`. A configuration that produces a colliding inner-node ID would silently overwrite the outdoor or ground boundary node, corrupting the assembled state-space matrix without any diagnostic.

The formula is presumably correct today, but the guarantee is by inspection rather than by assertion. A future refactor to the NodeId scheme can break the guarantee silently.

## Current Behavior

`crates/hares-envelope/src/boundary_rc.rs` (line ranges per Review 02 F12):
```rust
let inner_node = NodeId(...);  // no assert against OUTDOOR_NODE_ID / GROUND_NODE_ID
```

If the formula ever yields `OUTDOOR_NODE_ID` or `GROUND_NODE_ID` for some surface configuration, the matrix assembly silently produces a wrong topology.

## Required Behavior

Add `debug_assert_ne!(inner_node, OUTDOOR_NODE_ID)` and `debug_assert_ne!(inner_node, GROUND_NODE_ID)` immediately after the inner-node construction. In release builds the assert compiles out; in debug builds and tests, any collision panics with a clear message.

For maximum safety, prefer a non-debug `assert_ne!` if the formula is on a cold path (e.g. solver init) where the assert cost is negligible.

## Approach

1. Locate the `inner_node` construction site in `crates/hares-envelope/src/boundary_rc.rs`. The Review 02 F12 reference cites the formula but not the line numbers; grep for `inner_node = NodeId` or similar.
2. Add `assert_ne!` (init/cold path) or `debug_assert_ne!` (hot path) checks against both reserved IDs.
3. Add a unit test constructing a surface configuration that would have collided under a hypothetical broken formula, asserting the panic.

## Definition of Done

- [ ] `inner_node` construction site protected by `assert_ne!` or `debug_assert_ne!` against `OUTDOOR_NODE_ID` and `GROUND_NODE_ID`
- [ ] Unit test exercises the assertion path
- [ ] No production callsite can silently alias an inner node onto a reserved ID

## Verification

```bash
cargo test -p hares-envelope boundary_rc
```

## References

- HARES `crates/hares-envelope/src/boundary_rc.rs` — `OUTDOOR_NODE_ID` and `GROUND_NODE_ID` constants and `inner_node` construction.
- Rust API guidelines C-DEFENSIVE: assertions are a low-cost mechanism to catch invariant violations early.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation (touches the same matrix-assembly code)
- 090-rederive-per-surface-ua-from-first-principles

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — the file has no single `let inner_node = NodeId(...)` site as the ticket implies; instead there are two relevant locations:
  - **Material-layer path** (`assemble_building_rc`, line 604): `let inner_node = if n_cap_nodes > 0 { Some(NodeId(nodes_before + n_cap_nodes as u32 - 1)) } else { None };`
  - **Precomputed path** (`assemble_building_rc`, line 511): `let inner_node = if n_cap_nodes > 0 { Some(NodeId(nodes_before + n_cap_nodes as u32 - 1)) } else { None };`
  - Both use `nodes_before` which starts at `LAYER_NODE_BASE = 1000` (line 41). Neither path contains the described defensive assert.
- [x] Described logic matches current implementation — no `assert_ne!` or `debug_assert_ne!` guards the inner_node construction against `OUTDOOR_NODE_ID` / `GROUND_NODE_ID` collisions. This is accurate.
- [x] Bug is not already fixed — confirmed by grepping the entire `hares-envelope` crate for `assert_ne.*OUTDOOR` and `assert_ne.*GROUND`; the only matches are inside the *test* `many_boundaries_no_node_id_collision` (line 1838–1842), not in production code.
- [x] OCHRE cross-check result: **N/A** — OCHRE uses string labels (`label + str(i+1)`) for all nodes (`vendors/OCHRE/ochre/Models/Envelope.py`, line 377–377). There is no integer NodeId scheme in OCHRE and no concept of reserved integer sentinel IDs. The NodeId design and the collision risk are entirely HARES-specific; OCHRE provides no reference for this pattern.
- [x] EnergyPlus cross-check result: **N/A** — EnergyPlus also uses string or array-index-based node identification, not a sentinel-reserved integer scheme. This ticket's concern is architectural, not derived from EnergyPlus conventions.

### Web-Verified Citations

**Citation 1**: "Rust API guidelines C-DEFENSIVE: assertions are a low-cost mechanism to catch invariant violations early."

- **Source found**: Official Rust API Guidelines checklist — https://rust-lang.github.io/api-guidelines/checklist.html (fetched 2026-05-21)
- **Quoted passage**: The checklist enumerates these guideline codes under Dependability: **C-VALIDATE**, **C-DTOR-FAIL**, **C-DTOR-BLOCK**. Full code list confirmed: C-CASE, C-CONV, C-GETTER, C-ITER, C-ITER-TY, C-FEATURE, C-WORD-ORDER, C-COMMON-TRAITS, C-CONV-TRAITS, C-COLLECT, C-SERDE, C-SEND-SYNC, C-GOOD-ERR, C-NUM-FMT, C-RW-VALUE, C-EVOCATIVE, C-MACRO-ATTR, C-ANYWHERE, C-MACRO-VIS, C-MACRO-TY, C-CRATE-DOC, C-EXAMPLE, C-QUESTION-MARK, C-FAILURE, C-LINK, C-METADATA, C-RELNOTES, C-HIDDEN, C-SMART-PTR, C-CONV-SPECIFIC, C-METHOD, C-NO-OUT, C-OVERLOAD, C-DEREF, C-CTOR, C-INTERMEDIATE, C-CALLER-CONTROL, C-GENERIC, C-OBJECT, C-NEWTYPE, C-CUSTOM-TYPE, C-BITFLAG, C-BUILDER, C-VALIDATE, C-DTOR-FAIL, C-DTOR-BLOCK, C-DEBUG, C-DEBUG-NONEMPTY, C-SEALED, C-STRUCT-PRIVATE, C-NEWTYPE-HIDE, C-STRUCT-BOUNDS, C-STABLE, C-PERMISSIVE.
- **Also fetched**: https://rust-lang.github.io/api-guidelines/dependability.html — The Dependability section says under C-VALIDATE: "Dynamic enforcement with `debug_assert!` — Same as dynamic enforcement, but with the possibility of easily turning off expensive checks for production builds." No `C-DEFENSIVE` appears anywhere on the page.
- **Verdict**: **Incorrect** — `C-DEFENSIVE` does not exist in the Rust API Guidelines. The closest real guideline is **C-VALIDATE** on the Dependability page. The substance of the recommendation (use `debug_assert!` / `assert!` to guard invariants) is sound Rust practice and is supported by C-VALIDATE's discussion, but the cited code is fabricated.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core concern is real and worth tracking: no production-code assertion guards the inner_node construction sites against collision with `OUTDOOR_NODE_ID` (`u32::MAX - 1`) and `GROUND_NODE_ID` (`u32::MAX`). However, the risk is currently theoretical rather than latent — node IDs are allocated sequentially upward from `LAYER_NODE_BASE = 1000`, and reaching `u32::MAX - 1` would require allocating approximately 4.3 billion nodes in a single simulation, which is physically impossible. The existing test `many_boundaries_no_node_id_collision` (line 1812–1843) already checks that all internal node IDs returned in `node_index` are distinct from the reserved constants, providing post-assembly verification. What the ticket correctly identifies as missing is an *at-construction* assertion that would catch a broken NodeId scheme immediately — the "guarantee is by inspection rather than by assertion" framing is accurate. The standards citation (`C-DEFENSIVE`) is fabricated; the correct reference is C-VALIDATE from the Dependability section of the Rust API Guidelines. The ticket's description of the code site is imprecise — it says `let inner_node = NodeId(...)` but the actual construction is `NodeId(nodes_before + n_cap_nodes as u32 - 1)` inside `assemble_building_rc`, not inside `build_layered_boundary` or `build_precomputed_boundary`.

### Proposed Fix Summary

Add `assert_ne!` guards (cold path — solver init) immediately after the two `inner_node` constructions in `assemble_building_rc` (~lines 511–515 and 604–607), e.g.:

```rust
// (material-layer path)
if let Some(inner) = inner_node {
    assert_ne!(inner, NodeId(OUTDOOR_NODE_ID),
        "inner_node collides with OUTDOOR_NODE_ID — NodeId scheme broken");
    assert_ne!(inner, NodeId(GROUND_NODE_ID),
        "inner_node collides with GROUND_NODE_ID — NodeId scheme broken");
}
```

Plain `assert_ne!` (not `debug_assert_ne!`) is appropriate because these sites are on the solver-initialization cold path and the cost is negligible. This matches the ticket's own recommendation: "prefer a non-debug `assert_ne!` if the formula is on a cold path". The fix does NOT need to touch `build_layered_boundary` or `build_precomputed_boundary` — those functions return `layer_nodes[n_caps-1]` which has no reserved-range awareness; the guard belongs at the call-site in `assemble_building_rc` where the reserved sentinel context is known.

### Test Written

- **File**: `crates/hares-envelope/src/boundary_rc.rs` — appended to `#[cfg(test)] mod tests`
- **Tests added**:
  - `ticket_116_inner_node_does_not_collide_with_reserved_ids_material_path` — builds a two-layer material-path boundary and asserts `SurfaceLayerInfo.inner_node ≠ OUTDOOR_NODE_ID` and `≠ GROUND_NODE_ID`; also checks `BoundaryDiagnostic.inner_node` if present.
  - `ticket_116_inner_node_does_not_collide_with_reserved_ids_precomputed_path` — same for the precomputed-RC path.
- **Status**: Both tests pass today (the invariant holds). They will fail if a future NodeId scheme change causes a collision, making the implicit invariant explicit and machine-checked.
- **Note**: These are passing regression tests, not failing "demonstrates the bug" tests, because the underlying invariant holds for any physically realizable input. The ticket's claim that a test is needed to "exercise the assertion path" (i.e., a test that provokes the panic) would require the production assertion to be added first, and is only feasible via `#[should_panic]` with a synthetic broken NodeId value. The invariant-enforcement tests added here are more practical.
