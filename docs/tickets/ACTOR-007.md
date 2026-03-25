---
id: ACTOR-007
title: Update integration tests for equipment-based ideal HVAC
kind: implement
depends_on:
  - ACTOR-006
files_to_touch:
  - tests/freefloat_oracle.rs
  - tests/bestest/mod.rs
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo test --workspace
  - cargo test --test freefloat_oracle --features observe
---

## Background/Context

With the solver cleaned up and dwelling wired to use IdealHvac equipment + SolverFeedbackActor, update all integration tests that referenced the removed solver-internal ideal HVAC API.

## Work to Do

- [x] `tests/freefloat_oracle.rs`: Remove `dwelling.thermal_solver.set_ideal_hvac_zones(vec![])` calls. For free-float, clear all equipment (including auto-created IdealHvac).
- [x] `tests/bestest/mod.rs`: Verify BESTEST conditioned cases work through IdealHvac equipment (auto-created when setpoints present)
- [x] Verify regular HVAC equipment (furnace, AC, heat pump) still works unchanged via ports — on/off thermostat cycling doesn't depend on solver-internal ideal HVAC
- [x] Grep entire workspace for remaining references to removed fields/methods

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes
- [x] `cargo test --test freefloat_oracle --features observe` passes (6/6)
- [x] `grep -r 'set_ideal_hvac_zones\|ideal_setpoints_c' crates/ tests/` returns nothing

## Implementation Notes (2026-03-25)

This ticket was fully satisfied by work done in ACTOR-005 (solver cleanup) and ACTOR-006 (dwelling wiring):

- **freefloat_oracle.rs**: `set_ideal_hvac_zones()` calls replaced with `dwelling.clear_equipment()` during ACTOR-005/006. Freefloat tests strip all equipment so no IdealHvac interferes with envelope-only validation.
- **BESTEST conditioned cases**: Work through `Dwelling::simulate()` → equipment registry auto-creates IdealHvac when setpoints are present in the fixture config. No test changes needed.
- **Regular HVAC equipment**: Furnace, AC, heat pump operate via the Equipment trait and thermal ports. They are unaffected by the solver-internal ideal HVAC removal — thermostat cycling uses `update_control()` / `step()` / thermal port writes, not the old solver-side back-calculation.
- **Dead references**: Zero remaining references to `set_ideal_hvac_zones` or `ideal_setpoints_c` in `crates/` or `tests/`.
