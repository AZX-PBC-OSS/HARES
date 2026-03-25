"""Generate conditioned HVAC oracle data from OCHRE.

Runs OCHRE with HVAC equipment active and other equipment stripped.
Exports per-timestep CSV at verbosity=9 for comparison with HARES.

Two modes per scenario:
- use_ideal_capacity=True -> output to tests/fixtures/conditioned_ideal/
- use_ideal_capacity=False -> output to tests/fixtures/conditioned_dynamic/

Usage:
    cd /home/rich/src/HARES
    uv run tests/python/generate_conditioned_oracle.py
"""

from __future__ import annotations

import datetime as dt
import json
import subprocess
import sys
from pathlib import Path
from typing import TypedDict

REPO_ROOT = Path(__file__).resolve().parents[2]
OCHRE_ROOT = REPO_ROOT / "vendors" / "OCHRE"
sys.path.insert(0, str(OCHRE_ROOT))

from ochre import Dwelling

FIXTURE_ROOT = REPO_ROOT / "tests" / "fixtures"

HPXML_FILE = OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example.xml"
SCHEDULE_FILE = (
    OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example_schedule.csv"
)
WEATHER_FILE = (
    OCHRE_ROOT
    / "ochre"
    / "defaults"
    / "Weather"
    / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)

EQUIPMENT_TO_STRIP = [
    "Clothes Washer",
    "Clothes Dryer",
    "Dishwasher",
    "Refrigerator",
    "Cooking Range",
    "Exterior Lighting",
    "Indoor Lighting",
    "MELs",
    "TV",
    "Ventilation Fan",
    "Electric Resistance Water Heater",
]


class Scenario(TypedDict):
    name: str
    start_time: dt.datetime
    duration: dt.timedelta


SCENARIOS: list[Scenario] = [
    {
        "name": "beopt_spring_72h",
        "start_time": dt.datetime(2019, 5, 5, 12, 0, 0),
        "duration": dt.timedelta(hours=72),
    },
    {
        "name": "beopt_summer_48h",
        "start_time": dt.datetime(2019, 7, 15, 12, 0, 0),
        "duration": dt.timedelta(hours=48),
    },
    {
        "name": "beopt_winter_48h",
        "start_time": dt.datetime(2019, 1, 15, 12, 0, 0),
        "duration": dt.timedelta(hours=48),
    },
]


