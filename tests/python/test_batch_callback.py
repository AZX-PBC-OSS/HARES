"""The batch callback in Python: a callable attached to a dwelling receives
every flushed batch of the run's report, in order, and an exception from
the callable fails the run as a typed error instead of a panic."""

import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, cast

import polars as pl
import pytest
from conftest import make_dwelling
from hares import SimulationConfig
from hares._hares import HaresSimulationError

REPORT = "timestep"


class Collector:
    """Records the batches and contexts it receives; fails one batch on demand."""

    def __init__(self, fail_on=None):
        self.info = []
        self.batches = []
        self.fail_on = fail_on

    def __call__(self, info, batch):
        if self.fail_on == info.batch_index:
            raise RuntimeError("the store is down")
        self.info.append((info.report, info.bldg_id, info.batch_index))
        self.batches.append(batch)


def callback_config(output_path, retain_batches):
    return SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=24 * 3600,
        time_res=900,
        output_verbosity=2,
        write_output=True,
        output_path=output_path,
        retain_batches=retain_batches,
        output_chunk_size=40,
    )


def build_dwelling(collector, tmp_path):
    dw = make_dwelling(
        sim_config=callback_config(str(tmp_path / "run.parquet"), retain_batches=False)
    )
    dw.attach_callback(REPORT, collector)
    return dw


def reference_frame(tmp_path):
    dw = make_dwelling(
        sim_config=callback_config(str(tmp_path / "reference.parquet"), retain_batches=True)
    )
    dw.simulate()
    return dw.results()


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
    assert [batch.height for batch in collector.batches] == [40, 40, 16]

    concatenated = pl.concat(collector.batches)
    assert concatenated.equals(expected)


def test_callback_exception_fails_the_run(tmp_path):
    collector = Collector(fail_on=1)
    dw = build_dwelling(collector, tmp_path)
    with pytest.raises(HaresSimulationError) as excinfo:
        dw.simulate()
    message = str(excinfo.value)
    assert REPORT in message
    assert "the store is down" in message

    # The budget semantics: the run stays ended and the callback is not
    # called again.
    calls = len(collector.batches)
    with pytest.raises(HaresSimulationError) as later:
        dw.step()
    assert "the store is down" in str(later.value)
    assert len(collector.batches) == calls


def test_attach_requires_a_callable(tmp_path):
    dw = make_dwelling(write_output=True, output_path=str(tmp_path / "run.parquet"))
    with pytest.raises(TypeError):
        dw.attach_callback(REPORT, cast(Any, 42))


def test_unknown_report_name_is_rejected(tmp_path):
    dw = make_dwelling(write_output=True, output_path=str(tmp_path / "run.parquet"))

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
                output_verbosity=2,
                write_output=True,
                output_path=str(Path(tmp) / "run.parquet"),
                retain_batches=False,
                output_chunk_size=40,
            ),
            defaults_path=str(root / "defaults"),
            bldg_id=42,
        )
        dw.initialize()

        def reenter(info: object, batch: object) -> None:
            dw.results()

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
