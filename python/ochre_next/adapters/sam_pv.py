"""SAM PV performance model adapter.

Generates a PV power look-up table (LUT) from PySAM PVWatts and caches the
result as a Parquet file on disk.  The Rust consumer (``hares-equipment`` PV
model) reads this Parquet and performs N-dimensional linear interpolation at
each timestep.

The LUT is indexed on **solar-position-aware dimensions** (solar zenith and
azimuth angles) rather than calendar month/hour.  This makes the LUT
location-independent: the same sun position produces the same plane-of-array
irradiance regardless of which EPW weather file was used to generate the LUT.
PVWatts internally translates horizontal irradiance (GHI/DNI/DHI) to
tilted-surface irradiance (POA) using sun position; by making sun position an
explicit LUT dimension, the LUT output for a given (zenith, azimuth, GHI, DNI,
DHI, temperature) tuple is consistent across sites.

When PySAM is not installed, PV generation uses direct PVWatts equations in the
Rust core -- no adapter output is required in that path.
"""

from __future__ import annotations

import hashlib
import logging
import math
from dataclasses import dataclass, field as dataclass_field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, TypedDict

import pyarrow as pa
import pyarrow.parquet as pq

try:
    import PySAM.Pvwattsv8 as _pvwatts  # type: ignore[import-untyped]

    _HAS_PYSAM = True
except ImportError:
    _pvwatts = None
    _HAS_PYSAM = False

LOGGER = logging.getLogger(__name__)


class PvWattsOutput(TypedDict):
    ac: list[float]
    gh: list[float]
    dn: list[float]
    df: list[float]
    tamb: list[float]
    inv_eff: float
    losses: float


GHI_BIN_W_M2 = 50
DNI_BIN_W_M2 = 50
DHI_BIN_W_M2 = 50
TEMP_BIN_C = 5
ZENITH_BIN_DEG = 5
AZIMUTH_BIN_DEG = 10


def _canonical_hash(
    *,
    system_capacity_kw: float,
    tilt: float,
    azimuth: float,
    module_type: int,
    array_type: int,
    weather_file: Path,
    latitude_deg: float,
    longitude_deg: float,
    elevation_m: float,
    inv_efficiency: float,
) -> str:
    """Compute a deterministic SHA-256 hash of all input parameters.

    Keys are sorted alphabetically and values formatted with fixed precision
    (``{:.6f}``).  The weather file content hash and site location are
    included so that re-running with a different EPW produces a different
    cache key.
    """
    weather_sha = hashlib.sha256(weather_file.read_bytes()).hexdigest()
    parts: dict[str, str] = {
        "array_type": f"{array_type:.6f}",
        "azimuth": f"{azimuth:.6f}",
        "elevation_m": f"{elevation_m:.6f}",
        "inv_efficiency": f"{inv_efficiency:.6f}",
        "latitude_deg": f"{latitude_deg:.6f}",
        "longitude_deg": f"{longitude_deg:.6f}",
        "module_type": f"{module_type:.6f}",
        "system_capacity_kw": f"{system_capacity_kw:.6f}",
        "tilt": f"{tilt:.6f}",
        "weather_file_sha256": weather_sha,
    }
    canonical = "".join(f"{k}={v};" for k, v in sorted(parts.items()))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _parse_epw_header(weather_file: Path) -> dict[str, float]:
    """Extract location metadata from an EPW file header.

    EPW header line format (comma-separated):
      LOCATION, city, state, country, source, WMO#, lat, lon, tz, elevation
    Fields are 1-indexed in the EPW specification:
      1=LOCATION, 7=latitude, 8=longitude, 9=timezone, 10=elevation

    Returns ``{"latitude", "longitude", "timezone", "elevation"}``.
    """
    with weather_file.open() as fh:
        header = fh.readline().strip()
    fields = header.split(",")
    if len(fields) < 10:
        raise ValueError(
            f"EPW header line has fewer than 10 fields; "
            f"expected standard EPW format, got: {header!r}"
        )
    return {
        "latitude": float(fields[6]),
        "longitude": float(fields[7]),
        "timezone": float(fields[8]),
        "elevation": float(fields[9]),
    }


