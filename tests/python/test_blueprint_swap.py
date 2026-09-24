"""Test HVAC and WH equipment swapping via DwellingBlueprint."""

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from ochre_next import (
    ASHPHeater,
    DwellingBlueprint,
    ElectricResistanceWH,
    EndUse,
    HeatPumpWH,
)


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )


def test_swap_gas_furnace_to_ashp():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    assert "Gas Furnace" not in bp.equipment_names()

    ashp = ASHPHeater("ASHP", capacity_w=12000, hspf=9.5,
                       backup_capacity_w=10000)
    bp.add_equipment(ashp)
    assert "ASHP" in bp.equipment_names()

    dw = bp.build()
    dw.initialize()
    assert "ASHP" in dw.equipment_names()
    assert "Gas Furnace" not in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_swap_gas_wh_to_hpwh():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    assert all("Water Heater" not in n for n in bp.equipment_names())

    hpwh = HeatPumpWH("HPWH", tank_volume_m3=0.19, cop=3.5,
                       backup_capacity_w=4500.0,
                       avg_water_draw_l_per_day=200.0)
    bp.add_equipment(hpwh)
    dw = bp.build()
    dw.initialize()
    assert "HPWH" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_swap_gas_wh_to_electric_resistance():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    erwh = ElectricResistanceWH("ERWH", tank_volume_m3=0.19,
                                 heating_capacity_w=4500.0,
                                 uniform_energy_factor=0.95,
                                 avg_water_draw_l_per_day=200.0)
    bp.add_equipment(erwh)
    dw = bp.build()
    dw.initialize()
    assert "ERWH" in dw.equipment_names()

    for _ in range(5):
        dw.step()
