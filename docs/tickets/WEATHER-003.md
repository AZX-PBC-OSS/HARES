---
id: WEATHER-003
title: PSM3/NSRDB parser module for sub-hourly native solar data
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/src/psm3.rs
  - crates/hares-io/src/weather.rs
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/lib.rs
references:
  - vendors/OCHRE/ochre/utils/schedule.py (lines 187-203, PSM3 loading)
  - https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/
  - https://pvlib-python.readthedocs.io/en/stable/reference/generated/pvlib.iotools.parse_psm3.html
  - https://nsrdb.nrel.gov/data-sets/us-data
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

PSM3 (Physical Solar Model v3) files from NREL's NSRDB provide solar irradiance at
**5-minute, 15-minute, 30-minute, or 60-minute** native resolution. For sub-hourly PV
simulation, 5-minute native solar data is dramatically more accurate than interpolating
hourly EPW data, because EPW solar values are already period-averaged and any sub-hourly
treatment is inherently approximate.

OCHRE supports PSM3 via `pvlib.iotools.read_psm3()`. HARES should support PSM3
natively in Rust for consistency and performance.

Note: NREL now recommends GOES Aggregated v4.0.0 as the successor to PSM3, but PSM3
remains the dominant format in existing datasets and toolchains.

### PSM3 CSV format (SAM CSV)

PSM3 files are CSVs with a 2-line header:
- **Line 1**: Comma-separated metadata values (Source, Location ID, City, State,
  Country, Latitude, Longitude, Time Zone, Elevation, Local Time Zone, ...)
- **Line 2**: Column names (Year, Month, Day, Hour, Minute, GHI, DNI, DHI,
  Temperature, Pressure, Dew Point, Relative Humidity, Wind Speed, Wind Direction,
  Surface Albedo, Snow Depth, ...)
- **Lines 3+**: Data rows

When `pvlib.iotools.read_psm3(map_variables=True)` is used (as OCHRE does), columns
are renamed to pvlib-standard names: `temp_air`, `relative_humidity`, `ghi`, `dni`,
`dhi`, `wind_speed`, `wind_direction`, `pressure`, etc.

Key differences from EPW:
- Pressure in **millibar (mbar)** → convert to kPa by dividing by 10
  (confirmed: OCHRE `schedule.py:198` does `df["pressure"] /= 10`)
- RH in percentage (same as EPW)
- Temperature in °C (same)
- No horizontal infrared column → sky temperature requires Clark-Allen fallback
- Native sub-hourly resolution (no resampling needed at matching timestep)
- Timestamps are explicit (Year/Month/Day/Hour/Minute columns)
- Supported intervals: 5, 15, 30, or 60 minutes (60 required for TMY requests)

## Work to Do

- [ ] **Extract shared helpers from `epw.rs`** (REQUIRED — not optional):
  - `clark_allen_sky_temp_c` → change to `pub(crate)` visibility
  - `doe2_ground_temp_monthly` → change to `pub(crate)` visibility
  - `interpolate_ground_temp_c` → change to `pub(crate)` visibility
  - `monthly_average_dry_bulb` → change to `pub(crate)` visibility
  - These functions must remain in `epw.rs` (no need to move) but must be
    accessible from `psm3.rs` within the same crate

- [ ] Add `source_step_secs: u32` field to `WeatherMeta` in `weather.rs`:
  - Default value for EPW: `3600`
  - Update `parse_epw()` in `epw.rs` to set `source_step_secs: 3600`
  - **Breaking change**: ALL existing `WeatherMeta` struct literals must add
    `source_step_secs: 3600`. Full list of sites that must be updated:
    - `crates/hares-io/src/epw.rs:264` — `parse_location_header` return
    - `crates/hares-io/src/weather.rs:176` — `sample_series()` test helper
    - `crates/hares-io/src/schedule.rs:708` — `weather_meta()` test helper
    - `crates/hares-core/src/environment.rs:524` — `weather_series()` test helper
    - `crates/hares-core/src/dwelling/synthetic.rs:268` — synthetic weather builder
    - `crates/hares-io/tests/hpxml_parsing_tests.rs:53` — `empty_weather_meta()`
    - `crates/hares-io/tests/schedule_parity.rs` — if it has `WeatherMeta` literals
    - `crates/hares-io/tests/schedule_integration.rs` — if it has `WeatherMeta` literals

