"""Test autosizing and error handling for equipment."""

import pytest
from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from ochre_next import (
    AirConditioner,
    ASHPCooler,
    ASHPHeater,
    DwellingBlueprint,
    ElectricBaseboard,
    ElectricBoiler,
    ElectricFurnace,
    EndUse,
    GasBoiler,
    GasFurnace,
    GasWaterHeater,
    HeatPumpWH,
    IndirectTank,
    TanklessWaterHeater,
)


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        duration_s=300,
        time_res_s=60,
    )


def test_missing_capacity_raises():
    # autosize=True is the default, so capacity is optional.
    # Explicitly passing autosize=False requires capacity.
    with pytest.raises(ValueError, match="capacity_w is required"):
        GasFurnace("NoCap", autosize=False)

    with pytest.raises(ValueError, match="capacity_w is required"):
        AirConditioner("NoCap", autosize=False)

    with pytest.raises(ValueError, match="capacity_w is required"):
        ASHPHeater("NoCap", autosize=False)

    with pytest.raises(ValueError, match="capacity_w is required"):
        ASHPCooler("NoCap", autosize=False)

    with pytest.raises(ValueError, match="capacity_w is required"):
        ElectricBaseboard("NoCap", autosize=False)


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
    ashp = ASHPHeater("AutoASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000)
    bp.add_equipment(ashp)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_autosize_ashp_heater():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    ashp = ASHPHeater("AutoASHP", autosize=True, hspf=9.5, backup_capacity_w=5000)
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
    wh = GasWaterHeater(
        "AutoWH",
        autosize=True,
        uniform_energy_factor=0.65,
        avg_water_draw_l_per_day=200.0,
    )
    bp.add_equipment(wh)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_autosize_hpwh():
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    hpwh = HeatPumpWH(
        "AutoHPWH", autosize=True, cop=3.5, avg_water_draw_l_per_day=200.0
    )
    bp.add_equipment(hpwh)
    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_remove_equipment_by_name():
    bp = _make_blueprint()
    names_before = bp.equipment_names()
    assert len(names_before) >= 2
    bp.remove_equipment(names_before[0])
    names_after = bp.equipment_names()
    assert names_before[0] not in names_after
    assert len(names_after) == len(names_before) - 1


def test_remove_nonexistent_equipment_raises():
    bp = _make_blueprint()
    with pytest.raises(ValueError, match="not found"):
        bp.remove_equipment("NoSuchEquipment")


def test_remove_equipment_single_end_use():
    """remove_equipment_by_end_use accepts a single EndUse, not just a list."""
    bp = _make_blueprint()
    count = bp.remove_equipment_by_end_use(EndUse.WATER_HEATING)
    assert count >= 1
    assert all("Water Heater" not in n for n in bp.equipment_names())


def test_double_build_raises():
    """Building the same blueprint twice raises ValueError."""
    bp = _make_blueprint()
    bp.build()
    with pytest.raises(ValueError, match="already been built"):
        bp.build()


def test_add_equipment_rejects_unsupported_type():
    """Passing an arbitrary object to add_equipment raises ValueError."""
    bp = _make_blueprint()
    with pytest.raises(ValueError, match="must be a typed"):
        bp.add_equipment("not an equipment object")
    with pytest.raises(ValueError, match="must be a typed"):
        bp.add_equipment(42)
    with pytest.raises(ValueError, match="must be a typed"):
        bp.add_equipment(object())


def test_duplicate_equipment_name_rejected():
    """Adding two equipment with the same name is rejected."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    bp.add_equipment(GasFurnace("MyFurnace", afue=0.95))
    with pytest.raises((ValueError, RuntimeError), match="duplicate|already"):
        bp.add_equipment(GasFurnace("MyFurnace", afue=0.95))


def test_negative_capacity_raises():
    with pytest.raises(ValueError, match="capacity_w must be positive"):
        GasFurnace("bad", capacity_w=-5000)


def test_afue_out_of_range_raises():
    with pytest.raises(ValueError, match="afue must be between"):
        GasFurnace("bad", capacity_w=10000, afue=2.0)


def test_negative_tank_volume_raises():
    with pytest.raises(ValueError, match="tank_volume_m3 must be positive"):
        GasWaterHeater("bad", tank_volume_m3=-0.1, heating_capacity_w=4500)


def test_gas_boiler_construction_and_autosize():
    """GasBoiler can be constructed and added to a blueprint with autosize=True."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    gb = GasBoiler("AutoBoiler", autosize=True, afue=0.95)
    bp.add_equipment(gb)
    dw = bp.build()
    dw.initialize()
    assert "AutoBoiler" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_gas_boiler_explicit_capacity():
    """GasBoiler with explicit capacity works."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    gb = GasBoiler("MyBoiler", capacity_w=20000, afue=0.92)
    bp.add_equipment(gb)
    dw = bp.build()
    dw.initialize()
    assert "MyBoiler" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_electric_furnace_replaces_gas_furnace():
    """ElectricFurnace can replace GasFurnace in a blueprint."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    ef = ElectricFurnace("ElecFurnace", capacity_w=15000, eir=1.0)
    bp.add_equipment(ef)
    dw = bp.build()
    dw.initialize()
    assert "ElecFurnace" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_electric_furnace_autosize():
    """ElectricFurnace with autosize=True works."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    ef = ElectricFurnace("AutoElecFurnace", autosize=True, eir=1.0)
    bp.add_equipment(ef)
    dw = bp.build()
    dw.initialize()
    assert "AutoElecFurnace" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_electric_boiler_autosize():
    """ElectricBoiler with autosize=True works."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING])
    eb = ElectricBoiler("AutoElecBoiler", autosize=True, eir=1.0)
    bp.add_equipment(eb)
    dw = bp.build()
    dw.initialize()
    assert "AutoElecBoiler" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_tankless_water_heater_autosize():
    """TanklessWaterHeater with autosize=True works."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    twh = TanklessWaterHeater(
        "AutoTankless",
        autosize=True,
        uniform_energy_factor=0.85,
        avg_water_draw_l_per_day=200.0,
    )
    bp.add_equipment(twh)
    dw = bp.build()
    dw.initialize()
    assert "AutoTankless" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_indirect_tank_autosize():
    """IndirectTank with autosize=True works."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.WATER_HEATING])
    it = IndirectTank(
        "AutoIndirect",
        autosize=True,
        hx_ua_w_per_k=150.0,
        avg_water_draw_l_per_day=200.0,
    )
    bp.add_equipment(it)
    dw = bp.build()
    dw.initialize()
    assert "AutoIndirect" in dw.equipment_names()
    for _ in range(5):
        dw.step()


def test_missing_capacity_gas_boiler_raises():
    with pytest.raises(ValueError, match="capacity_w is required"):
        GasBoiler("NoCap", autosize=False)


def test_missing_capacity_electric_boiler_raises():
    with pytest.raises(ValueError, match="capacity_w is required"):
        ElectricBoiler("NoCap", autosize=False)


def test_missing_capacity_electric_furnace_raises():
    with pytest.raises(ValueError, match="capacity_w is required"):
        ElectricFurnace("NoCap", autosize=False)


def test_missing_capacity_tankless_raises():
    with pytest.raises(ValueError, match="heating_capacity_w is required"):
        TanklessWaterHeater("NoCap", autosize=False)


def test_missing_tank_volume_indirect_tank_raises():
    with pytest.raises(ValueError, match="tank_volume_m3 is required"):
        IndirectTank("NoVol", autosize=False)


def test_afue_out_of_range_gas_boiler_raises():
    with pytest.raises(ValueError, match="afue must be between"):
        GasBoiler("bad", capacity_w=20000, afue=2.0)


def test_uef_out_of_range_tankless_raises():
    with pytest.raises(ValueError, match="uniform_energy_factor must be between"):
        TanklessWaterHeater("bad", heating_capacity_w=20000, uniform_energy_factor=2.0)


def test_empty_blueprint_builds():
    """Removing all equipment and building should not crash."""
    bp = _make_blueprint()
    for name in list(bp.equipment_names()):
        bp.remove_equipment(name)
    assert len(bp.equipment_names()) == 0
    dw = bp.build()
    dw.initialize()
    for _ in range(3):
        dw.step()
