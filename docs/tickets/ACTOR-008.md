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

- [x] Verify solver is fully decoupled — no setpoint/control logic remains
- [x] Verify SolverFeedbackActor dispatches IdealCapacity through normal actor→dispatch pipeline
- [x] Verify dwelling never directly calls `apply_control` on equipment outside dispatcher
- [x] Verify existing tests (freefloat oracle, BESTEST, envelope unit tests) all pass
- [x] Verify regular HVAC equipment still works unchanged via ports
- [x] Check for dead code from the removal

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes
- [x] `cargo clippy --workspace` passes
- [x] `grep -r 'ideal_setpoints_c\|set_ideal_hvac_zones\|zone_setpoint_c' crates/ tests/` returns nothing

## Review Findings (2026-03-25)

### Solver Decoupling ✅
- `ThermalSolverConfig` no longer contains `ideal_setpoints_c` or `ideal_hvac_zones` fields
- `solve_ideal_capacity_for_target(zone, target_c)` is pure physics — takes explicit target temperature
- No setpoint knowledge baked into the solver; all control logic moved to equipment/actors

### SolverFeedbackActor Pipeline ✅
- Implements `Actor` trait with `name()` and `decide()` methods
- `collect_and_solve()` calls equipment `ideal_target()` and solver back-compute
- `decide()` emits `DispatchRequest` with `IdealCapacity` signal at `Schedule` priority
- Signals flow through normal dispatcher path (queue → dispatch_into → apply_to_matching)

### Dwelling Control Path ✅
- `run_timestep()` flow: thermal equipment `update_control()` → solver feedback `collect_and_solve()` → actors `decide()` → dispatcher `queue()` → dispatcher `dispatch_into()`
- No direct `apply_control` calls outside `apply_to_matching()` helper inside dispatcher
- All control signals (IdealCapacity, ThermalSetpoint, etc.) route through `ControlDispatcher`

### Test Results ✅
- Envelope unit tests: 176 passed
- Freefloat oracle: 6/6 passed
- BESTEST: 1 passed, 5 ignored (long-running)
- Equipment HVAC tests: all pass (furnace, heat pump, ideal_hvac)

### Regular HVAC Equipment ✅
- Furnace, heat pump use default `ideal_target() → None`
- Operate via thermal ports with on/off thermostat cycling
- Unaffected by ideal capacity architecture — no behavior change

### Dead Code ✅
- `IdealCapacitySolver` trait removed (was in ACTOR-001 post-review)
- `update_mode_and_duty_with_ideal_solver` removed
- No leftover references to removed APIs

### Pre-existing Warnings (Not Related to This Work)
- `hares-envelope/src/thermal_solver/infiltration.rs`: unused `q_infiltration_w`, `q_natural_vent_w`, `q_forced_vent_w`
- `hares-equipment/src/water_heater/`: unused `hares_physics::units as conv` imports
- `hares-core/src/dwelling/solver_builder.rs`: too many arguments warning
- `hares-python`: pyo3 deprecation warning

## Tests Added for End-to-End Wiring Confidence (2026-03-25)

Identified and filled critical test gaps for the actor→dispatch→equipment pipeline:

### Dwelling Unit Tests (`dwelling/mod.rs`)
- `dispatch_delivers_all_signals_no_loss` — N signals queued → N delivered; verifies no signal loss
- `dispatch_same_tier_same_target_last_write_wins` — two same-tier signals to same equipment; last wins
- `dispatch_by_end_use_targets_all_matching_equipment` — ByEndUse reaches all matching, skips non-matching
- `dispatch_queues_drained_no_carryover_between_steps` — queues empty after dispatch; no cross-step leak
- `solver_feedback_signal_count_matches_ideal_equipment_count` — 2 ideal + 1 non-ideal → exactly 2 signals with correct capacities
- `solver_feedback_signal_overridden_by_higher_priority_actor` — Schedule IdealCapacity overwritten by Grid priority
- `dispatch_continues_after_one_equipment_rejects_signal` — one equipment rejects, another still receives

### Integration Tests (`orchestration_parity.rs`)
- `hvac_heating_energy_reaches_thermal_solver` — heated vs unheated dwelling; proves dispatch→port→solver path
- `step_result_hvac_heating_w_positive_when_furnace_fires` — hvac_heating_w aggregation from thermal ports
- `normal_operation_produces_no_dispatch_warnings` — zero dispatch warnings during normal operation
