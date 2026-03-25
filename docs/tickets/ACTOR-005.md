---
id: ACTOR-005
title: Remove ideal HVAC internals from thermal solver
kind: implement
depends_on:
  - ACTOR-002
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/config.rs
  - crates/hares-envelope/src/thermal_solver/stepping.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/tests/synthetic_box.rs
  - crates/hares-envelope/tests/multi_zone_coupling.rs
  - crates/hares-envelope/tests/solver_energy_conservation.rs
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope
---

## Background/Context

Remove all ideal HVAC coupling from the thermal solver. The solver becomes pure physics — no setpoint knowledge, no control decisions. Only touches hares-envelope and its tests.

## Work to Do

- [x] `config.rs`: Remove `ideal_setpoints_c: HashMap<ZoneId, f64>` and `ideal_hvac_zones: Vec<ZoneId>` from ThermalSolverConfig + Default impl
- [x] `stepping.rs`: Remove `solve_ideal_capacity(&self, env, zone)` (replaced by `solve_ideal_capacity_for_target` from ACTOR-002)
- [x] `stepping.rs`: Remove `ideal_hvac_zones` parameter from `resolve_internal()`, remove both ideal HVAC loops (coupled + non-coupled), remove `ideal_heating_w`/`ideal_cooling_w` accumulation
- [x] `mod.rs`: Remove `zone_setpoint_c()`, `set_ideal_hvac_zones()`, update `resolve()` to not pass `ideal_hvac_zones`
- [x] Update envelope tests: remove `ideal_setpoints_c` and `ideal_hvac_zones` from config construction in synthetic_box.rs, multi_zone_coupling.rs, solver_energy_conservation.rs

## Verification

- [x] `cargo build -p hares-envelope` passes
- [x] `cargo test -p hares-envelope` passes
- [x] `grep -r 'ideal_setpoints_c\|ideal_hvac_zones\|set_ideal_hvac_zones\|zone_setpoint_c' crates/hares-envelope/` returns nothing

## Tests Removed

The following tests were removed because they specifically tested ideal HVAC behavior that is no longer implemented in the solver:

- `test_1r1c_steady_state_with_hvac` (synthetic_box.rs) - tested ideal HVAC maintaining setpoint
- `test_1r1c_infiltration_with_hvac_steady_state` (synthetic_box.rs) - tested ideal HVAC with infiltration coupling
- `test_two_zone_coupled_wall_steady_state` (multi_zone_coupling.rs) - tested ideal HVAC in multi-zone scenario
- `test_energy_conservation_with_hvac` (solver_energy_conservation.rs) - tested energy balance with ideal HVAC
- `solve_ideal_capacity_drives_zone_to_setpoint` (mod.rs) - tested the removed `solve_ideal_capacity` method

## Implementation Notes

- The `solve_ideal_capacity_for_target` method from ACTOR-002 remains and is now the only public ideal capacity solving method
- Removed unused fields `solve_rhs_buf` and `solve_gain_buf` from `ThermalSolver` struct
- The `u` vector in `resolve_internal()` is no longer mutable since ideal HVAC no longer modifies it
- Pre-existing warnings in infiltration.rs (`q_infiltration_w`, `q_natural_vent_w`, `q_forced_vent_w` unused) are unrelated to this ticket
- Updated `IdealCapacitySolver` trait in `hvac_core.rs` to use `solve_ideal_capacity_for_target(zone, target_c)` instead of `solve_ideal_capacity(env, zone)` — matches the new solver API

## Legacy Code Note

The `IdealCapacitySolver` trait and `update_mode_and_duty_with_ideal_solver` method in `hvac_core.rs` are legacy infrastructure that will be superseded by the new `IdealHvac` equipment + `SolverFeedbackActor` architecture. These are now effectively dead code paths but are left in place for the actor migration. They can be removed once the actor-based ideal HVAC is fully integrated.