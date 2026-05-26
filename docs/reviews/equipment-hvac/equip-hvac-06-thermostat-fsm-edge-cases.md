# Thermostat FSM edge cases: hysteresis, min cycle time, schedule transitions
**Review ID**: equip-hvac-06
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/thermostat.rs crates/hares-equipment/src/hvac/air_conditioner.rs crates/hares-equipment/src/hvac/heat_pump/heater.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings

### Finding 1: [Severity: medium]
**Description**: `ThermostatFsm::min_on_time_s` and `min_off_time_s` are not persisted in save/load state, silently reverting to zero on warmup restart or checkpoint restore.
**Code Location**: `thermostat.rs:210-213` (field declarations); `heater.rs:184-228` (HeaterState struct lacking these fields); `air_conditioner.rs:105-142` (AirConditionerState lacking these fields); `heater.rs:1984-2029` (save_state omits them).
**Root Cause**: The `ThermostatFsm` struct holds `min_on_time_s` and `min_off_time_s` (thermostat.rs:210-213), and `can_transition_mode` (thermostat.rs:296-315) reads them to block rapid mode transitions. However, neither `HeaterState` nor `AirConditionerState` serialise these fields. On warmup restart, `ThermostatFsm::new()` (thermostat.rs:217-231) re-initialises them to `0.0`. If a user configures non-zero values for short-cycle protection, the guard is silently inactivated after the first checkpoint.
**Impact**: Short-cycle protection via minimum on/off time enforcement is lost across warmup restarts. Equipment may cycle excessively after a checkpoint, producing unrealistic short-duration heating/cooling runs and inflating cycle counts.

### Finding 2: [Severity: medium]
**Description**: ER soft lockout temperature comparison uses strict greater-than (`>`) where OCHRE uses non-strict greater-or-equal (`>=`), causing earlier ER re-engagement after setpoint increases.
**Code Location**: `heater.rs:1754-1755` (`zone_rising` calculation); compare against `vendors/OCHRE/ochre/Equipment/HVAC.py:1338` (`temp_indoor >= self.temp_indoor_prev`).
**Root Cause**: HARES defines `zone_rising` as `zone.temperature_c > self.prev_zone_temp_c` (strict). OCHRE's condition is `temp_indoor >= self.temp_indoor_prev` (non-strict). When the zone temperature plateaus after a setpoint increase (i.e., temperature stops rising but hasn't started falling yet), HARES releases the soft lockout and allows ER to engage. OCHRE keeps the ER off until the temperature actually declines, which better prevents the heat pump from being supplemented before it has a chance to "catch up" to the load.
**Impact**: During mild shoulder-season conditions where the heat pump is nearly sufficient, the ER may engage one timestep earlier in HARES than in OCHRE, adding a small amount of unnecessary electric resistance heating energy. The magnitude is one timestep's worth of ER runtime per setpoint increase event.

### Finding 3: [Severity: low]
**Description**: Dual guard layers for short-cycle prevention (`is_cycle_change_allowed` and `can_transition_mode`) operate on independent time sources with independent defaults, creating configuration ambiguity.
**Code Location**: `thermostat.rs:339` (is_cycle_change_allowed check) and `thermostat.rs:420` (can_transition_mode check).
**Root Cause**: `update_mode` applies two sequential short-cycle guards: first `is_cycle_change_allowed` (reading `thermostat.min_cycle_time_s` from `ThermostatConfig` — default 0.0) at line 339, then `can_transition_mode` (reading `min_on_time_s`/`min_off_time_s` from `ThermostatFsm` — default 0.0, never set from config) at line 420. Both defaults are 0.0 so both are effectively disabled by default, but a user could configure `min_cycle_time_s` via HVAC equipment config while `min_on_time_s`/`min_off_time_s` have no config path at all. The `is_cycle_change_allowed` guard blocks all mode changes uniformly, while `can_transition_mode` separately enforces min-on and min-off time. Having two guards for the same concern with different data sources, different semantics, and only one of them user-configurable creates a maintenance burden and potential for unexpected behaviour if only one is configured.
**Impact**: If a user sets `min_cycle_time_s` to non-zero but the dual guard layers interact unexpectedly (e.g., `is_cycle_change_allowed` allows a change that `can_transition_mode` then blocks), the effective behaviour is that `can_transition_mode` becomes the actual governor for on↔off transitions but `is_cycle_change_allowed` adds an extra delay for Heating↔Cooling reversals. The net effect is the same as if only `can_transition_mode` were configured, but the asymmetry is undocumented.

