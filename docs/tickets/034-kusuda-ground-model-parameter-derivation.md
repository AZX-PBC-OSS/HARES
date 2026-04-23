# Kusuda-Achenbach Ground Model: Amplitude Fallback Uses Instantaneous Extremes, Not Monthly Means

**Severity**: Medium
**Status**: Open
**Areas**: hares-core/environment

## Problem

`compute_mains_inputs` at `crates/hares-core/src/environment.rs:683` derives the Kusuda-Achenbach
amplitude parameter by computing the range of monthly mean dry-bulb temperatures. Two defects
corrupt this derivation.

**Defect 1 — hardcoded non-leap February.**
At `environment.rs:694`, month day counts are hardcoded:
```
let month_days = [31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
```
For a leap-year weather file (8784 hourly records), the February slice is one day short (28 × 24 =
672 samples instead of 696), causing February to be computed from 28 days of data while the
remaining 29 days are silently absorbed into the March slice. The February monthly mean is wrong and
the March mean is wrong, producing a corrupted `mains_dt_annual_range_c`.

**Defect 2 — fallback uses instantaneous hourly extremes.**
At `environment.rs:696–697`, when `temps.len() < year_samples`:
```
return (annual_avg_c, simple_range(temps));
```
`simple_range` returns the difference between the minimum and maximum value across all hourly
records (`environment.rs:722–728`). The Kusuda-Achenbach `t_amplitude_c` is defined as half the
range of monthly mean temperatures — not half the range of instantaneous hourly values. Per Kusuda
and Achenbach (1965) ASHRAE Transactions 71(1):61-74 and EnergyPlus Engineering Reference §3.1,
`T_amplitude = (T_max_monthly_mean - T_min_monthly_mean) / 2`. Instantaneous hourly extremes are
typically 5–15 °C wider than the monthly-mean range. The fallback path therefore over-estimates the
Kusuda amplitude by 30–70%, producing ground temperature swings that are far too large.

Note: this defect only affects the simulation once ticket 055 is resolved and
`kusuda_achenbach_temp` is called from the thermal solver hot path. The parameter derivation is
currently moot at runtime.

## Current Behavior

`crates/hares-core/src/environment.rs:694`:
```
let month_days = [31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
```
Fixed non-leap February regardless of weather file length.

`crates/hares-core/src/environment.rs:696–697`:
```
if temps.len() < year_samples {
    return (annual_avg_c, simple_range(temps));
```
Returns instantaneous min/max range when fewer than 8760 hourly records are present.

## Required Behavior

1. Detect leap year from `weather.dry_bulb_c.len()`: if `len == 8784 * samples_per_hour` (or
   equivalently, if `len % (366 * samples_per_day) == 0` and `len / samples_per_day == 366`), use
   February day count of 29. Per EnergyPlus Engineering Reference §3.1, annual period is 365 or
   366 days depending on the weather file.
2. When `temps.len() < year_samples` (short or synthetic weather), derive the amplitude from the
   range of whatever monthly means can be computed from the available data, not from instantaneous
   extremes. Specifically: compute `month_means` for the months fully covered by `temps`, then
   apply `simple_range(&month_means)`. If fewer than 2 months of data are present, use 0.0 as
   the amplitude (constant ground temperature equal to mean), which is physically conservative and
   matches the synthetic weather case.

## Approach

1. In `compute_mains_inputs`, after computing `samples_per_day`, derive `is_leap` by checking
   whether `temps.len()` equals `366 * samples_per_day`. Pass `is_leap` to a `month_day_counts`
   helper (the equivalent already exists in `crates/hares-io/src/epw.rs:381` as
   `monthly_day_counts`; extract to a shared location or duplicate with a comment).
2. Replace the early-return fallback at line 696 with a partial-month-means approach: accumulate
   `month_means` for months where `cursor + month_samples <= temps.len()`, then call
   `simple_range(&month_means)` if `month_means.len() >= 2`, else use `0.0`.
3. Remove the `simple_range(temps)` call entirely from `compute_mains_inputs`; it must not be
   reachable after this change.

## Definition of Done

- `compute_mains_inputs` uses 29-day February when `temps.len()` corresponds to a leap year.
- The short-weather fallback path never calls `simple_range` on the raw `temps` slice.
- Test: a leap-year weather series (8784 hourly records) produces a February mean computed from
  29 days of data.
- Test: a weather series with only 2 months of data produces amplitude = `simple_range` of those
  2 monthly means, not the hourly min/max range.

## Verification

```
cargo test -p hares-core environment
cargo test -p hares-physics ground
```

The environment test suite must include:
- A synthetic 8784-record series confirming 29-day February averaging.
- A 48-record series (2 days) confirming amplitude is derived from available monthly means,
  not from `simple_range` of all 48 hourly values.

## References

- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 —
  `T_amplitude = (T_max_monthly_mean - T_min_monthly_mean) / 2`
- Burch, J. and Christensen, C. (2007), NREL/CP-550-41263 — monthly-mean range for `ΔT`
- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer" — `T_amplitude` from monthly-mean
  temperature extremes, annual period 365 or 366 days
