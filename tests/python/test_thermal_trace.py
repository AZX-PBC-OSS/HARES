"""48-hour OCHRE thermal trace comparison.

Requires the ochre dependency group and the extension built with
    uv run maturin develop --release
(the tracked columns need no extra cargo feature).

`import ochre` resolves to the vendors/OCHRE submodule via pyproject's
pytest pythonpath.
"""

from __future__ import annotations

import datetime as dt
import warnings
from pathlib import Path

import numpy as np
import polars as pl
import pytest
from ochre_names import hares_columns_for
from ochre_records import check_record
from ochre_units import register_removed_units

pytestmark = pytest.mark.ochre

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

# May 5 2019, midnight local (Denver LST = UTC-7)
START_LOCAL = dt.datetime(2019, 5, 5, 0, 0)  # noqa: DTZ001 (naive local for OCHRE)
START_HARES = "2019-05-05T00:00:00"
DURATION_H = 48
TIME_RES_MIN = 1
TOTAL_STEPS = DURATION_H * 60  # 2880

EXPECTED_OCHRE_VERSION = "0.9.2"

# OCHRE seeds numpy's global RNG only when given a seed or an output path;
# unseeded, its stochastic equipment makes the reference differ run to run.
OCHRE_SEED = 42

# The ochre dependency group is required: tests/python/conftest.py collects
# this module only in runs that select the ochre marker, where a broken OCHRE
# import is a collection error, not a skip.
import ochre as ochre_mod

# OCHRE's infiltration model converts with a unit name pint 0.25 removed from
# the default registry; re-register it before any dwelling is built.
from ochre.utils import units as ochre_unit_registry

register_removed_units(ochre_unit_registry.ureg)

import pandas as pd

if ochre_mod.__version__ != EXPECTED_OCHRE_VERSION:
    pytest.fail(
        f"Expected OCHRE {EXPECTED_OCHRE_VERSION}, got {ochre_mod.__version__}. "
        "Update vendors/OCHRE or adjust EXPECTED_OCHRE_VERSION."
    )


# ---------------------------------------------------------------------------
# Column names (OCHRE convention; HARES should match at verbosity >= 6)
# ---------------------------------------------------------------------------

COL_INDOOR_TEMP = "Temperature - Indoor (C)"
COL_OUTDOOR_TEMP = "Temperature - Outdoor (C)"
COL_HVAC_HEAT_KW = "HVAC Heating Electric Power (kW)"
COL_HVAC_COOL_KW = "HVAC Cooling Electric Power (kW)"
COL_WINDOW_SOLAR = "Window Transmitted Solar Gain (W)"
COL_INFILTRATION = "Infiltration Heat Gain - Indoor (W)"
COL_INTERNAL_GAIN = "Internal Heat Gain - Indoor (W)"

# All columns the tracked comparisons resolve, on both simulators' frames.
TRACKED_COLS: list[str] = [
    COL_INDOOR_TEMP,
    COL_OUTDOOR_TEMP,
    COL_HVAC_HEAT_KW,
    COL_HVAC_COOL_KW,
    COL_WINDOW_SOLAR,
    COL_INFILTRATION,
    COL_INTERNAL_GAIN,
]


def _hares_col(ochre_col: str) -> str:
    """Return the single HARES output column for an OCHRE column name.

    The shared map (tests/python/ochre_names.py) carries the OCHRE to HARES
    mappings; every tracked column resolves to one HARES column, so a
    sum-valued mapping here is a caller error. TypeError rather than
    AssertionError because the xfail marks below expect only the divergence
    assertion: any other error must fail the test even under the mark.
    """
    columns = hares_columns_for(ochre_col)
    if isinstance(columns, list):
        raise TypeError(
            f"OCHRE column '{ochre_col}' maps to a sum of {len(columns)} HARES "
            "columns, not a single column"
        )
    return columns


# Components compared by _divergence_detector
COMPONENT_COLS = [
    COL_WINDOW_SOLAR,
    COL_INFILTRATION,
    COL_INTERNAL_GAIN,
    COL_HVAC_HEAT_KW,
    COL_HVAC_COOL_KW,
]


