"""Tests for Python enum and bitflag bindings."""

import pytest
from ochre_next import (
    EndUse,
    FuelType,
    OperatingMode,
    Mode,
    ExecutionStage,
    FluidType,
    InverterPriority,
    DutyCycleComponent,
    SimStatus,
    AggregationResolution,
    ResStockVersion,
    ControlCapabilities,
    LutType,
    BatteryChemistry,
    ChargingLevel,
    DriverArchetype,
)


class TestEndUse:
    def test_standard_constants_accessible(self):
        assert EndUse.HVAC_HEATING.as_str() == "hvac_heating"
        assert EndUse.HVAC_COOLING.as_str() == "hvac_cooling"
        assert EndUse.WATER_HEATING.as_str() == "water_heating"
        assert EndUse.LIGHTING.as_str() == "lighting"
        assert EndUse.PLUG_LOADS.as_str() == "plug_loads"
        assert EndUse.REFRIGERATION.as_str() == "refrigeration"
        assert EndUse.VENTILATION.as_str() == "ventilation"
        assert EndUse.BATTERY.as_str() == "battery"
        assert EndUse.PV.as_str() == "pv"
        assert EndUse.EV.as_str() == "ev"
        assert EndUse.GENERATOR.as_str() == "generator"
        assert EndUse.DEHUMIDIFIER.as_str() == "dehumidifier"
        assert EndUse.OTHER.as_str() == "other"

    def test_custom_end_use(self):
        custom = EndUse.custom("my_load")
        assert custom.as_str() == "my_load"

    def test_is_standard_true_for_constants(self):
        assert EndUse.HVAC_HEATING.is_standard() is True
        assert EndUse.BATTERY.is_standard() is True
        assert EndUse.OTHER.is_standard() is True

    def test_is_standard_false_for_custom(self):
        custom = EndUse.custom("my_load")
        assert custom.is_standard() is False

    def test_equality_and_hash(self):
        assert EndUse.HVAC_HEATING == EndUse.HVAC_HEATING
        assert EndUse.HVAC_HEATING != EndUse.HVAC_COOLING
        assert hash(EndUse.HVAC_HEATING) == hash(EndUse.HVAC_HEATING)

    def test_repr(self):
        assert repr(EndUse.HVAC_HEATING) == "EndUse('hvac_heating')"

    def test_str(self):
        assert str(EndUse.HVAC_HEATING) == "hvac_heating"


class TestFuelType:
    def test_variants_accessible(self):
        assert FuelType.Electric is not None
        assert FuelType.Gas is not None
        assert FuelType.Propane is not None
        assert FuelType.Oil is not None
        assert FuelType.NoFuel is not None

    def test_static_constructors(self):
        assert FuelType.electric() == FuelType.Electric
        assert FuelType.gas() == FuelType.Gas
        assert FuelType.propane() == FuelType.Propane
        assert FuelType.oil() == FuelType.Oil
        assert FuelType.no_fuel() == FuelType.NoFuel

    def test_from_str_dotted_form(self):
        assert FuelType.from_str("None") == FuelType.NoFuel
        assert FuelType.from_str("NoFuel") == FuelType.NoFuel
        assert FuelType.from_str("Electric") == FuelType.Electric

    def test_from_str_invalid_raises(self):
        with pytest.raises(ValueError):
            FuelType.from_str("Invalid")

    def test_str_returns_no_fuel(self):
        assert str(FuelType.NoFuel) == "NoFuel"

    def test_repr(self):
        assert "NoFuel" in repr(FuelType.NoFuel)


class TestOperatingMode:
    def test_all_12_variants_accessible(self):
        assert OperatingMode.Off is not None
        assert OperatingMode.Heating is not None
        assert OperatingMode.Cooling is not None
        assert OperatingMode.Standby is not None
        assert OperatingMode.Defrost is not None
        assert OperatingMode.Charging is not None
        assert OperatingMode.Discharging is not None
        assert OperatingMode.HeatingHP is not None
        assert OperatingMode.HeatingER is not None
        assert OperatingMode.HeatingHPAndER is not None
        assert OperatingMode.HeatPumpWH is not None
        assert OperatingMode.BackupElement is not None

    def test_static_constructors(self):
        assert OperatingMode.off() == OperatingMode.Off
        assert OperatingMode.heating() == OperatingMode.Heating
        assert OperatingMode.cooling() == OperatingMode.Cooling
        assert OperatingMode.standby() == OperatingMode.Standby
        assert OperatingMode.defrost() == OperatingMode.Defrost
        assert OperatingMode.charging() == OperatingMode.Charging
        assert OperatingMode.discharging() == OperatingMode.Discharging
        assert OperatingMode.heating_hp() == OperatingMode.HeatingHP
        assert OperatingMode.heating_er() == OperatingMode.HeatingER
        assert OperatingMode.heating_hp_and_er() == OperatingMode.HeatingHPAndER
        assert OperatingMode.heat_pump_wh() == OperatingMode.HeatPumpWH
        assert OperatingMode.backup_element() == OperatingMode.BackupElement

    def test_from_str_round_trip(self):
        for mode in [
            OperatingMode.Off,
            OperatingMode.Heating,
            OperatingMode.Cooling,
            OperatingMode.Standby,
            OperatingMode.Defrost,
            OperatingMode.Charging,
            OperatingMode.Discharging,
            OperatingMode.HeatingHP,
            OperatingMode.HeatingER,
            OperatingMode.HeatingHPAndER,
            OperatingMode.HeatPumpWH,
            OperatingMode.BackupElement,
        ]:
            assert OperatingMode.from_str(str(mode)) == mode

    def test_from_str_invalid_raises(self):
        with pytest.raises(ValueError):
            OperatingMode.from_str("Invalid")

    def test_mode_alias_points_to_operating_mode(self):
        assert Mode is OperatingMode

    def test_equality_and_hash(self):
        assert OperatingMode.Off == OperatingMode.Off
        assert OperatingMode.Off != OperatingMode.Heating
        assert hash(OperatingMode.Off) == hash(OperatingMode.Off)


