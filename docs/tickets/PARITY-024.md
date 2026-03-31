---
id: PARITY-024
title: Fix ASHP thermostat cold-start — initial mode should reflect zone vs setpoint
kind: fix
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/hvac/thermostat.rs
references:
  - vendors/OCHRE/ochre/Equipment/HVAC.py (lines 520-540, OCHRE thermostat init)
  - crates/hares-equipment/src/hvac/heat_pump/heater.rs (lines 1148-1160)
verification:
  - cargo test -p hares-equipment --test hvac_tests
  - cargo test -p hares-core --test parity parity_outputs
---

## Background/Context

The thermostat FSM starts in `Deadband` mode regardless of the initial zone
temperature. When zone temp is initialized AT the heating setpoint (e.g. 20°C),
the thermostat never transitions to `Heating` because the zone temp must drop
below `setpoint - hysteresis` (e.g. 19.2°C) first. Without a thermal event to
cool the zone, HVAC never activates.

OCHRE handles this by running an initial control step that evaluates the zone
temp against setpoints before the first simulation step, allowing the thermostat
to start in the correct mode.

This is the root cause of 99%+ HVAC energy deviation in all ASHP parity
fixtures and the BESTEST zero-HVAC issue.

## Work to Do

- [ ] In thermostat initialization (or first `update_mode` call), compare
      initial zone temp against setpoints and set initial mode accordingly:
  - `zone_temp < heating_setpoint` → start in `Heating`
  - `zone_temp > cooling_setpoint` → start in `Cooling`
  - otherwise → `Deadband` (current default)
- [ ] Ensure the first `ideal_target()` call after init returns the correct
      target when the thermostat starts in Heating/Cooling mode
- [ ] Add unit test: thermostat starts in Heating when zone_temp < setpoint

## Files to Touch

- `crates/hares-equipment/src/hvac/thermostat.rs`: Initial mode selection logic

## Measures of Success

- [ ] ASHP parity fixtures: zone temp MAE drops from 0.5°C to <0.2°C
- [ ] ASHP parity fixtures: HVAC energy deviation drops from 99% to <20%
- [ ] BESTEST cases produce nonzero hvac_heating_w

## Verification

- [ ] `cargo check` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo test -p hares-core --test parity parity_outputs` — ASHP deviations improve
