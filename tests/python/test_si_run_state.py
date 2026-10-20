"""Acceptance tests for B-14's commit 3 (schedule row 94g): the Python
surfaces of the SI run state.

Each test is named in the entry's acceptance list. The fixture is the
golden manifest's inputs: `der_pv_battery_900s` (the base-pv-battery home
with the battery override), built the way `conftest.make_dwelling` builds.
"""

import datetime
from typing import Any

import pytest

HARES_DEFAULTS = "python/hares/defaults"
HPXML_PV = "tests/fixtures/hpxml/ochre_samples/base-pv.xml"
HPXML_PV_BATTERY = "tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml"
SCHEDULE = "data/examples/BEopt_example_schedule.csv"
WEATHER = "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"

BATTERY_OVERRIDE: dict[str, Any] = {
    "Battery": {
        "bms_mode": '{"SelfConsumption": {"min_soc": 0.1, "max_soc": 0.95, "solar_only_charging": false}}'
    }
}


def _der_pv_battery_900s(duration_s: int = 14400, time_res_s: int = 900):
    from hares import Dwelling, SimulationConfig

    dw = Dwelling.from_hpxml(
        HPXML_PV_BATTERY,
        SCHEDULE,
        WEATHER,
        config=SimulationConfig(
            start_time="2023-06-01T00:00:00-07:00",
            duration=duration_s,
            time_res=time_res_s,
            master_seed=0,
        ),
        defaults=HARES_DEFAULTS,
        bldg_id=1,
        overrides=BATTERY_OVERRIDE,
    )
    dw.initialize()
    return dw


# ---------------------------------------------------------------------------
# pv_sizing_returns_watts
# ---------------------------------------------------------------------------


class TestPvSizingReturnsWatts:
    """`size_pv_system` takes watts and returns watts, keyword-only."""

    def test_capacity_w_equals_the_parent_kw_times_1000(self):
        import hares

        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        result = hares.size_pv_system(usable, target_w=6000.0)
        # The parent commit returned capacity_kw == 6.16 for target_kw=6.0
        # (14 panels of 440 W); the recorded constant × 1000 in W.
        assert result.capacity_w == pytest.approx(6160.0, rel=1e-9)

    def test_target_kw_kwarg_names_its_replacement(self):
        import hares

        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        with pytest.raises(TypeError, match="target_kw"):  # pyright: ignore[reportCallIssue]
            hares.size_pv_system(usable, target_kw=6.0)  # pyright: ignore[reportCallIssue]

    def test_positional_forms_raise_typeerror(self):
        import hares

        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        with pytest.raises(TypeError):  # pyright: ignore[reportCallIssue]
            hares.size_pv_system(usable, 6000.0)  # pyright: ignore[reportCallIssue]
        with pytest.raises(TypeError):  # pyright: ignore[reportCallIssue]
            hares.required_main_panel_ampacity(6000.0)  # pyright: ignore[reportCallIssue]

    def test_estimate_pv_capacity_takes_target_w(self):
        dw = _der_pv_battery_900s(duration_s=3600)
        result = dw.estimate_pv_capacity(target_w=6000.0)
        assert result.capacity_w == pytest.approx(6160.0, rel=1e-9)
        with pytest.raises(TypeError):  # pyright: ignore[reportCallIssue]
            dw.estimate_pv_capacity(6000.0)  # pyright: ignore[reportCallIssue]
        with pytest.raises(TypeError, match="target_kw"):  # pyright: ignore[reportCallIssue]
            dw.estimate_pv_capacity(target_kw=6.0)  # pyright: ignore[reportCallIssue]


# ---------------------------------------------------------------------------
# TariffEvaluator.step takes watts
# ---------------------------------------------------------------------------


class TestTariffEvaluatorStepTakesWatts:
    def _build(self):
        import hares

        b = hares.ElectricTariff.builder()
        (
            b.set_name("flat-watts")
            .add_tou_period(
                "flat",
                [{"day": "any", "start_hour": 0.0, "end_hour": 24.0}],
                "all",
            )
            .add_energy_rate("flat", "all", 0.12)
        )
        tariff = b.build()
        start = datetime.datetime(2025, 1, 1, tzinfo=datetime.UTC)
        end = datetime.datetime(2025, 1, 2, tzinfo=datetime.UTC)
        interval = datetime.timedelta(hours=1)
        ev = hares.TariffEvaluator(tariff, start, end, interval)
        return ev, start, end

    def test_step_takes_net_power_w(self):
        ev, start, _end = self._build()
        dt = start + datetime.timedelta(hours=1)
        summary = ev.step(net_power_w=2000.0, dt_seconds=3600.0, current_time=dt)
        assert summary is None  # the period has not closed
        # The live state: 2000 W × 3600 s = 7.2e6 J (2 kWh priced at the
        # tariff's per-kWh rate).
        metrics = ev.current_metrics
        assert metrics.peak_demand_kw == pytest.approx(2.0, rel=1e-9)

    def test_positional_step_raises_typeerror(self):
        ev, start, _end = self._build()
        dt = start + datetime.timedelta(hours=1)
        with pytest.raises(TypeError):  # pyright: ignore[reportCallIssue]
            ev.step(2.0, 3600.0, dt)  # pyright: ignore[reportCallIssue]

    def test_net_power_kw_kwarg_names_its_replacement(self):
        ev, start, _end = self._build()
        dt = start + datetime.timedelta(hours=1)
        with pytest.raises(TypeError, match="net_power_kw"):  # pyright: ignore[reportCallIssue]
            ev.step(net_power_kw=2000.0, dt_seconds=3600.0, current_time=dt)  # pyright: ignore[reportCallIssue]
