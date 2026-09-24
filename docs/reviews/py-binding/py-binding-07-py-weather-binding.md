# Python weather binding: EPW/PSM3/TMY3 parsing from Python
**Review ID**: py-binding-07
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_weather.rs` (150 lines)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` -- weather data exchange API; no dedicated latitude/longitude/elevation exposure
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` -- weather file path via `get_weather_file_path()`, no metadata exposure
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` -- no weather metadata functions
- `vendors/EnergyPlus/src/EnergyPlus/api/func.py` -- no weather functions

## Findings

### Finding 1: [Severity: medium] No `resample()` method exposed to Python
**Description**: The `WeatherTimeSeries::resample()` and `resample_with()` methods in the Rust core support sub-hourly resampling with PCHIP (temperature), ZOH (solar irradiance), circular-linear (wind direction), and other strategies. However, the Python binding `PyWeatherTimeSeries` does not expose any resampling capabilities. Python users cannot programmatically resample weather data to a target timestep before inspection.
**Code Location**: `crates/hares-python/src/py_weather.rs:11-99` (entire `#[pymethods]` block lacks a `resample` method); compare to `crates/hares-io/src/weather.rs:538-761` (Rust `resample`/`resample_with`).
**Root Cause**: The Python binding was designed for read-only weather inspection + DataFrame export. Resampling is deferred to the `Dwelling::from_config()` construction path, where it happens internally inside `EnvironmentManager::new_with_resample()`.
**Impact**: Python users who load weather for data exploration cannot resize it for different simulation timesteps without creating a `Dwelling` first. This is a discoverability gap -- the resample logic exists and is correct, but the Python API hides it.

### Finding 2: [Severity: medium] `PyWeatherTimeSeries` cannot be passed directly into `DwellingConfig`
**Description**: `DwellingConfig.__init__()` (`py_config.rs:390-395`) accepts a `weather: String` (file path), not a `PyWeatherTimeSeries` object. The `Dwelling` constructor then re-parses the weather file from scratch (`dwelling/mod.rs:798`). If a user loads weather data via `parse_epw()`, inspects or modifies columns, and wants to use that modified data for simulation, they must write a new file and pass its path -- there is no programmatic hand-off.
**Code Location**: `crates/hares-python/src/py_config.rs:364` (`weather: String`); `crates/hares-python/src/py_dwelling.rs:534` (`weather: String`).
**Root Cause**: The dwelling construction pipeline is file-path-driven (`DwellingConfig { weather_path: PathBuf }`). There is no `from_preparsed` variant exposed to Python.
**Impact**: Prevents programmatic weather manipulation pipelines. Users who want to perturb weather inputs must round-trip through the filesystem. Low churn risk for the current codebase (no known Python users doing this), but limits Python API expressiveness compared to the Rust core.

### Finding 3: [Severity: medium] No column-length invariant enforcement on `WeatherTimeSeries`
**Description**: `WeatherTimeSeries` contains 14 `Vec<f64>` columns plus an `Option<Vec<f64>>` for albedo. All fields are `pub` with no constructor or validation that enforces equal lengths. The `len()` method (`weather.rs:500-502`) returns `self.dry_bulb_c.len()` as the canonical length, and the `get()` indexer (`weather.rs:515-536`) uses a direct `[timestep_index]` access that will panic if any other column is shorter.
**Code Location**: `crates/hares-io/src/weather.rs:468-495` (struct definition); `crates/hares-io/src/weather.rs:514-536` (unchecked `get()`).
**Root Cause**: The struct is built via struct-literal syntax in each parser (e.g., `epw.rs`, `psm3.rs`, `tmy3.rs`). Each parser constructs all columns within a single function and the lengths are naturally consistent, so no runtime bug exists today. However, there is no defensive `debug_assert!` or `new()` guard.
**Impact**: A future parser bug that silently produces a shorter column (e.g., an early-exit bug in row iteration, a builder-pattern oversight) would cause a runtime panic deep in `EnvironmentManager::update_in_place()`, not a graceful parse error. Mitigated in practice because all three format parsers already validate total record count (`8760 || 8784` for EPW/TMY3, format-specific counts for PSM3).

