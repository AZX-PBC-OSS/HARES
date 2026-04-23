# Speed/Startup Internal State Telemetry Gaps

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac, hares-types

## Problem

Several important internal state values are computed but never exposed to telemetry. These values are essential for validating HVAC performance and debugging speed/staging behavior, but they are invisible to both the output system and external observers:

| Value | Computed In | Written to Telemetry | Used Internally |
|---|---|---|---|
| `speed_frac` | `staging.rs:86-87,117` | ❌ No | Interpolation weight between speed stages |
| `part_load_ratio` (PLR) | `staging.rs:83-87,109,224-226` | ❌ No | Cycling fraction at lowest speed |
| `part_load_factor` (PLF) | `staging.rs:268-312` | ❌ No (stored in `plf_state`) | EIR degradation correction |
| `startup_multiplier` | `speed_control.rs:71-97` | ❌ No | Capacity ramp on compressor restart |
| `duty_cycle` | `hvac_core.rs` (field) | ❌ No | Thermostat on/off fraction |
| `time_at_current_speed_s` | `staging.rs:66,96,131,182` | ❌ No | Minimum time-per-speed guard |
| `mode_duration_s` | computed from `mode_start_at` | ❌ No | Minimum on/off time enforcement |

## Current Behavior

### `speed_frac` — never written

The `SpeedSelection` struct at `speed_control.rs:100-110` computes `speed_frac` (interpolation weight) for every speed selection. It is stored in `hvac.last_speed_frac` at `staging.rs:117` but never written to telemetry. The AC checkpoint state saves it at `air_conditioner.rs:117` (`last_speed_frac`), confirming it's important enough to persist across checkpoints.

### `part_load_ratio` — never written

PLR is the cycling fraction computed in `select_speed_with_zone_temp()` and returned in `SpeedSelection::part_load_ratio`. It determines whether the unit is cycling at the lowest speed (PLR < 1) or running continuously (PLR = 1). The value is used internally in `calculate_performance()` at `air_conditioner.rs:1095-1109` but never exposed.

### `part_load_factor` — stored internally, not exposed

`plf_state` is set at `staging.rs:278,310` inside `part_load_factor_for_stage()`. This value directly affects EIR (EIR is divided by PLF at `air_conditioner.rs:1117-1121`). The staging module does log a `tracing::warn!` at line 301 when PLF is below floor, but the actual PLF value is not available as telemetry.

### `startup_multiplier` — computed and applied, never exposed

`StartupConfig::capacity_multiplier()` at `speed_control.rs:71-97` computes the Winkler exponential ramp multiplier. The result is used to degrade capacity in `apply_startup_capacity_degradation()` at `staging.rs:320-322`. The multiplier value and `time_since_start_min` are stored in `hvac.startup` but never written to telemetry. The AC checkpoint saves them at `air_conditioner.rs:113-114`, confirming they're important.

### `duty_cycle` — HvacEquipment field, never exposed

`hvac.duty_cycle` is set in `update_control()` (e.g., `air_conditioner.rs:716-727`) based on speed selection PLR. It's the thermostat's raw on/off fraction. Not written to telemetry.

### `time_at_current_speed_s` — stored, not exposed

Advanced by `advance_speed_timer()` at `staging.rs:66`, reset at lines 96, 131, 182. Used as a guard in `select_two_speed_setpoint()` and `select_two_speed_time()`. Saved in AC checkpoint state at `air_conditioner.rs:137` but not in telemetry.

### `mode_duration_s` — not even computed, but `mode_start_at` is stored

`hvac.mode_start_at` is set in `set_mode()` at `hvac_core.rs:774`. The duration is computed on-the-fly in `can_transition_mode()` at line 801 but not persisted. To expose `mode_duration_s`, compute `(now - mode_start_at)` during step and write it.

## Required Behavior

All 7 internal values must be available as telemetry keys so they can be:
1. Inspected at runtime via the observer
2. Written to output columns at high verbosity
3. Used for post-hoc analysis and validation against OCHRE

## Approach

### Step 1: Add new telemetry key constants in `telemetry_keys.rs`

```rust
// ── HVAC speed/staging ─────────────────────────────────────────────────────
pub const SPEED_FRAC: &str = "speed_frac";
pub const PART_LOAD_RATIO: &str = "part_load_ratio";
pub const PART_LOAD_FACTOR: &str = "part_load_factor";
pub const STARTUP_MULTIPLIER: &str = "startup_multiplier";
pub const DUTY_CYCLE: &str = "duty_cycle";
pub const TIME_AT_CURRENT_SPEED_S: &str = "time_at_current_speed_s";
pub const MODE_DURATION_S: &str = "mode_duration_s";
```

### Step 2: Add `TelemetryField` descriptors

In the AC/furnace telemetry_fields() functions, add descriptors for each new key with appropriate units and descriptions.

### Step 3: Write telemetry in `step()` methods

For each HVAC equipment that uses `HvacEquipment`, add `telemetry.set()` calls at the end of `step()`:

