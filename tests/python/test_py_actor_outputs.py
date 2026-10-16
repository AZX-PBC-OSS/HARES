"""Python actors declare and publish output variables.

A Python actor declares its output variables by returning
``hares.OutputVariable`` objects from ``output_variables()``, and publishes
per-step values with ``self.set_output(name, value)`` during ``decide()``.
The declared variables enter the output catalogue as ``actor:<name>:<var>``
columns; a published key the declarations omit fails the step naming the
actor and the key, and a non-finite published value fails the run like any
actor's telemetry write.
"""

from pathlib import Path

import pyarrow as pa
import pytest

from hares import Actor

ROOT = Path(__file__).resolve().parents[2]

FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")
DEFAULTS = str(ROOT / "python" / "hares" / "defaults")

POWER_COLUMN = "actor:Probe:probe_power_w"
MODE_COLUMN = "actor:Probe:probe_mode"
LEVEL_COLUMN = "actor:Probe:probe_level"


def _day_config(**kw):
    from hares import SimulationConfig

    return SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=86400,
        time_res=900,
        **kw,
    )


def _dwelling(config):
    from hares import Dwelling

    return Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=DEFAULTS,
        bldg_id=1,
        config=config,
    )


def _schema_field_metadata(result, column: str) -> dict[bytes, bytes]:
    """The Arrow field metadata of one report column, read off the capsule
    export (the catalogue's declared unit rides the schema's field
    metadata)."""
    reader = pa.RecordBatchReader.from_stream(result.reports[0].arrow())
    table = reader.read_all()
    metadata = table.schema.field(column).metadata
    assert metadata is not None, f"column {column} carries no field metadata"
    return metadata


def _report_config(**kw):
    """A 24 h at 900 s config with one Memory report. The report rides a
    verbosity selection: the actor columns are appended at every tier, and
    a report's selection resolves against the catalogue at assembly, before
    ``add_actor`` can register a caller actor, so a ``Named`` selection
    naming ``actor:<name>:<var>`` cannot be configured on ``from_hpxml``."""
    from hares import Destination, ReportSpec, Selection

    return _day_config(
        reports=[ReportSpec("frame", Selection.verbosity(0), Destination.memory())],
        **kw,
    )


def test_python_actor_output_variable_appears_with_unit():
    from hares import OutputVariable

    class Probe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            return [
                OutputVariable(
                    "probe_power_w", "W", "power", description="the probe's power"
                )
            ]

        def decide(self, env):
            self.set_output("probe_power_w", 7.5)
            return []

    dw = _dwelling(_report_config())
    dw.add_actor(Probe())
    dw.initialize()
    result = dw.simulate()
    df = result.reports[0].table()

    assert df.height == 96, f"24 h at 900 s is 96 rows, got {df.height}"
    assert POWER_COLUMN in df.columns
    assert (df[POWER_COLUMN] == 7.5).all(), "the published value lands in the column"
    metadata = _schema_field_metadata(result, POWER_COLUMN)
    assert metadata[b"unit"] == b"W", "the catalogue carries the declared unit"


def test_python_actor_enum_output_decodes_labels():
    from hares import OutputVariable

    class ModeProbe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"
            self._code = -1

        def output_variables(self):
            return [
                OutputVariable(
                    "probe_mode",
                    "code",
                    "enum",
                    labels=["off", "low", "high"],
                )
            ]

        def decide(self, env):
            self._code = (self._code + 1) % 3
            self.set_output("probe_mode", float(self._code))
            return []

    dw = _dwelling(_report_config())
    dw.add_actor(ModeProbe())
    dw.initialize()
    result = dw.simulate()
    df = result.reports[0].table()

    values = df[MODE_COLUMN].to_list()
    assert set(values) == {0.0, 1.0, 2.0}, "the published codes are the column's values"
    for a, b in zip(values, values[1:]):
        assert (b - a) % 3 == 1, "the column advances one code per step"


def test_python_actor_quantity_declares_across_homes():
    from hares import OutputVariable

    class LevelProbe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            return [
                OutputVariable(
                    "probe_level",
                    "count",
                    "quantity",
                    aggregation="last",
                    across_homes="mean",
                )
            ]

        def decide(self, env):
            self.set_output("probe_level", 3.0)
            return []

    dw = _dwelling(_report_config())
    dw.add_actor(LevelProbe())
    dw.initialize()
    result = dw.simulate()
    df = result.reports[0].table()

    assert LEVEL_COLUMN in df.columns
    assert (df[LEVEL_COLUMN] == 3.0).all()


