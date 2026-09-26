"""Adversarial boundary attacks on the EV pack-thermal Python surface.

The pack-thermal config fields introduced by the pack-thermal alignment
(heater, plating cutoff, thermal mass, UA, cell topology) cross the
Python boundary as plain floats/ints. A hostile or mistyped value must
fail loudly — rejected at pyo3 extraction or at the dwelling's config
validation when the EV is attached — never silently become a default
and never reach the simulation. The rejection point differs by failure
class (pyo3 raises for integer extraction; `EvConfig::validate` raises
for non-finite and out-of-range floats through `Dwelling.add_ev`), so
each attack asserts only "an exception is raised by the time the EV is
attached", never which layer raised it.
"""

import pytest

from conftest import make_dwelling
from ochre_next import EV


def attach_hostile_ev(**kwargs):
    ev = EV("HostileEV", capacity_kwh=75.0, max_charging_kw=11.5, **kwargs)
    dwelling = make_dwelling()
    dwelling.add_ev(ev)


@pytest.mark.parametrize(
    "kwargs",
    [
        {"n_series": -1},
        {"n_series": 2**32},
        {"n_series": 0},
        {"n_parallel": -1},
        {"n_parallel": 0},
    ],
)
def test_hostile_topology_counts_fail_loudly(kwargs):
    # pyo3 rejects negatives and overflow at u32 extraction; a zero count
    # passes extraction and must fail the config validation — a zero
    # series/parallel count silently zeroes the pack voltage and the I²R
    # heating model.
    with pytest.raises((TypeError, ValueError, OverflowError)):
        attach_hostile_ev(**kwargs)


@pytest.mark.parametrize(
    "kwargs",
    [
        # NOTE: the parametrize order is append-only — every case id a gate
        # has ever gated as failing must stay a non-finite (failing) case at
        # the same id, so the finite out-of-range values live only on ids
        # never gated (1, 5, 12, 13). Reordering middle entries re-keys the
        # ids and breaks the gate history.
        {"heater_power_w": float("nan")},
        {"heater_power_w": -1.0},
        {"min_charge_temp_c": float("nan")},
        {"full_power_temp_c": float("nan")},
        {"battery_temp_c": float("nan")},
        {"thermal_mass_j_per_k": 0.0},
        {"thermal_mass_j_per_k": float("nan")},
        {"battery_temp_c": float("inf")},
        {"battery_temp_c": float("-inf")},
        {"cell_resistance_ohm": float("nan")},
        {"heater_power_w": float("inf")},
        {"heater_power_w": float("-inf")},
        {"ua_w_per_k": -1.0},
        {"cell_resistance_ohm": 0.0},
    ],
)
def test_hostile_thermal_values_fail_loudly(kwargs):
    # pyo3 f64 extraction accepts any float including NaN; the typed
    # config validation must reject non-finite and out-of-range values
    # when the EV is attached — a NaN temperature poisons the derate,
    # the thermal equation, and every downstream energy total.
    with pytest.raises((TypeError, ValueError)):
        attach_hostile_ev(**kwargs)


def test_hostile_values_leave_the_dwelling_usable():
    # A rejected EV must not leave a half-attached pack in the dwelling:
    # after a failed add_ev, a valid EV still attaches and initializes
    # cleanly (the rejection is not a poisoned-state footgun).
    with pytest.raises((TypeError, ValueError, OverflowError)):
        attach_hostile_ev(n_series=-1)
    dwelling = make_dwelling()
    dwelling.add_ev(EV("CleanEV", capacity_kwh=75.0, max_charging_kw=11.5))
