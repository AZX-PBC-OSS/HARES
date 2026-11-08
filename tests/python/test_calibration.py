"""The calibration evaluation surface: ``CalibrationDwelling.evaluate``.

The contract under witness:

- one ``evaluate`` call performs the whole override-build-plan-run-return
  transaction: a fresh immutable dwelling blueprint built with the call's
  overrides, ONE observation plan frozen before any run, the requested
  independent periods run to completion, and the observations returned as
  Arrow IPC carrying the field, timestamp, unit, seed, warm-up and
  provenance metadata;
- the same seed and overrides reproduce byte-identical IPC (schema and
  payload); the source dwelling and its state are bitwise unchanged;
- an unknown override or observation field fails with 0 run steps
  simulated; zero periods is refused (empty output is never substituted
  for a run); period bounds, non-finite inputs and callback failures are
  typed, the engine's own exception classes, each carrying the failing
  stage and the run steps simulated as attributes;
- ``warmup_residuals()`` reports exactly the engine's own
  ``RunHealth.warmup`` values, the disabled and the one-day (no
  predecessor) cases included.

Every witness reads the surface's behavior, never its implementation.
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from typing import Any, cast

import polars as pl
import pyarrow as pa
import pyarrow.ipc as pa_ipc
import pytest

from conftest import HARES_DEFAULTS, HPXML, ROOT, SCHEDULE, WEATHER

from hares import CalibrationDwelling, Dwelling, SimulationConfig, WarmupConfig
from hares._hares import (
    HaresCallbackError,
    HaresConfigError,
    HaresEquipmentError,
    HaresSimulationError,
)

BASE_PV = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml")

START = "2019-01-15T10:00:00-07:00"
# The witnesses' observation fields: the net power a PV override moves and
# an outdoor quantity no override moves.
FIELDS = ["total_power_w", "outdoor_temp"]


def cal_config(**kw: Any) -> SimulationConfig:
    """The calibration witnesses' config: the BEopt inputs at 900 s, the
    warm-up disabled unless a test sets it (the warm-up witnesses set the
    criteria explicitly)."""
    return SimulationConfig(
        start_time=START,
        duration=6 * 3600,
        time_res=900,
        warmup=None,
        **kw,
    )


def make_source(
    hpxml: str = HPXML, overrides: dict[str, Any] | None = None
) -> Dwelling:
    """The BEopt source dwelling; ``overrides`` rides its configuration's
    own override map, the base a call's overrides merge over."""
    dwelling = Dwelling.from_hpxml(
        hpxml,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=cal_config(),
        overrides=overrides,
    )
    dwelling.initialize()
    return dwelling


def evaluate(source: Dwelling, **kw: Any):
    """One evaluate call over the source dwelling, the arguments the
    witnesses name in full."""
    cal = CalibrationDwelling.from_dwelling(source)
    return cal.evaluate(
        kw.get("overrides"),
        kw.get("observation_fields", FIELDS),
        kw.get("periods", [(START, 6 * 3600)]),
        kw.get("seed", 42),
    )


def ipc_table(run) -> Any:
    """The run's IPC payload read back the way a calibration consumer
    reads it: the stream's bytes through pyarrow."""
    return pa_ipc.open_stream(run.ipc_bytes()).read_all()


def ipc_frame(run) -> pl.DataFrame:
    """The run's IPC payload as a polars frame, the equality checks'
    form."""
    return pl.DataFrame(cast(Any, ipc_table(run)))


def schema_metadata(run) -> dict[str, str]:
    """The run's IPC schema's metadata, the bytes keys decoded."""
    schema = cast(Any, ipc_table(run)).schema
    return {k.decode(): v.decode() for k, v in schema.metadata.items()}


def failure_of(excinfo) -> Any:
    """The raised exception as the attribute surface it carries: the
    evaluate failures ride ``stage`` and ``steps_simulated`` as
    attributes, set at raise, read here."""
    return cast(Any, excinfo.value)


