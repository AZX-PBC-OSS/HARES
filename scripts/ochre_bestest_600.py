"""
Run BESTEST Case 600 in OCHRE to determine if OCHRE's convection-only R_film
architecture also fails ASHRAE 140 reference bands.

Case 600: Low-mass conditioned building, annual heating/cooling loads.
- 8m x 6m x 2.7m, south-facing window (12m2)
- Lightweight wall (wood siding + insulation + plasterboard), U = 0.514 W/m2K
- Lightweight roof, U = 0.318 W/m2K
- Lightweight floor, U = 0.040 W/m2K (over crawlspace)
- Infiltration: 0.5 ACH (altitude-corrected for Denver 1609m)
- Internal gains: 200W continuous (60% radiative / 40% convective)
- Thermostat: heat < 20C, cool > 27C, deadband
- Denver TMY weather

This script constructs the OCHRE envelope directly (bypassing HPXML)
and runs an annual simulation, comparing results to ASHRAE 140 bands.
"""

import datetime as dt
import numpy as np
import pandas as pd
import sys
import os

# Add OCHRE to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "vendors", "OCHRE"))

from ochre.Models.Envelope import Envelope
from ochre.utils import envelope as env_utils

# BESTEST Case 600 parameters
ZONE_VOLUME = 8 * 6 * 2.7  # 129.6 m3
ZONE_FLOOR_AREA = 8 * 6  # 48 m2

# Wall areas (4 walls)
SOUTH_WALL_AREA = 8 * 2.7 - 12  # 9.6 m2 (minus window)
NORTH_WALL_AREA = 8 * 2.7  # 21.6 m2
EAST_WALL_AREA = 6 * 2.7  # 16.2 m2
WEST_WALL_AREA = 6 * 2.7  # 16.2 m2

# Window: south-facing, 12 m2
WINDOW_AREA = 12.0

# Roof area
ROOF_AREA = ZONE_FLOOR_AREA  # flat roof, 48 m2

# Floor area
FLOOR_AREA = ZONE_FLOOR_AREA  # 48 m2

# Film resistances from OCHRE's calculate_film_resistances
# Denver: avg wind speed 4.02 m/s, avg ambient temp ~10C + 5C = 15C
# Ground temp ~10C
LOCATION = {
    "Average Wind Speed (m/s)": 4.02,
    "Average Ambient Temperature (C)": 10,
    "Average Ground Temperature (C)": 10,
}

# Interior: TARP vertical, delta_t=12.9 -> h_natural = 1.31 * 12.9^(1/3) = 3.076
# R_film_int = 1/3.076 = 0.325 m2K/W (convection only)
# Exterior (rough): DOE-2 with wind=4.02 m/s
# h_natural ~ 3.076, h_glass = sqrt(3.076^2 + (3.4*4.02^0.75)^2) ~ 8.0
# r_f = 1.67 (rough), h_forced = 1.67*(8.0-3.076) = 8.23, h_total = 3.076+8.23 = 11.3
# Wait, that's not 29.3... Let me just use OCHRE's function to get the actual values

r_film_ext_wall = env_utils.calculate_film_resistances("South Wall", {
    "Exterior Zone Label": "EXT",
    "Interior Zone Label": "LIV",
    "Tilt (deg)": 90,
}, LOCATION)
r_film_ext_roof = env_utils.calculate_film_resistances("Roof", {
    "Exterior Zone Label": "EXT",
    "Interior Zone Label": "LIV",
    "Tilt (deg)": 0,
}, LOCATION)
r_film_floor = env_utils.calculate_film_resistances("Floor", {
    "Exterior Zone Label": "GND",
    "Interior Zone Label": "LIV",
    "Tilt (deg)": 0,
}, LOCATION)

print("=== OCHRE Film Resistances ===")
print(f"Wall:   ext={r_film_ext_wall['Exterior Film Resistance (m^2-K/W)']:.4f}, int={r_film_ext_wall['Interior Film Resistance (m^2-K/W)']:.4f}")
print(f"Roof:   ext={r_film_ext_roof['Exterior Film Resistance (m^2-K/W)']:.4f}, int={r_film_ext_roof['Interior Film Resistance (m^2-K/W)']:.4f}")
print(f"Floor:  ext={r_film_floor['Exterior Film Resistance (m^2-K/W)']:.4f}, int={r_film_floor['Interior Film Resistance (m^2-K/W)']:.4f}")

# The BESTEST specification requires:
# R_so = 1/29.3 = 0.0341 (rough surface)
# R_si = 1/8.29 = 0.1206 (combined vertical)
# But OCHRE gives conv-only R_si = 0.3255

# Case 600 wall: U = 0.514 W/m2K, so R_total = 1.946 m2K/W
# R_total = R_so + R_material + R_si
# With ASHRAE values: R_material = 1.946 - 0.0341 - 0.1206 = 1.791
# With OCHRE conv-only: R_total = 0.0341 + 1.791 + 0.3255 = 2.151 (10.5% too high)

