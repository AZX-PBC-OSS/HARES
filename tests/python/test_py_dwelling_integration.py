"""Integration tests for PyDwelling lifecycle and GIL safety.

Exercises the full dwelling lifecycle: construction → configuration → control
injection → stepping → metrics extraction → checkpoint/restore. Also includes
GIL safety stress tests for batch_step.
"""

import math
import threading
from datetime import datetime

import pytest

from conftest import make_dwelling

pl = pytest.importorskip("polars")


def _init_dwelling(**kw):
    dw = make_dwelling(**kw)
    dw.initialize()
    return dw


@pytest.fixture(scope="module")
def initialized_dwelling():
    """Single initialized dwelling shared across read-only test classes."""
    return _init_dwelling()


# ---------------------------------------------------------------------------
# Construction round-trip
# ---------------------------------------------------------------------------


class TestConstructionRoundTrip:
    def test_from_hpxml_returns_config(self, initialized_dwelling):
        dw = initialized_dwelling
        cfg = dw.config()
        assert cfg.duration_s == 300
        assert cfg.time_res_s == 60
        assert cfg.master_seed == 0


# ---------------------------------------------------------------------------
# Full simulation
# ---------------------------------------------------------------------------


class TestFullSimulation:
    def test_simulate_returns_nonempty_dataframe(self):
        duration_s = 300
        time_res_s = 60
        dw = _init_dwelling(duration_s=duration_s, time_res_s=time_res_s)
        df = dw.simulate()
        assert isinstance(df, pl.DataFrame)
        expected_rows = duration_s // time_res_s
        assert df.height == expected_rows, (
            f"Expected {expected_rows} rows, got {df.height}"
        )
        assert "Total Electric Power (kW)" in df.columns
        total_power = df["Total Electric Power (kW)"]
        assert total_power.is_not_nan().all(), "Total electric power has NaN values"
        assert total_power.sum() > 0, (
            "January Denver heating case should have positive total electric power"
        )

    def test_simulate_expected_columns(self):
        dw = _init_dwelling(output_verbosity=3)
        df = dw.simulate()
        cols = df.columns
        assert "Total Electric Power (kW)" in cols
        first_power = df["Total Electric Power (kW)"].to_list()[0]
        assert first_power is not None, "First power value should not be None"
        # verbosity >= 2: zone temperature columns with realistic values
        temp_cols = [c for c in cols if c.startswith("Temperature -")]
        assert len(temp_cols) > 0
        for tc in temp_cols:
            col = df[tc].drop_nulls()
            if col.len() > 0:
                assert col.min() >= 10.0, f"{tc} has unrealistically low temp: {col.min()}"
                assert col.max() <= 30.0, f"{tc} has unrealistically high temp: {col.max()}"
                break
        # verbosity >= 3: equipment mode columns
        mode_cols = [c for c in cols if c.endswith("Mode (-)")]
        assert len(mode_cols) > 0

    def test_verbosity_2_includes_outdoor_temp(self):
        dw = _init_dwelling(output_verbosity=2)
        df = dw.simulate()
        assert "Outdoor Dry Bulb (C)" in df.columns
        col = df["Outdoor Dry Bulb (C)"]
        assert col.null_count() == 0
        assert col.min() > -50.0
        assert col.max() < 60.0

    def test_verbosity_1_excludes_outdoor_temp(self):
        dw = _init_dwelling(output_verbosity=1)
        df = dw.simulate()
        assert "Outdoor Dry Bulb (C)" not in df.columns


# ---------------------------------------------------------------------------
# Step-by-step
# ---------------------------------------------------------------------------


class TestStepByStep:
    def test_step_loop_returns_expected_keys(self):
        dw = _init_dwelling(duration_s=180, time_res_s=60)
        for _ in range(3):
            result = dw.step()
            assert "timestamp" in result
            assert isinstance(result["timestamp"], datetime)
            power_keys = [k for k in result if "net_electric_power" in k]
            assert len(power_keys) > 0
            temp_keys = [k for k in result if "Temperature" in k]
            assert len(temp_keys) > 0


# ---------------------------------------------------------------------------
# Control injection — all ControlSignal variants
# ---------------------------------------------------------------------------