class TestDeterminism:
    def test_two_evaluations_same_seed_and_overrides_are_byte_identical(self):
        """Two evaluate calls with the same seed and overrides return
        byte-identical IPC: the schema ordering and the payload are a
        function of the inputs, never of the call."""
        source = make_source()
        periods = [(START, 6 * 3600), ("2019-02-15T10:00:00-07:00", 6 * 3600)]
        first = evaluate(source, periods=periods, seed=42)
        second = evaluate(source, periods=periods, seed=42)
        assert first.ipc_bytes() == second.ipc_bytes()
        assert first.rows > 0

    def test_repeated_ipc_bytes_calls_return_identical_bytes(self):
        """Repeated reads of one run's IPC return the same bytes: the
        serialization is the run's, not the call's."""
        run = evaluate(make_source(), seed=42)
        assert run.ipc_bytes() == run.ipc_bytes()

    def test_different_overrides_change_only_the_expected_rows(self):
        """A PV capacity override moves the net power where the sun
        shines and nothing else: the columns (names, types, order), the
        timestamps and the outdoor temperature are unchanged, the field,
        unit and seed metadata are unchanged, and the provenance names
        the merged overrides that rode the run."""
        source = make_source(BASE_PV)
        baseline = evaluate(source, overrides=None, seed=42)
        enlarged = evaluate(
            source,
            overrides={"PV": {"capacity_w": 9000}},
            seed=42,
        )

        base_table = cast(Any, ipc_table(baseline))
        big_table = cast(Any, ipc_table(enlarged))
        assert base_table.column_names == big_table.column_names
        assert [str(base_table.schema.field(name).type) for name in base_table.schema.names] == [
            str(big_table.schema.field(name).type) for name in big_table.schema.names
        ]
        assert base_table.column("timestamp").to_pylist() == big_table.column(
            "timestamp"
        ).to_pylist()
        # The outdoor temperature no override moves: row for row equal.
        assert base_table.column("outdoor_temp").to_pylist() == big_table.column(
            "outdoor_temp"
        ).to_pylist()
        # The net power the capacity drives: at least one sunlit step moves.
        assert base_table.column("total_power_w").to_pylist() != big_table.column(
            "total_power_w"
        ).to_pylist(), "the PV capacity override must move the net power rows"

        # The field, unit and seed metadata are the run-shape contract and
        # do not move; the provenance names the merged overrides.
        base_meta = schema_metadata(baseline)
        big_meta = schema_metadata(enlarged)
        assert base_meta["hares:fields"] == big_meta["hares:fields"]
        assert base_meta["hares:units"] == big_meta["hares:units"]
        assert base_meta["hares:seed"] == big_meta["hares:seed"]
        base_overrides = json.loads(base_meta["hares:provenance"])[0]["settings"][
            "overrides"
        ]
        big_overrides = json.loads(big_meta["hares:provenance"])[0]["settings"][
            "overrides"
        ]
        assert base_overrides is None
        assert big_overrides == {"PV": {"capacity_w": 9000}}


class TestOverrideMerge:
    def test_call_overrides_merge_over_the_configurations_own_per_equipment_key(self):
        """A call's override map merges over the configuration's own per
        equipment key: the caller's entry wins wholesale, the other keys
        survive, and the merged map is what the period's provenance names
        and what the rows are built from (the rows equal a direct
        evaluation with the merged map alone)."""
        source = make_source(BASE_PV, overrides={"PV": {"capacity_w": 5000}})
        merged = evaluate(source, overrides={"PV": {"capacity_w": 9000}}, seed=42)
        direct = evaluate(
            make_source(BASE_PV), overrides={"PV": {"capacity_w": 9000}}, seed=42
        )
        provenance = json.loads(schema_metadata(merged)["hares:provenance"])[0]
        assert provenance["settings"]["overrides"] == {"PV": {"capacity_w": 9000}}
        assert merged.ipc_bytes() == direct.ipc_bytes()

        # A call entry for another equipment key leaves the base entry in
        # place.
        other = evaluate(
            source,
            overrides={"Electric Resistance Water Heater": {"setpoint_c": 60.0}},
            seed=42,
        )
        other_provenance = json.loads(schema_metadata(other)["hares:provenance"])[0]
        assert other_provenance["settings"]["overrides"] == {
            "Electric Resistance Water Heater": {"setpoint_c": 60.0},
            "PV": {"capacity_w": 5000},
        }

        # The caller's entry replaces the base entry wholesale: the base's
        # tilt is not carried into the merged PV entry, so the rows are the
        # direct evaluation's.
        tilted = make_source(
            BASE_PV, overrides={"PV": {"capacity_w": 5000, "tilt_deg": 20.0}}
        )
        replaced = evaluate(tilted, overrides={"PV": {"capacity_w": 9000}}, seed=42)
        replaced_provenance = json.loads(schema_metadata(replaced)["hares:provenance"])[
            0
        ]
        assert replaced_provenance["settings"]["overrides"] == {
            "PV": {"capacity_w": 9000}
        }
        assert replaced.ipc_bytes() == direct.ipc_bytes()


