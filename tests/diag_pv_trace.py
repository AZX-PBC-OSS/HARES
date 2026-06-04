"""Diagnostic: trace PV intermediate values to find underproduction root cause."""
import sys
sys.path.insert(0, 'python')
import polars as pl
import pandas as pd
import numpy as np
from ochre_next import Dwelling, PV
from ochre_next.data import fetch_resstock_building

bldg = fetch_resstock_building(1, version='2025.1')

# --- Read weather data directly ---
w = pd.read_csv(bldg.weather_path)
print(f"Weather shape: {w.shape}")
print(f"Weather columns: {list(w.columns)}")

# Check GHI, DNI, DHI, Tamb, WS at noon hours
hours_of_interest = [f'2018-07-01 {h:02d}:00:00' for h in range(6, 20)]
for ts in hours_of_interest:
    row = w[w['date_time'] == ts]
    if len(row) == 0:
        continue
    print(f"  {ts}: GHI={row['Global Horizontal Radiation [W/m2]'].values[0]:7.1f}  "
          f"DNI={row['Direct Normal Radiation [W/m2]'].values[0]:7.1f}  "
          f"DHI={row.get('Diffuse Horizontal Radiation [W/m2]', pd.Series(['N/A'])).values[0]:7s}  "
          f"Tamb={row['Dry Bulb Temperature [°C]'].values[0]:5.1f}  "
          f"WS={row['Wind Speed [m/s]'].values[0]:4.1f}")

print(f"\nWind speed col: {[c for c in w.columns if 'wind' in c.lower() or 'ws' in c.lower()]}")
print(f"Diffuse col: {[c for c in w.columns if 'diffus' in c.lower() or 'dhi' in c.lower()]}")

# --- Manual Perez model ---
# Constants from hares-physics solar.rs
PEREZ_KAPPA = 1.041
COS_85_DEG = np.cos(np.radians(85.0))
ISOTROPIC_VIEW_FACTOR = 0.5

# Perez coefficients for 8 bins (from solar.rs lines 29-57)
PEREZ_COEFFICIENTS = np.array([
    [ 1.352, -1.221, -0.787, -0.196,  0.072,  0.116],  # bin 0
    [-0.767, -1.277, -0.954,  0.159,  0.049,  0.160],  # bin 1
    [ 1.102, -2.265, -0.865,  0.145,  0.113,  0.187],  # bin 2
    [-0.391,  0.260,  0.288, -0.393,  0.147,  0.237],  # bin 3
    [-1.059,  1.071,  0.805, -0.541,  0.288,  0.418],  # bin 4
    [-1.521,  2.462,  1.514, -1.036,  0.481,  0.724],  # bin 5
    [-1.765,  4.386,  2.960, -1.279,  0.725,  1.128],  # bin 6
    [-1.818,  6.750,  4.860, -1.499,  1.087,  1.801],  # bin 7
])

def perez_bin(epsilon):
    """Map epsilon to bin index (1-8 → 0-7)."""
    if epsilon < 1.065:
        return 0
    elif epsilon < 1.230:
        return 1
    elif epsilon < 1.500:
        return 2
    elif epsilon < 1.950:
        return 3
    elif epsilon < 2.800:
        return 4
    elif epsilon < 4.500:
        return 5
    elif epsilon < 6.200:
        return 6
    else:
        return 7

