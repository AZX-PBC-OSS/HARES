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
        assert s.total_import_kwh > 0, "House always draws power -- import must be positive"
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
        # Compare with tolerance -- f64 repr may differ at the last digit
        assert len(run1) == len(run2), "Different number of steps"
        for i, (a, b) in enumerate(zip(run1, run2)):
            assert abs(a - b) < 1e-12, f"Step {i}: {a} != {b}"

    def test_phev_archetype_produces_ev_power(self):
        """PHEV vehicle charges from grid -- must show positive EV power."""
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
            f"L1 median power {median_power:.2f} kW too low -- L1 should draw ~1.4 kW"
        )


class TestHpxmlDeclaredEv:
    """An EV declared in the HPXML document itself (no charging_strategy
    element, no override) must be driven and must charge.

    This is the reported surface: construction-time actor registration must
    attach an EvDriverActor for the default `Immediate` strategy, or the EV
    sits parked at full SOC for the whole run and its power column is flat.
    """

    def test_hpxml_declared_ev_default_strategy_charges(self, tmp_path):
        from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS
        from ochre_next import Dwelling

        xml = open(HPXML).read().replace(
            "</Systems>",
            "<ElectricVehicles>"
            "<ElectricVehicle>"
            "<SystemIdentifier id='ev1'/>"
            "<ChargingLevel>Level 2</ChargingLevel>"
            "<MaxChargingPower>7.2</MaxChargingPower>"
            "<BatteryCapacity><Value>60</Value><Units>kWh</Units></BatteryCapacity>"
            "</ElectricVehicle>"
            "</ElectricVehicles>"
            "</Systems>",
        )
        assert "ElectricVehicle" in xml, "injection point </Systems> not found"
        hpxml_path = tmp_path / "with_ev.xml"
        hpxml_path.write_text(xml)

        dw = Dwelling.from_hpxml(
            str(hpxml_path),
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=3 * 86400,
            time_res_s=900,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=42,
            master_seed=42,
            output_verbosity=1,
        )
        dw.initialize()
        df = dw.simulate()

        col = _ev_power_col(df, "EV")
        charge_kwh = df[col].filter(df[col] > 0).sum()
        assert charge_kwh > 10.0, (
            f"HPXML-declared default-strategy EV must charge over 3 days "
            f"(got {charge_kwh:.1f} kWh); a flat column means no driver actor "
            "was attached at construction to simulate driving"
        )


