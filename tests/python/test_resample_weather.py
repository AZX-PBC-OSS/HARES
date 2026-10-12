"""`resample_weather`: the engine's own per-step weather channels, exposed
through Python. The acceptance: the exposed channels match the run's
internal series bitwise."""

from pathlib import Path

import pytest

from conftest import HARES_DEFAULTS
from hares import (
    Destination,
    Dwelling,
    ParsedWeather,
    ReportSpec,
    Selection,
    SimulationConfig,
    resample_weather,
)

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"

WEATHER_VARIABLES = [
    "Outdoor Dry Bulb (C)",
    "Outdoor Humidity Ratio (kg/kg)",
    "Outdoor Pressure (kPa)",
    "Wind Speed (m/s)",
    "Sky Temperature (C)",
    "GHI (W/m2)",
    "DNI (W/m2)",
    "DHI (W/m2)",
    "Temperature - Ground (C)",
    "Hot Water Mains Temperature (C)",
]


def _config(time_res: int) -> SimulationConfig:
    return SimulationConfig(
        duration=48 * 3600,
        time_res=time_res,
        start_time="2023-01-01T00:00:00-07:00",
        reports=[],
    )


def _report_config(time_res: int) -> SimulationConfig:
    return SimulationConfig(
        duration=48 * 3600,
        time_res=time_res,
        start_time="2023-01-01T00:00:00-07:00",
        reports=[
            ReportSpec(
                "weather",
                Selection.named(WEATHER_VARIABLES),
                Destination.memory(),
            )
        ],
    )


def _site_dict() -> dict[str, float]:
    """The resolved site, the values the run's dwelling was built with:
    the fixture's HPXML site, whose elevation falls back to the weather
    header's."""
    weather = ParsedWeather.from_path(str(FIXTURE / "weather.epw"))
    return {
        "latitude_deg": 40.01,
        "longitude_deg": -105.27,
        "elevation_m": weather.elevation_m,
        "utc_offset_h": -7.0,
    }


def test_resample_weather_matches_in_python():
    site = _site_dict()
    config = _config(900)
    frame = resample_weather(str(FIXTURE / "weather.epw"), site, config)

    dwelling = Dwelling.from_hpxml(
        str(FIXTURE / "building.xml"),
        str(FIXTURE / "schedule.csv"),
        str(FIXTURE / "weather.epw"),
        config=_report_config(900),
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
    )
    dwelling.initialize()
    result = dwelling.simulate()
    run = result.reports[0].table()

    assert frame.height == run.height, "one row per step"
    assert frame.columns[:1] == ["Time"]
    assert frame.columns[1:] == WEATHER_VARIABLES

    for name in WEATHER_VARIABLES:
        exposed = frame.get_column(name).to_list()
        internal = run.get_column(name).to_list()
        assert len(exposed) == len(internal)
        for exposed_value, internal_value in zip(exposed, internal):
            assert exposed_value == internal_value, (
                f"{name}: the exposed channel equals the run's series on bits"
            )


@pytest.mark.parametrize("time_res", [900, 3600])
def test_resample_weather_row_counts(time_res: int):
    site = _site_dict()
    frame = resample_weather(str(FIXTURE / "weather.epw"), site, _config(time_res))
    steps = (48 * 3600) // time_res
    assert frame.height == steps
    assert frame.columns == ["Time", *WEATHER_VARIABLES]
