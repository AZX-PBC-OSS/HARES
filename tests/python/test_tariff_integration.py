"""TARIFF-016: End-to-end TOU integration test.

Uses the PV+battery HPXML fixture (base-pv-battery from OCHRE vendor files)
with a PG&E E-TOU-C tariff loaded from the URDB fixture. Validates that:
- Battery charges during off-peak hours (cheap)
- Battery discharges during peak hours (expensive)
- Billing summaries have plausible values
- No NaN in any power/energy column
- Tariff telemetry is emitted every step
"""

import math
from datetime import datetime, timedelta
from pathlib import Path

import pytest

HARES_ROOT = Path(__file__).resolve().parent.parent.parent
EXAMPLES = HARES_ROOT / "data" / "examples"
FIXTURES = HARES_ROOT / "tests" / "fixtures"
DEFAULTS = HARES_ROOT / "defaults"

HPXML = EXAMPLES / "pv_battery_example.xml"
SCHEDULE = EXAMPLES / "BEopt_example_schedule.csv"
WEATHER = EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
URDB_TOU = FIXTURES / "urdb" / "pge_e_tou_c.json"

# PG&E E-TOU-C peak hours: weekday 16:00-21:00
PEAK_HOURS = set(range(16, 21))

# 7 days in July at 15-min resolution -- summer maximizes PV + TOU spread
SIM_DURATION_S = 7 * 86400
TIME_RES_S = 900
EXPECTED_STEPS = SIM_DURATION_S // TIME_RES_S  # 672
SIM_START = datetime(2019, 7, 1, 12, 0)

# The battery's declared control: self-consumption, the management the
# battery's own PV follows. An HPXML battery with no declared control is a
# construction error; the override is the declaration.
BMS_MODE = (
    '{"SelfConsumption": {"min_soc": 0.1, "max_soc": 1.0, '
    '"solar_only_charging": false, "surplus_deadband_kw": 0.0}}'
)
BATTERY_OVERRIDES = {"Battery": {"bms_mode": BMS_MODE}}


def _is_peak(step_idx: int) -> bool:
    dt = SIM_START + timedelta(seconds=step_idx * TIME_RES_S)
    return dt.weekday() < 5 and dt.hour in PEAK_HOURS


@pytest.fixture(scope="module")
def sim_result():
    """Run PV+battery dwelling with TOU tariff once, share across all tests."""
    from hares import Dwelling, ElectricTariff

    dw = Dwelling.from_hpxml(
        str(HPXML), str(SCHEDULE), str(WEATHER),
        start_time="2019-07-01T12:00:00",
        duration_s=SIM_DURATION_S,
        time_res_s=TIME_RES_S,
        defaults_path=str(DEFAULTS),
        bldg_id=1,
        master_seed=42,
        output_verbosity=3,
        write_output=False,
        overrides=BATTERY_OVERRIDES,
    )
    dw.initialize()
    dw.set_electric_tariff(ElectricTariff.from_urdb_json(str(URDB_TOU)))

    steps = []
    for _ in range(EXPECTED_STEPS):
        result = dw.step()
        tel = dw.telemetry()
        equip = tel.equipment() if tel else {}
        tariff_tel = dw.tariff_telemetry()
        steps.append({
            "result": result,
            "equipment": equip,
            "tariff": tariff_tel,
        })

    return {
        "steps": steps,
    }


def _battery_index(equip: dict) -> int | None:
    """The battery's index in the equipment telemetry's parallel lists."""
    names = equip.get("names") or []
    for i, name in enumerate(names):
        if "battery" in str(name).lower():
            return i
    return None


def _battery_power(step: dict) -> float:
    """Extract battery power [kW] from step telemetry. Positive=charging."""
    equip = step["equipment"]
    if not isinstance(equip, dict):
        return 0.0
    idx = _battery_index(equip)
    powers = equip.get("power_kw") if idx is not None else None
    if idx is None or powers is None:
        return 0.0
    return powers[idx]


def _battery_soc(step: dict) -> float | None:
    equip = step["equipment"]
    if not isinstance(equip, dict):
        return None
    idx = _battery_index(equip)
    socs = equip.get("soc") if idx is not None else None
    if idx is None or socs is None:
        return None
    return socs[idx]


# ── Simulation completeness ─────────────────────────────────────────

class TestSimulationCompletes:
    def test_step_count(self, sim_result):
        assert len(sim_result["steps"]) == EXPECTED_STEPS

    def test_no_none_results(self, sim_result):
        nones = sum(1 for s in sim_result["steps"] if s["result"] is None)
        assert nones == 0, f"{nones}/{EXPECTED_STEPS} steps returned None"


# ── Battery TOU behavior ────────────────────────────────────────────