# ---------------------------------------------------------------------------
# Utilities
# ---------------------------------------------------------------------------


def _col_to_numpy_ochre(df: pd.DataFrame, col: str) -> np.ndarray | None:
    """Return column as float64 array, or None if absent."""
    if col not in df.columns:
        return None
    return df[col].to_numpy(dtype=np.float64, na_value=0.0)


def _col_to_numpy_hares(df: pl.DataFrame, col: str) -> np.ndarray | None:
    """Return column as float64 array, or None if absent."""
    if col not in df.columns:
        return None
    return df[col].cast(pl.Float64).fill_null(0.0).to_numpy()


def _find_col(
    df_ochre: pd.DataFrame, df_hares: pl.DataFrame, col: str
) -> tuple[np.ndarray | None, np.ndarray | None]:
    """Return (ochre_arr, hares_arr) for a column, warning if absent in either."""
    o = _col_to_numpy_ochre(df_ochre, col)
    h = _col_to_numpy_hares(df_hares, _hares_col(col))
    if o is None:
        warnings.warn(
            f"Column '{col}' absent in OCHRE output -- skipping", stacklevel=3
        )
    if h is None:
        warnings.warn(
            f"Column '{col}' absent in HARES output -- skipping", stacklevel=3
        )
    return o, h


# ---------------------------------------------------------------------------
# Divergence detector helper
# ---------------------------------------------------------------------------


def _divergence_detector(ochre_df: pd.DataFrame, hares_df: pl.DataFrame) -> None:
    """Find the first step where any tracked component diverges > 5% and print diagnostics."""
    print("\n--- Divergence Detector ---")
    found_step: int | None = None
    found_col: str | None = None

    for col in COMPONENT_COLS:
        o_arr, h_arr = _find_col(ochre_df, hares_df, col)
        if o_arr is None or h_arr is None:
            continue
        n = min(len(o_arr), len(h_arr))
        for step in range(n):
            ref = abs(o_arr[step])
            if ref < 1.0:  # skip near-zero steps for relative comparison
                continue
            rel_err = abs(h_arr[step] - o_arr[step]) / ref
            if rel_err > 0.05:
                if found_step is None or step < found_step:
                    found_step = step
                    found_col = col
                break  # earliest divergence within this column found

    if found_step is None:
        print("  No component divergence > 5% detected in tracked columns.")
        return

    elapsed_min = found_step * TIME_RES_MIN
    print(
        f"  First divergence > 5% at step {found_step} (t={elapsed_min} min, {elapsed_min // 60:02d}:{elapsed_min % 60:02d})"
    )
    print(f"  Leading divergent column: {found_col}")
    print()
    print(f"  {'Column':<50} {'OCHRE':>10} {'HARES':>10} {'Δ%':>8}")
    print("  " + "-" * 80)
    for col in COMPONENT_COLS:
        o_arr, h_arr = _find_col(ochre_df, hares_df, col)
        if o_arr is None or h_arr is None:
            print(f"  {col:<50} {'N/A':>10} {'N/A':>10} {'N/A':>8}")
            continue
        step = min(found_step, len(o_arr) - 1, len(h_arr) - 1)
        o_val = o_arr[step]
        h_val = h_arr[step]
        ref = abs(o_val)
        d_str = f"{(h_val - o_val) / ref * 100:+.1f}%" if ref > 1e-9 else "~0"
        print(f"  {col:<50} {o_val:>10.3f} {h_val:>10.3f} {d_str:>8}")

    print()
    print("  Diagnostic guidance:")
    if found_col == COL_WINDOW_SOLAR:
        print(
            "    Solar divergence → check solar geometry, window U/SHGC, or transmitted fraction."
        )
    elif found_col == COL_INFILTRATION:
        print(
            "    Infiltration divergence → check dry-air density or zone pressure coupling."
        )
    elif found_col in (COL_HVAC_HEAT_KW, COL_HVAC_COOL_KW):
        print("    HVAC divergence → check solve_for_output_input or COP calculation.")
    elif found_col == COL_INTERNAL_GAIN:
        print(
            "    Internal gain divergence → check schedule interpolation or occupancy model."
        )
    else:
        print("    LWR divergence → check β / view factor in 4-component LWR model.")
    print("--- End Divergence Detector ---\n")


