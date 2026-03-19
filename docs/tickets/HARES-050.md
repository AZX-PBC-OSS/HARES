---
id: HARES-050
title: "hares-python — Fleet Bindings and Conversions"
kind: implement
depends_on: [HARES-049, HARES-048]
files_to_touch:
  - crates/hares-python/src/lib.rs
  - crates/hares-python/src/py_fleet.rs
  - crates/hares-python/src/conversions.rs
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - maturin develop -p hares-python
  - cargo clippy -p hares-python -- -D warnings
---

## Background/Context
Python users need to run fleet simulations through the same polars-based API as single-dwelling simulations. The GIL must be released during the Rayon parallel execution so Python threads remain responsive. `FleetResults` is converted to two polars DataFrames: one per-dwelling metrics table and one aggregate timeseries.

## Work to Do
- [ ] Implement `py_fleet.rs`: `#[pyclass] PyFleet`
  - [ ] `from_resstock(metadata_path: str, hpxml_dir: str, weather_dir: str, filter: dict | None = None, resstock_version: str | None = None) -> PyFleet` class method
    - [ ] `resstock_version` selects the ResStock schema variant; `None` uses the default (current) schema
  - [ ] `simulate(n_threads: int | None = None) -> PyFleetResults`
    - [ ] Release GIL during `Fleet::simulate()` call (Rayon parallel execution)
    - [ ] `n_threads = None` passes `0` to Rust, which uses the Rayon global pool; `n_threads > 0` builds a local `ThreadPool` via `rayon::ThreadPoolBuilder::new().num_threads(n).build()` and calls `.install(|| ...)`. The global pool cannot be resized per-call.
  - [ ] `PyFleetResults` struct exposed to Python; wraps `DwellingOutcome` (not `DwellingResult`) from HARES-047
    - [ ] `per_dwelling_metrics` property → polars `DataFrame`
    - [ ] `aggregate_timeseries` property → polars `DataFrame`
- [ ] Extend `conversions.rs`
  - [ ] `fleet_results_to_py(results: FleetResults) -> PyFleetResults`: converts both Arrow `RecordBatch` fields to polars DataFrames
- [ ] Register `PyFleet` and `PyFleetResults` in `lib.rs` pymodule

## Files to Touch
- `crates/hares-python/src/lib.rs`: register `PyFleet` and `PyFleetResults` in the pymodule
- `crates/hares-python/src/py_fleet.rs`: new file — `PyFleet` and `PyFleetResults` classes
- `crates/hares-python/src/conversions.rs`: extend with `fleet_results_to_py`

## Measures of Success
- [ ] `from ochre_next._hares import PyFleet` succeeds after `maturin develop`
- [ ] `PyFleet.from_resstock(...)` constructs a fleet without error given a valid metadata path
- [ ] `fleet.simulate(n_threads=2)` returns a `PyFleetResults` with non-empty `per_dwelling_metrics` and `aggregate_timeseries` DataFrames
- [ ] Accepted `resstock_version` strings and their enum mappings: `"2024.1"` → `V2024_1`, `"2024.2"` → `V2024_2`, `"2025.1"` → `V2025_1`; a test using `"2024.2"` constructs successfully; `"2.2.1"` is not a valid version string
- [ ] Both DataFrames have the expected column schemas
- [ ] `PyFleet.from_resstock(..., resstock_version="invalid")` raises `ValueError`
- [ ] `PyFleet` output DataFrame columns are a superset of single-dwelling (`PyDwelling`) output DataFrame columns

## Verification
- [ ] `maturin develop -p hares-python` succeeds
- [ ] `cargo clippy -p hares-python -- -D warnings` passes
- [ ] `uv run pytest tests/python/ -v -k "fleet"` passes