class TestSchema:
    def test_ipc_schema_carries_the_contract_metadata(self):
        """The IPC schema names the surface, the seed, the periods, the
        fields and their units, the warm-up outcomes and the provenance,
        in stable keys; the columns are the period index, the wall-clock
        timestamp and one float64 per field, in the caller's field
        order."""
        run = evaluate(make_source(), seed=7)
        table = cast(Any, ipc_table(run))
        schema = table.schema
        assert schema.names == ["period", "timestamp", *FIELDS]
        types = [str(schema.field(name).type) for name in schema.names]
        assert types == ["uint32", "timestamp[ms]", "double", "double"]

        metadata = {k.decode(): v.decode() for k, v in schema.metadata.items()}
        assert metadata["hares:surface"] == "calibration.evaluate"
        assert metadata["hares:seed"] == "7"
        assert json.loads(metadata["hares:fields"]) == FIELDS
        # The units ride the engine's own unit vocabulary, keyed by field.
        assert json.loads(metadata["hares:units"]) == {
            "total_power_w": "W",
            "outdoor_temp": "C",
        }
        # The period bounds, per period, with the site offset that
        # recovers the wall clock's instant.
        periods = json.loads(metadata["hares:periods"])
        assert len(periods) == 1
        assert periods[0]["duration_s"] == 6 * 3600
        assert periods[0]["steps"] == run.rows
        assert periods[0]["utc_offset_s"] == -7 * 3600
        # The warm-up outcomes and the provenance, per period: this
        # run's warm-up is disabled and says so, honestly.
        assert json.loads(metadata["hares:warmup"]) == ["Disabled"]
        provenance = json.loads(metadata["hares:provenance"])
        assert len(provenance) == 1
        assert provenance[0]["build"]["version"]
        # The merged overrides the run ran under: none here.
        assert provenance[0]["settings"]["overrides"] is None

        # The Arrow stream surface is the same table: the same columns, the
        # same metadata and the same values, exported rather than
        # serialised.
        streamed = cast(Any, pa.RecordBatchReader.from_stream(run.arrow()).read_all())
        assert streamed.column_names == table.column_names
        assert streamed.schema.metadata == table.schema.metadata
        for name in table.column_names:
            assert streamed.column(name).to_pylist() == table.column(name).to_pylist()

    def test_timestamp_column_is_the_wall_clock_at_interval_start(self):
        """The timestamp column is the step's own time, the frame's
        convention: the local wall clock at the step's interval start,
        milliseconds; the site offset in the periods' metadata recovers
        the instant."""
        stamps = cast(Any, ipc_table(evaluate(make_source(), seed=42))).column(
            "timestamp"
        ).to_pylist()
        # 10:00 local wall clock, the start's hour at the site's offset.
        assert stamps[0] == datetime(2019, 1, 15, 10, 0, 0)
        assert stamps[1] == datetime(2019, 1, 15, 10, 15, 0)

    def test_period_start_metadata_is_the_local_start_at_the_site_offset(self):
        """A period's recorded start is the run's own local start at the
        site's offset, not the caller's spelling: a period named in UTC
        runs on the site's wall clock, and the recorded start read with
        the recorded offset is the first row's instant, so the metadata's
        start and the table's rows agree."""
        start = "2019-02-15T10:00:00Z"
        source = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            defaults=str(HARES_DEFAULTS),
            bldg_id=1,
            config=SimulationConfig(
                start_time=start, duration=6 * 3600, time_res=900, warmup=None
            ),
        )
        source.initialize()
        run = evaluate(source, periods=[(start, 6 * 3600)], seed=42)
        (period,) = json.loads(schema_metadata(run)["hares:periods"])
        recorded = datetime.fromisoformat(period["start"])
        site = timezone(timedelta(seconds=period["utc_offset_s"]))
        first = cast(Any, ipc_table(run)).column("timestamp")[0].as_py()
        # The site's offset, not the caller's: Denver is UTC-7 in February.
        assert recorded.utcoffset() == timedelta(hours=-7)
        # The recorded wall clock and the row's wall clock are one value,
        # and the recorded instant is that wall clock at the site offset.
        assert recorded.replace(tzinfo=None) == first
        assert recorded == first.replace(tzinfo=site)


