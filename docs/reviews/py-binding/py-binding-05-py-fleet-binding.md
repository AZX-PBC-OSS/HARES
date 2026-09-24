# Python fleet binding: parallel execution, progress, aggregation
**Review ID**: py-binding-05
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_fleet.rs` (568 lines)
- `crates/hares-fleet/src/fleet.rs` (960 lines)
- `crates/hares-fleet/src/aggregation.rs` (730 lines)
- `crates/hares-python/src/conversions.rs` (200 lines)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/api.py` — top-level API class exposing `state`, `functional`, `runtime`, `exchange` members
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — EnergyPlus callback and progress registration pattern, including the `all_callbacks` GC-pin list
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — explicit `new_state()` / `reset_state()` / `delete_state()` lifecycle for state objects
- `vendors/EnergyPlus/src/EnergyPlus/api/common.py` — `RealEP` type alias (`c_double`) for float precision
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — sensor/actuator handle API with `get_api_data` returning typed exchange points
- `vendors/EnergyPlus/src/EnergyPlus/api/func.py` — error callback GC-pin and `clear_callbacks()` lifecycle

## Findings

### Finding 1: [Severity: high] ResStock fleet path hardcodes 24-hour simulation with no duration override

**Description**: The `Fleet::from_resstock()` constructor applies a hardcoded `SimulationConfig` that runs only `Duration::hours(24)` (one day). This makes the ResStock fleet loading path effectively incapable of performing annual energy analysis — the primary use case for ResStock data — and provides no mechanism for the caller to override the simulation duration.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:542-560` (`default_resstock_sim_config`)
- `crates/hares-fleet/src/fleet.rs:144` (consumption site: `sim_config: default_resstock_sim_config()`)
- `crates/hares-python/src/py_fleet.rs:127-155` (the `from_resstock` Python binding that passes through to this default)

**Root Cause**: The `from_resstock()` signature accepts `(metadata_path, hpxml_dir, weather_dir, filter, resstock_version)` but has no `sim_config` or `duration` parameter. Every building in the fleet is assigned `default_resstock_sim_config()`, which produces a start time of `2019-01-01T00:00:00Z` and a duration of 24 hours. There is no way to pass `sim_config: Some(SimulationConfig { duration: Duration::hours(8760), .. })` from Python.

**Impact**: A user loading ResStock data via `Fleet::from_resstock()` will silently simulate only one day per dwelling. This is a data-correctness issue: the returned aggregation results will appear valid but represent 1/365th of the intended annual consumption. The `from_buildings()` path is unaffected because callers provide `DwellingConfig` directly.

**Comparison to vendor**: EnergyPlus (via `state.py`) takes an IDF input file that fully specifies the simulation run period. It does not hardcode the duration at the API layer — the configuration is in user-provided input.

**Recommendation**: Add an optional `duration_days: Option<i64>` parameter to `from_resstock()` (and its Python binding). When `Some(d)`, override the duration in the produced `SimulationConfig`. Alternatively, expose a `set_sim_config` or `with_sim_config` builder on `Fleet` to post-hoc mutate simulation parameters on all entries.

---

### Finding 2: [Severity: medium] Fleet is cloned to attach progress callback, incurring O(n) allocation per simulate call

**Description**: To work around PyO3's requirement that `#[pymethods]` use `&self`, `py_fleet.rs:179` clones the *entire* fleet (`self.fleet.clone()`) before installing the progress callback. This deep-copies all `FleetEntry` structs — each containing a `DwellingConfig` with multiple `PathBuf` fields.

**Code Location**:
- `crates/hares-python/src/py_fleet.rs:175-191` — the `simulate` method clones fleet, then calls `py.detach(|| fleet_clone.simulate(thread_count))`

**Root Cause**: The `Fleet` struct's `set_progress` requires `&mut self` (`fleet.rs:176`), but PyO3 `#[pymethods]` take `&self` (immutable borrow). The clone is a workaround for this type-system mismatch.

**Impact**: For a fleet of 10,000 dwellings, each `DwellingConfig` contains at least 3 `PathBuf` fields (hpxml, schedule, weather paths) plus optional paths. The clone allocates tens of megabytes just to attach a callback object. This clone happens on *every* `simulate()` call, not just once.

