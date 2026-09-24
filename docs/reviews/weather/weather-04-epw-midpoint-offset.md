# EPW hour-ending midpoint offset convention verification
**Review ID**: weather-04
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/epw.rs crates/hares-io/src/weather.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/schedule.py vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc

## Findings

### Finding 1: [Severity: medium]
**Description**: HOLIDAYS/DAYLIGHT SAVINGS EPW header line is fully discarded, losing the `WFAllowsLeapYears` flag and DST boundary information.

**Code Location**: `crates/hares-io/src/epw.rs:120`

```rust
// Line 105: line is read from the file
let holidays_daylight_line = lines
    .next()
    .ok_or_else(|| ...)?;
// Line 120: discarded immediately
let _ = holidays_daylight_line;
```

**Root Cause**: The HOLIDAYS/DAYLIGHT SAVINGS header carries two pieces of metadata that affect data validity:
1. **A1 (Leap Year Observed)**: "Yes" means Feb 29 data should be honoured; "No" means Feb 29 data should be ignored even when present. This is the `WFAllowsLeapYears` flag in EnergyPlus (WeatherManager.cc:7889).
2. **A2/A3 (DST Start/End Day)**: Defines daylight saving boundaries for the site. EnergyPlus parses these at WeatherManager.cc:7890-7907 into `EPWDaylightSaving`, `EPWDST.StartDate`, and `EPWDST.EndDate`.

HARES relies solely on record count (8760 vs 8784 rows) to detect leap years (epw.rs:271-276). This auto-detection is correct for files that include Feb 29 data, but fails for the edge case where an EPW file explicitly declares "No" for leap year observation while still containing 8784 rows. In such a file, HARES would incorrectly honour Feb 29 data that EnergyPlus would discard.

**Impact**: HARES would process Feb 29 weather rows from an EPW that explicitly forbids leap year observation. This could produce subtly incorrect annual energy totals (1 extra day of loads) compared to EnergyPlus results. The skip logic is present in EnergyPlus at WeatherManager.cc:2816-2827:

```cpp
if (WMonth == 2 && WDay == 29 && (!CurrentYearIsLeapYear || !WFAllowsLeapYears)) {
    EndDayOfMonth(2) = 28;
    SkipThisDay = true;
    ShowWarningError(state, "...Feb29 data encountered but will not be processed.");
}
```

**DST note**: The DST fields are parsed by EnergyPlus but never shift weather data -- EPW is always reported in local **standard** time. OCHRE ignores DST entirely (schedule.py:167-176). HARES correctly handles DST at a higher level in `compute_annual_offset` (environment.rs:690-724) by extracting wall-clock components from the already-timezone-adjusted simulation clock. The loss of DST boundary metadata is therefore low-impact for weather data but could affect schedule timing in DST-observing regions.

### Finding 2: [Severity: medium]
**Description**: The `midpoint_offset_secs` offset is correctly defined but the resampling self-consistency between PCHIP and Triangular differs from EnergyPlus's distinct solar vs. non-solar interpolation weighting.

**Code Location**:
- `crates/hares-io/src/epw.rs:330` -- `midpoint_offset_secs: 1800` (correct for EPW convention)
- `crates/hares-io/src/weather.rs:589-592` -- PCHIP defaults for continuous fields (dry-bulb, dew-point, IR, ground)
- `crates/hares-io/src/weather.rs:659-673` -- ZOH defaults for solar (GHI, DNI, DHI)
- `crates/hares-io/src/environment.rs:721` -- `(seconds_into_year + year_secs - meta.midpoint_offset_secs as u64) % year_secs`

**Root Cause**: EnergyPlus uses two distinct interpolation schemes for EPW hourly data:

1. **Continuous fields** (temperature, pressure, humidity, wind): Linear interpolation from previous hour-end to current hour-end (`Interpolation[ts] = ts/TS_per_Hour`; WeatherManager.cc:8332-8333). Hour-ending values are placed at the end of the hour array position.

2. **Solar fields** (GHI, DNI, DHI): Midpoint-centred linear interpolation (`SolarInterpolation[halfpoint] = 1.0`; WeatherManager.cc:8336-8367). Solar values are placed at the MIDPOINT of each hour, and a triangular weighting between previous hour midpoint and current hour midpoint is applied. This is explicit at WeatherManager.cc:3081-3082: `// It's at the half hour`.

