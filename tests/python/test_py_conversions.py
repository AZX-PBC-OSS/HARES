"""Tests for Arrow to Polars zero-copy conversion."""

from pathlib import Path

import polars as pl
import pytest
from hares import Destination, Dwelling, DwellingConfig, Fleet, ReportSpec, Selection, SimulationConfig

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"
HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")


@pytest.fixture(scope="session")
def dwelling():
    """Create a Dwelling from the committed BEopt example inputs, with the
    memory report the frame-reading tests read."""
    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        config=SimulationConfig(
            start_time="2019-01-01T00:00:00",
            duration=86400,
            reports=[ReportSpec("frame", Selection.verbosity(0), Destination.memory())],
        ),
    )
    dw.initialize()
    return dw


@pytest.fixture(scope="session")
def simulated_df(dwelling):
    """Run simulate() once and share the resulting DataFrame across tests."""
    result = dwelling.simulate()
    return result.reports[0].table()


# One ResStock 2025.1 fixture building (committed): HPXML + schedule + weather.
RESSTOCK_ROOT = ROOT / "tests" / "fixtures" / "resstock" / "2025.1"
BUILDING_DIR = RESSTOCK_ROOT / "bldg0000002"
BUILDING_WEATHER = RESSTOCK_ROOT / "weather" / "G0900090_2018.csv"


@pytest.fixture
def fleet(tmp_path: Path):
    """Build a one-dwelling fleet from a ResStock 2025.1 fixture building."""
    config = DwellingConfig(
        hpxml=str(BUILDING_DIR / "home.xml"),
        schedule=str(BUILDING_DIR / "in.schedules.csv"),
        weather=str(BUILDING_WEATHER),
        config=SimulationConfig(
            start_time="2018-01-01T00:00:00-05:00",
            duration=86400,
            time_res=3600,
            reports=[
                ReportSpec("frame", Selection.verbosity(0), Destination.memory()),
                ReportSpec("file", Selection.verbosity(0), Destination.csv(str(tmp_path / "fleet_bldg0000002.csv"))),
            ],
            metrics=True,
        ),
        defaults=str(ROOT / "python" / "hares" / "defaults"),
    )
    return Fleet.from_buildings([config])


# ---------------------------------------------------------------------------
# Dwelling tests
# ---------------------------------------------------------------------------


def test_simulate_returns_run_result(dwelling):
    """simulate() returns the run's RunResult whose memory report's table
    is a non-empty polars DataFrame with columns."""
    result = dwelling.simulate()

    assert isinstance(result.reports[0].table(), pl.DataFrame)
    frame = result.reports[0].table()
    assert frame.width > 0, "DataFrame should have at least one column"
    assert frame.height > 0, "DataFrame should have at least one row"


def test_no_report_run_has_no_metrics_and_no_reports(dwelling):
    """A run with no reports carries no reports and no metrics: the
    RunResult's surface is empty by construction (nothing was recorded)."""
    no_report = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400),
    )
    no_report.initialize()
    result = no_report.simulate()

    assert result.reports == []
    assert result.metrics is None
    assert result.warnings_dropped == 0


def test_results_dataframe_columns_include_time_and_power(simulated_df):
    """Verify DataFrame contains Time column and at least one power-related column."""
    columns = simulated_df.columns
    assert "Time" in columns, "DataFrame must have a Time column"

    power_columns = [c for c in columns if "(kW)" in c]
    assert len(power_columns) > 0, (
        "DataFrame must have at least one power column matching '(kW)'; "
        f"found columns: {columns}"
    )


def test_results_dataframe_shape(simulated_df):
    """Verify DataFrame has expected number of rows (1440 for 24h at 1min resolution)."""
    assert simulated_df.height > 0, "DataFrame should have rows"
    assert simulated_df.height == 1440, (
        f"Expected 1440 rows for 24h at 1min, got {simulated_df.height}"
    )


def test_results_dataframe_has_time_column(simulated_df):
    """Verify DataFrame has Time column."""
    assert "Time" in simulated_df.columns, "DataFrame should have Time column"


