# Thermostat FSM Decision Tracing

**Severity**: High
**Priority**: P0
**Status**: Open
**Areas**: hares-equipment/hvac, hares-types

## Problem

Zero `tracing` calls in the thermostat FSM. Cannot diagnose WHY mode transitions happen or are blocked. Only 5 `tracing` calls exist across the entire HVAC crate: 1 `debug!` in staging (PLF stage-index out of range, `staging.rs:287`) and 4 `warn!` calls at `hvac_core.rs:530`, `ac_config.rs:185`, `staging.rs:301`, and `duct_distribution.rs:27` — none in the thermostat decision path. A developer seeing `mode = Deadband` when they expected `Heating` cannot determine whether:

- The zone temperature was above the turn-on threshold
- `is_cycle_change_allowed()` blocked the transition due to `min_cycle_time_s`
- `can_transition_mode()` blocked due to minimum on/off time enforcement
- The setpoint was overridden by schedule or runtime control signal

This is the single most impactful debuggability improvement for HVAC.

## Current Behavior

1. **`update_mode()` at `hvac_core.rs:654-733`**: Computes `zone_temp`, `effective_setpoints()`, `next_mode`, then checks two guards (`is_cycle_change_allowed`, `can_transition_mode`) — all silently. When a guard blocks, the method returns `self.mode` with no trace of what happened or why. The method has 80 lines of branching logic with zero diagnostic output.

2. **`is_cycle_change_allowed()` at `thermostat.rs:171-184`**: Returns `false` when `min_cycle_time_s` has not elapsed since `last_mode_switch_at`. No log of the block reason, the elapsed time, or the required wait.

3. **`can_transition_mode()` at `hvac_core.rs:790-809`**: Returns `false` when `min_on_time_s` or `min_off_time_s` has not elapsed since `mode_start_at`. No log of which constraint is active, elapsed time, or remaining wait.

4. **Speed changes at `staging.rs:69-119`**: `select_speed_with_zone_temp()` updates `last_speed_index` and `last_speed_frac` silently. No trace of speed transitions or the `time_at_current_speed_s` timer that gates them.

5. **Startup degradation at `speed_control.rs:71-97`**: `capacity_multiplier()` computes the Winkler exponential ramp multiplier and applies it to capacity via `apply_startup_capacity_degradation()` at `staging.rs:315-323`. Neither the multiplier value nor `time_since_start_min` is ever logged.

6. **Existing `tracing` calls**: The only `tracing::debug!` in HVAC is at `staging.rs:287` (PLF stage-index out of range). There are 4 active `tracing::warn!` call sites: `hvac_core.rs:530` (invalid DSE), `ac_config.rs:185` (config warning), `staging.rs:301` (PLF curve below floor), and `duct_distribution.rs:27` (duct_zone == conditioned_zone with DSE < 1.0). `duct_distribution.rs` imports `use tracing::warn;` at line 9; the import has one active call site at line 27. None are in the thermostat decision path.

## Required Behavior

Every decision point in the thermostat FSM must emit a `tracing::debug!` event when debug logging is enabled, containing enough information to reconstruct the decision after the fact:

1. When `update_mode()` completes: log `zone_temp`, `effective_setpoints`, `current_mode`, `next_mode`, and which threshold triggered (or would have triggered) the transition.
2. When `is_cycle_change_allowed()` blocks: log the block reason (`min_cycle_time_s` not elapsed), the elapsed time, and the required minimum.
3. When `can_transition_mode()` blocks: log the reason (`min_on_time_s` or `min_off_time_s`), time elapsed, time remaining.
4. When speed changes: log `old_speed_index`, `new_speed_index`, `speed_frac`, `part_load_ratio`.
5. When startup degradation is active: log `startup_multiplier`, `c_d`, `time_since_start_min`.
6. Zero performance cost when debug logging is disabled (tracing macros are compiled out at build time when no subscriber enables the DEBUG level).

## Approach

### Step 1: Add `tracing::debug!` to `update_mode()` in `hvac_core.rs:654`

After computing `zone_temp` and `setpoints` (line 656-657), add a debug log of the input state:

```rust
tracing::debug!(
    zone_temp,
    heating_setpoint = setpoints.heating_c,
    cooling_setpoint = setpoints.cooling_c,
    current_mode = ?self.mode,
    "update_mode: evaluating mode transition"
);
```

After computing `next_mode` (line 722), before the guard checks, log the computed next mode:

```rust
tracing::debug!(
    current_mode = ?self.mode,
    next_mode = ?next_mode,
    offset,
    hysteresis_c = hysteresis,
    "update_mode: computed next_mode"
);
```

### Step 2: Add `tracing::debug!` to `is_cycle_change_allowed()` in `thermostat.rs:171`

When returning `false` (line 182-183 condition):

```rust
tracing::debug!(
    elapsed_s = elapsed_ms / 1000.0,
    min_cycle_time_s = thermostat.min_cycle_time_s,
    "is_cycle_change_allowed: blocked by min_cycle_time"
);
```

### Step 3: Instrument `can_transition_mode` blocks at the call site in `update_mode()` (`hvac_core.rs:728`)

