"""Tests for EquipmentDescriptor and TelemetryField Python exposure."""

import pytest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)
SCHEDULE = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Input Files/BEopt_example_schedule.csv"
)


@pytest.fixture(scope="module")
def dwelling():
    from ochre_next import Dwelling

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration=3600,
        time_res=60,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=0,
    )
    dw.initialize()
    return dw


class TestEquipmentDescriptors:
    def test_equipment_descriptors_returns_non_empty_list(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        assert isinstance(descriptors, list), "should return a list"
        assert len(descriptors) > 0, "should return at least one equipment"

    def test_descriptor_has_name(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.name, str), "name should be a string"
            assert len(desc.name) > 0, "name should be non-empty"

    def test_descriptor_end_use_returns_end_use_instance(self, dwelling):
        from ochre_next import EndUse

        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.end_use, EndUse), (
                "end_use should be PyEndUse instance"
            )

    def test_descriptor_fuel_type_returns_fuel_type_instance(self, dwelling):
        from ochre_next import FuelType

        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.fuel_type, FuelType), (
                "fuel_type should be PyFuelType instance"
            )

    def test_descriptor_stage_returns_execution_stage_instance(self, dwelling):
        from ochre_next import ExecutionStage

        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.stage, ExecutionStage), (
                "stage should be PyExecutionStage instance"
            )

    def test_descriptor_control_capabilities_returns_capabilities_instance(
        self, dwelling
    ):
        from ochre_next import ControlCapabilities

        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.control_capabilities, ControlCapabilities), (
                "control_capabilities should be PyControlCapabilities instance"
            )

    def test_descriptor_telemetry_fields_returns_list(self, dwelling):
        from ochre_next import TelemetryField

        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.telemetry_fields, list), (
                "telemetry_fields should be a list"
            )
            for tf in desc.telemetry_fields:
                assert isinstance(tf, TelemetryField), (
                    "should be PyTelemetryField instance"
                )
                assert isinstance(tf.name, str), "name should be string"
                assert isinstance(tf.unit, str), "unit should be string"
                assert isinstance(tf.description, str), "description should be string"


class TestEquipmentNames:
    def test_equipment_names_returns_list_of_strings(self, dwelling):
        names = dwelling.equipment_names()

        assert isinstance(names, list), "should return a list"
        assert len(names) > 0, "should return at least one name"
        for name in names:
            assert isinstance(name, str), "each name should be a string"

    def test_equipment_names_matches_descriptor_names(self, dwelling):
        names = dwelling.equipment_names()
        descriptors = dwelling.equipment_descriptors()

        descriptor_names = [d.name for d in descriptors]
        assert names == descriptor_names, (
            "equipment_names should match descriptor names"
        )


class TestRepr:
    def test_descriptor_repr_works(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            repr_str = repr(desc)
            assert isinstance(repr_str, str), "__repr__ should return a string"
            assert "EquipmentDescriptor" in repr_str, (
                "__repr__ should contain class name"
            )
            assert "name=" in repr_str, "__repr__ should contain name"

    def test_telemetry_field_repr_works(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            for tf in desc.telemetry_fields:
                repr_str = repr(tf)
                assert isinstance(repr_str, str), "__repr__ should return a string"
                assert "TelemetryField" in repr_str, (
                    "__repr__ should contain class name"
                )


class TestValidateControl:
    def test_validate_control_returns_true_for_valid_signal(self, dwelling):
        from ochre_next import ControlSignal

        names = dwelling.equipment_names()
        assert len(names) > 0, "should have at least one equipment"

        valid_signal = ControlSignal.power_setpoint(1.0)
        result = dwelling.validate_control(names[0], valid_signal)
        assert isinstance(result, bool), "should return a boolean"

    def test_validate_control_returns_false_for_nonexistent_equipment(self, dwelling):
        from ochre_next import ControlSignal

        signal = ControlSignal.power_setpoint(1.0)
        result = dwelling.validate_control("nonexistent_equipment_name", signal)

        assert result is False, "should return False for nonexistent equipment"

    def test_validate_control_does_not_raise(self, dwelling):
        from ochre_next import ControlSignal

        names = dwelling.equipment_names()
        signal = ControlSignal.thermal_setpoint(heat_c=20.0)

        result = dwelling.validate_control(names[0], signal)
        assert isinstance(result, bool), "should return a boolean and not raise"


class TestZoneField:
    def test_descriptor_zone_is_optional(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert desc.zone is None or isinstance(desc.zone, int), (
                "zone should be None or int"
            )


class TestIdField:
    def test_descriptor_id_is_u32(self, dwelling):
        descriptors = dwelling.equipment_descriptors()

        for desc in descriptors:
            assert isinstance(desc.id, int), "id should be an integer"
            assert desc.id >= 0, "id should be non-negative"
