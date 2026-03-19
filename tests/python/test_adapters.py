"""Tests for ochre_next.adapters (HARES-053)."""

from __future__ import annotations

import sys
import types
from pathlib import Path
from unittest import mock

import pyarrow.parquet as pq
import pytest


@pytest.fixture(autouse=True)
def _fake_hares():
    """Install a minimal fake ``ochre_next._hares`` so the package can import
    even when the Rust extension is not available."""
    key = "ochre_next._hares"
    already = key in sys.modules
    if not already:
        fake_mod = types.ModuleType(key)
        fake_mod.PyDwelling = type("FakePyDwelling", (), {})
        fake_mod.PyFleet = type("FakePyFleet", (), {})
        fake_mod.PyControlSignal = type("FakePyControlSignal", (), {})
        sys.modules[key] = fake_mod
    # Clear cached ochre_next package so it re-imports with the fake
    for mod_key in list(sys.modules):
        if mod_key.startswith("ochre_next") and mod_key != key:
            del sys.modules[mod_key]
    yield
    # Cleanup: remove adapter modules so they are fresh for each test
    for mod_key in list(sys.modules):
        if mod_key.startswith("ochre_next") and mod_key != key:
            del sys.modules[mod_key]


# ---------------------------------------------------------------------------
# SAM PV adapter
# ---------------------------------------------------------------------------