Place the trace at the call site in `update_mode()` (line 728), not inside `can_transition_mode()` itself. The call site has more context: it knows which mode is being transitioned to and from, what `next_mode` was computed as, and the full thermostat state. `can_transition_mode()` itself sees only the proposed mode and the clock — it stays free of logging.

Replace the guard block at line 728 with:

```rust
if !self.can_transition_mode(next_mode, env.current_time) {
    tracing::debug!(
        current_mode = ?self.mode,
        proposed_mode = ?next_mode,
        elapsed_s = self.mode_start_at
            .map(|t| (env.current_time - t).num_milliseconds().max(0) as f64 / 1000.0)
            .unwrap_or(0.0),
        min_on_time_s = self.min_on_time_s,
        min_off_time_s = self.min_off_time_s,
        "can_transition_mode: blocked by min on/off time"
    );
    return Ok(self.mode);
}
```

`can_transition_mode()` at `hvac_core.rs:790` receives no new logging; its single responsibility is returning a bool.

### Step 4: Add `tracing::debug!` for speed changes in `staging.rs:69`

In `select_speed_with_zone_temp()` at line 116-118, after updating `last_speed_index`/`last_speed_frac`, add:

```rust
if selection.speed_index != self.last_speed_index || selection.speed_frac != old_speed_frac {
    tracing::debug!(
        old_speed_index,
        new_speed_index = selection.speed_index,
        speed_frac = selection.speed_frac,
        part_load_ratio = selection.part_load_ratio,
        "speed transition"
    );
}
```

### Step 5: Add `tracing::debug!` for startup degradation in `staging.rs:315`

In `apply_startup_capacity_degradation()` at line 321, when `mult < 1.0`:

```rust
if mult < 1.0 {
    tracing::debug!(
        startup_multiplier = mult,
        c_d = self.startup.c_d,
        time_since_start_min = self.startup.time_since_start_min,
        steady_capacity_w,
        degraded_capacity_w = steady_capacity_w * mult,
        "startup capacity degradation active"
    );
}
```

### Step 6: Verify `tracing` crate is already a dependency

The `tracing` crate is already used in `hvac_core.rs`, `staging.rs`, `ac_config.rs`, and `duct_distribution.rs`. No new dependency needed — just add `use tracing::debug;` at the top of modules that don't already import it.

## Definition of Done

- [ ] `update_mode()` emits `tracing::debug!` with zone_temp, setpoints, current_mode, next_mode on every call
- [ ] `is_cycle_change_allowed()` emits `tracing::debug!` when blocking (min_cycle_time)
- [ ] `can_transition_mode()` emits `tracing::debug!` when blocking (min_on_time / min_off_time), including reason and time remaining
- [ ] Speed changes emit `tracing::debug!` with old/new speed_index, speed_frac, PLR
- [ ] Startup degradation emits `tracing::debug!` when multiplier < 1.0
- [ ] All new `tracing::debug!` calls use structured fields (no format-string interpolation)
- [ ] No performance impact when debug subscriber is not installed (compile-time elimination)
- [ ] Existing tests pass without modification (tracing is side-effect-only)

## Verification

1. Run the simulation with `RUST_LOG=hares_equipment::hvac=debug` and verify that mode transition decisions appear in the log with full context.
2. Force a `can_transition_mode` block by setting `min_on_time_s = 300` and verify the block reason and time remaining appear in the debug log.
3. Force startup degradation by setting `c_d = 0.25` and verify the multiplier value and `time_since_start_min` appear.
4. Run the full test suite without `RUST_LOG` set and verify no test failures or performance regressions.

## References

- `tracing` crate documentation: https://docs.rs/tracing — structured diagnostic events
- `hvac_core.rs:654-809`: `update_mode()`, `can_transition_mode()`, `set_mode()`
- `thermostat.rs:171-184`: `is_cycle_change_allowed()`
- `staging.rs:69-119`: `select_speed_with_zone_temp()`
- `staging.rs:315-323`: `apply_startup_capacity_degradation()`
- `speed_control.rs:71-97`: `StartupConfig::capacity_multiplier()`

## Coordination with Ticket 020

Both 016 and 020 emit data about `effective_setpoints`. The scopes are distinct and complementary:

- **016 (this ticket)** emits transient `tracing::debug!` events filterable by log level (`RUST_LOG=hares_equipment::hvac=debug`). These events are not persisted; they vanish when no debug subscriber is attached. They are for real-time diagnostic inspection only.
- **020** writes persistent telemetry keys (`SCHEDULE_HEATING_SETPOINT_C`, `RUNTIME_HEATING_SETPOINT_C`, etc.) that are read by the output schema and appear in output files.

The implementer of this ticket must NOT add any persistent telemetry keys. Conversely, 020 must NOT remove or duplicate the `tracing::debug!` calls added here.

## Related Tickets

- #019 — Expose speed_frac, PLR, PLF, startup_multiplier as telemetry keys (complements tracing with persistent data)
- #020 — Setpoint chain visibility (persistent telemetry; see coordination note above)
- #006 — Extract thermostat FSM (infrastructure for cleaner tracing insertion points)
