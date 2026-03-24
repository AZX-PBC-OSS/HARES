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

## Status: COMPLETE (verified — already fully implemented)

Audit confirms the full OCHRE-parity ER backup FSM is already implemented in heater.rs with 13 dedicated tests:

| Feature | Implementation | Tests |
|---------|---------------|-------|
| Hard lockout (setpoint change) | `er_lockout_remaining_s`, `er_hard_lockout_time_s` | `er_hard_lockout_blocks_er_for_configured_duration`, `actual_setpoint_raise_triggers_er_lockout` |
| Soft lockout (zone temp rising) | `er_soft_lockout`, `prev_zone_temp_c` | `er_soft_lockout_blocks_er_while_zone_temp_is_rising` |
| 4-mode FSM | HeatingHP/HeatingER/HeatingHPAndER/Off at lines 558-565 | `default_lockout_below_hp_threshold_runs_er_only`, `default_lockout_above_er_threshold_runs_hp_only` |
| ER fan power | `er_capacity_w * backup_eir` at line 760 | `er_backup_capacity_modulated_by_plr` |
| Config parsing | 6 config keys with OCHRE-matched defaults | `er_hard_lockout_defaults_to_ochre_parity_zero`, `er_hard_lockout_uses_explicit_config_value` |
| DR edge case | DR expiry doesn't trigger ER lockout | `dr_expiry_does_not_trigger_er_lockout` |

## Background/Context

Implemented during prior work. The gap analysis was based on an earlier state of the code.

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