class TestExecutionStage:
    def test_variants_accessible(self):
        assert ExecutionStage.Independent is not None
        assert ExecutionStage.Electrical is not None
        assert ExecutionStage.Thermal is not None
        assert ExecutionStage.EnvelopeResolution is not None

    def test_from_str_round_trip(self):
        for stage in [
            ExecutionStage.Independent,
            ExecutionStage.Electrical,
            ExecutionStage.Thermal,
            ExecutionStage.EnvelopeResolution,
        ]:
            assert ExecutionStage.from_str(str(stage)) == stage


class TestFluidType:
    def test_variants_accessible(self):
        assert FluidType.Water is not None
        assert FluidType.Glycol is not None
        assert FluidType.Refrigerant is not None

    def test_from_str_round_trip(self):
        for fluid in [FluidType.Water, FluidType.Glycol, FluidType.Refrigerant]:
            assert FluidType.from_str(str(fluid)) == fluid


class TestInverterPriority:
    def test_variants_accessible(self):
        assert InverterPriority.Watt is not None
        assert InverterPriority.Var is not None
        assert InverterPriority.Cpf is not None

    def test_from_str_round_trip(self):
        for priority in [
            InverterPriority.Watt,
            InverterPriority.Var,
            InverterPriority.Cpf,
        ]:
            assert InverterPriority.from_str(str(priority)) == priority


class TestDutyCycleComponent:
    def test_variants_accessible(self):
        assert DutyCycleComponent.Compressor is not None
        assert DutyCycleComponent.BackupElement is not None

    def test_from_str_round_trip(self):
        for comp in [DutyCycleComponent.Compressor, DutyCycleComponent.BackupElement]:
            assert DutyCycleComponent.from_str(str(comp)) == comp


class TestSimStatus:
    def test_ok_variant(self):
        status = SimStatus.ok()
        assert status.message is None
        assert "Ok" in repr(status)
        assert str(status) == "Ok"

    def test_flagged_variant(self):
        status = SimStatus.flagged("warning message")
        assert status.message == "warning message"
        assert "Flagged" in repr(status)

    def test_flagged_no_message(self):
        status = SimStatus.flagged(None)
        assert status.message is None

    def test_failed_variant(self):
        status = SimStatus.failed("error message")
        assert status.message == "error message"
        assert "Failed" in repr(status)

    def test_from_str_round_trip(self):
        assert SimStatus.from_str("ok") == SimStatus.ok()
        assert SimStatus.from_str("flagged") == SimStatus.flagged(None)
        assert SimStatus.from_str("failed") == SimStatus.failed(None)

    def test_from_str_invalid_raises(self):
        with pytest.raises(ValueError):
            SimStatus.from_str("Invalid")


class TestAggregationResolution:
    def test_variants_accessible(self):
        assert AggregationResolution.FifteenMin is not None
        assert AggregationResolution.Hourly is not None

    def test_from_str_round_trip(self):
        assert (
            AggregationResolution.from_str("15min") == AggregationResolution.FifteenMin
        )
        assert AggregationResolution.from_str("hourly") == AggregationResolution.Hourly


class TestResStockVersion:
    def test_variants_accessible(self):
        assert ResStockVersion.V2024_1 is not None
        assert ResStockVersion.V2024_2 is not None
        assert ResStockVersion.V2025_1 is not None

    def test_from_str_dotted_form(self):
        assert ResStockVersion.from_str("2024.1") == ResStockVersion.V2024_1
        assert ResStockVersion.from_str("2024.2") == ResStockVersion.V2024_2
        assert ResStockVersion.from_str("2025.1") == ResStockVersion.V2025_1

    def test_from_str_underscore_form(self):
        assert ResStockVersion.from_str("V2024_1") == ResStockVersion.V2024_1
        assert ResStockVersion.from_str("V2024_2") == ResStockVersion.V2024_2
        assert ResStockVersion.from_str("V2025_1") == ResStockVersion.V2025_1


