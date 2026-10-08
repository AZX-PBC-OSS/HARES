"""Generate per-surface POA irradiance using pvlib (matching OCHRE exactly).

Produces a CSV with per-surface direct/diffuse/reflected/aoi at 1-min resolution
that can be injected into HARES via the solar_override API.

Usage:
    cd /home/rich/src/HARES
    uv run --group ochre python tests/python/generate_pvlib_solar_override.py --out <dir>

Writes only into the --out directory. Replacing a committed fixture under
tests/fixtures/freefloat/ is a deliberate copy of the generated files, in a
commit that states the pvlib version and the reason.
"""

from __future__ import annotations

import argparse
import datetime as dt
import importlib.metadata
import math
import zoneinfo
from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict

if TYPE_CHECKING:
    import pandas as pd

REPO_ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = REPO_ROOT / "data" / "examples"

EPW_PATH = EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
HPXML_PATH = EXAMPLES / "BEopt_example.xml"

# BEopt building surface geometry (from HARES surface_geometry output).
# Order must match building.boundaries in HARES.
# tilt_deg: 0=horizontal up, 90=vertical, 180=horizontal down
# azimuth_deg: 0=N, 90=E, 180=S, 270=W
SURFACES = [
    # Walls (4 exterior walls)
    {"id": 0, "name": "Wall-S", "tilt": 90, "azimuth": 180, "area": 18.95},
    {"id": 1, "name": "Wall-N", "tilt": 90, "azimuth": 0, "area": 23.41},
    {"id": 2, "name": "Wall-E", "tilt": 90, "azimuth": 90, "area": 18.95},
    {"id": 3, "name": "Wall-W", "tilt": 90, "azimuth": 270, "area": 25.27},
    # Attic walls (2 gable ends)
    {"id": 4, "name": "AW-E", "tilt": 90, "azimuth": 90, "area": 13.42},
    {"id": 5, "name": "AW-W", "tilt": 90, "azimuth": 270, "area": 13.42},
    # Attic roofs (2 pitched faces)
    {"id": 6, "name": "Roof-N", "tilt": 26.57, "azimuth": 0, "area": 62.32},
    {"id": 7, "name": "Roof-S", "tilt": 26.57, "azimuth": 180, "area": 62.32},
    # Attic floor (connects indoor→attic, tilt=0 facing up)
    {"id": 8, "name": "AtticFloor", "tilt": 0, "azimuth": 0, "area": 111.48},
    # Door
    {"id": 9, "name": "Door", "tilt": 90, "azimuth": 0, "area": 1.86},
    # Slab (facing down = 180°)
    {"id": 10, "name": "Slab", "tilt": 180, "azimuth": 0, "area": 111.48},
    # Windows (6 windows on various walls)
    {"id": 11, "name": "Win-E1", "tilt": 90, "azimuth": 90, "area": 2.23},
    {"id": 12, "name": "Win-E2", "tilt": 90, "azimuth": 90, "area": 1.11},
    {"id": 13, "name": "Win-N1", "tilt": 90, "azimuth": 0, "area": 4.46},
    {"id": 14, "name": "Win-S1", "tilt": 90, "azimuth": 180, "area": 1.11},
    {"id": 15, "name": "Win-S2", "tilt": 90, "azimuth": 180, "area": 2.23},
    {"id": 16, "name": "Win-W1", "tilt": 90, "azimuth": 270, "area": 4.46},
    # Interior wall (no exterior surface -- zero solar)
    {"id": 17, "name": "IntWall", "tilt": 90, "azimuth": 180, "area": 111.48},
    # Furniture (no exterior surface -- zero solar)
    {"id": 18, "name": "Furniture", "tilt": 90, "azimuth": 180, "area": 44.59},
]

# Denver location
LAT, LON = 39.76, -104.86
TZ = "Etc/GMT+7"  # MST (no DST in TMY)
MST = zoneinfo.ZoneInfo(TZ)
ALBEDO = 0.2


class Scenario(TypedDict):
    name: str
    start_time: dt.datetime
    duration: dt.timedelta


SCENARIOS: list[Scenario] = [
    {
        "name": "beopt_spring_72h",
        "start_time": dt.datetime(2019, 5, 5, 12, 0, 0, tzinfo=MST),
        "duration": dt.timedelta(hours=72),
    },
    {
        "name": "beopt_summer_48h",
        "start_time": dt.datetime(2019, 7, 15, 12, 0, 0, tzinfo=MST),
        "duration": dt.timedelta(hours=48),
    },
    {
        "name": "beopt_winter_48h",
        "start_time": dt.datetime(2019, 1, 15, 12, 0, 0, tzinfo=MST),
        "duration": dt.timedelta(hours=48),
    },
]


def _component(irradiance: Mapping[str, float] | pd.DataFrame, key: str) -> float:
    """Extract one POA component from pvlib's get_total_irradiance result.

    pvlib documents the scalar-input return as "dict or DataFrame": recent
    releases return a mapping of numpy scalars (verified identical in shape
    from 0.14 through 0.16), while the DataFrame form is what pvlib's own
    source suggests to a type checker. float() unwraps numpy scalars, 0-d
    arrays and single-row frames alike, and anything else fails right here
    instead of silently degrading. perez evaluates its sky-clearness term
    as 0/0 when dhi is 0 while the sun is up, so non-finite components are
    rejected explicitly and can never reach the CSV.
    """
    raw = (
        irradiance[key] if isinstance(irradiance, Mapping) else irradiance[key].iloc[0]
    )
    component = float(raw)
    if not math.isfinite(component):
        raise ValueError(f"pvlib returned non-finite {key}: {raw!r}")
    return component


