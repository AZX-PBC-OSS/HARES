# DST schedule indexing and hour-skip/duplicate handling
**Review ID**: core-06
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/clock.rs`
- `crates/hares-io/src/schedule.rs`
- `crates/hares-io/src/schedule_resolve.rs`
- `crates/hares-core/src/environment.rs` (primary DST logic)
- `crates/hares-io/src/epw.rs` (EPW parsing, DST header handling)
- `crates/hares-core/tests/timezone_weather_regressions.rs` (DST regression tests)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc`

## Findings

### Finding 1: [Severity: medium]
**Description**: EPW HOLIDAYS/DAYLIGHT SAVING header is parsed but discarded. HARES cannot use DST transition dates embedded in weather files, relying entirely on IANA timezone lookups via the `civil_timezone` configuration option. EnergyPlus uses both sources: EPW-embedded `DaylightSaving` period data and user-specified `RunPeriodControl:DaylightSavingTime`.

**Code Location**: `crates/hares-io/src/epw.rs:105-106` — the `holidays_daylight_line` variable is read from line 5 of the EPW header but bound with `let _` (line 120) and never used.

**Root Cause**: The DST handling in HARES is delegated entirely to the `chrono-tz` IANA timezone database via `EnvironmentManager::new_with_resample` → `crates/hares-core/src/environment.rs:192-198`. The EPW header data sources (`EPWDaylightSaving` vs `IDFDaylightSaving`) that EnergyPlus uses at `WeatherManager.cc:1100-1101` and `WeatherManager.cc:5832` (`GetDSTData`) have no HARES counterpart. No comparison is made with EnergyPlus's DST date range resolution at `WeatherManager.cc:1524-1621` (`SetDSTDateRanges`).

**Impact**: Users simulating historical years where DST rules differed from current IANA data (e.g., pre-2007 US DST rules where spring-forward/fall-back occurred on different dates) will see incorrect schedule indexing because `chrono_tz` applies contemporary rules regardless of the simulation year. The EPW file's embedded DST info, which could provide the correct historical transition dates, is ignored.

### Finding 2: [Severity: low]
**Description**: `compute_schedule_idx` in `environment.rs:391-412` uses civil wall-clock hour arithmetic (`doy0 * 86400 + h * 3600 + m * 60 + s`) for index computation. This formula assumes all days have exactly 86400 seconds, but DST transition days have 82800 or 90000 civil seconds. While the civil-hour-skip/repeat produces correct indices for each wall-clock hour individually (the skipped hour is naturally absent and the repeated hour is naturally present twice), this formula means the index space fails to represent the DST transition day's true length. For schedules with `step_secs` shorter than a DST gap (e.g., 30-minute schedules), the index compaction could cause an hour-off alignment error — the schedule row that would correspond to "day boundary + part of the 25th hour" on a fall-back day is unreachable because `civil.hour()` never exceeds 23.

**Code Location**: `crates/hares-core/src/environment.rs:397-408`
```rust
let doy0 = civil.ordinal0() as u64;
let h = civil.hour() as u64;
let m = civil.minute() as u64;
let s = civil.second() as u64;
let civil_secs = doy0 * 86400 + h * 3600 + m * 60 + s;
```

**Root Cause**: The formula maps civil time to an annual index assuming uniform 86400-second days. On fall-back, the second instance of the duplicated hour (EST) produces the same `civil_secs` as the first (EDT), so both schedule lookups return identical values. This is correct behavioral semantics. However, for sub-hourly schedules, the duplicated hour is treated identically to the first occurrence — there is no way for a schedule to distinguish "first 01:00" from "second 01:00". EnergyPlus does distinguish these two instances via its `DSTIndicator` + timestamp mechanism (`WeatherManager.cc:2024`, `WeatherManager.cc:2860-2861`), allowing weather-aware equipment to differentiate between the two civil-hour occurrences.

**Impact**: Low. In practice, schedule fractions typically repeat identical values across the duplicated hour (the test at `timezone_weather_regressions.rs:269-303` confirms `vec![0.0, 1.0, 1.0, 2.0, 3.0]` — both instances of hour 1 return 1.0). Equipment that needs to distinguish the two instances (e.g., for cumulative energy tracking) would require changes to the index formula.

