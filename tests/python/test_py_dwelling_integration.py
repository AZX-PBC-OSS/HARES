"""Integration tests for PyDwelling lifecycle and GIL safety.

Exercises the full dwelling lifecycle: construction → configuration → control
injection → stepping → metrics extraction → checkpoint/restore. Also includes
GIL safety stress tests for batch_step.
"""

import threading
from datetime import datetime, timedelta
from pathlib import Path

import pytest

pl = pytest.importorskip("polars")

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)
SCHEDULE = str(
    ROOT / "vendors/OCHRE/ochre/defaults/Input Files/BEopt_example_schedule.csv"
)


def _make_dwelling(duration_s=300, time_res_s=60, seed=0, output_verbosity=0, **kw):
    from ochre_next import Dwelling

    return Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration_s=duration_s,
        time_res_s=time_res_s,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=seed,
        output_verbosity=output_verbosity,
        **kw,
    )


def _init_dwelling(**kw):
    dw = _make_dwelling(**kw)
    dw.initialize()
    return dw


# ---------------------------------------------------------------------------
# Construction round-trip
# ---------------------------------------------------------------------------


class TestConstructionRoundTrip:
    def test_from_hpxml_returns_config(self):
        dw = _init_dwelling()
        cfg = dw.config()
        assert cfg.duration_s == 300
        assert cfg.time_res_s == 60
        assert cfg.master_seed == 0


# ---------------------------------------------------------------------------
# Full simulation
# ---------------------------------------------------------------------------


class TestFullSimulation:
    def test_simulate_returns_nonempty_dataframe(self):
        dw = _init_dwelling()
        df = dw.simulate()
        assert isinstance(df, pl.DataFrame)
        assert df.height > 0
        assert "Time" in df.columns or any("time" in c.lower() for c in df.columns)


# ---------------------------------------------------------------------------
# Step-by-step
# ---------------------------------------------------------------------------


class TestStepByStep:
    def test_step_loop_returns_expected_keys(self):
        dw = _init_dwelling(duration_s=180, time_res_s=60)
        for _ in range(3):
            result = dw.step()
            assert "time" in result
            assert isinstance(result["time"], datetime)
            power_keys = [k for k in result if "net_electric_power" in k]
            assert len(power_keys) > 0
            temp_keys = [k for k in result if "Temperature" in k]
            assert len(temp_keys) > 0


# ---------------------------------------------------------------------------
# Control injection — all ControlSignal variants
# ---------------------------------------------------------------------------


class TestControlInjection:
    def test_thermal_setpoint_applied(self):
        from ochre_next import ControlSignal

        dw = _init_dwelling()
        signal = ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
        dw.apply_control("HVAC Heating", signal)
        result = dw.step()
        assert "time" in result

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
        assert "time" in result

    def test_set_grid_voltage(self):
        dw = _init_dwelling()
        dw.set_grid_voltage(1.05)
        result = dw.step()
        assert "time" in result


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
        assert "temperature_c" in zone
        assert "outdoor_temp_c" in zone

        equip = t.equipment()
        assert isinstance(equip, dict)

        power = t.total_power_kw()
        assert isinstance(power, float)


# ---------------------------------------------------------------------------
# Equipment descriptors
# ---------------------------------------------------------------------------


class TestEquipmentDescriptors:
    def test_equipment_descriptors_have_required_fields(self):
        dw = _init_dwelling()
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
        dw = _init_dwelling(output_verbosity=1)
        dw.simulate()
        m = dw.metrics()
        assert m.annual_energy_kwh is not None
        assert isinstance(m.annual_energy_kwh.total, float)
        assert m.peak_power_kw is not None


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
        assert "time" in result


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

        assert r1["time"] == r2["time"]
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


# ---------------------------------------------------------------------------
# OCHRE compat layer
# ---------------------------------------------------------------------------


class TestOchreCompat:
    def test_compat_dwelling_simulate(self):
        from ochre_next.compat.dwelling import Dwelling as CompatDwelling

        d = CompatDwelling(
            hpxml_file=HPXML,
            hpxml_schedule_file=SCHEDULE,
            weather_file=WEATHER,
            start_time=datetime(2019, 1, 1),
            time_res=timedelta(minutes=1),
            duration=timedelta(minutes=5),
        )

        df, metrics, df_hourly = d.simulate()
        assert isinstance(df, pl.DataFrame)
        assert df.height > 0
        assert isinstance(metrics, dict)
        assert isinstance(df_hourly, pl.DataFrame)


# ---------------------------------------------------------------------------
# GIL safety stress test — batch_step
# ---------------------------------------------------------------------------


class TestBatchStep:
    def test_batch_step_basic(self):
        from ochre_next import batch_step

        dw = _init_dwelling(duration_s=600, time_res_s=60)
        results = batch_step([dw], [[0.0]], ["total_power_kw"])
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
                        [[0.0]] * len(dw_list),
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
                        [[0.0]] * len(dw_list),
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
            assert "time" in result


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
            assert "time" in result


# ---------------------------------------------------------------------------
# Equipment mutation round-trip
# ---------------------------------------------------------------------------


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
        assert "time" in result

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

        class MyThermostat(Actor):
            def __init__(self):
                super().__init__()
                self.name = "MyThermostat"
                self._target = "HVAC Heating"

            def decide(self, env):
                return [
                    DispatchRequest.thermal_setpoint(
                        target=self._target,
                        heating_c=20.0,
                        cooling_c=24.0,
                    )
                ]

        dw = _init_dwelling(duration_s=300, time_res_s=60)
        actor = MyThermostat()
        dw.add_actor(actor)

        for _ in range(3):
            result = dw.step()
            assert "time" in result

    def test_builtin_actor_by_name(self):
        dw = _init_dwelling(duration_s=300, time_res_s=60)
        dw.add_actor_by_name("IdealThermostat", "thermo1", {})

        for _ in range(3):
            result = dw.step()
            assert "time" in result


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
        dw = _make_dwelling(
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

        # Expected row count: 2 weeks × 96 steps/day = 1344
        expected_rows = 14 * 96
        assert df.height == expected_rows, (
            f"Expected {expected_rows} rows, got {df.height}"
        )
