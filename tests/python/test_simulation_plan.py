"""Test multi-segment simulation with equipment swapping."""

from datetime import datetime

import polars as pl
import pytest
from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER
from ochre_next import ASHPCooler, ASHPHeater, Battery, DwellingBlueprint, EndUse
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
    assert seg1_mean > seg0_mean, (
        f"Expected ASHP segment electric power (mean={seg1_mean:.3f}) "
        f"to exceed gas-furnace segment (mean={seg0_mean:.3f})"
    )


def test_thermal_continuity_across_segments():
    """Indoor temperature is continuous across segment boundaries."""
    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        time_res_s=300,
    )

    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 1, 0),
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 1, 0),
        end=datetime(2024, 1, 1, 2, 0),
    )

    results = plan.run()

    seg0 = results.filter(pl.col("segment") == 0)
    seg1 = results.filter(pl.col("segment") == 1)

    assert not seg0.is_empty(), "Segment 0 produced no data"
    assert not seg1.is_empty(), "Segment 1 produced no data"

    temp_cols = [c for c in results.columns if "temp" in c.lower() and "indoor" in c.lower()]
    if temp_cols:
        temp_col = temp_cols[0]
        seg0_last = seg0[temp_col].tail(1).item()
        seg1_first = seg1[temp_col].head(1).item()
        assert abs(seg0_last - seg1_first) < 0.5, (
            f"Thermal discontinuity: seg0={seg0_last:.1f}C, seg1={seg1_first:.1f}C"
        )


def test_post_build_callback_adds_battery():
    """post_build callback can add equipment before initialize."""
    plan = SimulationPlan(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS), bldg_id=42,
        time_res_s=300,
    )
    battery_added = []

    def add_battery_post_build(dw):
        battery_added.append(True)
        assert hasattr(dw, "add_battery"), "Dwelling has no add_battery method"
        dw.add_battery(Battery("TestBat", capacity_kwh=10))

    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 10),
        post_build=add_battery_post_build,
    )
    results = plan.run()
    assert len(battery_added) == 1, "post_build callback was not invoked"
    assert results is not None
