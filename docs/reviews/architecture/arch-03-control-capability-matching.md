# Control capability matching: equipment declares capabilities, dispatcher matches signals
**Review ID**: arch-03
**Category**: architecture
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-control/src/capabilities.rs` — capability introspection and `can_accept()` utility
- `crates/hares-control/src/dispatch.rs` — `ControlDispatcher` queuing, priority tiers, `DispatchTarget`
- `crates/hares-control/src/signal.rs` — `ControlSignalConstructors` trait with ergonomic constructors
- `crates/hares-types/src/control_signal.rs` — `ControlSignal` enum (25 variants), `ControlCapabilities` bitflags (25 flags), `required_capability()`, `ensure_signal_supported()`
- `crates/hares-core/src/dwelling/mod.rs` — `ControlDispatcher` dispatch logic, `route_request()`, `apply_to_matching()` wiring
- `crates/hares-equipment/src/lib.rs` — `Equipment` trait: `apply_control()` capability gate, `validate_signal()`
- All equipment module files in `crates/hares-equipment/src/` — per-equipment `control_capabilities` declarations

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Equipment.py` — base class `update_external_control(control_signal)`; OCHRE uses flat dicts, no capability flags
- `vendors/OCHRE/ochre/Simulator.py` — `update_model(control_signal)` dispatch; `start_sub_update()` routes control to sub-simulators by name
- `vendors/OCHRE/ochre/Equipment/HVAC.py` — HVAC external control: Setpoint, Deadband, Max Capacity Fraction, Capacity, Load Fraction, Duty Cycle
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py` — WH external control: Setpoint, Deadband, Max Power, Load Fraction, Duty Cycle
- `vendors/OCHRE/ochre/Equipment/PV.py` — PV external control: P Setpoint, P Curtailment (kW or %), Q Setpoint, Power Factor, Priority
- `vendors/OCHRE/ochre/Equipment/Generator.py` — Generator external control: Self Consumption Mode, Max Import/Export Limit, P Setpoint
- `vendors/OCHRE/ochre/Equipment/Battery.py` — Battery model
- `vendors/OCHRE/ochre/Equipment/EV.py` — EV model

## Findings

### Finding 1: PROTOCOL_NATIVE capability has zero declarants — dead signal path
**Severity**: high
**Description**: The `ControlSignal::ProtocolNative` variant (line 88, `control_signal.rs`) maps to `ControlCapabilities::PROTOCOL_NATIVE` (bit 10), but no equipment type in the entire codebase declares this capability. Any dispatch of a `ProtocolNative` signal will be rejected by `ensure_signal_supported()` inside `Equipment::apply_control()` (line 127, `lib.rs`), producing a warning and silently dropping the signal. This is a dead code path — the signal exists but can never be delivered.
**Code Location**:
  - Signal variant: `crates/hares-types/src/control_signal.rs:88-91`
  - Capability flag: `crates/hares-types/src/control_signal.rs:170` (`1 << 10`)
  - Required capability mapping: `crates/hares-types/src/control_signal.rs:210`
  - Capability gate: `crates/hares-equipment/src/lib.rs:127`
  - Zero declarants: confirmed by full grep of `ControlCapabilities::PROTOCOL_NATIVE` across `crates/hares-equipment/src/`
**Root Cause**: `PROTOCOL_NATIVE` was added as a bitflag and signal variant for future extensibility (e.g., Modbus, BACnet, vendor-specific protocols) but no equipment was ever wired to accept it. The flag was likely added speculatively without a corresponding equipment implementation.
**Impact**: A controller attempting to send a `ProtocolNative` signal to any equipment will observe successful queuing into the `ControlDispatcher` but the signal will be silently rejected at the `apply_control()` gate. The rejection is logged only as a `tracing::warn!` message, making it easy to miss in production. This creates a silent failure mode where protocol-level integration appears to work from the controller's perspective but has zero effect on equipment behavior. Compare with OCHRE: OCHRE has no equivalent typed protocol signal — all control is via named dict keys, avoiding this class of dead-path issue.

### Finding 2: EV does not declare DEMAND_RESPONSE — key DR participant excluded
**Severity**: medium
**Description**: Electric vehicles are primary demand response participants in the field (managed charging, V2G, grid emergency load shed). However, the HARES EV model declares only `POWER_SETPOINT | SOC_TARGET | POWER_LIMIT | EV_PLUG_IN | EV_DRIVE | EV_AWAY_CHARGE | EV_SET_READY_BY` (line 134-140, `ev/mod.rs`) — `DEMAND_RESPONSE` is absent. A `DemandResponse` signal dispatched to an EV (by name or by `EndUse::EV`) will be rejected at the capability gate. By contrast, `Battery` declares `DEMAND_RESPONSE` (line 393, `battery/mod.rs`), and OCHRE's EV model supports `update_external_control` though it doesn't explicitly handle DR events as a named concept.
**Code Location**:
  - EV capability declaration: `crates/hares-equipment/src/ev/mod.rs:134-140`
  - Battery includes DEMAND_RESPONSE: `crates/hares-equipment/src/battery/mod.rs:393`
**Root Cause**: The `DEMAND_RESPONSE` capability was added to Battery and thermal equipment but the EV implementation was not updated to include it. The `apply_control_unchecked` in `ev/mod.rs` checks for `DemandResponse` but delegates to a `handle_demand_response()` method — however the capability gate at the `Equipment::apply_control()` trait level rejects the signal before the unchecked method is reached.
**Impact**: `DemandResponse` signals dispatched to `EndUse::EV` are silently dropped. DR controllers that target all curtailable loads by end-use would expect EV participation but observe no effect. Grid emergency scenarios cannot command EV charging curtailment via the DR signal path.

### Finding 3: PRICE_SIGNAL control variant absent from the architecture
**Severity**: medium
**Description**: The review asked to verify `HAS_PRICE_SIGNAL` by price-responsive equipment (EV, battery). However, the `ControlSignal` enum has no `PriceSignal` variant, and the `ControlCapabilities` bitflags have no `PRICE_SIGNAL` flag. Price-based control (time-of-use rates, real-time pricing, coincident peak pricing) has no typed signal path. Price-responsive equipment (Battery, EV, Generator with SELF_CONSUMPTION) cannot receive explicit price targets. OCHRE avoids explicit price signals by embedding pricing in schedules — HARES similarly relies on schedule-based or actor-based price awareness, but the lack of a `PriceSignal` variant means controllers have no standard way to communicate dynamic pricing to equipment.
**Code Location**:
  - `ControlSignal` enum: `crates/hares-types/src/control_signal.rs:37-146` (no `PriceSignal` variant)
  - `ControlCapabilities` bitflags: `crates/hares-types/src/control_signal.rs:158-186` (no `PRICE_SIGNAL` flag)
  - Price-responsive equipment: `battery/mod.rs:388-393` (has SELF_CONSUMPTION), `ev/mod.rs:134-140`, `generator.rs:519-521` (has SELF_CONSUMPTION)
**Root Cause**: The architecture delegates price awareness to Actor-level logic (e.g., `BatteryActor`, `EvActor`) rather than exposing price as a typed control signal flowing through the dispatcher. This is an intentional design choice but represents a gap compared to market-integrated controllers that emit explicit price signals.
**Impact**: External controllers cannot inject price-based control into the dwelling. Integration with real-time pricing markets, OpenADR price signals, or tariff-aware optimizers requires architectural work to add a `PriceSignal { price_per_kwh: f64, tier: PriceTier }` variant and corresponding equipment declarations.

### Finding 4: Simple HVAC equipment (Baseboard, Boiler, Furnace) lack MODE_OVERRIDE and DEMAND_RESPONSE
**Severity**: medium
**Description**: `Baseboard` (line 67, `baseboard.rs`), `Boiler` (line 140, `boiler.rs`), and `Furnace` (line 93, `furnace.rs`) declare only `THERMAL_SETPOINT | THERMAL_SETPOINT_DELTA | IDEAL_CAPACITY`. They do not declare `MODE_OVERRIDE` or `DEMAND_RESPONSE`. This means these heating devices cannot be forced on/off and cannot participate in demand response events. By contrast, `HeatPumpHeater` (line 428, `heater.rs`) and `AirConditioner` (line 433, `air_conditioner.rs`) declare both. In OCHRE, the `Heater` base class (line 63, `HVAC.py`) supports `update_external_control()` with setpoint/deadband/capacity control and has a `control_type` field (Time/Time2/Setpoint) — all heating equipment types are controllable.
**Code Location**:
  - Baseboard: `crates/hares-equipment/src/hvac/baseboard.rs:67-69`
  - Boiler: `crates/hares-equipment/src/hvac/boiler.rs:140-142`
  - Furnace: `crates/hares-equipment/src/hvac/furnace.rs:93-95`
  - Compare HP Heater: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:428-436`
  - Compare AC: `crates/hares-equipment/src/hvac/air_conditioner.rs:433-441`
