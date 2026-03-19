---
id: HARES-034
title: "hares-io — Schedule CSV Parser"
kind: implement
depends_on: [HARES-002]
files_to_touch:
  - crates/hares-io/src/schedule.rs
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
ResStock schedule CSVs provide sub-hourly (typically 15-minute) occupancy and load profiles for each end-use. OCHRE maps column names following the `"{equipment} ({unit})"` pattern and resamples to the simulation timestep. HARES must replicate this pipeline so that downstream equipment models receive correctly indexed, correctly named schedule data regardless of the source CSV resolution or the chosen simulation timestep.

## Work to Do
- [ ] Implement `schedule.rs`: CSV schedule parser and `ScheduleTimeSeries` struct
  - [ ] Detect the time column (first column or column named `"timestamp"` / `"Time"`) and parse as `DateTime<FixedOffset>`, preserving timezone information
  - [ ] Timezone fallback: when the CSV has no timezone annotation in its timestamp column, infer the timezone from the `WeatherMeta.timezone_offset_h` field. Accept an `Option<&WeatherMeta>` as an optional parameter to `parse_schedule_csv`; if neither source provides a timezone, return an error
  - [ ] Map remaining column names to OCHRE internal format: `"{equipment} ({unit})"` pattern; normalise whitespace and collapse multiple internal spaces to one
  - [ ] Validate that all required columns (passed as a `&[&str]` argument) are present; return a typed error listing any missing names
  - [ ] Coverage check: confirm the schedule's temporal range fully covers the requested simulation period; return an error with the gap details if not
  - [ ] Store data column-major as `Vec<Vec<f64>>` (outer index = column, inner index = timestep)
  - [ ] Build a `HashMap<String, usize>` name-to-column-index for O(1) lookup
  - [ ] ZOH resampling: `resample(target_step_secs: u32) -> ScheduleTimeSeries` — replicate each source row `source_step_secs / target_step_secs` times; error if target is not an integer divisor of the source step
  - [ ] Downsampling guard: if `target_step_secs > source_step_secs`, return an error: `"Schedule CSV downsampling not supported; source timestep ({src}s) must be ≤ simulation timestep ({tgt}s)"` where `{src}` and `{tgt}` are the actual values
  - [ ] `get_value(column_name: &str, timestep_index: usize) -> Result<f64>`: return value or a typed `ColumnNotFound` / `IndexOutOfRange` error
- [ ] Re-export `ScheduleTimeSeries` and `parse_schedule_csv` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/schedule.rs`: new file — CSV parsing, column mapping, ZOH resampling, `ScheduleTimeSeries`
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] Parsing a ResStock 15-min fixture produces the correct number of rows and the expected column names in OCHRE format
- [ ] OCHRE column name normalisation: a column header `"Clothes Washer  (kW)"` (double space) normalises to `"Clothes Washer (kW)"`
- [ ] `resample(60)` on a 15-min schedule produces a series 4× longer with values held constant per source interval
- [ ] `resample(3600)` on a 15-min (900 s) schedule returns an error (downsampling not supported)
- [ ] Requesting a missing required column returns an error that names the missing column
- [ ] A schedule that does not cover the simulation period returns an error identifying the uncovered interval
- [ ] A CSV with no timezone annotation and a `WeatherMeta` provided uses `WeatherMeta.timezone_offset_h` for timestamp parsing

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