def perez_tilted(gwh, dni, dhi, zenith_deg, az_sun_deg, tilt_deg, az_surf_deg, day_of_year, albedo=0.2):
    """Perez 1990 model, matching hares-physics/src/solar.rs:451-548."""
    if gwh <= 0 and dni <= 0 and dhi <= 0:
        return {'direct': 0.0, 'diffuse': 0.0, 'reflected': 0.0, 'poa': 0.0, 'aoi_deg': 90.0}

    # AOI
    alt = 90.0 - zenith_deg
    alt_r, az_s_r, tilt_r, az_p_r = np.radians([alt, az_sun_deg, tilt_deg, az_surf_deg])
    cos_aoi = np.cos(alt_r)*np.sin(tilt_r)*np.cos(az_s_r - az_p_r) + np.sin(alt_r)*np.cos(tilt_r)
    aoi_deg = np.degrees(np.arccos(np.clip(cos_aoi, -1, 1)))

    if dhi < 1.0 or zenith_deg > 87.0:
        # Isotropic fallback
        cos_zenith = np.cos(np.radians(zenith_deg))
        beam = dni * max(cos_aoi, 0)
        diff = dhi * (1 + np.cos(np.radians(tilt_deg))) / 2
        refl = gwh * albedo * (1 - np.cos(np.radians(tilt_deg))) / 2
        return {'direct': beam, 'diffuse': diff, 'reflected': refl, 'poa': beam+diff+refl, 'aoi_deg': aoi_deg}

    zenith_rad = np.radians(zenith_deg)
    tilt_rad = np.radians(tilt_deg)

    # Extraterrestrial irradiance
    i0 = 1367.0 * (1 + 0.033 * np.cos(2*np.pi*day_of_year/365))

    # Airmass
    cos_z = np.cos(zenith_rad)
    am = 1.0 / (cos_z + 0.50572*(96.07995 - zenith_deg)**(-1.6364)) if zenith_deg < 90 else 999

    # Epsilon, delta
    z3 = zenith_rad**3
    eps = ((dhi + dni) / dhi + PEREZ_KAPPA*z3) / (1 + PEREZ_KAPPA*z3)
    delta = dhi * am / i0

    bin_idx = perez_bin(eps)
    c = PEREZ_COEFFICIENTS[bin_idx]
    f1 = max(c[0] + c[1]*delta + c[2]*zenith_rad, 0.0)
    f2 = c[3] + c[4]*delta + c[5]*zenith_rad

    a = max(np.cos(np.radians(aoi_deg)), 0.0)
    b = max(cos_z, COS_85_DEG)

    diffuse = dhi * ((1-f1)*(1+np.cos(tilt_rad))*ISOTROPIC_VIEW_FACTOR + f1*a/b + f2*np.sin(tilt_rad))
    diffuse = max(diffuse, 0)

    beam = dni * max(np.cos(np.radians(aoi_deg)), 0)
    reflected = gwh * albedo * (1 - np.cos(tilt_rad)) * ISOTROPIC_VIEW_FACTOR
    reflected = max(reflected, 0)

    return {'direct': beam, 'diffuse': diffuse, 'reflected': reflected, 'poa': beam+diffuse+reflected, 'aoi_deg': aoi_deg}

# --- NON-LUT power calc (matching mod.rs:493-506) ---
def pv_power(capacity_kw, poa_w_m2, tamb_c, wind_speed_ms, noct_c=45.0, gamma=-0.0047,
             system_losses=0.14, inv_eff=0.96, t_ref=25.0):
    noct_factor = (noct_c - 20.0) / 800.0
    wind_correction = 9.5 / (5.7 + 3.8 * max(wind_speed_ms, 0.0))
    t_cell = tamb_c + poa_w_m2 * noct_factor * wind_correction
    temp_derate = max(1.0 + gamma * (t_cell - t_ref), 0.0)
    dc_before = capacity_kw * (poa_w_m2 / 1000.0) * temp_derate
    dc = dc_before * (1.0 - system_losses)
    ac = dc * inv_eff
    return {'ac': ac, 'dc_before': dc_before, 'dc': dc, 't_cell': t_cell, 'temp_derate': temp_derate}

# Pick a specific timestep to analyze
# July 1, noon
ts = '2018-07-01 12:00:00'
row = w[w['date_time'] == ts].iloc[0]
ghi = row['Global Horizontal Radiation [W/m2]']
dni = row['Direct Normal Radiation [W/m2]']
dhi = row.get('Diffuse Horizontal Radiation [W/m2]', np.nan)

if pd.isna(dhi):
    dhi_col = [c for c in w.columns if 'diffus' in c.lower() or 'dhi' in c.lower()]
    if dhi_col:
        dhi = row[dhi_col[0]]
    else:
        print("WARNING: DHI column not found, estimating from GHI-DNI")

