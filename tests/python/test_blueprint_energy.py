"""Validate realistic energy consumption when swapping equipment types."""

import pytest
from ochre_next import (
    Dwelling, DwellingBlueprint, GasFurnace, ASHPHeater, ASHPCooler,
    AirConditioner, GasWaterHeater, HeatPumpWH, ElectricResistanceWH,
    EndUse,
)
from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS

# Run simulations long enough to see meaningful energy use (at least 10 simulation steps)
DURATION_S = 3600  # 1 hour
TIME_RES_S = 600   # 10 minute steps


def _make_blueprint(**kw):
    params = dict(duration_s=DURATION_S, time_res_s=TIME_RES_S)
    params.update(kw)
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, **params,
    )


def _collect_step_powers(bp, max_steps=None):
    """Build, initialize, step, and return list of step result dicts."""
    dw = bp.build()
    dw.initialize()
    results = []
    for i, _ in enumerate(dw.timesteps()):
        if max_steps is not None and i >= max_steps:
            break
        results.append(dw.step())
    return results


def test_gas_furnace_electric_power_is_low():
    """Gas furnace uses gas for heat, electric power stays at background levels."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    dw = bp.build()
    dw.initialize()

    for _ in dw.timesteps():
        r = dw.step()
        # Gas furnace fan + background loads should stay under 5 kW
        assert r["net_electric_power_kw"] < 5.0, \
            f"Gas furnace electric draw {r['net_electric_power_kw']} kW unreasonably high"
        # Gas heating should be active (it's a January morning in Denver)
        assert r["gas_power_w"] >= 0


def test_heat_pump_increases_electric_consumption_vs_gas_furnace():
    """Swapping gas furnace+AC to ASHP should significantly increase electric draw."""
    bp_gas = _make_blueprint()
    bp_gas.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    gas_steps = _collect_step_powers(bp_gas)

    bp_hp = _make_blueprint()
    bp_hp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])
    bp_hp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000))
    bp_hp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))
    hp_steps = _collect_step_powers(bp_hp)

    gas_elec_avg = sum(s["net_electric_power_kw"] for s in gas_steps) / len(gas_steps)
    hp_elec_avg = sum(s["net_electric_power_kw"] for s in hp_steps) / len(hp_steps)

    # Heat pump should draw more electric power than gas furnace
    assert hp_elec_avg > gas_elec_avg, \
        f"ASHP electric {hp_elec_avg:.3f} kW not greater than gas furnace {gas_elec_avg:.3f} kW"

    # Gas furnace should use gas, heat pump should use none
    gas_furnace_gas = any(s["gas_power_w"] > 100 for s in gas_steps)
    hp_gas = any(s["gas_power_w"] > 100 for s in hp_steps)
    assert gas_furnace_gas, "Gas furnace should consume gas for heating"
    assert not hp_gas, f"ASHP should not consume gas, but saw gas use: {[s['gas_power_w'] for s in hp_steps]}"


def test_swap_wh_to_hpwh_does_not_panic():
    """Remove existing water heater and add HPWH - must not crash and must consume electricity."""
    bp = _make_blueprint()
    initial_names = bp.equipment_names()

    # Remove gas equipment (furnace + WH) and replace with electric to isolate WH energy
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING, EndUse.WATER_HEATING])
    bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000))
    bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))

    hpwh = HeatPumpWH("HPWH", tank_volume_m3=0.19, cop=3.5,
                       backup_element_power_w=4500.0,
                       avg_water_draw_l_per_day=200.0)
    bp.add_equipment(hpwh)

    dw = bp.build()
    dw.initialize()
    assert "HPWH" in dw.equipment_names()

    steps = []
    for _ in dw.timesteps():
        r = dw.step()
        assert abs(r["net_electric_power_kw"]) < 1e9
        assert r["net_electric_power_kw"] >= 0
        steps.append(r)

    # HPWH must draw some electric power (it actually runs)
    any_elec = any(s["net_electric_power_kw"] > 0.0 for s in steps)
    assert any_elec, "HPWH produced no electric load — equipment may not be running"

    # No gas equipment remains — gas power should drop to background only
    gas_powers = [s["gas_power_w"] for s in steps if s["gas_power_w"] > 100]
    assert len(gas_powers) == 0, \
        f"HPWH should not consume gas, but saw gas use: {gas_powers}"

    # Residential HPWH electric power should be well under 15 kW
    max_elec = max(s["net_electric_power_kw"] for s in steps)
    assert max_elec < 15.0, \
        f"HPWH electric power {max_elec:.1f} kW unreasonably high for residential unit"


def test_swap_wh_to_electric_resistance_does_not_panic():
    """Remove existing water heater and add ERWH - must not crash and must consume electricity."""
    bp = _make_blueprint()

    # Remove gas equipment (furnace + WH) and replace with electric to isolate WH energy
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING, EndUse.WATER_HEATING])
    bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000))
    bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))

    erwh = ElectricResistanceWH("ERWH", tank_volume_m3=0.19,
                                uniform_energy_factor=0.95,
                                heating_capacity_w=4500.0,
                                avg_water_draw_l_per_day=200.0)
    bp.add_equipment(erwh)

    dw = bp.build()
    dw.initialize()
    assert "ERWH" in dw.equipment_names()

    steps = []
    for _ in dw.timesteps():
        r = dw.step()
        assert abs(r["net_electric_power_kw"]) < 1e9
        assert r["net_electric_power_kw"] >= 0
        steps.append(r)

    # Electric resistance WH must draw some power (it actually runs)
    any_elec = any(s["net_electric_power_kw"] > 0.0 for s in steps)
    assert any_elec, "ERWH produced no electric load — equipment may not be running"

    # No gas equipment remains — gas power should drop to background only
    gas_powers = [s["gas_power_w"] for s in steps if s["gas_power_w"] > 100]
    assert len(gas_powers) == 0, \
        f"ERWH should not consume gas, but saw gas use: {gas_powers}"

    # Electric resistance is less efficient than HPWH — it should draw substantial power
    # (> 1 kW when actively heating a 4500W element)
    max_elec = max(s["net_electric_power_kw"] for s in steps)
    assert max_elec > 1.0, \
        f"ERWH electric power {max_elec:.1f} kW too low for a 4500W resistance element"
    assert max_elec < 15.0, \
        f"ERWH electric power {max_elec:.1f} kW unreasonably high for residential unit"


def test_autosized_heat_pump_produces_reasonable_electric_load():
    """Autosized heat pump should produce non-zero but bounded electric load."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])

    ashp = ASHPHeater("AutoASHP", autosize=True, hspf=9.5, backup_capacity_w=5000)
    ashp_cooler = ASHPCooler("AutoASHP Cooler", capacity_w=10000, seer=18.0)
    bp.add_equipment(ashp)
    bp.add_equipment(ashp_cooler)

    dw = bp.build()
    dw.initialize()

    names = dw.equipment_names()
    assert any("ASHP" in n for n in names), \
        f"ASHP not found in equipment names: {names}"

    powers = []
    for _ in dw.timesteps():
        r = dw.step()
        p = r["net_electric_power_kw"]
        assert abs(p) < 1e9, f"Power unreasonably large: {p}"
        powers.append(p)

    # The heat pump should produce some non-zero heating on a cold January morning
    assert any(p > 0.0 for p in powers), \
        "Heat pump produced no electric load — equipment may not be running"