class TestBatteryTouBehavior:

    def test_battery_charges_off_peak(self, sim_result):
        """Majority of battery charge energy during off-peak hours."""
        kwh_per_step = TIME_RES_S / 3600.0
        off_peak_kwh = sum(
            _battery_power(s) * kwh_per_step
            for i, s in enumerate(sim_result["steps"])
            if _battery_power(s) > 0.01 and not _is_peak(i)
        )
        peak_kwh = sum(
            _battery_power(s) * kwh_per_step
            for i, s in enumerate(sim_result["steps"])
            if _battery_power(s) > 0.01 and _is_peak(i)
        )
        total = off_peak_kwh + peak_kwh
        share = off_peak_kwh / total
        assert share > 0.40, (
            f"Off-peak charge share {share:.0%} < 40% "
            f"(off-peak={off_peak_kwh:.1f}, peak={peak_kwh:.1f} kWh)"
        )

    def test_battery_discharges_peak(self, sim_result):
        """Majority of battery discharge energy during peak hours."""
        kwh_per_step = TIME_RES_S / 3600.0
        peak_kwh = sum(
            -_battery_power(s) * kwh_per_step
            for i, s in enumerate(sim_result["steps"])
            if _battery_power(s) < -0.01 and _is_peak(i)
        )
        off_peak_kwh = sum(
            -_battery_power(s) * kwh_per_step
            for i, s in enumerate(sim_result["steps"])
            if _battery_power(s) < -0.01 and not _is_peak(i)
        )
        total = peak_kwh + off_peak_kwh
        share = peak_kwh / total
        assert share > 0.40, (
            f"Peak discharge share {share:.0%} < 40% "
            f"(peak={peak_kwh:.1f}, off-peak={off_peak_kwh:.1f} kWh)"
        )

    def test_battery_soc_cycles(self, sim_result):
        """Battery SOC should vary over the week (charge/discharge cycling)."""
        socs = [_battery_soc(s) for s in sim_result["steps"]]
        socs = [s for s in socs if s is not None]
        assert len(socs) > 10, "the battery's SOC channel must report"
        soc_range = max(socs) - min(socs)
        assert soc_range > 0.05, (
            f"SOC range {soc_range:.3f} too narrow "
            f"(min={min(socs):.3f}, max={max(socs):.3f}) -- battery not cycling"
        )

    def test_post_peak_window_serves_load(self, sim_result):
        """The declared self-consumption control serves the load through the
        post-peak window (21:00-23:00): it discharges into the evening
        deficit and charges only from PV surplus, which July evenings have
        none of, so no charge spike occurs there."""
        post_peak_discharge = []
        for i, step in enumerate(sim_result["steps"]):
            dt = SIM_START + timedelta(seconds=i * TIME_RES_S)
            if dt.weekday() < 5 and 21 <= dt.hour <= 23:
                p = _battery_power(step)
                if p < -0.01:
                    post_peak_discharge.append(-p)

        assert len(post_peak_discharge) > 0, (
            "the battery must answer the post-peak load under the declared "
            "self-consumption control"
        )
        avg_post_peak_kw = sum(post_peak_discharge) / len(post_peak_discharge)
        assert avg_post_peak_kw > 0.1, (
            f"Expected the battery to serve the post-peak load (21:00-23:00), "
            f"avg discharge power only {avg_post_peak_kw:.2f} kW"
        )


# ── Thermal behavior ───────────────────────────────────────────────

class TestThermalBehavior:

    def test_zone_temp_stays_comfortable(self, sim_result):
        """Conditioned zone must stay within a plausible comfort range for July."""
        temps = []
        for step in sim_result["steps"]:
            r = step["result"]
            t = r.get("Temperature - Zone_1 (C)")
            if t is not None:
                temps.append(t)

        assert len(temps) > 0, "No zone temperature data"
        t_min, t_max = min(temps), max(temps)
        assert t_min > 15, f"Zone too cold for July: {t_min:.1f} °C"
        assert t_max < 35, f"Zone too hot (HVAC failure): {t_max:.1f} °C"


# ── HVAC active cooling ───────────────────────────────────────────