class TestSourceUnchanged:
    def test_source_dwelling_state_is_bitwise_unchanged(self):
        """A successful evaluate leaves the source dwelling bitwise
        unchanged: the episode snapshot's bytes and the health totals are
        equal before and after, and a failed evaluate leaves them equal
        too."""
        source = make_source()
        state_before = source.snapshot_state()
        health_before = source.health()

        evaluate(source, seed=42)
        assert source.snapshot_state() == state_before
        assert source.health() == health_before

        # A failing evaluation (an unknown field) changes nothing either.
        with pytest.raises(HaresSimulationError):
            evaluate(source, observation_fields=["outdoor_temp", "no_such_field"])
        assert source.snapshot_state() == state_before
        assert source.health() == health_before

    def test_source_dwelling_still_steps_identically_after_an_evaluate(self):
        """The source dwelling's next step after an evaluate equals the
        same dwelling's next step without one: the evaluation consumed
        nothing of the source's state."""
        stepped = make_source()
        untouched = make_source()
        evaluate(stepped, seed=42)
        assert stepped.step() == untouched.step()


class TestTypedFailures:
    def test_unknown_observation_field_fails_with_zero_steps(self):
        """An unknown observation field fails at the plan's freeze,
        before any run step: the typed error names the field and reports
        0 run steps simulated."""
        with pytest.raises(HaresSimulationError, match="no_such_field") as excinfo:
            evaluate(
                make_source(),
                observation_fields=["outdoor_temp", "no_such_field"],
            )
        failure = failure_of(excinfo)
        assert failure.steps_simulated == 0
        assert failure.stage == "observation plan"
        assert "0 run steps simulated" in str(failure)

    def test_unknown_override_field_fails_with_zero_steps(self):
        """An unknown override field fails in the fresh blueprint's
        resolution, before any run step: the typed error names the
        equipment and the field and reports 0 run steps simulated."""
        with pytest.raises(HaresEquipmentError, match="totally_unknown") as excinfo:
            evaluate(
                make_source(BASE_PV),
                overrides={"PV": {"totally_unknown": 1.0}},
            )
        failure = failure_of(excinfo)
        assert failure.steps_simulated == 0
        assert "0 run steps simulated" in str(failure)

    def test_zero_periods_is_refused(self):
        """An evaluate with zero periods is refused outright: no run
        object, no empty table standing in for one."""
        with pytest.raises(HaresConfigError, match="zero periods") as excinfo:
            evaluate(make_source(), periods=[])
        assert failure_of(excinfo).steps_simulated == 0

    def test_period_bounds_are_typed(self):
        """A zero duration, a duration that does not divide into whole
        steps, and an unparseable start each fail with a typed error
        naming the period, before any run step."""
        for periods, match in [
            ([(START, 0)], "duration"),
            ([(START, 60 + 1)], "evenly divisible"),
            ([("not-a-time", 3600)], "not-a-time"),
        ]:
            with pytest.raises(HaresConfigError, match=match) as excinfo:
                evaluate(make_source(), periods=periods)
            assert failure_of(excinfo).steps_simulated == 0

    def test_non_finite_override_value_is_typed(self):
        """A non-finite override value fails at the boundary naming the
        value: JSON cannot carry it and a silent default would run a
        differently-configured dwelling."""
        with pytest.raises(ValueError, match="non-finite") as excinfo:
            evaluate(
                make_source(BASE_PV),
                overrides={"PV": {"capacity_w": float("nan")}},
            )
        assert failure_of(excinfo).steps_simulated == 0

    def test_callback_failure_is_typed_with_cause_chained(self):
        """A batch callback's exception fails the evaluation as the typed
        callback error, the raised object chained as ``__cause__``, and
        the failure names the run steps the evaluation completed before
        it. The report flushes every 2 rows, so the first delivery fires
        inside the second step, one completed step behind it."""
        from hares import Destination, ReportSpec, Selection

        config = SimulationConfig(
            start_time=START,
            duration=6 * 3600,
            time_res=900,
            warmup=None,
            reports=[ReportSpec("frame", Selection.verbosity(0), Destination.callback())],
            output_chunk_size=2,
        )
        dwelling = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            defaults=str(HARES_DEFAULTS),
            bldg_id=1,
            config=config,
        )
        dwelling.initialize()
        cal = CalibrationDwelling.from_dwelling(dwelling)

        raised = RuntimeError("the store is down")

        def fails(info: Any, batch: Any) -> None:
            raise raised

        cal.attach_callback("frame", fails)
        with pytest.raises(HaresCallbackError) as excinfo:
            cal.evaluate(None, FIELDS, [(START, 6 * 3600)], 42)
        message = str(excinfo.value)
        assert "the store is down" in message
        assert "run steps simulated" in message
        assert excinfo.value.__cause__ is raised


