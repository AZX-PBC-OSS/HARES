---
id: HELICS-005
title: "Implement HELICSFleet fleet-as-single-federate co-sim orchestrator"
kind: implement
depends_on: [HELICS-003, HELICS-004]
files_to_touch:
  - python/ochre_next/helics/fleet.py
  - python/ochre_next/helics/__init__.py
references:
  - docs/tickets/HARES-065.md
  - https://docs.helics.org/en/latest/user-guide/examples/fundamental_examples/fundamental_fedintegration.html
verification:
  - uv run pytest tests/python/ -v -k "helics_fleet"
---

## Background/Context
`HELICSFleet` implements the fleet-as-single-federate pattern: a single HELICS federate manages N dwellings, advancing all of them one timestep at each HELICS granted time. This pattern is efficient for large-scale grid studies where each dwelling doesn't need its own federate. It depends on `SteppableFleet` (HELICS-002/003) for parallel per-timestep stepping.

The fleet-as-single-federate pattern publishes aggregate metrics (total power, reactive power) and subscribes to fleet-wide or per-dwelling controls. Use the HELICS 3.6+ Python OOP API with typed double publications — no JSON serialization for numeric values.

## Work to Do
- [ ] Create `python/ochre_next/helics/fleet.py` with `HELICSFleet` class
- [ ] Constructor: `HELICSFleet(fleet: PySteppableFleet, fed_name: str, broker_address: str = "localhost", core_type: str = "zmq")`
  - Creates `HelicsFederateInfo`, sets core type and `core_init_string = f"--broker={broker_address}"`
  - Creates `HelicsValueFederate` via HELICS OOP API
  - Reads `time_res_s` and `total_steps` from `fleet.time_res_s()` and `fleet.total_steps()` (exposed in HELICS-002/003) for deterministic time progression
- [ ] `register_publications(self, prefix: str = "")` — registers:
  - `{prefix}{fed_name}/aggregate_power_kw` (double) — sum of all dwelling power
  - `{prefix}{fed_name}/aggregate_reactive_kvar` (double)
  - Optionally per-dwelling: `{prefix}{fed_name}/dwelling_{i}/total_power_kw` for each dwelling
- [ ] `register_subscriptions(self, voltage_topic: str = None, control_topic: str = None)` — fleet-wide or per-dwelling subscriptions:
  - Fleet-wide voltage: applies same voltage to all dwellings via `set_grid_voltage_all()`
  - Per-dwelling voltage: `{topic}/dwelling_{i}` pattern
  - Control: JSON-encoded dict keyed by dwelling index or equipment name. Wrap control decode/apply in `try/except` with `logging.warning()` + continue — malformed payloads must not crash the federate loop.
- [ ] `run(self)` — co-simulation loop. **CRITICAL**: HELICS time is simulation-relative seconds, not epoch timestamps. `next_time` advances deterministically by `time_res_s` each iteration. Wrap in `try/finally` to guarantee federate cleanup:
  ```python
  self._fed.enter_executing_mode()
  try:
      sim_time_s = 0.0
      while not self._fleet.is_finished():
          granted = self._fed.request_time(sim_time_s)
          assert granted >= sim_time_s, "granted time must be monotonically increasing"
          self._read_subscriptions()
          self._fleet.step()
          self._publish_results()
          sim_time_s += self._time_res_s
  finally:
      self._fed.disconnect()
  ```
- [ ] `_publish_results(self)` — aggregates telemetry across all dwellings and publishes
- [ ] HELICS import guard (same pattern as HELICS-004)
- [ ] Update `__init__.py` to export `HELICSFleet` via `__all__`
- [ ] All code must use full type annotations (PEP 604 union syntax, `from __future__ import annotations`)
- [ ] Tests with mock: 3-dwelling fleet, verify aggregate power published equals sum of individual dwelling powers

## Files to Touch
- `python/ochre_next/helics/fleet.py`: new file — `HELICSFleet` class
- `python/ochre_next/helics/__init__.py`: export `HELICSFleet`

## Measures of Success
- [ ] `HELICSFleet` steps N dwellings in parallel at each HELICS granted time
- [ ] Aggregate power publication equals sum of per-dwelling `total_power_kw`
- [ ] Per-dwelling voltage subscriptions correctly route to individual dwellings
- [ ] Fleet-wide voltage subscription applies to all dwellings simultaneously
- [ ] Co-simulation loop terminates when `is_finished()` returns `True`

## Verification
- [ ] `uv run pytest tests/python/ -v -k "helics_fleet"` passes
