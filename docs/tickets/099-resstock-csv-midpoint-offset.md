# ResStock CSV Midpoint Offset Should Be 1800 Seconds, Not Zero

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/resstock_csv

## Problem

`crates/hares-io/src/resstock_csv.rs:337` sets `midpoint_offset_secs = 0` for ResStock weather records. ResStock CSV timestamps are end-of-interval (per the ResStock weather format documentation), the same convention as TMY3. The TMY3 path was corrected by the B4 fix to `1800` (30 minutes); the ResStock path was not. The bug is structurally identical to the pre-B4 TMY3 defect.

Effect: solar position calculations, solar-time alignment, and any sub-hourly resampling that consumes the midpoint timestamp are off by 30 minutes when the dwelling is configured with a ResStock weather CSV. The systematic offset corrupts solar load magnitudes at sunrise and sunset and biases peak-load timing.

## Current Behavior

`crates/hares-io/src/resstock_csv.rs:337`:
```rust
midpoint_offset_secs: 0,  // wrong; ResStock timestamps are end-of-interval
```

No test guards this value. The simulation runs and produces plausible-looking results biased by 30 minutes in solar-time alignment.

## Required Behavior

1. Set `midpoint_offset_secs = 1800` for ResStock CSV records, matching the TMY3 fix.
2. Add a regression test pinning the value (analogous to ticket 097 for TMY3).
3. Add an integration test confirming solar position computed from a ResStock CSV at 12:00 timestamp produces the same solar zenith as a TMY3 fixture at 12:00, when both are interpreted with the correct midpoint offset.

## Approach

1. Open `crates/hares-io/src/resstock_csv.rs:337` and change the literal to `1800`.
2. Add inline comment citing the ResStock weather format (end-of-interval) and the parallel TMY3 fix.
3. Add a unit test in `crates/hares-io/tests/` asserting `meta.midpoint_offset_secs == 1800` for a representative ResStock fixture.
4. Cross-validate solar position calculations between a ResStock CSV and a TMY3 fixture for the same site/date — they should agree to within solar-position numerical precision once both midpoint offsets are correct.

## Definition of Done

- [ ] `crates/hares-io/src/resstock_csv.rs:337` sets `midpoint_offset_secs = 1800`
- [ ] Regression test pins the value
- [ ] Cross-validation test: solar zenith from ResStock CSV at noon-timestamp matches TMY3 reference for same site/date within 0.01°
- [ ] Inline comment cites the ResStock format and the TMY3 parallel fix

## Verification

```bash
cargo test -p hares-io resstock
cargo test -p hares-io solar_position
```

## References

- ResStock Weather Format documentation (NREL — see `reference_resstock_weather_format` project memory) — ResStock CSV timestamps are end-of-interval.
- NSRDB TMY3 User's Manual (NREL/TP-581-43156, 2007) §3 — analogous end-of-interval convention.
- HARES B4 fix at `crates/hares-io/src/tmy3.rs:417` — the parallel fix already in place for TMY3.

## Related Tickets

- 097-tmy3-midpoint-offset-regression-test (analogous TMY3 regression test)
- 029-resstock-csv-constant-pressure (related ResStock CSV defect)

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match (with correction): ticket cites line 337 for `midpoint_offset_secs: 0` — **confirmed at line 337** in `crates/hares-io/src/resstock_csv.rs`. Ticket also cites B4 fix at `tmy3.rs:417`; actual location is **line 244** (line number stale).
- [x] Described logic matches current implementation: `WeatherMeta { midpoint_offset_secs: 0, ... }` at `resstock_csv.rs:337` confirmed by `grep`.
- [x] OCHRE cross-check result: **diverges — but with important nuance**. OCHRE's `schedule.py:187–195` routes ResStock/NSRDB CSV files through `pvlib.iotools.read_psm3(weather_file, map_variables=True)` with `offset = None`. However, OCHRE uses the **NSRDB PSM3 format** (Year/Month/Day/Hour/Minute columns, beginning-of-interval, first row = `2008-01-01 00:00`), which is a structurally different file format from the HARES `resstock_csv.rs` parser, which handles the **ResStock AMY simplified 8-column CSV** (`date_time` string column, first row = `2018-01-01 01:00:00`, end-of-interval). OCHRE's `offset = None` reflects the PSM3 beginning-of-interval convention; it does NOT mean the ResStock AMY simplified CSV format is beginning-of-interval.
- [x] EnergyPlus cross-check result: **N/A** — `midpoint_offset_secs` is a HARES-internal field representing the shift from end-of-interval timestamps to interval midpoints. EnergyPlus does not have an equivalent field; it internally shifts EPW timestamps by 30 minutes in solar calculations. The HARES field correctly abstracts this convention.

