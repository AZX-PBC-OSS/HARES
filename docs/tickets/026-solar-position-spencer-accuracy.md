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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)
  - `environment.rs:659` — `let year_secs = 365_u64 * 86400;` confirmed present (independently re-verified 2026-05-21).
  - `environment.rs:656–658` — comment "EPW files always have 8760 rows (365 × 24 hours); use 365 days unconditionally. / Leap-year starts (ordinal0 ≥ 365) exceed 365*86400 and wrap via modulo, / mapping Dec 31 to an equivalent position in the 365-row array." confirmed present.
  - `environment.rs:191` — `let weather_start_offset = compute_annual_offset(&weather.meta, start_time, step_secs);` confirmed.
  - `environment.rs:217` — `let init_offset = compute_annual_offset(&weather.meta, start_time, step_secs);` confirmed.
  - `solar.rs:93` — `let utc_datetime = local_datetime.to_utc();` confirmed.
  - `solar.rs:105–116` — Spencer (1971) Fourier series for declination (lines 105–109) and equation of time (lines 111–116, using named constants `EOT_C0`–`EOT_C4` at lines 68–72) confirmed.
- [x] Described logic matches current implementation
  - `compute_annual_offset` signature is `fn compute_annual_offset(meta: &WeatherMeta, start_time: DateTime<FixedOffset>, step_secs: u32) -> usize` — the function does **not** receive a weather row count; the ticket's description of the defect is accurate.
  - `WeatherMeta` has `timezone_offset_h: f64` and `midpoint_offset_secs: u32` but **no `is_leap_year` field**. The leap-year flag lives at the `WeatherTimeSeries` level only (via `.len()`). The sole `tracing::warn!` in `environment.rs` (line 804) concerns missing zone volume, not timezone offset mismatch; the timezone-mismatch warning described in the ticket is confirmed absent.
- [x] OCHRE cross-check result: **N/A — OCHRE does not implement its own solar position calculation**. OCHRE delegates to `pvlib.solarposition.get_solarposition()` (`vendors/OCHRE/ochre/utils/envelope.py:141`). No `year_secs`, `compute_annual_offset`, or 365-day modulus equivalent exists in OCHRE. The leap-year row-indexing pattern is therefore unique to HARES and cannot be directly compared.
- [x] EnergyPlus cross-check result: **Diverges from the EPW format specification** — see web-verified citations below. The EPW spec (EnergyPlus 24.2 Auxiliary Programs, Big Ladder) explicitly supports `LeapYear Observed: Yes` and 8784 data records. EnergyPlus's own weather-conversion tooling uses `MaxNumRecordsToRead = 8784` for leap years. The `hares-io` layer already parses 8784-row EPW files (`epw.rs:22`: `const EXPECTED_RECORDS_LEAP: usize = 8784`; `epw.rs:237`: `let is_leap_year = records.len() == EXPECTED_RECORDS_LEAP;`). The flaw is that `compute_annual_offset` in `hares-core` is unaware of this and still hardcodes 365 days.

### Web-Verified Citations

**Citation 1**: EPW format claim — "EPW files always have 8760 rows (365 × 24 hours)"

- **Source found**: EnergyPlus 24.2 Auxiliary Programs — EPW Data Dictionary  
  <https://bigladdersoftware.com/epx/docs/24-2/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html>  
  EnergyPlus Auxiliary Programs (readthedocs): weather converter DEF file examples
- **Quoted passage**: "A1, \\field LeapYear Observed / \\type choice / \\key Yes / \\key No / \\note Yes if Leap Year will be observed for this file; No if Leap Year days (29 Feb) should be ignored in this file" (Big Ladder). Additionally from EnergyPlus readthedocs: `MaxNumRecordsToRead = 8760` (standard year) vs. `MaxNumRecordsToRead = 8784` (leap year).
- **Verdict**: **Incorrect** — the comment in `environment.rs:656` is wrong. The EPW format explicitly supports 8784-row (leap-year) files; EnergyPlus's own tooling formalises the 8784-row count. The `hares-io` parser already handles this correctly. The comment is a stale assertion that predates 8784-row EPW support in the IO layer.

**Citation 2**: PSM3 / TMY3 leap-year row counts

- **Source found**: NREL NSRDB documentation; HARES codebase `tmy3.rs:38`, `psm3.rs:27`
- **Quoted passage** (HARES `tmy3.rs:38`): `const EXPECTED_RECORDS_LEAP: usize = 8784;` — and `psm3.rs:27`: `/// Expected record counts for leap year at each supported interval.`
- **Verdict**: **Confirmed** — PSM3 and TMY3 parsers in HARES already handle 8784-row leap-year files.

**Citation 3**: Spencer (1971) Fourier-series declination model — claimed accuracy of "±0.03° in solar zenith"

- **Source found**: Wikipedia "Position of the Sun" <https://en.wikipedia.org/wiki/Position_of_the_Sun>; multiple academic and solar-modeling references (NREL SPA page, PlantPredict, blog citing Iqbal 1983).
- **Quoted passage** (Wikipedia, fetched 2026-05-21): *"The 1971 Spencer formula (based on a Fourier series) is also discouraged for having an error of up to 0.28°."* Secondary source: *"Simple formulas (Cooper, 1969; Spencer, 1971; Swift, 1976; Lamm, 1981) that find the declination or the equation of time usually have errors of the order of tenths of degree."*  
  The ±0.03° figure does appear in the Wikipedia article, but it refers to a **different, per-year-adjusted formula**, not to Spencer (1971) itself: *"Same formula, adjusted annually — less than ±0.03° for a given year."*
