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
# 3. fetch_resstock_building — basic download + extract
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

        # V2024.2 uses EPW format (TMY3) — weather is fetched via get_epw_for_fips
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
        zip_bytes = _make_zip()

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
        import urllib.request as _ur

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
            _try_download(f"https://example.com/test.zip", dest)

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
