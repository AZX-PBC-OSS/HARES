# Python control binding: signal construction, dispatch, compat
**Review ID**: py-binding-04
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_control.rs` — PyControlSignal class, 24 static constructors, `from_dict`, `to_dict`, helper parsers
- `crates/hares-python/src/py_actor.rs` — PyDispatchRequest, PySignal, PyPriority, PyMode, PyDRLevel enums
- `crates/hares-python/src/py_dwelling.rs` — `apply_control`, `validate_control`, `set_price_signal`
- `crates/hares-types/src/control_signal.rs` — ControlSignal enum (25 variants), DRLevel, ControlCapabilities
- `crates/hares-control/src/dispatch.rs` — PriorityTier, DispatchTarget, DispatchRequest
- `crates/hares-control/src/compat.rs` — OCHRE key-to-ControlSignal mapping
- `crates/hares-core/src/dwelling/mod.rs` (lines 359-494, 1442-1484) — ControlDispatcher, `apply_control_validated`
- `tests/python/test_py_control.py` (557 lines) — Python-side ControlSignal tests
- `tests/python/test_py_dwelling_integration.py` — integration tests for control dispatch

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/api.py` — API facade, no control signal construction objects
- `vendors/EnergyPlus/src/EnergyPlus/api/common.py` — Uses `EnergyPlusException` for parameter validation
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — 18 callback registration points; all float dispatch
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — Opaque c_void_p state handle; no typed control signals
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — `set_actuator_value(handle, value)` with raw float; no typed signal variants, no priority tiers

EnergyPlus' API is a lower-level handle-based sysem (get handle by string → set value as float) with no typed signal construction, no priority tiers, and no dispatch routing. HARES' typed signal system with priority dispatch is a substantial architectural improvement.

## Findings

### Finding 1: [Severity: critical] No numeric-range validation on any ControlSignal constructor or `from_dict`

**Description**: The review requirement explicitly states that *"Any Python-constructed signal with an invalid value (e.g., negative absolute power limit) must raise Python ValueError, not panic."* None of the 24 static constructor methods nor the `from_dict` deserializer validate numeric ranges. Negative power limits, out-of-range fractions > 1.0 or < 0.0, negative SOC targets, and negative capacity watts all pass through to the Rust layer silently.

**Code Location**:
- `py_control.rs:25` — `power_setpoint(kw: f64, ...)` accepts any f64, including negative kW
- `py_control.rs:144` — `power_limit(max_power_kw: f64, ...)` accepts negative absolute limits
- `py_control.rs:65` — `ideal_capacity(capacity_w: f64)` accepts negative wattage
- `py_control.rs:72` — `load_fraction(fraction: f64)` does not clamp to [0, 1]
- `py_control.rs:120` — `soc_target(target: f64, ...)` accepts values outside [0, 1]
- `py_control.rs:155` — `duty_cycle(on_fraction: f64, ...)` accepts any f64
- `py_control.rs:186` — `curtailment_percent(percent: f64)` no [0, 100] check
- `py_control.rs:253` — `ev_drive(kwh: f64)` accepts negative energy
- `py_control.rs:260` — `ev_away_charge(power_kw: f64)` accepts negative power
- `py_control.rs:277-393` — `from_dict()` uses `dict_required::<T>()` which only checks type, not range

**Root Cause**: The `ControlSignal` Rust enum stores values as `f64` with no semantic validation in the type system. The Python binding layer delegates range enforcement to the Rust layer, but the Rust layer's `validate_signal` (`hares-equipment/src/lib.rs:136`) only checks capability flags (`ControlCapabilities`), not numeric validity.

**Impact**: Python callers can construct and dispatch semantically invalid signals without error. While Rust code won't panic on negative f64 values, the resulting simulation behavior is undefined — equipment may clamp negative power or produce nonsensical results. This violates the Python defense-in-depth contract: Python APIs must catch programmer errors early.

**Contrast with vendor**: EnergyPlus' `set_actuator_value` validates at the C layer (rounds integers, converts booleans), and `EnergyPlusException` is raised for all parameter validation errors before values reach the simulation engine.

---

### Finding 2: [Severity: high] `apply_control()` hardcodes lowest priority tier with no override

