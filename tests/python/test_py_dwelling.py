"""Tests for Python-side dwelling behavior (step, telemetry, error handling)."""

import math
import pytest
from datetime import datetime

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")


class TestStep:
    def test_step_returns_correct_keys_and_types(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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

        result = dw.step()

        assert "timestamp" in result, "step() result should have 'timestamp' key"
        assert isinstance(result["timestamp"], datetime), "timestamp should be a datetime"
        assert result["timestamp"].year == 2019, "timestamp year should be 2019"

        net_electric_power_kw_keys = [
            k for k in result.keys() if "net_electric_power" in k
        ]
        assert len(net_electric_power_kw_keys) > 0, (
            "should have net_electric_power_kw key"
        )
        for key in net_electric_power_kw_keys:
            value = result[key]
            assert isinstance(value, (int, float)), f"{key} should be a number"
            assert math.isfinite(value), f"{key} must be finite, got {value}"
            assert -1000.0 <= value <= 1000.0, (
                f"{key} out of reasonable range [-1000, 1000] kW: {value}"
            )


class TestTelemetry:
    def test_telemetry_zone_returns_correct_data(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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
        dw.step()

        t = dw.telemetry()
        zone = t.zone()

        assert "temperature_c" in zone, "zone should have temperature_c"
        temps = zone["temperature_c"]
        assert isinstance(temps, list), "temperature_c should be a list"
        assert len(temps) > 0, "temperature_c list should be non-empty"
        for temp in temps:
            assert math.isfinite(temp), f"temperature_c value must be finite, got {temp}"
            assert -50.0 <= temp <= 80.0, (
                f"temperature_c out of reasonable range [-50, 80] C: {temp}"
            )


class TestStepError:
    def test_step_past_end_raises_runtime_error(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=180,
            time_res_s=60,
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

        assert "temperature_c" in zone, "zone should have temperature_c"
        temps = zone["temperature_c"]
        assert isinstance(temps, list), "temperature_c should be a list"
        for temp in temps:
            assert math.isfinite(temp), f"temperature_c value must be finite, got {temp}"
            assert -50.0 <= temp <= 80.0, (
                f"temperature_c out of reasonable range [-50, 80] C: {temp}"
            )

    def test_telemetry_equipment_returns_correct_data(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
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
        dw.step()

        t = dw.telemetry()
        equipment = t.equipment()

        assert isinstance(equipment, dict), "equipment() should return a dict"
        assert len(equipment) > 0, "equipment dict should be non-empty"
        for name in equipment:
            assert isinstance(name, str), f"equipment key should be a string, got {type(name)}"
            assert len(name) > 0, "equipment name should be non-empty"
