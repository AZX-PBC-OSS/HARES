# All 12 ControlSignal constructors: field correctness, optional field defaults
**Review ID**: ctrldp-01
**Category**: control-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/signal.rs` — constructor trait + impl (228 lines)
- `crates/hares-types/src/control_signal.rs` — `ControlSignal` enum definition (422 lines)
- `crates/hares-control/src/dispatch.rs` — `DispatchRequest`, `PriorityTier` (203 lines)
- `crates/hares-control/src/compat.rs` — OCHRE key-value -> ControlSignal mapping (287 lines)
- `crates/hares-equipment/src/lib.rs` — `apply_control` / `validate_signal` trait contract (lines 110-138)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py` — `update_model()` at line 236 (untyped key-value control signal dispatch)

## Findings

### Finding 1: No input validation in any constructor [Severity: high]
**Description**: All 12 constructors in `ControlSignalConstructors` blindly forward their parameters to the enum variant without range, sign, or finiteness checks. Physically impossible values (e.g., `target_soc = -0.5`, `on_fraction = 3.0`, `max_power_kw = -100.0`, `duration_s = Some(-10.0)`, `period_s = Some(0.0)`) are stored without rejection. Validation is deferred to equipment-level `apply_control_unchecked`, but enforcement is inconsistent across equipment types.
**Code Location**: `crates/hares-control/src/signal.rs:46-125` (all constructor bodies)
**Root Cause**: The `ControlSignalConstructors` trait is designed as a thin ergonomic wrapper around direct enum construction, with no validation contract. Validation responsibilities are left entirely to downstream equipment.
**Impact**:
- Air Conditioner (`air_conditioner.rs:1432`) **silently clamps** `on_fraction` to `[0,1]` and replaces negative `max_power_kw` with `f64::INFINITY` — incorrect values become silently corrected without notification.
- Heat Pump Heater (`heater.rs:2113-2119`) **returns an error** for invalid `on_fraction` — same signal, different behavior.
- Battery (`battery/mod.rs:1236`) unconditionally stores `max_power_kw` (only clamping negatives to 0 with `.max(0.0)`).
- There is no upstream guarantee that a signal reaching `apply_control` has already been validated — a consumer of the constructor API could construct and dispatch an invalid signal that silently corrupts behavior or crashes at the equipment boundary.

### Finding 2: DutyCycle constructor hides `component` field [Severity: medium]
**Description**: The `duty_cycle` constructor hardcodes `component: None` (line 89) and provides no parameter for the `component` field. Code that needs per-component duty cycle control (e.g., HPWH compressor vs. backup element) must bypass the constructor API and use struct literal syntax directly.
**Code Location**: `crates/hares-control/src/signal.rs:84-89`
```rust
fn duty_cycle(on_fraction: f64, period_s: Option<f64>) -> ControlSignal {
    ControlSignal::DutyCycle {
        on_fraction,
        period_s,
        component: None,  // hardcoded
    }
}
```
**Root Cause**: The `DutyCycle` variant was extended with the `component` field (`control_signal.rs:65-72`) after the constructor trait was stabilized. The constructor trait signature was not updated.
**Impact**: HPWH split duty-cycle control (`heat_pump_wh.rs:999-1013`) works correctly because the actor constructs `DutyCycle` variants directly via `ControlSignal::DutyCycle { on_fraction: ..., component: Some(DutyCycleComponent::Compressor), ... }`. However, any external control layer that uses only the constructor trait cannot target a specific component — it must use struct literal syntax, breaking the abstraction the trait is supposed to provide.

### Finding 3: Semantically significant defaults differ between None and Some(0.0) [Severity: medium]
**Description**: Several constructors accept `Option<f64>` parameters where `None` and `Some(0.0)` have different runtime semantics, but none of the constructors prevent a caller from accidentally passing `Some(0.0)` when they meant `None` (or vice versa).
**Code Location**: Multiple constructors in `crates/hares-control/src/signal.rs`:
- `demand_response` (line 107): `duration_s: Option<f64>` — `None` = indefinite (DR persists until explicitly cleared), `Some(0.0)` = clears on the very next `update_control` call (since `remaining <= 0.0` triggers immediate revert to `Normal` at `battery/mod.rs:822-827`).
- `duty_cycle` (line 84): `period_s: Option<f64>` — `None` = equipment default period. `Some(0.0)` would compute a zero-length period, potentially causing division-by-zero in duty-cycle-aware equipment.
- `power_limit` (line 65): `ramp_rate_kw_per_s: Option<f64>` — `None` = no ramp limit. `Some(0.0)` = instantaneous ramp allowed (semantically equivalent to `None` but zero-valued, differs from `None` only in checking `is_some()`).
**Impact**: A `demand_response` caller passing `Some(0.0)` thinking it means "no duration" will get an immediate revert (a no-op). A `duty_cycle` caller passing `Some(0.0)` for period could trigger arithmetic errors downstream.

