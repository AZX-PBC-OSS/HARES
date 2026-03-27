"""4-month DER performance benchmark: OCHRE vs HARES.

Both simulators run the BEopt dwelling with PV (5 kW south-facing) and Battery (10 kWh)
for 120 days at 1-minute resolution starting 2019-01-01 (winter→spring).

OCHRE receives DER via the Equipment kwarg; HARES adds them via add_pv/add_battery.

Requires:
    uv sync --group dev --group ochre
    uv run maturin develop --release -m crates/hares-python/Cargo.toml

Usage:
    uv run python tests/python/bench_4month_der.py
"""

from __future__ import annotations

import datetime as dt
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VENDOR_OCHRE = ROOT / "vendors" / "OCHRE"
EXAMPLES = ROOT / "data" / "examples"
HARES_DEFAULTS = ROOT / "defaults"

HPXML = str(EXAMPLES / "BEopt_example.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")

START_LOCAL = dt.datetime(2019, 5, 1, 7, 0)
START_HARES = "2019-05-01T07:00:00"

DURATION_DAYS = 120
DURATION_H = DURATION_DAYS * 24
TIME_RES_MIN = 1

# DER configuration (shared between both simulators)
PV_CAPACITY_KW = 5.0
PV_TILT_DEG = 25.0
# OCHRE azimuth: 0=south.  HARES azimuth: 180=south (HPXML convention).
PV_AZIMUTH_OCHRE = 0.0
PV_AZIMUTH_HARES = 180.0
PV_INVERTER_CAPACITY_KW = PV_CAPACITY_KW / 1.2
PV_INVERTER_EFFICIENCY = 96.0  # SAM expects percentage, not fraction

BATTERY_CAPACITY_KWH = 10.0
BATTERY_CAPACITY_KW = 5.0

OCHRE_TO_HARES: dict[str, list[str]] = {
    "HVAC Heating Electric Power (kW)": [
        "ASHP Heater Electric Power (kW)",
        "MSHP Heater Electric Power (kW)",
        "Gas Furnace Electric Power (kW)",
        "Electric Furnace Electric Power (kW)",
    ],
    "HVAC Cooling Electric Power (kW)": [
        "ASHP Cooler Electric Power (kW)",
        "MSHP Cooler Electric Power (kW)",
        "Air Conditioner Electric Power (kW)",
        "Room AC Electric Power (kW)",
    ],
}


def _resolve_hares_kwh(ochre_col: str, hares: dict[str, float]) -> float | None:
    if ochre_col in hares:
        return hares[ochre_col]
    aliases = OCHRE_TO_HARES.get(ochre_col, [])
    matched = [hares[a] for a in aliases if a in hares]
    if matched:
        return sum(matched)
    return None


def _run_ochre() -> tuple[dict[str, float], float, float]:
    """Run OCHRE with PV + Battery for 120 days."""
    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))
    from ochre import Dwelling as OchreDwelling  # type: ignore[import-not-found]

    equipment: dict[str, dict] = {
        "PV": {
            "capacity": PV_CAPACITY_KW,
            "tilt": PV_TILT_DEG,
            "azimuth": PV_AZIMUTH_OCHRE,
            "inverter_capacity": PV_INVERTER_CAPACITY_KW,
            "inverter_efficiency": PV_INVERTER_EFFICIENCY,
        },
        "Battery": {
            "capacity_kwh": BATTERY_CAPACITY_KWH,
            "capacity": BATTERY_CAPACITY_KW,
            "soc_init": 0.5,
            "soc_min": 0.05,
            "soc_max": 1.0,
            "efficiency_type": "constant",
            "self_consumption_mode": True,
        },
    }

    t0 = time.perf_counter()
    dwelling = OchreDwelling(
        name="bench_ochre_4mo",
        start_time=START_LOCAL,
        time_res=dt.timedelta(minutes=TIME_RES_MIN),
        duration=dt.timedelta(hours=DURATION_H),
        hpxml_file=HPXML,
        hpxml_schedule_file=SCHEDULE,
        weather_file=WEATHER,
        verbosity=6,
        save_results=False,
        Equipment=equipment,
    )
    init_elapsed = time.perf_counter() - t0

    t1 = time.perf_counter()
    df, _, _ = dwelling.simulate()
    sim_elapsed = time.perf_counter() - t1

    time_res_h = TIME_RES_MIN / 60.0
    kwh: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            kwh[col] = float((df[col] * time_res_h).sum())
    return kwh, init_elapsed, sim_elapsed


