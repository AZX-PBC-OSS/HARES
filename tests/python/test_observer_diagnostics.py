"""Observer-based diagnostics — captures intermediate simulation state to debug
OCHRE parity discrepancies.

Requires: uv run maturin develop -m crates/hares-python/Cargo.toml --features observe
"""

from __future__ import annotations

import datetime as dt
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
VENDOR_OCHRE = ROOT / "vendors" / "OCHRE"
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

START_UTC = dt.datetime(2019, 5, 5, 19, 0, 0, tzinfo=dt.timezone.utc)
START_LOCAL = dt.datetime(2019, 5, 5, 13, 0)
DURATION_H = 1
TIME_RES_MIN = 1


def _run_ochre_detailed() -> list[dict]:
    """Run OCHRE step-by-step, capturing per-step equipment state."""
    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))
    from ochre import Dwelling as OchreDwelling

    dwelling = OchreDwelling(
        name="diag_ochre",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=9,
        save_results=False,
    )
    df, _metrics, _hourly = dwelling.simulate()

    steps: list[dict] = []
    for i, (ts, row) in enumerate(df.iterrows()):
        step: dict = {"step": i, "time": str(ts)}
        for col in df.columns:
            step[col] = float(row[col]) if not isinstance(row[col], str) else row[col]
        steps.append(step)
    return steps


def _run_hares_observed() -> tuple[list[dict], list[dict]]:
    """Run HARES with observer enabled, returning (snapshots, ochre_steps)."""
    from ochre_next import Dwelling as HaresDwelling

    dwelling = HaresDwelling.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        start_time=START_UTC.isoformat(),
        time_res_s=TIME_RES_MIN * 60,
        duration_s=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
    )

    # Enable observer with capacity for all steps
    n_steps = DURATION_H * 60 // TIME_RES_MIN
    dwelling.enable_observer(n_steps + 1)

    step_results: list[dict] = []
    for i in range(n_steps):
        result = dwelling.step()
        step_results.append({"step": i, **result})

    snapshots = dwelling.drain_observations()
    return snapshots, step_results


ochre_mod = pytest.importorskip("ochre", reason="OCHRE not installed")


