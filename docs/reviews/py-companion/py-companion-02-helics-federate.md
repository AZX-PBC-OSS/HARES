# HELICS federate: dwelling and fleet federates, time sync, signal mapping
**Review ID**: py-companion-02
**Category**: py-companion
**Date**: 2026-05-26

## Files Reviewed
python/ochre_next/helics/dwelling.py python/ochre_next/helics/fleet.py python/ochre_next/helics/broker.py python/ochre_next/helics/runner.py python/ochre_next/helics/_types.py

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/api/

## Findings
### Finding 1: [Severity: high] Dwelling federate ignores `request_time` return value — no time-sync validation
**Description**: `HELICSDwelling.run()` calls `self._fed.request_time(sim_time_s)` but discards the return value (the granted HELICS time). If HELICS grants a time different from the request — due to a granularity mismatch, a misbehaving federate, or a time-desynchronization in the co-simulation — the dwelling will step at the wrong simulation time without detecting the discrepancy. In contrast, `HELICSFleet.run()` captures the granted time and asserts monotonicity.
**Code Location**: `python/ochre_next/helics/dwelling.py:136`
```python
self._fed.request_time(sim_time_s)  # return value ignored
```
**Root Cause**: The dwelling loop uses `for timestamp in self._timesteps` and computes `sim_time_s` from wall-clock timestamps independently, assuming HELICS will grant exactly the requested time. No cross-check is performed.
**Impact**: Time desynchronization between the dwelling federate and other federates (grid, weather) can go undetected, leading to silently incorrect simulation results — e.g., control signals applied at wrong timesteps, or load/generation reported at mismatched times.

### Finding 2: [Severity: high] Monotonicity guard uses bare `assert`, removable in optimized Python
**Description**: `HELICSFleet.run()` verifies that granted time is monotonically increasing via `assert granted >= sim_time_s`. When Python runs with `-O` (optimized mode), all `assert` statements are stripped. This safety check is critical for co-simulation correctness and should not be elidable.
**Code Location**: `python/ochre_next/helics/fleet.py:164`
```python
assert granted >= sim_time_s, "granted time must be monotonically increasing"
```
**Root Cause**: The guard was written as an assertion rather than a runtime `if` / `raise` check. HELICS itself can, in rare scenarios, grant non-monotonic times when federates request time out of sequence.
**Impact**: In `-O` deployments, non-monotonic time grants go undetected, potentially causing backward time stepping, duplicate timestep computation, or data corruption in telemetry outputs.

### Finding 3: [Severity: high] Fleet manually increments time independently of HELLICS granted time
**Description**: `HELICSFleet.run()` computes the next time request as `sim_time_s += self._time_res_s` after each step, using a locally-tracked accumulator rather than building the next request from the HELICS-granted time. If `granted > sim_time_s` (e.g., due to a co-simulation granularity adjustment or a delayed federate), the fleet drifts ahead of the actual simulation clock without any warning.
**Code Location**: `python/ochre_next/helics/fleet.py:170-171`
```python
step_count += 1
sim_time_s += self._time_res_s
```
**Root Cause**: The local `sim_time_s` variable is treated as authoritative. The check on line 164 confirms `granted >= sim_time_s` (requested time), but never checks whether `granted` exceeds the request by more than a small epsilon. The correct pattern is `next_request = granted + period`.
**Impact**: Timesteps can be effectively skipped if HELICS advances time by more than one period. The fleet would request and be granted `granted + period` on the next iteration, losing the intermediate step.

### Finding 4: [Severity: medium] Missing `try/except` around `request_time()` — broker disconnection hangs
**Description**: Both `HELICSDwelling.run()` and `HELICSFleet.run()` call `self._fed.request_time()` with no surrounding `try/except`. If the HELICS broker crashes mid-simulation, `request_time()` will either raise an unhandled HELICS exception or block indefinitely (HELICS ZMQ core can block without timeout configuration). Neither federate sets a HELICS time-grant timeout, so the federate will hang rather than fail with a descriptive error.
**Code Location**: `python/ochre_next/helics/dwelling.py:136`, `python/ochre_next/helics/fleet.py:163`
**Root Cause**: No `HELICS_PROPERTY_TIME_GRANT_TIMEOUT` is configured on the federates, and no wall-clock timeout wraps the `request_time()` call. EnergyPlus's `runtime.py` provides `stop_simulation()` for graceful termination, but HARES has no equivalent abort mechanism.
**Impact**: In multi-federate scenarios, one crashed federate causes all other federates to hang indefinitely at the next `request_time()`, requiring manual kill of all processes.