class TestWarmupResiduals:
    @staticmethod
    def cz4a_source(warmup: Any) -> Dwelling:
        """The cz4a fixture at 3600 s for 2 days, seed 7, the warm-up
        criteria the caller sets: the same inputs the plain comparison
        dwelling builds from."""
        cz4a = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
        source = Dwelling.from_hpxml(
            str(cz4a / "building.xml"),
            str(cz4a / "schedule.csv"),
            str(cz4a / "weather.epw"),
            defaults=str(HARES_DEFAULTS),
            bldg_id=1,
            config=SimulationConfig(
                start_time="2023-01-01T00:00:00-07:00",
                duration=2 * 86_400,
                time_res=3600,
                master_seed=7,
                warmup=warmup,
            ),
        )
        source.initialize()
        return source

    def test_warmup_residuals_equal_the_engine_health_exactly(self):
        """The run's warm-up outcome is the engine's own
        ``RunHealth.warmup`` value for the same inputs, seed and period:
        the converged and the unconverged cases compare equal, field for
        field, residuals to the last bit."""
        for warmup, converged in [
            (WarmupConfig(), True),
            (WarmupConfig(temperature_tolerance_c=1e-9, max_days=2), False),
        ]:
            source = self.cz4a_source(warmup)
            run = evaluate(
                source,
                periods=[("2023-01-01T00:00:00-07:00", 2 * 86_400)],
                seed=7,
            )
            outcomes = run.warmup_residuals()
            assert len(outcomes) == 1
            engine = source.health().warmup
            assert outcomes[0] == engine
            # The same run's own health, not a second run's: the report is
            # the finished period's `RunHealth.warmup`.
            assert outcomes[0] == run.results[0].health.warmup
            assert engine.outcome == "ran"
            assert engine.converged is converged
            if not converged:
                assert engine.days_run == 2

    def test_warmup_disabled_reports_disabled(self):
        """A warm-up-disabled evaluation reports ``disabled``: no warm-up
        day ran, no residuals exist, and the engine's own health agrees."""
        source = self.cz4a_source(None)
        run = evaluate(
            source,
            periods=[("2023-01-01T00:00:00-07:00", 2 * 86_400)],
            seed=7,
        )
        (outcome,) = run.warmup_residuals()
        assert outcome.outcome == "disabled"
        assert outcome.days_run is None
        assert outcome.converged is None
        assert outcome.residuals is None
        assert outcome == source.health().warmup

    def test_one_day_warmup_reports_no_residuals(self):
        """A one-day warm-up ran one day with no predecessor to diff
        against: the outcome is ``ran`` with residuals ``None``, the
        engine's own honest shape."""
        source = self.cz4a_source(WarmupConfig(min_days=1, max_days=1))
        run = evaluate(
            source,
            periods=[("2023-01-01T00:00:00-07:00", 2 * 86_400)],
            seed=7,
        )
        (outcome,) = run.warmup_residuals()
        assert outcome.outcome == "ran"
        assert outcome.days_run == 1
        assert outcome.residuals is None
        assert outcome == source.health().warmup

    def test_ipc_metadata_carries_the_warmup_outcomes(self):
        """The IPC's warm-up metadata is the engine's serialized
        outcomes, one entry per period: the disabled case reads
        ``["Disabled"]``."""
        source = self.cz4a_source(None)
        run = evaluate(
            source,
            periods=[("2023-01-01T00:00:00-07:00", 2 * 86_400)],
            seed=7,
        )
        assert json.loads(schema_metadata(run)["hares:warmup"]) == ["Disabled"]

    def test_each_period_reports_its_own_warmup_outcome(self):
        """A two-period run reports each period's own engine health, in
        period order: the periods start in different weather, so their
        warm-up residuals differ, and outcome i equals
        ``results[i].health.warmup`` for every i. A surface returning one
        period's outcome for all of them fails here."""
        source = self.cz4a_source(
            WarmupConfig(temperature_tolerance_c=1e-9, max_days=2)
        )
        run = evaluate(
            source,
            periods=[
                ("2023-01-01T00:00:00-07:00", 2 * 86_400),
                ("2023-01-02T12:00:00-07:00", 2 * 86_400),
            ],
            seed=7,
        )
        outcomes = run.warmup_residuals()
        assert len(outcomes) == 2
        assert [outcome == result.health.warmup for outcome, result in zip(
            outcomes, run.results
        )] == [True, True]
        # The two starts converge over different weather: the outcomes are
        # not one value repeated.
        assert outcomes[0] != outcomes[1]