def _find_thermal_equipment(dw) -> str:
    from ochre_next import ControlCapabilities

    descs = dw.equipment_descriptors()
    for d in descs:
        if ControlCapabilities.THERMAL_SETPOINT in d.control_capabilities:
            return d.name
    raise RuntimeError("No equipment with THERMAL_SETPOINT capability found")


class TestControlInjection:
    def test_thermal_setpoint_applied(self):
        from ochre_next import ControlSignal

        # Run baseline (default setpoints) and capture final zone temperature
        dw_base = _init_dwelling(duration_s=600, time_res_s=60)
        name_base = _find_thermal_equipment(dw_base)
        for _ in range(6):
            baseline_result = dw_base.step()
        temp_keys = [k for k in baseline_result if "Temperature" in k]
        assert len(temp_keys) > 0
        baseline_final_temp = baseline_result[temp_keys[0]]

        # Run with a heating setpoint well above default and capture final temperature.
        # January Denver: the zone starts around 20°C; setting heat_c=25 forces the
        # furnace to run harder, which raises the zone temp vs the baseline.
        dw_heat = _init_dwelling(duration_s=600, time_res_s=60)
        name_heat = _find_thermal_equipment(dw_heat)
        signal = ControlSignal.thermal_setpoint(heat_c=25.0, cool_c=30.0)
        dw_heat.apply_control(name_heat, signal)
        for _ in range(6):
            heated_result = dw_heat.step()
        heated_final_temp = heated_result[temp_keys[0]]

        # Zone should be meaningfully warmer when heating setpoint is 25°C vs default
        assert heated_final_temp > baseline_final_temp, (
            f"Zone temp with heat_c=25 ({heated_final_temp:.2f}°C) should exceed "
            f"baseline ({baseline_final_temp:.2f}°C) in January Denver"
        )

    def test_soc_target_charges_battery(self):
        """Applying SOCTarget to a half-charged battery must increase its SOC.

        Uses a July start so the battery cell temperature is above the
        0°C charge lockout threshold (Li-ion safety: no charging below 0°C).
        """
        from ochre_next import Battery, ControlSignal

        dw = _init_dwelling(
            duration_s=600, time_res_s=60, start_time="2019-07-01T12:00:00"
        )
        bat = Battery("Bat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.3)
        dw.add_battery(bat)
        dw.apply_control("Bat", ControlSignal.soc_target(target=0.9))
        for _ in range(5):
            dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("Bat")
        soc = tel["soc"][idx]
        assert soc > 0.3, (
            f"Battery SOC should increase toward target 0.9, got {soc}. "
            f"If stuck at 0.3, check cell_temp vs min_charge_temp_c lockout."
        )
        power = tel["power_kw"][idx]
        assert power > 0, f"Battery should be drawing power to charge, got {power}"

    def test_control_reflected_in_telemetry(self):
        from ochre_next import ControlSignal

        heat_c = 21.0
        cool_c = 25.0

        dw = _init_dwelling()
        name = _find_thermal_equipment(dw)
        signal = ControlSignal.thermal_setpoint(heat_c=heat_c, cool_c=cool_c)
        dw.apply_control(name, signal)
        dw.step()
        t = dw.telemetry()

        equip = t.equipment()
        assert name in equip["names"]
        idx = equip["names"].index(name)
        assert math.isfinite(equip["power_kw"][idx])

        zone = t.zone()
        assert any(
            abs(sp - heat_c) < 1e-9 for sp in zone["setpoint_heat_c"]
        ), f"Expected heating setpoint {heat_c} in zone telemetry, got {zone['setpoint_heat_c']}"
        assert any(
            abs(sp - cool_c) < 1e-9 for sp in zone["setpoint_cool_c"]
        ), f"Expected cooling setpoint {cool_c} in zone telemetry, got {zone['setpoint_cool_c']}"

    def test_validate_control(self):
        from ochre_next import ControlSignal

        dw = _init_dwelling()
        name = _find_thermal_equipment(dw)
        valid = ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
        assert dw.validate_control(name, valid) is True

        # Negative cases
        invalid = ControlSignal.soc_target(target=0.5)
        assert dw.validate_control(name, invalid) is False
        assert dw.validate_control("Nonexistent Equipment", valid) is False

    def test_all_control_signal_variants_construct(self):
        from ochre_next import (
            ControlSignal,
            DRLevel,
            DutyCycleComponent,
            InverterPriority,
            OperatingMode,
        )

        signals = [
            ControlSignal.power_setpoint(1.5),
            ControlSignal.power_setpoint(1.5, 0.5),
            ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0),
            ControlSignal.thermal_setpoint_delta(heating_delta_c=1.0),
            ControlSignal.ideal_capacity(5000.0),
            ControlSignal.load_fraction(0.75),
            ControlSignal.mode_override(OperatingMode.Heating),
            ControlSignal.mode_override_str("Cooling"),
            ControlSignal.demand_response(DRLevel.High),
            ControlSignal.demand_response_str("Normal"),
            ControlSignal.soc_target(target=0.8, min=0.2, max=1.0),
            ControlSignal.humidity_setpoint(target_rh=50.0),
            ControlSignal.power_limit(max_power_kw=5.0),
            ControlSignal.duty_cycle(on_fraction=0.5, period_s=300.0),
            ControlSignal.duty_cycle(
                on_fraction=0.5, component=DutyCycleComponent.Compressor
            ),
            ControlSignal.grid_connect(connected=True),
            ControlSignal.self_consumption(enabled=True, solar_only_charging=False),
            ControlSignal.curtailment_percent(50.0),
            ControlSignal.reactive_setpoint(kvar=1.0),
            ControlSignal.power_factor_setpoint(0.95),
            ControlSignal.inverter_priority_mode(InverterPriority.Watt),
            ControlSignal.protocol_native(42, b"\x01\x02"),
            ControlSignal.ideal_capacity_mode_override("auto"),
        ]
        for sig in signals:
            d = sig.to_dict()
            assert "type" in d
            rt = ControlSignal.from_dict(d)
            assert rt.to_dict()["type"] == d["type"]


