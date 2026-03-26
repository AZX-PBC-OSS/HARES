"""Tests for Arrow to Polars zero-copy conversion."""

from pathlib import Path
import pytest

import polars as pl


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

    return dwelling_class.from_hpxml(hpxml, schedule, weather)


@pytest.fixture(scope="session")
def simulated_df(dwelling):
    """Run simulate() once and share the resulting DataFrame across tests."""
    return dwelling.simulate()


RESSTOCK_METADATA = ROOT / "tests" / "fixtures" / "resstock_metadata.parquet"
RESSTOCK_HPXML_DIR = ROOT / "tests" / "fixtures" / "building_energy_models"
RESSTOCK_WEATHER_DIR = ROOT / "tests" / "fixtures" / "weather"


@pytest.fixture
def fleet():
    """Create a minimal fleet for testing.

    Requires pre-processed ResStock fixtures (HPXML zips, weather files,
    metadata parquet) that are not checked in. Tests skip automatically
    when these are absent.
    """
    if not RESSTOCK_METADATA.exists():
        pytest.skip("ResStock test fixtures not available")

    fleet_class = _fleet_class()
    return fleet_class.from_resstock(
        str(RESSTOCK_METADATA),
        str(RESSTOCK_HPXML_DIR),
        str(RESSTOCK_WEATHER_DIR),
        resstock_version="2024.2",
    )


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
    )
    df = fresh.results()

    assert isinstance(df, pl.DataFrame)
    assert df.height == 0, "Results before simulate should be empty"
    assert df.width > 0, "Schema should have columns even when no rows have been produced"
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
    energy_columns = [col for col in simulated_df.columns if "(kWh)" in col or "(kW)" in col]
    assert len(energy_columns) > 0, "Should have energy-related columns"


def test_results_column_dtypes(simulated_df):
    """Verify numeric columns are Float64."""
    numeric_cols = [col for col in simulated_df.columns if col != "Time"]
    for col in numeric_cols:
        dtype = simulated_df.get_column(col).dtype
        if dtype != pl.String:
            assert dtype == pl.Float64, f"Column {col} should be Float64, got {dtype}"


# ---------------------------------------------------------------------------
# Fleet tests — skipped at fixture level when HPXML data is unavailable
# ---------------------------------------------------------------------------


def test_fleet_aggregate_timeseries_returns_dataframe(fleet):
    """Verify fleet aggregate_timeseries returns polars DataFrame."""
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert isinstance(df, pl.DataFrame)


def test_fleet_aggregate_timeseries_has_rows(fleet):
    """Verify aggregate DataFrame has rows."""
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert df.height > 0, "Aggregate DataFrame should have rows"


def test_fleet_aggregate_timeseries_has_time_column(fleet):
    """Verify aggregate DataFrame has Time column."""
    results = fleet.simulate()
    df = results.aggregate_timeseries

    assert "Time" in df.columns, "Aggregate DataFrame should have Time column"


def test_fleet_per_dwelling_metrics_returns_dataframe(fleet):
    """Verify fleet per_dwelling_metrics returns polars DataFrame."""
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    assert isinstance(df, pl.DataFrame)


def test_fleet_per_dwelling_metrics_schema(fleet):
    """Verify per_dwelling_metrics has expected schema."""
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    expected_columns = {
        "annual_energy_kwh",
        "peak_power_kw",
        "sample_weight",
        "status",
        "failed",
    }
    actual_columns = set(df.columns)
    assert expected_columns.issubset(actual_columns), (
        f"Missing columns: {expected_columns - actual_columns}"
    )


def test_fleet_per_dwelling_metrics_row_count(fleet):
    """Verify per_dwelling_metrics has correct row count."""
    results = fleet.simulate()
    df = results.per_dwelling_metrics

    assert df.height == 1, "Should have 1 dwelling in test fixture"
