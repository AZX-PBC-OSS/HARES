# Every ControlSignal enum variant: constructability, dispatch handling, equipment consumption
**Review ID**: ctrldp-02
**Category**: control-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/control_signal.rs` — ControlSignal enum definition (25 variants), ControlCapabilities bitflags, signal constructors trait
- `crates/hares-control/src/signal.rs` — `ControlSignalConstructors` trait (12 of 25 variants)
- `crates/hares-control/src/compat.rs` — OCHRE compatibility mapping (7 variants)
- `crates/hares-control/src/dispatch.rs` — DispatchRequest, DispatchTarget, PriorityTier types
- `crates/hares-core/src/dwelling/mod.rs:339–495` — ControlDispatcher (queue/drain/route) and `route_request()`/`apply_to_matching()`
- `crates/hares-equipment/src/lib.rs:100–235` — Equipment trait with `apply_control()` capability gate and `apply_control_unchecked()`
- `crates/hares-equipment/src/macros.rs` — `delegate_equipment!` macro
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1417–1467` — AirConditioner apply_control_unchecked
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:2095–2168` — HeatPumpHeaterCore apply_control_unchecked
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:262–264` — D2ACooler/GSHP Cooler delegate
- `crates/hares-equipment/src/hvac/helpers.rs:178–217` — apply_heating_control_unchecked, apply_simple_heating_ideal_capacity_control
- `crates/hares-equipment/src/hvac/thermostat.rs:445–474` — apply_thermal_setpoint_signal
- `crates/hares-equipment/src/hvac/hvac_core.rs:665–672` — HvacEquipment::apply_control_signal
- `crates/hares-equipment/src/hvac/ideal_hvac.rs:714–781` — IdealHvac apply_control_unchecked
- `crates/hares-equipment/src/hvac/dehumidifier.rs:446–474` — Dehumidifier apply_control_unchecked
- `crates/hares-equipment/src/battery/mod.rs:1201–1249` — Battery apply_control_unchecked
- `crates/hares-equipment/src/ev/mod.rs:880–1009` — EV apply_control_unchecked
- `crates/hares-equipment/src/pv/mod.rs:623–694` — PV apply_control_unchecked
- `crates/hares-equipment/src/generator.rs:908–952` — Generator apply_control_unchecked
- `crates/hares-equipment/src/ventilation.rs:551–574` — Ventilation apply_control_unchecked
- `crates/hares-equipment/src/event_load.rs:658–701,1162–1196` — EventBasedLoad, WetAppliance apply_control_unchecked
- `crates/hares-equipment/src/scheduled_load.rs:660–685` — ScheduledLoad apply_control_unchecked
- `crates/hares-equipment/src/water_heater/resistance.rs:685–741` — ResistanceWH
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:967–1036` — HeatPumpWH
- `crates/hares-equipment/src/water_heater/gas.rs:659–708` — GasWH
- `crates/hares-equipment/src/water_heater/tankless.rs:481–525` — TanklessWH
- `crates/hares-equipment/src/hvac/baseboard.rs:226–230` — ElectricBaseboard
- `crates/hares-equipment/src/hvac/boiler.rs:340–344` — ElectricBoiler
- `crates/hares-equipment/src/hvac/furnace.rs:350–354` — ElectricFurnace
- `crates/hares-python/src/py_control.rs` — Python bindings for ControlSignal
- `crates/hares-python/src/py_actor.rs:201–377` — PySignal enum and into_control_signal conversion

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical] ProtocolNative is constructible but no equipment consumes it

**Description**: The `ProtocolNative` variant can be constructed through the Rust `ControlSignalConstructors::protocol_native()` trait method, the Python `ControlSignal.protocol_native()` static constructor, `from_dict`, and the test suite. However, no equipment type declares the `PROTOCOL_NATIVE` capability flag, and no equipment's `apply_control_unchecked` method matches on `ProtocolNative`. Any dispatch attempt will fail at the capability gate (`ensure_signal_supported`), making this variant permanently unreachable in a running simulation.

**Code Location**:
- Enum definition: `crates/hares-types/src/control_signal.rs:88–91`
- Constructor: `crates/hares-control/src/signal.rs:42,123–125`
- Python constructor: `crates/hares-python/src/py_control.rs:216–224`
- Missing: zero equipment types declare `PROTOCOL_NATIVE` (grep confirms no matches in `crates/hares-equipment/src/`)

**Root Cause**: The variant appears to be a forward-looking placeholder intended for protocol-native control (e.g., raw SunSpec, Modbus, EEBUS), but the equipment layer was never updated to support it. The `ControlCapabilities` bitflags reserve bit 10 (`PROTOCOL_NATIVE = 1 << 10`), suggesting the capability was anticipated but never wired in.

**Impact**: Any controller that constructs and dispatches a `ProtocolNative` signal will receive a capability-rejection error, causing the control action to fail silently through the dispatcher's `warnings` channel. This is dead code that signals an incomplete feature.

### Finding 2: [Severity: critical] AirConditioner silently ignores ThermalSetpointDelta via catch-all

**Description**: The `AirConditioner` (and by delegation, `D2ACooler` and `GshpCooler`) declares `THERMAL_SETPOINT_DELTA` in its `ControlCapabilities`, so the capability gate passes. However, its `apply_control_unchecked` method at `crates/hares-equipment/src/hvac/air_conditioner.rs:1464` has `_ => {}` as a catch-all, which silently discards `ThermalSetpointDelta` without applying it. This is in contrast to the `HeatPumpHeaterCore`, which falls through to `apply_heating_control_unchecked()` (line 2159–2164) and correctly routes `ThermalSetpointDelta` through `apply_control_signal` → `apply_thermal_setpoint_signal`.

**Code Location**:
- Capability declared: `crates/hares-equipment/src/hvac/air_conditioner.rs:434` (AirConditioner), `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:62,307` (D2ACooler, GshpCooler)
- Silent ignore: `crates/hares-equipment/src/hvac/air_conditioner.rs:1464` — `_ => {}`
- Correct handler (for comparison): `crates/hares-equipment/src/hvac/heat_pump/heater.rs:2159–2164` — delegates to `apply_heating_control_unchecked`

**Root Cause**: The AC cooler's match is an explicit list of handled variants with a `_ => {}` fallthrough. Unlike the HP heater, which uses `_ => { apply_heating_control_unchecked(...) }` as a routing pass-through, the AC just drops unrecognized signals. `ThermalSetpointDelta` was added later and the AC match was never updated.

**Impact**: A controller that dispatches `ThermalSetpointDelta` to an AirConditioner will receive no error (capability check passes) but the setpoint delta is silently ignored. Cooling schedules, DR programs, or user overrides that use delta-based adjustments will appear to succeed but have no effect on the AC's thermostat.

### Finding 3: [Severity: high] 13 of 25 ControlSignal variants lack constructor methods in `ControlSignalConstructors` trait

**Description**: The `ControlSignalConstructors` trait in `crates/hares-control/src/signal.rs` provides ergonomic constructor methods for only 12 of the 25 `ControlSignal` variants. The missing 13 are: `CurtailmentPercent`, `ReactiveSetpoint`, `PowerFactorSetpoint`, `InverterPriorityMode`, `IdealCapacity`, `ThermalSetpointDelta`, `IdealCapacityModeOverride`, `EvPlugIn`, `EvDrive`, `EvAwayCharge`, `EvSetReadyBy`, `EventDelay`, `MaxCapacityFraction`.

**Code Location**: `crates/hares-control/src/signal.rs:14–43` — trait definition with only 12 methods.

**Root Cause**: Many later-added variants (particularly EV, delta setpoints, inverter control, and capacity limits) were added directly to the enum definition and Python bindings without updating the Rust constructor trait. Rust-native controllers that want to create these signals must construct them directly from the enum variant, which is less ergonomic and bypasses potential validation.

**Impact**: Reduced ergonomics for Rust-native controllers; two-tier API where some signals have typed constructors and others require raw enum construction. No runtime safety impact.

### Finding 4: [Severity: high] MaxCapacityFraction has no Python static constructor

**Description**: The `MaxCapacityFraction` variant is accessible from Python only through `ControlSignal.from_dict({"type": "MaxCapacityFraction", "fraction": 0.5})`. There is no static factory method like `ControlSignal.max_capacity_fraction(fraction=0.5)` in `py_control.rs`, and it is absent from the `PySignal` enum in `py_actor.rs`, meaning Python actor controllers cannot emit it.

**Code Location**:
- `from_dict` support: `crates/hares-python/src/py_control.rs:382–384`
- Missing static method: `crates/hares-python/src/py_control.rs:22–274` (no `max_capacity_fraction` among the `#[staticmethod]` methods)
- Missing from PySignal: `crates/hares-python/src/py_actor.rs:201–256`

