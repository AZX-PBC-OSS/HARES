---
id: ACTOR-002
title: Add solve_ideal_capacity_for_target to ThermalSolver
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/stepping.rs
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope
---

## Background/Context

The existing `solve_ideal_capacity(&self, env, zone)` method reads the target temperature from `ideal_setpoints_c` in the solver config, coupling setpoint knowledge into the physics solver. We need a pure utility variant that accepts an explicit target temperature, so equipment can call it without the solver knowing about setpoints.

## Work to Do

- [x] Add `solve_ideal_capacity_for_target(&self, zone: ZoneId, target_c: f64) -> f64` to `ThermalSolver` in `crates/hares-envelope/src/thermal_solver/stepping.rs`
- [x] Implementation: identical to existing `solve_ideal_capacity` (lines 20-52) but uses `target_c` parameter instead of calling `zone_setpoint_c()`
- [x] Keep the existing `solve_ideal_capacity` method unchanged for now (will be removed in ACTOR-004)

## Files to Touch

- `crates/hares-envelope/src/thermal_solver/stepping.rs`: Add new public method alongside existing `solve_ideal_capacity`

## Measures of Success

- [x] New method compiles and is publicly accessible
- [x] Existing `solve_ideal_capacity` remains unchanged
- [x] No test regressions

## Verification

- [x] `cargo build -p hares-envelope` passes
- [x] `cargo test -p hares-envelope` passes
- [x] `cargo clippy -p hares-envelope` passes

## Tests Added

### hares-envelope
- `solve_ideal_capacity_for_target_returns_correct_load`
- `solve_ideal_capacity_for_target_returns_zero_for_unknown_zone`
- `solve_ideal_capacity_for_target_works_without_setpoint_config`
- `solve_ideal_capacity_for_target_returns_negative_for_cooling`

## Post-Review Fixes (2026-03-25)

- Refactored: `solve_ideal_capacity_for_target` is now the core method; `solve_ideal_capacity` calls it
- Removed internal `_internal` helper (indirection was backwards)
- Updated module doc to mention the new public method
- Added cooling-path test for negative capacity (target < zone_temp)
