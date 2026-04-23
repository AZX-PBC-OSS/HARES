# Extract Thermostat FSM from HvacEquipment and IdealHvac

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment/hvac, hares-equipment/hvac/thermostat

## Problem

Thermostat FSM logic is fully duplicated between `HvacEquipment` and `IdealHvac`. Five methods (~150 lines total) are copy-pasted across both structs with only minor naming differences. A bug fix in one path may not propagate to the other. This is the single largest DRY violation in the HVAC subsystem.

## Current Behavior

The following methods are duplicated:

| Method | `HvacEquipment` (hvac_core.rs) | `IdealHvac` (ideal_hvac.rs) |
|---|---|---|
| `resolve_profile_setpoints` / `resolve_schedule_setpoints` | lines 629–652 | lines 204–227 |
| `set_mode` | lines 770–776 | lines 240–249 |
| `can_transition_mode` | lines 790–809 | lines 251–266 |
| `update_mode` | lines 654–733 | lines 268–364 |
| `apply_control_signal` (ThermalSetpoint + Delta branches) | lines 572–608 | lines 654–720 |

### Key differences between the two copies

1. **`resolve_*` naming**: `HvacEquipment` calls it `resolve_profile_setpoints` (hvac_core.rs:629); `IdealHvac` calls it `resolve_schedule_setpoints` (ideal_hvac.rs:204). The body is identical.

2. **`set_mode` extra side-effect**: `IdealHvac::set_mode` clears `ideal_capacity_w = 0.0` on Deadband entry (ideal_hvac.rs:245–247). `HvacEquipment::set_mode` has no such branch. This is an `IdealHvac`-specific concern.

3. **`update_mode` target tracking**: `IdealHvac::update_mode` maintains `current_target_c` before and after the mode decision (ideal_hvac.rs:275–281, 355–361). `HvacEquipment::update_mode` does not.

4. **`apply_control_signal`**: `IdealHvac` adds validation via `validate_runtime_override` (ideal_hvac.rs:668, 701), deadband update (ideal_hvac.rs:669–673), `ModeOverride` handling (ideal_hvac.rs:675–683), `IdealCapacityModeOverride` (ideal_hvac.rs:703–704), and `LoadFraction` (ideal_hvac.rs:706–715). `HvacEquipment` handles only `ThermalSetpoint`, `ThermalSetpointDelta`, and `MaxCapacityFraction` (hvac_core.rs:572–608).

5. **Shared state**: Both structs carry `mode`, `mode_start_at`, `last_mode_switch_at`, `thermostat: ThermostatConfig`, `static_setpoints`, `schedule_setpoints`, `runtime_setpoints`, `heating_setpoint_source`, `cooling_setpoint_source`, `min_on_time_s`, `min_off_time_s`.

## Required Behavior

All thermostat FSM logic must live in a single `ThermostatFsm` struct. Both `HvacEquipment` and `IdealHvac` embed `thermostat_fsm: ThermostatFsm` and delegate to it. No behavioral change — all existing tests must pass unchanged.

## Approach

### Step 1: Define `ThermostatFsm` in `thermostat.rs`

Add to `crates/hares-equipment/src/hvac/thermostat.rs`:

```rust
pub struct ThermostatFsm {
    pub mode: ThermostatMode,
    pub mode_start_at: Option<DateTime<FixedOffset>>,
    pub last_mode_switch_at: Option<DateTime<FixedOffset>>,
    pub thermostat: ThermostatConfig,
    pub static_setpoints: ThermalSetpoints,
    pub schedule_setpoints: Option<ScheduleSetpoints>,
    pub runtime_setpoints: Option<RuntimeSetpointOverride>,
    pub heating_setpoint_source: Option<ScheduleSource>,
    pub cooling_setpoint_source: Option<ScheduleSource>,
    pub min_on_time_s: f64,
    pub min_off_time_s: f64,
}
```

### Step 2: Move the five duplicated methods onto `ThermostatFsm`

Implement on `ThermostatFsm`:

- `resolve_profile_setpoints(&mut self, env: &EnvironmentState)` — unified name for the identical logic from hvac_core.rs:629–652 / ideal_hvac.rs:204–227
- `set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>)` — the base logic from hvac_core.rs:770–776 (no `ideal_capacity_w` clear; that stays in `IdealHvac` as a post-hook)
- `can_transition_mode(&self, proposed: ThermostatMode, now: DateTime<FixedOffset>) -> bool` — identical in both (hvac_core.rs:790–809 / ideal_hvac.rs:251–266)
- `update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode>` — the core hysteresis + cycle-time logic (hvac_core.rs:654–733 / ideal_hvac.rs:268–364), but **without** the `IdealHvac`-specific `current_target_c` tracking. `IdealHvac` wraps the delegation and adds target tracking around it.
- `effective_setpoints(&self) -> ThermalSetpoints` — already exists in thermostat.rs as `ThermalSetpoints::with_schedule_override` + `with_control_override`; provide a convenience method.
- `apply_thermal_setpoint_signal(&mut self, signal: &ControlSignal)` — the `ThermalSetpoint` and `ThermalSetpointDelta` branches from hvac_core.rs:572–601. The `IdealHvac`-specific validation (`validate_runtime_override`) and deadband update remain in `IdealHvac` as a post-hook.

### Step 3: Embed `ThermostatFsm` in `HvacEquipment`

Replace the 11 scattered fields (mode, mode_start_at, last_mode_switch_at, thermostat, static_setpoints, schedule_setpoints, runtime_setpoints, heating_setpoint_source, cooling_setpoint_source, min_on_time_s, min_off_time_s — confirmed 11 by inspection of hvac_core.rs:122–249) with `pub thermostat_fsm: ThermostatFsm`.

Update all call sites in `hvac_core.rs` and `staging.rs` to go through `self.thermostat_fsm.mode`, `self.thermostat_fsm.update_mode(env)`, etc. The access pattern `self.mode` becomes `self.thermostat_fsm.mode`; a convenience `Deref`/`DerefMut` is NOT recommended because it hides the boundary — explicit delegation keeps the FSM boundary visible.

### Step 4: Embed `ThermostatFsm` in `IdealHvac`

Same field replacement. `IdealHvac` retains its extra fields (`ideal_capacity_w`, `current_target_c`, etc.) and adds wrapper methods:

```rust
fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>) {
    let prev = self.thermostat_fsm.mode;
    self.thermostat_fsm.set_mode(mode, when);
    if prev != mode && mode == ThermostatMode::Deadband {
        self.ideal_capacity_w = 0.0;
    }
}
```

`update_mode` wrapper updates `current_target_c` before/after delegating.

`apply_control_unchecked` delegates `ThermalSetpoint` / `ThermalSetpointDelta` to the FSM, then applies `validate_runtime_override` and deadband update.

### Step 5: Update `mod.rs` re-exports

Add `ThermostatFsm` to the `pub use thermostat::...` line in `mod.rs:33–35`.

### Step 6: Run full test suite

```
cargo test -p hares-equipment
```

All existing tests must pass with zero behavioral change.

## Definition of Done

- [ ] `ThermostatFsm` struct defined in `thermostat.rs` with all 5 previously-duplicated methods
- [ ] `HvacEquipment` embeds `ThermostatFsm` instead of the 11 individual fields (confirmed count: hvac_core.rs:122–249)
- [ ] `IdealHvac` embeds `ThermostatFsm` instead of the 11 individual fields (same 11 fields, confirmed)
- [ ] No method named `resolve_profile_setpoints` or `resolve_schedule_setpoints` exists on `HvacEquipment` or `IdealHvac` — only `ThermostatFsm::resolve_profile_setpoints`
- [ ] `IdealHvac`-specific side-effects (`ideal_capacity_w` clear, `current_target_c` update, `validate_runtime_override`) remain in `IdealHvac` as wrapper logic around FSM delegation
- [ ] `ThermostatFsm` is re-exported from `mod.rs`
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. `cargo clippy -p hares-equipment` — no new warnings
3. Grep for `resolve_profile_setpoints` and `resolve_schedule_setpoints` — only found on `ThermostatFsm`
4. Grep for direct assignment to `.mode = ` outside of `ThermostatFsm::set_mode` — should only be in init/constructors, never in control flow
5. Confirm the `IdealHvac::set_mode` wrapper still clears `ideal_capacity_w` on Deadband entry

## References

- `hvac_core.rs:629–809` — `HvacEquipment` thermostat FSM methods
- `ideal_hvac.rs:204–364` — `IdealHvac` thermostat FSM methods
- `thermostat.rs:1–184` — existing `ThermostatConfig`, `ThermostatMode`, `is_cycle_change_allowed`, `lookup_zone_temp`
- `mod.rs:33–35` — current re-exports from `thermostat` module

## Related Tickets

- 008-decompose-hvacequipment-struct — depends on this ticket; the struct decomposition is easier once thermostat fields are already grouped into `ThermostatFsm`
