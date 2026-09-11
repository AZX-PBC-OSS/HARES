"""Reactive power / power-factor Python bindings tests.

Covers:
  (a) Battery configured with power_factor=0.9 charging produces Q>0
      telemetry with Q/P ≈ tan(acos(0.9)).
  (b) Battery accepts a reactive / power-factor setpoint from Python and
      telemetry reflects it.
  (c) EV accepts reactive / power-factor setpoint from Python and
      telemetry reflects it (smart-inverter var control, IEEE 1547-2018 /
      SAE J3072). Default EV (pf=1.0) emits Q == 0.
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
# (c) EV smart-inverter var control — acceptance semantics, mirrors battery
# ---------------------------------------------------------------------------


class TestEvPowerFactorCharging:
    def test_charging_with_pf_0_9_produces_reactive_power_q_positive(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        ev = EV(
            "TestEV",
            capacity_kwh=75.0,
            max_charging_kw=7.2,
            initial_soc=0.5,
            initial_connection_state=EvConnectionState.HomePluggedIn,
            power_factor=0.9,
            charger_capacity_kva=15.0,
        )
        dw.add_ev(ev)
        dw.step()

        tel = _equipment_telemetry(dw, "TestEV")
        p = tel["active_power_kw"]
        q = tel["reactive_power_kvar"]
        assert p > 0.0, f"EV should be charging (P>0), got P={p}"
        assert q > 0.0, f"charging with pf=0.9 should yield Q>0 (absorbing), got Q={q}"

        expected_ratio = math.tan(math.acos(0.9))
        actual_ratio = q / p
        assert math.isclose(actual_ratio, expected_ratio, rel_tol=1e-6), (
            f"Q/P={actual_ratio} ≠ tan(acos(0.9))={expected_ratio}"
        )

    def test_core_output_reactive_matches_telemetry(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        ev = EV(
            "TestEV",
            capacity_kwh=75.0,
            max_charging_kw=7.2,
            initial_soc=0.5,
            initial_connection_state=EvConnectionState.HomePluggedIn,
            power_factor=0.9,
            charger_capacity_kva=15.0,
        )
        dw.add_ev(ev)
        dw.step()

        co = _equipment_core_output(dw, "TestEV")
        tel = _equipment_telemetry(dw, "TestEV")
        assert co.reactive_power_kvar is not None
        assert math.isclose(
            co.reactive_power_kvar, tel["reactive_power_kvar"], rel_tol=0, abs_tol=1e-12
        ), "CoreOutput Q and telemetry Q must agree"


class TestEvReactiveSetpoint:
    def _make_ev_dwelling(self, **ev_kw):
        from ochre_next import EV, EvConnectionState

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        defaults = dict(
            capacity_kwh=75.0,
            max_charging_kw=7.2,
            initial_soc=0.5,
            initial_connection_state=EvConnectionState.HomePluggedIn,
        )
        defaults.update(ev_kw)
        ev = EV("TestEV", **defaults)
        dw.add_ev(ev)
        return dw

    def test_reactive_setpoint_reflected_in_telemetry(self):
        from ochre_next import ControlSignal

        dw = self._make_ev_dwelling(
            power_factor=0.9, charger_capacity_kva=10.0
        )
        dw.apply_control("TestEV", ControlSignal.reactive_setpoint(1.5))
        dw.apply_control("TestEV", ControlSignal.power_setpoint(2.0))
        dw.step()

        tel = _equipment_telemetry(dw, "TestEV")
        q = tel["reactive_power_kvar"]
        assert math.isclose(q, 1.5, rel_tol=1e-6), (
            f"ReactiveSetpoint(1.5) should override baseline pf, got Q={q}"
        )

    def test_power_factor_setpoint_reflected_in_telemetry(self):
        from ochre_next import ControlSignal

        dw = self._make_ev_dwelling()
        dw.apply_control("TestEV", ControlSignal.power_factor_setpoint(0.8))
        dw.apply_control("TestEV", ControlSignal.power_setpoint(2.0))
        dw.step()

        tel = _equipment_telemetry(dw, "TestEV")
        p = tel["active_power_kw"]
        q = tel["reactive_power_kvar"]
        assert p > 0.0
        expected_q = p * math.tan(math.acos(0.8))
        assert math.isclose(q, expected_q, rel_tol=1e-6), (
            f"PowerFactorSetpoint(0.8): expected Q={expected_q}, got Q={q}"
        )

    def test_power_setpoint_with_reactive_kvar_accepted(self):
        from ochre_next import ControlSignal

        dw = self._make_ev_dwelling(charger_capacity_kva=10.0)
        dw.apply_control(
            "TestEV", ControlSignal.power_setpoint(2.0, reactive_kvar=0.75)
        )
        dw.step()

        tel = _equipment_telemetry(dw, "TestEV")
        q = tel["reactive_power_kvar"]
        assert math.isclose(q, 0.75, rel_tol=1e-6), (
            f"PowerSetpoint(reactive=0.75) should set Q=0.75, got Q={q}"
        )

    def test_real_power_unchanged_by_reactive_setpoint(self):
        """Sending a ReactiveSetpoint must not alter real power."""
        from ochre_next import ControlSignal

        dw = self._make_ev_dwelling(power_factor=0.9, charger_capacity_kva=10.0)
        dw.apply_control("TestEV", ControlSignal.power_setpoint(3.0))
        dw.step()
        tel_before = _equipment_telemetry(dw, "TestEV")
        p_before = tel_before["active_power_kw"]

        dw.apply_control("TestEV", ControlSignal.reactive_setpoint(0.5))
        dw.step()
        tel_after = _equipment_telemetry(dw, "TestEV")
        p_after = tel_after["active_power_kw"]
        q_after = tel_after["reactive_power_kvar"]

        assert math.isclose(p_after, p_before, rel_tol=1e-6), (
            f"Real power changed after ReactiveSetpoint: {p_before} → {p_after}"
        )
        assert math.isclose(q_after, 0.5, rel_tol=1e-6), (
            f"ReactiveSetpoint(0.5) should give Q=0.5, got Q={q_after}"
        )


class TestDefaultEvZeroQ:
    def test_default_ev_emits_q_zero_and_real_power_unchanged(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = make_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        ev = EV(
            "TestEV",
            capacity_kwh=75.0,
            max_charging_kw=7.2,
            initial_soc=0.5,
            initial_connection_state=EvConnectionState.HomePluggedIn,
        )
        dw.add_ev(ev)
        dw.apply_control("TestEV", ControlSignal.power_setpoint(3.0))
        dw.step()

        co = _equipment_core_output(dw, "TestEV")
        tel = _equipment_telemetry(dw, "TestEV")
        assert co.reactive_power_kvar is not None, (
            "EV with REACTIVE cap must set reactive_power_kvar to Some"
        )
        assert abs(co.reactive_power_kvar) < 1e-9, (
            f"Default pf=1.0 should give Q≈0, got {co.reactive_power_kvar}"
        )
        assert abs(tel["reactive_power_kvar"]) < 1e-9, (
            f"Telemetry Q should be ≈0 for default EV, got {tel['reactive_power_kvar']}"
        )
        assert tel["active_power_kw"] > 0, "Real power should be >0 with power setpoint"


# ---------------------------------------------------------------------------
# (d) ZIP pf override via config dict — Q changes, P bit-identical
# ---------------------------------------------------------------------------


class TestZipPfOverride:
    def test_zip_pf_override_changes_q_leaves_p_identical(self):
        from ochre_next import Dwelling

        # July afternoon so the AC compressor actually runs (a pf override
        # retargets the compressor component; the resistive crankcase heater
        # that runs on cold winter standby emits Q == 0 regardless of pf).
        common = dict(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time="2019-07-01T14:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )

        # Baseline: no override.
        dw_base = Dwelling.from_hpxml(**common)
        dw_base.initialize()

        # Override: change Air Conditioner compressor pf from 0.96 → 0.9.
        dw_ov = Dwelling.from_hpxml(
            overrides={"Air Conditioner": {"zip": {"pf": 0.9}}},
            **common,
        )
        dw_ov.initialize()

        # Step until the compressor draws power (thermostat may take a few
        # minutes to call for cooling from the initialized zone state).
        compressor_kw = 0.0
        for _ in range(60):
            dw_base.step()
            dw_ov.step()
            tel = _equipment_telemetry(dw_base, "Air Conditioner")
            compressor_kw = tel.get("compressor_kw", 0.0)
            if compressor_kw > 0.0:
                break
        assert compressor_kw > 0.0, "AC compressor must run in a July afternoon hour"

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

        # Per-component model: Q = comp*tan(acos(pf_comp)) + fan*tan(acos(0.87)).
        # The override changes only the compressor arm.
        tel = _equipment_telemetry(dw_base, "Air Conditioner")
        fan_kw = tel.get("fan_kw", 0.0)
        fan_q = fan_kw * math.tan(math.acos(0.87))
        expected_q_base = compressor_kw * math.tan(math.acos(0.96)) + fan_q
        expected_q_ov = compressor_kw * math.tan(math.acos(0.9)) + fan_q
        assert math.isclose(q_base, expected_q_base, rel_tol=1e-6), (
            f"Baseline Q={q_base} ≠ comp*tan(acos(0.96))+fan*tan(acos(0.87))"
            f"={expected_q_base}"
        )
        assert math.isclose(q_ov, expected_q_ov, rel_tol=1e-6), (
            f"Override Q={q_ov} ≠ comp*tan(acos(0.9))+fan*tan(acos(0.87))"
            f"={expected_q_ov}"
        )

    def test_zip_override_telemetry_reactive_key_present(self):
        from ochre_next import Dwelling

        # July afternoon: the compressor must run for Q != 0 — on winter
        # standby only the resistive crankcase heater draws power and the
        # per-component model correctly reports Q == 0 for it.
        common = dict(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time="2019-07-01T14:00:00",
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

        tel = None
        for _ in range(60):
            dw.step()
            tel = _equipment_telemetry(dw, "Air Conditioner")
            if tel.get("compressor_kw", 0.0) > 0.0:
                break
        assert tel is not None and tel.get("compressor_kw", 0.0) > 0.0, (
            "AC compressor must run in a July afternoon hour"
        )
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


# ---------------------------------------------------------------------------
# (f) ZIP / power-factor inspection surface
#     (Dwelling.equipment_zip and Equipment.resolved_zip)
# ---------------------------------------------------------------------------

ZIP_KEYS = {"zp", "ip", "pp", "zq", "iq", "pq", "pf", "v0", "real_power_zip_applies"}


def _equipment_resolved_zip(dw, name):
    """Return the resolved_zip property of the named equipment object."""
    for eq in dw.equipment():
        if eq.name == name:
            return eq.resolved_zip
    raise KeyError(name)


class TestResolvedZipInspection:
    def _add_battery(self, dw, **kw):
        from ochre_next import Battery

        bat = Battery(
            "Bat",
            10.0,
            max_charge_kw=5.0,
            max_discharge_kw=5.0,
            initial_soc=0.5,
            standby_power_w=0.0,
            **kw,
        )
        dw.add_battery(bat)

    def test_default_battery_resolved_zip_pf_unity(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        dw.initialize()
        self._add_battery(dw)

        zip_dict = dw.equipment_zip("Bat")
        assert zip_dict is not None
        assert set(zip_dict) == ZIP_KEYS
        assert zip_dict["pf"] == 1.0
        # Constant-power reactive-only ZIP (both polynomials (0, 0, 1)).
        assert (zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]) == (0.0, 0.0, 1.0)
        assert (zip_dict["zq"], zip_dict["iq"], zip_dict["pq"]) == (0.0, 0.0, 1.0)
        # The Equipment inspection object exposes the same view.
        assert _equipment_resolved_zip(dw, "Bat") == zip_dict

    def test_ashp_heater_resolved_zip_pf_084(self):
        from ochre_next import ASHPHeater, DwellingBlueprint, EndUse

        bp = DwellingBlueprint.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            duration_s=600,
            time_res_s=60,
        )
        bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
        bp.add_equipment(
            ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000)
        )
        dw = bp.build()
        dw.initialize()

        zip_dict = dw.equipment_zip("ASHP")
        assert zip_dict is not None
        # Primary component = compressor: ASHP Heater class default pf 0.84.
        assert zip_dict["pf"] == 0.84
        # Rule R1: real side forced to constant power.
        assert (zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]) == (0.0, 0.0, 1.0)
        assert _equipment_resolved_zip(dw, "ASHP") == zip_dict

    def test_user_zip_override_pf_09_visible(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-07-01T14:00:00",
            duration_s=600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
            overrides={"Air Conditioner": {"zip": {"pf": 0.9}}},
        )
        dw.initialize()

        zip_dict = dw.equipment_zip("Air Conditioner")
        assert zip_dict is not None
        assert zip_dict["pf"] == 0.9
        # The override merges over the class row: reactive coefficients keep
        # the Air Conditioner class values (Hajagos & Danai 1998).
        assert math.isclose(zip_dict["zq"], 12.53)
        assert math.isclose(zip_dict["iq"], -21.11)
        assert math.isclose(zip_dict["pq"], 9.58)
        assert _equipment_resolved_zip(dw, "Air Conditioner") == zip_dict

    def test_battery_power_factor_setpoint_updates_resolved_zip(self):
        from ochre_next import ControlSignal

        dw = make_dwelling(duration_s=600, time_res_s=60)
        dw.initialize()
        self._add_battery(dw)

        assert dw.equipment_zip("Bat")["pf"] == 1.0
        dw.apply_control("Bat", ControlSignal.power_factor_setpoint(0.8))
        # Control signals dispatch during the step.
        dw.step()
        zip_dict = dw.equipment_zip("Bat")
        assert zip_dict["pf"] == 0.8, (
            f"PowerFactorSetpoint(0.8) must be visible in resolved_zip, "
            f"got pf={zip_dict['pf']}"
        )
        assert _equipment_resolved_zip(dw, "Bat") == zip_dict

    def test_unknown_equipment_name_raises_value_error(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        dw.initialize()
        with pytest.raises(ValueError, match="not found"):
            dw.equipment_zip("No Such Equipment")