# ---------------------------------------------------------------------------
# Fixtures (module-scoped -- expensive 48h runs cached once per session)
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def ochre_48h() -> pd.DataFrame:
    """Run OCHRE for 48h and return the per-step pandas DataFrame."""
    from ochre import Dwelling as OchreDwelling

    dwelling = OchreDwelling(
        name="thermal_trace",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=9,
        save_results=False,
        seed=OCHRE_SEED,
    )
    result = dwelling.simulate()
    assert isinstance(result, tuple) and len(result) == 3, (
        "OCHRE simulate must return (df, metrics_by_end_use, ...)"
    )
    df = result[0]
    assert isinstance(df, pd.DataFrame), "OCHRE simulate must return a results DataFrame"
    return df


@pytest.fixture(scope="module")
def hares_48h(tmp_path_factory: pytest.TempPathFactory) -> pl.DataFrame:
    """Run HARES for 48h and return the per-step polars DataFrame."""
    from ochre_next import Dwelling as HaresDwelling

    dwelling = HaresDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=START_HARES,
        time_res_s=TIME_RES_MIN * 60,
        duration_s=DURATION_H * 3600,
        output_verbosity=9,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
        write_output=True,
        output_path=str(tmp_path_factory.mktemp("thermal_trace") / "hares_thermal_trace.csv"),
    )
    dwelling.initialize()
    return dwelling.simulate()


# ---------------------------------------------------------------------------
# test_output_columns_present
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_output_columns_present(
    ochre_48h: pd.DataFrame, hares_48h: pl.DataFrame
) -> None:
    """Every tracked column must resolve on both 48h frames.

    This test deliberately carries no xfail mark: the three comparisons below
    are xfail(strict=True) and their in-test presence asserts stay as defense
    in depth, but inside an xfail those asserts report as the declared XFAIL,
    so a column disappearing from either simulator's output schema would be
    swallowed there. Here a missing column fails loudly.
    """
    missing: list[str] = []
    for col in TRACKED_COLS:
        if _col_to_numpy_ochre(ochre_48h, col) is None:
            missing.append(f"OCHRE: '{col}'")
        if _col_to_numpy_hares(hares_48h, _hares_col(col)) is None:
            missing.append(f"HARES: '{_hares_col(col)}'")
    assert not missing, f"Tracked output columns missing: {', '.join(missing)}"


# ---------------------------------------------------------------------------
# Measured divergences
#
# HARES misses each comparison's threshold below and the cause is not yet
# triaged; the strict xfails declare that. The values measured against the
# seeded OCHRE reference are recorded here and checked first in each test
# (ochre_records.check_record), so the xfail reasons, built from these
# records, cannot go stale.
#
# The records moved twice: once when the attic, zone-volume, terrain,
# roughness and temperature-capacitance physics landed (every trace metric
# roughly halved toward OCHRE), and once when the initial zone humidity
# followed the weather's outdoor humidity ratio at initialization (the
# EnergyPlus zone init, HeatBalanceSurfaceManager.cc:2418-2419) instead of a
# fixed 0.008 kg/kg: the moisture balance starts from the weather the run
# starts in and the trace metrics moved toward OCHRE again. What remains is
# still untriaged.
# ---------------------------------------------------------------------------

TRACE_WORST_6H_C = 1.3243
TRACE_MEAN_48H_C = 0.3938
TRACE_HEATING_OCHRE_KWH = 47.052
TRACE_HEATING_HARES_KWH = 30.0599
OVERNIGHT_WORST_C = 1.5057
OVERNIGHT_MEAN_C = 0.3921
SOLAR_MEAN_RATIO = 1.3048
SOLAR_WORST_REL_ERR = 0.3151

UNTRIAGED = "The divergence is not yet triaged to a cause."


# ---------------------------------------------------------------------------
# test_48h_thermal_trace
# ---------------------------------------------------------------------------