### Finding 4: No typestate consistency enforcement between related fields [Severity: medium]
**Description**: Several constructors accept optional field pairs/triples that have physical constraints on their relative values, but constructors do not enforce them.
**Code Location**: Multiple constructors:
- `soc_target` (line 72-77): `min_soc`, `max_soc` accepted independently of `target_soc` with no check that `min_soc <= target_soc <= max_soc`.
- `humidity_setpoint` (line 111-121): `min_rh`, `max_rh` accepted independently of `target_rh` with no check that `min_rh <= target_rh <= max_rh`.
- `thermal_setpoint` (line 46-56): `heating_setpoint_c` and `cooling_setpoint_c` with no check that `heating < cooling` (when both are `Some`).
**Impact**: A signal with `min_soc = 0.9, target_soc = 0.6` is coherently constructable and will pass through all validation gates. The battery's `apply_control_unchecked` (`battery/mod.rs:1210-1221`) stores all three values without cross-validation, potentially causing the BMS actor to drive SOC to an unachievable target or oscillate between the stored bounds. OCHRE's `Dwelling.py:236-244` has the same lack of validation (raw key-value dispatch with no cross-field checks), so this is partially consistent with OCHRE behavior — but HARES's typed approach creates a stronger expectation of invariant enforcement.

### Finding 5: Priority, preemptible, and zone_restriction are absent from ControlSignal [Severity: medium]
**Description**: The review brief references `priority_override`, `preemptible` flag, and `zone_restriction` as optional fields that should default to appropriate values. None of these fields exist on `ControlSignal` variants. Priority is instead attached at the `DispatchRequest` level (`dispatch.rs:63-67`), separating the signal payload from its dispatch metadata. There is no `preemptible` flag or `zone_restriction` field anywhere in the signal or dispatch types.
**Code Location**: `crates/hares-control/src/dispatch.rs:63-67` — `DispatchRequest` carries `priority` alongside `signal`. No preemptible or zone restriction fields exist.
**Impact**:
- Priority: The separation is architecturally clean — the signal itself is a pure payload and priority is dispatch metadata. However, it means a `ControlSignal` constructed in isolation has no priority semantics; a downstream consumer that receives a raw `ControlSignal` (not a `DispatchRequest`) cannot determine its priority tier. If any code path dispatches `ControlSignal` directly without wrapping it in a `DispatchRequest` with explicit priority, it defaults to `PriorityTier::default()` (Schedule, the lowest tier), silently making all such signals preemptible.
- Preemptible flag: No mechanism exists to mark a signal as non-preemptible. The priority tier system (`Schedule < UserOverride < Grid < Safety`) provides coarse-grained preemption but cannot express that a specific `Grid` signal should not be overridden by another `Grid` signal. This could allow a grid DR curtailment to be silently overridden by a subsequent grid signal in the same timestep.
- Zone restriction: No zone-scoping on any signal. A `ThermalSetpoint` dispatched to `ByEndUse(EndUse::HVAC_HEATING)` applies to all zones served by HVAC Heating equipment. Per-zone control requires per-equipment-named dispatch targets.

### Finding 6: No constructor for 11 of 23 ControlSignal variants [Severity: low]
**Description**: The `ControlSignalConstructors` trait provides constructors for only 12 of the 23 `ControlSignal` variants. The remaining 11 variants (`CurtailmentPercent`, `ReactiveSetpoint`, `PowerFactorSetpoint`, `InverterPriorityMode`, `IdealCapacity`, `ThermalSetpointDelta`, `IdealCapacityModeOverride`, `EvPlugIn`, `EvDrive`, `EvAwayCharge`, `EvSetReadyBy`, `EventDelay`, `MaxCapacityFraction`) must be constructed via struct literal syntax.
**Code Location**: `crates/hares-control/src/signal.rs:14-43` (trait definition), `crates/hares-types/src/control_signal.rs:38-146` (enum definition)
**Root Cause**: The constructor trait was defined alongside the initial 12 variants and wasn't extended when additional variants were added.
**Impact**: Low. Struct literal construction is idiomatic Rust and works identically. However, the ergonomic promise of the trait ("`ControlSignal::thermal_setpoint(...)` style calls") is broken for nearly half the variants, creating a two-tier API that new contributors may find confusing.