# ---------------------------------------------------------------------------
# Price signal and grid voltage
# ---------------------------------------------------------------------------


class TestPriceAndGridVoltage:
    def test_set_price_signal(self):
        dw = _init_dwelling()
        dw.set_price_signal({"electricity_price": 0.15})
        result = dw.step()
        assert "timestamp" in result

    def test_set_grid_voltage(self):
        dw = _init_dwelling()
        dw.set_grid_voltage(1.05)
        result = dw.step()
        assert "timestamp" in result


# ---------------------------------------------------------------------------
# Telemetry
# ---------------------------------------------------------------------------


class TestTelemetry:
    def test_telemetry_zone_and_equipment(self):
        dw = _init_dwelling()
        dw.step()
        t = dw.telemetry()

        zone = t.zone()
        assert isinstance(zone, dict)
        for key in ("temperature_c", "setpoint_heat_c", "setpoint_cool_c",
                     "outdoor_temp_c", "outdoor_rh"):
            assert key in zone, f"Missing zone key: {key}"

        equip = t.equipment()
        assert isinstance(equip, dict)
        for key in ("names", "modes", "states", "soc", "power_kw"):
            assert key in equip, f"Missing equipment key: {key}"

        power = t.total_power_kw
        assert isinstance(power, float)
        assert math.isfinite(power)


# ---------------------------------------------------------------------------
# Equipment descriptors
# ---------------------------------------------------------------------------


class TestEquipmentDescriptors:
    def test_equipment_descriptors_have_required_fields(self, initialized_dwelling):
        dw = initialized_dwelling
        descs = dw.equipment_descriptors()
        assert isinstance(descs, list)
        assert len(descs) > 0
        for d in descs:
            assert hasattr(d, "name")
            assert hasattr(d, "end_use")
            assert hasattr(d, "fuel_type")
            assert hasattr(d, "control_capabilities")


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------