@pytest.mark.slow
@pytest.mark.xfail(
    reason=(
        f"HARES's zone temperature diverges from OCHRE's: worst step of the first "
        f"6 h {TRACE_WORST_6H_C:.2f} °C against the 0.1 °C threshold, 48 h mean "
        f"{TRACE_MEAN_48H_C:.2f} °C against 0.5 °C; 48 h heating {TRACE_HEATING_HARES_KWH:.1f} "
        f"kWh against OCHRE's {TRACE_HEATING_OCHRE_KWH:.1f} kWh. {UNTRIAGED}"
    ),
    raises=AssertionError,
    strict=True,
)
def test_48h_thermal_trace(ochre_48h: pd.DataFrame, hares_48h: pl.DataFrame) -> None:
    """Per-step zone temperature and cumulative HVAC energy parity over 48 hours."""
    o_temp = _col_to_numpy_ochre(ochre_48h, COL_INDOOR_TEMP)
    h_temp = _col_to_numpy_hares(hares_48h, COL_INDOOR_TEMP)

    assert o_temp is not None, f"OCHRE missing '{COL_INDOOR_TEMP}'"
    assert h_temp is not None, f"HARES missing '{COL_INDOOR_TEMP}'"

    n = min(len(o_temp), len(h_temp))
    assert n >= TOTAL_STEPS, (
        f"Expected >= {TOTAL_STEPS} steps, got {n} (OCHRE={len(o_temp)}, HARES={len(h_temp)})"
    )

    first_6h = 6 * 60  # 360 steps
    diff_6h = np.abs(h_temp[:first_6h] - o_temp[:first_6h])
    worst_step_6h = int(np.argmax(diff_6h))
    worst_val_6h = float(diff_6h[worst_step_6h])
    mean_abs_diff = float(np.mean(np.abs(h_temp[:n] - o_temp[:n])))

    time_res_h = TIME_RES_MIN / 60.0
    o_heat, h_heat = _find_col(ochre_48h, hares_48h, COL_HVAC_HEAT_KW)
    assert o_heat is not None, f"OCHRE output has no column '{COL_HVAC_HEAT_KW}'"
    assert h_heat is not None, (
        f"HARES output has no column for '{COL_HVAC_HEAT_KW}' "
        f"(looked up as '{_hares_col(COL_HVAC_HEAT_KW)}')"
    )
    o_kwh = float(np.sum(o_heat[:n])) * time_res_h
    h_kwh = float(np.sum(h_heat[:n])) * time_res_h

    check_record("worst 6 h zone temperature divergence (°C)", worst_val_6h, TRACE_WORST_6H_C)
    check_record("48 h mean zone temperature divergence (°C)", mean_abs_diff, TRACE_MEAN_48H_C)
    check_record("OCHRE 48 h heating (kWh)", o_kwh, TRACE_HEATING_OCHRE_KWH)
    check_record("HARES 48 h heating (kWh)", h_kwh, TRACE_HEATING_HARES_KWH)

    # --- Print comparison table: first 10 steps + every 60th step (hourly) ---
    print(f"\n{'=' * 72}")
    print("OCHRE vs HARES -- 48h thermal trace (BEopt, May 5 2019, Denver)")
    print(f"{'=' * 72}")
    print(
        f"  {'Step':>5}  {'Time':>6}  {'OCHRE T_in':>10}  {'HARES T_in':>10}  {'ΔT (K)':>8}"
    )
    print("  " + "-" * 50)

    sample_steps = list(range(10)) + list(range(60, TOTAL_STEPS, 60))
    for step in sample_steps:
        if step >= n:
            break
        elapsed_min = step * TIME_RES_MIN
        time_str = f"{elapsed_min // 60:02d}:{elapsed_min % 60:02d}"
        delta = h_temp[step] - o_temp[step]
        print(
            f"  {step:>5}  {time_str:>6}  {o_temp[step]:>10.3f}  {h_temp[step]:>10.3f}  {delta:>+8.3f}"
        )

    print()

    # --- Assert per-step divergence < 0.1°C for first 6 hours (360 steps) ---
    try:
        assert worst_val_6h < 0.1, (
            f"Per-step zone temp divergence {worst_val_6h:.4f}°C at step {worst_step_6h} "
            f"(t={worst_step_6h} min) exceeds 0.1°C threshold for first 6h"
        )
    except AssertionError:
        _divergence_detector(ochre_48h, hares_48h)
        raise

    # --- Assert cumulative divergence < 0.5°C mean absolute diff at 48h ---
    try:
        assert mean_abs_diff < 0.5, (
            f"Cumulative mean absolute zone temp divergence {mean_abs_diff:.4f}°C "
            f"over 48h exceeds 0.5°C threshold"
        )
    except AssertionError:
        _divergence_detector(ochre_48h, hares_48h)
        raise

    print(f"  Zone temp -- worst 6h step: {worst_val_6h:.4f}°C at step {worst_step_6h}")
    print(f"  Zone temp -- mean abs diff 48h: {mean_abs_diff:.4f}°C")

    # --- Assert cumulative HVAC heating kWh within 10% at 48h ---
    print(f"  HVAC heating -- OCHRE: {o_kwh:.3f} kWh, HARES: {h_kwh:.3f} kWh")
    if abs(o_kwh) < 1e-6:
        assert abs(h_kwh) < 0.01, (
            f"OCHRE HVAC heating ≈ 0 kWh but HARES = {h_kwh:.4f} kWh"
        )
    else:
        rel_err = abs(h_kwh - o_kwh) / abs(o_kwh)
        try:
            assert rel_err <= 0.10, (
                f"HVAC heating kWh relative error {rel_err:.1%} exceeds 10% "
                f"(OCHRE={o_kwh:.3f}, HARES={h_kwh:.3f})"
            )
        except AssertionError:
            _divergence_detector(ochre_48h, hares_48h)
            raise


