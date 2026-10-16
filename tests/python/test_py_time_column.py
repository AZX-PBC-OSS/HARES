"""The Time column's Python-surface acceptance: the frame's ``Time`` reads
as a naive-local millisecond Datetime, the step dict's ``timestamp`` is a
naive ``datetime`` equal to the frame's first ``Time``, and the
``docs/python.md`` "Time" section's example runs as written.

``step_time_is_naive_local``: the cz2a inputs of the ``full_year_hourly``
golden, shortened to 48 h.
"""

import re
from datetime import datetime
from pathlib import Path

import polars as pl
import pytest

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "python" / "hares" / "defaults"

FIXTURE = ROOT / "tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")

DOCS = ROOT / "docs/python.md"


def make_dwelling(duration_s: int = 48 * 3600):
    from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

    dwelling = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=duration_s,
            time_res=3600,
            reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
        ),
    )
    dwelling.initialize()
    return dwelling


def test_step_time_is_naive_local() -> None:
    """The step dict's timestamp is naive and equals the frame's first
    Time: one convention, the row's own wall clock."""
    dwelling = make_dwelling(duration_s=3600)
    step = dwelling.step()
    timestamp = step["timestamp"]
    assert isinstance(timestamp, datetime)
    assert timestamp.tzinfo is None, "the step timestamp is naive local"
    result = dwelling.simulate()
    frame = result.reports[0].table()
    first_time = frame["Time"][0]
    assert isinstance(first_time, datetime)
    assert timestamp == first_time, "the step timestamp equals the row's Time"


def test_frame_time_is_naive_millisecond_datetime() -> None:
    """The frame's Time column reads as a polars Datetime at millisecond
    precision with no time zone."""
    dwelling = make_dwelling()
    result = dwelling.simulate()
    frame = result.reports[0].table()
    dtype = frame.schema["Time"]
    assert dtype == pl.Datetime("ms"), f"Time dtype moved to Datetime(ms), got {dtype}"
    assert frame.height == 48
    first = frame["Time"][0]
    assert first == datetime(2023, 1, 1, 0, 0), "the start's local wall clock"


def _time_section_example(docs: str) -> str:
    """The first ```python fenced block of the docs' "Time" section."""
    section = re.search(r"^## Time\n(.*?)(?=^## )", docs, re.S | re.M)
    assert section, 'docs/python.md carries a "## Time" section'
    block = re.search(r"```python\n(.*?)```", section.group(1), re.S)
    assert block, 'the "Time" section carries a ```python example'
    return block.group(1)


def test_docs_time_example() -> None:
    """The docs' "Time" section example runs as written: it builds a
    dwelling, runs it, and checks the first row's Time against start_time."""
    docs = DOCS.read_text()
    code = _time_section_example(docs)
    import os

    cwd = os.getcwd()
    os.chdir(ROOT)
    try:
        exec(compile(code, str(DOCS), "exec"), {})
    finally:
        os.chdir(cwd)
