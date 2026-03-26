"""SAM PV performance model adapter.

Generates a PV power look-up table (LUT) from PySAM PVWatts and caches the
result as a Parquet file on disk.  The Rust consumer (``hares-equipment`` PV
model) reads this Parquet and performs N-dimensional linear interpolation at
each timestep.

When PySAM is not installed, PV generation uses direct PVWatts equations in the
Rust core -- no adapter output is required in that path.
"""

from __future__ import annotations

import hashlib
import logging
from dataclasses import dataclass, field as dataclass_field
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


GHI_BIN_W_M2 = 50
DNI_BIN_W_M2 = 50
DHI_BIN_W_M2 = 50
TEMP_BIN_C = 5


def _canonical_hash(
    *,
    system_capacity_kw: float,
    tilt: float,
    azimuth: float,
    module_type: int,
    array_type: int,
    weather_file: Path,
) -> str:
    """Compute a deterministic SHA-256 hash of all input parameters.

    Keys are sorted alphabetically and values formatted with fixed precision
    (``{:.6f}``).  The weather file content hash is included.
    """
    weather_sha = hashlib.sha256(weather_file.read_bytes()).hexdigest()
    parts: dict[str, str] = {
        "array_type": f"{array_type:.6f}",
        "azimuth": f"{azimuth:.6f}",
        "module_type": f"{module_type:.6f}",
        "system_capacity_kw": f"{system_capacity_kw:.6f}",
        "tilt": f"{tilt:.6f}",
        "weather_file_sha256": weather_sha,
    }
    canonical = "".join(f"{k}={v};" for k, v in sorted(parts.items()))
    return hashlib.sha256(canonical.encode()).hexdigest()


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

    def save(self, path: Path) -> None:
        """Write the LUT to a Parquet file on disk."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        pq.write_table(self.table, path)
        if "content_hash" in self.metadata:
            _write_cache(path, self.metadata["content_hash"])


def _run_pvwatts(
    system_capacity_kw: float,
    tilt: float,
    azimuth: float,
    module_type: int,
    array_type: int,
    weather_file: Path,
) -> PvWattsOutput:
    """Run PySAM PVWatts and return hourly results."""
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
    pv.execute()

    return {
        "ac": list(pv.Outputs.ac),
        "gh": list(pv.Outputs.gh),
        "dn": list(pv.Outputs.dn),
        "df": list(pv.Outputs.df),
        "tamb": list(pv.Outputs.tamb),
    }


def _bin_value(value: float, step: int) -> float:
    return round(round(value / step) * step, 6)


def _build_lut_table(pvwatts_output: PvWattsOutput) -> pa.Table:
    """Aggregate hourly PVWatts output into a binned LUT."""
    ac = pvwatts_output["ac"]
    gh = pvwatts_output["gh"]
    dn = pvwatts_output["dn"]
    df = pvwatts_output["df"]
    tamb = pvwatts_output["tamb"]

    n_hours = len(ac)
    aggregator: dict[tuple[int, int, float, float, float, float], list[float]] = {}

    for i in range(n_hours):
        hour_of_year = i
        month = (hour_of_year // 730) + 1
        month = min(month, 12)
        hour = hour_of_year % 24

        ghi_bin = _bin_value(gh[i], GHI_BIN_W_M2)
        dni_bin = _bin_value(dn[i], DNI_BIN_W_M2)
        dhi_bin = _bin_value(df[i], DHI_BIN_W_M2)
        temp_bin = _bin_value(tamb[i], TEMP_BIN_C)

        key = (month, hour, ghi_bin, dni_bin, dhi_bin, temp_bin)
        ac_kw = ac[i] / 1000.0
        aggregator.setdefault(key, []).append(ac_kw)

    months: list[int] = []
    hours: list[int] = []
    ghis: list[float] = []
    dnis: list[float] = []
    dhis: list[float] = []
    temps: list[float] = []
    powers: list[float] = []

    for (m, h, g, dn_v, dh, t), vals in sorted(aggregator.items()):
        months.append(m)
        hours.append(h)
        ghis.append(g)
        dnis.append(dn_v)
        dhis.append(dh)
        temps.append(t)
        powers.append(sum(vals) / len(vals))

    return pa.table(
        {
            "month": pa.array(months, type=pa.int32()),
            "hour": pa.array(hours, type=pa.int32()),
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
    cache_dir: Path | None = None,
) -> PvLut:
    """Generate a PV power LUT using PySAM PVWatts.

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
        Path to an EPW weather file.
    cache_dir:
        Optional directory for caching.  When provided, the adapter will
        check for a cached LUT and skip PySAM if the inputs have not changed.

    Returns
    -------
    PvLut
        A look-up table object.  Call ``.save(path)`` to write Parquet.
    """
    weather_file = Path(weather_file)
    content_hash = _canonical_hash(
        system_capacity_kw=system_capacity_kw,
        tilt=tilt,
        azimuth=azimuth,
        module_type=module_type,
        array_type=array_type,
        weather_file=weather_file,
    )

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "pv_lut.parquet"
        if _cache_valid(cached_path, content_hash):
            LOGGER.info("PV LUT cache hit: %s", cached_path)
            table = pq.read_table(cached_path)
            return PvLut(table=table, metadata={"content_hash": content_hash, "cached": True})

    LOGGER.info("Running PySAM PVWatts to generate PV LUT")
    raw = _run_pvwatts(
        system_capacity_kw=system_capacity_kw,
        tilt=tilt,
        azimuth=azimuth,
        module_type=module_type,
        array_type=array_type,
        weather_file=weather_file,
    )
    table = _build_lut_table(raw)
    lut = PvLut(table=table, metadata={"content_hash": content_hash, "cached": False})

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "pv_lut.parquet"
        lut.save(cached_path)

    return lut
