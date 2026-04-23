# Correct Comment About Windows `input_index` in Radiant Distribution

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-envelope/thermal_solver/ports

## Problem

The comment at `crates/hares-envelope/src/thermal_solver/ports.rs:135-137` claims "Windows (input_index=None)". This is wrong: `crates/hares-core/src/dwelling/solver_builder.rs:818` always sets `input_index: Some(zone_air_idx)` for windows. Window exclusion from the radiant distribution actually works because `solar_absorptance = 0.0` causes the radiant weight to be zero — the `input_index` is set, it's just contributing zero weight.

The comment is misleading. A future contributor reading "Windows (input_index=None)" will look for an `Option::None` branch that does not exist and may introduce a wrong mental model when modifying the code.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/ports.rs:135-137`:
```rust
// Windows (input_index=None) are excluded from radiant distribution
```

`crates/hares-core/src/dwelling/solver_builder.rs:818`:
```rust
input_index: Some(zone_air_idx),  // always set, even for windows
```

The actual exclusion mechanism is `solar_absorptance = 0.0` for windows, producing zero weight in the distribution loop.

## Required Behavior

Update the comment to reflect what the code actually does:

```rust
// Windows have solar_absorptance = 0.0 set at construction (solver_builder.rs),
// so they receive zero weight in the radiant distribution. The input_index field
// is always Some(zone_air_idx); the zero weight is the exclusion mechanism.
```

No code change needed. This is a documentation correction.

## Approach

1. Open `crates/hares-envelope/src/thermal_solver/ports.rs:135-137`.
2. Replace the misleading comment with the corrected text above.
3. Verify the assertion by inspection of `solver_builder.rs:818` — the comment must match the code.

## Definition of Done

- [ ] Comment at `ports.rs:135-137` corrected
- [ ] Comment cites the actual exclusion mechanism (`solar_absorptance = 0.0`)
- [ ] Comment cross-references `solver_builder.rs:818` for verifiability

## Verification

```bash
cargo test -p hares-envelope ports   # no behaviour change; sanity check
```

## References

- HARES `crates/hares-envelope/src/thermal_solver/ports.rs` — distribution loop.
- HARES `crates/hares-core/src/dwelling/solver_builder.rs:818` — window port construction.
- Project policy `feedback_no_useless_comments.md` — comments must accurately describe what the code does.

## Related Tickets

- 091-port-radiant-inputs-all-zones (related radiant-port work)
- 049-window-solar-shgc-vs-transmittance-absorbed-inward