# Need latitude, longitude to compute solar position
# ResStock building 1 is in... let's find the HPXML
import xml.etree.ElementTree as ET
tree = ET.parse(bldg.hpxml_path)
ns = {'h': 'http://hpxmlonline.com/2019/10'}
site = tree.find('.//h:Site', ns) or tree.find('.//h:BuildingDetails/h:ClimateandRiskZones', ns) or tree.find('.//h:BuildingSummary/h:Site', ns)
# Try to find lat/lon
lat_el = tree.find('.//h:Latitude', ns)
lon_el = tree.find('.//h:Longitude', ns)
lat = float(lat_el.text) if lat_el is not None else 40.0
lon = float(lon_el.text) if lon_el is not None else -105.0
tz = -7  # Mountain Standard

print(f"\n--- Manual Analysis ---")
print(f"Location: lat={lat}, lon={lon}")
print(f"Weather at {ts}: GHI={ghi}, DNI={dni}, DHI={dhi}")
print(f"Tamb={row['Dry Bulb Temperature [°C]']}, WS={row['Wind Speed [m/s]']}")

# Solar position at noon
from datetime import datetime, timezone
import pytz
# The weather timestamps are likely local standard time
local_tz = pytz.FixedOffset(tz * 60)  # UTC-7
dt_local = local_tz.localize(datetime(2018, 7, 1, 12, 0, 0))
dt_utc = dt_local.astimezone(pytz.UTC)
print(f"Local noon = {dt_utc} UTC")

# Solar declination
day_of_year = dt_local.timetuple().tm_yday
decl = 23.45 * np.sin(np.radians(360/365 * (284 + day_of_year)))
print(f"Day {day_of_year}, declination = {decl:.2f}°")

# Hour angle (at noon = 0, at 1pm = 15°)
for hour in [11, 12, 13, 14]:
    ha = (hour - 12) * 15  # hour angle in degrees
    cos_zenith = np.sin(np.radians(lat))*np.sin(np.radians(decl)) + np.cos(np.radians(lat))*np.cos(np.radians(decl))*np.cos(np.radians(ha))
    zenith = np.degrees(np.arccos(np.clip(cos_zenith, -1, 1)))
    if abs(ha) < 1e-9:
        az = 180.0  # south
    else:
        sin_az = -np.cos(np.radians(decl))*np.sin(np.radians(ha))/np.sin(np.radians(zenith))
        az = np.degrees(np.arcsin(np.clip(sin_az, -1, 1)))
        if np.cos(np.radians(ha)) >= np.tan(np.radians(decl))/np.tan(np.radians(lat)):
            az = 180 - az if ha < 0 else 180 + az
        az = az % 360
    print(f"  Hour {hour:2d}:00 — HA={ha:6.1f}°, zenith={zenith:.2f}°, azimuth={az:.1f}°")

# Compute Perez POA at noon for 20° tilt
zenith_noon = np.degrees(np.arccos(np.clip(np.sin(np.radians(lat))*np.sin(np.radians(decl)) + np.cos(np.radians(lat))*np.cos(np.radians(decl)), -1, 1)))
az_noon = 180.0

if pd.isna(dhi):
    # Estimate DHI from GHI and DNI if not available
    cos_z = np.cos(np.radians(zenith_noon))
    dhi_est = max(ghi - dni * cos_z, 0)
    print(f"\nDHI estimated: {dhi_est:.1f} (GHI={ghi} - DNI*cos(z)={dni*cos_z:.1f})")
    dhi = dhi_est

result = perez_tilted(ghi, dni, dhi, zenith_noon, az_noon, 20.0, 180.0, day_of_year)
print(f"\nPerez result for 20° tilt south-facing at noon:")
print(f"  Direct tilted:     {result['direct']:.1f} W/m²")
print(f"  Diffuse tilted:    {result['diffuse']:.1f} W/m²")
print(f"  Reflected:         {result['reflected']:.1f} W/m²")
print(f"  Total POA:         {result['poa']:.1f} W/m²")
print(f"  AOI:               {result['aoi_deg']:.1f}°")

