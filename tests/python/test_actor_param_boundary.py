"""Adversarial boundary attacks on the actor-param Python surface.

`Dwelling.add_actor_by_name` converts every params-dict value through
`py_to_config_value` (py_dwelling.rs) — the same Python→config float
conversion shape that the equipment typed-config boundary (the
`from_typed` finite walk) and the tariff `from_dict` converter already
reject non-finite floats at. The actor-param copy has no non-finite
guard: a NaN parameter is stored, arms the actor at seed time
(`actor_registry.rs` reads `heating_c`/`cooling_c`/`deadband_c`/
`hysteresis_c` with no finite check), and the failure surfaces only
mid-simulation: a telemetry write rejection (`Telemetry::set
called with non-finite value NaN for key 'heating_setpoint_c'`,
surfaced as a step failure), and, behind that unconditional
step-failure screen, an actor armed with a NaN parameter that never
heats. The loud rejection must happen at the boundary that received
the value, with the config field named — the same contract the two
fixed boundaries already meet.
"""

import pytest

from conftest import make_dwelling

# The fixture dwelling's heating equipment, the thermostat's target.
HEATING = "Gas Furnace"


@pytest.mark.parametrize(
    "param",
    ["heating_c", "cooling_c", "deadband_c", "hysteresis_c"],
)
def test_nan_thermostat_param_fails_loudly_at_the_boundary(param: str) -> None:
    with pytest.raises((TypeError, ValueError)):
        dwelling = make_dwelling()
        dwelling.add_actor_by_name(
            "IdealThermostat",
            "Thermostat",
            {"target": HEATING, param: float("nan")},
        )


@pytest.mark.parametrize(
    "param",
    ["heating_c", "cooling_c", "deadband_c", "hysteresis_c"],
)
def test_inf_thermostat_param_fails_loudly_at_the_boundary(param: str) -> None:
    with pytest.raises((TypeError, ValueError)):
        dwelling = make_dwelling()
        dwelling.add_actor_by_name(
            "IdealThermostat",
            "Thermostat",
            {"target": HEATING, param: float("inf")},
        )


def test_finite_thermostat_param_still_attaches_and_simulates(tmp_path) -> None:
    # Control: the rejection must be value-triggered, not blanket — a
    # finite setpoint attaches, simulates without panic, and the
    # thermostat's override heats the dwelling beyond its own schedule (the
    # observed behavior the NaN variant silently loses). The fixture's
    # scheduled heating setpoint is 20 °C, below the 21 °C override.
    def heating_delivered_w_sum(tag: str, params: dict[str, object] | None) -> float:
        dwelling = make_dwelling(
            output_verbosity=5, output_path=str(tmp_path / f"{tag}.csv"), write_output=True
        )
        if params is not None:
            dwelling.add_actor_by_name("IdealThermostat", "Thermostat", params)
        return float(dwelling.simulate()["HVAC Heating Delivered (W)"].sum())

    scheduled = heating_delivered_w_sum("scheduled", None)
    overridden = heating_delivered_w_sum("overridden", {"target": HEATING, "heating_c": 21.0})
    assert overridden > scheduled, (
        f"a finite 21 °C override must heat beyond the schedule alone "
        f"(override {overridden:.0f}, schedule {scheduled:.0f}), the observable "
        "the NaN variant silently loses"
    )


@pytest.mark.parametrize(
    "bad",
    [float("nan"), float("inf"), float("-inf")],
)
def test_non_finite_list_element_param_fails_loudly_at_the_boundary(bad: float) -> None:
    """A non-finite element inside a LIST-valued actor parameter must
    fail loudly at the same boundary — the converter's second guard arm
    (`py_to_config_value`'s element-wise check, `py_dwelling.rs`), which
    the scalar gates above do not reach: a dropped element check would
    silently store `FloatArray([..., NaN, ...])`, and every
    `get_f64_array` consumer (`actor_registry.rs`'s `occupancy_column` →
    the Occupant's presence schedule) is poisoned downstream with the
    same silent-never-acting release-build behavior the scalar guard's
    rationale names. `occupancy_column` is the legitimate list-valued
    param — the gate runs on the real consumer, not a synthetic key.
    """
    with pytest.raises((TypeError, ValueError), match="non-finite"):
        dwelling = make_dwelling()
        dwelling.add_actor_by_name(
            "Occupant",
            "Occupant",
            {"occupancy_column": [1.0, bad, 0.0]},
        )


def test_finite_list_element_param_still_attaches_and_simulates(tmp_path) -> None:
    """Control: the list-element rejection must be value-triggered, not
    blanket — a finite occupancy column attaches and the dwelling
    simulates (the channel the non-finite variant must be rejected from,
    not silently stored into).
    """
    dwelling = make_dwelling(output_verbosity=5, output_path=str(tmp_path / "dwelling_42.csv"))
    dwelling.add_actor_by_name(
        "Occupant",
        "Occupant",
        # The column must cover the run's steps (the default run is five
        # 60 s steps; a shorter column exhausts the presence schedule —
        # an unrelated loud error the landed occupant runtime raises).
        {"occupancy_column": [1.0, 0.0] * 6},
    )
    df = dwelling.simulate()
    assert df is not None and len(df) > 0, "the finite occupancy column must simulate"


def test_add_actor_by_name_without_params_uses_the_default() -> None:
    """The stub claims `params: dict[str, Any] | None = None`; the binding
    must carry that default. Under PyO3 0.29 a parameter without a
    `#[pyo3(signature)]` default is required, so the two-argument call the
    stub invites failed at call time with the stub's own default in place.
    The Occupant's parameters are all optional, so the two-argument call
    reaches its factory and attaches: the pin's vehicle. The
    IdealThermostat's factory requires its target and rejects the same
    call with the error that names it.
    """
    dwelling = make_dwelling()
    before = dwelling.actor_count()
    dwelling.add_actor_by_name("Occupant", "occupant")
    assert dwelling.actor_count() == before + 1, (
        "the two-argument call must attach the actor, not raise"
    )


def test_ideal_thermostat_without_params_is_rejected_naming_target() -> None:
    """A thermostat bound without its target is the misconfiguration the
    registry rejects at the boundary: the error names the parameter, it
    does not route the override to a default equipment name.
    """
    dwelling = make_dwelling()
    with pytest.raises(ValueError, match="target"):
        dwelling.add_actor_by_name("IdealThermostat", "t1")
