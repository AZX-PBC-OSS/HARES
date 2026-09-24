# Ideal thermostat actor: setpoint determination and interaction with equipment FSM
**Review ID**: agents-03
**Category**: agents
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actors/ideal_thermostat.rs`
- `crates/hares-equipment/src/hvac/thermostat.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: high]
**Description**: The `IdealThermostat` actor emits `ThermalSetpoint` signals without any cross-validation between heating and cooling setpoints, and `ThermostatFsm.apply_thermal_setpoint_signal()` (used by all non-ideal equipment paths: furnace, boiler, baseboard) stores them without validation. This allows physically impossible setpoint inversions (`heating_c > cooling_c`) to propagate into the FSM, where `update_mode()` produces overlapping deadband thresholds that mask the error by deterministically selecting Heating.

**Code Location**:
- `crates/hares-core/src/actors/ideal_thermostat.rs:229-267` — `decide()` emits `ThermalSetpoint` without any validation.
- `crates/hares-equipment/src/hvac/thermostat.rs:445-456` — `apply_thermal_setpoint_signal()` stores `RuntimeSetpointOverride` without deadband validation.
- `crates/hares-equipment/src/hvac/thermostat.rs:372-381` — In `update_mode()` deadband arm, inverted setpoints cause `heat_turn_on > cool_turn_on`, yet Heating is checked first, deterministically masking the error.

**Root Cause**: The `IdealThermostat` actor and `ThermostatFsm` delegate validation responsibility to equipment-specific callers, but only `IdealHvac` implements it via `validate_runtime_override()` (which calls `ThermalSetpoints::validate_for_deadband`). The generic heating equipment path (`apply_heating_control_unchecked` in `helpers.rs:177-197`) applies setpoints and deadband separately but does not cross-validate them.

**Impact**: A misconfigured or buggy thermostat script can push inverted setpoints (e.g., heat = 30 C, cool = 25 C) that silently produce nonsensical thermostat behavior: the deadband collapses, and the FSM always selects Heating regardless of zone temperature. This wastes energy and produces incorrect simulation results. In OCHRE, this class of error does not exist because each HVAC unit has only one setpoint (heating or cooling), not a dual-setpoint pair.

---

### Finding 2: [Severity: high]
**Description**: The `OverrideState::dual()` and `IdealThermostat::with_setpoints()` methods use `debug_assert!` to guard the `heating_c < cooling_c` invariant, which is stripped in release builds. The `set_override()` method (the primary runtime API) performs no validation at all.

**Code Location**:
- `crates/hares-core/src/actors/ideal_thermostat.rs:74-78` — `dual()`: `debug_assert!(heating_c < cooling_c)`.
- `crates/hares-core/src/actors/ideal_thermostat.rs:175-183` — `with_setpoints()`: same `debug_assert!`.
- `crates/hares-core/src/actors/ideal_thermostat.rs:197-199` — `set_override()`: unconditional assignment, zero checks.

**Root Cause**: `debug_assert!` is used for a logic invariant that should be enforced in all build configurations. The mutation API (`set_override`) bypasses builder validation entirely.

**Impact**: In production, an inverted override state is silently accepted. Combined with Finding 1, the invalid state flows through the entire equipment dispatch chain with no error or warning, producing physically impossible results.

---

### Finding 3: [Severity: medium]
**Description**: The `IdealThermostat` actor does not verify that heating and cooling setpoints satisfy the deadband constraint required by the equipment FSM. The review requirement "if `T_heat_setpoint + deadband > T_cool_setpoint`, the actor produces a clear error or adjusts the wider setpoint to maintain the deadband" is not met — the actor neither errors nor adjusts.

**Code Location**: `crates/hares-core/src/actors/ideal_thermostat.rs:229-267` — `decide()`.

**Root Cause**: The `IdealThermostat` was designed as a thin pass-through actor that pushes `OverrideState` directly into `ThermalSetpoint` signals. Validation is deferred entirely to equipment receivers. However, only `IdealHvac` performs this validation; the generic `HvacEquipment` path does not.

