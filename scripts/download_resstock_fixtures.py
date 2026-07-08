#!/usr/bin/env python3
"""Download representative ResStock HPXML + schedule + weather fixtures for
integration testing. Stores them in tests/fixtures/resstock/{version}/.

Usage:
  uv run python scripts/download_resstock_fixtures.py --bldg-ids 2,3,4 [--versions 2024.2,2025.1]
  uv run python scripts/download_resstock_fixtures.py --stratified [--target 25] [--versions 2024.2,2025.1]
  uv run python scripts/download_resstock_fixtures.py --regenerate-manifest

Each building downloads: home.xml, in.schedules.csv, and weather file.
Output layout:
  tests/fixtures/resstock/
    2024.2/
      bldg0000002/
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

SHA256 checksums are maintained in manifest.sha256 at the fixture root.
Before committing a downloaded file, its SHA256 is compared against the
manifest.  Mismatches are rejected to prevent corrupted fixtures from
entering version control.  ``--regenerate-manifest`` overwrites the
manifest with fresh hashes computed from all committed fixture files.
"""

from __future__ import annotations

import argparse
import hashlib
import logging
import shutil
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

from ochre_next.data import fetch_resstock_building
from ochre_next.data.resstock import _backoff_delay, _is_transient_error

log = logging.getLogger("download_resstock_fixtures")

_MANIFEST_FILENAME = "manifest.sha256"

_METADATA_COLUMN_CANDIDATES: dict[str, list[str]] = {
    "climate_zone": [
        "in.ashrae_iecc_climate_zone_2004",
        "in.building_america_climate_zone",
        "in.iecc_climate_zone", "in.climate_zone", "in.iecc_zone",
        "iecc_climate_zone", "climate_zone",
    ],
    "building_type": [
        "in.geometry_building_type", "in.building_type",
        "geometry_building_type", "building_type",
        "in.geometry_building_type_recs", "in.geometry_building_type_acs",
    ],
    "heating_fuel": [
        "in.heating_fuel", "in.heating_type",
        "heating_fuel", "heating_type",
    ],
    "vintage": [
        "in.vintage", "vintage",
    ],
    "pv": [
        "in.has_pv", "in.pv", "in.pv_system",
        "has_pv", "pv", "pv_system",
    ],
    "ev": [
        "in.electric_vehicle_ownership",
        "in.electric_vehicle",
        "in.has_ev", "in.ev", "in.ev_charger",
        "has_ev", "ev", "ev_charger",
    ],
    "water_heater": [
        "in.water_heater_type", "in.water_heater_fuel",
        "water_heater_type", "water_heater_fuel",
        "in.hot_water_fuel", "hot_water_fuel",
    ],
}


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
                delay = _backoff_delay(attempt)
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


def _compute_sha256(file_path: Path) -> str:
    """Compute the SHA256 hex digest of *file_path*."""
    sha = hashlib.sha256()
    with file_path.open("rb") as f:
        while chunk := f.read(65536):
            sha.update(chunk)
    return sha.hexdigest()


def _load_manifest(manifest_path: Path) -> dict[str, str]:
    """Load a sha256sum-format manifest, returning ``{relative_path: hash}``."""
    entries: dict[str, str] = {}
    if not manifest_path.exists():
        return entries
    for line in manifest_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("  ", 1)
        if len(parts) == 2:
            entries[parts[1]] = parts[0]
    return entries


def _write_manifest(manifest_path: Path, entries: dict[str, str]) -> None:
    """Write *entries* to *manifest_path* in sha256sum format (sorted by path)."""
    lines = [f"{h}  {p}" for p, h in sorted(entries.items())]
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text("\n".join(lines) + "\n")


def _regenerate_manifest(fixtures_root: Path) -> dict[str, str]:
    """Compute SHA256 hashes for every fixture file produced by this script.

    Only includes files matching the output layout: {version}/{bldg_name}/home.xml,
    {version}/{bldg_name}/in.schedules.csv, and {version}/weather/*.
    """
    entries: dict[str, str] = {}
    for pattern in ("*/*/home.xml", "*/*/in.schedules.csv", "*/weather/*"):
        for file_path in sorted(fixtures_root.glob(pattern)):
            rel = file_path.relative_to(fixtures_root).as_posix()
            entries[rel] = _compute_sha256(file_path)
    return entries


