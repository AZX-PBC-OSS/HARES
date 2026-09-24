# BillingState energy/demand accumulation: kWh, demand peaks, fixed charges, tier logic, period resets
**Review ID**: tariffdp-01
**Category**: tariff-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-tariff/src/billing.rs` (818 lines)
- `crates/hares-tariff/src/evaluator.rs` (caller context, 1679 lines)
- `crates/hares-tariff/src/types.rs` (data model context)
- `crates/hares-types/src/schedule.rs` (BillingCycle, SeasonFilter)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.cc` (4882 lines)
  - `GatherForEconomics()` at line 2527 — demand window accumulation, period energy tracking
  - `evaluateChargeBlock()` at line 3228 — tiered block/tier logic
  - `evaluateRatchet()` at line 3338 — ratchet/demand lookback
  - `GetInputEconomicsTariff()` at line 267 — demand window configuration

## Findings

### Finding 1: Demand window computes peak from incomplete (partial-fill) windows after reset [Severity: medium]
**Description**: After `BillingState::reset()` (at `billing.rs:249`), `DemandWindow::reset()` (line 54) zero-fills all samples and resets `count` to 0. The next `push()` immediately sets `count=1` and `average()` returns the instantaneous single-sample value rather than waiting for the window to fill. This means the peak demand tracked at `billing.rs:179` (`self.peak_demand_kw = self.peak_demand_kw.max(avg)`) can be set by a one-sample average that represents an artificially narrow averaging window. A single 5-minute spike after a billing boundary can set a demand peak that would not pass a proper 15-minute rolling average test.

**Code Location**: `billing.rs:46-51` (`DemandWindow::average`), `billing.rs:54-62` (`DemandWindow::reset`), `billing.rs:177-179` (`peak_demand_kw` update).

**Root Cause**: The `count` field starts at 0 after reset and increments linearly until it reaches `capacity`. The `average()` divides `running_sum` by `count` from the first sample, producing partial-window averages. The `peak_demand_kw` then uses `f64::max` against these nascent averages.

**Impact**: Demand peaks can be inflated immediately after billing period boundaries. For a 15-minute window with 5-minute intervals (`capacity=3`), the first reading after reset captures a 5-minute instantaneous peak; the second captures a 10-minute average; only the third reaches the full 15-minute window. A spike in the first post-reset timestep can produce a demand charge higher than the tariff's intended 15-minute-rolling-average maximum.

**EnergyPlus Reference**: In `EconomicTariff.cc:2557-2562`, EnergyPlus only evaluates demand when `collectTime >= demWinTime * SecsInHour` — it waits for the full window:
```cpp
tariff.collectEnergy += curInstantValue;
tariff.collectTime += state.dataGlobal->TimeStepZoneSec;
if (tariff.collectTime >= tariff.demWinTime * Constant::rSecsInHour) {
    curDemand = tariff.demandConv * tariff.collectEnergy / tariff.collectTime;
    // ... evaluate and accumulate demand
    tariff.collectEnergy = 0.0;
    tariff.collectTime = 0.0;
}
```

**Recommendation**: Add a `full_window_count` guard to `DemandWindow::average()` that returns the current average only when `count == samples.len()`. Until the window fills, either return 0.0 or skip peak comparisons. Example:
```rust
fn average_full(&self) -> Option<f64> {
    if self.count == self.samples.len() { Some(self.average()) } else { None }
}
```
Then use this in `update()` at line 177-179 to skip peak updates on partial windows.

---

### Finding 2: No per-TOU-period energy accumulation — only global import/export kWh tracked [Severity: medium]
**Description**: `BillingState` tracks only `cumulative_import_kwh` and `cumulative_export_kwh` as single global accumulators (`billing.rs:68-69`). The `period_idx` parameter received by `update()` (line 168) is used exclusively for demand tracking (`period_peak_demand_kw`, line 180-181) and not for energy disaggregation. There is no `per_period_energy_kwh: Vec<f64>` or equivalent. At period-close time (`evaluator.rs:321-354`), `compute_tiered_energy_cost` receives the total undifferentiated import kWh. This makes it impossible to compute tariffs that have per-period tiered rates, per-period minimum energy charges, or per-period baseline allowances without modification.

**Code Location**: `billing.rs:65-81` (struct definition), `billing.rs:162-191` (`update`), `evaluator.rs:331-336` (tiered cost computation).

**Root Cause**: Design simplification — the original HARES architecture assumes tiered rates apply to total consumption regardless of TOU period, which is a reasonable model for many residential tariffs but not all.

