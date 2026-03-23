---
id: WEATHER-005
title: Unified weather file dispatch (EPW + PSM3) with format detection
kind: implement
depends_on:
  - WEATHER-003
files_to_touch:
  - crates/hares-io/src/weather.rs
  - crates/hares-io/src/lib.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/psm3.rs
  - vendors/OCHRE/ochre/utils/schedule.py (file extension dispatch, lines 166-204)
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

After WEATHER-003 adds the PSM3 parser, callers need a single entry point that
detects the weather file format and dispatches to the correct parser. OCHRE does
this via file extension checks (`.epw` vs `.csv`).

Since `.csv` is a generic extension that could be other formats, HARES should use a
two-stage approach: extension check first, then header sniffing for CSV files to
confirm it's actually PSM3 format.

## Work to Do

- [ ] Add `WeatherFormat` enum to `weather.rs`:
      ```rust
      /// Supported weather file formats.
      pub enum WeatherFormat {
          /// EnergyPlus Weather file (.epw), hourly data.
          Epw,
          /// NREL NSRDB PSM3 file (.csv), SAM CSV format at 5/15/30/60-min resolution.
          /// Reference: https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/
          Psm3,
      }
      ```

- [ ] Add `pub fn detect_weather_format(path: impl AsRef<Path>) -> Result<WeatherFormat, WeatherError>`:
  - `.epw` extension → `WeatherFormat::Epw`
  - `.csv` extension → three-line header sniffing (PSM3 has 3 header lines):
    1. Read line 1 (field names): must start with `Source` AND contain 10+ comma-separated fields
    2. Skip line 2 (field values)
    3. Read line 3 (column names): must contain `Year,Month,Day,Hour,Minute` and at
       least one of `GHI,DNI,DHI`
    If checks pass → `WeatherFormat::Psm3`. If not → return descriptive error
    ("CSV file does not appear to be PSM3/SAM format; expected NSRDB header")
  - Other extensions → return error listing supported formats

- [ ] Add `pub fn parse_weather(path: impl AsRef<Path>) -> Result<WeatherTimeSeries, WeatherError>`:
  - Calls `detect_weather_format` then dispatches to `parse_epw` or `parse_psm3`
  - Doc comment should list supported formats and detection logic

- [ ] Re-export `parse_weather` and `WeatherFormat` from `crates/hares-io/src/lib.rs`

- [ ] Update `crates/hares-core/src/dwelling/mod.rs` to use `parse_weather` instead of
      `parse_epw` directly (preserving existing behavior for EPW files)

- [ ] Add tests:
  - `detect_epw_by_extension`: verify `.epw` → `WeatherFormat::Epw`
  - `detect_psm3_csv`: verify valid PSM3 CSV → `WeatherFormat::Psm3`
  - `reject_unknown_csv`: verify non-PSM3 CSV → descriptive error
  - `reject_unknown_extension`: verify `.json` → descriptive error

## Files to Touch

- `crates/hares-io/src/weather.rs`: Add `WeatherFormat`, `detect_weather_format`,
  `parse_weather`
- `crates/hares-io/src/lib.rs`: Re-export `parse_weather`, `WeatherFormat`
- `crates/hares-core/src/dwelling/mod.rs`: Update weather file loading call site

## Measures of Success

- [ ] `.epw` files continue to work exactly as before
- [ ] `.csv` PSM3 files are detected and dispatched correctly
- [ ] Non-PSM3 CSV files produce a clear, actionable error (not a confusing parse error)
- [ ] Unsupported extensions produce a clear error listing supported formats
- [ ] No behavior change for existing EPW-based simulations

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