**Root Cause**: The variant was added after the Python bindings were written; the `from_dict` path was updated but the static constructor was overlooked.

**Impact**: Python actors cannot set max capacity fractions on equipment. The only path is through `from_dict`, which is a deserialization interface not typically used in control loops.

### Finding 5: [Severity: medium] EventDelay has no Python static constructor

**Description**: The `EventDelay` variant is accessible from Python through `from_dict` and through the `PySignal` actor API, but there is no static factory method like `ControlSignal.event_delay(delay_s=300.0)` in `py_control.rs`. This is less severe than Finding 4 because the `PySignal::EventDelay` variant exists, giving actor-based controllers a path to construct it.

**Code Location**:
- `from_dict`: `crates/hares-python/src/py_control.rs:379–381`
- PySignal variant: `crates/hares-python/src/py_actor.rs:253–255,375`
- Missing static method: `crates/hares-python/src/py_control.rs:22–274`

**Root Cause**: Same as Finding 4 — partial update of Python bindings when the variant was added.

**Impact**: Non-actor Python code (direct `Dwelling.apply_control` calls) must use `from_dict` to create `EventDelay` signals, which is less type-safe and discoverable than a typed static constructor.

### Finding 6: [Severity: medium] ThermalSetpointDelta silently ignored by AirConditioner cooling core (matching Finding 2 detail)

