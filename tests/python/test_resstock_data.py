"""Tests for ochre_next.data.resstock (HARES-060)."""

from __future__ import annotations

import io
import sys
import zipfile
from pathlib import Path
from unittest import mock

import polars as pl
import pytest


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_zip(hpxml_content: str = "", schedule_content: str = "") -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("home.xml", hpxml_content or _minimal_hpxml("G0800130"))
        zf.writestr("in.schedules.csv", schedule_content or "hour,value\n0,1.0\n")
    return buf.getvalue()


def _minimal_hpxml(fips: str) -> str:
    return (
        '<?xml version="1.0"?>'
        "<HPXML>"
        "<Building><ClimateandRiskZones><WeatherStation>"
        f"<Name>{fips}</Name>"
        "</WeatherStation></ClimateandRiskZones></Building>"
        "</HPXML>"
    )


def _make_metadata_parquet(
    bldg_ids: list[int],
    states: list[str] | None = None,
    weights: list[float] | None = None,
) -> bytes:
    n = len(bldg_ids)
    data: dict[str, list] = {
        "bldg_id": bldg_ids,
        "in.state": states or ["CO"] * n,
        "sample_weight": weights or [1.0] * n,
    }
    df = pl.DataFrame(data)
    buf = io.BytesIO()
    df.write_parquet(buf)
    return buf.getvalue()


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# 1. Version config / URL construction
# ---------------------------------------------------------------------------


class TestVersionConfig:
    def test_v2024_1_zip_url(self):
        from ochre_next.data.resstock import _version_config, _zip_url

        cfg = _version_config("2024.1")
        url = _zip_url(cfg, bldg_id=1, upgrade_id=0)
        assert "2024/resstock_dataset_2024.1/resstock_tmy3/" in url
        assert "bldg0000001-up00.zip" in url

    def test_v2024_2_zip_url(self):
        from ochre_next.data.resstock import _version_config, _zip_url

        cfg = _version_config("2024.2")
        url = _zip_url(cfg, bldg_id=42, upgrade_id=0)
        assert "2024/resstock_tmy3_release_2/" in url
        assert "bldg0000042-up00.zip" in url
        assert "upgrade=0" in url

    def test_v2025_1_zip_url(self):
        from ochre_next.data.resstock import _version_config, _zip_url

        cfg = _version_config("2025.1")
        url = _zip_url(cfg, bldg_id=7, upgrade_id=1)
        assert "2025/resstock_amy2018_release_1/" in url
        assert "bldg0000007-up01.zip" in url

    def test_v2025_1_metadata_baseline(self):
        from ochre_next.data.resstock import _version_config, _metadata_url

        cfg = _version_config("2025.1")
        url = _metadata_url(cfg, upgrade_id=0)
        assert "upgrade0.parquet" in url
        assert "upgrade00" not in url  # NOT zero-padded for 2025.1

    def test_v2024_2_metadata_upgrade(self):
        from ochre_next.data.resstock import _version_config, _metadata_url

        cfg = _version_config("2024.2")
        url = _metadata_url(cfg, upgrade_id=3)
        assert "upgrade03_metadata_and_annual_results.parquet" in url

    def test_weather_url_v2024_2(self):
        from ochre_next.data.resstock import _version_config, _weather_url

        cfg = _version_config("2024.2")
        url = _weather_url(cfg, state="CO", fips="G0800130")
        assert "weather/state=CO/G0800130_TMY3.csv" in url

    def test_weather_url_v2025_1(self):
        from ochre_next.data.resstock import _version_config, _weather_url

        cfg = _version_config("2025.1")
        url = _weather_url(cfg, state="CO", fips="G0800130")
        assert "weather/state=CO/G0800130_2018.csv" in url

    def test_invalid_version_raises(self):
        from ochre_next.data.resstock import _version_config

        with pytest.raises(ValueError, match="Unknown ResStock version"):
            _version_config("9999.9")


# ---------------------------------------------------------------------------
# 2. ResStockBuilding dataclass
# ---------------------------------------------------------------------------


class TestResStockBuilding:
    def test_frozen(self):
        from ochre_next.data.resstock import ResStockBuilding

        b = ResStockBuilding(
            bldg_id=1,
            sample_weight=1.0,
            hpxml_path=Path("/a.xml"),
            schedule_path=Path("/b.csv"),
            weather_path=Path("/c.csv"),
        )
        with pytest.raises((dataclasses.FrozenInstanceError if False else Exception)):
            b.bldg_id = 99  # type: ignore[misc]

    def test_fields_accessible(self):
        from ochre_next.data.resstock import ResStockBuilding

        b = ResStockBuilding(1, 2.5, Path("/h.xml"), Path("/s.csv"), Path("/w.csv"))
        assert b.bldg_id == 1
        assert b.sample_weight == 2.5


import dataclasses  # noqa: E402 – needed for test_frozen above


class TestResStockBuildingFrozen:
    def test_mutation_raises(self):
        from ochre_next.data.resstock import ResStockBuilding

        b = ResStockBuilding(1, 1.0, Path("/a"), Path("/b"), Path("/c"))
        with pytest.raises(dataclasses.FrozenInstanceError):
            b.bldg_id = 2  # type: ignore[misc]


# ---------------------------------------------------------------------------
# 3. fetch_resstock_building -- basic download + extract
# ---------------------------------------------------------------------------


