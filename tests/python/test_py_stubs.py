"""Final type stub audit for the hares public API."""

from __future__ import annotations

import hares
import hares._hares as _hares
from hares import SimulationConfig


# ---------------------------------------------------------------------------
# __all__ consistency
# ---------------------------------------------------------------------------


class TestAllConsistency:
    """__all__ matches actual module exports -- no missing, no extra."""

    def test_all_names_are_importable(self) -> None:
        for name in hares.__all__:
            assert hasattr(hares, name), f"{name!r} in __all__ but not importable"

    def test_no_py_prefixed_names_in_all(self) -> None:
        leaked = [n for n in hares.__all__ if n.startswith("Py")]
        assert leaked == [], f"Py-prefixed names leaked into __all__: {leaked}"

    def test_all_matches_module_exports(self) -> None:
        """Every name in __all__ resolves and every re-export is listed."""
        declared = set(hares.__all__)
        # Verify declared names actually exist
        for name in declared:
            assert hasattr(hares, name), f"{name!r} declared but missing"

        # Verify public names imported from _hares are in __all__
        # (guards against silent re-exports that were forgotten)
        imported_from_hares = {
            name
            for name in vars(hares)
            if not name.startswith("_") and hasattr(_hares, name)
        }
        missing_from_all = imported_from_hares - declared
        assert missing_from_all == set(), (
            f"Names imported from _hares but absent from __all__: {missing_from_all}"
        )


# ---------------------------------------------------------------------------
# Spot-check: Dwelling interface
# ---------------------------------------------------------------------------


class TestDwellingInterface:
    def test_has_expected_classmethods(self) -> None:
        assert callable(hares.Dwelling.from_hpxml)

    def test_has_expected_instance_methods(self) -> None:
        expected = [
            "initialize",
            "timesteps",
            "simulate",
            "step",
            "apply_control",
            "add_actor",
            "add_actor_by_name",
            "set_price_signal",
            "set_grid_voltage",
            "telemetry",
            "equipment_descriptors",
            "equipment_names",
            "add_battery",
            "add_pv",
            "add_ev",
            "remove_equipment",
            "replace_equipment",
            "update_equipment",
            "validate_control",
            "results",
            "metrics",
            "reset_with_seed",
            "save_state",
            "load_state",
            "surface_ids",
            "roof_planes",
            "pv_candidates",
            "set_electric_tariff",
            "set_gas_tariff",
            "billing_summaries",
            "tariff_telemetry",
        ]
        for method in expected:
            assert hasattr(hares.Dwelling, method), (
                f"Dwelling missing expected method: {method!r}"
            )


# ---------------------------------------------------------------------------
# Spot-check: Battery interface
# ---------------------------------------------------------------------------


class TestBatteryInterface:
    def test_is_constructable_with_required_args(self) -> None:
        b = hares.Battery(name="test-battery", capacity_kwh=10.0)
        assert b.name == "test-battery"
        assert b.capacity_kwh == 10.0

    def test_optional_limits_default_to_none(self) -> None:
        b = hares.Battery(name="b", capacity_kwh=5.0)
        assert b.max_charge_kw is None
        assert b.max_discharge_kw is None

    def test_limits_round_trip_when_provided(self) -> None:
        b = hares.Battery(
            name="b", capacity_kwh=5.0, max_charge_kw=2.5, max_discharge_kw=3.0
        )
        assert b.max_charge_kw == 2.5
        assert b.max_discharge_kw == 3.0

    def test_repr_is_non_empty_string(self) -> None:
        b = hares.Battery(name="b", capacity_kwh=5.0)
        assert isinstance(repr(b), str) and repr(b)


# ---------------------------------------------------------------------------
# Spot-check: ControlSignal interface
# ---------------------------------------------------------------------------


class TestControlSignalInterface:
    def test_thermal_setpoint_smoke(self) -> None:
        sig = hares.ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
        assert sig is not None

    def test_power_setpoint_smoke(self) -> None:
        sig = hares.ControlSignal.power_setpoint(kw=3.0)
        assert sig is not None

    def test_to_dict_returns_dict(self) -> None:
        sig = hares.ControlSignal.thermal_setpoint(heat_c=20.0)
        d = sig.to_dict()
        assert isinstance(d, dict)
        assert d  # non-empty

    def test_from_dict_round_trip(self) -> None:
        sig = hares.ControlSignal.power_setpoint(kw=2.0)
        d = sig.to_dict()
        sig2 = hares.ControlSignal.from_dict(d)
        assert sig2.to_dict() == d

    def test_repr_is_non_empty_string(self) -> None:
        sig = hares.ControlSignal.thermal_setpoint(heat_c=18.0)
        assert isinstance(repr(sig), str) and repr(sig)

    def test_has_expected_factory_methods(self) -> None:
        factories = [
            "power_setpoint",
            "thermal_setpoint",
            "thermal_setpoint_delta",
            "soc_target",
            "humidity_setpoint",
            "power_limit",
            "duty_cycle",
            "grid_connect",
            "self_consumption",
            "curtailment_percent",
            "reactive_setpoint",
            "load_fraction",
            "mode_override",
            "mode_override_str",
            "mode_override_release",
            "demand_response",
            "demand_response_str",
            "inverter_priority_mode",
            "protocol_native",
            "power_factor_setpoint",
            "ideal_capacity_mode_override",
        ]
        for name in factories:
            assert hasattr(hares.ControlSignal, name), (
                f"ControlSignal missing factory: {name!r}"
            )


