"""Tests for SteppableFleet Python bindings."""

from __future__ import annotations

import threading
from pathlib import Path

import pytest

from hares._hares import HaresSimulationError
from hares import DwellingConfig, SimulationConfig, SteppableFleet
from hares._hares_types import FleetStepEntry

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"


def _build_valid_config(bldg_id: int = 1) -> DwellingConfig:
    sim_config = SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60)
    return DwellingConfig(
        hpxml=str(EXAMPLES / "BEopt_example.xml"),
        schedule=str(EXAMPLES / "BEopt_example_schedule.csv"),
        weather=str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        config=sim_config,
        bldg_id=bldg_id,
    )


def test_steppable_fleet_exposes_build_errors() -> None:
    valid = _build_valid_config(bldg_id=1)
    invalid = DwellingConfig(
        hpxml=str(EXAMPLES / "missing_building.xml"),
        schedule=str(EXAMPLES / "BEopt_example_schedule.csv"),
        weather=str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60),
        bldg_id=2,
    )

    fleet = SteppableFleet.from_configs([valid, invalid], n_threads=0)

    assert len(fleet) == 1
    assert len(fleet.build_errors) == 1
    assert fleet.build_errors[0]["bldg_id"] == 2
    assert isinstance(fleet.build_errors[0]["error"], str)
    assert fleet.build_errors[0]["error"]


def test_steppable_fleet_step_includes_reactive_power() -> None:
    valid = _build_valid_config(bldg_id=1)
    fleet = SteppableFleet.from_configs([valid], n_threads=0)

    entries = fleet.step()
    assert len(entries) == 1
    assert entries[0]["ok"] is True

    result = entries[0].get("result")
    assert result is not None
    assert "reactive_power_var" in result
    assert isinstance(result["reactive_power_var"], float)


def test_steppable_fleet_step_after_finish_raises() -> None:
    fleet = SteppableFleet.from_configs([_build_valid_config(bldg_id=1)], n_threads=0)

    while not fleet.is_finished():
        entries = fleet.step()
        assert all(entry["ok"] for entry in entries)

    with pytest.raises(HaresSimulationError) as exc_info:
        fleet.step()
    assert str(fleet.total_steps()) in str(exc_info.value)


def test_steppable_fleet_telemetry_out_of_range_raises_index_error() -> None:
    fleet = SteppableFleet.from_configs([_build_valid_config(bldg_id=1)], n_threads=0)

    with pytest.raises(IndexError):
        fleet.telemetry(len(fleet))


def _step_truth() -> dict[tuple[int, object], tuple[float, float]]:
    """Steps one solo fleet and records each step's own scalars.

    The engine is deterministic, so the ``(bldg_id, timestamp)`` key
    fixes every scalar the result dict carries for any identically built
    fleet, however its steps interleave with other callers'.
    """
    fleet = SteppableFleet.from_configs([_build_valid_config(bldg_id=1)], n_threads=0)
    truth: dict[tuple[int, object], tuple[float, float]] = {}
    while not fleet.is_finished():
        for entry in fleet.step():
            result = entry.get("result")
            assert result is not None, "the solo fleet's steps all succeed"
            assert "reactive_power_var" in result
            truth[(entry["bldg_id"], result["timestamp"])] = (
                result["net_electric_power_w"],
                result["reactive_power_var"],
            )
    return truth


def test_concurrent_steps_return_one_steps_own_results() -> None:
    """Two threads stepping one fleet concurrently each get one step's
    own results: every returned dict's reactive power pairs with the
    step its own timestamp names, and the two threads together see the
    whole episode exactly once.

    The regression: the binding released the fleet's locks after the
    step and re-read the reused results buffer while building the dicts,
    so with ``step`` taking ``&self`` and the GIL released across the
    step, a second Python thread could step in between and the first
    thread's dicts carried that thread's results paired with the first
    thread's telemetry reads. The binding now copies each dwelling's
    step data out under the locks and builds the dicts from the owned
    copies; the single-step binding takes ``&mut self``, so Python
    cannot share it across threads and it needs no copy.
    """
    truth = _step_truth()
    # The pairing check has teeth only if the scalars move across steps.
    assert len({reactive for _, reactive in truth.values()}) > 1, (
        "the fixture's reactive power must vary across steps for the "
        "pairing check to bite"
    )

    fleet = SteppableFleet.from_configs([_build_valid_config(bldg_id=1)], n_threads=0)
    collected: list[FleetStepEntry] = []
    collected_lock = threading.Lock()

    def step_until_finished() -> None:
        while True:
            try:
                entries = fleet.step()
            except HaresSimulationError:
                # The other thread stepped the episode past its end
                # between this thread's check and its step.
                return
            with collected_lock:
                collected.extend(entries)

    threads = [
        threading.Thread(target=step_until_finished, name=f"fleet-stepper-{n}")
        for n in range(2)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    # Every step of the episode came back exactly once across the two
    # threads, and each dict pairs its own step's scalars.
    assert len(collected) == len(truth), (
        f"the two threads returned {len(collected)} dicts for the "
        f"{len(truth)}-step episode"
    )
    seen: set[tuple[int, object]] = set()
    for entry in collected:
        result = entry.get("result")
        assert result is not None, "every window step succeeds in this fixture"
        key = (entry["bldg_id"], result["timestamp"])
        assert key in truth, f"step {key} is not one of the fleet's own steps"
        assert "reactive_power_var" in result
        net_electric_power_w, reactive_power_var = truth[key]
        assert result["net_electric_power_w"] == net_electric_power_w
        assert result["reactive_power_var"] == reactive_power_var
        seen.add(key)
    assert len(seen) == len(truth), "a step came back twice across the threads"
