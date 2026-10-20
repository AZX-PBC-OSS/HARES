"""The resolved-parameter read-back's Python surface: the parameters the
physics actually uses, readable from the built dwelling as a dict of SI
floats and strings, before any step runs."""

import pytest

from conftest import HPXML, make_dwelling


def read_back() -> dict:
    dwelling = make_dwelling(duration_s=300, time_res_s=60)
    return dwelling.resolved_model()


def names(model: dict) -> dict[str, dict]:
    return {eq["name"]: eq for eq in model["equipment"]}


def test_resolved_model_reads_back_the_input_values():
    model = read_back()
    furnace = names(model)["Gas Furnace"]
    parameters = {p["name"]: p for p in furnace["parameters"]}
    assert parameters["afue"]["value"] == pytest.approx(0.92)
    assert parameters["afue"]["unit"] == "fraction"
    # The declared HeatingCapacity is a Btu/h input; the read-back is the
    # SI watts the physics uses (36000 Btu/h = 10550.55852 W).
    assert parameters["rated_capacity_w"]["value"] == pytest.approx(10_550.55852, rel=1e-9)
    assert parameters["rated_capacity_w"]["unit"] == "W"
    # The declared HPXML setpoints read back as the input's source.
    assert furnace["heating_setpoint"] is not None
    assert "HPXML" in furnace["heating_setpoint"]


def test_resolved_model_is_a_dict_of_si_floats_and_strings():
    model = read_back()
    assert model["equipment"], "the read-back carries the resolved equipment"
    for eq in model["equipment"]:
        assert isinstance(eq["name"], str)
        assert isinstance(eq["end_use"], str)
        for parameter in eq["parameters"]:
            assert isinstance(parameter["name"], str)
            assert isinstance(parameter["value"], float)
            assert isinstance(parameter["unit"], str)
    for boundary in model["boundaries"]:
        assert isinstance(boundary["id"], str)
        assert isinstance(boundary["kind"], str)
        assert isinstance(boundary["same_zone"], bool)
        for key in ("area_m2", "r_effective_m2k_w", "films_m2k_w", "c_j_k"):
            assert isinstance(boundary[key], float)
        for layer in boundary["layers"]:
            assert isinstance(layer["r_m2k_w"], float)
            assert isinstance(layer["c_j_m2k"], float)
    for window in model["windows"]:
        for key in ("area_m2", "u_w_m2k", "shgc"):
            assert isinstance(window[key], float)
    infiltration = model["infiltration"]
    for key in ("ach50", "ela_m2", "constant_ach"):
        assert infiltration[key] is None or isinstance(infiltration[key], float)
    site = model["site"]
    for key in ("latitude_deg", "longitude_deg", "elevation_m", "utc_offset_h"):
        assert isinstance(site[key], float)


def test_resolved_model_boundaries_layers_sum_to_the_effective_resistance():
    model = read_back()
    checked = 0
    for boundary in model["boundaries"]:
        if boundary["kind"] in ("slab", "window", "skylight"):
            continue
        layers = sum(layer["r_m2k_w"] for layer in boundary["layers"])
        # A same-zone fin's network chain drops one half-resistor: the
        # cut layer's outer half, which neither build path wires (the
        # precomputed path removes the pad-average's first resistor, the
        # cut stub).
        dropped = (
            boundary["layers"][0]["r_m2k_w"] / 2 if boundary["same_zone"] else 0.0
        )
        expected = layers + boundary["films_m2k_w"] - dropped
        relative = abs(expected - boundary["r_effective_m2k_w"]) / boundary["r_effective_m2k_w"]
        assert relative <= 1e-12, (
            f"boundary {boundary['id']!r}: the network's chain (layers, minus the "
            f"fin's dropped half-resistor, plus films) must equal the effective "
            f"resistance to 1e-12 relative, got {relative}"
        )
        checked += 1
    assert checked > 0, "the base dwelling's opaque boundaries must be checked"


def test_resolved_model_reads_identical_twice():
    dwelling = make_dwelling(duration_s=300, time_res_s=60)
    first = dwelling.resolved_model()
    second = dwelling.resolved_model()
    assert first == second, "the read-back's dict builds identically on every read"


def test_resolved_model_carries_the_run_clock():
    """The dwelling's read-back carries the run's clock: the first step's
    local wall clock, the site offset, the step and the simulated step
    count of the configured horizon - the number a caller checks a
    report's row count against (`validate()` carries the same read-back;
    the built dwelling's must carry it too)."""
    model = read_back()
    clock = model["clock"]
    assert clock["steps"] == 5
    assert clock["time_res_s"] == 60
    assert clock["start"] == "2019-01-01T00:00:00"
    assert clock["utc_offset_s"] == -25200
