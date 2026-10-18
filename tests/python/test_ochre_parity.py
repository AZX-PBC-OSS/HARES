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
HARES_DEFAULTS = ROOT / "python" / "hares" / "defaults"

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

# The 24 h window the one-hour window hides gaps from (midnight local).
START_LOCAL_24H = dt.datetime(2019, 5, 5, 0, 0)  # noqa: DTZ001
START_HARES_24H = "2019-05-05T00:00:00"
DURATION_H_24 = 24

# OCHRE seeds numpy's global RNG only when given a seed or an output path;
# unseeded, its stochastic equipment makes the reference differ run to run.
OCHRE_SEED = 42

# OCHRE zero-order holds every weather field onto the run's grid (pandas
# resample().ffill()). The engine has no per-field resampling overrides: the
# HARES side reads the weather this package's adapter has already held
# (hares.adapters.ochre.hold_weather), so the engine's own resampling
# methods never enter the comparison.
def _held_weather(output_dir: Path, time_res_s: int) -> str:
    from hares.adapters.ochre import hold_weather

    held = output_dir / f"weather_ochre_hold_{time_res_s}s.epw"
    held.write_text(hold_weather(WEATHER, time_res_s), encoding="utf-8")
    return str(held)


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


def _run_ochre_24h() -> dict[str, float]:
    """Run OCHRE and return {column_name: kWh} for the 24-hour window."""
    from ochre import Dwelling as OchreDwelling

    dwelling = OchreDwelling(
        name="parity_24h",
        start_time=START_LOCAL_24H,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H_24),
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


def _frame_reports(verbosity: int) -> list:
    """One verbosity memory report: the frame simulate() returns."""
    from hares import Destination, ReportSpec, Selection

    return [ReportSpec("frame", Selection.verbosity(verbosity), Destination.memory())]


def _run_hares_simulate(output_dir: Path) -> dict[str, float]:
    """Run HARES via simulate() and parse the DataFrame for column kWh.

    The comparison holds both sides cold: OCHRE runs no initialization
    here (`initialization_time` is None, so its Dwelling simulates from
    its own initial state) and the HARES side holds its warm-up disabled
    for the same premise (the warm-up defaults on everywhere else). The
    warm-up's effect on the initial condition is the goldens' own
    movement, measured in the ledger's measurement log; comparing a
    warmed HARES state against a cold OCHRE state would measure the two
    initializations against each other, not the physics."""
    from hares import Dwelling as HaresDwelling
    from hares import SimulationConfig

    config = SimulationConfig.from_toml(
        "start_time = \"2023-05-05T13:00:00-07:00\"\n"
        "duration = 3600\n"
        "time_res = 60\n"
        "master_seed = 42\n"
        "warmup = \"disabled\"\n"
        "[[reports]]\n"
        "name = \"frame\"\n"
        "selection = { verbosity = 6 }\n"
        "destination = \"memory\"\n"
    )
    dwelling = HaresDwelling.from_hpxml(HPXML, SCHEDULE, _held_weather(output_dir, TIME_RES_MIN * 60), defaults=str(HARES_DEFAULTS), config=config)
    dwelling.initialize()
    df = dwelling.simulate().reports[0].table()

    time_res_h = TIME_RES_MIN / 60.0
    result: dict[str, float] = {}
    for col in df.columns:
        if col.endswith(("(kW)", "(therms/hour)")):
            result[col] = float(df[col].sum()) * time_res_h
    return result


def _run_hares_simulate_24h(output_dir: Path) -> dict[str, float]:
    """Run HARES via simulate() and parse the DataFrame for the 24-hour
    window, under the same cold-both-sides premise as the one-hour
    comparison (see `_run_hares_simulate`)."""
    from hares import Dwelling as HaresDwelling
    from hares import SimulationConfig

    config = SimulationConfig.from_toml(
        "start_time = \"2023-05-05T00:00:00-07:00\"\n"
        "duration = 86400\n"
        "time_res = 60\n"
        "master_seed = 42\n"
        "warmup = \"disabled\"\n"
        "[[reports]]\n"
        "name = \"frame\"\n"
        "selection = { verbosity = 6 }\n"
        "destination = \"memory\"\n"
    )
    dwelling = HaresDwelling.from_hpxml(HPXML, SCHEDULE, _held_weather(output_dir, TIME_RES_MIN * 60), defaults=str(HARES_DEFAULTS), config=config)
    dwelling.initialize()
    df = dwelling.simulate().reports[0].table()

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
from hares import SimulationConfig


