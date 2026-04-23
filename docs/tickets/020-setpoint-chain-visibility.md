# Setpoint Resolution Chain Visibility

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac, hares-types

## Problem

Only the final `effective_setpoints()` result is exposed via the `HEATING_SETPOINT_C` and `COOLING_SETPOINT_C` telemetry keys. Cannot distinguish whether a setpoint change came from the schedule, a runtime control signal override, or the static config. This makes it impossible to diagnose "why did the setpoint change?" from logs or output data.

The setpoint resolution chain is: **static base** → **schedule override** → **runtime (control signal) override** → **effective**.

When a user sees the cooling setpoint drop from 26°C to 22°C, they cannot tell whether:
- The schedule changed (e.g., "away" period ended)
- A control signal was applied (e.g., DR offset, or external controller override)
- The static config was modified

## Current Behavior

### Setpoint resolution at `hvac_core.rs:610-622`

```rust
pub fn effective_setpoints(&self) -> ThermalSetpoints {
    self.static_setpoints
        .with_schedule_override(self.schedule_setpoints)
        .with_control_override(self.runtime_setpoints)
}
```

The three stages are composed via `ThermalSetpoints::with_schedule_override()` and `ThermalSetpoints::with_control_override()` at `thermostat.rs:118-144`. Neither intermediate result is exposed — only the final composed value.

### Schedule setpoint resolution at `hvac_core.rs:629-652`

`resolve_profile_setpoints()` reads from `heating_setpoint_source` / `cooling_setpoint_source` schedule sources and stores the result in `self.schedule_setpoints: Option<ScheduleSetpoints>`. This intermediate value is never written to telemetry.

### Runtime override storage

`self.runtime_setpoints: Option<RuntimeSetpointOverride>` is set by `apply_control_signal()`. Defined at `thermostat.rs:156-161` with `heating_c: Option<f64>` and `cooling_c: Option<f64>`. Never written to telemetry.

### Current telemetry writes

Only the effective setpoints are written:

- `air_conditioner.rs:875-876`: `telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c)` / `telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c)`
- `furnace.rs:198-199,425-426`: Same pattern
- All other HVAC equipment follows the same pattern

### Ideal HVAC resolution at `ideal_hvac.rs:204-227`

`IdealHvac` has its own `resolve_schedule_setpoints()` with the same three-stage chain. Same gap — only effective setpoints are exposed.

## Required Behavior

Each stage of the setpoint chain must be visible in telemetry:

1. **Schedule setpoints**: `SCHEDULE_HEATING_SETPOINT_C`, `SCHEDULE_COOLING_SETPOINT_C` — the setpoints after the schedule override is applied (but before runtime override); always written.
2. **Runtime override setpoints**: `RUNTIME_HEATING_SETPOINT_C`, `RUNTIME_COOLING_SETPOINT_C` — written **only when a runtime override is actively set** (`self.hvac.runtime_setpoints` is `Some` and the corresponding axis is `Some(f64)`). When no override is active, these keys must be absent (null / not written). Per project policy, no sentinel values: absent means no override.
3. **Effective setpoints** (existing): `HEATING_SETPOINT_C`, `COOLING_SETPOINT_C` — remain as-is, the final composed value.

This enables diagnosing: "the schedule lowered the cooling setpoint to 24°C, but the control signal overrode it to 22°C" — you'd see `SCHEDULE_COOLING_SETPOINT_C = 24.0`, `RUNTIME_COOLING_SETPOINT_C = 22.0`, `COOLING_SETPOINT_C = 22.0`. When no override is active: `SCHEDULE_COOLING_SETPOINT_C = 24.0`, `RUNTIME_COOLING_SETPOINT_C = null`, `COOLING_SETPOINT_C = 24.0`.

## Approach

### Step 1: Add new telemetry key constants in `telemetry_keys.rs`

```rust
// ── Setpoint chain ──────────────────────────────────────────────────────────
pub const SCHEDULE_HEATING_SETPOINT_C: &str = "schedule_heating_setpoint_c";
pub const SCHEDULE_COOLING_SETPOINT_C: &str = "schedule_cooling_setpoint_c";
pub const RUNTIME_HEATING_SETPOINT_C: &str = "runtime_heating_setpoint_c";
pub const RUNTIME_COOLING_SETPOINT_C: &str = "runtime_cooling_setpoint_c";
```

### Step 2: Write schedule setpoints in `resolve_profile_setpoints()`

At `hvac_core.rs:643-651`, after setting `self.schedule_setpoints`, also compute the schedule-stage setpoints and store them for telemetry:

```rust
// After setting self.schedule_setpoints...
let schedule_stage = self.static_setpoints.with_schedule_override(self.schedule_setpoints);
self.schedule_heating_c = schedule_stage.heating_c;
self.schedule_cooling_c = schedule_stage.cooling_c;
```

Alternatively, compute schedule-stage setpoints at the point where effective setpoints are written to telemetry (simpler, no new fields needed on `HvacEquipment`).

### Step 3: Write runtime override setpoints

When `runtime_setpoints` is set (via `apply_control_signal()`), write the override values to telemetry. These are `self.runtime_setpoints.heating_c` and `self.runtime_setpoints.cooling_c`.

### Step 4: Write all 6 setpoint values in `step()` methods

In each HVAC equipment's `step()`, alongside the existing `telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c)`:

```rust
// Schedule-stage setpoints
let schedule_stage = self.hvac.static_setpoints
    .with_schedule_override(self.hvac.schedule_setpoints);
telemetry.set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
telemetry.set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);

// Runtime override setpoints — only written when an active override is present.
// When no runtime override is active, RUNTIME_HEATING_SETPOINT_C and
// RUNTIME_COOLING_SETPOINT_C are NOT written (they remain absent / null in output).
// This preserves the invariant: absent means no override; a value means an override
// is actively in effect. Never substitute the schedule value as a fallback — that
// makes "no override" indistinguishable from "override equals schedule value".
if let Some(ref rt) = self.hvac.runtime_setpoints {
    if let Some(h) = rt.heating_c {
        telemetry.set(tk::RUNTIME_HEATING_SETPOINT_C, h);
    }
    if let Some(c) = rt.cooling_c {
        telemetry.set(tk::RUNTIME_COOLING_SETPOINT_C, c);
    }
}
// else: no override active — do not write RUNTIME_*_SETPOINT_C keys at all.
```

The `Option<f64>` fields on `RuntimeSetpointOverride` (`thermostat.rs:156-161`) map directly to this: `None` means the axis is not overridden and must not appear in telemetry.

### Step 5: Add keys to default telemetry and TelemetryField descriptors

Add `telemetry.insert()` calls with 0.0 defaults in each equipment's default_telemetry() constructor. Add TelemetryField entries describing the setpoint chain stage.

### Step 6: Apply same pattern to IdealHvac

At `ideal_hvac.rs:204-227`, `resolve_schedule_setpoints()` is confirmed present and follows the same three-stage chain (`static_setpoints` → `schedule_setpoints` → `runtime_setpoints`). The method sets `self.schedule_setpoints` at line 219 and clears it at line 225, matching the `HvacEquipment` pattern exactly. Add the same telemetry writes there: schedule-stage keys always written; runtime-override keys written only when `self.runtime_setpoints` is `Some` with non-`None` axis values.

## Definition of Done

- [ ] `SCHEDULE_HEATING_SETPOINT_C` and `SCHEDULE_COOLING_SETPOINT_C` telemetry key constants defined
- [ ] `RUNTIME_HEATING_SETPOINT_C` and `RUNTIME_COOLING_SETPOINT_C` telemetry key constants defined
- [ ] Schedule-stage setpoints written in all HVAC equipment `step()` methods
- [ ] Runtime override setpoints written in all HVAC equipment `step()` methods
- [ ] Existing `HEATING_SETPOINT_C` / `COOLING_SETPOINT_C` remain unchanged as the effective (final) values
- [ ] Keys initialized in default telemetry constructors
- [ ] TelemetryField descriptors added for all 4 new keys
- [ ] IdealHvac also exposes setpoint chain
- [ ] When a runtime override is active: `RUNTIME_*_SETPOINT_C` keys are present with override values
- [ ] When no runtime override is active: `RUNTIME_*_SETPOINT_C` keys are absent (not written; null in output)
- [ ] Effective = runtime override of schedule-stage (i.e., `COOLING_SETPOINT_C` = runtime override applied on top of schedule)

## Verification

1. Run a simulation with a schedule that changes setpoints (e.g., weekday heating setback at night). Verify `SCHEDULE_HEATING_SETPOINT_C` shows the schedule override while `HEATING_SETPOINT_C` matches the effective value.
2. Apply a `ThermalSetpoint` control signal and verify `RUNTIME_HEATING_SETPOINT_C` / `RUNTIME_COOLING_SETPOINT_C` show the override values.
3. When no runtime override is active, verify `RUNTIME_*_SETPOINT_C` keys are absent (null in output) — NOT equal to the schedule value. This is the critical distinction between "no override" and "override matches schedule".
4. Verify `COOLING_SETPOINT_C` always equals the result of `with_schedule_override().with_control_override()` applied to static setpoints.

## References

- `hvac_core.rs:610-622`: `effective_setpoints()` — three-stage composition
- `hvac_core.rs:629-652`: `resolve_profile_setpoints()` — schedule resolution
- `thermostat.rs:118-144`: `with_schedule_override()`, `with_control_override()` — override composition
- `thermostat.rs:156-161`: `RuntimeSetpointOverride` — runtime control signal type
- `air_conditioner.rs:875-876`: Existing setpoint telemetry writes
- `furnace.rs:198-199,425-426`: Existing setpoint telemetry writes
- `ideal_hvac.rs:204-227`: IdealHvac schedule resolution (same gap)
- `telemetry_keys.rs:44-45`: Existing `HEATING_SETPOINT_C`, `COOLING_SETPOINT_C`

## Coordination with 016

Both 016 and 020 touch `effective_setpoints`. The scopes are distinct and complementary:

- **016** emits transient `tracing::debug!` events (filterable by log level, not persisted). It operates in the FSM decision path.
- **020 (this ticket)** writes persistent telemetry keys read by the output schema.

The implementer of this ticket must NOT remove or duplicate the `tracing::debug!` calls added by 016. If 016 has already shipped, verify its traces are intact after this ticket's step() changes.

## Related Tickets

- #018 — CoreOutput HVAC promotion (setpoint_c will move to CoreOutput as the effective value; schedule/runtime setpoints remain telemetry-only for diagnostics)
- #016 — Thermostat FSM decision tracing (tracing will log setpoint chain at decision points; see Coordination note)
