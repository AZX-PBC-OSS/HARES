"""Tests for the Fleet API."""

import tempfile
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from hares import Destination, DwellingConfig, ReportSpec, Selection, SimulationConfig
from hares import Fleet as PyFleet

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

_RESSTOCK_HPXML = """<?xml version="1.0" encoding="UTF-8"?>
<HPXML xmlns="http://hpxmlonline.com/2023/09" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <ClimateandRiskZones>
        <WeatherStation>
          <SystemIdentifier id="WeatherStation"/>
          <Name>G0800130</Name>
        </WeatherStation>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"""


def _write_extracted_building(buildings_dir: Path, bldg_id: int) -> Path:
    """Write one building in the extracted layout from_resstock requires."""
    bldg_dir = buildings_dir / f"bldg{bldg_id:07d}"
    bldg_dir.mkdir(parents=True, exist_ok=True)
    (bldg_dir / "home.xml").write_text(_RESSTOCK_HPXML)
    (bldg_dir / "in.schedules.csv").write_text("occupants\n1\n")
    return buildings_dir


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


def _build_simulatable_fleet(output_dir: Path, n: int = 1):
    """Build a fleet from the committed BEopt example inputs."""
    hpxml = str(EXAMPLES / "BEopt_example.xml")
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    return PyFleet.from_buildings(
        [
            DwellingConfig(
                hpxml=hpxml, schedule=schedule, weather=weather,
                config=SimulationConfig(
                    start_time="2019-01-01T00:00:00",
                    duration=3600,
                    time_res=60,
                    reports=[
                        ReportSpec("frame", Selection.verbosity(0), Destination.memory()),
                        ReportSpec("file", Selection.verbosity(0), Destination.csv(str(output_dir / f"fleet_{i + 1}.csv"))),
                    ],
                ),
                bldg_id=i + 1,
            )
            for i in range(n)
        ]
    )


def test_from_buildings_constructs_fleet(tmp_path: Path) -> None:
    """Verify from_buildings([DwellingConfig(...)]) constructs a fleet with correct length."""
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=1,
    )
    fleet = PyFleet.from_buildings([config])

    assert len(fleet) == 1


def test_from_buildings_with_custom_weights(tmp_path: Path) -> None:
    """Verify from_buildings with custom sample_weights=[2.0, 1.0]."""
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=1,
    )
    config2 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=2,
    )

    fleet = PyFleet.from_buildings([config1, config2], sample_weights=[2.0, 1.0])

    assert len(fleet) == 2


def test_from_buildings_raises_on_weight_length_mismatch(tmp_path: Path) -> None:
    """Verify from_buildings raises if sample_weights length != configs length."""
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=1,
    )

    with pytest.raises(ValueError, match="sample_weights length"):
        PyFleet.from_buildings([config], sample_weights=[1.0, 2.0])


@pytest.mark.parametrize("bad_weight", [float("nan"), float("inf"), -1.0])
def test_from_buildings_rejects_invalid_weight(tmp_path: Path, bad_weight: float) -> None:
    """Verify from_buildings rejects NaN/infinite/negative sample weights.

    A non-finite or negative weight would silently corrupt fleet-level weighted
    aggregation, so it must be rejected at construction rather than propagating
    into the simulation results.
    """
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=1,
    )
    config2 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=2,
    )

    with pytest.raises(ValueError, match="invalid sample_weight"):
        PyFleet.from_buildings([config1, config2], sample_weights=[1.0, bad_weight])


def test_from_buildings_empty_list_raises(tmp_path: Path) -> None:
    """Verify from_buildings([]) with empty list raises ValueError."""
    with pytest.raises(ValueError, match="at least one dwelling config"):
        PyFleet.from_buildings([])


def test_len_returns_correct_count(tmp_path: Path) -> None:
    """Verify __len__ returns correct count."""
    hpxml, schedule, weather = _create_minimal_dwelling_config(tmp_path)

    config1 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=1,
    )
    config2 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=2,
    )
    config3 = DwellingConfig(
        hpxml=hpxml, schedule=schedule, weather=weather,
        config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=86400), bldg_id=3,
    )

    fleet = PyFleet.from_buildings([config1, config2, config3])

    assert len(fleet) == 3


def test_from_resstock_accepts_enum(tmp_path: Path) -> None:
    """Verify from_resstock still works (regression)."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade_id": pa.array([0]),
            "weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)
    _write_extracted_building(tmp_path, 12345)

    from hares import ResStockVersion

    fleet = PyFleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version=ResStockVersion.V2025_1,
    )

    assert isinstance(fleet, PyFleet)


def test_from_resstock_accepts_string(tmp_path: Path) -> None:
    """Verify from_resstock accepts string fallback."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade": pa.array([0]),
            "sample_weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)
    _write_extracted_building(tmp_path, 12345)

    fleet = PyFleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version="2024.2",
    )

    assert isinstance(fleet, PyFleet)


