"""The variable catalogue and metrics_from_table in Python.

The Python Catalogue surface over the Rust catalogue
(Dwelling.catalogue()): every variable's name, unit, kind, aggregation,
sampling, nullability, source, labels, across-homes rule and description
readable, the digest, the iteration, and hares.metrics_from_table over
any __arrow_c_stream__ table plus the catalogue. The RED tests here are
written against that surface before it exists.
"""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")

TIME_COLUMN = "Time"


def result_config(**extra: Any):
    """The acceptance fixture's config: the cz4a inputs, start
    2023-01-01T00:00:00-07:00, 24 h at 900 s, one Verbosity(8) Memory
    report, the metrics request on."""
    from hares import Destination, ReportSpec, Selection, SimulationConfig

    return SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=86400,
        time_res=900,
        output_chunk_size=40,
        metrics=True,
        reports=[ReportSpec("frame", Selection.verbosity(8), Destination.memory())],
        **extra,
    )


def build(config: Any, **kw: Any):
    from hares import Dwelling

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
        config=config,
        **kw,
    )
    dw.initialize()
    return dw


def dwelling_with_battery():
    """The acceptance dwelling with a battery added: the battery's
    declarations join the catalogue on the roster change."""
    from hares import Battery

    dw = build(result_config())
    dw.add_battery(Battery("Battery", 13.5, max_charge_kw=5.0, max_discharge_kw=5.0))
    return dw


@pytest.fixture(scope="module")
def result():
    """One run over the acceptance fixture: the RunResult the catalogue
    and metrics tests read."""
    return build(result_config()).simulate()


def test_catalogue_unit_lookup_works_after_polars_import(result) -> None:
    """Every value column of a Verbosity(8) table resolves to a Variable
    with a unit read from the catalogue, not from the frame: polars drops
    Arrow field metadata on import, and the lookup works anyway. (The
    Time column is the row's index, not a catalogue variable.)"""
    import polars  # noqa: F401  (the import that drops the metadata)

    catalogue = result.catalogue
    frame = result.reports[0].table()
    assert len(catalogue) > 0
    assert [v.name for v in catalogue] == [v.name for v in catalogue.variables()]
    for column in frame.columns:
        if column == TIME_COLUMN:
            continue
        variable = catalogue[column]
        assert isinstance(variable.unit, str), column
    assert catalogue["Total Electric Power (kW)"].unit == "kW"
    assert catalogue["ASHP Heater Supply Temperature (C)"].unit == "C"
    with pytest.raises(KeyError):
        catalogue["Not A Column"]


def test_catalogue_json_round_trips(result) -> None:
    """to_json/from_json round trip: the rebuilt catalogue's variables,
    digest and equality are the originals'."""
    from hares import Catalogue

    catalogue = result.catalogue
    rebuilt = Catalogue.from_json(catalogue.to_json())
    assert rebuilt == catalogue
    assert rebuilt.digest() == catalogue.digest()
    assert [v.name for v in rebuilt] == [v.name for v in catalogue]


def test_enum_codes_decode_to_labels(result) -> None:
    """An enum variable's labels decode the column's codes: every code in
    the frame is an index into the variable's label list."""
    catalogue = result.catalogue
    variable = catalogue["ASHP Heater Mode (-)"]
    assert variable.kind == "enum"
    labels = variable.labels
    assert labels is not None
    assert len(labels) > 0
    assert all(isinstance(label, str) for label in labels)

    frame = result.reports[0].table()
    codes = frame["ASHP Heater Mode (-)"].unique().drop_nulls().to_list()
    assert codes, "the mode column carries codes over the day"
    for code in codes:
        assert 0 <= int(code) < len(labels), f"code {code} outside the label table"
        assert isinstance(labels[int(code)], str)


def test_catalogue_available_before_first_step() -> None:
    """Dwelling.catalogue() on a dwelling built without warm-up and not
    yet stepped equals RunResult.catalogue of the same dwelling after one
    step: the catalogue is readable from build on and does not move."""
    catalogue_before = build(result_config()).catalogue()
    assert len(catalogue_before.variables()) > 0
    assert isinstance(catalogue_before.digest(), str) and catalogue_before.digest()

    dw = build(result_config())
    catalogue_at_build = dw.catalogue()
    dw.step()
    result = dw.finish()

    assert result.catalogue == catalogue_at_build
    assert result.catalogue.digest() == catalogue_before.digest()


def test_variable_descriptions_come_from_declarations() -> None:
    """Every catalogue Variable carries its declaring site's description,
    the battery's declarations included. (The entry named the battery's
    DECLARED_CAPACITY_KWH variable: that telemetry key is scope internal
    and never becomes a catalogue column, so the pin runs on the
    battery's declared variables and on the declaration itself, through
    the battery descriptor's telemetry fields.)"""
    dw = dwelling_with_battery()
    catalogue = dw.catalogue()
    for variable in catalogue.variables():
        assert isinstance(variable.description, str) and variable.description, variable.name

    assert catalogue["Battery SOC (-)"].description == "State of charge"

    battery = next(
        descriptor
        for descriptor in dw.equipment_descriptors()
        if descriptor.name == "Battery"
    )
    declared = {field.name: field.description for field in battery.telemetry_fields}
    assert declared["declared_capacity_kwh"] == "Declared pack capacity from config"


def test_quantity_variables_carry_across_homes(result) -> None:
    """Every quantity Variable declares its across-homes rule (sum or
    mean); every other kind carries None."""
    for variable in result.catalogue.variables():
        if variable.kind == "quantity":
            assert variable.across_homes in ("sum", "mean"), variable.name
        else:
            assert variable.across_homes is None, variable.name


def test_metrics_from_table_matches_run_metrics_in_python(result) -> None:
    """metrics_from_table over the report's arrow() table and the run's
    catalogue equals the run's own metrics field for field."""
    from hares import metrics_from_table

    table = result.reports[0].arrow()
    from_table = metrics_from_table(table, result.catalogue, 900)
    assert from_table == result.metrics

    # The timedelta spelling of the step resolution computes the same
    # metrics.
    again = metrics_from_table(
        result.reports[0].arrow(), result.catalogue, timedelta(seconds=900)
    )
    assert again == result.metrics


def test_metrics_from_table_rejects_a_reduced_table(result) -> None:
    """An Hourly-reduced table passed with the step resolution is an
    error naming the mismatch, not wrong metrics."""
    import polars as pl

    from hares import metrics_from_table

    frame = result.reports[0].table()
    reduced = frame.group_by_dynamic(
        TIME_COLUMN, every="1h"
    ).agg([pl.col(c).mean() for c in frame.columns if c != TIME_COLUMN])
    with pytest.raises(Exception, match="Timestep"):
        metrics_from_table(reduced, result.catalogue, 900)


def test_metrics_from_table_rejects_a_non_stream_table(result) -> None:
    """A table without __arrow_c_stream__ is a TypeError naming the
    type."""
    from hares import metrics_from_table

    not_a_table: Any = "not a table"
    with pytest.raises(TypeError, match="__arrow_c_stream__"):
        metrics_from_table(not_a_table, result.catalogue, 900)
