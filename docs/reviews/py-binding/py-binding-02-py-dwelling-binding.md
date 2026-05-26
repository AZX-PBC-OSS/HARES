# Python dwelling binding: lifecycle, step execution, state access
**Review ID**: py-binding-02
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_dwelling.rs` (2209 lines)
- `crates/hares-python/src/py_enums.rs` (2638 lines)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — EnergyPlus `StateManager` lifecycle (new_state → reset_state → delete_state)
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — EnergyPlus `Runtime` callbacks and simulation step control

## Findings

### Finding 1: [Severity: medium]
**Description**: `step()` does not enforce the construction → initialization → stepping lifecycle contract. The `PyDwelling.initialized` flag is maintained but never checked before `step()`, so stepping an uninitialized dwelling proceeds silently instead of raising a Python exception.
**Code Location**: `crates/hares-python/src/py_dwelling.rs:607-644`
**Root Cause**: The `initialized` field (`py_dwelling.rs:519`) is set by `initialize()` (`py_dwelling.rs:553-567`) and reset by `reset_with_seed()` (`py_dwelling.rs:1163`), but `step()` (`py_dwelling.rs:607`) calls `step_core()` directly without any guard on `self.initialized`. The only consumer of this flag is `initialize()` itself (idempotency check, line 554) and `__repr__()` (line 1183-1187). The `step_core()` method (`py_dwelling.rs:1339-1341`) simply acquires the internal mutex and calls `Dwelling::step()`, which succeeds regardless of whether the Python-side `initialize()` was called.
**Impact**: Users who follow the documented lifecycle (construct → initialize → step) see no issue, but users who inadvertently skip `initialize()` get no error. The `initialized` flag exists but serves no enforcement purpose. This contradicts the intended contract described in the review specification. In contrast, EnergyPlus enforces a strict state machine: `state.py` requires `new_state()` before any runtime calls, and `runtime.py` requires a valid state object; passing a null/invalid state would segfault rather than proceed silently.

### Finding 2: [Severity: medium]
**Description**: `PyGridExportRule` is missing `from_str()` and static constructor methods (`solar_only()`, `unrestricted()`, `disabled()`), breaking the consistent enum API pattern established by all other enums in `py_enums.rs`.
**Code Location**: `crates/hares-python/src/py_enums.rs:2075-2108`
**Root Cause**: Every other enum in the file (PyFuelType, PyExecutionStage, PyFluidType, PyInverterPriority, PyDutyCycleComponent, PySimStatus, PyAggregationResolution, PyResStockVersion, PyLutType, PyBatteryChemistry, PyChargingLevel, PyVehicleType, PyEvConnectionState, PyBatteryProductId, PyVehicleId, PyEvArchetypeId) provides both `from_str()` and per-variant static constructors. `PyGridExportRule` only has `__repr__()`. The `#[pyclass(eq, hash, frozen, from_py_object)]` attributes give automatic `__eq__`/`__hash__` and variant access (e.g., `GridExportRule.SolarOnly`), but there is no string-parsing path. A Python user specifying grid export rules in a configuration file must write a manual string-to-variant mapping.
**Impact**: Forces Python users to use enum member access instead of string-based construction, inconsistent with the rest of the API. Medium severity because the enum is still usable via direct variant access.

### Finding 3: [Severity: low]
**Description**: `PyStormWatchTrigger` is missing a `from_str()` static method, preventing string-based construction.
**Code Location**: `crates/hares-python/src/py_enums.rs:2114-2190`
**Root Cause**: The struct provides `manual_enable()` and `weather_signal()` static constructors, plus `__repr__()`, but no `from_str()`. Since `StormWatchTrigger` is a struct (not an enum) with stateful variants (ManualEnable vs WeatherSignal with threshold), `from_str()` is less critical than for simple enums. However, BMS mode configuration code that uses `storm_watch()` (`py_enums.rs:2522-2552`) already accepts a string `trigger` parameter with manual string matching, making the omission symmetrical: the BmsMode constructor parses strings; the standalone trigger type cannot.
**Impact**: Minor inconsistency. Users who need to construct a standalone `StormWatchTrigger` from a string must write their own parsing logic.

### Finding 4: [Severity: low]
**Description**: `ThermalCategory` enum (6 variants: `HvacHeating`, `HvacCooling`, `InternalGain`, `JacketLoss`, `DuctLoss`, `HvacDehumidification`) is not exported to Python in `py_enums.rs`.
**Code Location**: Rust enum defined at `crates/hares-types/src/ports.rs:18-34`; Python bindings absent from `crates/hares-python/src/py_enums.rs`.
**Root Cause**: Thermal category data reaches Python only in aggregated form — the `step()` result returns pre-categorized columns (`hvac_heating_w`, `hvac_cooling_w`, etc.), and observation snapshots (`snapshot_to_py`, `py_dwelling.rs:1831-1953`) expose named fields. The raw per-category breakdown available in Rust via `PortSlots::sensible_for_category(ThermalCategory)` is not directly surfaced. The review specification lists "thermal categories" as a required enum type; however, current Python API usage patterns make this a low-impact gap since categories are handled implicitly.
**Impact**: Users implementing custom equipment or actor logic in Python cannot programmatically reference thermal categories. Workaround: users rely on pre-categorized output columns. Low severity given current API design.