def ochre_git_hash() -> str:
    """Get the git hash of the OCHRE vendor directory."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=OCHRE_ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def strip_non_hvac_equipment(dwelling: Dwelling) -> list[str]:
    """Remove non-HVAC equipment, keeping HVAC heater + cooler.

    Returns the list of equipment names that were removed.
    """
    from ochre.Equipment import HVAC

    removed = []
    equipment_to_keep = []

    for name, eq in list(dwelling.equipment.items()):
        if isinstance(eq, HVAC):
            equipment_to_keep.append(name)
        elif name in EQUIPMENT_TO_STRIP:
            removed.append(name)
        else:
            equipment_to_keep.append(name)

    for name in removed:
        eq = dwelling.equipment.pop(name)
        if eq in dwelling.sub_simulators:
            dwelling.sub_simulators.remove(eq)

    dwelling.equipment_by_end_use = {
        end_use: [e for e in dwelling.equipment.values() if e.end_use == end_use]
        for end_use in dwelling.equipment_by_end_use
    }

    return removed


def run_scenario(scenario: Scenario, use_ideal_capacity: bool) -> None:
    """Run a single conditioned scenario and export CSV + config."""
    name = scenario["name"]
    mode_name = "ideal" if use_ideal_capacity else "dynamic"
    output_dir = FIXTURE_ROOT / f"conditioned_{mode_name}" / name
    output_dir.mkdir(parents=True, exist_ok=True)

    csv_path = output_dir / "ochre_reference.csv"
    config_path = output_dir / "config.json"

    print(f"\n{'=' * 60}")
    print(f"Scenario: {name} ({mode_name})")
    print(f"  Start: {scenario['start_time']}")
    print(f"  Duration: {scenario['duration']}")
    print(f"  use_ideal_capacity: {use_ideal_capacity}")
    print(f"  Output: {csv_path}")

    dwelling = Dwelling(
        name=f"conditioned_{mode_name}_{name}",
        start_time=scenario["start_time"],
        time_res=dt.timedelta(minutes=1),
        duration=scenario["duration"],
        hpxml_file=str(HPXML_FILE),
        hpxml_schedule_file=str(SCHEDULE_FILE),
        weather_file=str(WEATHER_FILE),
        verbosity=9,
        metrics_verbosity=0,
        initialization_time=None,
        use_ideal_capacity=use_ideal_capacity,
    )

    removed = strip_non_hvac_equipment(dwelling)
    print(f"  Removed non-HVAC equipment: {removed}")

    hvac_names = [
        name
        for name, eq in dwelling.equipment.items()
        if hasattr(eq, "use_ideal_capacity")
    ]
    print(f"  HVAC equipment retained: {hvac_names}")

    df, _metrics, _hourly = dwelling.simulate()

    columns_of_interest = [
        col
        for col in df.columns
        if any(
            pattern in col
            for pattern in [
                "Temperature",
                "HVAC Heating",
                "HVAC Cooling",
                "Heat Gain",
                "Solar Gain",
                "LWR Gain",
                "Surface Temperature",
                "Flow Rate",
                "Air Changes",
                "Radiation Heat",
                "Sensible Heat",
                "Film Coefficient",
                "Setpoint",
                "Delivered",
                "COP",
            ]
        )
    ]

    df_out = df[columns_of_interest] if columns_of_interest else df

    df_out.to_csv(csv_path, index=True, float_format="%.6f")
    print(f"  Wrote {len(df_out)} rows x {len(df_out.columns)} columns to {csv_path}")

    print(f"  Columns:")
    for col in sorted(df_out.columns):
        vals = df_out[col].dropna()
        if len(vals) > 0:
            try:
                if vals.dtype in ("float64", "int64", "float32", "int32"):
                    print(
                        f"    {col:55s} mean={vals.mean():>10.2f}"
                        f"  min={vals.min():>10.2f}  max={vals.max():>10.2f}"
                    )
                else:
                    unique_vals = vals.unique()[:5]
                    print(
                        f"    {col:55s} dtype={vals.dtype}  unique={list(unique_vals)}"
                    )
            except (TypeError, ValueError):
                pass

    config = {
        "scenario": name,
        "mode": mode_name,
        "use_ideal_capacity": use_ideal_capacity,
        "hpxml_file": str(HPXML_FILE.relative_to(REPO_ROOT)),
        "schedule_file": str(SCHEDULE_FILE.relative_to(REPO_ROOT)),
        "weather_file": str(WEATHER_FILE.relative_to(REPO_ROOT)),
        "start_time": scenario["start_time"].isoformat(),
        "duration_hours": scenario["duration"].total_seconds() / 3600,
        "time_res_minutes": 1,
        "verbosity": 9,
        "equipment_removed": removed,
        "hvac_equipment_retained": hvac_names,
        "ochre_git_hash": ochre_git_hash(),
        "generated_at": dt.datetime.now().isoformat(),
        "columns": list(df_out.columns),
    }
    config_path.write_text(json.dumps(config, indent=2, default=str))
    print(f"  Wrote config to {config_path}")


def main() -> None:
    print(f"OCHRE root: {OCHRE_ROOT}")
    print(f"HPXML: {HPXML_FILE}")
    print(f"Weather: {WEATHER_FILE}")
    print(f"Fixture root: {FIXTURE_ROOT}")

    for scenario in SCENARIOS:
        run_scenario(scenario, use_ideal_capacity=True)
        run_scenario(scenario, use_ideal_capacity=False)

    print(f"\n{'=' * 60}")
    print("All scenarios complete.")
    print(f"Generated {len(SCENARIOS) * 2} fixture directories.")


if __name__ == "__main__":
    main()
