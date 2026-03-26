"""Tests for Python-side dwelling behavior (step, telemetry, error handling)."""

import os
import pytest
from datetime import datetime

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)
SCHEDULE = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Input Files/BEopt_example_schedule.csv"
)


class TestStep:
    def test_step_returns_correct_keys_and_types(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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

        result = dw.step()

        assert isinstance(result, dict), "step() should return a dict"
        assert "time" in result, "step() result should have 'time' key"
        assert isinstance(result["time"], datetime), "time should be a datetime"

        net_electric_power_kw_keys = [
            k for k in result.keys() if "net_electric_power" in k
        ]
        assert len(net_electric_power_kw_keys) > 0, (
            "should have net_electric_power_kw key"
        )
        for key in net_electric_power_kw_keys:
            assert isinstance(result[key], (int, float)), f"{key} should be a number"


class TestTelemetry:
    def test_telemetry_zone_returns_correct_data(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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
        dw.step()

        t = dw.telemetry()
        zone = t.zone()

        assert isinstance(zone, dict), "zone() should return a dict"
        assert "temperature_c" in zone, "zone should have temperature_c"
        assert isinstance(zone["temperature_c"], list), "temperature_c should be a list"


class TestStepError:
    def test_step_past_end_raises_runtime_error(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration=180,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        for _ in range(3):
            dw.step()

        with pytest.raises(RuntimeError):
            dw.step()

        t = dw.telemetry()
        zone = t.zone()

        assert isinstance(zone, dict), "zone() should return a dict"
        assert "temperature_c" in zone, "zone should have temperature_c"
        assert isinstance(zone["temperature_c"], (int, float)), (
            "temperature_c should be a number"
        )

    def test_telemetry_equipment_returns_correct_data(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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
        dw.step()

        t = dw.telemetry()
        equipment = t.equipment()

        assert isinstance(equipment, dict), "equipment() should return a dict"


class TestStepError:
    def test_step_past_end_raises_runtime_error(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration=180,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        for _ in range(3):
            dw.step()

        with pytest.raises(RuntimeError):
            dw.step()

        t = dw.telemetry()
        zone = t.zone()

        assert isinstance(zone, dict), "zone() should return a dict"
        assert "temperature_c" in zone, "zone should have temperature_c"
        assert isinstance(zone["temperature_c"], (int, float)), (
            "temperature_c should be a number"
        )

    def test_telemetry_equipment_returns_correct_data(self, sample_dwelling):
        hpxml_path, schedule_path, weather_path = sample_dwelling

        dw = Dwelling.from_hpxml(
            hpxml=hpxml_path,
            schedule=schedule_path,
            weather=weather_path,
        )
        dw.initialize()
        dw.step()

        t = dw.telemetry()
        equipment = t.equipment()

        assert isinstance(equipment, dict), "equipment() should return a dict"


class TestStepError:
    def test_step_past_end_raises_runtime_error(self, sample_dwelling):
        hpxml_path, schedule_path, weather_path = sample_dwelling

        dw = Dwelling.from_hpxml(
            hpxml=hpxml_path,
            schedule=schedule_path,
            weather=weather_path,
        )
        dw.initialize()

        for _ in range(5):
            dw.step()

        with pytest.raises(RuntimeError):
            dw.step()
