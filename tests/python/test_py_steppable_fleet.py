"""Tests for SteppableFleet Python bindings."""

from __future__ import annotations

from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"


def _fixtures_available() -> bool:
    return (
        (EXAMPLES / "BEopt_example.xml").exists()
        and (EXAMPLES / "BEopt_example_schedule.csv").exists()
        and (EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw").exists()
    )


def _build_valid_config(bldg_id: int = 1):
    try:
        from ochre_next import DwellingConfig, SimulationConfig
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")

    if not _fixtures_available():
        pytest.skip("OCHRE fixtures not available")

    sim_config = SimulationConfig(duration_s=3600, time_res_s=60)
    return DwellingConfig(
        hpxml=str(EXAMPLES / "BEopt_example.xml"),
        schedule=str(EXAMPLES / "BEopt_example_schedule.csv"),
        weather=str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        config=sim_config,
        bldg_id=bldg_id,
    )


def test_steppable_fleet_exposes_build_errors() -> None:
    try:
        from ochre_next import DwellingConfig, SteppableFleet
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")

    if not _fixtures_available():
        pytest.skip("OCHRE fixtures not available")

    valid = _build_valid_config(bldg_id=1)
    invalid = DwellingConfig(
        hpxml=str(EXAMPLES / "missing_building.xml"),
        schedule=str(EXAMPLES / "BEopt_example_schedule.csv"),
        weather=str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        bldg_id=2,
    )

    fleet = SteppableFleet.from_configs([valid, invalid], n_threads=0)

    assert len(fleet) == 1
    assert len(fleet.build_errors) == 1
    assert fleet.build_errors[0]["bldg_id"] == 2
    assert isinstance(fleet.build_errors[0]["error"], str)
    assert fleet.build_errors[0]["error"]


def test_steppable_fleet_step_includes_reactive_power() -> None:
    try:
        from ochre_next import SteppableFleet
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")

    valid = _build_valid_config(bldg_id=1)
    fleet = SteppableFleet.from_configs([valid], n_threads=0)

    entries = fleet.step()
    assert len(entries) == 1
    assert entries[0]["ok"] is True

    result = entries[0]["result"]
    assert result is not None
    assert "reactive_power_kvar" in result
    assert isinstance(result["reactive_power_kvar"], float)