### Finding 4: [Severity: medium] Several weather fields lack range validation and could silently propagate NaN
**Description**: The EPW parser applies range checks to dry bulb, pressure, GHI, wind speed, and horizontal infrared, but NOT to DNI, DHI, wind direction, opaque sky cover, or relative humidity. If an EPW file contains `"NaN"` or out-of-range values for these unchecked fields, they are parsed as `f64` and stored without validation. Rust's `f64::parse("NaN")` succeeds and produces `f64::NAN`, which can propagate into the simulation and produce silent garbage outputs.
**Code Location**: `crates/hares-io/src/epw.rs:184-189` (DNI, DHI parsed without range check); `crates/hares-io/src/epw.rs:191` (wind_dir unchecked); `crates/hares-io/src/epw.rs:199` (opaque sky cover unchecked); `crates/hares-io/src/epw.rs:172` (RH unchecked).
**Root Cause**: The parser added range checks only for fields with well-known physical bounds cited in the EPW specification. DNI/DHI lack a tight upper bound because they can briefly exceed the solar constant under cloud-edge enhancement. However, a simple `is_finite()` check would catch NaN.
**Impact**: Corrupt EPW files could produce physically impossible simulation results. Mitigated because real EPW files from NREL/DOE sources do not contain NaN, and the TMY3/PSM3 parsers use `parse_trimmed_f64` (which may have its own NaN treatment -- TMY3 parser `parse_f64` at `tmy3.rs:353-362` does not reject NaN either).

### Finding 5: [Severity: low] `design_conditions` not exposed to Python
**Description**: The Rust `WeatherTimeSeries` struct carries a `design_conditions: Option<DesignConditions>` field (`weather.rs:473`) with heating and cooling design dry-bulb temperatures extracted from EPW header line 2. The Python binding does not expose this field -- no getter, no DataFrame column.
**Code Location**: `crates/hares-python/src/py_weather.rs:60-98` (all getters, none for design conditions).
**Root Cause**: `DesignConditions` is a niche EPW-header feature not used by the OCHRE simulation engine directly; it's consumed by `EnvironmentManager::compute_weather_averages()`. The Python binding prioritized the simulation-relevant fields.
**Impact**: Users writing Python pre-processing scripts that need design temperatures must parse the EPW header manually. Low impact since design conditions are primarily used for equipment sizing, which happens in the Rust core.

### Finding 6: [Severity: low] TMY3 column name matching requires exact (case-insensitive) match
**Description**: The TMY3 parser's `build_column_map()` (`tmy3.rs:269-290`) requires exact column names like `"GHI (W/m^2)"`, `"DNI (W/m^2)"`, `"DHI (W/m^2)"`. If a source file has subtly different headers (e.g., `"GHI (W/m2)"` without the caret, `"GHI (w/m^2)"`, or `"Global Horizontal (W/m^2)"`), parsing fails with a `Parse` error.
**Code Location**: `crates/hares-io/src/tmy3.rs:270-290`; format-sniffing at `tmy3.rs:368-374` uses the same pattern.
**Root Cause**: TMY3 format specification (NREL/TP-581-43156) defines these column names precisely, so exact-match is correct for compliant files. Some third-party datasets may use slightly variant columns.
**Impact**: Non-standard TMY3 files require manual header normalization before parsing. The error message includes the missing column name, making diagnosis straightforward. Low severity since compliant TMY3 files are produced by NREL tooling.

