---
id: HARES-069
title: "hares-python — Custom Rust Equipment Registration"
kind: implement
depends_on: [HARES-018, HARES-049]
phase: 5
crate: hares-python
files_to_touch:
  - crates/hares-python/src/registry.rs
  - crates/hares-python/src/lib.rs
  - tests/python/test_custom_equipment.py
references:
  - docs/architecture/05-external-tools.md
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo test -p hares-python
  - uv run pytest tests/python/ -v -k "custom_equipment"
---

## Background/Context

The architecture (`05-external-tools.md`) describes registering custom Rust equipment types via `registry.register("Custom Name", factory_fn)`. HARES-018 implements the Rust `EquipmentRegistry`; this ticket exposes it to Python callers via PyO3. This enables third-party Rust equipment crates to register at Python import time, making custom equipment types available to any ochre_next workflow without patching the core library.

## Work to Do

- [ ] Expose `EquipmentRegistry::register(name, factory)` via PyO3 in `crates/hares-python/src/registry.rs`:
  - [ ] `#[pyclass] PyEquipmentRegistry` wrapping `EquipmentRegistry`
  - [ ] `#[pymethods] fn register(name: &str, factory: PyObject)` — accepts a Python callable that acts as a factory
  - [ ] `#[pymethods] fn create(name: &str, config: PyObject) -> PyResult<PyObject>` — creates an equipment instance by name
  - [ ] Re-export `PyEquipmentRegistry` from `crates/hares-python/src/lib.rs`
- [ ] Document the pattern for loading a custom Rust equipment crate as a Python extension:
  - [ ] The custom crate exports a `#[pymodule]` that calls `registry.register(...)` at import time
  - [ ] Document in `docs/architecture/05-external-tools.md` (or a new `docs/CUSTOM_EQUIPMENT.md`) with a minimal example
- [ ] Add integration test `tests/python/test_custom_equipment.py`:
  - [ ] Register a `MockEquipment` implemented in Python (via `PythonEquipment` from HARES-066)
  - [ ] Construct a dwelling using the registered custom equipment
  - [ ] Run 1 timestep and assert the custom equipment's port contributions appear in output
  - [ ] Assert custom equipment telemetry appears in `generate_results()` output

## Measures of Success

- [ ] `registry.register("GSHP", GroundSourceHeatPump.factory())` does not raise
- [ ] Dwelling constructed with registered custom equipment runs 1 timestep without error
- [ ] Custom equipment telemetry keys appear in dwelling output
- [ ] `registry.create("Unknown")` raises `KeyError` with a clear message naming the unknown equipment type

## Verification

- [ ] `cargo test -p hares-python` passes
- [ ] `cargo clippy -p hares-python -- -D warnings` passes
- [ ] `uv run pytest tests/python/ -v -k "custom_equipment"` passes
