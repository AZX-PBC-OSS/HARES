"""Shared pytest fixtures for HARES tests."""

from __future__ import annotations

from pathlib import Path
import sys

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