### Finding 3: [Severity: low]
**Description**: `infer_step_secs` in `schedule.rs:524-552` rejects any schedule CSV whose timestamp intervals are non-uniform, including timestamps that contain DST transitions. This means EPW files with DST-shifted local-clock timestamps cannot be loaded as schedule CSVs. The design is consistent with HARES's DST architecture (DST is handled at the indexing layer, not in the data), but the error message does not mention DST as a possible cause of non-uniform steps.

**Code Location**: `crates/hares-io/src/schedule.rs:539-546`
```rust
for i in 1..(timestamps.len() - 1) {
    let delta = (timestamps[i + 1] - timestamps[i]).num_seconds();
    if delta != step_secs {
        return Err(ScheduleError::Validation(format!(
            "non-uniform timestep at row {}: expected {step_secs}s, found {delta}s",
            i + 2
        )));
    }
}
```

**Root Cause**: The uniform-step validation is intentionally strict to prevent data inconsistencies. However, a user loading an EPW file as a schedule (which is a plausible use case for custom schedule injection) would encounter a validation error if the EPW uses local clock time with DST shifts. The error message `"non-uniform timestep at row ...: expected 3600s, found 0s"` (or `7200s` for fall-back) would be confusing without DST context.

**Impact**: Low. The failure is safe (the data is rejected rather than silently mis-indexed), but the diagnostic is unhelpful. The known workaround is to use `civil_timezone` configuration instead of embedding DST in the schedule CSV.

### Finding 4: [Severity: low]
**Description**: `SimClock` (`clock.rs:28-89`) is pure arithmetic (`current_time = start_time + step * time_res`) with no DST awareness. Its `.hour()`, `.weekday()`, `.ordinal0()` methods return the fixed-offset wall-clock values, which may differ from civil time during DST transitions. The module docs (`clock.rs:3-18`) describe the local-time convention but do not cross-reference the `civil_timezone` mechanism in `EnvironmentManager`, creating a discoverability gap for developers working on schedule-related code.

**Code Location**: `crates/hares-core/src/clock.rs:55-58`
```rust
pub fn current_time(&self) -> DateTime<FixedOffset> {
    let time_res_secs = self.time_res.num_seconds();
    let step_secs = i64::try_from(self.current_step).unwrap_or(i64::MAX);
    self.start_time + Duration::seconds(time_res_secs.saturating_mul(step_secs))
}
```

**Root Cause**: The design intentionally separates physical timekeeping (`SimClock`) from civil-DST schedule indexing (`EnvironmentManager::compute_schedule_idx`). But the module-level documentation does not mention that `current_time().hour()` is the fixed-offset wall-clock hour, not the DST-aware civil hour. A developer reading only `clock.rs` would reasonably assume `.hour()` on the return value of `current_time()` can be used for schedule lookups, which is incorrect when `civil_timezone` is active.

**Impact**: Low. The design is correct, but the documentation gap could lead to misuse in new code that accesses `current_time()` directly for schedule indexing instead of going through `EnvironmentManager`.

### Finding 5: [Severity: low]
**Description**: The `generate_annual_timestamps` function (`schedule.rs:481-511`) hardcodes year 2007 for index-based schedule timestamps, with the stated rationale of avoiding leap-year Feb 29 dates. When DST is enabled via `civil_timezone`, calling `with_timezone(&tz)` on a `DateTime<FixedOffset>` whose year is 2007 applies the 2007 DST rules to the schedule indexing, regardless of the simulation's actual year. This is benign because schedules in HARES are cyclic annual data where only the wall-clock hour matters — but the coupling is implicit and undocumented.

**Code Location**: `crates/hares-io/src/schedule.rs:497`
```rust
let year: i32 = 2007;
```

**Root Cause**: The year 2007 was chosen as a convenient non-leap year matching BEopt defaults. The DST transition dates for 2007 are: March 11 (second Sunday) and November 4 (first Sunday). For index-based schedules with `civil_timezone` in the same US timezone, this produces the correct DST day pattern (23-hour spring-forward day on March 11, 25-hour fall-back day on November 4). However, for index-based schedules generated by this function, the DST transitions depend on the IANA timezone rules for 2007, not the simulation year specified in `start_time`. This is currently irrelevant because index-based schedules are always generated with year 2007, and the simulation `current_time()` (used in `compute_schedule_idx`) uses `start_time`'s year. But if a future code change varies the schedule timestamp year, there would be a mismatch.

