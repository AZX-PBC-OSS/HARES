---
id: HARES-059
title: "HELICS Co-Simulation Integration"
kind: implement
depends_on: [HARES-049, HARES-043, HARES-047]
files_to_touch:
  - python/ochre_next/cosim/__init__.py
  - python/ochre_next/cosim/helics.py
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - uv run pytest tests/python/ -v -k "helics"
---

> **SUPERSEDED by [HARES-065](HARES-065.md).** This ticket is a duplicate. HARES-065 is the canonical ticket for HELICS co-simulation; it incorporates the requirements from this ticket and correctly identifies the Fleet::step() gap. Do not implement this ticket.

## Background/Context
HELICS co-simulation enables ochre_next dwellings to participate in grid-scale studies. The Python orchestration layer manages the HELICS federate lifecycle while the Rust engine handles the physics. Single-dwelling and fleet-as-single-federate patterns are both required. This is a Phase 3 deliverable per the roadmap.

## Work to Do
- [ ] Implement `python/ochre_next/cosim/helics.py`:
  - [ ] `HELICSDwelling` class wrapping `PyDwelling` with HELICS federate management
  - [ ] Constructor: `HELICSDwelling(dwelling: PyDwelling, fed_name: str, broker: str)`
  - [ ] Register HELICS publications: voltage, power, reactive power per equipment
  - [ ] Register HELICS subscriptions: grid voltage, price signals, control signals
  - [ ] Step loop pattern (order is significant — request time first, then read inputs, step, then publish):
    ```python
    for t in dwelling.timesteps():
        h.helicsFederateRequestTime(fed, t.timestamp())   # 1. Request time first
        if h.helicsInputIsUpdated(sub_voltage):            # 2. Guard before reading input
            dwelling.set_grid_voltage(h.helicsInputGetDouble(sub_voltage))
        dwelling.step()                                    # 3. Step
        h.helicsPublicationPublishDouble(pub_power, dwelling.telemetry().total_electric_kw)  # 4. Publish
    ```
  - [ ] `HELICSFleet` class: single federate controlling N dwellings (fleet-as-federate pattern)
  - [ ] HELICS is an optional dependency: `ImportError` guard with descriptive message
- [ ] Ensure `PyDwelling` exposes required methods (from HARES-049):
  - [ ] `set_grid_voltage(voltage_pu: float)`
  - [ ] `timesteps()` iterator
  - [ ] `initialize()`

## Measures of Success
- [ ] `HELICSDwelling` round-trip with a mock HELICS broker: publish power, subscribe voltage, step for 10 timesteps
- [ ] Grid voltage override propagates to equipment ZIP model (verify electrical output changes with voltage)
- [ ] `HELICSFleet` with 3 dwellings steps all dwellings at each HELICS time request
- [ ] Without HELICS installed, `import ochre_next.cosim.helics` raises `ImportError` with install instructions

## Verification
- [ ] `uv run pytest tests/python/ -v -k "helics"` passes (with mock broker)
