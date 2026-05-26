# Python actor binding: custom actor implementation from Python
**Review ID**: py-binding-08
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_actor.rs` (955 lines)
- `crates/hares-core/src/actor.rs` (437 lines)
- `crates/hares-core/src/dwelling/mod.rs` (run_timestep: lines 2305-2500, add_actor: lines 1507-1534)
- `crates/hares-python/src/py_dwelling.rs` (add_actor: lines 654-659)
- `crates/hares-types/src/environment.rs` (EnvironmentState, ZoneState: lines 80-87, 310-343)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/plugin.py` — EnergyPlusPlugin base class with callback dispatch pattern
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — EnergyPlus callback registration with GC pinning (line 63-67, `all_callbacks` global list)
- `vendors/EnergyPlus/src/EnergyPlus/api/func.py` — error callback GC pinning (line 66, `error_callbacks`)
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — StateManager lifecycle (new/reset/delete)

## Findings

### Finding 1: [Severity: high] Python exceptions during `decide()` are silently swallowed

**Description**: When a Python actor's `decide()` method raises an exception, it is caught at line 123-129 of `py_actor.rs` and logged at `tracing::warn` level. The simulation continues with an empty dispatch buffer for that actor. The Rust engine cannot distinguish between "actor intentionally returned no dispatch requests" and "actor crashed with an exception." This can lead to stale control outputs being carried forward from prior timesteps or silently missing safety-critical signals.

**Code Location**: `crates/hares-python/src/py_actor.rs:123-129`
```rust
Err(e) => {
    tracing::warn!(
        actor = %self.name,
        error = %e,
        "Python actor decide() raised exception"
    );
}
```

**Root Cause**: The `Actor` trait's `decide()` method has no return type — it's `fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>)`. This design precludes error propagation through the trait boundary. The Python→Rust FFI exception is caught but has no mechanism to signal failure to the engine.

**Impact**: A Python actor that raises (e.g., `KeyError` on `env["zones"]` or a numpy assertion failure) will cause the simulation to continue without its dispatch outputs. Over multiple timesteps, this can result in undetected safety violations (e.g., freeze protection signals never sent). The log is only `warn`-level, which may be suppressed or missed in batch runs.

**Comparison to EnergyPlus**: EnergyPlus plugins return `int` from callbacks: return value 0 = success, 1 = "terminate with fatal error" (see `plugin.py:110-161`). This allows the engine to stop on plugin failure. HARES lacks this return-value contract.

### Finding 2: [Severity: high] `Python::try_attach` assumes GIL is always held by caller

**Description**: The `decide()` implementation at line 95 uses `Python::try_attach(|py| { ... })` which returns `Option<bool>`. When the GIL is not held by any Python thread, `try_attach` returns `None`. The code at lines 134-139 logs an error and returns, which means the Python actor is silently skipped for that timestep.

**Code Location**: `crates/hares-python/src/py_actor.rs:95, 134-139`
```rust
let executed = Python::try_attach(|py| { ... });
if executed.is_none() {
    tracing::error!(
        actor = %self.name,
        "Python GIL not available for actor decide() - this should not happen in Python-driven simulations"
    );
}
```

**Root Cause**: The code acknowledges in the log message that this "should not happen in Python-driven simulations," but makes no provision for headless Rust-driven simulations that embed Python actors. The pure-Rust dwelling `step()` at `dwelling/mod.rs:1438` calls `run_timestep`, which iterates all actors. If the simulation is driven from Rust (not from a Python event loop), the GIL will not be held.

**Impact**: In a Rust-driven simulation with Python actors registered, those actors are silently skipped every timestep. The `tracing::error!` log is the only indication, and no `Result::Err` is returned to the caller. The actor's dispatch outputs are completely absent, which is worse than returning stale data — it means zero control signals from that actor, potentially causing equipment to run without setpoints or safety signals.

**Comparison to EnergyPlus**: EnergyPlus callbacks are registered from Python and invoked from C++ during `run_energyplus()`. The C++ engine is called from within a Python thread that holds the GIL, so callbacks always execute with GIL held. HARES should either enforce that `PyActorWrapper` can only be added when GIL is held, or use `Python::with_gil` (which blocks until the GIL is acquired) instead of `try_attach`.

