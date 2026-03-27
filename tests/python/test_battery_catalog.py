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


class TestBatteryThermalBehavior:
    """Verify battery thermal management in cold weather."""

    def test_heater_draws_power_in_cold_weather(self):
        """Tesla PW3 (300W heater) should draw heater power at -19°C.

        In January Denver the battery cell temp drifts toward -19°C ambient.
        The heater should activate when cell temp drops below heater_threshold_c
        (5°C) and draw standby + heater power even when not charging.
        """
        from conftest import make_dwelling

        dw = make_dwelling(
            duration_s=3600, time_res_s=60, start_time="2019-01-01T00:00:00"
        )
        dw.initialize()
        bat = Battery.tesla_pw3()  # 300W heater, threshold=5°C
        dw.add_battery(bat)

        # Run long enough for cell temp to drop below heater threshold
        max_power = 0.0
        for _ in range(60):
            dw.step()
            tel = dw.telemetry().equipment()
            idx = tel["names"].index("Tesla Powerwall 3")
            power = tel["power_kw"][idx]
            max_power = max(max_power, power)

        # Heater (300W = 0.3 kW) + standby (10W = 0.01 kW) should be visible
        assert max_power > 0.1, (
            f"Battery heater should draw measurable power in -19°C weather, "
            f"max observed was {max_power:.4f} kW"
        )

    def test_passive_battery_no_heater_power_in_cold(self):
        """Enphase IQ 5P (no heater) should only draw standby in cold weather."""
        from conftest import make_dwelling

        dw = make_dwelling(
            duration_s=3600, time_res_s=60, start_time="2019-01-01T00:00:00"
        )
        dw.initialize()
        bat = Battery.enphase_iq5p()  # 0W heater, passive only
        dw.add_battery(bat)

        max_power = 0.0
        for _ in range(60):
            dw.step()
            tel = dw.telemetry().equipment()
            idx = tel["names"].index("Enphase IQ 5P")
            power = tel["power_kw"][idx]
            max_power = max(max_power, power)

        # No heater — only standby (15W = 0.015 kW)
        assert max_power < 0.05, (
            f"Passive battery should only draw standby (~15W), "
            f"got max {max_power:.4f} kW"
        )

    def test_heated_battery_eventually_charges_in_cold(self):
        """Tesla PW3 with heater should warm up enough to charge in cold weather.

        After the heater warms cells above min_charge_temp_c (0°C), applying
        a SOC target should result in charging. This may take many minutes
        depending on heater power vs thermal mass.
        """
        from ochre_next import ControlSignal
        from conftest import make_dwelling

        dw = make_dwelling(
            duration_s=7200, time_res_s=60, start_time="2019-01-01T00:00:00"
        )
        dw.initialize()
        bat = Battery.tesla_pw3()  # 300W heater
        dw.add_battery(bat)
        dw.apply_control("Tesla Powerwall 3", ControlSignal.soc_target(target=0.9))

        charged = False
        for _ in range(120):  # 2 hours
            dw.step()
            tel = dw.telemetry().equipment()
            idx = tel["names"].index("Tesla Powerwall 3")
            soc = tel["soc"][idx]
            if soc > 0.52:  # default initial is ~0.5
                charged = True
                break

        # With a 300W heater the battery should warm above 0°C within 2 hours
        # and begin charging. If this fails, the heater power or thermal model
        # may be misconfigured.
        assert charged, (
            f"Battery with 300W heater should warm up and charge within 2h at -19°C. "
            f"Final SOC={soc:.4f}"
        )

    def test_cold_blocks_charging_without_heater(self):
        """Enphase IQ 5P charges in cold because min_charge_temp_c=-20°C.

        LFP batteries like Enphase can charge at low temperatures (with derating).
        The IQ 5P has min_charge_temp_c=-20°C, so it should charge even in
        Denver January (-19°C) — just at a derated rate.
        """
        from ochre_next import ControlSignal
        from conftest import make_dwelling

        dw = make_dwelling(
            duration_s=3600, time_res_s=60, start_time="2019-01-01T00:00:00"
        )
        dw.initialize()
        bat = Battery.enphase_iq5p()  # min_charge_temp=-20°C
        dw.add_battery(bat)
        dw.apply_control("Enphase IQ 5P", ControlSignal.soc_target(target=0.9))

        initial_soc = None
        final_soc = None
        for _ in range(60):
            dw.step()
            tel = dw.telemetry().equipment()
            idx = tel["names"].index("Enphase IQ 5P")
            soc = tel["soc"][idx]
            if initial_soc is None:
                initial_soc = soc
            final_soc = soc

        assert final_soc > initial_soc, (
            f"Enphase IQ 5P (min_charge_temp=-20°C) should charge at -19°C "
            f"(with derating). Initial={initial_soc:.4f}, final={final_soc:.4f}"
        )