- `speed_frac`: `self.hvac.last_speed_frac` — available from `staging.rs:117`
- `part_load_ratio` (`PART_LOAD_RATIO_W`): `SpeedSelection::part_load_ratio` from speed selection, written immediately after speed selection (start of step)
- `part_load_factor`: `self.hvac.plf_state` — set by `part_load_factor_for_stage()` at `staging.rs:278,310`
- `startup_multiplier`: call `self.hvac.startup.capacity_multiplier()` directly — `capacity_multiplier()` at `speed_control.rs:71` is a simple exponential; calling it twice per step is negligible, so no caching field is needed
- `duty_cycle` (`DUTY_CYCLE`): `self.hvac.duty_cycle` — written after `update_control()` (end of step)
- `time_at_current_speed_s`: `self.hvac.time_at_current_speed_s`
- `mode_duration_s`: `(env.current_time - self.hvac.mode_start_at.unwrap_or(env.current_time)).num_milliseconds() as f64 / 1000.0`

**Timing invariant**: `PART_LOAD_RATIO_W` is written after speed selection (start of step); `DUTY_CYCLE` is written after `update_control()` (end of step). For single-speed equipment these two values must be equal at the end of every step. Any divergence between them indicates a step-ordering bug and must be treated as a defect.

### Step 4: Add keys to default telemetry initialization

In `ac_config.rs:default_telemetry()` and each equipment's `default_telemetry()` / `xxx_default_telemetry()`, add `telemetry.insert()` calls for the new keys with initial value 0.0.

### Step 5: (Removed) No startup multiplier cache needed

The proposed `last_startup_multiplier: f64` field on `HvacEquipment` is dropped. `capacity_multiplier()` at `speed_control.rs:71` is a simple exponential computation — calling it a second time per step for telemetry is negligible cost. Adding a cache field for it would increase struct surface area for no meaningful gain.

## Definition of Done

- [ ] `SPEED_FRAC` telemetry key constant defined
- [ ] `PART_LOAD_RATIO` telemetry key constant defined
- [ ] `PART_LOAD_FACTOR` telemetry key constant defined
- [ ] `STARTUP_MULTIPLIER` telemetry key constant defined
- [ ] `DUTY_CYCLE` telemetry key constant defined
- [ ] `TIME_AT_CURRENT_SPEED_S` telemetry key constant defined
- [ ] `MODE_DURATION_S` telemetry key constant defined
- [ ] All 7 keys written in `CoolingCore::step()`, `ElectricFurnace::step()`, `GasFurnace::step()`, and heat pump equipment steps
- [ ] All 7 keys initialized in default telemetry constructors
- [ ] `TelemetryField` descriptors added for all 7 keys
- [ ] Existing tests pass (new keys default to 0.0; no behavioral change)

## Verification

1. Run a multi-speed AC simulation and verify:
   - `speed_frac` varies between 0.0 and 1.0 during inter-speed interpolation
   - `part_load_ratio` < 1.0 when cycling at lowest speed
   - `part_load_factor` = `1 - Cd * (1 - PLR)` for single-speed cycling
2. Run a single-speed AC with `c_d = 0.25` from cold start and verify:
   - `startup_multiplier` ramps from ~0.0 to 1.0 over `t_full` minutes
   - `mode_duration_s` increments while in a mode, resets on mode change
3. Verify `duty_cycle` matches `part_load_ratio` for single-speed equipment (they should be equal).

## References

- `speed_control.rs:100-110`: `SpeedSelection` struct with `speed_frac`, `part_load_ratio`
- `staging.rs:69-119`: `select_speed_with_zone_temp()` — computes `speed_frac` and PLR
- `staging.rs:268-312`: `part_load_factor_for_stage()` — computes PLF, stores in `plf_state`
- `staging.rs:315-323`: `apply_startup_capacity_degradation()` — applies startup multiplier
- `speed_control.rs:71-97`: `StartupConfig::capacity_multiplier()` — Winkler ramp formula
- `hvac_core.rs:770-776`: `set_mode()` — records `mode_start_at`
- `hvac_core.rs:790-809`: `can_transition_mode()` — computes `elapsed_s` from `mode_start_at`
- `telemetry_keys.rs:1-164`: Existing telemetry key constants

## Ordering

**This ticket must ship before ticket 017's RTF column work.** Ticket 017's `{name} Runtime Fraction (-)` column reads from `RUNTIME_FRACTION` (already exists), but the PLR/PLF columns added in 017 read from the `PART_LOAD_RATIO` and `PART_LOAD_FACTOR` telemetry keys introduced here. Implementing 017 before 019 means those column entries would have no backing keys.

## Related Tickets

- #016 — Thermostat FSM decision tracing (tracing complements telemetry — tracing for real-time diagnostics, telemetry for persistent output)
- #017 — Missing output columns v7 (PLR/PLF column work in 017 depends on keys from this ticket; ship 019 first)
- #018 — CoreOutput HVAC promotion (some of these values like speed_index and COP may move to CoreOutput)