# ---------------------------------------------------------------------------
# Spencer (1971) solar position model — matches hares-physics/src/solar.rs
# ---------------------------------------------------------------------------

# Spencer (1971) declination Fourier coefficients as (amplitude, harmonic, is_sine)
# triples.  Matches explicit form in hares-physics/src/solar.rs:105-109.
_SPENCER_DECL_COEFFS = [
    (0.006_918, 0.0, False),
    (-0.399_912, 1.0, False),
    (0.070_257, 1.0, True),
    (-0.006_758, 2.0, False),
    (0.000_907, 2.0, True),
    (-0.002_697, 3.0, False),
    (0.001_48, 3.0, True),
]
_EOT_SCALE = 229.18
# Spencer (1971) EOT Fourier coefficients as (amplitude, harmonic, is_sine)
# triples.  Matches explicit form in hares-physics/src/solar.rs:111-116.
_EOT_COEFFS = [
    (0.000_007_5, 0.0, False),
    (0.001_868, 1.0, False),
    (-0.032_077, 1.0, True),
    (-0.014_615, 2.0, False),
    (-0.040_849, 2.0, True),
]
_MINUTES_PER_DEGREE_LON = 4.0


def _solar_position(
    latitude_deg: float,
    longitude_deg: float,
    timezone_h: float,
    hour_of_year: int,
) -> tuple[float, float]:
    """Compute (solar_zenith_deg, solar_azimuth_deg) for a given hour.

    Uses the Spencer (1971) Fourier-series model for solar declination and
    equation of time, matching the Rust implementation in
    ``hares-physics/src/solar.rs``.

    Assumes a non-leap reference year (365 days) for day-of-year
    calculation.  The hour is treated at its midpoint (minute 30).

    Parameters
    ----------
    latitude_deg:
        Site latitude in degrees (north positive).
    longitude_deg:
        Site longitude in degrees (east positive).
    timezone_h:
        UTC offset in hours (e.g. -7.0 for MST, -8.0 for PST).
    hour_of_year:
        Hour index 0–8759, where 0 = Jan 1 00:00–01:00 local standard time.

    Returns
    -------
    (zenith_deg, azimuth_deg) in degrees.
    """
    day_of_year = hour_of_year // 24 + 1  # 1–365
    hour = hour_of_year % 24
    # Local standard time → UTC
    utc_hour = hour - timezone_h
    # Handle day wrapping from the UTC conversion
    if utc_hour < 0:
        utc_hour += 24
        day_of_year -= 1
    elif utc_hour >= 24:
        utc_hour -= 24
        day_of_year += 1

    minutes_utc = utc_hour * 60.0 + 30.0  # midpoint of the hour
    gamma = 2.0 * math.pi / 365.0 * (day_of_year - 1.0 + (minutes_utc - 720.0) / 1440.0)

    # Solar declination [rad]
    decl_rad = sum(
        a * (math.sin if is_sine else math.cos)(k * gamma)
        for a, k, is_sine in _SPENCER_DECL_COEFFS
    )

    # Equation of time [min]
    eq_time_min = _EOT_SCALE * sum(
        a * (math.sin if is_sine else math.cos)(k * gamma)
        for a, k, is_sine in _EOT_COEFFS
    )

    true_solar_time_min = minutes_utc + eq_time_min + _MINUTES_PER_DEGREE_LON * longitude_deg
    hour_angle_deg = true_solar_time_min * 0.25 - 180.0
    if hour_angle_deg < -180.0:
        hour_angle_deg += 360.0
    elif hour_angle_deg > 180.0:
        hour_angle_deg -= 360.0

    lat_rad = math.radians(latitude_deg)
    hour_angle_rad = math.radians(hour_angle_deg)

    cos_zenith = max(
        -1.0,
        min(
            1.0,
            math.sin(lat_rad) * math.sin(decl_rad)
            + math.cos(lat_rad) * math.cos(decl_rad) * math.cos(hour_angle_rad),
        ),
    )
    zenith_deg = math.degrees(math.acos(cos_zenith))

    # North-referenced azimuth matching hares-physics solar.rs
    numerator = math.sin(hour_angle_rad)
    denominator = math.cos(hour_angle_rad) * math.sin(lat_rad) - math.tan(decl_rad) * math.cos(
        lat_rad
    )
    azimuth_rad = (math.atan2(numerator, denominator) + math.pi) % (2.0 * math.pi)
    azimuth_deg = math.degrees(azimuth_rad)

    return (zenith_deg, azimuth_deg)


