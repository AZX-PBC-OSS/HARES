"""Tests for equipment mutation API -- add, remove, replace, update equipment at runtime."""

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
        # Take a baseline step without battery
        baseline = dw.step()

        dw2 = _make_dwelling()
        bat = Battery("TestBat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0)
        dw2.add_battery(bat)
        result = dw2.step()
        assert "timestamp" in result

        # Battery should appear in telemetry
        tel = dw2.telemetry().equipment()
        assert "TestBat" in tel["names"], "Battery should appear in equipment telemetry"

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
        assert "timestamp" in result

        # PV should appear in equipment telemetry after stepping
        tel = dw.telemetry().equipment()
        assert "TestPV" in tel["names"], "PV should appear in equipment telemetry"

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
        assert "timestamp" in result

        # EV should appear in equipment telemetry after stepping
        tel = dw.telemetry().equipment()
        assert "TestEV" in tel["names"], "EV should appear in equipment telemetry"

    def test_add_protocol_bridge_appears_in_equipment_names(self):
        from ochre_next import ProtocolBridge

        dw = _make_dwelling()
        bridge = ProtocolBridge("TestBridge")
        dw.add_protocol_bridge(bridge)
        assert "TestBridge" in dw.equipment_names()

    def test_add_protocol_bridge_step_succeeds(self):
        from ochre_next import ProtocolBridge

        dw = _make_dwelling()
        bridge = ProtocolBridge("TestBridge")
        dw.add_protocol_bridge(bridge)
        result = dw.step()
        assert "timestamp" in result

        tel = dw.telemetry().equipment()
        assert "TestBridge" in tel["names"], (
            "ProtocolBridge should appear in equipment telemetry"
        )

    def test_add_protocol_bridge_with_registered_protocols(self):
        from ochre_next import ProtocolBridge

        dw = _make_dwelling()
        bridge = ProtocolBridge("FilteredBridge", registered_protocols=[1, 42, 99])
        dw.add_protocol_bridge(bridge)
        assert "FilteredBridge" in dw.equipment_names()
        result = dw.step()
        assert "timestamp" in result

    def test_add_protocol_bridge_with_json_handler(self):
        from ochre_next import ProtocolBridge

        dw = _make_dwelling()
        bridge = ProtocolBridge("RoutedBridge", json_handlers=[17])
        dw.add_protocol_bridge(bridge)
        assert "RoutedBridge" in dw.equipment_names()
        result = dw.step()
        assert "timestamp" in result

        tel = dw.telemetry().equipment()
        assert "RoutedBridge" in tel["names"], (
            "ProtocolBridge with JSON handler should appear in equipment telemetry"
        )


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
        from ochre_next._hares import HaresConfigError

        dw = _make_dwelling()
        # HaresConfigError is a ValueError subclass; catch the specific type.
        with pytest.raises(HaresConfigError, match="not found"):
            dw.remove_equipment("NoSuchEquipment")

    def test_step_after_remove_succeeds(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0)
        dw.add_battery(bat)
        dw.remove_equipment("TestBat")
        result = dw.step()
        assert "timestamp" in result

        # After removal, the battery must not appear in equipment telemetry
        tel = dw.telemetry().equipment()
        assert "TestBat" not in tel["names"], (
            "Removed battery should not appear in equipment telemetry"
        )


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
        from ochre_next._hares import HaresConfigError

        dw = _make_dwelling()
        bat = Battery("NoSuch", 10.0)
        # HaresConfigError is a ValueError subclass; catch the specific type.
        with pytest.raises(HaresConfigError, match="not found"):
            dw.replace_equipment("NoSuch", bat)

    def test_step_after_replace_succeeds(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat1 = Battery("TestBat", 10.0)
        dw.add_battery(bat1)

        bat2 = Battery("TestBat", 20.0, max_charge_kw=10.0, max_discharge_kw=10.0)
        dw.replace_equipment("TestBat", bat2)

        result = dw.step()
        assert "timestamp" in result

        # After replacement, the battery should still appear in telemetry
        tel = dw.telemetry().equipment()
        assert "TestBat" in tel["names"], (
            "Replaced battery should still appear in equipment telemetry"
        )
        # SOC should be a valid number (not NaN), confirming the new battery is active
        idx = tel["names"].index("TestBat")
        import math
        assert not math.isnan(tel["soc"][idx]), (
            "Replaced battery SOC should not be NaN"
        )


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

        result = dw.step()
        assert "timestamp" in result

        # Battery should be present in telemetry with valid SOC after LUT update
        tel = dw.telemetry().equipment()
        assert "TestBat" in tel["names"]
        idx = tel["names"].index("TestBat")
        import math
        assert not math.isnan(tel["soc"][idx]), (
            "Battery SOC should be valid after OCV table update"
        )
        assert not math.isnan(tel["power_kw"][idx]), (
            "Battery power should be valid after OCV table update"
        )

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
        assert "timestamp" in result

        # Battery should be present in telemetry with valid SOC after Uneg table update
        tel = dw.telemetry().equipment()
        assert "TestBat" in tel["names"]
        idx = tel["names"].index("TestBat")
        import math
        assert not math.isnan(tel["soc"][idx]), (
            "Battery SOC should be valid after Uneg table update"
        )
        assert not math.isnan(tel["power_kw"][idx]), (
            "Battery power should be valid after Uneg table update"
        )

    def test_update_nonexistent_raises(self):
        from ochre_next._hares import HaresEquipmentError

        dw = _make_dwelling()
        # HaresEquipmentError is a ValueError subclass; catch the specific type.
        with pytest.raises(HaresEquipmentError, match="not found"):
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
        assert "timestamp" in result

        # Battery should report valid telemetry after LUT injection
        tel = dw.telemetry().equipment()
        assert "TestBat" in tel["names"]
        idx = tel["names"].index("TestBat")
        import math
        assert not math.isnan(tel["soc"][idx]), (
            "Battery SOC should be valid after set_equipment_lut"
        )
        assert not math.isnan(tel["power_kw"][idx]), (
            "Battery power should be valid after set_equipment_lut"
        )

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
        assert "timestamp" in result

        # After clearing the custom LUT, the battery should still function
        # with valid telemetry (reverts to default OCV curve)
        tel = dw.telemetry().equipment()
        assert "TestBat" in tel["names"]
        idx = tel["names"].index("TestBat")
        import math
        assert not math.isnan(tel["soc"][idx]), (
            "Battery SOC should be valid after clearing OCV LUT"
        )
        assert not math.isnan(tel["power_kw"][idx]), (
            "Battery power should be valid after clearing OCV LUT"
        )

    def test_set_lut_nonexistent_raises(self):
        from ochre_next import LutType
        from ochre_next._hares import HaresEquipmentError

        dw = _make_dwelling()
        # HaresEquipmentError is a ValueError subclass; catch the specific type.
        with pytest.raises(HaresEquipmentError, match="not found"):
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
        assert "timestamp" in result

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
        assert "timestamp" in result


class TestBatteryConfigParams:
    def test_soc_limits_add_and_step(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0, initial_soc=0.8, min_soc=0.15, max_soc=0.95)
        dw.add_battery(bat)
        result = dw.step()
        assert "timestamp" in result

    def test_soc_limits_round_trip_via_getters(self):
        from ochre_next import Battery

        bat = Battery("B", 10.0, initial_soc=0.8, min_soc=0.15, max_soc=0.95)
        assert bat.initial_soc == 0.8
        assert bat.min_soc == 0.15
        assert bat.max_soc == 0.95

    def test_chemistry_lfp_add_and_step(self):
        from ochre_next import Battery, BatteryChemistry

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0, chemistry=BatteryChemistry.Lfp)
        dw.add_battery(bat)
        result = dw.step()
        assert "timestamp" in result

    def test_chemistry_getter_round_trips(self):
        from ochre_next import Battery, BatteryChemistry

        bat = Battery("B", 10.0, chemistry=BatteryChemistry.Lfp)
        assert bat.chemistry == BatteryChemistry.Lfp

    def test_standby_and_self_discharge_add_and_step(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0, standby_power_w=10.0, self_discharge_pct_per_day=0.05)
        dw.add_battery(bat)
        result = dw.step()
        assert "timestamp" in result

    def test_standby_and_self_discharge_getters(self):
        from ochre_next import Battery

        bat = Battery("B", 10.0, standby_power_w=10.0, self_discharge_pct_per_day=0.05)
        assert bat.standby_power_w == 10.0
        assert bat.self_discharge_pct_per_day == 0.05

    def test_repr_defaults_only(self):
        from ochre_next import Battery

        bat = Battery("MyBat", 5.0)
        r = repr(bat)
        assert "MyBat" in r
        assert "capacity_kwh=5" in r
        assert "initial_soc" not in r
        assert "chemistry" not in r

    def test_repr_with_params(self):
        from ochre_next import Battery, BatteryChemistry

        bat = Battery(
            "MyBat",
            5.0,
            chemistry=BatteryChemistry.Nmc,
            initial_soc=0.5,
            standby_power_w=5.0,
        )
        r = repr(bat)
        assert "initial_soc=0.5" in r
        assert "standby_power_w=5" in r
        assert "Nmc" in r

    def test_min_soc_prevents_deep_discharge(self):
        """Battery with min_soc=0.2 should never discharge below 0.2."""
        from ochre_next import Battery, ControlSignal

        dw = _make_dwelling()
        bat = Battery("Bat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0,
                      initial_soc=0.3, min_soc=0.2)
        dw.add_battery(bat)
        dw.apply_control("Bat", ControlSignal.power_setpoint(-5.0))
        for _ in range(20):
            dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("Bat")
        soc = tel["soc"][idx]
        assert soc >= 0.19, f"SOC {soc} dropped below min_soc=0.2"

    def test_initial_soc_applied_in_telemetry(self):
        """Battery with initial_soc=0.8 must report SOC=0.8 on first step."""
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("Bat", 10.0, initial_soc=0.8)
        dw.add_battery(bat)
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("Bat")
        assert abs(tel["soc"][idx] - 0.8) < 0.05, (
            f"Initial SOC should be ~0.8, got {tel['soc'][idx]}"
        )

    def test_inverted_thermal_params_handled_gracefully(self):
        """Inverted min_charge_temp_c > full_power_temp_c must not silently corrupt state.

        The Rust equipment should either clamp, swap, or return an error. What it must
        not do is proceed with undefined behaviour. We verify by confirming the
        simulation either raises a clear exception or completes a step without producing
        a NaN power value.
        """
        from ochre_next import Battery

        dw = _make_dwelling()
        bat = Battery("TestBat", 10.0, min_charge_temp_c=5.0, full_power_temp_c=0.0)
        try:
            dw.add_battery(bat)
            result = dw.step()
            assert "timestamp" in result
            tel = dw.telemetry().equipment()
            names = tel["names"]
            powers = tel["power_kw"]
            idx = names.index("TestBat")
            import math
            assert not math.isnan(powers[idx]), "Battery power is NaN with inverted thermal params"
        except (ValueError, RuntimeError):
            pass  # explicit error from Rust is also acceptable


class TestCheckpointRoundTrip:
    def test_added_equipment_persists_through_checkpoint(self):
        from ochre_next import Battery

        dw = _make_dwelling()
        dw.add_battery(Battery("TestBat", 10.0))
        dw.step()

        state = dw.save_state()
        dw.load_state(state)

        assert "TestBat" in dw.equipment_names()
        result = dw.step()
        assert "timestamp" in result
