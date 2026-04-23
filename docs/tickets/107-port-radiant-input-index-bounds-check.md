# Bounds Check on `input_index` in Radiant Distribution

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/ports

## Problem

`crates/hares-envelope/src/thermal_solver/ports.rs:98-115` performs `u[input_index]` access in the radiant distribution loop without a bounds check. An out-of-range `input_index` (from a misregistered surface or stale port mapping) panics at runtime via Rust's bounds-check, producing a stack trace but no domain-meaningful error.

The function is on the hot path (called every timestep). A panic in this location is non-recoverable; the simulation crashes mid-run with no diagnostic about which surface/port caused the failure.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/ports.rs:98-115`:
```rust
for surface in surfaces {
    let weight = ...;
    u[surface.input_index] += radiant_w * weight;  // panics on OOB
}
```

If `surface.input_index >= u.len()`, the slice access panics. The user sees `index out of bounds: the len is N but the index is M` with no information about which surface or which equipment port produced the bad index.

## Required Behavior

Choose one:

A. **Defensive assertion at solver init** — validate every `input_index` against `u.len()` once during solver construction. Any out-of-range index produces a domain-meaningful error (`PortIndexOutOfRange { surface_id, input_index, u_len }`) at init, before the hot loop ever runs. The hot loop can then assume validity and uses unchecked or checked access depending on perf.

B. **Per-iteration `Result` return** — change the function to return `Result<(), ThermalSolverError>` and check the index in the loop. Higher overhead but safer if `input_index` can change at runtime.

Recommended path: A. The `input_index` mapping is set at solver construction and does not change at runtime; the hot-loop check would be wasted work.

## Approach

1. In the thermal solver constructor (`ThermalSolver::new` or `solver_builder.rs`), iterate every surface and assert `input_index < u_len`. Collect any out-of-range indices into an error.
2. Return the error from the constructor, including the surface identifier(s) and the index/length values.
3. Add a unit test: construct a solver with a deliberately misregistered surface and assert the constructor error.
4. The hot loop at `ports.rs:98-115` keeps its current direct slice access; the constructor guarantee makes it safe.

## Definition of Done

- [ ] `ThermalSolver::new` validates every surface's `input_index` against `u_len` at construction
- [ ] Out-of-range indices produce `ThermalSolverError::PortIndexOutOfRange { surface_id, input_index, u_len }`
- [ ] Unit test: misregistered surface causes constructor failure with the expected error variant
- [ ] Hot loop access at `ports.rs:98-115` unchanged (validated at init)
- [ ] No defensive check added to the hot loop (validated by inspection)

## Verification

```bash
cargo test -p hares-envelope thermal_solver
cargo test -p hares-envelope ports
```

## References

- Project policy `feedback_hot_loop_minimal.md` — validate at init, not in the hot loop.
- Project policy `feedback_no_silent_defaults.md` — fail loudly with a domain-meaningful error.
- Rust API guidelines C-PANIC: "Functions and methods should not panic for input that is reasonably valid for the type" — surface registration is user-influenced input.

## Related Tickets

- 040-radiant-gain-weights-hot-alloc (hot-loop minimisation)
- 091-port-radiant-inputs-all-zones (related radiant distribution fix)
