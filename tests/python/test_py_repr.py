"""Tests for Python binding __repr__ methods."""

from __future__ import annotations

from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)
SCHEDULE = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Input Files/BEopt_example_schedule.csv"
)


@pytest.fixture(scope="module")
def dwelling():
    from ochre_next import Dwelling as PyDwelling

    dw = PyDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration_s=3600,
        time_res_s=60,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=0,
    )
    dw.initialize()
    return dw


class TestPyRepr:
    def test_dwelling_repr(self, dwelling):
        r = repr(dwelling)
        assert "Dwelling" in r
        assert "42" in r
        assert "initialized" in r

    def test_timesteps_repr(self, dwelling):
        it = dwelling.timesteps()
        r = repr(it)
        assert "TimestepsIter" in r
        assert "step=" in r
        assert "/" in r


class TestBatteryRepr:
    def test_battery_repr(self):
        from ochre_next import Battery

        b = Battery(name="test_batt", capacity_kwh=10.0)
        r = repr(b)
        assert "Battery" in r
        assert "test_batt" in r
        assert "10" in r


class TestPvSoilingConfigRepr:
    def test_pv_soiling_config_repr(self):
        from ochre_next import PvSoilingConfig

        c = PvSoilingConfig()
        r = repr(c)
        assert "PvSoilingConfig" in r
        # repr must expose at least one numeric field value, not just the class name
        import re
        assert re.search(r"\d+\.?\d*", r), "repr contains no numeric field value"


class TestPvRepr:
    def test_pv_repr(self):
        from ochre_next import PV

        pv = PV(name="test_pv", capacity_kw=5.0, tilt=30.0, azimuth=180.0)
        r = repr(pv)
        assert "PV" in r
        assert "test_pv" in r
        assert "5" in r
        assert "30" in r
        assert "180" in r


class TestEvRepr:
    def test_ev_repr(self):
        from ochre_next import EV

        ev = EV(name="test_ev", capacity_kwh=75.0, max_charging_kw=11.0)
        r = repr(ev)
        assert "EV" in r
        assert "test_ev" in r
        assert "75" in r
