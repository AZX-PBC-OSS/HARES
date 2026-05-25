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
import shutil
from pathlib import Path

from ochre_next.data import fetch_resstock_building

log = logging.getLogger("download_resstock_fixtures")


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
            try:
                bldg = fetch_resstock_building(
                    bldg_id,
                    version=version,
                    upgrade_id=0,
                    cache_dir=None,  # use default ~/.cache/ochre_next
                )
            except Exception as e:
                log.warning("  [%s] %s FAILED: %s", version, bldg_name, e)
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

    log.info("Done. Fixtures at: %s", fixtures_root.relative_to(repo_root))


if __name__ == "__main__":
    main()
