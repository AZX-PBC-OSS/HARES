# ResStock data pipeline: OEDI S3 download, building sampling, weather pairing
**Review ID**: py-companion-03
**Category**: py-companion
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/data/resstock.py`
- `python/ochre_next/data/weather.py`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Analysis.py` — `download_resstock_model()` (single-building download via boto3, no retries or checksums)
- `vendors/OCHRE/ochre/utils/schedule.py` — `import_weather()` (EPW/NSRDB parsing), comment referencing TMY3 zip URL as "FUTURE"
- `vendors/OCHRE/ochre/utils/hpxml.py` — HPXML parser with ResStock-specific parameter defaults
- `vendors/OCHRE/docs/source/InputsAndArguments.rst` — ResStock dataset documentation and BuildStockBatch reference

## Findings

### Finding 1: [Severity: high]
**Description**: `fetch_resstock_fleet()` performs uniform random sampling instead of weight-proportional sampling. When `n_buildings` is specified, `df.sample(n=..., shuffle=True, seed=0)` selects rows with equal probability, ignoring the `sample_weight` column. The weight column is loaded (lines 473-489) and propagated to the `ResStockBuilding.sample_weight` field, but is never passed to the DataFrame's `sample()` method. This means a building archetype representing 10% of the housing stock has the same selection probability as one representing 0.01%, biasing fleet composition away from the true housing stock distribution.

**Code Location**: `python/ochre_next/data/resstock.py:482`
```python
df = df.sample(n=min(n_buildings, len(df)), shuffle=True, seed=0)
```

**Root Cause**: The `sample()` call omits the `weights` parameter. Polars `DataFrame.sample()` accepts `weights: str | pl.Series | None` — the code discovers `weight_col` (line 473-476) but never connects the two.

**Impact**: Any downstream analysis (energy consumption aggregates, load shape synthesis, policy simulations) that relies on fleet sampling will produce statistically incorrect results — rare archetypes will be overrepresented and common archetypes underrepresented, skewing fleet-level statistics toward unrepresentative buildings.

**Comparison to vendor**: The vendor OCHRE code does not perform bulk building sampling at all (`download_resstock_model()` downloads one building at a time by ID), so this correctness issue is unique to the HARES companion.

---

### Finding 2: [Severity: high]
**Description**: Download functions have no retry logic for transient network failures. `_try_download()` in `resstock.py` (line 149) and `_download_large_file()` in `weather.py` (line 113) each make a single HTTP/S3 request. If the request fails due to a transient error (socket timeout, DNS resolution failure, S3 503 SlowDown, connection reset), the exception propagates immediately to the caller with no retry. The 760 MB `BuildStock_TMY3_FIPS.zip` download in `weather.py` is especially vulnerable — a single connection drop near completion voids the entire transfer.

**Code Location**:
- `python/ochre_next/data/resstock.py:149-181` — `_try_download()` (three backends, zero retries)
- `python/ochre_next/data/weather.py:113-145` — `_download_large_file()` (two backends, zero retries)

**Root Cause**: Neither function implements a retry loop. Both write to a `.tmp` file and rename atomically on success (good practice), but the `except Exception` blocks delete the `.tmp` and re-raise without attempting again. No exponential backoff, no jitter, no configurable retry count.

**Impact**: Pipeline runs against the full ResStock dataset (hundreds of thousands of buildings) will fail non-deterministically with network errors. Since each building requires a separate download, the probability of at least one transient failure in a fleet-size run approaches 100%. Users running batch simulations overnight will find them aborted mid-run.

**Comparison to vendor**: The vendor OCHRE `download_resstock_model()` (Analysis.py:67-68) similarly has no retry — a single `s3_client.download_file()` call with no error handling wrapper. Both implementations share this gap.

---

### Finding 3: [Severity: high]
**Description**: No file integrity verification after download. Downloaded files are validated only by checking `st_size > 0` (resstock.py:288, 323-324, 389-390; weather.py:56). There is no SHA256, MD5, or even CRC checksum comparison against a known good value. A truncated download that still writes non-zero bytes, a bit-flip in transit, or a corrupted S3 object would pass the `st_size > 0` check and be used as valid data, potentially causing silent incorrect simulation results or cryptic downstream parsing errors.

