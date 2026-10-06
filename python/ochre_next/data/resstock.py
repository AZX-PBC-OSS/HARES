"""ResStock S3 data fetchers."""

from __future__ import annotations

import atexit
from concurrent.futures import Future, ThreadPoolExecutor, wait
import contextvars
import dataclasses
import enum
import logging
import os
import random
import tempfile
import threading
import urllib.request
import warnings
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import TYPE_CHECKING, Any

from typing_extensions import TypeIs  # typing.TypeIs is Python 3.13+

from ochre_next.data._checksum import (
    remove_cache_with_sidecar as _remove_cache_with_sidecar,
    validate_cache_integrity as _validate_cache_integrity,
    write_sha256_sidecar as _write_sha256_sidecar,
)

if TYPE_CHECKING:
    import httpx

    from ochre_next.data._cuttable_transport import Connections
    import polars as pl

log = logging.getLogger(__name__)

_WEATHER_LOCK = threading.Lock()

# Buildings a fleet fetch downloads at once. Downloads are I/O bound, so
# threads overlap them; the bound keeps a large fleet from opening thousands
# of connections at the same time.
_FLEET_DOWNLOAD_WORKERS = 16

# How often a fleet fetch waiting on its downloads returns to the interpreter
# to act on a Ctrl-C (see _result_interruptibly).
_INTERRUPT_CHECK_S = 0.1

# Connect, read, write and pool timeout for every download, in seconds: long
# enough for a slow S3 stream to keep going between chunks, finite so a
# stalled connection fails and is retried instead of holding a worker.
_DOWNLOAD_TIMEOUT_S = 120.0

_OEDI_BASE = (
    "https://oedi-data-lake.s3.amazonaws.com/"
    "nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/"
)

_HPXML_NS = {
    "h": "http://hpxmlonline.com/2023/09",
    "h19": "http://hpxmlonline.com/2019/10",
}


def _is_mapping(value: object) -> TypeIs[dict[str, Any]]:
    """Narrow an exception attribute to the string-keyed mappings that
    botocore's ClientError.response and httpx use."""
    return isinstance(value, dict)


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
        raise ValueError(
            f"Unknown ResStock version {version!r}. Valid: {valid}"
        ) from None


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


def metadata_url(cfg: _VersionConfig, upgrade_id: int) -> str:
    if upgrade_id == 0:
        rel = cfg.metadata_baseline
    else:
        rel = cfg.metadata_upgrade.format(upgrade=upgrade_id)
    return _OEDI_BASE + cfg.base_path + rel


def _weather_url(cfg: _VersionConfig, state: str, fips: str) -> str:
    rel = cfg.weather_template.format(state=state, fips=fips)
    return _OEDI_BASE + cfg.base_path + rel


# --- Retry / backoff for OEDI S3 downloads ------------------------------------
#
# Network interruptions during OEDI S3 downloads are transient: a connection
# reset, a read timeout, or a 5xx/429 from the endpoint typically succeeds on a
# subsequent attempt.  Each download path therefore retries transient failures
# with exponential backoff before giving up, while permanent failures (a 404 for
# a missing building, a malformed URL) fail fast without wasting attempts.

# HTTP statuses worth retrying: request timeout, rate limiting, and the 5xx
# family the S3 fronting layer returns under load or during partial outages.
_TRANSIENT_HTTP_STATUSES: frozenset[int] = frozenset({408, 429, 500, 502, 503, 504})

# Number of attempts per download (1 initial try + 2 retries) before giving up.
_MAX_DOWNLOAD_ATTEMPTS: int = 3

# Exponential backoff base in seconds: attempt N waits base**N + jitter.
_RETRY_BACKOFF_BASE_S: float = 2.0