**Recommendation**: Either (a) change `simulate` to `#[pyo3(signature = (...))]` with `&mut self` (acceptable since simulation is a mutating operation conceptually), or (b) refactor `Fleet` to hold the progress callback behind `Arc<Mutex<Option<ProgressCallback>>>` so it can be set through a shared reference. Option (a) is simpler and matches the EnergyPlus pattern where callbacks are set on a mutable state object before running.

---

### Finding 3: [Severity: medium] No explicit cleanup or shutdown of dwelling resources

**Description**: Neither `Fleet` nor `SteppableFleet` provides a `shutdown()`, `cleanup()`, or `close()` method. The `Dwelling` struct at `dwelling/mod.rs:651-790` holds resources including:
- `StreamingRecorder` (file handle for CSV/Parquet output, line 662)
- `diagnostic_writer: Option<BufWriter<File>>` (line 779)
- Pre-allocated scratch buffers (`thermal_update_buf`, `electrical_update_buf`, etc.)
- Solver memory (`ThermalSolver`, `HumiditySolver`, `ElectricalSolver`, `FluidSolver`)
- `EnvironmentManager` holding weather data parsed from EPW files

These are cleaned up only when the Rust `Drop` implementation runs, which for the `PyFleet`/`PySteppableFleet` Python objects happens when the Python GC collects them. There is no deterministic path for Python users to release file handles or solver memory before the GC runs.

**Code Location**:
- `crates/hares-python/src/py_fleet.rs:80-83` (PyFleet — no `__del__` or explicit cleanup)
- `crates/hares-python/src/py_fleet.rs:353-356` (PySteppableFleet — no `__del__` or explicit cleanup)
- `crates/hares-fleet/src/fleet.rs:267-273` (SteppableFleet — no `Drop` impl or shutdown method)
- `crates/hares-fleet/src/fleet.rs:87-90` (Fleet — no `Drop` impl or shutdown method)
- `crates/hares-core/src/dwelling/mod.rs:792` (Dwelling — no `Drop` impl)

**Impact**: In a long-running Python process that creates and discards multiple fleet instances (e.g., hyperparameter tuning, scenario analysis), file handles may accumulate until the Python GC triggers. The EnergyPlus API comparison is instructive: `state.py:99` provides `delete_state()` to explicitly free C++ memory, and `runtime.py:604` provides `clear_callbacks()` to release GC-pinned callback references. HARES has no equivalent explicit lifecycle management.

**Recommendation**: Add a `shutdown()` method on `PyFleet` and `PySteppableFleet` that (a) drops the inner `Fleet`/`SteppableFleet`, (b) sets the inner state to an empty/config-free sentinel, and (c) logs a warning if called when dwellings are still mid-simulation. If these structs are not reusable after shutdown, wrap the inner value in `Option<Fleet>` and take it in `shutdown`.

---

### Finding 4: [Severity: medium] Progress callback documentation references deprecated Python::with_gil

**Description**: The doc comment on `Fleet::set_progress()` at `fleet.rs:169` advises that "Python bindings should reacquire the GIL inside this callback (for example, using `Python::with_gil`)". The actual Python binding at `py_fleet.rs:182` correctly uses `Python::attach` (appropriate for PyO3 0.28), but the fleet-level documentation is misleading. A developer adding a second Python binding path or embedding the fleet in another context may copy the wrong pattern.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:164-169` (doc comment with `Python::with_gil` reference)
- `crates/hares-python/src/py_fleet.rs:182` (correct usage of `Python::attach`)

**Impact**: Low risk of actual misuse since there is only one Python binding path, but the out-of-date comment is a maintenance hazard.

**Recommendation**: Update the doc comment to reference `Python::attach`.

---

### Finding 5: [Severity: low] Panicked dwellings don't trigger the Python progress callback

**Description**: When a dwelling simulation panics (caught by `catch_unwind`), the fleet code correctly increments the `AtomicUsize` completed counter (`fleet.rs:247`), but does **not** invoke the progress callback. This means the Python progress bar will stall at N-1 if even a single dwelling panics.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:243-253` (panic catch branch: progress counter incremented, but `cb(...)` is never called)
- Contrast with `fleet.rs:237-240` (success branch: `cb(done, total)` is called)

