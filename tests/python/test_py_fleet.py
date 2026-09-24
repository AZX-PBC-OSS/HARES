"""Tests for Fleet API completion (PY-009)."""

from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"


def _pyfleet_class():
    try:
        from ochre_next import Fleet, DwellingConfig
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")
    return Fleet, DwellingConfig


def _pyfleet_results_class():
    try:
        from ochre_next import FleetResults
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")
    return FleetResults


def _create_minimal_dwelling_config(tmp_path: Path):
    """Create minimal HPXML, schedule, and weather files for testing."""
    hpxml_content = """<?xml version="1.0" encoding="UTF-8"?>
<HPXML xmlns="http://hpxmlonline.com/2019/10" xml="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingID>1</BuildingID>
    <Site>
      <Address><City>TestCity</City><State>CO</State></Address>
    </Site>
  </Building>
</HPXML>"""
    hpxml_file = tmp_path / "building.xml"
    hpxml_file.write_text(hpxml_content)

    schedule_content = "timestamp,occupancy\n2019-01-01 00:00:00,1\n"
    schedule_file = tmp_path / "schedules.csv"
    schedule_file.write_text(schedule_content)

    weather_content = """Site,Year,,Jan,Feb,Mar,Apr,May,Jun,Jul,Aug,Sep,Oct,Nov,Dec
TestSite,2019,T Dry Bulb (C),5,6,7,8,10,15,20,19,15,10,7,5
"""
    weather_file = tmp_path / "weather.epw"
    weather_file.write_text(
        "2024 \n"
        "1 1 1 0 0 0\n"
        "50 50 50 50 50 50 50 50 50 50 50 50\n"
        "0 0 0 0 0 0 0 0 0 0 0 0\n"
        "0 0 0 0 0 0 0 0 0 0 0 0\n"
        "0 0 0 0 0 0 0 0 0 0 0 0\n"
        "0 0 0 0 0 0 0 0 0 0 0 0\n"
        "0 0 0 0 0 0 0 0 0 0 0 0\n"
    )

    return str(hpxml_file), str(schedule_file), str(weather_file)


def _ochre_fixtures_available() -> bool:
    return (EXAMPLES / "BEopt_example.xml").exists() and (EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw").exists()


def _build_simulatable_fleet(n: int = 1):
    """Build a fleet from real OCHRE fixtures that can actually simulate."""
    PyFleet, DwellingConfig = _pyfleet_class()
    from ochre_next import SimulationConfig

    if not _ochre_fixtures_available():
        pytest.skip("OCHRE fixtures not available")

    hpxml = str(EXAMPLES / "BEopt_example.xml")
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    sim_config = SimulationConfig(duration_s=3600, time_res_s=60)
    configs = [
        DwellingConfig(
            hpxml=hpxml, schedule=schedule, weather=weather,
            config=sim_config, bldg_id=i + 1,
        )
        for i in range(n)
    ]
    return PyFleet.from_buildings(configs)


def test_from_buildings_constructs_fleet(tmp_path: Path) -> None:
    """Verify from_buildings([DwellingConfig(...)]) constructs a fleet with correct length."""
    PyFleet, DwellingConfig = _pyfleet_class()
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=1)
    fleet = PyFleet.from_buildings([config])

    assert len(fleet) == 1


def test_from_buildings_with_custom_weights(tmp_path: Path) -> None:
    """Verify from_buildings with custom sample_weights=[2.0, 1.0]."""
    PyFleet, DwellingConfig = _pyfleet_class()
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=1)
    config2 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=2)

    fleet = PyFleet.from_buildings([config1, config2], sample_weights=[2.0, 1.0])

    assert len(fleet) == 2


def test_from_buildings_raises_on_weight_length_mismatch(tmp_path: Path) -> None:
    """Verify from_buildings raises if sample_weights length != configs length."""
    PyFleet, DwellingConfig = _pyfleet_class()
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=1)

    with pytest.raises(ValueError, match="sample_weights length"):
        PyFleet.from_buildings([config], sample_weights=[1.0, 2.0])


@pytest.mark.parametrize("bad_weight", [float("nan"), float("inf"), -1.0])
def test_from_buildings_rejects_invalid_weight(tmp_path: Path, bad_weight: float) -> None:
    """Verify from_buildings rejects NaN/infinite/negative sample weights.

    A non-finite or negative weight would silently corrupt fleet-level weighted
    aggregation, so it must be rejected at construction rather than propagating
    into the simulation results.
    """
    PyFleet, DwellingConfig = _pyfleet_class()
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=1)
    config2 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=2)

    with pytest.raises(ValueError, match="invalid sample_weight"):
        PyFleet.from_buildings([config1, config2], sample_weights=[1.0, bad_weight])


