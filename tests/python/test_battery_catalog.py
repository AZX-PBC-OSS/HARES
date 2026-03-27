"""Tests for the battery product catalog."""

import pytest
from ochre_next import Battery, BatteryProductId


PRODUCT_IDS = Battery.product_catalog()


class TestBatteryProductId:
    def test_catalog_has_11_products(self):
        assert len(PRODUCT_IDS) == 11

    def test_from_str_round_trips(self):
        for pid in PRODUCT_IDS:
            assert BatteryProductId.from_str(str(pid)) == pid

    def test_from_str_case_insensitive(self):
        assert BatteryProductId.from_str("tesla_pw3") == BatteryProductId.tesla_pw3()
        assert BatteryProductId.from_str("TESLA_PW3") == BatteryProductId.tesla_pw3()

    def test_repr(self):
        assert "TeslaPw3" in repr(BatteryProductId.tesla_pw3())


class TestBatteryCatalogFactories:
    @pytest.mark.parametrize("product_id", PRODUCT_IDS)
    def test_from_product_creates_valid_battery(self, product_id):
        bat = Battery.from_product(product_id)
        assert bat.capacity_kwh > 0
        assert bat.max_charge_kw is not None and bat.max_charge_kw > 0
        assert bat.max_discharge_kw is not None and bat.max_discharge_kw > 0
        assert bat.chemistry is not None
        assert bat.charge_efficiency is not None
        assert bat.discharge_efficiency is not None

    def test_tesla_pw3_specs(self):
        bat = Battery.tesla_pw3()
        assert bat.capacity_kwh == 13.5
        assert bat.max_charge_kw == 11.5
        assert bat.max_discharge_kw == 11.5
        assert str(bat.chemistry) == "LFP"
        assert bat.min_soc == 0.05
        assert bat.max_soc == 1.0

    def test_by_product_id_string_lookup(self):
        bat = Battery.by_product_id("tesla_pw3")
        assert bat.capacity_kwh == 13.5

    def test_by_product_id_invalid_raises(self):
        with pytest.raises(ValueError):
            Battery.by_product_id("nonexistent_battery")

    def test_all_shorthands(self):
        shorthands = [
            Battery.tesla_pw3,
            Battery.tesla_pw2,
            Battery.tesla_pw3_x2,
            Battery.enphase_iq5p,
            Battery.enphase_iq5p_x2,
            Battery.enphase_iq10c,
            Battery.franklin_apower,
            Battery.franklin_apower2,
            Battery.franklin_apower2_x2,
            Battery.solaredge_home,
            Battery.lg_resu10h,
        ]
        assert len(shorthands) == 11
        for factory in shorthands:
            bat = factory()
            assert bat.capacity_kwh > 0

    def test_custom_battery_still_works(self):
        bat = Battery("Custom", 20.0)
        assert bat.name == "Custom"
        assert bat.capacity_kwh == 20.0

    def test_catalog_battery_charges_in_simulation(self):
        """Tesla Powerwall 3 from catalog must charge when SOC target applied.

        Uses July start to ensure cell temperature is above the 0°C charge
        lockout (Denver January ambient is -19°C which blocks charging).
        """
        from ochre_next import ControlSignal
        from conftest import make_dwelling

        dw = make_dwelling(
            duration_s=300, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        dw.initialize()
        bat = Battery.tesla_pw3()
        dw.add_battery(bat)
        dw.apply_control("Tesla Powerwall 3", ControlSignal.soc_target(target=0.95))
        initial_soc = None
        for _ in range(5):
            dw.step()
            tel = dw.telemetry().equipment()
            idx = tel["names"].index("Tesla Powerwall 3")
            if initial_soc is None:
                initial_soc = tel["soc"][idx]
        final_soc = tel["soc"][idx]
        assert final_soc > initial_soc, (
            f"Catalog battery should charge: initial={initial_soc}, final={final_soc}"
        )
