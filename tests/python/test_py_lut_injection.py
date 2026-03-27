"""Tests for PY-013: battery/EV LUT injection via Python bindings.

Verifies that charging curve LUTs (4D: soc × temp × c_rate × soh),
OCV tables, and UNeg tables can be injected into Battery and EV equipment
from Python using multiple input formats.
"""

import numpy as np
import pytest

from pathlib import Path

from ochre_next import Battery, ControlSignal, Dwelling, EV, LutType

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_dwelling() -> Dwelling:
    """Create a minimal dwelling for LUT injection testing."""
    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration_s=3600,
        time_res_s=60,
        defaults_path=str(HARES_DEFAULTS),
    )
    dw.initialize()
    return dw


def _simple_charging_lut() -> dict:
    """1D-degenerate 4D LUT: only SOC axis varies."""
    return {
        "soc_grid": np.array([0.0, 0.5, 0.8, 0.9, 1.0]),
        "temp_grid": np.array([25.0]),
        "crate_grid": np.array([1.0]),
        "soh_grid": np.array([1.0]),
        "lut": np.array([1.0, 1.0, 0.8, 0.5, 0.1], dtype=np.float32),
    }


def _lfp_ocv_table() -> list[tuple[float, float]]:
    """Simplified LFP OCV curve (flat plateau ~3.3V)."""
    return [
        (0.0, 2.8),
        (0.1, 3.2),
        (0.2, 3.28),
        (0.5, 3.30),
        (0.8, 3.35),
        (0.9, 3.40),
        (1.0, 3.60),
    ]


def _simple_uneg_table() -> list[tuple[float, float]]:
    """Simplified negative electrode potential table."""
    return [
        (0.0, 1.2),
        (0.5, 0.12),
        (1.0, 0.08),
    ]


# ---------------------------------------------------------------------------
# Battery LUT injection tests
# ---------------------------------------------------------------------------


class TestBatteryLutInjection:
    def test_add_battery_with_charging_curve_lut(self):
        lut = _simple_charging_lut()
        bat = Battery("TestBat", 10.0, charging_curve_lut=lut)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_ocv_table(self):
        ocv = _lfp_ocv_table()
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_uneg_table(self):
        uneg = _simple_uneg_table()
        bat = Battery("TestBat", 10.0, uneg_table=uneg)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_all_luts(self):
        bat = Battery(
            "TestBat",
            10.0,
            charging_curve_lut=_simple_charging_lut(),
            ocv_table=_lfp_ocv_table(),
            uneg_table=_simple_uneg_table(),
        )
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_set_equipment_lut_charging_curve(self):
        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.ChargingCurve, _simple_charging_lut())
        assert dw.has_equipment_lut("TestBat", LutType.ChargingCurve)

    def test_set_equipment_lut_ocv(self):
        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.Ocv, _lfp_ocv_table())
        assert dw.has_equipment_lut("TestBat", LutType.Ocv)

    def test_set_equipment_lut_uneg(self):
        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.UNeg, _simple_uneg_table())
        assert dw.has_equipment_lut("TestBat", LutType.UNeg)

    def test_clear_equipment_lut_charging_curve(self):
        bat = Battery("TestBat", 10.0, charging_curve_lut=_simple_charging_lut())
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert dw.has_equipment_lut("TestBat", LutType.ChargingCurve)
        dw.clear_equipment_lut("TestBat", LutType.ChargingCurve)
        assert not dw.has_equipment_lut("TestBat", LutType.ChargingCurve)

    def test_clear_equipment_lut_ocv_resets_to_default(self):
        bat = Battery("TestBat", 10.0, ocv_table=_lfp_ocv_table())
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert dw.has_equipment_lut("TestBat", LutType.Ocv)
        dw.clear_equipment_lut("TestBat", LutType.Ocv)
        assert not dw.has_equipment_lut("TestBat", LutType.Ocv)

    def test_set_lut_nonexistent_equipment_raises(self):
        dw = _make_dwelling()
        with pytest.raises(Exception, match="not found"):
            dw.set_equipment_lut("NoSuch", LutType.ChargingCurve, _simple_charging_lut())

    def test_set_ocv_on_ev_succeeds(self):
        """EV supports OCV injection (chemistry-aware, same as Battery)."""
        ev = EV("TestEV", capacity_kwh=65.0)
        dw = _make_dwelling()
        dw.add_ev(ev)
        dw.set_equipment_lut("TestEV", LutType.Ocv, _lfp_ocv_table())
        result = dw.step()
        assert "time" in result


# ---------------------------------------------------------------------------
# EV LUT injection tests
# ---------------------------------------------------------------------------


class TestEvLutInjection:
    def test_add_ev_with_charging_curve_lut(self):
        lut = _simple_charging_lut()
        ev = EV("TestEV", capacity_kwh=65.0, charging_curve_lut=lut)
        dw = _make_dwelling()
        dw.add_ev(ev)
        assert "TestEV" in dw.equipment_names()

    def test_set_ev_charging_curve_after_add(self):
        ev = EV("TestEV", capacity_kwh=65.0)
        dw = _make_dwelling()
        dw.add_ev(ev)
        dw.set_equipment_lut("TestEV", LutType.ChargingCurve, _simple_charging_lut())
        assert dw.has_equipment_lut("TestEV", LutType.ChargingCurve)

    def test_clear_ev_charging_curve(self):
        ev = EV("TestEV", capacity_kwh=65.0, charging_curve_lut=_simple_charging_lut())
        dw = _make_dwelling()
        dw.add_ev(ev)
        assert dw.has_equipment_lut("TestEV", LutType.ChargingCurve)
        dw.clear_equipment_lut("TestEV", LutType.ChargingCurve)
        assert not dw.has_equipment_lut("TestEV", LutType.ChargingCurve)