HARES takes a unified approach instead:
- All fields are indexed via the same `midpoint_offset_secs` shift that subtracts 1800 s from the lookup time (environment.rs:721). This maps simulation time to the correct hourly row considering the hour-ending convention.
- **Continuous fields**: Default to `PchipCyclic` (C1 cubic spline through hour-ending knot values). EnergyPlus uses linear (C0 continuous) interpolation, which is simpler but less smooth.
- **Solar fields**: Default to `Zoh` (zero-order hold) to preserve hourly energy integrals exactly. EnergyPlus uses midpoint-centred linear interpolation.

The PCHIP default for continuous fields ramps from the previous row's value through the current row's value -- both placed at the period-midpoint-adjacent array indices. Since PCHIP knots are at array index positions and `midpoint_offset_secs` shifts the time-to-index mapping, the PCHIP curve passes through the hour-ending values at the correct time positions.

**Impact**: HARES resampling is mathematically correct but not bit-identical to EnergyPlus. The PCHIP (C1) interpolation produces smoother transitions than EnergyPlus's linear (C0) interpolation, which is acceptable and likely more physically realistic. The ZOH default for solar is conservative (preserves energy) while EnergyPlus's triangular interpolation at midpoints does not. The `ResampleOverrides` mechanism allows users to match EnergyPlus conventions when parity testing is needed.

### Finding 3: [Severity: low]
**Description**: Ground temperature interpolation applies a hardcoded hour-0.5 shift at the interpolation layer, creating a redundant copy of the `midpoint_offset_secs` correction.

**Code Location**: `crates/hares-io/src/epw.rs:693-697`

```rust
// EPW uses hour-ending convention: hour 12 covers 11:00–12:00.
// Place the PCHIP knot at the midpoint of the period (11:30 for hour 12)
// to match OCHRE/pvlib's +30min offset convention.
let hour_fraction = (f64::from(hour) - 0.5) / 24.0;
let x = timestamp_day + hour_fraction;
```

**Root Cause**: The ground temperature interpolation at `interpolate_ground_temp_c` (epw.rs:684-739) independently applies a `(hour - 0.5)` shift to place the timestamp at the hour's midpoint. This is functionally equivalent to the `midpoint_offset_secs` metadata applied in `compute_annual_offset`, but it hardcodes the assumption rather than reading from the metadata. If the source format changes (e.g., an EPW variant using a different convention or PSM3 data with offset=0 being fed through the EPW ground temp path), this hardcoded 0.5h shift would be inconsistent.

EnergyPlus computes ground temperatures similarly but linear-interpolates between December/January to handle the year boundary (WeatherManager.cc:253-254 in the DOE-2 formula). HARES's wrap-around at `anchors[11]` (epw.rs:704-712) achieves the same effect.

**Impact**: Redundant logic creates maintenance risk. The hardcoded 0.5h and the `midpoint_offset_secs` metadata could diverge. Currently they are consistent (both 1800s / 0.5h for EPW) but this should ideally reference `WeatherMeta.midpoint_offset_secs` rather than be hardcoded.

### Finding 4: [Severity: low]
**Description**: HARES correctly auto-detects leap years by record count (8760 vs 8784), matching EnergyPlus but diverging from OCHRE.

**Code Location**: `crates/hares-io/src/epw.rs:24-25, 271-278`

```rust
const EXPECTED_RECORDS_STANDARD: usize = 8760;
const EXPECTED_RECORDS_LEAP: usize = 8784;

if records.len() != EXPECTED_RECORDS_STANDARD && records.len() != EXPECTED_RECORDS_LEAP {
    return Err(WeatherError::Validation(...));
}
let is_leap_year = records.len() == EXPECTED_RECORDS_LEAP;
```

**Root Cause**: 

