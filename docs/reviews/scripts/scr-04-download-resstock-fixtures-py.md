# download_resstock_fixtures.py: S3 download robustness and fixture coverage
**Review ID**: scr-04
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
- `scripts/download_resstock_fixtures.py`
- `python/ochre_next/data/resstock.py` (indirect — script delegates all download logic here)
- `python/ochre_next/data/weather.py` (indirect — weather EPW downloads)
- `tests/fixtures/resstock/` (committed fixture corpus)

## Vendor/Reference Files Consulted
None — no vendor equivalent exists for fixture download orchestration.

## Findings

### Finding 1: [Severity: critical]
**Description**: No retry logic for transient S3/HTTP failures. The script delegates all downloads to `fetch_resstock_building()` (line 80), which internally calls `_try_download()` in `resstock.py:149`. That function makes a single attempt via httpx, boto3, or urllib with zero retries — no exponential backoff, no jitter, no configurable retry count. If any transient error occurs (S3 503 SlowDown, socket timeout, DNS resolution failure, connection reset), the exception propagates to the script's `except Exception` handler (line 86-88), which logs a warning and skips to the next building. The weather file download path (`_download_large_file()` in `weather.py:113`) has the same single-attempt behavior, including the 760 MB `BuildStock_TMY3_FIPS.zip` that must be downloaded once per cache miss.

**Code Location**:
- `scripts/download_resstock_fixtures.py:80` — delegates to `fetch_resstock_building()` with no retry wrapper
- `python/ochre_next/data/resstock.py:149-181` — `_try_download()`: httpx → boto3 → urllib fallback chain, zero retries at each backend
- `python/ochre_next/data/resstock.py:291` — `_fetch_weather()` calls `_download_file()` with no retry
- `python/ochre_next/data/weather.py:113-145` — `_download_large_file()`: single attempt with temporary file, no retry

**Root Cause**: Neither the script layer nor the data layer adds a retry wrapper. Both write downloads to a `.tmp` file and atomically rename on success (`tmp.replace(dest)`), which is the correct atomic-write pattern, but the `except Exception` blocks delete the `.tmp` and re-raise without attempting again. The script's `continue` on exception (line 88) limits blast radius but does not fix the underlying transient-failure vulnerability.

**Impact**: On any non-trivial fixture set, the probability of at least one download failing transiently is non-trivial. Since each building + weather file constitutes 2+ separate HTTP/S3 requests per version, downloading 3 buildings across 2 versions requires ~12+ network requests. A single transient failure silently drops that building from the fixture set with only a `WARNING` log line. For CI runners on unreliable networks, fixture downloads become non-deterministic. This overlap with `py-companion-03` Finding 2 exists because the data layer's gap propagates upward to the script.

---

### Finding 2: [Severity: critical]
**Description**: No checksum or content-integrity verification of downloaded files. After `fetch_resstock_building()` returns, the script copies files to the fixture directory with `shutil.copy2()` (lines 93-100), but never verifies their content. The underlying data layer validates cached files only by `st_size > 0` (resstock.py:288, 323-324; weather.py:56) — a zero-byte check, not an integrity check. There is no SHA256, MD5, or ETag comparison against any known-good value. A truncated download that writes non-zero bytes (e.g., a TCP connection closed mid-stream after the first chunk), a bit-flip in transit, or a corrupted S3 object would pass the size check and be committed as a valid fixture, producing silently wrong simulation results in CI.

**Code Location**:
- `scripts/download_resstock_fixtures.py:93-100` — `shutil.copy2()` copies files with no integrity check
- `python/ochre_next/data/resstock.py:323-324` — cache validity check: `hpxml_path.stat().st_size > 0 and schedule_path.stat().st_size > 0`
- `python/ochre_next/data/resstock.py:288` — weather CSV validity: `weather_dest.stat().st_size > 0`
- `python/ochre_next/data/weather.py:56` — EPW validity: `epw_path.stat().st_size > 0`