### Finding 5: [Severity: low]
**Description**: No explicit `close()` or `shutdown()` method on `PyDwelling` for deterministic resource release. Resource cleanup relies entirely on Rust's automatic `Drop` triggered by Python GC.
**Code Location**: `crates/hares-python/src/py_dwelling.rs:513-524` (struct definition) — no `close`/`shutdown` method in `#[pymethods]` (lines 526-1336).
**Root Cause**: `Dwelling` (`hares-core/src/dwelling/mod.rs:651`) holds an optional `StreamingRecorder` (line 662), a `ChaCha8Rng` (line 663), multiple solver instances, and pre-allocated buffers. All of these are Rust-owned types that implement `Drop` correctly (file handles in recorders, solver memory, etc.). However, Python's GC is non-deterministic — a `PyDwelling` object may survive in memory long after the user's `del dwelling` call if reference cycles exist. The EnergyPlus reference API (`state.py`) provides an explicit `delete_state()` method for deterministic resource release, which HARES lacks.
**Impact**: In normal single-threaded usage, Rust `Drop` cleanup is sufficient. In long-running server contexts where many dwellings are created and discarded, delayed GC could accumulate memory. No user-reported issues; low severity.

### Finding 6: [Severity: informational]
**Description**: GIL handling in `step()` and `simulate()` is correctly implemented — the GIL is released during Rust computation and re-acquired only when returning Python objects.
**Code Location**: `crates/hares-python/src/py_dwelling.rs:606-644` (step), `py_dwelling.rs:585-603` (simulate)
**Details**: Both methods use `py.detach()` to release the GIL before running the computation closure, then `catch_unwind` with `AssertUnwindSafe` to convert panics to `PyRuntimeError`. In `step()`, the closure calls `step_core()` which internally acquires the `Mutex<Dwelling>` — providing both GIL-free parallelism and thread safety. After the closure returns, the GIL is automatically re-acquired, and results are marshalled into Python objects. The `simulate()` method additionally calls `drop(dwelling)` on line 602 to release the mutex lock before calling `batches_or_steps_to_polars_df()` which requires Python access. This follows the pattern in the PyO3 documentation for long-running computations.
**Impact**: No issue. Verified correct. Note that `step()` takes `&mut self` (Python-level exclusive access), so concurrent `step()` calls from multiple Python threads are serialized by PyO3 before reaching the GIL release. This is a reasonable design choice for a simulation step that must be sequential.

## Summary
- Total findings: 6
- Critical: 0
- High: 0
- Medium: 2 (unchecked initialized flag, missing GridExportRule from_str)
- Low: 3 (missing StormWatchTrigger from_str, missing ThermalCategory bindings, no explicit close/shutdown)
- Informational: 1 (GIL handling confirmed correct)

## Recommendations

1. **Enforce the initialization contract in `step()`**: Add a check at the top of `step()` (after line 607) that raises `PyRuntimeError` if `!self.initialized`, with a descriptive message guiding the user to call `initialize()` first. This turns the maintained `initialized` flag into an enforced guard.

2. **Add `from_str()` and static constructors to `PyGridExportRule`**: Follow the pattern established by `PyDutyCycleComponent` or `PyChargingLevel` — add `solar_only()`, `unrestricted()`, `disabled()` static methods and a `from_str()` accepting `"solar_only"`, `"unrestricted"`, `"disabled"`. This ensures configuration-driven workflows can use string-based enum construction consistently.

3. **Add `from_str()` to `PyStormWatchTrigger`**: Accept strings like `"manual"` and `"weather_signal:<threshold>"` (e.g., `"weather_signal:25.0"`), consistent with the string parsing already done in `PyBmsMode::storm_watch()` at `py_enums.rs:2526-2541`.

4. **Consider adding `PyThermalCategory` enum bindings**: Expose the 6 `ThermalCategory` variants with `from_str()` support. This enables Python-side custom equipment to write to the correct thermal category in port slots. The implementation should mirror the `PyEndUse` pattern (`py_enums.rs:30-151`) — a newtype wrapper around the Rust enum with static class attributes and optional message payload.

5. **Consider adding `Dwelling.close()` for explicit resource release**: A no-arguments method that drops the internal `Mutex<Dwelling>` (replacing it with a poisoned sentinel or taking it via `Option`), releases the `StreamingRecorder` file handles, and clears pre-allocated buffers. This mirrors EnergyPlus's `delete_state()` pattern. All subsequent method calls after `close()` should return `PyRuntimeError`.

## References / Citations

- EnergyPlus API StateManager: `vendors/EnergyPlus/src/EnergyPlus/api/state.py:59-105` — explicit `new_state()` / `reset_state()` / `delete_state()` lifecycle
- EnergyPlus Runtime API: `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py:177-222` — `run_energyplus()` requires pre-constructed state object; null state causes undefined behavior
- Rust `ThermalCategory` enum: `crates/hares-types/src/ports.rs:18-34`
- Rust `GridExportRule` enum: `crates/hares-types/src/equipment.rs:580-585`
- Rust `BmsMode` enum: `crates/hares-types/src/equipment.rs:669-701`
- PyDwelling struct definition: `crates/hares-python/src/py_dwelling.rs:513-524`
- PyDwelling `initialize()`: `crates/hares-python/src/py_dwelling.rs:553-567`
- PyDwelling `step()`: `crates/hares-python/src/py_dwelling.rs:607-644`
- PyDwelling `step_core()`: `crates/hares-python/src/py_dwelling.rs:1339-1341`
- PyO3 GIL management: `py.detach()` usage at `py_dwelling.rs:607-609`, `585-592`
- PyGridExportRule: `crates/hares-python/src/py_enums.rs:2075-2108`
- PyStormWatchTrigger: `crates/hares-python/src/py_enums.rs:2114-2190`
