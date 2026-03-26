"""Tests for Python weather parsing bindings."""

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
EPW_PATH = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")

EXPECTED_COLUMNS = [
    "index",
    "dry_bulb_c",
    "dew_point_c",
    "rel_humidity_pct",
    "pressure_kpa",
    "ghi_w_m2",
    "dni_w_m2",
    "dhi_w_m2",
    "wind_speed_m_s",
    "wind_dir_deg",
    "opaque_sky_cover",
    "horizontal_infrared_w_m2",
    "sky_temp_c",
    "ground_temp_c",
    "liquid_precip_m",
    "surface_albedo",
]


class TestParseEpw:
    def test_len_equals_8760(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert len(weather) == 8760

    def test_to_polars_returns_dataframe(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        df = weather.to_polars()

        import polars as pl

        assert isinstance(df, pl.DataFrame)

    def test_to_polars_has_15_columns(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        df = weather.to_polars()

        assert len(df.columns) >= 15

    def test_to_polars_column_names_match(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        df = weather.to_polars()

        assert df.columns == EXPECTED_COLUMNS

    def test_latitude_from_epw_header(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.latitude == pytest.approx(39.83, rel=0.01)

    def test_longitude_from_epw_header(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.longitude == pytest.approx(-104.65, rel=0.01)

    def test_elevation_m_from_epw_header(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.elevation_m == pytest.approx(1650.0, rel=0.01)

    def test_source_step_secs_equals_3600(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.source_step_secs == 3600

    def test_midpoint_offset_secs_equals_1800(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.midpoint_offset_secs == 1800

    def test_surface_albedo_none_for_epw(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert weather.surface_albedo is None

    def test_location_is_non_empty_string(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        assert isinstance(weather.location, str)
        assert len(weather.location) > 0


class TestParseWeatherAutoDetect:
    def test_parse_weather_auto_detects_epw(self):
        from ochre_next import parse_weather, parse_epw

        weather_auto = parse_weather(EPW_PATH)
        weather_explicit = parse_epw(EPW_PATH)

        assert len(weather_auto) == len(weather_explicit)
        assert weather_auto.location == weather_explicit.location
        assert weather_auto.latitude == weather_explicit.latitude


class TestErrorHandling:
    def test_invalid_path_raises_error_with_path_in_message(self):
        from ochre_next import parse_epw

        with pytest.raises(Exception) as exc_info:
            parse_epw("/nonexistent/path/to/file.epw")

        assert "/nonexistent/path/to/file.epw" in str(exc_info.value)


class TestParseResStockCsv:
    def test_parse_resstock_csv_requires_all_4_location_params(self):
        from ochre_next import parse_resstock_csv
        import inspect

        sig = inspect.signature(parse_resstock_csv)
        params = list(sig.parameters.keys())

        assert "elevation_m" in params
        assert "latitude" in params
        assert "longitude" in params
        assert "timezone_offset_h" in params


class TestRepr:
    def test_repr_works_without_error(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        repr_str = repr(weather)

        assert "WeatherTimeSeries" in repr_str
        assert "8760" in repr_str


class TestDataFrameRowCount:
    def test_dataframe_row_count_matches_len(self):
        from ochre_next import parse_epw

        weather = parse_epw(EPW_PATH)
        df = weather.to_polars()

        assert len(df) == len(weather)