**Impact**: Low. No actual bug exists. This is a documentation gap about the interaction between the hardcoded schedule-generation year and the DST-aware schedule indexing across different simulation years.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 1 / 4

## Assessment

The DST schedule indexing implementation in HARES is functionally correct for its intended use cases. The key architectural decisions are sound:

1. **Spring-forward**: `compute_schedule_idx` correctly skips the nonexistent civil hour (2:00-3:00 AM) because `chrono_tz`'s `with_timezone` conversion maps the UTC instant to the next valid civil hour (3:00 AM). Verified by test `schedule_spring_forward_skips_civil_hour` (`environment.rs:2369-2403`) producing `vec![0.0, 1.0, 3.0, 4.0, 5.0]` and by regression test `spring_forward_civil_timezone_skips_schedule_hour_but_not_weather_hour` (`timezone_weather_regressions.rs:230-265`).

2. **Fall-back**: Both instances of the duplicated civil hour (1:00-2:00 AM) correctly produce the same schedule index. Verified by test `schedule_fall_back_reuses_civil_hour` (`environment.rs:2409-2444`) producing `vec![0.0, 1.0, 1.0, 2.0, 3.0]` and by regression test `fall_back_civil_timezone_repeats_schedule_hour_but_not_weather_hour` (`timezone_weather_regressions.rs:269-303`).

3. **Separation of concerns**: Weather indexing is sequential-position-based — DST transitions never affect weather data lookups. Schedule indexing is optionally DST-aware via `civil_timezone`. Verified by test `weather_unaffected_by_dst_setting` (`environment.rs:2449-2490`).

4. **Year-round consistency**: For a non-leap year (8760 hours), a full-year simulation with DST produces correct schedule-to-civil-time alignment at every hour. Verified by test `year_round_schedule_alignment` (`environment.rs:2495-2528`).

Compared to EnergyPlus:
- HARES uses IANA timezone database vs. EnergyPlus's explicit DST date rules.
- HARES ignores EPW-embedded DST data; EnergyPlus can use it.
- Both correctly separate weather data (always sequential) from schedule indexing (optionally DST-aware).
- HARES's chrono-based approach is simpler and more maintainable but less configurable for non-standard historical DST rules.

## Recommendations

1. **Parse and optionally use EPW DST header data** (`epw.rs:105-106`). When the EPW file contains DST transition dates in the HOLIDAYS/DAYLIGHT SAVING header, extract them and compare against the IANA-derived transitions. If they differ (e.g., for historical weather files with pre-2007 US DST rules), emit a warning. This closes the gap between HARES and EnergyPlus's EPW DST handling.

2. **Document the `current_time().hour()` vs. civil-hour distinction** in `clock.rs` module docs. Add a cross-reference to `EnvironmentManager::compute_schedule_idx` noting that `current_time().hour()` is the fixed-offset wall-clock hour, not the DST-aware civil hour.

3. **Improve the `infer_step_secs` error message** to hint at DST as a possible cause when a timestamp gap of exactly 0 or 7200 seconds is detected between consecutive rows. Example: `"non-uniform timestep at row N: expected 3600s, found 0s (possible DST spring-forward in timestamp data)"`.

4. **Add a debug assertion or documentation note** clarifying that `generate_annual_timestamps` hardcodes year 2007, and that the DST rules for that year (not the simulation year) govern the generated timestamps' civil-time behavior when `civil_timezone` is active on index-based schedules.

## References / Citations
- HARES DST schedule indexing: `crates/hares-core/src/environment.rs:383-412` (`compute_schedule_idx`)
- HARES DST feature gate: `crates/hares-core/src/environment.rs:190-198` (`civil_tz` parsing)
- HARES DST regression tests: `crates/hares-core/tests/timezone_weather_regressions.rs:228-303`
- HARES DST unit tests: `crates/hares-core/src/environment.rs:2340-2528`
- HARES EPW DST header discard: `crates/hares-io/src/epw.rs:105-106,120`
- HARES uniform-timestep validation: `crates/hares-io/src/schedule.rs:524-552`
- HARES index-based timestamp generation: `crates/hares-io/src/schedule.rs:481-511`
- EnergyPlus DST indicator setup: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:1100-1107`
- EnergyPlus DST date range computation: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:1524-1621`
- EnergyPlus DST data input: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:5832` (`GetDSTData`)
- EnergyPlus daily DST indicator: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:2024`
- EnergyPlus tomorrow DST indicator: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:2860-2861`
