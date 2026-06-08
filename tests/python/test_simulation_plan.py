"""Test multi-segment simulation with equipment swapping."""

from datetime import datetime

import polars as pl
import pytest
from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from ochre_next import ASHPCooler, ASHPHeater, DwellingBlueprint, EndUse
from ochre_next.simulation_plan import SimulationPlan


def test_simulation_plan_single_segment():
    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        time_res_s=60,
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 10),
    )
    results = plan.run()
    assert results is not None
    assert len(results) > 0
    assert "segment" in results.columns


def test_simulation_plan_two_segments_same_equipment():
    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        time_res_s=60,
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 5),
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 5),
        end=datetime(2024, 1, 1, 0, 10),
    )
    results = plan.run()
    assert len(results) > 0
    segments = sorted(results["segment"].unique().to_list())
    assert segments == [0, 1], f"Expected segments [0, 1], got {segments}"


def test_simulation_plan_empty_segments_raises():
    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        time_res_s=300,
    )
    with pytest.raises(ValueError, match="no segments"):
        plan.run()


def test_simulation_plan_swap_furnace_to_ashp():
    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        time_res_s=60,
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 5),
    )

    def setup_ashp(bp: DwellingBlueprint) -> None:
        bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])
        bp.add_equipment(
            ASHPHeater("ASHP", capacity_w=12000, hspf=9.5, backup_capacity_w=10000)
        )
        bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))

    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 5),
        end=datetime(2024, 1, 1, 0, 10),
        setup=setup_ashp,
    )
    results = plan.run()
    assert results is not None
    assert len(results) > 0
    # Verify both segments produced data
    segments = results["segment"].unique().sort().to_list()
    assert segments == [0, 1], f"Expected segments [0, 1], got {segments}"
    # Verify equipment actually changed: electric power differs between segments
    # (gas furnace draws minimal electricity; ASHP draws substantial power)
    power = results["Total Electric Power (kW)"]
    assert power.is_not_nan().all()
    power_col = "Total Electric Power (kW)"
    seg0_mean = results.filter(pl.col("segment") == 0)[power_col].mean()
    seg1_mean = results.filter(pl.col("segment") == 1)[power_col].mean()
    assert seg0_mean != seg1_mean, (
        f"Expected electric power to differ between "
        f"gas-furnace segment (mean={seg0_mean:.3f}) "
        f"and ASHP segment (mean={seg1_mean:.3f})"
    )
