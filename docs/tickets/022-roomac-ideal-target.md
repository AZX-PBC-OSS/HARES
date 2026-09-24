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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **Line numbers match** — All cited locations confirmed against current source:
  - `air_conditioner.rs:280-325` — RoomAC `Equipment` impl block, 10 methods, `ideal_target()` absent ✓
  - `air_conditioner.rs:275-278` — AirConditioner `ideal_target()` delegating to `self.core.ideal_target()` ✓
  - `air_conditioner.rs:1364-1374` — `CoolingCore::ideal_target()` implementation ✓
  - `air_conditioner.rs:220-227` — RoomAC struct and constructor with `CoolingCore::new(config, true)` ✓
  - `air_conditioner.rs:489` — `CoolingCore::new()` `use_ideal: false` initialization ✓
  - `air_conditioner.rs:662` — `use_ideal = self.hvac.use_ideal_capacity(env)` in `update_control()` ✓
  - `equipment.rs (lib.rs):151-153` — Default `ideal_target()` returning `None` ✓
  - `hvac_core.rs:736-761` — `use_ideal_capacity()` method ✓
  - `hvac_core.rs:750-757` — `supports_auto_ideal` includes `AcCooler` ✓
  - `hvac_core.rs:22` — `IDEAL_CAPACITY_TIME_RES_THRESHOLD_S = 300` (5 min) ✓

- [x] **Described logic matches current implementation** — Every structural claim in the ticket is accurate. RoomAC genuinely lacks `ideal_target()`, the default trait returns `None`, and AirConditioner correctly delegates.

- [x] **Visibility note corrected** — Ticket Step 1 says visibility must change to `pub(super)`. This is **overly cautious but harmless**: `CoolingCore::ideal_target()` at line 1364 is `fn` (fully private), but Rust allows access to private struct methods from within the same module (`air_conditioner.rs` is a single module with no submodules containing either `RoomAC` or `CoolingCore`). No visibility change is strictly required; `self.core.ideal_target()` is already callable from the RoomAC impl block.

- [x] **OCHRE cross-check**: **Matches OCHRE intent, diverges in architecture**.
  - Vendored `vendors/OCHRE/ochre/Equipment/HVAC.py:1080-1086`: `RoomAC(AirConditioner)` has only `__init__`. It overrides nothing — inherits `use_ideal_capacity`, `solve_ideal_capacity`, `update_capacity` fully from `AirConditioner`.
  - OCHRE `HVAC.py:239-241`: `use_ideal_capacity = self.time_res >= dt.timedelta(minutes=5) or self.n_speeds >= 4` — single-speed RoomAC activates ideal mode at ≥5 min, exactly as HARES.
  - **Architecture divergence (intentional)**: OCHRE calls `envelope_model.solve_for_inputs()` internally from `solve_ideal_capacity()` (HVAC.py:411-429). HARES uses an external solver that queries `ideal_target()` from equipment and dispatches an `IdealCapacity` signal back. HARES's design is a deliberate inversion of control; the bug is that RoomAC breaks out of this loop by returning `None`.

- [x] **EnergyPlus cross-check** (BigLadder EnergyPlus 24.1 Engineering Reference):
  - The EnergyPlus `ZoneHVAC:IdealLoadsAirSystem` calculates required supply flow as `ṁs = Q̇z / (cp,air · (Ts − Tz))`. This is a different abstraction (ideal VAV, not equipment-level capacity back-solve).
  - EnergyPlus's ideal loads system is specific to `ZoneHVAC:IdealLoadsAirSystem` and is not a generic pattern applicable to room ACs. The HARES/OCHRE concept (all single-zone cooling equipment participating in a capacity solver at coarse timesteps) has no direct EnergyPlus equivalent in this form. **N/A — no divergence, different concept.**

### Web-Verified Citations

The ticket contains **no standards citations** (no ASHRAE, NFRC, DOE, ISO, or EnergyPlus section numbers). All references are to internal source files.

The implicit reference to OCHRE's 5-minute threshold for `use_ideal_capacity` is verified:

