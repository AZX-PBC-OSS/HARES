# ResStock building dataset integration: sampling, fixture loading, AMY pairing
**Review ID**: config-io-05
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/resstock.rs crates/hares-io/src/resstock_csv.rs python/ochre_next/data/resstock.py

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Analysis.py (download_resstock_model), vendors/OCHRE/ochre/utils/schedule.py (import_weather), vendors/OCHRE/ochre/utils/hpxml.py (WeatherStation parsing)

## Findings
### Finding 1: [Severity: high]
**Description**: Weighted sampling by housing stock weights is NOT implemented. The Python `fetch_resstock_fleet()` function reads `sample_weight` from metadata but never uses it for sampling.

**Code Location**: `python/ochre_next/data/resstock.py:482` -- `df.sample(n=min(n_buildings, len(df)), shuffle=True, seed=0)` performs uniform random sampling without weight argument. Weights are collected into a dictionary at lines 487-489 but stored only for downstream attribution, not for weighted selection.

**Root Cause**: The `polars.DataFrame.sample()` API supports a `weights` parameter, but it is not passed. The weight column is discovered at lines 473-476 via a fuzzy column name match, then stored on `ResStockBuilding` objects at line 408, but never inform the sampling step.

**Impact**: A sample of N buildings will be drawn uniformly from the metadata table regardless of the housing stock weighting. This produces non-representative building fleets. For example, if detached single-family homes in a given climate zone represent 60% of the stock weight but only 10% of the metadata rows, they will be dramatically under-represented in the sample.

### Finding 2: [Severity: high]
**Description**: No climate-zone validation between building HPXML and paired weather file. A building from one ASHRAE climate zone can be silently paired with weather from an entirely different zone.

**Code Location**: `python/ochre_next/data/resstock.py:253-292` (`_fetch_weather`). Weather file is matched to building solely by FIPS code extracted from the HPXML `WeatherStation/Name` element (line 270). The HPXML contains an explicit `ClimateandRiskZones/ASHRAE/Zone` element (per the OCHRE reference at `vendors/OCHRE/ochre/utils/hpxml.py:1806`), but this value is never read or cross-referenced against the weather file's climate zone. Similarly, `crates/hares-fleet/src/fleet.rs:121-156` (`Fleet::from_resstock`) builds weather paths via `remap_weather_path()` without any zone validation.

**Root Cause**: The weather pairing pipeline trusts the FIPS-to-weather mapping without verifying climate zone compatibility. The OCHRE reference at `vendors/OCHRE/ochre/utils/schedule.py:125-129` similarly trusts the mapping but issues a WARNING when the weather station name differs from the weather file name -- HARES has no equivalent sanity check.

**Impact**: Cross-zone pairings (e.g., a Miami FL building paired with a Minneapolis MN weather file) would produce physically meaningless simulation results with no detection. This risk is highest when weather files are overridden via `weather_override` (line 309-310) or `weather_format` for cross-version pairing (e.g., TMY3 EPW for an AMY 2018 building).

### Finding 3: [Severity: high]
**Description**: The Rust `ResStockBuilding.weather_path` is set to the building ZIP file path, not to an actual weather file. The remap logic in `Fleet::from_resstock()` does not construct a valid per-building weather path.

**Code Location**: `crates/hares-io/src/resstock.rs:287-293` sets `hpxml_path`, `schedule_path`, and `weather_path` all to the same `zip_path` (e.g., `bldg0123456-up00.zip`). The `remap_weather_path()` function at `crates/hares-fleet/src/fleet.rs:575-583` extracts the filename from this zip path and joins it with `weather_dir`, producing e.g. `weather_dir/bldg0123456-up00.zip`. This is not a weather file.

**Root Cause**: The ResStock building zip contains HPXML and schedule CSV only. Weather files are stored separately on OEDI S3 (e.g., `weather/state=CO/G0800130_2018.csv`) and must be resolved via the HPXML WeatherStation element. The Rust path does not perform this resolution, unlike the Python implementation at `resstock.py:253-292` which correctly extracts the FIPS code and constructs a separate weather URL.

**Impact**: The `Fleet::from_resstock()` constructor cannot run simulations because the weather path points to the building zip, not a parseable weather file. This path is effectively dead unless `weather_dir` happens to contain files named identically to the ResStock building zips. Contrast with Python `fetch_resstock_building()` (line 334) which correctly resolves weather via `_fetch_weather()`.

