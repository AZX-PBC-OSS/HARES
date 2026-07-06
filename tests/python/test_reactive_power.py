"""Reactive power / power-factor Python bindings tests.

Covers:
  (a) Battery configured with power_factor=0.9 charging produces Q>0
      telemetry with Q/P ≈ tan(acos(0.9)).
  (b) Battery accepts a reactive / power-factor setpoint from Python and
      telemetry reflects it.
  (c) EV given any reactive command raises a Python exception.
  (d) ZIP pf override via config dict changes REACTIVE_POWER_KVAR for that
      equipment while leaving real power bit-identical.
  (e) Default battery (pf=1.0) emits Q == 0.
"""

import math

import pytest

from conftest import make_dwelling

ROOT = __import__("pathlib").Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")


def _battery_kw(name, equipment_list):
    idx = equipment_list["names"].index(name)
    return equipment_list["power_kw"][idx]


def _equipment_telemetry(dw, name):
    """Return the telemetry dict for *name* after the latest step."""
    for eq in dw.equipment():
        if eq.name == name:
            return eq.telemetry()
    raise KeyError(name)


def _equipment_core_output(dw, name):
    for eq in dw.equipment():
        if eq.name == name:
            return eq.core_output
    raise KeyError(name)


# ---------------------------------------------------------------------------
# (a) Battery pf=0.9 charging → Q>0, Q/P ≈ tan(acos(0.9))
# ---------------------------------------------------------------------------


class TestBatteryPowerFactorCharging:
    def test_charging_q_positive_and_ratio_matches_tan_acos_pf(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.3,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
            power_factor=0.9,
        )
        dw.add_battery(bat)
        dw.apply_control("Bat", ControlSignal.power_setpoint(3.0))
        dw.step()

        tel = _equipment_telemetry(dw, "Bat")
        p = tel["active_power_kw"]
        q = tel["reactive_power_kvar"]
        assert p > 0.0, f"battery should be charging (P>0), got P={p}"
        assert q > 0.0, f"charging with pf=0.9 should yield Q>0 (absorbing), got Q={q}"

        expected_ratio = math.tan(math.acos(0.9))
        actual_ratio = q / p
        assert math.isclose(actual_ratio, expected_ratio, rel_tol=1e-6), (
            f"Q/P={actual_ratio} ≠ tan(acos(0.9))={expected_ratio}"
        )

    def test_core_output_reactive_matches_telemetry(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.3,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
            power_factor=0.9,
        )
        dw.add_battery(bat)
        dw.apply_control("Bat", ControlSignal.power_setpoint(3.0))
        dw.step()

        co = _equipment_core_output(dw, "Bat")
        tel = _equipment_telemetry(dw, "Bat")
        assert co.reactive_power_kvar is not None
        assert math.isclose(
            co.reactive_power_kvar, tel["reactive_power_kvar"], rel_tol=0, abs_tol=1e-12
        ), "CoreOutput Q and telemetry Q must agree"


# ---------------------------------------------------------------------------
# (b) Battery accepts reactive / power-factor setpoint
# ---------------------------------------------------------------------------


class TestBatteryReactiveSetpoint:
    def test_reactive_setpoint_reflected_in_telemetry(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.5,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
            power_factor=0.9,
            inverter_capacity_kva=10.0,
        )
        dw.add_battery(bat)
        # ReactiveSetpoint takes precedence over baseline pf.
        dw.apply_control("Bat", ControlSignal.reactive_setpoint(1.5))
        # Also command charging so P>0.
        dw.apply_control("Bat", ControlSignal.power_setpoint(2.0))
        dw.step()

        tel = _equipment_telemetry(dw, "Bat")
        q = tel["reactive_power_kvar"]
        assert math.isclose(q, 1.5, rel_tol=1e-6), (
            f"ReactiveSetpoint(1.5) should override baseline pf, got Q={q}"
        )

    def test_power_factor_setpoint_reflected_in_telemetry(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.5,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
        )
        dw.add_battery(bat)
        # PowerFactorSetpoint updates pf; Q follows new pf baseline.
        dw.apply_control("Bat", ControlSignal.power_factor_setpoint(0.8))
        dw.apply_control("Bat", ControlSignal.power_setpoint(2.0))
        dw.step()

        tel = _equipment_telemetry(dw, "Bat")
        p = tel["active_power_kw"]
        q = tel["reactive_power_kvar"]
        assert p > 0.0
        expected_q = p * math.tan(math.acos(0.8))
        assert math.isclose(q, expected_q, rel_tol=1e-6), (
            f"PowerFactorSetpoint(0.8): expected Q={expected_q}, got Q={q}"
        )

    def test_power_setpoint_with_reactive_kvar_accepted(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.5,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
            inverter_capacity_kva=10.0,
        )
        dw.add_battery(bat)
        dw.apply_control(
            "Bat", ControlSignal.power_setpoint(2.0, reactive_kvar=0.75)
        )
        dw.step()

        tel = _equipment_telemetry(dw, "Bat")
        q = tel["reactive_power_kvar"]
        assert math.isclose(q, 0.75, rel_tol=1e-6), (
            f"PowerSetpoint(reactive=0.75) should set Q=0.75, got Q={q}"
        )