**Impact**: The deadband between heating and cooling modes can be violated silently at runtime, potentially causing the FSM to cycle between modes or (due to Finding 1's ordering bias) stay locked in one mode. OCHRE avoids this by maintaining separate heating and cooling equipment with single setpoints.

---

### Finding 4: [Severity: medium]
**Description**: The `ThermostatFsm.update_mode()` method resolves simultaneous heating/cooling conditions via an if/else-if chain (`Heating` checked before `Cooling`), making the inversion outcome deterministic but formally incorrect. When setpoints are inverted, the deadband zone collapses and the FSM enters a state where both heating and cooling conditions are simultaneously met at most temperatures, but the branch ordering silently selects Heating.

**Code Location**: `crates/hares-equipment/src/hvac/thermostat.rs:375-380`:
```rust
if zone_temp < heat_turn_on {
    ThermostatMode::Heating
} else if zone_temp > cool_turn_on {
    ThermostatMode::Cooling
} else {
    ThermostatMode::Deadband
}
```

**Root Cause**: The code treats the heat-cool deadband as a naturally ordered range (heat_turn_on < cool_turn_on). When inverted, the range collapses but the branching structure masks the error rather than surfacing it.

**Impact**: When combined with Findings 1-2 (inverted setpoints reach the FSM), the system silently heats when it should cool or vice versa. The error is invisible in simulation output unless users manually inspect thermostat mode vs. zone temperature.

---

### Finding 5: [Severity: low]
**Description**: The `IdealThermostat` actor cannot emit signals to explicitly disable heating or cooling — equivalent to the schedule-level `no_space_heating: true` / `HEATING_DISABLED_SETPOINT_C` (`-999.0`) and `no_space_cooling: true` / `COOLING_DISABLED_SETPOINT_C` (`999.0`) sentinels. Using `Option<f64>::None` in the `ThermalSetpoint` signal means "keep current value" (via `RuntimeSetpointOverride`'s `unwrap_or`), not "disable this mode."

**Code Location**:
- `crates/hares-core/src/actors/ideal_thermostat.rs:258-266` — `ThermalSetpoint` emission uses `Option<f64>`.
- `crates/hares-equipment/src/hvac/thermostat.rs:136-144` — `with_control_override`: `unwrap_or` preserves prior value when `None`.
- `crates/hares-equipment/src/hvac/thermostat.rs:7-8` — sentinel constants `HEATING_DISABLED_SETPOINT_C = -999.0`, `COOLING_DISABLED_SETPOINT_C = 999.0`.

**Root Cause**: The `ThermalSetpoint` control signal treats `None` as no-op rather than explicit disable. There is no separate "disable heating" / "disable cooling" signal variant.

**Impact**: Demand response scenarios requiring per-mode curtailment (e.g., pre-cool and disable heating to prevent heater/cooler fighting during a DR event) cannot be implemented through the `IdealThermostat` actor alone. The sentinel values `-999.0` and `999.0` correctly propagate through the schedule layer and are unambiguously different from valid temperatures, but there is no control signal path to set them from the actor layer.

---

### Finding 6: [Severity: low]
**Description**: The `ControlDispatcher` processes same-tier signals to the same target without detection or warning of conflicts. Within the `UserOverride` tier (index 1), if two `IdealThermostat` actors or an `IdealThermostat` plus another `UserOverride`-level actor dispatch to the same equipment, both are applied sequentially — the second silently overwrites the first with no conflict log.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:408-441` — `drain_tiers()` conflict check: `tier_idx < prev_tier` (reject lower) and `tier_idx > prev_tier` (log overwrite). When `tier_idx == prev_tier`, neither branch matches — the second request overwrites the first silently.

**Root Cause**: The dispatcher's priority-resolution model assumes a strict total ordering across tiers but offers no ordering guarantee or conflict warning within the same tier. This is a design choice, not a bug, but it introduces fragility.

**Impact**: Low practical risk since typically only one `IdealThermostat` is configured per equipment. However, if future composition patterns introduce multiple `UserOverride` sources (e.g., separate DR pre-cool actor + user override actor), their interaction order is nondeterministic.

---

## Summary
- **Total findings**: 6
- **Critical**: 0
- **High**: 2
- **Medium**: 2
- **Low**: 2

## Recommendations

1. **Add release-build setpoint validation in `set_override()` and `decide()`**. Replace `debug_assert!` with a runtime check that returns a `Result` or logs a warning. The validation should use `ThermalSetpoints::validate_for_deadband(hysteresis_c)` to check the `cooling_c - heating_c >= 2 * hysteresis_c` invariant before dispatching.

2. **Add deadband validation to the non-ideal equipment path**. In `apply_heating_control_unchecked()` (helpers.rs:177-197) or `ThermostatFsm.apply_thermal_setpoint_signal()`, validate the candidate runtime setpoints against the static/schedule base using `validate_for_deadband()` before storing. Reject invalid overrides with `HaresError::Control`.

3. **Add a deadband collision check in `update_mode()`**. In the deadband arm of the thermal FSM, assert or log a warning when `heat_turn_on >= cool_turn_on` to surface setpoint inversion. This serves as a defense-in-depth catch for cases that bypass the actor-level validation.

4. **Consider explicit disable signals**. Add `no_space_heating: bool` and `no_space_cooling: bool` fields to `ThermalSetpoint` (or a companion signal) so the `IdealThermostat` can explicitly disable one mode. Alternatively, document that sending `heating_setpoint_c: Some(-999.0)` is the intended disable path and add named constructors for it.

## References / Citations
- OCHRE `HVAC.py` line 217-228: Thermostat deadband/offset initialization. Single-setpoint-per-equipment model avoids the dual-setpoint inversion class of bugs.
- OCHRE `HVAC.py` line 255-320: `update_external_control()` — external setpoint directly overwrites schedule; simpler but no priority-tiered arbitration.
- HARES `thermostat.rs:106-115`: `ThermalSetpoints::validate_for_deadband()` — exists and works, but is only called by `IdealHvac`, not by generic equipment path or the actor.
- HARES `dispatch.rs:15-21`: Priority tier ordering: `Schedule(0) < UserOverride(1) < Grid(2) < Safety(3)`. Correctly places user overrides above schedule but below grid/safety.
- HARES `dwelling/mod.rs:397-443`: `drain_tiers()` — per-step conflict ledger correctly enforces cross-tier priority but is transparent to same-tier collisions.
