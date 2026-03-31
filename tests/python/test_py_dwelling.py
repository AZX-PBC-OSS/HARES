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


def assert_core_output_semantics(co):
    assert co.electric_convention in {"consumption", "generation", "bidirectional"}

    if co.electric_kw is not None:
        assert isinstance(co.electric_kw, (int, float))
        assert math.isfinite(co.electric_kw)
        if co.electric_convention == "consumption":
            assert co.electric_kw >= 0.0
        elif co.electric_convention == "generation":
            assert co.electric_kw <= 0.0
        else:
            assert co.electric_convention == "bidirectional"
    else:
        # Python binding maps missing electric output to "consumption".
        assert co.electric_convention == "consumption"

    if co.reactive_power_kvar is not None:
        assert isinstance(co.reactive_power_kvar, (int, float))
        assert math.isfinite(co.reactive_power_kvar)

    if co.fuel_w is not None:
        assert isinstance(co.fuel_w, (int, float))
        assert math.isfinite(co.fuel_w)
        assert co.fuel_type in {"Electric", "Gas", "Propane", "Oil", "NoFuel"}
    else:
        assert co.fuel_type is None

    if co.operating_mode is not None:
        assert isinstance(co.operating_mode, str)
        assert len(co.operating_mode) > 0

    if co.soc is not None:
        assert isinstance(co.soc, (int, float))
        assert math.isfinite(co.soc)
        assert 0.0 <= co.soc <= 1.0


def assert_core_output_matches_telemetry(dw):
    equipment_diag = dw.telemetry().equipment()
    names = equipment_diag["names"]
    power_kw = equipment_diag["power_kw"]
    soc = equipment_diag["soc"]
    by_name = {name: idx for idx, name in enumerate(names)}

    equipment = dw.equipment()
    assert len(equipment) > 0, "dw.equipment() should return active equipment"

    for eq in equipment:
        assert eq.name in by_name, f"missing telemetry index for equipment {eq.name!r}"
        idx = by_name[eq.name]
        co = eq.core_output

        assert_core_output_semantics(co)

        if co.electric_kw is None:
            assert power_kw[idx] == 0.0
        else:
            assert math.isclose(co.electric_kw, power_kw[idx], rel_tol=0.0, abs_tol=1e-9)

        if co.soc is None:
            assert soc[idx] == 0.0
        else:
            assert math.isclose(co.soc, soc[idx], rel_tol=0.0, abs_tol=1e-9)


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

    def test_equipment_core_output_matches_telemetry_vectors(self):
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
        for _ in range(3):
            dw.step()
            assert_core_output_matches_telemetry(dw)


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
