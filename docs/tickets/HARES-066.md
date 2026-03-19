---
id: HARES-066
title: "Python Equipment Adapter (PythonEquipment)"
kind: implement
depends_on: [HARES-049, HARES-018]
files_to_touch:
  - python/ochre_next/equipment/__init__.py
  - python/ochre_next/equipment/adapter.py
  - crates/hares-equipment/src/python_equipment.rs
  - crates/hares-python/src/py_equipment.rs
references:
  - docs/architecture/05-external-tools.md
  - docs/architecture/02-equipment-and-ports.md
verification:
  - uv run pytest tests/python/ -v -k "equipment_adapter"
---

## Background/Context
The architecture requires custom Python equipment models to plug into the Rust simulation engine without rewriting them in Rust. This enables researchers to prototype equipment models in Python and integrate them with the full simulation.

## Work to Do
- [ ] Define `PythonEquipment` Python base class with methods matching the Rust `Equipment` trait:
  - [ ] `descriptor()`, `ports()`, `init()`, `apply_control()`, `update_control()`, `step()`, `telemetry()`
- [ ] Implement Rust-side `PyEquipmentAdapter` that wraps a Python object implementing `PythonEquipment` and presents it as `Box<dyn Equipment>`
- [ ] GIL acquisition strategy: acquire GIL only during the Python equipment's `step()` call; release for all Rust equipment
- [ ] `save_state()` / `load_state()`: delegates to Python pickle or custom serialization for checkpoint/restore of Python equipment state
- [ ] Registration: `EquipmentRegistry.register_python("CustomName", python_class)`

## Measures of Success
- [ ] A Python-defined equipment model runs inside a Dwelling simulation and produces correct port contributions
- [ ] Multiple Python equipment instances in one dwelling work correctly
- [ ] Python equipment step() is called with correct EnvironmentState values
- [ ] `save_state()` / `load_state()` round-trip preserves Python equipment state
- [ ] Performance: GIL acquisition per step is ~1-5μs (per architecture target)
