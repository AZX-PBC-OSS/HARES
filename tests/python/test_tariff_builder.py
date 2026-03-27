"""Tests for ElectricTariff and GasTariff PyO3 bindings."""

from __future__ import annotations

import pytest
from ochre_next import ElectricTariff, GasTariff, GasTariffBuilder, TariffBuilder


def _make_simple_tou_builder() -> TariffBuilder:
    """Build a basic 2-period TOU tariff builder (on-peak / off-peak)."""
    return (
        ElectricTariff.builder()
        .set_name("Simple TOU")
        .add_tou_period(
            "on-peak",
            [{"day": "weekdays", "start_hour": 16, "end_hour": 21}],
            "summer",
        )
        .add_tou_period(
            "off-peak",
            [{"day": "any", "start_minute": 0, "end_minute": 960}],
            "all",
        )
        .add_energy_rate("on-peak", "summer", 0.35)
        .add_energy_rate("off-peak", "all", 0.10)
    )


class TestBuilderSimpleTou:
    def test_builds_successfully(self) -> None:
        tariff = _make_simple_tou_builder().build()
        assert tariff.name == "Simple TOU"

    def test_repr_contains_name(self) -> None:
        tariff = _make_simple_tou_builder().build()
        r = repr(tariff)
        assert "Simple TOU" in r
        assert "tou_periods=2" in r
        assert "energy_rates=2" in r

    def test_energy_rate_values(self) -> None:
        tariff = _make_simple_tou_builder().build()
        d = tariff.to_dict()
        rates = {r["period_name"]: r["rate_per_kwh"] for r in d["energy_rates"]}
        assert rates["on-peak"] == pytest.approx(0.35)
        assert rates["off-peak"] == pytest.approx(0.10)


class TestBuilderWithDemandCharges:
    def test_demand_rate_with_ratchet(self) -> None:
        tariff = (
            _make_simple_tou_builder()
            .add_demand_rate(
                18.50,
                "summer",
                period_name="on-peak",
                ratchet_fraction=0.85,
                lookback_months=11,
            )
            .build()
        )
        d = tariff.to_dict()
        assert len(d["demand_rates"]) == 1
        dr = d["demand_rates"][0]
        assert dr["rate_per_kw"] == pytest.approx(18.50)
        assert dr["ratchet"]["minimum_fraction"] == pytest.approx(0.85)
        assert dr["ratchet"]["lookback_months"] == 11

    def test_demand_rate_without_ratchet(self) -> None:
        tariff = (
            _make_simple_tou_builder()
            .add_demand_rate(10.0, "all")
            .build()
        )
        d = tariff.to_dict()
        assert len(d["demand_rates"]) == 1
        assert d["demand_rates"][0]["ratchet"] is None

    def test_partial_ratchet_args_raises(self) -> None:
        with pytest.raises(ValueError, match="ratchet_fraction and lookback_months"):
            _make_simple_tou_builder().add_demand_rate(
                10.0, "summer", ratchet_fraction=0.5
            )


class TestBuilderWithTiers:
    def test_tiered_rates(self) -> None:
        tariff = (
            ElectricTariff.builder()
            .set_name("Tiered")
            .set_tiered_rates("summer", [500.0, 1000.0], [0.10, 0.15, 0.25])
            .build()
        )
        d = tariff.to_dict()
        assert len(d["tiered_rates"]) == 1
        assert len(d["tiered_rates"][0]["thresholds_kwh"]) == 2

    def test_invalid_tier_count_raises(self) -> None:
        with pytest.raises(ValueError):
            ElectricTariff.builder().set_tiered_rates(
                "all", [500.0], [0.10, 0.15, 0.25]
            )


