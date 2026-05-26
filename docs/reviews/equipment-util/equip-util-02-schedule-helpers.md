# Schedule evaluation helpers: duty cycle, stochastic, interpolation
**Review ID**: equip-util-02
**Category**: equipment-util
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/schedule_helpers.rs crates/hares-io/src/schedule.rs

Additional HARES files consulted for full context:
- crates/hares-types/src/schedule.rs (ScheduleSource, DayFilter, TimeWindow, DistributionKind, StochasticState)
- crates/hares-io/src/schedule_resolve.rs (schedule CSV column resolution, annual_mean_fraction)
- crates/hares-equipment/src/hvac/heat_pump/heater.rs (duty cycle processing)
- crates/hares-equipment/src/hvac/air_conditioner.rs (duty cycle processing)
- crates/hares-equipment/src/scheduled_load.rs (DailyProfile config parsing)

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/schedule.py

## Findings
### Finding 1: No holiday schedule support [Severity: low]
**Description**: HARES has no concept of holiday-specific schedule day types. The `DayFilter` enum supports `Any`, `Weekdays`, `Weekends`, and `Day(Weekday)`, but no `Holidays` variant. The `DailyProfile::value_at` method uses `num_days_from_monday() >= 5` to classify weekends, with no calendar-aware holiday overrides. TimeWindow matching similarly lacks holiday awareness.

**Code Location**: `crates/hares-types/src/schedule.rs:13-33` (DayFilter), `crates/hares-types/src/schedule.rs:596` (weekend detection in value_at)

**Root Cause**: HARES follows the same approach as OCHRE, which also lacks holiday logic — OCHRE explicitly ignores `lighting_exterior_holiday` in its `Ignore` category (`schedule.py:59`).

**Impact**: Simulations covering federal holidays will apply weekday schedules on those days. For most residential energy models this is a minor effect since occupancy-driven holiday behavior is not typically modeled at this fidelity. No runtime errors.

### Finding 2: Stochastic clamping may bias analytical mean vs. true mean [Severity: low]
**Description**: The `ScheduleSource::mean()` method for `Stochastic` computes `kind.mean().clamp(lo, hi)` — clamping the analytical mean after the fact. For distributions with significant probability mass beyond the clamp bounds, the true truncated mean differs from this approximation. For example, `Gaussian { mean: 5.0, std_dev: 2.0 }` with `clamp_min: Some(0.0)` has a true truncated mean > 5.0 due to the missing left tail, but `mean()` returns exactly 5.0. The code documents this limitation at line 728: *"this is an approximation when clamping is active"*.

**Code Location**: `crates/hares-types/src/schedule.rs:764-774` (mean computation), `crates/hares-types/src/schedule.rs:641-654` (value_at with clamping)

**Root Cause**: Analytical truncated distribution means require computing the expected value of the truncated distribution, which is non-trivial for arbitrary clamp bounds. The current implementation uses a simple post-hoc clamp on the analytical mean as a first-order approximation.

**Impact**: Equipment sizing estimates using `mean()` with clamped stochastic sources may be slightly off, particularly for distributions with heavy tails (Gaussian with large std_dev, Exponential). In practice, clamping is typically used only with `min_value: Some(0.0)` to prevent negative values, and the bias is small for distributions centered well above zero.

### Finding 3: StochasticState PartialEq excludes RNG word position [Severity: low]
**Description**: The `PartialEq` implementation for `StochasticState` compares only `seed` and `draw_count`, deliberately excluding the internal RNG state word position (line 293: `// RNG state excluded for deterministic replay`). This means two `StochasticState` instances with identical seed and draw count compare as equal even if their RNGs are at different internal word positions — a situation that can only arise when RNG has been advanced through a non-draw-count path (e.g., manual seek via `set_word_pos`).

**Code Location**: `crates/hares-types/src/schedule.rs:291-295`

**Root Cause**: The design choice to enable checkpoint comparison through seed+draw_count identity. Since ChaCha8 is deterministic given seed+draw_count for the standard `sample()` path, this is correct for all normal operation. The only path that violates this invariant is `ChaCha8Rng::set_word_pos()` used in the `NoisyTimeWindows` restore path.