- [ ] Create `crates/hares-io/src/psm3.rs` with:
  - `/// Parse a PSM3/NSRDB CSV file (SAM CSV format) into a WeatherTimeSeries.`
  - `///`
  - `/// PSM3 files are produced by NREL's National Solar Radiation Database (NSRDB)`
  - `/// Physical Solar Model v3. Format: 2-line header (metadata + column names),`
  - `/// then data rows at 5/15/30/60-minute intervals.`
  - `///`
  - `/// Reference: https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/`
  - `pub fn parse_psm3(path: impl AsRef<Path>) -> Result<WeatherTimeSeries, WeatherError>`
  - Parse line 1: split by comma, extract Latitude, Longitude, Time Zone, Elevation
    by position (indices match pvlib's implementation)
  - Parse line 2: column name header
  - Parse data rows: map columns to `WeatherTimeSeries` fields by name
  - Unit conversions: pressure mbar→kPa (÷10)
  - Sky temperature: use `clark_allen_sky_temp_c` from epw.rs (no IR data in PSM3)
  - Ground temperature: use `doe2_ground_temp_monthly` + `interpolate_ground_temp_c`
    from epw.rs with monthly dry-bulb averages
  - Auto-detect native timestep: compute interval between first two timestamps,
    validate it's one of {300, 900, 1800, 3600} seconds
  - Set `meta.source_step_secs` to detected interval

- [ ] Rewrite `WeatherTimeSeries::resample()` to handle non-hourly source data:
  - **IMPORTANT**: The existing guard `3600 % target_step_secs != 0` assumes hourly
    source data and must be **removed/replaced**. The new logic must use
    `meta.source_step_secs` to determine the source resolution and compute the
    correct factor/direction.
  - New validation: `source_step_secs % target_step_secs == 0` (upsampling) OR
    `target_step_secs % source_step_secs == 0` (downsampling). Error on neither.
  - If source is already at target resolution → return clone (no-op)
  - If source is finer than target → downsample (target_step > source_step):
    - `ratio = target_step_secs / source_step_secs` (how many source slots per output)
    - Mean aggregation for instantaneous fields (temp, pressure, humidity, wind)
    - Mean for solar (GHI/DNI/DHI): PSM3 solar values are average irradiance
      (W/m²) over each interval, same convention as EPW. Mean of N sub-interval
      averages = average over the coarser interval. This preserves total energy
      because energy = mean_irradiance × total_duration (unchanged).
    - Sum for accumulated fields (precipitation depth)
  - If source is coarser than target → upsample (source_step > target_step):
    - `factor = source_step_secs / target_step_secs`
    - Use existing PCHIP/ZOH/distribute logic with this factor
  - The `source_step_secs` field determines which case applies

- [ ] Add `pub mod psm3;` and `pub use psm3::parse_psm3;` to `lib.rs`

- [ ] Add validation:
  - Record count consistent with detected timestep and year length
    (e.g., 5-min data: 105120 or 105408 for leap year)
  - Range checks matching EPW: temperature [-60, 55]°C, pressure [60, 110] kPa,
    GHI [0, 1500] W/m², wind [0, 60] m/s
  - Dew point ≤ dry bulb (same physical constraint as EPW)

## Files to Touch

- `crates/hares-io/src/epw.rs`: Change shared helpers to `pub(crate)`, set
  `source_step_secs: 3600` in `parse_epw`
- `crates/hares-io/src/weather.rs`: Add `source_step_secs` to `WeatherMeta`, update
  `resample()` for non-hourly sources, update test `WeatherMeta` struct literals
- `crates/hares-io/src/psm3.rs`: New file — PSM3 parser
- `crates/hares-io/src/lib.rs`: Add module + re-export

## Measures of Success

- [ ] `parse_psm3()` successfully loads a PSM3 CSV and produces a valid
      `WeatherTimeSeries` with correct units
- [ ] 5-minute PSM3 data can be used directly without resampling at 300s timestep
- [ ] 5-minute PSM3 data can be downsampled to hourly and produces values close to
      corresponding EPW data for the same location
- [ ] Shared helpers (Clark-Allen, DOE-2 ground temp) are reused without duplication
- [ ] All existing EPW tests still pass (with updated `WeatherMeta` struct literals)
- [ ] Doc comments on `parse_psm3` cite format source (NREL NSRDB PSM3)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
