# BMS operating mode transitions and hysteresis
**Review ID**: core-12
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actors/bms.rs` (primary: BatteryManagementActor)
- `crates/hares-types/src/equipment.rs:669–767` (BmsMode enum definition and validation)
- `crates/hares-core/src/dwelling/mod.rs:3314–3358` (mode fallback logic at construction time)
- `crates/hares-core/src/actors/ideal_thermostat.rs` (thermal setpoints — compared for scope context)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Controller.py` (not found — OCHRE does not ship a Controller.py in this copy)
- `vendors/OCHRE/ochre/Equipment/Battery.py` (OCHRE battery model for comparison)
- `vendors/OCHRE/ochre/Equipment/Generator.py` (OCHRE generator base class — ramp-rate and mode-switching patterns)

## Context: Scope Clarification

The review prompt describes a Building Management System with HVAC operating modes (occupied, unoccupied, setback, vacation, demand-response) and concerns about heating/cooling setpoints, deadbands, and 100°C setpoint bugs. **The reviewed file `bms.rs` implements a Battery Management System** — it dispatches battery charge/discharge setpoints, not thermal setpoints. The thermal/HVAC actor is `ideal_thermostat.rs`. This review therefore evaluates the battery dispatch mode-transition logic that actually exists in `bms.rs`, while noting the absence of the thermal BMS the prompt anticipates.

## Findings

### Finding 1: Zero hysteresis on all within-mode charge/discharge/idle transitions [Severity: high]

**Description**: Every BMS mode uses hard equality/inequality comparisons with no hysteresis band. When environmental inputs (PV, load, price, wind speed) oscillate near a threshold, the actor can toggle between charge and discharge on every single timestep. The `SelfConsumption` mode is especially vulnerable: `surplus > 0.0` triggers charge, `surplus < 0.0` triggers discharge (lines 177, 191). A dwelling with PV close to load under passing clouds would flip between charge and discharge every step, causing excessive switching losses and control jitter.

**Code Location**:
- `bms.rs:177` — `surplus > 0.0 && soc < *max_soc` → charge; `bms.rs:191` — `surplus < 0.0 && soc > *min_soc` → discharge. Zero-width deadband around `surplus == 0`.
- `bms.rs:246` — `price <= self.charge_price_threshold` charge; `bms.rs:260` — `price >= self.discharge_price_threshold` discharge. If `charge_threshold == discharge_threshold`, both arms fire and charge wins (executed first).
- `bms.rs:284` — `soc < *target_soc` charge; else idle. No hysteresis around `target_soc`.
- `bms.rs:403` — `wind_speed_m_s > *wind_speed_threshold_m_s` → active. No deactivation hysteresis on the weather signal.

**Root Cause**: The decision logic is purely stateless per timestep. There is no `last_mode` state, no Schmitt-trigger pattern, and no configurable deadband parameter in any mode variant.

**Impact**: Rapid toggling can cause:
1. Inverter/contactor cycling that exceeds equipment lifetimes in real deployments.
2. Physically unrealistic simulation results (e.g., a battery that charges and discharges 30 kWh over 10 steps but ends at the same SOC).
3. The `DemandResponse` mode computes `is_dr_active` using `current_price > 2.0 * self.daily_avg_price` (line 483). If price straddles exactly `2.0 * avg`, the mode toggles every step between DR discharging and base-mode behavior.

**Comparison with OCHRE**: OCHRE's `Generator.update_internal_control` (Generator.py:103–124) also evaluates self-consumption per-step without hysteresis. However, OCHRE has an explicit `ramp_rate` parameter (Generator.py:46) that limits how fast the setpoint can change between steps, providing a mechanical damping mechanism that HARES lacks in `SelfConsumption` mode.

---

### Finding 2: No minimum dwell time enforcement in any mode [Severity: high]

**Description**: There is no minimum timestep count that a mode or sub-action must persist before it can change. This means that transient conditions (a single 5-minute price spike, a single step of wind above threshold) immediately and unconditionally change the dispatch decision. In the real world, DR events, storm watches, and TOU window boundaries have minimum durations (typically 30 minutes to 4 hours) to prevent thrashing.