**Root Cause**: The pipeline trusts that any non-empty file is correct. No checksum manifest exists (not in the repo, not on S3, not in a sidecar file). The OEDI data lake does not expose ETag headers through the unsigned S3 access path. Without a manifest, the script cannot verify that committed fixtures match the upstream source. If the upstream ResStock data is updated (new release of the same version number), there is no mechanism to detect the drift.

**Impact**: Silent data corruption that propagates to all downstream consumers. A corrupted HPXML could produce subtly wrong building physics parameters (incorrect insulation R-values, wrong equipment sizes) that would not cause the smoke tests to fail but would bias simulation results. A corrupted weather file could produce physically plausible but incorrect temperature/humidity profiles. The overlap with `py-companion-03` Finding 3 is acknowledged; the script inherits this gap and does not mitigate it.

---

### Finding 3: [Severity: high]
**Description**: Fixture selection is not representative. The script downloads buildings by sequential integer ID (`--bldg-ids 1,2,3` by default) with no stratification across the dimensions that matter for simulation correctness: climate zones, building types, heating/cooling equipment, vintages, PV presence, or EV chargers. The docstring claims "representative" (line 2), but sequential IDs are arbitrary and do not correspond to a stratified sample. Analysis of the 6 committed fixtures (IDs 2,3,4 across two versions) reveals:

| Dimension | Covered | Missing |
|---|---|---|
| Climate zones | IECC 3B, 5A, 5B (3 of 16+ zones) | 1A–2C, 3A,3C, 4A–4C, 6A–6C, 7, 8 |
| Building types | single-family detached, apartment unit | manufactured/mobile home, single-family attached (townhouse) |
| Heating fuel | natural gas (5), electricity (1) | propane, fuel oil, wood |
| Heating type | furnace, boiler, wall furnace, heat pump | electric resistance, stove/space heater |
| Water heater fuel | electricity (4), natural gas (2) | heat pump WH, propane, fuel oil |
| PV system | None (0 of 6) | **None** |
| EV charger | None (0 of 6) | **None** |
| Battery storage | None (0 of 6) | **None** |

The complete absence of PV, EV, and battery storage fixtures means any test exercising DER equipment pathways (PV inverter, EV charger, battery cycling) has no ResStock fixture to run against — even though these DER configurations exist in the full ResStock dataset.

**Code Location**: `scripts/download_resstock_fixtures.py:46` — `default="1,2,3"` with no stratification logic
`scripts/download_resstock_fixtures.py:70-71` — iterates `bldg_ids` in a simple for-loop with no metadata-driven selection

**Root Cause**: The script accepts arbitrary building IDs from the command line and has no mechanism to query the ResStock metadata parquet to select buildings with desired characteristics. The `fetch_resstock_fleet()` function in `resstock.py:427` already supports metadata-driven filtering via `pl.read_parquet()` and column equality filters (`filter` parameter), but the fixture download script does not use it. The script treats building IDs as opaque integers with no connection to building characteristics.

**Impact**: Smoke tests that pass on the current fixture set may fail on the first building that has a PV system, EV charger, or manufactured home geometry because those code paths are never exercised. The test suite provides a false sense of coverage — `resstock_smoke.rs` runs successfully on all committed fixtures, but the fixtures represent only a narrow slice of the ResStock diversity.

---

### Finding 4: [Severity: medium]
**Description**: Script default arguments (`--bldg-ids 1,2,3`) do not match the committed fixture set (bldg 2,3,4). Running the script with defaults would attempt to download building 1, which does not exist in either version's committed fixtures. The actual committed fixtures are IDs 2,3,4 — suggesting building 1 failed to download for both versions and was replaced by re-running with `--bldg-ids 2,3,4`. The script's docstring (`default: 1,2,3`) is now out of date with reality.

