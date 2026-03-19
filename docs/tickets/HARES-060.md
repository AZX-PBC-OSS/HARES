---
id: HARES-060
title: "ResStock Python Data Fetchers"
kind: implement
depends_on: [HARES-050]
files_to_touch:
  - python/ochre_next/data/__init__.py
  - python/ochre_next/data/resstock.py
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - uv run pytest tests/python/ -v -k "resstock_data"
---

## Background/Context
ResStock building bundles are hosted on NREL's OEDI S3 bucket. Python convenience loaders fetch HPXML + schedule + weather bundles by building ID or fleet filter, with local caching. This is the primary workflow for running ResStock studies and must support AMY weather overrides for historical event analysis.

**Design notes vs. architecture example:**
- The architecture example uses `local_dir` for the cache parameter; this ticket uses `cache_dir` as a more descriptive name. Either is acceptable — if the architecture example is updated to `cache_dir`, no shim is needed; if kept as `local_dir`, add an alias `local_dir = cache_dir` for compatibility.
- The architecture example uses separate `year` + `release` parameters; this ticket consolidates them into a single `version: str` (e.g., `"2024.2"`). This is an intentional simplification — the version string directly maps to `ResStockVersion` enum variants (`"2024.1"` → `V2024_1`, `"2024.2"` → `V2024_2`, `"2025.1"` → `V2025_1`) and avoids ambiguity when release identifiers are not purely numeric.

## Work to Do
- [ ] Define `ResStockBuilding` dataclass in `python/ochre_next/data/resstock.py`:
  ```python
  @dataclasses.dataclass(frozen=True)
  class ResStockBuilding:
      bldg_id: int
      sample_weight: float
      hpxml_path: Path
      schedule_path: Path
      weather_path: Path
  ```
- [ ] Implement `python/ochre_next/data/resstock.py`:
  - [ ] `fetch_resstock_building(bldg_id: int, version: str = "2024.2", upgrade_id: int = 0, cache_dir: Path | None = None, weather_override: Path | None = None) -> ResStockBuilding`
    - [ ] `upgrade_id`: ResStock distinguishes baseline (0) from upgrade scenarios (1, 2, …); pass through to the S3 path
    - [ ] Returns a `ResStockBuilding` dataclass (not a plain dict) with typed fields: `bldg_id: int`, `sample_weight: float`, `hpxml_path: Path`, `schedule_path: Path`, `weather_path: Path`
    - [ ] Downloads from NREL OEDI S3 via `httpx` (or `boto3` if available)
    - [ ] Caches to `~/.cache/ochre_next/resstock/{version}/bldg{id}/` by default
    - [ ] Skips download if cached files exist and are non-empty
    - [ ] `weather_override`: when provided, uses this EPW instead of the ResStock TMY weather
  - [ ] `fetch_resstock_fleet(metadata_path: Path, bldg_ids: list[int] | None = None, n_buildings: int | None = None, version: str = "2024.2", cache_dir: Path | None = None, filter: dict | None = None) -> list[ResStockBuilding]`
    - [ ] Fetches bundles for multiple buildings
    - [ ] `bldg_ids`: explicit list; `n_buildings`: random sample from metadata
    - [ ] `filter`: optional dict of metadata column filters to scope the fleet (e.g., `{"in.state": "CO", "in.hvac_heating_type": "Electricity"}`) — ResStock metadata Parquet files use `in.*` prefixed column names; applied before `bldg_ids` or `n_buildings` selection
    - [ ] Returns list of `ResStockBuilding` dataclass instances (same schema as `fetch_resstock_building`)
    - [ ] Parallel downloads via `asyncio` + `httpx.AsyncClient`
  - [ ] Both functions are pure Python — no Rust dependency
  - [ ] `httpx` and `boto3` are optional dependencies: fallback to `urllib.request` if neither available

## Measures of Success
- [ ] `fetch_resstock_building(1)` with a mock S3 endpoint returns a `ResStockBuilding` with valid local paths
- [ ] Second call to same building ID reads from cache (no HTTP request)
- [ ] `weather_override` substitutes the weather path in the returned `ResStockBuilding`
- [ ] `fetch_resstock_fleet` with `n_buildings=3` returns 3 `ResStockBuilding` instances
- [ ] `fetch_resstock_fleet` with `filter={"in.state": "CO"}` only returns buildings matching the filter
- [ ] Works without `boto3` installed (falls back to `httpx` or `urllib`)

## Verification
- [ ] `uv run pytest tests/python/ -v -k "resstock_data"` passes
