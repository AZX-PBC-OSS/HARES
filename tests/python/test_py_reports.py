"""The report surface's Python acceptance: a memory report's frame.

`python_memory_report_returns_frame`: the cz4a_ashp_hpwh parity inputs, one
Memory report at ``Selection.verbosity(2)``: ``simulate()`` returns a frame
with that report's schema and 96 rows.
"""

from __future__ import annotations

import concurrent.futures
import multiprocessing
import pickle
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "python" / "hares" / "defaults"

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
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=86400,
            time_res=900,
            reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
        ),
    )
    dwelling.initialize()
    result = dwelling.simulate()
    df = result.reports[0].table()

    assert df.height == 96, f"24 h at 900 s is 96 rows, got {df.height}"
    assert "Time" in df.columns
    assert "Total Electric Power (kW)" in df.columns
    # The verbosity-2 tier's columns are there (the outdoor dry bulb is a
    # context column at every tier; the zone temperature needs 2).
    assert "Outdoor Dry Bulb (C)" in df.columns
    assert any(c.startswith("Temperature -") for c in df.columns)


def test_frequency_minutes_zero_is_a_value_error_not_a_panic() -> None:
    """``Frequency.minutes(0)`` is the typed reporting-frequency value
    error (the rule's own message), never a Rust panic surface."""
    from hares import Frequency

    with pytest.raises(ValueError, match="must divide the hour"):
        Frequency.minutes(0)


def test_simulate_without_a_memory_report_has_no_frame(tmp_path) -> None:
    """A run whose reports write to files only carries the file report's
    result: its table() names the report and its destination (no step
    fallback frame exists)."""
    import pytest
    from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig
    from hares._hares import HaresSimulationError

    target = tmp_path / "run.csv"
    dwelling = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=86400,
            time_res=900,
            metrics=True,
            reports=[
                ReportSpec(
                    "file",
                    Selection.verbosity(2),
                    Destination.csv(str(target)),
                )
            ],
        ),
    )
    dwelling.initialize()
    result = dwelling.simulate()

    assert result.metrics is not None, "the run requested metrics, so it carries them"
    with pytest.raises(HaresSimulationError, match="file"):
        result.reports[0].table()
    assert result.reports[0].path == str(target)


def test_report_surface_objects_hash_by_value() -> None:
    """Every report-surface class that compares by value also hashes by
    value: equal objects hash equal and the objects are dict keys, the
    frozen-dataclass rule a custom ``__eq__`` owes its own ``__hash__``."""
    from hares import Destination, Frequency, Selection

    same_pair = [
        (Selection.verbosity(2), Selection.verbosity(2)),
        (Selection.named(["a", "b"]), Selection.named(["a", "b"])),
        (Selection.pattern("PV*"), Selection.pattern("PV*")),
        (Frequency.timestep(), Frequency.timestep()),
        (Frequency.minutes(15), Frequency.minutes(15)),
        (Frequency.daily(), Frequency.daily()),
        (Destination.memory(), Destination.memory()),
        (Destination.callback(), Destination.callback()),
        (Destination.csv("/tmp/a.csv", overwrite=True),
         Destination.csv("/tmp/a.csv", overwrite=True)),
        (Destination.parquet("/tmp/a.parquet"),
         Destination.parquet("/tmp/a.parquet")),
    ]
    for a, b in same_pair:
        assert a == b, f"{a!r} and {b!r} compare equal"
        assert hash(a) == hash(b), f"equal {type(a).__name__} objects hash equal"
    distinct = {Selection.verbosity(2), Selection.verbosity(2), Selection.verbosity(5),
                Selection.named(["a"]), Selection.pattern("PV*")}
    assert len(distinct) == 4, "a selection sits in a set by its value"
    distinct_freqs = {Frequency.timestep(), Frequency.minutes(15), Frequency.hourly()}
    assert len(distinct_freqs) == 3, "a frequency sits in a set by its value"
    distinct_dests = {Destination.memory(), Destination.callback(),
                      Destination.csv("/tmp/a.csv"), Destination.csv("/tmp/b.csv")}
    assert len(distinct_dests) == 4, "a destination sits in a set by its value"


def _report_surface_objects() -> list:
    """One of every report-surface shape the surface can build."""
    from hares import Destination, Frequency, ReportSpec, Selection

    return [
        Selection.verbosity(2),
        Selection.named(["Total Electric Power (kW)"]),
        Selection.pattern("PV*"),
        Frequency.timestep(),
        Frequency.minutes(15),
        Frequency.hourly(),
        Frequency.daily(),
        Frequency.annual(),
        Destination.memory(),
        Destination.callback(),
        Destination.csv("/tmp/a.csv", overwrite=True),
        Destination.parquet("/tmp/a.parquet"),
        ReportSpec("run", Selection.verbosity(2), Destination.memory(),
                   Frequency.timestep()),
        ReportSpec("file", Selection.named(["a"]), Destination.csv("/tmp/a.csv"),
                   Frequency.minutes(15)),
    ]


def test_report_surface_objects_pickle_round_trip() -> None:
    """Every report-surface pyclass survives pickle: the rebuild callable is
    pickled by reference and the object comes back as the same value, the
    property a spawned pool worker's transport needs."""
    from hares import ReportSpec

    for obj in _report_surface_objects():
        back = pickle.loads(pickle.dumps(obj))
        assert type(back) is type(obj), f"{type(obj).__name__} pickles as its own class"
        if isinstance(obj, ReportSpec):
            assert back.name == obj.name and back.selection == obj.selection
            assert back.destination == obj.destination
            assert back.frequency == obj.frequency
        else:
            assert back == obj, f"{obj!r} comes back as the same value"


def _echo(obj):
    """The spawn worker's job: hand the object back untouched."""
    return obj


def _same_value(back, obj) -> bool:
    """A report-surface object round-tripped to the same value: value types
    compare with their own ``__eq__``; a ReportSpec (identity semantics)
    compares field by field through its getters."""
    from hares import ReportSpec

    if type(back) is not type(obj):
        return False
    if isinstance(obj, ReportSpec):
        return (back.name == obj.name and back.selection == obj.selection
                and back.destination == obj.destination
                and back.frequency == obj.frequency)
    return back == obj


def test_report_surface_objects_survive_a_spawn_pool() -> None:
    """The report-surface objects cross a real spawned worker boundary and
    come back as equal values: the transport the consumer's pool uses."""
    ctx = multiprocessing.get_context("spawn")
    with concurrent.futures.ProcessPoolExecutor(max_workers=1, mp_context=ctx) as pool:
        for obj in _report_surface_objects():
            back = pool.submit(_echo, obj).result(timeout=120)
            assert _same_value(back, obj), (
                f"{type(obj).__name__} did not survive the spawn boundary: "
                f"the parent received {back!r}"
            )