### Finding 4: [Severity: medium]
**Description**: No retry or exponential backoff on OEDI S3 downloads. Network interruptions cause immediate, unrecoverable failures.

**Code Location**: `python/ochre_next/data/resstock.py:137-181` (`_download_file` and `_try_download`). The download chain tries `httpx` → `boto3` → `urllib` in sequence, but within each transport there are zero retries. The `_async` variant at lines 352-366 (`_download_building_async`) likewise uses a single `await client.get(url)` with no retry logic. The `.tmp` file is cleaned up on failure (line 145: `tmp.unlink(missing_ok=True)`), so a fresh download must restart from zero.

**Root Cause**: The download implementations treat all failures as permanent. Transient network errors (socket resets, DNS failures, HTTP 503) become terminal.

**Impact**: Large fleet downloads (thousands of buildings) will experience sporadic failures in production environments. A single failed building can abort the entire `asyncio.gather()` call chain at line 395 if exceptions are not suppressed individually.

### Finding 5: [Severity: medium]
**Description**: No checksum verification for downloaded files. Corrupted downloads pass size checks and go undetected.

**Code Location**: `python/ochre_next/data/resstock.py:287-289` -- cache validity is determined by `weather_dest.exists() and weather_dest.stat().st_size > 0`. No MD5/SHA digest comparison. Similarly, building zip cache checks at lines 323-324 (`hpxml_path.exists() and hpxml_path.stat().st_size > 0`) and lines 389-390 use only existence + size. The TMY3 EPW ZIP download at `python/ochre_next/data/weather.py:79-110` likewise has no integrity check.

**Root Cause**: OEDI S3 does not provide content hashes in the download response. NREL Data Catalog files (e.g., `Buildstock_TMY3_FIPS-1678817889.zip`) have a known-size ZIP but no published checksum.

**Impact**: A partially-downloaded or corrupted ZIP or CSV file will yield corrupted HPXML data or invalid weather time series, producing NaN/infinite simulation results with no clear root cause. Compare to the SAM PV adapter at `python/ochre_next/adapters/sam_pv.py:62-69` which does compute and store a SHA256 hash for EPW files.

### Finding 6: [Severity: medium]
**Description**: Zero and NaN sample weights are not validated. A building with `sample_weight == 0.0` or `sample_weight == NaN` is parsed without error and downstream aggregation silently corrupts results.

**Code Location**: `crates/hares-io/src/resstock.rs:274` -- `let sample_weight = weight_arr.value(row_idx)` reads an `f64` from Arrow with no range/sanity check. Null is caught at line 261, but zero and NaN pass through. The weight is propagated to `crates/hares-fleet/src/aggregation.rs:358-359` where it is used in weighted sums: `weighted_values[idx] += *v * *sample_weight; total_weight[idx] += *sample_weight`. A NaN weight would propagate NaN through all aggregation; a zero-weight building would produce zero-weighted aggregations but still consumes compute.

**Root Cause**: The Arrow `Float64Array` permits any IEEE 754 value. No post-read validation is performed.

**Impact**: NaN weights silently corrupt fleet-level aggregation metrics with NaN values that are indistinguishable from simulation failures. All-zero weights waste compute on buildings that contribute nothing to population estimates.

### Finding 7: [Severity: medium]
**Description**: The Python `fetch_resstock_building()` hardcodes `sample_weight=1.0`, silently discarding metadata weights when fetching individual buildings (non-fleet path).

**Code Location**: `python/ochre_next/data/resstock.py:341` -- `sample_weight=1.0` in the `ResStockBuilding` dataclass constructor. The fleet path at line 408 correctly carries weights from metadata, but the single-building path has no access to the metadata parquet and defaults to `1.0`.

**Root Cause**: `fetch_resstock_building()` does not accept a metadata path or weight override parameter. The function is standalone and only handles download + extraction.

**Impact**: Individual building simulations using `fetch_resstock_building()` produce results that cannot be correctly re-weighted for population-level aggregation. Users must separately track metadata weights.