### Finding 5: [Severity: medium] `finalize()` does not protect against disconnect failures
**Description**: Both `HELICSDwelling.finalize()` and `HELICSFleet.finalize()` call `self._fed.disconnect()` without a `try/except`. If the broker has already disconnected (due to error or shutdown), `disconnect()` may raise an exception, masking the original simulation error with a secondary cleanup failure.
**Code Location**: `python/ochre_next/helics/dwelling.py:147`, `python/ochre_next/helics/fleet.py:179`
```python
def finalize(self) -> None:
    if self._finalized:
        return
    self._fed.disconnect()  # no try/except
    self._finalized = True
```
**Root Cause**: The `destroy_broker()` helper in `broker.py:123-129` wraps `disconnect()` in `try/except Exception: pass`, demonstrating awareness of this risk. The same pattern was not applied to federate finalization.
**Impact**: An original `request_time()` failure followed by a `finalize()` failure produces a confusing stack trace where the real error (broker disconnection) is obscured by the cleanup error.

### Finding 6: [Severity: medium] Federate does not destroy C-level resources after disconnect
**Description**: Both federate classes call `self._fed.disconnect()` but never call `helicsFederateDestroy()` or `helicsFederateFree()`. In HELICS, `disconnect()` signals the federate is leaving the federation, but the underlying C handle remains allocated. For short-lived processes this is acceptable (OS reclaims memory on exit), but for long-running processes or test suites that create/destroy many federates, this leaks native HELICS handles.
**Code Location**: `python/ochre_next/helics/dwelling.py:147`, `python/ochre_next/helics/fleet.py:179`
**Root Cause**: The HELICS Python API separates `disconnect` (logical) from `destroy`/`free` (resource). The HARES implementation only performs the logical step.
**Impact**: Memory/resource leak when federates are repeatedly created and destroyed within the same process (e.g., in test suites or iterative parameter sweeps).

### Finding 7: [Severity: medium] No `ControlSignal` variant validation before applying to equipment
**Description**: `_read_subscriptions()` parses JSON into a `ControlSignal` via `ControlSignal.from_dict()` and immediately calls `apply_control()` without checking whether the signal variant is supported by the target equipment. The `Dwelling.validate_control(name, signal)` method (documented in `_hares.pyi:1768`) is never invoked.
**Code Location**: `python/ochre_next/helics/dwelling.py:182-183`, `python/ochre_next/helics/fleet.py:216-217`
```python
signal = ControlSignal.from_dict(signal_body)
self._dwelling.apply_control(equipment, signal)  # no validate_control call
```
**Root Cause**: The `validate_control` API exists in the underlying HARES library but is not wired into the HELICS layer. The EnergyPlus reference (`datatransfer.py`) validates actuator handles and type compatibility before setting values.
**Impact**: Invalid control signal types are caught only at the Rust/C++ layer inside `apply_control()`, potentially causing less-graceful errors than a pre-validation check with a descriptive HELICS-level warning.

### Finding 8: [Severity: medium] Empty control payload silently passes through edge cases
**Description**: In `HELICSFleet._iter_control_entries()`, an empty dict `{}` returns `[]` (line 311-312), which causes `_read_subscriptions()` to proceed without updating any controls. An empty list from `_iter_control_entries()` is indistinguishable from "no valid entries parsed" — there is no logging or distinction between an intentionally empty control message and a malformed one.
**Code Location**: `python/ochre_next/helics/fleet.py:311-312`
```python
if not payload:
    return []
```
**Root Cause**: The empty-dict shortcut returns cleanly without indicating to the caller or logging.
**Impact**: A grid federate that publishes `{}` intending to clear all controls will have no effect; the dwellings silently retain their previous control signals.

### Finding 9: [Severity: low] Period inference from single timestep produces hardcoded 1.0s fallback
**Description**: `HELICSDwelling._peek_timing()` needs at least two timestamps to compute the simulation period. When only one is available (e.g., a short-duration simulation), it defaults to `period_s = 1.0` with a warning. This hardcoded fallback may mismatch the actual `time_res_s` configured in `DwellingConfig`.
**Code Location**: `python/ochre_next/helics/dwelling.py:207-214`
```python
period_s = 1.0
if second_time is not None:
    period_s = (second_time - start_time).total_seconds()
else:
    _LOG.warning("Dwelling timesteps() yielded a single timestamp; defaulting HELICS period to 1.0s")
```
**Root Cause**: The `Dwelling` object likely has a `time_res_s` or equivalent property on its config, but `_peek_timing()` does not consult it, instead relying exclusively on timestamp differencing.
**Impact**: For simulations with a non-1-second resolution, the HELICS time period is misconfigured, causing the federate to request time at the wrong intervals and potentially desync from other federates.