**Description**: Beyond the AC root case in Finding 2, the issue extends to the `CoolingCore::apply_control_unchecked` method (`air_conditioner.rs:1417`), which is the actual implementation for all AC variants including the standard `AirConditioner`, `D2ACooler`, and `GshpCooler`. All three declare `THERMAL_SETPOINT_DELTA` and all three delegate to `CoolingCore` which drops the signal.

**Code Location**:
- `CoolingCore::apply_control_unchecked`: `crates/hares-equipment/src/hvac/air_conditioner.rs:1417–1467`
- D2ACooler delegate: `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:262–264`
- GshpCooler delegate: `crates/hares-equipment/src/hvac/heat_pump/cooler.rs:326–328` (via `delegate_equipment!`)

**Root Cause**: The `CoolingCore` match arms were never extended to include `ThermalSetpointDelta`. The catch-all `_ => {}` masks the gap.

**Impact**: Same as Finding 2 — silent failure for AC equipment. Combined scope: all cooling equipment types (AirConditioner, D2ACooler, GshpCooler) silently discard `ThermalSetpointDelta`.

### Finding 7: [Severity: low] Inconsistent catch-all patterns across equipment types

**Description**: Equipment types exhibit three different strategies for handling unrecognized signals in `apply_control_unchecked`:
1. **Error-reject**: Battery (`_ => Err(...)` at `battery/mod.rs:1242`), PV (line 690), generator (line 945), ventilation (line 567), event_load (line 694), wet_appliance (line 1188), scheduled_load (line 681), EV (line 1001)
2. **Silent ignore**: AC Cooler (`_ => {}` at `air_conditioner.rs:1464`), IdealHvac (line 778), Dehumidifier (line 471), all water heaters (resistance line 738, HPWH line 1033, gas line 705, tankless line 522)
3. **Delegating**: HP Heater (`_ => { apply_heating_control_unchecked(...) }` at `heater.rs:2159`)

Strategies 2 and 3 both have risk of silent signal loss. Strategy 1 compounds with the capability gate: if a signal passes the capability check but the `apply_control_unchecked` doesn't handle it, the equipment returns an error that the dispatcher logs as a warning. This means a capability declaration without an implementation arm produces logged errors rather than silent failure.

**Code Location**: Listed above for each equipment type.

