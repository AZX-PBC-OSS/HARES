# Schedule resolution pipeline: HPXML -> schedule source -> runtime evaluation

**Review ID**: wiring-06
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-io/src/schedule.rs`
- `crates/hares-io/src/schedule_resolve.rs`
- `crates/hares-core/src/environment.rs`

## Vendor/Reference Files Consulted

- `vendors/OCHRE/ochre/utils/schedule.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: `compute_schedule_offset` produces a one-day schedule misalignment after Feb 29 when the simulation clock crosses a leap year boundary. The function derives the start offset from `start_time.ordinal0()` (which returns 0–364 for non-leap years and 0–365 for leap years), but the schedule is always 365 rows × (24/"step_secs") hours. After Feb 29 in a leap-year simulation, `ordinal0()` is +1 relative to its non-leap counterpart for the same calendar date, so the computed schedule index permanently shifts by one day for the remainder of the year.

**Code Location**:
- `crates/hares-core/src/environment.rs:728-742` — `compute_schedule_offset` derives offset solely from wall-clock ordinal; never consults `schedule.len()` for year-length.
- `crates/hares-core/src/environment.rs:391-412` — the DST-aware `compute_schedule_idx` uses `civil.ordinal0()` directly, inheriting the same leap-year bias.
- `crates/hares-io/src/schedule.rs:481-511` — `generate_annual_timestamps` deliberately pins the synthetic year to 2007 (non-leap) to sidestep this for index-based schedules, but actual clock-driven schedules are not protected.

**Root Cause**: The schedule offset function implicitly assumes a 365-day year. For weather-array indexing, `compute_annual_offset` (`environment.rs:700-724`) correctly derives year-seconds from `weather.len() * step_secs`, which auto-adapts to leap-year file sizes (8784 rows). The schedule counterpart lacks this adaptation.

**Impact**: Annual energy totals may be slightly biased for leap-year simulations that cross Feb 29. The bias magnitude depends on schedule variability across adjacent days; peak demand during the mismatched day is off by one schedule row.

**Recommendation**: Mirror the weather-array approach: compute `year_secs = schedule.len() as u64 * step_secs as u64` and use `seconds_into_year % year_secs / step_secs` for the offset. Alternatively, match OCHRE's explicit rejection of leap-year data (`schedule.py:97-98`) by rejecting simulation start-times or durations that land on Feb 29 when the schedule is non-leap.

### Finding 2: [Severity: low]
**Description**: `ScheduleSource::DailyProfile::value_at` and the separate `annual_mean_fraction` function (used during `max_kw` scaling in `schedule_resolve.rs`) compute the annual-average fraction differently: the former applies month multipliers uniformly per month, while the latter weights each month's multiplier by days-per-month. The two formulations are mathematically self-consistent across a full non-leap year (annual energy balances correctly), but if a third-party consumer of `ScheduleSource::mean()` compares its output to the profile's annual-average as computed by `annual_mean_fraction`, the values diverge when month multipliers vary across months of unequal length.

**Code Location**:
- `crates/hares-types/src/schedule.rs:735-747` — `ScheduleSource::mean()` for `DailyProfile` uses simple 12-month average of `month_multipliers` (`sum / 12`).
- `crates/hares-io/src/schedule_resolve.rs:341-357` — `annual_mean_fraction()` uses days-per-month weighted average of `month_multipliers`.

**Root Cause**: `ScheduleSource::mean()` is documented as an "approximate statistical mean" and makes a deliberate simplicity trade-off; the more precise weighting lives only in `schedule_resolve.rs`.

**Impact**: Low. Runtime energy conservation is correct; only introspective `mean()` callers would see a discrepancy (a few percent for strongly seasonal profiles).

---

### Finding 3: [Severity: low]
**Description**: The `TimeWindow` variant supports per-window additive noise via `DistributionKind` (Gaussian, Uniform, etc.). The `with_noise` constructor documents that the noise distribution "should typically have `mean: 0.0` (for Gaussian) so that `value` is the true center" (`schedule.rs:97-100`), but this zero-mean convention is not enforced at compile time or runtime. A caller could supply a noise distribution with a non-zero mean (e.g., `Gaussian { mean: 5.0, std_dev: 1.0 }`) causing the long-run average to deviate from `w.value` by the distribution mean, violating the expectation that stochastic generation preserves expected energy around the deterministic baseline.

**Code Location**:
- `crates/hares-types/src/schedule.rs:95-134` — `TimeWindow::with_noise()` accepts arbitrary `DistributionKind` with no zero-mean validation.
- `crates/hares-types/src/schedule.rs:815-833` — `resolve_window_value()` applies noise as `w.value + noise` (additive), making the mean shift by `distribution_mean`.
- `crates/hares-types/src/schedule.rs:227-252` — `DistributionKind::sample()` is generic; no zero-mean constraint.

**Root Cause**: Documentation-only guard. The type system does not distinguish between zero-mean and biased distributions.

**Impact**: If misconfigured, stochastic window schedules would produce a biased time mean. Users following the documentation convention are unaffected.

**Recommendation**: Add a `debug_assert!` or runtime validation in `with_noise` that checks `(noise.mean() - 0.0).abs() < 1e-9` for `Gaussian` variants (and symmetric bounds for `Uniform`), emitting a warning if the zero-mean convention is violated. Alternatively, split `DistributionKind` into zero-mean-only and general variants.

---