### Finding 4: [Severity: low]
**Description**: No coordination between independent ThermostatFsm instances in dual-fuel (ASHP) systems — heating and cooling FSMs could theoretically both activate simultaneously.
**Code Location**: `hvac_core.rs:686-688` (HvacEquipment::update_mode delegates to its thermostat_fsm); `air_conditioner.rs:685-688` (CoolingCore::update_control delegates to hvac.update_mode); `heater.rs:1722` (resolve_control calls hvac.update_mode). Compare against `vendors/OCHRE/ochre/Equipment/HVAC.py:1176-1315` (ASHPHeater with HP and ER modes).
**Root Cause**: In HARES, HP heating and HP cooling are separate Equipment instances, each with an independent `ThermostatFsm`. Both FSMs read the same zone temperature from `EnvironmentState` and independently decide whether to call for heating or cooling. There is no cross-communication or mutual exclusion between the two FSMs. If zone temperature oscillates near the heating/cooling boundary — e.g., during a rapid outdoor temperature swing — the heating FSM could enter Heating mode while the cooling FSM simultaneously enters Cooling mode. OCHRE has the same architectural separation (ASHPCooler vs ASHPHeater are separate classes), so this is behaviour parity rather than a deviation, but the OCHRE `run_thermostat_control` uses a single on/off decision per equipment (not a tri-state Heating/Cooling/Deadband matrix), making the conflict less likely to manifest as visible simultaneous opposing operation.
**Impact**: In edge cases with rapid outdoor temperature swings, both heating and cooling compressors could run in the same timestep. A physical heat pump cannot heat and cool simultaneously; the model would over-count energy consumption and under-deliver effective thermal action. This is a low-probability scenario requiring zone temperature to cross both heating and cooling thresholds within the same timestep, but it is architecturally possible.

### Finding 5: [Severity: informational]
**Description**: Schedule setpoint changes are applied immediately within `update_mode` but mode transitions blocked by `is_cycle_change_allowed` are deferred, meaning the transition occurs against potentially different setpoints than when the physical condition first triggered.
**Code Location**: `thermostat.rs:327` (resolve_profile_setpoints called unconditionally at start of update_mode) and `thermostat.rs:339` (is_cycle_change_allowed guard can block transition).
**Root Cause**: At the top of `update_mode`, `resolve_profile_setpoints(env)` is called unconditionally (line 327), pulling the latest schedule values from the environment into `self.schedule_setpoints`. Then `is_cycle_change_allowed` (line 339) checks whether enough time has passed since the last mode switch. If the guard blocks, `self.mode` is returned unchanged but the schedule_setpoints have already been updated. On the next timestep when the guard lifts, the mode decision is based on the current (possibly further-changed) setpoints, not the setpoints in effect when the temperature first crossed the threshold. This is analogous to OCHRE's `update_setpoint` being called before `run_thermostat_control`.
**Impact**: In practice this is likely the desired behaviour — you don't want to freeze the setpoint just because a transition was blocked — but it means a blocked mode transition may occur at a slightly different zone temperature than if the guard weren't present. The effect is one timestep's temperature drift at most (bounded by guard duration × heating/cooling rate). No energy accounting error, but the transition history will show delayed switching relative to the instantaneous threshold.

