"""T-0543: Standalone tariff evaluator integration tests.

Exercises PyTariffEvaluator from Python — standalone tariff evaluation
without a full dwelling simulation.
"""

import datetime as _dt
from datetime import datetime, timedelta

from ochre_next import ElectricTariff, TariffEvaluator

off_utc = _dt.timezone(_dt.timedelta(0))


def build_flat_tariff(rate_per_kwh: float) -> ElectricTariff:
    b = ElectricTariff.builder()
    (b.set_name("flat-test")
     .add_tou_period(
         "flat",
         [{"day": "any", "start_hour": 0.0, "end_hour": 24.0}],
         "all",
     )
     .add_energy_rate("flat", "all", rate_per_kwh))
    return b.build()


def build_tou_tariff() -> ElectricTariff:
    b = ElectricTariff.builder()
    (b.set_name("tou-test")
     .add_tou_period(
         "on-peak",
         [{"day": "weekdays", "start_hour": 16.0, "end_hour": 21.0}],
         "all",
     )
     .add_tou_period(
         "off-peak",
         [{"day": "any", "start_hour": 0.0, "end_hour": 24.0}],
         "all",
     )
     .add_energy_rate("on-peak", "all", 0.35)
     .add_energy_rate("off-peak", "all", 0.10))
    return b.build()


class TestStandaloneFlatRate:
    """Verify a flat-rate tariff evaluates correctly with constant load."""

    def test_full_year_constant_load(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2026, 1, 1, tzinfo=off_utc)
        interval = timedelta(hours=1)

        ev = TariffEvaluator(tariff, start, end, interval)

        summaries = []
        dt = start
        for _ in range(8760):
            dt += interval
            s = ev.step(1.0, 3600.0, dt)
            if s is not None:
                summaries.append(s)

        final = ev.finalize(end)
        if final is not None:
            summaries.append(final)

        total_kwh = sum(s.total_import_kwh for s in summaries)
        total_cost = sum(s.energy_charge_usd for s in summaries)

        assert abs(total_kwh - 8760.0) < 1e-6, f"expected 8760 kWh, got {total_kwh}"
        assert abs(total_cost - 1051.2) < 1e-6, f"expected $1051.20, got {total_cost}"


class TestTOUPricing:
    """TOU tariff with peak/off-peak periods produces correct charges."""

    def test_weekday_peak_different_from_offpeak(self):
        tariff = build_tou_tariff()
        # Monday Jan 6, 2025
        start = datetime(2025, 1, 6, tzinfo=off_utc)
        end = datetime(2025, 1, 7, tzinfo=off_utc)
        interval = timedelta(hours=1)

        ev = TariffEvaluator(tariff, start, end, interval)

        summaries = []
        dt = start
        for hour in range(24):
            dt += interval
            # Peak hours: 16:00-21:00 (UTC, same as local here)
            load = 2.0 if 16 <= hour < 21 else 1.0
            s = ev.step(load, 3600.0, dt)
            if s is not None:
                summaries.append(s)

        final = ev.finalize(end)
        if final is not None:
            summaries.append(final)

        total_kwh = sum(s.total_import_kwh for s in summaries)
        total_cost = sum(s.energy_charge_usd for s in summaries)
        assert abs(total_kwh - 29.0) < 1e-6  # 19*1 + 5*2 = 29
        expected_cost = 19.0 * 0.10 + 10.0 * 0.35  # 1.90 + 3.50 = 5.40
        assert abs(total_cost - expected_cost) < 1e-6


class TestIdempotent:
    """Repeated evaluation with same inputs produces identical outputs."""

    def test_same_tariff_same_load_same_result(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 2, tzinfo=off_utc)
        interval = timedelta(hours=1)

        def run():
            ev = TariffEvaluator(tariff, start, end, interval)
            summaries = []
            dt = start
            for hour in range(24):
                dt += interval
                load = 2.0 if hour % 2 == 0 else 1.0
                s = ev.step(load, 3600.0, dt)
                if s is not None:
                    summaries.append(s)
            final = ev.finalize(end)
            if final is not None:
                summaries.append(final)
            return [(s.total_import_kwh, s.energy_charge_usd, s.net_bill_usd)
                    for s in summaries]

        r1 = run()
        r2 = run()
        assert r1 == r2


class TestProperties:
    """PyTariffEvaluator exposes readable properties."""

    def test_tariff_name_readable(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 2, tzinfo=off_utc)
        interval = timedelta(hours=1)
        ev = TariffEvaluator(tariff, start, end, interval)
        assert ev.tariff_name == "flat-test"

    def test_start_time_end_time(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 2, tzinfo=off_utc)
        interval = timedelta(hours=1)
        ev = TariffEvaluator(tariff, start, end, interval)
        assert ev.start_time == start
        assert ev.end_time == end


class TestCurrentMetrics:
    """current_metrics provides a snapshot of the current billing period."""

    def test_current_metrics_mid_period(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 3, tzinfo=off_utc)  # 2 days
        interval = timedelta(hours=1)
        ev = TariffEvaluator(tariff, start, end, interval)

        dt = start + timedelta(hours=12)
        ev.step(2.0, 3600.0, dt)

        metrics = ev.current_metrics
        assert metrics.total_import_kwh > 0
        assert metrics.energy_charge_usd > 0
        # Peak demand should reflect the 2 kW load
        assert abs(metrics.peak_demand_kw - 2.0) < 1e-6

    def test_current_metrics_before_any_steps(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 2, tzinfo=off_utc)
        interval = timedelta(hours=1)
        ev = TariffEvaluator(tariff, start, end, interval)
        # Before any steps, metrics should still be accessible
        metrics = ev.current_metrics
        assert metrics.total_import_kwh == 0.0


class TestRepr:
    """__repr__ shows useful info."""

    def test_repr_contains_tariff_name(self):
        tariff = build_flat_tariff(0.12)
        start = datetime(2025, 1, 1, tzinfo=off_utc)
        end = datetime(2025, 1, 2, tzinfo=off_utc)
        interval = timedelta(hours=1)
        ev = TariffEvaluator(tariff, start, end, interval)
        r = repr(ev)
        assert "TariffEvaluator" in r
        assert "flat-test" in r