**Impact**: If `PartialEq` is used for deduplication or caching decisions involving `NoisyTimeWindows` RNG states, it could produce false positive equality. In practice, `PartialEq` on `ScheduleSource` (line 476) uses the full `StochasticState` comparison only for `Stochastic` sources, not `TimeWindows`. For `TimeWindows`, the RNG state is compared via `Box` pointer identity (line 577), so this finding has no practical impact on schedule evaluation.

### Finding 4: Setpoint schedules use piecewise-constant (ZOH) interpolation [Severity: low, Observational]
**Description**: The `DailyProfile::value_at` method selects the current hour's value via `env.current_time.hour() as usize` with no within-hour interpolation. This means setpoints and equipment schedules are piecewise-constant (zero-order hold) across sub-hourly timesteps. This is correct for binary availability schedules (on/off, fraction-based duty cycle) and matches OCHRE's `resample().ffill()` convention for discrete schedules. However, temperature setpoint schedules would be more physically accurate with linear interpolation within the hour.

**Code Location**: `crates/hares-types/src/schedule.rs:594-602` (DailyProfile hour lookup), `crates/hares-io/src/schedule.rs:199-234` (ZOH upsampling)

**Root Cause**: HARES treats all schedules as piecewise-constant, consistent with OCHRE's `resample_and_reindex` default behavior (`ffill` for upsampling). The weather module uses PCHIP for continuous fields (temperature, pressure) and ZOH for discrete/solar fields, but this separation does not extend to setpoint schedules.

**Impact**: At typical sub-hourly simulation timesteps (1-15 minutes), the piecewise-constant assumption for setpoints may cause minor discontinuities at hour boundaries but does not create energy accounting errors. High-fidelity thermal models using 1-minute timesteps could see small artifacts from instantaneous setpoint changes at hour transitions.

### Finding 5: Annual mean fraction uses non-leap-year day counts [Severity: low, Observational]
**Description**: `annual_mean_fraction()` in `schedule_resolve.rs` hardcodes February as 28 days (`const DAYS_PER_MONTH: [f64; 12] = [31.0, 28.0, 31.0, ...]`). The comment at line 339 explicitly labels this as "non-leap year." This is used to compute `max_kw = annual_kwh / 8760 / mean_fraction` for converting normalized schedule fractions to absolute kW.

**Code Location**: `crates/hares-io/src/schedule_resolve.rs:341-357`

**Root Cause**: The HPXML annual energy inputs are defined on a standard 365-day year basis (8760 hours), matching OCHRE's convention. Using non-leap-year day counts is therefore correct for the intended conversion.

**Impact**: None for standard HPXML inputs. The comment explicitly calls out the non-leap-year assumption, which is appropriate documentation.

### Finding 6: Schedule resampling requires integer-multiple timestep ratios [Severity: low, Observational]
**Description**: `ScheduleTimeSeries::resample()` requires `source_step_secs` and `target_step_secs` to be integer multiples (in either direction). This is validated at lines 201-206 (upsampling) and 237-242 (downsampling). While this covers all standard timestep ratios (60→300→900→3600), it rejects non-integer ratios that could arise with unusual simulation configurations.

**Code Location**: `crates/hares-io/src/schedule.rs:201-206, 237-242`

**Root Cause**: The integer-multiple constraint simplifies implementation and matches typical simulation timestep conventions (1 min, 5 min, 15 min, 1 hr). Both HARES and OCHRE (`resample_and_reindex`, schedule.py:542-563) require compatible time resolutions.

**Impact**: Simulation configurations using non-standard timestep ratios will fail with a clear error message. No correctness impact for standard configurations.

### Finding 7: Time-window midnight wrapping correctly handles day boundaries [Severity: positive]
**Description**: `TimeWindow::contains()` correctly handles windows that wrap across midnight (e.g., 22:00–06:00). The pre-midnight portion matches the anchor day, and the post-midnight portion matches the next calendar day via `weekday.pred()`. This correctly handles Sunday→Saturday wrapping. Tests at lines 1255-1267 verify this behavior.