### Finding 4: [Severity: low]
**Description**: The `DailyProfile` series in `ScheduleSource::value_at` (`schedule.rs:588-603`) discriminates only between weekday and weekend, with no mechanism for holiday overrides. The `is_weekend` variable is computed as `weekday().num_days_from_monday() >= 5`, matching OCHRE's `create_simple_schedule` (`schedule.py:282-304`) which similarly lacks holiday awareness. The `TimeWindows` variant could express holiday-specific windows through `DayFilter::Day`, but the HPXML import path (`schedule_resolve.rs:730-779`) maps parsed fractions to `DailyProfile` only, never to `TimeWindows`.

**Code Location**:
- `crates/hares-types/src/schedule.rs:596` — weekday/weekend binary split.
- `crates/hares-io/src/schedule_resolve.rs:754-758` — `resolve_hpxml_profile` returns `DefaultScheduleProfile` without holiday list.

**Root Cause**: HPXML/BEopt/ResStock schedules encode only weekday/weekend fractions; the OCHRE reference also lacks holiday handling for simple schedules.

**Impact**: Holiday energy consumption may be underestimated for loads that differ significantly on holidays (e.g., increased cooking on Thanksgiving). Affects annual energy totals by roughly `n_holidays / 365` (about 1–2% for major holidays).

---

## Verified-Correct Behaviors

This section confirms areas where the pipeline meets the review criteria and matches the OCHRE reference implementation.

### Duty cycle fraction storage (0.0–1.0, not 0–100)
Schedule CSV values (e.g., `0.1`, `0.2`, `1.0`) are parsed as raw `f64` in `schedule.rs:389` and stored without scaling. The `inject_power_schedule` path (`schedule_resolve.rs:678-686`) computes `mean_fraction = sum(values) / len` and derives `max_kw = (annual_kwh / 8760) / mean_fraction`. This matches OCHRE's `convert_power_column` (`schedule.py:309-313`): `annual_mean = properties["Annual Electric Energy (kWh)"] / 8760; max_value = annual_mean / schedule_mean`. Fractions are in 0.0–1.0 range and multiply rated power, not percentages.

### Multiplier combination (multiplicative, not additive)
In `DailyProfile::value_at` (`schedule.rs:602`): `frac * month_multipliers[month_idx] * max_value`. This is multiplicative: hour-fraction × month-multiplier × kW-scaling. OCHRE's `create_simple_schedule` (`schedule.py:304`) confirms: `return df["w_fracs"] * df["m_fracs"]`. No addition or replacement of multipliers.

### Schedule lookup at timestep boundaries: value for [t, t+dt)
The `SimClock::current_time()` (`clock.rs:55-58`) returns `start_time + step * time_res`, producing the start of interval [t, t+dt). The schedule index at line 486 (`environment.rs`) reads `col[schedule_idx]`, which is the value for this interval. The OCHRE reference follows the same convention via DataFrame resampling with `inclusive="left"` (`schedule.py:565`). No [-dt, t) off-by-one pattern was found.

### Midnight wrapping and month boundaries
The schedule index wraps via `(step + offset) % schedule_len` (`environment.rs:411`). At the final step of Dec 31, the wrap returns to index 0 (Jan 1 values), producing correct midnight transition behavior. `month0()` (0-indexed) maps directly to the 12-element `month_multipliers` array, with no off-by-one in month boundary transitions. The `TimeWindow::contains` method (`schedule.rs:138-155`) correctly handles midnight-wrapping windows via the predecessor-day filter logic, validated by integration tests at `schedule.rs:1464-1493`.

### Stochastic generation determinism and reproducibility
The `ScheduleSource::Stochastic` variant uses `ChaCha8Rng` seeded with a fixed 32-byte array. `reset()` (`schedule.rs:691-713`) rewinds the RNG to the seed, enabling deterministic replay and checkpoint restart. This is tested at `schedule.rs:1181-1201`. The `TimeWindows` path similarly wraps its RNG in `StochasticState` with the same reset capability.

### DST-aware schedule indexing
The `compute_schedule_idx` DST path (`environment.rs:394-408`) converts the fixed-offset simulation time to civil (wall-clock) time via `sim_time.with_timezone(&tz)`, then computes schedule index from civil `ordinal0()` and wall-clock hour. This correctly handles spring-forward (schedule hour skipped, index jumps) and fall-back (hour duplicated, same index read twice). Weather indexing is explicitly excluded from DST awareness to preserve physical solar position accuracy.

---

## Summary

- Total findings: 4
- Critical: 0
- High: 0
- Medium: 1 (leap-year schedule offset misalignment)
- Low: 3 (mean computation inconsistency, stochastic noise convention, holiday handling)

## Recommendations

1. **Adapt schedule offset for leap years** by using `schedule.len() * step_secs` as the annual period, matching the pattern already established in `compute_annual_offset` for weather data. This eliminates the one-day schedule drift after Feb 29 in leap-year simulations.

2. **Add zero-mean validation to `TimeWindow::with_noise`** to catch misconfigured noise distributions at construction time rather than silently producing biased long-run averages.

3. **Consider uniformizing `ScheduleSource::mean()` and `annual_mean_fraction`** weighting if third-party consumers of the analytical mean require consistency with the profile-scaling path. Current behavior is functionally correct for the primary use case (energy scaling).

4. **Document the holiday-limitation explicitly** in the `DailyProfile` variant comment, noting that `TimeWindows` with `DayFilter::Day` can express specific-date overrides if needed.

## References / Citations

- OCHRE `schedule.py:89-119` — `set_annual_index` rejects leap-year data (`duration.days != 365`).
- OCHRE `schedule.py:282-304` — `create_simple_schedule` multiplies weekday/weekend fractions by month multipliers.
- OCHRE `schedule.py:307-346` — `convert_power_column` derives `max_value = annual_mean / schedule_mean`.
- OCHRE `schedule.py:480-568` — `resample_and_reindex` uses `inclusive="left"` for half-open interval convention.