def _is_transient_error(exc: BaseException) -> bool:
    """Return True if *exc* represents a network-level transient failure worth retrying.

    Walks the ``__cause__`` chain so a transient error wrapped by another
    transient error is still detected.  A non-transient error anywhere in the
    chain (a programming/value error) short-circuits to ``False`` so genuine
    bugs are not retried into oblivion.
    """
    found_transient = False
    current: BaseException | None = exc
    while current is not None:
        # Non-transient anywhere in the chain -> do not retry.
        if isinstance(
            current,
            (
                ValueError,
                TypeError,
                KeyError,
                AttributeError,
                LookupError,
                ImportError,
                NotImplementedError,
            ),
        ):
            return False
        # httpx-style HTTP status code via a response object.
        http_status = getattr(getattr(current, "response", None), "status_code", None)
        if isinstance(http_status, int) and http_status in _TRANSIENT_HTTP_STATUSES:
            found_transient = True
        # botocore-style ClientError carries response as a dict with HTTP status.
        response_dict = getattr(current, "response", None)
        if _is_mapping(response_dict):
            meta_http = response_dict.get("ResponseMetadata", {}).get("HTTPStatusCode")
            if isinstance(meta_http, int) and meta_http in _TRANSIENT_HTTP_STATUSES:
                found_transient = True
            error_code = response_dict.get("Error", {}).get("Code", "")
            if error_code in (
                "SlowDown",
                "InternalError",
                "ServiceUnavailable",
                "RequestTimeout",
                "Throttling",
            ):
                found_transient = True
        # Standard-library network / timeout exceptions.  ``socket.error`` and
        # ``urllib.error.URLError`` are both subclasses of ``OSError``.
        if isinstance(current, (TimeoutError, ConnectionError, OSError)):
            found_transient = True
        # httpx errors (ConnectError, TimeoutException, ReadError, ...) do not
        # inherit from the stdlib network exceptions, so match on the class name.
        cls_name = type(current).__qualname__
        if any(
            term in cls_name
            for term in ("Timeout", "Connect", "Network", "Read", "RemoteProtocol")
        ):
            found_transient = True
        current = current.__cause__
    return found_transient


def _backoff_delay(attempt: int) -> float:
    """Return the backoff delay in seconds before retrying *attempt* (0-indexed).

    Exponential backoff with full jitter: ``base**attempt + U[0, 1)``.  The
    jitter decorrelates concurrent fleet retries so they do not stampede the S3
    endpoint in lockstep after a shared transient failure.
    """
    return _RETRY_BACKOFF_BASE_S**attempt + random.uniform(0.0, 1.0)


def _next_retry_delay(
    exc: Exception, attempt: int, max_attempts: int, url: str
) -> float:
    """Return the backoff delay before the next retry, or re-raise *exc*.

    Re-raises *exc* when it is non-transient (fail fast) or when the final
    attempt has already been made (retries exhausted), so callers can write a
    plain ``sleep(_next_retry_delay(...))`` retry loop that terminates by
    propagating the original error with its traceback intact.
    """
    if not _is_transient_error(exc):
        raise exc
    if attempt >= max_attempts - 1:
        log.error(
            "OEDI download failed after %d attempts: url=%s error=%s",
            max_attempts,
            url,
            exc,
        )
        raise exc
    delay = _backoff_delay(attempt)
    log.warning(
        "OEDI download attempt %d/%d failed, retrying in %.1fs: url=%s error=%s",
        attempt + 1,
        max_attempts,
        delay,
        url,
        exc,
    )
    return delay


# --- ZIP integrity verification -----------------------------------------------


class ZipIntegrityError(ValueError):
    """ZIP file failed integrity check — ``testzip()`` found a bad member."""


def _verify_zip(zip_path: Path) -> None:
    """Verify *zip_path* integrity via CRC check.

    Raises ``ZipIntegrityError`` if any member has a bad CRC or the file is not
    a valid ZIP.  ``testzip()`` returns the name of the first bad member, or
    ``None`` when all members pass.
    """
    try:
        with zipfile.ZipFile(zip_path) as zf:
            bad = zf.testzip()
            if bad is not None:
                raise ZipIntegrityError(
                    f"Corrupted ZIP {zip_path.name}: bad member {bad!r}"
                )
    except zipfile.BadZipFile as exc:
        raise ZipIntegrityError(f"Not a valid ZIP file {zip_path.name}") from exc


