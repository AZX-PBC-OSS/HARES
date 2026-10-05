"""OCHRE parity test -- runs both OCHRE and HARES on identical inputs and compares.

Requires:
    uv sync --all-groups (the ochre group supplies OCHRE's dependencies)
    uv run maturin develop -m crates/hares-python/Cargo.toml

`import ochre` resolves to the vendors/OCHRE submodule via pyproject's
pytest pythonpath.
"""

from __future__ import annotations

import datetime as dt
from pathlib import Path

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

# OCHRE seeds numpy's global RNG only when given a seed or an output path;
# unseeded, its stochastic equipment makes the reference differ run to run.
OCHRE_SEED = 42

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
        seed=OCHRE_SEED,
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


def _run_hares_simulate(output_dir: Path) -> dict[str, float]:
    """Run HARES via simulate() and parse the DataFrame for column kWh."""
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
        output_path=str(output_dir / "hares_parity.csv"),
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

# Requires the ochre dependency group: tests/python/conftest.py collects this
# module only in runs that select the ochre marker (CI's OCHRE comparison
# job), where a broken OCHRE import is a collection error, not a skip.
pytestmark = pytest.mark.ochre

# OCHRE's infiltration model converts with a unit name pint 0.25 removed from
# the default registry; re-register it before any dwelling is built.
from ochre.utils import units as ochre_unit_registry

register_removed_units(ochre_unit_registry.ureg)

import pandas as pd


@pytest.fixture(scope="module")
def ochre_results() -> dict[str, float]:
    return _run_ochre()


@pytest.fixture(scope="module")
def hares_results(tmp_path_factory: pytest.TempPathFactory) -> dict[str, float]:
    return _run_hares_simulate(tmp_path_factory.mktemp("hares_parity"))


PARITY_COLUMNS: list[str] = [
    "Total Electric Power (kW)",
    "HVAC Cooling Electric Power (kW)",
    "HVAC Heating Electric Power (kW)",
    "Ventilation Fan Electric Power (kW)",
    "MELs Electric Power (kW)",
    "TV Electric Power (kW)",
    "Refrigerator Electric Power (kW)",
    "Indoor Lighting Electric Power (kW)",
    "Exterior Lighting Electric Power (kW)",
    "Water Heating Electric Power (kW)",
    "Lighting Electric Power (kW)",
    "Other Electric Power (kW)",
]

# Over the parity hour both simulators run the same schedules on the same
# resampled weather, and every case that agrees does so to floating-point
# rounding (largest measured relative error 1.8e-15, Exterior Lighting). The
# tolerance holds that agreement with room for summation-order differences
# across platforms, so any change to a compared quantity fails the case.
AGREEMENT_REL_TOL = 1e-9
# HVAC Heating and Water Heating draw nothing in this hour on either side
# (both measured exactly 0.0 kWh); a nonzero HARES value is a change.
ZERO_ABS_TOL_KWH = 1e-12

# Declared divergences: a case whose measured relative error exceeds the
# tolerance stays as a strict xfail citing the measured pair, so a future fix
# that closes the gap turns the XPASS into a visible test failure and the
# mark gets removed. Only the comparison assertion is expected: a missing
# column raises LookupError, which fails the case even under the mark, and
# test_parity_columns_present pins every column without a mark.
PARITY_XFAILS: dict[str, str] = {
    "Lighting Electric Power (kW)": (
        "HARES's Lighting End Use aggregate holds Indoor Lighting only: "
        "columns.rs's LIGHTING arm does not tag 'Exterior Lighting', which "
        "falls to the Other aggregate; measured over the parity hour: OCHRE "
        "0.09256122877254946 kWh vs HARES 0.08545972626253563 kWh, a relative "
        "error of 7.7%, the Exterior Lighting share"
    ),
}


def _parity_cases() -> list[object]:
    """Build the parametrize cases, carrying the declared divergences' marks."""
    cases: list[object] = []
    for col in PARITY_COLUMNS:
        reason = PARITY_XFAILS.get(col)
        if reason is None:
            cases.append(pytest.param(col, id=col))
        else:
            cases.append(
                pytest.param(
                    col,
                    id=col,
                    marks=pytest.mark.xfail(
                        reason=reason, raises=AssertionError, strict=True
                    ),
                )
            )
    return cases


def _lookup(
    col: str, ochre_results: dict[str, float], hares_results: dict[str, float]
) -> tuple[float, float]:
    o_val = ochre_results.get(col)
    if o_val is None:
        raise LookupError(f"OCHRE output has no column '{col}'")
    h_val = resolve_hares_kwh(col, hares_results)
    if h_val is None:
        raise LookupError(f"HARES output has no column matching '{col}'")
    return o_val, h_val


def test_parity_columns_present(
    ochre_results: dict[str, float], hares_results: dict[str, float]
) -> None:
    """Every compared column resolves on both simulators' results."""
    missing: list[str] = []
    for col in PARITY_COLUMNS:
        if col not in ochre_results:
            missing.append(f"OCHRE: '{col}'")
        if resolve_hares_kwh(col, hares_results) is None:
            missing.append(f"HARES: '{col}'")
    assert not missing, f"Parity columns missing: {', '.join(missing)}"


@pytest.mark.parametrize("col", _parity_cases())
def test_parity(
    col: str,
    ochre_results: dict[str, float],
    hares_results: dict[str, float],
) -> None:
    o_val, h_val = _lookup(col, ochre_results, hares_results)

    if o_val == 0.0:
        assert abs(h_val) <= ZERO_ABS_TOL_KWH, f"{col}: OCHRE 0 kWh but HARES {h_val!r} kWh"
        return

    rel_err = abs(h_val - o_val) / abs(o_val)
    assert rel_err <= AGREEMENT_REL_TOL, (
        f"{col}: relative error {rel_err:.3e} exceeds {AGREEMENT_REL_TOL:.0e} "
        f"(OCHRE={o_val!r} kWh, HARES={h_val!r} kWh)"
    )
