# Output metrics computation: aggregation, resampling, correctness
**Review ID**: output-03
**Category**: output
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/output/metrics.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Analysis.py`

## Findings

### Finding 1: [Severity: critical]
**Description**: NaN values silently contaminate all accumulators because `value_at` only checks Arrow `is_null()`, not floating-point NaN.

**Code Location**: `crates/hares-io/src/output/metrics.rs:940-946` (`value_at`) and every call site in `accumulate()` (lines 519-682).

**Root Cause**: The `value_at` helper uses Arrow's `is_null()` to detect null entries, but IEEE 754 NaN is a non-null bit pattern in Arrow (`Float64Array::is_null` returns `false` for NaN). A simulation timestep producing NaN output (e.g., from a division-by-zero in thermal calculations) passes through `value_at` as `Some(f64::NAN)` and is then multiplied by `self.timestep_h`. Since `NaN * x = NaN` and `accumulator += NaN = NaN`, a single NaN poisons every downstream accumulator — annual energy totals, end-use sums, envelope loads, efficiency ratios, comfort/unmet counts. The NaN propagates silently with no error, warning, or aggregation-period flag.

**Impact**: A single NaN timestep makes all `accumulate()`-based results mathematically invalid. There is zero defense-in-depth. Compare OCHRE's `calculate_metrics()` at `vendors/OCHRE/ochre/Analysis.py:332`, which calls `sum(skipna=False)` — this causes a loud NaN in the result dict so callers know the computation is suspect.

**Vendor Comparison**: OCHRE explicitly propagates NaN via `skipna=False` on `DataFrame.sum()` and `DataFrame.mean()` (lines 332, 350, 388–389, 408, 438, 444, 449, 459, 467, 472–473). This guarantees that contaminated timesteps produce visible NaN metrics rather than silently corrupted numbers.

---

### Finding 2: [Severity: high]
**Description**: `continue` inside the setpoint block skips envelope load, HVAC thermal, HVAC electric, and battery accumulation for rows with missing setpoint or zone temperature data.

**Code Location**: `crates/hares-io/src/output/metrics.rs:564` and `:567`

**Root Cause**: Inside the `for row in 0..batch.num_rows()` loop, the `continue` statements for missing heating/cooling setpoint values (`None`) apply to the enclosing `for` loop, not just the `if let` block. This means that when any row in a batch has a `None` heating or cooling setpoint value, all post-setpoint processing for that row is skipped:
  - Envelope component load accumulation (lines 603–643)
  - HVAC thermal delivered (lines 645–655)
  - HVAC electric power for COP (lines 657–671)
  - Battery energy tracking (lines 673–682)

The same issue applies for rows with non-finite or negative deadband values (line 577: `if !deadband_c.is_finite() || deadband_c < 0.0 { continue; }`). Note that this `continue` at line 577 is **below** the deadband validation but **above** the comfort/unmet logic — it skips everything below it for the current row.

**Impact**: Energy totals, COP ratios, and envelope loads are under-reported whenever setpoint data has gaps, deadband columns have invalid rows, or zone temperature telemetry has nulls. The miss is silent — column values exist for other rows, so totals are simply lower than correct. This is especially damaging because `value_at` returning `None` for a setpoint column can be a transient data quality issue, not a permanent schema change.

---

### Finding 3: [Severity: high]
**Description**: No leap year awareness — "annual" energy totals label all accumulated energy as annual regardless of simulation duration.

**Code Location**: `crates/hares-io/src/output/metrics.rs:714` and the `AnnualEnergyKwh` struct definition at `:43-49`.

**Root Cause**: The `MetricsCalculator` is a streaming sum-of-all-data accumulator. It has no concept of simulation start time, end time, or duration in its computation logic. The `SimulationConfig` passed to `new()` contains `start_time` (a `DateTime<FixedOffset>`) and `duration` (a `chrono::Duration`) at `config.rs:45-48`, but these fields are only used to discover the `time_res` and deadband configuration — the year and duration are never checked against the accumulated data to determine whether it represents a full year, a leap year (8784 hours), a partial year, or an overrun of multiple years.

The test at line 1001 (`annual_energy_total_matches_8760_constant_load`) hardcodes 8760 hours and passes with expected 8760 kWh, but there is no test for a leap year (8784 hours), a short simulation, or a multi-year run. If a user runs 8784 hours in a leap year, the result is still called `annual_energy_kwh` but reflects 8784 kWh of energy, not normalized to a standard year.

**Impact**: Leap year simulations produce 0.27% inflated energy totals vs. non-leap years (24 extra hours / 8760 hours). Across a portfolio of 1000 buildings, this introduces a systematic bias. Multi-year simulations silently aggregate all years into a single "annual" value. Partial-year simulations report fractional-year energy as "annual" with no indication of incompleteness.

**Vendor Comparison**: OCHRE/EnergyPlus processes exactly 8760 hourly rows for a non-leap year and enforces this count (`Analysis.py:188-189`: `if len(df) != 8760: raise OCHREException(...)`). The enforced row count guards against partial/incomplete data. HARES has no equivalent guard.

---

### Finding 4: [Severity: medium]
**Description**: No monthly aggregation — only annual totals are computed, with no monthly or sub-annual breakdown.

**Code Location**: `crates/hares-io/src/output/metrics.rs:136-148` (`SimulationMetrics` struct lacks monthly fields)

**Root Cause**: The `MetricsCalculator` is a flat accumulator: `total_electric_energy_kwh` (line 306), `energy_by_end_use` (line 312), etc. There is no `[f64; 12]` monthly accumulator, no month-indexed BTreeMap, and no logic to bin timesteps by month using `config.start_time`. The `AnnualEnergyKwh` struct (line 43-49) has only `total` and `per_end_use` — no per-month breakdown.

**Impact**: Residential energy analysis commonly requires monthly utility bill comparisons, monthly net metering reports, and seasonal COP analysis. Without monthly aggregation, a second pass over the full timeseries output is required, doubling I/O. Time-of-use and seasonal diagnostics (e.g., "why is January heating COP lower than March?") are impossible from metrics alone.

---

### Finding 5: [Severity: medium]
**Description**: Peak demand initialization is inconsistent — `peak_by_end_use` starts at `f64::NEG_INFINITY` while `peak_import_kw` and `peak_export_kw` start at `0.0`.

**Code Location**: `crates/hares-io/src/output/metrics.rs:411` vs `:441-442`

**Root Cause**: End-use peaks use the standard sentinel pattern (`f64::NEG_INFINITY` → any real value replaces it, then in `finish()` sentinels are clamped to `0.0` at lines 689-694). Grid interaction peaks (`peak_import_kw`, `peak_export_kw`) are initialized to `0.0`. This works but is inconsistent. More critically, if a simulation has only positive grid import (no export), `peak_export_kw` remains at `0.0` which is mathematically correct but ambiguous — the consumer cannot distinguish "peak export is zero" from "no export data was ever processed."

**Impact**: Low practical impact. A simulation with exclusively negative grid power (net export only, no import) would report `peak_import_kw = 0.0`, which is correct. However, a consumer receiving metrics from a run with no grid power column at all (e.g., a gas-only simulation) would also see `peak_import_kw = 0.0` — there's no discriminator. Using `Option<f64>` or a NaN sentinel would allow downstream code to distinguish "no data" from "peak is zero."

---

### Finding 6: [Severity: low]
**Description**: No sub-hourly to hourly resampling of energy quantities in the metrics calculator.

**Code Location**: `crates/hares-io/src/output/metrics.rs` — entire file (absence of resampling logic)

**Root Cause**: The `MetricsCalculator` operates at the simulation's native timestep (e.g., 10 minutes). Energy integration at line 521 (`total_kw * self.timestep_h`) is correct for energy accumulation at the native resolution. However, there is no post-process step to resample the aggregated energy timeseries to hourly resolution for standard reporting. The `RollingPeakBuffer` (line 186-274) does correctly compute sub-hourly rolling averages for demand peaks (15min, 30min, 60min) per ASHRAE and utility conventions — that part is well-implemented.

**Impact**: External consumers of the output CSV/Parquet must perform their own hourly resampling. The energy accumulation arithmetic itself is correct (time-weighted via multiplication by `timestep_h`), so this is a feature gap rather than a correctness bug.

---

### Finding 7: [Severity: low]
**Description**: Test coverage of the `continue`-skips-accumulation bug from Finding 2 is missing — no test verifies that envelope loads still accumulate when a row's setpoint value is null.

**Code Location**: Tests section at `crates/hares-io/src/output/metrics.rs:948-1445` — no test exercises null setpoint values

**Root Cause**: All existing comfort/unmet tests (`comfort_hours_equals_duration_when_all_inside_deadband` at line 1038, `unmet_load_hours_count_rows_at_capacity_and_outside_setpoint` at line 1113) use fully-populated Float64Arrays with no nulls. The `build_batch` helper at line 978 creates arrays with `Field::new(*name, DataType::Float64, false)` — the `false` parameter means "not nullable," so Arrow-level nulls are impossible in tests. This masks the `continue`-skips-accumulation bug.

**Impact**: The gap between nullable-column support in the schema validation code and non-nullable test data means this regression would only surface in production with real telemetry.

---

## Summary
- **Total findings**: 7
- **Critical**: 1 (NaN silent contamination)
- **High**: 2 (continue-skips-accumulation, no leap year awareness)
- **Medium**: 2 (no monthly aggregation, inconsistent peak initialization)
- **Low**: 2 (no hourly resampling, missing null test coverage)

## Recommendations

1. **Add NaN guard in `value_at`**: After the `is_null()` check, add `if v.is_nan() { return None }` so NaN values are treated identically to nulls. Alternatively, follow OCHRE's pattern and propagate NaN through the accumulators explicitly with a warning log. Either way, the current silent contamination is unacceptable.

2. **Fix `continue` scope in the setpoint block**: Replace `continue` with `{ }` (empty block body) or restructure the loop so that missing setpoint values only skip comfort/unmet calculations, not envelope and HVAC energy accumulation. Move the envelope/HVAC/battery accumulation above the setpoint block, or use a `'row:` label with targeted `continue`.