def test_from_resstock_default_version_is_2025_1(tmp_path: Path) -> None:
    """Verify from_resstock defaults to V2025_1 when no version specified."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade_id": pa.array([0]),
            "weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)
    _write_extracted_building(tmp_path, 12345)

    fleet = PyFleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
    )

    assert isinstance(fleet, PyFleet)
    assert len(fleet) == 1


def test_from_resstock_accepts_a_defaults_path(tmp_path: Path) -> None:
    """The binding takes the defaults_path keyword its stub declares."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade_id": pa.array([0]),
            "weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)
    _write_extracted_building(tmp_path, 12345)

    fleet = PyFleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        defaults_path=str(HARES_DEFAULTS),
    )

    assert len(fleet) == 1


def test_from_resstock_hpxml_dir_names_buildings_dir(tmp_path: Path) -> None:
    """The parameter is buildings_dir; the old hpxml_dir keyword errors
    with the removed-keys table's message, which names the replacement."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade_id": pa.array([0]),
            "weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)
    _write_extracted_building(tmp_path, 12345)

    # The static surface no longer carries the removed keyword, so the call
    # goes through getattr on purpose. All three required positionals are
    # passed so Python's own missing-argument TypeError cannot satisfy the
    # match before the removed-keys machinery runs.
    legacy_from_resstock = getattr(PyFleet, "from_resstock")
    with pytest.raises(TypeError, match="pass the unpacked buildings directory"):
        legacy_from_resstock(
            str(metadata),
            str(tmp_path),
            str(tmp_path),
            hpxml_dir=str(tmp_path),
        )


def test_resstock_version_rejects_invalid_string(tmp_path: Path) -> None:
    """Verify from_resstock rejects invalid version string."""
    metadata = tmp_path / "metadata.parquet"
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade": pa.array([0]),
            "sample_weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, metadata)

    with pytest.raises(ValueError, match="invalid ResStockVersion"):
        PyFleet.from_resstock(
            str(metadata),
            str(tmp_path),
            str(tmp_path),
            resstock_version="invalid",
        )


# ---------------------------------------------------------------------------
# simulate() tests -- require OCHRE vendor fixtures
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_simulate_resolution_enum_works(tmp_path: Path) -> None:
    """Verify simulate(resolution=AggregationResolution.Hourly) works."""
    from hares import AggregationResolution

    fleet = _build_simulatable_fleet(tmp_path, 1)
    results = fleet.simulate(resolution=AggregationResolution.Hourly)
    assert results.n_succeeded == 1


@pytest.mark.slow
def test_simulate_resolution_string_works(tmp_path: Path) -> None:
    """Verify simulate(resolution="hourly") string convenience works."""
    fleet = _build_simulatable_fleet(tmp_path, 1)
    results = fleet.simulate(resolution="hourly")
    assert results.n_succeeded == 1


@pytest.mark.slow
def test_simulate_default_resolution(tmp_path: Path) -> None:
    """Verify simulate() default uses FifteenMin."""
    fleet = _build_simulatable_fleet(tmp_path, 1)
    results = fleet.simulate()
    assert results.n_succeeded == 1
    # Default is FifteenMin; aggregate_timeseries should have data
    df = results.aggregate_timeseries
    assert df.height > 0


@pytest.mark.slow
def test_failures_property_empty_on_success(tmp_path: Path) -> None:
    """Verify failures property is empty list on all-success runs."""
    fleet = _build_simulatable_fleet(tmp_path, 1)
    results = fleet.simulate()
    assert results.failures == []


@pytest.mark.slow
def test_n_succeeded_and_n_failed_properties(tmp_path: Path) -> None:
    """Verify n_succeeded and n_failed properties."""
    fleet = _build_simulatable_fleet(tmp_path, 2)
    results = fleet.simulate()
    assert results.n_succeeded == 2
    assert results.n_failed == 0


@pytest.mark.slow
def test_progress_callback_invoked(tmp_path: Path) -> None:
    """Verify progress callback is invoked with (completed, total)."""
    fleet = _build_simulatable_fleet(tmp_path, 2)
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
    # Use invalid paths to force simulation failure
    sim_config = SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60)
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
    hpxml = str(EXAMPLES / "BEopt_example.xml")
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    sim_config = SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60)

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

    fixture_dir = ROOT / "tests" / "fixtures" / "hpxml" / "ochre_samples"
    schedule = str(EXAMPLES / "BEopt_example_schedule.csv")
    weather = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

    sim_config = SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60, verbosity=1)

    # The battery dwelling declares the control it tests: `Manual`, the
    # idle battery the column-mismatch scenario was built on.
    battery_overrides = {"Battery": {"bms_mode": '"Manual"'}}

    config1 = DwellingConfig(
        config=sim_config,
        hpxml=str(fixture_dir / "base.xml"),
        schedule=schedule,
        weather=weather,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
    )
    config2 = DwellingConfig(
        config=sim_config,
        hpxml=str(fixture_dir / "base-battery.xml"),
        schedule=schedule,
        weather=weather,
        defaults_path=str(HARES_DEFAULTS),
        overrides=battery_overrides,
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