@pytest.fixture(scope="module")
def ochre_results() -> dict[str, float]:
    return _run_ochre()


@pytest.fixture(scope="module")
def hares_results(tmp_path_factory: pytest.TempPathFactory) -> dict[str, float]:
    return _run_hares_simulate(tmp_path_factory.mktemp("hares_parity"))


@pytest.fixture(scope="module")
def ochre_results_24h() -> dict[str, float]:
    return _run_ochre_24h()


@pytest.fixture(scope="module")
def hares_results_24h(tmp_path_factory: pytest.TempPathFactory) -> dict[str, float]:
    return _run_hares_simulate_24h(tmp_path_factory.mktemp("hares_parity_24h"))


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
# The Lighting mark came off (2026-10-07): columns.rs's LIGHTING arm tags
# "Exterior Lighting", the aggregate holds indoor plus exterior, and the case
# passes within the tolerance.
PARITY_XFAILS: dict[str, str] = {}


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


# ---------------------------------------------------------------------------
# 24 h window (seeded both sides)
#
# One hour cannot see gaps that build up over a day. Measured over the 24 h
# from 2019-05-05 00:00 at the 2026-10-07 physics tip, the schedule-driven
# end uses agree to floating-point rounding (worst measured relative error
# 1.9e-14, the Other aggregate), and the end uses below differ because OCHRE
# deviates from the reference:
#
# - Water Heating: OCHRE 1.008 kWh against HARES 11.550 kWh (11.5x). OCHRE's
#   tank never receives its fixtures' hot-water draw: its schedule builder
#   emits the fixtures column as "Water Fixtures (L/min)"
#   (ochre/utils/schedule.py:47, convert_water_column at :350-368) while its
#   tank reads "Water Heating (L/min)" (ochre/Models/Water.py:179, :287), so
#   the draw silently vanishes; only the dishwasher and clothes washer
#   columns reach the tank (0.0 for the washer this day). HARES keeps the
#   reference draw (DIVERGENCES D-017): the fixtures' 256 L/day at the
#   40.6 C delivery temperature is 8.1 kWh of delivered enthalpy, plus the
#   tank's standby and the appliances' draws (whose daily volumes follow
#   OpenStudio-HPXML's cycles rule, not OCHRE's older equations).
# - HVAC Heating: HARES 28.070 kWh against OCHRE's 30.997 kWh (-9.4%). The
#   defrost penalty is applied by one model (the discrete cycle resolves
#   the continuous average into events, and the defrost draw replaces the
#   heating draw during the events instead of stacking under it), and the
#   two envelope models put the same constructions' capacitance in
#   different places (DIVERGENCES D-012's class), so the thermostat's
#   duty integrates differently over a day.
# - Other Electric: HARES 7.356 kWh against OCHRE's 7.409 kWh (-0.7%). The
#   wet appliances' annual energy and hot water follow OpenStudio-HPXML's
#   cycles rule (operational in residents, Addendum 81 asset otherwise)
#   where OCHRE runs its own older cycle equations over the occupancy-
#   adjusted bedroom count; the columns' shapes are the same schedule.
# ---------------------------------------------------------------------------

# Measured 2026-10-08, both sides seeded 42 (kWh); the HVAC and Total
# HARES records re-captured 2026-10-17 for the defrost consolidation (the
# penalty applied by one model: the discrete cycle's events concentrate the
# continuous average and the defrost draw replaces the heating draw during
# the events, so the heating draw fell 32.5037 to 28.0696 kWh and the total
# 55.0445 to 50.6104 kWh).
PARITY_24H_HEATING_OCHRE_KWH = 30.9974
PARITY_24H_HEATING_HARES_KWH = 28.0696
PARITY_24H_WATER_HEATING_OCHRE_KWH = 1.0083
PARITY_24H_WATER_HEATING_HARES_KWH = 11.5501
PARITY_24H_TOTAL_OCHRE_KWH = 43.0500
PARITY_24H_TOTAL_HARES_KWH = 50.6104

