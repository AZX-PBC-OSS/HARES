---
id: ACTOR-004
title: Implement IdealHvac equipment
kind: implement
depends_on:
  - ACTOR-001
files_to_touch:
  - crates/hares-types/src/equipment.rs
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
  - crates/hares-equipment/src/hvac/mod.rs
  - crates/hares-equipment/src/registry.rs
references:
  - crates/hares-equipment/src/hvac/furnace.rs
  - crates/hares-equipment/src/hvac/hvac_core.rs
  - crates/hares-equipment/src/hvac/thermostat.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment
---

## Background/Context

IdealHvac is an equipment alternative that replaces the solver-internal ideal HVAC back-calculation. It manages dual heating/cooling setpoints from HPXML 24-hour schedules, determines heating/cooling mode via thermostat logic, receives ideal capacity from the solver (via ControlSignal::IdealCapacity dispatched by SolverFeedbackActor), and writes the load to thermal ports.

Whether ideal capacity mode is used should be simulation-configurable with Auto/On/Off semantics matching OCHRE's behavior:
- **Auto** (default): use ideal capacity when `time_res >= 5 min` or variable-speed equipment (OCHRE rule)
- **On**: force ideal capacity mode regardless of timestep/speed
- **Off**: force dynamic on/off thermostat cycling (duty_cycle = 1.0 when on)

Equipment is self-contained with its own internal setpoint schedules. Actors push external overrides via ControlSignal::ThermalSetpoint.

## Work to Do

- [x] Add `IdealCapacityMode` enum to `crates/hares-types/src/equipment.rs` (shared config): `Auto`, `On`, `Off`
- [x] Create `crates/hares-equipment/src/hvac/ideal_hvac.rs` implementing the `Equipment` trait
- [x] Struct fields: zone_id, static_setpoints, heating_setpoint_source, cooling_setpoint_source (ScheduleSource), schedule_setpoints, runtime_setpoints, deadband_c, ideal_capacity_w, current_target_c, mode, ideal_capacity_mode, rated_capacity_w, cooling_capacity_w, is_variable_speed, last_mode_switch_at
- [x] Use `ScheduleSource` for setpoints (matching `HvacEquipment` pattern) - enables per-timestep interpolated values via `ColumnRef`
- [x] Builder methods: `with_setpoints()`, `with_heating_setpoint_source()`, `with_cooling_setpoint_source()`, `with_ideal_capacity_mode()`
- [x] `resolve_schedule_setpoints(env)`: call `source.value_at(env)` to resolve setpoints - uses pre-interpolated CSV values for ColumnRef, hourly lookup for DailyProfile
- [x] `update_control(env)`: resolve setpoints via ScheduleSource, compare zone temp, determine mode (Heat/Cool/Off), store current_target_c
- [x] `ideal_target()`: when ideal capacity is active (Auto resolves based on `env.time_step_secs() >= 300` OR `is_variable_speed`, On always, Off never), return `Some((zone_id, current_target_c))`. When Off or in deadband, return `None`
- [x] `apply_control_unchecked`: handle `IdealCapacity { capacity_w }` (store it), `ThermalSetpoint` (override setpoints), `ModeOverride` (force off), `LoadFraction` (0 = off)
- [x] `step()`: Ideal mode → write `ideal_capacity_w` to thermal port. Dynamic mode → write `rated_capacity_w` (heating) or `cooling_capacity_w` (cooling) when on, 0 when off
- [x] Descriptor: end_use=HvacHeating, stage=Thermal, capabilities=IDEAL_CAPACITY|THERMAL_SETPOINT|MODE_OVERRIDE
- [x] Export from `hvac/mod.rs`, register as `"Ideal HVAC"` in `registry.rs`
- [x] `is_variable_speed`: set from config (`n_speeds >= 4`), default `false`

## Performance Constraints

- [x] Zero heap allocation in `update_control()`, `step()`, `ideal_target()`, `apply_control_unchecked()`
- [x] Setpoint sources stored as `Option<ScheduleSource>` - reuses established pattern from `HvacEquipment`
- [x] `PortContribution` written by value, not boxed
- [x] `ideal_target()` returns `Option<(ZoneId, f64)>` by value (Copy types)

## Verification