def generate_solar_override(scenario: Scenario, out_dir: Path) -> None:
    # Imported here, not at module level: main() parses its arguments first,
    # so a missing --out is reported whatever the environment can import.
    import numpy as np
    import pandas as pd
    import pvlib

    name = scenario["name"]
    start = pd.Timestamp(scenario["start_time"])
    n_steps = int(scenario["duration"].total_seconds() / 60)  # 1-min resolution
    times = pd.date_range(start, periods=n_steps, freq="1min")

    print(f"\n{'=' * 60}")
    print(f"Scenario: {name} ({n_steps} steps)")

    # Read EPW
    epw_df, _location = pvlib.iotools.read_epw(str(EPW_PATH))

    # Resample EPW to 1-min with forward fill (matching OCHRE)
    epw_annual = epw_df.copy()
    epw_annual.index = pd.date_range(start="2019-01-01", periods=8760, freq="1h", tz=TZ)
    # Shift EPW timestamps +30min for midpoint convention
    epw_annual.index = epw_annual.index + pd.Timedelta(minutes=30)
    epw_1min = epw_annual.resample("1min").ffill()

    # Get weather at simulation times
    weather = epw_1min.reindex(times, method="ffill")

    # Solar position
    solpos = pvlib.solarposition.get_solarposition(times, LAT, LON)
    dni_extra = pvlib.irradiance.get_extra_radiation(times)

    # Build per-surface POA
    rows = []
    for i, t in enumerate(times):
        z = float(solpos["zenith"].iloc[i])
        a = float(solpos["azimuth"].iloc[i])
        ghi = float(weather["ghi"].iloc[i])
        dni = float(weather["dni"].iloc[i])
        dhi = float(weather["dhi"].iloc[i])
        de = float(dni_extra.iloc[i])

        step_data: dict[str, float] = {"step": i}
        for surf in SURFACES:
            sid = surf["id"]
            tilt = surf["tilt"]
            azimuth = surf["azimuth"]

            if tilt == 180:
                # Floor facing down -- no solar
                step_data[f"s{sid}_direct"] = 0.0
                step_data[f"s{sid}_diffuse"] = 0.0
                step_data[f"s{sid}_reflected"] = 0.0
                step_data[f"s{sid}_aoi"] = 3.14159
                continue

            if surf["name"] in ("IntWall", "Furniture"):
                # Interior surfaces -- no exterior solar
                step_data[f"s{sid}_direct"] = 0.0
                step_data[f"s{sid}_diffuse"] = 0.0
                step_data[f"s{sid}_reflected"] = 0.0
                step_data[f"s{sid}_aoi"] = 3.14159
                continue

            # The aoi stays physical whenever the sun is up, including the
            # no-input case below.
            aoi_val = float(pvlib.irradiance.aoi(tilt, azimuth, z, a))

            if dni == 0.0 and ghi == 0.0 and dhi == 0.0:
                # No irradiance input: every POA term is a product with one
                # of these three, so all components are exactly zero. perez
                # instead evaluates its epsilon as 0/0 and yields NaN there.
                # The sun may still be just above the horizon, so nothing is
                # assumed about the aoi.
                direct = 0.0
                sky_diff = 0.0
                reflected = 0.0
            else:
                irr = pvlib.irradiance.get_total_irradiance(
                    tilt,
                    azimuth,
                    z,
                    a,
                    dni,
                    ghi,
                    dhi,
                    dni_extra=de,
                    model="perez",
                    albedo=ALBEDO,
                )
                direct = max(0.0, _component(irr, "poa_direct"))
                diffuse = max(0.0, _component(irr, "poa_diffuse"))
                # Split ground diffuse from sky diffuse isn't straightforward,
                # so use poa_ground_diffuse for reflected
                reflected = max(0.0, _component(irr, "poa_ground_diffuse"))
                sky_diff = max(0.0, diffuse - reflected)

            step_data[f"s{sid}_direct"] = direct
            step_data[f"s{sid}_diffuse"] = sky_diff
            step_data[f"s{sid}_reflected"] = reflected
            step_data[f"s{sid}_aoi"] = np.radians(aoi_val)

        rows.append(step_data)

        if i % 1000 == 0:
            print(f"  step {i}/{n_steps}")

    df = pd.DataFrame(rows)
    out_path = out_dir / name / "pvlib_solar_override.csv"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(out_path, index=False, float_format="%.6f")
    print(f"  Wrote {len(df)} steps × {len(df.columns)} cols to {out_path}")

    # Print step-0 summary
    print("  Step 0 sample (south wall, surface 0):")
    print(f"    direct={df['s0_direct'].iloc[0]:.1f} W/m²")
    print(f"    diffuse={df['s0_diffuse'].iloc[0]:.1f} W/m²")
    print(f"    reflected={df['s0_reflected'].iloc[0]:.1f} W/m²")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate per-surface POA irradiance CSVs with pvlib (matching OCHRE exactly).",
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        metavar="DIR",
        help="Directory to write the scenario CSVs into; the generator writes only there.",
    )
    args = parser.parse_args()
    out_dir: Path = args.out

    # Standard library only up to here: the numpy/pandas/pvlib imports live
    # below the argument parse (see generate_solar_override), so a missing
    # --out is reported whatever the environment can import.
    print(f"pvlib version: {importlib.metadata.version('pvlib')}")
    for scenario in SCENARIOS:
        generate_solar_override(scenario, out_dir)
    print(f"\n{'=' * 60}")
    print("All scenarios complete.")


if __name__ == "__main__":
    main()