# ---------------------------------------------------------------------------
# test_overnight_losses
# ---------------------------------------------------------------------------


@pytest.mark.slow
@pytest.mark.xfail(
    reason=(
        f"HARES's overnight zone temperature (18:00 to 08:00) diverges from OCHRE's "
        f"by up to {OVERNIGHT_WORST_C:.2f} °C against the 0.3 °C threshold "
        f"(mean {OVERNIGHT_MEAN_C:.2f} °C). {UNTRIAGED}"
    ),
    raises=AssertionError,
    strict=True,
)
def test_overnight_losses(ochre_48h: pd.DataFrame, hares_48h: pl.DataFrame) -> None:
    """Nighttime zone temperature: 18:00 to 08:00 (840 steps).

    No solar after sunset, so the zone temperature follows the envelope
    losses (conduction, infiltration, longwave) and the heating system.
    """
    # step 1080 = 18:00 local (18h * 60 min/h), step 1920 = 32h = 08:00 next day
    STEP_START = 18 * 60  # 1080
    STEP_END = 32 * 60  # 1920

    o_temp = _col_to_numpy_ochre(ochre_48h, COL_INDOOR_TEMP)
    h_temp = _col_to_numpy_hares(hares_48h, COL_INDOOR_TEMP)

    assert o_temp is not None, f"OCHRE missing '{COL_INDOOR_TEMP}'"
    assert h_temp is not None, f"HARES missing '{COL_INDOOR_TEMP}'"

    n = min(len(o_temp), len(h_temp))
    assert n >= STEP_END, f"Need at least {STEP_END} steps for overnight slice, got {n}"

    slice_o = o_temp[STEP_START:STEP_END]
    slice_h = h_temp[STEP_START:STEP_END]
    diff = np.abs(slice_h - slice_o)
    worst_offset = int(np.argmax(diff))
    worst_val = float(diff[worst_offset])
    worst_step = STEP_START + worst_offset
    elapsed_min = worst_step * TIME_RES_MIN

    print(f"\n{'=' * 60}")
    print("Overnight losses (18:00 → 08:00) -- zone temp comparison")
    print(f"  Slice steps: {STEP_START}–{STEP_END} ({STEP_END - STEP_START} steps)")
    print(
        f"  Worst divergence: {worst_val:.4f}°C at step {worst_step} "
        f"(t={elapsed_min // 60:02d}:{elapsed_min % 60:02d})"
    )
    mean_val = float(np.mean(diff))
    print(f"  Mean abs diff: {mean_val:.4f}°C")

    check_record("overnight worst zone temperature divergence (°C)", worst_val, OVERNIGHT_WORST_C)
    check_record("overnight mean zone temperature divergence (°C)", mean_val, OVERNIGHT_MEAN_C)

    assert worst_val < 0.3, (
        f"Overnight zone temp divergence {worst_val:.4f}°C at step {worst_step} "
        f"(t={elapsed_min // 60:02d}:{elapsed_min % 60:02d}) exceeds 0.3°C threshold"
    )