3. **Add leap year and duration awareness**: Use `config.start_time` and `config.duration` to (a) validate that the simulation covers exactly one calendar year (8759–8784 hours depending on leap status) or (b) normalize totals to a standard 8760-hour year when the simulation spans a leap year. At minimum, document that `AnnualEnergyKwh` represents total energy over the simulation period regardless of its length, and rename to `TotalEnergyKwh` if annualization is not guaranteed.

4. **Add monthly aggregation**: Implement a `[f64; 12]` monthly energy accumulator indexed by `config.start_time.month() + (step_index * timestep_h) % (365/366 days)`. This enables monthly utility bill comparisons without a second data pass.

5. **Standardize peak initialization**: Initialize `peak_import_kw` and `peak_export_kw` to `f64::NEG_INFINITY` and clamp to `0.0` in `finish()`, consistent with `peak_by_end_use`. Consider `Option<f64>` to distinguish "no data" from "peak is zero."

6. **Add nullable test scenarios**: Modify `build_batch` (line 978) or add a `build_nullable_batch` helper that creates `Field::new(*name, DataType::Float64, true)` with `Float64Array::from(vec![Some(1.0), None, Some(3.0)])` to verify correct null/NaN handling, including the case where setpoint columns have nulls and envelope loads must still accumulate.

