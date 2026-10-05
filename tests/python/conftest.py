"""Shared pytest fixtures for HARES tests."""

from __future__ import annotations

import sys
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path

import pytest
from _pytest.mark.expression import Expression
from skip_gate import pytest_addoption, pytest_sessionfinish  # noqa: F401  (hooks)

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

# Modules marked ochre import OCHRE (or its group's xmltodict) at module level
# with no skip guard. Marker selection applies after collection, so they are
# kept out of collection unless the run's -m expression can select an ochre
# test; a run that does select them errors on a broken OCHRE install.
OCHRE_MODULES = frozenset(
    {"test_ochre_parity.py", "test_thermal_trace.py", "test_ashrae_reference.py"}
)


def selects_ochre(markexpr: str) -> bool:
    """Whether a -m expression selects a test marked ochre (alone or also slow)."""
    if not markexpr:
        return True
    expr = Expression.compile(markexpr)

    def matcher(marks: frozenset[str]):
        return lambda name, /, **_: name in marks

    return any(
        expr.evaluate(matcher(marks))
        for marks in (frozenset({"ochre"}), frozenset({"ochre", "slow"}))
    )


def pytest_ignore_collect(collection_path: Path, config: pytest.Config) -> bool | None:
    if collection_path.name in OCHRE_MODULES and not selects_ochre(
        config.option.markexpr
    ):
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
    return name in ("helics", "ochre_next.helics") or name.startswith(("helics.", "ochre_next.helics."))


@pytest.fixture
def restore_helics_modules() -> Iterator[None]:
    """Put back the real ``helics`` and ``ochre_next.helics`` modules after the test.

    For a test that re-imports ``ochre_next.helics`` under a fake ``helics``:
    without it the fake-bound modules stay in ``sys.modules``, and every later
    test in the process that imports them gets the fake.
    """
    import ochre_next

    saved = {name: module for name, module in sys.modules.items() if _is_helics_module(name)}
    package = ochre_next.__dict__.get("helics")
    yield
    for name in [name for name in sys.modules if _is_helics_module(name)]:
        del sys.modules[name]
    sys.modules.update(saved)
    if package is None:
        ochre_next.__dict__.pop("helics", None)
    else:
        ochre_next.__dict__["helics"] = package


def make_dwelling(
    duration_s: int = 300,
    time_res_s: int = 60,
    seed: int = 0,
    output_verbosity: int = 0,
    start_time: str = "2019-01-01T00:00:00",
    write_output: bool = False,
    **kw,
):
    from ochre_next import Dwelling

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=start_time,
        duration_s=duration_s,
        time_res_s=time_res_s,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=seed,
        output_verbosity=output_verbosity,
        write_output=write_output,
        **kw,
    )
    dw.initialize()
    return dw
