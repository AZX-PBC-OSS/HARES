"""OCHRE parity test -- runs both OCHRE and HARES on identical inputs and compares.

Requires:
    uv sync --all-groups (the ochre group supplies OCHRE's dependencies)
    uv run maturin develop -m crates/hares-python/Cargo.toml

`import ochre` resolves to the vendors/OCHRE submodule via pyproject's
pytest pythonpath.
"""

from __future__ import annotations

import datetime as dt
import tempfile
from pathlib import Path

import pandas as pd
import pytest
from ochre_names import resolve_hares_kwh
from ochre_units import register_removed_units

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

# Denver LST = UTC-7.  13:00 local = 20:00 UTC.
# Both OCHRE and HARES take local standard time as the start time.
START_LOCAL = dt.datetime(2019, 5, 5, 13, 0)  # noqa: DTZ001 (naive local for OCHRE)
# HARES takes an ISO string; EPW timezone offset (-7h) is applied internally.
START_HARES = "2019-05-05T13:00:00"
DURATION_H = 1
TIME_RES_MIN = 1

# ZOH resampling for all continuous weather fields -- matches OCHRE's pandas ffill().
# Note: sky_temp is not overridable -- HARES always recomputes it from the
# interpolated inputs (see ResampleOverrides::ochre_compat in hares-io).
OCHRE_COMPAT_RESAMPLE: dict[str, str] = {
    "dry_bulb": "zoh",
    "dew_point": "zoh",
    "rel_humidity": "zoh",
    "pressure": "zoh",
    "infrared": "zoh",
    "ground_temp": "zoh",
    "opaque_sky_cover": "zoh",
}


# ---------------------------------------------------------------------------
# OCHRE runner
# ---------------------------------------------------------------------------


def _run_ochre() -> dict[str, float]:
    """Run OCHRE and return {column_name: kWh} for 1-hour window."""
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
    result = dwelling.simulate()
    assert isinstance(result, tuple) and len(result) == 3, (
        "OCHRE simulate must return (df, metrics_by_end_use, ...)"
    )
    df = result[0]
    assert isinstance(df, pd.DataFrame), "OCHRE simulate must return a results DataFrame"

    time_res_h = TIME_RES_MIN / 60.0
    totals: dict[str, float] = {}
    for col in df.columns:
        if col.endswith(("(kW)", "(therms/hour)")):
            totals[col] = float((df[col] * time_res_h).sum())
    return totals


# ---------------------------------------------------------------------------
# HARES runner
# ---------------------------------------------------------------------------


def _run_hares_simulate() -> dict[str, float]:
    """Run HARES via simulate() and parse the DataFrame for column kWh."""
    pytest.importorskip("polars")
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
        write_output=True,
        output_path=str(Path(tempfile.mkdtemp()) / "hares_parity.csv"),
    )
    dwelling.initialize()
    df = dwelling.simulate()

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith(("(kW)", "(therms/hour)")):
            result[col] = float(df[col].sum()) * time_res_h
    return result


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

# Scoped to the runs whose selector includes the ochre marker (the slow job's
# "slow or ochre"); the default job's "not slow and not ochre" deselects the
# module. Environments without the ochre group still import the module during
# collection, so the guard below reports a module-level skip there.
pytestmark = pytest.mark.ochre

ochre = pytest.importorskip("ochre", reason="OCHRE not installed")

# OCHRE's infiltration model converts with a unit name pint 0.25 removed from
# the default registry; re-register it before any dwelling is built.
import ochre.utils.units  # noqa: E402

register_removed_units(ochre.utils.units.ureg)


@pytest.fixture(scope="module")
def ochre_results() -> dict[str, float]:
    return _run_ochre()


@pytest.fixture(scope="module")
def hares_results() -> dict[str, float]:
    return _run_hares_simulate()


# Tolerances per THERMAL-008 / ASHRAE 140-2023 §5.2.
# Schedule-driven loads: 2% (exact match expected).
# HVAC: 15% (ASHRAE 140 acceptance range for annual heating energy).
# Total: 10% (allows for unimplemented event-driven equipment).
# Water Heating: 5% (thermostatic, matching the HVAC Cooling tier).
# Lighting: 5% (schedule-driven, matching Indoor Lighting).
# Other: 2% (a sum of schedule-driven members).
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
    ("Water Heating Electric Power (kW)", 0.05),
    ("Lighting Electric Power (kW)", 0.05),
    ("Other Electric Power (kW)", 0.02),
]

# Declared divergences: a case whose measured relative error exceeds its
# tolerance stays as a strict xfail citing the measured pair, so a future fix
# that closes the gap turns the XPASS into a visible test failure and the
# mark gets removed. Only the cited assertion is expected; a setup error
# fails the case even under the mark. A missing-column assertion inside an
# xfail-marked case would report as the declared xfail: the tracked columns
# are pinned loudly by test_output_columns_present instead.
PARITY_XFAILS: dict[str, str] = {
    "Lighting Electric Power (kW)": (
        "HARES's Lighting End Use aggregate holds Indoor Lighting only: "
        "columns.rs's LIGHTING arm does not tag 'Exterior Lighting', which "
        "falls to the Other aggregate; measured over the parity hour: OCHRE "
        "0.09256122877254946 kWh vs HARES 0.08545972626253563 kWh, a relative "
        "error of 7.7% against the 5% tolerance"
    ),
}


def _parity_cases() -> list[object]:
    """Build the parametrize cases, carrying the declared divergences' marks."""
    cases: list[object] = []
    for col, tol in PARITY_CHECKS:
        reason = PARITY_XFAILS.get(col)
        if reason is None:
            cases.append(pytest.param(col, tol, id=col))
        else:
            cases.append(
                pytest.param(
                    col,
                    tol,
                    id=col,
                    marks=pytest.mark.xfail(
                        reason=reason, raises=AssertionError, strict=True
                    ),
                )
            )
    return cases


@pytest.mark.parametrize("col,tol", _parity_cases())
def test_parity(
    col: str,
    tol: float,
    ochre_results: dict[str, float],
    hares_results: dict[str, float],
) -> None:
    o_val = ochre_results.get(col)
    assert o_val is not None, f"OCHRE output has no column '{col}'"
    h_val = resolve_hares_kwh(col, hares_results)
    assert h_val is not None, f"HARES output has no column matching '{col}'"

    if abs(o_val) < 1e-9:
        assert abs(h_val) < 1e-3, f"{col}: OCHRE≈0 but HARES={h_val:.4f}"
        return

    rel_err = abs(h_val - o_val) / abs(o_val)
    assert rel_err <= tol, (
        f"{col}: relative error {rel_err:.1%} exceeds tolerance {tol:.0%} "
        f"(OCHRE={o_val:.4f}, HARES={h_val:.4f})"
    )