def test_from_buildings_empty_list_raises(tmp_path: Path) -> None:
    """Verify from_buildings([]) with empty list raises ValueError."""
    PyFleet, DwellingConfig = _pyfleet_class()

    with pytest.raises(ValueError, match="at least one dwelling config"):
        PyFleet.from_buildings([])


def test_len_returns_correct_count(tmp_path: Path) -> None:
    """Verify __len__ returns correct count."""
    PyFleet, DwellingConfig = _pyfleet_class()
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=1)
    config2 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=2)
    config3 = DwellingConfig(hpxml=hpxml, schedule=schedule, weather=weather, bldg_id=3)

    fleet = PyFleet.from_buildings([config1, config2, config3])

    assert len(fleet) == 3


def test_from_resstock_accepts_enum(tmp_path: Path) -> None:
    """Verify from_resstock still works (regression)."""
    py_fleet = _pyfleet_class()[0]
    pa = pytest.importorskip("pyarrow")
    pq = pytest.importorskip("pyarrow.parquet")

    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": [12345],
            "upgrade_id": [0],
            "weight": [1.0],
            "in.state": ["CO"],
        }
    )
    pq.write_table(table, metadata)

    from ochre_next import ResStockVersion

    fleet = py_fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version=ResStockVersion.V2025_1,
    )

    assert isinstance(fleet, py_fleet)


def test_from_resstock_accepts_string(tmp_path: Path) -> None:
    """Verify from_resstock accepts string fallback."""
    py_fleet = _pyfleet_class()[0]
    pa = pytest.importorskip("pyarrow")
    pq = pytest.importorskip("pyarrow.parquet")

    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": [12345],
            "upgrade": [0],
            "sample_weight": [1.0],
            "in.state": ["CO"],
        }
    )
    pq.write_table(table, metadata)

    fleet = py_fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version="2024.2",
    )

    assert isinstance(fleet, py_fleet)


def test_from_resstock_default_version_is_2025_1(tmp_path: Path) -> None:
    """Verify from_resstock defaults to V2025_1 when no version specified."""
    py_fleet = _pyfleet_class()[0]
    pa = pytest.importorskip("pyarrow")
    pq = pytest.importorskip("pyarrow.parquet")

    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": [12345],
            "upgrade_id": [0],
            "weight": [1.0],
            "in.state": ["CO"],
        }
    )
    pq.write_table(table, metadata)

    fleet = py_fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
    )

    assert isinstance(fleet, py_fleet)
    assert len(fleet) == 1


def test_resstock_version_rejects_invalid_string(tmp_path: Path) -> None:
    """Verify from_resstock rejects invalid version string."""
    py_fleet = _pyfleet_class()[0]
    pa = pytest.importorskip("pyarrow")
    pq = pytest.importorskip("pyarrow.parquet")

    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": [12345],
            "upgrade": [0],
            "sample_weight": [1.0],
            "in.state": ["CO"],
        }
    )
    pq.write_table(table, metadata)

    with pytest.raises(ValueError, match="invalid ResStockVersion"):
        py_fleet.from_resstock(
            str(metadata),
            str(tmp_path),
            str(tmp_path),
            resstock_version="invalid",
        )


# ---------------------------------------------------------------------------
# simulate() tests -- require OCHRE vendor fixtures
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_simulate_resolution_enum_works() -> None:
    """Verify simulate(resolution=AggregationResolution.Hourly) works."""
    from ochre_next import AggregationResolution

    fleet = _build_simulatable_fleet(1)
    results = fleet.simulate(resolution=AggregationResolution.Hourly)
    assert results.n_succeeded == 1


@pytest.mark.slow
def test_simulate_resolution_string_works() -> None:
    """Verify simulate(resolution="hourly") string convenience works."""
    fleet = _build_simulatable_fleet(1)
    results = fleet.simulate(resolution="hourly")
    assert results.n_succeeded == 1


@pytest.mark.slow
def test_simulate_default_resolution() -> None:
    """Verify simulate() default uses FifteenMin."""
    fleet = _build_simulatable_fleet(1)
    results = fleet.simulate()
    assert results.n_succeeded == 1
    # Default is FifteenMin; aggregate_timeseries should have data
    df = results.aggregate_timeseries
    assert df.height > 0


@pytest.mark.slow
def test_failures_property_empty_on_success() -> None:
    """Verify failures property is empty list on all-success runs."""
    fleet = _build_simulatable_fleet(1)
    results = fleet.simulate()
    assert results.failures == []


@pytest.mark.slow
def test_n_succeeded_and_n_failed_properties() -> None:
    """Verify n_succeeded and n_failed properties."""
    fleet = _build_simulatable_fleet(2)
    results = fleet.simulate()
    assert results.n_succeeded == 2
    assert results.n_failed == 0


