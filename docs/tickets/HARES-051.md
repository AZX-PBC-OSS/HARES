---
id: HARES-051
title: "Pure Python — OCHRE Compat Layer"
kind: implement
depends_on: [HARES-049]
files_to_touch:
  - python/ochre_next/__init__.py
  - python/ochre_next/compat/__init__.py
  - python/ochre_next/compat/dwelling.py
  - python/ochre_next/_hares.pyi
references:
  - docs/architecture/03-control-interfaces.md
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - uv run pytest tests/python/ -v
---

## Background/Context
Existing OCHRE users construct `Dwelling` with keyword arguments (`hpxml_file`, `hpxml_schedule_file`, `weather_file`, `start_time`, `time_res`, `duration`, `Equipment={}` overrides). The compat layer wraps `PyDwelling` behind the same interface so that consumer code requires no changes beyond updating the import path. Type stubs for the Rust extension make IDEs and type checkers work correctly.

## Work to Do
- [ ] `python/ochre_next/__init__.py`: re-export `Dwelling`, `Fleet`, `ControlSignal` from `ochre_next._hares`
- [ ] `python/ochre_next/compat/dwelling.py`: `Dwelling` class wrapping `PyDwelling`
  - [ ] Constructor accepts OCHRE kwargs: `hpxml_file`, `hpxml_schedule_file`, `weather_file`, `start_time`, `time_res`, `duration`, `initialization_time`, `Equipment={}` (overrides dict)
    - [ ] `start_time`, `time_res`, and `duration` are required OCHRE kwargs; map these to the corresponding `PyDwelling.from_hpxml()` parameters
  - [ ] Maps kwargs to `PyDwelling.from_hpxml()` call
  - [ ] `simulate() -> tuple[polars.DataFrame, dict, polars.DataFrame]`: runs full simulation; returns `(df, metrics, df_hourly)` matching OCHRE's return convention — a timeseries DataFrame, a dict of summary metrics keyed by metric name, and an hourly aggregated DataFrame; provide a `.to_pandas()` helper on the returned DataFrames for callers that need pandas compatibility
  - [ ] `update_model(control_signal: dict) -> None`: applies a control signal dict (OCHRE format) and steps one timestep; matches OCHRE's `update_model()` which has no return value; unrecognized control keys are logged as warnings and skipped — they do not raise
  - [ ] `generate_results() -> dict[str, float]`: triggers the post-simulation metrics pass (equivalent to `Analysis.calculate_metrics()` in OCHRE); returns a dict keyed by measurement name (e.g., `"Total Electric Power (kW)": float`); must only be called after `simulate()` or after the step loop ends — not an alias for `simulate()`. Operates on the Python-side DataFrame returned by `simulate()` (stored on the wrapper between calls). Does NOT re-invoke Rust — it computes metrics from the stored DataFrame.
  - [ ] Warning on unrecognized `update_model()` keys is a deliberate improvement over OCHRE (which silently ignores them). Document this as an intentional behavioral change.
- [ ] `python/ochre_next/compat/__init__.py`: export `Dwelling` from `compat.dwelling`
- [ ] `python/ochre_next/_hares.pyi`: type stubs for all Rust extension classes
  - [ ] `PyDwelling`, `PyFleet`, `PyControlSignal`, `Battery`, `PV`, `EV` with typed method signatures
  - [ ] Return types annotated with `polars.DataFrame` and `dict[str, Any]` where appropriate
  - [ ] Include `initialize() -> None` and `timesteps() -> Iterator[Any]` stubs for `PyDwelling` once HARES-049 adds these methods

## Files to Touch
- `python/ochre_next/__init__.py`: top-level re-exports
- `python/ochre_next/compat/__init__.py`: compat subpackage exports
- `python/ochre_next/compat/dwelling.py`: OCHRE-compatible `Dwelling` wrapper
- `python/ochre_next/_hares.pyi`: type stubs for Rust extension

## Measures of Success
- [ ] `from ochre_next.compat import Dwelling; d = Dwelling(hpxml_file=..., hpxml_schedule_file=..., weather_file=..., start_time=..., time_res=..., duration=...)` constructs without error
- [ ] `d.simulate()` returns a polars `DataFrame` with columns matching OCHRE output format; `d.simulate().to_pandas()` returns an equivalent pandas DataFrame
- [ ] `d.update_model({"HVAC Heating": {"Setpoint Temperature (C)": 21.0}})` does not raise and returns `None` (uses OCHRE end-use category and key format per architecture)
- [ ] `d.update_model({"UnknownEquipment": {"SomeKey": 1.0}})` does not raise; a warning is emitted and the key is skipped
- [ ] `d.generate_results()` after simulation returns a `dict[str, float]` keyed by measurement name and does not re-run the simulation
- [ ] Mypy or pyright reports no errors on a file that imports from `ochre_next.compat` using the stubs
- [ ] At least one reference building produces output within architecture-specified tolerance bands (±1% annual energy, ±0.1°C MAE zone temperature) compared to OCHRE reference output

## Verification
- [ ] `uv run pytest tests/python/ -v` passes