### Finding 3: [Severity: medium] `environment_to_py_dict` omits equipment telemetry, internal gains, and price signal

**Description**: The `environment_to_py_dict()` function at lines 143-184 of `py_actor.rs` converts `EnvironmentState` to a Python dict. It currently exposes:
- Zones: `id`, `temperature_c`, `humidity_ratio`, `relative_humidity`, `volume_m3`
- Weather: outdoor temp, humidity, wind, pressure, solar irradiance (GHI/DNI/DHI), solar altitude
- Grid: voltage, frequency
- Current time and timestep resolution

It does **not** expose:
- `equipment_telemetry` (HashMap<String, Telemetry>) — equipment SOC, power, mode status, connection state
- `equipment_core` (HashMap<EquipmentId, CoreOutput>) — typed core outputs
- `price_signal` (PriceSignal) — electricity price, export price, GHG intensity
- `electrical` (ElectricalSummary) — total load, PV generation, battery power
- `custom_domains` — domain-specific state updates
- Any internal gains / occupancy data

**Code Location**: `crates/hares-python/src/py_actor.rs:143-184`

**Root Cause**: The conversion function was implemented piecemeal and never extended to cover the full `EnvironmentState` struct defined in `crates/hares-types/src/environment.rs:312-343`.

**Impact**: Python actors cannot make informed decisions that depend on equipment status (e.g., "battery SOC below 20% → reduce load"), current electricity price, or total electrical load. This limits Python actors to simple thermostat-like controllers that only react to zone temperature and weather.

### Finding 4: [Severity: medium] No timestep budget enforcement for Python actor execution

**Description**: The `run_timestep()` function at `dwelling/mod.rs:2447-2456` iterates all actors and calls `actor.decide()` sequentially with no timeout or wall-clock budget check. When using the `actor_profiling` feature, per-actor elapsed time is recorded at line 2448-2450, but this is purely diagnostic.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2447-2456`
```rust
for (i, actor) in self.actors.iter_mut().enumerate() {
    let start = Instant::now();
    actor.decide(&self.latest_env, &mut self.actor_dispatch_buf);
    self.per_actor_timing.push((i, start.elapsed()));
}
```

**Root Cause**: No mechanism exists to:
1. Declare a maximum per-actor execution budget
2. Interrupt a runaway Python actor (e.g., infinite loop in `decide()`)
3. Accumulate and report drift from real-time in co-simulation mode

**Impact**: A Python actor with an infinite loop or expensive computation (e.g., a reinforcement learning model taking 30+ seconds per inference) will block the entire simulation with no abort. In co-simulation scenarios (e.g., HARES coupled with a building automation system over BACnet/OPC-UA), this can cause missed real-time deadlines and protocol timeout errors on the external system side.

### Finding 5: [Severity: low] No `Drop` implementation for `PyActorWrapper` — reliance on PyO3's `Py<T>` refcount is implicit

**Description**: `PyActorWrapper` (line 70-73) holds a `Py<PyActor>` reference-counted pointer. PyO3's `Py<T>` implements `Drop` to decrement the Python reference count. This is technically correct, but there is no explicit drop guard or assertion that the Python object remains alive.

**Code Location**: `crates/hares-python/src/py_actor.rs:70-73`
```rust
pub struct PyActorWrapper {
    obj: Py<PyActor>,
    name: Arc<str>,
}
```

**Root Cause**: The design relies on `Py<T>`'s built-in GC-rooting behavior, which prevents the Python object from being collected while the Rust wrapper is alive. However, if a bug causes the Python object to be explicitly deleted from the Python side (e.g., via `del actor` while Rust still holds the `Py<PyActor>` reference in the dwelling's `actors` Vec), subsequent calls to `obj.bind(py)` at line 98 would access freed Python memory.

**Impact**: Currently low risk because: (a) `Py<T>` is a strong reference that prevents GC, (b) Python actors are typically registered at initialization and kept alive for the simulation duration. However, if a future feature allows runtime actor removal/replacement, this could become exploitable.

**Comparison to EnergyPlus**: EnergyPlus explicitly pins callback references in a global `all_callbacks` list (runtime.py:67) to prevent Python GC from collecting `CFUNCTYPE`-wrapped callbacks while C++ holds raw C function pointers. The comment at lines 62-66 explicitly warns: "CFUNCTYPE wrapped Python callbacks need to be kept in memory explicitly, otherwise GC takes it...causes undefined behavior but generally segfaults." HARES's approach using `Py<T>` is safer than raw C function pointers.

### Finding 6: [Severity: low] No tests exist for `PyActorWrapper` or the Python actor lifecycle

**Description**: The file `crates/hares-python/src/py_actor.rs` contains no `#[cfg(test)]` module or test functions. None of the unit tests in `crates/hares-core/src/actor.rs` test the Python actor path. The end-to-end tests in `crates/hares-core/tests/` test only Rust-native actors.