def test_results_dataframe_time_is_datetime(simulated_df):
    """Verify Time column is a naive-local millisecond Datetime (the Arrow
    Timestamp type travels the IPC path intact)."""
    time_col = simulated_df.get_column("Time")
    assert time_col.dtype == pl.Datetime("ms"), (
        "Time column is a millisecond timestamp from Arrow IPC"
    )


def test_results_dataframe_has_expected_columns(simulated_df):
    """Verify DataFrame has expected columns for energy metrics."""
    energy_columns = [
        col for col in simulated_df.columns if "(kWh)" in col or "(kW)" in col
    ]
    assert len(energy_columns) > 0, "Should have energy-related columns"


def test_results_column_dtypes(simulated_df):
    """Verify numeric columns are Float64."""
    numeric_cols = [col for col in simulated_df.columns if col != "Time"]
    for col in numeric_cols:
        dtype = simulated_df.get_column(col).dtype
        if dtype != pl.String:
            assert dtype == pl.Float64, f"Column {col} should be Float64, got {dtype}"


# ---------------------------------------------------------------------------
# Fleet tests -- run against one ResStock 2025.1 fixture building
# ---------------------------------------------------------------------------


def test_fleet_aggregate_returns_dataframe(fleet):
    """Verify FleetResult.aggregate.table() returns polars DataFrame."""
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.aggregate.table()

    assert isinstance(df, pl.DataFrame)


def test_fleet_aggregate_shape(fleet):
    """Verify the aggregate DataFrame has the fixture's exact shape.

    One day at 3600 s gives 24 hourly rows; the aggregate frame carries the
    6 reserved aggregate columns (Time, the three totals, outdoor and indoor
    temperature) for the one-building fleet.
    """
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.aggregate.table()

    assert df.shape == (24, 6), (
        f"Aggregate DataFrame shape {df.shape} != (24, 6); columns: {df.columns}"
    )


def test_fleet_aggregate_has_time_column(fleet):
    """Verify the aggregate DataFrame has a Time column."""
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.aggregate.table()

    assert "Time" in df.columns, "Aggregate DataFrame should have Time column"


def test_fleet_homes_returns_dataframe(fleet):
    """Verify FleetResult.homes.table() returns polars DataFrame."""
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.homes.table()

    assert isinstance(df, pl.DataFrame)


def test_fleet_homes_schema(fleet):
    """Verify FleetResult.homes has exactly the expected schema.

    Exact set equality: the homes frame is pinned to (1, 8) by the shape
    test, so a column added to or removed from the frame must update a pin.
    The fixture's config requests metrics, so the run metrics' columns ride
    the homes table.
    """
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.homes.table()

    expected_columns = [
        "bldg_id",
        "weight",
        "status",
        "reason",
        "steps",
        "periods_folded",
        "total_energy_kwh",
        "peak_power_kw",
    ]
    assert df.columns == expected_columns, (
        f"Column mismatch; expected: {expected_columns}, got: {df.columns}"
    )


def test_fleet_homes_row_count(fleet):
    """Verify the homes frame's exact shape and the weighting math.

    One building with no sample_weights gets the default weight 1.0, so the
    home's run metrics must relate to the aggregate frame's electric series
    directly: its total energy is the summed power over the 1 h steps and its
    peak power is the series maximum.
    """
    from hares import FleetReport, Frequency

    results = fleet.simulate(FleetReport(Selection.verbosity(0), Frequency.hourly()))
    df = results.homes.table()

    assert df.shape == (1, 8), f"Homes DataFrame shape {df.shape} != (1, 8)"
    row = df.row(0, named=True)
    assert row["weight"] == 1.0, (
        f"One building with no sample_weights must weigh 1.0, got {row['weight']}"
    )
    assert row["status"] != "failed", "the fixture building runs"
    assert row["periods_folded"] == 24, "the home folded every hourly period"

    aggregate = results.aggregate.table()
    electric_kw = aggregate["Total Electric Power (kW)"]
    assert row["total_energy_kwh"] == pytest.approx(
        float(electric_kw.sum()), rel=1e-12
    ), (
        "total_energy_kwh must equal the aggregate's summed electric "
        "power over the 1 h steps"
    )
    assert row["peak_power_kw"] == pytest.approx(float(electric_kw.max()), rel=1e-12), (
        "peak_power_kw must equal the aggregate's peak electric power"
    )

