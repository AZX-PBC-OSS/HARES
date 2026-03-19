---
id: HARES-049
title: "hares-python — Core Bindings (Dwelling and Control)"
kind: implement
depends_on: [HARES-044, HARES-045]
files_to_touch:
  - crates/hares-python/src/lib.rs
  - crates/hares-python/src/py_dwelling.rs
  - crates/hares-python/src/py_control.rs
  - crates/hares-python/src/py_equipment.rs
  - crates/hares-python/src/py_telemetry.rs
  - crates/hares-python/src/conversions.rs
references:
  - docs/architecture/03-control-interfaces.md
  - docs/architecture/04-data-ingestion-and-fleet.md
  - docs/architecture/05-external-tools.md
verification:
  - maturin develop -p hares-python
  - cargo clippy -p hares-python -- -D warnings
  - uv run pytest tests/python/ -v -k "dwelling"
---

## Background/Context
`hares-python` exposes the Rust simulation core to Python via PyO3/maturin. All CPU-bound calls must release the GIL so that Python threads are not blocked. Arrow RecordBatch output is converted to polars DataFrames for idiomatic Python consumption. This crate is the foundation for the OCHRE compat layer and the RL gymnasium interface.

## Work to Do
- [ ] Implement `lib.rs`: `#[pymodule]` registering all classes and functions
- [ ] Implement `py_dwelling.rs`: `#[pyclass] PyDwelling`
  - [ ] `from_hpxml(hpxml, schedule, weather, **kwargs) -> PyDwelling` class method
  - [ ] `initialize()` — must be called once before the step loop; sets up equipment state and output buffers
  - [ ] `timesteps() -> iterator` — yields one token per simulation timestep as a Python `datetime.datetime` object; used as `for t in dwelling.timesteps():`; consistent with the HELICS orchestration example in the architecture
  - [ ] `simulate() -> polars.DataFrame` — releases GIL during Rust computation
  - [ ] `step() -> dict` — single timestep, releases GIL
  - [ ] `apply_control(name: str, signal: PyControlSignal)`
  - [ ] `set_price_signal(signal: dict)`
  - [ ] `set_grid_voltage(voltage_pu: float)` — writes `voltage_pu` into `GridState`; used by HELICS co-sim to inject grid voltage each timestep
  - [ ] `telemetry() -> PyTelemetry` — returns a typed object with `.zone()` and `.equipment()` accessor methods (not a plain dict), matching the architecture
  - [ ] `results() -> polars.DataFrame`
- [ ] Implement `py_control.rs`: `#[pyclass] PyControlSignal`
  - [ ] Python constructors: `.power_setpoint(kw=...)`, `.thermal_setpoint(heat_c=..., cool_c=...)`, `.soc_target(target=..., min=..., max=...)`
  - [ ] `ControlSignal.from_dict(d: dict) -> PyControlSignal` class method — deserializes a plain dict (e.g., from a HELICS JSON payload) into a typed `PyControlSignal`; document the dict schema for all variants: `ThermalSetpoint`, `ModeOverride`, `PowerSetpoint`, `HumiditySetpoint`, `SelfConsumption`, and any other defined variant (note: `PriceSignal` is NOT a `ControlSignal` variant per the architecture — price signals are set separately via `set_price_signal()`)
- [ ] Implement `py_equipment.rs`: equipment config constructors for native Python API
  - [ ] `Battery(name, capacity_kwh, ...)`, `PV(name, capacity_kw, tilt, azimuth, ...)`, `EV(name, ...)`
- [ ] Implement `py_telemetry.rs`: `#[pyclass] PyTelemetry` with `.zone()` and `.equipment()` accessor methods exposing zone temps, equipment states, SOC values
- [ ] Implement `conversions.rs`
  - [ ] Arrow `RecordBatch` → polars `DataFrame` conversion
  - [ ] Numpy array helpers for RL observations: produce contiguous `f64` buffers without copying
- [ ] Implement `fn batch_step(dwellings: &mut [PyDwelling], actions: &[Vec<f64>]) -> Vec<StepResult>` using `py.allow_threads()` and Rayon parallel iteration. Define `StepResult { obs: Vec<f64>, reward: f64, terminated: bool, truncated: bool, info: HashMap<String, f64> }`. This is the Rust entry point for `VecDwellingGymEnv`.
- [ ] Implement `PyDwelling::reset_with_seed(seed: u64)` that restores initial state AND re-seeds the internal `ChaCha8Rng`. Required for RL `reset(seed=N)` determinism.
- [ ] Implement `PyDwelling::save_state() -> PyBytes` and `PyDwelling::load_state(state: &[u8]) -> PyResult<()>` wrapping `DwellingCheckpoint` serialization. Required for RL checkpoint/restore in HARES-052.
- [ ] Ensure `polars>=1` and `pyarrow>=18` are added to `[project.dependencies]` in `/home/rich/src/HARES/pyproject.toml` (not optional deps — they are required for core `simulate()` return type).

## Files to Touch
- `crates/hares-python/src/lib.rs`: pymodule entry point, class registration
- `crates/hares-python/src/py_dwelling.rs`: `PyDwelling` class
- `crates/hares-python/src/py_control.rs`: `PyControlSignal` class
- `crates/hares-python/src/py_equipment.rs`: equipment config classes
- `crates/hares-python/src/py_telemetry.rs`: telemetry dict conversion
- `crates/hares-python/src/conversions.rs`: Arrow→polars and numpy helpers

## Measures of Success
- [ ] `from ochre_next._hares import PyDwelling` succeeds after `maturin develop`
- [ ] `PyDwelling.from_hpxml(...)` constructs a dwelling without error
- [ ] `initialize()` can be called without error before the step loop
- [ ] `for t in dwelling.timesteps():` iterates for the expected number of timesteps
- [ ] `simulate()` returns a polars `DataFrame` with the expected column names
- [ ] `PyControlSignal.power_setpoint(kw=5.0)` is constructible from Python
- [ ] `ControlSignal.from_dict({"type": "PowerSetpoint", "active_power_kw": 3.0})` returns a valid `PyControlSignal`
- [ ] `set_grid_voltage(1.05)` does not raise and is reflected in subsequent telemetry
- [ ] `telemetry()` returns a `PyTelemetry` object; `.zone()` contains zone temperature data and `.equipment()` contains SOC and equipment state data
- [ ] Threading test: `step()` releases the GIL — verified by a pytest test that acquires a Python `threading.Lock` from a background thread during `step()` and confirms it is not blocked

## Verification
- [ ] `maturin develop -p hares-python` succeeds
- [ ] `cargo clippy -p hares-python -- -D warnings` passes
- [ ] `uv run pytest tests/python/ -v -k "dwelling"` passes