def _verify_file_against_manifest(
    source: Path,
    rel_path: str,
    manifest: dict[str, str],
    mismatch_entries: list[tuple[str, str, str]],
) -> bool:
    """Verify *source* SHA256 against the manifest entry for *rel_path*.

    Returns ``True`` if the file can be committed (hash matches or new entry).
    On mismatch appends ``(rel_path, expected, actual)`` to *mismatch_entries*
    and returns ``False``.  A new entry is added to *manifest* in-place.
    """
    actual = _compute_sha256(source)

    if rel_path in manifest:
        expected = manifest[rel_path]
        if actual != expected:
            log.error(
                "SHA256 MISMATCH for %s: expected=%s actual=%s",
                rel_path, expected, actual,
            )
            mismatch_entries.append((rel_path, expected, actual))
            return False
        log.debug("SHA256 match: %s", rel_path)
        return True

    manifest[rel_path] = actual
    log.debug("New manifest entry: %s", rel_path)
    return True


def _fetch_metadata_parquet(version: str, tmp_dir: Path) -> Path:
    """Download the ResStock metadata parquet for *version* to *tmp_dir*.

    Returns the path to the downloaded parquet file, or raises if the
    download fails.
    """
    from ochre_next.data.resstock import _download_file, _metadata_url, _version_config

    cfg = _version_config(version)
    url = _metadata_url(cfg, upgrade_id=0)
    dest = tmp_dir / f"metadata_{version.replace('.', '_')}.parquet"
    if not dest.exists() or dest.stat().st_size == 0:
        log.info("  Downloading metadata for %s from %s", version, url)
        dest.parent.mkdir(parents=True, exist_ok=True)
        _download_file(url, dest)
    else:
        log.info("  Metadata for %s already cached at %s", version, dest)
    return dest


def _discover_columns(df_columns: list[str]) -> dict[str, str | None]:
    """Map stratification dimension keys to available DataFrame column names.

    Returns a dict mapping each dimension name to the first matching column
    name found in *df_columns*, or ``None`` if no match.
    """
    cols = set(df_columns)
    mapping: dict[str, str | None] = {}
    for dim, candidates in _METADATA_COLUMN_CANDIDATES.items():
        found = next((c for c in candidates if c in cols), None)
        mapping[dim] = found
    return mapping


