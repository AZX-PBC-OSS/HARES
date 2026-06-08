"""Test autosizing and error handling for equipment."""

import pytest
from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from ochre_next import (
    AirConditioner,
    ASHPCooler,
    ASHPHeater,
    DwellingBlueprint,
    EndUse,
    GasFurnace,
    GasWaterHeater,
    HeatPumpWH,
)


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )


def test_missing_capacity_raises():
    with pytest.raises(ValueError, match="capacity_w is required"):
        GasFurnace("NoCap")


def test_explicit_capacity_heating():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    furnace = GasFurnace("AutoFurnace", capacity_w=15000, afue=0.95)
    bp.add_equipment(furnace)
    dw = bp.build()
    dw.initialize()
    assert "AutoFurnace" in dw.equipment_names()

    for _ in range(5):
        result = dw.step()
        assert result is not None
        power = result["net_electric_power_kw"]
        assert abs(power) < 1e9


@pytest.mark.xfail(
    reason="Autosizing not yet implemented for GasFurnace — requires typed config"
)
def test_autosize_heating():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    furnace = GasFurnace("AutoFurnace", autosize=True, afue=0.95)
    bp.add_equipment(furnace)
    dw = bp.build()
    dw.initialize()
    assert "AutoFurnace" in dw.equipment_names()

    for _ in range(5):
        result = dw.step()
        assert result is not None
        power = result["net_electric_power_kw"]
        assert abs(power) < 1e9


def test_explicit_capacity_ashp_heater():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    ashp = ASHPHeater("AutoASHP", capacity_w=12000, hspf=9.5,
                       backup_capacity_w=5000)
    bp.add_equipment(ashp)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_autosize_ashp_heater():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    ashp = ASHPHeater("AutoASHP", autosize=True, hspf=9.5,
                       backup_capacity_w=5000)
    bp.add_equipment(ashp)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_explicit_capacity_ashp_cooler():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    ashp = ASHPCooler("AutoASHP", capacity_w=10000, seer=18.0)
    bp.add_equipment(ashp)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


@pytest.mark.xfail(
    reason="Autosizing not yet implemented for ASHPCooler — requires cooling_eir"
)
def test_autosize_ashp_cooler():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    ashp = ASHPCooler("AutoASHP", autosize=True, seer=18.0)
    bp.add_equipment(ashp)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_explicit_capacity_ac():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    ac = AirConditioner("AutoAC", capacity_w=10000, seer=16.0)
    bp.add_equipment(ac)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


@pytest.mark.xfail(
    reason="Autosizing not yet implemented for AirConditioner — requires typed config"
)
def test_autosize_ac():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    ac = AirConditioner("AutoAC", autosize=True, seer=16.0)
    bp.add_equipment(ac)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_autosize_water_heater():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    wh = GasWaterHeater("AutoWH", autosize=True, uniform_energy_factor=0.65,
                         avg_water_draw_l_per_day=200.0)
    bp.add_equipment(wh)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_autosize_hpwh():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    hpwh = HeatPumpWH("AutoHPWH", autosize=True, cop=3.5,
                       avg_water_draw_l_per_day=200.0)
    bp.add_equipment(hpwh)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()