class TestMetrics:
    def test_metrics_after_simulate(self):
        # Use a 2-hour evening simulation in January — zone will cool below
        # the thermostat turn-on threshold (19.2°C) forcing the gas furnace
        # to run. Midnight start was too warm from prior internal gains.
        dw = _init_dwelling(
            duration_s=7200,
            time_res_s=60,
            output_verbosity=1,
            start_time="2019-01-01T05:00:00",
        )
        dw.simulate()
        m = dw.metrics()
        assert m.annual_energy_kwh is not None
        assert isinstance(m.annual_energy_kwh.total, float)
        assert m.annual_energy_kwh.total > 0, (
            "January Denver simulation must have positive total energy"
        )
        # Check that at least one equipment has nonzero energy
        per_use = m.annual_energy_kwh.per_end_use
        assert any(v > 0 for v in per_use.values()), (
            f"At least one equipment should consume energy; got {per_use}"
        )
        assert m.peak_power_kw is not None
        assert m.peak_power_kw.rolling_15min_kw > 0, "Peak power must be positive"


# ---------------------------------------------------------------------------
# Checkpoint save/restore
# ---------------------------------------------------------------------------


class TestCheckpoint:
    @pytest.mark.xfail(reason="equipment state deserialization not yet supported")
    def test_save_and_load_state(self):
        dw = _init_dwelling(duration_s=600, time_res_s=60)
        dw.step()
        dw.step()

        state = dw.save_state()
        assert isinstance(state, bytes)
        assert len(state) > 0

        # Step a few more times to change state
        dw.step()

        # Restore
        dw.load_state(state)

        # Next step after restore should succeed
        result = dw.step()
        assert "timestamp" in result


# ---------------------------------------------------------------------------
# Deterministic replay via reset_with_seed
# ---------------------------------------------------------------------------


class TestDeterministicReplay:
    def test_reset_with_seed_produces_identical_output(self):
        dw = _init_dwelling(duration_s=300, time_res_s=60)

        dw.reset_with_seed(42)
        r1 = dw.step()

        dw.reset_with_seed(42)
        r2 = dw.step()

        assert r1["timestamp"] == r2["timestamp"]
        p1 = [v for k, v in r1.items() if "net_electric_power" in k]
        p2 = [v for k, v in r2.items() if "net_electric_power" in k]
        assert p1 == p2


# ---------------------------------------------------------------------------
# Timesteps iterator
# ---------------------------------------------------------------------------


class TestTimesteps:
    def test_timesteps_iterator(self):
        dw = _init_dwelling(duration_s=300, time_res_s=60)
        steps = list(dw.timesteps())
        assert len(steps) == 5  # 300 / 60

    def test_timesteps_monotonic(self):
        dw = _init_dwelling(duration_s=600, time_res_s=60)
        steps = list(dw.timesteps())
        for i in range(1, len(steps)):
            assert steps[i] > steps[i - 1], f"Timestamps not monotonic at index {i}"


# ---------------------------------------------------------------------------
# GIL safety stress test — batch_step
# ---------------------------------------------------------------------------