**Code Location**: `bms.rs:496–509` (`decide` method). Each call to `decide` produces a fresh, stateless evaluation. No dwell timer or minimum-persistence field exists on `BatteryManagementActor`.

**Root Cause**: The actor struct (lines 17–33) holds no counter for consecutive steps in the same action, and the `BmsMode` enum variants have no `min_duration` field.

**Impact**: A price schedule with high-frequency noise (e.g., 1-minute real-time pricing) could cause `DemandResponse` to engage for a single step and then revert. Similarly, `StormWatch` with a wind-speed threshold close to ambient conditions would toggle every step during gusty weather. This produces unrealistic control and can make simulation results highly sensitive to the timestep resolution.

---

### Finding 3: Implicit mode priority through nesting — no documented priority chain [Severity: medium]

**Description**: The `DemandResponse` and `StormWatch` modes both wrap a `base_mode: Box<BmsMode>`. When active, each mode overrides the base. When inactive, it delegates to `self.evaluate_mode(base_mode, env, out)` (lines 341, 417). The priority between them is determined entirely by the nesting order set at construction time:

- `StormWatch { base_mode: DemandResponse { base_mode: SelfConsumption } }` — StormWatch overrides DR, which overrides SelfConsumption.
- `DemandResponse { base_mode: StormWatch { base_mode: SelfConsumption } }` — DR overrides StormWatch.

There is no documentation or enforcement of a recommended priority chain. This ordering has material impact on behavior: if DR is nested inside StormWatch, a DR price spike while StormWatch is active will be ignored (StormWatch blocks DR). Conversely, if StormWatch is nested inside DR, DR discharge could override the storm-prep charging.

**Code Location**: `bms.rs:313–343` (DemandResponse), `bms.rs:394–419` (StormWatch), `equipment.rs:686–698` (enum definition shows the nesting structure).

**Root Cause**: The recursive delegation pattern (`evaluate_mode` recursing into `base_mode`) is elegant for composition but makes priority implicit — no systematic conflict resolution (e.g., "StormWatch always wins regardless of nesting order") is enforced.

**Impact**: Two operators configuring the same dwelling with the same intent but different nesting order get different behavior. There is no validation warning if StormWatch and DemandResponse are combined in a suboptimal order.

---

### Finding 4: DemandResponse threshold hardcoded at 2x daily average [Severity: medium]

**Description**: The `is_dr_active` method (lines 472–484) determines DR eligibility with a fixed rule: `current_price > 2.0 * self.daily_avg_price`. The `2.0` multiplier is hardcoded — there is no `dr_threshold_multiplier` field in `BmsMode::DemandResponse` (equipment.rs:686–690). Different utilities and DR programs use different trigger thresholds (1.5x, 3x, absolute price thresholds), and many programs use external dispatch signals rather than price.

**Code Location**: `bms.rs:472–484`

```rust
fn is_dr_active(&mut self, env: &EnvironmentState, current_price: f64) -> bool {
    self.ensure_daily_prices(env);
    if self.price_schedule.is_none() { return false; }
    if self.daily_avg_price <= 0.0 { return false; }
    current_price > 2.0 * self.daily_avg_price
}
```

**Root Cause**: The multiplier is a magic number in the implementation rather than a configurable field on the mode variant.

**Impact**: Users with DR programs that trigger at 1.5x the typical price (a common utility threshold) cannot configure that sensitivity. Additionally, the DR mode requires `price_schedule` to be provided (line 429–431 of `ensure_daily_prices` / line 475), so an external signal-based DR program that does not include a price forecast cannot use this mode at all.

---

### Finding 5: No guardrail on derived power setpoints — max limits set by constructor but not by mode parameter [Severity: medium]

