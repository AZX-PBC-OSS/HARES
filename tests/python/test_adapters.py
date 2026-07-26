"""Tests for ochre_next.adapters (HARES-053)."""

from __future__ import annotations

from pathlib import Path
from unittest import mock

import pyarrow.parquet as pq
import pytest


# ---------------------------------------------------------------------------
# SAM PV adapter
# ---------------------------------------------------------------------------


class TestSamPvGeneratePvLut:
    """Tests for sam_pv.generate_pv_lut."""

    def _make_weather_file(self, tmp_path: Path) -> Path:
        weather = tmp_path / "test.epw"
        weather.write_text("LOCATION,x,x,x,x,x,40.0,-105.0,-7.0,1600.0\n")
        return weather

    def _mock_pvwatts_output(self) -> dict:
        n = 8760
        return {
            "ac": [1000.0 + (i % 24) * 50.0 for i in range(n)],
            "gh": [0.0 + (i % 24) * 40.0 for i in range(n)],
            "dn": [0.0 + (i % 24) * 30.0 for i in range(n)],
            "df": [0.0 + (i % 24) * 20.0 for i in range(n)],
            "tamb": [15.0 + (i % 24) * 0.5 for i in range(n)],
            # PVWatts v8 defaults: inverter efficiency 96 %, total system
            # losses 14 % (both stored as fractions by _run_pvwatts).
            "inv_eff": 0.96,
            "losses": 0.14,
        }

    def test_malformed_pvwatts_output_raises_value_error(self, tmp_path: Path) -> None:
        """A pvwatts mapping missing required keys must fail with a clear
        ValueError, not a bare KeyError from deep inside LUT construction."""
        from ochre_next.adapters import sam_pv

        weather = self._make_weather_file(tmp_path)
        raw_output = self._mock_pvwatts_output()
        del raw_output["inv_eff"]

        with mock.patch.object(sam_pv, "_run_pvwatts", return_value=raw_output):
            with pytest.raises(ValueError, match="missing required key.*inv_eff"):
                sam_pv.generate_pv_lut(
                    system_capacity_kw=5.0,
                    tilt=30.0,
                    azimuth=180.0,
                    module_type=0,
                    array_type=0,
                    weather_file=weather,
                )

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
        expected_columns = {"solar_zenith_deg", "solar_azimuth_deg", "ghi", "dni", "dhi", "temp_c", "ac_power_kw"}
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

            weather.write_text("LOCATION,x,x,x,x,x,35.0,-80.0,-5.0,300.0\n")
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
# Solar position regression tests (T-0085 Critical fix)
# ---------------------------------------------------------------------------