**Code Location**: `scripts/download_resstock_fixtures.py:46` — `default="1,2,3"`
`tests/fixtures/resstock/2024.2/` — contains only `bldg0000002`, `bldg0000003`, `bldg0000004`
`tests/fixtures/resstock/2025.1/` — contains only `bldg0000002`, `bldg0000003`, `bldg0000004`

**Root Cause**: The script uses a fixed default that was set at creation time. When building 1 was unavailable (likely a dataset gap — building IDs in ResStock are not guaranteed to include every integer), the author re-ran the script with different IDs to work around the failure, but did not update the default or document the selection rationale.

**Impact**: A new developer or CI job running the script with defaults will fail on building 1 and produce a partial fixture set (only buildings 2 and 3), different from the set expected by the smoke tests. This breaks reproducibility.

---

### Finding 5: [Severity: medium]
**Description**: Error handling is partially correct but lacks a failure summary. The script's `except Exception` handler (line 86-88) correctly logs a warning and `continue`s to the next building, ensuring one failed download does not block remaining fixtures. However, the script never produces a summary of which buildings failed, making failures easy to overlook in CI logs. The warning log line is at `WARNING` level, which may be filtered out of CI output by default log level settings. Additionally, the script exits with status code 0 even when all downloads fail — there is no error tracking or non-zero exit code.

**Code Location**:
- `scripts/download_resstock_fixtures.py:86-88` — `log.warning(...)` + `continue`
- `scripts/download_resstock_fixtures.py:104` — `log.info("Done.")` regardless of success/failure count

**Root Cause**: No accumulator for failed downloads, no post-loop summary inspection, and no `sys.exit()` with a non-zero code. The script treats download failures as non-fatal but does not distinguish "all succeeded" from "all failed" in its exit behavior.

**Impact**: In a CI pipeline, a completely empty fixture download (all buildings failed) would exit 0 and appear successful. Downstream tests that depend on fixtures may then fail with "file not found" errors that are harder to diagnose than a clear "fixture download failed" message. A developer running the script manually would need to scan log output carefully to notice building-level failures.

---

### Finding 6: [Severity: low]
**Description**: No disk space check before downloading. The script creates fixture directories (`fixtures_root.mkdir(parents=True, exist_ok=True)`) and copies files with `shutil.copy2()` but never checks available disk space. Each ResStock building bundle (HPXML + schedule CSV + weather file) is a few megabytes, so disk space is unlikely to be an issue for the current small fixture set, but could become a concern if the fixture set is expanded to hundreds of buildings. The underlying data layer also lacks disk space checks — `_download_file()` writes to a `tempfile.TemporaryDirectory()` but does not verify free space. On macOS CI runners with limited disk allocation (typically 14 GB on GitHub-hosted runners), this is unlikely to matter today but represents a missing safety check.

**Code Location**: `scripts/download_resstock_fixtures.py:62-68` — directory creation with no space check
`python/ochre_next/data/resstock.py:326-328` — `TemporaryDirectory()` usage with no space check

**Root Cause**: Disk space checking is not part of the script's design. The ResStock dataset can be 500+ GB in full, but the script only downloads individual buildings. The risk is low but non-zero for edge cases (e.g., a CI runner with a near-full disk from prior pipeline steps).

**Impact**: Low risk today. Could become a problem if the fixture set is expanded significantly or if CI runners have constrained disk space. The script operates on pathlib paths, which are cross-platform compatible between Linux and macOS.

---

### Finding 7: [Severity: low]
**Description**: No manifest or README documenting fixture selection criteria in the fixture directory. The `tests/fixtures/resstock/` directory contains no README file explaining what buildings are included, why they were selected, or how to regenerate the fixtures. The `tests/fixtures/hpxml/README.md` (an existing fixture directory with documentation) demonstrates the expected pattern. The script's own docstring (lines 1-25) provides some layout documentation but does not explain the selection rationale.

**Code Location**: `tests/fixtures/resstock/` — no README or manifest file present
`scripts/download_resstock_fixtures.py:1-25` — docstring describes output layout but not selection criteria

