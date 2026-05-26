# Python tariff binding: URDB parsing, evaluation, billing
**Review ID**: py-binding-06
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_tariff.rs` (740 lines)
- `crates/hares-tariff/src/lib.rs` (19 lines)
- `crates/hares-tariff/src/types.rs` (881 lines)
- `crates/hares-tariff/src/urdb.rs` (933 lines)
- `crates/hares-tariff/src/evaluator.rs` (1679 lines)
- `crates/hares-tariff/src/billing.rs` (818 lines)
- `crates/hares-types/src/schedule.rs` (SeasonFilter, SeasonalSplit, TouPeriod, BillingCycle definitions)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/api.py` — infrastructure only, no tariff logic
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — meter/variable I/O, no tariff logic
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — callback hooks, no tariff logic
- `vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.hh` — C++ header defining `DemandWindow` enum (Quarter, Half, Hour, Day, Week), `BuySell` enum (BuyFromUtility, SellToUtility, NetMetering), `Season` enum (Winter, Spring, Summer, Fall, Annual), and `Cat` enum (EnergyCharges, DemandCharges, ServiceCharges, etc.). Used as reference for how EnergyPlus models demand windows and seasons.

## Findings

### Finding 1: [Severity: high]
**Description**: `SeasonFilter::contains_month()` ignores `SeasonalSplit`, using hardcoded June-September summer/winter boundaries at runtime. The `SeasonalSplit` struct (which supports custom boundaries and wrapping e.g. southern hemisphere Nov-Feb summer) is parsed from URDB data, stored in `ElectricTariff.seasonal_split`, and validated — but is **never consulted during evaluation**.

**Code Location**: `crates/hares-types/src/schedule.rs:878-888` (`SeasonFilter::contains_month()`), `crates/hares-tariff/src/evaluator.rs:98` (evaluator calls `contains_month()`), `crates/hares-tariff/src/billing.rs:288` (billing calls `contains_month()`)

**Root Cause**: `SeasonFilter::contains_month()` hardcodes:
- Summer = months 6..=9 (June through September)
- Winter = months 1..=5 and 10..=12 (October through May)

The evaluator code at `evaluator.rs:96-110` checks `period.season.contains_month(month)` and the tiered billing code at `billing.rs:288` checks `block.season.contains_month(month)`. Neither path reads `self.tariff.seasonal_split` or passes it to `contains_month()`. The `SeasonalSplit::is_summer()` method at `schedule.rs:933-943` correctly handles wrapping ranges but is dead code — unreferenced by any evaluator or billing logic.

**Impact**: Any tariff with a non-standard seasonal boundary (southern hemisphere utilities, utilities with custom summer periods like May-October) will produce incorrect billing results:
- Periods assigned `SeasonFilter::Summer` at parse time (based on the *correct* custom months) will be excluded by `contains_month()` during months outside hardcoded June-September.
- Conversely, `SeasonFilter::Winter` periods will be incorrectly included during months outside hardcoded October-May.
- The URDB parser's `detect_summer_months()` (`urdb.rs:117-150`) correctly handles southern hemisphere inversion (more than 6 months differ from December → invert), but the resulting custom summer months are discarded at evaluation time.

**Fix**: Either (a) change `SeasonFilter::contains_month()` to accept an `Option<SeasonalSplit>` parameter and delegate to `SeasonalSplit::is_summer()` when present, or (b) have the evaluator resolve `SeasonFilter` at evaluation time using `self.tariff.seasonal_split`.

---

### Finding 2: [Severity: high]
**Description**: No standalone `evaluate()` method is exposed in the Python bindings. `PyElectricTariff` provides construction methods (`from_urdb_json`, `from_json`, `from_dict`, `to_dict`), a builder (`TariffBuilder`), `name`, `__repr__`, and `__eq__` — but no way to evaluate the tariff against a load profile from Python. The `TariffEvaluator` struct in `evaluator.rs` is entirely absent from the Python binding.

**Code Location**: `crates/hares-python/src/py_tariff.rs:148-219` (PyElectricTariff definition)

**Root Cause**: The `TariffEvaluator` and `BillingState`/[`BillingPeriodSummary`] types are defined in `hares-tariff` crate but are never wrapped as PyO3 classes. The `PyElectricTariff` struct only holds an `ElectricTariff` data object, not an evaluator. While the evaluator is coupled into the dwelling simulation engine (`py_dwelling.rs:1308` passes a tariff reference, and the simulation runs the evaluator internally), there is no standalone evaluation path exposed to Python users.

**Impact**: Python users cannot perform standalone tariff analysis such as:
- "Given this URDB tariff and this hourly load profile, what's the monthly bill?"
- Comparing two tariffs against the same load profile
- Testing tariff behavior without running a full dwelling simulation

**Fix**: Expose `TariffEvaluator` as a PyO3 class (`PyTariffEvaluator`) with methods for `new(tariff, start, end, interval)`, `step(net_power_kw, dt_seconds, current_time) -> Option<BillingPeriodSummary>`, `finalize(sim_end) -> BillingPeriodSummary`, and accessors. The existing `PyBillingPeriodSummary` in `py_telemetry.rs` could serve as the return type.

