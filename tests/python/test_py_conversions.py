"""Tests for Arrow to Polars zero-copy conversion."""

from pathlib import Path

import polars as pl
import pytest

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"


def _dwelling_class():
    try:
        from ochre_next import Dwelling
    except ModuleNotFoundError:
        pytest.skip("ochre_next.Dwelling is not available in this environment")
    return Dwelling


def _fleet_class():
    try:
        from ochre_next import Fleet
    except ModuleNotFoundError:
        pytest.skip("ochre_next.Fleet is not available in this environment")
    return Fleet


@pytest.fixture(scope="session")
def dwelling():
    """Create a Dwelling using the test fixtures."""
    dwelling_class = _dwelling_class()

    if (EXAMPLES / "BEopt_example.xml").exists():
        hpxml = str(EXAMPLES / "BEopt_example.xml")
        schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
        weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")
    else:
        pytest.skip("OCHRE fixtures not available")

    dw = dwelling_class.from_hpxml(hpxml, schedule, weather, write_output=False)
    dw.initialize()
    return dw


@pytest.fixture(scope="session")
def simulated_df(dwelling):
    """Run simulate() once and share the resulting DataFrame across tests."""
    return dwelling.simulate()


# One ResStock 2025.1 fixture building (committed): HPXML + schedule + weather.
RESSTOCK_ROOT = ROOT / "tests" / "fixtures" / "resstock" / "2025.1"
BUILDING_DIR = RESSTOCK_ROOT / "bldg0000002"
BUILDING_WEATHER = RESSTOCK_ROOT / "weather" / "G0900090_2018.csv"


@pytest.fixture
def fleet(tmp_path: Path):
    """Build a one-dwelling fleet from a ResStock 2025.1 fixture building."""
    fleet_class = _fleet_class()
    from ochre_next import DwellingConfig, SimulationConfig

    config = DwellingConfig(
        hpxml=str(BUILDING_DIR / "home.xml"),
        schedule=str(BUILDING_DIR / "in.schedules.csv"),
        weather=str(BUILDING_WEATHER),
        config=SimulationConfig(
            start_time="2018-01-01T00:00:00-05:00",
            duration_s=86400,
            time_res_s=3600,
            write_output=True,
            output_path=str(tmp_path / "fleet_bldg0000002.csv"),
        ),
        defaults_path=str(ROOT / "defaults"),
    )
    return fleet_class.from_buildings([config])


# ---------------------------------------------------------------------------
# Dwelling tests
# ---------------------------------------------------------------------------


def test_simulate_returns_dataframe(simulated_df):
    """Verify that simulate() returns a non-empty polars DataFrame with columns."""
    assert isinstance(simulated_df, pl.DataFrame)
    assert simulated_df.width > 0, "DataFrame should have at least one column"
    assert simulated_df.height > 0, "DataFrame should have at least one row"


def test_results_before_simulate_returns_dataframe(dwelling):
    """Verify results() returns DataFrame even before simulate() (empty batches fallback).

    Note: uses a fresh dwelling (not the simulated one) to test the pre-simulate path.
    """
    dwelling_class = _dwelling_class()

    if not (EXAMPLES / "BEopt_example.xml").exists():
        pytest.skip("OCHRE fixtures not available")

    fresh = dwelling_class.from_hpxml(
        str(EXAMPLES / "BEopt_example.xml"),
        str(EXAMPLES / "BEopt_example_schedule.csv"),
        str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        write_output=False,
    )
    df = fresh.results()

    assert isinstance(df, pl.DataFrame)
    assert df.height == 0, "Results before simulate should be empty"
    assert df.width > 0, (
        "Schema should have columns even when no rows have been produced"
    )
    assert "Time" in df.columns, "Schema should include a Time column"


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


def test_results_dataframe_time_is_string(simulated_df):
    """Verify Time column is parsed as string (Arrow IPC preserves timestamp as string)."""
    time_col = simulated_df.get_column("Time")
    assert time_col.dtype == pl.String, "Time column is stored as string from Arrow IPC"


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

# The fleet block runs in the same job as the OCHRE reference block (the
# "slow or ochre" selector); the default job's "not slow and not ochre"
# deselects these six instead of running them there.


@pytest.mark.ochre
def test_fleet_aggregate_timeseries_returns_dataframe(fleet):
    """Verify fleet aggregate_timeseries returns polars DataFrame."""
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert isinstance(df, pl.DataFrame)


@pytest.mark.ochre
def test_fleet_aggregate_timeseries_shape(fleet):
    """Verify aggregate DataFrame has the fixture's exact shape.

    One day at 3600 s gives 24 hourly rows; the aggregate frame carries the
    6 reserved aggregate columns (Time, the three totals, outdoor and indoor
    temperature) for the one-building fleet.
    """
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert df.shape == (24, 6), (
        f"Aggregate DataFrame shape {df.shape} != (24, 6); columns: {df.columns}"
    )


@pytest.mark.ochre
def test_fleet_aggregate_timeseries_has_time_column(fleet):
    """Verify aggregate DataFrame has Time column."""
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert "Time" in df.columns, "Aggregate DataFrame should have Time column"


@pytest.mark.ochre
def test_fleet_per_dwelling_metrics_returns_dataframe(fleet):
    """Verify fleet per_dwelling_metrics returns polars DataFrame."""
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    assert isinstance(df, pl.DataFrame)


@pytest.mark.ochre
def test_fleet_per_dwelling_metrics_schema(fleet):
    """Verify per_dwelling_metrics has exactly the expected schema.

    Exact set equality: the per-dwelling frame is pinned to (1, 5) elsewhere,
    so a column added to or removed from the frame must update a pin.
    """
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    expected_columns = {
        "total_energy_kwh",
        "peak_power_kw",
        "sample_weight",
        "status",
        "failed",
    }
    actual_columns = set(df.columns)
    assert actual_columns == expected_columns, (
        f"Column mismatch; missing: {expected_columns - actual_columns}, "
        f"unexpected: {actual_columns - expected_columns}"
    )


@pytest.mark.ochre
def test_fleet_per_dwelling_metrics_row_count(fleet):
    """Verify the per-dwelling frame's exact shape and the weighting math.

    One building with no sample_weights gets the default weight 1.0, so the
    per-dwelling metrics must equal the aggregate frame's electric series
    directly: its total energy is the summed power over the 1 h steps and its
    peak power is the series maximum.
    """
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    assert df.shape == (1, 5), f"Per-dwelling DataFrame shape {df.shape} != (1, 5)"
    row = df.row(0, named=True)
    assert row["sample_weight"] == 1.0, (
        f"One building with no sample_weights must weigh 1.0, got {row['sample_weight']}"
    )

    aggregate = results.aggregate_timeseries
    electric_kw = aggregate["Total Electric Power (kW)"]
    assert row["total_energy_kwh"] == pytest.approx(
        float(electric_kw.sum()), rel=1e-12
    ), (
        "total_energy_kwh must equal the weighted aggregate's summed electric "
        "power over the 1 h steps"
    )
    assert row["peak_power_kw"] == pytest.approx(float(electric_kw.max()), rel=1e-12), (
        "peak_power_kw must equal the weighted aggregate's peak electric power"
    )
