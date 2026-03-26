"""Tests for equipment mutation API — add, remove, replace, update equipment at runtime."""

import pytest

from conftest import make_dwelling


def _make_dwelling():
    dw = make_dwelling(duration_s=3600, time_res_s=60)
    dw.initialize()
    return dw


class TestAddEquipment:
    def test_add_battery_appears_in_equipment_names(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

    def test_add_battery_step_succeeds(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0)
        dw.add_battery(bat)
        result = dw.step()
        assert "time" in result

    def test_add_pv_appears_in_equipment_names(self):
        from ochre_next import PV

        dw = _make_dwelling()
        pv = PV("TestPV", 5.0, 30.0, 180.0)
        dw.add_pv(pv)
        assert "TestPV" in dw.equipment_names()

    def test_add_pv_step_succeeds(self):
        from ochre_next import PV

        dw = _make_dwelling()
        pv = PV("TestPV", 5.0, 30.0, 180.0)
        dw.add_pv(pv)
        result = dw.step()
        assert "time" in result

    def test_add_ev_appears_in_equipment_names(self):
        from ochre_next import EV

        dw = _make_dwelling()
        ev = EV("TestEV", capacity_kwh=75.0)
        dw.add_ev(ev)
        assert "TestEV" in dw.equipment_names()

    def test_add_ev_step_succeeds(self):
        from ochre_next import EV

        dw = _make_dwelling()
        ev = EV("TestEV", capacity_kwh=75.0, max_charging_kw=7.2)
        dw.add_ev(ev)
        result = dw.step()
        assert "time" in result


    def test_ev_max_charging_kw_config_key_applied(self):
        """Regression: max_charging_kw must map to max_charging_power_kw config key.

        Starts the EV plugged in at home with low SOC so it charges on the first
        step. Configures max_charging_kw=3.6 (below L2 default ~7.7 kW). If the
        config key were wrong, the EV would charge at the default power.
        """
        from ochre_next import EV, EvConnectionState

        dw = _make_dwelling()
        ev = EV(
            "TestEV",
            capacity_kwh=60.0,
            max_charging_kw=3.6,
            initial_soc=0.2,
            initial_connection_state=EvConnectionState.HomePluggedIn,
        )
        dw.add_ev(ev)

        dw.step()
        tel = dw.telemetry().equipment()
        names = tel["names"]
        powers = tel["power_kw"]
        ev_idx = names.index("TestEV")
        ev_power = powers[ev_idx]

        assert ev_power > 0, "EV did not charge despite being plugged in with low SOC"
        assert ev_power <= 3.6 + 1e-6, (
            f"EV charged at {ev_power:.2f} kW, exceeding configured max of 3.6 kW"
        )


class TestRemoveEquipment:
    def test_remove_added_equipment(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)
        assert "TestBat" in dw.equipment_names()

        dw.remove_equipment("TestBat")
        assert "TestBat" not in dw.equipment_names()

    def test_remove_nonexistent_raises(self):
        dw = _make_dwelling()
        with pytest.raises(ValueError, match="not found"):
            dw.remove_equipment("NoSuchEquipment")

    def test_step_after_remove_succeeds(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)
        dw.remove_equipment("TestBat")
        result = dw.step()
        assert "time" in result


class TestReplaceEquipment:
    def test_replace_battery_with_new_battery(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat1 = Battery("TestBat", 10.0)
        dw.add_battery(bat1)

        bat2 = Battery("TestBat", 20.0)
        dw.replace_equipment("TestBat", bat2)

        # Name should still be present
        assert "TestBat" in dw.equipment_names()

        # Verify descriptor shows updated equipment
        descs = {d.name: d for d in dw.equipment_descriptors()}
        assert "TestBat" in descs

    def test_replace_nonexistent_raises(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("NoSuch", 10.0)
        with pytest.raises(ValueError, match="not found"):
            dw.replace_equipment("NoSuch", bat)

    def test_step_after_replace_succeeds(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat1 = Battery("TestBat", 10.0)
        dw.add_battery(bat1)

        bat2 = Battery("TestBat", 20.0, max_charge_kw=10.0, max_discharge_kw=10.0)
        dw.replace_equipment("TestBat", bat2)

        result = dw.step()
        assert "time" in result


class TestUpdateEquipment:
    def test_update_battery_ocv_table(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)

        # LFP-style OCV curve (lower voltages than NMC)
        ocv_data = [
            (0.0, 2.8),
            (0.1, 3.1),
            (0.2, 3.2),
            (0.3, 3.22),
            (0.4, 3.24),
            (0.5, 3.26),
            (0.6, 3.28),
            (0.7, 3.30),
            (0.8, 3.32),
            (0.9, 3.35),
            (1.0, 3.60),
        ]
        dw.update_equipment("TestBat", ocv_table=ocv_data)

        # Should still step fine
        result = dw.step()
        assert "time" in result

    def test_update_battery_uneg_table(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)

        uneg_data = [
            (0.0, 1.3),
            (0.1, 0.25),
            (0.2, 0.18),
            (0.3, 0.15),
            (0.4, 0.13),
            (0.5, 0.12),
            (0.6, 0.12),
            (0.7, 0.11),
            (0.8, 0.09),
            (0.9, 0.09),
            (1.0, 0.09),
        ]
        dw.update_equipment("TestBat", uneg_table=uneg_data)
        result = dw.step()
        assert "time" in result

    def test_update_nonexistent_raises(self):
        dw = _make_dwelling()
        with pytest.raises(ValueError, match="not found"):
            dw.update_equipment(
                "NoSuch",
                ocv_table=[(0.0, 3.0), (1.0, 4.2)],
            )

    def test_update_requires_kwargs(self):
        dw = _make_dwelling()
        with pytest.raises(ValueError, match="at least one keyword"):
            dw.update_equipment("anything")


class TestSetEquipmentLut:
    def test_set_ocv_from_dict(self):
        from ochre_next import Battery, LutType

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)

        ocv_dict = {
            "soc": [0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
            "voltage": [3.0, 3.3, 3.5, 3.7, 3.9, 4.2],
        }
        dw.set_equipment_lut("TestBat", LutType.ocv(), ocv_dict)
        result = dw.step()
        assert "time" in result

    def test_clear_ocv_lut(self):
        from ochre_next import Battery, LutType

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)

        ocv_dict = {
            "soc": [0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
            "voltage": [3.0, 3.3, 3.5, 3.7, 3.9, 4.2],
        }
        dw.set_equipment_lut("TestBat", LutType.ocv(), ocv_dict)
        dw.clear_equipment_lut("TestBat", LutType.ocv())
        result = dw.step()
        assert "time" in result

    def test_set_lut_nonexistent_raises(self):
        from ochre_next import LutType

        dw = _make_dwelling()
        with pytest.raises(ValueError, match="not found"):
            dw.set_equipment_lut(
                "NoSuch",
                LutType.ocv(),
                {"soc": [0.0, 1.0], "voltage": [3.0, 4.2]},
            )


class TestMultipleMutations:
    def test_add_battery_and_pv_then_step(self):
        from ochre_next import Battery, PV

        dw = _make_dwelling()
        dw.add_battery(Battery("Bat1", 10.0))
        dw.add_pv(PV("PV1", 5.0, 30.0, 180.0))

        names = dw.equipment_names()
        assert "Bat1" in names
        assert "PV1" in names

        result = dw.step()
        assert "time" in result

    def test_add_then_remove_then_add_different(self):
        from ochre_next import Battery, PV

        dw = _make_dwelling()
        dw.add_battery(Battery("Bat1", 10.0))
        dw.remove_equipment("Bat1")
        dw.add_pv(PV("PV1", 5.0, 30.0, 180.0))

        names = dw.equipment_names()
        assert "Bat1" not in names
        assert "PV1" in names

        result = dw.step()
        assert "time" in result


class TestCheckpointRoundTrip:
    @pytest.mark.xfail(
        reason="checkpoint deserialization does not yet support dynamically added equipment"
    )
    def test_added_equipment_persists_through_checkpoint(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        dw.add_battery(Battery("TestBat", 10.0))
        dw.step()

        state = dw.save_state()
        dw.load_state(state)

        assert "TestBat" in dw.equipment_names()
        result = dw.step()
        assert "time" in result