**Description**: Python code calling `dwelling.apply_control(name, signal)` can never set the dispatch priority. The underlying `apply_control_validated` method always uses `PriorityTier::default()` (i.e., `Schedule`, the lowest tier). There is no overload or parameter allowing Python callers to specify a higher priority.

**Code Location**:
- `py_dwelling.rs:646-651` — `apply_control` accepts only `(name, signal)` with no priority parameter
- `dwelling/mod.rs:1461-1484` — `apply_control_validated` queues at `priority: Default::default()`, which is `PriorityTier::Schedule`
- `dispatch.rs:15-21` — `PriorityTier` derives `Default` with `Schedule` at index 0

**Root Cause**: The `apply_control` API was designed as a simple injection endpoint, reserving priority-tier control for the actor subsystem. Python actors can set priority through `DispatchRequest` static methods (e.g., `DispatchRequest.thermal_setpoint(priority=Priority.safety())`), but standalone `apply_control` calls cannot.

**Impact**: External Python controllers (OCHRE bridges, external BMS, manual overrides) cannot inject Safety or Grid-priority signals. A safety freeze-protection signal from a Python script would be queued at the same priority as a schedule, meaning a DR event from a Rust actor could override the safety signal.

---

### Finding 3: [Severity: high] `PySignal` enum (actor dispatch) missing 11 of 25 ControlSignal variants

**Description**: The `PySignal` enum at `py_actor.rs:199-256` defines only 14 variants: ThermalSetpoint, ThermalSetpointDelta, ModeOverride, LoadFraction, PowerLimit, PowerSetpoint, SOCTarget, DutyCycle, DemandResponse, IdealCapacity, EvPlugIn, EvDrive, EvSetReadyBy, EvAwayCharge, EventDelay. This is 14 (counting carefully: actually 15). The remaining 10 variants of `ControlSignal` have no `PySignal` equivalent, meaning Python actors cannot emit: HumiditySetpoint, GridConnect, SelfConsumption, ProtocolNative, CurtailmentPercent, ReactiveSetpoint, PowerFactorSetpoint, InverterPriorityMode, IdealCapacityModeOverride, MaxCapacityFraction.

**Code Location**:
- `py_actor.rs:199-256` — PySignal enum definition (14 variants)
- `control_signal.rs:38-146` — ControlSignal enum (25 variants)

**Root Cause**: `PySignal` was designed as a curated actor-facing subset, not a 1:1 mapping. The missing variants were likely deprioritized during initial development.

**Impact**: Python actors cannot perform self-consumption mode switching, grid islanding, curtailment, reactive power dispatch, or ideal capacity mode overrides. These operations must be done either through the raw `dwelling.apply_control` path (with its Schedule-priority limitation) or bypassed entirely.

---

### Finding 4: [Severity: medium] `PyDispatchRequest.duty_cycle()` missing `component` parameter

**Description**: The `PyDispatchRequest.duty_cycle()` constructor at `py_actor.rs:535-550` accepts `(target, on_fraction, period_s=None, priority=None)` but does not expose the `component` field. By contrast, `ControlSignal.duty_cycle()` at `py_control.rs:154-167` does accept `component: Option<PyDutyCycleComponent>`.

**Code Location**:
- `py_actor.rs:535-550` — `fn duty_cycle()` — signature: `(target, on_fraction, period_s=None, priority=None)`
- `py_actor.rs:229-232` — `PySignal::DutyCycle` has no `component` field
- `py_actor.rs:350-357` — `PySignal::DutyCycle` → `ControlSignal::DutyCycle` maps `component: None`
- `py_control.rs:154-167` — `ControlSignal.duty_cycle()` accepts `component`

**Root Cause**: The `PySignal` and `PyDispatchRequest` types were implemented before the `DutyCycleComponent` split-control feature was added. They were never updated to expose it.

**Impact**: Python actors can only apply duty cycles to entire equipment, not to individual compressor or backup-element components. This prevents fine-grained DR curtailment (e.g., "curtail compressor but allow backup element for freeze protection"), which is a key use case documented in `control_signal.rs:12-16`.

---

### Finding 5: [Severity: medium] OCHRE self-consumption uses strict f64 equality

**Description**: The OCHRE compat layer at `compat.rs:73` uses `*value == 1.0` to check the self-consumption boolean. OCHRE's convention is to encode booleans as 0.0/1.0, but floating-point imprecision from serialization can produce values like 0.9999999, which would be treated as disabled.

