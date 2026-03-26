"""Tests for Python-side simulation metrics exposure."""

import pytest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)
SCHEDULE = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Input Files/BEopt_example_schedule.csv"
)

from ochre_next import Dwelling, SimulationMetrics


class TestMetrics:
    def test_metrics_after_simulate(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration=3600,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=0,
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert isinstance(metrics, SimulationMetrics)

        annual = metrics.annual_energy_kwh
        assert isinstance(annual.total, float)
        assert annual.total >= 0
        assert isinstance(annual.per_end_use, dict)
        assert all(isinstance(k, str) for k in annual.per_end_use.keys())
        assert all(isinstance(v, float) for v in annual.per_end_use.values())

        peak = metrics.peak_power_kw
        assert isinstance(peak.rolling_15min_kw, float)
        assert peak.rolling_15min_kw >= 0
        assert isinstance(peak.per_end_use, dict)

        grid = metrics.grid_interaction
        assert isinstance(grid.peak_import_kw, float)
        assert grid.peak_import_kw >= 0
        assert isinstance(grid.peak_export_kw, float)
        assert grid.peak_export_kw >= 0

        eff = metrics.efficiency
        assert eff.hvac_heating_cop is None or eff.hvac_heating_cop > 0
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
            duration=3600,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert isinstance(repr(metrics.annual_energy_kwh), str)
        assert isinstance(repr(metrics.peak_power_kw), str)
        assert isinstance(repr(metrics.grid_interaction), str)
        assert isinstance(repr(metrics.efficiency), str)

    def test_metrics_before_simulate_raises(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration=3600,
            time_res=60,
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
            duration=3600,
            time_res=60,
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
            duration=3600,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=6,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()
        env = metrics.envelope_loads_kwh

        assert env is not None
        assert isinstance(env.window_solar_kwh, float)
        assert isinstance(env.opaque_solar_lwr_kwh, float)
        assert isinstance(env.interior_lwr_kwh, float)
        assert isinstance(env.infiltration_kwh, float)
        assert isinstance(env.ventilation_kwh, float)
        assert isinstance(env.hvac_heating_kwh, float)
        assert isinstance(env.hvac_cooling_kwh, float)
        assert isinstance(env.internal_gains_kwh, float)
        assert isinstance(env.duct_loss_kwh, float)

    def test_gas_energy_available_when_fuel_type_present(self):
        dw = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration=3600,
            time_res=60,
            defaults_path=str(HARES_DEFAULTS),
            output_verbosity=1,
        )
        dw.initialize()
        dw.simulate()

        metrics = dw.metrics()

        assert metrics.gas_energy is not None
        assert metrics.gas_energy.total_therms >= 0
        assert metrics.gas_energy.total_kwh_equivalent >= 0
