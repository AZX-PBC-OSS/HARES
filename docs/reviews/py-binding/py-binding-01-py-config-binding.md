# Python config binding: completeness, type conversion safety, error handling
**Review ID**: py-binding-01
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
crates/hares-python/src/py_config.rs crates/hares-python/src/conversions.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/api/

## Findings

### Finding 1: No TOML round-trip path exposed to Python [Severity: high]
**Description**: The Rust `SimulationConfig::from_toml()` at `crates/hares-io/src/config.rs:87` is never exposed through the Python bindings. There is no Python-facing function to load a TOML config string, parse it through the Rust config parser, or serialize it back to TOML. The review requirement for TOML round-tripping from Python is unmet — Python users have no path to load a `.toml` file and get a validated `SimulationConfig`.
**Code Location**: `crates/hares-python/src/lib.rs:47-141` (module init has no `from_toml` or `parse_config` function); `crates/hares-io/src/config.rs:87` (existing but unwrapped Rust function).
**Root Cause**: The `SimulationConfig::from_toml` and TOML serialization (via serde) exist in the Rust layer but were never wrapped in `#[pyfunction]` or exposed as a `@staticmethod` on `PySimulationConfig`.
**Impact**: Python callers cannot parse TOML configs; they must construct `SimulationConfig` and `DwellingConfig` objects entirely via Python kwargs/dicts. There is no way to save a config back to TOML. This breaks the expected workflow where a TOML file is the canonical config format (as used throughout the Rust test suite).

### Finding 2: PyDwellingConfig has no property setters; config mutation is impossible [Severity: high]
**Description**: `PyDwellingConfig` at `crates/hares-python/src/py_config.rs:388-545` defines getters for `hpxml`, `schedule`, `weather`, `config`, `defaults_path`, `bldg_id`, `initialization_duration`, `resample_overrides`, and `overrides`, but provides **zero setters**. None of these fields can be mutated after construction. While `PySimulationConfig` (lines 67-310) has individual field setters, the `config` field on `PyDwellingConfig` — and the `schedule` / `weather` paths — cannot be changed once the `DwellingConfig` is created. The only way to change a schedule is to construct a brand-new `PyDwellingConfig` and re-create the `Dwelling`.
**Code Location**: `crates/hares-python/src/py_config.rs:388-545` (entire `#[pymethods] impl PyDwellingConfig` block contains only getters and `__repr__`).
**Root Cause**: Setters were never implemented for `PyDwellingConfig`.
**Impact**: The review requirement that "config mutation from Python (e.g., overriding a schedule or setpoint) correctly propagates through to the Rust config object before simulation start" cannot be satisfied. Users cannot programmatically mutate any aspect of the DwellingConfig from Python; they must instead construct new objects. This also means that `PyDwelling` (`crates/hares-python/src/py_dwelling.rs:527-551`) stores a `DwellingConfig` at creation time and never provides a method to update it.

### Finding 3: f64-to-i64 truncation in extract_seconds lacks range validation [Severity: medium]
**Description**: The `extract_seconds` helper at `crates/hares-python/src/py_dwelling.rs:1734-1750` handles `f64` inputs by rounding: `v.round() as i64` (line 1740). No check verifies that the rounded `f64` value fits within the `i64` range (−9.22×10¹⁸ to 9.22×10¹⁸). While practical simulation durations will never approach this bound, a Python user could pass `float('inf')` (which extracts as `f64` but is `inf`, not caught here) or an astronomically large value. `inf.round()` returns `inf`, and `inf as i64` yields `i64::MIN` (−9223372036854775808) — a silently corrupted negative duration.
**Code Location**: `crates/hares-python/src/py_dwelling.rs:1739-1741`.
**Root Cause**: The `f64` extraction path only rounds; there is no finite-range check before the cast. Compare with the constructor at `py_config.rs:122-124` which validates `duration <= 0` but never checks `is_finite` or the upper bound after the `f64 → i64` cast.
**Impact**: A Python user who accidentally passes an infinite or extremely large float for `duration_s` or `time_res_s` will get a silently corrupted value (either `i64::MIN` or an overflow wrap). The downstream `duration <= 0` check would catch `i64::MIN` (negative), but only with the error message "duration_s must be positive" — misleading for an `inf` input. Other paths using `extract_seconds` (e.g., `initialization_duration` at line 1613) share the same risk.

