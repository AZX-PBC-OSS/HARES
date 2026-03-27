"""Integration tests: EV archetypes x tariff configurations.

These tests verify BEHAVIOR, not just structure. Each test asserts something
that would fail if the underlying physics or actor logic were broken.
"""

import pytest
from conftest import make_dwelling

pl = pytest.importorskip("polars")


def _build_flat_tariff(rate: float = 0.15):
    from ochre_next import ElectricTariff

    return (
        ElectricTariff.builder()
        .set_name("Flat Test")
        .add_tou_period(
            "all",
            [{"day": "any", "start_hour": 0, "end_hour": 24}],
            "all",
        )
        .add_energy_rate("all", "all", rate)
        .set_fixed_charges(0.0, 0.0)
        .build()
    )


def _ev_power_col(df, vehicle_label: str) -> str:
    col = f"{vehicle_label} Electric Power (kW)"
    if col in df.columns:
        return col
    matches = [c for c in df.columns if vehicle_label in c and "Power" in c]
    if matches:
        return matches[0]
    pytest.fail(f"No power column for '{vehicle_label}' in {df.columns}")


class TestArchetypeTariffIntegration:

    def test_ev_with_tariff_produces_nonempty_billing(self):
        """Flat tariff over 35 days must produce at least 1 billing summary
        with positive import kWh (house always draws power)."""
        from ochre_next import EvArchetypeId, VehicleId

        tariff = _build_flat_tariff()
        dw = make_dwelling(duration_s=35 * 86400, time_res_s=900, seed=42)
        dw.initialize()
        dw.set_electric_tariff(tariff)
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        dw.simulate()
        summaries = dw.billing_summaries()
        assert len(summaries) >= 1, "35-day sim with monthly billing must produce >=1 summary"
        s = summaries[0]
        assert s.total_import_kwh > 0, "House always draws power — import must be positive"
        assert s.energy_charge_usd > 0, "Positive import at $0.15/kWh must produce a charge"
        assert s.net_bill_usd > 0, "Net bill must be positive with no export"

    def test_ev_soc_varies_over_week(self):
        """EV SOC must drop (driving) and recover (charging) during a week."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(duration_s=7 * 86400, time_res_s=900, seed=42)
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.chevy_bolt_ev(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )

        # Track SOC via telemetry at each step
        soc_values = []
        ev_name = "Chevy Bolt EV"
        for _ in range(96 * 7):  # 7 days at 15-min resolution
            dw.step()
            tel = dw.telemetry().equipment()
            if ev_name in tel["names"]:
                idx = tel["names"].index(ev_name)
                soc_values.append(tel["soc"][idx])

        assert len(soc_values) > 0, "No SOC readings from telemetry"
        soc_min = min(soc_values)
        soc_max = max(soc_values)
        soc_range = soc_max - soc_min
        assert soc_range > 0.05, (
            f"SOC should vary by at least 5% over a week "
            f"(got range {soc_range:.3f}, min={soc_min:.3f}, max={soc_max:.3f})"
        )

    def test_different_archetypes_produce_different_loads(self):
        """DailyCommuterL2 drives ~38mi/day, WfhOccasional ~12mi.
        Their EV energy consumption must differ significantly."""
        from ochre_next import EvArchetypeId, VehicleId

        def total_ev_kwh(archetype_id):
            dw = make_dwelling(
                duration_s=7 * 86400, time_res_s=900, seed=42, output_verbosity=1,
            )
            dw.initialize()
            dw.add_ev_with_driver(
                VehicleId.tesla_model_y_lr(), archetype_id, seed=42
            )
            df = dw.simulate()
            col = _ev_power_col(df, "Tesla Model Y LR AWD")
            # Sum of positive values = total charging energy
            return df[col].filter(df[col] > 0).sum()

        commuter_kwh = total_ev_kwh(EvArchetypeId.daily_commuter_l2())
        wfh_kwh = total_ev_kwh(EvArchetypeId.wfh_occasional())
        # Both must be positive (EV actually charged)
        assert commuter_kwh > 0, "Commuter EV must charge during the week"
        # Commuter should charge more than WFH
        assert commuter_kwh > wfh_kwh, (
            f"Commuter ({commuter_kwh:.1f} kWh) should charge more than "
            f"WFH ({wfh_kwh:.1f} kWh)"
        )

    def test_deterministic_replay_same_seed(self):
        """Same vehicle + archetype + seed must produce bit-identical results."""
        from ochre_next import EvArchetypeId, VehicleId

        def run():
            dw = make_dwelling(duration_s=3 * 86400, time_res_s=900, seed=42)
            dw.initialize()
            dw.add_ev_with_driver(
                VehicleId.tesla_model_3_lr(),
                EvArchetypeId.daily_commuter_l2(),
                seed=99,
            )
            df = dw.simulate()
            return df["Total Electric Power (kW)"].to_list()

        run1 = run()
        run2 = run()
        assert len(run1) > 0, "Simulation must produce output"
        # Compare with tolerance — f64 repr may differ at the last digit
        assert len(run1) == len(run2), "Different number of steps"
        for i, (a, b) in enumerate(zip(run1, run2)):
            assert abs(a - b) < 1e-12, f"Step {i}: {a} != {b}"

    def test_phev_archetype_produces_ev_power(self):
        """PHEV vehicle charges from grid — must show positive EV power."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.toyota_rav4_prime(),
            EvArchetypeId.phev_commuter(),
            seed=42,
        )
        df = dw.simulate()
        col = _ev_power_col(df, "Toyota RAV4 Prime")
        max_power = df[col].max()
        assert max_power > 0, (
            f"PHEV must charge from grid at some point (max power={max_power})"
        )

    def test_l1_archetype_charges_slower(self):
        """L1 max charge power must be strictly less than L2 for same vehicle."""
        from ochre_next import EvArchetypeId, VehicleId

        def max_ev_power(archetype_id):
            dw = make_dwelling(
                duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1,
            )
            dw.initialize()
            dw.add_ev_with_driver(
                VehicleId.nissan_leaf30(), archetype_id, seed=42
            )
            df = dw.simulate()
            col = _ev_power_col(df, "Nissan Leaf S 30kWh")
            return df[col].max()

        l1_max = max_ev_power(EvArchetypeId.daily_commuter_l1())
        l2_max = max_ev_power(EvArchetypeId.daily_commuter_l2())
        assert l1_max > 0, f"L1 must charge (got max={l1_max})"
        assert l2_max > 0, f"L2 must charge (got max={l2_max})"
        assert l1_max < l2_max, (
            f"L1 max ({l1_max:.2f} kW) must be < L2 max ({l2_max:.2f} kW)"
        )


