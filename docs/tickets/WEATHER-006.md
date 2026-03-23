---
id: WEATHER-006
title: DST-aware schedule indexing for civil-time schedules
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/src/environment.rs
references:
  - vendors/OCHRE/ochre/utils/schedule.py (DST handling, lines 635-652)
  - crates/hares-core/src/environment.rs (compute_schedule_offset)
  - https://docs.rs/chrono-tz/latest/chrono_tz/
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

HARES currently uses `FixedOffset` (constant UTC offset) for all time handling.
Weather data itself does not need DST — solar position is computed from UTC and
geographic coordinates. However, **occupancy schedules and utility rate structures**
follow civil time with DST transitions. When a simulation runs through a DST
transition (e.g., March "spring forward" or November "fall back"), schedule indexing
with a fixed offset will be misaligned by 1 hour for part of the year.

OCHRE handles this by supporting `time_zone="DST"` which maps to US pytz timezones
(US/Eastern, US/Central, etc.) that include DST rules. HARES should exceed OCHRE by
supporting arbitrary IANA timezone strings, not just US timezones.

This ticket addresses **schedule indexing only**. Weather indexing remains on fixed
offset (physically correct for solar/meteorological data).

### Correct DST behavior for schedule indexing

The schedule is indexed by civil (wall-clock) time. The simulation clock runs in
continuous time (UTC or fixed offset). To map simulation time → schedule row:

1. Convert simulation timestamp to civil time in the DST-aware timezone
2. Look up the schedule row by civil time

This means:
- **Spring forward** (2:00 AM → 3:00 AM): The simulation clock advances continuously.
  Civil time jumps from 1:59:59 to 3:00:00. Schedule rows for the nonexistent
  2:00-2:59 AM civil hour are simply never accessed — no skipping logic needed.
- **Fall back** (2:00 AM → 1:00 AM): Civil time 1:00-1:59 AM occurs twice. The
  schedule row for that civil hour is used for both occurrences — no repeat logic
  needed.

The key insight is that mapping simulation time → civil time → schedule row handles
both transitions naturally, without special skip/repeat logic.

## Work to Do

- [ ] Add `chrono-tz` as an **optional dependency** behind a `dst` cargo feature:
  - `chrono-tz` 0.10+ embeds the full IANA timezone database (~400KB compiled).
    This exceeds the 500KB threshold for unconditional inclusion.
  - In `crates/hares-core/Cargo.toml`:
    ```toml
    [features]
    dst = ["chrono-tz"]

    [dependencies]
    chrono-tz = { version = "0.10", optional = true }
    ```
  - All DST-related code must be gated with `#[cfg(feature = "dst")]`
  - When the `dst` feature is disabled, `civil_timezone` is not available and
    the build is identical to today's (zero binary size impact)

- [ ] Add `civil_timezone: Option<chrono_tz::Tz>` parameter to
      `EnvironmentManager::new()`. This is a breaking change — update all call sites:
  - `crates/hares-core/src/dwelling/mod.rs:353` — production call site
  - `crates/hares-core/src/environment.rs` — 17 test call sites (lines 665, 681,
    700, 715, 733, 759, 776, 798, 826, 854, 879, 929, 1058, 1089, 1151, 1207, 1221)
  - All test call sites pass `None` (no DST in unit tests unless testing DST)
  - The field is stored on `EnvironmentManager`, NOT on `EnvironmentState` or
    `WeatherState` — it's internal config, not runtime state.

- [ ] Update schedule indexing in `EnvironmentManager::update()`:
  - When `civil_timezone` is `Some(tz)`:
    1. Compute current simulation time as `DateTime<FixedOffset>`
    2. Convert to civil time: `let civil = sim_time.with_timezone(&tz);`
    3. Extract civil time components:
       ```rust
       let doy0 = civil.ordinal0() as u64;
       let h = civil.hour() as u64;
       let m = civil.minute() as u64;
       let s = civil.second() as u64;
       let civil_secs = doy0 * 86400 + h * 3600 + m * 60 + s;
       let schedule_idx = (civil_secs / step_secs as u64) as usize % schedule_len;
       ```
    4. This naturally handles spring-forward (civil time jumps, schedule index
       jumps) and fall-back (civil time repeats, same schedule index reused)
  - When `civil_timezone` is `None`:
    - Use existing `compute_schedule_offset()` unchanged

- [ ] Ensure weather indexing (`compute_annual_offset`) is NOT affected by DST.
      Weather stays on fixed offset. Add a code comment explaining why.

- [ ] Add tests:
  - `schedule_without_dst_unchanged`: verify `None` civil_timezone produces identical
    results to current behavior
  - `schedule_spring_forward_skips_civil_hour`: simulate through March DST transition
    in `America/New_York`, verify schedule row for 2:00 AM is never accessed and
    3:00 AM row is used correctly
  - `schedule_fall_back_reuses_civil_hour`: simulate through November DST transition,
    verify schedule row for 1:00 AM civil time is used for both occurrences
  - `weather_unaffected_by_dst_setting`: verify identical weather state whether
    `civil_timezone` is `Some("America/Denver")` or `None`
  - `year_round_schedule_alignment`: run a full-year simulation with DST enabled,
    verify schedule rows align with civil time at every hour

## Files to Touch

- `crates/hares-core/Cargo.toml`: Add `chrono-tz` optional dep behind `dst` feature
- `crates/hares-core/src/environment.rs`: Add `civil_timezone` parameter to `new()`,
  store on `EnvironmentManager`, update schedule indexing in `update()`, update all
  17 test call sites to pass `None`, add DST-specific tests
- `crates/hares-core/src/dwelling/mod.rs:353`: Update `EnvironmentManager::new()` call

## Measures of Success

- [ ] Occupancy schedules align with civil time through DST transitions
- [ ] Weather indexing is completely unaffected by DST setting
- [ ] `None` civil_timezone produces identical results to current behavior
      (full backward compatibility)
- [ ] Spring-forward and fall-back boundaries produce correct schedule alignment
- [ ] Arbitrary IANA timezone strings work (exceeds OCHRE's US-only DST support)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