### Finding 7: [Severity: low] PSM3 format has minimal Python test coverage
**Description**: The Python test file (`tests/python/test_py_weather.py:222-227`) only tests that `parse_psm3` raises on a nonexistent path. There is no test that loads a real PSM3 file, verifies record count, checks metadata, or confirms the `surface_albedo` field (PSM3's distinguishing feature) is populated.
**Code Location**: `tests/python/test_py_weather.py:222-227`.
**Root Cause**: No PSM3 fixture file was available in `data/examples/` when the test was written. The Rust-level tests (`psm3.rs:529-852` and `tests/psm3_parity.rs:474`) have comprehensive coverage.
**Impact**: A Python packaging or binding bug specific to PSM3 (e.g., the albedo `Option<Vec<f64>>` → Python `Optional[List[float]]` conversion) would not be caught. Low risk since the binding is a thin passthrough.

### Finding 8: [Severity: low] Precipitation parse failure is silently replaced with 0.0 in EPW
**Description**: In the EPW parser (`epw.rs:218-234`), when precipitation field parsing fails, the error is logged with `warn!` but the value is silently substituted with `0.0`. This is the **only** field across all three parsers that uses silent-fallback instead of a hard error.
**Code Location**: `crates/hares-io/src/epw.rs:232-234`.
**Root Cause**: EPW field 33 (Liquid Precipitation Depth) is documented as optional/historically unreliable. Many legacy EPW files use sentinel values (999) or leave this field blank. The silent fallback prevents legacy files from being rejected.
**Impact**: Users who inadvertently point to a file where field-33 data matters will not see an error. A `warn!` log is emitted, but this may not be visible from Python. Mitigated because precipitation is not a critical input for most residential energy simulations (which are driven by temperature and solar).

## Summary
- Total findings: 8
- Critical: 0 / High: 0 / Medium: 4 / Low: 4

## Recommendations

1. **Expose `resample()` on `PyWeatherTimeSeries`**: Add a method like `weather.resample(target_step_secs: int, overrides: dict | None = None) -> PyWeatherTimeSeries` so Python users can resize weather data before inspection or dwelling construction. This unlocks exploratory data analysis at arbitrary timesteps.

2. **Accept `PyWeatherTimeSeries` in `DwellingConfig`**: Add a code path where `weather` can be either a `str` (file path) or a `PyWeatherTimeSeries` object. Use a PyO3 `#[pyo3(from_py_with = "...")]` or `#[new]` overload. This enables programmatic weather modification workflows.

3. **Add column-length invariant check**: Add a `debug_assert!` in a `WeatherTimeSeries::new()` constructor (or `#[cfg(debug_assertions)]` check) that verifies all column vectors have equal length. For release, add a one-time check in `resample()` output. This catches parser bugs before they become runtime panics.

4. **Add `is_finite()` checks for DNI/DHI/RH/sky-cover**: In the EPW parser, add `if !value.is_finite() { return Err(...) }` for the currently-unchecked fields. This is a one-line addition per field and prevents NaN propagation.

5. **Add a PSM3 fixture and Python test**: Place a small PSM3 fixture file in `data/examples/` and add tests for metadata extraction, record count, and `surface_albedo` access. This completes Python format coverage parity with the Rust test suite.

6. **Consider exposing `design_conditions` to Python**: Add a `design_conditions` property returning `Optional[tuple[float, float]]` (heating_db, cooling_db) or a dict. Low priority but useful for pre-processing.

## References / Citations

- EnergyPlus WeatherManager.cc `ProcessEPWHeader()` (`vendors/EnergyPlus/src/EnergyPlus/api/` → C++ source not in vendor tree, but structurally confirmed via `WeatherManager.hh:792-795` storage of `WeatherFileLatitude`, `WeatherFileLongitude`, `WeatherFileTimeZone`, `WeatherFileElevation`)
- EnergyPlus Python API `datatransfer.py:389-400` -- `get_weather_file_path()` is the closest metadata exposure; no dedicated latitude/longitude/elevation accessors
- HARES EPW parser: `crates/hares-io/src/epw.rs:80-280` -- parsing, validation, record-count check
- HARES PSM3 parser: `crates/hares-io/src/psm3.rs:43-470` -- timestep detection, albedo, column mapping
- HARES TMY3 parser: `crates/hares-io/src/tmy3.rs:41-375` -- column-name matching, station header
- HARES resampling: `crates/hares-io/src/weather.rs:538-761` -- upsampling (PCHIP/ZOH), downsampling (mean/sum)
- HARES weather indexing: `crates/hares-core/src/environment.rs:478-485` -- `(step + offset) % weather_len` with `midpoint_offset_secs` for EPW hour-ending convention
- Python tests: `tests/python/test_py_weather.py:1-227`