def _select_stratified_ids(
    df,  # polars DataFrame
    col_map: dict[str, str | None],
    target: int,
    version: str,
) -> tuple[list[int], dict[int, dict[str, str]]]:
    """Select building IDs providing stratified coverage of key dimensions.

    Returns ``(selected_ids, metadata_dict)`` where *selected_ids* is a
    sorted list of building IDs and *metadata_dict* maps each building ID
    to a dict of dimension → value for that building.
    """
    import polars as pl

    selected: set[int] = set()
    metadata: dict[int, dict[str, str]] = {}

    def _id_col() -> str:
        for candidate in ("bldg_id", "building_id", "Building"):
            if candidate in df.columns:
                return candidate
        raise ValueError(f"Cannot find building ID column in: {df.columns}")

    id_col = _id_col()

    def _add_ids(ids: list[int], dim_values: dict[str, str]) -> None:
        for bid in ids:
            if bid not in selected:
                selected.add(bid)
                metadata[bid] = {"bldg_id": str(bid), **dim_values}

    def _pick_one(matches: pl.DataFrame, dim_values: dict[str, str]) -> None:
        if len(matches) == 0:
            return
        # Prefer an unselected building ID when one is available.
        unselected = matches.filter(~pl.col(id_col).is_in(list(selected)))
        source = unselected if len(unselected) > 0 else matches
        row = source.row(0, named=True)  # type: ignore[arg-type]
        bid = int(row[id_col])
        _add_ids([bid], dim_values)

    def _pick_n(matches: pl.DataFrame, dim_values: dict[str, str], n: int) -> None:
        if len(matches) == 0:
            return
        unselected = matches.filter(~pl.col(id_col).is_in(list(selected)))
        source = unselected if len(unselected) >= n else matches
        sample = source.sample(n=min(n, len(source)), shuffle=True, seed=42)
        for bid in sample[id_col].to_list():
            _add_ids([int(bid)], dim_values)

    # --- Climate zone groups (1-2, 3-4, 5-6, 7-8) ---
    if climate_col := col_map.get("climate_zone"):
        try:
            df = df.with_columns(
                pl.col(climate_col).cast(pl.Utf8).str.strip_chars().alias("_zone_str")
            )
            groups = {
                "climate_1_2": range(1, 3),
                "climate_3_4": range(3, 5),
                "climate_5_6": range(5, 7),
                "climate_7_8": range(7, 9),
            }
            for group_name, digits in groups.items():
                zone_col = pl.col("_zone_str")
                patterns = "|".join(f"^{d}" for d in digits)
                matches = df.filter(zone_col.str.contains(patterns))
                _pick_one(matches, {"dimension": "climate_zone_group", "value": group_name})
        except Exception:
            log.debug("Climate zone stratification unavailable — skipping", exc_info=True)

    # --- Building types ---
    if btype_col := col_map.get("building_type"):
        desired = [
            ("single-family detached", "single-family detached"),
            ("single-family attached", "single-family attached"),
            ("apartment", "apartment"),
            ("manufactured", "manufactured"),
        ]
        for label, pattern in desired:
            matches = df.filter(
                pl.col(btype_col).cast(pl.Utf8).str.to_lowercase().str.contains(pattern)
            )
            _pick_one(matches, {"dimension": "building_type", "value": label})

    # --- Heating fuel types ---
    if fuel_col := col_map.get("heating_fuel"):
        for fuel in ("natural gas", "electricity", "propane", "fuel oil"):
            matches = df.filter(
                pl.col(fuel_col).cast(pl.Utf8).str.to_lowercase() == fuel
            )
            _pick_one(matches, {"dimension": "heating_fuel", "value": fuel})

    # --- PV systems ---
    if pv_col := col_map.get("pv"):
        matches = df.filter(
            pl.col(pv_col).cast(pl.Utf8).str.to_lowercase().is_in(
                ["1", "true", "yes", "pv", "present", "solar"]
            )
        )
        _pick_n(matches, {"dimension": "pv", "value": "present"}, 2)

    # --- EV chargers ---
    if ev_col := col_map.get("ev"):
        matches = df.filter(
            pl.col(ev_col).cast(pl.Utf8).str.to_lowercase().is_in(
                ["1", "true", "yes", "ev", "present", "electric vehicle"]
            )
        )
        _pick_n(matches, {"dimension": "ev", "value": "present"}, 2)

    # --- Heat pump water heater ---
    if wh_col := col_map.get("water_heater"):
        matches = df.filter(
            pl.col(wh_col).cast(pl.Utf8).str.to_lowercase().str.contains("heat pump")
        )
        _pick_one(matches, {"dimension": "water_heater", "value": "heat pump"})

    # --- Vintage: pre-1980 and 2020+ ---
    if vintage_col := col_map.get("vintage"):
        pre_1980 = df.filter(
            pl.col(vintage_col).cast(pl.Utf8).str.contains(
                r"(?i)(pre[-\s]?19|19[0-7]\d|<[-\s]?194)"
            )
        )
        _pick_one(pre_1980, {"dimension": "vintage", "value": "pre-1980"})

        post_2020 = df.filter(
            pl.col(vintage_col).cast(pl.Utf8).str.contains(r"(?i)(20[2-9]\d|2020\+)")
        )
        _pick_one(post_2020, {"dimension": "vintage", "value": "2020+"})

    # --- Fill remaining slots via random sampling ---
    remaining = target - len(selected)
    if remaining > 0:
        available = df.filter(~pl.col(id_col).is_in(list(selected)))
        if len(available) > 0:
            filler = available.sample(n=min(remaining, len(available)), shuffle=True, seed=7)
            for bid in filler[id_col].to_list():
                _add_ids([int(bid)], {"dimension": "random_fill", "value": ""})

    return sorted(selected), metadata


