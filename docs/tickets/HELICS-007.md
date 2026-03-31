---
id: HELICS-007
title: "End-to-end HELICS integration test with mock aggregator"
kind: implement
depends_on: [HELICS-004, HELICS-005, HELICS-006]
files_to_touch:
  - tests/python/test_helics_integration.py
references:
  - docs/tickets/HARES-065.md
verification:
  - uv run pytest tests/python/test_helics_integration.py -v
---

## Background/Context
The HELICS integration needs an end-to-end test that verifies the full co-simulation lifecycle: broker startup, federate registration, time synchronization, data exchange, and clean shutdown. This test uses a mock aggregator federate (pure Python) alongside real HARES dwellings to validate the integration without requiring GridLAB-D.

This test is the final validation gate — if it passes, HARES is ready for GridLAB-D co-simulation.

## Work to Do
- [ ] Create `tests/python/test_helics_integration.py`
- [ ] Mark all tests with `@pytest.mark.skipif(not helics_available, reason="helics not installed")` where `helics_available` checks for the optional dependency
- [ ] All tests must use ephemeral broker ports (via `create_broker(port=None)`) to avoid port collisions under parallel pytest (`-n auto`)
- [ ] Test: `test_single_dwelling_cosim_with_mock_aggregator`
  - Start a HELICS broker with 2 federates on an ephemeral port
  - Run `HELICSDwelling` in a thread
  - Run a mock aggregator in the main thread that:
    - Subscribes to dwelling power publication
    - Publishes voltage (e.g., 0.95 pu) on the voltage topic
    - Steps for 10 timesteps
  - Verify: dwelling publishes non-zero `total_power_kw`
  - Verify: voltage subscription causes dwelling's ZIP model to adjust load
  - Verify: both federates complete without error
- [ ] Test: `test_fleet_cosim_with_mock_aggregator`
  - 3-dwelling fleet via `HELICSFleet`
  - Mock aggregator subscribes to aggregate power
  - Verify aggregate power is sum of individual dwelling powers
  - 10 timesteps
- [ ] Test: `test_control_signal_via_helics`
  - Aggregator publishes a control signal as JSON string with equipment routing: `{"equipment": "Battery", "signal": {"type": "PowerSetpoint", "active_power_kw": 3.0}}`
  - Dwelling receives, parses equipment name, applies `ControlSignal.from_dict(signal_body)` via `dwelling.apply_control(equipment_name, signal)`
  - Verify equipment responds to control in next step's telemetry
- [ ] Test: `test_helics_time_domain_is_simulation_relative`
  - Verify that `helicsFederateRequestTime` is called with simulation-relative seconds (0.0, 60.0, ...) not Unix epoch timestamps (~1.7 billion). Instrument via mock and assert the full sequence `[0, dt, 2*dt, ...]` across all steps — not just the first call. Add upper-bound guard: no requested time may exceed `total_steps * time_res_s` (catches epoch-scale values).
- [ ] Test: `test_federate_cleanup_on_exception`
  - Inject an exception during step loop (e.g., dwelling step raises). Verify `finalize()` is still called on the federate (no leaked federates that stall broker shutdown).
- [ ] Test: `test_malformed_control_payload_does_not_crash_federate`
  - Aggregator publishes malformed JSON (e.g., `"not json"`, `{"unknown_equipment": {...}}`, `{"equipment": "Battery", "signal": {"type": "InvalidType"}}`)
  - Verify dwelling federate logs a warning and continues stepping — does NOT raise or terminate
- [ ] Test: `test_fleet_invalid_dwelling_index`
  - Attempt `set_grid_voltage(dwelling_index=999, ...)` on a 3-dwelling fleet
  - Verify clean error (IndexError or similar), not a panic
- [ ] Test: `test_helics_import_guard`
  - With helics not importable, verify `from ochre_next.helics import HELICSDwelling` raises `ImportError` with install instructions

## Files to Touch
- `tests/python/test_helics_integration.py`: new file — end-to-end integration tests

## Measures of Success
- [ ] Single-dwelling co-sim completes 10 timesteps with data exchange verified
- [ ] Fleet co-sim with 3 dwellings produces correct aggregate power
- [ ] Control signals transmitted via HELICS are applied to equipment
- [ ] Grid voltage override via HELICS subscription is reflected in electrical output (ZIP correction)
- [ ] Import guard test passes on systems without HELICS installed
- [ ] All tests are deterministic — no RNG dependence, fixed weather/schedule data

## Verification
- [ ] `uv run pytest tests/python/test_helics_integration.py -v` passes (when helics is installed)
- [ ] Tests skip cleanly when helics is not installed
