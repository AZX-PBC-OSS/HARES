"""Tests for EV archetype presets (DER-010)."""

import pytest
from ochre_next import EV, EvArchetypeId, VehicleId

from conftest import make_dwelling


ARCHETYPE_IDS = EvArchetypeId.archetype_catalog()


class TestEvArchetypeId:
    def test_catalog_has_12_archetypes(self):
        assert len(ARCHETYPE_IDS) == 12

    @pytest.mark.parametrize("aid", ARCHETYPE_IDS)
    def test_from_str_round_trips(self, aid):
        assert EvArchetypeId.from_str(str(aid)) == aid

    def test_from_str_case_insensitive(self):
        assert EvArchetypeId.from_str("daily_commuter_l2") == EvArchetypeId.daily_commuter_l2()
        assert EvArchetypeId.from_str("DAILY_COMMUTER_L2") == EvArchetypeId.daily_commuter_l2()

    def test_repr(self):
        assert "DailyCommuterL2" in repr(EvArchetypeId.daily_commuter_l2())

    def test_from_str_invalid_raises(self):
        with pytest.raises(ValueError):
            EvArchetypeId.from_str("nonexistent_archetype")


class TestFromVehicleWithArchetype:
    @pytest.mark.parametrize("aid", ARCHETYPE_IDS)
    def test_all_archetypes_create_valid_ev(self, aid):
        ev = EV.from_vehicle_with_archetype(VehicleId.tesla_model_y_lr(), aid, seed=42)
        assert ev.capacity_kwh is not None and ev.capacity_kwh > 0
        assert ev.max_charging_kw is not None and ev.max_charging_kw > 0

    def test_l1_archetype_caps_charging_power(self):
        ev = EV.from_vehicle_with_archetype(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l1(),
            seed=0,
        )
        assert ev.max_charging_kw <= 1.8

    def test_l2_archetype_preserves_vehicle_power(self):
        ev = EV.from_vehicle_with_archetype(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=0,
        )
        assert ev.max_charging_kw == 11.5


class TestAddEvWithDriver:
    def test_creates_both_ev_and_actor(self):
        dw = make_dwelling()
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        names = dw.equipment_names()
        assert any("Tesla Model Y" in n for n in names)

    def test_steps_without_error(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.chevy_bolt_ev(),
            EvArchetypeId.wfh_occasional(),
            seed=7,
        )
        for _ in dw.timesteps():
            pass

    def test_deterministic_with_same_seed(self):
        results = []
        for _ in range(2):
            dw = make_dwelling(duration_s=600, time_res_s=60)
            dw.initialize()
            dw.add_ev_with_driver(
                VehicleId.tesla_model_y_lr(),
                EvArchetypeId.daily_commuter_l2(),
                seed=123,
            )
            df = dw.simulate()
            results.append(df)
        assert results[0].equals(results[1])


class TestCustomConstructionStillWorks:
    def test_custom_ev_independent_of_archetypes(self):
        ev = EV("CustomEV", capacity_kwh=100.0, max_charging_kw=7.2)
        assert ev.name == "CustomEV"
        assert ev.capacity_kwh == 100.0


class TestEvFullDayCycle:
    def test_ev_full_day_cycle(self):
        dw = make_dwelling(duration_s=1500 * 60, time_res_s=60)
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )

        soc_values: list[float] = []
        for _ in dw.timesteps():
            soc = dw.get_equipment_telemetry("Tesla Model Y LR", "soc")
            if soc is not None:
                soc_values.append(soc)

        assert len(soc_values) > 0, "no SOC telemetry recorded"
        assert all(0.0 <= s <= 1.0 for s in soc_values), (
            f"SOC out of [0, 1]: min={min(soc_values):.4f}, max={max(soc_values):.4f}"
        )
        assert soc_values[0] != soc_values[-1], (
            f"SOC did not change over simulation: {soc_values[0]:.4f} -> {soc_values[-1]:.4f}"
        )