**Code Location**: Absence of test module throughout `crates/hares-python/src/py_actor.rs` (955 lines, zero test functions)

**Root Cause**: Testing Python↔Rust FFI requires a Python interpreter at test time, which adds complexity to the Rust test harness. PyO3 supports `#[test]` in Rust with embedded Python through `pyo3::prepare_freethreaded_python()`, but this is not configured in the project.

**Impact**: Regression risk for all the error paths described above: exception handling, GIL absence, type conversion failures, and memory safety. Changes to the `Actor` trait or `EnvironmentState` struct could silently break Python actor behavior.

## Summary
- Total findings: 6
- High: 2 (exception swallowing, GIL assumption)
- Medium: 2 (missing state in env dict, no timing budget)
- Low: 2 (no explicit Drop, no tests)

## Recommendations
1. **Change `Python::try_attach` to `Python::with_gil`** in `py_actor.rs:95` so Rust-driven simulations can invoke Python actors without requiring the GIL to be pre-held. Alternatively, enforce at registration time that the GIL is held and document that Python actors require a Python-driven event loop.
2. **Propagate Python actor errors to the Rust engine** by either: (a) adding a `Result` return to `Actor::decide()`, or (b) storing a per-actor error flag in `PyActorWrapper` and exposing it via a new `Actor::healthy()` trait method. At minimum, upgrade the log level from `warn` to `error` and record the actor name and exception traceback.
3. **Expose `equipment_telemetry`, `price_signal`, and `electrical` in `environment_to_py_dict()`** so Python actors can make equipment-aware decisions. See `EnvironmentState` fields at `environment.rs:319-342`.
4. **Add optional per-actor deadline enforcement** in `run_timestep()` using `std::time::Instant` and a configurable budget (e.g., 100ms default). If an actor exceeds budget, log a warning and optionally skip remaining actors for that timestep. For co-simulation mode, track cumulative drift.
5. **Implement `Drop` on `PyActorWrapper`** to deregister from the dwelling on drop (if the wrapper is removed while the Python object still exists), or add a `#[cfg(debug_assertions)]` check that the Python object is still valid.
6. **Add Python actor integration tests** using `#[cfg(test)]` with `pyo3::prepare_freethreaded_python()`, testing: (a) successful `decide()` returns dispatch requests, (b) exception in `decide()` produces log and empty output, (c) GIL-not-held scenario (if `try_attach` is kept).

## References / Citations
- `crates/hares-python/src/py_actor.rs:94-141` — `decide()` GIL handling and error swallowing
- `crates/hares-python/src/py_actor.rs:143-184` — `environment_to_py_dict()` missing telemetry/gains/price
- `crates/hares-core/src/dwelling/mod.rs:2443-2459` — actor invocation loop with no timeout
- `crates/hares-core/src/dwelling/mod.rs:1507-1511` — actor registration order documentation
- `crates/hares-types/src/environment.rs:312-343` — full `EnvironmentState` fields
- `vendors/EnergyPlus/src/EnergyPlus/api/plugin.py:110-161` — EnergyPlus callback signature with `int` return for error signaling
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py:62-67` — EnergyPlus callback GC pinning pattern
- `crates/hares-python/src/py_dwelling.rs:654-658` — Python-side `add_actor` registration
- `crates/hares-core/src/actor.rs:43-76` — `Actor` trait definition (Send + Sync + 'static, no error return)
