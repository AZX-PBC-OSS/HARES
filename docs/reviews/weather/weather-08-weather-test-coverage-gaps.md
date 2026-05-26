# Weather test coverage gaps: synthetic weather, edge-case formats
**Review ID**: weather-08
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/epw.rs` (2001 lines)
- `crates/hares-io/src/tmy3.rs` (604 lines)
- `crates/hares-io/src/psm3.rs` (852 lines)
- `crates/hares-io/src/resstock_csv.rs` (881 lines)
- `crates/hares-io/tests/weather_parity.rs` (1151 lines)
- `crates/hares-io/tests/psm3_parity.rs` (474 lines)
- `crates/hares-io/tests/resstock_csv_midpoint_offset.rs` (182 lines)
- `crates/hares-io/tests/weather_reexport.rs` (15 lines)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py` (654 lines) — OCHRE weather loading: strips Feb 29 for leap-year EPW, uses pvlib for EPW/PSM3 parsing, DST-unaware, multi-year handled by index wrap-around
- `vendors/EnergyPlus/weather/` (89 files) — EnergyPlus official weather test suite including leap-year, malformed-header, cold-extreme, sentinel-value, and climate-zone fixtures

## Test Fixture Inventory
The `tests/fixtures/weather/` directory contains only **2 files**:
1. `test_location.epw` — standard Denver TMY3 EPW (1995, non-leap, 8760 rows, 35 fields)
2. `.gitkeep` — placeholder

This is the sole EPW fixture exercising the parser against any real on-disk file. The vendor tree provides 89 EPW files (30 unique EPW filenames) covering leap years, extreme cold, sentinel-valued fields, malformed headers, and multiple climate zones — **none** are referenced in any test.

Additionally:
- `tests/fixtures/weather/synthetic_psm3_5min.csv` — **referenced** by `psm3_parity.rs:17` but **does not exist on disk**. All PSM3 integration tests that use this fixture (e.g., `parses_synthetic_psm3_fixture` at line 40) will fail at runtime.
- 11 parity test `weather.epw` files under `tests/fixtures/parity/` — used only for full-scale integration simulations, not for parser-focused edge-case testing.

## Findings

### Finding 1: Missing synthetic PSM3 5-minute fixture file
**Severity**: critical
**Description**: The integration test `parses_synthetic_psm3_fixture` in `psm3_parity.rs:40` calls `parse_psm3(fixture_path())` which resolves to `tests/fixtures/weather/synthetic_psm3_5min.csv`. This file does not exist in the repository — only `test_location.epw` and `.gitkeep` are present in `tests/fixtures/weather/`. The test cannot execute successfully, meaning PSM3 5-minute parsing has **zero integration-test coverage against on-disk files** despite the format supporting 5/15/30/60-minute resolutions.
**Code Location**: `crates/hares-io/tests/psm3_parity.rs:15-17`, `crates/hares-io/tests/psm3_parity.rs:40`
**Root Cause**: The fixture file was never committed to the repository, or a pre-test generation step was omitted from the build system.
**Impact**: The PSM3 5-minute resolution path — the highest-resolution format HARES supports — has no end-to-end integration test against a real on-disk fixture. Unit tests in `psm3.rs` use synthetic in-memory CSV strings but cannot verify file-I/O paths. Compare with `epw.rs` which has `parse_epw_reads_from_path` testing the file-I/O path.

### Finding 2: Vendor leap-year EPW fixture never exercised
**Severity**: high
**Description**: `vendors/EnergyPlus/weather/MadeUpLeapYear.epw` is a 8784-row (leap-year 2016) EPW file with full design conditions, 3-depth ground temperatures, DST rules, and `?9?9?9?9` sentinels in the extraterrestrial radiation field. The EPW unit test `supports_amy_leap_record_count` at `epw.rs:954` only tests that a **synthetic** in-memory leap-year EPW (with 8784 copied rows from a standard file) parses successfully. The real vendor fixture — which exercises actual Feb 29 data rows with `?9?9?9?9` sentinel values in adjacent fields — is never parsed. OCHRE explicitly checks for leap day in loaded data and emits a warning (`schedule.py:207-208`); HARES has no comparable guard.
**Code Location**: `crates/hares-io/src/epw.rs:954-965` (synthetic test), `vendors/EnergyPlus/weather/MadeUpLeapYear.epw` (unused vendor fixture)
**Root Cause**: No test harness support for walking the vendor EnergyPlus weather directory.
**Impact**: The leap-year path (8784 rows, Feb 29 data, month-day accounting in `monthly_day_counts` and `doe2_ground_temp_monthly`) is only tested against copy-paste synthetic data. Real-world leap-year EPW files with sentinel values in the extraterrestrial radiation column and non-trivial DST headers are never exercised. The `weather_file_issue_9161.epw` vendor file also uses `?9?9?9?9` sentinels on all 8760 rows — parsing through that file could reveal robustness issues with sentinel-value propagation.