| Implementation | Leap year handling |
|---|---|
| EnergyPlus | Skips Feb 29 when `WFAllowsLeapYears = false`; fatal error when leap year simulation has no Feb 29 data (WeatherManager.cc:2816-2836) |
| OCHRE | Explicitly strips Feb 29 data from all 8784-row EPW files: `df = df.loc[~((df.index.month == 2) & (df.index.day == 29)), :]` (schedule.py:172-173) |
| HARES | Preserves Feb 29 data. `monthly_day_counts()` returns 29 for February when `is_leap_year = true` (epw.rs:447-453) |

HARES's approach is the most complete: it preserves leap year data for faithful annual energy simulation. OCHRE strips Feb 29, which discards real weather information. EnergyPlus requires the user to set the `WFAllowsLeapYears` flag correctly.

**Impact**: HARES users get 8784 data points from leap-year EPW files, producing 366-day simulations with slightly higher annual energy totals than OCHRE (which uses only 365 days). This is the physically correct behaviour for a leap year. The divergence from OCHRE is documented in HARES's EPW module header comment (epw.rs:3-9).

### Finding 5: [Severity: low]
**Description**: Validation of EPW `hour` field (1..=24) is correct per EPW specification; EnergyPlus uses identical range.

**Code Location**: `crates/hares-io/src/epw.rs:147-151`

```rust
if !(1..=24).contains(&hour) {
    return Err(WeatherError::Validation(format!(
        "row {row}: hour must be in 1..=24, got {hour}"
    )));
}
```

**Root Cause**: The EPW Data Dictionary specifies hour values 1-24 where hour 1 = 00:00-01:00 and hour 24 = 23:00-24:00. EnergyPlus's `ReadEPlusWeatherForDay` explicitly handles `hour == 1` and `hour == 24` for day-boundary transitions:

```cpp
// WeatherManager.cc:2599: for (int CurTimeStep = 1; CurTimeStep <= NumIntervalsPerHour; ++CurTimeStep)
// The hour loop goes 1..=24
```

**Impact**: None. Validation is correct and matches EnergyPlus.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 2
- Low: 3

## Recommendations

1. **Parse HOLIDAYS/DAYLIGHT SAVINGS header** to extract the Leap Year Observed field (A1). Use it to gate Feb 29 data acceptance (match EnergyPlus behavior). If "No", skip Feb 29 even from 8784-row files, log a warning, and reduce `EXPECTED_RECORDS_LEAP` acceptance accordingly. (Addresses Finding 1)

2. **Consider documenting the interpolation difference** between HARES (PCHIP for continuous, ZOH for solar) and EnergyPlus (linear for continuous, midpoint-triangular for solar) in the `ResampleMethod` documentation. Note that `ResampleOverrides::ochre_compat()` provides parity with OCHRE's ZOH-all approach, and additional overrides could be added for EnergyPlus parity if required. (Addresses Finding 2)

3. **Replace the hardcoded `hour - 0.5`** in `interpolate_ground_temp_c` (epw.rs:696) with `hour - (meta.midpoint_offset_secs as f64 / 3600.0)` to avoid redundancy and ensure consistency across weather formats. (Addresses Finding 3)

## References / Citations

- EnergyPlus WeatherManager.cc:3081-3082 -- Solar interpolation placed at "the half hour"
- EnergyPlus WeatherManager.cc:3951 -- `// hour values = integrated over hour ending at time of hour`
- EnergyPlus WeatherManager.cc:8332-8368 -- `SetupInterpolationValues` defining Interpolation and SolarInterpolation weight arrays
- EnergyPlus WeatherManager.cc:7889 -- `WFAllowsLeapYears` flag parsed from HOLIDAYS/DAYLIGHT SAVINGS header
- EnergyPlus WeatherManager.cc:2816-2827 -- Leap year day skipping logic
- OCHRE schedule.py:168 -- `offset = dt.timedelta(minutes=30)` for EPW files
- OCHRE schedule.py:172-173 -- Leap year Feb 29 stripping
- OCHRE schedule.py:107-113 -- `set_annual_index` applying the +30min offset
- EPW Data Dictionary v9.6 §2.1 -- Hour-ending convention: each record represents data for the hour ending at that time
- Wilcox & Marion (2008), NREL/TP-581-43156 -- TMY3 specification: "Each hourly value represents the value for the hour ending at the indicated time"
