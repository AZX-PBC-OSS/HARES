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