**Root Cause**: No convention was established. Some equipment types were written with the philosophy "only receive what you declare" (so the catch-all is unreachable), others added capability flags later without updating their match arms, and the AC case is a bug where the capability WAS declared but the arm was never added.

**Impact**: Difficulty auditing control signal coverage. The inconsistent patterns make it hard to determine by reading the code whether a missing arm is intentional or an oversight.

### Finding 8: [Severity: low] 9 of 25 variants lack doc comments describing expected equipment behavior

**Description**: The following 9 `ControlSignal` variants have no doc comments: `ThermalSetpoint`, `HumiditySetpoint`, `PowerSetpoint`, `PowerLimit`, `SOCTarget`, `ModeOverride`, `DutyCycle`, `LoadFraction`, `GridConnect`, `SelfConsumption`, `DemandResponse`, `ProtocolNative`, `CurtailmentPercent`, `ReactiveSetpoint`, `PowerFactorSetpoint`, `InverterPriorityMode`. Of these, 10 are core signals that every equipment developer needs to understand.

The variants that DO have doc comments are: `ThermalSetpointDelta`, `IdealCapacityModeOverride`, `EvPlugIn`, `EvDrive`, `EvAwayCharge`, `EvSetReadyBy`, `EventDelay`, `MaxCapacityFraction`.

**Exact count**: 17 variants without doc comments, 8 with doc comments.

**Code Location**: `crates/hares-types/src/control_signal.rs:38–146`

**Root Cause**: Documentation was added retroactively only for newer variants; the original set of signals was never documented.

**Impact**: An equipment developer adding a new equipment type cannot determine from the `ControlSignal` definition alone which signals their equipment must handle. They must read other equipment implementations to infer conventions.

### Findings Summary Table

| # | Variant | Constructible | Dispatch (Cap Gate) | Equipment Consumer(s) | Python Static Ctor | PySignal |
|---|---------|--------------|---------------------|-----------------------|-------------------|----------|
| 1 | ThermalSetpoint | Yes | Yes | AC, HP, IdealHvac, all WH, baseboard, boiler, furnace | Yes | Yes |
| 2 | HumiditySetpoint | Yes | Yes | Dehumidifier | Yes | No |
| 3 | PowerSetpoint | Yes | Yes | Battery, EV, PV, generator, EventBasedLoad, ScheduledLoad | Yes | Yes |
| 4 | PowerLimit | Yes | Yes | AC, HP, Battery, EV, PV, all WH | Yes | Yes |
| 5 | SOCTarget | Yes | Yes | Battery, EV | Yes | Yes |
| 6 | ModeOverride | Yes | Yes | AC, HP, all WH, dehumidifier, ventilation, generator, EventBasedLoad, ScheduledLoad, IdealHvac | Yes | Yes |
| 7 | DutyCycle | Yes | Yes | AC, HP, all WH | Yes | Yes |
| 8 | LoadFraction | Yes | Yes | AC, HP, all WH, ventilation, EventBasedLoad, ScheduledLoad, IdealHvac | Yes | Yes |
| 9 | GridConnect | Yes | Yes | Battery | Yes | No |
| 10 | SelfConsumption | Yes | Yes | Battery, generator | Yes | No |
| 11 | DemandResponse | Yes | Yes | AC, HP, all WH, Battery, ventilation | Yes | Yes |
| 12 | ProtocolNative | Yes | **NO** — no equipment declares PROTOCOL_NATIVE | **NONE** | Yes | No |
| 13 | CurtailmentPercent | Yes | Yes | PV | Yes | No |
| 14 | ReactiveSetpoint | Yes | Yes | PV | Yes | No |
| 15 | PowerFactorSetpoint | Yes | Yes | PV | Yes | No |
| 16 | InverterPriorityMode | Yes | Yes | PV | Yes | No |
| 17 | IdealCapacity | Yes | Yes | AC, HP, IdealHvac, baseboard, boiler, furnace | Yes | Yes |
| 18 | ThermalSetpointDelta | Yes | Yes | HP heater, IdealHvac, thermostat (baseboard/boiler/furnace via hvac_core) | Yes | Yes |
| 19 | IdealCapacityModeOverride | Yes | Yes | IdealHvac | Yes | No |
| 20 | EvPlugIn | Yes | Yes | EV | Yes | Yes |
| 21 | EvDrive | Yes | Yes | EV | Yes | Yes |
| 22 | EvAwayCharge | Yes | Yes | EV | Yes | Yes |
| 23 | EvSetReadyBy | Yes | Yes | EV | Yes | Yes |
| 24 | EventDelay | Yes | Yes | EventBasedLoad, WetAppliance | **No** | Yes |
| 25 | MaxCapacityFraction | Yes | Yes | AC, HP, HvacEquipment (hvac_core) | **No** | **No** |