@pytest.mark.slow
def test_progress_callback_invoked() -> None:
    """Verify progress callback is invoked with (completed, total)."""
    fleet = _build_simulatable_fleet(2)
    calls = []

    def on_progress(completed: int, total: int) -> None:
        calls.append((completed, total))

    results = fleet.simulate(progress=on_progress)
    assert results.n_succeeded == 2
    assert len(calls) == 2
    # All calls should have total == 2
    assert all(total == 2 for _, total in calls)
    # completed values should include 1 and 2 (order depends on threading)
    completed_values = sorted(c for c, _ in calls)
    assert completed_values == [1, 2]


@pytest.mark.slow
def test_simulate_raise_on_failure_true_raises(tmp_path: Path) -> None:
    """Verify simulate(raise_on_failure=True) raises on any failure."""
    PyFleet, DwellingConfig = _pyfleet_class()
    from ochre_next import SimulationConfig

    # Use invalid paths to force simulation failure
    sim_config = SimulationConfig(duration_s=3600, time_res_s=60)
    config = DwellingConfig(
        hpxml="/nonexistent/building.xml",
        schedule="/nonexistent/schedule.csv",
        weather="/nonexistent/weather.epw",
        config=sim_config,
        bldg_id=1,
    )
    fleet = PyFleet.from_buildings([config])

    with pytest.raises(ValueError, match="failed"):
        fleet.simulate(raise_on_failure=True)


@pytest.mark.slow
def test_simulate_fault_tolerant_populates_failures(tmp_path: Path) -> None:
    """Verify default fault-tolerant mode collects failures without raising."""
    PyFleet, DwellingConfig = _pyfleet_class()
    from ochre_next import SimulationConfig

    if not _ochre_fixtures_available():
        pytest.skip("OCHRE fixtures not available")

    hpxml = str(EXAMPLES / "BEopt_example.xml")
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    sim_config = SimulationConfig(duration_s=3600, time_res_s=60)

    # One valid config, one invalid -- partial failure
    good = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=sim_config, bldg_id=1,
    )
    bad = DwellingConfig(
        hpxml="/nonexistent/building.xml",
        schedule="/nonexistent/schedule.csv",
        weather="/nonexistent/weather.epw",
        config=sim_config,
        bldg_id=2,
    )
    fleet = PyFleet.from_buildings([good, bad])

    results = fleet.simulate(raise_on_failure=False)
    assert results.n_succeeded == 1
    assert results.n_failed == 1
    assert len(results.failures) == 1
    assert results.failures[0]["bldg_id"] == 2
    assert isinstance(results.failures[0]["error"], str)


@pytest.mark.slow
def test_heterogeneous_equipment_column_mismatch_emits_warning() -> None:
    """Verify simulate() emits a warning when heterogeneous equipment causes column mismatch.

    Two dwellings with different equipment types produce different output
    column sets (e.g., a battery-equipped dwelling has extra battery columns).
    The fleet aggregator detects the mismatch and returns an empty aggregate
    batch. The Python guard then emits a warning so callers can detect the
    condition without inspecting the DataFrame shape.
    """
    import warnings

    PyFleet, DwellingConfig = _pyfleet_class()
    from ochre_next import SimulationConfig

    if not _ochre_fixtures_available():
        pytest.skip("OCHRE fixtures not available")

    fixture_dir = ROOT / "tests" / "fixtures" / "hpxml" / "ochre_samples"
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    sim_config = SimulationConfig(duration_s=3600, time_res_s=60, output_verbosity=1)

    config1 = DwellingConfig(
        hpxml=str(fixture_dir / "base.xml"),
        schedule=schedule,
        weather=weather,
        config=sim_config,
        bldg_id=1,
    )
    config2 = DwellingConfig(
        hpxml=str(fixture_dir / "base-battery.xml"),
        schedule=schedule,
        weather=weather,
        config=sim_config,
        bldg_id=2,
    )
    fleet = PyFleet.from_buildings([config1, config2])

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        results = fleet.simulate()

    # Both dwellings should complete (the guard emits a warning, not an error).
    assert results.n_succeeded == 2, (
        f"Expected both dwellings to succeed, got n_succeeded={results.n_succeeded}"
    )

    # The aggregate timeseries is empty because column sets differ across dwellings.
    assert results.aggregate_timeseries.height == 0

    # The Python guard must emit a warning so callers can detect the condition.
    assert len(w) >= 1, (
        f"Expected at least 1 warning for heterogeneous fleet, got {len(w)}"
    )
    assert any(
        "empty timeseries" in str(warning.message).lower()
        for warning in w
    ), (
        f"No 'empty timeseries' warning found; warnings: "
        f"{[str(x.message) for x in w]}"
    )
