"""Shared pytest fixtures for HARES tests."""

from __future__ import annotations

import ast
import sys
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path

import pytest
from skip_gate import pytest_addoption, pytest_configure  # noqa: F401  (hooks)

# Every test runs off the network unless marked ``network`` (offline_guard).
pytest_plugins = ["offline_guard_plugin", "pytester"]

ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = ROOT / "python"

if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

# The helics wheel's libzmq exports the C++ standard-library symbols it was
# built against without declaring libstdc++ as a dependency, so importing
# helics before pyarrow poisons symbol resolution and the first pyarrow
# parquet writer segfaults. Importing pyarrow first resolves every symbol
# from libstdc++ and both libraries work. Every worker process imports this
# conftest before any test module, which fixes the order for the whole run;
# the defect and its signal are pinned in test_helics_pyarrow_symbol_order.py.
import pyarrow.parquet  # noqa: F401  (must precede any helics import)

HARES_DEFAULTS = ROOT / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

# The removed-key messages the four site keywords raise on every Python
# constructor, exactly as the Rust removed-keys table spells them. One
# constant: every Python pin of the message reads it, so a wording change
# is one edit, and the pins assert the full text, not a fragment.
REMOVED_SITE_KEYWORD_MESSAGES: dict[str, str] = {
    "latitude": (
        "configuration key 'latitude' was removed: pass site_location = "
        "SiteLocationOverride(latitude_deg=...)"
    ),
    "longitude": (
        "configuration key 'longitude' was removed: pass site_location = "
        "SiteLocationOverride(longitude_deg=...)"
    ),
    "elevation_m": (
        "configuration key 'elevation_m' was removed: pass site_location = "
        "SiteLocationOverride(elevation_m=...)"
    ),
    "utc_offset_h": (
        "configuration key 'utc_offset_h' was removed: pass site_location = "
        "SiteLocationOverride(utc_offset_h=...)"
    ),
}

# The modules marked ochre import OCHRE (or its group's xmltodict) at module
# level, with no skip guard, and marker selection applies only after a module
# is imported. The rule:
# - Found by directory collection, they are left out unless the -m expression
#   can select a test marked ochre, alone or also slow (selects_ochre). The
#   default "not slow and not ochre" leaves them out; "ochre", "not slow" or
#   no expression collects them.
# - Named on the command line, a module is always collected (pytest does not
#   consult pytest_ignore_collect for its own arguments).
# A collected module imports OCHRE, so without the ochre dependency group the
# run fails with a collection error: it never skips.
OCHRE_MODULES = frozenset(
    {"test_ochre_parity.py", "test_thermal_trace.py", "test_ashrae_reference.py"}
)


class _NotPlainMarkerLogic(Exception):
    pass


def _matches(node: ast.expr, marks: frozenset[str]) -> bool:
    match node:
        case ast.Name(id=name):
            return name in marks
        case ast.UnaryOp(op=ast.Not(), operand=operand):
            return not _matches(operand, marks)
        case ast.BoolOp(op=ast.And(), values=values):
            return all(_matches(v, marks) for v in values)
        case ast.BoolOp(op=ast.Or(), values=values):
            return any(_matches(v, marks) for v in values)
        case _:
            raise _NotPlainMarkerLogic


def selects_ochre(markexpr: str) -> bool:
    """Whether a -m expression can select a test marked ochre (alone or also slow).

    Marker names joined by and, or, not and parentheses are evaluated here;
    any other expression (marker keyword arguments, or one pytest itself will
    reject) counts as selecting, so the modules are collected and fail loudly
    rather than being left out.
    """
    if not markexpr.strip():
        return True
    try:
        tree = ast.parse(markexpr, mode="eval")
        return any(
            _matches(tree.body, marks)
            for marks in (frozenset({"ochre"}), frozenset({"ochre", "slow"}))
        )
    except (SyntaxError, _NotPlainMarkerLogic):
        return True


def pytest_ignore_collect(collection_path: Path, config: pytest.Config) -> bool | None:
    markexpr: str = config.getoption("markexpr") or ""
    if collection_path.name in OCHRE_MODULES and not selects_ochre(markexpr):
        return True
    return None


def output_in_test_dir(config: Mapping[str, object], filename: str) -> Callable[..., None]:
    """An autouse fixture pointing ``config["output_path"]`` into each test's ``tmp_path``.

    For a module whose shared run config must write output: bind the result to
    a module-level name and every test in the module writes its own file,
    removed with the test's temporary directory.
    """

    @pytest.fixture(autouse=True)
    def _output_in_test_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setitem(config, "output_path", str(tmp_path / filename))

    return _output_in_test_dir


def _is_helics_module(name: str) -> bool:
    return name in ("helics", "hares.helics") or name.startswith(("helics.", "hares.helics."))


@pytest.fixture
def restore_helics_modules() -> Iterator[None]:
    """Put back the real ``helics`` and ``hares.helics`` modules after the test.

    For a test that re-imports ``hares.helics`` under a fake ``helics``:
    without it the fake-bound modules stay in ``sys.modules``, and every later
    test in the process that imports them gets the fake.
    """
    import hares

    saved = {name: module for name, module in sys.modules.items() if _is_helics_module(name)}
    package = hares.__dict__.get("helics")
    yield
    for name in [name for name in sys.modules if _is_helics_module(name)]:
        del sys.modules[name]
    sys.modules.update(saved)
    if package is None:
        hares.__dict__.pop("helics", None)
    else:
        hares.__dict__["helics"] = package


def make_dwelling(
    duration_s: int = 300,
    time_res_s: int = 60,
    seed: int = 0,
    output_verbosity: int = 0,
    start_time: str = "2019-01-01T00:00:00",
    write_output: bool = False,
    output_path: str | None = None,
    retain_batches: bool = True,
    sim_config=None,
    **kw,
):
    """Build and initialize a dwelling from the committed BEopt inputs.

    The sim config states every field the tests' frames read:
    `retain_batches=True` keeps the recorder's batches in memory, which
    `results()` and `metrics()` read for the full schema. Nothing in the
    engine defaults it on: the config object always carries it.
    """
    from hares import Dwelling, SimulationConfig

    if sim_config is None:
        sim_config = SimulationConfig(
            start_time=start_time,
            duration=duration_s,
            time_res=time_res_s,
            master_seed=seed,
            output_verbosity=output_verbosity,
            write_output=write_output,
            output_path=output_path,
            retain_batches=retain_batches,
        )
    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        config=sim_config,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        **kw,
    )
    dw.initialize()
    return dw