### Finding 4: default_start_time() uses expect() — potential Python interpreter panic [Severity: medium]
**Description**: `default_start_time()` at `crates/hares-python/src/py_config.rs:62-65` calls `.expect("valid default start time")` on a `DateTime::parse_from_rfc3339` result. The `DEFAULT_START` constant on line 15 is hardcoded to `"2019-01-01T00:00:00Z"`, which is currently valid. However, if this constant is ever changed incorrectly (e.g., during a refactor), the `.expect()` will panic, crashing the Python interpreter rather than raising a Python exception.
**Code Location**: `crates/hares-python/src/py_config.rs:62-65`.
**Root Cause**: The comment says "SAFETY: DEFAULT_START is a hardcoded constant known to be valid ISO 8601." While true today, `expect()` in a function called from Python `__new__` means any future mistake propagates as a hard panic. The identical function at `crates/hares-python/src/py_dwelling.rs:1752-1754` has the same pattern with the same `DEFAULT_START` constant (defined locally at line 53) and the same `expect()`.
**Impact**: If `DEFAULT_START` were corrupted, every call to `SimulationConfig()` or `Dwelling.from_hpxml()` would crash the Python process with no recoverable exception. This is a latent reliability risk.

### Finding 5: Duplicate python_to_json implementations with behavioral divergence risk [Severity: medium]
**Description**: Two nearly identical functions convert Python objects to `serde_json::Value`:
- `python_to_json` at `crates/hares-python/src/py_config.rs:20-60` (used by `PyDwellingConfig::new` for overrides)
- `python_to_json_value` at `crates/hares-python/src/py_dwelling.rs:1756-1797` (used by `build_config` for overrides)

The implementations are subtly different:
- `py_config.rs` checks `PyBool` via `is_instance_of::<PyBool>()` before `i64` extraction; `py_dwelling.rs` uses direct `extract::<bool>()`
- `py_config.rs` uses `serde_json::Number::from_f64(n)` with a fallback; `py_dwelling.rs` uses `serde_json::json!(f)`
- Error messages differ (`"unsupported Python type for JSON conversion"` vs `"cannot convert Python object of type ... to JSON for overrides"`)

If one is fixed or enhanced (e.g., adding support for `datetime` objects), the other will silently diverge.
**Code Location**: `crates/hares-python/src/py_config.rs:20-60` and `crates/hares-python/src/py_dwelling.rs:1756-1797`.
**Root Cause**: The converter was written twice in different modules instead of being placed in `utils.rs` and shared.
**Impact**: Maintenance hazard and potential behavioral inconsistency. A fix applied to one copy will not reach the other.

### Finding 6: PyDwellingConfig fields hpxml/schedule/weather are not validated as paths [Severity: low]
**Description**: `PyDwellingConfig::new` at `crates/hares-python/src/py_config.rs:394-471` accepts any string for `hpxml`, `schedule`, and `weather` without validating that the paths exist, are readable, or have correct file extensions (`.xml`, `.csv`, `.epw`). The Rust `DwellingConfig` stores these as `PathBuf` values but defers file-open to `Dwelling::from_config()`, which means Python users get a late and potentially confusing `HaresConfigError` back from `Dwelling.from_hpxml()` rather than from `DwellingConfig()`.
**Code Location**: `crates/hares-python/src/py_config.rs:460-463` (fields assigned without path validation).
**Root Cause**: Field validation in the Python constructor is limited to numeric/logic domains; no file-system checks are performed.
**Impact**: Delayed error detection. A user who passes a malformed HPXML path won't discover the problem until `Dwelling.from_hpxml()` is called, possibly minutes later in a batch workflow. Early validation with a `FileNotFoundError` or `PyValueError` at construction time would provide a better experience.