**Code Location**: `crates/hares-types/src/schedule.rs:138-155`

### Finding 8: Stochastic checkpoint replay correctly handles all distribution types [Severity: positive]
**Description**: `restore_schedule_source_state()` re-seeds the RNG and replays all draws from the start (line 78-81). This is correct for all distribution types including Poisson (which uses rejection sampling with variable RNG word consumption per draw) because it re-derives the RNG state rather than attempting a word-level seek.

**Code Location**: `crates/hares-equipment/src/schedule_helpers.rs:74-83`

### Finding 9: Downsampling energy conservation verified by tests [Severity: positive]
**Description**: The `resample_downsamples_preserves_total_energy` test (schedule.rs:835-851) verifies that `Sum` aggregation preserves the total energy integral when downsampling. The `resample_downsamples_preserves_mean` test (schedule.rs:854-871) verifies that `Mean` aggregation preserves the average. Both tests pass.

**Code Location**: `crates/hares-io/src/schedule.rs:835-871`

### Finding 10: Duty cycle does not accumulate rounding error [Severity: positive]
**Description**: Duty cycle controls are applied as a simple multiplicative factor: `effective_load = ctrl_duty_cycle * ctrl_load_fraction * dr_load_fraction * dr_duty_cycle` (heater.rs:1624-1627). Power is then computed as `rated_power * effective_load * timestep_duration`. Each timestep's computation is independent — there is no carry-forward of partial minutes or rounding state. The f64 multiplication provides sufficient precision for any practical simulation duration.

**Code Location**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1622-1640`

## Summary
- Total findings: 10
- Critical: 0 / High: 0 / Medium: 0 / Low: 6 / Positive: 4

### Severity distribution by focus area
| Area | Low | Positive |
|------|-----|----------|
| Holiday / day-type selection | 1 | 0 |
| Stochastic schedule generation | 2 | 1 |
| Interpolation / resampling | 2 | 1 |
| Duty cycle evaluation | 0 | 2 |
| Annual mean / normalization | 1 | 0 |

## Recommendations
1. **Holiday support**: If holiday-specific schedule behavior is needed in the future, add a `Holidays` variant to `DayFilter` and provide a configurable holiday calendar (e.g., US federal holidays). This would be a non-breaking addition since `DayFilter` is not `#[non_exhaustive]`.
2. **Stochastic clamping bias**: Consider computing true truncated distribution means for clamped stochastic sources if the analytical `mean()` is used for equipment sizing decisions. Add an integration test that compares empirical mean over many realizations against the analytical approximation to quantify the bias under realistic clamp configurations.
3. **Setpoint interpolation**: Consider adding optional linear interpolation for temperature setpoint `DailyProfile` sources at sub-hourly timesteps. This could be a configuration flag on the source (e.g., `interpolate: true` on `DailyProfile`) to maintain backward compatibility with piecewise-constant default.
4. **Setpoint interpolation**: For temperature setpoints, consider using the PCHIP resampling path (already available in the weather module at `weather.rs:905`) rather than the ZOH resampling used for schedule columns, to produce smoother setpoint transitions at sub-hourly resolution.
5. **PartialEq for TimeWindows**: Consider comparing `TimeWindows` RNG state content (not just pointer identity) for safer equality checks, or document the pointer-identity comparison as intentional.

## References / Citations
- OCHRE `schedule.py:289-304` — `create_simple_schedule` weekday/weekend merge using `weekday < 5`
- OCHRE `schedule.py:480-568` — `resample_and_reindex` with `ffill()` for upsampling, `mean()`/`sum()` for downsampling
- OCHRE `schedule.py:89-119` — `set_annual_index` non-leap-year constraint (raises error for leap year)
- HARES `schedule.rs:835-851` — energy conservation test for Sum aggregation downsampling
- HARES `schedule.rs:199-234` — ZOH upsampling implementation matching OCHRE ffill convention
- HARES `schedule_helpers.rs:54-84` — stochastic state restore via seed replay
- HARES `schedule.rs:341-357` — non-leap-year day counts in annual_mean_fraction
- HARES `schedule.rs:138-155` — midnight-wrapping TimeWindow day boundary handling
