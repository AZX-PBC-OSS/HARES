---
id: HARES-053
title: "Pure Python — External Tool Adapters"
kind: implement
depends_on: [HARES-049]
files_to_touch:
  - python/ochre_next/adapters/__init__.py
  - python/ochre_next/adapters/sam_pv.py
  - python/ochre_next/adapters/sam_battery.py
  - python/ochre_next/adapters/pybamm_battery.py
references:
  - docs/architecture/05-external-tools.md
verification:
  - uv run pytest tests/python/ -v -k "adapter"
---

## Background/Context
PV performance and battery electrochemical parameters come from external tools (PySAM, PyBaMM) that are optional dependencies. Adapters cache their outputs as Parquet LUTs or TOML parameter files so the expensive external calls happen only when inputs change. A fallback chain (PyBaMM → SAM → built-in defaults) ensures the system degrades gracefully when optional tools are absent.

## Work to Do
- [ ] Implement `sam_pv.py`: `generate_pv_lut`
  - [ ] Signature: `generate_pv_lut(system_capacity_kw: float, tilt: float, azimuth: float, module_type: int, array_type: int, weather_file: Path) -> PvLut` returning a LUT object (not a bare `Path`)
  - [ ] `PvLut` exposes a `.save(path: Path) -> None` method that writes the Parquet file to disk
  - [ ] LUT maps `(month, hour, ghi, dni, dhi, temp_c)` → `ac_power_kw` using PySAM PVWatts; GHI/DNI/DHI binned at 50 W/m² intervals, temperature at 5°C intervals; interpolation is multilinear (N-dimensional linear); LUT is written as Parquet with column schema: `month`, `hour`, `ghi`, `dni`, `dhi`, `temp_c`, `ac_power_kw`. The Rust consumer (in hares-equipment PV model) reads this Parquet and performs the interpolation.
  - [ ] Cache to disk: compute a SHA-256 hash of all input parameter values concatenated with the SHA-256 of the weather file content; parameter concatenation uses canonical serialization: keys sorted alphabetically, values formatted with fixed precision (`{:.6}`); this prevents hash instability from dict ordering or float formatting differences; store the hash in a sidecar `.hash` file alongside the Parquet file; skip regeneration if the Parquet file and sidecar exist and the stored hash matches
  - [ ] Changed input params or weather file content must invalidate the cache and trigger full regeneration
  - [ ] When PySAM is absent, PV generation uses direct PVWatts equations in the Rust core — no adapter output is required in this path; document this clearly in the module docstring
- [ ] Implement `sam_battery.py`: `extract_cell_params`
  - [ ] Signature: `extract_cell_params(chemistry: str, capacity_kwh: float, source: str = "SAM") -> CellParams` returning a params object (not a bare `Path`)
  - [ ] `CellParams` exposes a `.save(path: Path) -> None` method that writes the TOML file to disk
  - [ ] TOML contains: `v_nominal`, `ah_rated`, `r_internal`, `n_series`, `n_parallel`, `soc_ocv` table, thermal params, losses, degradation model parameters
  - [ ] Apply the same SHA-256 + sidecar `.hash` caching scheme as `generate_pv_lut` (same canonical serialization: keys sorted alphabetically, values at `{:.6}` precision)
- [ ] Implement `pybamm_battery.py`: `generate_efficiency_lut` and `generate_degradation_params`
  - [ ] `generate_efficiency_lut(chemistry: str, capacity_ah: float, n_series: int, n_parallel: int, temperature_range_c: tuple[float, float], soc_range: tuple[float, float], power_range_kw: tuple[float, float], age_cycles: int) -> EfficiencyLut` — returns a LUT object with `.save(path: Path) -> None`; Parquet on save
  - [ ] `generate_degradation_params(chemistry: str, capacity_ah: float, temperature_range_c: float) -> DegradationParams` — returns a params object with `.save(path: Path) -> None`; TOML on save
  - [ ] Skip gracefully (return built-in defaults) if PyBaMM is not installed
- [ ] Fallback chain implemented in `__init__.py`: try PyBaMM → SAM → built-in defaults; log which source was used
- [ ] All adapters are optional: import guards with `ImportError` catch and descriptive messages

## Files to Touch
- `python/ochre_next/adapters/__init__.py`: fallback chain logic, re-exports
- `python/ochre_next/adapters/sam_pv.py`: `generate_pv_lut` with PySAM and caching
- `python/ochre_next/adapters/sam_battery.py`: `extract_cell_params` with SAM and caching
- `python/ochre_next/adapters/pybamm_battery.py`: `generate_efficiency_lut`, `generate_degradation_params`

## Measures of Success
- [ ] `generate_pv_lut` with a mock PySAM returns a `PvLut` object; calling `.save(path)` writes a Parquet file with the expected schema (`month`, `hour`, `ghi`, `dni`, `dhi`, `temp_c`, `ac_power_kw`)
- [ ] `extract_cell_params` returns a `CellParams` object; calling `.save(path)` writes a TOML file containing all required keys
- [ ] A second call to `generate_pv_lut` with identical inputs reads from cache (sidecar `.hash` matches) and does not invoke PySAM again
- [ ] Changing any input parameter or the weather file content invalidates the cache and triggers PySAM regeneration
- [ ] `generate_efficiency_lut` uses `power_range_kw` parameter name; calling with `power_range` raises `TypeError`
- [ ] With PyBaMM not installed, `generate_efficiency_lut` returns an `EfficiencyLut` backed by built-in defaults without raising
- [ ] All adapter tests pass when optional dependencies are absent (marked with `pytest.importorskip`)

## Verification
- [ ] `uv run pytest tests/python/ -v -k "adapter"` passes