## Summary
- **Total findings**: 8
- **Critical**: 2 (Finding 1: ProtocolNative orphan; Finding 2: AC silently ignores ThermalSetpointDelta)
- **High**: 2 (Finding 3: incomplete Rust constructors; Finding 4: MaxCapacityFraction missing Python ctor)
- **Medium**: 2 (Finding 5: EventDelay missing Python ctor; Finding 6: ThermalSetpointDelta scope)
- **Low**: 2 (Finding 7: inconsistent catch-all patterns; Finding 8: missing doc comments)

## Recommendations

1. **Fix ProtocolNative orphan (Finding 1)**: Either implement a consumer equipment type that handles `ProtocolNative` (e.g., a generic protocol-bridge equipment that parses the payload for a registered protocol handler), or deprecate the variant and the `PROTOCOL_NATIVE` capability flag. The current state is dead code.

2. **Fix AC ThermalSetpointDelta silent ignore (Finding 2, Finding 6)**: Add a `ControlSignal::ThermalSetpointDelta` match arm to `CoolingCore::apply_control_unchecked` at `crates/hares-equipment/src/hvac/air_conditioner.rs:1417` that delegates to `self.hvac.apply_control_signal(signal)`, matching the pattern used in `HeatPumpHeaterCore` at `crates/hares-equipment/src/hvac/heat_pump/heater.rs:2159–2164`.

3. **Add static Python constructors for EventDelay and MaxCapacityFraction (Findings 4, 5)**: Add `ControlSignal.event_delay(delay_s)` and `ControlSignal.max_capacity_fraction(fraction)` static methods to `crates/hares-python/src/py_control.rs`. Also add `MaxCapacityFraction` to the `PySignal` enum in `crates/hares-python/src/py_actor.rs` if Python actors should be able to emit capacity fraction limits.

4. **Complete the Rust ControlSignalConstructors trait (Finding 3)**: Add constructor methods for the missing 13 variants. This is lower priority since Rust controllers can use raw enum construction, but it improves API consistency.

5. **Standardize catch-all handling (Finding 7)**: Adopt a convention for equipment `apply_control_unchecked` methods. Recommended approach: always error-reject on unrecognized signals (`_ => Err(...)`) to surface capability/implementation mismatches. This prevents the silent-ignore class of bugs that Finding 2 represents. An even stronger approach would be to make the match exhaustive (no catch-all) and let the compiler catch missing arms, but this would require every equipment to explicitly list all signals they declare support for and explicitly reject the rest.

6. **Add doc comments (Finding 8)**: Add Rust doc comments to each `ControlSignal` variant describing expected equipment behavior, which equipment types normally respond to it, and what the numeric fields mean. At minimum, document `ThermalSetpoint`, `PowerSetpoint`, `SOCTarget`, and `ModeOverride` as the four most commonly used signals.

7. **Add compile-time verification**: As the `scripts/triage.py:67` comment notes, there is "no compile-time test that every ControlSignal variant has at least one equipment declarant." Add a test that iterates all `ControlCapabilities` flags and verifies at least one equipment type registers each flag, or a test that constructs/dispatches each variant and verifies non-rejection by at least one equipment type.

## References / Citations
- `ControlSignal` enum: `crates/hares-types/src/control_signal.rs:38–146`
- `ControlCapabilities` bitflags: `crates/hares-types/src/control_signal.rs:156–186`
- Equipment trait with capability gate: `crates/hares-equipment/src/lib.rs:118–138`
- Dispatch routing (no match arms — pure routing): `crates/hares-core/src/dwelling/mod.rs:446–495`
- Triage note about missing validation: `scripts/triage.py:67`
