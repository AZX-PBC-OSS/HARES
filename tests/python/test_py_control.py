"""Tests for ControlSignal Python bindings."""

import pytest
from ochre_next import (
    ControlSignal,
    OperatingMode,
    Mode,
    DRLevel,
    InverterPriority,
    DutyCycleComponent,
)


class TestControlSignalConstructors:
    """Test all 18 ControlSignal constructors."""

    def test_power_setpoint(self):
        sig = ControlSignal.power_setpoint(1.5)
        assert sig.to_dict()["type"] == "PowerSetpoint"
        assert sig.to_dict()["active_power_kw"] == 1.5
        assert "PowerSetpoint" in repr(sig)

    def test_power_setpoint_with_reactive(self):
        sig = ControlSignal.power_setpoint(1.5, 0.5)
        d = sig.to_dict()
        assert d["reactive_power_kvar"] == 0.5

    def test_thermal_setpoint(self):
        sig = ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
        d = sig.to_dict()
        assert d["type"] == "ThermalSetpoint"
        assert d["heating_setpoint_c"] == 20.0
        assert d["cooling_setpoint_c"] == 24.0
        assert "ThermalSetpoint" in repr(sig)

    def test_thermal_setpoint_optional_params(self):
        sig = ControlSignal.thermal_setpoint(heat_c=20.0, deadband_c=2.0)
        d = sig.to_dict()
        assert d["heating_setpoint_c"] == 20.0
        assert d["deadband_c"] == 2.0

    def test_thermal_setpoint_delta(self):
        sig = ControlSignal.thermal_setpoint_delta(
            heating_delta_c=1.0, cooling_delta_c=-0.5
        )
        d = sig.to_dict()
        assert d["type"] == "ThermalSetpointDelta"
        assert d["heating_delta_c"] == 1.0
        assert d["cooling_delta_c"] == -0.5
        assert "ThermalSetpointDelta" in repr(sig)

    def test_ideal_capacity(self):
        sig = ControlSignal.ideal_capacity(5000.0)
        d = sig.to_dict()
        assert d["type"] == "IdealCapacity"
        assert d["capacity_w"] == 5000.0
        assert "IdealCapacity" in repr(sig)

    def test_load_fraction(self):
        sig = ControlSignal.load_fraction(0.75)
        d = sig.to_dict()
        assert d["type"] == "LoadFraction"
        assert d["fraction"] == 0.75
        assert "LoadFraction" in repr(sig)

    def test_mode_override_with_enum(self):
        sig = ControlSignal.mode_override(OperatingMode.Heating)
        d = sig.to_dict()
        assert d["type"] == "ModeOverride"
        assert d["mode"] == "Heating"
        assert "ModeOverride" in repr(sig)

    def test_mode_override_str(self):
        sig = ControlSignal.mode_override_str("Cooling")
        d = sig.to_dict()
        assert d["mode"] == "Cooling"

    def test_mode_override_str_invalid_raises(self):
        with pytest.raises(ValueError):
            ControlSignal.mode_override_str("InvalidMode")

    def test_demand_response_with_enum(self):
        sig = ControlSignal.demand_response(DRLevel.High)
        d = sig.to_dict()
        assert d["type"] == "DemandResponse"
        assert d["level"] == "High"
        assert "DemandResponse" in repr(sig)

    def test_demand_response_with_duration(self):
        sig = ControlSignal.demand_response(DRLevel.High, duration_s=3600.0)
        d = sig.to_dict()
        assert d["duration_s"] == 3600.0

    def test_demand_response_str(self):
        sig = ControlSignal.demand_response_str("Normal")
        d = sig.to_dict()
        assert d["level"] == "Normal"

    def test_demand_response_str_invalid_raises(self):
        with pytest.raises(ValueError):
            ControlSignal.demand_response_str("Invalid")

    def test_soc_target(self):
        sig = ControlSignal.soc_target(target=0.8, min=0.2, max=1.0)
        d = sig.to_dict()
        assert d["type"] == "SOCTarget"
        assert d["target_soc"] == 0.8
        assert d["min_soc"] == 0.2
        assert d["max_soc"] == 1.0
        assert "SOCTarget" in repr(sig)

    def test_soc_target_optional_params(self):
        sig = ControlSignal.soc_target(target=0.8)
        d = sig.to_dict()
        assert "min_soc" not in d
        assert "max_soc" not in d

    def test_humidity_setpoint(self):
        sig = ControlSignal.humidity_setpoint(target_rh=50.0, min_rh=40.0, max_rh=60.0)
        d = sig.to_dict()
        assert d["type"] == "HumiditySetpoint"
        assert d["target_rh"] == 50.0
        assert d["min_rh"] == 40.0
        assert d["max_rh"] == 60.0
        assert "HumiditySetpoint" in repr(sig)

    def test_power_limit(self):
        sig = ControlSignal.power_limit(max_power_kw=5.0, ramp_rate_kw_per_s=0.1)
        d = sig.to_dict()
        assert d["type"] == "PowerLimit"
        assert d["max_power_kw"] == 5.0
        assert d["ramp_rate_kw_per_s"] == 0.1
        assert "PowerLimit" in repr(sig)

    def test_power_limit_optional_params(self):
        sig = ControlSignal.power_limit(max_power_kw=5.0)
        d = sig.to_dict()
        assert "ramp_rate_kw_per_s" not in d

    def test_duty_cycle(self):
        sig = ControlSignal.duty_cycle(on_fraction=0.5, period_s=300.0)
        d = sig.to_dict()
        assert d["type"] == "DutyCycle"
        assert d["on_fraction"] == 0.5
        assert d["period_s"] == 300.0
        assert "DutyCycle" in repr(sig)

    def test_duty_cycle_with_component(self):
        sig = ControlSignal.duty_cycle(
            on_fraction=0.5, period_s=300.0, component=DutyCycleComponent.Compressor
        )
        d = sig.to_dict()
        assert d["component"] == "Compressor"

    def test_duty_cycle_with_backup_element(self):
        sig = ControlSignal.duty_cycle(
            on_fraction=0.5, component=DutyCycleComponent.BackupElement
        )
        d = sig.to_dict()
        assert d["component"] == "BackupElement"

    def test_duty_cycle_domain_validation(self):
        sig = ControlSignal.duty_cycle(on_fraction=1.5)
        d = sig.to_dict()
        assert d["on_fraction"] == 1.5

    def test_grid_connect(self):
        sig = ControlSignal.grid_connect(connected=True)
        d = sig.to_dict()
        assert d["type"] == "GridConnect"
        assert d["connected"] is True
        assert "GridConnect" in repr(sig)

    def test_grid_connect_disconnected(self):
        sig = ControlSignal.grid_connect(connected=False)
        d = sig.to_dict()
        assert d["connected"] is False

    def test_self_consumption(self):
        sig = ControlSignal.self_consumption(enabled=True, solar_only_charging=True)
        d = sig.to_dict()
        assert d["type"] == "SelfConsumption"
        assert d["enabled"] is True
        assert d["solar_only_charging"] is True
        assert "SelfConsumption" in repr(sig)

    def test_self_consumption_disabled(self):
        sig = ControlSignal.self_consumption(enabled=False, solar_only_charging=False)
        d = sig.to_dict()
        assert d["enabled"] is False
        assert d["solar_only_charging"] is False

    def test_curtailment_percent(self):
        sig = ControlSignal.curtailment_percent(50.0)
        d = sig.to_dict()
        assert d["type"] == "CurtailmentPercent"
        assert d["percent"] == 50.0
        assert "CurtailmentPercent" in repr(sig)

    def test_reactive_setpoint(self):
        sig = ControlSignal.reactive_setpoint(kvar=1.0)
        d = sig.to_dict()
        assert d["type"] == "ReactiveSetpoint"
        assert d["kvar"] == 1.0
        assert "ReactiveSetpoint" in repr(sig)

    def test_power_factor_setpoint(self):
        sig = ControlSignal.power_factor_setpoint(0.95)
        d = sig.to_dict()
        assert d["type"] == "PowerFactorSetpoint"
        assert d["power_factor"] == 0.95
        assert "PowerFactorSetpoint" in repr(sig)

    def test_inverter_priority_mode(self):
        sig = ControlSignal.inverter_priority_mode(InverterPriority.Watt)
        d = sig.to_dict()
        assert d["type"] == "InverterPriorityMode"
        assert d["priority"] == "Watt"
        assert "InverterPriorityMode" in repr(sig)

    def test_inverter_priority_mode_var(self):
        sig = ControlSignal.inverter_priority_mode(InverterPriority.Var)
        d = sig.to_dict()
        assert d["priority"] == "Var"

    def test_inverter_priority_mode_cpf(self):
        sig = ControlSignal.inverter_priority_mode(InverterPriority.Cpf)
        d = sig.to_dict()
        assert d["priority"] == "Cpf"

    def test_protocol_native(self):
        sig = ControlSignal.protocol_native(42, b"\x01\x02\x03")
        d = sig.to_dict()
        assert d["type"] == "ProtocolNative"
        assert d["protocol"] == 42
        assert d["payload"] == b"\x01\x02\x03"
        assert "ProtocolNative" in repr(sig)

    def test_protocol_native_round_trip(self):
        sig = ControlSignal.protocol_native(42, b"\x01\x02\x03")
        d = sig.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict()["protocol"] == 42

    def test_ideal_capacity_mode_override_auto(self):
        sig = ControlSignal.ideal_capacity_mode_override("auto")
        d = sig.to_dict()
        assert d["type"] == "IdealCapacityModeOverride"
        assert d["mode"] == "Auto"
        assert "IdealCapacityModeOverride" in repr(sig)

    def test_ideal_capacity_mode_override_on(self):
        sig = ControlSignal.ideal_capacity_mode_override("on")
        d = sig.to_dict()
        assert d["mode"] == "On"

    def test_ideal_capacity_mode_override_off(self):
        sig = ControlSignal.ideal_capacity_mode_override("off")
        d = sig.to_dict()
        assert d["mode"] == "Off"

    def test_ideal_capacity_mode_override_invalid_raises(self):
        with pytest.raises(ValueError):
            ControlSignal.ideal_capacity_mode_override("invalid")