def test_python_actor_enum_without_labels_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="labels"):
        OutputVariable("probe_mode", "code", "enum")


def test_python_actor_enum_with_empty_labels_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="empty"):
        OutputVariable("probe_mode", "code", "enum", labels=[])


def test_python_actor_quantity_without_across_homes_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="across_homes"):
        OutputVariable("probe_level", "count", "quantity", aggregation="sum")


def test_python_actor_across_homes_on_a_power_variable_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="across_homes"):
        OutputVariable("probe_power_w", "W", "power", across_homes="sum")


def test_python_actor_quantity_without_aggregation_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="sum") as excinfo:
        OutputVariable("probe_level", "count", "quantity", across_homes="sum")
    assert "mean" in str(excinfo.value) and "last" in str(excinfo.value), (
        "the error names the aggregation choices"
    )


def test_python_actor_unknown_unit_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="unit"):
        OutputVariable("probe_power_w", "furlongs", "power")


def test_python_actor_disallowed_aggregation_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="aggregation"):
        OutputVariable("probe_power_w", "kW", "power", aggregation="sum")


def test_python_actor_unknown_kind_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="kind"):
        OutputVariable("probe_power_w", "kW", "shimmer")


def test_python_actor_unknown_sampling_is_an_error():
    from hares import OutputVariable

    with pytest.raises(ValueError, match="sampling"):
        OutputVariable("probe_power_w", "kW", "power", sampling="whenever")


def test_python_actor_undeclared_output_raises():
    from hares import OutputVariable

    class Probe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            return [OutputVariable("probe_power_w", "W", "power")]

        def decide(self, env):
            return []

    dw = _dwelling(_day_config())
    probe = Probe()
    dw.add_actor(probe)
    with pytest.raises(KeyError, match="not_declared"):
        probe.set_output("not_declared", 1.0)


def test_python_actor_undeclared_published_key_fails_the_step():
    from hares import OutputVariable
    from hares._hares import HaresSimulationError

    class SneakyProbe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            return [OutputVariable("probe_power_w", "W", "power")]

        def decide(self, env):
            # The publication is attempted, the KeyError is caught, and the
            # step still fails: the sealed telemetry latches the undeclared
            # key and the step-end check names the actor and the key.
            try:
                self.set_output("sneaky", 1.0)
            except KeyError:
                pass
            self.set_output("probe_power_w", 7.5)
            return []

    dw = _dwelling(_day_config())
    dw.add_actor(SneakyProbe())
    with pytest.raises(HaresSimulationError) as excinfo:
        dw.initialize()
        dw.step()
    message = str(excinfo.value)
    assert "sneaky" in message, f"the failure names the key: {message}"
    assert "Probe" in message, f"the failure names the actor: {message}"


def test_python_actor_non_finite_output_fails_run():
    from hares import OutputVariable
    from hares._hares import HaresSimulationError

    class NanProbe(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            return [OutputVariable("probe_power_w", "W", "power")]

        def decide(self, env):
            self.set_output("probe_power_w", float("nan"))
            return []

    dw = _dwelling(_day_config())
    dw.add_actor(NanProbe())
    with pytest.raises(HaresSimulationError) as excinfo:
        dw.initialize()
        dw.step()
    message = str(excinfo.value)
    assert "probe_power_w" in message, f"the failure names the key: {message}"
    assert "Probe" in message, f"the failure names the actor: {message}"


def test_python_actor_enum_without_labels_fails_add_actor():
    from hares import OutputVariable

    class BadEnum(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            # Built lazily so the ValueError fires at registration.
            return [OutputVariable("probe_mode", "code", "enum")]

        def decide(self, env):
            return []

    dw = _dwelling(_day_config())
    with pytest.raises(ValueError, match="labels"):
        dw.add_actor(BadEnum())


def test_python_actor_output_variables_wrong_element_type_is_an_error():
    from typing import Any

    class BadType(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            # A deliberate wrong element: the declaration read must reject
            # it with a TypeError at registration.
            bad: list[Any] = [42]
            return bad

        def decide(self, env):
            return []

    dw = _dwelling(_day_config())
    with pytest.raises(TypeError):
        dw.add_actor(BadType())


def test_python_actor_output_variables_exception_propagates():
    class Raising(Actor):
        def __init__(self):
            super().__init__()
            self.name = "Probe"

        def output_variables(self):
            raise RuntimeError("declaration blew up")

        def decide(self, env):
            return []

    dw = _dwelling(_day_config())
    with pytest.raises(RuntimeError, match="declaration blew up"):
        dw.add_actor(Raising())