def _download_and_extract_zip(url: str, dest_dir: Path) -> None:
    """Download a building ZIP, verify CRC integrity, and extract.

    Retries the full download-and-verify cycle when the ZIP is corrupted (the
    CRC check is separate from the network-level retries inside
    ``_download_file``, which handle transient connection failures).
    """
    for attempt in range(_MAX_DOWNLOAD_ATTEMPTS):
        with tempfile.TemporaryDirectory() as tmp:
            zip_dest = Path(tmp) / "building.zip"
            _download_file(url, zip_dest)
            try:
                _verify_zip(zip_dest)
            except ZipIntegrityError as exc:
                if attempt < _MAX_DOWNLOAD_ATTEMPTS - 1:
                    delay = _backoff_delay(attempt)
                    log.warning(
                        "ZIP integrity check failed attempt %d/%d for %s, "
                        "retrying in %.1fs: %s",
                        attempt + 1,
                        _MAX_DOWNLOAD_ATTEMPTS,
                        url,
                        delay,
                        exc,
                    )
                    _wait_before_retry(delay)
                    continue
                raise
            _extract_zip(zip_dest, dest_dir)
            for member_name in ("home.xml", "in.schedules.csv"):
                member_path = dest_dir / member_name
                if member_path.exists():
                    _write_sha256_sidecar(member_path)
            return


def _download_file(
    url: str, dest: Path, *, max_attempts: int = _MAX_DOWNLOAD_ATTEMPTS
) -> None:
    """Download url to dest using httpx, boto3, or urllib (in that order).

    Transient failures are retried with exponential backoff.  The temporary
    ``.tmp`` file is removed only once all retries are exhausted, so a mid-stream
    failure does not force the caller to restart from a clean slate prematurely.
    """
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".tmp")
    try:
        _download_with_retry(url, tmp, max_attempts=max_attempts)
        tmp.replace(dest)
    except Exception:
        # Reached only after retries are exhausted (or a non-transient error);
        # discard the partial download so it never masquerades as a valid file.
        tmp.unlink(missing_ok=True)
        raise


def _download_with_retry(
    url: str, dest: Path, *, max_attempts: int = _MAX_DOWNLOAD_ATTEMPTS
) -> None:
    """Call ``_try_download`` with exponential-backoff retries on transient errors."""
    for attempt in range(max_attempts):
        _raise_if_cancelled()
        try:
            _try_download(url, dest)
            return
        except Exception as exc:
            _raise_if_cancelled()
            _wait_before_retry(_next_retry_delay(exc, attempt, max_attempts, url))


class _DownloadCancelled(Exception):
    """The fetch this download belongs to was interrupted."""


# The signal a download outside any fleet sees; nothing sets it.
_NEVER_CANCELLED = threading.Event()

# The cancellation signal of the fleet fetch a worker thread downloads for.
_cancellation: contextvars.ContextVar[threading.Event] = contextvars.ContextVar(
    "resstock_download_cancellation", default=_NEVER_CANCELLED
)


def _raise_if_cancelled() -> None:
    if _cancellation.get().is_set():
        raise _DownloadCancelled


def _wait_before_retry(delay_s: float) -> None:
    """Wait out a retry delay, ending it at once if the fetch is cancelled."""
    if _cancellation.get().wait(delay_s):
        raise _DownloadCancelled


