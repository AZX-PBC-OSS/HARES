"""Equipment roster changes through the Python entrances.

A rejected change raises the exception class of its reason and leaves the
dwelling as it was; an accepted EV joins with its driver without
disturbing the drivers already attached.
"""

import pytest
from conftest import make_dwelling


def _ev_soc(dw, name: str) -> float:
    tel = dw.telemetry().equipment()
    return tel["soc"][tel["names"].index(name)]


def test_duplicate_equipment_name_raises_equipment_error():
    from ochre_next import Battery
    from ochre_next._hares import HaresEquipmentError

    dw = make_dwelling()
    dw.add_battery(Battery("Bat", 10.0))
    names = dw.equipment_names()

    with pytest.raises(HaresEquipmentError, match="duplicate equipment name 'Bat'"):
        dw.add_battery(Battery("Bat", 12.0))

    assert dw.equipment_names() == names


def test_replacing_missing_equipment_raises_config_error():
    from ochre_next import Battery
    from ochre_next._hares import HaresConfigError

    dw = make_dwelling()
    names = dw.equipment_names()

    with pytest.raises(HaresConfigError, match="not found"):
        dw.replace_equipment("No Such Equipment", Battery("Bat", 10.0))

    assert dw.equipment_names() == names


def test_rejected_add_pv_registers_no_irradiance_surface():
    from ochre_next import PV
    from ochre_next._hares import HaresEquipmentError

    dw = make_dwelling()
    dw.add_pv(PV("PV1", 5.0, 30.0, 180.0))
    surfaces = dw.surface_ids()

    with pytest.raises(HaresEquipmentError):
        dw.add_pv(PV("PV1", 5.0, 45.0, 90.0))

    assert dw.surface_ids() == surfaces


def test_duplicate_add_ev_with_driver_reports_the_duplicate_equipment():
    from ochre_next import EvArchetypeId, VehicleId
    from ochre_next._hares import HaresEquipmentError

    dw = make_dwelling()
    dw.add_ev_with_driver(VehicleId.tesla_model_y_lr(), EvArchetypeId.daily_commuter_l2(), seed=42)
    actors = dw.telemetry().actors()

    with pytest.raises(HaresEquipmentError, match="duplicate equipment name"):
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(), EvArchetypeId.daily_commuter_l2(), seed=42
        )

    assert dw.telemetry().actors() == actors


def test_adding_an_ev_leaves_the_existing_drivers_trajectory_unchanged():
    """A second add_ev attaches the new EV's driver only: the first EV's
    driver keeps its state, so the first EV's state of charge follows the
    same path as in a dwelling where the second EV never arrives."""
    from ochre_next import EV

    def ev1_soc(add_second: bool) -> list[float]:
        dw = make_dwelling(duration_s=3 * 86400, time_res_s=900, seed=42)
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        for _ in range(50):
            dw.step()
        if add_second:
            dw.add_ev(EV("EV2", capacity_kwh=60.0, max_charging_kw=7.2))
        trajectory = []
        for _ in range(2 * 96):
            dw.step()
            trajectory.append(_ev_soc(dw, "EV1"))
        return trajectory

    with_second = ev1_soc(add_second=True)
    assert max(with_second) - min(with_second) > 0.05, "EV1 must be driven"
    assert with_second == ev1_soc(add_second=False)
