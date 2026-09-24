"""Tests for the EV vehicle catalog."""

import pytest
from ochre_next import EV, VehicleId


VEHICLE_IDS = EV.vehicle_catalog()


class TestVehicleId:
    def test_catalog_has_13_vehicles(self):
        assert len(VEHICLE_IDS) == 13

    def test_from_str_round_trips(self):
        for vid in VEHICLE_IDS:
            assert VehicleId.from_str(str(vid)) == vid

    def test_from_str_case_insensitive(self):
        assert VehicleId.from_str("tesla_model_y_lr") == VehicleId.tesla_model_y_lr()
        assert VehicleId.from_str("TESLA_MODEL_Y_LR") == VehicleId.tesla_model_y_lr()

    def test_repr(self):
        assert "TeslaModelYLr" in repr(VehicleId.tesla_model_y_lr())


class TestEvCatalogFactories:
    @pytest.mark.parametrize("vehicle_id", VEHICLE_IDS)
    def test_from_vehicle_creates_valid_ev(self, vehicle_id):
        ev = EV.from_vehicle(vehicle_id)
        assert ev.capacity_kwh is not None and ev.capacity_kwh > 0
        assert ev.max_charging_kw is not None and ev.max_charging_kw > 0

    def test_tesla_model_y_lr_specs(self):
        ev = EV.tesla_model_y_lr()
        assert ev.capacity_kwh == 77.0
        assert ev.max_charging_kw == 11.5

    def test_by_vehicle_id_string_lookup(self):
        ev = EV.by_vehicle_id("tesla_model_y_lr")
        assert ev.capacity_kwh == 77.0

    def test_by_vehicle_id_invalid_raises(self):
        with pytest.raises(ValueError):
            EV.by_vehicle_id("nonexistent_vehicle")

    def test_all_shorthands(self):
        shorthands = [
            EV.tesla_model_y_lr,
            EV.tesla_model_y_sr,
            EV.tesla_model_3_lr,
            EV.chevy_bolt_ev,
            EV.chevy_bolt_euv,
            EV.ford_mache_sr,
            EV.ford_mache_er,
            EV.ford_lightning_er,
            EV.hyundai_ioniq5_lr,
            EV.nissan_leaf30,
            EV.jeep_4xe,
            EV.toyota_rav4_prime,
            EV.chevy_volt_gen1,
        ]
        assert len(shorthands) == 13
        for factory in shorthands:
            ev = factory()
            assert ev.capacity_kwh is not None and ev.capacity_kwh > 0

    def test_custom_ev_still_works(self):
        ev = EV("Custom", capacity_kwh=200.0)
        assert ev.name == "Custom"
        assert ev.capacity_kwh == 200.0
