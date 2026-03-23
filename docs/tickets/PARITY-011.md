---
id: PARITY-011
title: "ASHP backup ER control FSM"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/hvac/heat_pump/heater.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 15)
  - vendors/OCHRE/ochre/models/HVAC.py (ASHPHeater, lines 1176-1478)
  - docs/equipment/hvac.md (Backup/Auxiliary section)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES has backup ER capacity/EIR fields but lacks OCHRE's sophisticated lockout logic: hard lockout after setpoint changes, soft lockout while HP is winning (zone temp rising), setpoint change detection, and ER fan power adjustment.

**Target**: Match OCHRE's 4-mode FSM (HP On, HP+ER, ER Only, Off) with full lockout timing.

## Work to Do

- [ ] Add to `HeatPumpHeaterCore`:
  - `er_hard_lockout_remaining_s: f64` — countdown after setpoint change
  - `er_soft_lockout_active: bool` — active while zone temp is rising toward setpoint
  - `last_setpoint_c: f64` — detect setpoint changes
  - `er_hard_lockout_duration_s: f64` — configurable (default 300s per OCHRE)
- [ ] In `update_control()`:
  - Detect setpoint changes: `if heating_setpoint != last_setpoint { trigger hard lockout }`
  - Decrement hard lockout timer
  - Evaluate soft lockout: `zone_temp > zone_temp_prev → HP is winning → keep ER locked out`
- [ ] 4-mode FSM logic:
  - `Off`: below deadband threshold
  - `HP On`: call for heat, ER locked out (hard or soft)
  - `HP+ER`: call for heat, HP running, ER lockout expired, zone still below setpoint
  - `ER Only`: HP locked out (below -17.78°C) but ER available
- [ ] ER fan power: add configurable `er_fan_power_w` applied when ER is firing
- [ ] Parse config: `er_hard_lockout_s`, `er_soft_lockout_enabled`
- [ ] Add tests: setpoint raise triggers lockout, HP warming zone extends soft lockout, ER activates after lockout expires

## Files to Touch

- `crates/hares-equipment/src/hvac/heat_pump/heater.rs`: ER lockout FSM, setpoint detection, fan power

## Measures of Success

- [ ] ER does not fire during hard lockout period after setpoint change
- [ ] ER does not fire while zone temp is rising (soft lockout)
- [ ] HP+ER mode activates when HP alone cannot maintain setpoint
- [ ] ER Only mode activates when OAT < HP lockout temp

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