# ---------------------------------------------------------------------------
# Input format tests
# ---------------------------------------------------------------------------


class TestInputFormats:
    def test_numpy_dict_format(self):
        """Charging curve LUT as dict of numpy arrays."""
        lut = {
            "soc_grid": np.array([0.0, 0.5, 1.0]),
            "temp_grid": np.array([25.0]),
            "crate_grid": np.array([1.0]),
            "soh_grid": np.array([1.0]),
            "lut": np.array([1.0, 0.5, 0.0], dtype=np.float32),
        }
        bat = Battery("TestBat", 10.0, charging_curve_lut=lut)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_ocv_tuple_list_format(self):
        ocv = [(0.0, 3.0), (0.5, 3.6), (1.0, 4.2)]
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_ocv_dict_format(self):
        ocv = {"soc": [0.0, 0.5, 1.0], "voltage": [3.0, 3.6, 4.2]}
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_uneg_tuple_list_format(self):
        uneg = [(0.0, 1.2), (0.5, 0.12), (1.0, 0.08)]
        bat = Battery("TestBat", 10.0, uneg_table=uneg)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_npz_path_format(self, tmp_path):
        """Charging curve LUT from NPZ file path."""
        lut_data = _simple_charging_lut()
        npz_path = tmp_path / "test_lut.npz"
        np.savez_compressed(npz_path, **lut_data)

        bat = Battery("TestBat", 10.0, charging_curve_lut=str(npz_path))
        dw = _make_dwelling()
        dw.add_battery(bat)


# ---------------------------------------------------------------------------
# Validation tests
# ---------------------------------------------------------------------------


class TestValidation:
    def test_valid_ocv_single_point_does_not_raise(self):
        Battery("TestBat", 10.0, ocv_table=[(0.0, 3.0)])

    def test_invalid_ocv_mismatched_lengths_raises(self):
        with pytest.raises(Exception):
            Battery(
                "TestBat",
                10.0,
                ocv_table={"soc": [0.0, 0.5], "voltage": [3.0]},
            )

    def test_invalid_ocv_non_increasing_soc_raises(self):
        with pytest.raises(Exception):
            Battery(
                "TestBat",
                10.0,
                ocv_table=[(0.5, 3.6), (0.3, 3.4), (1.0, 4.2)],
            )

    def test_invalid_charging_lut_shape_mismatch_raises(self):
        lut = {
            "soc_grid": np.array([0.0, 0.5, 1.0]),
            "temp_grid": np.array([25.0]),
            "crate_grid": np.array([1.0]),
            "soh_grid": np.array([1.0]),
            "lut": np.array([1.0, 0.5], dtype=np.float32),  # wrong length
        }
        with pytest.raises(Exception, match="length"):
            Battery("TestBat", 10.0, charging_curve_lut=lut)


# ---------------------------------------------------------------------------
# Behavioral LUT tests
# ---------------------------------------------------------------------------


class TestLutBehavior:
    def test_ocv_table_affects_battery_voltage(self):
        """Battery with custom OCV should behave differently from default."""
        from conftest import make_dwelling

        # Create two batteries: one default NMC, one with LFP OCV
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.initialize()
        bat_default = Battery("Default", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.5)
        bat_custom = Battery("Custom", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.5)
        dw.add_battery(bat_default)
        dw.add_battery(bat_custom)

        # Inject LFP OCV table on custom battery (much flatter plateau than default NMC)
        lfp_ocv = {
            "soc": [0.0, 0.2, 0.5, 0.8, 1.0],
            "voltage": [2.5, 3.2, 3.27, 3.32, 3.6],
        }
        dw.set_equipment_lut("Custom", LutType.ocv(), lfp_ocv)

        dw.apply_control("Default", ControlSignal.soc_target(target=0.9))
        dw.apply_control("Custom", ControlSignal.soc_target(target=0.9))
        for _ in range(5):
            dw.step()

        tel = dw.telemetry().equipment()
        idx_d = tel["names"].index("Default")
        idx_c = tel["names"].index("Custom")

        soc_d = tel["soc"][idx_d]
        soc_c = tel["soc"][idx_c]
        # Both batteries should be charging (SOC above initial 0.5)
        assert soc_d > 0.5, f"Default battery should be charging, SOC={soc_d}"
        assert soc_c > 0.5, f"Custom battery should be charging, SOC={soc_c}"
        # Different OCV curves produce different internal voltages, leading to
        # measurably different SOC trajectories
        assert abs(soc_d - soc_c) > 0.001, (
            f"Custom OCV should produce different charging behavior: "
            f"default SOC={soc_d:.4f}, custom SOC={soc_c:.4f}"
        )
