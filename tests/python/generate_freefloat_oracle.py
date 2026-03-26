"""Generate free-floating envelope oracle data from OCHRE.

Runs OCHRE with all equipment removed (free-floating envelope only) and
exports per-timestep CSV at verbosity=9 for comparison with HARES.

Usage:
    cd /home/rich/src/HARES
    uv run tests/python/generate_freefloat_oracle.py
"""

from __future__ import annotations

import datetime as dt
import json
import subprocess
import sys
from pathlib import Path
from typing import TypedDict

# Add OCHRE to path
REPO_ROOT = Path(__file__).resolve().parents[2]
OCHRE_ROOT = REPO_ROOT / "vendors" / "OCHRE"
EXAMPLES = REPO_ROOT / "data" / "examples"
sys.path.insert(0, str(OCHRE_ROOT))

from ochre import Dwelling  # noqa: E402

FIXTURE_ROOT = REPO_ROOT / "tests" / "fixtures" / "freefloat"

HPXML_FILE = EXAMPLES / "BEopt_example.xml"
SCHEDULE_FILE = EXAMPLES / "BEopt_example_schedule.csv"
WEATHER_FILE = EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"

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


def strip_equipment(dwelling: Dwelling) -> list[str]:
    """Remove all equipment from a Dwelling, leaving only the envelope.

    Returns the list of equipment names that were removed.
    """
    removed = list(dwelling.equipment.keys())

    # Remove equipment from sub_simulators (keep envelope)
    dwelling.sub_simulators = [
        s for s in dwelling.sub_simulators if s is dwelling.envelope
    ]
    dwelling.equipment.clear()
    dwelling.equipment_by_end_use = {
        k: [] for k in dwelling.equipment_by_end_use
    }
    dwelling.zones_for_schedule.clear()

    return removed


def run_scenario(scenario: Scenario) -> None:
    """Run a single free-floating scenario and export CSV + config."""
    name = scenario["name"]
    output_dir = FIXTURE_ROOT / name
    output_dir.mkdir(parents=True, exist_ok=True)

    csv_path = output_dir / "ochre_reference.csv"
    config_path = output_dir / "config.json"

    print(f"\n{'='*60}")
    print(f"Scenario: {name}")
    print(f"  Start: {scenario['start_time']}")
    print(f"  Duration: {scenario['duration']}")
    print(f"  Output: {csv_path}")

    dwelling = Dwelling(
        name=f"freefloat_{name}",
        start_time=scenario["start_time"],
        time_res=dt.timedelta(minutes=1),
        duration=scenario["duration"],
        hpxml_file=str(HPXML_FILE),
        hpxml_schedule_file=str(SCHEDULE_FILE),
        weather_file=str(WEATHER_FILE),
        verbosity=9,
        metrics_verbosity=0,
        initialization_time=None,
    )

    removed = strip_equipment(dwelling)
    print(f"  Removed equipment: {removed}")

    df, _metrics, _hourly = dwelling.simulate()

    # Select columns of interest
    columns_of_interest = [
        col
        for col in df.columns
        if any(
            pattern in col
            for pattern in [
                "Temperature",
                "Heat Gain",
                "Solar Gain",
                "LWR Gain",
                "Surface Temperature",
                "Flow Rate",
                "Air Changes",
                "Radiation Heat",
                "Sensible Heat",
                "Film Coefficient",
            ]
        )
    ]

    # Always include all columns for maximum diagnostic value
    df_out = df[columns_of_interest] if columns_of_interest else df

    # Write CSV with timestamp index
    df_out.to_csv(csv_path, index=True, float_format="%.6f")
    print(f"  Wrote {len(df_out)} rows x {len(df_out.columns)} columns to {csv_path}")

    # Print column summary
    print(f"  Columns:")
    for col in sorted(df_out.columns):
        vals = df_out[col].dropna()
        if len(vals) > 0:
            print(
                f"    {col:55s} mean={vals.mean():>10.2f}"
                f"  min={vals.min():>10.2f}  max={vals.max():>10.2f}"
            )

    # Write config
    config = {
        "scenario": name,
        "hpxml_file": str(HPXML_FILE.relative_to(REPO_ROOT)),
        "schedule_file": str(SCHEDULE_FILE.relative_to(REPO_ROOT)),
        "weather_file": str(WEATHER_FILE.relative_to(REPO_ROOT)),
        "start_time": scenario["start_time"].isoformat(),
        "duration_hours": scenario["duration"].total_seconds() / 3600,
        "time_res_minutes": 1,
        "verbosity": 9,
        "equipment_removed": removed,
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
        run_scenario(scenario)

    print(f"\n{'='*60}")
    print("All scenarios complete.")


if __name__ == "__main__":
    main()
