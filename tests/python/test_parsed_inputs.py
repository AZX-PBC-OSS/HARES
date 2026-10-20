"""The parsed-input handles: parse once, share across dwellings.

The weather file the handles are parsed from is deleted before the
dwellings are built: construction from a handle reads no file.
"""

import shutil

import pytest

from conftest import HARES_DEFAULTS, HPXML, ROOT, SCHEDULE, WEATHER
from hares import Destination, Dwelling, ParsedSchedule, ParsedWeather, ReportSpec, Selection, SimulationConfig


def _frame(dwelling):
    dwelling.initialize()
    result = dwelling.simulate()
    return result.reports[0].table()


def _config(duration_s=6 * 3600):
    return SimulationConfig(
        duration=duration_s,
        time_res=900,
        start_time="2019-01-01T00:00:00",
        reports=[
            ReportSpec("frame", Selection.verbosity(2), Destination.memory())
        ],
    )


def test_parsed_weather_shared_across_dwellings(tmp_path):
    copy = tmp_path / "weather.epw"
    shutil.copy(WEATHER, copy)
    parsed = ParsedWeather.from_path(str(copy))
    copy.unlink()

    base_pv = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml")
    from_handle_base = Dwelling.from_hpxml(
        HPXML, SCHEDULE, parsed, defaults=str(HARES_DEFAULTS), bldg_id=7, config=_config()
    )
    from_handle_pv = Dwelling.from_hpxml(
        base_pv, SCHEDULE, parsed, defaults=str(HARES_DEFAULTS), bldg_id=8, config=_config()
    )
    from_path_base = Dwelling.from_hpxml(
        HPXML, SCHEDULE, WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=7, config=_config()
    )
    from_path_pv = Dwelling.from_hpxml(
        base_pv, SCHEDULE, WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=8, config=_config()
    )

    for handle_dwelling, path_dwelling in [
        (from_handle_base, from_path_base),
        (from_handle_pv, from_path_pv),
    ]:
        f_handle = _frame(handle_dwelling)
        f_path = _frame(path_dwelling)
        assert f_handle.columns == f_path.columns
        assert f_handle.equals(f_path), "construction from a handle reads no file"


def _naive_schedule_csv(tmp_path):
    """The example schedule's first day under naive local timestamps: a
    time column whose values carry no UTC offset."""
    lines = open(SCHEDULE).read().splitlines()
    day = lines[:25]
    rows = ["time," + day[0]]
    for idx, row in enumerate(day[1:]):
        rows.append(f"2019-01-01 {idx:02d}:00:00,{row}")
    csv = tmp_path / "naive_schedule.csv"
    csv.write_text("\n".join(rows) + "\n")
    return csv


def test_schedule_handle_takes_the_weather_meta_fallback(tmp_path):
    """A schedule CSV whose timestamps carry no UTC offset parses through
    the handle with the weather's meta, the same fallback the config path
    passes; without it the parse fails naming the missing fallback."""
    csv = _naive_schedule_csv(tmp_path)

    with pytest.raises(Exception, match="no WeatherMeta fallback"):
        ParsedSchedule.from_path(str(csv))

    weather = ParsedWeather.from_path(WEATHER)
    handle = ParsedSchedule.from_path(str(csv), weather_meta=weather)
    from_text = ParsedSchedule.from_text(csv.read_text(), weather_meta=weather)

    from_handle = Dwelling.from_hpxml(
        HPXML, handle, WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=9, config=_config()
    )
    from_text_handle = Dwelling.from_hpxml(
        HPXML, from_text, WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=9, config=_config()
    )
    from_path = Dwelling.from_hpxml(
        HPXML, str(csv), WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=9, config=_config()
    )

    f_handle = _frame(from_handle)
    assert f_handle.equals(_frame(from_text_handle)), "from_path and from_text parse the same"
    assert f_handle.equals(_frame(from_path)), "the handle's parse is the config path's parse"