### Finding 10: [Severity: low] `HELICSDwelling` does not validate voltage/price subscription types
**Description**: In `register_subscriptions()`, the voltage and price subscriptions are registered as `"double"` type but the code never validates that the received value is within a reasonable range (e.g., voltage between 0.8–1.2 p.u., price >= 0). The values are applied directly after `float()` conversion.
**Code Location**: `python/ochre_next/helics/dwelling.py:152-155`, `dwelling.py:157-160`
```python
self._dwelling.set_grid_voltage(float(self._sub_voltage.double))
self._dwelling.set_price_signal({"electricity_price": price})
```
**Root Cause**: No sanity-checking bounds are applied. The EnergyPlus API (`datatransfer.py`) does not validate values either, but EnergyPlus actuators propagate through internal validation layers that reject out-of-range inputs with warning messages.
**Impact**: Errant federates publishing extreme values (e.g., negative prices, zero voltage) cause physically unrealistic dwelling behavior without any HELICS-level diagnostic.

### Finding 11: [Severity: low] No version compatibility check between HARES and HELICS
**Description**: The EnergyPlus API (`api.py:152-158`) performs `verify_api_version_match()` to ensure the Python bindings are compatible with the loaded C library version. The HARES HELICS wrappers perform no such check — the code uses `hasattr()` probes to select between old and new HELICS API surfaces, but never verifies that the selected API surface matches the library version.
**Code Location**: `python/ochre_next/helics/dwelling.py:222-229`, `python/ochre_next/helics/broker.py:222-226`
**Root Cause**: The `hasattr()`-based API probing works for a limited range of HELICS versions (2.x → 3.x transition), but offers no guarantee of behavioral compatibility beyond API surface availability.
**Impact**: Semantic changes in HELICS behavior between versions (e.g., different time-grant semantics, signal delivery ordering) may not be detected until incorrect simulation results are observed.

### Finding 12: [Severity: low] No explicit test coverage for multi-federate data exchange
**Description**: The `python/ochre_next/helics/` directory contains no test files. There are no integration tests that stand up multiple federates (dwelling + grid + weather), run a co-simulation, and assert correct data exchange at each timestep. The review specification explicitly asks about multi-federate scenarios.
**Code Location**: `python/ochre_next/helics/` — no `test_*.py` files exist
**Root Cause**: The HELICS layer is a thin wrapper; unit tests for the underlying `Dwelling`/`SteppableFleet` classes exist elsewhere, but co-simulation integration tests require a HELICS runtime, which may be expensive to set up in CI.
**Impact**: Regressions in time synchronization or signal mapping are likely to be caught only in manual integration testing or production runs.

## Summary
- Total findings: 12
- Critical / High / Medium / Low: 0 / 3 / 6 / 3

## Recommendations
1. **Capture and validate the granted time** in `HELICSDwelling.run()` by storing the return value of `request_time()` and checking it matches the expected simulation time within a small epsilon.
2. **Replace the bare `assert`** in `HELICSFleet.run():164` with an explicit `if granted < sim_time_s: raise RuntimeError(...)` that cannot be compiled away.
3. **Rebase the fleet's time accumulator** on the HELICS-granted time: `sim_time_s = granted + self._time_res_s` rather than blind self-increment.
4. **Set a HELICS time-grant timeout** (`HELICS_PROPERTY_TIME_GRANT_TIMEOUT`) on both federates to prevent indefinite hangs when other federates crash. Wrap `request_time()` in `try/except` with descriptive error messages that identify the failed federate and timestep.
5. **Wrap `finalize()` disconnect calls** in `try/except` to avoid masking original errors during cleanup.
6. **Call `helicsFederateDestroy()` or `helicsFederateFree()`** after `disconnect()` and before marking finalized, to release C-level resources.
7. **Wire in `Dwelling.validate_control()`** before calling `apply_control()` in `_read_subscriptions()` to produce descriptive HELICS-level warnings for unsupported signal types.
8. **Log a warning** when an empty control payload is received in `HELICSFleet` to distinguish intentional clears from malformed messages.
9. **Consult `DwellingConfig.time_res_s`** (or equivalent) in `_peek_timing()` instead of defaulting to 1.0s for single-timestep simulations.
10. **Add integration tests** that run a minimal co-simulation (e.g., two dwelling federates + broker) and assert correct time advancement and signal exchange at each timestep.

## References / Citations
- HELICS User Guide — Time Management: Federates request time via `requestTime()` and HELICS grants the next allowed step based on the federation's time dependencies. Federates must not assume request == grant.
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py:222-223` — EnergyPlus returns an exit code from `run_energyplus()`, providing a clear contract. HARES federates have no equivalent exit protocol for co-simulation errors.
- `vendors/EnergyPlus/src/EnergyPlus/api/api.py:152-158` — `verify_api_version_match()` pattern for guard against library version mismatches.
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py:630-655` — `set_actuator_value()` validates handle and value types with descriptive `EnergyPlusException` messages, analogous to the missing `validate_control()` call in HARES.
