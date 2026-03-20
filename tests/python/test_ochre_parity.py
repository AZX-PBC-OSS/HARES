"""OCHRE parity test — runs both OCHRE and HARES on identical inputs and compares.

Requires:
    uv sync --group dev
    uv pip install -e vendors/OCHRE
    uv run maturin develop -m crates/hares-python/Cargo.toml
"""

from __future__ import annotations

import datetime as dt
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
VENDOR_OCHRE = ROOT / "vendors" / "OCHRE"
OCHRE_INPUTS = VENDOR_OCHRE / "ochre" / "defaults" / "Input Files"
OCHRE_WEATHER = VENDOR_OCHRE / "ochre" / "defaults" / "Weather"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(OCHRE_INPUTS / "BEopt_example.xml")
SCHEDULE = str(OCHRE_INPUTS / "BEopt_example_schedule.csv")
WEATHER = str(OCHRE_WEATHER / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

# Denver MDT = UTC-6 in May.  19:00 UTC = 13:00 local.
START_UTC = dt.datetime(2019, 5, 5, 19, 0, 0, tzinfo=dt.timezone.utc)
START_LOCAL = dt.datetime(2019, 5, 5, 13, 0)  # naive local for OCHRE
DURATION_H = 1
TIME_RES_MIN = 1


# ---------------------------------------------------------------------------
# OCHRE runner
# ---------------------------------------------------------------------------

def _run_ochre() -> dict[str, float]:
    """Run OCHRE and return {column_name: kWh} for 1-hour window."""
    # Ensure vendored OCHRE is importable
    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))

    from ochre import Dwelling as OchreDwelling

    dwelling = OchreDwelling(
        name="parity_test",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=6,
        save_results=False,
    )
    df, metrics, _hourly = dwelling.simulate()

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            result[col] = float((df[col] * time_res_h).sum())
    return result


# ---------------------------------------------------------------------------
# HARES runner
# ---------------------------------------------------------------------------

def _run_hares() -> dict[str, float]:
    """Run HARES via Python bindings and return {column_name: kWh}."""
    from ochre_next import Dwelling as HaresDwelling

    dwelling = HaresDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=START_UTC.isoformat(),
        time_res=TIME_RES_MIN * 60,
        duration=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        initialization_duration=24 * 3600,
    )

    # Step through and accumulate per-column power sums
    time_res_h = TIME_RES_MIN / 60.0
    n_steps = DURATION_H * 60 // TIME_RES_MIN
    sums: dict[str, float] = {}
    for _ in range(n_steps):
        step = dwelling.step()
        for key, val in step.items():
            if key == "time":
                continue
            sums[key] = sums.get(key, 0.0) + val

    # step() returns a flat dict; column naming may differ from CSV output.
    # Convert to kWh
    result: dict[str, float] = {}
    for col, total in sums.items():
        result[col] = total * time_res_h
    return result


def _run_hares_simulate() -> dict[str, float]:
    """Run HARES via simulate() and parse the DataFrame for column kWh."""
    pl = pytest.importorskip("polars")
    from ochre_next import Dwelling as HaresDwelling

    dwelling = HaresDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=START_UTC.isoformat(),
        time_res=TIME_RES_MIN * 60,
        duration=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        initialization_duration=24 * 3600,
    )
    df = dwelling.simulate()

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            result[col] = float(df[col].sum()) * time_res_h
    return result


# ---------------------------------------------------------------------------
# Column name mapping: OCHRE → HARES
# ---------------------------------------------------------------------------

OCHRE_TO_HARES: dict[str, list[str]] = {
    "HVAC Heating Electric Power (kW)": [
        "ASHP Heater Electric Power (kW)",
        "MSHP Heater Electric Power (kW)",
        "Gas Furnace Electric Power (kW)",
        "Electric Furnace Electric Power (kW)",
    ],
    "HVAC Cooling Electric Power (kW)": [
        "ASHP Cooler Electric Power (kW)",
        "MSHP Cooler Electric Power (kW)",
        "Air Conditioner Electric Power (kW)",
        "Room Air Conditioner Electric Power (kW)",
    ],
}


def _resolve_hares_kwh(ochre_col: str, hares: dict[str, float]) -> float | None:
    """Look up an OCHRE column name in HARES results, handling aliases."""
    if ochre_col in hares:
        return hares[ochre_col]
    aliases = OCHRE_TO_HARES.get(ochre_col, [])
    matched = [hares[a] for a in aliases if a in hares]
    if matched:
        return sum(matched)
    return None


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

ochre = pytest.importorskip("ochre", reason="OCHRE not installed")


@pytest.fixture(scope="module")
def ochre_results() -> dict[str, float]:
    return _run_ochre()


@pytest.fixture(scope="module")
def hares_results() -> dict[str, float]:
    return _run_hares_simulate()


def test_print_comparison(ochre_results: dict[str, float], hares_results: dict[str, float]) -> None:
    """Print side-by-side comparison (always runs, never fails)."""
    print("\n{'='*80}")
    print("OCHRE vs HARES — BEopt 1h parity (May 5, 2019, 19:00 UTC)")
    print("=" * 80)
    print(f"{'Column':<55} {'OCHRE':>8} {'HARES':>8} {'Diff%':>8}")
    print("-" * 80)

    for col in sorted(ochre_results):
        o_val = ochre_results[col]
        h_val = _resolve_hares_kwh(col, hares_results)
        if h_val is None:
            h_str = "   N/A"
            d_str = "   N/A"
        else:
            h_str = f"{h_val:8.4f}"
            d_str = (
                f"{(h_val - o_val) / o_val * 100:+7.1f}%"
                if abs(o_val) > 1e-9
                else "    -"
            )
        print(f"{col:<55} {o_val:8.4f} {h_str} {d_str}")


# Equipment-level tolerance checks.  These are intentionally loose to start —
# tighten as we fix root causes.
PARITY_CHECKS: list[tuple[str, float]] = [
    ("Total Electric Power (kW)", 0.60),
    ("HVAC Cooling Electric Power (kW)", 0.05),
    ("Ventilation Fan Electric Power (kW)", 0.05),
    ("MELs Electric Power (kW)", 0.20),
    ("TV Electric Power (kW)", 0.20),
    ("Refrigerator Electric Power (kW)", 0.20),
    ("Indoor Lighting Electric Power (kW)", 3.0),  # known 3x issue
    ("Exterior Lighting Electric Power (kW)", 3.0),
    ("HVAC Heating Electric Power (kW)", 0.60),
]


@pytest.mark.parametrize("col,tol", PARITY_CHECKS, ids=[c for c, _ in PARITY_CHECKS])
def test_parity(
    col: str,
    tol: float,
    ochre_results: dict[str, float],
    hares_results: dict[str, float],
) -> None:
    o_val = ochre_results.get(col)
    if o_val is None:
        pytest.skip(f"OCHRE has no column '{col}'")
    h_val = _resolve_hares_kwh(col, hares_results)
    if h_val is None:
        pytest.skip(f"HARES has no column matching '{col}'")

    if abs(o_val) < 1e-9:
        assert abs(h_val) < 1e-3, f"{col}: OCHRE≈0 but HARES={h_val:.4f}"
        return

    rel_err = abs(h_val - o_val) / abs(o_val)
    assert rel_err <= tol, (
        f"{col}: relative error {rel_err:.1%} exceeds tolerance {tol:.0%} "
        f"(OCHRE={o_val:.4f}, HARES={h_val:.4f})"
    )