**Root Cause**: The panic handler only increments the atomic counter, not calling the callback. The comment on line 245 says "Increment progress even on panic so the bar reaches 100%", but the counter increment alone doesn't trigger the Python callback — only the success-path code calls `cb(...)`. The Python callback only sees successful completions.

**Impact**: If any dwelling panics, the Python progress indicator (e.g., tqdm) will never reach `total` completions. Users have no way to distinguish "still running" from "one dwelling panicked and progress is stuck." In practice, panics in this codebase are rare, but the invariant is broken.

**Recommendation**: Invoke the progress callback inside the panic branch (after the `fetch_add`), or restructure the code so that `cb()` is called unconditionally after the `fetch_add`, regardless of success or panic:

```rust
let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
if let Some(cb) = &progress {
    cb(done, total);
}
```

---

### Finding 6: [Severity: low] aggregate_timeseries Time column uses Utf8 string instead of Arrow timestamp type

**Description**: The fleet aggregation output schema at `aggregation.rs:386` stores the Time column as `Field::new("Time", DataType::Utf8, false)`. This means timestamps are RFC 3339 strings rather than a native Arrow `Timestamp(Second, Some("UTC"))` type. All downstream consumers (Polars, DuckDB, PyArrow) must parse the string back into a native timestamp, which is both slower and loses type information.

**Code Location**:
- `crates/hares-fleet/src/aggregation.rs:385-386` (schema construction in `build_aggregate_batch`)
- `crates/hares-fleet/src/aggregation.rs:339-340` (`dt.to_rfc3339_opts(SecondsFormat::Secs, true)` to `StringBuilder`)

**Impact**: When Python users call `fleet_results.aggregate_timeseries` (which becomes a Polars DataFrame), the Time column will be a `pl.Utf8` column rather than `pl.Datetime`. The user must call `.str.strptime(...)` to parse it for time-series operations. This adds an extra step and may lead to surprising "all strings" appearance if users don't inspect the schema carefully.

**Recommendation**: Use `DataType::Timestamp(TimeUnit::Second, Some(Arc::new("+00:00")))` and store epoch seconds directly. The `record_batches_to_polars_df` function in `conversions.rs` would then produce a proper `pl.Datetime` column in the resulting DataFrame.

---

### Finding 7: [Severity: low] SteppableFleet enforces homogeneous timing; Fleet::simulate does not

**Description**: `SteppableFleet::from_configs()` at `fleet.rs:341-368` removes any dwelling whose `total_steps` or `time_res_s` differ from the first dwelling. The removed dwellings are reported in `build_errors`. In contrast, `Fleet::simulate()` at `fleet.rs:227-263` runs all dwellings in parallel regardless of timing — each dwelling uses its own `SimulationConfig` independently.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:341-368` (SteppableFleet culling logic)
- `crates/hares-fleet/src/fleet.rs:227-263` (Fleet parallel simulation, no homogeneity check)

**Impact**: Heterogeneous dwellings (different building types, different HPXML files, different run durations) work correctly in batch `Fleet::simulate()` mode but are silently removed in `SteppableFleet` step-by-step mode. Python users switching between the two modes may be surprised when half their dwellings disappear. The heterogeneity is a design feature of the batch mode but a constraint of the synchronous step mode — this trade-off is not documented.

**Recommendation**: Document the timing homogeneity requirement for `SteppableFleet` in its docstring. Consider emitting a `warn!` for each rejected dwelling so the behavior is visible in logs.

---

### Finding 8: [Severity: low] SteppableFleet from_configs builds dwellings sequentially

**Description**: `SteppableFleet::from_configs()` at `fleet.rs:314-329` iterates over dwelling configs in a sequential `for` loop, building each `Dwelling::from_config()` one at a time. HPXML parsing and dwelling initialization can be expensive. For a fleet of 1,000+ dwellings, this serial initialization becomes a startup bottleneck before any parallel stepping begins.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:314-329` (sequential dwelling construction loop)

**Impact**: Cold-start time for a large steppable fleet is dominated by sequential HPXML I/O and building initialization. This is acceptable for small fleets but becomes minutes-long for large ResStock cohorts.

