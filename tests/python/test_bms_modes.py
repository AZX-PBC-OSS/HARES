"""Tests for BmsMode, BmsAction, BmsScheduleWindow, GridExportRule, StormWatchTrigger, and DepartureConstraint bindings."""

from __future__ import annotations

import pytest
from ochre_next import (
    BmsAction,
    BmsMode,
    BmsScheduleWindow,
    ChargingStrategy,
    DepartureConstraint,
    GridExportRule,
    StormWatchTrigger,
)


def test_bms_self_consumption() -> None:
    mode = BmsMode.self_consumption(min_soc=0.1, max_soc=0.95, solar_only_charging=True)
    r = repr(mode)
    assert "self_consumption" in r
    assert "0.1" in r
    assert "0.95" in r


def test_bms_self_consumption_defaults() -> None:
    mode = BmsMode.self_consumption()
    r = repr(mode)
    assert "self_consumption" in r
    assert "0.1" in r
    assert "1" in r


def test_bms_tou_optimization() -> None:
    mode = BmsMode.time_of_use_optimization(
        reserve_soc=0.2,
        charge_threshold_percentile=25.0,
        discharge_threshold_percentile=75.0,
    )
    r = repr(mode)
    assert "time_of_use_optimization" in r
    assert "25" in r
    assert "75" in r


def test_bms_backup_reserve() -> None:
    mode = BmsMode.backup_reserve(target_soc=0.8, charge_from_grid=True, charge_rate_fraction=0.5)
    r = repr(mode)
    assert "backup_reserve" in r
    assert "0.8" in r
    assert "0.5" in r


def test_bms_demand_response_nested() -> None:
    base = BmsMode.self_consumption(min_soc=0.1, max_soc=0.9)
    mode = BmsMode.demand_response(base_mode=base, dr_discharge_rate=0.8, min_soc_during_dr=0.15)
    r = repr(mode)
    assert "demand_response" in r
    assert "self_consumption" in r


def test_bms_scheduled_with_windows() -> None:
    w = BmsScheduleWindow(
        day="weekdays",
        start_minute=0,
        end_minute=360,
        action=BmsAction.charge(rate_fraction=1.0),
    )
    mode = BmsMode.scheduled(windows=[w])
    r = repr(mode)
    assert "scheduled" in r
    assert "1 window" in r


def test_bms_storm_watch_nested() -> None:
    mode = BmsMode.storm_watch(
        target_soc=1.0,
        trigger="weather_signal",
        base_mode=BmsMode.manual(),
    )
    r = repr(mode)
    assert "storm_watch" in r
    assert "manual" in r


def test_bms_manual() -> None:
    mode = BmsMode.manual()
    assert repr(mode) == "BmsMode.manual()"


def test_bms_equality() -> None:
    a = BmsMode.self_consumption(min_soc=0.1, max_soc=0.9)
    b = BmsMode.self_consumption(min_soc=0.1, max_soc=0.9)
    c = BmsMode.manual()
    assert a == b
    assert a != c


def test_grid_export_rule_enum() -> None:
    assert GridExportRule.SolarOnly != GridExportRule.Unrestricted
    assert GridExportRule.SolarOnly != GridExportRule.Disabled
    assert GridExportRule.Unrestricted != GridExportRule.Disabled
    assert "SolarOnly" in repr(GridExportRule.SolarOnly)
    assert "Unrestricted" in repr(GridExportRule.Unrestricted)
    assert "Disabled" in repr(GridExportRule.Disabled)


def test_storm_watch_trigger_enum() -> None:
    assert StormWatchTrigger.ManualEnable != StormWatchTrigger.WeatherSignal
    assert "ManualEnable" in repr(StormWatchTrigger.ManualEnable)
    assert "WeatherSignal" in repr(StormWatchTrigger.WeatherSignal)


def test_bms_action_variants() -> None:
    c = BmsAction.charge(rate_fraction=0.5)
    d = BmsAction.discharge(rate_fraction=0.8)
    i = BmsAction.idle()
    h = BmsAction.hold(target_soc=0.6)
    assert "charge" in repr(c)
    assert "0.5" in repr(c)
    assert "discharge" in repr(d)
    assert "idle" in repr(i)
    assert "hold" in repr(h)
    assert c != d
    assert i != h


def test_departure_constraint() -> None:
    dc = DepartureConstraint(day_filter="weekdays", departure_minute=480, target_soc=0.8)
    assert dc.day_filter == "weekdays"
    assert dc.departure_minute == 480
    assert dc.target_soc == 0.8
    assert "weekdays" in repr(dc)


def test_departure_constraint_defaults() -> None:
    dc = DepartureConstraint()
    assert dc.day_filter == "weekdays"
    assert dc.departure_minute == 480
    assert dc.target_soc == 0.8


def test_charging_strategy_v2g() -> None:
    s = ChargingStrategy.v2g(min_soc=0.2, max_export_kw=5.0, price_threshold=0.15)
    assert "V2G" in repr(s)


def test_charging_strategy_v2h() -> None:
    s = ChargingStrategy.v2h(discharge_threshold_soc=0.8, min_soc=0.2)
    assert "V2H" in repr(s)


def test_charging_strategy_solar_surplus() -> None:
    dc = DepartureConstraint(day_filter="weekdays", departure_minute=480, target_soc=0.9)
    s = ChargingStrategy.solar_surplus(min_charge_rate_kw=1.2, departure_schedule=[dc])
    assert "SolarSurplus" in repr(s)


def test_bms_schedule_window_properties() -> None:
    w = BmsScheduleWindow(
        day="weekends",
        start_minute=60,
        end_minute=300,
        action=BmsAction.discharge(rate_fraction=0.7),
    )
    assert w.day == "weekends"
    assert w.start_minute == 60
    assert w.end_minute == 300
    assert "discharge" in repr(w.action)


def test_bms_mode_validation_rejects_bad_soc() -> None:
    with pytest.raises(ValueError, match="min_soc"):
        BmsMode.self_consumption(min_soc=-0.1, max_soc=1.0)
    with pytest.raises(ValueError, match="min_soc must be <= max_soc"):
        BmsMode.self_consumption(min_soc=0.9, max_soc=0.1)


def test_bms_scheduled_empty_rejects() -> None:
    with pytest.raises(ValueError, match="empty"):
        BmsMode.scheduled(windows=[])
