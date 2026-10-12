"""The `DwellingBlueprint.validate()` and `.catalogue()` acceptance:
every assembly-time check runs, the resolved model reads back as a dict,
and the catalogue equals the built dwelling's, all without running a
step."""

import pytest
from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from hares import DwellingBlueprint, SimulationConfig
from hares._hares import HaresEquipmentError


def _blueprint(duration_s=48 * 3600, time_res=3600, overrides=None):
    return DwellingBlueprint.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
        config=SimulationConfig(duration=duration_s, time_res=time_res, start_time="2023-01-01T00:00:00"),
        overrides=overrides,
    )


def test_blueprint_catalogue_in_python():
    """The blueprint's catalogue equals the built dwelling's."""
    blueprint = _blueprint()
    from_blueprint = blueprint.catalogue()
    dwelling = _blueprint().build()
    from_dwelling = dwelling.catalogue()
    assert from_blueprint == from_dwelling
    assert len(from_blueprint) > 0


def test_validate_returns_the_resolved_model_dict():
    """`validate()` reads the resolved model back as a dict: the site with
    its per-field sources, the clock, and every boundary's layers."""
    model = _blueprint().validate()
    assert set(model["site"]) >= {
        "latitude_deg",
        "longitude_deg",
        "elevation_m",
        "utc_offset_h",
    }
    for key in ("latitude_deg", "longitude_deg", "elevation_m", "utc_offset_h"):
        assert isinstance(model["site"][f"{key}_source"], str)

    clock = model["clock"]
    assert clock["steps"] == 48
    assert clock["time_res_s"] == 3600
    assert clock["utc_offset_s"] == -25200
    assert clock["start"] == "2023-01-01T00:00:00"

    boundaries = model["boundaries"]
    assert len(boundaries) > 0
    for boundary in boundaries:
        assert set(boundary) >= {
            "id",
            "kind",
            "area_m2",
            "r_effective_m2k_w",
            "films_m2k_w",
            "c_j_k",
            "layers",
        }
        for layer in boundary["layers"]:
            assert set(layer) == {"r_m2k_w", "c_j_m2k"}


def test_validate_rejects_an_infeasible_override():
    """An infeasible override fails `validate()` naming the cause, the
    same message the build fails with."""
    infeasible = {"Gas Furnace": {"afue": 1.33}}
    with pytest.raises(HaresEquipmentError, match="afue"):
        _blueprint(overrides=infeasible).validate()
    with pytest.raises(HaresEquipmentError, match="afue"):
        _blueprint(overrides=infeasible).build()