**Code Location**:
- `python/ochre_next/data/resstock.py:288` — weather CSV cache check: `weather_dest.stat().st_size > 0`
- `python/ochre_next/data/resstock.py:323-324` — building ZIP cache check: `hpxml_path.stat().st_size > 0 and schedule_path.stat().st_size > 0`
- `python/ochre_next/data/weather.py:56` — EPW cache check: `epw_path.stat().st_size > 0`

**Root Cause**: The pipeline trusts that any non-empty cached file is correct. Integrity checks are absent from both the download layer and the cache retrieval layer. Without a checksum manifest or content hash stored alongside cached files, there is no way to detect corruption after the fact.

**Impact**: Silent data corruption. A corrupted HPXML could produce subtly wrong building physics parameters. A corrupted weather file could produce physically impossible results that pass validation. Because results are cached indefinitely, the corruption persists across pipeline runs until the cache is manually cleared.

**Comparison to vendor**: The vendor OCHRE code similarly has no checksum verification on download — the gap exists in both implementations. However, OCHRE targets single-building interactive use, whereas the HARES companion targets fleet-scale batch operation where silent corruption has compounding effects.

---

### Finding 4: [Severity: medium]
**Description**: Cache invalidation relies solely on file existence — there is no mechanism to detect when upstream data changes. If NREL publishes a corrected ResStock dataset, the pipeline will continue using stale cached files indefinitely. The cache keys (building ID, version string, upgrade ID) are structural but not content-aware.

**Code Location**:
- `python/ochre_next/data/resstock.py:323-324` — cache hit logic: checks file existence + non-zero size only
- `python/ochre_next/data/weather.py:56, 60-65` — cache hit logic: file existence + `.extracted` marker presence

**Root Cause**: The cache has no notion of content freshness. No ETag header, `Last-Modified` timestamp, S3 object version ID, or content hash is stored or compared against upstream metadata. The `.extracted` marker pattern in `weather.py:60-64` is particularly problematic — if the upstream `BuildStock_TMY3_FIPS.zip` is updated at NREL, the marker prevents re-extraction forever.

**Impact**: Users must manually delete `~/.cache/ochre_next/resstock/` (or `~/.cache/ochre_next/weather/`) to pick up upstream data corrections. This is error-prone and undocumented. Rolling back to an older dataset version requires the same manual intervention.

**Comparison to vendor**: The vendor OCHRE `download_resstock_model()` has an `overwrite=False` parameter (Analysis.py:64) but defaults to bypassing download if the ZIP exists locally — same pattern. Neither implementation handles upstream changes.

---

### Finding 5: [Severity: medium]
**Description**: Cache corruption is not detected or self-healed. If a cached `.epw` file from `BuildStock_TMY3_FIPS.zip` is truncated or corrupted during extraction (e.g., due to a crash, disk full, or concurrent process race), the pipeline will serve the corrupt file because the `.extracted` marker already exists. There is no per-file integrity marker, no re-extraction trigger, and no automatic repair.

**Code Location**:
- `python/ochre_next/data/weather.py:60-65` — `.extracted` marker gate prevents re-extraction even for individual corrupted files
- `python/ochre_next/data/weather.py:100-101` — individual EPW extraction still only checks `target.exists()`, not integrity

**Root Cause**: The `.extracted` marker is a coarse-grained "all done" signal that assumes extraction is atomic and infallible. Individual file extraction has the same `st_size > 0` check pattern (`resstock.py:288`) without content verification. Concurrent extraction from multiple processes could write the `.extracted` marker before all files are fully written (the atomic `.tmp` rename on line 101-104 mitigates per-file races but not the marker race).

**Impact**: Intermittent and hard-to-diagnose errors. A corrupted EPW file could produce Invalid TMY data that downstream EnergyPlus or OCHRE simulations silently accept but produce wrong results. Users would not know to clear the cache.

---