### Finding 3: No test for EPW files with minimal column count (24 fields)
**Severity**: high
**Description**: The EPW parser requires at least 24 fields per data row (`EPW_RECORD_MIN_FIELDS = 24` at `epw.rs:28`), which provides access through field 23 (opaque sky cover). Fields 24–33 (including liquid precipitation depth at index 33) are optional. While the parser gracefully handles the *absence* of field 33 (lines 236–238: zero-fills liquid precip with a debug flag), there is **no test** for an EPW file that provides exactly 24 fields — the minimum viable EPW. The synthetic EPW builders (`build_synthetic_epw`, `build_hourly_from_monthly`) all generate rows with exactly 35 fields. If a real-world EPW has fewer than 24 fields, the rejection path at `epw.rs:136-140` is also untested with a real fixture.
**Code Location**: `crates/hares-io/src/epw.rs:136-140` (rejection), `crates/hares-io/src/epw.rs:236-238` (optional-field fallback), `crates/hares-io/src/epw.rs:28` (minimum field constant)
**Root Cause**: Test fixtures always use the full 35-field EPW format.
**Impact**: Users with older or simplified EPW files (e.g., 24-field minimum format from older EnergyPlus versions or third-party tools) may encounter unverified behavior. OCHRE delegates to pvlib's `read_epw` which handles variable column counts transparently (`schedule.py:182`).

### Finding 4: No test for extreme but valid temperature operation
**Severity**: medium
**Description**: The EPW dry-bulb validation range is [-60, +55] °C (`epw.rs:159`). The existing rejection tests (`rejects_wind_speed_above_limit`, `rejects_ghi_above_limit`, `rejects_pressure_out_of_range`, `rejects_infrared_above_700`, `rejects_negative_infrared`) verify that values **outside** the range are rejected. However, there is **no test** verifying correct physical behavior for values **at the extremes** of the valid range — e.g., -50 °C dry bulb with very low dew point, or +55 °C with high humidity. Sky temperature computation (`compute_sky_temp_c`, `epw.rs:542`), Clark-Allen fallback (`epw.rs:565`), and ground temperature DOE-2 model (`epw.rs:391`) may exhibit numerical instability or unrealistic outputs at extreme temperatures. The vendor `Drycold_blast.epw` has temperatures down to -24.4 °C (not extreme enough) and is never parsed in tests.
**Code Location**: `crates/hares-io/src/epw.rs:159` (validation bound), `crates/hares-io/src/epw.rs:209-214` (sky temp computation), tests at `crates/hares-io/src/epw.rs:1112-1224`
**Root Cause**: Validation tests focus on rejection-at-boundary, not correct-operation-at-boundary.
**Impact**: Sky temperature (Stefan-Boltzmann + Clark-Allen) and DOE-2 ground temperature models have never been validated at near-boundary dry-bulb values. At -50 °C with clear sky (low IR), sky temperatures could become physically unrealistically low. At +55 °C with high humidity, the Clark-Allen formula may approach the dew-point bound in untested ways.

### Finding 5: DST/holidays header line is silently ignored — no test for pass-through
**Severity**: medium
**Description**: The EPW HOLIDAYS/DAYLIGHT SAVINGS header line (line 5 of 8) is read but immediately discarded (`let _ = holidays_daylight_line;` at `epw.rs:120`). The vendor `MadeUpLeapYear.epw` has DST enabled (`Yes,2nd Sunday in March,1st Sunday in November`), and `Drycold_blast.epw` has 9 enumerated holidays. **No test verifies that EPW files with DST-enabled headers parse without error.** While HARES does not currently use DST information, regressions in header-line consumption (e.g., miscounting indices and reading header line 5 as line 6) could silently corrupt the data-period header (line 8).
**Code Location**: `crates/hares-io/src/epw.rs:107`, `crates/hares-io/src/epw.rs:120`
**Root Cause**: The discarded line is an untested code path. All synthetic builders use a minimal placeholder string; no test exercises a real DST-bearing header.
**Impact**: If the EPW header order is misinterpreted or a future parser change miscounts header lines, DST-bearing files would be the first to break without detection.