def _run_hares() -> tuple[dict[str, float], float, float]:
    """Run HARES with PV + Battery for 120 days."""
    from ochre_next import Battery, Dwelling as HaresDwelling, PV  # type: ignore[import-not-found]

    t0 = time.perf_counter()
    dwelling = HaresDwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=START_HARES,
        time_res_s=TIME_RES_MIN * 60,
        duration_s=DURATION_H * 3600,
        output_verbosity=6,
        defaults_path=str(HARES_DEFAULTS),
        master_seed=42,
    )
    dwelling.initialize()

    dwelling.add_pv(PV("PV", PV_CAPACITY_KW, PV_TILT_DEG, PV_AZIMUTH_HARES))
    dwelling.add_battery(
        Battery("Battery", capacity_kwh=BATTERY_CAPACITY_KWH, max_charge_kw=BATTERY_CAPACITY_KW, max_discharge_kw=BATTERY_CAPACITY_KW)
    )
    dwelling.add_actor_by_name(
        "BatteryManagement",
        "Battery_bms",
        {"target": "Battery", "mode": "self_consumption", "grid_export_rule": "unrestricted", "steps_per_day": 86400 // (TIME_RES_MIN * 60)},
    )
    init_elapsed = time.perf_counter() - t0

    t1 = time.perf_counter()
    df = dwelling.simulate()
    sim_elapsed = time.perf_counter() - t1

    time_res_h = TIME_RES_MIN / 60.0
    kwh: dict[str, float] = {}
    for col in df.columns:
        if col.endswith("(kW)") or col.endswith("(therms/hour)"):
            kwh[col] = float(df[col].sum()) * time_res_h
    return kwh, init_elapsed, sim_elapsed


def _print_performance(
    ochre_init: float,
    ochre_sim: float,
    hares_init: float,
    hares_sim: float,
    n_steps: int,
) -> None:
    total_ochre = ochre_init + ochre_sim
    total_hares = hares_init + hares_sim

    def speedup(a: float, b: float) -> str:
        return f"{a / max(b, 1e-9):.1f}x"

    print(f"\n  {'':30} {'OCHRE':>10} {'HARES':>10} {'Speedup':>10}")
    print(f"  {'Init time (s)':30} {ochre_init:10.2f} {hares_init:10.2f} {speedup(ochre_init, hares_init):>10}")
    print(f"  {'Sim time (s)':30} {ochre_sim:10.2f} {hares_sim:10.2f} {speedup(ochre_sim, hares_sim):>10}")
    print(f"  {'Total time (s)':30} {total_ochre:10.2f} {total_hares:10.2f} {speedup(total_ochre, total_hares):>10}")
    ochre_sps = n_steps / max(ochre_sim, 1e-9)
    hares_sps = n_steps / max(hares_sim, 1e-9)
    print(f"  {'Steps/sec (sim only)':30} {ochre_sps:10.0f} {hares_sps:10.0f} {'':>10}")


def _print_energy_parity(
    ochre_kwh: dict[str, float],
    hares_kwh: dict[str, float],
) -> None:
    col_w = 55
    print(f"\n  {'Column':<{col_w}} {'OCHRE kWh':>10} {'HARES kWh':>10} {'Diff%':>8}")
    print("  " + "-" * (col_w + 30))

    missing: list[str] = []

    for col in sorted(ochre_kwh):
        o_val = ochre_kwh[col]
        h_val = _resolve_hares_kwh(col, hares_kwh)
        if h_val is None:
            if abs(o_val) > 1e-6:
                missing.append(col)
            continue
        if abs(o_val) < 1e-6 and abs(h_val) < 1e-6:
            continue
        d_pct = (h_val - o_val) / o_val * 100 if abs(o_val) > 1e-9 else 0.0
        print(f"  {col:<{col_w}} {o_val:10.2f} {h_val:10.2f} {d_pct:+7.1f}%")

    ochre_covered: set[str] = set(ochre_kwh)
    for aliases in OCHRE_TO_HARES.values():
        ochre_covered.update(aliases)
    hares_only = [
        (col, v)
        for col, v in sorted(hares_kwh.items())
        if col not in ochre_covered and abs(v) > 1e-6
    ]
    if hares_only:
        print(f"\n  {'HARES-only columns':<{col_w}} {'':>10} {'HARES kWh':>10}")
        for col, v in hares_only:
            print(f"  {col:<{col_w}} {'—':>10} {v:10.2f}")

    if missing:
        print(f"\n  OCHRE columns with no HARES match (non-zero):")
        for col in missing:
            print(f"    {col}")


def main() -> None:
    n_steps = DURATION_DAYS * 24 * 60 // TIME_RES_MIN
    sep = "=" * 90

    print(sep)
    print(f"4-MONTH DER BENCHMARK — {DURATION_DAYS} days ({n_steps:,} steps at {TIME_RES_MIN}-min resolution)")
    print(f"HPXML  : {HPXML}")
    print(f"Weather: {WEATHER}")
    print(f"Start  : {START_LOCAL}  Duration: {DURATION_DAYS} days")
    print(f"DER    : {PV_CAPACITY_KW} kW PV + {BATTERY_CAPACITY_KWH} kWh / {BATTERY_CAPACITY_KW} kW battery")
    print(sep)

    print("\nRunning OCHRE ...", flush=True)
    ochre_kwh, ochre_init, ochre_sim = _run_ochre()
    print(f"  done  (init={ochre_init:.1f}s  sim={ochre_sim:.1f}s)")

    print("\nRunning HARES ...", flush=True)
    hares_kwh, hares_init, hares_sim = _run_hares()
    print(f"  done  (init={hares_init:.1f}s  sim={hares_sim:.1f}s)")

    print(f"\n{sep}")
    print("PERFORMANCE")
    print(sep)
    _print_performance(ochre_init, ochre_sim, hares_init, hares_sim, n_steps)

    print(f"\n{sep}")
    print(f"ENERGY PARITY (kWh over {DURATION_DAYS} days)")
    print(sep)
    _print_energy_parity(ochre_kwh, hares_kwh)

    print(f"\n{sep}\n")


if __name__ == "__main__":
    main()