class TestBatchStep:
    def test_batch_step_basic(self):
        from ochre_next import batch_step

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        results = batch_step([dw], [[]], ["total_power_kw"])
        assert isinstance(results, list)
        assert len(results) == 1
        r = results[0]
        assert "obs" in r
        assert "reward" in r
        assert "terminated" in r
        assert "truncated" in r
        assert "info" in r

    def test_batch_step_concurrent_stress(self):
        """Verify no segfault or deadlock under concurrent batch_step calls."""
        from ochre_next import batch_step

        n_dwellings = 4
        dwellings = [
            _init_dwelling(duration_s=600, time_res_s=60, seed=i)
            for i in range(n_dwellings)
        ]

        errors = []

        def worker(dw_list):
            try:
                for _ in range(5):
                    results = batch_step(
                        dw_list,
                        [[]] * len(dw_list),
                        ["total_power_kw"],
                    )
                    for r in results:
                        assert isinstance(r, dict)
                        assert "obs" in r
            except Exception as e:
                errors.append(e)

        # Split dwellings into two groups and step concurrently
        threads = [
            threading.Thread(target=worker, args=(dwellings[:2],)),
            threading.Thread(target=worker, args=(dwellings[2:],)),
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
            assert not t.is_alive(), "Thread deadlocked"

        assert len(errors) == 0, f"Errors in threads: {errors}"

    def test_batch_step_empty_dwellings(self):
        """batch_step with empty dwellings list returns empty results."""
        from ochre_next import batch_step

        results = batch_step([], [], ["total_power_kw"])
        assert isinstance(results, list)
        assert len(results) == 0

    def test_batch_step_8_dwellings_4_threads(self):
        """Stress test with 8 dwellings across 4 threads."""
        from ochre_next import batch_step

        dwellings = [
            _init_dwelling(duration_s=600, time_res_s=60, seed=i)
            for i in range(8)
        ]

        errors = []

        def worker(dw_list):
            try:
                for _ in range(3):
                    results = batch_step(
                        dw_list,
                        [[]] * len(dw_list),
                        ["total_power_kw"],
                    )
                    assert len(results) == len(dw_list)
                    for r in results:
                        assert isinstance(r, dict)
                        assert "obs" in r
            except Exception as e:
                errors.append(e)

        groups = [dwellings[i : i + 2] for i in range(0, 8, 2)]
        threads = [threading.Thread(target=worker, args=(g,)) for g in groups]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
            assert not t.is_alive(), "Thread deadlocked"

        assert len(errors) == 0, f"Errors in threads: {errors}"


# ---------------------------------------------------------------------------
# Solar override lifecycle
# ---------------------------------------------------------------------------


class TestSolarOverride:
    def test_solar_override_lifecycle(self):
        from ochre_next import PV

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        dw.add_pv(PV("TestPV", 5.0, 30.0, 180.0))

        surface_ids = dw.surface_ids()
        assert isinstance(surface_ids, list)

        # Step once without override
        dw.step()
        assert not dw.has_solar_override()

        # Build synthetic POA data as a dict keyed by surface ID
        # Requires keys: direct, diffuse, reflected, aoi per surface
        n_steps = 9  # remaining steps
        poa_data = {
            sid: {
                "direct": [400.0] * n_steps,
                "diffuse": [80.0] * n_steps,
                "reflected": [20.0] * n_steps,
                "aoi": [0.5] * n_steps,
            }
            for sid in surface_ids
        }
        dw.set_solar_override(poa_data)
        assert dw.has_solar_override()

        dw.step()

        dw.clear_solar_override()
        assert not dw.has_solar_override()

        dw.step()


# ---------------------------------------------------------------------------
# Battery LUT injection
# ---------------------------------------------------------------------------


class TestBatteryLutInjection:
    def test_add_battery_with_ocv_table(self):
        from ochre_next import Battery

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        bat = Battery("B1", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0)
        dw.add_battery(bat)

        # Set an OCV table (simpler than 4D charging curve LUT)
        ocv_data = [
            (0.0, 3.0), (0.2, 3.3), (0.4, 3.5),
            (0.6, 3.7), (0.8, 3.9), (1.0, 4.2),
        ]
        dw.update_equipment("B1", ocv_table=ocv_data)

        # Step should succeed
        for _ in range(5):
            result = dw.step()
            assert "timestamp" in result


# ---------------------------------------------------------------------------
# EV with charging curve
# ---------------------------------------------------------------------------


class TestEvLifecycle:
    def test_add_ev_and_step(self):
        from ochre_next import EV

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68)
        dw.add_ev(ev)
        assert "EV1" in dw.equipment_names()

        for _ in range(3):
            result = dw.step()
            assert "timestamp" in result

    def test_add_ev_with_charging_curve_lut(self):
        import numpy as np

        from ochre_next import EV, LutType

        # Build a minimal 4D LUT: soc × temp × c_rate × soh → power_fraction
        soc_grid = np.array([0.0, 0.5, 1.0])
        temp_grid = np.array([25.0])
        crate_grid = np.array([0.5, 1.0])
        soh_grid = np.array([1.0])
        # Shape: (3, 1, 2, 1) = 6 values — taper at high SoC
        lut = np.array([1.0, 1.0, 0.8, 0.8, 0.1, 0.1], dtype=np.float32).reshape(
            (3, 1, 2, 1)
        )

        cc_cv_lut = {
            "soc_grid": soc_grid,
            "temp_grid": temp_grid,
            "crate_grid": crate_grid,
            "soh_grid": soh_grid,
            "lut": lut,
        }

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=75.0, max_charging_kw=7.68, charging_curve_lut=cc_cv_lut)
        dw.add_ev(ev)
        assert "EV1" in dw.equipment_names()
        assert dw.has_equipment_lut("EV1", LutType.charging_curve())

        for _ in range(5):
            result = dw.step()
            assert "timestamp" in result


