"""The parsed-input handles: parse once, share across dwellings.

The weather file the handles are parsed from is deleted before the
dwellings are built: construction from a handle reads no file.
"""

import shutil

from conftest import HARES_DEFAULTS, HPXML, ROOT, SCHEDULE, WEATHER
from hares import Destination, Dwelling, ParsedWeather, ReportSpec, Selection, SimulationConfig


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
        HPXML, SCHEDULE, parsed, defaults_path=str(HARES_DEFAULTS), bldg_id=7, config=_config()
    )
    from_handle_pv = Dwelling.from_hpxml(
        base_pv, SCHEDULE, parsed, defaults_path=str(HARES_DEFAULTS), bldg_id=8, config=_config()
    )
    from_path_base = Dwelling.from_hpxml(
        HPXML, SCHEDULE, WEATHER, defaults_path=str(HARES_DEFAULTS), bldg_id=7, config=_config()
    )
    from_path_pv = Dwelling.from_hpxml(
        base_pv, SCHEDULE, WEATHER, defaults_path=str(HARES_DEFAULTS), bldg_id=8, config=_config()
    )

    for handle_dwelling, path_dwelling in [
        (from_handle_base, from_path_base),
        (from_handle_pv, from_path_pv),
    ]:
        f_handle = _frame(handle_dwelling)
        f_path = _frame(path_dwelling)
        assert f_handle.columns == f_path.columns
        assert f_handle.equals(f_path), "construction from a handle reads no file"
