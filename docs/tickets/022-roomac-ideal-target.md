# RoomAC Missing ideal_target() Implementation

**Severity**: Medium
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment/hvac

## Problem

RoomAC does not implement `ideal_target()`. The `Equipment` impl for RoomAC at `air_conditioner.rs:280-325` lacks this method, meaning RoomAC cannot participate in solver-driven ideal-capacity computation even though it inherits the `use_ideal` flag from `CoolingCore`.

When the thermal solver back-calculates the ideal capacity needed to reach a zone setpoint, it queries `ideal_target()` on each equipment. RoomAC returns `None` (the default from `equipment.rs:151-153`), so the solver ignores it even when:
- The user has set `use_ideal_capacity = true` in the RoomAC config
- The timestep is >= 5 minutes (triggering auto-ideal mode)
- The thermostat is calling for cooling

This is a correctness gap: the solver cannot account for RoomAC's contribution, leading to unmet cooling loads or incorrect capacity allocation.

## Current Behavior

### RoomAC `Equipment` impl at `air_conditioner.rs:280-325`

```rust
impl Equipment for RoomAC {
    fn descriptor(&self) -> &EquipmentDescriptor { &self.core.descriptor }
    fn ports(&self) -> &[PortDeclaration] { &self.core.ports }
    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> { ... }
    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode { ... }
    fn step(&mut self, env: &EnvironmentState, dt: Duration, ports: &mut PortSlots) -> ... { ... }
    fn telemetry(&self) -> &Telemetry { &self.core.telemetry }
    fn core_output(&self) -> &CoreOutput { &self.core.core_output }
    fn save_state(&self) -> Vec<u8> { ... }
    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> { ... }
    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> { ... }
    // Missing: fn ideal_target(&self) -> Option<(ZoneId, f64)>
}
```

No `ideal_target()` method. The default implementation from `equipment.rs:151-153` returns `None`.

### AirConditioner `Equipment` impl at `air_conditioner.rs:275-278`

```rust
fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
    self.core.ideal_target()
}
```

AirConditioner delegates to `CoolingCore::ideal_target()`.

### CoolingCore `ideal_target()` at `air_conditioner.rs:1364-1374`

```rust
fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
    if !self.use_ideal {
        return None;
    }
    if self.operating_mode != OperatingMode::Cooling {
        return None;
    }
    let setpoint = self.hvac.effective_setpoints().cooling_c + self.dr_setpoint_offset_c;
    Some((self.hvac.zone_id, setpoint))
}
```

This is a private method on `CoolingCore` — RoomAC has access to the same `self.core` field but doesn't wire it through.

### Default `ideal_target()` at `equipment.rs:151-153`

```rust
fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
    None
}
```

### RoomAC has `use_ideal` flag

RoomAC constructs `CoolingCore` with `is_room_ac: true` at `air_conditioner.rs:223-225`, and `CoolingCore::new()` initializes `use_ideal: false` at line 489. In `update_control()`, the flag is set from `self.hvac.use_ideal_capacity(env)` at `air_conditioner.rs:662`. RoomAC goes through the same `update_control()` path as AirConditioner (both use `CoolingCore::update_control()`), so `use_ideal` is correctly computed.

### RoomAC is single-speed but can still use ideal capacity

The `use_ideal_capacity()` method at `hvac_core.rs:736-761` determines whether to use ideal mode based on:
- `thermostat.use_ideal_capacity` flag
- Variable-speed mode
- 4+ speed stages
- Coarse timestep (>= 5 min) for `supports_auto_ideal` equipment types

RoomAC is type `AcCooler` which is in `supports_auto_ideal` at line 752. So even though RoomAC is single-speed, it will use ideal capacity mode when the timestep is >= 5 minutes. But since `ideal_target()` returns `None`, the solver cannot use it.

## Required Behavior

RoomAC must implement `ideal_target()` following the same pattern as AirConditioner: delegate to `CoolingCore::ideal_target()` which checks `use_ideal`, `operating_mode`, and returns the zone + setpoint.

## Approach

### Step 1: Make `CoolingCore::ideal_target()` pub(super)

The method at `air_conditioner.rs:1364-1374` is currently private to `CoolingCore`. Change visibility to `pub(super)` so the RoomAC impl block can call `self.core.ideal_target()`.

Actually, looking at the code: `CoolingCore` is `pub(super)` and `RoomAC` has `core: CoolingCore` (line 41), and they're in the same module. So `self.core.ideal_target()` should already be accessible from `RoomAC`'s impl block within the same `air_conditioner.rs` file. The method just needs to be `pub(crate)` or `pub(super)` instead of private.

### Step 2: Add `ideal_target()` to RoomAC `Equipment` impl

At `air_conditioner.rs:280-325`, add:

```rust
fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
    self.core.ideal_target()
}
```

This is exactly the same delegation pattern as AirConditioner at line 275-278.

### Step 3: Test the fix

Add a test that creates a RoomAC, sets a coarse timestep (>= 5 min), calls `update_control()`, and verifies `ideal_target()` returns `Some((zone_id, setpoint))` when in cooling mode.

## Definition of Done

- [ ] RoomAC `Equipment` impl includes `ideal_target()` delegating to `self.core.ideal_target()`
- [ ] `CoolingCore::ideal_target()` has sufficient visibility for the RoomAC impl
- [ ] RoomAC returns `Some((zone_id, cooling_setpoint))` when `use_ideal` is true and mode is Cooling
- [ ] RoomAC returns `None` when mode is Off or Deadband (same as AirConditioner)
- [ ] RoomAC returns `None` when `use_ideal` is false (same as AirConditioner)
- [ ] Test added verifying RoomAC ideal_target behavior
- [ ] Existing tests pass

## Verification

1. Create a RoomAC with coarse timestep (15 min), zone temp below cooling setpoint.
2. Call `update_control()` — verify `operating_mode == Cooling`.
3. Call `ideal_target()` — verify it returns `Some((zone_id, cooling_setpoint_c))`.
4. Compare with AirConditioner behavior: same input conditions should produce the same `ideal_target()` output.
5. With fine timestep (1 min), verify `use_ideal` is false and `ideal_target()` returns `None`.

## References

- `air_conditioner.rs:280-325`: RoomAC `Equipment` impl (missing `ideal_target()`)
- `air_conditioner.rs:275-278`: AirConditioner `ideal_target()` delegation pattern
- `air_conditioner.rs:1364-1374`: `CoolingCore::ideal_target()` implementation
- `air_conditioner.rs:220-227`: RoomAC struct and constructor
- `equipment.rs:151-153`: Default `ideal_target()` returning `None`
- `hvac_core.rs:736-761`: `use_ideal_capacity()` determination (includes AcCooler in `supports_auto_ideal`)

## Related Tickets

- #007 — Unify variable speed selection (RoomAC is constrained to SingleSpeed but still participates in ideal mode at coarse timesteps)