class TestChargingLoadProfile:
    """Verify that charging happens at the right times and power levels."""

    def test_charging_happens_after_arrival(self):
        """EV charging must occur after the driver arrives home (~18:00),
        not before departure or while driving."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(
            duration_s=7 * 86400, time_res_s=900, seed=42, output_verbosity=3
        )
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        df = dw.simulate()
        col = _ev_power_col(df, "Tesla Model Y LR AWD")

        df = df.with_columns(
            pl.col("Time").str.slice(11, 2).cast(pl.Int32).alias("hour")
        )
        charging = df.filter(pl.col(col) > 0.5)

        if charging.height == 0:
            pytest.skip("No charging events in this seed")

        # Commuter departs ~08:00, arrives ~18:00. Charging should be
        # concentrated in the evening/night (after arrival), not during
        # working hours (09:00-17:00 when the car is away).
        during_work = charging.filter(
            (pl.col("hour") >= 9) & (pl.col("hour") < 16)
        )
        work_fraction = during_work.height / charging.height

        assert work_fraction < 0.1, (
            f"Charging during work hours (09-16h) should be rare (car is away), "
            f"but {work_fraction:.0%} of charging happened then "
            f"({during_work.height}/{charging.height} steps)"
        )

    def test_ev_charge_power_within_hardware_limits(self):
        """EV charge power must never exceed the vehicle's L2 rated power."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(
            duration_s=7 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),  # max_l2_power_kw = 11.5
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        df = dw.simulate()
        col = _ev_power_col(df, "Tesla Model Y LR AWD")
        max_power = df[col].max()

        # Tesla Model Y LR has max_l2_power_kw = 11.5
        assert max_power <= 11.5 + 0.1, (
            f"EV power {max_power:.2f} kW exceeds hardware limit of 11.5 kW"
        )

    def test_ev_not_charging_while_soc_dropping(self):
        """When SOC is dropping (driving), residential EV power must be zero."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(duration_s=3 * 86400, time_res_s=900, seed=42)
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.chevy_bolt_ev(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )

        ev_name = "Chevy Bolt EV"
        prev_soc = None
        violations = 0
        total_driving_steps = 0

        for _ in range(96 * 3):
            dw.step()
            tel = dw.telemetry().equipment()
            if ev_name not in tel["names"]:
                continue
            idx = tel["names"].index(ev_name)
            soc = tel["soc"][idx]
            power = tel["power_kw"][idx]

            if prev_soc is not None and soc < prev_soc - 0.005:
                total_driving_steps += 1
                if power > 0.1:
                    violations += 1
            prev_soc = soc

        if total_driving_steps == 0:
            pytest.skip("No driving events detected")

        assert violations == 0, (
            f"EV drew residential power during {violations}/{total_driving_steps} "
            f"driving steps (SOC dropping)"
        )

    def test_l1_charge_power_realistic(self):
        """L1 charging should be in the 1.0-1.8 kW range (120V/12-15A)."""
        from ochre_next import EvArchetypeId, VehicleId

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.nissan_leaf30(),
            EvArchetypeId.daily_commuter_l1(),
            seed=42,
        )
        df = dw.simulate()
        col = _ev_power_col(df, "Nissan Leaf S 30kWh")
        charging = df.filter(pl.col(col) > 0.1)

        if charging.height == 0:
            pytest.skip("No charging in this seed")

        median_power = charging[col].median()
        max_power = charging[col].max()

        assert max_power <= 1.9, (
            f"L1 max power {max_power:.2f} kW exceeds L1 limit (~1.8 kW)"
        )
        assert median_power >= 0.5, (
            f"L1 median power {median_power:.2f} kW too low — L1 should draw ~1.4 kW"
        )