class _SharedClient:
    """The httpx client every download shares, built once and abortable.

    One thread-safe client serves single-building and fleet fetches alike, so
    a fleet's concurrent downloads reuse connections instead of paying a TLS
    handshake each; it is closed at interpreter exit. Its transport records
    every socket it opens, so ``abort`` can end a request at any stage:
    connecting, in the TLS handshake, or reading.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._current: tuple[httpx.Client, Connections] | None = None

    def get(self) -> httpx.Client:
        with self._lock:
            if self._current is None:
                import httpx

                from ochre_next.data._cuttable_transport import Connections, CuttableTransport

                connections = Connections()
                client = httpx.Client(
                    follow_redirects=True,
                    timeout=_DOWNLOAD_TIMEOUT_S,
                    transport=CuttableTransport(connections),
                )
                atexit.register(client.close)
                self._current = (client, connections)
            return self._current[0]

    def abort(self) -> None:
        """Close the client and end every request in flight on it.

        Every download in the process that is using it fails; the next
        ``get`` builds a new client, so one that is not cancelled retries on
        that.
        """
        with self._lock:
            current, self._current = self._current, None
        if current is not None:
            client, connections = current
            connections.cut()
            client.close()


_shared_client = _SharedClient()


def _try_download(url: str, dest: Path) -> None:
    try:
        client = _shared_client.get()
    except ImportError:
        pass
    else:
        # An interrupt cancels the fleet before it aborts the client, so a
        # download that got the client built after the abort stops here; one
        # that got the aborted client has its connection cut as it opens.
        _raise_if_cancelled()
        with client.stream("GET", url) as resp:
            resp.raise_for_status()
            with dest.open("wb") as fh:
                for chunk in resp.iter_bytes(chunk_size=65536):
                    fh.write(chunk)
        return

    try:
        import boto3
        from botocore import UNSIGNED
        from botocore.config import Config

        prefix = "https://oedi-data-lake.s3.amazonaws.com/"
        if url.startswith(prefix):
            key = url[len(prefix) :]
        else:
            raise ValueError(f"Cannot parse S3 key from {url!r}")
        s3 = boto3.client("s3", config=Config(signature_version=UNSIGNED))
        s3.download_file("oedi-data-lake", key, str(dest))
        return
    except ImportError:
        pass

    with (
        urllib.request.urlopen(url, timeout=_DOWNLOAD_TIMEOUT_S) as resp,  # noqa: S310
        dest.open("wb") as fh,
    ):
        while chunk := resp.read(65536):
            fh.write(chunk)


def _extract_zip(zip_path: Path, dest: Path) -> None:
    with zipfile.ZipFile(zip_path) as zf:
        _extract_zip_members(zf, dest)


def _extract_zip_members(zf: zipfile.ZipFile, dest: Path) -> None:
    """Extract *zf* into *dest*, flattening member paths to their basename.

    Flattening both matches the ResStock bundle layout (``home.xml`` and
    ``in.schedules.csv`` at the archive root) and neutralises zip-slip: a member
    named ``../../etc/passwd`` collapses to ``passwd`` inside *dest*.
    """
    dest.mkdir(parents=True, exist_ok=True)
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
    for uri in _HPXML_NS.values():
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
    "01": "AL",
    "02": "AK",
    "04": "AZ",
    "05": "AR",
    "06": "CA",
    "08": "CO",
    "09": "CT",
    "10": "DE",
    "11": "DC",
    "12": "FL",
    "13": "GA",
    "15": "HI",
    "16": "ID",
    "17": "IL",
    "18": "IN",
    "19": "IA",
    "20": "KS",
    "21": "KY",
    "22": "LA",
    "23": "ME",
    "24": "MD",
    "25": "MA",
    "26": "MI",
    "27": "MN",
    "28": "MS",
    "29": "MO",
    "30": "MT",
    "31": "NE",
    "32": "NV",
    "33": "NH",
    "34": "NJ",
    "35": "NM",
    "36": "NY",
    "37": "NC",
    "38": "ND",
    "39": "OH",
    "40": "OK",
    "41": "OR",
    "42": "PA",
    "44": "RI",
    "45": "SC",
    "46": "SD",
    "47": "TN",
    "48": "TX",
    "49": "UT",
    "50": "VT",
    "51": "VA",
    "53": "WA",
    "54": "WV",
    "55": "WI",
    "56": "WY",
}

# IECC climate zone number ranges valid for each US state.
# Source: IECC 2021 climate zone map (ASHRAE 169-2021 Table B-1).
# Each list contains the integer zone numbers (1-8) that appear in that state.
# Used for coarse cross-zone validation between building HPXML and weather EPW.
_IECC_STATE_ZONES: dict[str, frozenset[int]] = {
    "AL": frozenset({2, 3}),
    "AK": frozenset({7, 8}),
    "AZ": frozenset({2, 3, 4, 5}),
    "AR": frozenset({3, 4}),
    "CA": frozenset({2, 3, 4, 5, 6}),
    "CO": frozenset({4, 5, 6, 7}),
    "CT": frozenset({5}),
    "DE": frozenset({4}),
    "FL": frozenset({1, 2}),
    "GA": frozenset({2, 3, 4}),
    "HI": frozenset({1}),
    "ID": frozenset({5, 6}),
    "IL": frozenset({4, 5}),
    "IN": frozenset({4, 5}),
    "IA": frozenset({5, 6}),
    "KS": frozenset({3, 4, 5}),
    "KY": frozenset({4}),
    "LA": frozenset({2, 3}),
    "ME": frozenset({6, 7}),
    "MD": frozenset({4}),
    "MA": frozenset({5}),
    "MI": frozenset({5, 6, 7}),
    "MN": frozenset({6, 7}),
    "MS": frozenset({2, 3}),
    "MO": frozenset({4, 5}),
    "MT": frozenset({6, 7}),
    "NE": frozenset({5, 6}),
    "NV": frozenset({3, 4, 5}),
    "NH": frozenset({5, 6}),
    "NJ": frozenset({4, 5}),
    "NM": frozenset({3, 4, 5}),
    "NY": frozenset({4, 5, 6}),
    "NC": frozenset({3, 4, 5}),
    "ND": frozenset({6, 7}),
    "OH": frozenset({4, 5}),
    "OK": frozenset({3, 4}),
    "OR": frozenset({4, 5}),
    "PA": frozenset({4, 5}),
    "RI": frozenset({5}),
    "SC": frozenset({2, 3}),
    "SD": frozenset({5, 6}),
    "TN": frozenset({3, 4}),
    "TX": frozenset({1, 2, 3, 4}),
    "UT": frozenset({5, 6, 7}),
    "VT": frozenset({5, 6}),
    "VA": frozenset({3, 4, 5}),
    "WA": frozenset({4, 5, 6}),
    "WV": frozenset({4, 5}),
    "WI": frozenset({6, 7}),
    "WY": frozenset({6, 7}),
    "DC": frozenset({4}),
    "PR": frozenset({1}),
}


def _parse_climate_zone(hpxml_path: Path) -> str | None:
    """Extract IECC climate zone from HPXML ClimateandRiskZones/ClimateZoneIECC/ClimateZone."""
    try:
        tree = ET.parse(hpxml_path)  # noqa: S314
        root = tree.getroot()
    except ET.ParseError:
        return None

    for uri in _HPXML_NS.values():
        zone_el = root.find(
            f".//{{{uri}}}ClimateandRiskZones/{{{uri}}}ClimateZoneIECC/{{{uri}}}ClimateZone"
        )
        if zone_el is not None and zone_el.text:
            return zone_el.text.strip()

    zone_el = root.find(".//ClimateandRiskZones/ClimateZoneIECC/ClimateZone")
    if zone_el is not None and zone_el.text:
        return zone_el.text.strip()

    return None


def _extract_epw_state(epw_path: Path) -> str | None:
    """Extract US state abbreviation from an EPW file's LOCATION header line.

    EPW LOCATION header format (EPW Data Dictionary):
        LOCATION,city_state_country,state_province,country,source,WMO#,lat,lon,tz,elev
    Field index 2 is the state/province abbreviation (e.g. 'CO').
    """
    try:
        with epw_path.open() as fh:
            first_line = fh.readline()
    except OSError:
        return None

    if not first_line.startswith("LOCATION"):
        return None

    fields = first_line.strip().split(",")
    if len(fields) < 3:
        return None

    state = fields[2].strip()
    if len(state) == 2 and state.isascii() and state.isalpha():
        return state.upper()
    return None


def _validate_zone_for_state(building_zone: str, state: str) -> None:
    """Validate that a building's IECC zone is plausible for a given US state.

    Checks whether the building's IECC zone number is plausible for the state.
    Emits ``warnings.warn()`` for mismatches; raises ``ValueError`` for
    detectably invalid combinations (zone differences ≥ 4).
    """
    valid_zone_numbers = _IECC_STATE_ZONES.get(state)
    if valid_zone_numbers is None:
        return

    try:
        building_zone_number = int(building_zone[0])
    except (IndexError, ValueError):
        return

    if building_zone_number in valid_zone_numbers:
        return

    diff = min(abs(building_zone_number - n) for n in valid_zone_numbers)
    if diff >= 4:
        raise ValueError(
            f"Building IECC zone {building_zone!r} is incompatible with state "
            f"{state!r} (valid zone numbers: {sorted(valid_zone_numbers)}). "
            f"The building location is too far from the weather location "
            f"to produce physically meaningful results."
        )

    warnings.warn(
        f"Building IECC zone {building_zone!r} may not match state "
        f"{state!r} (valid zone numbers for state: {sorted(valid_zone_numbers)}). "
        f"Cross-zone pairings produce physically invalid results with no other visible "
        f"symptom.",
        UserWarning,
        stacklevel=2,
    )


def _validate_zone_for_weather_path(building_zone: str, weather_path: Path) -> None:
    """Validate that a weather file's location is compatible with the building's IECC zone.

    Parses the EPW LOCATION header to extract the weather file's state, then
    delegates to ``_validate_zone_for_state``.
    """
    if not weather_path.name or weather_path.suffix.lower() != ".epw":
        return

    epw_state = _extract_epw_state(weather_path)
    if epw_state is None:
        return

    _validate_zone_for_state(building_zone, epw_state)


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

    # Buildings of a fleet are fetched concurrently and share weather caches
    # (one EPW archive, one CSV per county); one fetch at a time keeps two
    # buildings from writing the same cache file.
    with _WEATHER_LOCK:
        return _fetch_weather_for_station(
            cfg, hpxml_path, cache_dir, version, station, weather_format
        )


def _fetch_weather_for_station(
    cfg: _VersionConfig,
    hpxml_path: Path,
    cache_dir: Path,
    version: str,
    station: tuple[str, str],
    weather_format: WeatherFormat | None,
) -> Path:
    building_zone = _parse_climate_zone(hpxml_path)
    state, fips = station

    fmt = weather_format if weather_format is not None else cfg.weather_format

    if fmt is WeatherFormat.EPW:
        # TMY3 EPW from BuildStock_TMY3_FIPS.zip.
        # All TMY3 versions share the same EPW dataset, so use a
        # version-independent cache directory.
        from ochre_next.data.weather import get_epw_for_fips

        weather_cache = cache_dir / "weather"
        weather_path = get_epw_for_fips(fips, cache_dir=weather_cache)
        if building_zone is not None and weather_path.exists():
            _validate_zone_for_weather_path(building_zone, weather_path)
        return weather_path

    # CSV: download the simplified ResStock CSV from S3.
    weather_dest = cache_dir / version / "weather" / f"{fips}_{cfg.weather_suffix}.csv"
    if weather_dest.exists() and weather_dest.stat().st_size > 0:
        if _validate_cache_integrity(weather_dest):
            if building_zone is not None:
                _validate_zone_for_state(building_zone, state)
            return weather_dest
        # SHA256 mismatch — discard corrupted file and sidecar, re-download.
        _remove_cache_with_sidecar(weather_dest)
    url = _weather_url(cfg, state, fips)
    _download_file(url, weather_dest)
    _write_sha256_sidecar(weather_dest)
    log.debug("Weather file %s downloaded, SHA256 stored", fips)
    if building_zone is not None:
        _validate_zone_for_state(building_zone, state)
    return weather_dest


def _weight_column(df: pl.DataFrame) -> str | None:
    """Return the ResStock sample-weight column name, or None if absent.

    ResStock metadata parquets name the weight column ``in.sample_weight`` (or
    a close variant); it is the per-building population multiplier used to
    re-weight individual simulations up to the national stock.
    """
    return next((c for c in df.columns if "sample_weight" in c.lower()), None)


def _lookup_sample_weight(metadata_path: str | Path, bldg_id: int) -> float | None:
    """Return *bldg_id*'s sample weight from a metadata parquet, or None.

    Returns None when the parquet has no recognisable weight column or the
    building is not present in it, so the caller can decide how to handle a
    missing weight. Uses the same column-detection logic as the fleet path so
    the single-building and fleet weights are guaranteed to agree.
    """
    import polars as pl

    df = pl.read_parquet(metadata_path)
    weight_col = _weight_column(df)
    if weight_col is None:
        return None
    id_col = _bldg_id_col(df)
    matched = df.filter(pl.col(id_col) == bldg_id)
    if matched.height == 0:
        return None
    return float(matched[weight_col][0])


def _resolve_sample_weight(
    bldg_id: int,
    metadata_path: str | Path | None,
    sample_weight: float | None,
) -> float:
    """Resolve the sample weight for a single building.

    Precedence: an explicit *sample_weight* wins over *metadata_path*; a
    metadata lookup wins over the default. When neither yields a weight the
    value defaults to 1.0 and a ``UserWarning`` is emitted, because an
    unweighted result cannot be correctly aggregated to population level.
    """
    if sample_weight is not None:
        weight = sample_weight
    elif metadata_path is not None:
        looked_up = _lookup_sample_weight(metadata_path, bldg_id)
        if looked_up is None:
            warnings.warn(
                f"Building {bldg_id} not found in metadata {metadata_path!r}; "
                f"sample_weight defaults to 1.0. Population-level aggregation of "
                f"this building will not be correctly weighted.",
                UserWarning,
                stacklevel=3,
            )
            weight = 1.0
        else:
            weight = looked_up
    else:
        warnings.warn(
            "No metadata_path or sample_weight provided; sample_weight defaults "
            "to 1.0. Population-level aggregation of this building will not be "
            "correctly weighted.",
            UserWarning,
            stacklevel=3,
        )
        weight = 1.0
    log.debug("Resolved sample weight for building %d: %s", bldg_id, weight)
    return weight


def fetch_resstock_building(
    bldg_id: int,
    version: str = "2024.2",
    upgrade_id: int = 0,
    cache_dir: Path | None = None,
    weather_override: Path | None = None,
    weather_format: WeatherFormat | None = None,
    metadata_path: str | Path | None = None,
    sample_weight: float | None = None,
    _checksum_failures: list[int] | None = None,
) -> ResStockBuilding:
    """Download a ResStock building bundle from OEDI S3 and return local paths.

    Files are cached to *cache_dir* and re-used on subsequent calls.

    Parameters
    ----------
    weather_override:
        Explicit path to a weather file -- skips all weather fetching.
    weather_format:
        ``WeatherFormat.EPW`` to force TMY3 EPW files (even for AMY
        versions like 2025.1), ``WeatherFormat.CSV`` to force the
        simplified S3 CSV, or ``None`` (default) to use the version's
        native format.
    metadata_path:
        Local path to a ResStock metadata parquet. When provided, the
        building's ``sample_weight`` is looked up from it by building ID so
        the returned weight matches the fleet path.
    sample_weight:
        Explicit population sample weight. Takes precedence over
        *metadata_path*. When neither is given the weight defaults to 1.0 and
        a ``UserWarning`` is emitted, because an unweighted result cannot be
        correctly aggregated to population level.
    """
    base_cache = cache_dir if cache_dir is not None else _default_cache_dir()
    cfg = _version_config(version)
    bldg_dir = _building_cache_dir(base_cache, version, bldg_id)
    hpxml_path = bldg_dir / "home.xml"
    schedule_path = bldg_dir / "in.schedules.csv"

    if (
        hpxml_path.exists()
        and hpxml_path.stat().st_size > 0
        and schedule_path.exists()
        and schedule_path.stat().st_size > 0
    ):
        if not (
            _validate_cache_integrity(hpxml_path)
            and _validate_cache_integrity(schedule_path)
        ):
            if _checksum_failures is not None:
                _checksum_failures[0] += 1
            _remove_cache_with_sidecar(hpxml_path)
            _remove_cache_with_sidecar(schedule_path)
            url = _zip_url(cfg, bldg_id, upgrade_id)
            _download_and_extract_zip(url, bldg_dir)
    else:
        url = _zip_url(cfg, bldg_id, upgrade_id)
        _download_and_extract_zip(url, bldg_dir)

    if weather_override is not None:
        weather_path = weather_override
        building_zone = _parse_climate_zone(hpxml_path)
        if building_zone is not None and weather_path.exists():
            _validate_zone_for_weather_path(building_zone, weather_path)
            if weather_path.suffix.lower() != ".epw":
                station = _parse_weather_station(hpxml_path)
                if station is not None:
                    _validate_zone_for_state(building_zone, station[0])
    else:
        weather_path = _fetch_weather(
            cfg,
            hpxml_path,
            base_cache,
            version,
            weather_format=weather_format,
        )

    resolved_weight = _resolve_sample_weight(bldg_id, metadata_path, sample_weight)

    return ResStockBuilding(
        bldg_id=bldg_id,
        sample_weight=resolved_weight,
        hpxml_path=hpxml_path,
        schedule_path=schedule_path,
        weather_path=weather_path,
    )


def _result_interruptibly(future: Future[ResStockBuilding]) -> ResStockBuilding:
    """``future.result()``, waited for in slices so that Ctrl-C can interrupt it.

    Importing polars, which every fleet fetch does, installs a SIGINT handler
    with ``SA_RESTART``: the kernel then restarts a main thread's untimed lock
    wait after the signal, and the interpreter never gets the chance to raise
    ``KeyboardInterrupt``. A timed wait returns to the interpreter, which
    raises the pending interrupt.
    """
    while not future.done():
        wait([future], timeout=_INTERRUPT_CHECK_S)
    return future.result()


def _fetch_fleet(
    bldg_ids: list[int],
    version: str,
    base_cache: Path,
    upgrade_id: int,
    weights: dict[int, float],
    weather_format: WeatherFormat | None = None,
) -> list[ResStockBuilding]:
    """Fetch every building through ``fetch_resstock_building``, concurrently.

    ``fetch_resstock_building`` is the one download path for a single building
    and for a fleet alike, so a stand-in for it covers both. A building whose
    fetch fails is logged and skipped; the rest of the fleet is returned in
    request order. An unknown ``version`` fails the whole fetch up front.

    An interrupt (Ctrl-C) stops the fleet at once: queued buildings never
    start, retries stop, and with httpx installed the downloads in flight are
    cut, whether connecting, in the TLS handshake or reading. Without httpx,
    a download in flight on boto3 or urllib finishes or reaches its timeout
    first.
    """
    _parse_version(version)
    checksum_failures = {bid: [0] for bid in bldg_ids}
    cancelled = threading.Event()

    def fetch(bid: int) -> ResStockBuilding:
        _cancellation.set(cancelled)
        return fetch_resstock_building(
            bid,
            version=version,
            upgrade_id=upgrade_id,
            cache_dir=base_cache,
            weather_format=weather_format,
            sample_weight=weights.get(bid, 1.0),
            _checksum_failures=checksum_failures[bid],
        )

    results: list[ResStockBuilding] = []
    workers = max(1, min(_FLEET_DOWNLOAD_WORKERS, len(bldg_ids)))
    pool = ThreadPoolExecutor(max_workers=workers)
    try:
        futures = [pool.submit(fetch, bid) for bid in bldg_ids]
        for bid, future in zip(bldg_ids, futures):
            try:
                results.append(_result_interruptibly(future))
            except Exception as exc:
                log.error("ResStock building %d skipped: %s: %s", bid, type(exc).__name__, exc)
    except BaseException:
        cancelled.set()
        _shared_client.abort()
        pool.shutdown(cancel_futures=True)
        raise
    pool.shutdown()

    log.info(
        "ResStock fleet download complete: %d succeeded, %d failed, "
        "%d checksum failures (of %d requested)",
        len(results),
        len(bldg_ids) - len(results),
        sum(count[0] for count in checksum_failures.values()),
        len(bldg_ids),
    )
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

    weight_col = _weight_column(df)

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

    return _fetch_fleet(
        [int(i) for i in selected_ids],
        version,
        base_cache,
        upgrade_id,
        weights,
        weather_format=weather_format,
    )


def _bldg_id_col(df: pl.DataFrame) -> str:
    """Return the building ID column name from a polars DataFrame."""
    import polars as pl

    for candidate in ("bldg_id", "building_id", "Building"):
        if candidate in df.columns:
            return candidate
    for col in df.columns:
        dtype = df[col].dtype
        # Membership against the dtype classes trips pyright's
        # reportUnnecessaryContains (a DataType instance and the class
        # objects "have no overlap"), so the same exact-class set is spelled
        # as an equality chain. Unlike is_integer(), Int8/Int16/UInt8/UInt16
        # deliberately do not match.
        if (
            dtype == pl.Int32
            or dtype == pl.Int64
            or dtype == pl.UInt32
            or dtype == pl.UInt64
        ):
            return col
    raise ValueError(f"Cannot find building ID column in: {df.columns}")