# We need to construct OCHRE with the BESTEST material R-values
# and see what annual loads it produces

# Case 600 wall construction (from EnergyPlus BESTEST report):
# R_material = 1.791 m2K/W (from U=0.514 minus films)
# Case 600 roof: U = 0.318 W/m2K, R_total = 3.145 m2K/W
# R_material_roof = 3.145 - 0.0341 - 0.1206 = 2.990 (roughly)
# Actually roof is horizontal, so R_si = 1/9.26 = 0.108 (upward) or 1/6.13=0.163 (downward)
# For roof (heat up through roof in winter), downward flow: R_si = 1/6.13 = 0.163
# But OCHRE uses conv-only which will be different

# Case 600 floor: U = 0.040 W/m2K over crawlspace (not ground-coupled)
# Floor is over crawlspace at outdoor temperature, so R_so applies
# R_total = 1/0.040 = 25.0 m2K/W
# R_material_floor = 25.0 - 0.0341 - 0.163 = 24.80 (downward flow from room)

# Window: U = 3.0 W/m2K (double pane), SHGC = 0.767

# Let me use OCHRE's simple Envelope interface with explicit capacitances/resistances
# rather than HPXML boundaries, so we can control the exact R-values

# For a simple 1-node-per-boundary model:
# Wall: C_wall (lightweight), R_wall (material only)
# The film resistances get added by OCHRE

# Actually, the simplest approach: use OCHRE's direct capacitance/resistance interface
# and manually construct the RC network

# For BESTEST Case 600, the zones are: Indoor (LIV), Outdoor (EXT), Ground (GND)
# We need: 4 walls + roof + floor + window, each as a boundary

# Actually, let me just build the simplest possible model:
# Single zone with lumped wall R-value to outdoor

# Total wall UA (excluding window and floor):
# South wall (net): 9.6 * 0.514 = 4.934
# North wall: 21.6 * 0.514 = 11.102
# East wall: 16.2 * 0.514 = 8.327
# West wall: 16.2 * 0.514 = 8.327
# Roof: 48 * 0.318 = 15.264
# Floor: 48 * 0.040 = 1.920
# Window: 12 * 3.0 = 36.0
# Total UA = 4.934 + 11.102 + 8.327 + 8.327 + 15.264 + 1.920 + 36.0 = 85.874

# This approach is too simplified. Let me instead construct proper OCHRE boundaries.
# OCHRE needs HPXML-style boundary definitions. Let me check what format it expects.

# Actually, for a quick test, the simplest approach is to just construct a minimal
# envelope with the right total UA and thermal mass, and compare heating/cooling loads.
# The key question is: does conv-only R_film + LWR produce the same loads as combined R_film?

# Let me try running OCHRE in both "full" and "linear" modes with the same wall
# and see if they produce different annual loads.

# For the minimal test, I'll create a single boundary with the Case 600 wall properties
# and run a short simulation.

# First, let me understand how OCHRE constructs boundaries from HPXML
print("\n=== Checking OCHRE boundary construction ===")
print("OCHRE needs HPXML-style boundary data. Checking get_boundary_rc_values...")

# Let me look at what OCHRE needs for boundary construction
from ochre.utils.envelope import get_boundary_rc_values, BOUNDARY_GROUPS

# Actually, let me try a different approach - use the Dwelling class which has
# HPXML import. But we don't have BESTEST HPXML files.
# Let me instead directly construct the RC model.

# Simplest approach: construct an Envelope with direct capacitances/resistances
# that match Case 600, run it in both modes.

# Case 600 RC network (simplified 1R1C for the zone):
# Zone air node: LIV
# External node: EXT
# Single boundary: wall with combined UA

# For a proper test, let me create a proper OCHRE envelope with boundaries
# using the HPXML-style boundary format that OCHRE expects.

# Boundary format from OCHRE's get_boundary_rc_values:
# Each boundary has: Area, Zone, R Value, etc.

# Let me try a super-simple approach: single wall, single zone, Denver TMY

# For a quick diagnostic, I can compute what the steady-state heating load
# difference would be between conv-only and combined R_film, without running
# a full annual simulation.

# The key insight: in steady state, LWR injection DOES compensate for conv-only R_film
# because the total effective conductance is h_conv + h_rad_to_zone.
# The question is whether the transient (hourly) lag causes a 7% error.

# Let me compute this analytically instead of running a full sim.

print("\n=== Analytical comparison: conv-only vs combined R_film ===")

# Case 600 wall (vertical, interior side)
h_conv = 3.076  # TARP vertical, delta_t=12.9
h_rad = 4 * 0.9 * 5.670374e-8 * 293.15**3  # at T=20C
h_combined = h_conv + h_rad
R_film_conv = 1.0 / h_conv
R_film_combined = 1.0 / h_combined

