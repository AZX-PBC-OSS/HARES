"""Routed-finding gates: the dwelling's external-input boundaries that
still lack the non-finite guard.

The same-shape sweep of `py_dwelling.rs`'s external-input surface (the
class the landed `set_solar_override` walk belongs to): values cross
pyo3 `f64` extraction with no `is_finite` guard, the dwelling's
downstream finiteness screens are cfg-gated to debug/
`check_invariants` builds, and a NaN's comparisons are always false, so
a crossed value silently stops the consuming conditionals from firing.
The fixed-and-gated members of the class: `set_solar_override` (the
post-parse walk), `add_actor_by_name` params (`py_to_config_value`),
the tariff objects (guarded at construction, `python_to_json_value`),
and the equipment constructors (`from_typed`'s finite walk).

The unguarded members found by this round's sweep, each **proven by
execution** (the value silently crosses today):

1. `set_price_signal` — `dict_optional`'s f64 extraction: a NaN
   electricity price silently arms the price actors (the EvDriver's
   price logic: NaN comparisons always false).
2. `set_grid_voltage` — a raw `f64` kwarg with no guard: a NaN
   per-unit voltage silently lands in the grid state.
3. `set_equipment_lut` — `extract_ocv_table`/`extract_f64_axis` (the
   OCV/U-neg/LUT-grid extractors): a NaN OCV point silently poisons the
   pack electrical model's terminal-voltage solve — the exact machinery
   the pack-thermal alignment built and guards at every other entry.

The boundary guards **landed** (I-07 fix round: the shared
`validate_finite`/`finite_f64_*` helpers in `utils.rs`, the OCV/U-neg
`new` constructors' finiteness legs in `battery/ocv.rs` mirroring
`RegularGridInterpolator::new`'s), so the former strict-xfail markers
were removed per the self-destruct contract — these tests are now the
permanent regression gates: every face below must keep raising loudly
(TypeError/ValueError) at the boundary, and the finite controls beside
them must keep attaching (value-triggered rejection, never a blanket
failure).
"""

import pytest

from conftest import make_dwelling

import ochre_next