- [x] `cargo build -p hares-equipment` passes
- [x] `cargo test -p hares-equipment` passes
- [x] `cargo clippy -p hares-equipment` passes

## Tests Added

### hares-types
- `ideal_capacity_mode_default_is_auto` - verifies Default trait
- `ideal_capacity_mode_round_trips_through_json` - verifies serde serialization

### hares-equipment (ideal_hvac.rs)
- `ideal_hvac_heating_turns_on_and_writes_capacity` - verifies heating mode engagement and target temperature
- `ideal_hvac_cooling_turns_on_when_zone_hot` - verifies cooling mode engagement
- `ideal_hvac_deadband_returns_none_target` - verifies deadband mode returns None from ideal_target()
- `ideal_hvac_accepts_ideal_capacity_signal` - verifies IdealCapacity control signal handling
- `ideal_hvac_accepts_thermal_setpoint_signal` - verifies ThermalSetpoint control signal handling
- `ideal_hvac_mode_override_forces_off` - verifies ModeOverride signal forces off
- `ideal_hvac_step_writes_ideal_capacity_to_thermal_port` - verifies thermal port output
- `ideal_hvac_state_round_trips` - verifies state serialization/deserialization
- `ideal_hvac_descriptor_has_correct_capabilities` - verifies IDEAL_CAPACITY, THERMAL_SETPOINT, MODE_OVERRIDE caps
- `registry_includes_ideal_hvac` - verifies registration
- `ideal_capacity_mode_auto_engages_at_coarse_timestep` - verifies Auto mode timestep threshold
- `ideal_capacity_mode_on_always_uses_ideal` - verifies On mode always uses ideal
- `ideal_capacity_mode_off_never_uses_ideal` - verifies Off mode never uses ideal
- `variable_speed_forces_ideal_in_auto_mode` - verifies n_speeds >= 4 triggers variable speed
- `load_fraction_zero_forces_off` - verifies LoadFraction=0 forces off
- `column_ref_setpoint_source_reads_schedule_domain` - verifies ColumnRef reads interpolated per-timestep values from schedule CSV
- `with_setpoint_source_builder_method` - verifies builder methods for custom ScheduleSource
- `daily_profile_setpoints_vary_by_hour` - verifies DailyProfile 24-hour schedules

## Post-Review Fixes (2026-03-25)

- **Fixed discarded cooling capacity**: Added `cooling_capacity_w` field and properly store configured cooling capacity (was being computed but discarded with `let _ =`)
- **Fixed is_weekday() stub**: Changed from hardcoded `true` to proper implementation using `!self.is_weekend(dt)` with DateTime parameter
- **Fixed hardcoded hour in setpoint methods**: `current_heating_setpoint()` and `current_cooling_setpoint()` now accept `DateTime<FixedOffset>` parameter to use actual current hour instead of hardcoded `hour = 0`
- **Fixed effective_setpoints() signature**: Updated to accept DateTime parameter and pass it to setpoint lookup methods
- **Replaced hand-rolled schedule lookup with ScheduleSource**: Changed from inline integer hour lookup to using `Option<ScheduleSource>` with `source.value_at(env)` - this matches `HvacEquipment` pattern and enables proper sub-hourly interpolation via `ColumnRef`

## Implementation Notes

### ScheduleSource for Sub-Hourly Resolution Support

The implementation uses `ScheduleSource` for setpoints (matching `HvacEquipment` pattern):
- `heating_setpoint_source: Option<ScheduleSource>` - supports per-timestep interpolated values
- `cooling_setpoint_source: Option<ScheduleSource>` - supports per-timestep interpolated values

**Resolution behavior by source type:**
1. `ColumnRef` - reads **pre-interpolated per-timestep values** from schedule CSV via `env.custom_domains`. This provides full 1-minute resolution support - the CSV column contains values that have already been interpolated by the schedule loader.
2. `DailyProfile` - uses integer hour lookup (step function, no interpolation). This matches OCHRE's behavior for hourly thermostat schedules - setpoint changes only at hour boundaries.
3. `Constant` - fixed value regardless of time.