### Web-Verified Citations

**Citation 1**: "ResStock Weather Format documentation (NREL) — ResStock CSV timestamps are end-of-interval."
- **Source found**: Real NREL ResStock AMY 2018 CSV files fetched directly from the NREL public S3 bucket: `https://oedi-data-lake.s3.amazonaws.com/nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/2025/comstock_amy2018_release_1/weather/amy2018/G0100630_2018.csv` and `G5107750_2018.csv`.
- **Quoted passage** (header + first two rows of `G0100630_2018.csv`):
  ```
  date_time,Dry Bulb Temperature [°C],Relative Humidity [%],Wind Speed [m/s],Wind Direction [Deg],Global Horizontal Radiation [W/m2],Direct Normal Radiation [W/m2],Diffuse Horizontal Radiation [W/m2]
  2018-01-01 01:00:00,-5.6,43.318985334884644,6.2,360.0,0.0,0.0,0.0
  2018-01-01 02:00:00,...
  ```
  First data row starts at `01:00:00`, not `00:00:00`, confirming end-of-interval convention. Verified independently with two different county CSV files.
- **Verdict**: **Confirmed** — ResStock AMY simplified CSV format uses end-of-interval timestamps.

**Citation 2**: "NSRDB TMY3 User's Manual (NREL/TP-581-43156, 2007) §3 — analogous end-of-interval convention."
- **Source found**: NREL SAM forum post at `https://sam.nlr.gov/forum/forum-general/161-question-about-the-time-in-the-data-files.html` (official NREL SAM support, authored by NREL staff). Also confirmed by SAM forum post at `https://sam.nlr.gov/forum/forum-general/1421-weather-data-timestamp.html`.
- **Quoted passage**: "the first row of data in the file is for the hour ending at 1 am on January 1st, local time." (from SAM forum #161). From SAM forum #1421: "The old NREL TMY files identified the first hour as Hour 1 (hour ending at 1 am, or beginning at 12 am)" — confirming that hour 1 timestamp = end of the 00:00–01:00 interval.
- **Verdict**: **Confirmed** — TMY3 uses hour-ending (end-of-interval) convention. The ResStock AMY simplified CSV inherits the same convention (both start data at 01:00:00).

**Citation 3**: "HARES B4 fix at `crates/hares-io/src/tmy3.rs:417`"
- **Source found**: `crates/hares-io/src/tmy3.rs` inspected directly.
- **Quoted passage**: Line 244: `midpoint_offset_secs: 1800,` with comment "TMY3 uses hour-ending convention: timestamp marks the end of each measurement interval (e.g., hour 1 = 00:01–01:00). The midpoint of each hour is 30 minutes before the timestamp, so offset = 1800 s."
- **Verdict**: **Partially correct** — The B4 fix exists and is correct; however, the cited line number (417) is **stale**: the fix is at **line 244**, not line 417.

### Legitimacy
- **Verdict**: **Legitimate**
- **Rationale**: The bug is real and confirmed. The `midpoint_offset_secs: 0` literal at `resstock_csv.rs:337` is incorrect: two independently fetched real NREL ResStock AMY 2018 CSV files (`G0100630_2018.csv` and `G5107750_2018.csv`) both start at `2018-01-01 01:00:00`, establishing end-of-interval convention. This exactly parallels the TMY3 convention corrected by the B4 fix (verified via NREL SAM forum, confirmed in `tmy3.rs:244`). The regression test written as part of this audit (`resstock_midpoint_offset_is_1800_seconds`) fails immediately with `left: 0, right: 1800`. The ticket's claim about OCHRE parallelism requires a caveat: OCHRE uses a different CSV format (NSRDB PSM3) routed through `pvlib.iotools.read_psm3`, so the OCHRE comparison is not directly applicable, but this does not undermine the main finding. The only inaccuracy in the ticket is the stale line number for the B4 fix (417 vs. 244).

### Proposed Fix Summary
In `crates/hares-io/src/resstock_csv.rs:337`, change `midpoint_offset_secs: 0` to `midpoint_offset_secs: 1800` and add an inline comment citing the end-of-interval convention and the parallel TMY3 fix. No other production code changes are required. Do NOT implement this fix here.

### Test Written
- **File**: `crates/hares-io/src/resstock_csv.rs` (inline `#[cfg(test)]` module, function `resstock_midpoint_offset_is_1800_seconds`)
- **What it tests**: Asserts that `WeatherMeta.midpoint_offset_secs == 1800` when a standard 8760-row ResStock CSV is parsed. The test currently **fails** (panics with `left: 0, right: 1800`), demonstrating the bug. It will pass once the fix is applied.