# ---------------------------------------------------------------------------
# test_solar_peak
# ---------------------------------------------------------------------------


@pytest.mark.slow
@pytest.mark.xfail(
    reason=(
        f"HARES's window transmitted solar gain over 10:00 to 16:00 is "
        f"{SOLAR_MEAN_RATIO:.3f} times OCHRE's on average, every sampled step "
        f"outside the 5% band, the worst {SOLAR_WORST_REL_ERR:.1%} high. {UNTRIAGED}"
    ),
    raises=AssertionError,
    strict=True,
)
def test_solar_peak(ochre_48h: pd.DataFrame, hares_48h: pl.DataFrame) -> None:
    """Solar peak window: 10:00–16:00 on day 1 (600–960 steps).

    Compare Window Transmitted Solar Gain within 5% at steps where solar > 10 W.
    """
    STEP_START = 10 * 60  # 600
    STEP_END = 16 * 60  # 960

    o_solar, h_solar = _find_col(ochre_48h, hares_48h, COL_WINDOW_SOLAR)

    assert o_solar is not None, f"OCHRE output has no column '{COL_WINDOW_SOLAR}'"
    assert h_solar is not None, f"HARES output has no column '{COL_WINDOW_SOLAR}'"

    n = min(len(o_solar), len(h_solar))
    assert n >= STEP_END, (
        f"Need at least {STEP_END} steps for solar peak slice, got {n}"
    )

    slice_o = o_solar[STEP_START:STEP_END]
    slice_h = h_solar[STEP_START:STEP_END]

    print(f"\n{'=' * 60}")
    print("Solar peak (10:00–16:00 day 1) -- Window Transmitted Solar Gain")
    print(
        f"  {'Step':>5}  {'Time':>5}  {'OCHRE (W)':>10}  {'HARES (W)':>10}  {'Δ%':>7}"
    )
    print("  " + "-" * 45)

    violations: list[tuple[int, float, float, float]] = []
    ratios: list[float] = []
    worst_rel_err = 0.0
    for offset in range(0, STEP_END - STEP_START, 10):
        step = STEP_START + offset
        if step >= n:
            break
        o_val = slice_o[offset]
        h_val = slice_h[offset]
        elapsed_min = step * TIME_RES_MIN
        time_str = f"{elapsed_min // 60:02d}:{elapsed_min % 60:02d}"
        if abs(o_val) > 10.0:
            rel_err = abs(h_val - o_val) / abs(o_val)
            ratios.append(float(h_val / o_val))
            worst_rel_err = max(worst_rel_err, float(rel_err))
            d_str = f"{(h_val - o_val) / o_val * 100:+.1f}%"
            print(
                f"  {step:>5}  {time_str:>5}  {o_val:>10.1f}  {h_val:>10.1f}  {d_str:>7}"
            )
            if rel_err > 0.05:
                violations.append((step, o_val, h_val, rel_err))
        else:
            print(
                f"  {step:>5}  {time_str:>5}  {o_val:>10.1f}  {h_val:>10.1f}  {'(skip)':>7}"
            )

    if not ratios:
        pytest.fail("No sampled step in the solar peak window carries more than 10 W")
    check_record("mean window solar gain ratio HARES/OCHRE", float(np.mean(ratios)), SOLAR_MEAN_RATIO)
    check_record("worst window solar gain relative error", worst_rel_err, SOLAR_WORST_REL_ERR)

    if violations:
        msgs = [
            f"step {s}: OCHRE={o:.1f}W, HARES={h:.1f}W, err={e:.1%}"
            for s, o, h, e in violations
        ]
        raise AssertionError(
            f"Window solar gain exceeds 5% tolerance at {len(violations)} step(s):\n"
            + "\n".join(f"  {m}" for m in msgs)
        )