# ---------------------------------------------------------------------------
# Equipment mutation round-trip
# ---------------------------------------------------------------------------


class TestEvControlSignals:
    def test_ev_plug_in_disconnected_zero_power(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=60.0, max_charging_kw=7.2, initial_soc=0.3)
        dw.add_ev(ev)
        dw.apply_control("EV1", ControlSignal.ev_plug_in(EvConnectionState.Disconnected))
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("EV1")
        assert tel["power_kw"][idx] == 0.0

    def test_ev_drive_deducts_soc(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=75.0, initial_soc=0.5)
        dw.add_ev(ev)
        # Disconnect then drive
        dw.apply_control("EV1", ControlSignal.ev_plug_in(EvConnectionState.Disconnected))
        dw.apply_control("EV1", ControlSignal.ev_drive(10.0))
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("EV1")
        soc = tel["soc"][idx]
        # SOC should drop by ~10/75 ≈ 0.133
        assert soc < 0.5, f"SOC should decrease after driving, got {soc}"
        assert abs(soc - (0.5 - 10.0 / 75.0)) < 0.02

    def test_ev_away_charge_no_residential_power(self):
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=60.0, initial_soc=0.3)
        dw.add_ev(ev)
        dw.apply_control("EV1", ControlSignal.ev_plug_in(EvConnectionState.Disconnected))
        dw.step()
        dw.apply_control("EV1", ControlSignal.ev_plug_in(EvConnectionState.AwayPluggedIn))
        dw.apply_control("EV1", ControlSignal.ev_away_charge(11.5))
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("EV1")
        # Residential power should be 0 (away charging)
        assert tel["power_kw"][idx] == 0.0
        # SOC should increase
        assert tel["soc"][idx] > 0.3

    def test_ev_home_plugged_in_charges_by_default(self):
        """EV defaults to HomePluggedIn and charges to soc_max without any actor."""
        from ochre_next import EV

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        ev = EV("EV1", capacity_kwh=60.0, max_charging_kw=7.2, initial_soc=0.3)
        dw.add_ev(ev)
        for _ in range(5):
            dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("EV1")
        assert tel["soc"][idx] > 0.3, "EV should charge when plugged in at home"

    def test_ev_drive_while_home_plugged_in_raises(self):
        """EvDrive must be rejected when not Disconnected."""
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = _init_dwelling(duration_s=300, time_res_s=60)
        ev = EV("EV1", capacity_kwh=60.0, initial_soc=0.5,
                initial_connection_state=EvConnectionState.HomePluggedIn)
        dw.add_ev(ev)
        with pytest.raises((ValueError, RuntimeError)):
            dw.apply_control("EV1", ControlSignal.ev_drive(5.0))

    def test_ev_away_charge_while_home_raises(self):
        """EvAwayCharge must be rejected when not AwayPluggedIn."""
        from ochre_next import ControlSignal, EV, EvConnectionState

        dw = _init_dwelling(duration_s=300, time_res_s=60)
        ev = EV("EV1", capacity_kwh=60.0, initial_soc=0.3,
                initial_connection_state=EvConnectionState.HomePluggedIn)
        dw.add_ev(ev)
        with pytest.raises((ValueError, RuntimeError)):
            dw.apply_control("EV1", ControlSignal.ev_away_charge(7.2))


class TestEquipmentMutationRoundTrip:
    def test_add_battery_set_lut_save_load_remove(self):
        from ochre_next import Battery

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        bat = Battery("B1", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0)
        dw.add_battery(bat)
        assert "B1" in dw.equipment_names()

        # Set a LUT
        ocv_data = [(0.0, 3.0), (0.5, 3.5), (1.0, 4.2)]
        dw.update_equipment("B1", ocv_table=ocv_data)

        dw.step()

        # Remove battery
        dw.remove_equipment("B1")
        assert "B1" not in dw.equipment_names()

        # Step after removal should still work
        result = dw.step()
        assert "timestamp" in result

    @pytest.mark.xfail(
        reason="checkpoint deserialization does not yet support dynamically added equipment"
    )
    def test_lut_persists_through_checkpoint(self):
        from ochre_next import Battery

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        bat = Battery("B1", 10.0)
        dw.add_battery(bat)
        dw.update_equipment("B1", ocv_table=[(0.0, 3.0), (1.0, 4.2)])

        state = dw.save_state()
        dw.load_state(state)

        assert "B1" in dw.equipment_names()


