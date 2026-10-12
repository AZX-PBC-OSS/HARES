"""The batch callback in Python: a callable attached to a dwelling receives
every flushed batch of the run's report, in order, as the zero-copy Arrow
batch, an exception from the callable fails the run as the typed callback
error with the raised object chained as ``__cause__``, and the callable is
never called for a report it was not attached to."""

import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, cast

import polars as pl
import pytest
from conftest import make_dwelling
from hares import Destination, ReportSpec, Selection, SimulationConfig
from hares._hares import HaresCallbackError, HaresSimulationError

REPORT = "timestep"


class Collector:
    """Records the batches and contexts it receives; fails one batch on demand."""

    def __init__(self, fail_on=None, exception=None):
        self.info = []
        self.batches = []
        self.fail_on = fail_on
        self.exception = exception if exception is not None else RuntimeError("the store is down")

    def __call__(self, info, batch):
        if self.fail_on == info.batch_index:
            raise self.exception
        self.info.append((info.report, info.bldg_id, info.batch_index))
        self.batches.append(batch)


def callback_config(output_path, destinations, output_chunk_size=40):
    """A 24 h run at 900 s over the committed BEopt inputs.

    ``destinations`` maps report name to destination, so a test declares
    exactly the reports it attaches to and reads."""
    return SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=24 * 3600,
        time_res=900,
        reports=[
            ReportSpec(name, Selection.verbosity(2), destination)
            for name, destination in destinations.items()
        ],
        output_chunk_size=output_chunk_size,
    )


def build_dwelling(collector, tmp_path, destinations=None, attach_to=REPORT):
    if destinations is None:
        destinations = {REPORT: Destination.memory()}
    dw = make_dwelling(
        sim_config=callback_config(str(tmp_path / "run.parquet"), destinations)
    )
    dw.attach_callback(attach_to, collector)
    return dw


def reference_frame(tmp_path, report=REPORT):
    dw = make_dwelling(
        sim_config=callback_config(
            str(tmp_path / "reference.parquet"), {report: Destination.memory()}
        )
    )
    result = dw.simulate()
    return result.reports[0].table()


def test_callback_receives_every_batch_in_order(tmp_path):
    expected = reference_frame(tmp_path)
    collector = Collector()
    dw = build_dwelling(collector, tmp_path)
    dw.simulate()

    assert collector.info == [
        (REPORT, 42, 0),
        (REPORT, 42, 1),
        (REPORT, 42, 2),
    ]
    frames = [pl.DataFrame(batch) for batch in collector.batches]
    assert [frame.height for frame in frames] == [40, 40, 16]
    for batch in collector.batches:
        assert hasattr(batch, "__arrow_c_array__"), (
            "the batch must export the Arrow PyCapsule array interface"
        )

    concatenated = pl.concat(frames)
    assert concatenated.equals(expected)


def test_callback_exception_fails_run_with_cause_chained(tmp_path):
    raised = RuntimeError("the store is down")
    collector = Collector(fail_on=1, exception=raised)
    dw = build_dwelling(collector, tmp_path)
    with pytest.raises(HaresCallbackError) as excinfo:
        dw.simulate()
    assert isinstance(excinfo.value, HaresSimulationError)
    message = str(excinfo.value)
    assert REPORT in message
    assert "the store is down" in message
    assert excinfo.value.__cause__ is raised, (
        "the raised object must be chained as __cause__"
    )

    # The budget semantics: the run stays ended and the callback is not
    # called again. The engine's latch re-raises the ended run as the
    # simulation error carrying the latched message.
    calls = len(collector.batches)
    with pytest.raises(HaresSimulationError) as later:
        dw.step()
    assert "the store is down" in str(later.value)
    assert len(collector.batches) == calls


def test_first_attached_callback_failure_chains_its_cause(tmp_path):
    """Two reports each with an attached callback: the first-attached
    callback's failure chains its own exception as ``__cause__``. The
    adapters share the dwelling's one cause slot, so a later attachment
    cannot take the first one's slot away."""
    raised = RuntimeError("the store is down")
    failing = Collector(fail_on=0, exception=raised)
    quiet = Collector()
    dw = make_dwelling(
        sim_config=callback_config(
            str(tmp_path / "run.parquet"),
            {REPORT: Destination.memory(), "extra": Destination.memory()},
        )
    )
    dw.attach_callback(REPORT, failing)
    dw.attach_callback("extra", quiet)
    with pytest.raises(HaresCallbackError) as excinfo:
        dw.simulate()
    assert REPORT in str(excinfo.value)
    assert excinfo.value.__cause__ is raised, (
        "the first-attached callback's raised object must be chained"
    )


def test_latched_finish_reraise_chains_the_cause(tmp_path):
    """The latched run's finish() re-raises the callback error: the
    re-raise carries the original exception as ``__cause__``, the same
    one the failing run's error chained."""
    raised = RuntimeError("the store is down")
    collector = Collector(fail_on=1, exception=raised)
    dw = build_dwelling(collector, tmp_path)
    with pytest.raises(HaresCallbackError) as failing:
        dw.simulate()
    assert failing.value.__cause__ is raised
    with pytest.raises(HaresCallbackError) as reraised:
        dw.finish()
    assert reraised.value.__cause__ is raised, (
        "the latched re-raise must chain the original exception"
    )


