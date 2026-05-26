# Clock DST transition edge cases: spring-forward 23h, fall-back 25h, leap day Feb-29
**Review ID**: coredeep-01
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/clock.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: `SimClock` uses `FixedOffset` — cannot independently model DST transitions [Severity: high]
**Description**: `SimClock` uses `DateTime<FixedOffset>` for all timestamps and `current_time()` computes `start_time + Duration::seconds(N * time_res)` using naive fixed-offset arithmetic. A `FixedOffset` does not change across DST boundaries. The result is that `SimClock` is a linear time-counter with no knowledge of civil (wall-clock) time discontinuities. Any code path that reads `.hour()`, `.minute()`, `.ordinal()`, or other calendar fields directly from `clock.current_time()` without first converting to a DST-aware timezone via `.with_timezone(&tz)` will observe fictitious wall-clock times during DST transitions.

**Code Location**: `crates/hares-core/src/clock.rs:28-58`

**Root Cause**: The clock is declared with `pub start_time: DateTime<FixedOffset>` (line 29) and `current_time()` (lines 55-58) does:
```rust
self.start_time + Duration::seconds(time_res_secs.saturating_mul(step_secs))
```
The offset carried in `start_time` is preserved in every `+ Duration` operation. On a spring-forward day (e.g., 2024-03-10 in US/Eastern), the clock produces wall-clock digits that include `02:00 EST` — an hour that does not exist in civil time. On a fall-back day (2024-11-03), the clock produces `01:00 EDT` for the first hour-after-midnight but never reproduces the ambiguous second `01:00 EST`.

**Impact**: 
- Any consumer that interprets `clock.current_time()` as true civil wall-clock time is broken during DST transitions.
- The `EnvironmentManager` correctly mitigates this for schedule indexing (via `compute_schedule_idx` at environment.rs:390-408, which uses `with_timezone()`) and weather indexing (step-based, unaffected by DST). However, `day_of_year` computations in the environment update path (see Finding 2) do not apply the DST-aware conversion.
- This is a known design choice — the module docs (clock.rs:3-18) state "the offset is effectively ignored" — but the downstream consequences are material.

---

### Finding 2: `day_of_year` derived from raw `DateTime<FixedOffset>` — off-by-one near spring-forward boundaries [Severity: medium]
**Description**: The `day_of_year` value stored in `EnvironmentState.weather.day_of_year` (environment.rs:657) and used for solar irradiance and mains water temperature (environment.rs:511-517) is computed as `clock.current_time().ordinal()` — i.e., the ordinal from the raw `DateTime<FixedOffset>`, not from the DST-aware civil time. During the spring-forward transition, the clock's stored `DateTime<FixedOffset>` can fall on a different calendar day than the true civil time.

**Concrete example**: With `start_time = 2024-03-10T00:00:00-05:00` (EST, spring-forward day) and hourly steps:
- Step 23: `clock.current_time()` = `2024-03-10T23:00:00-05:00`; `.ordinal()` yields March 10 (day 69).
- Civil time via `with_timezone(&America/New_York)`: `2024-03-11T00:00:00-04:00 EDT`; ordinal yields March 11 (day 70).
- The clock reports day 69 while the true civil day is 70.

**Code Location**:
- `crates/hares-core/src/environment.rs:511` — `let day_of_year = now.ordinal();`
- `crates/hares-core/src/environment.rs:657` — `state.weather.day_of_year = clock.current_time().ordinal() as f64;`
- `crates/hares-core/src/environment.rs:543,557` — `day_of_year` passed to `omni_directional_irradiance()` and `perez_tilted_irradiance()`

**Root Cause**: The `day_of_year` extraction does not apply the DST-aware civil timezone conversion (`with_timezone()`) that is already available in the `EnvironmentManager` via `self.civil_tz`.

