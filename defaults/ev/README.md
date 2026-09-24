# EV Defaults Data

## Purpose

This directory contains electric vehicle (EV) charging load profiles for
residential load simulation. Data is sourced from NREL EVI-Pro (EVERMI)
simulation output and provides both time-series load profiles and session-level
charging records.

## File Summary

### Time-Series Load Data

| File | Schema | Description |
|------|--------|-------------|
| `EV Profiles.csv` | `Time, Vehicle 1..Vehicle 50` | 10-minute-interval power (W) for 50 anonymous vehicles over one year (52,560 rows). Values are predominantly 0 W (idle/not plugged in) or 1,920 W (Level 1 charging at 16 A / 120 V). **This is a time-series load file, not a vehicle specification lookup table.** |

### Session-Level Charging Data (Aggregate by Type)

Files with schema `,vehicle_id, day_id, day_of_week, plug_id, plug_type, destination_type, destination_id, destination_category, destination_description, avg_power_kw, tou_participation, res_access, cbsa_code, cbsa_name, Capacity (kWh), County Type, start_time, duration, charge_time, total_charge, start_soc`:

| File | Vehicle Type | Capacity (kWh) | Avg Power (kW) | Records |
|------|-------------|----------------|----------------|---------|
| `BEV_level_1.csv` | `MY2030_BEV_SUV` | 117.6 | 1.26 | ~2,297 |
| `BEV_level_2.csv` | `MY2030_BEV_SUV` | 117.6 | 10.26 | ~15,258 |
| `PHEV_level_1.csv` | `MY2030_PHEV_SUV` | 14.8 | 1.26 | ~5,135 |
| `PHEV_level_2.csv` | `MY2030_PHEV_SUV` | 14.8 | 7.20 | ~24,489 |

**Note**: These files have a leading empty column (the first comma in the
header). The `pdf_Veh*` files below do not. Parsers must handle both formats
when reading by column name rather than position.

### Session-Level Charging Data (Per Vehicle)

Files with schema `day_id, start_time, duration, start_soc, weekday, temperature`:

| File | Vehicle | Description | Records |
|------|---------|-------------|---------|
| `pdf_Veh1_Level0.csv` | Vehicle 1 | Level 0 (idle/unplugged) sessions | ~41 |
| `pdf_Veh1_Level1.csv` | Vehicle 1 | Level 1 charging sessions | ~644,939 |
| `pdf_Veh1_Level2.csv` | Vehicle 1 | Level 2 charging sessions | ~642,587 |
| `pdf_Veh2_Level1.csv` | Vehicle 2 | Level 1 charging sessions | |
| `pdf_Veh2_Level2.csv` | Vehicle 2 | Level 2 charging sessions | |
| `pdf_Veh3_Level1.csv` | Vehicle 3 | Level 1 charging sessions | |
| `pdf_Veh3_Level2.csv` | Vehicle 3 | Level 2 charging sessions | |
| `pdf_Veh4_Level1.csv` | Vehicle 4 | Level 1 charging sessions | |
| `pdf_Veh4_Level2.csv` | Vehicle 4 | Level 2 charging sessions | |

**Note**: The `pdf_Veh*` files do not contain a vehicle type column. The
vehicle type (BEV/PHEV, capacity, charger rating) for these four vehicles is
not explicitly declared in any file.

### Field Descriptions

| Field | Unit | Description |
|-------|------|-------------|
| `day_id` | — | EVI-Pro day identifier |
| `start_time` | minutes from midnight | Session start time |
| `duration` | minutes | Total plugged-in duration |
| `charge_time` | minutes | Active charging time (≤ duration) |
| `total_charge` | kWh | Energy delivered in session |
| `start_soc` | % | State of charge at session start |
| `avg_power_kw` | kW | Average charging power (battery-side, after losses) |
| `Capacity (kWh)` | kWh | Vehicle battery capacity |
| `plug_type` / `plug_id` | — | Charging level identifier (`level_1` or `level_2`) |
| `temperature` | °C | Ambient temperature for session |
| `weekday` | 1–7 | Day of week (1 = Sunday) |

### Charging Power Notes