**Config keys** (via `build_setpoint_source` from `core_config`):
- `{prefix}_setpoint_schedule_col` → `ColumnRef` (per-timestep CSV column, enables 1-min resolution)
- `{prefix}_weekday_setpoints_c` + `{prefix}_weekend_setpoints_c` → `DailyProfile` (hourly step function)

**Builder methods:**
- `with_setpoints(heating_weekday, heating_weekend, cooling_weekday, cooling_weekend)` - creates `DailyProfile` sources
- `with_heating_setpoint_source(source)` - custom `ScheduleSource` for heating
- `with_cooling_setpoint_source(source)` - custom `ScheduleSource` for cooling

**Resolution code** (`resolve_schedule_setpoints`):
```rust
fn resolve_schedule_setpoints(&mut self, env: &EnvironmentState) {
    let heating_c = self.heating_setpoint_source
        .as_mut()
        .and_then(|source| source.value_at(env).ok());
    let cooling_c = self.cooling_setpoint_source
        .as_mut()
        .and_then(|source| source.value_at(env).ok());
    // ...
}
```

This enables actors to use proper interpolated data at 1-minute resolution via the schedule CSV rather than being limited to hourly step functions.

### Separate Heating/Cooling Capacities

The implementation uses two distinct capacity fields:
- `rated_capacity_w`: Heating capacity (positive output in heating mode)
- `cooling_capacity_w`: Cooling capacity (negative output in cooling mode, defaults to 10kW)

This allows different capacities for heating vs cooling systems, common in heat pumps with different heating/cooling ratings.

## Post-Review Fixes - Second Round (2026-03-25)

### CRITICAL Fixes

1. **Cooling capacity sign inversion** (line 402): Fixed `sensible_gain_w: capacity_w.abs()` to `sensible_gain_w: capacity_w`. Cooling now correctly writes negative values to remove heat instead of injecting it.

2. **Replaced hand-rolled schedule with ScheduleSource pattern**: Now uses `Option<ScheduleSource>` with `build_setpoint_source()` + `source.value_at(env)` matching `HvacEquipment`. This enables:
   - `ColumnRef`: Pre-interpolated sub-hourly values from schedule CSV (full 1-min resolution)
   - `DailyProfile`: Hourly step function (matches OCHRE behavior)

### HIGH Priority Fixes

3. **OCHRE-compatible asymmetric thermostat hysteresis**: Added `ThermostatConfig` with `deadband_offset` (default 0.2) and `cutout_ratio` support. The thermostat now uses the same asymmetric deadband model as `HvacEquipment.update_mode()`.

4. **Minimum cycle time lockout and compressor protection**: Added:
   - `is_cycle_change_allowed()` check for thermostat `min_cycle_time_s`
   - `can_transition_mode()` for compressor-level `min_on_time_s` / `min_off_time_s`
   - `mode_start_at` tracking for duration enforcement

5. **`ideal_target()` now guards on mode**: Returns `None` when `IdealCapacityMode::Off` or in deadband. Caches `use_ideal_capacity` result in `use_ideal_cached` during `update_control()` to avoid wasted solver work.

### MEDIUM Priority Fixes

6. **Removed duplicate constant**: Now uses `IDEAL_CAPACITY_TIME_RES_THRESHOLD_S` from `hvac_core.rs` (made `pub`).

7. **Setpoint validation**: Added `validate_for_deadband()` check in `init()` - overlapping setpoints now rejected with error.

8. **LoadFraction partial curtailment**: `LoadFraction` in (0, 1) now scales capacity by `load_fraction` instead of being silently ignored.

### LOW Priority Fixes

9. **Removed `let _ = dt;`**: Changed to `_dt` in signature.

### Additional Tests Added

- `ideal_hvac_cooling_step_writes_negative_to_thermal_port` - catches the critical cooling sign bug
- `ideal_capacity_mode_off_returns_none_from_ideal_target` - verifies Off mode returns None
- `ideal_capacity_mode_off_step_uses_rated_capacity` - verifies rated capacity fallback
- `load_fraction_partial_scales_capacity` - verifies partial curtailment works
- `overlapping_setpoints_rejected` - verifies validation

### State Serialization Updates

`IdealHvacState` now includes:
- `mode_start_at` - for minimum on/off time enforcement across checkpoints
- `load_fraction` - for DR curtailment state preservation