### Finding 6: Multi-year EPW files hard-rejected without error-message test
**Severity**: medium
**Description**: The EPW parser enforces exactly 8760 or 8784 data rows (`epw.rs:271-276`). Files with any other record count — including multi-year EPW files (e.g., 17,520 rows for 2 years) — are rejected with a validation error. EnergyPlus supports multi-year EPW files, and the vendor tree contains statistics for multi-year runs (`94810-1956-1957.stat`). HARES handles multi-year **simulation** via modular wrap-around indexing in `weather.rs:303-311`, but the EPW file parser cannot load 2+ year files. **No test verifies the rejection error message** for non-standard row counts (e.g., 17,520, 4,380 for half-year, 0 for empty).
**Code Location**: `crates/hares-io/src/epw.rs:271-276`
**Root Cause**: The record-count rejection has no unit test; only positive-path tests exist for 8760 and 8784 row counts.
**Impact**: Users providing multi-year EPW files get a generic validation error with no guidance on workarounds. Compare with OCHRE which uses `set_annual_index` (`schedule.py:195`) and index-wrapping for multi-year behavior.

### Finding 7: No test for EPW `?9?9?9?9` sentinel-value robustness
**Severity**: low
**Description**: EnergyPlus EPW files commonly use `?9?9?9?9E0...` sentinel strings in the Extraterrestrial Horizontal Radiation field (field index 5). The `MadeUpLeapYear.epw` and `weather_file_issue_9161.epw` vendor files both contain these sentinels on all data rows. HARES does not parse field 5 (it skips directly to field 6 for dry-bulb), so these particular sentinels are harmless. However, **if a sentinel value appeared in a parsed field** (e.g., GHI at index 13, DNI at index 14, IR at index 12), the `parse_f64` call would fail and terminate parsing. No test verifies that HARES can survive a file where optional-but-parsed fields contain sentinel values, or that the error message is actionable.
**Code Location**: `crates/hares-io/src/epw.rs:181` (GHI parse), `crates/hares-io/src/epw.rs:188-189` (DNI/DHI parse), `crates/hares-io/src/epw.rs:201-202` (IR parse)
**Root Cause**: Sentinels are not handled per the EPW Data Dictionary ("missing" conventions per field).
**Impact**: Low — sentinels in the main radiation fields are unusual in production EPW files. However, the `weather_file_issue_9161.epw` vendor file is specifically a regression test and suggests EnergyPlus has encountered this in practice.

### Finding 8: No test for EPW `Bland Excitation 1.epw` (space variant) malformed header
**Severity**: low
**Description**: `vendors/EnergyPlus/weather/Bland Excitation 1.epw` (with a space in the filename) omits the required `LOCATION,` prefix on line 1, uses placeholder text (`header line 2 (design conditions)`) for header lines 2–7, and omits the `DATA PERIODS,` prefix on line 8. This file would be **rejected** by HARES — which is correct behavior — but the exact rejection path (line 1 LOCATION check at `epw.rs:309` vs. line 8 DATA PERIODS check at `epw.rs:341`) is never tested. The underscore variant `Bland_Excitation_1.epw` has valid headers but non-standard pressure units (hPa vs Pa) that might pass leniently.
**Code Location**: `crates/hares-io/src/epw.rs:309`, `crates/hares-io/src/epw.rs:341`
**Root Cause**: No negative-path test for header format violations against real-world vendor fixtures.
**Impact**: Low — the file is correctly rejected. A test would solidify the contract and prevent regressions in header parsing.

### Finding 9: No test for sub-hourly EPW files (rejection path)
**Severity**: low
**Description**: The `ensure_hourly_data_period()` function at `epw.rs:348-352` rejects EPW files where the DATA PERIODS header declares `records_per_hour != 1`. No test exercises this rejection path. EnergyPlus supports sub-hourly EPW files; HARES intentionally does not, but the rejection message ("EPW downsampling not supported; source must be hourly") is never verified in tests.
**Code Location**: `crates/hares-io/src/epw.rs:348-352`, test `rejects_non_hourly_data_period` at `epw.rs:1101`
**Root Cause**: The test `rejects_non_hourly_data_period` at line 1101 only tests with a completely malformed header, not with a correctly-structured `DATA PERIODS,4,1,...` (4 records per hour) header.
**Impact**: Low — the error path is reachable but unverified. A malformed test that doesn't exercise the actual `records_per_hour != 1` check is a gap in negative-path coverage.

