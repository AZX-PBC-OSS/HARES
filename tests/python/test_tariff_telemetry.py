"""Tests for BillingPeriodSummary and TariffTelemetry PyO3 bindings."""

from __future__ import annotations

import datetime

from conftest import make_dwelling

from ochre_next import BillingPeriodSummary, ElectricTariff, TariffTelemetry


def _build_tou_tariff():
    return (
        ElectricTariff.builder()
        .set_name("Test TOU")
        .add_tou_period(
            "peak",
            [{"day": "any", "start_hour": 14, "end_hour": 20}],
            "all",
        )
        .add_tou_period(
            "offpeak",
            [{"day": "any", "start_hour": 20, "end_hour": 14}],
            "all",
        )
        .add_energy_rate("peak", "all", 0.30)
        .add_energy_rate("offpeak", "all", 0.10)
        .set_fixed_charges(10.0, 0.0)
        .build()
    )


def test_billing_summary_emitted():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=35 * 86400, time_res_s=900)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    dw.simulate()

    summaries = dw.billing_summaries()
    assert len(summaries) >= 1
    assert all(isinstance(s, BillingPeriodSummary) for s in summaries)


def test_billing_summary_fields():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=35 * 86400, time_res_s=900)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    dw.simulate()

    summaries = dw.billing_summaries()
    assert len(summaries) >= 1
    s = summaries[0]

    assert isinstance(s.period_start, datetime.datetime)
    assert isinstance(s.period_end, datetime.datetime)
    assert s.period_start.tzinfo is not None, "period_start must be timezone-aware"
    assert s.period_end.tzinfo is not None, "period_end must be timezone-aware"
    assert s.period_end > s.period_start
    assert s.total_import_kwh >= 0.0
    assert s.total_export_kwh >= 0.0
    assert isinstance(s.net_bill_usd, float)
    assert isinstance(s.energy_charge_usd, float)
    assert isinstance(s.demand_charge_usd, float)
    assert isinstance(s.fixed_charge_usd, float)
    assert isinstance(s.export_credit_usd, float)
    assert isinstance(s.peak_demand_kw, float)


def test_billing_summary_to_dict():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=35 * 86400, time_res_s=900)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    dw.simulate()

    summaries = dw.billing_summaries()
    assert len(summaries) >= 1
    d = summaries[0].to_dict()

    expected_keys = {
        "period_start",
        "period_end",
        "energy_charge_usd",
        "demand_charge_usd",
        "fixed_charge_usd",
        "export_credit_usd",
        "net_bill_usd",
        "peak_demand_kw",
        "total_import_kwh",
        "total_export_kwh",
    }
    assert set(d.keys()) == expected_keys


def test_billing_summary_repr():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=35 * 86400, time_res_s=900)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    dw.simulate()

    summaries = dw.billing_summaries()
    assert len(summaries) >= 1
    r = repr(summaries[0])
    assert "BillingPeriodSummary" in r
    assert "net_bill=$" in r


def test_tariff_telemetry_none_without_tariff():
    dw = make_dwelling(duration_s=300, time_res_s=60)
    dw.initialize()
    assert dw.tariff_telemetry() is None


def test_tariff_telemetry_available_after_tariff_set():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=300, time_res_s=60)
    dw.initialize()
    dw.set_electric_tariff(tariff)

    t = dw.tariff_telemetry()
    assert t is not None
    assert isinstance(t, TariffTelemetry)
    assert isinstance(t.period_name, str)
    assert isinstance(t.current_rate_usd_per_kwh, float)
    assert isinstance(t.export_rate_usd_per_kwh, float)
    assert isinstance(t.cumulative_import_kwh, float)
    assert isinstance(t.cumulative_export_kwh, float)
    assert isinstance(t.peak_demand_kw, float)
    assert isinstance(t.cumulative_energy_cost_usd, float)


def test_tariff_telemetry_repr():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=300, time_res_s=60)
    dw.initialize()
    dw.set_electric_tariff(tariff)

    t = dw.tariff_telemetry()
    r = repr(t)
    assert "TariffTelemetry" in r
    assert "rate=$" in r


def test_tariff_telemetry_to_dict():
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=300, time_res_s=60)
    dw.initialize()
    dw.set_electric_tariff(tariff)

    t = dw.tariff_telemetry()
    d = t.to_dict()
    expected_keys = {
        "period_name",
        "current_rate_usd_per_kwh",
        "export_rate_usd_per_kwh",
        "cumulative_import_kwh",
        "cumulative_export_kwh",
        "peak_demand_kw",
        "cumulative_energy_cost_usd",
    }
    assert set(d.keys()) == expected_keys


def test_tariff_telemetry_rate_matches_tariff():
    tariff = _build_tou_tariff()
    # Start time is 2019-01-01T00:00:00, which is midnight = offpeak (hour 0-14)
    dw = make_dwelling(duration_s=300, time_res_s=60)
    dw.initialize()
    dw.set_electric_tariff(tariff)

    t = dw.tariff_telemetry()
    # At midnight, should be offpeak with rate 0.10
    assert t.period_name == "offpeak"
    assert abs(t.current_rate_usd_per_kwh - 0.10) < 1e-6


def test_billing_energy_charge_matches_rate_times_kwh():
    """Energy charge must approximately equal total_import_kwh × rate."""
    tariff = (
        ElectricTariff.builder()
        .set_name("Flat")
        .add_tou_period("all", [{"day": "any", "start_hour": 0, "end_hour": 24}], "all")
        .add_energy_rate("all", "all", 0.15)
        .set_fixed_charges(0.0, 0.0)
        .build()
    )
    dw = make_dwelling(duration_s=35 * 86400, time_res_s=900)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    dw.simulate()
    summaries = dw.billing_summaries()
    assert len(summaries) >= 1
    for s in summaries:
        if s.total_import_kwh > 0:
            expected = s.total_import_kwh * 0.15
            assert abs(s.energy_charge_usd - expected) / expected < 0.10, (
                f"energy_charge {s.energy_charge_usd:.2f} != import "
                f"{s.total_import_kwh:.1f} × 0.15 = {expected:.2f}"
            )


def test_tariff_cumulative_cost_increases():
    """Cumulative energy cost must increase as house consumes power."""
    tariff = _build_tou_tariff()
    dw = make_dwelling(duration_s=3600, time_res_s=60)
    dw.initialize()
    dw.set_electric_tariff(tariff)
    costs = []
    for _ in range(10):
        dw.step()
        t = dw.tariff_telemetry()
        if t is not None:
            costs.append(t.cumulative_energy_cost_usd)
    assert len(costs) >= 2
    assert costs[-1] > costs[0], "Cumulative cost must increase with consumption"