PARITY_24H_XFAILS: dict[str, str] = {
    "HVAC Heating Electric Power (kW)": (
        "OCHRE 30.997 kWh against HARES 28.070 kWh over 24 h (-9.4%): the "
        "defrost penalty is applied by one model (the discrete cycle's events "
        "concentrate the continuous average, and the defrost draw replaces "
        "the heating draw during the events instead of stacking under it) "
        "and the two envelope models' capacitance placement differs "
        "(DIVERGENCES D-012's class)"
    ),
    "Water Heating Electric Power (kW)": (
        "OCHRE 1.008 kWh against HARES 11.550 kWh over 24 h (11.5x): OCHRE's tank "
        "never receives the fixtures draw (schedule.py:47 names the column Water "
        "Fixtures (L/min); Water.py:287 reads Water Heating (L/min)); HARES keeps "
        "the reference draw (DIVERGENCES D-017)"
    ),
    "Other Electric Power (kW)": (
        "OCHRE 7.409 kWh against HARES 7.356 kWh over 24 h (-0.7%): the wet "
        "appliances' energy and hot water follow OpenStudio-HPXML's cycles rule "
        "where OCHRE runs its own older cycle equations over the occupancy-"
        "adjusted bedroom count"
    ),
    "Total Electric Power (kW)": (
        "OCHRE 43.050 kWh against HARES 50.610 kWh over 24 h: the sum of the "
        "three declared end-use divergences above"
    ),
}


def _parity_24h_cases() -> list[object]:
    """Build the 24 h parametrize cases with the declared divergences' marks."""
    cases: list[object] = []
    for col in PARITY_COLUMNS:
        reason = PARITY_24H_XFAILS.get(col)
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


@pytest.mark.parametrize("col", _parity_24h_cases())
def test_parity_24h(
    col: str,
    ochre_results_24h: dict[str, float],
    hares_results_24h: dict[str, float],
) -> None:
    o_val, h_val = _lookup(col, ochre_results_24h, hares_results_24h)

    if o_val == 0.0:
        assert abs(h_val) <= ZERO_ABS_TOL_KWH, f"{col}: OCHRE 0 kWh but HARES {h_val!r} kWh"
        return

    rel_err = abs(h_val - o_val) / abs(o_val)
    assert rel_err <= AGREEMENT_REL_TOL, (
        f"{col}: relative error {rel_err:.3e} exceeds {AGREEMENT_REL_TOL:.0e} "
        f"(OCHRE={o_val!r} kWh, HARES={h_val!r} kWh)"
    )


def test_parity_24h_records(ochre_results_24h: dict[str, float], hares_results_24h: dict[str, float]) -> None:
    """The 24 h xfail reasons' measured pairs cannot go stale.

    These records bind the strict xfails' quoted values: a moved pair more
    than 2% off its record fails here, outside the marks.
    """
    for col, o_record, h_record in [
        (
            "HVAC Heating Electric Power (kW)",
            PARITY_24H_HEATING_OCHRE_KWH,
            PARITY_24H_HEATING_HARES_KWH,
        ),
        (
            "Water Heating Electric Power (kW)",
            PARITY_24H_WATER_HEATING_OCHRE_KWH,
            PARITY_24H_WATER_HEATING_HARES_KWH,
        ),
        (
            "Total Electric Power (kW)",
            PARITY_24H_TOTAL_OCHRE_KWH,
            PARITY_24H_TOTAL_HARES_KWH,
        ),
    ]:
        o_val, h_val = _lookup(col, ochre_results_24h, hares_results_24h)
        for name, measured, record in [
            (f"{col} OCHRE", o_val, o_record),
            (f"{col} HARES", h_val, h_record),
        ]:
            rel = abs(measured - record) / abs(record)
            assert rel <= 0.02, (
                f"{name}: measured {measured:.4f} kWh is {rel:.1%} off its "
                f"record {record:.4f} kWh; re-capture the 24 h record with the "
                f"delta and the cause"
            )
