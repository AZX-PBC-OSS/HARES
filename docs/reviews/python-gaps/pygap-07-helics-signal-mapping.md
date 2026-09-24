# HELICS signal mapping: bidirectional HARES ControlSignal ↔ HELICS signal, unit conversion at boundary
**Review ID**: pygap-07
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/helics/_types.py` (23 lines, Protocol definitions)
- `python/ochre_next/helics/dwelling.py` (298 lines, single-dwelling HELICS federate)
- `python/ochre_next/helics/fleet.py` (374 lines, fleet-wide HELICS federate)
- `python/ochre_next/helics/runner.py` (165 lines, runner config helpers)
- `python/ochre_next/_hares.pyi` (1813 lines, Python stubs consulted for ControlSignal + Telemetry)
- `tests/python/test_helics_dwelling.py` (unit tests)
- `tests/python/test_helics_fleet.py` (unit tests)
- `tests/python/test_helics_integration.py` (integration tests)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: No unit conversion at HELICS boundary — raw float casts used throughout
**Severity**: critical
**Description**: All values crossing the HELICS boundary pass through a bare `float()` cast with zero unit conversion, scaling, or validation. The publication keys embed unit hints in their names (e.g. `total_power_kw`, `reactive_power_kvar`) but no conversion logic exists. The incoming voltage subscription has no unit hint in its topic name (`grid/voltage`) yet expects a per-unit value — if an external simulator publishes voltage in volts (e.g. 240), a 240× error silently propagates.
**Code Location**:
- `dwelling.py:153` — `self._dwelling.set_grid_voltage(float(self._sub_voltage.double))` — no range check, no unit assertion
- `dwelling.py:159` — `price = float(self._sub_price.double)` — no range check
- `dwelling.py:193` — `self._pub_power.publish(float(telemetry.total_power_kw))` — assumes field is already in desired units
- `dwelling.py:194` — `self._pub_reactive.publish(float(telemetry.reactive_power_kvar))` — same assumption
- `fleet.py:185` — `self._fleet.set_grid_voltage_all(float(self._sub_voltage_all.double))` — no validation
- `fleet.py:231` — `power_kw = float(telemetry.total_power_kw)` — no conversion
**Root Cause**: The HELICS wrappers treat the boundary as transparent — whatever the HARES engine reports is published directly, and whatever the external federate sends is consumed without inspection. No abstraction layer or conversion table exists between HELICS signal values and HARES internal units.
**Impact**: A power value arriving in kW that is interpreted as W creates a 1000× error. A voltage value in volts (240) instead of per-unit (0.95) creates a 240× error. No unit metadata is attached to HELICS publications (the `helicsPublicationSetInfo` API is unused), so downstream consumers must infer units from the publication key string alone, which is error-prone.

### Finding 2: Missing critical telemetry publications — only total power and reactive power exposed
**Severity**: high
**Description**: The HELICS publication surface is limited to `total_power_kw` and `reactive_power_kvar` (plus fleet aggregate variants). None of the following telemetry data — all available via the `Dwelling.telemetry()` API — is published to HELICS:
- Zone temperatures (indoor air, mean radiant)
- Zone humidity
- Equipment State of Charge (battery, EV)
- Per-equipment power (kW electric per load)
- PV generation output
- Equipment operating mode (on/off, heating, cooling, idle)
- Equipment status

An external simulator that needs to make decisions based on zone temperature or battery SOC has no HELICS signal to subscribe to.
**Code Location**:
- `dwelling.py:191-194` (`_publish_results`) — publishes exactly two scalar values
- `fleet.py:226-242` (`_publish_results`) — publishes exactly aggregate + per-dwelling power/reactive
- `_hares.pyi:823-836` (Telemetry class) — defines `zone()`, `equipment()`, `total_power_kw`, `reactive_power_kvar` but only the latter two are surfaced
- `_hares.pyi:838-853` (CoreOutput) — defines `electric_kw`, `soc`, `operating_mode`, `reactive_power_kvar`, `fuel_w` — none published individually
**Root Cause**: The HELICS wrapper is designed for grid-integration co-simulation (power flow) only, not for building-level or equipment-level interaction.
**Impact**: External controllers cannot implement closed-loop HVAC control, battery dispatch based on SOC feedback, or demand response based on zone conditions. Co-simulation scenarios beyond simple power exchange require extensions to the publication surface.

### Finding 3: No range or type validation on incoming subscription values
**Severity**: high
**Description**: Incoming HELICS subscription values are consumed without sanity checks. Specifically:
- Grid voltage (`set_grid_voltage`) expects per-unit (~0.8–1.2) but accepts any `float`
- Price signal accepts any `float` with no range check
- Control JSON payload is parsed and iterated, but only structural JSON validation is performed — signal-specific value ranges (e.g. `kw` in `power_setpoint`, `heat_c` in `thermal_setpoint`, `on_fraction` in `duty_cycle`) are not checked before forwarding to `ControlSignal.from_dict()`
**Code Location**:
- `dwelling.py:151-155` — voltage subscription: no range check between reading `double` and calling `set_grid_voltage`
- `dwelling.py:157-161` — price subscription: no range check
- `dwelling.py:167-189` — control subscription: validates JSON structural shape only, defers all value-level validation to `ControlSignal.from_dict()` and `Dwelling.apply_control()`
- `fleet.py:183-194` — fleet voltage subscriptions: same missing range checks
**Root Cause**: The `_read_subscriptions()` method performs only structural validation (JSON parseability, dict shape). Value-level semantics (valid ranges, unit consistency) are delegated entirely to downstream methods without guardrails at the boundary.
**Impact**: An out-of-range voltage (e.g. 0.0 or 240 pu) or a negative price can produce physically invalid simulation results without any diagnostic warning. A `thermal_setpoint` control with `heat_c = 500` (Celsius) would propagate through to the dwelling model.

### Finding 4: Signal naming convention is implicit, undocumented, and inconsistent between dwelling and fleet
**Severity**: medium
**Description**: The HELICS hierarchical signal naming convention is embedded in code only and not documented. Specific issues:
- Dwelling publications: `{prefix}{fed_name}/total_power_kw`, `{prefix}{fed_name}/reactive_power_kvar`
- Fleet publications: `{prefix}{fed_name}/aggregate_power_kw`, `{prefix}{fed_name}/dwelling_{index}/total_power_kw`
- The fleet uses opaque numeric indices (`dwelling_0`, `dwelling_1`) instead of building/ID names, making the topic opaque to external simulators
- Subscription topic names are caller-provided (`voltage_topic`, `control_topic`, `price_topic`) with no naming convention enforced — the external federate must independently know the correct topic strings
- No documentation describes the expected `{scope}/{entity}/{metric}` hierarchy
**Code Location**:
- `dwelling.py:87-101` (`register_publications`) — constructs publication keys from prefix + fed_name
- `fleet.py:75-100` (`register_publications`) — constructs fleet + per-dwelling publication keys
- `fleet.py:102-144` (`register_subscriptions`) — accepts arbitrary topic strings
**Root Cause**: The publication keys are auto-derived from internal parameters; subscription keys are externally provided. The union of these two naming paths has no documented contract.
**Impact**: External simulator operators must reverse-engineer HARES signal names from source code. Naming mismatches cause silent subscription misses (see Finding 5).

### Finding 5: No missing-signal detection — unmatched subscriptions and unregistered publications are silent
**Severity**: medium
**Description**: If an external simulator subscribes to a HARES publication that does not exist, HELICS returns a default value (typically 0.0). HARES publishes no registration-completeness summary, and no warning is logged if a publication key was expected but never registered. Conversely, if a HARES subscription topic matches no external publication, `is_updated()` returns `False` on every timestep and the subscription is silently idle — no "subscription never received data" diagnostic is emitted.
**Code Location**:
- `dwelling.py:87-101` — `register_publications()` returns config metadata but never cross-checks against a manifest of required signals
- `dwelling.py:103-125` — `register_subscriptions()` conditionally registers based on optional parameters; no check that every optional key will be published by another federate
- `dwelling.py:150-165` — `_read_subscriptions()` checks `is_updated()` but never tracks how many timesteps have passed since the last update
- `fleet.py:182-194` — same pattern in fleet
**Root Cause**: The HELICS wrappers treat publication/subscription registration as configuration-driven with no validation of the complete co-simulation signal surface.
**Impact**: An external simulator subscribing to a misspelled topic name silently receives 0 for all HARES telemetry, potentially producing wrong results for thousands of timesteps. Conversely, a HARES voltage subscription that never receives data silently runs with no voltage override.

### Finding 6: Boolean, integer, and complex HELICS types not leveraged for appropriate signals
**Severity**: low
**Description**: All non-control HARES → HELICS publications use type `"double"`. HELICS natively supports `bool`, `int`, `string`, `complex`, and `vector` types, but none of these are used. Specific cases:
- Equipment on/off status (boolean) is not published at all
- `timestep_index` (integer) is not published
- `grid_connect(connected: bool)` and `self_consumption(enabled: bool)` ControlSignals — which have boolean semantics — are serialized as JSON booleans within a wrapped string, not as native HELICS booleans
- The `_types.py` protocol (`HelicsPublicationLike`) only exposes `publish(value: float)`, precluding non-double publications from the type system
**Code Location**:
- `_types.py:15-16` — `HelicsPublicationLike` protocol bounds publications to `float` only
- `_types.py:19-23` — `HelicsSubscriptionLike` protocol exposes `double` and `string` only
- `dwelling.py:91-101` — all publication registrations use `"double"`
- `fleet.py:75-100` — same
**Root Cause**: The protocol wrapper intentionally restricts to `double` and `string` for simplicity.
**Impact**: Low. The JSON-string control channel is flexible enough to carry boolean and integer data. Native HELICS types would provide tighter semantic coupling and potentially better performance for large federations, but the current design is workable.

### Finding 7: No end-to-end closed-loop feedback test (thermostat → temperature → setpoint)
**Severity**: low
**Description**: The integration tests exercise individual signal directions in isolation:
- `test_single_dwelling_cosim_with_mock_aggregator` — voltage → power (one direction)
- `test_control_signal_via_helics` — injects a single control signal at step 1 without reading published state
No test implements a full closed-loop controller (e.g., an external thermostat federate that subscribes to temperature, computes a setpoint, and publishes it back). The review instructions specifically call for testing "a thermostat that reads temperature and sets a heating setpoint" — this test does not exist because zone temperature is not published (see Finding 2).
**Code Location**:
- `tests/python/test_helics_integration.py:355-406` (`test_control_signal_via_helics`) — one-shot open-loop control injection
- Missing: a test with a feedback loop that reads HARES telemetry, computes a decision, and publishes a control signal based on that decision
**Root Cause**: Zone temperature is not a published HELICS signal, making a thermostat feedback test impossible with the current publication surface.
**Impact**: The bidirectional signal mapping is not behaviorally validated. Subtle issues like timing races between request/grant and `is_updated()` semantics could cause stale-data bugs that unit tests miss.

## Summary
- **Total findings**: 7
- **Critical**: 1 (unit conversion)
- **High**: 2 (missing telemetry publications, no range validation)
- **Medium**: 2 (undocumented naming convention, no missing-signal detection)
- **Low**: 2 (type under-utilization, no closed-loop feedback test)

## Recommendations
1. Add an explicit unit-conversion layer at the HELICS boundary. At minimum, use `assert` or warning guards on incoming voltage (0.5 ≤ pu ≤ 1.5), price (≥ 0), and document the expected units for each publication key. Consider attaching unit metadata via HELICS `setInfo()` on publications.
2. Extend the publication surface to include zone-level and equipment-level telemetry: zone temperatures, humidity, battery SOC, EV SOC, PV output, per-equipment electric power, and equipment operating modes. Register these as named HELICS publications so external simulators can subscribe to them.
3. Add range validation on all incoming subscription values before forwarding to the dwelling/fleet model. Log warnings when values are outside expected physical ranges.
4. Document the HELICS signal naming convention (hierarchy, expected units, data types) in a `docs/` file or in the module docstring so external simulator developers can integrate without reverse-engineering source code.
5. Log a warning if a subscription has gone N consecutive timesteps without receiving data (detect "never-published" subscription topics). Log the full list of registered publications on federate entry so the operator can verify completeness.
6. Consider publishing `timestep_index` as an integer-type HELICS signal and equipment status as boolean signals to leverage HELICS native type support.
7. Add an end-to-end closed-loop test involving a mock external federate that reads published HARES state, computes a control decision, and publishes it back — once the missing publications from Finding 2 are addressed.

## References / Citations
- HELICS publication metadata API: `helicsPublicationSetInfo()` / `helicsInputSetInfo()` — unused in this codebase
- HELICS supported data types: `helics_data_type_*` constants (double, int, string, bool, complex, vector) — only double and string used here
- `python/ochre_next/helics/dwelling.py:150-189` — `_read_subscriptions()` (subscription consumption path)
- `python/ochre_next/helics/dwelling.py:191-194` — `_publish_results()` (publication path)
- `python/ochre_next/helics/fleet.py:226-243` — `_publish_results()` (fleet publication path)
- `python/ochre_next/_hares.pyi:567-651` — `ControlSignal` (20+ signal constructors)
- `python/ochre_next/_hares.pyi:823-836` — `Telemetry` (available but unpublishhed fields)
- `python/ochre_next/_hares.pyi:1730` — `Dwelling.set_grid_voltage(voltage_pu: float)` (per-unit expectation)