# ---------------------------------------------------------------------------
# Spot-check: EndUse enum
# ---------------------------------------------------------------------------


class TestEndUseEnum:
    def test_standard_variants_are_accessible(self) -> None:
        variants = [
            "HVAC_HEATING",
            "HVAC_COOLING",
            "WATER_HEATING",
            "LIGHTING",
            "PLUG_LOADS",
            "REFRIGERATION",
            "VENTILATION",
            "BATTERY",
            "PV",
            "EV",
            "GENERATOR",
            "DEHUMIDIFIER",
            "OTHER",
        ]
        for v in variants:
            assert hasattr(hares.EndUse, v), f"EndUse missing variant: {v!r}"

    def test_hvac_heating_is_standard(self) -> None:
        assert hares.EndUse.HVAC_HEATING.is_standard()

    def test_custom_is_not_standard(self) -> None:
        custom = hares.EndUse.custom("my_load")
        assert not custom.is_standard()

    def test_as_str_returns_string(self) -> None:
        result = hares.EndUse.HVAC_HEATING.as_str()
        assert isinstance(result, str) and result

    def test_equality_and_hash(self) -> None:
        a = hares.EndUse.HVAC_HEATING
        b = hares.EndUse.HVAC_HEATING
        assert a == b
        assert hash(a) == hash(b)

    def test_different_variants_not_equal(self) -> None:
        assert hares.EndUse.HVAC_HEATING != hares.EndUse.HVAC_COOLING


# ---------------------------------------------------------------------------
# Spot-check: FuelType enum
# ---------------------------------------------------------------------------


class TestFuelTypeEnum:
    def test_factory_constructors(self) -> None:
        assert hares.FuelType.electric() is not None
        assert hares.FuelType.gas() is not None
        assert hares.FuelType.propane() is not None
        assert hares.FuelType.oil() is not None
        assert hares.FuelType.no_fuel() is not None

    def test_equality(self) -> None:
        assert hares.FuelType.electric() == hares.FuelType.electric()
        assert hares.FuelType.electric() != hares.FuelType.gas()

    def test_from_str(self) -> None:
        ft = hares.FuelType.from_str("Electric")
        assert ft == hares.FuelType.electric()

    def test_repr_and_str_are_strings(self) -> None:
        ft = hares.FuelType.electric()
        assert isinstance(repr(ft), str)
        assert isinstance(str(ft), str)


# ---------------------------------------------------------------------------
# Spot-check: Mode enum
# ---------------------------------------------------------------------------


class TestModeEnum:
    def test_off_factory(self) -> None:
        m = hares.Mode.off()
        assert m is not None

    def test_equality(self) -> None:
        assert hares.Mode.off() == hares.Mode.off()
        assert hares.Mode.off() != hares.Mode.heating()

    def test_operating_mode_alias(self) -> None:
        # OperatingMode must be the same class as Mode
        assert hares.OperatingMode is hares.Mode

    def test_factory_methods_present(self) -> None:
        factories = [
            "off", "heating", "cooling", "standby", "defrost",
            "charging", "discharging", "heating_hp", "heating_er",
            "heating_hp_and_er", "heat_pump_wh", "backup_element",
        ]
        for name in factories:
            assert hasattr(hares.Mode, name), f"Mode missing factory: {name!r}"


# ---------------------------------------------------------------------------
# Spot-check: SimulationConfig interface
# ---------------------------------------------------------------------------


class TestSimulationConfigInterface:
    def test_construction(self) -> None:
        cfg = hares.SimulationConfig(start_time="2024-01-01T00:00:00Z", duration=3600)
        assert cfg is not None

    def test_properties_readable_after_construction(self) -> None:
        cfg = hares.SimulationConfig(start_time="2024-01-01T00:00:00Z", duration=3600, time_res=60)
        assert cfg.duration == 3600
        assert cfg.time_res == 60

    def test_mutable_properties_are_settable(self) -> None:
        cfg = hares.SimulationConfig(start_time="2024-01-01T00:00:00Z", duration=3600)
        cfg.duration = 7200
        assert cfg.duration == 7200

    def test_repr_is_non_empty_string(self) -> None:
        cfg = hares.SimulationConfig(start_time="2024-01-01T00:00:00Z", duration=3600)
        assert isinstance(repr(cfg), str) and repr(cfg)

    def test_optional_properties_accept_none(self) -> None:
        cfg = hares.SimulationConfig(
            start_time="2024-01-01T00:00:00Z", duration=3600, output_path=None, civil_timezone=None
        )
        assert cfg.output_path is None
        assert cfg.civil_timezone is None