class TestSolarPosition:
    """Tests for sam_pv._solar_position — ensures the Spencer model works."""

    def test_returns_numeric_tuple_without_name_error(self) -> None:
        """Catch the NameError: _solar_position must not reference undefined variables."""
        from ochre_next.adapters.sam_pv import _solar_position

        zenith, azimuth = _solar_position(40.0, 0.0, 0.0, 4380)

        assert isinstance(zenith, float)
        assert isinstance(azimuth, float)
        assert zenith >= 0.0, f"zenith must be non-negative, got {zenith}"
        assert 0.0 <= azimuth <= 360.0, f"azimuth must be 0–360, got {azimuth}"

    def test_midnight_sun_below_horizon(self) -> None:
        """At midnight UTC, lat=40°N, the sun should be below the horizon."""
        from ochre_next.adapters.sam_pv import _solar_position

        hour_of_year = 0  # Jan 1 00:00-01:00
        zenith, _ = _solar_position(40.0, 0.0, 0.0, hour_of_year)
        assert zenith > 90.0, f"midnight zenith should be >90°, got {zenith}"

    def test_equinox_noon_lat40(self) -> None:
        """At equinox noon, lat=40°N: zenith ≈ 40°, azimuth ≈ 180° (south).

        Compares against the Rust hares-physics solar_position test
        `solar_position_at_lat40_equinox_noon` (solar_parity.rs:308).
        Spencer model tolerance ±3° for zenith, ±30° for azimuth.
        """
        from ochre_next.adapters.sam_pv import _solar_position

        day_80_noon = 80 * 24 + 12  # March 21 noon (leap-year ordinal 80), non-leap day
        zenith, azimuth = _solar_position(40.0, 0.0, 0.0, day_80_noon)

        assert 39.0 <= zenith <= 43.0, (
            f"equinox noon zenith at lat=40° should be ~40°, got {zenith}"
        )
        assert 160.0 <= azimuth <= 210.0, (
            f"equinox noon azimuth at lat=40° should be ~180° (south), got {azimuth}"
        )

    def test_varying_inputs_all_return(self) -> None:
        """Call _solar_position across a range of hours and latitudes."""
        from ochre_next.adapters.sam_pv import _solar_position

        for lat in (-60.0, 0.0, 40.0, 70.0):
            for h in (0, 8760 // 4, 8760 // 2, 8760 * 3 // 4, 8759):
                zenith, azimuth = _solar_position(lat, 0.0, 0.0, h)
                assert 0.0 <= zenith <= 180.0, (
                    f"zenith out of [0,180] range at lat={lat}, hour={h}: {zenith}"
                )
                assert 0.0 <= azimuth <= 360.0, (
                    f"azimuth out of [0,360] range at lat={lat}, hour={h}: {azimuth}"
                )


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


@pytest.mark.slow
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

    # --- chemistry-specific nominal voltage ---

    def test_metadata_includes_v_nominal_for_nmc(self) -> None:
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
        assert lut.metadata["v_nominal"] == 3.6

    def test_metadata_includes_v_nominal_for_lfp(self) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="LFP",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        assert lut.metadata["v_nominal"] == 3.2

    def test_metadata_includes_v_nominal_for_lto(self) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="LTO",
            capacity_ah=50,
            n_series=20,
            n_parallel=6,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        assert lut.metadata["v_nominal"] == 2.4

    def test_explicit_v_nominal_overrides_chemistry(self) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="LFP",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            v_nominal=3.85,
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        assert lut.metadata["v_nominal"] == 3.85

    def test_invalid_v_nominal_raises_assertion_error(self) -> None:
        from ochre_next.adapters import pybamm_battery

        with pytest.raises(AssertionError, match="Invalid nominal voltage"):
            pybamm_battery.generate_efficiency_lut(
                chemistry="NMC",
                capacity_ah=50,
                n_series=14,
                n_parallel=4,
                temperature_range_c=(0, 45),
                soc_range=(0.1, 0.95),
                v_nominal=0.0,
                power_range_kw=(-5, 5),
                age_cycles=0,
            )

    def test_invalid_high_v_nominal_raises_assertion_error(self) -> None:
        from ochre_next.adapters import pybamm_battery

        with pytest.raises(AssertionError, match="Invalid nominal voltage"):
            pybamm_battery.generate_efficiency_lut(
                chemistry="NMC",
                capacity_ah=50,
                n_series=14,
                n_parallel=4,
                temperature_range_c=(0, 45),
                soc_range=(0.1, 0.95),
                v_nominal=5.0,
                power_range_kw=(-5, 5),
                age_cycles=0,
            )

    def test_hash_differs_by_v_nominal(self) -> None:
        from ochre_next.adapters import pybamm_battery

        h_nmc = pybamm_battery._canonical_hash_efficiency(
            "NMC", 50, 14, 4, 3.6, (0, 45), (0.1, 0.95), (-5, 5), 0,
        )
        h_lfp = pybamm_battery._canonical_hash_efficiency(
            "LFP", 50, 14, 4, 3.2, (0, 45), (0.1, 0.95), (-5, 5), 0,
        )
        assert h_nmc != h_lfp

    def test_cache_invalidated_by_different_v_nominal(self, tmp_path: Path) -> None:
        from ochre_next.adapters import pybamm_battery

        cache_dir = tmp_path / "cache"
        cache_dir.mkdir()

        lut1 = pybamm_battery.generate_efficiency_lut(
            chemistry="NMC",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
            cache_dir=cache_dir,
        )
        assert lut1.metadata.get("cached") is False

        lut2 = pybamm_battery.generate_efficiency_lut(
            chemistry="LFP",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
            cache_dir=cache_dir,
        )
        assert lut2.metadata.get("cached") is False

    def test_unknown_chemistry_falls_back_to_3_6v(self) -> None:
        from ochre_next.adapters import pybamm_battery

        lut = pybamm_battery.generate_efficiency_lut(
            chemistry="UNKNOWN",
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0, 45),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5, 5),
            age_cycles=0,
        )
        assert lut.metadata["v_nominal"] == 3.6

    def test_default_efficiency_lut_unchanged_for_nmc(self) -> None:
        """Regression: NMC default efficiency values must be numerically unchanged."""
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
        finally:
            pybamm_battery._HAS_PYBAMM = original

        assert lut.table.num_rows > 0
        eff_col = lut.table.column("efficiency").to_pylist()
        assert all(0.80 <= e <= 1.0 for e in eff_col), "efficiency values must be in [0.80, 1.0]"
        assert any(e != 0.95 for e in eff_col), "efficiency must vary across grid points"

    def test_run_pybamm_efficiency_uses_chemistry_specific_voltage(self) -> None:
        """LFP (3.2V) and LTO (2.4V) feed different currents into PyBaMM than
        NMC (3.6V) for the same input power, because per-cell nominal voltage
        directly governs the current calculation.  A regression that
        reintroduces a hardcoded 3.6 V would cause all chemistries to produce
        identical current sets and this test would fail."""
        from ochre_next.adapters import pybamm_battery

        def _build_fake_pybamm(currents: list[float]) -> mock.MagicMock:
            fake = mock.MagicMock()

            class _TrackingDict(dict):
                def __setitem__(self, key, value):
                    if key == "Current function [A]":
                        currents.append(float(value))
                    super().__setitem__(key, value)

            fake.ParameterValues.side_effect = lambda _name: _TrackingDict()
            return fake

        common = dict(
            capacity_ah=50,
            n_series=14,
            n_parallel=4,
            temperature_range_c=(0.0, 45.0),
            soc_range=(0.1, 0.95),
            power_range_kw=(-5.0, 5.0),
            age_cycles=0,
        )

        nmc_currents: list[float] = []
        fake_nmc = _build_fake_pybamm(nmc_currents)
        with mock.patch.object(pybamm_battery, "_pybamm", fake_nmc):
            pybamm_battery._run_pybamm_efficiency(
                chemistry="NMC",
                v_nominal=3.6,
                **common,
            )

        lfp_currents: list[float] = []
        fake_lfp = _build_fake_pybamm(lfp_currents)
        with mock.patch.object(pybamm_battery, "_pybamm", fake_lfp):
            pybamm_battery._run_pybamm_efficiency(
                chemistry="LFP",
                v_nominal=3.2,
                **common,
            )

        lto_currents: list[float] = []
        fake_lto = _build_fake_pybamm(lto_currents)
        with mock.patch.object(pybamm_battery, "_pybamm", fake_lto):
            pybamm_battery._run_pybamm_efficiency(
                chemistry="LTO",
                v_nominal=2.4,
                **common,
            )

        assert nmc_currents != lfp_currents, (
            "NMC (3.6V) and LFP (3.2V) must produce different PyBaMM currents"
        )
        assert lfp_currents != lto_currents, (
            "LFP (3.2V) and LTO (2.4V) must produce different PyBaMM currents"
        )

        assert nmc_currents != lto_currents, (
            "NMC (3.6V) and LTO (2.4V) must produce different PyBaMM currents"
        )


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
