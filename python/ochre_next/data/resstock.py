"""ResStock S3 data fetchers."""

from __future__ import annotations

import asyncio
import dataclasses
import enum
import io
import os
import tempfile
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import httpx
    import polars as pl

_OEDI_BASE = (
    "https://oedi-data-lake.s3.amazonaws.com/"
    "nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/"
)

_HPXML_NS = {
    "h": "http://hpxmlonline.com/2023/09",
    "h19": "http://hpxmlonline.com/2019/10",
}


class ResStockVersion(str, enum.Enum):
    V2024_1 = "2024.1"
    V2024_2 = "2024.2"
    V2025_1 = "2025.1"


class WeatherFormat(str, enum.Enum):
    """Weather file format for ResStock buildings."""

    EPW = "epw"
    """TMY3 EPW files from BuildStock_TMY3_FIPS.zip (full data, ~1200 stations)."""

    CSV = "csv"
    """Simplified 8-column CSV from OEDI S3 (dry bulb, RH, wind, solar only)."""


@dataclasses.dataclass(frozen=True)
class ResStockBuilding:
    bldg_id: int
    sample_weight: float
    hpxml_path: Path
    schedule_path: Path
    weather_path: Path


@dataclasses.dataclass(frozen=True)
class _VersionConfig:
    base_path: str
    zip_template: str
    metadata_baseline: str
    metadata_upgrade: str
    weather_template: str
    weather_suffix: str
    weather_format: WeatherFormat = WeatherFormat.EPW


_VERSION_CONFIGS: dict[ResStockVersion, _VersionConfig] = {
    ResStockVersion.V2024_1: _VersionConfig(
        base_path="2024/resstock_dataset_2024.1/resstock_tmy3/",
        zip_template="model_and_schedule_files/building_energy_models/upgrade={upgrade}/bldg{bldg_id:07d}-up{upgrade:02d}.zip",
        metadata_baseline="metadata_and_annual_results/national/parquet/baseline_metadata_and_annual_results.parquet",
        metadata_upgrade="metadata_and_annual_results/national/parquet/upgrade{upgrade:02d}_metadata_and_annual_results.parquet",
        weather_template="weather/state={state}/{fips}_TMY3.csv",
        weather_suffix="TMY3",
    ),
    ResStockVersion.V2024_2: _VersionConfig(
        base_path="2024/resstock_tmy3_release_2/",
        zip_template="model_and_schedule_files/building_energy_models/upgrade={upgrade}/bldg{bldg_id:07d}-up{upgrade:02d}.zip",
        metadata_baseline="metadata_and_annual_results/national/parquet/baseline_metadata_and_annual_results.parquet",
        metadata_upgrade="metadata_and_annual_results/national/parquet/upgrade{upgrade:02d}_metadata_and_annual_results.parquet",
        weather_template="weather/state={state}/{fips}_TMY3.csv",
        weather_suffix="TMY3",
    ),
    ResStockVersion.V2025_1: _VersionConfig(
        base_path="2025/resstock_amy2018_release_1/",
        zip_template="building_energy_models/upgrade={upgrade}/bldg{bldg_id:07d}-up{upgrade:02d}.zip",
        metadata_baseline="metadata_and_annual_results/national/full/parquet/upgrade0.parquet",
        metadata_upgrade="metadata_and_annual_results/national/full/parquet/upgrade{upgrade}.parquet",
        weather_template="weather/state={state}/{fips}_2018.csv",
        weather_suffix="2018",
        weather_format=WeatherFormat.CSV,  # No EPW source for AMY 2018; use ResStock CSV
    ),
}


def _parse_version(version: str) -> ResStockVersion:
    try:
        return ResStockVersion(version)
    except ValueError:
        valid = [v.value for v in ResStockVersion]
        raise ValueError(f"Unknown ResStock version {version!r}. Valid: {valid}") from None


def _version_config(version: str) -> _VersionConfig:
    return _VERSION_CONFIGS[_parse_version(version)]


def _default_cache_dir() -> Path:
    xdg = os.environ.get("XDG_CACHE_HOME")
    base = Path(xdg) if xdg else Path.home() / ".cache"
    return base / "ochre_next" / "resstock"


def _building_cache_dir(cache_dir: Path, version: str, bldg_id: int) -> Path:
    return cache_dir / version / f"bldg{bldg_id:07d}"


def _zip_url(cfg: _VersionConfig, bldg_id: int, upgrade_id: int) -> str:
    rel = cfg.zip_template.format(upgrade=upgrade_id, bldg_id=bldg_id)
    return _OEDI_BASE + cfg.base_path + rel