**Code Location**:
- `compat.rs:72-73` — `KEY_SELF_CONSUMPTION_MODE => out.push(ControlSignal::self_consumption(*value == 1.0, false))`

**Root Cause**: Direct equality comparison on `f64` without epsilon tolerance.

**Impact**: Under edge cases of JSON serialization round-trips or floating-point arithmetic, a validly-set self-consumption-enabled signal could be silently treated as disabled. The OCHRE compat tests (`compat.rs:182-202`) only test with exact 1.0 and 0.0 values, so the edge case is untested.

---

### Finding 6: [Severity: medium] No price-signal variant in ControlSignal

**Description**: The review instruction specifies that Python code should be able to construct *"price signals (numeric $/kWh)"* as control signal variants. No such variant exists in the `ControlSignal` enum. Instead, price signals are injected via `Dwelling.set_price_signal(dict)` (`py_dwelling.rs:691`), which is a completely separate API path from the ControlSignal dispatch system.

**Code Location**:
- `control_signal.rs:38-146` — 25-variant ControlSignal enum, no PriceSignal variant
- `py_dwelling.rs:691-702` — separate `set_price_signal` API
- `py_control.rs:21-615` — no price signal constructor among the 24 static methods

**Root Cause**: Architectural choice — price signals feed the controller/optimization layer (tariffs), not individual equipment. The `docs/equipment/control-system.md` explicitly states: *"Price signal system (separate from ControlSignal, feeds controllers)."*

**Impact**: Python controllers expecting a unified signal construction API (where price signals are just another ControlSignal variant) will not find one. They must use `Dwelling.set_price_signal()` instead. This is a discoverability concern rather than a functional gap — the price-signal path works, but through a different API surface.

---

### Finding 7: [Severity: low] Misleading test name — "domain_validation" test doesn't validate

**Description**: The test at `test_py_control.py:162-165` is named `test_duty_cycle_domain_validation` but confirms that an out-of-range value (`on_fraction=1.5`, exceeding [0.0, 1.0]) is accepted without error. This conflates "testing construction succeeds" with "testing domain validation exists."

**Code Location**:
- `tests/python/test_py_control.py:162-165`

```python
def test_duty_cycle_domain_validation(self):
    sig = ControlSignal.duty_cycle(on_fraction=1.5)
    d = sig.to_dict()
    assert d["on_fraction"] == 1.5
```

**Root Cause**: The test was written to verify construction, but the name misleadingly implies validation.

**Impact**: A future developer might see this test and assume domain validation is working when it isn't. Combined with Finding 1, this creates a false sense of safety.

---

### Finding 8: [Severity: low] `from_dict` test coverage incomplete — comment says "18 variants" but 25 exist

**Description**: The test `test_from_dict_all_18_variants` at `test_py_control.py:280-304` has a stale name (18) and only tests 19 of the 25 ControlSignal variants. The following 6 variants are exerciced elsewhere in `from_dict` but not in the comprehensive variant test: EvPlugIn, EvDrive, EvAwayCharge, EvSetReadyBy, EventDelay, MaxCapacityFraction.

**Code Location**:
- `tests/python/test_py_control.py:280` — test name says "18" but tests 19 variants
- `py_control.rs:362-384` — code handles all 25 correctly in from_dict

**Root Cause**: The test was written when the enum had 18 variants and wasn't updated as new EV and capacity variants were added. The code itself is correct.

**Impact**: Minor — the from_dict code handles all 25 variants. The test gap means there's no unit-level round-trip verification for the 6 untested variants through from_dict, though integration tests in `test_py_dwelling_integration.py` cover EV variants at the dwelling level.

---

### Finding 9: [Severity: low] No `auto` or `schedule` mode available for mode overrides

**Description**: The review instruction mentions *"mode overrides (auto/heat/cool/off/schedule)"*. The `OperatingMode` enum and `parse_mode()` parser only accept specific equipment modes (Off, heating, Cooling, Defrost, Standby, Charging, Discharging, HeatingHP, HeatingER, HeatingHPAndER, HeatPumpWH, BackupElement, On). There is no "Auto" (return to autonomous control) or "Schedule" mode.

**Code Location**:
- `py_control.rs:647-666` — `parse_mode()` — 13 accepted values, no "Auto" or "Schedule"
- `py_actor.rs:272-285` — `PyMode` enum — 12 variants, no Auto

