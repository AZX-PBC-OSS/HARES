# Occupant Count Silently Returns 0.0 When Schedule Domain Absent

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-core/dwelling

## Problem

`crates/hares-core/src/dwelling/mod.rs:1903` silently returns 0.0 occupants when the schedule domain that supplies occupant counts is absent. There is no `tracing::warn!`, no error, no diagnostic — the dwelling proceeds as if the building were unoccupied. This produces zero occupant heat gains, zero CO2 contribution, and zero hot-water draw scaling, all without any indication that a required schedule input was missing.

This violates `feedback_no_silent_defaults`: missing or invalid input must error loudly, not be silently substituted with a fallback value.

## Current Behavior

`crates/hares-core/src/dwelling/mod.rs:1903`:
- The occupant-count accessor returns `0.0` when the schedule domain key is not present in the schedule registry.
- No log line, no diagnostic, no error.
- Downstream consumers (occupant heat gain, CO2 actor, water draw scaling) receive a falsy value indistinguishable from "the schedule said zero occupants in this hour".

## Required Behavior

When the schedule domain that supplies occupant counts is absent at the time the dwelling first attempts to read it:

1. If the absence is a configuration error (no schedule was registered at all for the dwelling type that requires one), return `Err(DwellingError::MissingScheduleDomain { domain: "occupants" })` from the construction or step path that first observed the absence.
2. If the absence is legitimate (e.g. an unoccupied test fixture explicitly has no occupant schedule), the dwelling configuration must declare `occupants_present: false` and the accessor must `debug_assert!(false)` if it is called against such a configuration — calling it would be a programming error.
3. Either path must surface the absence; silent zero is forbidden.

## Approach

1. Identify all callers of the accessor in `dwelling/mod.rs:1903`. Determine whether they rely on the silent-zero behaviour or whether they would prefer a loud error.
2. Add a `MissingScheduleDomain { domain: &'static str }` variant to `DwellingError` (or the equivalent error enum used by the construction path).
3. Replace the silent `0.0` return with the new error, propagated through the construction path. The hot-step accessor must not return errors — instead, the absence must be detected at construction time by validating that all required schedule domains are present given the equipment and actor set the dwelling was built with.
4. For dwellings that are legitimately unoccupied (e.g. BESTEST 600/900 base cases), the dwelling builder must set an explicit `occupants_present: false` flag that bypasses the schedule lookup entirely.
5. Add a unit test that constructs a dwelling with occupant-dependent equipment but no occupant schedule and asserts the construction errors with the new variant.

## Definition of Done

- [ ] Accessor at `crates/hares-core/src/dwelling/mod.rs:1903` no longer returns silent 0.0
- [ ] Construction-time validation rejects dwellings that require an occupant schedule but lack one
- [ ] `DwellingError::MissingScheduleDomain` (or equivalent) variant added
- [ ] `occupants_present: false` flag exists for legitimately unoccupied dwellings
- [ ] Unit test asserts loud failure when occupant schedule is absent
- [ ] All existing dwelling fixtures pass either the loud-error or explicit-unoccupied path

## Verification

```bash
cargo test -p hares-core dwelling
cargo test -p hares-core --test dwelling_integration
cargo build -p hares-core
```

## References

- HARES project memory `feedback_no_silent_defaults` — never silently substitute fallback values for missing/invalid input; error loudly.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Internal Loads — People" — occupant heat gain is a primary driver of cooling load; silently zeroing it produces 200-500 W systematic underestimate per absent occupant.
- HPXML Specification v4.x §11 "Schedules" — occupant schedule is a required input for residential dwellings.

## Related Tickets

- 102-thermal-solver-init-indoor-zone-loud-error (same loud-error pattern for thermal solver init)
- 103-zone-capacitance-air-density-loud-error (same loud-error pattern for air density)
- 114-scheduled-load-sensible-fraction-loud-error (same loud-error pattern for sensible fraction)