class TestControlCapabilities:
    def test_class_attributes_exist(self):
        assert ControlCapabilities.POWER_SETPOINT is not None
        assert ControlCapabilities.SOC_TARGET is not None
        assert ControlCapabilities.THERMAL_SETPOINT is not None
        assert ControlCapabilities.POWER_LIMIT is not None
        assert ControlCapabilities.MODE_OVERRIDE is not None
        assert ControlCapabilities.DUTY_CYCLE is not None
        assert ControlCapabilities.LOAD_FRACTION is not None
        assert ControlCapabilities.GRID_CONNECT is not None
        assert ControlCapabilities.SELF_CONSUMPTION is not None
        assert ControlCapabilities.DEMAND_RESPONSE is not None
        assert ControlCapabilities.PROTOCOL_NATIVE is not None
        assert ControlCapabilities.HUMIDITY_SETPOINT is not None
        assert ControlCapabilities.CURTAILMENT_PERCENT is not None
        assert ControlCapabilities.REACTIVE_SETPOINT is not None
        assert ControlCapabilities.POWER_FACTOR_SETPOINT is not None
        assert ControlCapabilities.INVERTER_PRIORITY_MODE is not None

    def test_or_combine_flags(self):
        caps = ControlCapabilities.POWER_SETPOINT | ControlCapabilities.SOC_TARGET
        assert ControlCapabilities.POWER_SETPOINT in caps
        assert ControlCapabilities.SOC_TARGET in caps

    def test_contains_check(self):
        caps = ControlCapabilities.POWER_SETPOINT | ControlCapabilities.SOC_TARGET
        assert ControlCapabilities.POWER_SETPOINT in caps
        assert ControlCapabilities.THERMAL_SETPOINT not in caps

    def test_from_list_valid(self):
        caps = ControlCapabilities.from_list(["POWER_SETPOINT", "SOC_TARGET"])
        assert ControlCapabilities.POWER_SETPOINT in caps
        assert ControlCapabilities.SOC_TARGET in caps

    def test_from_list_invalid_raises(self):
        with pytest.raises(ValueError):
            ControlCapabilities.from_list(["INVALID"])

    def test_repr_shows_flags(self):
        caps = ControlCapabilities.POWER_SETPOINT | ControlCapabilities.SOC_TARGET
        r = repr(caps)
        assert "POWER_SETPOINT" in r
        assert "SOC_TARGET" in r


class TestLutType:
    def test_variants_accessible(self):
        assert LutType.ChargingCurve is not None
        assert LutType.Ocv is not None
        assert LutType.UNeg is not None

    def test_from_str_round_trip(self):
        for lut in [LutType.ChargingCurve, LutType.Ocv, LutType.UNeg]:
            assert LutType.from_str(str(lut)) == lut


class TestBatteryChemistry:
    def test_variants_accessible(self):
        assert BatteryChemistry.Nmc is not None
        assert BatteryChemistry.Lfp is not None
        assert BatteryChemistry.Nca is not None
        assert BatteryChemistry.Lto is not None

    def test_from_str_round_trip(self):
        for chem in [
            BatteryChemistry.Nmc,
            BatteryChemistry.Lfp,
            BatteryChemistry.Nca,
            BatteryChemistry.Lto,
        ]:
            assert BatteryChemistry.from_str(str(chem)) == chem


class TestChargingLevel:
    def test_variants_accessible(self):
        assert ChargingLevel.L1 is not None
        assert ChargingLevel.L2 is not None
        assert ChargingLevel.DcFast is not None

    def test_from_str_round_trip(self):
        for level in [ChargingLevel.L1, ChargingLevel.L2, ChargingLevel.DcFast]:
            assert ChargingLevel.from_str(str(level)) == level


class TestDriverArchetype:
    def test_variants_accessible(self):
        assert DriverArchetype.Commuter is not None
        assert DriverArchetype.WorkFromHome is not None
        assert DriverArchetype.ShiftWorker is not None
        assert DriverArchetype.WeekendWarrior is not None
        assert DriverArchetype.SeniorRetiree is not None
        assert DriverArchetype.SchoolRunFamily is not None

    def test_from_str_round_trip(self):
        for archetype in [
            DriverArchetype.Commuter,
            DriverArchetype.WorkFromHome,
            DriverArchetype.ShiftWorker,
            DriverArchetype.WeekendWarrior,
            DriverArchetype.SeniorRetiree,
            DriverArchetype.SchoolRunFamily,
        ]:
            assert DriverArchetype.from_str(str(archetype)) == archetype