class TestBuilderChaining:
    def test_all_methods_return_builder(self) -> None:
        b = ElectricTariff.builder()
        assert isinstance(b, TariffBuilder)

        b = b.set_name("Chain Test")
        assert isinstance(b, TariffBuilder)

        b = b.add_tou_period(
            "peak",
            [{"day": "any", "start_hour": 16, "end_hour": 21}],
            "all",
        )
        assert isinstance(b, TariffBuilder)

        b = b.add_energy_rate("peak", "all", 0.30)
        assert isinstance(b, TariffBuilder)

        b = b.add_demand_rate(10.0, "all")
        assert isinstance(b, TariffBuilder)

        b = b.set_tiered_rates("all", [500.0], [0.10, 0.20])
        assert isinstance(b, TariffBuilder)

        b = b.set_fixed_charges(monthly_usd=12.0)
        assert isinstance(b, TariffBuilder)

        b = b.set_export_net_metering()
        assert isinstance(b, TariffBuilder)

        b = b.set_export_flat_rate(0.08)
        assert isinstance(b, TariffBuilder)

        b = b.set_minimum_charge(5.0)
        assert isinstance(b, TariffBuilder)

        tariff = b.build()
        assert isinstance(tariff, ElectricTariff)

    def test_fluent_one_liner(self) -> None:
        tariff = (
            ElectricTariff.builder()
            .set_name("Fluent")
            .add_tou_period(
                "peak",
                [{"day": "any", "start_hour": 14, "end_hour": 20}],
                "all",
            )
            .add_energy_rate("peak", "all", 0.25)
            .set_fixed_charges(monthly_usd=10.0)
            .build()
        )
        assert tariff.name == "Fluent"


class TestFromDictRoundtrip:
    def test_roundtrip(self) -> None:
        original = _make_simple_tou_builder().set_fixed_charges(monthly_usd=15.0).build()
        d = original.to_dict()
        restored = ElectricTariff.from_dict(d)
        assert original == restored

    def test_to_dict_has_expected_keys(self) -> None:
        tariff = _make_simple_tou_builder().build()
        d = tariff.to_dict()
        assert "name" in d
        assert "tou_schedule" in d
        assert "energy_rates" in d
        assert "fixed_charges" in d


class TestFromJson:
    def test_from_json_string(self) -> None:
        import json

        original = _make_simple_tou_builder().build()
        json_str = json.dumps(original.to_dict())
        restored = ElectricTariff.from_json(json_str)
        assert original == restored


class TestExportModes:
    def test_net_metering(self) -> None:
        tariff = (
            _make_simple_tou_builder()
            .set_export_net_metering()
            .build()
        )
        d = tariff.to_dict()
        assert d["export_rate"]["mode"] == "NetMetering"

    def test_flat_rate(self) -> None:
        tariff = (
            _make_simple_tou_builder()
            .set_export_flat_rate(0.08)
            .build()
        )
        d = tariff.to_dict()
        assert d["export_rate"]["mode"] == {"FlatRate": pytest.approx(0.08)}

    def test_net_billing(self) -> None:
        tariff = (
            _make_simple_tou_builder()
            .set_export_net_billing([
                {"period_name": "on-peak", "season": "summer", "rate": 0.20},
            ])
            .build()
        )
        d = tariff.to_dict()
        assert d["export_rate"]["mode"] == "NetBilling"
        assert len(d["export_rate"]["tou_credits"]) == 1


class TestGasTariffBuilder:
    def test_build_gas_tariff(self) -> None:
        tariff = (
            GasTariff.builder()
            .set_name("Gas Baseline")
            .set_tiered_rates("winter", [25.0, 50.0], [1.05, 1.35, 1.85])
            .set_tiered_rates("summer", [15.0], [0.95, 1.25])
            .set_fixed_charges(monthly_usd=10.0)
            .build()
        )
        assert tariff.name == "Gas Baseline"
        d = tariff.to_dict()
        assert len(d["tiered_rates"]) == 2

    def test_gas_chaining(self) -> None:
        b = GasTariff.builder()
        assert isinstance(b, GasTariffBuilder)
        b = b.set_name("Test Gas")
        assert isinstance(b, GasTariffBuilder)
        b = b.set_tiered_rates("all", [], [1.00])
        assert isinstance(b, GasTariffBuilder)

    def test_gas_roundtrip(self) -> None:
        original = (
            GasTariff.builder()
            .set_name("RT Gas")
            .set_tiered_rates("all", [30.0], [0.80, 1.10])
            .build()
        )
        d = original.to_dict()
        restored = GasTariff.from_dict(d)
        assert original == restored

    def test_gas_repr(self) -> None:
        tariff = (
            GasTariff.builder()
            .set_name("Repr Gas")
            .set_tiered_rates("all", [], [1.00])
            .build()
        )
        r = repr(tariff)
        assert "Repr Gas" in r
        assert "tiered_rates=1" in r