**Root Cause**: The capability declarations for Baseboard, Boiler, and Furnace were written with a minimal set (thermal setpoint control only) and never expanded to include `MODE_OVERRIDE` or `DEMAND_RESPONSE`. Their internal `apply_control_unchecked()` methods may or may not handle mode override logic — the capability gate prevents the signal from ever reaching them.
**Impact**: `ModeOverride` signals to `EndUse::HVAC_HEATING` will reach only heat pump heating equipment, not baseboards, boilers, or furnaces. `DemandResponse` signals routed to `EndUse::HVAC_HEATING` will similarly miss these equipment types. This creates an inconsistency where some heating equipment participates in DR and others don't, depending on technology type rather than end-use.

### Finding 5: Generator lacks GRID_CONNECT while Battery declares it
**Severity**: low
**Description**: `Battery` declares `GRID_CONNECT` (line 390, `battery/mod.rs`) enabling it to receive binary grid connect/disconnect signals for islanding support. `Generator` (line 519-521, `generator.rs`) declares `SELF_CONSUMPTION` but NOT `GRID_CONNECT`. In islanded microgrid scenarios, generators are the primary island-forming source and need to know grid connection state. OCHRE's Generator (line 67, `Generator.py`) handles both "Self Consumption Mode" and import/export power limits — the equivalent of combined grid+self-consumption control.
**Code Location**:
  - Generator: `crates/hares-equipment/src/generator.rs:519-521`
  - Battery comparator: `crates/hares-equipment/src/battery/mod.rs:388-393`
