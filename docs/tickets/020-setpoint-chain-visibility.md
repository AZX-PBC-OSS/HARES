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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation
- [x] Referenced line numbers still match (with minor offset noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: diverges (intentional — see below)
- [x] EnergyPlus cross-check result: N/A — ticket makes no EnergyPlus claims; checked for context (see below)

#### Line number notes

| Ticket citation | Actual location | Status |
|---|---|---|
| `hvac_core.rs:610-622` — `effective_setpoints()` | `effective_setpoints()` is at **618-622**; lines 610-617 are `set_schedule_setpoints()` / `clear_schedule_setpoints()`. The range is inclusive but starts 8 lines early. | Minor inaccuracy |
| `hvac_core.rs:629-652` — `resolve_profile_setpoints()` | Confirmed at 629-652 exactly | ✓ |
| `thermostat.rs:118-144` — `with_schedule_override()`, `with_control_override()` | Confirmed at 118-134 and 136-144 | ✓ |
| `thermostat.rs:156-161` — `RuntimeSetpointOverride` | Confirmed at 156-161 | ✓ |
| `air_conditioner.rs:875-876` | Confirmed at 875-876 | ✓ |
| `furnace.rs:198-199,425-426` | Confirmed at 198-199 and 425-426 | ✓ |
| `ideal_hvac.rs:204-227` — `resolve_schedule_setpoints()` | Confirmed at 204-227 | ✓ |
| `telemetry_keys.rs:44-45` — `HEATING_SETPOINT_C`, `COOLING_SETPOINT_C` | Confirmed at 44-45 | ✓ |

#### Additional gap not mentioned in ticket

`IdealHvac` does **not** write `HEATING_SETPOINT_C` / `COOLING_SETPOINT_C` to telemetry **at all** today (its `step()` telemetry writes end at `tk::HVAC_COOLING_CAPACITY_W` — `ideal_hvac.rs:596-600`; no setpoint key is written). This is a more fundamental gap than the ticket describes: IdealHvac doesn't just lack the chain-visibility keys; it also lacks the existing effective setpoint telemetry that all other equipment already has. The ticket's Step 6 instruction to "add the same telemetry writes there" is correct and complete, but the baseline description ("all other HVAC equipment follows the same pattern") is imprecise — IdealHvac is absent entirely, not just missing the new keys.

#### OCHRE cross-check

OCHRE (`vendors/OCHRE/ochre/Equipment/HVAC.py`) uses a single `temp_setpoint` field for the effective setpoint. External control signals via `update_external_control()` (line 271-273) mutate `current_schedule[f"{self.end_use} Setpoint (C)"]` directly, which is then read by `update_setpoint()` (line 372) and stored in `self.temp_setpoint`. **There is no intermediate distinction** between schedule-sourced and runtime-override setpoints — OCHRE merges them in-place into `current_schedule` and writes only the single `{end_use} Setpoint (C)` result to telemetry output (line 583, verbosity ≥ 4). HARES's three-stage chain (`static_setpoints → schedule_setpoints → runtime_setpoints`) is a deliberate architectural improvement over OCHRE. The ticket's proposed telemetry exposure of each stage is a new capability not present in OCHRE and is intentional.

#### EnergyPlus cross-check

EnergyPlus (verified via EnergyPlus 23.2 I/O Reference, bigladdersoftware.com, fetched 2026-05-20) exposes only two thermostat output variables at the effective level: `Zone Thermostat Heating Setpoint Temperature [C]` ("the current zone thermostat heating setpoint in degrees C") and `Zone Thermostat Cooling Setpoint Temperature [C]` ("the current zone thermostat cooling setpoint in degrees C"). The documentation explicitly states that **no separate output variables distinguish between schedule-derived setpoints and EMS/runtime override setpoints** — only the final effective value is exposed. HARES's proposal to add `SCHEDULE_*` and `RUNTIME_*` keys is therefore **more capable than EnergyPlus** in this respect. This is not a divergence from a reference implementation; it is an intentional enhancement enabling control-signal diagnostics that EnergyPlus does not natively support.

### Web-Verified Citations

This ticket contains no external standards citations (ASHRAE, NFRC, DOE, ISO, or EnergyPlus). The references section lists only internal file paths. Web searches were performed to verify the EnergyPlus and OCHRE baseline behavior described above.

- **Citation**: Ticket implies effective-setpoint-only is the prevailing approach (implicitly compared to EnergyPlus / OCHRE)
- **Source found**: EnergyPlus 23.2 Input/Output Reference — Group Zone Controls Thermostats (https://bigladdersoftware.com/epx/docs/23-2/input-output-reference/group-zone-controls-thermostats.html); OCHRE HVAC.py (vendors/OCHRE/ochre/Equipment/HVAC.py, lines 271-273, 583)
- **Quoted passage (EnergyPlus)**: "Zone Thermostat Heating Setpoint Temperature [C] — This is the current zone thermostat heating setpoint in degrees C. If there is no heating thermostat active, then the value will be 0." No intermediate schedule vs. runtime-override distinction is exposed in EnergyPlus output variables.
- **Quoted passage (OCHRE)**: `ext_setpoint = control_signal.get("Setpoint"); if ext_setpoint is not None: self.current_schedule[f"{self.end_use} Setpoint (C)"] = ext_setpoint` — control overrides are merged into the schedule slot; `results[f"{self.end_use} Setpoint (C)"] = self.temp_setpoint` — only final value reported.
- **Verdict**: confirmed — HARES's chain-visibility feature is novel and correct; neither EnergyPlus nor OCHRE provide equivalent chain decomposition.

### Legitimacy
- **Verdict**: Legitimate
- **Rationale**: Every code location cited in the ticket was confirmed in the current codebase. The three-stage setpoint resolution chain (`static → schedule → runtime`) exists exactly as described, and none of the intermediate values are currently exposed in telemetry. The new key constants (`SCHEDULE_HEATING_SETPOINT_C`, `SCHEDULE_COOLING_SETPOINT_C`, `RUNTIME_HEATING_SETPOINT_C`, `RUNTIME_COOLING_SETPOINT_C`) are confirmed absent from `telemetry_keys.rs`. The OCHRE and EnergyPlus baselines corroborate that this is a deliberate HARES enhancement rather than a deviation from an established reference. One uncited gap was found: `IdealHvac` also does not write the existing `HEATING_SETPOINT_C` / `COOLING_SETPOINT_C` keys (not just the new chain keys), making the IdealHvac scope slightly broader than the ticket implies — but the ticket's Step 6 fix is still correct. The effective-setpoint-only gap is real, the proposed fix is well-scoped, and the absent-means-no-override invariant for `RUNTIME_*` keys is sound.

### Proposed Fix Summary

1. Add four string constants to `telemetry_keys.rs`: `SCHEDULE_HEATING_SETPOINT_C`, `SCHEDULE_COOLING_SETPOINT_C`, `RUNTIME_HEATING_SETPOINT_C`, `RUNTIME_COOLING_SETPOINT_C`.
2. In each HVAC equipment `step()` method that already writes `HEATING_SETPOINT_C` / `COOLING_SETPOINT_C` (air_conditioner.rs, furnace.rs x2, heat_pump/heater.rs), compute `schedule_stage = self.hvac.static_setpoints.with_schedule_override(self.hvac.schedule_setpoints)` and write the two schedule-stage keys unconditionally; then write the two runtime keys conditionally under `if let Some(ref rt) = self.hvac.runtime_setpoints`.
3. `IdealHvac::step()` needs the same treatment plus the pre-existing gap: add all 6 setpoint telemetry writes (effective + schedule + runtime), and add `HEATING_SETPOINT_C` / `COOLING_SETPOINT_C` to its `default_telemetry()` and `TelemetryField` descriptors.
4. Add the four new keys to `default_telemetry()` and `TelemetryField` descriptors in all config modules (ac_config.rs, heater_config.rs, furnace.rs). Do NOT pre-initialise `RUNTIME_*` keys to 0.0 in default telemetry — they must be absent when no override is active.

### Test Written
- File: `crates/hares-equipment/tests/hvac_tests.rs` (appended at end of file)
- What it tests:
  - `ticket_020_schedule_setpoint_keys_absent_from_telemetry` — asserts `schedule_heating_setpoint_c` and `schedule_cooling_setpoint_c` are present in AC telemetry after a step and equal the static setpoints when no schedule source is configured (currently panics because keys are absent).
  - `ticket_020_runtime_setpoint_key_absent_when_no_override` — round-trip: applies an override, clears it (None axes), steps; asserts key is absent. Then re-applies override and steps; asserts key is present at 25.0°C. (Currently panics because keys are never written.)
  - `ticket_020_runtime_setpoint_key_present_when_override_active` — applies 25°C heating override on gas furnace; asserts `runtime_heating_setpoint_c == 25.0`, effective equals override, schedule-stage differs from effective. (Currently panics.)
  - `ticket_020_ideal_hvac_setpoint_chain_absent` — asserts IdealHvac writes `schedule_heating/cooling_setpoint_c` AND `heating/cooling_setpoint_c` after a step; catches both the chain-visibility gap and the pre-existing effective-setpoint telemetry absence. (Currently panics.)
  - All 4 tests run as `#[should_panic(expected = "ticket-020")]` and are confirmed passing today (panicking as expected). They will flip to plain passing tests once ticket-020 is implemented.