---

### Finding 3: [Severity: medium]
**Description**: No idempotency test for tariff evaluation. While the evaluator code is deterministic (no RNG, no timestamp-dependent state beyond the load profile), there are zero tests verifying that creating two evaluators with identical tariffs and load profiles produces identical billing results.

**Code Location**: `crates/hares-tariff/src/evaluator.rs:469-1679` (test module)

**Root Cause**: The test suite covers many scenarios (TOU pricing, demand charges, tiered rates, export modes, minimum charges, DST handling, seasonal boundaries) but none re-runs the same tariff+load combination and asserts identical outputs. The `finalize()` method is idempotent via a `finalized` flag (`evaluator.rs:418-424`), but full-simulation idempotency from a new evaluator is untested.

**Impact**: Without explicit idempotency tests, a future refactor could inadvertently introduce non-deterministic behavior (e.g., caching timestamps, adding a mutable RNG for brownout simulation, adding timezone-aware rounding that differs between calls). Confidence in reproducible billing relies on code review rather than automated verification.

**Fix**: Add a test that creates two `TariffEvaluator` instances with the same tariff, runs the same step sequence, and asserts `BillingPeriodSummary` fields are bitwise equal. Pattern: test the flat-rate tariff with a full year of steps, collect all summaries from both evaluators, and assert equality.

---

### Finding 4: [Severity: medium]
**Description**: `PyGasTariff` is missing a `from_urdb_json` method, despite `PyElectricTariff` having one. The UX is asymmetric: electric tariffs can be loaded from URDB JSON files, but gas tariffs cannot. Furthermore, there is no URDB gas tariff parser in `hares-tariff` at all.

**Code Location**: `crates/hares-python/src/py_tariff.rs:568-629` (PyGasTariff impl, missing `from_urdb_json`), `crates/hares-tariff/src/urdb.rs` (only `parse()` for `ElectricTariff`)

**Root Cause**: The URDB parser (`urdb.rs:386-618`) only returns `ElectricTariff`. Gas tariffs can only be constructed via the builder API (`from_dict`/`from_json`/`PyGasTariffBuilder`). URDB does contain gas rate structures in the same JSON format (same `energyratestructure`, `energyweekdayschedule` fields), but gas-specific fields like `fixedchargeunits` in therms are not handled.

**Impact**: Users with URDB gas tariff data must manually convert it to the HARES JSON format before loading. This is a usability gap for combined electric+gas utility billing.

**Fix**: Implement `parse_gas()` in `urdb.rs` analogous to `parse()` but producing a `GasTariff` from the same schedule/rate structures, and expose `PyGasTariff::from_urdb_json()` in `py_tariff.rs`.

---

### Finding 5: [Severity: low]
**Description**: `DemandWindow` floating-point drift correction runs every 1000 pushes, which may be insufficient for multi-year simulations at fine granularity.

**Code Location**: `crates/hares-tariff/src/billing.rs:41-43`

**Root Cause**: The `running_sum` is maintained incrementally (subtract evicted sample, add new sample) to avoid O(n) recomputation. Float rounding error accumulates each push. The periodic resync (`running_sum = samples.iter().sum()`) at `push_count % 1000 == 0` corrects this, but a 5-minute simulation over 5 years produces ~525,600 pushes — only ~526 recorrections. Between corrections, the error could accumulate: a 60-sample window (60-minute demand, 1-min interval) experiences ~1000 floating-point additions between corrections, each with potential ~1 ULP error. In practice the error is negligible for billing (well below $0.01), but for regulatory-grade reporting it may be worth running more frequent recorrections.

**Impact**: Negligible for residential billing (errors on the order of 1e-12 * sample_count), but could accumulate for long simulations with large sample windows.

**Fix**: Consider recorrecting every 100-200 pushes, or using a compensated summation (Kahan algorithm) for `running_sum`.

---

### Finding 6: [Severity: low]
**Description**: URDB parser silently defaults to `ExportMode::None` for unrecognized `dgrules` field values, without emitting a warning.

**Code Location**: `crates/hares-tariff/src/urdb.rs:333-351` (`parse_export_mode()`)

**Root Cause**: The match on `dgrules` handles "Net Metering", "Net Billing Instantaneous"/"Net Billing Hourly", and "Buy All Sell All", but the wildcard arm `_ => ExportMode::None` silently discards any new or unexpected URDB export rules. The `warn_unsupported()` function at line 374-380 only covers `reactivepowercharge`, `voltagecategory`, and `phasewiring` — it does not include `dgrules`.

**Impact**: If URDB adds a new export mode (e.g., "Net Billing Monthly"), HARES will silently treat it as no export compensation, potentially understating export credits in billing results.

