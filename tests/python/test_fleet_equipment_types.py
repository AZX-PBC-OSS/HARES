"""The stepping fleet's typed equipment surface: per-dwelling
descriptors the per-DER folds group on.

The descriptor's ``equipment_type`` is the engine's own kind of the
equipment (the same value the single dwelling's ``equipment()``
descriptors carry), never a name heuristic: a fleet-side fold groups
trip counts on it the way the single dwelling's surface does.
"""

from __future__ import annotations

from collections import Counter
from pathlib import Path

from hares import DwellingConfig, SimulationConfig, SteppableFleet

ROOT = Path(__file__).resolve().parents[2]

BMS_MODE = '"Manual"'


def _pv_battery_config(bldg_id: int) -> DwellingConfig:
    sim_config = SimulationConfig(
        start_time="2023-06-01T00:00:00-07:00",
        duration=21600,
        time_res=900,
    )
    return DwellingConfig(
        hpxml=str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml"),
        schedule=str(ROOT / "data/examples/BEopt_example_schedule.csv"),
        weather=str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        config=sim_config,
        bldg_id=bldg_id,
        overrides={
            "PV": {"der_1547": {"category": "II"}},
            "Battery": {"der_1547": {"category": "II"}, "bms_mode": BMS_MODE},
        },
    )


def test_mixed_fleet_per_type_trip_counts() -> None:
    """The typed enumeration: the descriptors' equipment order is the
    published telemetry's order, and a sag's per-type trip counts group
    on the descriptor's own kind. The red state at the parent was the
    surface's absence: the fleet carried names only, so a fold could only
    classify by name substrings and was wrong for any non-default name.

    The sag: 0.6 pu sits inside Category II's UV1 band (0.70 pu) and the
    900 s step covers its 10 s clearing time, so both dwellings' PV and
    Battery inverters trip on the sag step (the single dwelling's
    der_1547 end-to-end numbers). The non-DER roster reads 0.0: the layer
    does not govern it.
    """
    fleet = SteppableFleet.from_configs(
        [_pv_battery_config(1), _pv_battery_config(2)], n_threads=0
    )

    fleet.step()
    fleet.set_grid_voltage_all(0.6)
    fleet.step()

    per_type_tripped: Counter[str] = Counter()
    for index in range(len(fleet)):
        descriptors = fleet.equipment_descriptors(index)
        assert [d.name for d in descriptors] != [], "the dwelling carries equipment"
        types = [d.equipment_type for d in descriptors]
        assert "PV" in types and "Battery" in types

        telemetry = fleet.telemetry(index).equipment()
        names = list(telemetry["names"])
        flags = list(telemetry["der1547_tripped"])
        # The published equipment order is the descriptors' order: the
        # grouping keys on positions, never on name guesses.
        assert [d.name for d in descriptors] == names
        assert len(flags) == len(names)
        for descriptor, flag in zip(descriptors, flags):
            per_type_tripped[descriptor.equipment_type] += int(flag)

    assert per_type_tripped["PV"] == 4, (
        "both dwellings' PV arrays trip (the fixture's roster carries two)"
    )
    assert per_type_tripped["Battery"] == 2, "both dwellings' battery inverters trip"
    non_der = sum(
        count
        for equipment_type, count in per_type_tripped.items()
        if equipment_type not in ("PV", "Battery")
    )
    assert non_der == 0, "no non-DER equipment trips: the layer does not govern it"


def test_fleet_equipment_descriptors_rejects_an_unknown_home() -> None:
    fleet = SteppableFleet.from_configs([_pv_battery_config(1)], n_threads=0)
    try:
        fleet.equipment_descriptors(5)
    except IndexError as exc:
        assert "out of range" in str(exc)
    else:
        raise AssertionError("an out-of-range dwelling index must raise")
