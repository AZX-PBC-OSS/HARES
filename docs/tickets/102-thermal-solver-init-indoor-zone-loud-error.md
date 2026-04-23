# Thermal Solver Init Must Error Loudly When `indoor_zone_id` Missing From Indices

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/initialization

## Problem

`crates/hares-envelope/src/thermal_solver/initialization.rs:79-83` silently returns a flat `indoor_temp_c` profile when `indoor_zone_id` is absent from `zone_state_indices`. This is a silent default in violation of `feedback_no_silent_defaults` — a configuration error that should be surfaced loudly is masked, and the simulation proceeds with a wrong initial state.

If `indoor_zone_id` cannot be resolved against `zone_state_indices`, the dwelling is misconfigured (the indoor zone wasn't registered, or the IDs disagree). Returning a flat-temperature fallback hides the misconfiguration; the user gets a successfully-running simulation with subtly wrong initial conditions and no diagnostic.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/initialization.rs:79-83`:
```rust
if !zone_state_indices.contains_key(&indoor_zone_id) {
    return Ok(vec![indoor_temp_c; n_nodes]);  // silent flat fallback
}
```

Initialization completes successfully with a uniform-temperature node vector. The error only manifests downstream as biased zone temperatures or unphysical first-step solver behaviour.

## Required Behavior

1. If `indoor_zone_id` is not in `zone_state_indices`, return `Err(ThermalSolverError::IndoorZoneIdNotRegistered { id, registered })` (or the equivalent local error type) with the missing ID and the list of registered IDs.
2. Do not substitute a fallback initial-state vector.
3. The error must propagate to the caller (`Dwelling::new` or wherever the thermal solver is initialised) and result in a hard initialisation failure visible to the user.

## Approach

1. Open `crates/hares-envelope/src/thermal_solver/initialization.rs:79-83` and replace the silent fallback with an explicit error return.
2. Add a new error variant to the local `ThermalSolverError` enum (or extend the existing `EnvelopeError`).
3. Plumb the error through `Dwelling::new` so the caller sees a useful message.
4. Add a unit test: construct a thermal solver with `indoor_zone_id` not present in `zone_state_indices` and assert the returned error type and message.
5. Audit the rest of `initialization.rs` for any other silent defaults and ticket them separately if found (do not bundle).

## Definition of Done

- [ ] Silent fallback at `initialization.rs:79-83` removed
- [ ] New error variant covers the missing-indoor-zone case with both the missing ID and registered IDs in the message
- [ ] Error propagates to the dwelling constructor
- [ ] Unit test asserts the error variant and message contents
- [ ] No other silent fallback in `initialization.rs` (audit complete; new tickets opened for any found)

## Verification

```bash
cargo test -p hares-envelope thermal_solver initialization
cargo test -p hares-core dwelling
```

## References

- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values for missing/invalid input; error loudly.
- Project policy `feedback_no_broken_windows.md` — fix all issues encountered; no deferring.

## Related Tickets

- 103-zone-capacitance-air-density-loud-error (parallel silent-default fix)
- 045-equipment-ports-applied-before-zone-state-update (related zone state consistency)