- **Level 1 power (1.26 kW)**: This represents battery-side delivered power
  after charging losses. At typical L1 grid-to-battery efficiency of ~85–88%,
  wall power is approximately 1.44–1.48 kW (12 A at 120 V). The 1.26 kW value
  bundles charging losses into a single number rather than separating wall
  power and efficiency.

  Source: SAE J1772 Standard, typical residential L1 deployment 12–16 A at
  120 V = 1.44–1.92 kW wall power.

- **BEV Level 2 power (10.26 kW)**: This represents ~43 A at 240 V, consistent
  with a 48 A hardwired EVSE installation. Residential L2 installations more
  commonly deliver 7.7 kW (32 A at 240 V) via NEMA 14-50 outlets. The 10.26 kW
  value models the high end of residential capability.

  Source: SAE J1772 Standard, typical residential L2 deployment 32–48 A at
  240 V = 7.7–11.5 kW.

- **PHEV Level 2 power (7.2 kW)**: Consistent with a 30 A circuit or 32 A
  onboard charger at ~225 V. Reasonable for a PHEV SUV.

### Level 0

`pdf_Veh1_Level0.csv` defines "Level 0" sessions — this is not a standard SAE
J1772 charging level. Based on the data (41 records, low start_soc values,
durations of 631–712 minutes), these likely represent idle/unplugged periods or
trickle-discharge states where the vehicle is at home but not charging.

## Cross-Reference: Vehicle Relationships

### Anonymous Vehicles in EV Profiles.csv

`EV Profiles.csv` contains 50 anonymous vehicles (columns `Vehicle 1` through
`Vehicle 50`). There is **no mapping** between these 50 vehicles and the
vehicle types/profiles in the other CSV files. This is a known data gap.

The two datasets were generated independently:

| Dataset | Files | Vehicles | Has Type Info? |
|---------|-------|----------|----------------|
| Time-series loads | `EV Profiles.csv` | 50 anonymous vehicles | No — power values only |
| Session records | `BEV_*.csv`, `PHEV_*.csv` | 2 vehicle types (BEV SUV, PHEV SUV) | Yes — capacity, power, efficiency |
| Session records | `pdf_Veh1–4_*.csv` | 4 per-vehicle profiles | No type info — session timing only |

### Observed Charging Power in EV Profiles.csv

The EV Profiles time-series contains non-zero power values at exactly 1,920 W
(consistent with Level 1 charging at 16 A / 120 V wall power). No values match
Level 2 charging power (7.2–10.26 kW), suggesting the 50 profiled vehicles are
charging exclusively at Level 1.

> **Known Gap**: The 1,920 W wall power in EV Profiles.csv is not directly
> comparable to the 1.26 kW battery-side power in the BEV/PHEV session files.
> At ~87.5% efficiency, 1.26 kW battery power corresponds to ~1.44 kW wall
> power, not 1.92 kW. This discrepancy has not been resolved — it may reflect
> different charging rate assumptions or different data pipeline processing.

### Vehicle Fleet Diversity

Only two vehicle types are represented in the type-specific files:
- `MY2030_BEV_SUV`: 117.6 kWh capacity, mid-size electric SUV (comparable to
  Tesla Model X ~100 kWh, F-150 Lightning ER ~131 kWh, Rivian R1S ~135 kWh)
- `MY2030_PHEV_SUV`: 14.8 kWh capacity, mid-size PHEV SUV (comparable to
  RAV4 Prime ~18.1 kWh, Mitsubishi Outlander PHEV ~13.8 kWh)

Fleet diversity (short-range commuter cars, medium sedans, long-range trucks)
is not captured through distinct vehicle type parameters. Driving pattern
diversity is modeled solely through variance in session SOC deltas and
durations within a single type.

### CSV Format Inconsistency

| File Group | Leading Empty Column? |
|------------|----------------------|
| `BEV_level_*.csv`, `PHEV_level_*.csv` | **Yes** — header starts with `,vehicle_id` |
| `pdf_Veh*_Level*.csv` | No — header starts with `day_id` |

## Data Sources

- **EVI-Pro / EVERMI** (NREL): Vehicle charging profiles, session schedules,
  battery capacities, and charger parameters for projected 2030-era vehicles.
- **SAE J1772**: Charging level definitions and connector standards.
- **DOE fueleconomy.gov**: Vehicle specifications for capacity and efficiency
  validation.

## Last Updated

2026-06-06 — Documentation added as part of review finding defdata-05
resolution. Data files unchanged from NREL EVI-Pro source output.
