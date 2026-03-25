---
id: ACTOR-015
title: Code review — IdealHvac + solver decoupling
kind: review
depends_on:
  - ACTOR-001
  - ACTOR-002
  - ACTOR-003
  - ACTOR-004
  - ACTOR-005
  - ACTOR-006
  - ACTOR-007
  - ACTOR-008
  - ACTOR-009
  - ACTOR-010
  - ACTOR-011
  - ACTOR-012
  - ACTOR-013
  - ACTOR-014
files_to_touch: []
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

Review all changes from ACTOR-001 through ACTOR-006 for correctness, code quality, separation of concerns, and adherence to project standards (SI units, strong typing, no legacy shims, no useless comments).

## Work to Do

- [ ] Review IdealHvac equipment implementation for correctness and code quality
- [ ] Verify solver is fully decoupled from setpoint/control logic
- [ ] Verify ControlSignal::IdealCapacity flow works correctly through dispatch
- [ ] Verify dwelling orchestrator correctly mediates solver feedback
- [ ] Review test updates — ensure no functionality was silently dropped
- [ ] Verify conditioned oracle test tolerances are reasonable
- [ ] Check for any remaining references to removed fields
- [ ] Verify no regressions in freefloat oracle or BESTEST tests

## Measures of Success

- [ ] No references to ideal_setpoints_c, ideal_hvac_zones, set_ideal_hvac_zones, zone_setpoint_c
- [ ] IdealHvac follows Equipment trait patterns established by furnace/AC
- [ ] All findings addressed or explicitly deferred with ticket reference

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
- [ ] `cargo test --test freefloat_oracle --features observe` passes
- [ ] `cargo test --test conditioned_oracle --features observe` passes
