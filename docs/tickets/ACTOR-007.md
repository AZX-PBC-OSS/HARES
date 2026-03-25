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

- [ ] `tests/freefloat_oracle.rs`: Remove `dwelling.thermal_solver.set_ideal_hvac_zones(vec![])` calls. For free-float, clear all equipment (including auto-created IdealHvac).
- [ ] `tests/bestest/mod.rs`: Verify BESTEST conditioned cases work through IdealHvac equipment (auto-created when setpoints present)
- [ ] Verify regular HVAC equipment (furnace, AC, heat pump) still works unchanged via ports — on/off thermostat cycling doesn't depend on solver-internal ideal HVAC
- [ ] Grep entire workspace for remaining references to removed fields/methods

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo test --test freefloat_oracle --features observe` passes
- [ ] `grep -r 'set_ideal_hvac_zones\|ideal_setpoints_c' crates/ tests/` returns nothing
