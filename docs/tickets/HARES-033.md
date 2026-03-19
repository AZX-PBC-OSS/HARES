---
id: HARES-033
title: "hares-io — EPW Weather Parser"
kind: implement
depends_on: [HARES-002, HARES-005]
files_to_touch:
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/weather.rs
  - crates/hares-io/src/lib.rs
references:
  - docs/architecture/06-input-output.md
  - vendors/OCHRE/ochre/utils/schedule.py
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
EPW (EnergyPlus Weather) files are the canonical weather input for building energy simulation. They encode hourly meteorological data for a full year (8760 or 8784 rows for leap years) plus a structured header containing site metadata. HARES needs a robust parser that extracts this data, validates ranges, derives computed fields (sky temperature, ground temperature), and resamples to the simulation timestep via zero-order hold so that the rest of the stack receives a uniform time-indexed weather series regardless of the source file resolution.

## Work to Do
- [ ] Implement `epw.rs`: EPW file parser
  - [ ] Parse header line 1: location name, latitude, longitude, timezone offset, elevation (field 10)
  - [ ] Skip header lines 2–8 (data source, design conditions, typical/extreme periods, ground temperatures, holiday/daylight saving, comments, data periods)
  - [ ] Parse 8760 (standard year) or 8784 (leap year) hourly data records into `EpwRecord` per row; EPW is always hourly — if the file has a non-hourly data period marker, return an error ("EPW downsampling not supported; source must be hourly")
  - [ ] Extract per-record fields: `dry_bulb_c`, `dew_point_c`, `rel_humidity_pct`, `pressure_kpa` (convert Pa → kPa at parse time: divide by 1000), `ghi_w_m2`, `dni_w_m2`, `dhi_w_m2`, `wind_speed_m_s`, `wind_dir_deg`, `opaque_sky_cover`
  - [ ] Validate wind speed range: 0–60 m/s; return error on first violation with row number
  - [ ] GHI range: 0–1500 W/m²; error if any record exceeds 1500
  - [ ] Pressure range: 60–110 kPa; error if out of range
  - [ ] Dew point ≤ dry bulb at every record; error identifying the row
  - [ ] Compute `sky_temp_c` from dry-bulb and dew-point using the Clark–Allen formula. Use this unambiguous form: `T_sky_K = T_db_K * (0.787 + 0.764 * ln(T_dp_K / 273.15)).powf(0.25)` where `T_db_K` and `T_dp_K` are absolute temperatures in Kelvin. Convert result back to Celsius. Cite Clark & Allen (1978) eq. 3 as reproduced in ASHRAE HOF 2017 Ch.14 for this formula.
  - [ ] Extract monthly ground temperatures from header line 3 (GROUND TEMPERATURES section); if multiple depths are present, select the entry at the shallowest depth, matching EnergyPlus convention; fall back to a fixed 10 °C default when the section is absent
  - [ ] Compute per-hour `ground_temp_c` by linear interpolation between monthly midpoints. Monthly midpoint convention: 15th of each month. Year boundary: interpolate between December 15 and January 15 with wrap-around. Reference EnergyPlus Engineering Reference ground temperature interpolation.
- [ ] Implement `weather.rs`: `WeatherTimeSeries` struct
  - [ ] Column-parallel storage: one `Vec<f64>` per field, length = record count; `pressure_kpa` stored as converted kPa value (not raw Pa)
  - [ ] Include `wind_dir_deg` column (needed by `EnvironmentManager` for directional wind pressure calculations)
  - [ ] Indexed access `get(field, timestep_index) -> f64`
  - [ ] ZOH (zero-order hold) resampling: `resample(target_step_secs: u32) -> WeatherTimeSeries` — each source hour is replicated `3600 / target_step_secs` times; if `3600 % target_step_secs != 0`, return an error identifying the incompatible timestep
  - [ ] Expose `location`, `latitude`, `longitude`, `timezone_offset_h`, `elevation_m` as public fields on a `WeatherMeta` struct
- [ ] Re-export `WeatherTimeSeries`, `WeatherMeta`, and `parse_epw` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/epw.rs`: new file — EPW header + record parsing, sky/ground temp computation
- `crates/hares-io/src/weather.rs`: new file — `WeatherTimeSeries`, `WeatherMeta`, ZOH resampling
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] Parsing a real EPW fixture produces exactly 8760 or 8784 records (no off-by-one)
- [ ] All dry-bulb temperatures fall within −60 °C to 55 °C
- [ ] GHI is 0 W/m² for all hours between civil sunset and civil sunrise
- [ ] ZOH resampling from 60-min EPW to 1-min holds each hour's value constant across all 60 sub-steps
- [ ] Elevation extracted from header matches the known value for the fixture file
- [ ] Pressure converted and stored in kPa: `EpwRecord.pressure_kpa` and `WeatherTimeSeries` pressure column are both in kPa
- [ ] Sky temperature formula test: at `T_db = 293.15 K`, `T_dp = 283.15 K`, computed `T_sky_K` is approximately 278.5 K (±1 K tolerance)
- [ ] `wind_dir_deg` is present as a column in the returned `WeatherTimeSeries`
- [ ] Wind speed of 70 m/s in a fixture returns a validation error naming the row
- [ ] An EPW with GHI=1600 at any row returns error
- [ ] An EPW with pressure=55kPa returns error
- [ ] An EPW with dew_point > dry_bulb at row N returns error naming row N
- [ ] `resample(7)` returns error for non-divisor timestep

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