@pytest.mark.parametrize("bad", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_price_signal_fails_loudly_at_the_boundary(bad: float) -> None:
    dwelling = make_dwelling()
    with pytest.raises((TypeError, ValueError)):
        dwelling.set_price_signal({"electricity_price": bad})


def test_finite_price_signal_attaches() -> None:
    # Control: the channel works for finite values today and after the
    # guard lands (value-triggered rejection, never a blanket failure).
    dwelling = make_dwelling()
    dwelling.set_price_signal({"electricity_price": 0.15, "export_price": 0.05})
    assert dwelling is not None


@pytest.mark.parametrize("bad", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_grid_voltage_fails_loudly_at_the_boundary(bad: float) -> None:
    dwelling = make_dwelling()
    with pytest.raises((TypeError, ValueError)):
        dwelling.set_grid_voltage(bad)


def test_finite_grid_voltage_attaches() -> None:
    # Control: the channel works for finite values today and after the
    # guard lands.
    dwelling = make_dwelling()
    dwelling.set_grid_voltage(1.0)
    assert dwelling is not None


@pytest.mark.parametrize("bad", [float("nan"), float("inf")])
def test_non_finite_ocv_table_fails_loudly_at_the_boundary(bad: float) -> None:
    dwelling = make_dwelling()
    dwelling.add_battery(
        ochre_next.Battery(
            "Bat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.5
        )
    )
    with pytest.raises((TypeError, ValueError)):
        dwelling.set_equipment_lut(
            "Bat",
            ochre_next.LutType.Ocv,
            [(0.0, 3.0), (0.5, bad), (1.0, 3.7)],
        )


def test_negative_infinite_ocv_table_is_already_rejected() -> None:
    """The -inf face is already loud — `OcvTable::new`'s one-sided
    "voltage_v must be positive" validation rejects it (verified by
    execution: the only non-finite OCV value that does NOT silently
    cross). Pinned as a passing probe-gate so the one-sided boundary
    stays loud alongside the gate above (now the permanent regression
    gate for the NaN and +inf faces the constructor's finiteness legs
    reject — the faces that formerly slipped the comparison).
    """
    dwelling = make_dwelling()
    dwelling.add_battery(
        ochre_next.Battery(
            "Bat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.5
        )
    )
    with pytest.raises((TypeError, ValueError)):
        dwelling.set_equipment_lut(
            "Bat",
            ochre_next.LutType.Ocv,
            [(0.0, 3.0), (0.5, float("-inf")), (1.0, 3.7)],
        )


def test_finite_ocv_table_attaches() -> None:
    # Control: the LUT channel works for finite values today and after the
    # guard lands.
    dwelling = make_dwelling()
    dwelling.add_battery(
        ochre_next.Battery(
            "Bat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.5
        )
    )
    dwelling.set_equipment_lut(
        "Bat",
        ochre_next.LutType.Ocv,
        [(0.0, 3.0), (0.5, 3.5), (1.0, 3.7)],
    )
    assert dwelling.has_equipment_lut("Bat", ochre_next.LutType.ocv())


# The fourth member of the class, found by the coverage round's
# reassessment sweep (the `dict_optional`/`dict_required` copy in
# `py_control.rs`): the ControlSignal construction boundary applies the
# shared `validate_finite`/`validate_range` mechanisms inconsistently.
# The fix round guarded every raw-crossing f64 field — the 15 gated
# below plus the sweep's own findings (`kvar`, `power_factor`,
# `ThermalSetpointDelta`'s deltas, `EventDelay`'s `delay_s`,
# `EvSetReadyBy`'s `departure_hour`, `DemandResponse`'s `duration_s`,
# and the same fields on the typed constructors) — with the shared
# `validate_finite`/`finite_f64_*` helpers now unified in `utils.rs`.
# The sharpest evidence nothing prevented the gap: `PowerSetpoint`'s
# `min_soc`/`max_soc` were raw while the SOCTarget arm's same-named
# fields ARE validated — the same mechanism, applied next door,
# missing here. A dispatched NaN silently stops the receiving
# equipment's conditionals (a NaN heating setpoint never heats; a NaN
# `min_soc` breaks the EV's effective-floor max).


@pytest.mark.parametrize(
    "build",
    [
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "ThermalSetpoint", "heating_setpoint_c": float("nan")}
            ),
            id="from_dict-thermal-heating_setpoint_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "ThermalSetpoint", "cooling_setpoint_c": float("nan")}
            ),
            id="from_dict-thermal-cooling_setpoint_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "ThermalSetpoint", "deadband_c": float("nan")}
            ),
            id="from_dict-thermal-deadband_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "PowerSetpoint", "active_power_kw": 1.0, "min_soc": float("nan")}
            ),
            id="from_dict-power-min_soc",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "PowerSetpoint", "active_power_kw": 1.0, "max_soc": float("nan")}
            ),
            id="from_dict-power-max_soc",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "HumiditySetpoint", "target_rh": float("nan")}
            ),
            id="from_dict-humidity-target_rh",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "HumiditySetpoint", "target_rh": 0.5, "min_rh": float("nan")}
            ),
            id="from_dict-humidity-min_rh",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "HumiditySetpoint", "target_rh": 0.5, "max_rh": float("nan")}
            ),
            id="from_dict-humidity-max_rh",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "PowerLimit", "max_power_kw": 5.0, "ramp_rate_kw_per_s": float("nan")}
            ),
            id="from_dict-power_limit-ramp_rate",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.from_dict(
                {"type": "DutyCycle", "on_fraction": 0.5, "period_s": float("nan")}
            ),
            id="from_dict-duty_cycle-period_s",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.thermal_setpoint(heat_c=float("nan")),
            id="typed-thermal-heat_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.thermal_setpoint(cool_c=float("nan")),
            id="typed-thermal-cool_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.thermal_setpoint(deadband_c=float("nan")),
            id="typed-thermal-deadband_c",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.power_setpoint(kw=1.0, min_soc=float("nan")),
            id="typed-power-min_soc",
        ),
        pytest.param(
            lambda: ochre_next.ControlSignal.power_setpoint(kw=1.0, max_soc=float("nan")),
            id="typed-power-max_soc",
        ),
    ],
)
def test_non_finite_control_signal_fields_fail_loudly_at_the_boundary(build) -> None:
    with pytest.raises((TypeError, ValueError)):
        build()


def test_validated_control_signal_fields_stay_loud() -> None:
    """Control: the shared validate_finite/validate_range mechanisms work
    where applied — the validated faces reject non-finite loudly today
    (proven by execution), so the routed gate above is about the missing
    applications of a working mechanism, not a broken one — and these
    faces must stay loud through the fix.
    """
    with pytest.raises((TypeError, ValueError), match="finite"):
        ochre_next.ControlSignal.from_dict(
            {"type": "SOCTarget", "target_soc": float("nan")}
        )
    with pytest.raises((TypeError, ValueError), match="finite"):
        ochre_next.ControlSignal.power_setpoint(kw=float("nan"))