def test_callback_not_called_for_other_reports(tmp_path):
    collector = Collector()
    dw = build_dwelling(
        collector,
        tmp_path,
        destinations={REPORT: Destination.memory(), "extra": Destination.memory()},
        attach_to=REPORT,
    )
    dw.simulate()

    assert collector.info, "the attached callable must run"
    assert {info[0] for info in collector.info} == {REPORT}, (
        "the callable is called only for the report it was attached to"
    )


def test_callback_info_carries_bldg_id(tmp_path):
    collector = Collector()
    dw = build_dwelling(collector, tmp_path)
    dw.simulate()

    assert collector.info == [
        (REPORT, 42, 0),
        (REPORT, 42, 1),
        (REPORT, 42, 2),
    ]


def test_callback_destination_marker_delivers(tmp_path):
    """Destination.callback() declares the OR-14 marker: the report's
    batches go to the attached callable and nothing is retained."""
    collector = Collector()
    dw = make_dwelling(
        sim_config=callback_config(
            str(tmp_path / "run.parquet"), {REPORT: Destination.callback()}
        )
    )
    dw.attach_callback(REPORT, collector)
    result = dw.simulate()

    assert [info[2] for info in collector.info] == [0, 1, 2]
    assert [pl.DataFrame(batch).height for batch in collector.batches] == [40, 40, 16]
    assert repr(Destination.callback()) == "Destination.callback()"
    with pytest.raises(HaresSimulationError) as excinfo:
        result.reports[0].table()
    assert REPORT in str(excinfo.value)


def test_construction_does_not_surface_the_callback(tmp_path):
    """Construction runs with the GIL released: the registered callback
    belongs to the dwelling it was attached to and must not fire during a
    construction that runs underneath it. reset_with_seed rebuilds the
    dwelling (the one construction a built dwelling can run); the callback
    surfaces only at the batches of a run's steps, never during it."""
    collector = Collector()
    dw = build_dwelling(collector, tmp_path)

    # Construction under release: the reset rebuilds the dwelling with the
    # GIL released while the collector stays registered on the object.
    dw.reset_with_seed(7)
    assert collector.info == []
    assert collector.batches == []

    # The rebuilt dwelling carries no callback (the reset replaced it);
    # after a fresh attach, the callback surfaces at the run's batches,
    # never during construction.
    dw.attach_callback(REPORT, collector)
    dw.simulate()
    assert [info[2] for info in collector.info] == [0, 1, 2]


def test_attach_requires_a_callable(tmp_path):
    dw = make_dwelling(
        sim_config=callback_config(
            str(tmp_path / "run.parquet"), {REPORT: Destination.memory()}
        )
    )
    with pytest.raises(TypeError):
        dw.attach_callback(REPORT, cast(Any, 42))


def test_unknown_report_name_is_rejected(tmp_path):
    dw = make_dwelling(
        sim_config=callback_config(
            str(tmp_path / "run.parquet"), {REPORT: Destination.memory()}
        )
    )

    def callback(info, batch):
        raise AssertionError("never called")

    with pytest.raises(HaresSimulationError) as excinfo:
        dw.attach_callback("hourly", callback)
    assert REPORT in str(excinfo.value)


def _reentry_probe() -> int:
    """The re-entry pin's child body: attach a callback that calls back
    into the dwelling and require the typed refusal, never a hang."""
    from hares import Dwelling, SimulationConfig

    root = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory() as tmp:
        dw = Dwelling.from_hpxml(
            str(root / "tests/fixtures/hpxml/ochre_samples/base.xml"),
            str(root / "data/examples/BEopt_example_schedule.csv"),
            str(root / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            config=SimulationConfig(
                start_time="2019-01-01T00:00:00",
                duration=24 * 3600,
                time_res=900,
                reports=[
                    ReportSpec(
                        REPORT,
                        Selection.verbosity(2),
                        Destination.parquet(str(Path(tmp) / "run.parquet")),
                    )
                ],
                output_chunk_size=40,
            ),
            defaults=str(root / "defaults"),
            bldg_id=42,
        )
        dw.initialize()

        def reenter(info: object, batch: object) -> None:
            dw.health()

        dw.attach_callback(REPORT, reenter)
        try:
            dw.simulate()
        except HaresSimulationError as exc:
            assert "a callback may not re-enter the dwelling" in str(exc), str(exc)
        else:
            raise AssertionError("the re-entrant callback must fail the run")

        # The budget semantics: the run stays ended, the error carried
        # through.
        try:
            dw.step()
        except HaresSimulationError as exc:
            assert "a callback may not re-enter the dwelling" in str(exc), str(exc)
        else:
            raise AssertionError("the run stays ended after the refusal")
    return 0


def test_callback_may_not_reenter_the_dwelling():
    """A callback calling back into the dwelling (results, metrics, step,
    apply_control) is refused as a typed error that fails the run, never a
    deadlock: the delivery thread holds the dwelling's mutex and the GIL
    across the call, so a re-entrant accessor parks forever and starves
    every other thread, which no in-process check could report. The probe
    therefore runs in a child process joined on a timeout, so a regression
    fails fast instead of hanging the suite."""
    probe = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), "--reentry-probe"],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert probe.returncode == 0, (
        f"the re-entry probe failed (returncode {probe.returncode}):\n{probe.stderr}"
    )


if __name__ == "__main__":
    if "--reentry-probe" in sys.argv:
        raise SystemExit(_reentry_probe())