@pytest.mark.skip(reason="diagnostic visualization tool, not regression test")
def test_observer_hvac_diagnosis():
    """Compare HVAC behavior step-by-step between OCHRE and HARES."""
    ochre_steps = _run_ochre_detailed()
    hares_snaps, hares_steps = _run_hares_observed()

    print("\n" + "=" * 100)
    print("HVAC DIAGNOSIS — Step-by-step comparison")
    print("=" * 100)

    # OCHRE heating/cooling columns
    ochre_heat_col = "HVAC Heating Electric Power (kW)"
    ochre_cool_col = "HVAC Cooling Electric Power (kW)"
    ochre_indoor_col = "Temperature - Indoor (C)"
    ochre_outdoor_col = None
    # Find outdoor temp column
    for col in ochre_steps[0]:
        if "outdoor" in col.lower() and "temp" in col.lower():
            ochre_outdoor_col = col
            break

    print(f"\n{'Step':>4} | {'OCHRE':^40} | {'HARES':^40}")
    print(f"{'':>4} | {'Heat kW':>8} {'Cool kW':>8} {'Indoor C':>8} {'Outdr C':>8} | {'Heat kW':>8} {'Cool kW':>8} {'Indoor C':>8} {'Outdr C':>8}")
    print("-" * 100)

    for i in range(min(10, len(ochre_steps))):
        o = ochre_steps[i]
        o_heat = o.get(ochre_heat_col, 0)
        o_cool = o.get(ochre_cool_col, 0)
        o_indoor = o.get(ochre_indoor_col, float("nan"))
        o_outdoor = o.get(ochre_outdoor_col, float("nan")) if ochre_outdoor_col else float("nan")

        # HARES: extract from snapshots
        h_snap = hares_snaps[i] if i < len(hares_snaps) else {}
        h_env = h_snap.get("post_environment", {})
        h_outdoor = h_env.get("outdoor_temp_c", float("nan"))
        h_zones = h_env.get("zone_temps_c", [])
        h_indoor = h_zones[0][1] if h_zones else float("nan")

        # HARES equipment from snapshots
        h_heat_kw = 0.0
        h_cool_kw = 0.0
        for phase_key in ("post_nonthermal_equipment", "post_thermal_equipment"):
            phase = h_snap.get(phase_key, {})
            for eq in phase.get("equipment", []):
                telem = eq.get("telemetry", {})
                if "Heater" in eq["name"]:
                    h_heat_kw += telem.get("electric_kw", telem.get("active_power_kw", 0))
                elif "Cooler" in eq["name"] or "Air Conditioner" in eq["name"]:
                    h_cool_kw += telem.get("electric_kw", telem.get("active_power_kw", 0))

        print(
            f"{i:4d} | {o_heat:8.4f} {o_cool:8.4f} {o_indoor:8.3f} {o_outdoor:8.2f}"
            f" | {h_heat_kw:8.4f} {h_cool_kw:8.4f} {h_indoor:8.3f} {h_outdoor:8.2f}"
        )

    # Print last 3 steps
    if len(ochre_steps) > 13:
        print("  ...")
        for i in range(len(ochre_steps) - 3, len(ochre_steps)):
            o = ochre_steps[i]
            h_snap = hares_snaps[i] if i < len(hares_snaps) else {}
            h_env = h_snap.get("post_environment", {})
            h_zones = h_env.get("zone_temps_c", [])
            h_indoor = h_zones[0][1] if h_zones else float("nan")
            h_outdoor = h_env.get("outdoor_temp_c", float("nan"))

            h_heat_kw = 0.0
            h_cool_kw = 0.0
            for phase_key in ("post_nonthermal_equipment", "post_thermal_equipment"):
                phase = h_snap.get(phase_key, {})
                for eq in phase.get("equipment", []):
                    telem = eq.get("telemetry", {})
                    if "Heater" in eq["name"]:
                        h_heat_kw += telem.get("electric_kw", telem.get("active_power_kw", 0))
                    elif "Cooler" in eq["name"] or "Air Conditioner" in eq["name"]:
                        h_cool_kw += telem.get("electric_kw", telem.get("active_power_kw", 0))

            o_heat = o.get(ochre_heat_col, 0)
            o_cool = o.get(ochre_cool_col, 0)
            o_indoor = o.get(ochre_indoor_col, float("nan"))
            o_outdoor = o.get(ochre_outdoor_col, float("nan")) if ochre_outdoor_col else float("nan")
            print(
                f"{i:4d} | {o_heat:8.4f} {o_cool:8.4f} {o_indoor:8.3f} {o_outdoor:8.2f}"
                f" | {h_heat_kw:8.4f} {h_cool_kw:8.4f} {h_indoor:8.3f} {h_outdoor:8.2f}"
            )


@pytest.mark.skip(reason="diagnostic visualization tool, not regression test")
def test_observer_equipment_detail():
    """Dump per-equipment telemetry + port contributions from first snapshot."""
    hares_snaps, _ = _run_hares_observed()
    snap = hares_snaps[0]

    print("\n" + "=" * 100)
    print(f"EQUIPMENT DETAIL — Step 0, ts={snap.get('timestamp', '?')}")
    print("=" * 100)

    for phase_key in ("post_nonthermal_equipment", "post_thermal_equipment"):
        phase = snap.get(phase_key, {})
        if not phase:
            continue
        print(f"\n--- {phase_key} ---")

        for eq in phase.get("equipment", []):
            name = eq["name"]
            eq_type = eq.get("equipment_type", "?")
            end_use = eq.get("end_use", "?")
            telem = eq.get("telemetry", {})

            print(f"\n  {name} ({eq_type}, {end_use}):")

            # Key telemetry values
            for key in sorted(telem):
                val = telem[key]
                if val != 0.0:
                    print(f"    {key}: {val:.6f}")

        # Port totals
        ports = phase.get("ports", {})
        if ports:
            print(f"\n  Port totals ({phase_key}):")
            print(f"    electrical_load_kw: {ports.get('electrical_load_kw', 0):.4f}")
            print(f"    electrical_gen_kw: {ports.get('electrical_gen_kw', 0):.4f}")
            for z, s, l in ports.get("thermal", []):
                if s != 0.0 or l != 0.0:
                    print(f"    thermal zone {z}: sensible={s:.2f} W, latent={l:.2f} W")
            for ft, v in ports.get("fuel_consumption_w", []):
                if v != 0.0:
                    print(f"    fuel {ft}: {v:.2f} W")


