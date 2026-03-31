---
id: PARITY-023
title: Implement ideal_target() for GasFurnace and ElectricFurnace
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/hvac/furnace.rs
references:
  - crates/hares-equipment/src/hvac/ideal_hvac.rs (lines 546-554, reference impl)
  - crates/hares-equipment/src/hvac/heat_pump/heater.rs (lines 1148-1160, ASHPHeater impl)
  - crates/hares-core/src/actors/solver_feedback.rs (lines 52-79, consumer)
  - vendors/OCHRE/ochre/Equipment/HVAC.py (lines 540-561, ideal capacity algorithm)
verification:
  - cargo test -p hares-equipment --test hvac_tests
  - cargo test -p hares-core --test orchestration_parity
---

## Background/Context

The solver feedback actor calls `equipment.ideal_target()` to get the desired
zone temperature, then solves for the required thermal capacity. The Furnace
equipment registers `IDEAL_CAPACITY` as a control capability but never returns
`Some(...)` from `ideal_target()`, so the solver feedback actor never dispatches
capacity signals to the furnace.

This causes the thermostat cold-start problem: zone starts at setpoint (20°C),
thermostat requires temp to drop below turn-on threshold (19.2°C) to enter
heating mode, but without ideal capacity dispatch the zone never receives the
initial thermal nudge to start the cycle.

The IdealHVAC equipment (ideal_hvac.rs) and ASHPHeater both implement
`ideal_target()` correctly and work with the solver feedback actor.

## Work to Do

- [ ] Implement `ideal_target()` for `GasFurnace` — return `Some((zone_id, setpoint_c))`
      when thermostat is in Heating mode (use heating setpoint as target)
- [ ] Implement `ideal_target()` for `ElectricFurnace` — same pattern
- [ ] Handle `IdealCapacity` control signal in furnace `apply_control()` to set duty cycle
      from solver-computed capacity
- [ ] Add unit tests verifying ideal_target returns correct values per thermostat mode

## Files to Touch

- `crates/hares-equipment/src/hvac/furnace.rs`: Both `GasFurnace` and `ElectricFurnace` impl blocks

## Measures of Success

- [ ] Orchestration parity tests pass (furnace fires within 10 steps at -10°C outdoor)
- [ ] `hvac_heating_energy_reaches_thermal_solver` test passes
- [ ] Gas furnace parity fixture zone temp MAE improves

## Verification

- [ ] `cargo check` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo test -p hares-core --test orchestration_parity` — all 11 tests pass