**Impact**: Tariffs with the following features cannot be modeled accurately:
- Per-period block/tiered rates (e.g., "first 200 kWh on-peak at $0.10, remainder at $0.15")
- Per-period minimum energy charges
- Per-period baseline/customer charge credits
- Separate reporting of on-peak vs. off-peak consumption

**EnergyPlus Reference**: In `EconomicTariff.cc:2588`, EnergyPlus accumulates energy in a 2D array: `tariff.gatherEnergy(curMonth)[(int)curPeriod]` — energy is tracked per-month and per-TOU-period (up to 4 periods: Peak, Shoulder, OffPeak, MidPeak). Native variables like `PeakEnergy`, `OffPeakEnergy`, `ShoulderEnergy` expose these per-period totals to the rate computation engine (`EconomicTariff.cc:142-178`).

**Recommendation**: Add a `per_period_energy_kwh: Vec<f64>` to `BillingState` (same indexing as `period_peak_demand_kw`) and accumulate in `update()`:
```rust
if let Some(slot) = self.per_period_energy_kwh.get_mut(period_idx as usize) {
    *slot += import_kwh;
}
```
Reset alongside `period_peak_demand_kwh` in `reset()`. Expose via a `fn period_energy_kwh(&self, idx: u16) -> f64` accessor.

---

### Finding 3: Tier block season selection uses only the billing-period-start month [Severity: low]
**Description**: `compute_tiered_energy_cost()` at `billing.rs:288` selects a single tier block via `blocks.iter().find(|b| b.season.contains_month(month))` where `month` is `period_start.month()` (set in `evaluator.rs:322`). For `BillingCycle::Custom(days)` cycles that span month boundaries (e.g., a 45-day billing cycle from Mar-15 to Apr-29), the tier block for the entire period is chosen based on March. If the tier blocks differ between March (winter) and April (summer), the April consumption is incorrectly billed at March's tier structure.

**Code Location**: `evaluator.rs:322` (month derivation), `billing.rs:282-312` (tier selection and computation).

**Root Cause**: The tiered cost function receives a single `month` parameter with no knowledge of the billing period's temporal extent. It was designed for `BillingCycle::Monthly` where the month is unambiguous.

**Impact**: Minor, because most residential tariffs that use tiered rates have annual (single) tier structures, and monthly billing cycles are the default. Only affects `Custom(days)` billing cycles that span a seasonal boundary where tier blocks differ.

**Recommendation**: Either (a) document that `Custom(days)` billing cycles must not span seasonal tier boundaries (constrain to <= 1 month), or (b) extend the function to accept a date range and compute a time-weighted blend of tier blocks.

---

### Finding 4: Timestep boundary energy allocation — entire step goes to start-of-step period [Severity: low]
**Description**: The evaluator precomputes `period_indices` per timestep based on the step's start time (`evaluator.rs:90, 168`). The `update()` function applies a single `period_idx` to the entire step's energy. If a timestep straddles a TOU period boundary (e.g., 3:55pm-4:05pm where on-peak ends at 4:00pm), the entire step's energy is allocated to the start-period's bucket. There is no proration or sub-step splitting.

**Code Location**: `evaluator.rs:89-170` (period precomputation per step start time), `billing.rs:162-191` (`update` applies single period_idx).

**Root Cause**: The precomputed array model assumes period transitions align with timestep boundaries. In practice, period boundaries defined by clock time (e.g., 4:00pm) will not always coincide with simulation timestep boundaries.

**Impact**: Minor for typical hourly simulations with period boundaries at whole hours. More significant for 15-minute or 30-minute simulations where a timestep can span a peak/off-peak transition. The energy misfile is at most one timestep's worth per period transition per day — typically < 2% of daily energy.

**EnergyPlus Reference**: EnergyPlus has the same behavior — it collects energy over the demand window and then allocates the entire chunk to the period at window-close time (`EconomicTariff.cc:2588`). Neither implementation prorates across period boundaries.

**Recommendation**: Document this limitation. If precise boundary handling is needed, users can choose simulation intervals that align with period boundaries, or the `step()` function could accept a sub-period flag to indicate boundary crossings within a step.

---

### Finding 5: No fixed charge proration for initial partial billing periods [Severity: low]
**Description**: When the simulation starts mid-month (e.g., Jan 15), `BillingState::new()` sets `period_start = Jan 15` and `period_end = Feb 15`. At the first period close, `evaluator.rs:328-329` computes `days_in_period = 31` and charges the full `monthly_usd` and full daily charges. The initial period is not treated as partial. Only the final period (via `finalize()` at line 433-438) prorates fixed charges using elapsed days.

