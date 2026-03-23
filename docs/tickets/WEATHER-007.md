---
id: WEATHER-007
title: ResStock weather integration — EPW fetching by FIPS code
kind: implement
depends_on:
  - WEATHER-005
files_to_touch:
  - python/ochre_next/data/resstock.py
  - python/ochre_next/data/weather.py
references:
  - https://data.nrel.gov/submissions/156 (BuildStock_TMY3_FIPS.zip — EPW files by county)
  - https://data.openei.org/submissions/8217 (OEDI mirror)
  - vendors/OCHRE/ochre/utils/schedule.py (OCHRE's CSV weather handling)
  - python/ochre_next/data/resstock.py (current ResStock fetcher)
verification:
  - uv run pytest python/tests/ -v
---

## Background/Context

ResStock weather files on OEDI S3 are a **simplified 8-column CSV** format (not EPW,
not PSM3). They contain only: datetime, dry bulb, RH, wind speed/direction, GHI, DNI,
DHI. They are **missing** pressure, dew point, precipitation, infrared radiation, and
sky cover — fields HARES needs for psychrometrics (wet bulb, humidity ratio), sky
temperature (Clark-Allen needs dew point), water mains temperature, and PV soiling.

HARES cannot run a physically accurate simulation from ResStock CSV weather alone.

However, NREL publishes the **full EPW files** used to generate the ResStock weather
CSVs. These EPW files contain all 35+ fields and are organized by the same FIPS codes
ResStock uses:

### TMY3 EPW source
- **Dataset**: [TMY3 Weather Data for ComStock and ResStock](https://data.nrel.gov/submissions/156)
- **File**: `BuildStock_TMY3_FIPS.zip` (760 MB, updated Dec 2024)
- **Contents**: One EPW file per US county, named by FIPS code (e.g., `G0800130.epw`)
- **Coverage**: All US counties (including 3 previously missing Texas counties)
- **Resolution**: Hourly (8760 records)

### AMY 2018
- No public EPW download exists for the AMY 2018 dataset
- The 2025 ResStock release (`resstock_amy2018_release_1`) provides only the simplified
  8-column CSV
- For AMY 2018 simulations, the ResStock CSV would need to be augmented with estimated
  missing fields (pressure from elevation, dew point from RH + temp), or users must
  source EPW files from the NSRDB API directly

### Current data flow (broken)
```
ResStock S3 → {fips}_TMY3.csv (8-col CSV) → HARES Rust parse_epw() → FAILS
```

### Target data flow
```
ResStock S3 → FIPS code → fetch EPW from BuildStock_TMY3_FIPS.zip → parse_epw() → OK
```

For AMY 2018 (no EPW available):
```
ResStock S3 → {fips}_2018.csv (8-col CSV) → parse_resstock_csv() → WeatherTimeSeries
                                              (estimate missing fields)
```

## Work to Do

### Phase 1: TMY3 EPW fetching (primary path)

- [ ] Add EPW download/cache support to `python/ochre_next/data/weather.py`:
  - Download `BuildStock_TMY3_FIPS.zip` from NREL Data Catalog on first use
  - Cache to `~/.cache/ochre_next/weather/BuildStock_TMY3_FIPS/`
  - Extract individual EPW files by FIPS code
  - `def get_epw_for_fips(fips: str, cache_dir: Path) -> Path` — returns path to
    cached EPW file
  - Lazy download: only fetch the ZIP when first needed, then cache permanently

- [ ] Update `python/ochre_next/data/resstock.py`:
  - For TMY3 versions (V2024_1, V2024_2): fetch EPW instead of CSV
  - Change `_fetch_weather` to call `get_epw_for_fips(fips)` for TMY3 versions
  - `ResStockBuilding.weather_path` now points to `.epw` file
  - HARES Rust core already handles EPW via `parse_weather()` (WEATHER-005)

### Phase 2: AMY 2018 ResStock CSV parser (fallback path)

- [ ] Create `crates/hares-io/src/resstock_csv.rs`:
  - `pub fn parse_resstock_csv(path) -> Result<WeatherTimeSeries, WeatherError>`
  - Parse single-header-row CSV with 8 columns
  - Estimate missing fields:
    - **Pressure**: ISA standard atmosphere from elevation
      (`P = 101.325 * (1 - 2.25577e-5 * elevation_m)^5.25588` kPa)
      Elevation comes from HPXML or WeatherMeta
    - **Dew point**: from dry bulb + RH using ASHRAE psychrometric formula
      (already available in `hares-physics/src/psychrometrics.rs`)
    - **Horizontal infrared**: set to 0.0 (triggers Clark-Allen for sky temp)
    - **Sky temperature**: Clark-Allen from dry bulb + estimated dew point
    - **Opaque sky cover**: set to 0.0 (not available)
    - **Precipitation**: set to 0.0 (not available)
    - **Ground temperature**: DOE-2 model from monthly dry-bulb averages
  - Document which fields are measured vs estimated in the doc comment

- [ ] Update WEATHER-005 dispatch (`parse_weather`):
  - Add ResStock CSV detection: single header line containing
    `Dry Bulb Temperature` and `Global Horizontal Radiation`
  - Dispatch to `parse_resstock_csv`

- [ ] For V2025_1 (AMY 2018): use `parse_resstock_csv` as fallback since no EPW exists

### Phase 3: NSRDB API integration (future, optional)

- [ ] For users who want full-fidelity AMY data, provide an NSRDB API download option:
  - User provides NREL API key
  - Fetch PSM3 data by lat/lon for any year
  - Cache as PSM3 CSV, parsed by existing `parse_psm3()`
  - This is optional and not required for ResStock parity

## Files to Touch

- `python/ochre_next/data/weather.py`: EPW download/cache by FIPS code
- `python/ochre_next/data/resstock.py`: Update weather fetching for TMY3 → EPW
- `crates/hares-io/src/resstock_csv.rs`: New ResStock CSV parser (Phase 2)
- `crates/hares-io/src/weather.rs`: Add ResStock CSV to `parse_weather` dispatch
- `crates/hares-io/src/lib.rs`: Add module + re-export

## Measures of Success

- [ ] `get_epw_for_fips("G0800130")` downloads, caches, and returns a valid EPW path
- [ ] `ResStockBuilding.weather_path` points to `.epw` for TMY3 versions
- [ ] Full HARES simulation runs end-to-end with a ResStock building + EPW weather
- [ ] ResStock CSV parser produces valid `WeatherTimeSeries` with estimated fields
- [ ] Estimated dew point from RH + temp matches EPW dew point within 2°C for the
      same location (cross-validation)

## Verification

- [ ] `uv run pytest python/tests/ -v` passes
- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-io` passes

## Data Sources

- TMY3 EPW by FIPS: https://data.nrel.gov/submissions/156
- OEDI mirror: https://data.openei.org/submissions/8217
- ResStock TMY3 CSV: https://data.openei.org/s3_viewer?bucket=oedi-data-lake&prefix=nrel-pds-building-stock%2Fend-use-load-profiles-for-us-building-stock%2F2024%2Fresstock_tmy3_release_2%2Fweather%2F
- ResStock AMY2018 CSV: https://data.openei.org/s3_viewer?bucket=oedi-data-lake&prefix=nrel-pds-building-stock%2Fend-use-load-profiles-for-us-building-stock%2F2025%2Fresstock_amy2018_release_1%2Fweather%2F
- NSRDB API: https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/