**Root Cause**: `GRID_CONNECT` was added to Battery for storage-centric islanding but Generator was not updated to include it. Generator's `SELF_CONSUMPTION` capability addresses a different concern (prioritizing self-generated power over grid import).
**Impact**: Islanding logic must use `SelfConsumption` signals for generators instead of `GridConnect`, creating an API inconsistency. A controller dispatching `GridConnect(false)` to all equipment by `EndUse::GENERATOR` would be silently rejected.

### Finding 6: No compile-time or integration-test verification of signal→capability→equipment coverage
**Severity**: low
**Description**: The mapping from `ControlSignal` variants → `ControlCapabilities` flags → equipment declarations has no automated validation. There is no test that iterates all `ControlSignal` variants and verifies at least one equipment type declares the corresponding capability. Finding 1 (dead `PROTOCOL_NATIVE`) would have been caught by such a test. The `ControlCapabilities` test suite (`control_signal.rs:245-422`) validates bitflag composition, JSON round-trips, and `ensure_signal_supported()` behavior, but never checks cross-coverage between signal variants and equipment declarations.
**Code Location**:
  - `required_capability()`: `crates/hares-types/src/control_signal.rs:197-227`
  - Missing test: no integration test in `crates/hares-types/tests/` or `crates/hares-equipment/tests/` validates full coverage
**Root Cause**: The test strategy validates the mechanism (bitflag operations, round-trips) but not the completeness of the capability graph. OCHRE avoids this class of issue because equipment directly inspects dict keys — there's no separate declaration to validate.
**Impact**: Future additions of `ControlSignal` variants may go undeclared by any equipment without automated detection. Dead signal paths will accumulate over time.

### Finding 7: Event-Based Load constructor at line 716 omits POWER_SETPOINT while line 233 includes it
**Severity**: low
**Description**: `EventLoad` has two capability declaration sites:
  - Line 233: `LOAD_FRACTION | MODE_OVERRIDE | POWER_SETPOINT | EVENT_DELAY`
  - Line 716: `LOAD_FRACTION | MODE_OVERRIDE | EVENT_DELAY` (no `POWER_SETPOINT`)
