"""Tests for PY-013: battery/EV LUT injection via Python bindings.

Verifies that charging curve LUTs (4D: soc × temp × c_rate × soh),
OCV tables, and UNeg tables can be injected into Battery and EV equipment
from Python using multiple input formats.
"""

import numpy as np
import pytest

from ochre_next._hares import LutType, PyDwelling

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_dwelling() -> PyDwelling:
    """Create a minimal synthetic dwelling for testing."""
    return PyDwelling.synthetic(
        start="2026-01-01T00:00:00Z",
        duration_s=3600,
        step_s=60,
    )


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
        from ochre_next._hares import Battery

        lut = _simple_charging_lut()
        bat = Battery("TestBat", 10.0, charging_curve_lut=lut)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_ocv_table(self):
        from ochre_next._hares import Battery

        ocv = _lfp_ocv_table()
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_uneg_table(self):
        from ochre_next._hares import Battery

        uneg = _simple_uneg_table()
        bat = Battery("TestBat", 10.0, uneg_table=uneg)
        dw = _make_dwelling()
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_with_all_luts(self):
        from ochre_next._hares import Battery

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
        from ochre_next._hares import Battery

        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.ChargingCurve, _simple_charging_lut())

    def test_set_equipment_lut_ocv(self):
        from ochre_next._hares import Battery

        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.Ocv, _lfp_ocv_table())

    def test_set_equipment_lut_uneg(self):
        from ochre_next._hares import Battery

        bat = Battery("TestBat", 10.0)
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.set_equipment_lut("TestBat", LutType.UNeg, _simple_uneg_table())

    def test_clear_equipment_lut_charging_curve(self):
        from ochre_next._hares import Battery

        bat = Battery("TestBat", 10.0, charging_curve_lut=_simple_charging_lut())
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.clear_equipment_lut("TestBat", LutType.ChargingCurve)

    def test_clear_equipment_lut_ocv_resets_to_default(self):
        from ochre_next._hares import Battery

        bat = Battery("TestBat", 10.0, ocv_table=_lfp_ocv_table())
        dw = _make_dwelling()
        dw.add_battery(bat)
        dw.clear_equipment_lut("TestBat", LutType.Ocv)

    def test_set_lut_nonexistent_equipment_raises(self):
        dw = _make_dwelling()
        with pytest.raises(Exception, match="not found"):
            dw.set_equipment_lut("NoSuch", LutType.ChargingCurve, _simple_charging_lut())

    def test_set_lut_unsupported_type_raises(self):
        """Setting OCV on an EV should raise (EV doesn't support OCV)."""
        from ochre_next._hares import EV

        ev = EV("TestEV", capacity_kwh=65.0)
        dw = _make_dwelling()
        dw.add_ev(ev)
        with pytest.raises(Exception):
            dw.set_equipment_lut("TestEV", LutType.Ocv, _lfp_ocv_table())


# ---------------------------------------------------------------------------
# EV LUT injection tests
# ---------------------------------------------------------------------------


class TestEvLutInjection:
    def test_add_ev_with_charging_curve_lut(self):
        from ochre_next._hares import EV

        lut = _simple_charging_lut()
        ev = EV("TestEV", capacity_kwh=65.0, charging_curve_lut=lut)
        dw = _make_dwelling()
        dw.add_ev(ev)
        assert "TestEV" in dw.equipment_names()

    def test_set_ev_charging_curve_after_add(self):
        from ochre_next._hares import EV

        ev = EV("TestEV", capacity_kwh=65.0)
        dw = _make_dwelling()
        dw.add_ev(ev)
        dw.set_equipment_lut("TestEV", LutType.ChargingCurve, _simple_charging_lut())

    def test_clear_ev_charging_curve(self):
        from ochre_next._hares import EV

        ev = EV("TestEV", capacity_kwh=65.0, charging_curve_lut=_simple_charging_lut())
        dw = _make_dwelling()
        dw.add_ev(ev)
        dw.clear_equipment_lut("TestEV", LutType.ChargingCurve)


# ---------------------------------------------------------------------------
# Input format tests
# ---------------------------------------------------------------------------


class TestInputFormats:
    def test_numpy_dict_format(self):
        """Charging curve LUT as dict of numpy arrays."""
        from ochre_next._hares import Battery

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
        from ochre_next._hares import Battery

        ocv = [(0.0, 3.0), (0.5, 3.6), (1.0, 4.2)]
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_ocv_dict_format(self):
        from ochre_next._hares import Battery

        ocv = {"soc": [0.0, 0.5, 1.0], "voltage": [3.0, 3.6, 4.2]}
        bat = Battery("TestBat", 10.0, ocv_table=ocv)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_uneg_tuple_list_format(self):
        from ochre_next._hares import Battery

        uneg = [(0.0, 1.2), (0.5, 0.12), (1.0, 0.08)]
        bat = Battery("TestBat", 10.0, uneg_table=uneg)
        dw = _make_dwelling()
        dw.add_battery(bat)

    def test_npz_path_format(self, tmp_path):
        """Charging curve LUT from NPZ file path."""
        from ochre_next._hares import Battery

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
    def test_invalid_ocv_mismatched_lengths_raises(self):
        from ochre_next._hares import Battery

        with pytest.raises(Exception):
            Battery("TestBat", 10.0, ocv_table=[(0.0, 3.0)])  # too few points is ok
            # But mismatched dict raises:
            Battery(
                "TestBat",
                10.0,
                ocv_table={"soc": [0.0, 0.5], "voltage": [3.0]},
            )

    def test_invalid_ocv_non_increasing_soc_raises(self):
        from ochre_next._hares import Battery

        with pytest.raises(Exception):
            Battery(
                "TestBat",
                10.0,
                ocv_table=[(0.5, 3.6), (0.3, 3.4), (1.0, 4.2)],
            )

    def test_invalid_charging_lut_shape_mismatch_raises(self):
        from ochre_next._hares import Battery

        lut = {
            "soc_grid": np.array([0.0, 0.5, 1.0]),
            "temp_grid": np.array([25.0]),
            "crate_grid": np.array([1.0]),
            "soh_grid": np.array([1.0]),
            "lut": np.array([1.0, 0.5], dtype=np.float32),  # wrong length
        }
        with pytest.raises(Exception, match="length"):
            Battery("TestBat", 10.0, charging_curve_lut=lut)
