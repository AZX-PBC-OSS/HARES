# HELICS time sync: HARES timestep matching, end-of-simulation handshake, multi-rate co-simulation
**Review ID**: pygap-06
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/helics/dwelling.py` (298 lines)
- `python/ochre_next/helics/fleet.py` (374 lines)
- `python/ochre_next/helics/_types.py` (23 lines)
- `python/ochre_next/helics/broker.py` (257 lines)
- `tests/python/test_helics_dwelling.py` (457 lines)
- `tests/python/test_helics_fleet.py` (502 lines)
- `tests/python/test_helics_integration.py` (526 lines)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical]
**Description**: Multi-rate co-simulation is broken — HARES unconditionally steps the dwelling at whatever simulation time HELICS grants, even when a faster federate causes intermediate grants. In `dwelling.py:134-136`, the run loop calls `self._fed.request_time(sim_time_s)` but does not compare the granted time to the expected `sim_time_s`. HELICS grants the minimum requested time across all federates. If a grid model federate runs at 1-second timesteps while HARES runs at 60-second timesteps, HELICS grants 1s to HARES. The dwelling then steps at 1s virtual time (the first timestep), then immediately steps again at the second timestep when the next 2s grant arrives — processing all 60 timesteps in 60 seconds of wall time but at completely wrong simulation times. In `fleet.py:163-164`, the assertion `assert granted >= sim_time_s` is semantically inverted — HELICS never grants time *greater* than requested, so this assert will fire on the very first multi-rate step (e.g., granted `1.0 >= 60.0` is `False`). This means the fleet version crashes immediately when a faster federate is present, while the dwelling version silently desynchronizes. Neither handles concurrent `request_time` calls with intermediate grant values.
**Code Location**: `python/ochre_next/helics/dwelling.py:133-139`, `python/ochre_next/helics/fleet.py:155-171`
**Root Cause**: Both run loops assume `granted == requested` — an assumption that only holds when all federates in the federation use identical time periods. The code does not buffer state or skip steps when `granted < requested`. The `HELICS_PROPERTY_TIME_PERIOD` property is set on the federate info but is advisory only; HELICS does not use it to partition time grants. The fleet's `assert granted >= sim_time_s` incorrectly checks for `>=` when HELICS guarantees `granted <= requested`.
**Impact**: Multi-rate co-simulation cannot work. Silent desynchronization in the dwelling path corrupts published telemetry data (power values are associated with wrong timestamps). The fleet path crashes. No integration test exercises a multi-rate scenario — `test_helics_integration.py` uses `_TIME_RES_S = 60.0` uniformly across both federates (aggregator and dwelling).

### Finding 2: [Severity: high]
**Description**: No end-of-simulation handshake — HARES never calls `request_time(helics.HELICS_TIME_MAXTIME)` to signal completion to the federation. After the last timestep, `dwelling.py:141` and `fleet.py:173` call `finalize()` which directly calls `self._fed.disconnect()`. The HELICS protocol expects a federate to request `HELICS_TIME_MAXTIME` or call `helicsFederateFinalize()` to indicate it has no further time steps, allowing the broker to coordinate clean shutdown across all federates. By disconnecting without signalling completion, HARES forces the broker to infer termination from the disconnect, which may cause a delay, a timeout, or leave other federates stalled at their next `request_time` call until the broker detects the broken connection.
**Code Location**: `python/ochre_next/helics/dwelling.py:127-141`, `python/ochre_next/helics/fleet.py:146-173`
**Root Cause**: The `run()` loop simply exits when the timestep iterator is exhausted (dwelling) or `is_finished()` returns true (fleet). There is no explicit "I'm done" signal to HELICS — just a `disconnect()` in `finally`.
**Impact**: Co-simulations with multiple federates may hang for 30-120 seconds at the end (HELICS default federate timeout) waiting for HARES to request its next time step, until the broker processes the disconnect. In federations with strict timeouts, this may cause spurious timeout errors. The integration tests don't observe this because the aggregator also disconnects immediately after its last step (e.g., `test_helics_integration.py:254`), and both threads are daemonic (line 186), so the test process exits cleanly regardless.

### Finding 3: [Severity: high]
**Description**: No detection of federation termination sentinel — when another federate crashes and `HELICS_FLAG_TERMINATE_ON_ERROR` triggers federation termination, the next `request_time()` call returns `helics.HELICS_TIME_MAXTIME` (approximately `9.22e18`). Neither `dwelling.py` nor `fleet.py` checks for this sentinel value. In `dwelling.py:133-139`, the `for timestamp in self._timesteps` loop would continue through all 131,400 remaining timesteps of a year-long simulation, calling `request_time()` each time (which repeatedly returns `HELICS_TIME_MAXTIME`), stepping the dwelling, and publishing results — all completely useless work after the federation is gone. The `uninterruptible` flag (line 74) compounds this by preventing signal interruption.
**Code Location**: `python/ochre_next/helics/dwelling.py:133-139`, `python/ochre_next/helics/fleet.py:155-171`, `python/ochre_next/helics/dwelling.py:74`
**Root Cause**: The run loops assume `request_time()` always returns a meaningful, bounded time. No check like `if requested >= helics.HELICS_TIME_MAXTIME: break` exists. The tests never simulate federation termination — the fake federates always return the requested time (`_FakeFederate.request_time` at `test_helics_dwelling.py:84-87` and `test_helics_fleet.py:92-95`).
**Impact**: After an external federate crash, HARES wastes CPU for hours simulating dead time. For the fleet path specifically, the `assert granted >= sim_time_s` would pass (since `HELICS_TIME_MAXTIME` is enormous), but each dwellstep would still execute uselessly. For the dwelling path, there is no guard at all — the loop runs to exhaustion.

### Finding 4: [Severity: medium]
**Description**: Iterative time mode (`request_time_iterative`) is neither supported nor documented. Both `dwelling.py:136` and `fleet.py:163` use the non-iterative `request_time()`. There is no configuration for `HELICS_PROPERTY_INT_MAX_ITERATIONS`, no flag for `HELICS_FLAG_ITERATION_REQUESTING`, and no iterative convergence logic. This is acceptable if HARES is single-pass only, but the limitation is not stated in docstrings, module-level comments, `README.md`, or error messages. If a user configures the federation for iterative mode (which HELICS supports), HARES would silently operate in single-pass mode, potentially causing convergence failures in other federates.
**Code Location**: `python/ochre_next/helics/dwelling.py:40-52` (class docstring), `python/ochre_next/helics/dwelling.py:127-141` (run loop), `python/ochre_next/helics/fleet.py:26-35` (class docstring), `python/ochre_next/helics/fleet.py:146-173` (run loop)
**Root Cause**: Single-pass is a valid design choice for a building energy simulator that does not participate in power-flow convergence, but this was never documented.
**Impact**: Users configuring iterative HELICS federations may experience unexpected behavior or convergence failures without understanding that HARES ignores iterative time requests.

### Finding 5: [Severity: medium]
**Description**: No validation that HARES's timestep is consistent with other federates. The period is derived from the dwelling/fleet and set as `HELICS_PROPERTY_TIME_PERIOD` (`dwelling.py:247-251`, `fleet.py:273-277`), but there is no comparison against the federation's desired minimum time delta. If HARES uses a 4-minute (240s) period and the grid model uses a 5-minute (300s) period, HELICS will grant at 240s intervals (minimum of all requests), and the grid model's data will be published at misaligned times — 240s, 480s, 720s — instead of the intended 300s, 600s, 900s. The drift accumulates to 60s per step, or 6 hours of misalignment over a 24-hour simulation. There is no WARN-level log when `period_s` is derived from the dwelling timesteps, and no config-cross-check is performed.
**Code Location**: `python/ochre_next/helics/dwelling.py:196-218` (`_peek_timing`), `python/ochre_next/helics/dwelling.py:247-251` (`_configure_federate_info`), `python/ochre_next/helics/fleet.py:51-52`, `python/ochre_next/helics/fleet.py:273-277`
**Root Cause**: Timestep derivation is local to HARES with no inter-federate validation. HELICS itself does not reject federates with incompatible periods; it grants at the minimum.
**Impact**: Data exchange at misaligned timesteps — HARES publishes power at 4-minute boundaries while the grid model provides voltage at 5-minute boundaries, leading to stale or zero-padded data depending on HELICS publication grant semantics.

### Finding 6: [Severity: low]
**Description**: No time-sync logging at all. There is no DEBUG-level log for each `request_time` call (requested, granted), no INFO-level log for simulation start (`enter_executing_mode`), period boundaries, or finalization, and no WARN-level log for sync anomalies (e.g., granted time != requested time, or `HELICS_TIME_MAXTIME` detection). The only logging in the HELICS modules is for error recovery in subscription parsing (`dwelling.py:155,162,171,177,185`). For a year-long simulation with 4-minute timesteps (131,400 steps), adding a DEBUG-level `"request_time(%.1f) -> %.1f" % (requested, granted)` per step would generate 131,400 lines — manageable at ~4 MB of log output — while providing essential diagnostic visibility.
**Code Location**: `python/ochre_next/helics/dwelling.py:127-141`, `python/ochre_next/helics/fleet.py:146-173`, all of `python/ochre_next/helics/dwelling.py` (no `_LOG.debug` calls anywhere), all of `python/ochre_next/helics/fleet.py` (no `_LOG.info` calls anywhere)
**Root Cause**: The logger `_LOG` is instantiated at module level (line 25) but used only for WARNING-level error recovery. No INFO or DEBUG calls are present in the time-sync path.
**Impact**: Debugging a time-sync problem in a 131,400-step simulation requires external HELICS tracing or `helics.apps.Player`/`Recorder` wrappers because HARES itself provides zero visibility into federation timing.

### Finding 7: [Severity: low]
**Description**: Single-timestep default period is 1.0 second — when `dwelling.timesteps()` yields only one timestamp, `_peek_timing()` defaults `period_s = 1.0` (line 207). This produces a WARNING about the single timestamp (line 212-214) but does not warn about the severe performance implication: a 1.0s period means HELICS would grant time at every 1-second boundary, requiring 86,400 steps per simulated day. The default should be a reasonable building simulation timestep (e.g., 60s or 300s), or a ValueError should be raised since a single-timestep dwelling is almost certainly a configuration error.
**Code Location**: `python/ochre_next/helics/dwelling.py:207`
**Root Cause**: The fallback value `period_s = 1.0` was chosen as a safe minimal default, but for building energy simulation it is pathologically small.
**Impact**: If triggered, a single-timestep dwelling causes extremely slow co-simulation with 86,400x the expected step count. The existing WARNING mentions "single timestamp" but does not convey the performance cost.

## Summary
- Total findings: 7
- Critical: 1 (multi-rate co-simulation silently broken)
- High: 2 (no end-of-simulation handshake, no federation termination detection)
- Medium: 2 (no iteration mode documentation, no inter-federate timestep validation)
- Low: 2 (no time-sync logging, extreme 1.0s default period)

## Recommendations
1. **Multi-rate support**: After `request_time(sim_time_s)`, compare `granted` to `sim_time_s`. If `granted < sim_time_s`, do not step the dwelling — just publish results (which are unchanged from the last step) and call `request_time` again with the original `sim_time_s`. Use a while-loop pattern: keep requesting the same `sim_time_s` until `granted >= sim_time_s`, then step and publish. This lets faster federates run sub-steps without HARES stepping prematurely.
2. **End-of-simulation handshake**: After the final timestep's `_publish_results()`, call `self._fed.request_time(helics.HELICS_TIME_MAXTIME)` to signal completion, then call `self.finalize()` (already in `finally`). Alternatively, use `helicsFederateFinalize()`.
3. **Federation termination detection**: Check `if granted >= helics.HELICS_TIME_MAXTIME: break` after every `request_time()` call. Break from the run loop immediately rather than processing remaining timesteps. Log a WARNING that the federation terminated.
4. **Iterative mode documentation**: Add a note to the `HELICSDwelling` and `HELICSFleet` class docstrings stating: "This federate uses non-iterative (`request_time`) time requests; it does not participate in iteration convergence and is single-pass only."
5. **Inter-federate timestep validation**: Log the derived `period_s` at INFO level during `__init__`. Document in the docstring that all federates must use the same time period for correct data exchange.
6. **Time-sync logging**: Add `_LOG.debug("HELICS request_time(%.1f) granted %.1f", requested, granted)` after each grant. Add `_LOG.info("HELICS federate %s entered executing mode with period=%.1fs", self._fed_name, self._period_s)` after `enter_executing_mode()`. Add `_LOG.info("HELICS federate %s finalizing", self._fed_name)` in `finalize()`.
7. **Default period**: Change the single-timestep fallback from `period_s = 1.0` to raising a `ValueError` with a message like "Cannot infer HELICS period from a single timestep; provide a dwelling with at least two timesteps." If a default is preferred, raise it to at least 60.0s and log the severity.

## References / Citations
- HELICS User Guide: Federate Time Management (https://docs.helics.org/en/latest/user-guide/fundamental_topics/timing.html)
- HELICS API Reference: `helicsFederateRequestTime` — returns `HELICS_TIME_MAXTIME` on federation termination
- HELICS API Reference: `helicsFederateFinalize` — signals federate completion
- HELICS API Reference: `HELICS_FLAG_TERMINATE_ON_ERROR` — federate flag for error-driven federation termination
- HELICS API Reference: `HELICS_PROPERTY_TIME_PERIOD` — advisory period, does not enforce grant alignment