def _verify_hpxml_equipment(hpxml_path: Path, dimensions: dict[str, str]) -> list[str]:
    """Verify that the HPXML at *hpxml_path* contains expected equipment.

    Returns a list of OK/warning/error strings describing results for each
    checked dimension.
    """
    results: list[str] = []
    try:
        tree = ET.parse(hpxml_path)  # noqa: S314
        root = tree.getroot()
    except ET.ParseError:
        return [f"Could not parse HPXML at {hpxml_path}"]

    ns = {"h": "http://hpxmlonline.com/2023/09"}

    # Check PV
    if dimensions.get("dimension") == "pv" and dimensions.get("value") == "present":
        pv_systems = root.findall(".//h:PVSystem", ns)
        if not pv_systems:
            # Try namespace-free
            pv_systems = root.findall(".//PVSystem")
        if pv_systems:
            results.append(f"PV: found {len(pv_systems)} PVSystem element(s)")
        else:
            results.append("PV: NOT FOUND — building selected for PV has no <PVSystem>")

    # Check EV
    if dimensions.get("dimension") == "ev" and dimensions.get("value") == "present":
        ev_elements = (
            root.findall(".//h:Vehicle", ns)
            or root.findall(".//Vehicle")
        )
        if ev_elements:
            results.append(f"EV: found {len(ev_elements)} Vehicle element(s)")
        else:
            results.append("EV: NOT FOUND — building selected for EV has no <Vehicle>")

    # Check heat pump water heater
    if dimensions.get("dimension") == "water_heater" and dimensions.get("value") == "heat pump":
        wh_elements = root.findall(".//h:WaterHeatingSystem", ns) or root.findall(".//WaterHeatingSystem")
        found_hpwh = False
        for wh in wh_elements:
            wtype = wh.find("h:WaterHeaterType", ns)
            if wtype is None:
                wtype = wh.find("WaterHeaterType")
            if wtype is not None and wtype.text and "heat pump" in wtype.text.lower():
                found_hpwh = True
                break
        if found_hpwh:
            results.append("HPWH: found heat pump water heater in HPXML")
        else:
            results.append("HPWH: NOT FOUND — building selected for heat pump WH has none")

    # Check heating fuel — search both HeatingSystem and HeatPump elements,
    # since electric heating in ResStock HPXML is represented as a HeatPump.
    # HeatingSystem uses <HeatingSystemFuel>; HeatPump uses <HeatPumpFuel>.
    if dimensions.get("dimension") == "heating_fuel":
        expected_fuel = dimensions.get("value", "")
        heating_systems = (
            root.findall(".//h:HeatingSystem", ns)
            or root.findall(".//HeatingSystem")
        )
        heat_pumps = (
            root.findall(".//h:HeatPump", ns)
            or root.findall(".//HeatPump")
        )
        fuel_tags = ("h:HeatingSystemFuel", "HeatingSystemFuel",
                     "h:HeatPumpFuel", "HeatPumpFuel")
        found = False
        for system in (*heating_systems, *heat_pumps):
            for tag in fuel_tags:
                ftype = system.find(tag, ns) if tag.startswith("h:") else system.find(tag)
                if ftype is not None and ftype.text and expected_fuel in ftype.text.lower():
                    found = True
                    break
            if found:
                break
        if found:
            results.append(f"heating_fuel: found '{expected_fuel}' in HPXML")
        else:
            results.append(f"heating_fuel: NOT FOUND — expected '{expected_fuel}'")

    return results if results else [f"No equipment checks applicable for {dimensions}"]


def _build_summary(
    total_attempted: int,
    succeeded: list[str],
    failed: list[dict[str, str]],
    total_retries: int,
    mismatch_count: int,
) -> str:
    """Build a human-readable summary of per-building download results."""
    lines = [f"  Attempted: {total_attempted}"]
    lines.append(f"  Succeeded: {len(succeeded)} ({', '.join(succeeded) if succeeded else 'none'})")
    if failed:
        lines.append(f"  Failed: {len(failed)}")
        for f in failed:
            lines.append(f"    {f['bldg_name']} ({f['version']}): {f['reason']}")
    else:
        lines.append("  Failed: 0")
    if total_retries:
        lines.append(f"  Retries: {total_retries}")
    if mismatch_count:
        lines.append(f"  SHA256 mismatches: {mismatch_count}")
    return "\n".join(lines)


