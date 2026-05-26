# Tariff demand averaging window silently collapses
**Review ID**: types-physics-08
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-tariff/src/billing.rs` (DemandWindow, BillingState, apply_ratchet)
- `crates/hares-tariff/src/evaluator.rs` (TariffEvaluator::new window capacity validation, compute_demand_charge)
- `crates/hares-tariff/src/types.rs` (ElectricTariff, DemandRate, RatchetConfig)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py` — Building envelope / HPXML parser only; contains no tariff, demand, or billing logic. No relevant demand-averaging comparison available.

## Findings

### Finding 1: Demand window silently collapses to a single sample when step size exceeds the window [Severity: high]
**Description**: The demand averaging window capacity is computed as integer division of the window duration by the simulation interval (`billing.rs:126`). When the simulation step (e.g., 60 minutes) exceeds the demand window (e.g., 15 minutes), the division yields 0, which is silently capped to 1 via `.max(1)` (`billing.rs:130`). This means a single sample — the step-average power — is used as the demand, rather than any sub-step aggregation. An hourly-average power value under-reports the true 15-minute peak demand.

**Code Location**:
- `crates/hares-tariff/src/billing.rs:125-130` — `capacity` computation and silent clamp
- `crates/hares-tariff/src/evaluator.rs:183-198` — Validation that only checks divisibility when `window_seconds > interval_seconds`, leaving the opposite case unguarded

**Root Cause**: No sub-step disaggregation exists. The `DemandWindow` is a simple ring buffer that holds `capacity` samples; each sample is a single `net_power_kw` value from one simulation step. There is no mechanism to split a coarse step into finer sub-steps matching the demand window resolution. The code comment at `evaluator.rs:184-187` treats this as intentional ("This is physically correct (the interval IS the averaging window)"), but this is incorrect for the common case where the step value represents an average over the full interval, not a peak over a sub-interval.

**Impact**: When simulating with hourly (3600s) steps and a standard 15-minute demand window:
- An hour containing a 10 kW 15-minute spike and 0 kW for the remaining 45 minutes yields an hourly average of 2.5 kW, but the true demand is 10 kW.
- Residential demand charges (often $10–20/kW) would be under-reported by up to 4x.
- No warning, error, or log message is emitted — the collapse is completely silent.

**Test Gap**: No test exercises the case `demand_window_minutes < interval_minutes`. The existing `demand_window_30_minutes` and `demand_window_default_15_minutes` tests (`evaluator.rs:1426–1490`) both use 5-minute intervals smaller than their respective windows.

### Finding 2: No warning or error when demand window < simulation interval [Severity: medium]
**Description**: The validations at `evaluator.rs:189-198` only check for the case where the demand window exceeds the interval but is not evenly divisible. The opposite case — demand window is shorter than the interval — produces no warning, error, or log message. Users running hourly simulations with a 15-minute demand window receive no indication that the demand calculation has degraded to a 1-sample instantaneous peak.

**Code Location**: `crates/hares-tariff/src/evaluator.rs:189-198`

**Root Cause**: The validation condition `if window_seconds > interval_seconds as u64` excludes the `window_seconds <= interval_seconds` case entirely. There is no secondary check or user-facing warning.

**Impact**: Users may unknowingly run hourly simulations expecting valid 15-minute demand charges, receiving silently incorrect results. This violates the principle of least surprise for user-facing simulation tools.

### Finding 3: The `push_count` periodic recomputation uses an arbitrary multiplier [Severity: low]
**Description**: The `DemandWindow::push` method periodically recomputes `running_sum` from scratch via `self.samples.iter().sum()` every 1000 pushes (`billing.rs:41-43`). While the code comment at `billing.rs:59-61` explains this as a floating-point drift correction ("the periodic recomputation guard restarts"), the multiplier value 1000 is arbitrary and untuned. With 5-minute intervals, 1000 pushes = ~3.5 days of simulation; accumulated drift over that period from subtraction/addition is typically negligible (~1e-12) for f64. The recomputation provides no measurable benefit while adding a branch to the hot path.

**Code Location**: `crates/hares-tariff/src/billing.rs:41-43`

**Root Cause**: Defensive coding without evidence of need. f64 subtraction of near-equal values over 1000 iterations exhibits minimal cancellation error.

**Impact**: Negligible — no incorrect behavior. Minor performance cost from a branch and conditional sum on every 1000th push.

### Finding 4: Demand ratchet is correctly initialized for the first billing period [Severity: none]
**Description**: Verified that `apply_ratchet` (`billing.rs:369-382`) returns `current_peak` unchanged when `prior_peaks_kw` is empty (i.e., the first billing period). The ratchet floor is only applied from the second period onward once historical peaks are recorded. No zero-initial-peak undercharge scenario exists: the first period's peak is preserved in `prior_peaks_kw` via `reset()` (`billing.rs:250`) before the current peak is zeroed, so the second period's ratchet uses valid historical data.

**Code Location**: `crates/hares-tariff/src/billing.rs:369-382` (apply_ratchet), `249-261` (reset prior_peaks push)

### Finding 5: DemandWindow ring buffer has no off-by-one error at boundary [Severity: none]
**Description**: The `DemandWindow::push` and `DemandWindow::average` implementations (`billing.rs:28-52`) correctly handle partial (not-yet-full) windows using a `count` counter that increments until it reaches `capacity`. The ring buffer head wraps correctly via modulo arithmetic. The average divides by `count` (actual samples), not `capacity`. When the window is full, `count == capacity` and the average reflects exactly `capacity` samples. This is confirmed by test `demand_window_rolling_average` (`billing.rs:398-407`).

## Summary
- Total findings: 5
- Critical: 0 / High: 1 / Medium: 1 / Low: 1 / None: 2

## Recommendations
1. **Add sub-step capacity warning**: At `evaluator.rs:189`, when `window_seconds <= interval_seconds`, emit at least a `log::warn!` (or return an error for safety) indicating that the demand window will collapse to a single step. The error path for the reverse case should be elevated to at least a warning as well — currently it only checks even divisibility.

2. **Consider sub-step interpolation**: For coarse-timestep simulations where `interval_seconds > demand_window_minutes * 60`, the evaluator could accept a finer time-series input (e.g., 15-minute load profiles from a sub-hourly load disaggregator) or require the caller to provide the peak sub-interval power. Alternatively, document clearly that demand charges require the simulation interval to be no larger than the demand averaging window.

3. **Remove or justify `push_count` drift correction**: Either remove the periodic recomputation (f64 drift over 1000 iterations is negligible), or document the specific scenario that motivated it with a supporting test.

## References / Citations
- NERC/FERC standard demand interval: 15 minutes for metering (ANSI C12.1)
- IEEE 1459-2010: demand interval definitions
- `crates/hares-tariff/src/billing.rs:9-63` — DemandWindow implementation
- `crates/hares-tariff/src/evaluator.rs:183-198` — Window capacity validation
- `crates/hares-tariff/src/billing.rs:369-382` — Ratchet application logic
