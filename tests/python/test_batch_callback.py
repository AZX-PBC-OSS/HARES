"""The batch callback in Python: a callable attached to a dwelling receives
every flushed batch of the run's report, in order, and an exception from
the callable fails the run as a typed error instead of a panic."""

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
