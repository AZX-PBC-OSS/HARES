---
id: HELICS-004
title: "Implement HELICSDwelling single-dwelling co-sim orchestrator"
kind: implement
depends_on: [HELICS-001]
files_to_touch:
  - python/ochre_next/helics/__init__.py
  - python/ochre_next/helics/dwelling.py
references:
  - docs/tickets/HARES-065.md
  - https://docs.helics.org/en/latest/user-guide/examples/fundamental_examples/fundamental_fedintegration.html
verification:
  - uv run pytest tests/python/ -v -k "helics_dwelling"
---

## Background/Context
`HELICSDwelling` wraps a `PyDwelling` with HELICS federate lifecycle management for single-dwelling co-simulation. This is the primary integration point for tools like GridLAB-D. The step loop must follow the architecture: request time from broker → read subscriptions → update dwelling state → step → publish results.

Use the HELICS 3.6+ Python OOP API (`HelicsValueFederate`, `.register_publication()`, `.request_time()`, etc.) — not the C-style function API. Publications must use typed `double` values directly, not JSON-serialized strings.

## Work to Do
- [ ] Create `python/ochre_next/helics/dwelling.py` with `HELICSDwelling` class
- [ ] Constructor: `HELICSDwelling(dwelling: PyDwelling, fed_name: str, broker_address: str = "localhost", core_type: str = "zmq")`
  - Creates `HelicsFederateInfo`, configures time period, core type, broker address
  - Creates `HelicsValueFederate` via `helics.helicsCreateValueFederate(fed_name, fedinfo)`
  - Sets `HELICS_FLAG_UNINTERRUPTIBLE` and `HELICS_FLAG_TERMINATE_ON_ERROR`
  - Wires `broker_address` into core init string: `fedinfo.core_init_string = f"--broker={broker_address}"` — without this, federates cannot find each other in multi-process co-sim (e.g., GridLAB-D on a remote host)
  - Obtains `start_time` by peeking at the first value from `dwelling.timesteps()` (there is no `dwelling.start_time` property — the clock is internal to Rust). Cache this as `self._start_time` for simulation-relative time conversion in the step loop.
- [ ] Define `HELICSPublicationConfig` and `HELICSSubscriptionConfig` dataclasses for type-safe pub/sub configuration:
  ```python
  @dataclass(frozen=True, slots=True)
  class HELICSPublicationConfig:
      key: str           # e.g., "house_1/total_power_kw"
      type: str = "double"

  @dataclass(frozen=True, slots=True)
  class HELICSSubscriptionConfig:
      key: str           # e.g., "grid/bus_1/voltage_pu"
      type: str = "double"
  ```
- [ ] `register_publications(self, prefix: str = "")` — registers typed double publications:
  - `{prefix}{fed_name}/total_power_kw` (type: double)
  - `{prefix}{fed_name}/reactive_power_kvar` (type: double)
  - Returns `list[HELICSPublicationConfig]` for introspection
- [ ] `register_subscriptions(self, voltage_topic: str | None = None, control_topic: str | None = None, price_topic: str | None = None)` — registers subscriptions:
  - `voltage_topic` → feeds `dwelling.set_grid_voltage()`
  - `control_topic` → JSON payload must include equipment routing: `{"equipment": "Battery", "signal": {"type": "PowerSetpoint", "active_power_kw": 3.0}}` (single) or `{"Battery": {"type": "PowerSetpoint", ...}, "EV": {...}}` (multi). Parse and call `dwelling.apply_control(name, ControlSignal.from_dict(signal_dict))` for each entry.
  - `price_topic` → feeds `dwelling.set_price_signal()`
- [ ] `run(self)` — the co-simulation loop. **CRITICAL**: HELICS time is simulation-relative seconds (0.0, 60.0, 120.0, ...), NOT Unix epoch timestamps. Use `(t - start_time).total_seconds()` to convert. Wrap in `try/finally` to guarantee federate cleanup on exceptions:
  ```python
  self._fed.enter_executing_mode()
  try:
      for t in self._dwelling.timesteps():
          sim_time_s = (t - self._start_time).total_seconds()
          granted = self._fed.request_time(sim_time_s)
          self._read_subscriptions()
          self._dwelling.step()
          self._publish_results()
  finally:
      self._fed.disconnect()
  ```
- [ ] `_read_subscriptions(self)` — reads all registered subscriptions, applies to dwelling:
  - Voltage: `self._sub_voltage.double` → `dwelling.set_grid_voltage()` (typed double, no parsing)
  - Control: `self._sub_control.string` → JSON parse → extract `equipment` name and `signal` body → `ControlSignal.from_dict(signal_body)` → `dwelling.apply_control(equipment_name, signal)`. Supports both single-equipment and multi-equipment payloads (see schema above). Control is the only subscription that uses string type — voltage and power are always typed doubles.
  - Guard with `sub.is_updated()` before reading
  - Wrap control decode/apply in `try/except` with `logging.warning()` + continue semantics — a malformed JSON payload or unknown equipment name must not crash the federate loop. `apply_control()` raises on invalid equipment, so this guard is essential for production co-sim robustness.
- [ ] `_publish_results(self)` — publishes telemetry using typed double publications (no JSON serialization):
  - `self._pub_power.publish(dwelling.telemetry().total_power_kw)`
  - `self._pub_reactive.publish(dwelling.telemetry().reactive_power_kvar)`
- [ ] `finalize(self)` — explicit cleanup: `self._fed.disconnect()` (HELICS 3.x OOP API)
- [ ] HELICS import guard: at module top, `try: import helics` with `ImportError` raising descriptive message: `"HELICS not installed. Install with: pip install 'ochre_next[helics]'"`
- [ ] Update `python/ochre_next/helics/__init__.py` to export `HELICSDwelling` via `__all__`
- [ ] All code must use full type annotations (PEP 604 union syntax, `from __future__ import annotations`)
- [ ] Tests with mock: create a test that patches `helics` module, verifies:
  - Federate is created with correct config
  - Publications are registered
  - Step loop calls `helicsFederateRequestTime` before `dwelling.step()`
  - `helicsPublicationPublishDouble` is called with telemetry values after step
  - Voltage subscription updates `set_grid_voltage()`
  - `helicsFederateRequestTime` receives simulation-relative seconds (not epoch timestamps)
  - Control subscription correctly routes to equipment by name
  - `finalize()` is called even when step loop raises an exception

## Files to Touch
- `python/ochre_next/helics/dwelling.py`: new file — `HELICSDwelling` class
- `python/ochre_next/helics/__init__.py`: export `HELICSDwelling`

## Measures of Success
- [ ] `HELICSDwelling` can be constructed with a `PyDwelling` instance
- [ ] Step loop follows correct HELICS ordering: requestTime → read → step → publish
- [ ] Voltage subscription propagates to dwelling's ZIP model (verifiable via telemetry power change)
- [ ] Control signals via JSON subscription are deserialized and applied
- [ ] Without HELICS installed, importing raises `ImportError` with install instructions
- [ ] Mock test verifies full federate lifecycle (init → execute → step loop → finalize)

## Verification
- [ ] `uv run pytest tests/python/ -v -k "helics_dwelling"` passes
