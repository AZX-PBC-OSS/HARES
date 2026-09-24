"""Regenerate OCHRE reference parquet files for the HARES parity test corpus.

For each complete parity fixture (building.xml + schedule.csv + weather.epw + config.toml),
runs OCHRE with the fixture simulation parameters and writes the output DataFrame to
reference_output.parquet.

Usage (from repo root):
    uv run --group ochre python tests/python/generate_parity_reference.py

Optional flags:
    --fixture <name>   Only regenerate a specific fixture (by directory name).
    --dry-run          Print what would be run without writing any files.
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import pandas as pd

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_ROOT = ROOT / "tests" / "fixtures" / "parity"
VENDOR_OCHRE = ROOT / "vendors" / "OCHRE"

REQUIRED_FILES = ("building.xml", "schedule.csv", "weather.epw", "config.toml")


def _ensure_ochre_importable() -> None:
    vendor_str = str(VENDOR_OCHRE)
    if vendor_str not in sys.path:
        sys.path.insert(0, vendor_str)
    try:
        import ochre  # noqa: F401
    except ImportError as exc:
        raise SystemExit(
            f"OCHRE is not importable from {VENDOR_OCHRE}. "
            "Run: uv run --group ochre python tests/python/generate_parity_reference.py"
        ) from exc


def _load_sim_config(config_path: Path) -> dict:
    cfg = tomllib.loads(config_path.read_text())
    sim = cfg.get("simulation", cfg)
    start_iso = sim["start_time"]
    # Strip timezone offset for OCHRE (it takes naive local time)
    start_dt = dt.datetime.fromisoformat(start_iso)
    start_local = start_dt.replace(tzinfo=None)
    duration_s = int(sim["duration"])
    time_res_s = int(sim["time_res"])
    verbosity = int(sim.get("output_verbosity", 3))
    return {
        "start_local": start_local,
        "duration": dt.timedelta(seconds=duration_s),
        "time_res": dt.timedelta(seconds=time_res_s),
        "verbosity": verbosity,
    }


def _discover_fixtures() -> list[Path]:
    fixtures = []
    for entry in sorted(FIXTURE_ROOT.iterdir()):
        if not entry.is_dir():
            continue
        missing = [f for f in REQUIRED_FILES if not (entry / f).exists()]
        if missing:
            print(f"  [skip] {entry.name}: missing {missing}")
            continue
        fixtures.append(entry)
    return fixtures


def _run_ochre_for_fixture(fixture_dir: Path) -> pd.DataFrame:
    """Run OCHRE on a fixture and return the minute-resolution output DataFrame."""
    from ochre import Dwelling as OchreDwelling

    sim_cfg = _load_sim_config(fixture_dir / "config.toml")

    dwelling = OchreDwelling(
        name=f"parity_ref_{fixture_dir.name}",
        start_time=sim_cfg["start_local"],
        time_res=sim_cfg["time_res"],
        duration=sim_cfg["duration"],
        hpxml_file=str((fixture_dir / "building.xml").resolve()),
        hpxml_schedule_file=str((fixture_dir / "schedule.csv").resolve()),
        weather_file=str((fixture_dir / "weather.epw").resolve()),
        verbosity=sim_cfg["verbosity"],
        save_results=False,
    )
    df, _metrics, _hourly = dwelling.simulate()
    return df


def _df_to_parquet(df: pd.DataFrame, out_path: Path) -> None:
    """Write a pandas DataFrame to parquet via pyarrow, coercing all numeric columns to float64."""
    import pandas as pd  # noqa: F811 -- runtime import, TYPE_CHECKING has the static one
    import pyarrow as pa
    import pyarrow.parquet as pq

    # Reset index so Time becomes a column
    out = df.copy()
    if out.index.name == "Time" or (hasattr(out.index, "name") and out.index.name):
        out = out.reset_index()

    # Coerce all non-Time columns to float64; drop non-numeric columns that can't be cast
    float_cols: list[str] = []
    for col in out.columns:
        if col == "Time":
            float_cols.append(col)
            continue
        try:
            out[col] = pd.to_numeric(out[col], errors="raise").astype("float64")
            float_cols.append(col)
        except (ValueError, TypeError):
            pass  # drop columns that cannot be cast to float

    out = out[float_cols]

    table = pa.Table.from_pandas(out, preserve_index=False)
    pq.write_table(table, out_path, compression="snappy")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fixture",
        default=None,
        help="Only regenerate this fixture (directory name under tests/fixtures/parity/).",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print what would be run without writing any files.",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    _ensure_ochre_importable()

    fixtures = _discover_fixtures()

    if args.fixture:
        fixtures = [f for f in fixtures if f.name == args.fixture]
        if not fixtures:
            raise SystemExit(
                f"Fixture '{args.fixture}' not found or missing required files under {FIXTURE_ROOT}"
            )

    if not fixtures:
        raise SystemExit(f"No complete fixtures found under {FIXTURE_ROOT}")

    print(f"Found {len(fixtures)} fixture(s) to regenerate:")
    for fixture in fixtures:
        print(f"  {fixture.name}")

    errors: list[str] = []
    for fixture_dir in fixtures:
        out_path = fixture_dir / "reference_output.parquet"
        print(f"\n[{fixture_dir.name}] running OCHRE ...", flush=True)

        if args.dry_run:
            print(f"  [dry-run] would write {out_path}")
            continue

        try:
            df = _run_ochre_for_fixture(fixture_dir)
            _df_to_parquet(df, out_path)
            print(f"  wrote {out_path} ({len(df)} rows, {len(df.columns)} columns)")
        except (RuntimeError, ValueError, OSError, ImportError, KeyError) as exc:
            msg = f"[{fixture_dir.name}] FAILED: {exc}"
            print(f"  ERROR: {exc}", file=sys.stderr)
            errors.append(msg)

    if errors:
        print("\nErrors encountered:")
        for err in errors:
            print(f"  {err}")
        raise SystemExit(1)

    print("\nDone. All reference parquets regenerated.")


if __name__ == "__main__":
    main()