### Finding 8: [Severity: low]
**Description**: Test fixtures cover only one climate zone (Colorado) and one building vintage. No coverage across ASHRAE climate zones, building vintages, or geographic regions.

**Code Location**: `crates/hares-io/src/resstock.rs:360-381` (`sample_batch`) uses `bldg_id=12345`, `state=CO`, `upgrade=3`. All test variants (`sample_batch_v2024_2`, `sample_batch_v2025`) use the same ID and state. `crates/hares-io/tests/comprehensive_tests.rs:363-765` covers version mismatch and null handling but uses the same CO singletons.

**Root Cause**: Tests were designed to verify parsing mechanics, not dataset representativeness. No test fixture builder generates diverse buildings.

**Impact**: Parsing bugs specific to certain climate zones (e.g., territories with non-standard FIPS codes like DC=11, PR=72) or vintage-specific HPXML structures would go undetected.

### Finding 9: [Severity: low]
**Description**: S3 download errors do not identify which building ID caused the failure. The `asyncio.gather()` call at line 395 will propagate the first exception with generic HTTP status.

**Code Location**: `python/ochre_next/data/resstock.py:362-363` -- `resp.raise_for_status()` raises `HTTPStatusError` without building ID context. The result is a stack trace that requires the user to correlate failure timestamps with the request order.

**Root Cause**: Exceptions from `_download_building_async()` are not caught and re-wrapped with building ID context.

**Impact**: For fleet downloads of thousands of buildings, diagnosing which specific building failed requires manual investigation.

## Summary
- Total findings: 9
- Critical: 0
- High: 3
- Medium: 4
- Low: 2

## Recommendations
1. Implement weighted reservoir/stratified sampling in `fetch_resstock_fleet()` using the `sample_weight` column already discovered at lines 473-476. Pass `weights=` to `pl.DataFrame.sample()`.
2. Extract ASHRAE climate zone from HPXML during `_parse_weather_station()` (or via a new `_parse_climate_zone()`) and validate against the weather file's station metadata before accepting the pairing. Issue a warning for known-cross-zone cases; error for detectably invalid combos.
3. Fix the Rust weather path: either (a) parse the HPXML WeatherStation element from the ZIP to resolve the FIPS code and construct a proper weather file path, or (b) match OCHRE's approach of extracting the WeatherStation name and looking up the EPW/CSV by FIPS. The current `weather_path` pointing to the building ZIP is unusable for simulation.
4. Add retry with exponential backoff (e.g., 3 retries, 1s/2s/4s delays) for all OEDI S3 download paths. Wrap individual building downloads in exception handlers that report which bldg_id failed.
5. Publish or embed metadata checksums (SHA256) for the major ResStock release files and verify after download. At minimum, verify ZIP file integrity via `ZipFile.testzip()` before extraction.
6. Validate sample weights post-parse: reject NaN, log warnings for zero, and check that at least one weight is positive before proceeding with fleet construction.
7. Accept a `metadata_path` argument in `fetch_resstock_building()` so single-building callers can look up correct sample weights instead of defaulting to 1.0.
8. Expand test fixtures to include at least one building from each of the major IECC/ASHRAE climate zones (1A, 2B, 3C, 4A, 5B, 6, 7, 8) and at least one from each major vintage bin (pre-1950, 1950s, 1970s, 1990s, 2000s, 2010s).

## References / Citations
- OCHRE `download_resstock_model()`: `vendors/OCHRE/ochre/Analysis.py:23-77` (S3 download via boto3, no retry, no checksum)
- OCHRE `import_weather()`: `vendors/OCHRE/ochre/utils/schedule.py:123-270` (weather file lookup by station name, TMY3/EPW fallback)
- OCHRE HPXML WeatherStation extraction: `vendors/OCHRE/ochre/utils/hpxml.py:1805-1807`
- OCHRE `run_multiple.py`: `vendors/OCHRE/bin/run_multiple.py:67-80` (hardcoded single Denver weather file for all buildings, no zone validation)
- SAM PV SHA256 pattern: `python/ochre_next/adapters/sam_pv.py:62-69` (reference for checksum verification)
- polars `DataFrame.sample()` weights parameter: https://docs.pola.rs/api/python/stable/reference/dataframe/api/polars.DataFrame.sample.html
- ResStock dataset documentation: https://resstock.nrel.gov/datasets