- **Citation**: "timestep is >= 5 minutes (triggering auto-ideal mode)" and "IDEAL_CAPACITY_TIME_RES_THRESHOLD_S = 300"
- **Source found**: OCHRE HVAC.py on GitHub (https://github.com/NREL/OCHRE) and vendored copy at `vendors/OCHRE/ochre/Equipment/HVAC.py:239-241`
- **Quoted passage**:
  ```python
  if use_ideal_capacity is None:
      use_ideal_capacity = self.time_res >= dt.timedelta(minutes=5) or self.n_speeds >= 4
  self.use_ideal_capacity = use_ideal_capacity
  ```
- **Verdict**: Confirmed. HARES's 300-second threshold (`IDEAL_CAPACITY_TIME_RES_THRESHOLD_S`) matches OCHRE's `timedelta(minutes=5)` exactly.

- **Citation**: "RoomAC is type `AcCooler` which is in `supports_auto_ideal` at line 752"
- **Source found**: `crates/hares-equipment/src/hvac/hvac_core.rs:750-757` (local, cross-checked against OCHRE pattern above)
- **Quoted passage**:
  ```rust
  let supports_auto_ideal = matches!(
      self.equipment_type,
      HvacEquipmentType::AcCooler
          | HvacEquipmentType::MiniSplitCool
          | HvacEquipmentType::AshpHeatPumpOnly
          | HvacEquipmentType::AshpHeatPumpAux
          | HvacEquipmentType::MiniSplitHeat
  );
  ```
- **Verdict**: Confirmed. `AcCooler` is the equipment type assigned to RoomAC (`air_conditioner.rs:437-441`).

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: Every structural and behavioral claim in the ticket is accurate. The code confirms that (1) `impl Equipment for RoomAC` at lines 280-325 has no `ideal_target()` method; (2) `AirConditioner` at line 275-278 does delegate to `CoolingCore::ideal_target()`; (3) `CoolingCore::ideal_target()` is fully implemented and checks `use_ideal` + `operating_mode`; (4) `use_ideal_capacity()` in `hvac_core.rs:736-761` correctly enables ideal mode for `AcCooler` at coarse timesteps; and (5) the default `Equipment::ideal_target()` returns `None`. The OCHRE cross-check confirms that OCHRE's `RoomAC` inherits ideal-capacity participation from `AirConditioner` without override — the intent is for RoomAC to fully participate. Two regression tests (`room_ac_ideal_target_returns_some_at_coarse_timestep` and `room_ac_ideal_target_matches_air_conditioner_parity`) were written and confirmed to **fail** with the current code, demonstrating the bug. One minor ticket inaccuracy: Step 1 claims `CoolingCore::ideal_target()` needs `pub(super)` visibility, but since all code is in the same Rust module, no visibility change is required — the `fn` (private) method is already accessible.

### Proposed Fix Summary

Add one method to `impl Equipment for RoomAC` in `air_conditioner.rs` at line 325 (after `apply_control_unchecked`):

```rust
fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
    self.core.ideal_target()
}
```

No visibility change to `CoolingCore::ideal_target()` is needed. The method is private but accessible within the same module. The fix is a single 3-line addition identical to AirConditioner's `ideal_target()` delegation at lines 275-277.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/air_conditioner.rs` — appended to `mod ideal_capacity_tests`
- **Tests added** (3 tests):
  1. `room_ac_ideal_target_returns_some_at_coarse_timestep` — **FAILS** until fix applied. Verifies RoomAC returns `Some((ZoneId(1), ~24.0°C))` at 900 s timestep with zone above cooling turn-on threshold.
  2. `room_ac_ideal_target_matches_air_conditioner_parity` — **FAILS** until fix applied. Verifies RoomAC and AirConditioner return identical `ideal_target()` for identical inputs at coarse timestep.
  3. `room_ac_ideal_target_returns_none_at_fine_timestep` — **Passes** now and after fix. Verifies `ideal_target()` returns `None` at 60 s timestep where `use_ideal=false`.