print(f"h_conv = {h_conv:.4f} W/m2K")
print(f"h_rad  = {h_rad:.4f} W/m2K") 
print(f"h_combined = {h_combined:.4f} W/m2K")
print(f"R_film_conv = {R_film_conv:.4f} m2K/W")
print(f"R_film_combined = {R_film_combined:.4f} m2K/W")
print(f"ASHRAE R_si = {1/8.29:.4f} m2K/W")

# Material R-value (same in both modes)
R_material = 1.0/0.514 - 1.0/29.3 - 1.0/8.29  # from spec
R_so = 1.0/29.3

# In "combined" mode, total wall R:
R_total_combined = R_so + R_material + R_film_combined
U_combined = 1.0 / R_total_combined

# In "conv-only + LWR" mode:
# The RC network has R_total_conv = R_so + R_material + R_film_conv
# But LWR adds an additional path from surface to zone air
# In steady state, effective conductance = h_conv + h_rad_to_zone_air
# The radiation_frac determines how much LWR goes to zone air directly

# With conv-only R_film:
# radiation_frac = R_film_conv / (R_film_conv + R_material/2)  # for a 2-node wall
# For a 1-node wall (no half-R), it's different

# Actually in OCHRE, radiation_frac = res_film / (res_film + res_material)
# where res_film is in K/W (area-normalized), res_material is in K/W
# For a 1-node wall: res_film = R_film * Area, res_material = R_material * Area

# For a 2-node wall (2R1C), OCHRE splits R_material into two halves
# radiation_frac = res_film / (res_film + res_material_half)

# The key: radiation_frac with conv-only R_film is much higher than with combined
# This means MORE LWR goes to the surface node and LESS to zone air
# The LWR that goes to surface node must still flow through R_film to reach zone air
# This creates a lag.

# Let me compute the effective time constant for both modes
# tau = C * R (where C is thermal capacitance, R is the dominant resistance)

# Case 600 wall thermal mass (very lightweight):
# Wood siding: 5mm, rho=544, cp=1210 -> C = 0.005*544*1210 = 3291 J/m2K
# Insulation: 65mm, rho=12, cp=840 -> C = 0.065*12*840 = 655 J/m2K
# Plasterboard: 12mm, rho=950, cp=840 -> C = 0.012*950*840 = 9576 J/m2K
# Total: ~13522 J/m2K (lightweight)

C_wall = 13522  # J/m2K, approximate for Case 600 wall

# Time constant with combined R_film (surface to zone air)
tau_combined = C_wall * R_film_combined  # seconds
tau_conv_only = C_wall * R_film_conv  # seconds

print(f"\nWall thermal mass: {C_wall:.0f} J/m2K")
print(f"tau (combined R_film): {tau_combined/3600:.3f} hours")
print(f"tau (conv-only R_film): {tau_conv_only/3600:.3f} hours")

# For the full wall (surface to outdoor):
tau_full_combined = C_wall * (R_film_combined + R_material + R_so)
tau_full_conv = C_wall * (R_film_conv + R_material + R_so)
print(f"tau_full (combined): {tau_full_combined/3600:.2f} hours")
print(f"tau_full (conv-only): {tau_full_conv/3600:.2f} hours")

# The difference in time constant is ~0.55 hours, which on a 1-hour timestep
# introduces a phase lag. For heating-dominated climates (Denver winter),
# this lag means the zone doesn't warm up as fast during setback recovery,
# leading to higher heating consumption. But we're seeing LOWER heating...

# Wait - both heating AND cooling are below band minimums.
# This means the zone is TOO WELL INSULATED, not too poorly insulated.
# The 10.5% higher wall R means less heat flows through walls in BOTH directions.

# In steady state, LWR should compensate. But does it?
# ScriptF exchanges heat BETWEEN surfaces - it's zero-sum.
# The only net heat flow to zone air from LWR is through the radiation_frac split.
# With conv-only R_film, radiation_frac ≈ 0.89, so only 11% of LWR goes to zone air.
# With combined R_film, radiation_frac ≈ 0.76, so 24% of LWR goes to zone air.

# But wait - LWR IS zero-sum. The net LWR to all surfaces is zero.
# The split only determines where the zero-sum exchange is injected.
# In steady state, the total heat flow through the wall should be the same
# regardless of the split, because the surface temperature adjusts.

# The issue is TRANSIENT: on an hourly timestep, the surface temperature
# doesn't fully adjust, so the effective coupling is different.

# Let me compute the effective hourly-averaged coupling
# This requires solving the transient RC equations, which is complex.
# Instead, let me just run the OCHRE experiment.

print("\n=== OCHRE experiment needed ===")
print("Need to run OCHRE Case 600 in both 'full' and 'linear' modes")
print("to determine if conv-only R_film is the root cause of BESTEST failures.")
