"""The report surface's Python acceptance: a memory report's frame.

`python_memory_report_returns_frame`: the cz4a_ashp_hpwh parity inputs, one
Memory report at ``Selection.verbosity(2)``: ``simulate()`` returns a frame
with that report's schema and 96 rows.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")


def test_python_memory_report_returns_frame() -> None:
    """One Memory report at verbosity 2: simulate() returns the report's
    frame with 96 rows (24 h at 900 s), the full selected schema."""
    from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

    dwelling = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=86400,
            time_res=900,
            reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
        ),
    )
    dwelling.initialize()
    df = dwelling.simulate()

    assert df.height == 96, f"24 h at 900 s is 96 rows, got {df.height}"
    assert "Time" in df.columns
    assert "Total Electric Power (kW)" in df.columns
    # The verbosity-2 tier's columns are there (the outdoor dry bulb is a
    # context column at every tier; the zone temperature needs 2).
    assert "Outdoor Dry Bulb (C)" in df.columns
    assert any(c.startswith("Temperature -") for c in df.columns)