- **Verdict**: **Incorrect** — the ticket's claim of "±0.03° in solar zenith" for Spencer (1971) is unsupported. The correct figure is up to **±0.28°** in declination/zenith, roughly an order of magnitude worse. The ±0.03° figure belongs to a distinct per-year-adjusted variant, not the Fourier series implemented in HARES. This does not change the severity verdict on the primary (leap-year) defect; Spencer is correctly treated as a secondary concern in the ticket.

**Citation 4**: NREL/TP-560-34302, Reda & Andreas (2004/2008) SPA accuracy

- **Source found**: NREL SPA page <https://midcdmz.nrel.gov/spa/>; PlantPredict Solar Geometry docs; multiple secondary sources citing NREL/TP-560-34302.
- **Quoted passage** (NREL SPA page, fetched 2026-05-21): *"uncertainties of +/- 0.0003 degrees based on the date, time, and location on Earth."* PlantPredict: *"The algorithm achieves uncertainties of ±0.0003° for the period −2000 to 6000."*
- **Verdict**: **Confirmed** — the report exists, the citation is real, and the ±0.0003° accuracy figure is correct. The ticket's description of SPA as the reference algorithm is accurate.

**Citation 5**: Spencer (1971) declination and EOT coefficients in HARES vs. pvlib reference

- **Source found**: pvlib-python <https://raw.githubusercontent.com/pvlib/pvlib-python/main/pvlib/solarposition.py>; pvlib docs for `declination_spencer71` and `equation_of_time_spencer71`.
- **Quoted passage** (pvlib `declination_spencer71`, fetched 2026-05-21):  
  `0.006918 - 0.399912*cos(B) + 0.070257*sin(B) - 0.006758*cos(2B) + 0.000907*sin(2B) - 0.002697*cos(3B) + 0.00148*sin(3B)`  
  pvlib `equation_of_time_spencer71`:  
  `0.0000075 + 0.001868*cos(B) - 0.032077*sin(B) - 0.014615*cos(2B) - 0.040849*sin(2B)`
- **Verdict**: **Confirmed** — HARES `solar.rs:105–109` uses identical declination coefficients, and `solar.rs:68–72` uses identical EOT coefficients (`EOT_C0 = 0.000_007_5`, etc.). Notably, HARES `solar.rs:66–67` includes an explicit comment that `0.0000075` corrects a misprint of `0.000075` in the original Spencer (1971) paper — matching pvlib's documented correction. All 12 coefficients (7 declination + 5 EOT) agree to full floating-point precision.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The primary defect is real and confirmed by independent code inspection and a failing regression test: `compute_annual_offset` at `environment.rs:659` unconditionally hardcodes `year_secs = 365_u64 * 86400`, causing incorrect row indexing for any 8784-row leap-year weather file. The failure mode — Dec 31 00:00 of a leap year reads row 0 (Jan 1, `dry_bulb = 0.0`) instead of row 8760 (Dec 31, `dry_bulb = 8760.0`) — is pinned by the regression test `leap_year_dec31_weather_index_bug_026`, which fails with `left: 0.0, right: 8760.0` when run against the current codebase. The secondary defect (missing timezone-mismatch `tracing::warn!`) is also confirmed absent. The `hares-io` EPW/PSM3/TMY3 parsers already handle 8784-row files correctly; the gap is exclusively in `hares-core/environment.rs`. The EPW format's official support for leap-year files (confirmed via Big Ladder and EnergyPlus readthedocs) further validates the ticket. The only inaccurate claim is that Spencer (1971) has accuracy of "±0.03° in solar zenith" — the correct figure is up to ±0.28°, independently confirmed via Wikipedia and academic sources. This does not affect the primary fix priority; the Spencer model is correctly identified as a secondary concern. All five citations have been independently web-verified.

### Proposed Fix Summary

1. **Primary fix** (`compute_annual_offset`): add a `weather_hourly_rows: usize` parameter. Replace `let year_secs = 365_u64 * 86400;` with `let year_secs = weather_hourly_rows as u64 * 3600;`. At the two call sites (`environment.rs:191` and `217`), pass `weather.len()` (raw hourly row count). For PSM3 sub-hourly files, pass the equivalent hourly count. Update existing `compute_annual_offset` unit tests to supply the row count argument.
2. **Secondary fix** (`EnvironmentManager::new`): after constructing the weather object, compute `provided_offset_secs = start_time.offset().local_minus_utc()` and `file_offset_secs = (weather.meta.timezone_offset_h * 3600.0).round() as i32`; if `(provided_offset_secs - file_offset_secs).abs() > 1800`, emit `tracing::warn!` naming both values. No breaking API change required.
3. **Documentation fix** (ticket and code comment only): correct "±0.03° in solar zenith" to "up to ±0.28° in declination/zenith" for the Spencer (1971) model throughout the ticket text.

### Test Written

- **File**: `crates/hares-core/tests/weather_integration.rs`
- **Function**: `leap_year_dec31_weather_index_bug_026` (present in codebase as of 2026-05-21)
- **What it tests**: Constructs an 8784-row leap-year `WeatherTimeSeries` where `dry_bulb_c[i] == i as f64`. Starts simulation at Dec 31 00:00 UTC of leap year 2020 (ordinal0 = 365). Asserts that step 0 reads `dry_bulb = 8760.0` (row 8760 = Dec 31). The 365-day modulus wraps `365 * 86400 % (365 * 86400) = 0`, yielding row 0.
- **Confirmed failing** (2026-05-21): `cargo test -p hares-core --test weather_integration leap_year_dec31_weather_index_bug_026` → `FAILED` — `assertion failed: left: 0.0, right: 8760.0`
