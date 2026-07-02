#!/usr/bin/env python3
"""Download representative ResStock HPXML + schedule + weather fixtures for
integration testing. Stores them in tests/fixtures/resstock/{version}/.

Usage:
  uv run python scripts/download_resstock_fixtures.py [--bldg-ids 1,2,3] [--versions 2024.2,2025.1]

Each building downloads: home.xml, in.schedules.csv, and weather file.
Output layout:
  tests/fixtures/resstock/
    2024.2/
      bldg0000001/
        home.xml
        in.schedules.csv
      weather/
        G0800130_TMY3.csv (or shared EPW)
    2025.1/
      bldg0000007/
        home.xml
        in.schedules.csv
      weather/
        G0800130_2018.csv

This script can commit these files; the ZIP raw data is not stored.
"""

from __future__ import annotations

import argparse
import logging
import random
import shutil
import time
from pathlib import Path

from ochre_next.data import fetch_resstock_building

log = logging.getLogger("download_resstock_fixtures")

_TRANSIENT_HTTP_STATUSES: frozenset[int] = frozenset({408, 429, 500, 502, 503, 504})


def _is_transient_error(exc: BaseException) -> bool:
    """Return True if *exc* represents a network-level transient failure worth retrying."""
    found_transient = False
    current: BaseException | None = exc
    while current is not None:
        # Non-transient anywhere in the chain → do not retry
        if isinstance(current, (ValueError, TypeError, KeyError, AttributeError,
                                LookupError, ImportError, NotImplementedError)):
            return False
        # httpx-style HTTP status code via response object
        http_status = getattr(getattr(current, "response", None), "status_code", None)
        if isinstance(http_status, int) and http_status in _TRANSIENT_HTTP_STATUSES:
            found_transient = True
        # botocore-style ClientError carries response as a dict with HTTP status
        response_dict = getattr(current, "response", None)
        if isinstance(response_dict, dict):
            meta_http = response_dict.get("ResponseMetadata", {}).get("HTTPStatusCode")
            if isinstance(meta_http, int) and meta_http in _TRANSIENT_HTTP_STATUSES:
                found_transient = True
            error_code = response_dict.get("Error", {}).get("Code", "")
            if error_code in ("SlowDown", "InternalError", "ServiceUnavailable",
                              "RequestTimeout", "Throttling"):
                found_transient = True
        # Standard library network / timeout exceptions
        if isinstance(current, (TimeoutError, ConnectionError, OSError)):
            found_transient = True
        # httpx errors do not inherit from stdlib ConnectionError/TimeoutError
        cls_name = type(current).__qualname__
        if any(term in cls_name
               for term in ("Timeout", "Connect", "Network", "Read", "RemoteProtocol")):
            found_transient = True
        current = current.__cause__
    return found_transient


def _retry_fetch_resstock_building(
    bldg_id: int,
    version: str,
    bldg_name: str,
    max_attempts: int = 3,
) -> tuple[object, int]:  # (ResStockBuilding | None, retry_count)
    """Call fetch_resstock_building with exponential-backoff retries.

    Returns ``(building, retry_count)`` where *building* is ``None`` when
    all attempts are exhausted.  Each retry is logged at INFO level; the
    final failure is logged at ERROR.
    """
    retry_count = 0
    for attempt in range(max_attempts):
        try:
            bldg = fetch_resstock_building(
                bldg_id,
                version=version,
                upgrade_id=0,
                cache_dir=None,
            )
            return bldg, retry_count
        except Exception as exc:
            if not _is_transient_error(exc):
                log.error("  [%s] %s FAILED (non-transient): %s", version, bldg_name, exc)
                return None, retry_count
            if attempt < max_attempts - 1:
                retry_count += 1
                delay = 2**attempt + random.uniform(0, 1)
                log.info(
                    "  [%s] %s retry %d/%d (%.1fs delay): %s",
                    version, bldg_name, attempt + 1, max_attempts - 1, delay, exc,
                )
                time.sleep(delay)
            else:
                log.error(
                    "  [%s] %s FAILED after %d retries: %s",
                    version, bldg_name, retry_count, type(exc).__name__,
                )
                return None, retry_count


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")

    parser = argparse.ArgumentParser(description="Download ResStock test fixtures")
    parser.add_argument(
        "--bldg-ids",
        type=str,
        default="1,2,3",
        help="Comma-separated building IDs to download (default: 1,2,3)",
    )
    parser.add_argument(
        "--versions",
        type=str,
        default="2024.2,2025.1",
        help="Comma-separated ResStock versions (default: 2024.2,2025.1)",
    )
    args = parser.parse_args()

    bldg_ids = [int(x.strip()) for x in args.bldg_ids.split(",")]
    versions = [v.strip() for v in args.versions.split(",")]

    repo_root = Path(__file__).resolve().parent.parent
    fixtures_root = repo_root / "tests" / "fixtures" / "resstock"
    fixtures_root.mkdir(parents=True, exist_ok=True)

    total_retries = 0
    failed_count = 0

    for version in versions:
        version_dir = fixtures_root / version
        version_dir.mkdir(parents=True, exist_ok=True)
        weather_dir = version_dir / "weather"
        weather_dir.mkdir(parents=True, exist_ok=True)

        for bldg_id in bldg_ids:
            bldg_name = f"bldg{bldg_id:07d}"
            dest_dir = version_dir / bldg_name

            if dest_dir.exists():
                log.info("  [%s] %s already cached, skipping", version, bldg_name)
                continue

            log.info("  [%s] %s downloading...", version, bldg_name)
            bldg, retries = _retry_fetch_resstock_building(bldg_id, version, bldg_name)
            total_retries += retries
            if bldg is None:
                failed_count += 1
                continue

            dest_dir.mkdir(parents=True, exist_ok=True)

            # Copy home.xml and schedule
            shutil.copy2(bldg.hpxml_path, dest_dir / "home.xml")
            shutil.copy2(bldg.schedule_path, dest_dir / "in.schedules.csv")

            # Copy weather file into version/weather/
            weather_src = Path(bldg.weather_path)
            weather_dest = weather_dir / weather_src.name
            if not weather_dest.exists():
                shutil.copy2(weather_src, weather_dest)

            log.info("    -> %s (weather: %s)", dest_dir.relative_to(repo_root), weather_src.name)

    if total_retries:
        log.info("Total retries: %d", total_retries)
    if failed_count:
        log.warning("Failed downloads: %d", failed_count)
    log.info("Done. Fixtures at: %s", fixtures_root.relative_to(repo_root))


if __name__ == "__main__":
    main()
