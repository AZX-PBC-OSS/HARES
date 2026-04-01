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
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

# Denver LST = UTC-7.  13:00 local = 20:00 UTC.
# Both OCHRE and HARES take local standard time as the start time.
START_LOCAL = dt.datetime(2019, 5, 5, 13, 0)  # naive local for OCHRE
# HARES takes an ISO string; EPW timezone offset (-7h) is applied internally.
START_HARES = "2019-05-05T13:00:00"
DURATION_H = 1
TIME_RES_MIN = 1

# ZOH resampling for all continuous weather fields — matches OCHRE's pandas ffill().
OCHRE_COMPAT_RESAMPLE: dict[str, str] = {
    "dry_bulb": "zoh",
    "dew_point": "zoh",
    "rel_humidity": "zoh",
    "pressure": "zoh",
    "infrared": "zoh",
    "sky_temp": "zoh",
    "ground_temp": "zoh",
    "opaque_sky_cover": "zoh",
}


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
        start_time=START_HARES,
        time_res_s=TIME_RES_MIN * 60,
        duration_s=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        resample_overrides=OCHRE_COMPAT_RESAMPLE,
    )

    # Step through and accumulate per-column power sums
    time_res_h = TIME_RES_MIN / 60.0
    n_steps = DURATION_H * 60 // TIME_RES_MIN
    sums: dict[str, float] = {}
    for _ in range(n_steps):
        step = dwelling.step()
        for key, val in step.items():
            if key == "timestamp":
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
        start_time=START_HARES,
        time_res_s=TIME_RES_MIN * 60,
        duration_s=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        resample_overrides=OCHRE_COMPAT_RESAMPLE,
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
        "Room AC Electric Power (kW)",
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


# Tolerances per THERMAL-008 / ASHRAE 140-2023 §5.2.
# Schedule-driven loads: 2% (exact match expected).
# HVAC: 15% (ASHRAE 140 acceptance range for annual heating energy).
# Total: 10% (allows for unimplemented event-driven equipment).
PARITY_CHECKS: list[tuple[str, float]] = [
    ("Total Electric Power (kW)", 0.10),
    ("HVAC Cooling Electric Power (kW)", 0.05),
    ("HVAC Heating Electric Power (kW)", 0.15),
    ("Ventilation Fan Electric Power (kW)", 0.02),
    ("MELs Electric Power (kW)", 0.02),
    ("TV Electric Power (kW)", 0.02),
    ("Refrigerator Electric Power (kW)", 0.02),
    ("Indoor Lighting Electric Power (kW)", 0.05),
    ("Exterior Lighting Electric Power (kW)", 0.10),
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


# ---------------------------------------------------------------------------
# 7-day performance + parity benchmark
# ---------------------------------------------------------------------------

BENCH_DURATION_H = 7 * 24  # 1 week


def _run_ochre_7d() -> tuple[dict[str, float], float, float]:
    """Run OCHRE for 7 days, return (kwh_dict, init_elapsed, sim_elapsed)."""
    import time

    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))
    from ochre import Dwelling as OchreDwelling

    t0 = time.perf_counter()
    dwelling = OchreDwelling(
        name="bench_ochre_7d",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=BENCH_DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=6,
        save_results=False,
    )
    init_elapsed = time.perf_counter() - t0

    t1 = time.perf_counter()
    df, _metrics, _hourly = dwelling.simulate()
    sim_elapsed = time.perf_counter() - t1

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            result[col] = float((df[col] * time_res_h).sum())
    return result, init_elapsed, sim_elapsed


def _run_hares_7d() -> tuple[dict[str, float], float, float]:
    """Run HARES for 7 days, return (kwh_dict, init_elapsed, sim_elapsed)."""
    import time

    from ochre_next import Dwelling as HaresDwelling

    t0 = time.perf_counter()
    dwelling = HaresDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=START_HARES,
        time_res_s=TIME_RES_MIN * 60,
        duration_s=BENCH_DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        resample_overrides=OCHRE_COMPAT_RESAMPLE,
    )
    init_elapsed = time.perf_counter() - t0

    t1 = time.perf_counter()
    df = dwelling.simulate()
    sim_elapsed = time.perf_counter() - t1

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            result[col] = float(df[col].sum()) * time_res_h
    return result, init_elapsed, sim_elapsed


def test_7day_benchmark():
    """7-day performance benchmark: energy parity + execution speed."""
    ochre_kwh, ochre_init, ochre_sim = _run_ochre_7d()
    hares_kwh, hares_init, hares_sim = _run_hares_7d()

    print("\n" + "=" * 90)
    print(f"7-DAY BENCHMARK — {BENCH_DURATION_H}h at {TIME_RES_MIN}-min resolution ({BENCH_DURATION_H * 60} steps)")
    print("=" * 90)

    # Performance
    print(f"\n  PERFORMANCE:")
    print(f"    {'':30} {'OCHRE':>10} {'HARES':>10} {'Speedup':>10}")
    print(f"    {'Init time (s)':30} {ochre_init:10.3f} {hares_init:10.3f} {ochre_init / max(hares_init, 1e-9):10.1f}x")
    print(f"    {'Sim time (s)':30} {ochre_sim:10.3f} {hares_sim:10.3f} {ochre_sim / max(hares_sim, 1e-9):10.1f}x")
    total_ochre = ochre_init + ochre_sim
    total_hares = hares_init + hares_sim
    print(f"    {'Total (s)':30} {total_ochre:10.3f} {total_hares:10.3f} {total_ochre / max(total_hares, 1e-9):10.1f}x")
    steps = BENCH_DURATION_H * 60 // TIME_RES_MIN
    print(f"    {'Steps/sec (sim only)':30} {steps / max(ochre_sim, 1e-9):10.0f} {steps / max(hares_sim, 1e-9):10.0f}")

    # Energy parity
    print(f"\n  ENERGY PARITY (kWh over {BENCH_DURATION_H}h):")
    print(f"    {'Column':<55} {'OCHRE':>8} {'HARES':>8} {'Diff%':>8}")
    print("    " + "-" * 80)

    for col in sorted(ochre_kwh):
        o_val = ochre_kwh[col]
        h_val = _resolve_hares_kwh(col, hares_kwh)
        if h_val is None:
            continue
        if abs(o_val) < 1e-6 and abs(h_val) < 1e-6:
            continue
        d_pct = (h_val - o_val) / o_val * 100 if abs(o_val) > 1e-9 else 0.0
        print(f"    {col:<55} {o_val:8.2f} {h_val:8.2f} {d_pct:+7.1f}%")