# ---------------------------------------------------------------------------
# (c) EV given reactive command raises a Python exception
# ---------------------------------------------------------------------------


class TestEvReactiveRejection:
    def _make_dwelling_with_ev(self):
        from ochre_next import EV

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        ev = EV("TestEV", capacity_kwh=75.0, max_charging_kw=7.2, initial_soc=0.5)
        dw.add_ev(ev)
        return dw

    def test_power_setpoint_with_reactive_raises(self):
        from ochre_next import ControlSignal

        dw = self._make_dwelling_with_ev()
        with pytest.raises(ValueError, match="EV does not support reactive power"):
            dw.apply_control(
                "TestEV", ControlSignal.power_setpoint(3.0, reactive_kvar=1.0)
            )

    def test_reactive_setpoint_raises(self):
        from ochre_next import ControlSignal

        dw = self._make_dwelling_with_ev()
        with pytest.raises(ValueError, match="unsupported control signal"):
            dw.apply_control("TestEV", ControlSignal.reactive_setpoint(1.0))

    def test_power_factor_setpoint_raises(self):
        from ochre_next import ControlSignal

        dw = self._make_dwelling_with_ev()
        with pytest.raises(ValueError, match="unsupported control signal"):
            dw.apply_control("TestEV", ControlSignal.power_factor_setpoint(0.95))

    def test_power_setpoint_with_zero_reactive_accepted(self):
        """PowerSetpoint with explicit Q=0 is a no-op, not an error."""
        from ochre_next import ControlSignal

        dw = self._make_dwelling_with_ev()
        dw.apply_control(
            "TestEV", ControlSignal.power_setpoint(3.0, reactive_kvar=0.0)
        )


# ---------------------------------------------------------------------------
# (d) ZIP pf override via config dict — Q changes, P bit-identical
# ---------------------------------------------------------------------------


class TestZipPfOverride:
    def test_zip_pf_override_changes_q_leaves_p_identical(self):
        from ochre_next import Dwelling

        common = dict(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )

        # Baseline: no override.
        dw_base = Dwelling.from_hpxml(**common)
        dw_base.initialize()
        dw_base.step()

        # Override: change Air Conditioner pf from default 0.96 → 0.9.
        dw_ov = Dwelling.from_hpxml(
            overrides={"Air Conditioner": {"zip": {"pf": 0.9}}},
            **common,
        )
        dw_ov.initialize()
        dw_ov.step()

        co_base = _equipment_core_output(dw_base, "Air Conditioner")
        co_ov = _equipment_core_output(dw_ov, "Air Conditioner")

        p_base = co_base.electric_kw
        p_ov = co_ov.electric_kw
        assert p_base is not None and p_ov is not None
        assert p_base == p_ov, (
            f"Real power must be bit-identical (Rule R1), got {p_base} vs {p_ov}"
        )

        q_base = co_base.reactive_power_kvar
        q_ov = co_ov.reactive_power_kvar
        assert q_base is not None and q_ov is not None
        assert q_base != q_ov, (
            f"Q must change with pf override, got baseline={q_base} override={q_ov}"
        )

        # Verify the Q values match the expected tan(acos(pf)) relationship.
        if p_base != 0.0:
            expected_q_base = p_base * math.tan(math.acos(0.96))
            expected_q_ov = p_base * math.tan(math.acos(0.9))
            assert math.isclose(q_base, expected_q_base, rel_tol=1e-6), (
                f"Baseline Q={q_base} ≠ P*tan(acos(0.96))={expected_q_base}"
            )
            assert math.isclose(q_ov, expected_q_ov, rel_tol=1e-6), (
                f"Override Q={q_ov} ≠ P*tan(acos(0.9))={expected_q_ov}"
            )

    def test_zip_override_telemetry_reactive_key_present(self):
        from ochre_next import Dwelling

        common = dict(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )

        dw = Dwelling.from_hpxml(
            overrides={"Air Conditioner": {"zip": {"pf": 0.9}}},
            **common,
        )
        dw.initialize()
        dw.step()

        tel = _equipment_telemetry(dw, "Air Conditioner")
        assert "reactive_power_kvar" in tel
        assert tel["reactive_power_kvar"] != 0.0


# ---------------------------------------------------------------------------
# (e) Default battery (pf=1.0) emits Q == 0
# ---------------------------------------------------------------------------


class TestDefaultBatteryZeroQ:
    def test_default_battery_q_zero(self):
        from ochre_next import Battery, ControlSignal

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.5,
            standby_power_w=0.0,
            charge_efficiency=1.0,
            discharge_efficiency=1.0,
        )
        dw.add_battery(bat)
        dw.apply_control("Bat", ControlSignal.power_setpoint(2.0))
        dw.step()

        co = _equipment_core_output(dw, "Bat")
        tel = _equipment_telemetry(dw, "Bat")
        assert co.reactive_power_kvar is not None, (
            "Battery with REACTIVE cap must set reactive_power_kvar to Some"
        )
        assert abs(co.reactive_power_kvar) < 1e-9, (
            f"Default pf=1.0 should give Q≈0, got {co.reactive_power_kvar}"
        )
        assert abs(tel["reactive_power_kvar"]) < 1e-9