def _should_exit_with_error(failed_count: int, total_attempted: int) -> bool:
    """Return True if >50% of attempted downloads failed."""
    if total_attempted == 0:
        return False
    return failed_count > total_attempted * 0.5


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(message)s")

    parser = argparse.ArgumentParser(description="Download ResStock test fixtures")
    parser.add_argument(
        "--bldg-ids",
        type=str,
        default=None,
        help="Comma-separated building IDs to download (required unless --stratified or --regenerate-manifest)",
    )
    parser.add_argument(
        "--versions",
        type=str,
        default="2024.2,2025.1",
        help="Comma-separated ResStock versions (default: 2024.2,2025.1)",
    )
    parser.add_argument(
        "--regenerate-manifest",
        action="store_true",
        help="Recompute all SHA256 hashes from the fixture files and overwrite manifest.sha256",
    )
    parser.add_argument(
        "--stratified",
        action="store_true",
        help="Use metadata-driven stratified sampling instead of explicit --bldg-ids",
    )
    parser.add_argument(
        "--target",
        type=int,
        default=25,
        help="Target total building count for --stratified mode (default: 25, distributed across versions)",
    )
    args = parser.parse_args()

    if not args.stratified and not args.regenerate_manifest and args.bldg_ids is None:
        parser.error(
            "one of --bldg-ids or --stratified is required to download buildings "
            "(or use --regenerate-manifest to rebuild the manifest)"
        )

    if args.stratified and args.bldg_ids is not None:
        log.warning(
            "--bldg-ids is ignored when --stratified is used; "
            "building IDs are selected from metadata"
        )

    versions = [v.strip() for v in args.versions.split(",")]
    stratified_version_failures: list[str] = []

    repo_root = Path(__file__).resolve().parent.parent
    fixtures_root = repo_root / "tests" / "fixtures" / "resstock"
    fixtures_root.mkdir(parents=True, exist_ok=True)

    regenerate = args.regenerate_manifest
    manifest_path = fixtures_root / _MANIFEST_FILENAME
    manifest: dict[str, str] = {} if regenerate else _load_manifest(manifest_path)
    manifest_changed = False
    mismatch_entries: list[tuple[str, str, str]] = []

    total_retries = 0
    succeeded: list[str] = []
    failed: list[dict[str, str]] = []

    # --- Decide building IDs per version ---
    # (version → list of (bldg_id, optional metadata dict))
    version_buildings: dict[str, list[tuple[int, dict[str, str] | None]]] = {}

    if args.stratified:
        import polars as pl

        target_per_version = max(1, args.target // len(versions))
        remaining_target = args.target

        with tempfile.TemporaryDirectory() as tmp_dir_str:
            tmp_dir = Path(tmp_dir_str)
            for version in versions:
                try:
                    meta_path = _fetch_metadata_parquet(version, tmp_dir)
                    df = pl.read_parquet(meta_path)
                    log.info("  Metadata for %s: %d buildings, %d columns",
                             version, len(df), len(df.columns))
                    log.debug("  Available columns: %s", sorted(df.columns))

                    col_map = _discover_columns(df.columns)
                    available_dims = [d for d, c in col_map.items() if c is not None]
                    missing_dims = [d for d, c in col_map.items() if c is None]
                    if available_dims:
                        log.info("  Dimensions available: %s", ", ".join(available_dims))
                    if missing_dims:
                        log.info("  Dimensions not found in metadata: %s", ", ".join(missing_dims))

                    ver_target = min(target_per_version, remaining_target)
                    selected_ids, metadata = _select_stratified_ids(
                        df, col_map, ver_target, version,
                    )

                    # --- Log the selection table ---
                    log.info("  Selected %d buildings for %s:", len(selected_ids), version)
                    for bid in selected_ids:
                        meta = metadata.get(bid, {})
                        dim = meta.get("dimension", "")
                        val = meta.get("value", "")
                        if dim == "random_fill":
                            log.info("    bldg %s — random fill", bid)
                        else:
                            log.info("    bldg %s — %s: %s", bid, dim, val)

                    version_buildings[version] = [(bid, metadata.get(bid))
                                                  for bid in selected_ids]
                    remaining_target -= len(selected_ids)
                except Exception:
                    log.error("  Failed to select stratified buildings for %s", version,
                              exc_info=True)
                    version_buildings[version] = []
                    stratified_version_failures.append(version)
    else:
        if args.bldg_ids is None:
            bldg_ids = []
        else:
            bldg_ids = [int(x.strip()) for x in args.bldg_ids.split(",")]
            log.info("Building IDs: %s", bldg_ids)
        for version in versions:
            version_buildings[version] = [(bid, None) for bid in bldg_ids]

    # --- Download buildings ---
    for version in versions:
        version_dir = fixtures_root / version
        version_dir.mkdir(parents=True, exist_ok=True)
        weather_dir = version_dir / "weather"
        weather_dir.mkdir(parents=True, exist_ok=True)

        for bldg_id, meta in version_buildings.get(version, []):
            bldg_name = f"bldg{bldg_id:07d}"
            dest_dir = version_dir / bldg_name

            if dest_dir.exists():
                log.info("  [%s] %s already cached, skipping", version, bldg_name)
                continue

            log.info("  [%s] %s downloading...", version, bldg_name)
            bldg, retries = _retry_fetch_resstock_building(bldg_id, version, bldg_name)
            total_retries += retries
            if bldg is None:
                failed.append({"bldg_name": bldg_name, "version": version, "reason": "download failed"})
                continue

            if not bldg.schedule_path.exists():
                log.warning(
                    "  [%s] %s skipped — %s missing from ResStock data bundle",
                    version, bldg_name, bldg.schedule_path.name,
                )
                failed.append({"bldg_name": bldg_name, "version": version, "reason": f"schedule file '{bldg.schedule_path.name}' missing"})
                continue

            # Verify source files against manifest before committing
            hpxml_rel = f"{version}/{bldg_name}/home.xml"
            sched_rel = f"{version}/{bldg_name}/in.schedules.csv"
            weather_src = Path(bldg.weather_path)
            weather_rel = f"{version}/weather/{weather_src.name}" if weather_src.name else ""

            verification_ok = True
            if not regenerate:
                for src, rel in [(bldg.hpxml_path, hpxml_rel),
                                  (bldg.schedule_path, sched_rel)]:
                    is_new = rel not in manifest
                    if not _verify_file_against_manifest(src, rel, manifest, mismatch_entries):
                        verification_ok = False
                    elif is_new:
                        manifest_changed = True
                if weather_rel:
                    is_new = weather_rel not in manifest
                    if not _verify_file_against_manifest(
                        weather_src, weather_rel, manifest, mismatch_entries,
                    ):
                        verification_ok = False
                    elif is_new:
                        manifest_changed = True
            else:
                # --regenerate-manifest: record all entries in-memory so they
                # are included in the fresh manifest written at shutdown.
                for src, rel in [(bldg.hpxml_path, hpxml_rel),
                                  (bldg.schedule_path, sched_rel)]:
                    manifest.setdefault(rel, _compute_sha256(src))
                    manifest_changed = True
                if weather_rel:
                    manifest.setdefault(weather_rel, _compute_sha256(weather_src))
                    manifest_changed = True

            if not verification_ok:
                log.error(
                    "  [%s] %s SKIPPED — SHA256 verification failed", version, bldg_name,
                )
                failed.append({"bldg_name": bldg_name, "version": version, "reason": "SHA256 verification failed"})
                continue

            dest_dir.mkdir(parents=True, exist_ok=True)

            # Copy home.xml and schedule
            shutil.copy2(bldg.hpxml_path, dest_dir / "home.xml")
            shutil.copy2(bldg.schedule_path, dest_dir / "in.schedules.csv")

            # Copy weather file into version/weather/
            weather_dest = weather_dir / weather_src.name
            if not weather_dest.exists() and weather_src.name:
                shutil.copy2(weather_src, weather_dest)

            log.info("    -> %s (weather: %s)", dest_dir.relative_to(repo_root), weather_src.name)

            # --- Verify downloaded HPXML against expected equipment ---
            if meta is not None:
                dest_hpxml = dest_dir / "home.xml"
                if dest_hpxml.exists():
                    verify_results = _verify_hpxml_equipment(dest_hpxml, meta)
                    for vr in verify_results:
                        if "NOT FOUND" in vr:
                            log.warning("    %s", vr)
                        else:
                            log.info("    %s", vr)

            succeeded.append(bldg_name)

    if regenerate:
        manifest = _regenerate_manifest(fixtures_root)
        _write_manifest(manifest_path, manifest)
        log.info("Regenerated manifest (%d entries)", len(manifest))
    elif manifest_changed:
        _write_manifest(manifest_path, manifest)
        log.info("Updated manifest (%d entries)", len(manifest))

    mismatch_count = len(mismatch_entries)
    total_attempted = len(succeeded) + len(failed)
    summary = _build_summary(total_attempted, succeeded, failed, total_retries, mismatch_count)
    log_fn = log.error if failed else log.info
    log_fn("Download summary:\n%s", summary)
    log.info("Done. Fixtures at: %s", fixtures_root.relative_to(repo_root))

    if args.stratified and stratified_version_failures:
        log.error(
            "Stratified mode: %d version(s) produced zero buildings: %s",
            len(stratified_version_failures),
            ", ".join(stratified_version_failures),
        )
        sys.exit(1)

    if _should_exit_with_error(len(failed), total_attempted):
        log.error(
            "Failure rate %.0f%% exceeds 50%% threshold — exiting with code 1",
            len(failed) / total_attempted * 100,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
