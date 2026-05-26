# HELICS co-simulation integration test coverage gaps
**Review ID**: fleet-python-06
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/helics/__init__.py`
- `python/ochre_next/helics/_types.py`
- `python/ochre_next/helics/broker.py`
- `python/ochre_next/helics/dwelling.py`
- `python/ochre_next/helics/fleet.py`
- `python/ochre_next/helics/runner.py`
- `tests/python/test_helics_dwelling.py`
- `tests/python/test_helics_fleet.py`
- `tests/python/test_helics_integration.py`
- `tests/python/test_helics_broker_runner.py`
- `tests/python/conftest.py` (fixture dependencies)
- `crates/hares-python/src/` (no HELICS coupling at Rust level)

## Vendor/Reference Files Consulted
- EnergyPlus BCVTB (Building Controls Virtual Test Bed) co-simulation robustness patterns (conceptual comparison only)

---

## Findings

### Finding 1: [Severity: critical] No multi-year co-simulation test for time accumulation errors
**Description**: The longest simulation in any HELICS test runs `_TOTAL_STEPS = 10` at `_TIME_RES_S = 60.0`, totalling 600 seconds of simulation time. There is no test that simulates a full year at hourly timesteps (8760 steps) across multiple federates. Floating-point time accumulation over thousands of steps can silently drift beyond HELICS period tolerance, causing time grants to misalign or the broker to reject time requests. The 8760-step weather data already exists in the test infrastructure (`tests/python/test_py_weather.py:32` verifies `len(weather) == 8760`), but no HELICS test leverages it.
**Code Location**: `tests/python/test_helics_integration.py:32-34` — `_TIME_RES_S` and `_TOTAL_STEPS` constants cap all integration tests at 10 steps; `python/ochre_next/helics/dwelling.py:134-141` — the run loop accumulates `sim_time_s = (timestamp - start_time).total_seconds()` with no drift guard.
**Root Cause**: All integration-fixture Dwelling instances are created with `duration_s=_DURATION_S` where `_DURATION_S = int(_TIME_RES_S * _TOTAL_STEPS) = 600`. No parameterized derivation for year-scale durations exists.
**Impact**: Production year-long HELICS co-simulations with hourly periods may encounter silent time misalignment after ~tens of days or produce non-deterministic results depending on platform FPU behavior. This is the highest-risk gap for any deployment targeting annual energy analysis.

### Finding 2: [Severity: critical] Missing multi-federate fault-propagation test — broker detection and peer notification
**Description**: The error-handling integration test `test_federate_cleanup_on_exception` runs only 1 federate against the broker, checking that the local federate disconnects on failure. There is no test where one federate fails mid-simulation in a multi-federate federation and the HELICS broker detects the failure and notifies peer federates. The `HELICS_FLAG_TERMINATE_ON_ERROR` flag is set to `True` in both `HELICSDwelling.__init__` (`dwelling.py:75`) and `HELICSFleet.__init__` (`fleet.py:60`), but its broker-side effect is never verified.
**Code Location**: `tests/python/test_helics_integration.py:433-454` — uses `n_federates=1`; `python/ochre_next/helics/dwelling.py:75` — sets `HELICS_FLAG_TERMINATE_ON_ERROR = True`; `python/ochre_next/helics/fleet.py:60` — same flag.
**Root Cause**: Error handling tests only verify that the local `_FederateProbe.disconnect_called` becomes `True`, but do not validate broker state transitions or other federates' `request_time()` behavior after a sibling fails.
**Impact**: In production, if one dwelling federate crashes while a GridLAB-D federate continues, the remaining federates may hang indefinitely on `request_time()` rather than receiving a time-grant denial or exception, leading to resource leaks in orchestrated deployments.

### Finding 3: [Severity: high] No lockstep enforcement verification for multi-federate time synchronization
**Description**: All multi-federate integration tests use a sequential pattern where the aggregator first publishes its value, then calls `request_time()`, then reads the dwelling's published subscription. This means the dwelling federate advances one step ahead before the aggregator reads its results. There is no test that verifies that an eager federate cannot jump multiple timesteps ahead of its peer — i.e., that the HELICS broker actually enforces lockstep and that `granted_time >= requested_time` is an invariant that holds across all federates.
**Code Location**: `tests/python/test_helics_integration.py:248-254` — aggregator loop synchronizes sequentially; `python/ochre_next/helics/dwelling.py:136` — `self._fed.request_time(sim_time_s)` returns granted time but the return value is discarded; `python/ochre_next/helics/fleet.py:163` — `granted = float(self._fed.request_time(sim_time_s))` with assertion at line 164 but no cross-federate validation.
**Root Cause**: The test design assumes lockstep works and only validates deterministic output correctness. No test instrument probes the granted time across federates simultaneously.
**Impact**: Subtle HELICS core configuration mismatches (e.g., period vs. offset discrepancies) could allow one federate to run ahead, producing either time-aliased control signals or stale telemetry reads.

### Finding 4: [Severity: high] Zone temperature telemetry publication not implemented
**Description**: The review requirement specifies that a dwelling federate should publish telemetry including "zone temperature." The HELICS publication registration in `HELICSDwelling.register_publications()` only registers `total_power_kw` and `reactive_power_kvar`. The `_publish_results()` method (`dwelling.py:191-194`) publishes only these two values. Zone temperature is available via `dwelling.telemetry()` but is never published over HELICS.
**Code Location**: `python/ochre_next/helics/dwelling.py:87-101` — `register_publications` registers only power keys; `python/ochre_next/helics/dwelling.py:191-194` — `_publish_results` reads only `total_power_kw` and `reactive_power_kvar`. Same limitation applies to `HELICSFleet.register_publications()` at `fleet.py:75-100`.
**Root Cause**: The publication set was designed for GridLAB-D power flow coupling and not extended to include thermal telemetry. The Dwelling `telemetry()` method provides zone temperature and other fields, but they are never mapped to HELICS publications.
**Impact**: Any grid-to-building co-simulation that needs zone temperature feedback (e.g., thermal comfort-based demand response) cannot operate over the HELICS interface without reimplementation.

### Finding 5: [Severity: medium] No HELICS core leak or resource-release verification after finalization
**Description**: The broker, dwelling, and fleet tests verify that `disconnect()` is called, but no test validates that all HELICS resources (shared memory segments, ZMQ sockets, broker process handles) are fully released after simulation completion. The `_disconnect_broker` helper in `test_helics_integration.py:164-173` calls `helicsCloseLibrary()` only at broker teardown, but the `destroy_broker` function in `broker.py:113-129` explicitly avoids calling `helicsCloseLibrary()` because it is "process-global." There is no test that runs multiple co-simulation cycles in the same process to detect residual state.
**Code Location**: `python/ochre_next/helics/broker.py:113-129` — `destroy_broker` does not call `helicsCloseLibrary()`; `tests/python/test_helics_integration.py:164-173` — `_disconnect_broker` calls `helicsCloseLibrary()` only in the integration test helper, not in production code paths.
**Root Cause**: HELICS brokers are managed independently from federates. There is no "full teardown" integration test that creates a broker, runs federates, finalizes everything, and verifies that a second broker can be created with the same port.
**Impact**: In long-running services that create and destroy HELICS federations dynamically (e.g., an interactive simulation platform), leaked HELICS state could exhaust port ranges or accumulate orphaned ZMQ connections.

### Finding 6: [Severity: medium] No grid power limit-specific control signal test
**Description**: The existing control signal integration test (`test_control_signal_via_helics`) publishes a `PowerSetpoint` to a dwelling's Battery equipment. The scenario specified in the review requirements — a dwelling subscribing to a *grid power limit* and constraining total import — is not tested. The control routing infrastructure exists (subscription to `grid/control` topic with JSON payload routing per equipment), but no test validates a dispatch that limits aggregate dwelling import power.
**Code Location**: `tests/python/test_helics_integration.py:355-406` — uses `PowerSetpoint` on specific equipment, not a dwelling-level kW limit; `python/ochre_next/helics/dwelling.py:164-189` — `_read_subscriptions` dispatches controls to individual equipment via `apply_control`, but no mechanism for dwelling-level power limit enforcement is exposed.
**Root Cause**: The control signal architecture is equipment-centric (`apply_control(equipment_name, signal)`) rather than dwelling-centric. A grid power limit would need to be consumed differently (e.g., as a tariff signal or as a constraint on total dwelling demand).
**Impact**: Utility-side grid constraint dispatch patterns (e.g., "reduce import to < 4 kW") cannot be tested end-to-end through the HELICS interface. This is a fundamental gap for distribution system co-simulation.

### Finding 7: [Severity: medium] No temporal alignment verification for publication values
**Description**: Integration tests verify that telemetry values are received (e.g., `test_single_dwelling_cosim_with_mock_aggregator` checks `power_trace` is non-empty), but no test asserts that the power value at timestep N corresponds to dwelling state at timestep N. The fleet aggregate test (`test_fleet_cosim_with_mock_aggregator`) validates that `aggregate_power == sum(per_dwelling_power)` at each sampled step, which is a consistency check but not a temporal alignment check — a one-timestep lag in all signals would still satisfy the sum invariant.
**Code Location**: `tests/python/test_helics_integration.py:294-352` — aggregate/sum check at lines 349-350; `tests/python/test_helics_integration.py:281-291` — nominal vs. low-voltage comparison checks magnitude difference but not time-index alignment.
**Root Cause**: The `_FakeFederate` used in unit tests returns `requested` as `granted`, bypassing the broker's time grant logic. The integration test aggregator uses a sequential `request_time` -> `is_updated` pattern that assumes synchrony.
**Impact**: In production with real ZMQ latency, a federate could read stale subscription data from the previous timestep, producing non-physical results that pass existing validation checks.

### Finding 8: [Severity: medium] HELICSDwelling missing `enter_executing_mode` failure test
**Description**: `HELICSFleet`'s unit test suite includes `test_helics_fleet_finalize_on_enter_exception` (`test_helics_fleet.py:404-418`), which verifies that `finalize()` is called and the federate disconnects even when `enter_executing_mode()` raises. `HELICSDwelling` lacks the equivalent test. The `_FakeFederate` class in `test_helics_dwelling.py` does not support a `raise_on_enter` parameter.
**Code Location**: `tests/python/test_helics_fleet.py:404-418` — fleet version exists; `tests/python/test_helics_dwelling.py:54-92` — `_FakeFederate` has no `raise_on_enter` support; `python/ochre_next/helics/dwelling.py:127-141` — `run()` calls `_fed.enter_executing_mode()` at line 133 with no try/except around it.
**Root Cause**: The test fixture for dwelling was not extended to support enter-executing-mode failure, likely because the fleet fixture was written later and incorporated the pattern.
**Impact**: If `enter_executing_mode()` fails in production (e.g., broker connection timeout), the dwelling federate may leak the underlying HELICS federate handle without calling `disconnect()`, since `enter_executing_mode` is called outside the `try/finally` block in `dwelling.py:133`.

### Finding 9: [Severity: low] No BCVTB-pattern validated-results comparison test
**Description**: EnergyPlus's BCVTB validation patterns compare co-simulation output against a known reference result (e.g., a standalone EnergyPlus run). The HARES HELICS tests validate that signals propagate and values are non-zero, but do not compare integrated co-simulation results against a non-HELICS reference simulation. No test asserts that a dwelling run through HELICS produces identical results to a standalone run.
**Code Location**: `tests/python/test_helics_integration.py:281-291` — the nominal vs. low-voltage test checks that voltage changes load, but does not compare against a baseline standalone dwelling run with the same voltage.
**Root Cause**: The test harness creates fresh `Dwelling` instances inside each test; there is no shared reference trace to compare against.
**Impact**: Regressions in the HELICS coupling layer (e.g., a time offset misconfiguration that skews results by one timestep) could go undetected because outputs are validated for internal consistency only, not against a trusted reference.

---

## Summary
- **Total findings**: 9
- **Critical**: 2 (Finding 1: multi-year co-simulation, Finding 2: multi-federate error propagation)
- **High**: 2 (Finding 3: lockstep verification, Finding 4: zone temperature telemetry)
- **Medium**: 4 (Finding 5: core leaks, Finding 6: grid power limit, Finding 7: temporal alignment, Finding 8: enter_executing_mode failure)
- **Low**: 1 (Finding 9: BCVTB reference comparison)

## Scenario Coverage Matrix

| Scenario | Covered? | Assessment |
|---|---|---|
| 1. Federate setup/teardown | Partial | Unit + integration tests cover lifecycle; no leak/cycle-test |
| 2. Time synchronization lockstep | Partial | Times verified for single federates; no cross-federate invariant test |
| 3. Signal subscription (grid power limit) | Missing | Equipment setpoints tested; dwelling-level kW limit not tested |
| 4. Signal publication (zone temp) | Missing | Power telemetry verified; zone temperature not published at all |
| 5. Error handling (broker + peer notification) | Missing | Local cleanup verified; broker detection + peer notification not tested |
| 6. Multi-year co-simulation (8760 steps) | Missing | Max test = 10 steps; no hour-scale or year-scale tests |

## Recommendations

1. **Implement a year-scale integration test** (Critical, Finding 1) — create a parameterized test that runs a dwelling for 8760 hourly steps against a HELICS broker, sampling time grants every N steps and asserting that cumulative drift remains below 1e-6 seconds per step. Use the existing 8760-hour weather data already tested in `test_py_weather.py`.

2. **Add multi-federate fault-propagation test** (Critical, Finding 2) — create a 2-federate scenario where one federate raises after step N, and assert that the peer federate's next `request_time()` returns an error grant or raises a HELICS exception rather than hanging. Add `thread.join(timeout=5.0)` and assert the surviving federate thread exits within the timeout.

3. **Add cross-federate lockstep invariant assertion** (High, Finding 3) — in the existing multi-federate integration tests, record `granted_time` from both federates at each step and assert `abs(granted_aggregator - granted_dwelling) <= 1e-6`. Override `request_time` with a probe that logs the grant (as done in `_FederateProbe`), and extend it to a multi-federate test.

4. **Implement and test zone temperature telemetry publication** (High, Finding 4) — extend `HELICSDwelling.register_publications()` and `_publish_results()` to optionally publish `zone_temperature_c` from `dwelling.telemetry()`. Add a subscription consumer in the mock aggregator that records temperature values and asserts they are within physically plausible bounds (e.g., -20°C to 50°C).

5. **Add HELICS resource leak cycle test** (Medium, Finding 5) — write a test that runs a complete broker -> federate -> finalize cycle 5 times in the same process, asserting each cycle succeeds without port conflicts or HELICS errors. Verify that `_PORT_BY_BROKER` weak references are cleaned up.

6. **Add dwelling-level power limit test** (Medium, Finding 6) — implement a dwelling-level `set_power_limit_kw()` method on PyDwelling (or use the existing price signal as a proxy) and write a HELICS integration test where a grid aggregator publishes a `{"power_limit_kw": 4.0}` signal that the dwelling applies.

7. **Add time-indexed publication validation** (Medium, Finding 7) — extend the aggregator in integration tests to record `(granted_time, value)` pairs and assert that consecutive power values change monotonically with time (no repeats from stale data).

8. **Add HELICSDwelling `enter_executing_mode` failure test** (Medium, Finding 8) — add `raise_on_enter` support to `_FakeFederate` in `test_helics_dwelling.py` and write a test analogous to `test_helics_fleet_finalize_on_enter_exception`.

9. **Add reference-comparison regression test** (Low, Finding 9) — run a dwelling standalone and capture its power trace, then run the same dwelling through HELICS (with a pass-through aggregator that doesn't alter signals), and assert the power traces match to within 1e-6 relative tolerance at every timestep.

## References / Citations
- HELICS User Guide: Federate Flags (HELICS_FLAG_UNINTERRUPTIBLE, HELICS_FLAG_TERMINATE_ON_ERROR) — `python/ochre_next/helics/dwelling.py:74-75`
- HELICS Federate Info time property configuration — `python/ochre_next/helics/dwelling.py:232-258`
- BCVTB co-simulation validation patterns: Wetter, M. (2011). "Co-simulation of building energy and control systems with the Building Controls Virtual Test Bed." Journal of Building Performance Simulation, 4(3), 185–203.
- `helicsCloseLibrary()` process-global teardown caveat — `python/ochre_next/helics/broker.py:116-118`