### Finding 7: Clone semantics are correct — no mutable state in signals [Severity: low]
**Description**: `ControlSignal` derives `Clone` (not `Copy`). When dispatched to multiple equipment controllers, each receives a clone. There is no mutable state within `ControlSignal` itself — fields like `dr_duration_remaining_s` are stored on each equipment instance, not on the signal. The signal is purely data.
**Code Location**: `crates/hares-types/src/control_signal.rs:37` (derive Clone), `crates/hares-equipment/src/battery/mod.rs:274` (equipment-local `dr_duration_remaining_s` field)
**Impact**: None. The design correctly avoids aliasing bugs. Duration countdown is equipment-local (`update_control` decrements it per-timestep) and is never written back into the shared signal. Each equipment maintains independent DR state correctly.

### Finding 8: Inconsistent validation across equipment for the same signal type [Severity: low]
**Description**: When a signal reaches `apply_control_unchecked`, equipment implementations disagree on how to handle out-of-range values:
- **DutyCycle**: `heater.rs:2113-2119` returns `Err` for non-finite or out-of-range `on_fraction`; `air_conditioner.rs:1432` silently clamps.
- **LoadFraction**: `heater.rs:2122-2129` returns `Err`; `air_conditioner.rs:1435` silently clamps.
- **PowerLimit**: `heater.rs:2131-2134` returns `Err` for negative; `air_conditioner.rs:1438-1441` replaces negative with `f64::INFINITY`.
- **Deadband**: `heater.rs:2105-2109` validates thermal deadband; `air_conditioner.rs:1425-1427` validates it differently; others skip deadband validation entirely.

This is not a constructor issue per se, but it means constructors cannot rely on downstream validation as a safety net — a negative `on_fraction` will crash one equipment type and silently pass through another.

## Summary
- Total findings: 8
- High: 1 (no input validation in constructors)
- Medium: 4 (hidden component field, None vs Some(0) semantics, no cross-field validation, missing priority/preemptible/zone)
- Low: 3 (missing 11 constructors, Clone semantics correct, inconsistent validation downstream)

## Recommendations

1. **Add assert/validation to constructors** — At minimum, add `debug_assert!` calls for range checks (`on_fraction` in [0,1], `target_soc` in [0,1], `max_power_kw >= 0`, duration non-negative when Some). For `soc_target`, validate `min_soc <= target_soc <= max_soc` when all three are `Some`. This catches bugs early and documents invariants.

2. **Extend `duty_cycle` constructor to accept `component`** — Add an optional `component: Option<DutyCycleComponent>` parameter (or a separate `duty_cycle_component` constructor for backward compatibility). Remove the hardcoded `None`.

3. **Add `preemptible` field to `DispatchRequest`** — Allow individual dispatch requests to opt out of preemption within the same priority tier. Default to `true` for backward compatibility.

4. **Add per-zone scoping** — Extend `ThermalSetpoint` and `HumiditySetpoint` with an optional `zone_id: Option<ZoneId>` field that restricts the signal to a specific thermal zone. `None` = apply to all zones (current behavior).

5. **Add constructors for remaining 11 variants** — Complete the trait so all 23 variants have ergonomic constructor methods. This prevents struct-literal bypass and ensures validation (once added) applies uniformly.

6. **Standardize equipment-level validation** — All `apply_control_unchecked` implementations should either (a) always error on invalid values, or (b) always clamp with a warning log. The current mix creates unpredictable behavior depending on which equipment type receives the signal.

## References / Citations
- OCHRE `Dwelling.py:236-244` — OCHRE control signals are untyped `dict` of key-value `f64` pairs, dispatched by end-use name. No validation occurs at dispatch time. HARES's typed enum is a strict improvement.
- `hares-equipment/src/battery/mod.rs:822-827` — DR duration countdown logic: when `remaining <= 0.0`, DR level reverts to Normal. Powers the `None`-vs-`Some(0.0)` semantic distinction.
- `hares-equipment/src/hvac/air_conditioner.rs:1431-1443` — Example of silent clamping vs error rejection inconsistency.
- `hares-equipment/src/hvac/heat_pump/heater.rs:2113-2134` — Example of error-return validation for the same signal types.