**Root Cause**: The script and fixture directory were created as infrastructure without the documentation convention already established for other fixture directories (`tests/fixtures/hpxml/README.md`, `tests/fixtures/parity/README.md`).

**Impact**: New contributors cannot determine whether fixture additions are needed or how to regenerate the fixture set if upstream ResStock data changes.

---

## Summary
- Total findings: 7
- Critical: 2 / High: 1 / Medium: 3 / Low: 1

## Recommendations

1. **Add retry with exponential backoff** at the script layer. Wrap `fetch_resstock_building()` in a retry loop (e.g., 3 attempts at 1s, 2s, 4s delays with jitter). Use `tenacity` or a manual `for attempt in range(3): try: ... except: time.sleep(2**attempt); continue`. This mitigates the data layer's lack of retries without requiring changes to the shared `ochre_next` library.

2. **Implement checksum verification**. Generate a SHA256 manifest file (`manifest.sha256`) alongside the fixtures containing the hash of each committed file. During download, compute the hash of the downloaded file and compare against the manifest. If the upstream data has changed (new ResStock release), the manifest must be regenerated. Store the manifest in the fixture root so CI can validate fixture integrity.

3. **Replace sequential ID selection with metadata-driven stratified sampling**. Use `fetch_resstock_fleet()` with `filter` parameters to select buildings that cover the missing dimensions:
   - At least one building per IECC climate zone group (1–2, 3–4, 5–6, 7–8)
   - At least one of each building type (single-family detached, attached, apartment, manufactured)
   - At least one building with PV, one with EV, one with heat pump water heater
   - At least one building of each major heating fuel type (natural gas, electricity, propane, fuel oil)
   - At least one pre-1980 vintage and one 2020+ vintage
   A target of 20-30 buildings across both versions would provide meaningful coverage while keeping fixture storage under 100 MB.

4. **Add a failure summary**. Track successful and failed building IDs, print a table at the end of execution, and exit with a non-zero status code if any downloads failed.

5. **Update default `--bldg-ids`** to match the actual committed fixture set (2,3,4) or remove the default entirely and require explicit IDs. Add a `--stratified` flag that uses metadata-driven selection instead of explicit IDs.

6. **Add a README.md** to `tests/fixtures/resstock/` documenting the selection criteria, the regeneration procedure, and the expected coverage dimensions. Follow the pattern established by `tests/fixtures/hpxml/README.md`.

7. **Add a disk space check** before downloading: query `shutil.disk_usage()` on the fixture root parent and warn if free space is below a threshold (e.g., 500 MB).

## References / Citations
- `python/ochre_next/data/resstock.py:149-181` — `_try_download()`: single-attempt download with fallback chain but no retries
- `python/ochre_next/data/resstock.py:295-345` — `fetch_resstock_building()`: main download function delegated to by the script
- `python/ochre_next/data/resstock.py:427-504` — `fetch_resstock_fleet()`: metadata-driven fleet download with `filter` parameter (available but unused by the script)
- `python/ochre_next/data/weather.py:113-145` — `_download_large_file()`: single-attempt weather download with no retries
- `python/ochre_next/data/weather.py:29-76` — `get_epw_for_fips()`: EPW weather file resolution with atomic extraction
- `tests/resstock_smoke.rs:29-41` — `fixture_building_dirs()`: consumer that dynamically discovers all building directories in the fixture tree, so any directory populated by the script will be tested
- `docs/reviews/py-companion/py-companion-03-resstock-data-pipeline.md` — overlapping review of the underlying data pipeline (retry/checksum gaps documented as Findings 2 and 3)
- `docs/reviews/infrastructure/infra-04-ci-no-test-execution.md` — CI does not currently run tests, including fixture-dependent smoke tests
- `tests/fixtures/hpxml/README.md` — exemplar of a documented fixture directory with selection criteria and source attribution