class TestControlSignalFromDict:
    """Test from_dict parsing."""

    def test_from_dict_power_setpoint(self):
        d = {"type": "PowerSetpoint", "active_power_kw": 1.5}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["type"] == "PowerSetpoint"

    def test_from_dict_thermal_setpoint(self):
        d = {"type": "ThermalSetpoint", "heating_setpoint_c": 20.0}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["heating_setpoint_c"] == 20.0

    def test_from_dict_all_18_variants(self):
        variants = [
            {"type": "PowerSetpoint", "active_power_kw": 1.0},
            {"type": "ThermalSetpoint", "heating_setpoint_c": 20.0},
            {"type": "ThermalSetpointDelta", "heating_delta_c": 1.0},
            {"type": "HumiditySetpoint", "target_rh": 50.0},
            {"type": "PowerLimit", "max_power_kw": 5.0},
            {"type": "DutyCycle", "on_fraction": 0.5},
            {"type": "GridConnect", "connected": True},
            {"type": "SelfConsumption", "enabled": True, "solar_only_charging": True},
            {"type": "CurtailmentPercent", "percent": 50.0},
            {"type": "ReactiveSetpoint", "kvar": 1.0},
            {"type": "PowerFactorSetpoint", "power_factor": 0.95},
            {"type": "InverterPriorityMode", "priority": "Watt"},
            {"type": "SOCTarget", "target_soc": 0.8},
            {"type": "LoadFraction", "fraction": 0.75},
            {"type": "ModeOverride", "mode": "Heating"},
            {"type": "DemandResponse", "level": "High"},
            {"type": "ProtocolNative", "protocol": 42, "payload": b"\x01"},
            {"type": "IdealCapacity", "capacity_w": 5000.0},
            {"type": "IdealCapacityModeOverride", "mode": "Auto"},
        ]
        for v in variants:
            sig = ControlSignal.from_dict(v)
            assert sig.to_dict()["type"] == v["type"]

    def test_from_dict_accepts_string_mode(self):
        d = {"type": "ModeOverride", "mode": "Heating"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["mode"] == "Heating"

    def test_from_dict_accepts_typed_enum_mode(self):
        d = {"type": "ModeOverride", "mode": OperatingMode.Cooling}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["mode"] == "Cooling"

    def test_from_dict_accepts_string_dr_level(self):
        d = {"type": "DemandResponse", "level": "Normal"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["level"] == "Normal"

    def test_from_dict_accepts_typed_enum_dr_level(self):
        d = {"type": "DemandResponse", "level": DRLevel.Critical}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["level"] == "Critical"

    def test_from_dict_accepts_string_inverter_priority(self):
        d = {"type": "InverterPriorityMode", "priority": "Var"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["priority"] == "Var"

    def test_from_dict_accepts_typed_enum_inverter_priority(self):
        d = {"type": "InverterPriorityMode", "priority": InverterPriority.Cpf}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["priority"] == "Cpf"

    def test_from_dict_accepts_string_duty_cycle_component(self):
        d = {"type": "DutyCycle", "on_fraction": 0.5, "component": "Compressor"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["component"] == "Compressor"

    def test_from_dict_accepts_typed_enum_duty_cycle_component(self):
        d = {
            "type": "DutyCycle",
            "on_fraction": 0.5,
            "component": DutyCycleComponent.BackupElement,
        }
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["component"] == "BackupElement"

    def test_from_dict_ideal_capacity_mode_auto(self):
        d = {"type": "IdealCapacityModeOverride", "mode": "auto"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["mode"] == "Auto"

    def test_from_dict_ideal_capacity_mode_on(self):
        d = {"type": "IdealCapacityModeOverride", "mode": "on"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["mode"] == "On"

    def test_from_dict_ideal_capacity_mode_off(self):
        d = {"type": "IdealCapacityModeOverride", "mode": "off"}
        sig = ControlSignal.from_dict(d)
        assert sig.to_dict()["mode"] == "Off"

    def test_from_dict_missing_required_key_raises(self):
        d = {"type": "PowerSetpoint"}
        with pytest.raises(ValueError):
            ControlSignal.from_dict(d)

    def test_from_dict_invalid_duty_cycle_component_raises(self):
        d = {"type": "DutyCycle", "on_fraction": 0.5, "component": "garbage"}
        with pytest.raises(ValueError):
            ControlSignal.from_dict(d)


class TestControlSignalRoundTrip:
    """Test to_dict -> from_dict round-trip for all variants."""

    def test_power_setpoint_round_trip(self):
        original = ControlSignal.power_setpoint(1.5, 0.3)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_thermal_setpoint_round_trip(self):
        original = ControlSignal.thermal_setpoint(
            heat_c=20.0, cool_c=24.0, deadband_c=2.0
        )
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_thermal_setpoint_delta_round_trip(self):
        original = ControlSignal.thermal_setpoint_delta(heating_delta_c=1.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_ideal_capacity_round_trip(self):
        original = ControlSignal.ideal_capacity(5000.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_load_fraction_round_trip(self):
        original = ControlSignal.load_fraction(0.75)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_mode_override_round_trip(self):
        original = ControlSignal.mode_override(OperatingMode.Heating)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_demand_response_round_trip(self):
        original = ControlSignal.demand_response(DRLevel.High, 3600.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_soc_target_round_trip(self):
        original = ControlSignal.soc_target(target=0.8, min=0.2, max=1.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_humidity_setpoint_round_trip(self):
        original = ControlSignal.humidity_setpoint(
            target_rh=50.0, min_rh=40.0, max_rh=60.0
        )
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_power_limit_round_trip(self):
        original = ControlSignal.power_limit(max_power_kw=5.0, ramp_rate_kw_per_s=0.1)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_duty_cycle_round_trip(self):
        original = ControlSignal.duty_cycle(on_fraction=0.5, period_s=300.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_duty_cycle_with_component_round_trip(self):
        original = ControlSignal.duty_cycle(
            on_fraction=0.5, component=DutyCycleComponent.Compressor
        )
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_grid_connect_round_trip(self):
        original = ControlSignal.grid_connect(connected=True)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_self_consumption_round_trip(self):
        original = ControlSignal.self_consumption(
            enabled=True, solar_only_charging=True
        )
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_curtailment_percent_round_trip(self):
        original = ControlSignal.curtailment_percent(50.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_reactive_setpoint_round_trip(self):
        original = ControlSignal.reactive_setpoint(kvar=1.0)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_power_factor_setpoint_round_trip(self):
        original = ControlSignal.power_factor_setpoint(0.95)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_inverter_priority_mode_round_trip(self):
        original = ControlSignal.inverter_priority_mode(InverterPriority.Watt)
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d

    def test_ideal_capacity_mode_override_round_trip(self):
        original = ControlSignal.ideal_capacity_mode_override("auto")
        d = original.to_dict()
        round_trip = ControlSignal.from_dict(d)
        assert round_trip.to_dict() == d


class TestControlSignalRepr:
    """Test __repr__ contains variant name."""

    @pytest.mark.parametrize(
        "constructor,name",
        [
            (lambda: ControlSignal.power_setpoint(1.0), "PowerSetpoint"),
            (lambda: ControlSignal.thermal_setpoint(heat_c=20.0), "ThermalSetpoint"),
            (lambda: ControlSignal.thermal_setpoint_delta(), "ThermalSetpointDelta"),
            (lambda: ControlSignal.ideal_capacity(5000.0), "IdealCapacity"),
            (lambda: ControlSignal.load_fraction(0.5), "LoadFraction"),
            (lambda: ControlSignal.mode_override(OperatingMode.Off), "ModeOverride"),
            (lambda: ControlSignal.demand_response(DRLevel.Normal), "DemandResponse"),
            (lambda: ControlSignal.soc_target(0.8), "SOCTarget"),
            (lambda: ControlSignal.humidity_setpoint(50.0), "HumiditySetpoint"),
            (lambda: ControlSignal.power_limit(5.0), "PowerLimit"),
            (lambda: ControlSignal.duty_cycle(0.5), "DutyCycle"),
            (lambda: ControlSignal.grid_connect(True), "GridConnect"),
            (lambda: ControlSignal.self_consumption(True, True), "SelfConsumption"),
            (lambda: ControlSignal.curtailment_percent(50.0), "CurtailmentPercent"),
            (lambda: ControlSignal.reactive_setpoint(1.0), "ReactiveSetpoint"),
            (lambda: ControlSignal.power_factor_setpoint(0.95), "PowerFactorSetpoint"),
            (
                lambda: ControlSignal.inverter_priority_mode(InverterPriority.Watt),
                "InverterPriorityMode",
            ),
            (lambda: ControlSignal.protocol_native(42, b"test"), "ProtocolNative"),
            (
                lambda: ControlSignal.ideal_capacity_mode_override("auto"),
                "IdealCapacityModeOverride",
            ),
        ],
    )
    def test_repr_contains_variant_name(self, constructor, name):
        sig = constructor()
        assert name in repr(sig)


class TestTypedEnumArgValidation:
    """Test that incorrect enum type raises TypeError."""

    def test_mode_override_rejects_dr_level(self):
        with pytest.raises(TypeError):
            ControlSignal.mode_override(DRLevel.High)

    def test_demand_response_rejects_operating_mode(self):
        with pytest.raises(TypeError):
            ControlSignal.demand_response(OperatingMode.Heating)

    def test_inverter_priority_mode_rejects_operating_mode(self):
        with pytest.raises(TypeError):
            ControlSignal.inverter_priority_mode(OperatingMode.Heating)

    def test_duty_cycle_rejects_inverter_priority(self):
        with pytest.raises(TypeError):
            ControlSignal.duty_cycle(0.5, component=InverterPriority.Watt)