**Description**: The actor uses `max_charge_kw` and `max_discharge_kw` from the constructor as absolute hardware limits (lines 27–28). However, none of the BMS modes provide per-mode rate scaling except `BackupReserve.charge_rate_fraction` (line 296–306) and `Scheduled` window `rate_fraction` (lines 353, 363). The `SelfConsumption` mode emits a `SelfConsumption { enabled: true }` signal with no power cap, relying on the equipment's self-consumption handler to determine the power magnitude. The `StormWatch` mode emits a `SOCTarget` without an accompanying `PowerLimit`, potentially commanding full-power charging into a storm.

**Code Location**:
- `bms.rs:183–189` — `SelfConsumption` emits no power cap (equipment decides magnitude).
- `bms.rs:407–414` — `StormWatch` emits `SOCTarget` only; no rate limit.
- `bms.rs:252–257` — `TimeOfUseOptimization` uses `max_charge_kw` directly.
- `bms.rs:329–335` — `DemandResponse` scales with `dr_discharge_rate` (good).

**Root Cause**: Different modes have different power-limiting strategies — some emit a rate cap, some rely on equipment defaults, and some use the global `max_*_kw`. There is no consistent power-limiting strategy.

**Impact**: The `StormWatch::ManualEnable` mode (line 400) immediately issues `SOCTarget { target_soc: 1.0 }` without any rate cap. If the global `max_charge_kw` is very high, this could unrealistically command a battery to charge from 20% to 100% in a single timestep. The OCHRE Battery (`Battery.py:234–239`, `Generator.py:126–148`) always applies power limits from `get_power_limits()` which enforces SOC-based capacity constraints, providing an additional safety layer HARES relies on the equipment to provide.

---

### Finding 6: Static mode assignment with no runtime transition mechanism [Severity: medium]

**Description**: The `bms_mode` field is set at construction (line 77) and never changes during simulation. The `decide` method (lines 496–509) uses `std::mem::take` to move the mode out and back, but always the same mode. The dwelling code (`mod.rs:3327–3343`) provides a one-time downgrade from `TimeOfUseOptimization` to `SelfConsumption` when no tariff is present, but there is no mechanism for:
- Scheduled mode changes (e.g., switch to BackupReserve overnight, switch to SelfConsumption during the day).
- External signals to change mode at runtime.
- Occupancy-based or calendar-based mode changes.

The mode system is purely single-mode-per-simulation.

**Code Location**: `bms.rs:496–509` (decide), `mod.rs:3327–3343` (construction-time fallback).

**Root Cause**: The `BmsMode` type models individual strategies but does not include a compound or scheduled mode selector. The only composition mechanism is recursive wrapping (`DemandResponse`, `StormWatch`), which provides overriding behavior, not scheduled switching between equal-priority modes.

**Impact**: Users cannot model a battery that operates in `SelfConsumption` during the day and `BackupReserve` at night. Each simulation run must choose exactly one top-level strategy. The `Scheduled` mode partially addresses this through time windows, but those windows map to individual charge/discharge/idle/hold actions, not to mode delegation.

**Comparison with OCHRE**: OCHRE's `Battery.update_external_control` (Battery.py:169–197) receives per-step external control signals (`SOC`, `Min SOC`, `Max SOC`, `P Setpoint`, `Self Consumption Mode`) that can dynamically change the battery's behavior. The HARES `BatteryManagementActor` produces these same `ControlSignal` variants but cannot be reconfigured externally mid-simulation.

---

### Finding 7: TOU percentile computation boundaries are ambiguous when thresholds are equal [Severity: low]

**Description**: `TimeOfUseOptimization` uses `charge_price_threshold` (from `charge_threshold_percentile`) and `discharge_price_threshold` (from `discharge_threshold_percentile`). When `charge_threshold_percentile` equals or exceeds `discharge_threshold_percentile`, the actor can simultaneously satisfy both `price <= charge_threshold` and `price >= discharge_threshold` (lines 246, 260). The charge branch runs first and wins, but this represents an undefined region where the correct behavior is unclear.

**Code Location**: `bms.rs:246–260`, `equipment.rs:719–731` (validation only checks fractions are in [0,1], not that charge_percentile < discharge_percentile).