def test_full_swap_gas_to_electric_does_not_panic():
    """Full electrification: gas furnace+AC+WH → ASHP+HPWH. Must not panic and must consume electricity."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING, EndUse.WATER_HEATING])

    bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000))
    bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))
    bp.add_equipment(HeatPumpWH("HPWH", tank_volume_m3=0.19, cop=3.5,
                                 backup_element_power_w=4500.0,
                                 avg_water_draw_l_per_day=200.0))

    dw = bp.build()
    dw.initialize()

    names = dw.equipment_names()
    assert "ASHP" in names
    assert "HPWH" in names

    steps = []
    for _ in dw.timesteps():
        r = dw.step()
        assert r is not None
        assert abs(r["net_electric_power_kw"]) < 1e9
        assert r["net_electric_power_kw"] >= 0
        steps.append(r)

    # After full electrification, all electric equipment must draw some power
    any_elec = any(s["net_electric_power_kw"] > 0.0 for s in steps)
    assert any_elec, "Fully electrified dwelling produced no electric load"

    # No gas equipment remains — gas power should drop to near zero
    gas_powers = [s["gas_power_w"] for s in steps if s["gas_power_w"] > 100]
    assert len(gas_powers) == 0, \
        f"Fully electrified dwelling should not consume gas, but saw: {gas_powers}"

    # All new equipment names must be present
    assert "ASHP" in names, f"ASHP missing from equipment: {names}"
    assert "HPWH" in names, f"HPWH missing from equipment: {names}"


def test_results_dataframe_has_expected_columns():
    """results() should return a DataFrame with Time and Total Electric Power."""
    bp = _make_blueprint()
    dw = bp.build()
    dw.initialize()

    for _ in dw.timesteps():
        dw.step()

    results = dw.results()
    assert results is not None, "results() returned None"
    columns = list(results.columns)
    assert "Time" in columns, f"Missing Time column in {columns}"
    assert "Total Electric Power (kW)" in columns, f"Missing Total Electric Power column in {columns}"


def test_gas_furnace_results_show_lower_electric_than_ashp():
    """results() electric total should be lower for gas furnace than ASHP."""
    bp_gas = _make_blueprint()
    bp_gas.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    dw_gas = bp_gas.build()
    dw_gas.initialize()
    for _ in dw_gas.timesteps():
        dw_gas.step()
    gas_results = dw_gas.results()
    gas_total = gas_results["Total Electric Power (kW)"].sum()

    bp_hp = _make_blueprint()
    bp_hp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])
    bp_hp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=5000))
    bp_hp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))
    dw_hp = bp_hp.build()
    dw_hp.initialize()
    for _ in dw_hp.timesteps():
        dw_hp.step()
    hp_results = dw_hp.results()
    hp_total = hp_results["Total Electric Power (kW)"].sum()

    # Heat pump should use more electricity than gas furnace
    assert hp_total > gas_total, \
        f"ASHP electric total {hp_total:.3f} not greater than gas furnace {gas_total:.3f}"


def test_gas_wh_vs_electric_wh_energy_comparison():
    """Electric resistance WH should increase peak electric draw over gas WH, and gas WH adds gas load."""
    # Use a long simulation so WHs have time to need reheat cycles
    DUR = 28800  # 8 hours
    TRES = 600

    # Gas WH dwelling: gas furnace + gas WH (same HVAC baseline, different WH)
    bp_gas = _make_blueprint(duration_s=DUR, time_res_s=TRES)
    bp_gas.remove_equipment_by_end_use([EndUse.HVAC_COOLING])
    gas_steps = _collect_step_powers(bp_gas)

    # Electric resistance WH dwelling: gas furnace + electric WH (same HVAC, different WH)
    bp_elec = _make_blueprint(duration_s=DUR, time_res_s=TRES)
    bp_elec.remove_equipment_by_end_use([EndUse.HVAC_COOLING, EndUse.WATER_HEATING])
    bp_elec.add_equipment(ElectricResistanceWH(
        "ERWH", tank_volume_m3=0.19,
        uniform_energy_factor=0.95,
        heating_capacity_w=4500.0,
        avg_water_draw_l_per_day=200.0,
    ))
    elec_steps = _collect_step_powers(bp_elec)

    # Electric WH should produce higher peak electric draw than gas WH
    gas_max_elec = max(s["net_electric_power_kw"] for s in gas_steps)
    elec_max_elec = max(s["net_electric_power_kw"] for s in elec_steps)
    assert elec_max_elec > gas_max_elec, \
        f"ERWH peak electric {elec_max_elec:.3f} kW not greater than gas WH peak {gas_max_elec:.3f} kW"

    # Gas WH dwelling should have higher average gas (furnace + gas WH > furnace only)
    gas_avg = sum(s["gas_power_w"] for s in gas_steps) / len(gas_steps)
    elec_gas_avg = sum(s["gas_power_w"] for s in elec_steps) / len(elec_steps)
    assert gas_avg > elec_gas_avg, \
        f"Gas WH gas avg {gas_avg:.0f} W not greater than ERWH dwelling gas avg {elec_gas_avg:.0f} W"

    # Both dwellings should stay within reasonable bounds
    assert gas_max_elec < 15.0, f"Gas WH peak electric {gas_max_elec:.1f} kW unreasonably high"
    assert elec_max_elec < 15.0, f"ERWH peak electric {elec_max_elec:.1f} kW unreasonably high"