**Code Location**: `evaluator.rs:326-329` (full-period fixed charge), `evaluator.rs:430-438` (finalize proration).

**Impact**: Minor. A simulation starting Jan 15 with a $30/month fixed charge is billed $30 for a 31-day billing period instead of ~$16 for a half-month. This overstates costs by < 1 billing period. Simulations starting at month boundaries are unaffected.

**Recommendation**: Either document that simulations should start at a billing-period boundary, or add a `initial_partial` flag to `BillingState::new()` that causes the first `reset()` to prorate. EnergyPlus avoids this by always operating on a full annual cycle with month-aligned reporting.

---

### Finding 6: No annual true-up logic for net metering [Severity: low]
**Description**: Export credits are accumulated step-by-step in `cumulative_export_credit_usd` (`billing.rs:176`) and applied at each billing period boundary (`evaluator.rs:337, 446`). There is no end-of-year reconciliation (true-up) that would settle accumulated credits at an annual avoided-cost rate if the customer is a net exporter annually. The `ExportMode::NetMetering` variant exists in `types.rs:140` but is not distinguished in the runtime billing logic — both `NetMetering` and `NetBilling` are processed identically through per-step export prices.

**Code Location**: `types.rs:137-145` (ExportMode enum), `evaluator.rs:337, 346` (export credit applied per period), `billing.rs:70` (cumulative_export_credit_usd resets per period).

**Root Cause**: The net metering implementation defers to the caller (export price computation in the evaluator constructor) to set prices correctly. The `BillingState` has no concept of an annual accumulation period distinct from the billing period.

**Impact**: Net metering with annual true-up (California NEM 2.0, many US states) cannot be modeled without external post-processing. The current design works for per-period net metering where credits are applied at the billing cycle frequency and do not carry forward.

**EnergyPlus Reference**: EnergyPlus supports `BuyFromUtility`, `SellToUtility`, and `NetMetering` as buy/sell options on the tariff object (`EconomicTariff.cc:776-780`). Net metering uses the `ElectricityNet:Facility` meter which already nets production and consumption, so the net energy is treated as a single signed value — credits and charges are handled through the same rate structure rather than through a separate credit mechanism.

**Recommendation**: For annual true-up support, add an `annual_export_accumulator` to `BillingState` that persists across billing period resets and is only settled at the true-up anniversary date. The evaluator would need to know the true-up month.

---

## Summary
- **Total findings**: 6
- **Medium**: 2 (Finding 1 — demand window partial-fill, Finding 2 — no per-period energy tracking)
- **Low**: 4 (Finding 3 — tier month-boundary, Finding 4 — timestep boundary, Finding 5 — initial partial period, Finding 6 — annual true-up)
- **Critical / High**: 0

## Recommendations
1. **Fix demand window partial-fill (Finding 1)**: Add a guard in `DemandWindow` that skips peak comparisons until `count == capacity`. This is the most impactful finding — it can inflate demand charges after each billing period reset.
2. **Add per-period energy tracking (Finding 2)**: Extend `BillingState` with `per_period_energy_kwh: Vec<f64>` to enable per-period tiered rate computation. This is needed for full URDB tariff compatibility.
3. **Document limitations** for findings 3-6 in the tariff module documentation (`//!` comments) with guidance on simulation setup to avoid known edge cases.
4. For net metering annual true-up (Finding 6), design a carrier-over mechanism before implementing it, since it interacts with multi-period accumulation and may require changes to `BillingPeriodSummary` to distinguish between "bill credit" and "true-up settlement."

## References / Citations
- HARES `DemandWindow` ring buffer with running sum: `billing.rs:9-63`
- HARES `BillingState::update()` accumulation: `billing.rs:162-191`
- HARES `BillingState::reset()` period transition: `billing.rs:249-271`
- HARES `compute_tiered_energy_cost()`: `billing.rs:282-312`
- HARES `TariffEvaluator::step()` period-close logic: `evaluator.rs:293-363`
- HARES `TariffEvaluator::finalize()` proration: `evaluator.rs:420-466`
- EnergyPlus `GatherForEconomics()` demand window & period energy: `EconomicTariff.cc:2527-2627`
- EnergyPlus demand window configuration: `EconomicTariff.cc:645-755`
- EnergyPlus `evaluateChargeBlock()` tier/block logic: `EconomicTariff.cc:3228-3336`
- EnergyPlus native variable per-period energy breakdown: `EconomicTariff.cc:142-178`
