# Python metrics bindings: annual totals, monthly summaries, peak demand, LDCs, energy cost, unit consistency
**Review ID**: pygap-04
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_metrics.rs` — Python bindings for `SimulationMetrics` and all sub-types (329 lines)
- `crates/hares-io/src/output/metrics.rs` — Rust-side `MetricsCalculator`, accumulators, and `RollingPeakBuffer` (1445 lines)
- `crates/hares-python/src/py_telemetry.rs` — `BillingPeriodSummary`, `TariffTelemetry` (228 lines)
- `crates/hares-python/src/py_dwelling.rs:1157–1183` — `Dwelling.metrics()` wiring
- `python/ochre_next/_hares.pyi:1482–1574` — Type stubs for all metrics classes
- `tests/python/test_py_metrics.py` — Python integration tests (171 lines)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: No NaN / infinite guards on telemetry power values during accumulation
**Severity**: high

**Description**: The `MetricsCalculator::accumulate` method (line 519–683 of `metrics.rs`) reads `f64` values from Arrow arrays and accumulates energy (`total_kwh * timestep_h`) without checking whether the source value is finite. If the simulation produces a `NaN` or `+Inf` power value at any timestep (e.g. from solver divergence, uninitialised state, or a physics edge case), that NaN/Inf propagates silently into every accumulator:
- `total_electric_energy_kwh`
- `energy_by_end_use`
- `peak_by_end_use`
- `peak_import_kw` / `peak_export_kw`
- `rolling_peak` buffer
- envelope component accumulators (Wh)
- HVAC electric accumulators for COP
- battery energy accumulators
- comfort/unmet step counters

The `finish()` method (line 688) then materialises these poisoned accumulators into the final `FullSimulationMetrics`, which the Python bindings expose as-is. Downstream plotting tools (matplotlib, plotly) silently render NaN as blank/empty areas.

**Code Location**: `crates/hares-io/src/output/metrics.rs:519–683` (`accumulate` loop body). The only finite check in the entire file is at line 576 for `deadband_c` — no corresponding check for power values.

**Root Cause**: The design assumes Arrow columns are always clean, but NaN/Inf can arise from ODE solver divergence, uninitialised equipment state, or numerical overflow in the physics layer. The `OptionalFloat64Array` wrapper (`value_at`, line 940) correctly returns `None` for null sentinels but passes through all valid IEEE 754 bit patterns without inspection.

**Impact**: Silent NaN output makes metrics appear blank/missing in visualisations. Users cannot distinguish "no data computed" from "computation produced invalid results."

---

### Finding 2: Monthly summaries not implemented
**Severity**: medium

**Description**: The review scope explicitly calls for monthly energy summaries (total and per-end-use kWh broken down by calendar month). Neither the Rust `MetricsCalculator` nor the Python binding layer provides monthly aggregation. The `MetricsCalculator` accumulates a single set of totals over all batches; there is no per-month accumulator, no month-boundary detection, and no `SimClock` awareness inside the calculator.

**Code Location**: Absent from `crates/hares-io/src/output/metrics.rs` and `crates/hares-python/src/py_metrics.rs`.

**Root Cause**: Feature not yet implemented. The Arrow batches contain timestamps (available via the `SimClock` or a time column), but the calculator does not consume or require them.

**Impact**: Python users must post-process the full time-series output in pandas/polars to produce monthly summaries, duplicating work that belongs in the metrics layer.

---

### Finding 3: Load duration curves not implemented
**Severity**: medium

**Description**: The review scope calls for load duration curves (LDC) — sorting all timestep loads from highest to lowest and plotting against percentage of time (0–100%). Neither the Rust calculator nor the Python bindings provide an LDC function. No sorting of time-series data occurs in the metrics layer.

**Code Location**: Absent from `crates/hares-io/src/output/metrics.rs` and `crates/hares-python/src/py_metrics.rs`.

**Root Cause**: Feature not yet implemented.

**Impact**: Users must fetch the full time-series DataFrame and sort manually — but the data must first be dumped to file (Parquet/CSV) and loaded back via polars, since the in-memory Arrow batches are not directly exposed.

---

### Finding 4: `AnnualEnergyKwh.total` is net energy, not gross consumption
**Severity**: medium

**Description**: The `AnnualEnergyKwh.total` field accumulates `total_kw * timestep_h` for every row regardless of sign (`metrics.rs:521`). The column `"Total Electric Power (kW)"` tracks net electric power (consumption minus generation). For a dwelling with PV, this sum can approach zero because import and export cancel. The Rust doc comment says "Total annual electric energy (kWh)" and the Python `__repr__` shows `AnnualEnergyKwh(total=XXX kWh)` — both suggest gross consumption, not net throughput.

The Rust calculator separately tracks `total_consumption_kwh` (line 522–524, only positive values) and `total_pv_generation_kwh_abs` (line 543–546, abs of negative PV), but these are not exposed in the `AnnualEnergyKwh` struct or in the Python bindings. Similarly, `FullSimulationMetrics::combined_annual_energy_kwh()` (line 177) is a Rust-only convenience that has no Python getter.

**Code Location**:
- Accumulation: `crates/hares-io/src/output/metrics.rs:521`
- Struct definition: `crates/hares-io/src/output/metrics.rs:44–49`
- Python binding: `crates/hares-python/src/py_metrics.rs:16–30`
- Missing exposure: `total_consumption_kwh` at line 308, `total_pv_generation_kwh_abs` at line 309, `combined_annual_energy_kwh()` at line 177

**Root Cause**: The semantics of "total" were not clarified for the bidirectional-power case. The `per_end_use` values are safe because end-use power columns are consumption-only (positive), but the total column can be negative.

**Impact**: A user simulating a solar-powered dwelling gets `annual.total ≈ 0 kWh`, which looks like a simulation failure when it actually indicates a well-balanced system. The information needed to compute gross consumption (total_consumption_kwh) and gross generation (total_pv_generation_kwh_abs) exists internally but cannot be retrieved.

---

### Finding 5: Rolling peak window overestimates duration when timestep doesn't evenly divide 15/30 minutes
**Severity**: low

**Description**: `RollingPeakBuffer::new` (line 211–228) computes window sizes with `ceil(0.25/timestep_h)`, `ceil(0.5/timestep_h)`, and `ceil(1.0/timestep_h)`. When the timestep does not evenly divide the window, the `ceil` rounds up — making the window longer than the nominal duration.

Example with 4-minute timesteps (timestep_h = 0.0667 h):
- 15-min window: `ceil(0.25/0.0667)` = `ceil(3.75)` = **4 steps = 16 minutes** (7% overestimate)
- 30-min window: `ceil(0.5/0.0667)` = `ceil(7.5)` = **8 steps = 32 minutes** (7% overestimate)
- 60-min window: `ceil(1.0/0.0667)` = `ceil(15.0)` = **15 steps = 60 minutes** (exact)

Because the rolling peak tracks the *maximum* average, a longer window produces a *lower* peak (more smoothing), potentially undercounting the true 15-minute demand by several percent. The tariff evaluator (`billing.rs` demand window) converts the window to an exact count of steps and validates divisibility at construction time (`evaluator.rs:189–198`), making it more precise than the metrics calculator.

**Code Location**: `crates/hares-io/src/output/metrics.rs:214–216`

**Root Cause**: No fractional-weight or exact-ratio approach. A correct implementation would use the closest integer count (round, not ceil) or assign fractional weight to the edge timestep.

**Impact**: Minor demand peak inaccuracy for non-standard timesteps. Standard timesteps (1 min, 5 min, 15 min, 30 min, 60 min) divide evenly and are unaffected.

---

### Finding 6: `THERMS_TO_KWH` constant slightly inconsistent with `GAS_THERMS_PER_HOUR_TO_W`
**Severity**: low

**Description**: Two different therm→energy conversion constants exist in the codebase:
- `GAS_THERMS_PER_HOUR_TO_W = 29_307.107_017_222_2` in `crates/hares-physics/src/constants.rs:139` (NIST SP 811: 1 BTU_IT = 1055.05585262 J)
- `THERMS_TO_KWH = 29.3001` in `crates/hares-io/src/output/metrics.rs:28`

The physics constant (`29_307.107… W per therm/hour`) is equivalent to 29.3071 kWh per therm. The metrics constant (29.3001) differs by ~0.024%. A dwelling consuming 1000 therms/year would have gas kWh-equivalent reported as 29,300 kWh instead of 29,307 kWh — a 7 kWh difference, well within simulation uncertainty but avoidable.

**Code Location**:
- `crates/hares-io/src/output/metrics.rs:28`
- `crates/hares-physics/src/constants.rs:139`

**Root Cause**: The metrics constant predates the physics constant and was likely computed from an older conversion factor (possibly 1 therm = 29.3001 kWh from an earlier reference like 3412 BTU/kWh).

**Impact**: Gas energy metrics in kWh-equivalent (exposed as `GasEnergyMetrics.total_kwh_equivalent`) are ~0.024% low. Not meaningful for building energy analysis but worth aligning for precision.

---

### Finding 7: No Python exposure of `total_consumption_kwh`, `total_pv_generation_kwh_abs`, or `combined_annual_energy_kwh`
**Severity**: low

**Description**: The Rust `FullSimulationMetrics` struct contains three fields/methods computed during `finish()` that have no corresponding Python getters:
- `total_consumption_kwh` — gross electric consumption (positive-only total power integral)
- `total_pv_generation_kwh_abs` — gross PV generation (abs of negative PV power integral)
- `combined_annual_energy_kwh()` — electric kWh + gas kWh equivalent

These are computed at `metrics.rs:308–309` and `metrics.rs:177–183`, but neither `PyAnnualEnergyKwh` nor `PySimulationMetrics` exposes them. The Python `.pyi` stubs also omit them.

**Code Location**: `crates/hares-python/src/py_metrics.rs:246–328` (missing getters on `PySimulationMetrics`)

**Root Cause**: The Python bindings were written to mirror the initial `SimulationMetrics` struct shape; the later-added `FullSimulationMetrics` convenience fields were not propagated.

**Impact**: Python users who need gross consumption, PV generation totals, or combined fuel energy must re-derive these from per-end-use sums or post-processing, which is error-prone.

---

### Finding 8: Energy cost not exposed through `SimulationMetrics`
**Severity**: low

**Description**: The `SimulationMetrics` object (and its Python wrapper) exposes annual energy, peak demand, efficiency, and comfort — but not energy cost (USD). Cost data is tracked separately in the tariff subsystem (`TariffEvaluator`, exposed as `TariffTelemetry` and per-period `BillingPeriodSummary` objects). The review asks whether the metrics layer re-computes cost independently; it does not — there is no duplicate cost pathway, so there is no discrepancy risk. However, a user calling `dw.metrics()` expecting "total cost" will not find it there.

**Code Location**: Cost is absent from `crates/hares-io/src/output/metrics.rs` (all structs) and `crates/hares-python/src/py_metrics.rs`. It is available via `TariffTelemetry.cumulative_energy_cost_usd` (`py_telemetry.rs:202`) and `BillingPeriodSummary.net_bill_usd` (`py_telemetry.rs:119`).

**Root Cause**: Intentional separation of concerns — metrics handles physics/post-processing aggregation; tariff handles financials. The cost data is correct in the tariff subsystem and is not duplicated.

**Impact**: Users must retrieve cost from a different accessor (`TariffTelemetry` or billing summaries) rather than from `metrics()`. Not a correctness issue, but a discoverability concern.

---

## Summary
- **Total findings**: 8
- **Critical**: 0
- **High**: 1 (NaN propagation risk)
- **Medium**: 3 (missing monthly summaries, missing LDC, net-vs-gross energy semantics)
- **Low**: 4 (rolling peak window approximation, therm constant inconsistency, missing Python getters, cost discoverability)

### Areas without findings
- **(b) Monthly summaries**: Not implemented (Finding 2). No month-boundary off-by-one errors to evaluate since there's no code.
- **(e) Energy cost**: No duplicate calculation risk (Finding 8). Cost is computed once in the tariff evaluator, correctly.
- **(f) Unit consistency**: No J-to-kWh conversion factor (3.6e6) exists in the metrics pathway. The internal convention is kW and kWh throughout (kW × hours = kWh; W × hours = Wh, ÷1000 = kWh). All conversions are correct.
- **(h) Return types**: The `.pyi` stub file (`python/ochre_next/_hares.pyi:1482–1574`) correctly types all 9 metrics classes with proper `float | None` for optionals and `dict[str, float]` for BTreeMap fields. Mypy/pyright will not flag these.

## Recommendations
1. **NaN guard (high priority)**: Add `is_finite()` checks on power values before accumulation, and either skip the timestep, flag the result, or return an error. At minimum, document that NaN in the output telemetry means the metrics are unreliable.
2. **Expose gross consumption and PV generation**: Add `total_consumption_kwh` and `total_pv_generation_kwh_abs` getters to `PyAnnualEnergyKwh` (or `PySimulationMetrics`), and expose `combined_annual_energy_kwh()`.
3. **Clarify `AnnualEnergyKwh.total` semantics**: Either rename to `net_energy_kwh` or add a separate `gross_consumption_kwh` field to disambiguate.
4. **Align `THERMS_TO_KWH`**: Replace `29.3001` with `29.3071070172222` to match `GAS_THERMS_PER_HOUR_TO_W / 1000.0` (the kW per therm/h constant divided by 1000 to get kWh per therm, since 1 kW × 1 h = 1 kWh).
5. **Implement monthly summaries and LDC**: These are feature gaps. Monthly summaries require timestamp-aware bucketing inside `MetricsCalculator` (or a post-processing step on flushed batches). LDC requires retaining or re-reading the per-timestep load values and sorting them.
6. **Improve rolling peak window precision**: For non-divisible timesteps, use the closest integer count (rounding) and apply a fractional weight to the edge value, or validate that the simulation timestep evenly divides 15 minutes at configuration time.

## References / Citations
- `MetricsCalculator::accumulate`: `crates/hares-io/src/output/metrics.rs:475–683`
- `MetricsCalculator::finish`: `crates/hares-io/src/output/metrics.rs:688–774`
- `RollingPeakBuffer::new`: `crates/hares-io/src/output/metrics.rs:211–228`
- `RollingPeakBuffer::push`: `crates/hares-io/src/output/metrics.rs:231–249`
- `FullSimulationMetrics` definition: `crates/hares-io/src/output/metrics.rs:154–184`
- Python metrics bindings: `crates/hares-python/src/py_metrics.rs:1–329`
- Type stubs: `python/ochre_next/_hares.pyi:1482–1574`
- Integration tests: `tests/python/test_py_metrics.py:1–171`
- Tariff cost pathway: `crates/hares-python/src/py_telemetry.rs:107–228`
- Physics constant: `crates/hares-physics/src/constants.rs:139` (`GAS_THERMS_PER_HOUR_TO_W`)
- NIST SP 811: 1 BTU_IT = 1055.05585262 J; 1 therm_US = 100,000 BTU_IT
