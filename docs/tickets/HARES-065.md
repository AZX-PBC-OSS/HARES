---
id: HARES-065
title: "HELICS Co-Simulation Orchestration"
kind: implement
depends_on: [HARES-049, HARES-047]
files_to_touch:
  - python/ochre_next/cosim/__init__.py
  - python/ochre_next/cosim/helics.py
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - uv run pytest tests/python/ -v -k "helics"
---

## Background/Context
HELICS co-simulation enables ochre_next dwellings to participate in grid-scale studies. The Python orchestration layer manages the HELICS federate lifecycle while the Rust engine handles physics. Both single-dwelling and fleet-as-single-federate patterns are required. The `Fleet::step()` method (per-timestep parallel advance) is needed for fleet HELICS but absent from HARES-047 which only defines `Fleet::simulate()`.

## Work to Do
- [ ] Implement `HELICSDwelling` class wrapping `PyDwelling`:
  - [ ] Federate registration with configurable publications/subscriptions
  - [ ] `set_grid_voltage()` from subscribed bus voltage
  - [ ] `ControlSignal.from_dict()` for JSON payloads from HELICS (depends on HARES-049)
  - [ ] pub/sub lifecycle management
  - [ ] Step loop order per architecture (03-control-interfaces.md): `helicsFederateRequestTime` BEFORE `step()`, not after — the federate must request the granted time before advancing the dwelling
    ```python
    for t in dwelling.timesteps():
        granted_time = h.helicsFederateRequestTime(fed, t)
        voltage = h.helicsInputGetDouble(sub_voltage)
        dwelling.set_grid_voltage(voltage)
        dwelling.step()
        h.helicsPublicationPublishDouble(pub_power, dwelling.telemetry().total_electric_kw)
    ```
  - [ ] HELICS is an optional dependency: `ImportError` guard with descriptive install message
- [ ] Implement `HELICSFleet` class wrapping `PyFleet`:
  - [ ] Fleet-as-single-federate pattern
  - [ ] Requires `Fleet::step()` (per-timestep parallel advance) — **must be added to HARES-047's scope** (HARES-047 currently only defines `Fleet::simulate()`)
- [ ] HELICS orchestration helpers (broker setup, time management)

## Measures of Success
- [ ] Single-dwelling round-trip: pub/sub with mock aggregator passes (publish `total_electric_kw`, subscribe voltage, step for 10 timesteps)
- [ ] Fleet HELICS: N dwellings advance one timestep in parallel via Fleet::step()
- [ ] Grid voltage subscription updates dwelling's voltage state (verify electrical output changes with voltage via ZIP model)
- [ ] Without HELICS installed, `import ochre_next.cosim.helics` raises `ImportError` with install instructions
