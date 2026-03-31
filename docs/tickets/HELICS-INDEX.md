# HELICS Co-Simulation Integration Tickets

## Design Principles
- **No OCHRE legacy.** OCHRE's `run_cosimulation.py` is a reference for *what* HELICS integration looks like, not *how* to implement it. Do not copy its patterns (string-typed publications, module globals, C-style function API, `time.sleep` polling).
- **HELICS 3.6+ OOP API.** Use `HelicsValueFederate`, `HelicsFederateInfo`, `.request_time()`, `.register_publication()`, `.disconnect()` — not the C-style `helicsFederateRequestTime()` functions.
- **Typed publications.** Power and voltage are `double` publications. Only control signals use `string` type (JSON). No serializing numeric values as JSON strings.
- **Strong Python typing.** All Python code uses `from __future__ import annotations`, PEP 604 unions, typed dataclasses, `Path` for file arguments. No `dict` where a dataclass belongs.
- **Rust-first design.** `SteppableFleet` owns `Vec<Dwelling>` directly at the Rust level. The `Mutex` wrapper exists only at the Python boundary (`PyDwelling`). No `Arc<Mutex>` in the fleet hot path.

## Dependency Graph

```
HELICS-001 (telemetry reactive_kvar)  ──┐
                                        ├── HELICS-004 (HELICSDwelling) ──┐
                                        │                                 │
HELICS-002 (Fleet::step Rust) ──────────┤                                 ├── HELICS-006 (broker/runner helpers)
    │                                   │                                 │         │
    └── HELICS-003 (PySteppableFleet) ──┘── HELICS-005 (HELICSFleet) ────┘         │
                                                                                    │
                                                                          HELICS-007 (e2e integration test)
```

## Parallelism

**Wave 1** (independent — run in parallel):
- HELICS-001: Add `reactive_power_kvar` to `DwellingTelemetry`
- HELICS-002: Add `SteppableFleet` to Rust fleet crate

**Wave 2** (depends on Wave 1):
- HELICS-003: Python bindings for `SteppableFleet` (depends on HELICS-002)
- HELICS-004: `HELICSDwelling` orchestrator (depends on HELICS-001)

**Wave 3** (depends on Wave 2):
- HELICS-005: `HELICSFleet` orchestrator (depends on HELICS-003, HELICS-004)

**Wave 4** (sequential — HELICS-007 depends on HELICS-006):
- HELICS-006: Broker helpers and runner config (depends on HELICS-004, HELICS-005)
- HELICS-007: End-to-end integration tests (depends on HELICS-006) — must run after HELICS-006

## Ticket Summary

| ID | Title | Kind | Files | Depends On |
|----|-------|------|-------|------------|
| HELICS-001 | Add `reactive_power_kvar` to `DwellingTelemetry` | implement | telemetry.rs, dwelling/mod.rs, py_telemetry.rs | — |
| HELICS-002 | Add `SteppableFleet` to Rust fleet crate | implement | fleet.rs, lib.rs | — |
| HELICS-003 | Python bindings for `SteppableFleet` | implement | py_fleet.rs, lib.rs | HELICS-002 |
| HELICS-004 | `HELICSDwelling` single-dwelling co-sim orchestrator | implement | helics/dwelling.py, helics/__init__.py | HELICS-001 |
| HELICS-005 | `HELICSFleet` fleet-as-single-federate orchestrator | implement | helics/fleet.py, helics/__init__.py | HELICS-003, HELICS-004 |
| HELICS-006 | Broker helpers and GridLAB-D runner config | implement | helics/broker.py, helics/runner.py, helics/__init__.py | HELICS-004, HELICS-005 |
| HELICS-007 | End-to-end integration test with mock aggregator | implement | tests/python/test_helics_integration.py | HELICS-004, HELICS-005, HELICS-006 |

## Superseded Tickets
- **HARES-059**: Superseded by HARES-065 (noted in ticket). Both are now superseded by this decomposition.
- **HARES-065**: Requirements incorporated into HELICS-001 through HELICS-007.