class TestSamPvGeneratePvLut:
    """Tests for sam_pv.generate_pv_lut."""

    def _make_weather_file(self, tmp_path: Path) -> Path:
        weather = tmp_path / "test.epw"
        weather.write_text("fake epw content for testing\n")
        return weather

    def _mock_pvwatts_output(self) -> dict:
        n = 8760
        return {
            "ac": [1000.0 + (i % 24) * 50.0 for i in range(n)],
            "gh": [0.0 + (i % 24) * 40.0 for i in range(n)],
            "dn": [0.0 + (i % 24) * 30.0 for i in range(n)],
            "df": [0.0 + (i % 24) * 20.0 for i in range(n)],
            "tamb": [15.0 + (i % 24) * 0.5 for i in range(n)],
        }

    def test_generate_returns_pvlut_with_correct_schema(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output):
            lut = sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
            )

        assert isinstance(lut, sam_pv.PvLut)
        expected_columns = {"month", "hour", "ghi", "dni", "dhi", "temp_c", "ac_power_kw"}
        assert set(lut.table.column_names) == expected_columns

    def test_save_writes_parquet(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output):
            lut = sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
            )

        out_path = tmp_path / "pv_lut.parquet"
        lut.save(out_path)
        assert out_path.exists()

        table = pq.read_table(out_path)
        assert "ac_power_kw" in table.column_names

    def test_cache_hit_skips_pysam(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()
        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output) as mock_run:
            sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 1

            lut2 = sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 1
            assert lut2.metadata.get("cached") is True

    def test_changed_input_invalidates_cache(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()
        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output) as mock_run:
            sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 1

            sam_pv.generate_pv_lut(
                system_capacity_kw=10.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 2

    def test_changed_weather_file_invalidates_cache(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()
        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output) as mock_run:
            sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 1

            weather.write_text("different weather content\n")
            sam_pv.generate_pv_lut(
                system_capacity_kw=5.0,
                tilt=30.0,
                azimuth=180.0,
                module_type=0,
                array_type=0,
                weather_file=weather,
                cache_dir=cache_dir,
            )
            assert mock_run.call_count == 2


# ---------------------------------------------------------------------------
# SAM Battery adapter
# ---------------------------------------------------------------------------


class TestSamBatteryExtractCellParams:
    """Tests for sam_battery.extract_cell_params."""

    def test_returns_cellparams_with_defaults(self) -> None:
        from ochre_next.adapters import sam_battery

        result = sam_battery.extract_cell_params("LFP", 10.0, source="defaults")
        assert isinstance(result, sam_battery.CellParams)
        assert result.params["cell"]["chemistry"] == "LFP"
        assert "v_nominal" in result.params["cell"]
        assert "ah_rated" in result.params["cell"]
        assert "r_internal_ohm" in result.params["cell"]
        assert "n_series" in result.params["cell"]
        assert "n_parallel" in result.params["cell"]

    def test_save_writes_toml(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_battery

        result = sam_battery.extract_cell_params("NMC", 10.0, source="defaults")
        out_path = tmp_path / "cell_params.toml"
        result.save(out_path)
        assert out_path.exists()

        text = out_path.read_text()
        assert "[cell]" in text
        assert "v_nominal" in text
        assert "[soc_ocv]" in text
        assert "[thermal]" in text
        assert "[losses]" in text
        assert "[degradation]" in text

    def test_toml_contains_all_required_keys(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_battery

        result = sam_battery.extract_cell_params("NMC", 10.0, source="defaults")
        out_path = tmp_path / "cell_params.toml"
        result.save(out_path)

        import tomllib

        data = tomllib.loads(out_path.read_text())
        assert "cell" in data
        for key in ("v_nominal", "ah_rated", "r_internal_ohm", "n_series", "n_parallel"):
            assert key in data["cell"], f"Missing key: cell.{key}"
        assert "soc_ocv" in data
        assert "soc" in data["soc_ocv"]
        assert "v_oc" in data["soc_ocv"]
        assert "thermal" in data
        assert "losses" in data
        assert "degradation" in data

    def test_cache_hit(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_battery

        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        r1 = sam_battery.extract_cell_params("NMC", 10.0, source="defaults", cache_dir=cache_dir)
        assert r1.metadata.get("cached") is False

        r2 = sam_battery.extract_cell_params("NMC", 10.0, source="defaults", cache_dir=cache_dir)
        assert r2.metadata.get("cached") is True

    def test_different_chemistry_invalidates_cache(self, tmp_path: Path) -> None:
        from ochre_next.adapters import sam_battery

        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        r1 = sam_battery.extract_cell_params("NMC", 10.0, source="defaults", cache_dir=cache_dir)
        assert r1.params["cell"]["chemistry"] == "NMC"

        r2 = sam_battery.extract_cell_params("LFP", 10.0, source="defaults", cache_dir=cache_dir)
        assert r2.metadata.get("cached") is False

    def test_unknown_chemistry_falls_back_to_nmc(self) -> None:
        from ochre_next.adapters import sam_battery

        result = sam_battery.extract_cell_params("UNKNOWN", 10.0, source="defaults")
        assert result.params["cell"]["chemistry"] == "UNKNOWN"
        assert result.params["cell"]["v_nominal"] == 3.6


# ---------------------------------------------------------------------------
# PyBaMM Battery adapter
# ---------------------------------------------------------------------------


class TestPybammBatteryEfficiencyLut:
    """Tests for pybamm_battery.generate_efficiency_lut."""

    def test_returns_efficiency_lut_with_defaults(self) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="NMC",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        assert isinstance(lut, pybamm_battery.EfficiencyLut)
        expected_columns = {"soc", "power_kw", "temperature_c", "efficiency"}
        assert set(lut.table.column_names) == expected_columns
        assert lut.table.num_rows > 0

    def test_save_writes_parquet(self, tmp_path: Path) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="NMC",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        out_path = tmp_path / "eff_lut.parquet"
        lut.save(out_path)
        assert out_path.exists()

        table = pq.read_table(out_path)
        assert "efficiency" in table.column_names

    def test_power_range_kw_is_required_keyword(self) -> None:
        from ochre_next.adapters import pybamm_battery

        with pytest.raises(TypeError):
            pybamm_battery.generate_efficiency_lut(
                "NMC", 50, 14, 4, (0, 45), (0.1, 0.95), (-5, 5), 0  # type: ignore[misc]
            )

    def test_without_pybamm_returns_defaults(self) -> None:
        from ochre_next.adapters import pybamm_battery

        original = pybamm_battery._HAS_PYBAMM
        try:
            pybamm_battery._HAS_PYBAMM = False
            lut = pybamm_battery.generate_efficiency_lut(
                chemistry="NMC",
                capacity_ah=50,
                n_series=14,
                n_parallel=4,
                temperature_range_c=(0, 45),
                soc_range=(0.1, 0.95),
                power_range_kw=(-5, 5),
                age_cycles=0,
            )
            assert isinstance(lut, pybamm_battery.EfficiencyLut)
            assert lut.metadata.get("source") == "defaults"
            assert lut.table.num_rows > 0
        finally:
            pybamm_battery._HAS_PYBAMM = original


class TestPybammBatteryDegradationParams:
    """Tests for pybamm_battery.generate_degradation_params."""

    def test_returns_degradation_params(self) -> None:
        from ochre_next.adapters import pybamm_battery

        result = pybamm_battery.generate_degradation_params(
            chemistry="NMC",
            capacity_ah=50,
            temperature_range_c=25.0,
        )
        assert isinstance(result, pybamm_battery.DegradationParams)
        assert "model" in result.params

    def test_save_writes_toml(self, tmp_path: Path) -> None:
        from ochre_next.adapters import pybamm_battery

        result = pybamm_battery.generate_degradation_params(
            chemistry="NMC",
            capacity_ah=50,
            temperature_range_c=25.0,
        )
        out_path = tmp_path / "degradation.toml"
        result.save(out_path)
        assert out_path.exists()
        text = out_path.read_text()
        assert "model" in text

    def test_without_pybamm_returns_defaults(self) -> None:
        from ochre_next.adapters import pybamm_battery

        original = pybamm_battery._HAS_PYBAMM
        try:
            pybamm_battery._HAS_PYBAMM = False
            result = pybamm_battery.generate_degradation_params(
                chemistry="NMC",
                capacity_ah=50,
                temperature_range_c=25.0,
            )
            assert isinstance(result, pybamm_battery.DegradationParams)
            assert "model" in result.params
        finally:
            pybamm_battery._HAS_PYBAMM = original


# ---------------------------------------------------------------------------
# Fallback chain (__init__)
# ---------------------------------------------------------------------------


class TestFallbackChain:
    """Tests for adapters.__init__.resolve_battery_params."""

    def test_resolve_without_external_tools(self) -> None:
        from ochre_next.adapters import resolve_battery_params

        result = resolve_battery_params("NMC", 10.0)
        assert result.params["cell"]["chemistry"] == "NMC"
        assert "v_nominal" in result.params["cell"]

    def test_resolve_with_cache(self, tmp_path: Path) -> None:
        from ochre_next.adapters import resolve_battery_params

        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        r1 = resolve_battery_params("LFP", 10.0, cache_dir=cache_dir)
        assert r1.params["cell"]["chemistry"] == "LFP"

    def test_reexports(self) -> None:
        from ochre_next import adapters

        assert callable(adapters.generate_pv_lut)
        assert callable(adapters.extract_cell_params)
        assert callable(adapters.generate_efficiency_lut)
        assert callable(adapters.generate_degradation_params)
        assert adapters.CellParams is not None
        assert adapters.DegradationParams is not None
        assert adapters.EfficiencyLut is not None
        assert adapters.PvLut is not None