def _cache_valid(parquet_path: Path, expected_hash: str) -> bool:
    hash_path = parquet_path.with_suffix(".hash")
    if not parquet_path.exists() or not hash_path.exists():
        return False
    stored = hash_path.read_text().strip()
    return stored == expected_hash


def _write_cache(parquet_path: Path, content_hash: str) -> None:
    hash_path = parquet_path.with_suffix(".hash")
    hash_path.write_text(content_hash + "\n")


@dataclass
class PvLut:
    """PV power look-up table backed by an Arrow table."""

    table: pa.Table
    metadata: dict[str, Any] = dataclass_field(default_factory=dict)
    latitude_deg: float = 0.0
    longitude_deg: float = 0.0
    elevation_m: float = 0.0

    def save(self, path: Path) -> None:
        """Write the LUT to a Parquet file on disk with embedded metadata."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        # Embed location and SAM configuration metadata as Parquet
        # file-level key-value metadata so the Rust consumer can
        # correct for SAM's internal inverter efficiency and system
        # losses (T-0086).
        kv_meta = {
            b"harvest_lut_latitude_deg": f"{self.latitude_deg:.6f}".encode(),
            b"harvest_lut_longitude_deg": f"{self.longitude_deg:.6f}".encode(),
            b"harvest_lut_elevation_m": f"{self.elevation_m:.6f}".encode(),
        }
        if "inv_eff" in self.metadata:
            kv_meta[b"harvest_lut_sam_inv_eff"] = f"{self.metadata['inv_eff']:.6f}".encode()
        if "losses" in self.metadata:
            kv_meta[b"harvest_lut_sam_losses"] = f"{self.metadata['losses']:.6f}".encode()
        table_with_meta = self.table.replace_schema_metadata(kv_meta)
        pq.write_table(table_with_meta, path)
        if "content_hash" in self.metadata:
            _write_cache(path, self.metadata["content_hash"])


def _run_pvwatts(
    system_capacity_kw: float,
    tilt: float,
    azimuth: float,
    module_type: int,
    array_type: int,
    weather_file: Path,
    *,
    inv_efficiency: float,
) -> PvWattsOutput:
    """Run PySAM PVWatts and return hourly results.

    Parameters
    ----------
    inv_efficiency:
        Inverter efficiency as a fraction (0.0–1.0). Passes through to SAM's
        ``SystemDesign.inv_eff`` (percentage scale) so the AC output embeds
        the caller's chosen efficiency rather than SAM's default 0.96.
    """
    if _pvwatts is None:
        raise ImportError(
            "PySAM is required for SAM PV adapter; install with: "
            "pip install 'ochre_next[sam]'"
        )

    pv = _pvwatts.new()
    pv.SolarResource.solar_resource_file = str(weather_file)
    pv.SystemDesign.system_capacity = system_capacity_kw
    pv.SystemDesign.tilt = tilt
    pv.SystemDesign.azimuth = azimuth
    pv.SystemDesign.module_type = module_type
    pv.SystemDesign.array_type = array_type
    # SAM PVWatts inv_eff is a percentage (0–100); convert from fraction.
    pv.SystemDesign.inv_eff = inv_efficiency * 100.0
    LOGGER.info("SAM PVWatts inv_eff set to %.4f (%.2f %%)", inv_efficiency, inv_efficiency * 100.0)
    pv.execute()

    return {
        "ac": list(pv.Outputs.ac),
        "gh": list(pv.Outputs.gh),
        "dn": list(pv.Outputs.dn),
        "df": list(pv.Outputs.df),
        "tamb": list(pv.Outputs.tamb),
        "inv_eff": pv.SystemDesign.inv_eff / 100.0,
        "losses": pv.SystemDesign.losses / 100.0,
    }


def _validate_pvwatts_output(raw: PvWattsOutput) -> None:
    """Validate a PVWatts output mapping before LUT construction.

    Raises a descriptive ``ValueError`` instead of letting a malformed
    mapping (e.g. a stale mock, or a PySAM API change) surface as a bare
    ``KeyError`` deep inside LUT construction.
    """
    series_keys = ("ac", "gh", "dn", "df", "tamb")
    scalar_keys = ("inv_eff", "losses")
    missing = [k for k in (*series_keys, *scalar_keys) if k not in raw]
    if missing:
        raise ValueError(
            f"PVWatts output is missing required key(s) {missing}; "
            f"expected hourly series {list(series_keys)} and scalars "
            f"{list(scalar_keys)}. Got keys: {sorted(raw)}"
        )
    lengths = {k: len(raw[k]) for k in series_keys}  # type: ignore[literal-required]
    if len(set(lengths.values())) != 1:
        raise ValueError(
            f"PVWatts hourly series have inconsistent lengths: {lengths}"
        )
    for k in scalar_keys:
        v = raw[k]  # type: ignore[literal-required]
        if not isinstance(v, (int, float)) or not math.isfinite(v):
            raise ValueError(
                f"PVWatts output scalar '{k}' must be a finite number, got {v!r}"
            )


def _bin_value(value: float, step: int) -> float:
    return round(round(value / step) * step, 6)


def _build_lut_table(
    pvwatts_output: PvWattsOutput,
    *,
    latitude_deg: float,
    longitude_deg: float,
    timezone_h: float,
) -> pa.Table:
    """Aggregate hourly PVWatts output into a binned LUT indexed on solar position.

    Solar zenith and azimuth are computed at the midpoint of each hour
    using the Spencer (1971) model.  Irradiance and temperature are
    quantised to fixed bins; zenith and azimuth are binned at
    ``ZENITH_BIN_DEG`` / ``AZIMUTH_BIN_DEG`` resolution.

    After building the sparse (weather-observed) grid, each observed cell
    is expanded by one bin step along every axis.  Cells that differ from
    an observed cell on a single axis are added if absent and filled by
    nearest-neighbor from the sparse data.  This improves multi-linear
    coverage (fewer missing corners) compared to the weather-file-only
    grid, but does not guarantee all 2^6 = 64 corners are present for
    every in-hull query.  Brackets that need simultaneous displacement on
    two or more axes for a corner may still miss it.  See Known
    Limitations in the ticket for the full constraint.
    """
    ac = pvwatts_output["ac"]
    gh = pvwatts_output["gh"]
    dn = pvwatts_output["dn"]
    df = pvwatts_output["df"]
    tamb = pvwatts_output["tamb"]

    n_hours = len(ac)
    aggregator: dict[tuple[float, float, float, float, float, float], list[float]] = {}

    for i in range(n_hours):
        zenith, azimuth = _solar_position(latitude_deg, longitude_deg, timezone_h, i)

        zenith_bin = _bin_value(zenith, ZENITH_BIN_DEG)
        azimuth_bin = _bin_value(azimuth, AZIMUTH_BIN_DEG)
        ghi_bin = _bin_value(gh[i], GHI_BIN_W_M2)
        dni_bin = _bin_value(dn[i], DNI_BIN_W_M2)
        dhi_bin = _bin_value(df[i], DHI_BIN_W_M2)
        temp_bin = _bin_value(tamb[i], TEMP_BIN_C)

        key = (zenith_bin, azimuth_bin, ghi_bin, dni_bin, dhi_bin, temp_bin)
        ac_kw = ac[i] / 1000.0
        aggregator.setdefault(key, []).append(ac_kw)

    # Average AC power for each sparse bin (weather-observed data).
    sparse_grid: dict[tuple[float, float, float, float, float, float], float] = {}
    for key, vals in aggregator.items():
        sparse_grid[key] = sum(vals) / len(vals)

    # Collect sorted unique axis values from the sparse data.
    zenith_axis = sorted({k[0] for k in sparse_grid})
    azimuth_axis = sorted({k[1] for k in sparse_grid})
    ghi_axis = sorted({k[2] for k in sparse_grid})
    dni_axis = sorted({k[3] for k in sparse_grid})
    dhi_axis = sorted({k[4] for k in sparse_grid})
    temp_axis = sorted({k[5] for k in sparse_grid})

    # Build index maps for each axis: value → sorted index.
    zenith_idx: dict[float, int] = {v: i for i, v in enumerate(zenith_axis)}
    azimuth_idx: dict[float, int] = {v: i for i, v in enumerate(azimuth_axis)}
    ghi_idx: dict[float, int] = {v: i for i, v in enumerate(ghi_axis)}
    dni_idx: dict[float, int] = {v: i for i, v in enumerate(dni_axis)}
    dhi_idx: dict[float, int] = {v: i for i, v in enumerate(dhi_axis)}
    temp_idx: dict[float, int] = {v: i for i, v in enumerate(temp_axis)}

    # Pre-compute axis normalization factors for normalized nearest-neighbor
    # distance (matches the Rust PvLut nearest-neighbor metric).
    _axis_norm = {}
    axes: dict[str, list[float]] = {
        "zenith": zenith_axis,
        "azimuth": azimuth_axis,
        "ghi": ghi_axis,
        "dni": dni_axis,
        "dhi": dhi_axis,
        "temp": temp_axis,
    }
    for name, axis in axes.items():
        mn = axis[0]
        mx = axis[-1]
        rng = mx - mn
        _axis_norm[name] = (mn, 1.0 / max(rng, 1e-12))

    def _nn_fill(key: tuple[float, float, float, float, float, float]) -> float:
        """Return the AC power of the nearest sparse-grid point."""
        best_dist = float("inf")
        best_val: float = 0.0
        z_q, a_q, g_q, dn_q, dh_q, t_q = key
        z_n = (z_q - _axis_norm["zenith"][0]) * _axis_norm["zenith"][1]
        a_n = (a_q - _axis_norm["azimuth"][0]) * _axis_norm["azimuth"][1]
        g_n = (g_q - _axis_norm["ghi"][0]) * _axis_norm["ghi"][1]
        dn_n = (dn_q - _axis_norm["dni"][0]) * _axis_norm["dni"][1]
        dh_n = (dh_q - _axis_norm["dhi"][0]) * _axis_norm["dhi"][1]
        t_n = (t_q - _axis_norm["temp"][0]) * _axis_norm["temp"][1]
        for (z_s, a_s, g_s, dn_s, dh_s, t_s), val in sparse_grid.items():
            dz = z_n - (z_s - _axis_norm["zenith"][0]) * _axis_norm["zenith"][1]
            da = a_n - (a_s - _axis_norm["azimuth"][0]) * _axis_norm["azimuth"][1]
            dg = g_n - (g_s - _axis_norm["ghi"][0]) * _axis_norm["ghi"][1]
            dd = dn_n - (dn_s - _axis_norm["dni"][0]) * _axis_norm["dni"][1]
            dh = dh_n - (dh_s - _axis_norm["dhi"][0]) * _axis_norm["dhi"][1]
            dt = t_n - (t_s - _axis_norm["temp"][0]) * _axis_norm["temp"][1]
            dist = dz * dz + da * da + dg * dg + dd * dd + dh * dh + dt * dt
            if dist < best_dist:
                best_dist = dist
                best_val = val
        return best_val

    def _axis_neighbors(values: list[float], idx: int):
        """Yield (next_lower_idx, next_higher_idx) values within bounds."""
        if idx > 0:
            yield values[idx - 1]
        if idx + 1 < len(values):
            yield values[idx + 1]

    # Expand each sparse grid point by adding cells one bin step away
    # along each axis.  This improves multi-linear coverage compared to
    # the weather-file-only sparse grid, but a bracket corner that
    # requires simultaneous displacement on two or more axes may still
    # be absent.  See Known Limitations in the ticket.
    full_grid: dict[tuple[float, float, float, float, float, float], float] = dict(sparse_grid)
    n_filled: int = 0
    for (z, a, g, dn_v, dh, t), _ac in sparse_grid.items():
        zi = zenith_idx[z]
        ai = azimuth_idx[a]
        gi = ghi_idx[g]
        dni = dni_idx[dn_v]
        dhi = dhi_idx[dh]
        ti = temp_idx[t]

        # Neighbouring bin values on each axis (one step away).
        z_neighbors = list(_axis_neighbors(zenith_axis, zi))
        a_neighbors = list(_axis_neighbors(azimuth_axis, ai))
        g_neighbors = list(_axis_neighbors(ghi_axis, gi))
        dn_neighbors = list(_axis_neighbors(dni_axis, dni))
        dh_neighbors = list(_axis_neighbors(dhi_axis, dhi))
        t_neighbors = list(_axis_neighbors(temp_axis, ti))

        for nz in z_neighbors:
            key = (nz, a, g, dn_v, dh, t)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1
        for na in a_neighbors:
            key = (z, na, g, dn_v, dh, t)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1
        for ng in g_neighbors:
            key = (z, a, ng, dn_v, dh, t)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1
        for nd in dn_neighbors:
            key = (z, a, g, nd, dh, t)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1
        for ndh in dh_neighbors:
            key = (z, a, g, dn_v, ndh, t)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1
        for nt in t_neighbors:
            key = (z, a, g, dn_v, dh, nt)
            if key not in full_grid:
                full_grid[key] = _nn_fill(key)
                n_filled += 1

    if n_filled > 0:
        LOGGER.info(
            "PV LUT grid infill: added %d cells to dense %d-cell sparse grid",
            n_filled,
            len(sparse_grid),
        )

    zeniths: list[float] = []
    azimuths: list[float] = []
    ghis: list[float] = []
    dnis: list[float] = []
    dhis: list[float] = []
    temps: list[float] = []
    powers: list[float] = []

    for (z, a, g, dn_v, dh, t), pw in sorted(full_grid.items()):
        zeniths.append(z)
        azimuths.append(a)
        ghis.append(g)
        dnis.append(dn_v)
        dhis.append(dh)
        temps.append(t)
        powers.append(pw)

    return pa.table(
        {
            "solar_zenith_deg": pa.array(zeniths, type=pa.float64()),
            "solar_azimuth_deg": pa.array(azimuths, type=pa.float64()),
            "ghi": pa.array(ghis, type=pa.float64()),
            "dni": pa.array(dnis, type=pa.float64()),
            "dhi": pa.array(dhis, type=pa.float64()),
            "temp_c": pa.array(temps, type=pa.float64()),
            "ac_power_kw": pa.array(powers, type=pa.float64()),
        }
    )


def generate_pv_lut(
    system_capacity_kw: float,
    tilt: float,
    azimuth: float,
    module_type: int,
    array_type: int,
    weather_file: Path,
    *,
    inv_efficiency: float = 0.96,
    cache_dir: Path | None = None,
) -> PvLut:
    """Generate a PV power LUT using PySAM PVWatts.

    The LUT is indexed on solar zenith and azimuth angles so that the same
    sun position produces the same output regardless of which EPW file was
    used to generate it (location-independent).

    Parameters
    ----------
    system_capacity_kw:
        Nameplate DC capacity in kW.
    tilt:
        Panel tilt angle in degrees.
    azimuth:
        Panel azimuth in degrees (180 = south).
    module_type:
        PVWatts module type (0 = Standard, 1 = Premium, 2 = Thin Film).
    array_type:
        PVWatts array type (0 = Fixed Roof, 1 = Fixed Open Rack, etc.).
    weather_file:
        Path to an EPW weather file.  The EPW header is parsed for
        latitude, longitude, timezone, and elevation.
    inv_efficiency:
        Inverter efficiency as a fraction (0.0–1.0). Passes through to SAM's
        ``SystemDesign.inv_eff`` so the LUT AC output embeds this value
        rather than SAM's default 0.96. Defaults to 0.96 (SAM PVWatts v8
        default; NREL/TP-7A40-80694).
    cache_dir:
        Optional directory for caching.  When provided, the adapter will
        check for a cached LUT and skip PySAM if the inputs have not changed.

    Returns
    -------
    PvLut
        A look-up table object.  Call ``.save(path)`` to write Parquet.
    """
    weather_file = Path(weather_file)
    loc = _parse_epw_header(weather_file)
    latitude_deg = loc["latitude"]
    longitude_deg = loc["longitude"]
    timezone_h = loc["timezone"]
    elevation_m = loc["elevation"]

    content_hash = _canonical_hash(
        system_capacity_kw=system_capacity_kw,
        tilt=tilt,
        azimuth=azimuth,
        module_type=module_type,
        array_type=array_type,
        weather_file=weather_file,
        latitude_deg=latitude_deg,
        longitude_deg=longitude_deg,
        elevation_m=elevation_m,
        inv_efficiency=inv_efficiency,
    )

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "pv_lut.parquet"
        if _cache_valid(cached_path, content_hash):
            LOGGER.info("PV LUT cache hit: %s", cached_path)
            table = pq.read_table(cached_path)
            schema_meta = table.schema.metadata or {}
            inv_eff = float(schema_meta.get(b"harvest_lut_sam_inv_eff", b"0.0"))
            losses = float(schema_meta.get(b"harvest_lut_sam_losses", b"0.0"))
            return PvLut(
                table=table,
                metadata={
                    "content_hash": content_hash,
                    "cached": True,
                    "inv_eff": inv_eff,
                    "losses": losses,
                },
                latitude_deg=latitude_deg,
                longitude_deg=longitude_deg,
                elevation_m=elevation_m,
            )

    LOGGER.info("Running PySAM PVWatts to generate PV LUT (inv_eff=%.4f)", inv_efficiency)
    raw = _run_pvwatts(
        system_capacity_kw=system_capacity_kw,
        tilt=tilt,
        azimuth=azimuth,
        module_type=module_type,
        array_type=array_type,
        weather_file=weather_file,
        inv_efficiency=inv_efficiency,
    )
    _validate_pvwatts_output(raw)
    table = _build_lut_table(
        raw,
        latitude_deg=latitude_deg,
        longitude_deg=longitude_deg,
        timezone_h=timezone_h,
    )
    lut = PvLut(
        table=table,
        metadata={
            "content_hash": content_hash,
            "cached": False,
            "inv_eff": raw["inv_eff"],
            "losses": raw["losses"],
        },
        latitude_deg=latitude_deg,
        longitude_deg=longitude_deg,
        elevation_m=elevation_m,
    )

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "pv_lut.parquet"
        lut.save(cached_path)

    return lut
