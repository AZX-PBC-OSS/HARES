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
        duration=3600,
        time_res=60,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=0,
    )
    dw.initialize()
    return dw


class TestPyRepr:
    def test_dwelling_repr_contains_class_name(self, dwelling):
        r = repr(dwelling)
        assert r
        assert "Dwelling" in r

    def test_dwelling_repr_contains_bldg_id(self, dwelling):
        r = repr(dwelling)
        assert "bldg_id" in r
        assert "42" in r

    def test_dwelling_repr_contains_initialized(self, dwelling):
        r = repr(dwelling)
        assert "initialized" in r


class TestTimestepsIterRepr:
    def test_timesteps_repr_contains_class_name(self, dwelling):
        it = dwelling.timesteps()
        r = repr(it)
        assert r
        assert "TimestepsIter" in r

    def test_timesteps_repr_shows_step_progress(self, dwelling):
        it = dwelling.timesteps()
        r = repr(it)
        assert "step=" in r
        assert "/" in r


class TestBatteryRepr:
    def test_battery_repr_contains_class_name(self):
        from ochre_next._hares import Battery

        b = Battery(name="test_batt", capacity_kwh=10.0)
        r = repr(b)
        assert r
        assert "Battery" in r

    def test_battery_repr_contains_name(self):
        from ochre_next._hares import Battery

        b = Battery(name="test_batt", capacity_kwh=10.0)
        r = repr(b)
        assert "test_batt" in r

    def test_battery_repr_contains_capacity(self):
        from ochre_next._hares import Battery

        b = Battery(name="test_batt", capacity_kwh=10.0)
        r = repr(b)
        assert "10" in r


class TestPvSoilingConfigRepr:
    def test_pv_soiling_config_repr_contains_class_name(self):
        from ochre_next._hares import PvSoilingConfig

        c = PvSoilingConfig()
        r = repr(c)
        assert r
        assert "PvSoilingConfig" in r


class TestPvRepr:
    def test_pv_repr_contains_class_name(self):
        from ochre_next._hares import PV

        pv = PV(name="test_pv", capacity_kw=5.0, tilt=30.0, azimuth=180.0)
        r = repr(pv)
        assert r
        assert "PV" in r

    def test_pv_repr_contains_name(self):
        from ochre_next._hares import PV

        pv = PV(name="test_pv", capacity_kw=5.0, tilt=30.0, azimuth=180.0)
        r = repr(pv)
        assert "test_pv" in r

    def test_pv_repr_contains_capacity(self):
        from ochre_next._hares import PV

        pv = PV(name="test_pv", capacity_kw=5.0, tilt=30.0, azimuth=180.0)
        r = repr(pv)
        assert "5" in r


class TestEvRepr:
    def test_ev_repr_contains_class_name(self):
        from ochre_next._hares import EV

        ev = EV(name="test_ev", capacity_kwh=75.0, max_charging_kw=11.0)
        r = repr(ev)
        assert r
        assert "EV" in r

    def test_ev_repr_contains_name(self):
        from ochre_next._hares import EV

        ev = EV(name="test_ev", capacity_kwh=75.0, max_charging_kw=11.0)
        r = repr(ev)
        assert "test_ev" in r