## References / Citations

- OCHRE `calculate_metrics()`: `vendors/OCHRE/ochre/Analysis.py:304-575` — reference implementation for metric aggregation unit semantics and `skipna=False` guard.
- OCHRE `get_agg_func()`: `vendors/OCHRE/ochre/Analysis.py:85-94` — unit-based aggregation dispatch (sum for kWh/therms, mean for temperatures).
- OCHRE hourly resampling: `vendors/OCHRE/ochre/Analysis.py:121` — `df.resample(resample_res).mean(numeric_only=True)` for time-weighted average resampling.
- OCHRE enforced row count: `vendors/OCHRE/ochre/Analysis.py:188` — `if len(df) != 8760: raise OCHREException(...)` — validates full-year data completeness.
- HARES `value_at`: `crates/hares-io/src/output/metrics.rs:940-946` — only checks Arrow null, not NaN.
- HARES setpoint `continue`: `crates/hares-io/src/output/metrics.rs:561-601` — `continue` skips all downstream accumulation.
- HARES `RollingPeakBuffer`: `crates/hares-io/src/output/metrics.rs:186-274` — correctly handles sub-hourly rolling demand averages.
- HARES `SimulationConfig` time fields: `crates/hares-io/src/config.rs:45-54` — `start_time` and `duration` available but unused for year validation.
