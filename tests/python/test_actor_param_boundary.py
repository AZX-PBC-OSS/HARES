"""Adversarial boundary attacks on the actor-param Python surface.

`Dwelling.add_actor_by_name` converts every params-dict value through
`py_to_config_value` (py_dwelling.rs) — the same Python→config float
conversion shape that the equipment typed-config boundary (the
`from_typed` finite walk) and the tariff `from_dict` converter already
reject non-finite floats at. The actor-param copy has no non-finite
guard: a NaN parameter is stored, arms the actor at seed time
(`actor_registry.rs` reads `heating_c`/`cooling_c`/`deadband_c`/
`hysteresis_c` with no finite check), and the failure surfaces only
mid-simulation — a telemetry panic in debug builds (`Telemetry::set
called with non-finite value NaN for key 'heating_setpoint_c'`), and,
with that panic cfg-gated to debug/check_invariants builds, a silent
never-heating actor behind a `tracing::error!` log line in release
builds. The loud rejection must happen at the boundary that received
the value, with the config field named — the same contract the two
fixed boundaries already meet.
"""

import pytest

from conftest import make_dwelling


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
            {"target": "HVAC", param: float("nan")},
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
            {"target": "HVAC", param: float("inf")},
        )


def test_finite_thermostat_param_still_attaches_and_simulates() -> None:
    # Control: the rejection must be value-triggered, not blanket — a
    # finite setpoint attaches, simulates without panic, and the
    # thermostat heats the dwelling (the observed behavior the NaN
    # variant silently loses).
    dwelling = make_dwelling(output_verbosity=5)
    dwelling.add_actor_by_name(
        "IdealThermostat",
        "Thermostat",
        {"target": "HVAC", "heating_c": 21.0},
    )
    df = dwelling.simulate()
    heat_cols = [
        c for c in df.columns if "HVAC" in c and "Heating" in c and "kW" in c
    ]
    assert heat_cols, "the fixture dwelling exposes an HVAC heating column"
    assert df[heat_cols[0]].sum() > 0.0, (
        "a finite 21 °C heating setpoint must produce heating energy — "
        "the observable the NaN variant silently loses"
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


def test_finite_list_element_param_still_attaches_and_simulates() -> None:
    """Control: the list-element rejection must be value-triggered, not
    blanket — a finite occupancy column attaches and the dwelling
    simulates (the channel the non-finite variant must be rejected from,
    not silently stored into).
    """
    dwelling = make_dwelling(output_verbosity=5)
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