**Recommendation**: Consider parallelizing the `for config in configs` loop using `rayon::par_iter()` since each `Dwelling::from_config()` is independent. The timing validation step (lines 341-368) would then need to run after all builds complete but before culling. Note that `DwellingConfig` implements `Clone`, and `Dwelling::from_config` takes `config` by value — both are compatible with `par_iter`.

---

### Finding 9: [Severity: low] Per-dwelling metrics DataFrame lacks bldg_id column

**Description**: The `dwelling_metrics_to_polars_df` function at `conversions.rs:96-124` builds a Polars DataFrame with columns `annual_energy_kwh`, `peak_power_kw`, `sample_weight`, `status`, and `failed`, but does **not** include `bldg_id`. Without a building identifier, users cannot join per-dwelling metrics back to their input configurations.

**Code Location**:
- `crates/hares-python/src/conversions.rs:96-124` (columns: annual_energy_kwh, peak_power_kw, sample_weight, status, failed — no bldg_id)

**Impact**: The Python user receiving `fleet_results.per_dwelling_metrics` gets a DataFrame with 5 columns. To identify which metric belongs to which dwelling, they must match by position against the input configs list. This is error-prone if configs had filter failures or the fleet silently dropped some dwellings.

**Recommendation**: Add `bldg_id` to `DwellingMetrics` (aggregation.rs:22-28) and include it as an `Int64` column in the DataFrame.

---

## Summary

- **Total findings**: 9
- **High**: 1 (Finding 1 — ResStock 24-hour hardcoded duration)
- **Medium**: 3 (Findings 2–4 — clone-for-progress, no cleanup/shutdown, doc references deprecated GIL API)
- **Low**: 5 (Findings 5–9 — panic skips progress callback, Utf8 timestamps, timing homogeneity, sequential init, missing bldg_id)

## Recommendations

1. **ResStock duration (Finding 1, high)**: Add an optional `duration_days` or `sim_config` override parameter to `Fleet::from_resstock()` and its Python binding. This is the only finding with data-correctness impact.

2. **Fleet clone (Finding 2, medium)**: Either accept `&mut self` on `PyFleet::simulate` or use `Mutex<RwLock<Option<ProgressCallback>>>` for interior mutability. Eliminates O(n) memory allocation per simulate call.

3. **Cleanup lifecycle (Finding 3, medium)**: Add `PyFleet::shutdown()` and `PySteppableFleet::shutdown()` methods that explicitly drop inner state and file handles. Wrap inner fields in `Option<Fleet>` / `Option<Mutex<SteppableFleet>>` so shutdown can take ownership.

4. **Progress callback for panics (Finding 5, low)**: Move `cb(done, total)` to after the atomic fetch, calling it unconditionally for both success and panic paths.

5. **Native Arrow timestamps (Finding 6, low)**: Use `DataType::Timestamp(TimeUnit::Second, Some(Arc::new("+00:00")))` instead of UTF-8 strings for the Time column in aggregate output.

6. **Doc update (Finding 4, medium)**: Correct the `fleet.rs:169` doc comment from `Python::with_gil` to `Python::attach`.

7. **bldg_id column (Finding 9, low)**: Add `bldg_id` to `DwellingMetrics` and the per-dwelling Polars DataFrame.

8. **Parallel init (Finding 8, low)**: Parallelize `SteppableFleet::from_configs()` dwelling construction using `rayon::par_iter()`.

## References / Citations

- PyO3 0.28 GIL management: `Python::attach` is the canonical API for acquiring the GIL from non-Python threads; `Python::with_gil` is retained but not the primary entry point in versions >= 0.22.
- EnergyPlus API lifecycle: `vendors/EnergyPlus/src/EnergyPlus/api/state.py:82-105` — `new_state()`, `reset_state()`, `delete_state()` provide explicit lifecycle management.
- EnergyPlus callback GC pinning: `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py:67` — `all_callbacks = []` global list prevents `CFUNCTYPE`-wrapped Python callbacks from being garbage collected.
- Arrow timestamp types: Native `DataType::Timestamp` supports second, millisecond, microsecond, and nanosecond precision with optional timezone. RFC 3339 string round-trips lose 10–100x query performance in columnar engines.
- ResStock typical configuration: The ResStock 2024.x schema includes a `buildstock.csv` with `*.hpxml` entries intended for annual (8760-hour) simulation. A 24-hour default contradicts this standard use case.