### Finding 7: output_chunk_size setter accepts zero without validation [Severity: low]
**Description**: `PySimulationConfig::set_output_chunk_size` at `crates/hares-python/src/py_config.rs:257-259` accepts any `usize` value, including `0`. At `to_sim_config()` time (line 332), this `0` is passed directly into `SimulationConfig`. While the default is `10_000`, a user could set `output_chunk_size = 0`, which might cause division-by-zero errors or infinite loops in the batch flushing logic depending on how the chunk size is used downstream.
**Code Location**: `crates/hares-python/src/py_config.rs:257-259`.
**Root Cause**: No minimum value check exists for `output_chunk_size`. The constructor at line 151 uses `DEFAULT_CHUNK_SIZE` (10_000) when not provided, but the setter has no floor.
**Impact**: A `0` chunk size could cause unexpected behavior in the Arrow RecordBatch writer at `crates/hares-python/src/conversions.rs:68-82`, where batches are flushed per-chunk. The IPC writer path should handle empty batches gracefully, but the simulation output pipeline is untested with a zero chunk size.

### Finding 8: Duration divisibility validated only at to_sim_config(), not at setter time [Severity: low]
**Description**: The constraint that `duration_s % time_res_s == 0` is validated in `PySimulationConfig::to_sim_config()` at `crates/hares-python/src/py_config.rs:314-319`, but NOT when `duration_s` or `time_res_s` are changed individually via their setters (`set_duration_s` at line 185, `set_time_res_s` at line 199). A user can set a valid `duration_s` and `time_res_s`, then separately change one to create an invalid combination, and the error only surfaces when `to_sim_config()` is called (i.e., at dwelling construction time).
**Code Location**: `crates/hares-python/src/py_config.rs:185-205` (setters lack cross-field validation).
**Root Cause**: Setters validate each field in isolation but don't re-validate the divisibility constraint as the pair.
**Impact**: Delayed error feedback. The Python API cannot detect misaligned duration/time_res until the Rust config conversion path is invoked. This is mitigated by the fact that `to_sim_config()` does re-validate before the config reaches the simulation, but early detection would be better UX.

## Field Coverage Audit

| Rust `SimulationConfig` field | Python `PySimulationConfig` field | Notes |
|---|---|---|
| `start_time: DateTime<FixedOffset>` | `start_time: String` (RFC 3339) | String-based getter/setter with `parse_datetime_str` validation |
| `duration: Duration` | `duration: i64` (seconds) | Converted via `Duration::seconds()` / `num_seconds()` |
| `time_res: Duration` | `time_res: i64` (seconds) | Same conversion pattern |
| `output_verbosity: u8` | `output_verbosity: u8` | Direct mapping, validated 0–8 |
| `output_path: Option<PathBuf>` | `output_path: Option<String>` | PathBuf ↔ String via `to_string_lossy()` |
| `write_output: bool` | `write_output: bool` | Direct mapping |
| `output_format: OutputFormat` | `output_to_parquet: bool` | Enum collapsed to bool; `Parquet` = true, `Csv` = false |
| `output_chunk_size: usize` | `output_chunk_size: usize` | Direct mapping |
| `setpoint_deadband_c: Option<f64>` | `setpoint_deadband_c: Option<f64>` | Direct mapping, validated ≥ 0 |
| `master_seed: u64` | `master_seed: u64` | Direct mapping |
| `civil_timezone: Option<String>` | `civil_timezone: Option<String>` | Direct mapping |

| Rust `DwellingConfig` field | Python `PyDwellingConfig` field | Notes |
|---|---|---|
| `hpxml_path: PathBuf` | `hpxml: String` | Converted at `to_dwelling_config()` |
| `schedule_path: PathBuf` | `schedule: String` | Converted at `to_dwelling_config()` |
| `weather_path: PathBuf` | `weather: String` | Converted at `to_dwelling_config()` |
| `defaults_path: Option<PathBuf>` | `defaults_path: Option<String>` | None / Some mapping |
| `sim_config: SimulationConfig` | `config: Option<PySimulationConfig>` | Optional wrapper; defaults applied when None |
| `overrides: Option<serde_json::Value>` | `overrides: Option<HashMap<String, Value>>` | PyDict → serde_json via `python_to_json` |
| `bldg_id: i64` | `bldg_id: i64` | Direct mapping, default 0 |
| `initialization_duration: Option<StdDuration>` | `initialization_duration: Option<i64>` | Seconds; 0 maps to None |
| `resample_overrides: Option<ResampleOverrides>` | `resample_overrides: Option<HashMap<String, String>>` | Raw strings parsed in `to_dwelling_config()` |

