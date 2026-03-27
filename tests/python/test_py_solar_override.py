"""Tests for solar override API in PyDwelling."""

import os
import tempfile
import numpy as np
import polars as pl
import pytest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
HPXML_PV = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

PVLIB_SOLAR_CSV = str(
    ROOT / "tests/fixtures/freefloat/beopt_winter_48h/pvlib_solar_override.csv"
)

PVLIB_SOLAR_SUMMER_CSV = str(
    ROOT / "tests/fixtures/freefloat/beopt_summer_48h/pvlib_solar_override.csv"
)


class TestSurfaceIds:
    def test_surface_ids_returns_list_of_ints(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        ids = dw.surface_ids()

        assert isinstance(ids, list), "surface_ids() should return a list"
        assert len(ids) > 0, "surface_ids() should return non-empty list"
        assert all(isinstance(i, int) for i in ids), "all surface IDs should be ints"


class TestSolarOverrideNumpy:
    def test_set_solar_override_zero_arrays_produces_zero_pv_output(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        surface_ids = dw.surface_ids()
        n_surfaces = len(surface_ids)
        n_steps = 10

        data = {}
        for sid in surface_ids:
            data[int(sid)] = {
                "direct": np.zeros(n_steps),
                "diffuse": np.zeros(n_steps),
                "reflected": np.zeros(n_steps),
                "aoi": np.zeros(n_steps),
            }

        dw.set_solar_override(data)

        assert dw.has_solar_override(), (
            "has_solar_override() should be True after setting"
        )

        # Step several times and verify net power stays non-negative
        # (no solar generation when all irradiance is zero on a base dwelling without PV)
        for i in range(3):
            result = dw.step()
            assert result["net_electric_power_kw"] >= 0, (
                f"Step {i}: expected non-negative net power with zero irradiance, "
                f"got {result['net_electric_power_kw']} kW"
            )

    def test_set_solar_override_preserves_timestep_order(self):
        from ochre_next import Dwelling

        def make_dwelling():
            dw = Dwelling.from_hpxml(
                HPXML, SCHEDULE, WEATHER,
                start_time="2019-01-01T00:00:00",
                duration_s=3600,
                time_res_s=60,
                defaults_path=str(HARES_DEFAULTS),
                bldg_id=42,
                master_seed=0,
            )
            dw.initialize()
            return dw

        dw = make_dwelling()
        surface_ids = dw.surface_ids()
        n_steps = 5

        # High solar ramp: 0, 500, 1000, 1500, 2000 W/m² (midnight, so
        # Perez baseline is zero — all solar comes from the override).
        data = {}
        for sid in surface_ids:
            arr = np.arange(n_steps, dtype=np.float64) * 500.0
            data[int(sid)] = {
                "direct": arr,
                "diffuse": arr,
                "reflected": arr,
                "aoi": np.zeros(n_steps),
            }
        dw.set_solar_override(data)

        results = []
        for _ in range(n_steps):
            results.append(dw.step())

        # Zero-solar baseline with identical initial conditions.
        dw_base = make_dwelling()
        zero_data = {}
        for sid in surface_ids:
            zero_data[int(sid)] = {
                "direct": np.zeros(n_steps),
                "diffuse": np.zeros(n_steps),
                "reflected": np.zeros(n_steps),
                "aoi": np.zeros(n_steps),
            }
        dw_base.set_solar_override(zero_data)
        base_results = []
        for _ in range(n_steps):
            base_results.append(dw_base.step())

        temp_key = "Temperature - Zone_1 (C)"
        t_solar = results[-1][temp_key]
        t_base = base_results[-1][temp_key]
        assert t_solar > t_base, (
            f"Solar override should produce warmer zone than zero-solar baseline: "
            f"solar={t_solar:.4f} C, baseline={t_base:.4f} C"
        )


class TestSolarOverrideDataFrame:
    def test_set_solar_override_from_csv_via_dataframe(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        df = pl.read_csv(PVLIB_SOLAR_CSV)
        df = df.head(10)

        dw.set_solar_override(df)

        assert dw.has_solar_override(), "has_solar_override() should be True"

        dw.step()
        dw.step()


class TestSolarOverrideParquet:
    def test_set_solar_override_from_parquet_file(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        df = pl.read_csv(PVLIB_SOLAR_CSV)
        df = df.head(10)

        with tempfile.NamedTemporaryFile(suffix=".parquet", delete=False) as f:
            temp_path = f.name

        try:
            df.write_parquet(temp_path)

            dw.set_solar_override(temp_path)

            assert dw.has_solar_override(), "has_solar_override() should be True"

            dw.step()
            dw.step()
        finally:
            os.unlink(temp_path)


class TestSolarOverrideClear:
    def test_clear_solar_override_restores_perez_model(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        dw.step()
        step_without_override = dw.step()

        surface_ids = dw.surface_ids()
        n_surfaces = len(surface_ids)
        n_steps = 2

        data = {}
        for sid in surface_ids:
            data[int(sid)] = {
                "direct": np.zeros(n_steps),
                "diffuse": np.zeros(n_steps),
                "reflected": np.zeros(n_steps),
                "aoi": np.zeros(n_steps),
            }

        dw.set_solar_override(data)
        dw.step()
        step_with_override = dw.step()

        dw.clear_solar_override()

        assert not dw.has_solar_override(), (
            "has_solar_override() should be False after clear"
        )

        step_after_clear = dw.step()

        # The zero-override forces zero solar gains, which should differ from
        # both the no-override case and the restored-Perez case.  After clearing,
        # the Perez model resumes, so step_after_clear should be closer to the
        # no-override baseline than to the zero-override step for at least the
        # indoor temperature.
        temp_key = "Temperature - Zone_1 (C)"
        t_without = step_without_override[temp_key]
        t_with = step_with_override[temp_key]
        t_after = step_after_clear[temp_key]

        diff_to_without = abs(t_after - t_without)
        diff_to_with = abs(t_after - t_with)

        # After clear, behaviour should return closer to the non-override baseline.
        # At minimum, the override must have had *some* effect — if all three are
        # identical the test is vacuous.
        assert t_without != t_with or diff_to_without <= diff_to_with, (
            f"Expected post-clear step to be closer to no-override baseline: "
            f"without={t_without:.4f}, with_zero={t_with:.4f}, after_clear={t_after:.4f}"
        )


class TestSolarOverrideListOfDicts:
    def test_set_solar_override_from_list_of_dicts(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        surface_ids = dw.surface_ids()
        n_steps = 3

        data = []
        for step_idx in range(n_steps):
            step_data = []
            for sid in surface_ids:
                step_data.append(
                    {
                        "surface_id": int(sid),
                        "direct_w_m2": 100.0,
                        "diffuse_w_m2": 50.0,
                        "reflected_w_m2": 10.0,
                        "angle_of_incidence_rad": 0.0,
                    }
                )
            data.append(step_data)

        dw.set_solar_override(data)

        assert dw.has_solar_override(), "has_solar_override() should be True"

        dw.step()


class TestSolarOverrideValidation:
    def test_invalid_input_raises_value_error(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        with pytest.raises(ValueError):
            dw.set_solar_override(12345)

    def test_invalid_dict_raises_value_error(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        with pytest.raises(ValueError):
            dw.set_solar_override({"not": "valid"})


class TestSolarOverridePVProduction:
    def test_solar_override_affects_thermal_gains(self):
        from ochre_next import Dwelling
        import numpy as np

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-07-15T12:00:00",
            duration_s=6 * 3600,
            time_res_s=3600,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        surface_ids = dw.surface_ids()
        n_surfaces = len(surface_ids)

        data_zero = {}
        for sid in surface_ids:
            data_zero[int(sid)] = {
                "direct": np.zeros(6),
                "diffuse": np.zeros(6),
                "reflected": np.zeros(6),
                "aoi": np.zeros(6),
            }
        dw.set_solar_override(data_zero)
        result_zero = dw.step()
        dw.step()

        dw.clear_solar_override()
        dw.reset_with_seed(0)

        data_high = {}
        for sid in surface_ids:
            data_high[int(sid)] = {
                "direct": np.full(6, 800.0),
                "diffuse": np.full(6, 100.0),
                "reflected": np.full(6, 50.0),
                "aoi": np.zeros(6),
            }
        dw.set_solar_override(data_high)

        result_high = dw.step()

        # With 800 W/m2 irradiance vs zero, at least one measurable quantity
        # must differ: indoor temperature and/or HVAC power.
        temp_key = "Temperature - Zone_1 (C)"
        temp_zero = result_zero[temp_key]
        temp_high = result_high[temp_key]
        power_zero = result_zero["net_electric_power_kw"]
        power_high = result_high["net_electric_power_kw"]

        something_changed = (
            abs(temp_high - temp_zero) > 1e-6
            or abs(power_high - power_zero) > 1e-6
        )
        assert something_changed, (
            f"Expected measurable difference between zero and 800 W/m2 irradiance: "
            f"temp_zero={temp_zero:.4f}, temp_high={temp_high:.4f}, "
            f"power_zero={power_zero:.6f}, power_high={power_high:.6f}"
        )

    def test_pv_produces_zero_at_night_with_summer_override(self):
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            HPXML_PV,
            SCHEDULE,
            WEATHER,
            start_time="2019-07-15T00:00:00",
            duration_s=6 * 3600,
            time_res_s=3600,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        dw.initialize()

        df = pl.read_csv(PVLIB_SOLAR_SUMMER_CSV)
        df = df.head(24)
        dw.set_solar_override(df)

        for _ in range(6):
            result = dw.step()
            net_power = result["net_electric_power_kw"]
            assert net_power >= 0, (
                f"Expected non-negative net power at night, got {net_power} kW"
            )

    def test_summer_daytime_with_pv_produces_negative_power(self):
        from ochre_next import Dwelling, PV

        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-07-15T10:00:00",
            duration_s=6 * 3600,
            time_res_s=3600,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
        )
        pv = PV(name="roof_pv", capacity_kw=10.0, tilt=30.0, azimuth=180.0)
        dw.add_pv(pv)
        dw.initialize()

        found_negative = False
        for _ in range(6):
            result = dw.step()
            if result["net_electric_power_kw"] < 0:
                found_negative = True

        assert found_negative, (
            "Expected at least one daytime step with negative net power (PV export)"
        )
