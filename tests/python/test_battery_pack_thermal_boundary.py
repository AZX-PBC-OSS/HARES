"""Adversarial boundary attacks on the stationary Battery's pack-thermal
Python surface — the sibling of the EV boundary file.

The stationary Battery's pack-thermal fields (heater, plating cutoff,
cell topology) cross the same Python boundary and the same typed-config
round-trip as the EV's: `EquipmentConfig::from_typed` serializes to JSON,
and JSON cannot represent non-finite floats, so a `Some(NaN)` would
silently become `None` and the field's documented default would apply
with no error anywhere — the exact silent-substitution channel the EV
boundary file measured. The non-finite guard at the `from_typed` choke
point (one guard, every typed config, every entry path) is what makes
these fail; this file pins the sibling surface end to end so the guard
cannot regress for one equipment only. Each attack asserts only "an
exception is raised by the time the battery is attached", never which
layer raised it.
"""

import pytest

from conftest import make_dwelling
from ochre_next import Battery


def attach_hostile_battery(**kwargs):
    battery = Battery("HostileBattery", 10.0, **kwargs)
    dwelling = make_dwelling()
    dwelling.add_battery(battery)


@pytest.mark.parametrize(
    "kwargs",
    [
        {"heater_power_w": float("nan")},
        {"heater_power_w": float("inf")},
        {"heater_power_w": float("-inf")},
        {"heater_threshold_c": float("nan")},
        {"min_charge_temp_c": float("nan")},
        {"cell_resistance_ohm": float("nan")},
    ],
)
def test_hostile_thermal_values_fail_loudly(kwargs):
    # pyo3 f64 extraction accepts any float including NaN; the non-finite
    # guard at the typed-config choke point must reject it by the time the
    # battery is attached — a silently nulled field would apply its default
    # (a heaterless config with the default heater, a plating cutoff at
    # the default) with no error anywhere.
    with pytest.raises((TypeError, ValueError)):
        attach_hostile_battery(**kwargs)
