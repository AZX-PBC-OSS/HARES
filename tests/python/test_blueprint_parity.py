"""Verify DwellingBlueprint.from_hpxml().build() == Dwelling.from_hpxml()."""

import pytest
from ochre_next import Dwelling, DwellingBlueprint
from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def test_blueprint_parity_default():
    dw1 = Dwelling.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    dw1.initialize()

    bp = DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    dw2 = bp.build()
    dw2.initialize()

    assert sorted(dw1.equipment_names()) == sorted(dw2.equipment_names())

    for _ in range(5):
        r1 = dw1.step()
        r2 = dw2.step()
        assert r1 is not None
        assert r2 is not None
        assert abs(r1["net_electric_power_kw"] - r2["net_electric_power_kw"]) < 1e-6


def test_blueprint_equipment_names():
    bp = DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    names = bp.equipment_names()
    assert isinstance(names, list)
    assert all(isinstance(n, str) for n in names)
    assert len(names) >= 3
