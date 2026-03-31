---
id: HELICS-003
title: "Add PySteppableFleet Python bindings"
kind: implement
depends_on: [HELICS-002]
files_to_touch:
  - crates/hares-python/src/py_fleet.rs
  - crates/hares-python/src/lib.rs
  - python/ochre_next/_hares.pyi
references:
  - docs/tickets/HARES-065.md
  - crates/hares-fleet/src/fleet.rs
verification:
  - cargo check -p hares-python
  - cargo clippy -p hares-python -- -D warnings
  - maturin develop -p hares-python
---

## Background/Context
The Rust `SteppableFleet` (HELICS-002) needs Python bindings so that the HELICS orchestration layer can step a fleet of dwellings from Python. The binding must release the GIL during the parallel Rust step call so Python threads are not blocked.

## Work to Do
- [ ] Add `#[pyclass(name = "SteppableFleet")] PySteppableFleet` to `py_fleet.rs`
- [ ] `from_configs(configs: list[PyDwellingConfig], n_threads: int = 0) -> PySteppableFleet` classmethod
- [ ] `step() -> list[dict]`: calls `SteppableFleet::step()`, releases GIL during Rust computation. Returns list of per-dwelling result dicts with error semantics preserved: `{"ok": True, "result": {...step data...}}` on success, `{"ok": False, "error": "...message...", "bldg_id": N}` on failure. This matches the Rust `Vec<Result<StepResult, SimError>>` contract without silently dropping errors.
- [ ] `set_grid_voltage(dwelling_index: int, voltage_pu: float)`: sets voltage for one dwelling
- [ ] `set_grid_voltage_all(voltage_pu: float)`: sets voltage for all dwellings
- [ ] `apply_control(dwelling_index: int, name: str, signal: PyControlSignal)`: queues control signal
- [ ] `telemetry(dwelling_index: int) -> PyTelemetry`: returns telemetry for one dwelling
- [ ] `is_finished() -> bool`: whether all dwellings have completed
- [ ] `time_res_s() -> float`: timestep resolution in seconds (needed by `HELICSFleet` to drive federate time loop)
- [ ] `total_steps() -> int`: total simulation steps
- [ ] `current_step() -> int`: current step index
- [ ] `__len__() -> int` and `__repr__() -> str`
- [ ] Register `PySteppableFleet` in the pymodule in `lib.rs`
- [ ] Update `python/ochre_next/_hares.pyi` to add `SteppableFleet` stub class with all public methods

## Files to Touch
- `crates/hares-python/src/py_fleet.rs`: add `PySteppableFleet` pyclass
- `crates/hares-python/src/lib.rs`: register new class in pymodule
- `python/ochre_next/_hares.pyi`: add `SteppableFleet` stub class

## Measures of Success
- [ ] `SteppableFleet.from_configs(...)` constructs from Python without error
- [ ] `step()` releases the GIL and returns per-dwelling results
- [ ] `set_grid_voltage_all(0.95)` affects subsequent step results via ZIP model
- [ ] `is_finished()` transitions to `True` after stepping through all timesteps

## Verification
- [ ] `cargo check -p hares-python` passes
- [ ] `cargo clippy -p hares-python -- -D warnings` passes
- [ ] `maturin develop -p hares-python` succeeds