### Finding 6: [Severity: medium]
**Description**: `fetch_resstock_fleet()` uses `asyncio.gather(*tasks)` (line 395) with no concurrency limit. When fetching a large fleet (e.g., N=1000+ buildings), the function creates an unbounded number of concurrent HTTP connections to the OEDI S3 bucket. This can trigger S3 rate limiting (503 SlowDown), overwhelm local file descriptors, cause memory pressure from simultaneous ZIP downloads held in RAM, and produce poor throughput due to connection contention. The `timeout=120.0` (line 383) provides a slow timeout but no flow control.

**Code Location**: `python/ochre_next/data/resstock.py:383-395`
```python
async with httpx.AsyncClient(follow_redirects=True, timeout=120.0) as client:
    tasks = []
    for bid in bldg_ids:
        ...
        tasks.append(_download_building_async(client, cfg, bid, upgrade_id, bldg_dir))
    await asyncio.gather(*tasks)
```

**Root Cause**: No semaphore, connection pool limit, or batching is applied. `httpx.AsyncClient` defaults to `limits=Limits(max_connections=100)` internally, but 100 concurrent downloads to S3 is still aggressive for large fleets. Additionally, `_download_building_async` (line 364) loads the entire ZIP into RAM with `resp.content` rather than streaming to disk, so concurrent ZIPs compete for memory.

**Impact**: For fleet sizes of 1000+, this will cause: (a) S3 rate limiting with opaque error messages, (b) memory exhaustion if many ZIPs are cached simultaneously (line 364 `resp.content` buffers entire ZIPs in RAM), (c) excessive open file handles on systems with low `ulimit -n`. The `except ImportError` fallback path (line 414-423) is sequential and avoids this problem, but the async path is the primary code path.

**Comparison to vendor**: Not applicable — vendor OCHRE downloads one building at a time.

---

### Finding 7: [Severity: medium]
**Description**: Building-to-weather pairing relies on brittle FIPS code extraction from HPXML weather station names. The `_fips_from_weather_name()` function (line 219-235) assumes station names follow the pattern `G + state_fips(2) + county_fips(3) + suffix(2)` (e.g., `G0100290`). It requires the name to start with `G`, be at least 7 characters, and have a recognizable state FIPS prefix. Station names that deviate from this pattern (e.g., custom station names, non-US locations, alternative TMY naming schemes) silently return `None`, causing `_fetch_weather()` to return `Path("")` — an empty path that downstream simulation will fail on with a cryptic file-not-found error rather than a clear "no weather station found" message.

**Code Location**: `python/ochre_next/data/resstock.py:219-235`

**Root Cause**: The FIPS extraction is pattern-matched rather than looked up from the HPXML or the metadata parquet. The metadata parquet (loaded at line 467) likely contains climate zone or county information in structured columns (e.g., `in.county`, `in.state`, `in.iecc_climate_zone`), but this structured data is not used for weather pairing. The HPXML also contains structured `<ClimateandIECCZones/>` elements (part of the HPXML schema) that could provide authoritative mapping but are not parsed.

**Impact**: Buildings with non-standard station naming silently receive empty weather paths. The error surfaces far from the root cause, making debugging difficult. Additionally, the code does not validate that the resolved weather file actually matches the building's IECC or ASHRAE 169-2013 climate zone — a mismatch (e.g., a Florida home paired with Alaska weather) would go undetected.

---

### Finding 8: [Severity: low]
**Description**: `weather.py` `_download_large_file()` comment says "Download a large file with progress" (line 113) but no progress reporting is implemented. For a 760 MB download, users have no indication of download speed, ETA, or whether the process is stalled.

**Code Location**: `python/ochre_next/data/weather.py:113-114`

**Root Cause**: The docstring is aspirational — the implementation writes chunks to disk without any callback or progress hook.

**Impact**: Poor user experience for first-time setup. Users may kill the process thinking it's hung.

---

