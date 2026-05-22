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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match:
  - `update_mode()` — `hvac_core.rs:654-734` (ticket says 654-733; off by 1 on the closing brace; function body is correct)
  - `is_cycle_change_allowed()` — `thermostat.rs:171-184` ✓ exact match
  - `can_transition_mode()` — `hvac_core.rs:790-809` ✓ exact match
  - `select_speed_with_zone_temp()` — `staging.rs:75-119` (ticket says 69-119; function signature starts at 75, outer wrapper `select_speed` at 69; both are present)
  - `apply_startup_capacity_degradation()` — `staging.rs:315-323` ✓ exact match
  - `capacity_multiplier()` — `speed_control.rs:71-97` ✓ exact match
  - `tracing::debug!` in staging — `staging.rs:287` ✓ exact match
  - `tracing::warn!` calls — `hvac_core.rs:530`, `ac_config.rs:185`, `staging.rs:301`, `duct_distribution.rs:27` ✓ all confirmed
  - `use tracing::warn` import in `duct_distribution.rs` — line 9 ✓ exact match
- [x] Described logic matches current implementation — the ticket correctly describes each function's guard conditions and the absence of any logging.
- [x] Bug confirmed: grep of `crates/hares-equipment/src/hvac/` for `tracing::debug!` or `tracing::warn!` returns exactly 5 hits: 1 `debug!` (staging.rs:287) and 4 `warn!` (hvac_core.rs:530, ac_config.rs:185, staging.rs:301, duct_distribution.rs:27). Zero are in `update_mode()`, `is_cycle_change_allowed()`, `can_transition_mode()`, `select_speed_with_zone_temp()`, or `apply_startup_capacity_degradation()`.
- [x] OCHRE cross-check: **OCHRE has no equivalent logging** — the Python files `HVAC.py` and `Equipment.py` contain no `import logging` or `logger.*` calls in their thermostat/HVAC mode-transition paths. The HARES gap mirrors (and even slightly exceeds) OCHRE's own lack of diagnostics. No divergence — both are silent.
- [x] EnergyPlus cross-check: **N/A for this ticket** — the ticket concerns diagnostic instrumentation (`tracing::debug!`), not physics or algorithms. EnergyPlus source code and engineering reference are not relevant to whether log calls are present.

### Web-Verified Citations

This ticket contains one reference: the `tracing` crate documentation at `https://docs.rs/tracing`.