The second constructor produces event-based loads that cannot receive `PowerSetpoint` signals. This asymmetry means a controller dispatching `PowerSetpoint` to an `EndUse` that includes these EventLoad instances would get partial delivery — some accept it, others silently reject it.
**Code Location**: `crates/hares-equipment/src/event_load.rs:233-236` and `716-718`
**Root Cause**: The line 716 constructor appears intended for read-only or purely schedule-driven event loads that don't accept external power targets. However, the capability mismatch between the two constructors creates inconsistent behavior for the same equipment type.
**Impact**: Non-deterministic control behavior depending on which EventLoad constructor was used. A controller has no way to know at dispatch time whether a particular EventLoad instance accepts `PowerSetpoint`.

## Summary
- Total findings: 7
- Critical: 0
- High: 1 — PROTOCOL_NATIVE dead signal path with zero equipment declarants
- Medium: 3 — EV missing DEMAND_RESPONSE; PRICE_SIGNAL absent from architecture; simple HVAC missing MODE_OVERRIDE and DEMAND_RESPONSE
- Low: 3 — Generator missing GRID_CONNECT; no automated coverage verification; EventLoad constructor capability inconsistency

## Recommendations
1. **Fix PROTOCOL_NATIVE dead path (high priority).** Either implement `PROTOCOL_NATIVE` acceptance in at least one equipment type (e.g., EV for OCPP, PV for SunSpec Modbus) with a corresponding `apply_control_unchecked()` handler, or remove the `ProtocolNative` variant and `PROTOCOL_NATIVE` flag from the codebase if they are not planned for near-term implementation. Dead code paths in the control architecture undermine confidence in the dispatch system.
2. **Add DEMAND_RESPONSE to EV.** Add `ControlCapabilities::DEMAND_RESPONSE` to the EV's `control_capabilities` declaration at `ev/mod.rs:134-140` and ensure `apply_control_unchecked()` handles the `DemandResponse` variant. EVs are the most important DR resource in residential settings.
3. **Add MODE_OVERRIDE and DEMAND_RESPONSE to Baseboard, Boiler, Furnace.** Extend capability declarations at `baseboard.rs:67-69`, `boiler.rs:140-142`, and `furnace.rs:93-95` to match `HeatPumpHeater`'s capability set where applicable. OCHRE treats all heating equipment as externally controllable.
4. **Evaluate adding PRICE_SIGNAL.** If price-based control is needed for TOU optimization or market integration, add a `ControlSignal::PriceSignal` variant with a corresponding `ControlCapabilities::PRICE_SIGNAL` flag, and declare it on Battery and EV.
5. **Add GRID_CONNECT to Generator.** Extend `generator.rs:519-521` to include `GRID_CONNECT` for islanding parity with Battery.
6. **Add integration test for signal→capability→equipment coverage.** Write a test in `crates/hares-equipment/tests/` that constructs one instance of each equipment type and verifies that every `ControlCapabilities` flag is declared by at least one equipment type. This prevents future dead signal paths.
7. **Reconcile EventLoad constructors.** Decide whether event-based loads should uniformly accept `POWER_SETPOINT` and make both constructors consistent, or document the asymmetry explicitly.

## References / Citations
- ControlSignal enum: `crates/hares-types/src/control_signal.rs:37-146`
- ControlCapabilities bitflags: `crates/hares-types/src/control_signal.rs:158-186`
- `required_capability()`: `crates/hares-types/src/control_signal.rs:197-227`
- `ensure_signal_supported()`: `crates/hares-types/src/control_signal.rs:230-243`
- `can_accept()`: `crates/hares-control/src/capabilities.rs:8-10`
- Equipment::apply_control gate: `crates/hares-equipment/src/lib.rs:126-129`
- ControlDispatcher dispatch: `crates/hares-core/src/dwelling/mod.rs:339-443`
- Battery capabilities: `crates/hares-equipment/src/battery/mod.rs:388-393`
- EV capabilities: `crates/hares-equipment/src/ev/mod.rs:134-140`
- OCHRE Equipment.update_external_control: `vendors/OCHRE/ochre/Equipment/Equipment.py:176-178`
- OCHRE HVAC external control: `vendors/OCHRE/ochre/Equipment/HVAC.py:255-323`
- OCHRE WH external control: `vendors/OCHRE/ochre/Equipment/WaterHeater.py:88-135`