class TestFetchResStockBuilding:
    def test_downloads_and_extracts(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip()
        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("fake EPW")

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=_fake_download(zip_bytes, tmp_path),
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path)

        assert result.bldg_id == 1
        assert result.hpxml_path.exists()
        assert result.schedule_path.exists()
        assert result.hpxml_path.name == "home.xml"
        assert result.schedule_path.name == "in.schedules.csv"

    def test_returns_resstock_building_instance(self, tmp_path: Path):
        from ochre_next.data.resstock import ResStockBuilding, fetch_resstock_building

        zip_bytes = _make_zip()
        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("fake EPW")

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=_fake_download(zip_bytes, tmp_path),
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(5, version="2024.2", cache_dir=tmp_path)

        assert isinstance(result, ResStockBuilding)

    def test_sample_weight_is_one(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip()
        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("fake EPW")

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=_fake_download(zip_bytes, tmp_path),
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(1, cache_dir=tmp_path)

        assert result.sample_weight == 1.0

    def test_uses_cache_on_second_call(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip()
        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("fake EPW")
        download_calls: list[str] = []

        def recording_download(url: str, dest: Path) -> None:
            download_calls.append(url)
            _fake_download(zip_bytes, tmp_path)(url, dest)

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=recording_download,
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            fetch_resstock_building(1, cache_dir=tmp_path)
            fetch_resstock_building(1, cache_dir=tmp_path)

        # ZIP download should only happen once
        zip_urls = [u for u in download_calls if u.endswith(".zip")]
        assert len(zip_urls) == 1

    def test_weather_override_skips_weather_download(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip()
        weather_file = tmp_path / "my_weather.epw"
        weather_file.write_text("EPW data")
        download_calls: list[str] = []

        def recording_download(url: str, dest: Path) -> None:
            download_calls.append(url)
            _fake_download(zip_bytes, tmp_path)(url, dest)

        with mock.patch(
            "ochre_next.data.resstock._download_file", side_effect=recording_download
        ):
            result = fetch_resstock_building(
                1,
                cache_dir=tmp_path,
                weather_override=weather_file,
            )

        assert result.weather_path == weather_file
        # No weather CSV download attempted
        weather_urls = [u for u in download_calls if ".csv" in u and "weather" in u]
        assert len(weather_urls) == 0

    def test_weather_downloaded_from_hpxml(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip(hpxml_content=_minimal_hpxml("G0800130"))

        # V2024.2 uses EPW format (TMY3) -- weather is fetched via get_epw_for_fips
        dummy_epw = tmp_path / "weather" / "BuildStock_TMY3_FIPS" / "G0800130.epw"
        dummy_epw.parent.mkdir(parents=True, exist_ok=True)
        dummy_epw.write_text("fake EPW data")

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=_fake_download(zip_bytes, tmp_path),
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path)

        assert result.weather_path.suffix == ".epw"
        assert "G0800130" in result.weather_path.name

    def test_upgrade_id_in_url(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip()
        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("fake EPW")
        captured_urls: list[str] = []

        def recording_download(url: str, dest: Path) -> None:
            captured_urls.append(url)
            _fake_download(zip_bytes, tmp_path)(url, dest)

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=recording_download,
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            fetch_resstock_building(1, upgrade_id=3, cache_dir=tmp_path)

        zip_urls = [u for u in captured_urls if u.endswith(".zip")]
        assert "upgrade=3" in zip_urls[0]
        assert "up03.zip" in zip_urls[0]


# ---------------------------------------------------------------------------
# 4. fetch_resstock_fleet
# ---------------------------------------------------------------------------


@pytest.mark.slow
class TestFetchResStockFleet:
    def _metadata_file(self, tmp_path: Path, bldg_ids: list[int], **kwargs) -> Path:
        data = _make_metadata_parquet(bldg_ids, **kwargs)
        p = tmp_path / "metadata.parquet"
        p.write_bytes(data)
        return p

    def test_returns_n_buildings(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(tmp_path, list(range(1, 11)))

        with mock.patch(
            "ochre_next.data.resstock.fetch_resstock_building",
            side_effect=_fake_fleet_building(tmp_path),
        ):
            results = fetch_resstock_fleet(meta, n_buildings=3, cache_dir=tmp_path)

        assert len(results) == 3

    def test_returns_explicit_bldg_ids(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(tmp_path, [1, 2, 3, 4, 5])

        with mock.patch(
            "ochre_next.data.resstock.fetch_resstock_building",
            side_effect=_fake_fleet_building(tmp_path),
        ):
            results = fetch_resstock_fleet(meta, bldg_ids=[2, 4], cache_dir=tmp_path)

        returned_ids = {r.bldg_id for r in results}
        assert returned_ids == {2, 4}

    def test_filter_by_state(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(
            tmp_path,
            [1, 2, 3, 4],
            states=["CO", "CA", "CO", "TX"],
        )

        with mock.patch(
            "ochre_next.data.resstock.fetch_resstock_building",
            side_effect=_fake_fleet_building(tmp_path),
        ):
            results = fetch_resstock_fleet(
                meta,
                filter={"in.state": "CO"},
                cache_dir=tmp_path,
            )

        assert len(results) == 2
        assert all(r.bldg_id in (1, 3) for r in results)

    def test_filter_then_n_buildings(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(
            tmp_path,
            list(range(1, 11)),
            states=["CO"] * 7 + ["CA"] * 3,
        )

        with mock.patch(
            "ochre_next.data.resstock.fetch_resstock_building",
            side_effect=_fake_fleet_building(tmp_path),
        ):
            results = fetch_resstock_fleet(
                meta,
                filter={"in.state": "CO"},
                n_buildings=3,
                cache_dir=tmp_path,
            )

        assert len(results) == 3

    def test_sample_weights_from_metadata(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(
            tmp_path,
            [10, 20],
            weights=[42.5, 17.3],
        )

        with mock.patch(
            "ochre_next.data.resstock.fetch_resstock_building",
            side_effect=_fake_fleet_building(tmp_path),
        ):
            results = fetch_resstock_fleet(meta, bldg_ids=[10, 20], cache_dir=tmp_path)

        by_id = {r.bldg_id: r for r in results}
        assert by_id[10].sample_weight == pytest.approx(42.5)
        assert by_id[20].sample_weight == pytest.approx(17.3)

    def test_bldg_ids_and_n_buildings_mutually_exclusive(self, tmp_path: Path):
        from ochre_next.data.resstock import fetch_resstock_fleet

        meta = self._metadata_file(tmp_path, [1, 2])
        with pytest.raises(ValueError, match="at most one"):
            fetch_resstock_fleet(meta, bldg_ids=[1], n_buildings=1, cache_dir=tmp_path)


# ---------------------------------------------------------------------------
# 5. Fallback to urllib (no httpx, no boto3)
# ---------------------------------------------------------------------------


class TestFallbackToUrllib:
    def test_urllib_fallback(self, tmp_path: Path):
        """_try_download falls back to urllib when httpx/boto3 are absent."""
        from ochre_next.data.resstock import _try_download

        zip_bytes = _make_zip()
        dest = tmp_path / "out.zip"

        fake_resp = io.BytesIO(zip_bytes)
        fake_resp.read = fake_resp.read  # already has .read

        class _FakeUrllibCtx:
            def __enter__(self):
                return fake_resp

            def __exit__(self, *_):
                pass

        with (
            mock.patch.dict(sys.modules, {"httpx": None, "boto3": None}),
            mock.patch("urllib.request.urlopen", return_value=_FakeUrllibCtx()),
        ):
            _try_download("https://example.com/test.zip", dest)

        assert dest.exists()
        assert dest.stat().st_size > 0


# ---------------------------------------------------------------------------
# 6. HPXML weather station parsing
# ---------------------------------------------------------------------------


class TestParseWeatherStation:
    def test_parses_fips_code(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_weather_station

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_minimal_hpxml("G0800130"))
        result = _parse_weather_station(hpxml)
        assert result == ("CO", "G0800130")

    def test_returns_none_for_unknown_fips(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_weather_station

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_minimal_hpxml("UNKNOWN_STATION"))
        result = _parse_weather_station(hpxml)
        assert result is None

    def test_returns_none_for_missing_element(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_weather_station

        hpxml = tmp_path / "home.xml"
        hpxml.write_text("<HPXML><Building/></HPXML>")
        result = _parse_weather_station(hpxml)
        assert result is None

    def test_returns_none_for_bad_xml(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_weather_station

        hpxml = tmp_path / "home.xml"
        hpxml.write_text("not xml at all <<<")
        result = _parse_weather_station(hpxml)
        assert result is None


# ---------------------------------------------------------------------------
# 7. ResStockVersion enum
# ---------------------------------------------------------------------------


class TestResStockVersion:
    def test_values(self):
        from ochre_next.data.resstock import ResStockVersion

        assert ResStockVersion.V2024_1.value == "2024.1"
        assert ResStockVersion.V2024_2.value == "2024.2"
        assert ResStockVersion.V2025_1.value == "2025.1"

    def test_default_version_is_2024_2(self):
        import inspect
        from ochre_next.data.resstock import fetch_resstock_building

        sig = inspect.signature(fetch_resstock_building)
        assert sig.parameters["version"].default == "2024.2"


# ---------------------------------------------------------------------------
# Internal mock helpers
# ---------------------------------------------------------------------------


def _fake_download(zip_bytes: bytes, tmp_path: Path):
    """Return a side_effect that writes zip_bytes to dest for .zip URLs
    and an empty CSV for .csv URLs."""

    def _inner(url: str, dest: Path) -> None:
        dest.parent.mkdir(parents=True, exist_ok=True)
        if url.endswith(".zip"):
            dest.write_bytes(zip_bytes)
        else:
            dest.write_text("fake,weather\n0,0\n")

    return _inner


def _fake_fleet_building(tmp_path: Path):
    """Return a side_effect for fetch_resstock_building that creates fake files."""
    from ochre_next.data.resstock import ResStockBuilding

    def _inner(
        bldg_id: int,
        version: str = "2024.2",
        upgrade_id: int = 0,
        cache_dir: Path | None = None,
        weather_override: Path | None = None,
        weather_format=None,
        **kwargs: object,
    ) -> ResStockBuilding:
        bdir = (cache_dir or tmp_path) / version / f"bldg{bldg_id:07d}"
        bdir.mkdir(parents=True, exist_ok=True)
        h = bdir / "home.xml"
        s = bdir / "in.schedules.csv"
        w = bdir / "weather.csv"
        h.write_text(_minimal_hpxml("G0800130"))
        s.write_text("hour,val\n0,1\n")
        w.write_text("weather\n")
        return ResStockBuilding(
            bldg_id=bldg_id,
            sample_weight=1.0,
            hpxml_path=h,
            schedule_path=s,
            weather_path=w,
        )

    return _inner


# ---------------------------------------------------------------------------
# 8. Climate zone extraction and zone validation
# ---------------------------------------------------------------------------


def _make_hpxml_with_zone(zone: str) -> str:
    return (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">'
        "<Building>"
        "<BuildingDetails>"
        "<ClimateandRiskZones>"
        "<ClimateZoneIECC>"
        "<Year>2006</Year>"
        f"<ClimateZone>{zone}</ClimateZone>"
        "</ClimateZoneIECC>"
        "</ClimateandRiskZones>"
        "</BuildingDetails>"
        "</Building>"
        "</HPXML>"
    )


def _make_epw_file(path: Path, state: str) -> None:
    path.write_text(f"LOCATION,Test,{state},USA,TMY3,999999,39.7,-104.9,-7.0,1600.0")


class TestClimateZoneExtraction:
    def test_parses_iecc_zone(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_climate_zone

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_make_hpxml_with_zone("5B"))
        result = _parse_climate_zone(hpxml)
        assert result == "5B"

    def test_parses_iecc_zone_2a(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_climate_zone

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_make_hpxml_with_zone("2A"))
        result = _parse_climate_zone(hpxml)
        assert result == "2A"

    def test_returns_none_for_missing_zone(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_climate_zone

        hpxml = tmp_path / "home.xml"
        hpxml.write_text("<HPXML><Building/></HPXML>")
        result = _parse_climate_zone(hpxml)
        assert result is None

    def test_returns_none_for_bad_xml(self, tmp_path: Path):
        from ochre_next.data.resstock import _parse_climate_zone

        hpxml = tmp_path / "home.xml"
        hpxml.write_text("not xml <<<")
        result = _parse_climate_zone(hpxml)
        assert result is None


class TestExtractEpwState:
    def test_extracts_co(self, tmp_path: Path):
        from ochre_next.data.resstock import _extract_epw_state

        epw = tmp_path / "weather.epw"
        _make_epw_file(epw, "CO")
        result = _extract_epw_state(epw)
        assert result == "CO"

    def test_returns_none_for_non_epw_extension(self, tmp_path: Path):
        from ochre_next.data.resstock import _extract_epw_state

        csv = tmp_path / "weather.csv"
        csv.write_text("some,data")
        result = _extract_epw_state(csv)
        assert result is None

    def test_returns_none_for_missing_file(self, tmp_path: Path):
        from ochre_next.data.resstock import _extract_epw_state

        result = _extract_epw_state(tmp_path / "nonexistent.epw")
        assert result is None


class TestValidateZoneForState:
    def test_match_co_zone5(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Zone 5B is valid for CO — should not raise or warn.
        _validate_zone_for_state("5B", "CO")

    def test_match_fl_zone1(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Zone 1A is valid for FL — should not raise or warn.
        _validate_zone_for_state("1A", "FL")

    def test_mismatch_warns_close_zone(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Zone 3A in MN (valid zones: 6, 7) — diff=3, should warn not raise.
        with pytest.warns(UserWarning, match="Building IECC zone"):
            _validate_zone_for_state("3A", "MN")

    def test_mismatch_raises_far_zone(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Zone 1A in MN (valid zones: 6, 7) — diff>=4, should raise ValueError.
        with pytest.raises(ValueError, match="incompatible"):
            _validate_zone_for_state("1A", "MN")

    def test_unknown_state_skips(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Unknown state abbreviation — should not raise or warn.
        _validate_zone_for_state("5B", "ZZ")

    def test_invalid_zone_format_skips(self):
        from ochre_next.data.resstock import _validate_zone_for_state

        # Zone that doesn't start with a number — should skip silently.
        _validate_zone_for_state("ABC", "CO")


class TestValidateZoneForWeatherPath:
    def test_epw_match_passes(self, tmp_path: Path):
        from ochre_next.data.resstock import _validate_zone_for_weather_path

        epw = tmp_path / "weather.epw"
        _make_epw_file(epw, "CO")
        # Zone 5B is valid for CO — should not raise or warn.
        _validate_zone_for_weather_path("5B", epw)

    def test_epw_mismatch_warns(self, tmp_path: Path):
        from ochre_next.data.resstock import _validate_zone_for_weather_path

        epw = tmp_path / "weather.epw"
        _make_epw_file(epw, "CO")
        # Zone 2A is not valid for CO (diff=3<4) — should warn.
        with pytest.warns(UserWarning, match="Building IECC zone"):
            _validate_zone_for_weather_path("2A", epw)

    def test_non_epw_skips(self, tmp_path: Path):
        from ochre_next.data.resstock import _validate_zone_for_weather_path

        csv = tmp_path / "weather.csv"
        csv.write_text("some,data")
        # Non-EPW file — should be silently skipped.
        _validate_zone_for_weather_path("1A", csv)


# ---------------------------------------------------------------------------
# 9. Download retry / exponential backoff
# ---------------------------------------------------------------------------


class _FakeHTTPStatusError(Exception):
    """Mimics httpx.HTTPStatusError: an error carrying a response status code."""

    def __init__(self, status_code: int) -> None:
        super().__init__(f"HTTP {status_code}")
        self.response = type("_Resp", (), {"status_code": status_code})()


class _FakeAsyncResponse:
    def __init__(self, *, content: bytes = b"", status_error: Exception | None = None) -> None:
        self.content = content
        self._status_error = status_error

    def raise_for_status(self) -> None:
        if self._status_error is not None:
            raise self._status_error


class _FakeAsyncClient:
    """Async client whose ``get`` replays a queued list of responses/exceptions."""

    def __init__(self, responses: list) -> None:
        self._responses = list(responses)
        self.get_calls = 0

    async def get(self, url: str):
        self.get_calls += 1
        item = self._responses.pop(0)
        if isinstance(item, Exception):
            raise item
        return item


class TestDownloadRetry:
    def test_download_retry_on_503(self, tmp_path: Path):
        """A 503 on the first two attempts followed by a 200 succeeds."""
        from ochre_next.data import resstock

        zip_bytes = _make_zip()
        dest = tmp_path / "out.zip"
        attempts: list[str] = []

        def flaky(url: str, dest_path: Path) -> None:
            attempts.append(url)
            if len(attempts) < 3:
                raise _FakeHTTPStatusError(503)
            dest_path.write_bytes(zip_bytes)

        with (
            mock.patch.object(resstock, "_try_download", side_effect=flaky),
            mock.patch.object(resstock.time, "sleep") as sleep,
        ):
            resstock._download_file("https://oedi/out.zip", dest)

        assert dest.read_bytes() == zip_bytes
        assert len(attempts) == 3
        assert sleep.call_count == 2  # slept before each of the two retries

    def test_download_exhausts_retries(self, tmp_path: Path):
        """Persistent transient failure raises after exactly 3 attempts."""
        from ochre_next.data import resstock

        dest = tmp_path / "out.zip"
        attempts: list[str] = []

        def always_503(url: str, dest_path: Path) -> None:
            attempts.append(url)
            raise _FakeHTTPStatusError(503)

        with (
            mock.patch.object(resstock, "_try_download", side_effect=always_503),
            mock.patch.object(resstock.time, "sleep"),
            pytest.raises(_FakeHTTPStatusError),
        ):
            resstock._download_file("https://oedi/out.zip", dest)

        assert len(attempts) == 3
        assert not dest.exists()
        # .tmp is removed only once retries are exhausted, never left behind.
        assert not (tmp_path / "out.zip.tmp").exists()

    def test_download_no_retry_on_404(self, tmp_path: Path):
        """A permanent 404 fails immediately without retrying."""
        from ochre_next.data import resstock

        dest = tmp_path / "out.zip"
        attempts: list[str] = []

        def always_404(url: str, dest_path: Path) -> None:
            attempts.append(url)
            raise _FakeHTTPStatusError(404)

        with (
            mock.patch.object(resstock, "_try_download", side_effect=always_404),
            mock.patch.object(resstock.time, "sleep") as sleep,
            pytest.raises(_FakeHTTPStatusError),
        ):
            resstock._download_file("https://oedi/out.zip", dest)

        assert len(attempts) == 1  # 404 is permanent — no retry
        assert sleep.call_count == 0
        assert not dest.exists()

    def test_async_download_retries_then_succeeds(self, tmp_path: Path):
        """The async path retries a transient 503 and extracts on success."""
        import asyncio

        from ochre_next.data import resstock

        zip_bytes = _make_zip()
        cfg = resstock._version_config("2024.2")
        bldg_dir = tmp_path / "bldg0000001"
        client = _FakeAsyncClient([
            _FakeAsyncResponse(status_error=_FakeHTTPStatusError(503)),
            _FakeAsyncResponse(status_error=_FakeHTTPStatusError(503)),
            _FakeAsyncResponse(content=zip_bytes),
        ])

        with mock.patch.object(
            resstock.asyncio, "sleep", new_callable=mock.AsyncMock
        ) as sleep:
            asyncio.run(resstock._download_building_async(client, cfg, 1, 0, bldg_dir))

        assert (bldg_dir / "home.xml").exists()
        assert (bldg_dir / "in.schedules.csv").exists()
        assert client.get_calls == 3
        assert sleep.await_count == 2

    def test_async_download_no_retry_on_404(self, tmp_path: Path):
        """The async path fails fast on a permanent 404."""
        import asyncio

        from ochre_next.data import resstock

        cfg = resstock._version_config("2024.2")
        bldg_dir = tmp_path / "bldg0000001"
        client = _FakeAsyncClient([
            _FakeAsyncResponse(status_error=_FakeHTTPStatusError(404)),
        ])

        with (
            mock.patch.object(resstock.asyncio, "sleep", new_callable=mock.AsyncMock) as sleep,
            pytest.raises(_FakeHTTPStatusError),
        ):
            asyncio.run(resstock._download_building_async(client, cfg, 1, 0, bldg_dir))

        assert client.get_calls == 1
        assert sleep.await_count == 0


class TestFleetResilience:
    def _metadata_file(self, tmp_path: Path, bldg_ids: list[int], **kwargs) -> Path:
        data = _make_metadata_parquet(bldg_ids, **kwargs)
        p = tmp_path / "metadata.parquet"
        p.write_bytes(data)
        return p

    def test_fleet_download_continues_after_building_failure(self, tmp_path: Path):
        """One failed building is skipped; the rest of the fleet is returned.

        Forces the httpx-absent synchronous fallback so the behaviour is
        deterministic regardless of whether the optional httpx dependency is
        installed in the test environment.
        """
        from ochre_next.data import resstock

        meta = self._metadata_file(tmp_path, [1, 2, 3])
        good = _fake_fleet_building(tmp_path)

        def flaky(bldg_id: int, **kwargs):
            if bldg_id == 2:
                raise ConnectionError("network down for bldg 2")
            return good(bldg_id, **kwargs)

        with (
            mock.patch.dict(sys.modules, {"httpx": None, "boto3": None}),
            mock.patch.object(resstock, "fetch_resstock_building", side_effect=flaky),
        ):
            results = resstock.fetch_resstock_fleet(
                meta, bldg_ids=[1, 2, 3], cache_dir=tmp_path,
            )

        returned_ids = {r.bldg_id for r in results}
        assert returned_ids == {1, 3}

    def test_fleet_async_download_continues_after_building_failure(self, tmp_path: Path):
        """The async gather tolerates one building's exhausted-retry failure."""
        import asyncio
        import types

        from ochre_next.data import resstock

        cfg = resstock._version_config("2024.2")

        fake_httpx = types.ModuleType("httpx")

        class _FakeAC:
            def __init__(self, **kwargs) -> None:
                pass

            async def __aenter__(self):
                return self

            async def __aexit__(self, *exc):
                return False

        fake_httpx.AsyncClient = _FakeAC  # type: ignore[attr-defined]

        async def fake_download(client, cfg_, bid, upgrade, bdir, **kwargs) -> None:
            if bid == 2:
                raise ConnectionError("boom for bldg 2")
            bdir.mkdir(parents=True, exist_ok=True)
            (bdir / "home.xml").write_text(_minimal_hpxml("G0800130"))
            (bdir / "in.schedules.csv").write_text("hour,val\n0,1\n")

        with (
            mock.patch.dict(sys.modules, {"httpx": fake_httpx}),
            mock.patch.object(resstock, "_download_building_async", side_effect=fake_download),
            mock.patch.object(resstock, "_fetch_weather", return_value=tmp_path / "w.csv"),
        ):
            results = asyncio.run(
                resstock._fetch_fleet_async(
                    [1, 2, 3], cfg, "2024.2", tmp_path, 0,
                    {1: 1.0, 2: 1.0, 3: 1.0},
                )
            )

        returned_ids = {r.bldg_id for r in results}
        assert returned_ids == {1, 3}


# ---------------------------------------------------------------------------
# 10. ZIP integrity checks (testzip)
# ---------------------------------------------------------------------------


class TestZipIntegrity:
    def test_valid_zip_passes_testzip(self, tmp_path: Path):
        """_verify_zip returns without error for a valid ZIP."""
        from ochre_next.data.resstock import _verify_zip

        zip_path = tmp_path / "good.zip"
        zip_bytes = _make_zip()
        zip_path.write_bytes(zip_bytes)

        # Should not raise
        _verify_zip(zip_path)

    def test_truncated_zip_raises(self, tmp_path: Path):
        """_verify_zip raises ZipIntegrityError for a truncated ZIP."""
        from ochre_next.data.resstock import ZipIntegrityError, _verify_zip

        zip_path = tmp_path / "bad.zip"
        zip_bytes = _make_zip()
        truncated = zip_bytes[: len(zip_bytes) // 2]
        zip_path.write_bytes(truncated)

        with pytest.raises(ZipIntegrityError):
            _verify_zip(zip_path)

    def test_empty_file_raises(self, tmp_path: Path):
        """_verify_zip raises ZipIntegrityError for an empty file."""
        from ochre_next.data.resstock import ZipIntegrityError, _verify_zip

        zip_path = tmp_path / "empty.zip"
        zip_path.write_bytes(b"")

        with pytest.raises(ZipIntegrityError):
            _verify_zip(zip_path)

    def test_download_and_extract_valid_zip(self, tmp_path: Path):
        """_download_and_extract_zip downloads a valid ZIP and extracts it."""
        from ochre_next.data.resstock import _download_and_extract_zip

        zip_bytes = _make_zip()
        dest_dir = tmp_path / "extracted"
        url = "https://example.com/building.zip"

        with mock.patch(
            "ochre_next.data.resstock._download_file",
            side_effect=_fake_download(zip_bytes, tmp_path),
        ):
            _download_and_extract_zip(url, dest_dir)

        assert (dest_dir / "home.xml").exists()
        assert (dest_dir / "in.schedules.csv").exists()

    def test_download_and_extract_retries_on_corrupt_zip(self, tmp_path: Path):
        """_download_and_extract_zip retries when verify fails."""
        zip_bytes = _make_zip()
        dest_dir = tmp_path / "extracted"
        url = "https://example.com/building.zip"
        call_count = [0]

        def flaky_download(url_: str, dest: Path) -> None:
            call_count[0] += 1
            if call_count[0] == 1:
                dest.write_bytes(zip_bytes[: len(zip_bytes) // 2])
            else:
                dest.write_bytes(zip_bytes)

        with (
            mock.patch("ochre_next.data.resstock._download_file", side_effect=flaky_download),
            mock.patch("ochre_next.data.resstock.time.sleep"),
        ):
            from ochre_next.data.resstock import _download_and_extract_zip

            _download_and_extract_zip(url, dest_dir)

        assert (dest_dir / "home.xml").exists()
        assert call_count[0] == 2

    def test_async_download_retries_on_corrupt_zip(self, tmp_path: Path):
        """_download_building_async retries on ZIP corruption."""
        import asyncio

        from ochre_next.data import resstock

        zip_bytes = _make_zip()
        cfg = resstock._version_config("2024.2")
        bldg_dir = tmp_path / "bldg0000001"
        truncated = zip_bytes[: len(zip_bytes) // 2]

        # Return truncated data first, then valid ZIP
        call_count = [0]

        async def flaky_download(client, url, *, max_attempts=3):
            call_count[0] += 1
            if call_count[0] == 1:
                return truncated
            return zip_bytes

        with (
            mock.patch.object(resstock, "_download_bytes_async", side_effect=flaky_download),
            mock.patch.object(resstock.asyncio, "sleep", new_callable=mock.AsyncMock) as sleep_mock,
        ):
            asyncio.run(resstock._download_building_async(
                None, cfg, 1, 0, bldg_dir,  # client unused when _download_bytes_async is mocked
            ))

        assert (bldg_dir / "home.xml").exists()
        assert call_count[0] == 2
        assert sleep_mock.await_count >= 1


# ---------------------------------------------------------------------------
# 11. SHA256 sidecar cache integrity
# ---------------------------------------------------------------------------


class TestSha256Sidecar:
    def test_sidecar_hash_matches(self, tmp_path: Path):
        """_validate_cache_integrity returns True when sidecar matches."""
        from ochre_next.data._checksum import (
            compute_sha256_hex,
            sha256_path,
        )
        from ochre_next.data.resstock import _validate_cache_integrity

        f = tmp_path / "data.csv"
        f.write_text("col1,col2\n1.0,2.0\n")
        digest = compute_sha256_hex(f)
        sha256_path(f).write_text(digest + "\n")

        assert _validate_cache_integrity(f) is True

    def test_sidecar_hash_mismatch(self, tmp_path: Path):
        """_validate_cache_integrity returns False when sidecar mismatches."""
        from ochre_next.data._checksum import sha256_path
        from ochre_next.data.resstock import _validate_cache_integrity

        f = tmp_path / "data.csv"
        f.write_text("col1,col2\n1.0,2.0\n")
        sha256_path(f).write_text("deadbeef\n")

        assert _validate_cache_integrity(f) is False

    def test_no_sidecar_returns_true(self, tmp_path: Path):
        """_validate_cache_integrity returns True when no sidecar exists."""
        from ochre_next.data.resstock import _validate_cache_integrity

        f = tmp_path / "data.csv"
        f.write_text("col1,col2\n1.0,2.0\n")

        assert _validate_cache_integrity(f) is True

    def test_remove_cache_with_sidecar(self, tmp_path: Path):
        """_remove_cache_with_sidecar deletes both file and sidecar."""
        from ochre_next.data._checksum import sha256_path
        from ochre_next.data.resstock import _remove_cache_with_sidecar

        f = tmp_path / "data.csv"
        f.write_text("data")
        sha256_path(f).write_text("hash\n")

        _remove_cache_with_sidecar(f)

        assert not f.exists()
        assert not sha256_path(f).exists()


# ---------------------------------------------------------------------------
# 12. Weather CSV SHA256 sidecar cache hit / mismatch
# ---------------------------------------------------------------------------


class TestWeatherCacheIntegrity:
    def test_csv_cache_reused_when_sidecar_matches(self, tmp_path: Path):
        """Weather CSV with valid sidecar is reused without re-download."""
        from ochre_next.data.resstock import _write_sha256_sidecar

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_minimal_hpxml("G0800130"))
        cache_dir = tmp_path / "cache"
        version = "2024.2"
        weather_dest = cache_dir / version / "weather" / "G0800130_TMY3.csv"
        weather_dest.parent.mkdir(parents=True, exist_ok=True)
        weather_dest.write_text("fake,weather\n0,1\n")
        _write_sha256_sidecar(weather_dest)

        download_calls: list[str] = []

        def recording_download(url: str, dest: Path) -> None:
            download_calls.append(url)

        with mock.patch(
            "ochre_next.data.resstock._download_file",
            side_effect=recording_download,
        ):
            from ochre_next.data.resstock import (
                WeatherFormat,
                _fetch_weather,
                _version_config,
            )

            cfg = _version_config(version)
            result = _fetch_weather(
                cfg=cfg,
                hpxml_path=hpxml,
                cache_dir=cache_dir,
                version=version,
                weather_format=WeatherFormat.CSV,
            )

        assert result == weather_dest
        assert len(download_calls) == 0

    def test_csv_re_downloaded_when_sidecar_mismatch(self, tmp_path: Path):
        """Weather CSV with mismatched sidecar triggers re-download."""
        from ochre_next.data._checksum import sha256_path

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_minimal_hpxml("G0800130"))
        cache_dir = tmp_path / "cache"
        version = "2024.2"
        weather_dest = cache_dir / version / "weather" / "G0800130_TMY3.csv"
        weather_dest.parent.mkdir(parents=True, exist_ok=True)
        weather_dest.write_text("fake,weather\n0,1\n")
        sha256_path(weather_dest).write_text("0000000000000000000000000000000000000000\n")

        download_calls: list[Path] = []
        fresh_content = b"col1,col2\n3.0,4.0\n"

        def recording_download(url: str, dest: Path) -> None:
            dest.write_bytes(fresh_content)
            download_calls.append(dest)

        with mock.patch(
            "ochre_next.data.resstock._download_file",
            side_effect=recording_download,
        ):
            from ochre_next.data.resstock import (
                WeatherFormat,
                _fetch_weather,
                _version_config,
            )

            cfg = _version_config(version)
            _fetch_weather(
                cfg=cfg,
                hpxml_path=hpxml,
                cache_dir=cache_dir,
                version=version,
                weather_format=WeatherFormat.CSV,
            )

        assert len(download_calls) == 1
        from ochre_next.data.resstock import _validate_cache_integrity

        assert _validate_cache_integrity(weather_dest) is True

    def test_no_sidecar_with_content_reuses_cache(self, tmp_path: Path):
        """A file with content >0 but no sidecar uses the cache."""
        from ochre_next.data.resstock import (
            WeatherFormat,
            _fetch_weather,
            _version_config,
        )

        hpxml = tmp_path / "home.xml"
        hpxml.write_text(_minimal_hpxml("G0800130"))
        cache_dir = tmp_path / "cache"
        version = "2024.2"
        weather_dest = cache_dir / version / "weather" / "G0800130_TMY3.csv"
        weather_dest.parent.mkdir(parents=True, exist_ok=True)
        weather_dest.write_text("old,data\n0,0\n")

        download_calls: list[str] = []

        def recording_download(url: str, dest: Path) -> None:
            download_calls.append(url)

        with mock.patch(
            "ochre_next.data.resstock._download_file",
            side_effect=recording_download,
        ):
            cfg = _version_config(version)
            result = _fetch_weather(
                cfg=cfg,
                hpxml_path=hpxml,
                cache_dir=cache_dir,
                version=version,
                weather_format=WeatherFormat.CSV,
            )

        assert result == weather_dest
        assert len(download_calls) == 0


# ---------------------------------------------------------------------------
# 13. Weather EPW ZIP integrity and sidecar tests
# ---------------------------------------------------------------------------


class TestWeatherEpwIntegrity:
    def test_epw_cache_hit_with_valid_sidecar(self, tmp_path: Path):
        """get_epw_for_fips reuses cached EPW when SHA256 sidecar matches."""
        from ochre_next.data._checksum import compute_sha256_hex, sha256_path
        from ochre_next.data.weather import get_epw_for_fips

        epw_dir = tmp_path / "BuildStock_TMY3_FIPS"
        epw_dir.mkdir(parents=True)
        epw_path = epw_dir / "G0800130.epw"
        epw_path.write_text("LOCATION,City,CO,USA,TMY3,999999,39,-104,-7,1600\n")
        digest = compute_sha256_hex(epw_path)
        sha256_path(epw_path).write_text(digest + "\n")

        result = get_epw_for_fips("G0800130", cache_dir=tmp_path)
        assert result == epw_path

    def test_epw_cache_mismatch_triggers_removal(self, tmp_path: Path):
        """get_epw_for_fips removes corrupted EPW when sidecar mismatches."""
        from ochre_next.data._checksum import sha256_path
        from ochre_next.data.weather import get_epw_for_fips

        epw_dir = tmp_path / "BuildStock_TMY3_FIPS"
        epw_dir.mkdir(parents=True)
        epw_path = epw_dir / "G0800130.epw"
        epw_path.write_text("LOCATION,City,CO,USA,TMY3,999999,39,-104,-7,1600\n")
        sha256_path(epw_path).write_text("0000000000000000000000000000000000000000\n")

        with mock.patch(
            "ochre_next.data.weather._ensure_tmy3_zip_extracted",
            side_effect=lambda epw_d, cache_d: epw_path.write_text(
                "LOCATION,City,CO,USA,TMY3,999999,39,-104,-7,1600\n"
            ),
        ):
            result = get_epw_for_fips("G0800130", cache_dir=tmp_path)

        assert result == epw_path
        assert result.exists()

    def test_tmy3_zip_testzip_retries(self, tmp_path: Path):
        """_ensure_tmy3_zip_extracted retries ZIP download on corruption."""
        from ochre_next.data import weather

        epw_dir = tmp_path / "BuildStock_TMY3_FIPS"
        zip_bytes = _make_zip()
        truncated = zip_bytes[: len(zip_bytes) // 2]
        download_attempts = [0]

        def flaky_download(url: str, dest: Path) -> None:
            download_attempts[0] += 1
            if download_attempts[0] == 1:
                dest.write_bytes(truncated)
            else:
                dest.write_bytes(zip_bytes)

        with (
            mock.patch.object(weather, "_download_large_file", side_effect=flaky_download),
            mock.patch.object(weather.time, "sleep"),
        ):
            weather._ensure_tmy3_zip_extracted(epw_dir, tmp_path)

        marker = epw_dir / ".extracted"
        assert marker.exists()
        assert download_attempts[0] == 2


# ---------------------------------------------------------------------------
# 14. Building ZIP cache integrity (sidecar validation on cache hit)
# ---------------------------------------------------------------------------


class TestBuildingCacheIntegrity:
    def test_sidecars_written_after_download_and_extract(self, tmp_path: Path):
        """_download_and_extract_zip writes .sha256 sidecars for extracted files."""
        from ochre_next.data._checksum import sha256_path, validate_cache_integrity
        from ochre_next.data.resstock import _download_and_extract_zip

        zip_bytes = _make_zip()
        dest_dir = tmp_path / "extracted"

        with mock.patch(
            "ochre_next.data.resstock._download_file",
            side_effect=_fake_download(zip_bytes, tmp_path),
        ):
            _download_and_extract_zip("https://example.com/b.zip", dest_dir)

        hpxml_path = dest_dir / "home.xml"
        schedule_path = dest_dir / "in.schedules.csv"
        assert hpxml_path.exists()
        assert schedule_path.exists()
        assert sha256_path(hpxml_path).exists()
        assert sha256_path(schedule_path).exists()
        assert validate_cache_integrity(hpxml_path) is True
        assert validate_cache_integrity(schedule_path) is True

    def test_cached_building_reused_when_sidecars_valid(self, tmp_path: Path):
        """fetch_resstock_building skips re-download when cached files pass SHA256."""
        from ochre_next.data._checksum import write_sha256_sidecar
        from ochre_next.data.resstock import fetch_resstock_building

        bldg_dir = tmp_path / "2024.2" / "bldg0000001"
        bldg_dir.mkdir(parents=True)
        hpxml_path = bldg_dir / "home.xml"
        schedule_path = bldg_dir / "in.schedules.csv"
        hpxml_path.write_text(_minimal_hpxml("G0800130"))
        schedule_path.write_text("hour,val\n0,1\n")
        write_sha256_sidecar(hpxml_path)
        write_sha256_sidecar(schedule_path)

        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("LOCATION,City,CO,USA,TMY3,999999,39,-104,-7,1600\n")

        download_calls: list[str] = []

        def record_download(url: str, dest: Path) -> None:
            download_calls.append(url)

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=record_download,
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path)

        assert result.bldg_id == 1
        # No ZIP download should have occurred — cache hit.
        zip_urls = [u for u in download_calls if u.endswith(".zip")]
        assert len(zip_urls) == 0

    def test_corrupted_building_cache_detected_and_redownloaded(self, tmp_path: Path):
        """fetch_resstock_building re-downloads when cached files fail SHA256."""
        from ochre_next.data.resstock import fetch_resstock_building

        zip_bytes = _make_zip(
            hpxml_content=_minimal_hpxml("G0800130"),
            schedule_content="hour,val\n0,1\n",
        )
        bldg_dir = tmp_path / "2024.2" / "bldg0000001"
        bldg_dir.mkdir(parents=True)
        hpxml_path = bldg_dir / "home.xml"
        schedule_path = bldg_dir / "in.schedules.csv"

        # Write corrupted files with mismatched sidecars.
        import hashlib

        hpxml_path.write_text("corrupted xml <<<")
        hpxml_path.with_suffix(".xml.sha256").write_text(
            hashlib.sha256(b"corrupted xml <<<").hexdigest() + "\n"
        )
        schedule_path.write_text("corrupted csv <<<")
        schedule_path.with_suffix(".csv.sha256").write_text(
            "0000000000000000000000000000000000000000\n"
        )

        dummy_epw = tmp_path / "dummy.epw"
        dummy_epw.write_text("LOCATION,City,CO,USA,TMY3,999999,39,-104,-7,1600\n")

        download_calls: list[str] = []

        def record_download(url: str, dest: Path) -> None:
            download_calls.append(url)
            # Write fresh content to dest for extraction.
            _fake_download(zip_bytes, tmp_path)(url, dest)

        with (
            mock.patch(
                "ochre_next.data.resstock._download_file",
                side_effect=record_download,
            ),
            mock.patch(
                "ochre_next.data.weather.get_epw_for_fips",
                return_value=dummy_epw,
            ),
        ):
            result = fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path)

        assert result.bldg_id == 1
        assert result.hpxml_path.exists()
        # Should have re-downloaded (one ZIP download).
        zip_urls = [u for u in download_calls if u.endswith(".zip")]
        assert len(zip_urls) == 1
        # After re-download, sidecars should be valid.
        from ochre_next.data._checksum import validate_cache_integrity

        assert validate_cache_integrity(result.hpxml_path) is True
        assert validate_cache_integrity(result.schedule_path) is True