- **Citation**: "`tracing` crate documentation: https://docs.rs/tracing — structured diagnostic events"
- **Source found**: [https://docs.rs/tracing/latest/tracing/](https://docs.rs/tracing/latest/tracing/) and [https://docs.rs/tracing/latest/tracing/level_filters/index.html](https://docs.rs/tracing/latest/tracing/level_filters/index.html)
- **Quoted passage** (from level_filters module): *"Trace instrumentation at disabled levels will be skipped and will not even be present in the resulting binary unless the verbosity level is specified dynamically."* and *"A crate can disable trace level instrumentation in debug builds and trace, debug, and info level instrumentation in release builds with features like `'max_level_debug'` and `'release_max_level_warn'`."*
- **Verdict**: Confirmed. The `tracing` crate provides both runtime filtering (no event construction when no subscriber expresses interest) and compile-time static filtering via feature flags (`max_level_*`, `release_max_level_*`).

**Nuance on the ticket's claim**: The ticket states "tracing macros are compiled out at build time when no subscriber enables the DEBUG level." This is a slight overstatement. Tracing macros are compiled out at build time **only when the `max_level_debug` or similar feature flag is explicitly set** in `Cargo.toml`. Without those flags (the current state — workspace `Cargo.toml` only sets `release_max_level_info`), the runtime path takes a fast subscriber-interest check that avoids constructing the event when no subscriber is active, but the call sites themselves remain in the binary. The performance claim is still substantially correct: the overhead is negligible when no subscriber is active. The feature flag `release_max_level_info` in the workspace confirms that in release builds, `debug!` calls are fully eliminated at compile time. In debug builds they use runtime filtering.

- **Citation (implicit)**: The approach of not logging inside `can_transition_mode()` itself (ticket §Step 3 rationale) is a design choice without an external citation — correctly attributed to the principle of "single responsibility."
- **Verdict**: No citation to verify; the design rationale is sound.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: All five code locations cited by the ticket are confirmed to have zero diagnostic logging. The tracing-call inventory (1 debug + 4 warn, none in thermostat paths) matches exactly. The `tracing` crate reference is confirmed: `docs.rs/tracing` is the authoritative source and it supports both runtime and compile-time performance guarantees. The ticket's proposal to log at `tracing::debug!` level using structured fields is idiomatic Rust and correct for the problem described. The coordination note with ticket-020 (persistent telemetry vs transient tracing) is a sound architectural distinction. The only minor inaccuracy is "compiled out at build time when no subscriber enables the DEBUG level" — the compile-out only happens when the `max_level_*` feature flag is set; otherwise it is runtime-filtered. In release builds, the workspace `release_max_level_info` feature does compile out `debug!` calls, so the claim is correct for release builds.

### Proposed Fix Summary

Add `tracing::debug!` calls at six locations in `crates/hares-equipment/src/hvac/`:

1. **`hvac_core.rs` — `update_mode()` entry** (~line 657): log `zone_temp`, `heating_setpoint`, `cooling_setpoint`, `current_mode` before any guard check.
2. **`hvac_core.rs` — `update_mode()` after computing `next_mode`** (~line 722): log `current_mode`, `next_mode`, `offset`, `hysteresis_c`.
3. **`thermostat.rs` — `is_cycle_change_allowed()`** (line 182): when returning `false`, log `elapsed_s`, `min_cycle_time_s`.
4. **`hvac_core.rs` — `update_mode()` call site of `can_transition_mode()`** (line 728): when the guard returns `false`, log `current_mode`, `proposed_mode`, `elapsed_s`, `min_on_time_s`, `min_off_time_s`.
5. **`staging.rs` — `select_speed_with_zone_temp()`** (~line 116): when `speed_index` or `speed_frac` changes, log `old_speed_index`, `new_speed_index`, `speed_frac`, `part_load_ratio`.
6. **`staging.rs` — `apply_startup_capacity_degradation()`** (~line 321): when `mult < 1.0`, log `startup_multiplier`, `c_d`, `time_since_start_min`, `steady_capacity_w`, `degraded_capacity_w`.

Modules that don't already have `use tracing::debug;` will need the import added. No production logic changes; no new telemetry keys. `thermostat.rs` and `staging.rs` will each need `use tracing::debug;` added.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs` (appended after line 2382)
- **Functions added**:
  - `ticket_016_update_mode_heating_to_deadband_transition` — exercises the full `update_mode()` path (Deadband→Heating→Deadband) via the public Equipment API with a Gas Furnace. Passes before and after the ticket is implemented.
  - `ticket_016_update_mode_cooling_transition` — exercises the Cooling branch of `update_mode()` (Deadband→Cooling→Deadband) via the public Equipment API with an Air Conditioner. Passes before and after the ticket is implemented.
- **Note on min_on_time / min_cycle_time paths**: These are already comprehensively covered by internal unit tests in `hvac_core.rs` (`can_transition_mode_allows_when_disabled`, `min_on_time_blocks_early_shutdown`, `min_off_time_blocks_early_restart`, `grid_emergency_off_blocked_until_min_on_time_elapses`) and `thermostat.rs`. The startup degradation path is covered by unit tests in `speed_control.rs` (`startup_ramp_below_one_at_first_step`, `startup_ramp_reaches_one_at_t_full`). These internal tests cannot be easily supplemented from external test binaries without access to `#[cfg(test)] test_extras_mut`.
- **Why these tests don't "fail" before the ticket**: Ticket-016 adds purely additive `tracing::debug!` calls that have no effect on return values or observable state. There are no correctness bugs to surface. The regression tests document and protect the behavioural contracts of the code paths being instrumented, ensuring that adding tracing does not accidentally break mode-transition logic.