**Root Cause**: Equipment returns to autonomous mode by not receiving a ModeOverride signal, not by receiving an "Auto" command.

**Impact**: Low functional impact, but the API doesn't match the expectation set in the review specification. A Python controller attempting to send `ControlSignal.mode_override_str("auto")` will get a `PyValueError` instead of the expected behavior.

---

## Summary
- **Total findings**: 9
- **Critical**: 1
- **High**: 2
- **Medium**: 3
- **Low**: 3

## Recommendations

1. **Add `f64` range validation to all ControlSignal constructors and `from_dict`** (Finding 1). Each static method should check numeric bounds and raise `PyValueError` for invalid values:
   - `power_limit`: `max_power_kw` must be ≥ 0
   - `power_setpoint`, `ev_away_charge`: `active_power_kw`/`power_kw` must be ≥ 0
   - `load_fraction`, `duty_cycle.on_fraction`: must be in [0.0, 1.0]
   - `soc_target`: all SOC values in [0.0, 1.0]
   - `curtailment_percent`: in [0.0, 100.0]
   - `ev_drive`: `kwh` must be ≥ 0
   - `from_dict` should apply the same checks after extraction

2. **Add optional `priority` parameter to `dwelling.apply_control()`** (Finding 2). Accept `priority: Optional[PyPriority]` with a default of `Schedule` to maintain backward compatibility, allowing external Python controllers to inject Safety/Grid-priority signals.

3. **Populate missing `PySignal` variants** (Finding 3). Add HumiditySetpoint, GridConnect, SelfConsumption, ProtocolNative, CurtailmentPercent, ReactiveSetpoint, PowerFactorSetpoint, InverterPriorityMode, IdealCapacityModeOverride, MaxCapacityFraction to the `PySignal` enum and `PyDispatchRequest` constructors, enabling Python actors to emit these signal types.

4. **Add `component` parameter to `PyDispatchRequest.duty_cycle()`** (Finding 4). Mirror the `ControlSignal.duty_cycle` signature by accepting `component: Option[PyDutyCycleComponent]`.

5. **Use epsilon comparison in OCHRE self-consumption compat** (Finding 5). Replace `*value == 1.0` with a tolerance-based comparison like `*value > 0.5` or `(*value - 1.0).abs() < 1e-9`.

6. **Expand `test_from_dict_all_*_variants` test** (Findings 7, 8). Add the 6 missing variants (EvPlugIn, EvDrive, EvAwayCharge, EvSetReadyBy, EventDelay, MaxCapacityFraction) and rename to `test_from_dict_all_25_variants`. Add explicit negative-value rejection tests once numeric validation is implemented (Finding 1).

7. **Consider adding `"auto"` mode to ModeOverride** (Finding 9). Accept `"auto"` as a special keyword that clears the mode override, or document the intentional absence of auto/schedule modes.

## References / Citations

- `crates/hares-python/src/py_control.rs:25` — `power_setpoint` constructor, no range validation
- `crates/hares-python/src/py_control.rs:144` — `power_limit` constructor, no negative check
- `crates/hares-python/src/py_control.rs:277-393` — `from_dict` dispatcher, no numeric validation
- `crates/hares-python/src/py_dwelling.rs:646-651` — `apply_control`, no priority parameter
- `crates/hares-core/src/dwelling/mod.rs:1478-1482` — `apply_control_validated`, hardcoded `Default::default()` priority
- `crates/hares-python/src/py_actor.rs:199-256` — `PySignal` enum, 14 of 25 variants
- `crates/hares-python/src/py_actor.rs:535-550` — `PyDispatchRequest.duty_cycle`, missing `component`
- `crates/hares-python/src/py_actor.rs:350-357` — `PySignal::DutyCycle → ControlSignal::DutyCycle`, `component: None` hardcoded
- `crates/hares-control/src/compat.rs:72-73` — strict f64 equality for boolean check
- `crates/hares-equipment/src/lib.rs:136-138` — `validate_signal` default impl only checks capabilities
- `tests/python/test_py_control.py:162-165` — misleading `test_duty_cycle_domain_validation`
- `tests/python/test_py_control.py:280` — stale test name "18 variants"
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — reference: handle-based float dispatch, no typed signals
