"""Shared pytest fixtures for HARES tests."""

from __future__ import annotations

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = ROOT / "python"

if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

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
        **kw,
    )
    dw.initialize()
    return dw