# Now feed into power calc
tamb = row['Dry Bulb Temperature [°C]']
ws = row['Wind Speed [m/s]']
p = pv_power(10.0, result['poa'], tamb, ws)
print(f"\nPV Power (non-LUT path, default params):")
print(f"  T_cell:            {p['t_cell']:.1f}°C")
print(f"  Temp derate:       {p['temp_derate']:.4f}")
print(f"  DC before losses:  {p['dc_before']:.2f} kW")
print(f"  DC after losses:   {p['dc']:.2f} kW")
print(f"  AC output:         {p['ac']:.2f} kW")

# Also check with different NOCT
print(f"\n--- Sensitivity ---")
for noct in [45.0, 49.0]:
    for array_type in ['OpenRack', 'RoofMounted']:
        p = pv_power(10.0, result['poa'], tamb, ws, noct_c=noct)
        print(f"  NOCT={noct} ({array_type}): AC={p['ac']:.2f} kW, Tcell={p['t_cell']:.1f}°C")

# Check at different hours
print(f"\n--- Power at different hours (20° tilt, south) ---")
for hour in range(9, 17):
    ts_h = f'2018-07-01 {hour:02d}:00:00'
    row_h = w[w['date_time'] == ts_h].iloc[0]
    ghi_h = row_h['Global Horizontal Radiation [W/m2]']
    dni_h = row_h['Direct Normal Radiation [W/m2]']
    if dhi_col:
        dhi_h = row_h[dhi_col[0]]
    else:
        cos_z_h = np.cos(np.radians(zenith_for_hour(lat, decl, hour)))
        dhi_h = max(ghi_h - dni_h * cos_z_h, 0)

    ha = (hour - 12) * 15
    cos_z = np.sin(np.radians(lat))*np.sin(np.radians(decl)) + np.cos(np.radians(lat))*np.cos(np.radians(decl))*np.cos(np.radians(ha))
    z_h = np.degrees(np.arccos(np.clip(cos_z, -1, 1)))
    az_h = 180.0 if abs(ha) < 1e-3 else 180.0 + (np.sign(ha) * 50)  # approximate
    # Better azimuth calc
    if abs(ha) < 0.001:
        az_h = 180.0
    else:
        sin_az = -np.cos(np.radians(decl))*np.sin(np.radians(ha))/np.sin(np.radians(z_h))
        cos_az = (np.sin(np.radians(decl)) - np.sin(np.radians(lat))*np.cos(np.radians(z_h)))/(np.cos(np.radians(lat))*np.sin(np.radians(z_h)))
        az_h = np.degrees(np.arctan2(sin_az, cos_az)) % 360

    r = perez_tilted(ghi_h, dni_h, dhi_h, z_h, az_h, 20.0, 180.0, day_of_year)
    p_h = pv_power(10.0, r['poa'], row_h['Dry Bulb Temperature [°C]'], row_h['Wind Speed [m/s]'])
    print(f"  {hour:2d}:00 — GHI={ghi_h:6.0f} DNI={dni_h:6.0f} POA={r['poa']:6.0f} "
          f"Tamb={row_h['Dry Bulb Temperature [°C]']:5.1f}°C Tcell={p_h['t_cell']:5.1f}°C AC={p_h['ac']:.2f} kW")

# Now run the actual simulation and compare
print(f"\n--- Simulation Comparison ---")
dw = Dwelling.from_hpxml(
    str(bldg.hpxml_path), str(bldg.schedule_path), str(bldg.weather_path),
    defaults_path='vendor/HARES/defaults',
    start_time='2021-07-01T12:00:00', duration_s=7200, time_res_s=900,
    output_verbosity=4,
)
dw.initialize()
dw.add_pv(PV("PV", capacity_kw=10.0, tilt=20.0, azimuth=180.0))
df = dw.simulate()
print("Simulated PV output per timestep:")
for row in df.iter_rows(named=True):
    print(f"  {row['Time']}: PV={-row['PV Electric Power (kW)']:.2f} kW")
print(f"\nPeak PV AC: {-df['PV Electric Power (kW)'].min():.2f} kW")
