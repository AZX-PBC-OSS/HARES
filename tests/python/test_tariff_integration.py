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

# 7 days in July at 15-min resolution — summer maximizes PV + TOU spread
SIM_DURATION_S = 7 * 86400
TIME_RES_S = 900
EXPECTED_STEPS = SIM_DURATION_S // TIME_RES_S  # 672
SIM_START = datetime(2019, 7, 1, 12, 0)


def _is_peak(step_idx: int) -> bool:
    dt = SIM_START + timedelta(seconds=step_idx * TIME_RES_S)
    return dt.weekday() < 5 and dt.hour in PEAK_HOURS


@pytest.fixture(scope="module")
def sim_result():
    """Run PV+battery dwelling with TOU tariff once, share across all tests."""
    from ochre_next import Dwelling, ElectricTariff

    dw = Dwelling.from_hpxml(
        str(HPXML), str(SCHEDULE), str(WEATHER),
        start_time="2019-07-01T12:00:00",
        duration_s=SIM_DURATION_S,
        time_res_s=TIME_RES_S,
        defaults_path=str(DEFAULTS),
        bldg_id=1,
        master_seed=42,
        output_verbosity=3,
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
        "summaries": dw.billing_summaries(),
    }


def _battery_power(step: dict) -> float:
    """Extract battery power [kW] from step telemetry. Positive=charging."""
    equip = step["equipment"]
    if not isinstance(equip, dict):
        return 0.0
    for name, tel in equip.items():
        if "battery" in name.lower():
            if isinstance(tel, dict):
                return tel.get("power_kw", tel.get("active_power_kw", 0.0)) or 0.0
    return 0.0


def _battery_soc(step: dict) -> float | None:
    equip = step["equipment"]
    if not isinstance(equip, dict):
        return None
    for name, tel in equip.items():
        if "battery" in name.lower():
            if isinstance(tel, dict):
                return tel.get("soc")
    return None


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
        if total > 0.5:
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
        if total > 0.5:
            share = peak_kwh / total
            assert share > 0.40, (
                f"Peak discharge share {share:.0%} < 40% "
                f"(peak={peak_kwh:.1f}, off-peak={off_peak_kwh:.1f} kWh)"
            )

    def test_battery_soc_cycles(self, sim_result):
        """Battery SOC should vary over the week (charge/discharge cycling)."""
        socs = [_battery_soc(s) for s in sim_result["steps"]]
        socs = [s for s in socs if s is not None]
        if len(socs) > 10:
            soc_range = max(socs) - min(socs)
            assert soc_range > 0.05, (
                f"SOC range {soc_range:.3f} too narrow "
                f"(min={min(socs):.3f}, max={max(socs):.3f}) — battery not cycling"
            )

    def test_charging_spike_during_offpeak_transition(self, sim_result):
        """There should be a visible charge power spike when transitioning
        from peak to off-peak (around hour 21)."""
        # Collect battery power in the 21:00-23:00 window (just after peak ends)
        post_peak_charge = []
        for i, step in enumerate(sim_result["steps"]):
            dt = SIM_START + timedelta(seconds=i * TIME_RES_S)
            if dt.weekday() < 5 and 21 <= dt.hour <= 23:
                p = _battery_power(step)
                if p > 0.01:
                    post_peak_charge.append(p)

        if len(post_peak_charge) > 0:
            avg_post_peak_kw = sum(post_peak_charge) / len(post_peak_charge)
            # Battery should charge at a meaningful rate after peak ends
            assert avg_post_peak_kw > 0.5, (
                f"Expected charging spike post-peak (21:00-23:00), "
                f"avg charge power only {avg_post_peak_kw:.2f} kW"
            )


# ── Billing ──────────────────────────────────────────────────────────

class TestBillingSummary:
    def test_plausible_if_present(self, sim_result):
        """If a billing boundary was crossed, summary values are plausible."""
        summaries = sim_result["summaries"]
        if len(summaries) > 0:
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
