"""Tests for Python-side simulation metrics exposure."""

import math

import pytest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
HPXML_PV = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

from ochre_next import Dwelling, SimulationMetrics


class TestMetrics:
    def test_metrics_after_simulate(self):
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
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert isinstance(metrics, SimulationMetrics)

        annual = metrics.total_energy_kwh
        assert annual.net_energy_kwh > 0, (
            "January Denver ASHP case must have positive net energy"
        )
        assert math.isfinite(annual.net_energy_kwh)
        assert annual.gross_consumption_kwh >= annual.net_energy_kwh, (
            "gross consumption must be >= net energy"
        )
        assert annual.gross_pv_generation_kwh >= 0
        assert math.isfinite(annual.gross_consumption_kwh)
        assert math.isfinite(annual.gross_pv_generation_kwh)
        assert len(annual.per_end_use) > 0
        assert all(v >= 0 for v in annual.per_end_use.values())
        assert all(math.isfinite(v) for v in annual.per_end_use.values())

        peak = metrics.peak_power_kw
        assert peak.rolling_15min_kw >= peak.rolling_30min_kw >= peak.rolling_60min_kw
        assert peak.rolling_15min_kw > 0, "Peak power must be positive for a heating case"
        assert len(peak.per_end_use) > 0

        grid = metrics.grid_interaction
        assert math.isfinite(grid.peak_import_kw)
        assert math.isfinite(grid.peak_export_kw)

        eff = metrics.efficiency
        if eff.hvac_heating_cop is not None:
            assert 1.0 <= eff.hvac_heating_cop <= 5.0, (
                f"ASHP heating COP should be in [1.0, 5.0], got {eff.hvac_heating_cop}"
            )
        assert eff.hvac_cooling_cop is None or eff.hvac_cooling_cop > 0
        assert eff.water_heater_cop is None or eff.water_heater_cop > 0
        assert (
            eff.battery_round_trip_efficiency is None
            or eff.battery_round_trip_efficiency > 0
        )

    def test_metrics_repr_works(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert isinstance(repr(metrics.total_energy_kwh), str)
        assert isinstance(repr(metrics.peak_power_kw), str)
        assert isinstance(repr(metrics.grid_interaction), str)
        assert isinstance(repr(metrics.efficiency), str)

    def test_metrics_before_simulate_raises(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
        )
        dw.initialize()

        with pytest.raises(ValueError, match="no batches flushed"):
            dw.metrics()

    def test_envelope_loads_none_at_low_verbosity(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=0,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert metrics.envelope_loads_kwh is None

    def test_envelope_loads_present_at_high_verbosity(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=6,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()
        env = metrics.envelope_loads_kwh

        assert env is not None
        assert math.isfinite(env.window_solar_kwh)
        assert math.isfinite(env.opaque_solar_lwr_kwh)
        assert math.isfinite(env.interior_lwr_kwh)
        assert math.isfinite(env.infiltration_kwh)
        assert math.isfinite(env.ventilation_kwh)
        assert math.isfinite(env.hvac_heating_kwh)
        assert math.isfinite(env.hvac_cooling_kwh)
        assert math.isfinite(env.internal_gains_kwh)
        assert math.isfinite(env.duct_loss_kwh)

    def test_gas_energy_available_when_fuel_type_present(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert metrics.gas_energy is not None
        assert metrics.gas_energy.total_therms >= 0
        assert metrics.gas_energy.total_kwh_equivalent >= 0
        if metrics.gas_energy.total_kwh_equivalent > 0:
            assert metrics.gas_energy.total_therms > 0

    def test_combined_total_energy_kwh(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()
        combined = metrics.combined_total_energy_kwh

        assert math.isfinite(combined)
        assert combined >= metrics.total_energy_kwh.net_energy_kwh, (
            "combined_total_energy_kwh must be >= net electric energy"
        )

    def test_total_energy_kwh_has_expected_properties(self):
        """Verify the renamed/added TotalEnergyKwh properties are accessible."""
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        annual = dw.metrics().total_energy_kwh
        # Verify new fields are present and consistent
        assert math.isfinite(annual.net_energy_kwh)
        assert math.isfinite(annual.gross_consumption_kwh)
        assert math.isfinite(annual.gross_pv_generation_kwh)
        # gross_consumption_kwh and gross_pv_generation_kwh must be >= 0
        assert annual.gross_consumption_kwh >= 0
        assert annual.gross_pv_generation_kwh >= 0
        # The old `total` attribute must not exist
        assert not hasattr(annual, "total"), (
            "'total' attribute must not exist; use 'net_energy_kwh'"
        )

    def test_pv_generation_is_tracked_separately(self):
        """Regression: PV generation must appear in gross_pv_generation_kwh.

        For a dwelling without PV, gross_pv_generation_kwh should be zero.
        """
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3600,
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        annual = dw.metrics().total_energy_kwh
        # The base.xml fixture has no PV, so gross_pv_generation should be 0
        assert annual.gross_pv_generation_kwh == 0.0, (
            "base.xml has no PV — gross_pv_generation_kwh should be zero"
        )
        # gross_consumption should equal net_energy when there's no PV
        assert abs(annual.gross_consumption_kwh - annual.net_energy_kwh) < 0.01, (
            "without PV, gross_consumption_kwh ~= net_energy_kwh"
        )

    @pytest.mark.slow
    def test_pv_equipped_dwelling_metrics_show_consumption_net_split(self):
        """PV dwelling: gross consumption > net because PV offsets load.

        Verifies the primary regression from the ticket: net_energy_kwh (which
        sums all total_kw including PV export) is meaningfully less than
        gross_consumption_kwh (which counts only positive total_kw) when PV
        is present.

        gross_pv_generation_kwh is asserted to be >= 0 (sanity only) — a
        positive-value assertion requires the \"PV End Use Electric Power
        (kW)\" aggregate column, which is a pre-existing infrastructure gap
        (see T-0326 Known Limitations).
        """
        dw = Dwelling.from_hpxml(
            HPXML_PV,
            SCHEDULE,
            WEATHER,
            start_time="2019-07-15T00:00:00Z",
            duration_s=86400,
            time_res_s=900,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        annual = dw.metrics().total_energy_kwh
        assert annual.gross_consumption_kwh > 0, (
            "PV-equipped dwelling must have non-zero gross consumption"
        )
        gap = annual.gross_consumption_kwh - annual.net_energy_kwh
        assert gap > 0.01, (
            f"PV dwelling must show meaningful gap between gross consumption "
            f"({annual.gross_consumption_kwh:.2f}) and net ({annual.net_energy_kwh:.2f})"
        )
        assert annual.gross_pv_generation_kwh >= 0.0
