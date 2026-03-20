from __future__ import annotations

from datetime import datetime, timedelta
import importlib
import logging
import sys
import types

import pytest

pl = pytest.importorskip("polars")


def _install_fake_hares(monkeypatch: pytest.MonkeyPatch):
    class FakePyControlSignal:
        @staticmethod
        def thermal_setpoint(heat_c=None, cool_c=None, deadband_c=None):
            return {"kind": "thermal", "heat_c": heat_c, "cool_c": cool_c, "deadband_c": deadband_c}

        @staticmethod
        def power_setpoint(kw, reactive_kvar=None):
            return {"kind": "power", "kw": kw}

        @staticmethod
        def soc_target(target, min=None, max=None):
            return {"kind": "soc", "target": target, "min": min, "max": max}

        @classmethod
        def from_dict(cls, d):
            return {"kind": d.get("type", "dict"), **d}

    class FakePyDwelling:
        def __init__(self):
            self.apply_calls = []
            self.step_calls = 0
            self.sim_calls = 0

        @classmethod
        def from_hpxml(cls, hpxml, schedule, weather, **kwargs):
            obj = cls()
            obj.from_hpxml_args = {
                "hpxml": hpxml,
                "schedule": schedule,
                "weather": weather,
                "kwargs": kwargs,
            }
            return obj

        def simulate(self):
            self.sim_calls += 1
            return pl.DataFrame(
                {
                    "Time": [
                        "2020-01-01T00:00:00Z",
                        "2020-01-01T00:30:00Z",
                        "2020-01-01T01:00:00Z",
                    ],
                    "Total Electric Power (kW)": [1.0, 2.0, 3.0],
                    "Temperature - Indoor (C)": [20.0, 20.5, 21.0],
                }
            )

        def apply_control(self, name, signal):
            self.apply_calls.append((name, signal))

        def step(self):
            self.step_calls += 1
            return {"time": "2020-01-01T00:01:00Z", "net_electric_power_kw": 1.0}

    fake_mod = types.ModuleType("ochre_next._hares")
    fake_mod.PyControlSignal = FakePyControlSignal
    fake_mod.PyDwelling = FakePyDwelling
    fake_mod.PyFleet = type("FakePyFleet", (), {})

    monkeypatch.setitem(sys.modules, "ochre_next._hares", fake_mod)
    sys.modules.pop("ochre_next", None)
    sys.modules.pop("ochre_next.compat", None)
    sys.modules.pop("ochre_next.compat.dwelling", None)

    import ochre_next.compat.dwelling as dwelling_mod

    return importlib.reload(dwelling_mod), FakePyDwelling


def test_constructor_maps_ochre_kwargs(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)

    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
        Equipment={"Battery": {"capacity_kwh": 10}},
    )

    assert d._dwelling.from_hpxml_args["hpxml"] == "in.xml"
    assert d._dwelling.from_hpxml_args["schedule"] == "schedule.csv"
    assert d._dwelling.from_hpxml_args["weather"] == "weather.epw"


def test_simulate_returns_ochre_tuple(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    df, metrics, df_hourly = d.simulate()
    assert isinstance(df, pl.DataFrame)
    assert "Total Electric Power (kW)" in df.columns
    assert hasattr(df, "to_pandas")
    assert isinstance(metrics, dict)
    assert "Total Electric Power (kW)" in metrics
    assert isinstance(df_hourly, pl.DataFrame)


def test_update_model_warns_and_skips_unknown(monkeypatch: pytest.MonkeyPatch, caplog):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    with caplog.at_level(logging.WARNING):
        out = d.update_model(
            {
                "HVAC Heating": {"Setpoint": 21.0},
                "UnknownEquipment": {"SomeKey": 1.0},
            }
        )

    assert out is None
    assert d._dwelling.step_calls == 1
    assert len(d._dwelling.apply_calls) == 1
    name, signal = d._dwelling.apply_calls[0]
    assert name == "HVAC Heating"
    assert signal["heat_c"] == 21.0
    assert signal["cool_c"] is None
    assert "unrecognized or invalid control" in caplog.text
    assert "UnknownEquipment" in caplog.text


def test_generate_results_requires_simulate(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    with pytest.raises(RuntimeError):
        d.generate_results()


@pytest.mark.parametrize(
    "equipment_name",
    ["HVAC Cooling", "Air Conditioner", "Room AC", "ASHP Cooler", "MSHP Cooler"],
)
def test_cooling_setpoint_routes_to_cool_c(
    monkeypatch: pytest.MonkeyPatch, equipment_name: str,
):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    d.update_model({equipment_name: {"Setpoint": 26.0}})

    assert len(d._dwelling.apply_calls) == 1
    name, signal = d._dwelling.apply_calls[0]
    assert name == equipment_name
    assert signal["cool_c"] == 26.0
    assert signal["heat_c"] is None


def test_deadband_only_routes_correctly(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    d.update_model({"HVAC Heating": {"Deadband": 2.0}})

    assert len(d._dwelling.apply_calls) == 1
    _, signal = d._dwelling.apply_calls[0]
    assert signal["deadband_c"] == 2.0
    assert signal["heat_c"] is None
    assert signal["cool_c"] is None


def test_setpoint_with_deadband_combined(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    d.update_model({"HVAC Cooling": {"Setpoint": 25.0, "Deadband": 1.5}})

    assert len(d._dwelling.apply_calls) == 1
    _, signal = d._dwelling.apply_calls[0]
    assert signal["cool_c"] == 25.0
    assert signal["heat_c"] is None
    assert signal["deadband_c"] == 1.5


def test_unknown_keys_log_warning(monkeypatch: pytest.MonkeyPatch, caplog):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    d = dwelling_mod.Dwelling(
        hpxml_file="in.xml",
        hpxml_schedule_file="schedule.csv",
        weather_file="weather.epw",
        start_time=datetime(2020, 1, 1),
        time_res=timedelta(minutes=1),
        duration=timedelta(hours=1),
    )

    with caplog.at_level(logging.WARNING):
        d.update_model({"HVAC Heating": {"UnknownKey": 42}})

    assert len(d._dwelling.apply_calls) == 0
    assert "unrecognized or invalid control" in caplog.text
    assert "HVAC Heating" in caplog.text


def test_top_level_reexports(monkeypatch: pytest.MonkeyPatch):
    dwelling_mod, _ = _install_fake_hares(monkeypatch)
    import ochre_next

    assert ochre_next.Dwelling is dwelling_mod.PyDwelling
    assert ochre_next.Fleet.__name__ == "FakePyFleet"
    assert ochre_next.ControlSignal is dwelling_mod.PyControlSignal