**Root Cause**: `BmsMode::validate` (equipment.rs:719–731) checks that each percentile is `[0,1]` but does not enforce that `charge_threshold_percentile < discharge_threshold_percentile` — a necessary constraint for a well-defined deadband.

**Impact**: With `charge_threshold_percentile = 0.5` and `discharge_threshold_percentile = 0.5`, the median price triggers both charge and discharge. Charge wins due to branch ordering, creating an implicit bias toward charging that the user may not intend.

---

### Finding 8: `bms_action_code` classifies `hold` action strings as unknown (−1) [Severity: low]

**Description**: The `bms_action_code` function (lines 533–545) maps action strings to numeric telemetry codes by substring matching. The `Scheduled::Hold` action sets `last_action = "scheduled:hold"` (line 386), which contains neither "charge", "discharge", "grid_disconnect", nor "idle" — it falls through to the `-1.0` (unknown) return value.

**Code Location**: `bms.rs:533–545` (mapping function), `bms.rs:386` (hold action string).

**Root Cause**: The mapping function was written before the `Hold` action was added to `BmsAction` and was never updated.

**Impact**: Telemetry consumers relying on `bms_action` codes will see −1 for hold actions, which may be misinterpreted as an error condition. A `Hold` is semantically closest to `idle` (0) since it maintains SOC rather than actively charging or discharging.

---

## Summary
- Total findings: 8
- Critical: 0
- High: 2 (Finding 1: zero hysteresis, Finding 2: no minimum dwell time)
- Medium: 4 (Finding 3: implicit mode priority, Finding 4: hardcoded DR threshold, Finding 5: inconsistent power caps, Finding 6: static mode assignment)
- Low: 2 (Finding 7: ambiguous TOU thresholds, Finding 8: hold telemetry code)

## Recommendations

1. **Add configurable hysteresis to charge/discharge transitions** (Finding 1). Each mode that uses numerical thresholds should accept a deadband parameter. For `SelfConsumption`, a `surplus_deadband_kw` parameter would prevent toggling when PV is near load. For `TimeOfUseOptimization`, a `price_deadband` around the thresholds. For `StormWatch`, a deactivation threshold lower than the activation threshold (Schmitt trigger).

2. **Add minimum dwell time** (Finding 2). A `min_consecutive_steps` parameter on modes that use price or weather triggers (`DemandResponse`, `StormWatch`, `TimeOfUseOptimization`) would prevent single-step toggling.

3. **Document and validate mode priority** (Finding 3). Add a `validate()` check that warns or rejects ambiguous nesting of `DemandResponse` and `StormWatch` without a clear priority, or add an explicit `priority: u8` field to the wrapping modes.

4. **Make the DR threshold configurable** (Finding 4). Add a `dr_threshold_multiplier: f64` field to `BmsMode::DemandResponse` with a default of `2.0`.

5. **Add rate caps to `StormWatch`** (Finding 5). Include `charge_rate_fraction` in `BmsMode::StormWatch` so storm-prep charging does not command unlimited power.

6. **Add runtime mode switching** (Finding 6). Consider a `BmsMode::Composite { schedule: Vec<(TimeWindow, BmsMode)> }` variant that allows different strategies at different times of day.

7. **Validate TOU percentile ordering** (Finding 7). Add a check in `BmsMode::validate` that `charge_threshold_percentile < discharge_threshold_percentile` (or at least `<=`).

8. **Fix `bms_action_code` for Hold** (Finding 8). Add `"hold"` to the substring match, mapping it to code `0` (idle).

## References / Citations
- OCHRE Battery.py:169–197 — external control signal handling with min/max SOC guards
- OCHRE Generator.py:46–48 — `ramp_rate` kW/min damping mechanism
- OCHRE Generator.py:103–124 — per-step self-consumption evaluation (no hysteresis, but ramp-limited)
- HARES `equipment.rs:669–701` — `BmsMode` enum with all variants
- HARES `ideal_thermostat.rs` — the actual thermal/HVAC control actor (separate from battery BMS)