All Rust config fields are represented in the Python binding. The `output_format` enum → `output_to_parquet` bool mapping is a deliberate simplification. The `ResampleOverrides` struct with its 12 typed `Option<ResampleMethod>` fields is represented as a `HashMap<String, String>` in Python, with method-name validation applied at construction time.

## Vendor Comparison Notes

EnergyPlus's Python API (`vendors/EnergyPlus/src/EnergyPlus/api/`) uses ctypes to wrap C functions with explicit `argtypes`/`restype` declarations and runtime argument-count validation (e.g., `_check_callback_args` in `runtime.py:147-153`). HARES uses PyO3 with typed constructors, which provides compile-time type enforcement and eliminates the class of errors that ctypes-based bindings are susceptible to (wrong arg count, wrong ctypes). EnergyPlus also provides `EnergyPlusException` (`common.py`) for all API errors — this aligns with HARES's use of `HaresConfigError`, `HaresEquipmentError`, and `HaresSimulationError` as domain-specific Python exception types.

## Summary
- Total findings: 8
- Critical: 0
- High: 2 (No TOML round-trip; no PyDwellingConfig setters)
- Medium: 3 (f64-to-i64 range check; expect() panic risk; duplicate implementations)
- Low: 3 (no path validation; zero chunk size; delayed divisibility check)

## Recommendations
1. Expose `SimulationConfig.from_toml(s: str)` as a `@staticmethod` on `PySimulationConfig`, and add a `to_toml()` method for round-tripping. This should call `hares_io::SimulationConfig::from_toml` and map `ConfigError` to `HaresConfigError`.
2. Add property setters to `PyDwellingConfig` for all mutable fields, and add a `reconfigure()` method to `PyDwelling` that accepts a `PyDwellingConfig` and applies changes before simulation start.
3. Add `is_finite()` and `i64` range checks in `extract_seconds` at `crates/hares-python/src/py_dwelling.rs:1739-1741`. Reject `inf`, `NaN`, and values ≥ `i64::MAX` with a clear `PyValueError`.
4. Replace `expect()` in `default_start_time()` (`py_config.rs:62-65`) and `default_start()` (`py_dwelling.rs:1752-1754`) with `map_err(|e| PyValueError::new_err(format!("internal default start time invalid: {e}")))`.
5. Consolidate `python_to_json` and `python_to_json_value` into a single public function in `crates/hares-python/src/utils.rs` and update both call sites.
6. Consider path exist/extension validation in `PyDwellingConfig::new` to give early feedback; at minimum, validate that `.xml`, `.csv`, `.epw` extensions are present.
7. Add a minimum value check in `set_output_chunk_size` (e.g., `> 0`).

## References / Citations
- `crates/hares-python/src/py_config.rs:20-622` — primary Python binding for config types
- `crates/hares-python/src/py_dwelling.rs:1437-1797` — config construction from kwargs, `extract_seconds`, `python_to_json_value`
- `crates/hares-python/src/utils.rs:28-89` — `parse_datetime_str`, `parse_resample_method`
- `crates/hares-io/src/config.rs:35-308` — Rust `SimulationConfig` struct and `from_toml`
- `crates/hares-core/src/dwelling/mod.rs:99-114` — Rust `DwellingConfig` struct
- `crates/hares-io/src/weather.rs:382-396` — Rust `ResampleOverrides` struct
- `crates/hares-python/src/lib.rs:47-141` — Python module initialization
- `tests/python/test_py_config.py` — existing Python config tests
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — EnergyPlus Python API (comparison)
