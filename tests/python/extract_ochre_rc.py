"""Extract ALL RC network parameters from OCHRE for the BEopt example building.

Used to diagnose why HARES temperatures are 4-5 degrees C off vs OCHRE.

Run with:
    /home/rich/src/HARES/vendors/OCHRE/.venv/bin/python tests/python/extract_ochre_rc.py
"""

from __future__ import annotations

import datetime as dt
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
OCHRE_ROOT = REPO_ROOT / "vendors" / "OCHRE"
sys.path.insert(0, str(OCHRE_ROOT))

from ochre import Dwelling  # noqa: E402

HPXML_FILE = OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example.xml"
SCHEDULE_FILE = OCHRE_ROOT / "ochre" / "defaults" / "Input Files" / "BEopt_example_schedule.csv"
WEATHER_FILE = (
    OCHRE_ROOT / "ochre" / "defaults" / "Weather" / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)


def main() -> None:
    print(f"OCHRE root: {OCHRE_ROOT}")
    print(f"HPXML:      {HPXML_FILE}")
    print(f"Weather:    {WEATHER_FILE}")

    dwelling = Dwelling(
        name="rc_extract",
        start_time=dt.datetime(2019, 5, 5, 12, 0, 0),
        time_res=dt.timedelta(minutes=1),
        duration=dt.timedelta(hours=1),
        hpxml_file=str(HPXML_FILE),
        hpxml_schedule_file=str(SCHEDULE_FILE),
        weather_file=str(WEATHER_FILE),
        verbosity=9,
        metrics_verbosity=0,
        initialization_time=None,
    )

    env = dwelling.envelope

    # -------------------------------------------------------------------------
    # 1. State space dimensions
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("STATE SPACE DIMENSIONS")
    print(f"{'='*70}")
    print(f"States : {len(env.state_names)}")
    print(f"Inputs : {len(env.input_names)}")
    print(f"Outputs: {len(env.output_names)}")

    print(f"\nState names  ({len(env.state_names)}):")
    for i, name in enumerate(env.state_names):
        print(f"  [{i:2d}] {name}")

    print(f"\nInput names  ({len(env.input_names)}):")
    for i, name in enumerate(env.input_names):
        print(f"  [{i:2d}] {name}")

    print(f"\nOutput names ({len(env.output_names)}):")
    for i, name in enumerate(env.output_names):
        print(f"  [{i:2d}] {name}")

    # -------------------------------------------------------------------------
    # 2. Continuous-time A_c matrix diagonal (time constants)
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("CONTINUOUS-TIME A_c MATRIX (diagonal = -1/RC, units 1/s)")
    print(f"{'='*70}")
    n_temp_states = len([n for n in env.state_names if n.startswith("T_")])
    for i in range(n_temp_states):
        tau = -1.0 / env.A_c[i, i] if env.A_c[i, i] != 0 else float("inf")
        print(f"  A_c[{i:2d},{i:2d}] = {env.A_c[i,i]:+.8e}  tau = {tau/3600:.3f} h  ({env.state_names[i]})")

    # -------------------------------------------------------------------------
    # 3. Full continuous-time A_c off-diagonals (coupling between nodes)
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("CONTINUOUS-TIME A_c OFF-DIAGONAL (non-zero coupling, 1/s)")
    print(f"{'='*70}")
    for i in range(n_temp_states):
        for j in range(n_temp_states):
            if i != j and env.A_c[i, j] != 0:
                print(
                    f"  A_c[{i:2d},{j:2d}] = {env.A_c[i,j]:+.8e}"
                    f"  {env.state_names[i]} <- {env.state_names[j]}"
                )

    # -------------------------------------------------------------------------
    # 4. Discrete-time A matrix diagonal
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("DISCRETE-TIME A MATRIX (1-min timestep)")
    print(f"{'='*70}")
    print(f"A matrix shape: {env.A.shape}")
    for i in range(n_temp_states):
        print(f"  A[{i:2d},{i:2d}] = {env.A[i,i]:.8f}  ({env.state_names[i]})")

    # -------------------------------------------------------------------------
    # 5. B matrix - non-zero entries for temperature states
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("CONTINUOUS-TIME B_c MATRIX (non-zero entries for T states)")
    print(f"{'='*70}")
    print(f"B_c matrix shape: {env.B_c.shape}")
    for i in range(n_temp_states):
        for j in range(env.B_c.shape[1]):
            if env.B_c[i, j] != 0:
                print(
                    f"  B_c[{i:2d},{j:2d}] = {env.B_c[i,j]:+.8e}"
                    f"  state={env.state_names[i]}  input={env.input_names[j]}"
                )

    # -------------------------------------------------------------------------
    # 6. Per-zone info
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("ZONE INFORMATION")
    print(f"{'='*70}")
    for zone_name, zone in env.zones.items():
        print(f"\nZone: {zone_name}  (label={zone.label})")
        print(f"  Volume        : {zone.volume} m^3")
        print(f"  Capacitance   : {zone.capacitance:.2f} kJ/K  = {zone.capacitance*1000:.0f} J/K")
        print(f"  t_idx (output): {zone.t_idx}")
        print(f"  h_idx (input) : {zone.h_idx}")
        print(f"  Infil method  : {zone.infiltration_method}")
        if zone.infiltration_method == "ASHRAE":
            p = zone.infiltration_parameters
            print(f"  ASHRAE infil params: {p}")
        elif zone.infiltration_method == "ELA":
            p = zone.infiltration_parameters
            print(f"  ELA infil params: {p}")
        elif zone.infiltration_method == "ACH":
            p = zone.infiltration_parameters
            print(f"  ACH infil params: {p}")
        print(f"  n_surfaces: {len(zone.surfaces)}")
        for s in zone.surfaces:
            ext_int = "ext" if s.is_exterior else "int"
            print(
                f"    [{ext_int}] {s.boundary.label}/{s.boundary_name:20s}"
                f"  area={s.area:7.3f} m^2"
                f"  e={s.emissivity:.3f}"
                f"  a={s.absorptivity:.3f}"
                f"  res_film={s.res_film:.5f} m^2-K/W"
                f"  rad_frac={s.radiation_frac:.4f}"
                f"  rad_res={s.radiation_res:.6f} K/W"
                f"  node={s.node}"
                f"  t_idx={s.t_idx}"
                f"  h_idx={s.h_idx}"
            )

    # -------------------------------------------------------------------------
    # 7. Per-boundary info (the full RC network per boundary)
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("BOUNDARY INFORMATION")
    print(f"{'='*70}")
    for b in env.boundaries:
        print(f"\nBoundary: {b.name}  (label={b.label})")
        print(f"  Area         : {b.area:.4f} m^2")
        print(f"  Ext zone     : {b.ext_zone_label}")
        print(f"  Int zone     : {b.int_zone_label}")
        print(f"  n_nodes      : {b.n_nodes}")
        print(f"  All nodes    : {b.all_nodes}")
        print(f"  Capacitors (kJ/K):")
        for node, val in b.capacitors.items():
            print(f"    C[{node}] = {val:.6f} kJ/K  = {val*1000:.2f} J/K")
        print(f"  Resistors (K/W per unit):")
        for (n1, n2), val in b.resistors.items():
            print(f"    R[{n1},{n2}] = {val:.8f} K/W")
        ext_s = b.ext_surface
        print(f"  Ext surface  : node={ext_s.node}, zone={ext_s.zone_label}")
        print(f"    emissivity={ext_s.emissivity:.4f}, absorptivity={ext_s.absorptivity:.4f}")
        print(f"    res_film={ext_s.res_film:.6f} m^2-K/W, sky_view={ext_s.sky_view_factor:.4f}")
        if hasattr(b, "int_surface") and b.int_surface.zone_label:
            int_s = b.int_surface
            print(f"  Int surface  : node={int_s.node}, zone={int_s.zone_label}")
            print(f"    emissivity={int_s.emissivity:.4f}, absorptivity={int_s.absorptivity:.4f}")
            print(f"    res_film={int_s.res_film:.6f} m^2-K/W")

    # -------------------------------------------------------------------------
    # 8. Raw capacitance and resistance dictionaries (as passed to RCModel)
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("RAW RC NETWORK (as assembled from all boundaries + zones)")
    print(f"{'='*70}")
    cap, res = env.load_rc_data(
        time_res=dt.timedelta(minutes=1),
        initial_schedule={
            "Ambient Dry Bulb (C)": 15.0,
            "Ground Temperature (C)": 10.0,
            "HVAC Heating Setpoint (C)": 21.0,
            "HVAC Cooling Setpoint (C)": 24.0,
        },
    )
    print(f"\nCapacitances ({len(cap)} nodes, in J/K):")
    for node, val in cap.items():
        print(f"  C[{node}] = {val:.2f} J/K")

    print(f"\nResistances ({len(res)} links, in K/W):")
    for (n1, n2), val in res.items():
        print(f"  R[{n1},{n2}] = {val:.8f} K/W")

    # -------------------------------------------------------------------------
    # 9. Initial state vector
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("INITIAL STATE VECTOR")
    print(f"{'='*70}")
    for i, name in enumerate(env.state_names):
        print(f"  x[{i:2d}] {name:30s} = {env.states[i]:.6f}")

    # -------------------------------------------------------------------------
    # 10. Initial input vector
    # -------------------------------------------------------------------------
    print(f"\n{'='*70}")
    print("INITIAL INPUT VECTOR")
    print(f"{'='*70}")
    for i, name in enumerate(env.input_names):
        print(f"  u[{i:2d}] {name:45s} = {env.inputs[i]:.6f}")

    print(f"\n{'='*70}")
    print("EXTRACTION COMPLETE")
    print(f"{'='*70}")


if __name__ == "__main__":
    main()