**Fix**: Add a `tracing::warn!` in the wildcard arm of `parse_export_mode()` when `dgrules` is present but unrecognized. Optionally add `dgrules` to the `warn_unsupported()` list for unrecognized values.

---

## Summary
- **Total findings**: 6
- **Critical**: 0
- **High**: 2 (SeasonalSplit ignored at runtime, no standalone evaluate() in Python bindings)
- **Medium**: 2 (no idempotency test, missing gas URDB parsing)
- **Low**: 2 (DemandWindow drift recorrection interval, silent fallback for unknown export modes)

## Recommendations

1. **Fix SeasonalSplit evaluation** (Finding 1): Modify `SeasonFilter::contains_month()` to accept `Option<SeasonalSplit>` and delegate to `SeasonalSplit::is_summer()` when a custom split is present. Update all call sites in `evaluator.rs` and `billing.rs` to pass `self.tariff.seasonal_split`. This is a one-line change in each call site and a small refactor of `contains_month()`.

2. **Expose TariffEvaluator to Python** (Finding 2): Implement a `PyTariffEvaluator` PyO3 class wrapping `TariffEvaluator`, with `step()` and `finalize()` methods. This enables standalone tariff analysis without requiring a full dwelling simulation. The `PyBillingPeriodSummary` type already exists in `py_telemetry.rs` and can be reused.

3. **Add idempotency regression test** (Finding 3): Write a test creating two `TariffEvaluator` instances with identical tariffs, running identical load profiles, and asserting bitwise-equal `BillingPeriodSummary` outputs. This protects against future non-deterministic changes.

4. **Add gas URDB parsing** (Finding 4): Implement `parse_gas()` in `urdb.rs` and expose `PyGasTariff::from_urdb_json()`. Gas rates in URDB use the same schedule/rate structure with therms as the energy unit.

5. **Improve DemandWindow precision** (Finding 5): Reduce the periodic recorrection interval from 1000 to 100 or use Kahan compensated summation for `running_sum`. The performance cost is negligible.

6. **Warn on unrecognized export modes** (Finding 6): Add `tracing::warn!` for unrecognized `dgrules` values in `parse_export_mode()`.

## Positive Observations

The codebase demonstrates strong architecture:

- **URDB parsing is robust**: Handles 12-month × 24-hour schedule matrices, seasonal detection with southern hemisphere support via inversion heuristic, union-based hour range merging for multi-month periods, and clean separation of weekday/weekend schedules. The `detect_summer_months()` + `seasonal_split_from_months()` pipeline is elegant.

- **Demand calculation is correct**: The `DemandWindow` ring buffer implements true rolling-window averaging (not instantaneous peak). The `demand_window_minutes` parameter with validation against simulation interval ensures correct configurability. The evaluator correctly rejects incompatible window/interval combinations (`evaluator.rs:191-198`).

- **Billing arithmetic is correct**: Net bill = energy + demand + fixed − export credit, with minimum charge floor applied at the right point based on `minimum_charge_excludes_export`. Tiered rates use proper blended-cost computation across tier boundaries. Per-period demand peaks for TOU demand rates are tracked independently.

- **Edge cases are handled**: Zero consumption produces zero variable charges (test at `evaluator.rs:1505-1528`). Negative net consumption correctly splits into import/export components (`billing.rs:171-176`). Net metering, net billing, and flat-rate export are all supported. Custom billing cycles work with ratchet lookback truncation.

- **Validation is thorough**: Every struct has a `validate()` method with comprehensive checks (finite rates, non-negative values, strictly ascending thresholds, period name cross-references, TOU window validity, demand window range). URDB parsing ends with `tariff.validate()` as a safety net.

- **Test coverage is excellent**: 80+ tests covering TOU pricing, seasonal boundaries, DST transitions, demand charges with ratchets, tiered block rates, export modes, minimum charges, custom billing cycles, edge cases, and error conditions.

## References / Citations

- EnergyPlus `EconomicTariff.hh:103-112` — Defines `DemandWindow` enum with Quarter, Half, Hour, Day, Week options. HARES's configurable `demand_window_minutes` parameter (5-60 minutes, validated at `types.rs:299-303`) covers the relevant residential demand windows (5, 15, 30, 60 min) but lacks Day/Week windows for commercial tariffs.
- EnergyPlus `EconomicTariff.hh:114-121` — Defines `BuySell` enum with `NetMetering` as a first-class mode. HARES matches this with `ExportMode::NetMetering` and extends it with `NetBilling`, `FlatRate`, and `None`.
- EnergyPlus `EconomicTariff.hh:124-135` — Defines `Season` enum with Winter, Spring, Summer, Fall, Annual. HARES simplifies to Summer/Winter/All with a configurable `SeasonalSplit` (though the SeasonalSplit integration bug in Finding 1 means this configuration is currently ineffective).
- URDB API v7 specification: schedule matrices (12×24), rate structures (periods → tiers with rate/adj/max/sell fields), optional demand structures, and export rules via `dgrules`. HARES's parser handles all required fields and gracefully tolerates unknown optional fields.