# ---------------------------------------------------------------------------
# Actor system — Python subclass
# ---------------------------------------------------------------------------


class TestActorSystem:
    def test_python_actor_subclass(self):
        from ochre_next import Actor, DispatchRequest

        dw = _init_dwelling(duration_s=300, time_res_s=60)
        thermal_target = _find_thermal_equipment(dw)

        class MyThermostat(Actor):
            def __init__(self):
                super().__init__()
                self.name = "MyThermostat"
                self._target = thermal_target

            def decide(self, env):
                return [
                    DispatchRequest.thermal_setpoint(
                        target=self._target,
                        heating_c=20.0,
                        cooling_c=24.0,
                    )
                ]

        actor = MyThermostat()
        dw.add_actor(actor)

        for _ in range(3):
            result = dw.step()
            assert "timestamp" in result

    def test_builtin_actor_by_name(self):
        dw = _init_dwelling(duration_s=300, time_res_s=60)
        dw.add_actor_by_name("IdealThermostat", "thermo1", {})

        for _ in range(3):
            result = dw.step()
            assert "timestamp" in result


# ---------------------------------------------------------------------------
# DER simulation explorer workflow (end-to-end)
# ---------------------------------------------------------------------------


class TestDerSimulationExplorer:
    @pytest.mark.slow
    def test_pv_battery_ev_end_to_end(self):
        """End-to-end DER simulation matching the simulation_explorer.py workflow."""
        from ochre_next import ControlSignal
        from ochre_next import EV, PV, Battery

        # 2-week simulation at 15-min resolution
        two_weeks_s = 14 * 24 * 3600
        dw = make_dwelling(
            duration_s=two_weeks_s,
            time_res_s=900,  # 15 minutes
            seed=0,
            output_verbosity=3,
        )
        dw.initialize()

        # Add PV
        dw.add_pv(PV("PV", 6.0, 26.0, 180.0))

        # Add Battery
        bat = Battery("Battery", 13.5, max_charge_kw=5.0, max_discharge_kw=5.0)
        dw.add_battery(bat)

        # Self-consumption control
        dw.apply_control(
            "Battery",
            ControlSignal.self_consumption(True, False),
        )

        # Add EV
        ev = EV("EV", capacity_kwh=75.0, max_charging_kw=7.68)
        dw.add_ev(ev)

        # Run simulation
        df = dw.simulate()
        assert isinstance(df, pl.DataFrame)

        cols = df.columns
        # Check expected columns exist
        assert "Total Electric Power (kW)" in cols
        assert "PV Electric Power (kW)" in cols
        assert "Battery Electric Power (kW)" in cols
        assert "Battery SOC (-)" in cols
        assert "EV Electric Power (kW)" in cols
        assert "EV SOC (-)" in cols

        # PV should produce power (positive in telemetry convention)
        pv_col = df["PV Electric Power (kW)"]
        assert pv_col.max() > 0, "PV should produce power during daylight"

        # Battery SOC should be in [0, 1]
        bat_soc = df["Battery SOC (-)"]
        assert bat_soc.min() >= 0.0
        assert bat_soc.max() <= 1.0

        # Total electric power should have negative values (export from PV)
        total = df["Total Electric Power (kW)"]
        assert total.min() < 0, "Should have export (negative) power from PV"
        assert total.max() > 0, "Should have import (positive) power"
        assert total.is_not_nan().all(), "Net import must be finite (no NaN)"

        # Battery SOC should have variance — it charged/discharged at least once
        bat_soc_std = bat_soc.std()
        assert bat_soc_std > 0, (
            "Battery SOC should vary over 2-week simulation (charged/discharged)"
        )

        # PV total generation sum should be positive (producing power)
        pv_sum = pv_col.sum()
        assert pv_sum > 0, (
            f"PV total generation should be positive (producing), got {pv_sum}"
        )

        # Expected row count: 2 weeks × 96 steps/day = 1344
        expected_rows = 14 * 96
        assert df.height == expected_rows, (
            f"Expected {expected_rows} rows, got {df.height}"
        )