class TestPeriods:
    def test_periods_run_independently_and_in_period_order(self):
        """The periods' rows appear in period order, each period's
        timestamps restart at its own start, and EVERY period's rows are
        the same rows that period runs alone: the periods are independent
        runs, not one run sliced or a later period continuing an earlier
        one's state."""
        source = make_source()
        first_start = "2019-01-15T10:00:00-07:00"
        second_start = "2019-02-15T10:00:00-07:00"
        both = evaluate(
            source,
            periods=[(first_start, 6 * 3600), (second_start, 6 * 3600)],
            seed=42,
        )
        solo_first = evaluate(source, periods=[(first_start, 6 * 3600)], seed=42)
        solo_second = evaluate(source, periods=[(second_start, 6 * 3600)], seed=42)

        frame = ipc_frame(both)
        n_first = frame.filter(pl.col("period") == 0).height
        assert n_first > 0
        assert frame["period"].to_list() == [0] * n_first + [1] * (
            frame.height - n_first
        )
        # Each period's timestamps restart at its own start.
        stamps = frame["timestamp"].to_list()
        assert stamps[0] == datetime(2019, 1, 15, 10, 0, 0)
        assert stamps[n_first] == datetime(2019, 2, 15, 10, 0, 0)
        # Every period's rows are the solo run's rows, bitwise: the first
        # period proves the later periods' presence moves nothing, and the
        # last proves no earlier period's state carried into it.
        assert (
            frame.filter(pl.col("period") == 0)
            .drop("period")
            .equals(ipc_frame(solo_first).drop("period"))
        ), "the first period's rows must not depend on the later periods"
        assert (
            frame.filter(pl.col("period") == 1)
            .drop("period")
            .equals(ipc_frame(solo_second).drop("period"))
        ), "a later period's rows must not depend on the earlier periods"

    def test_period_spanning_a_skipped_feb_29_runs_the_clocks_steps(self):
        """A period whose 48-hour span contains a Feb 29 the typical
        weather year does not carry runs the engine's own simulated step
        count, not the span's raw division: the calendar steps less the
        skipped day's, exactly the steps the same configuration's ordinary
        run reports. The evaluation must reproduce that run, not step past
        its configured end."""
        start = "2020-02-28T00:00:00-07:00"
        source = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            defaults=str(HARES_DEFAULTS),
            bldg_id=1,
            config=SimulationConfig(
                start_time=start, duration=2 * 86_400, time_res=3600, warmup=None
            ),
        )
        source.initialize()
        # The engine's own clock over this span: Feb 28 alone, 24 hourly
        # steps (the typical-year rule skips Feb 29).
        reference = list(source.timesteps())
        assert len(reference) == 24

        run = evaluate(source, periods=[(start, 2 * 86_400)], seed=42)
        stamps = cast(Any, ipc_table(run)).column("timestamp").to_pylist()
        assert stamps == reference
        assert run.rows == 24
        assert run.steps_simulated == 24
        assert json.loads(schema_metadata(run)["hares:periods"])[0]["steps"] == 24


class TestSurfaceShape:
    def test_run_reports_the_contract_totals(self):
        """The run names its fields, seed, period count, row count and
        the run steps it simulated, and the totals agree: the rows are
        the periods' steps, one observation per step."""
        run = evaluate(make_source(), seed=42, periods=[(START, 6 * 3600)])
        assert run.fields == FIELDS
        assert run.seed == 42
        assert run.n_periods == 1
        assert run.rows == 24
        assert run.steps_simulated == run.rows