class TestDefaultStrategyEv:
    """An EV with no charging_strategy override (the default `Immediate`)
    must still get vehicle-use simulation and charge.

    The charging strategy selects when/how to charge, never whether the
    vehicle is driven: the auto-registered driver actor is what simulates
    trips and depletes SOC. An EV that is never driven never charges, so a
    permanently flat power column here means the driver was never attached.
    """

    def test_add_ev_default_strategy_charges_with_tariff(self):
        """add_ev (no override) + tariff set afterwards must charge.

        set_electric_tariff re-runs actor auto-registration, which consumes
        the EV's actor seed. Over 3 days of driving the EV must draw a
        substantial, non-zero amount of energy.
        """
        from ochre_next import EV

        tariff = _build_flat_tariff()
        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        dw.set_electric_tariff(tariff)
        df = dw.simulate()

        col = _ev_power_col(df, "EV1")
        charge_kwh = df[col].filter(df[col] > 0).sum()
        assert charge_kwh > 10.0, (
            f"Default-strategy EV must charge over 3 days (got {charge_kwh:.1f} kWh); "
            "a flat column means no driver actor was attached to simulate driving"
        )

    def test_add_ev_default_strategy_charges_without_tariff(self):
        """add_ev (no override, no tariff) must charge.

        Equipment added after dwelling construction never sees the
        construction-time actor registration; add_ev re-runs registration
        itself so the EV is driven whether or not a tariff is ever set.
        """
        from ochre_next import EV

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        df = dw.simulate()

        col = _ev_power_col(df, "EV1")
        charge_kwh = df[col].filter(df[col] > 0).sum()
        assert charge_kwh > 10.0, (
            f"Default-strategy EV must charge over 3 days without a tariff "
            f"(got {charge_kwh:.1f} kWh); a flat column means no driver actor "
            "was attached to simulate driving"
        )

    def test_add_ev_with_driver_set_tariff_after_does_not_duplicate_driver(self):
        """set_electric_tariff after add_ev_with_driver must not attach a
        second, competing driver to the same EV.

        Setting a tariff re-runs actor auto-registration, which consumes the
        EV equipment's actor seed (every charging strategy produces one). The
        manually attached driver claims the canonical "EvDriver:<equipment>"
        name, so the re-run must recognize it and skip — two drivers would
        double the vehicle's driving schedule.
        """
        from ochre_next import EvArchetypeId, VehicleId

        tariff = _build_flat_tariff()
        dw = make_dwelling(duration_s=2 * 86400, time_res_s=900, seed=42)
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        dw.set_electric_tariff(tariff)
        dw.step()

        actors = dw.telemetry().actors()
        drivers = [n for n in actors if "Tesla Model Y LR AWD" in n]
        assert drivers == ["EvDriver:Tesla Model Y LR AWD"], (
            f"expected exactly one driver actor, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )

    def test_two_add_ev_vehicles_each_get_exactly_one_driver(self):
        """A second add_ev re-runs actor auto-registration: the eviction of
        the first EV's auto-registered driver and the rebuild for both EVs
        must leave exactly one driver per vehicle, and both EVs must charge.

        A rebuild that failed to evict, or a dedup keyed coarser than the
        equipment name, would leave a duplicated or missing driver here.
        """
        from ochre_next import EV

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        dw.add_ev(EV("EV2", capacity_kwh=60.0, max_charging_kw=7.2))
        dw.step()

        actors = dw.telemetry().actors()
        drivers = sorted(n for n in actors if n.startswith("EvDriver:"))
        assert drivers == ["EvDriver:EV1", "EvDriver:EV2"], (
            f"expected exactly one driver per EV, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )

        df = dw.simulate()
        for name in ("EV1", "EV2"):
            col = _ev_power_col(df, name)
            charge_kwh = df[col].filter(df[col] > 0).sum()
            assert charge_kwh > 10.0, (
                f"{name} must charge over 3 days (got {charge_kwh:.1f} kWh); "
                "a flat column means its driver was lost on re-registration"
            )

    def test_add_ev_after_add_ev_with_driver_keeps_one_driver_per_vehicle(self):
        """add_ev's auto-registration must coexist with a manually attached
        driver for a different vehicle.

        The manual driver (add_ev_with_driver) is not auto-registered, so
        the eviction step must not remove it, and the dedup must skip its
        vehicle's seed while still building the add_ev vehicle's driver —
        then set_electric_tariff re-runs the whole dance again.
        """
        from ochre_next import EV, EvArchetypeId, VehicleId

        tariff = _build_flat_tariff()
        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev_with_driver(
            VehicleId.tesla_model_y_lr(),
            EvArchetypeId.daily_commuter_l2(),
            seed=42,
        )
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        dw.set_electric_tariff(tariff)
        dw.step()

        actors = dw.telemetry().actors()
        drivers = sorted(n for n in actors if n.startswith("EvDriver:"))
        assert drivers == ["EvDriver:EV1", "EvDriver:Tesla Model Y LR AWD"], (
            f"expected exactly one driver per vehicle, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )

        df = dw.simulate()
        for label in ("EV1", "Tesla Model Y LR AWD"):
            col = _ev_power_col(df, label)
            charge_kwh = df[col].filter(df[col] > 0).sum()
            assert charge_kwh > 10.0, (
                f"{label} must charge over 3 days (got {charge_kwh:.1f} kWh)"
            )

    def test_add_ev_mid_simulation_attaches_driver_and_ev_soc_moves(self):
        """add_ev after the simulation has started must still attach the
        driver and produce a living EV, not a parked one.

        Stepping first means actor auto-registration now runs mid-simulation
        (evicting/rebuilding/prepending actors and rebuilding the schedule
        while the run is in progress) — the EV must still get its driver,
        and its SOC must actually move over the following day.
        """
        from ochre_next import EV

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.step()  # simulation is now in progress
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))

        soc_values = []
        for _ in range(2 * 96):  # two days of 15-min steps
            dw.step()
            tel = dw.telemetry().equipment()
            if "EV1" in tel["names"]:
                idx = tel["names"].index("EV1")
                soc_values.append(tel["soc"][idx])

        actors = dw.telemetry().actors()
        drivers = [n for n in actors if n.startswith("EvDriver:")]
        assert drivers == ["EvDriver:EV1"], (
            f"expected exactly one driver for the mid-run EV, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )
        assert len(soc_values) > 0, "no SOC readings for the mid-run EV"
        soc_range = max(soc_values) - min(soc_values)
        assert soc_range > 0.05, (
            f"mid-run EV SOC must move over two days (range {soc_range:.3f}); "
            "a flat SOC means the driver was never attached or never acted"
        )

    def test_set_tariff_mid_trip_rebuilt_driver_keeps_ev_living(self):
        """set_electric_tariff called mid-run, while the EV is away on its
        trip, evicts the auto-registered driver and rebuilds a fresh one.

        A fresh EvDriverActor is born believing it is home and plugged in
        (phase=HomePluggedIn, no day event) while the equipment is actually
        Disconnected mid-depletion: the in-flight trip's remaining energy is
        abandoned, and the only code path that ever re-plugs the vehicle is
        a later day's full depart -> drive -> arrive cycle. This pins the
        survival invariants after the rebuild: exactly one driver, the
        vehicle keeps driving (SOC moves), and it actually recharges (SOC
        gains > 2% within some 24h window). A frozen SOC or a never-charging
        EV is the stranded-vehicle failure shape.
        """
        from ochre_next import EV

        tariff = _build_flat_tariff()
        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))

        total_steps = 3 * 96  # quarter-hour steps in 3 days

        def ev_soc():
            tel = dw.telemetry().equipment()
            assert "EV1" in tel["names"], (
                f"EV1 missing from equipment telemetry: {tel['names']}"
            )
            return tel["soc"][tel["names"].index("EV1")]

        # Step until the EV is mid-trip: the auto-attached driver departs
        # daily and depletes SOC over its driving steps. Fire the tariff
        # after two consecutive dropping steps so the trip is in progress
        # (equipment Disconnected, drive energy remaining) when actor
        # auto-registration re-runs.
        prev = None
        drops = 0
        steps_done = 0
        while steps_done < total_steps and drops < 2:
            dw.step()
            steps_done += 1
            soc = ev_soc()
            if prev is not None:
                drops = drops + 1 if soc < prev - 0.005 else 0
            prev = soc

        assert drops >= 2, (
            "setup failed: never caught the EV mid-trip (two consecutive "
            f"SOC drops) within {total_steps} steps; the attack is vacuous"
        )

        # Mid-trip: evicts EvDriver:EV1 and rebuilds a fresh driver whose
        # internal state disagrees with the equipment's actual state.
        dw.set_electric_tariff(tariff)

        dw.step()
        steps_done += 1
        actors = dw.telemetry().actors()
        drivers = sorted(n for n in actors if n.startswith("EvDriver:"))
        assert drivers == ["EvDriver:EV1"], (
            f"expected exactly one rebuilt driver, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )

        post_soc = [ev_soc()]
        while steps_done < total_steps:
            dw.step()
            steps_done += 1
            post_soc.append(ev_soc())

        assert len(post_soc) >= 2, "no post-tariff SOC readings"
        post_range = max(post_soc) - min(post_soc)
        assert post_range > 0.05, (
            f"EV SOC must keep moving after the mid-trip rebuild "
            f"(range {post_range:.3f} over {len(post_soc)} steps); a flat "
            "SOC means the rebuilt driver stranded the vehicle (it never "
            "drives again, or never re-plugs to charge)"
        )

        # Charging still occurs: some 24h window post-tariff gains > 2%.
        window = 96  # steps per 24h at 15-min resolution
        best_gain = max(
            post_soc[j] - post_soc[i]
            for i in range(len(post_soc))
            for j in range(i + 1, min(i + window + 1, len(post_soc)))
        )
        assert best_gain > 0.02, (
            f"EV must recharge after the mid-trip rebuild (best 24h-window "
            f"SOC gain {best_gain:.3f}); no gain means the fresh driver, "
            "which assumes home-and-plugged-in, never re-plugged the "
            "vehicle on a later arrival"
        )

    def test_set_tariff_mid_trip_does_not_amputate_in_flight_trip(self):
        """Setting a tariff mid-trip must not silently cancel the rest of
        the in-flight trip.

        The eviction + rebuild in auto_register_actors replaces the
        driver with a fresh actor that has no knowledge of the trip in
        flight; the remaining drive energy of that trip is never
        dispatched. With an Immediate strategy a flat tariff changes no
        charging decision, so an identical control dwelling that never
        sets the tariff must follow the *same* SOC trajectory: any
        divergence after the tariff call is the amputated trip, not
        tariff effects. Both runs share one seed, so the schedules are
        identical and the trajectories are comparable step-for-step.
        """
        from ochre_next import EV

        def run(set_tariff_mid_trip: bool):
            tariff = _build_flat_tariff()
            dw = make_dwelling(
                duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
            )
            dw.initialize()
            dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))

            socs = []
            prev = None
            drops = 0
            armed = False
            for step in range(3 * 96):
                if set_tariff_mid_trip and armed and drops >= 2:
                    dw.set_electric_tariff(tariff)
                    armed = False
                dw.step()
                tel = dw.telemetry().equipment()
                idx = tel["names"].index("EV1")
                socs.append(tel["soc"][idx])
                soc = socs[-1]
                if prev is not None:
                    drops = drops + 1 if soc < prev - 0.005 else 0
                prev = soc
            return socs

        control = run(set_tariff_mid_trip=False)
        attack = run(set_tariff_mid_trip=True)
        assert len(control) == len(attack) == 3 * 96, (
            f"both runs must record every step (got {len(control)} vs "
            f"{len(attack)}); the comparison is meaningless otherwise"
        )

        worst = max(abs(a - c) for a, c in zip(attack, control))
        assert worst < 0.05, (
            f"mid-trip tariff diverged the EV's SOC trajectory from the "
            f"no-tariff control by {worst:.3f} SOC; an Immediate strategy "
            "ignores prices, so the divergence is the in-flight trip being "
            "silently amputated by the actor rebuild"
        )

    def test_add_ev_removed_then_re_added_gets_exactly_one_living_driver(self):
        """remove_equipment after add_ev leaves the auto-attached driver
        orphaned; re-adding an EV with the same name must end with exactly
        one *living* driver for the new equipment.

        The orphan is only reachable because add_ev now attaches actors.
        Re-registration must evict it (it is auto-registered) and build a
        fresh driver for the new equipment. A driver name alone cannot
        distinguish the orphan from a fresh driver (telemetry is
        name-keyed), so the load-bearing assertion is that the re-added
        EV's SOC actually moves: an orphan surviving eviction keeps its
        stale equipment id, the dedup then suppresses the new EV's seed,
        and the re-added EV is never driven.
        """
        from ochre_next import EV

        dw = make_dwelling(
            duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
        )
        dw.initialize()
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))
        dw.remove_equipment("EV1")
        dw.add_ev(EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68))

        soc_values = []
        for _ in range(2 * 96):  # two days of 15-min steps
            dw.step()
            tel = dw.telemetry().equipment()
            if "EV1" in tel["names"]:
                idx = tel["names"].index("EV1")
                soc_values.append(tel["soc"][idx])

        actors = dw.telemetry().actors()
        drivers = [n for n in actors if n.startswith("EvDriver:")]
        assert drivers == ["EvDriver:EV1"], (
            f"expected exactly one driver for the re-added EV, got {drivers}; "
            f"all actors: {sorted(actors)}"
        )
        assert len(soc_values) > 0, "no SOC readings for the re-added EV"
        soc_range = max(soc_values) - min(soc_values)
        assert soc_range > 0.05, (
            f"re-added EV SOC must move over two days (range {soc_range:.3f}); "
            "a flat SOC means the orphaned driver survived eviction and the "
            "new EV was left undriven"
        )

    def test_add_ev_with_driver_removed_then_re_added_drives_exactly_once(self):
        """Re-adding the same vehicle after remove_equipment must not
        double-drive it.

        remove_equipment leaves the manually attached driver orphaned
        (it only removes equipment), and add_actor has no duplicate-name
        guard, so a second add_ev_with_driver for the same vehicle
        attaches a second same-named driver. Telemetry is name-keyed and
        cannot distinguish one driver from two, so the observable is
        energy: the re-added vehicle's charging over 3 days must be
        comparable to a single-add control dwelling -- roughly twice the
        control means two drivers are depleting (and recharging) one
        vehicle, and near zero means the re-added vehicle lost its driver.
        """
        from ochre_next import EvArchetypeId, VehicleId

        def charging_kwh(readd: bool) -> float:
            dw = make_dwelling(
                duration_s=3 * 86400, time_res_s=900, seed=42, output_verbosity=1
            )
            dw.initialize()
            dw.add_ev_with_driver(
                VehicleId.tesla_model_y_lr(),
                EvArchetypeId.daily_commuter_l2(),
                seed=42,
            )
            if readd:
                dw.remove_equipment("Tesla Model Y LR AWD")
                dw.add_ev_with_driver(
                    VehicleId.tesla_model_y_lr(),
                    EvArchetypeId.daily_commuter_l2(),
                    seed=42,
                )
            df = dw.simulate()
            col = _ev_power_col(df, "Tesla Model Y LR AWD")
            return df[col].filter(df[col] > 0).sum()

        control = charging_kwh(readd=False)
        readded = charging_kwh(readd=True)
        assert control > 10.0, (
            f"control dwelling must charge its vehicle (got {control:.1f} kWh); "
            "the comparison is meaningless otherwise"
        )
        assert readded < 1.5 * control, (
            f"re-added vehicle draws {readded:.1f} kWh vs control {control:.1f} kWh; "
            "a second same-named driver is double-driving it"
        )
        assert readded > 0.5 * control, (
            f"re-added vehicle draws {readded:.1f} kWh vs control {control:.1f} kWh; "
            "the re-added vehicle lost its driver entirely"
        )