### Finding 10: ResStock DST transition timestamps not tested
**Severity**: low
**Description**: ResStock CSV files may contain timestamps spanning DST transitions (e.g., "spring forward" where 02:00–03:00 is skipped, or "fall back" where 01:00 appears twice). The ResStock CSV parser has special midnight-handling logic (`resstock_csv.rs:308-322` for `00:00` → hour 24 of previous day) but no test for DST-ambiguous or DST-gap timestamps. The TMY3 integration test at `resstock_csv_midpoint_offset.rs` tests only midday (11:30) solar zenith on Jan 1 — far from any DST boundary.
**Code Location**: `crates/hares-io/src/resstock_csv.rs:308-322` (midnight handling), `crates/hares-io/tests/resstock_csv_midpoint_offset.rs:80-120`
**Root Cause**: All ResStock tests use synthetic timestamps in non-DST periods.
**Impact**: Low — if timestamp parsing relies on chrono `NaiveDate` handling, DST-transition timestamps may produce subtle off-by-one-hour errors in datetime construction. Currently unverified.

## Summary

- Total findings: 10
- Critical: 1 (missing PSM3 fixture file)
- High: 2 (vendor leap-year EPW never exercised, no test for minimal-column-count EPW)
- Medium: 3 (extreme-temperature operation, DST header pass-through, multi-year rejection message)
- Low: 4 (sentinel robustness, malformed header rejection, sub-hourly rejection, ResStock DST timestamps)

**Test fixture count: 2** (1 EPW + 1 .gitkeep) vs. **vendor fixture count: 89** (30 unique EPW files, 0 linked to tests).

## Recommendations

1. **Commit or generate `tests/fixtures/weather/synthetic_psm3_5min.csv`** and ensure `cargo test` passes the PSM3 integration tests. Alternatively, remove the dead test code referencing a non-existent fixture.

2. **Add a vendor-fixture integration test** that walks `vendors/EnergyPlus/weather/*.epw` and verifies correct parse-or-reject behavior for each file, starting with the highest-signal files:
   - `MadeUpLeapYear.epw` — leap-year + sentinel values + DST header
   - `Drycold_blast.epw` — cold extremes + holiday list + "GROUND TEMPERATURES,0" fallback
   - `weather_file_issue_9161.epw` — sentinel-value regression test
   - `Bland_Excitation_1.epw` — valid format (365-day TMY2 year, different minute convention)
   - `Bland Excitation 1.epw` — verify it's rejected with the expected error message

3. **Add a boundary-value test** that synthesizes an EPW with extreme-but-valid temperatures (e.g., -50 °C and +55 °C dry bulb) and verifies that sky temperature and ground temperature computations produce physically plausible outputs (no NaN, no absurd values).

4. **Add a record-count rejection test** for non-standard row counts (17,520 for 2-year multi-year, 4,380 for half-year, 0 for empty) to ensure the error message is clear and detectable.

5. **Add an EPW tests** for:
   - Exactly 24 fields per row (minimum valid EPW) — verifies optional-field gracefulness
   - DST-bearing header — verifies line 5 discard doesn't corrupt subsequent header parsing
   - Sub-hourly `DATA PERIODS,4,1,...` header — verifies the `records_per_hour != 1` rejection

6. **Shore up the `rejects_non_hourly_data_period` test** (`epw.rs:1101`) to use a properly-structured sub-hourly header rather than a garbled one, so it actually exercises the `records_per_hour != 1` check at line 348.

## References / Citations

- EnergyPlus EPW Data Dictionary v9.6 — specifies 35-field EPW format, `\missing 999` sentinel for field 33 (liquid precipitation), `\missing 9999` for field 12 (IR)
- EnergyPlus `weather_file_issue_9161.epw` — regression test for EnergyPlus issue #9161, 8760 rows with `?9?9?9?9` sentinels in extraterrestrial radiation field
- OCHRE `schedule.py:207-208` — leap day detection and warning
- OCHRE `schedule.py:182` — delegates EPW parsing to pvlib's `read_epw`, which handles variable column counts
- OCHRE `schedule.py:195` — multi-year simulation via `set_annual_index` wrapping
- HARES `epw.rs:542-565` — `compute_sky_temp_c` routing: Stefan-Boltzmann above IR >= 50 W/m², Clark-Allen fallback below
- HARES `epw.rs:391-439` — DOE-2 ground temperature model with sinusoidal approximation at 0.5 m reference depth
- HARES `weather.rs:303-311` — modular wrap-around indexing for multi-year simulation
