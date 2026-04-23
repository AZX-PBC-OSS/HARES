# Solar Position: Leap-Year Weather Array Indexed With 365-Day Modulus

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-core/environment

## Problem

`compute_annual_offset` at `crates/hares-core/src/environment.rs:659` hardcodes a 365-day year:

```rust
let year_secs = 365_u64 * 86400;
```

The comment on line 656–658 asserts "EPW files always have 8760 rows (365 × 24 hours)". This is true for standard EPW files, but the codebase also accepts PSM3 and TMY3 inputs which may have leap-year row counts, and `compute_annual_offset` is called for all weather formats via `EnvironmentManager::new`. For any weather file with 8784 hourly rows (366 days), a simulation starting after February 28 uses the 365-day modulus and reads from one step earlier than the correct row in the weather array. At 15-minute sub-hourly resolution the mismatch is 96 rows per affected day — equivalent to reading the wrong day's weather for every step from March 1 onward.

Secondary issue: `EnvironmentManager::new` accepts `start_time: DateTime<FixedOffset>` but does not validate that the embedded offset matches `weather.meta.timezone_offset_h`. The conversion `local_datetime.to_utc()` in `solar_position` at `crates/hares-physics/src/solar.rs:93` is correct solar physics, but only when the caller provides a correctly-offset local time. A UTC `DateTime<FixedOffset::east_opt(0)>` passed to a weather file with timezone −7 produces solar noon 7 hours off. There is no validation or warning.

The Spencer (1971) Fourier-series declination model (solar.rs:105-116) has documented accuracy of ±0.03° in solar zenith, which is acceptable for annual energy simulation. It is not the primary defect here. NREL SPA (NREL/TP-560-34302, Reda & Andreas 2004) is more accurate but the improvement over Spencer is below 0.5% annual energy impact for most building locations.

## Current Behavior

`crates/hares-core/src/environment.rs:659`: `year_secs = 365_u64 * 86400` — any 366-day weather file causes incorrect row indexing for all timesteps after February 28.

`crates/hares-core/src/environment.rs:191` and `217`: `compute_annual_offset` called at `EnvironmentManager::new` for all weather formats.

`crates/hares-physics/src/solar.rs:93`: `local_datetime.to_utc()` — correct but unvalidated; mismatched timezone offset silently produces wrong solar position.

## Required Behavior

1. `compute_annual_offset` must derive `year_secs` from the actual weather array length rather than hardcoding 365 days. Pass the weather row count (or is-leap-year flag derived from `weather.meta` or `weather.len()`) and compute `year_secs = weather_hourly_rows as u64 * 3600`. For PSM3 sub-hourly files, derive from the equivalent hourly row count.

2. `EnvironmentManager::new` must emit `tracing::warn!` when `start_time.offset().local_minus_utc() / 3600` differs from `weather.meta.timezone_offset_h` by more than 0.5 hours. The caller is responsible for providing a correctly-offset local time; the warning makes misuse detectable without requiring a breaking API change.

3. No silent substitution for the timezone mismatch — the warning must name the offending values (provided offset vs. file's offset).

Primary citations:
- NREL/TP-560-34302: Reda, I. and Andreas, A. (2004/2008), "Solar Position Algorithm for Solar Radiation Applications" — reference SPA algorithm for solar position
- Spencer, J.W. (1971), "Fourier series representation of the position of the sun", Search, 2(5), 172 — current model; ±0.03° accuracy documented

## Approach

In `compute_annual_offset`, add a `weather_hourly_rows: usize` parameter. Compute `year_secs = weather_hourly_rows as u64 * 3600`. Update call sites at `environment.rs:191` and `217` to pass `weather.len()` (or `weather.meta.n_rows`). Update the existing tests that hardcode expected offsets to reflect the corrected modulus.

In `EnvironmentManager::new`, after constructing the weather object, compare `start_time.offset().local_minus_utc()` against `(weather.meta.timezone_offset_h * 3600.0) as i32` and emit `tracing::warn!` if the difference exceeds 1800 seconds.

## Definition of Done

- [ ] `compute_annual_offset` derives `year_secs` from the actual weather array row count
- [ ] Leap-year weather file (8784 rows), simulation starting March 1: weather index matches expected date
- [ ] Non-leap weather file (8760 rows): behavior unchanged from current
- [ ] `EnvironmentManager::new` emits `tracing::warn!` when start time timezone offset differs from weather file's offset by > 0.5 hours
- [ ] Existing `compute_annual_offset` tests updated to pass the weather row count parameter

## Verification

```bash
cargo test -p hares-core weather_integration
cargo test -p hares-core timezone_weather_regressions
```

Test for the leap-year case: construct a synthetic 8784-row weather file, set start time to March 1 00:00, assert weather index step 0 = row 1416 (31 Jan days + 29 Feb days = 1416 hourly steps).