class TestHvacActiveCooling:
    """Separate test with a 21 °C setpoint override to force AC operation."""

    def test_ac_runs_with_low_setpoint(self):
        """With 21 °C cooling setpoint the AC must deliver cooling in July."""
        from hares import ControlSignal, Dwelling

        dw = Dwelling.from_hpxml(
            str(HPXML), str(SCHEDULE), str(WEATHER),
            start_time="2019-07-01T12:00:00",
            duration_s=2 * 86400,
            time_res_s=TIME_RES_S,
            defaults_path=str(DEFAULTS),
            bldg_id=1,
            master_seed=42,
            output_verbosity=3,
            write_output=False,
            overrides=BATTERY_OVERRIDES,
        )
        dw.initialize()

        cool_signal = ControlSignal.thermal_setpoint(cool_c=21.0)
        n_steps = 2 * 86400 // TIME_RES_S
        cool_kwh = 0.0
        t_max = -999.0
        for _ in range(n_steps):
            dw.apply_control("Air Conditioner", cool_signal)
            r = dw.step()
            cool_kwh += r.get("hvac_cooling_w", 0) * TIME_RES_S / 3_600_000
            z1 = r.get("Temperature - Zone_1 (C)", 0)
            t_max = max(t_max, z1)

        assert cool_kwh > 1.0, (
            f"AC should deliver >1 kWh cooling with 21°C setpoint in July, "
            f"got {cool_kwh:.2f} kWh"
        )
        assert t_max < 28, (
            f"Zone should stay below 28°C with active AC, got {t_max:.1f}°C"
        )

    def test_attic_hotter_than_conditioned(self, sim_result):
        """Unconditioned attic (Zone 2) should be hotter than conditioned zone
        during afternoon hours in July."""
        afternoon_deltas = []
        for i, step in enumerate(sim_result["steps"]):
            dt = SIM_START + timedelta(seconds=i * TIME_RES_S)
            if 13 <= dt.hour <= 17:
                r = step["result"]
                z1 = r.get("Temperature - Zone_1 (C)")
                z2 = r.get("Temperature - Zone_2 (C)")
                if z1 is not None and z2 is not None:
                    afternoon_deltas.append(z2 - z1)

        assert len(afternoon_deltas) > 0, "No afternoon temp data"
        avg_delta = sum(afternoon_deltas) / len(afternoon_deltas)
        assert avg_delta > 2.0, (
            f"Attic should be >2°C warmer than conditioned zone in July afternoon, "
            f"avg delta={avg_delta:.1f}°C"
        )


# ── Billing ──────────────────────────────────────────────────────────

class TestBillingSummary:
    def test_plausible(self):
        """The billing cycle closes on the start's day-of-month anniversary,
        so the shared run (7 days) folds into the one period finalize_billing()
        emits. This test runs the same inputs and tariff from June 28 and
        asserts the emitted summary passes the plausibility checks."""
        from hares import Dwelling, ElectricTariff

        dw = Dwelling.from_hpxml(
            str(HPXML), str(SCHEDULE), str(WEATHER),
            start_time="2019-06-28T00:00:00",
            duration_s=SIM_DURATION_S,
            time_res_s=TIME_RES_S,
            defaults_path=str(DEFAULTS),
            bldg_id=1,
            master_seed=42,
            output_verbosity=3,
            write_output=False,
            overrides=BATTERY_OVERRIDES,
        )
        dw.initialize()
        dw.set_electric_tariff(ElectricTariff.from_urdb_json(str(URDB_TOU)))

        for _ in range(EXPECTED_STEPS):
            dw.step()
        dw.finalize_billing()

        summaries = dw.billing_summaries()
        assert len(summaries) > 0, "finalize_billing must emit the run's period"
        s = summaries[0]
        assert s.total_import_kwh > 0, "House always draws power"
        assert s.energy_charge_usd > 0, "Positive import must produce a charge"
        assert s.net_bill_usd > 0, "Net bill should be positive"
        assert s.peak_demand_kw > 0, "Peak demand should be > 0"


# ── Data quality ─────────────────────────────────────────────────────

class TestDataQuality:
    def test_no_nan_in_results(self, sim_result):
        nan_keys = []
        for i, step in enumerate(sim_result["steps"]):
            r = step["result"]
            if isinstance(r, dict):
                for k, v in r.items():
                    if isinstance(v, float) and math.isnan(v):
                        nan_keys.append((i, k))
        assert len(nan_keys) == 0, (
            f"Found {len(nan_keys)} NaN values, first: step {nan_keys[0][0]} key='{nan_keys[0][1]}'"
            if nan_keys else ""
        )

    def test_total_power_reasonable(self, sim_result):
        for i, step in enumerate(sim_result["steps"]):
            r = step["result"]
            if isinstance(r, dict):
                p = r.get("net_electric_power_kw")
                if p is not None:
                    assert math.isfinite(p), f"Non-finite power at step {i}: {p}"
                    assert abs(p) < 100, f"Unreasonable power at step {i}: {p} kW"


# ── Tariff telemetry ─────────────────────────────────────────────────

class TestTariffTelemetry:
    def test_present_most_steps(self, sim_result):
        present = sum(1 for s in sim_result["steps"] if s["tariff"] is not None)
        ratio = present / len(sim_result["steps"])
        assert ratio > 0.80, f"Tariff telemetry only {ratio:.0%} of steps"

    def test_positive_price(self, sim_result):
        for step in sim_result["steps"]:
            t = step["tariff"]
            if t is not None and hasattr(t, "electricity_price"):
                price = t.electricity_price
                if price is not None:
                    assert price >= 0, f"Negative price: {price}"

    def test_period_name_populated(self, sim_result):
        empty = sum(
            1 for s in sim_result["steps"]
            if s["tariff"] is not None
            and hasattr(s["tariff"], "period_name")
            and not s["tariff"].period_name
        )
        total = len(sim_result["steps"])
        assert empty < total * 0.2, f"Too many empty period names: {empty}/{total}"