**Impact**: 
- Off-by-one day error in day-of-year affects extraterrestrial irradiance (ETR) computation and mains water temperature for up to one hour per spring-forward day per year. The ETR error magnitude is ~0.4% (Earth's orbital eccentricity), which is negligible in practice. The mains temperature error is comparably small.
- At a 15-minute simulation resolution, this affects 4 timesteps per year.
- The `solar_position()` function (solar.rs:93) correctly uses `.to_utc()` internally and gets the correct UTC-based ordinal regardless — see Finding 3 for the inconsistency.

---

### Finding 3: Inconsistent day-of-year sources — `solar_position` uses UTC ordinal; Perez functions use local ordinal [Severity: medium]
**Description**: The solar position and irradiance pipeline has an inconsistency in day-of-year computation. `solar_position()` (solar.rs:88-102) converts to UTC internally and uses `utc_datetime.ordinal()`. However, `omni_directional_irradiance()` and `perez_tilted_irradiance()` receive `day_of_year` as a parameter derived from `clock.current_time().ordinal()` (the local `FixedOffset` ordinal). During the spring-forward boundary hour, the UTC day-of-year can differ from the local day-of-year by 1.

**Code Location**:
- `crates/hares-physics/src/solar.rs:93-95` — UTC-based ordinal in `solar_position()`
- `crates/hares-core/src/environment.rs:511` — `let day_of_year = now.ordinal();` (local ordinal)
- `crates/hares-core/src/environment.rs:543` — `day_of_year` passed to Perez function
- `crates/hares-core/src/environment.rs:557` — `day_of_year` passed to Perez function

**Root Cause**: Two different ordinal sources are used within the same per-step environment-update call. `solar_position()` independently derives UTC from the `DateTime<FixedOffset>` parameter and extracts UTC ordinal. The Perez irradiance functions receive a separately-computed local ordinal. In the steady state these match; during the DST transition boundary hour they can differ.

**Impact**: During the spring-forward transition hour, solar declination (from `solar_position()`) and extraterrestrial irradiance (from Perez functions) are computed for different days. The combined error in total plane-of-array irradiance is negligible in practice but represents a correctness inconsistency.

---

### Finding 4: `total_steps()` ignores DST day-length variation [Severity: low]
**Description**: `SimClock::total_steps()` (clock.rs:65-75) divides `duration.num_seconds() / time_res.num_seconds()` using integer arithmetic. A `Duration::days(1)` is always 86,400 seconds, regardless of whether the calendar day is 23h (spring-forward), 24h, or 25h (fall-back). The simulation always runs for an integer number of physical seconds, which means:
- On a spring-forward Sunday, a "1 calendar day" simulation covers 24 physical hours (not 23 civil hours).
- On a fall-back Sunday, a "1 calendar day" simulation covers 24 physical hours (not 25 civil hours), omitting 1 civil hour from the simulated window.

**Code Location**: `crates/hares-core/src/clock.rs:65-75`

**Root Cause**: The clock is a physical time counter, not a calendar iterator. `Duration::days(N)` is a fixed number of seconds, not a calendar-aware duration. The `chrono::Duration::days()` function always returns exactly `N * 86400` seconds.

**Impact**: Users who specify simulation duration in terms of calendar days (e.g., `Duration::days(30)`) will get 30 × 86400 = 2,592,000 seconds of simulation, not 30 actual calendar days. During months with a DST transition, this is off by ±1 hour of civil time. For most energy simulation use cases (30-day or annual horizons), this 1-hour discrepancy per affected month is negligible. For sub-daily simulations crossing a DST boundary, the schedule indexing (which IS DST-aware when the feature is enabled) produces the correct schedule values for the physical hours simulated.

---

### Finding 5: Leap day Feb-29 — relies entirely on `chrono` calendar arithmetic [Severity: low]
**Description**: `SimClock` has no independent leap-year awareness. It relies on `chrono`'s `DateTime<FixedOffset>` + `Duration` arithmetic to correctly handle Feb 29. When `start_time` is in a leap year and the clock advances past Feb 29, chrono handles the month-length boundary correctly.

**Code Location**: `crates/hares-core/src/clock.rs:55-58`

**Root Cause**: The clock is a duration-based iterator. Leap-year correctness is entirely delegated to `chrono`'s calendar-aware `DateTime + Duration` operator.

**Impact**: 
- Single-year leap-year simulations function correctly because `chrono` handles Feb 29 in the arithmetic.
- Multi-year simulations (wrapping weather/schedule data) could produce incorrect Feb 29 behavior in non-leap follow-on years. However, `EnvironmentManager`'s weather indexing (environment.rs:700-724) derives `year_secs` from the actual weather file length, and schedule indexing uses `civil.ordinal0()` which chrono adjusts for the correct year. This means the downstream consumers (weather, schedule) are handled correctly regardless of `SimClock`'s naivety.
- No known bug remains; this is documented for completeness.

---

### Finding 6: DST feature is opt-in and not enabled by default [Severity: low]
**Description**: DST-aware schedule indexing is gated behind `#[cfg(feature = "dst")]` in the `EnvironmentManager` (environment.rs:112-113, 191-198, 394-408). The `SimClock` itself has no DST support. Without the `dst` feature, any attempt to pass a `civil_timezone` IANA string returns `EnvironmentManagerError::DstNotEnabled`. This means:
1. `SimClock` is purely a fixed-offset clock by default.
2. Schedule indexing uses fixed-offset wall-clock arithmetic by default.
3. Weather indexing is always step-based and unaffected.

**Code Location**:
- `crates/hares-core/Cargo.toml:14` — `dst = []`
- `crates/hares-core/src/environment.rs:191-202` — feature-gated civil_tz parsing
- `crates/hares-core/src/environment.rs:394-408` — feature-gated DST-aware schedule indexing
- `crates/hares-core/tests/timezone_weather_regressions.rs:228-303` — feature-gated DST tests

**Root Cause**: The `chrono-tz` dependency and IANA timezone parsing are optional, keeping the default binary smaller and avoiding the `chrono-tz` compile time cost for users who simulate in fixed-offset timezones.

**Impact**: Deployment in North American or European regions with DST-observing timezones requires enabling the `dst` cargo feature and providing a valid IANA timezone string (e.g., `"America/New_York"`, `"America/Denver"`, `"Europe/London"`) to the `EnvironmentManager`. Without this, tariff TOU periods defined in local wall-clock time will be misaligned during DST months, producing incorrect energy-cost results for the ~7 months of DST each year. This is correctly documented but represents a deployment prerequisite that could be overlooked.

---

## Summary
- Total findings: 6
- Critical: 0
- High: 1
- Medium: 2
- Low: 3

## Recommendations

1. **Add DST-aware `day_of_year` computation in `EnvironmentManager::update()`** (Findings 2, 3): When `self.civil_tz` is `Some`, derive `day_of_year` from the DST-aware civil time rather than from the raw `DateTime<FixedOffset>`. This eliminates the 1-hour-per-year off-by-one in solar irradiance and mains temperature computations. The change would be:
   - In environment.rs:511, replace `let day_of_year = now.ordinal();` with a branch: if DST-aware civil_tz is available, use `now.with_timezone(&self.civil_tz).ordinal()`.
   - Same for environment.rs:657.

2. **Document the `dst` feature prominently in README or user-facing docs** (Finding 6): Make it clear that deploying in any region that observes daylight saving time requires `--features dst` and a valid IANA timezone string. The current `clock.rs` module docs mention this but users of the public API may not see it.

3. **Consider adding a `SimClock::current_civil_time()` accessor** (Finding 1): A convenience method that returns the DST-aware civil time (when a timezone is configured) would reduce the risk of consumers accidentally using the raw `FixedOffset` time for calendar-sensitive computations. This would be most impactful as `SimClock` and `EnvironmentManager` are in the same crate.

4. **Add regression test for day-of-year across DST boundaries** (Findings 2, 3): The existing `timezone_weather_regressions.rs` tests cover schedule and weather indexing across DST transitions but do not assert `day_of_year` values. A test that starts at 2024-03-10T00:00:00 EST and checks `weather.day_of_year` for steps near the spring-forward transition would catch the ordinal discrepancy.

5. **Consider explicit `SimClock` DST-awareness** (Finding 1): Long-term, consider whether `SimClock` should accept an optional `chrono_tz::Tz` and use it for all calendar-field accessors. This would make DST handling a first-class concern rather than something delegated wholly to downstream consumers. The current separation works but creates a maintenance burden where every new consumer of `clock.current_time()` must remember to apply `with_timezone()`.

## References / Citations
- `chrono` crate documentation: `DateTime<FixedOffset>` + `Duration` maintains the same offset; DST transitions require `chrono_tz::Tz` and `.with_timezone()`.
- IANA timezone database: Spring-forward in `America/New_York` occurs at 2024-03-10T07:00:00Z (02:00 EST → 03:00 EDT). Fall-back occurs at 2024-11-03T06:00:00Z (02:00 EDT → 01:00 EST).
- Existing tests in `crates/hares-core/tests/timezone_weather_regressions.rs` validate DST-aware schedule indexing and weather progression but do not cover day-of-year ordinal correctness.
- `compute_annual_offset` (environment.rs:700-724) correctly handles leap-year weather files by deriving `year_secs` from the actual array length rather than hardcoding 365 days.
