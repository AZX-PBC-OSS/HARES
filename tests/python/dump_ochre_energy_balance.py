"""Dump OCHRE's complete per-node, per-surface energy balance for 10 steps.

Produces exact ground truth for diagnosing HARES thermal solver discrepancies.
Prints: per-node temperatures, per-surface heat flows, per-zone energy balance.

Usage:
    cd /home/rich/src/HARES
    PYTHONPATH=vendors/OCHRE vendors/OCHRE/.venv/bin/python tests/python/dump_ochre_energy_balance.py
"""
from __future__ import annotations

import datetime as dt
import sys
from pathlib import Path

import numpy as np

REPO_ROOT = Path(__file__).resolve().parents[2]
OCHRE_ROOT = REPO_ROOT / "vendors" / "OCHRE"
sys.path.insert(0, str(OCHRE_ROOT))

from ochre import Dwelling  # noqa: E402

HPXML = OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example.xml"
SCHEDULE = OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example_schedule.csv"
WEATHER = OCHRE_ROOT / "ochre" / "defaults" / "Weather" / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"

N_STEPS = 10


def main() -> None:
    dwelling = Dwelling(
        name="energy_diag",
        start_time=dt.datetime(2019, 5, 5, 12, 0, 0),
        time_res=dt.timedelta(minutes=1),
        duration=dt.timedelta(minutes=N_STEPS),
        hpxml_file=str(HPXML),
        hpxml_schedule_file=str(SCHEDULE),
        weather_file=str(WEATHER),
        verbosity=9,
        metrics_verbosity=0,
        initialization_time=None,
    )

    # Strip equipment for free-float
    removed = list(dwelling.equipment.keys())
    dwelling.sub_simulators = [s for s in dwelling.sub_simulators if s is dwelling.envelope]
    dwelling.equipment.clear()
    dwelling.equipment_by_end_use = {k: [] for k in dwelling.equipment_by_end_use}
    dwelling.zones_for_schedule.clear()
    print(f"Removed equipment: {removed}")

    env = dwelling.envelope

    # Print RC network structure
    print(f"\n{'='*70}")
    print("RC NETWORK STRUCTURE")
    print(f"{'='*70}")
    print(f"States: {len(env.state_names)}")
    print(f"State names: {env.state_names}")
    print(f"Input names: {env.input_names}")

    # Print initial state
    print(f"\nInitial state vector (temperatures °C):")
    for i, name in enumerate(env.state_names):
        if name.startswith("T_"):
            print(f"  {name:10s} = {env.states[i]:.4f}°C")

    # Step through and dump energy balance
    for step in range(N_STEPS):
        print(f"\n{'='*70}")
        print(f"STEP {step}")
        print(f"{'='*70}")

        # Capture inputs before update
        schedule = dwelling.current_schedule if hasattr(dwelling, 'current_schedule') else {}

        # Run one step
        dwelling.update()

        # State vector after step
        print(f"\nNode temperatures after step {step}:")
        for i, name in enumerate(env.state_names):
            if name.startswith("T_"):
                print(f"  {name:10s} = {env.states[i]:.4f}°C")

        # Input vector
        print(f"\nInput vector:")
        for i, name in enumerate(env.input_names):
            val = env.inputs[i] if i < len(env.inputs) else 0.0
            if abs(val) > 1e-10:
                print(f"  {name:10s} = {val:.6f}")

        # Zone temperatures and heat gains
        for zone_name, zone in env.zones.items():
            print(f"\nZone: {zone_name}")
            print(f"  T_zone = {zone.temperature:.4f}°C")
            print(f"  radiation_heat = {zone.radiation_heat:.2f} W")
            if hasattr(zone, 'inf_heat'):
                print(f"  inf_heat = {zone.inf_heat:.2f} W")
            if hasattr(zone, 'inf_flow'):
                print(f"  inf_flow = {zone.inf_flow:.6f} m³/s")
            if hasattr(zone, 'vent_heat'):
                print(f"  vent_heat = {zone.vent_heat:.2f} W")

            # Per-surface details
            print(f"  Interior surfaces ({len(zone.surfaces)}):")
            for s in zone.surfaces:
                t_surf = s.temperature if hasattr(s, 'temperature') else float('nan')
                rad_to_zone = s.radiation_to_zone if hasattr(s, 'radiation_to_zone') else 0.0
                print(
                    f"    {s.boundary.label:15s} node={s.node:5s}"
                    f"  T_surf={t_surf:.2f}°C"
                    f"  rad_to_zone={rad_to_zone:.1f}W"
                    f"  rad_frac={s.radiation_frac:.4f}"
                )

        # Exterior surface details
        print(f"\nExterior surfaces ({len(env.ext_boundaries)}):")
        for boundary in env.ext_boundaries:
            surface = boundary.ext_surface
            solar = surface.solar_gain if hasattr(surface, 'solar_gain') else 0.0
            lwr = surface.lwr_gain if hasattr(surface, 'lwr_gain') else 0.0
            t_surf = surface.temperature if hasattr(surface, 'temperature') else float('nan')
            transmitted = surface.transmitted_gain if hasattr(surface, 'transmitted_gain') else 0.0
            print(
                f"  {boundary.label:15s}"
                f"  solar={solar:.1f}W"
                f"  lwr={lwr:.1f}W"
                f"  T_surf={t_surf:.2f}°C"
                f"  transmitted={transmitted:.1f}W"
            )

        # Component loads
        if hasattr(env, 'add_component_loads') and not env.reduced:
            cl = env.add_component_loads()
            print(f"\nComponent loads (indoor zone):")
            for name, val in cl.items():
                if abs(val) > 0.01:
                    print(f"  {name:45s} = {val:.2f} W")

        # Weather
        t_ext = env.current_schedule.get("Ambient Dry Bulb (C)", float('nan'))
        t_sky = env.current_schedule.get("Sky Temperature (C)", float('nan'))
        wind = env.current_schedule.get("Wind Speed (m/s)", 0.0)
        t_ground = env.current_schedule.get("Ground Temperature (C)", float('nan'))
        print(f"\nWeather: T_ext={t_ext:.1f}°C  T_sky={t_sky:.1f}°C  wind={wind:.1f}m/s  T_ground={t_ground:.1f}°C")


if __name__ == "__main__":
    main()
