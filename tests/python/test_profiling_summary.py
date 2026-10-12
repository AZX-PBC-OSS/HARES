"""Tests for the profiling-summary binding contract in PyDwelling."""

from pathlib import Path

import pytest
from hares import SimulationConfig

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"
CZ2A = ROOT / "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh"


def _cz2a_dwelling():
    from hares import Dwelling

    dw = Dwelling.from_hpxml(str(CZ2A / "building.xml"), str(CZ2A / "schedule.csv"), str(CZ2A / "weather.epw"), defaults=str(HARES_DEFAULTS), bldg_id=1, config=SimulationConfig(start_time="2023-01-01T00:00:00-07:00", duration=3600, time_res=3600,master_seed=0))
    dw.initialize()
    return dw


def test_profiling_summary_raises_without_the_feature():
    """Without the `profiling` feature the binding refuses to report timings."""
    dw = _cz2a_dwelling()

    with pytest.raises(
        NotImplementedError,
        match=r"profiling.*maturin develop --release --features profiling",
    ):
        dw.profiling_summary()