def _metadata_url(cfg: _VersionConfig, upgrade_id: int) -> str:
    if upgrade_id == 0:
        rel = cfg.metadata_baseline
    else:
        rel = cfg.metadata_upgrade.format(upgrade=upgrade_id)
    return _OEDI_BASE + cfg.base_path + rel


def _weather_url(cfg: _VersionConfig, state: str, fips: str) -> str:
    rel = cfg.weather_template.format(state=state, fips=fips)
    return _OEDI_BASE + cfg.base_path + rel


def _download_file(url: str, dest: Path) -> None:
    """Download url to dest using httpx, boto3, or urllib (in that order)."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".tmp")
    try:
        _try_download(url, tmp)
        tmp.replace(dest)
    except Exception:
        tmp.unlink(missing_ok=True)
        raise


def _try_download(url: str, dest: Path) -> None:
    try:
        import httpx  # type: ignore[import-not-found]

        with httpx.Client(follow_redirects=True) as client:
            with client.stream("GET", url) as resp:
                resp.raise_for_status()
                with dest.open("wb") as fh:
                    for chunk in resp.iter_bytes(chunk_size=65536):
                        fh.write(chunk)
        return
    except ImportError:
        pass

    try:
        import boto3  # type: ignore[import-not-found]
        from botocore import UNSIGNED  # type: ignore[import-not-found]
        from botocore.config import Config  # type: ignore[import-not-found]

        prefix = "https://oedi-data-lake.s3.amazonaws.com/"
        if url.startswith(prefix):
            key = url[len(prefix):]
        else:
            raise ValueError(f"Cannot parse S3 key from {url!r}")
        s3 = boto3.client("s3", config=Config(signature_version=UNSIGNED))
        s3.download_file("oedi-data-lake", key, str(dest))
        return
    except ImportError:
        pass

    with urllib.request.urlopen(url) as resp, dest.open("wb") as fh:  # noqa: S310
        while chunk := resp.read(65536):
            fh.write(chunk)


def _extract_zip(zip_path: Path, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path) as zf:
        for member in zf.namelist():
            # Sanitize member paths to prevent zip-slip (path traversal).
            safe_name = Path(member).name
            if not safe_name:
                continue
            target = dest / safe_name
            with zf.open(member) as src, target.open("wb") as dst:
                dst.write(src.read())


def _parse_weather_station(hpxml_path: Path) -> tuple[str, str] | None:
    """Return (state_abbr, fips) from HPXML WeatherStation/Name, or None."""
    try:
        tree = ET.parse(hpxml_path)  # noqa: S314
        root = tree.getroot()
    except ET.ParseError:
        return None

    # Try both known namespace versions
    for prefix, uri in _HPXML_NS.items():
        name_el = root.find(f".//{{{uri}}}WeatherStation/{{{uri}}}Name")
        if name_el is not None and name_el.text:
            return _fips_from_weather_name(name_el.text)

    # Namespace-free fallback
    name_el = root.find(".//WeatherStation/Name")
    if name_el is not None and name_el.text:
        return _fips_from_weather_name(name_el.text)

    return None


def _fips_from_weather_name(name: str) -> tuple[str, str] | None:
    """Extract (state, fips) from a weather station name like 'G0100290'.

    ResStock uses county FIPS codes as weather station names (e.g. 'G0100290').
    The state FIPS prefix maps to a 2-letter abbreviation via a lookup.
    Returns None if the name does not match expected patterns.
    """
    # ResStock weather station names are FIPS codes like 'G0100290'
    # State is the 3rd-4th chars of the FIPS (after 'G' prefix + 2-digit padding)
    # Pattern: G + state_fips(2) + county_fips(3) + suffix(2) e.g. G0100290
    clean = name.strip()
    if clean.startswith("G") and len(clean) >= 7:
        state_fips = clean[1:3]
        state_abbr = _STATE_FIPS.get(state_fips)
        if state_abbr:
            return state_abbr, clean
    return None


_STATE_FIPS: dict[str, str] = {
    "01": "AL", "02": "AK", "04": "AZ", "05": "AR", "06": "CA",
    "08": "CO", "09": "CT", "10": "DE", "11": "DC", "12": "FL",
    "13": "GA", "15": "HI", "16": "ID", "17": "IL", "18": "IN",
    "19": "IA", "20": "KS", "21": "KY", "22": "LA", "23": "ME",
    "24": "MD", "25": "MA", "26": "MI", "27": "MN", "28": "MS",
    "29": "MO", "30": "MT", "31": "NE", "32": "NV", "33": "NH",
    "34": "NJ", "35": "NM", "36": "NY", "37": "NC", "38": "ND",
    "39": "OH", "40": "OK", "41": "OR", "42": "PA", "44": "RI",
    "45": "SC", "46": "SD", "47": "TN", "48": "TX", "49": "UT",
    "50": "VT", "51": "VA", "53": "WA", "54": "WV", "55": "WI",
    "56": "WY",
}


def _fetch_weather(
    cfg: _VersionConfig,
    hpxml_path: Path,
    cache_dir: Path,
    version: str,
    weather_format: WeatherFormat | None = None,
) -> Path:
    """Fetch the weather file for a building.

    Parameters
    ----------
    weather_format:
        Override the version's default weather format.  ``WeatherFormat.EPW``
        forces TMY3 EPW files (from BuildStock_TMY3_FIPS.zip) even for AMY
        versions.  ``WeatherFormat.CSV`` forces the simplified S3 CSV.
        ``None`` uses the version's native format.
    """
    station = _parse_weather_station(hpxml_path)
    if station is None:
        return Path("")
    state, fips = station

    fmt = weather_format if weather_format is not None else cfg.weather_format

    if fmt is WeatherFormat.EPW:
        # TMY3 EPW from BuildStock_TMY3_FIPS.zip.
        # All TMY3 versions share the same EPW dataset, so use a
        # version-independent cache directory.
        from ochre_next.data.weather import get_epw_for_fips

        weather_cache = cache_dir / "weather"
        return get_epw_for_fips(fips, cache_dir=weather_cache)

    # CSV: download the simplified ResStock CSV from S3.
    weather_dest = cache_dir / version / "weather" / f"{fips}_{cfg.weather_suffix}.csv"
    if weather_dest.exists() and weather_dest.stat().st_size > 0:
        return weather_dest
    url = _weather_url(cfg, state, fips)
    _download_file(url, weather_dest)
    return weather_dest


def fetch_resstock_building(
    bldg_id: int,
    version: str = "2024.2",
    upgrade_id: int = 0,
    cache_dir: Path | None = None,
    weather_override: Path | None = None,
    weather_format: WeatherFormat | None = None,
) -> ResStockBuilding:
    """Download a ResStock building bundle from OEDI S3 and return local paths.

    Files are cached to *cache_dir* and re-used on subsequent calls.

    Parameters
    ----------
    weather_override:
        Explicit path to a weather file — skips all weather fetching.
    weather_format:
        ``WeatherFormat.EPW`` to force TMY3 EPW files (even for AMY
        versions like 2025.1), ``WeatherFormat.CSV`` to force the
        simplified S3 CSV, or ``None`` (default) to use the version's
        native format.
    """
    base_cache = cache_dir if cache_dir is not None else _default_cache_dir()
    cfg = _version_config(version)
    bldg_dir = _building_cache_dir(base_cache, version, bldg_id)
    hpxml_path = bldg_dir / "home.xml"
    schedule_path = bldg_dir / "in.schedules.csv"

    if not (hpxml_path.exists() and hpxml_path.stat().st_size > 0
            and schedule_path.exists() and schedule_path.stat().st_size > 0):
        url = _zip_url(cfg, bldg_id, upgrade_id)
        with tempfile.TemporaryDirectory() as tmp:
            zip_dest = Path(tmp) / "building.zip"
            _download_file(url, zip_dest)
            _extract_zip(zip_dest, bldg_dir)

    if weather_override is not None:
        weather_path = weather_override
    else:
        weather_path = _fetch_weather(
            cfg, hpxml_path, base_cache, version,
            weather_format=weather_format,
        )

    return ResStockBuilding(
        bldg_id=bldg_id,
        sample_weight=1.0,
        hpxml_path=hpxml_path,
        schedule_path=schedule_path,
        weather_path=weather_path,
    )


async def _noop() -> None:
    pass


async def _download_building_async(
    client: httpx.AsyncClient,
    cfg: _VersionConfig,
    bldg_id: int,
    upgrade_id: int,
    bldg_dir: Path,
) -> None:
    """Download and extract a single building ZIP using an async httpx client."""
    url = _zip_url(cfg, bldg_id, upgrade_id)
    bldg_dir.mkdir(parents=True, exist_ok=True)
    resp = await client.get(url)
    resp.raise_for_status()
    data = resp.content
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        zf.extractall(bldg_dir)


async def _fetch_fleet_async(
    bldg_ids: list[int],
    cfg: _VersionConfig,
    version: str,
    base_cache: Path,
    upgrade_id: int,
    weights: dict[int, float],
    weather_format: WeatherFormat | None = None,
) -> list[ResStockBuilding]:
    results: list[ResStockBuilding] = []

    try:
        import httpx  # type: ignore[import-not-found]

        async with httpx.AsyncClient(follow_redirects=True, timeout=120.0) as client:
            tasks = []
            for bid in bldg_ids:
                bldg_dir = _building_cache_dir(base_cache, version, bid)
                hpxml_path = bldg_dir / "home.xml"
                schedule_path = bldg_dir / "in.schedules.csv"
                if hpxml_path.exists() and hpxml_path.stat().st_size > 0 \
                        and schedule_path.exists() and schedule_path.stat().st_size > 0:
                    tasks.append(_noop())
                else:
                    tasks.append(_download_building_async(client, cfg, bid, upgrade_id, bldg_dir))

            await asyncio.gather(*tasks)

        for bid in bldg_ids:
            bldg_dir = _building_cache_dir(base_cache, version, bid)
            hpxml_path = bldg_dir / "home.xml"
            schedule_path = bldg_dir / "in.schedules.csv"
            weather_path = _fetch_weather(
                cfg, hpxml_path, base_cache, version,
                weather_format=weather_format,
            )
            results.append(ResStockBuilding(
                bldg_id=bid,
                sample_weight=weights.get(bid, 1.0),
                hpxml_path=hpxml_path,
                schedule_path=schedule_path,
                weather_path=weather_path,
            ))
        return results

    except ImportError:
        pass

    # Synchronous fallback when httpx is not available
    for bid in bldg_ids:
        b = fetch_resstock_building(
            bid, version=version, upgrade_id=upgrade_id,
            cache_dir=base_cache, weather_format=weather_format,
        )
        results.append(dataclasses.replace(b, sample_weight=weights.get(bid, 1.0)))
    return results


def fetch_resstock_fleet(
    metadata_path: Path,
    bldg_ids: list[int] | None = None,
    n_buildings: int | None = None,
    version: str = "2024.2",
    upgrade_id: int = 0,
    cache_dir: Path | None = None,
    filter: dict[str, object] | None = None,
    weather_format: WeatherFormat | None = None,
) -> list[ResStockBuilding]:
    """Fetch a fleet of ResStock buildings with metadata-driven sample weights.

    Parameters
    ----------
    metadata_path:
        Local path to a ResStock metadata parquet file.
    bldg_ids:
        Explicit list of building IDs to fetch.
    n_buildings:
        Random sample size (mutually exclusive with bldg_ids).
    version:
        ResStock dataset version string (e.g. "2024.2").
    upgrade_id:
        Upgrade scenario (0 = baseline).
    cache_dir:
        Override for the local cache root.
    filter:
        Column equality filters applied before building selection
        (e.g. {"in.state": "CO"}).
    weather_format:
        ``WeatherFormat.EPW`` to force TMY3 EPW weather for all
        buildings (useful for OCHRE parity with AMY versions),
        ``WeatherFormat.CSV`` to force S3 CSVs, or ``None`` to use
        the version's native format.
    """
    import polars as pl

    if bldg_ids is not None and n_buildings is not None:
        raise ValueError("Specify at most one of bldg_ids or n_buildings.")

    df = pl.read_parquet(metadata_path)

    if filter:
        for col, val in filter.items():
            df = df.filter(pl.col(col) == val)

    weight_col = next(
        (c for c in df.columns if "sample_weight" in c.lower()),
        None,
    )

    if bldg_ids is not None:
        id_col = _bldg_id_col(df)
        df = df.filter(pl.col(id_col).is_in(bldg_ids))
    elif n_buildings is not None:
        df = df.sample(n=min(n_buildings, len(df)), shuffle=True, seed=0)

    id_col = _bldg_id_col(df)
    selected_ids: list[int] = df[id_col].to_list()
    weights: dict[int, float] = {}
    if weight_col is not None:
        for bid, w in zip(df[id_col].to_list(), df[weight_col].to_list()):
            weights[int(bid)] = float(w)

    base_cache = cache_dir if cache_dir is not None else _default_cache_dir()
    cfg = _version_config(version)

    return asyncio.run(
        _fetch_fleet_async(
            [int(i) for i in selected_ids],
            cfg,
            version,
            base_cache,
            upgrade_id,
            weights,
            weather_format=weather_format,
        )
    )


def _bldg_id_col(df: pl.DataFrame) -> str:
    """Return the building ID column name from a polars DataFrame."""
    import polars as _pl

    for candidate in ("bldg_id", "building_id", "Building"):
        if candidate in df.columns:
            return candidate
    for col in df.columns:
        if df[col].dtype in (_pl.Int32, _pl.Int64, _pl.UInt32, _pl.UInt64):
            return col
    raise ValueError(f"Cannot find building ID column in: {df.columns}")