### Finding 9: [Severity: low]
**Description**: `fetch_resstock_fleet()` loads the full metadata parquet into a Polars DataFrame (line 467) and creates a `ResStockBuilding` list for all selected buildings (line 378), both held in memory simultaneously. For a full national dataset (hundreds of thousands of rows in the metadata parquet, tens of thousands of selected buildings), this could exhaust memory on constrained systems. However, this is mitigated by the use of Polars (columnar, lazy evaluation possible) and the fact that individual `ResStockBuilding` dataclasses are small (~5 fields).

**Code Location**: `python/ochre_next/data/resstock.py:467, 378`

**Root Cause**: The pipeline uses eager `pl.read_parquet()` rather than `pl.scan_parquet()` with lazy evaluation, and collects all results into a Python list rather than yielding a generator. For 10,000 buildings with ~100 bytes per dataclass, the list overhead is ~1 MB — acceptable. For 550,000 buildings (full ResStock dataset), it's ~55 MB — manageable on modern hardware but worth noting.

**Impact**: Low risk for typical workstation hardware (16+ GB RAM). Higher risk on cloud instances with constrained memory or when multiple pipeline instances run simultaneously.

---

## Summary
- **Total findings**: 9
- **Critical**: 0
- **High**: 3 (weight-proportional sampling omission, no retry/backoff for downloads, no checksum verification)
- **Medium**: 4 (stale cache invalidation, no corruption self-healing, unbounded concurrency on fleet download, brittle FIPS extraction without climate zone validation)
- **Low**: 2 (no download progress, potential memory pressure at extreme scale)

## Recommendations
1. **Fix weighted sampling** (Finding 1): Pass `weights=weight_col` to `df.sample()` when a sample weight column is available. This is a one-line change with high impact on statistical correctness.
2. **Add retry with exponential backoff** (Finding 2): Wrap `_try_download()` and `_download_large_file()` with a retry loop (e.g., `tenacity` or manual loop with `asyncio.sleep`). Include jitter to avoid thundering herd. Configure at least 3 retries for 503/429/timeout errors.
3. **Add file integrity verification** (Finding 3): Download checksum manifests alongside data files (ResStock metadata includes parquet files with row-level metadata — consider extending). Compute SHA256 of cached files and validate on each cache hit. Store a `.sha256` sidecar file next to cached artifacts.
4. **Implement cache freshness** (Finding 4): On cache hit, issue an HTTP HEAD request to check ETag or Last-Modified against upstream. Store the ETag/timestamp alongside cached files. Provide a `--refresh`/`force` parameter to bypass cache.
5. **Improve cache corruption handling** (Finding 5): Replace the monolithic `.extracted` marker with per-file integrity tracking. On cache miss or corruption detection, automatically re-download only the affected files. Consider atomic writes to a staging directory then rename into cache.
6. **Add concurrency limiting** (Finding 6): Use `asyncio.Semaphore` or `httpx.Limits(max_connections=...)` to cap concurrent downloads to a reasonable number (e.g., 10-20). Stream ZIP downloads to disk instead of buffering in RAM via `resp.aiter_bytes()` with chunked writes.
7. **Use structured metadata for weather pairing** (Finding 7): Extract climate zone and county FIPS from the metadata parquet columns (e.g., `in.iecc_climate_zone`, `in.county`) rather than pattern-matching on HPXML weather station names. Validate that the paired weather file's location matches the building's climate zone.
8. **Add download progress** (Finding 8): Implement optional progress reporting using `tqdm` or similar for the 760 MB weather zip download.
9. **Consider lazy evaluation for metadata** (Finding 9): Use `pl.scan_parquet()` for metadata loading when only a subset of columns/rows is needed, to defer I/O and reduce memory footprint.

## References / Citations
- OEDI S3 bucket base: `https://oedi-data-lake.s3.amazonaws.com/nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/`
- NREL BuildStock TMY3 dataset: `https://data.nrel.gov/submissions/156`
- Polars `DataFrame.sample()` weight parameter: https://docs.pola.rs/api/python/stable/reference/dataframe/api/polars.DataFrame.sample.html
- HTTP `Range` request for resume: RFC 7233
- AWS S3 retry best practices: exponential backoff with jitter for 5xx/429 errors
- `tenacity` library for retry decorators: https://tenacity.readthedocs.io/
