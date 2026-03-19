---
id: HARES-068
title: "ochre_next — OCHRE Compat simulate/update_model/generate_results"
kind: implement
depends_on: [HARES-051]
files_to_touch:
  - python/ochre_next/dwelling.py
references:
  - docs/architecture/09-roadmap.md
  - ochre/models/dwelling.py (upstream reference)
verification:
  - uv run pytest tests/python/ -v -k "compat_api"
---

## Background/Context

OCHRE's `Dwelling` exposes `simulate()`, `update_model()`, and `generate_results()` as its primary API. The compat layer (HARES-051) must implement these with matching signatures and return types so existing OCHRE-based workflows run against ochre_next without modification.

## Work to Do

- [ ] Implement `Dwelling.simulate(start_time=None, duration=None, verbosity=None) -> tuple[DataFrame, dict, DataFrame]`:
  - [ ] Runs the full simulation from `start_time` for `duration` (both default to values from sim config if None)
  - [ ] Returns `(timeseries_df, metrics_dict, hourly_df)` — a 3-tuple matching OCHRE's return signature
  - [ ] `timeseries_df`: per-timestep results as a `polars.DataFrame` with a datetime column (use `.to_pandas()` for pandas compat)
  - [ ] `metrics_dict`: scalar summary metrics as a `dict[str, float]`
  - [ ] `hourly_df`: hourly-aggregated results as a `polars.DataFrame`
  - [ ] `verbosity` int controls which columns appear in results (matching OCHRE verbosity levels 0–3)
- [ ] Implement `Dwelling.update_model(control_signal: dict | None = None) -> None`:
  - [ ] Applies OCHRE-format control signal dict and advances one timestep
  - [ ] Returns `None` (matching OCHRE — not the timestep result)
  - [ ] `control_signal=None` advances one timestep with no external control input
- [ ] Implement `Dwelling.generate_results(verbosity: int = 0) -> dict[str, float]`:
  - [ ] Returns current-timestep results as a dict keyed by OCHRE measurement name
  - [ ] At verbosity 0: must include at minimum `"Time"` and `"Total Electric Power (kW)"` keys
  - [ ] Higher verbosity levels add equipment-specific keys (per-equipment power, mode, temperatures)

## Measures of Success

- [ ] `d.simulate()` returns a 3-tuple `(DataFrame, dict, DataFrame)`
- [ ] `d.simulate()` 3-tuple: first element is a DataFrame with a datetime index
- [ ] `d.update_model({"HVAC Heating": {"Duty Cycle": 0.5}})` returns `None`
- [ ] `d.update_model(None)` advances one timestep without error
- [ ] `d.generate_results()` returns a dict containing at minimum `"Time"` and `"Total Electric Power (kW)"` keys at verbosity 0
- [ ] `d.generate_results(verbosity=3)` returns a superset of keys from `d.generate_results(verbosity=0)`

## Verification

- [ ] `uv run pytest tests/python/ -v -k "compat_api"` passes
- [ ] `uv run ruff check python/ochre_next/dwelling.py` passes