@pytest.mark.skip(reason="diagnostic visualization tool, not regression test")
def test_observer_envelope_gains():
    """Dump envelope component gains from thermal solver for all steps."""
    hares_snaps, _ = _run_hares_observed()

    print("\n" + "=" * 100)
    print("ENVELOPE COMPONENT GAINS — All steps")
    print("=" * 100)

    gains_keys = [
        "window_solar_w", "opaque_solar_lwr_w", "interior_lwr_w",
        "infiltration_w", "ventilation_w", "natural_ventilation_w",
        "port_sensible_w", "internal_gain_w",
    ]
    print(f"\n{'Step':>4} | " + " | ".join(f"{k:>14}" for k in gains_keys))
    print("-" * (6 + 17 * len(gains_keys)))

    for i, snap in enumerate(hares_snaps[:10]):
        solvers = snap.get("post_solvers", {})
        vals = [solvers.get(k, 0.0) for k in gains_keys]
        print(f"{i:4d} | " + " | ".join(f"{v:14.2f}" for v in vals))

    if len(hares_snaps) > 13:
        print("  ...")
        for snap in hares_snaps[-3:]:
            i = snap.get("step_index", "?")
            solvers = snap.get("post_solvers", {})
            vals = [solvers.get(k, 0.0) for k in gains_keys]
            print(f"{i:>4} | " + " | ".join(f"{v:14.2f}" for v in vals))


@pytest.mark.skip(reason="diagnostic visualization tool, not regression test")
def test_observer_schedule_comparison():
    """Compare scheduled load power values between OCHRE and HARES."""
    ochre_steps = _run_ochre_detailed()
    hares_snaps, _ = _run_hares_observed()

    scheduled_names = [
        ("Indoor Lighting", "Indoor Lighting Electric Power (kW)"),
        ("Exterior Lighting", "Exterior Lighting Electric Power (kW)"),
        ("MELs", "MELs Electric Power (kW)"),
        ("TV", "TV Electric Power (kW)"),
        ("Refrigerator", "Refrigerator Electric Power (kW)"),
    ]

    print("\n" + "=" * 100)
    print("SCHEDULED LOADS — Step 0 comparison")
    print("=" * 100)

    o_step0 = ochre_steps[0]
    h_snap0 = hares_snaps[0]

    for hares_name, ochre_col in scheduled_names:
        o_kw = o_step0.get(ochre_col, float("nan"))

        # Find in HARES snapshots
        h_kw = 0.0
        h_telem = {}
        for phase_key in ("post_nonthermal_equipment", "post_thermal_equipment"):
            phase = h_snap0.get(phase_key, {})
            for eq in phase.get("equipment", []):
                if eq["name"] == hares_name:
                    h_telem = eq.get("telemetry", {})
                    h_kw = h_telem.get("electric_kw", 0)
                    break

        ratio = h_kw / o_kw if abs(o_kw) > 1e-9 else float("inf")
        print(f"\n  {hares_name}:")
        print(f"    OCHRE:  {o_kw:.6f} kW")
        print(f"    HARES:  {h_kw:.6f} kW  (ratio: {ratio:.3f}x)")
        if h_telem:
            for k in sorted(h_telem):
                if h_telem[k] != 0.0:
                    print(f"    telem.{k} = {h_telem[k]:.6f}")


@pytest.mark.skip(reason="diagnostic visualization tool, not regression test")
def test_observer_ochre_equipment_detail():
    """Dump OCHRE per-equipment state at step 0 for comparison."""
    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))
    from ochre import Dwelling as OchreDwelling

    dwelling = OchreDwelling(
        name="diag_ochre_eq",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=9,
        save_results=False,
    )

    print("\n" + "=" * 100)
    print("OCHRE EQUIPMENT STATE — Before simulation")
    print("=" * 100)

    for name, eq in dwelling.equipment.items():
        print(f"\n  {name} (end_use={getattr(eq, 'end_use', '?')}):")
        # Print key properties
        for attr in ["is_electric", "is_gas", "capacity", "capacity_list",
                      "rated_cop", "eir_list", "max_power_kw", "schedule_name"]:
            if hasattr(eq, attr):
                val = getattr(eq, attr)
                if val is not None:
                    print(f"    {attr}: {val}")

        # Setpoints for HVAC
        for attr in ["heating_setpoint", "cooling_setpoint", "deadband",
                      "temp_setpoint", "setpoint"]:
            if hasattr(eq, attr):
                val = getattr(eq, attr)
                if val is not None:
                    print(f"    {attr}: {val}")

    # Run 1 step and check state
    dwelling.simulate()
    df = dwelling.current_schedule
    if df is not None:
        print("\n  OCHRE Schedule (first row):")
        for col in df.columns[:20]:
            print(f"    {col}: {df[col].iloc[0]:.6f}")
