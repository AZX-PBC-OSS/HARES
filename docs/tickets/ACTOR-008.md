---
id: ACTOR-008
title: Intermediate review — solver decoupling checkpoint
kind: review
depends_on:
  - ACTOR-005
  - ACTOR-006
  - ACTOR-007
files_to_touch: []
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

Review checkpoint after the core solver decoupling before proceeding to oracle tests and actor infrastructure. Catches architectural issues early.

## Work to Do

- [ ] Verify solver is fully decoupled — no setpoint/control logic remains
- [ ] Verify SolverFeedbackActor dispatches IdealCapacity through normal actor→dispatch pipeline
- [ ] Verify dwelling never directly calls `apply_control` on equipment outside dispatcher
- [ ] Verify existing tests (freefloat oracle, BESTEST, envelope unit tests) all pass
- [ ] Verify regular HVAC equipment still works unchanged via ports
- [ ] Check for dead code from the removal

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
- [ ] `grep -r 'ideal_setpoints_c\|set_ideal_hvac_zones\|zone_setpoint_c' crates/ tests/` returns nothing
