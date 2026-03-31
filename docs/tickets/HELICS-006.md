---
id: HELICS-006
title: "HELICS broker helpers and GridLAB-D runner config"
kind: implement
depends_on: [HELICS-004, HELICS-005]
files_to_touch:
  - python/ochre_next/helics/broker.py
  - python/ochre_next/helics/runner.py
  - python/ochre_next/helics/orchestrator.py
  - python/ochre_next/helics/__init__.py
references:
  - docs/tickets/HARES-065.md
  - https://docs.helics.org/en/latest/user-guide/examples/fundamental_examples/fundamental_fedintegration.html
verification:
  - uv run pytest tests/python/ -v -k "helics"
---

## Background/Context
Running a HELICS co-simulation requires configuring a broker, generating federate runner configs, and managing the lifecycle of multiple processes. HARES provides programmatic helpers for broker creation and cosim config generation, using the HELICS 3.6+ Python OOP API directly.

## Work to Do
- [ ] Create `python/ochre_next/helics/broker.py`:
  - [ ] `create_broker(n_federates: int, core_type: str = "zmq", port: int | None = None) -> helics.HelicsBroker`: creates and starts a HELICS broker. When `port` is `None`, allocate an ephemeral free port via `socket.bind(('', 0))` to avoid collisions under parallel test execution. Return the broker handle.
  - [ ] `wait_for_broker(broker: helics.HelicsBroker, timeout: float = 60.0)`: blocks until broker is connected or timeout
  - [ ] `get_broker_port(broker: helics.HelicsBroker) -> int`: returns the port the broker is listening on (needed by federate constructors)
- [ ] Create `python/ochre_next/helics/runner.py`:
  - [ ] Define `FederateConfig` dataclass: `name: str`, `host: str = "localhost"`, `directory: str = "."`, `exec_command: str`. Typed — not a plain dict.
  - [ ] `generate_cosim_config(federates: list[FederateConfig], broker: bool = True) -> dict`: generates a HELICS runner JSON config. Uses `FederateConfig` for type safety instead of untyped dicts.
  - [ ] `run_cosimulation(config: dict | Path)`: wraps `helics.cli.run()` — accepts either a dict (writes temp JSON) or path to existing config file
  - [ ] `make_dwelling_federate_config(name: str, hpxml_path: Path, weather_path: Path, schedule_path: Path, **kwargs) -> FederateConfig`: generates a single dwelling federate entry with `Path` types for file arguments
- [ ] Remove the empty stub `python/ochre_next/helics/orchestrator.py` — its intended functionality is now split across `dwelling.py`, `fleet.py`, `broker.py`, and `runner.py`
- [ ] Update `__init__.py` to export broker and runner helpers via `__all__`
- [ ] All code must use full type annotations (PEP 604 union syntax, `from __future__ import annotations`)
- [ ] Documentation: each function includes a docstring with a usage example showing GridLAB-D integration pattern
- [ ] Test: `generate_cosim_config()` produces valid JSON structure with correct federate count; `create_broker` can be instantiated and shut down cleanly (mock test)

## Files to Touch
- `python/ochre_next/helics/broker.py`: new file — broker creation and management
- `python/ochre_next/helics/runner.py`: new file — cosim config generation and runner
- `python/ochre_next/helics/orchestrator.py`: delete — empty stub superseded by new modules
- `python/ochre_next/helics/__init__.py`: export new helpers

## Measures of Success
- [ ] `generate_cosim_config()` produces a valid HELICS runner config JSON
- [ ] `create_broker(n_federates=3)` starts a broker that accepts 3 federates
- [ ] Config structure matches HELICS CLI runner expectations (`name`, `broker`, `federates` keys)
- [ ] Example in docstring shows GridLAB-D as one federate alongside HARES dwellings

## Verification
- [ ] `uv run pytest tests/python/ -v -k "helics"` passes