### Finding 6: [Severity: informational]
**Description**: `ThermostatConfig::cutout_ratio` is dead code in the default (offset > 0) configuration path, serving only the symmetric-hysteresis fallback that is never exercised with default settings.
**Code Location**: `thermostat.rs:14` (field declaration); `thermostat.rs:384-409` (else branch using cutout_ratio); `thermostat.rs:354-383` (if-branch using deadband_offset, the active path by default).
**Root Cause**: `deadband_offset` defaults to 0.2 (thermostat.rs:40), so the `if offset > 0.0` branch at line 354 is always taken. The `else` branch at line 384 (which uses `cutout_ratio` instead of `offset` for hysteresis) is only reached when the user explicitly sets `deadband_offset` to 0.0. The `cutout_ratio` field has no equivalent in OCHRE — it appears to be a HARES-specific addition that provides a different asymmetric-hysteresis model when offset=0. With the default offset of 0.2, the cutout_ratio model is bypassed.
**Impact**: The `cutout_ratio` is untested in the default code path. If a user configures `deadband_offset: 0.0` and relies on `cutout_ratio` for hysteresis, the behaviour would diverge from OCHRE with no clear documentation of the difference. The field should either be integrated into the primary offset model or removed to reduce configuration surface area.

## Summary
- Total findings: 6
- Critical: 0 / High: 0 / Medium: 2 / Low: 2 / Informational: 2

## Recommendations
1. **Persist min_on_time_s / min_off_time_s in state snapshots** (Finding 1): Add `min_on_time_s` and `min_off_time_s` fields to both `HeaterState` (heater.rs:184) and `AirConditionerState` (air_conditioner.rs:105), round-trip them in save_state / load_state. This prevents silent loss of short-cycle protection across warmup restarts.

2. **Align ER soft lockout with OCHRE's non-strict comparison** (Finding 2): Change `zone_rising` from `>` to `>=` at heater.rs:1754-1755 to match OCHRE HVAC.py:1338. This prevents ER engagement during the first timestep where zone temperature plateaus after a setpoint increase, consistent with OCHRE's intent to give the heat pump sufficient time to satisfy the new load.

3. **Unify the dual short-cycle guard into a single mechanism** (Finding 3): Consider either (a) making `min_cycle_time_s` from ThermostatConfig the sole guard and removing `can_transition_mode` entirely, or (b) deriving `min_on_time_s`/`min_off_time_s` directly from `min_cycle_time_s` at init time so they are always synchronised. Document the unified behaviour clearly.

4. **Add diagnostic warning for simultaneous heating+cooling** (Finding 4): In the HP system coordinator (or wherever the two thermostat FSMs are orchestrated), add a diagnostic log warning if both the heating and cooling thermostats are simultaneously in active (non-Deadband) mode. This won't prevent the condition but will surface it for model validation.

5. **Clarify cutout_ratio vs deadband_offset documentation** (Finding 6): Document in `ThermostatConfig` that `cutout_ratio` only applies when `deadband_offset = 0.0`, and that the recommended (default) path uses `deadband_offset = 0.2`. Consider renaming `cutout_ratio` to `symmetric_cutout_ratio` or adding a validation warning when both are non-zero.

## References / Citations
- `vendors/OCHRE/ochre/Equipment/HVAC.py:392-409` — OCHRE base `run_thermostat_control` (symmetric hysteresis with deadband_offset)
- `vendors/OCHRE/ochre/Equipment/HVAC.py:1176-1315` — OCHRE `ASHPHeater.update_internal_control` (HP+ER dual thermostat)
- `vendors/OCHRE/ochre/Equipment/HVAC.py:1317-1369` — OCHRE `ASHPHeater.run_er_thermostat_control` (ER hard/soft lockout)
- `thermostat.rs:171-187` — HARES `is_cycle_change_allowed` (min_cycle_time_s guard)
- `thermostat.rs:296-315` — HARES `can_transition_mode` (min_on_time_s / min_off_time_s guard)
- `thermostat.rs:322-437` — HARES `ThermostatFsm::update_mode` (full hysteresis + cycle-time logic)
- `heater.rs:1721-1901` — HARES `HeatPumpHeaterCore::resolve_control` (HP+ER dual control with lockout)
