---
id: HARES-058
title: "PythonEquipment Adapter"
kind: implement
depends_on: [HARES-018, HARES-049]
files_to_touch:
  - crates/hares-equipment/src/python_equipment.rs
  - crates/hares-python/src/py_equipment.rs
  - python/ochre_next/equipment.py
references:
  - docs/architecture/05-external-tools.md
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo test -p hares-equipment
  - uv run pytest tests/python/ -v -k "python_equipment"
---

> **SUPERSEDED by [HARES-066](HARES-066.md).** This ticket is a duplicate. HARES-066 is the canonical ticket for the PythonEquipment adapter; it incorporates the save_state/load_state requirements from this ticket. Do not implement this ticket.

## Background/Context
The architecture describes `PythonEquipment` as "the most common extension point" — a base class that allows custom Python equipment to plug into the Rust simulation engine via PyO3 callback. This enables researchers to prototype new equipment types in Python before porting to Rust. The adapter bridges the `Equipment` trait with a Python class that implements `step()`, `apply_control()`, etc.

## Work to Do
- [ ] Implement `python_equipment.rs` in `hares-equipment`:
  - [ ] `PythonEquipmentAdapter` struct implementing `Equipment` trait
  - [ ] Holds a `PyObject` reference to the Python equipment instance
  - [ ] `step()`: acquires GIL, calls Python `.step(env_dict)`, converts returned dict to `Vec<PortContribution>`
  - [ ] `apply_control()`: acquires GIL, calls Python `.apply_control(signal_dict)`
  - [ ] `telemetry()`: acquires GIL, calls Python `.telemetry()` → dict → Telemetry
  - [ ] `save_state()` / `load_state()`: delegates to Python pickle or custom serialization
  - [ ] `descriptor()`: reads from Python class attributes (name, equipment_type, stage, etc.)
- [ ] Implement `py_equipment.rs` in `hares-python`:
  - [ ] `#[pyclass] PythonEquipment` base class with default method implementations
  - [ ] Users subclass in Python and override `step()`, `apply_control()`, etc.
- [ ] Implement `python/ochre_next/equipment.py`:
  - [ ] `PythonEquipment` Python base class with documented interface
  - [ ] Example: `SimpleBattery(PythonEquipment)` showing minimal implementation
- [ ] Register `PythonEquipment` adapter in `EquipmentRegistry` with factory that accepts a Python class

## Measures of Success
- [ ] A Python subclass of `PythonEquipment` with a custom `step()` runs correctly in a dwelling simulation
- [ ] Port contributions from Python equipment appear in the electrical/thermal accumulators
- [ ] `apply_control()` from Python reaches the custom equipment
- [ ] `save_state()` / `load_state()` round-trip preserves Python equipment state
- [ ] Performance: GIL acquisition per step is <100μs (acceptable for prototype equipment)

## Verification
- [ ] `cargo test -p hares-equipment` passes
- [ ] `uv run pytest tests/python/ -v -k "python_equipment"` passes
