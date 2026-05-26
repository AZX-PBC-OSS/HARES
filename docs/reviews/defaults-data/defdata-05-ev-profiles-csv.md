# EV Profiles.csv and per-vehicle/per-level CSVs vs EVI-Pro and DOE data

**Review ID**: defdata-05
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed

- `defaults/ev/EV Profiles.csv`
- `defaults/ev/BEV_level_1.csv`
- `defaults/ev/BEV_level_2.csv`
- `defaults/ev/PHEV_level_1.csv`
- `defaults/ev/PHEV_level_2.csv`
- `defaults/ev/pdf_Veh1_Level0.csv`
- `defaults/ev/pdf_Veh1_Level1.csv`
- `defaults/ev/pdf_Veh1_Level2.csv`
- `defaults/ev/pdf_Veh2_Level1.csv`
- `defaults/ev/pdf_Veh2_Level2.csv`
- `defaults/ev/pdf_Veh3_Level1.csv`
- `defaults/ev/pdf_Veh3_Level2.csv`
- `defaults/ev/pdf_Veh4_Level1.csv`
- `defaults/ev/pdf_Veh4_Level2.csv`

## Vendor/Reference Files Consulted

None

## Findings

### Finding 1: [Severity: high]
**Description**: `EV Profiles.csv` is a time-series load data file, NOT a master vehicle specification list. The file contains 146 rows of 10-minute-interval power data (watts) for 50 vehicles, not vehicle type specifications with battery capacity, charger power, and efficiency. The review instructions anticipated a lookup/master table but this file is raw load profile data.
**Code Location**: `defaults/ev/EV Profiles.csv:1` (header: `Time,Vehicle 1...Vehicle 50`)
**Root Cause**: File naming and/or review expectations are mismatched — there is no master vehicle-spec table anywhere under `defaults/ev/`. All vehicle specification metadata (capacity, power, efficiency) is embedded only in the BEV/PHEV session-level CSV files.
**Impact**: Users expecting a vehicle specs lookup table will not find one. The 50 vehicles in `EV Profiles.csv` have no metadata describing their type (BEV/PHEV), capacity, or charging parameters. It is unclear whether these 50 vehicles correspond to the 4 pdf_Veh profiles or to separate vehicle types.

### Finding 2: [Severity: high]
**Description**: Level 1 charging power in `BEV_level_1.csv` and `PHEV_level_1.csv` is 1.26 kW, which is below the typical residential L1 range (12-16 A at 120 V = 1.44-1.92 kW) and inconsistent with the actual charging power observed in `EV Profiles.csv` (1920 W = 1.92 kW). 1.26 kW implies only ~10.5 A at 120 V or assumes 85% grid-to-battery efficiency at 1.44 kW wall power.
**Code Location**: `defaults/ev/BEV_level_1.csv:2` (column `avg_power_kw`, value `1.26`); `defaults/ev/PHEV_level_1.csv:2` (same column, value `1.26`)
**Root Cause**: The `avg_power_kw` field appears to represent delivered battery power (after charging losses) rather than wall power. At typical L1 efficiency of ~85-88%, this yields ~1.44-1.48 kW wall power. Alternatively, this is a conservative modeling assumption for a 12 A/120 V circuit with significant voltage drop.
**Impact**: Total energy delivered per L1 session is understated by ~13-34% compared to wall-power-based modeling. For a simulation aggregating hundreds of vehicles, this understates residential L1 load by the same margin.

### Finding 3: [Severity: medium]
**Description**: Level 2 charging power differs between BEV (10.26 kW) and PHEV (7.2 kW). BEV L2 at 10.26 kW exceeds typical residential 32 A/240 V (7.7 kW) and implies ~43 A at 240 V. While within the J1772 maximum (80 A/19.2 kW), 10.26 kW is at the upper end of residential capability and typical of a 48 A hardwired EVSE. PHEV L2 at 7.2 kW is reasonable for a 30 A circuit or 32 A at ~225 V.
**Code Location**: `defaults/ev/BEV_level_2.csv:2` (`avg_power_kw = 10.26`); `defaults/ev/PHEV_level_2.csv:2` (`avg_power_kw = 7.2`)
**Root Cause**: These likely represent EVI-Pro/EVERMI vehicle model assumptions where BEV SUVs have higher onboard charger ratings (48 A) while PHEVs have lower onboard chargers (30 A).
**Impact**: Residential L2 load is modeled at the high end of BEV capability. For homes with shared circuits or older wiring, this may overestimate available charging power. No differentiation between 32 A, 40 A, and 48 A L2 installations is present — all BEV L2 sessions use a single fixed value.

### Finding 4: [Severity: medium]
**Description**: Only two vehicle types are represented in the BEV/PHEV aggregate files: `MY2030_BEV_SUV` (117.6 kWh) and `MY2030_PHEV_SUV` (14.8 kWh). The `pdf_Veh1-4` files have NO vehicle type column. This means the simulation uses at most ONE BEV model and ONE PHEV model, rather than the diverse fleet (short-range commuter cars, medium sedans, long-range trucks, etc.) recommended by the review instructions.
**Code Location**: `BEV_level_1.csv:2` (column `vehicle_id` = `MY2030_BEV_SUV`); `PHEV_level_1.csv:2` (`MY2030_PHEV_SUV`); `pdf_Veh1_Level1.csv:1` (no vehicle type in header).
**Root Cause**: The data appears sourced from a single EVI-Pro vehicle class simulation (2030 mid-size SUV BEV and PHEV). Additional vehicle classes (compact sedan, full-size truck, etc.) have not been incorporated.
**Impact**: The diversity of driving patterns (short-range commuter 10-30 mi/day, medium 30-60 mi/day, long 60+ mi/day, weekend-only, fleet, stay-at-home) must be captured entirely through variance in individual session SOC deltas and durations within a single vehicle type, rather than using fleet-composition weighting with distinct vehicle parameters. This limits the fidelity of aggregate load shape modeling.

### Finding 5: [Severity: medium]
**Description**: No mapping exists between the 50 vehicles in `EV Profiles.csv` and the vehicle profiles in the per-vehicle/per-type CSV files. The EV Profiles file has 50 anonymous vehicles (Vehicle 1 through Vehicle 50), but the per-vehicle files cover only pdf_Veh1-4 plus aggregate BEV/PHEV types. There is no key, lookup, or metadata file explaining which of the 50 profile vehicles correspond to which vehicle type.
**Code Location**: `defaults/ev/EV Profiles.csv` (50 vehicle columns, no type mapping); `defaults/ev/` directory (no mapping file present)
**Root Cause**: The two datasets (time-series load profiles and charging session data) appear to be generated/computed independently without a cross-reference.
**Impact**: Any code that reads these defaults files must either hard-code or infer the vehicle-to-type mapping. A mismatch could silently apply wrong charging data to wrong vehicles.

### Finding 6: [Severity: low]
**Description**: `start_soc` values in BEV/PHEV session files use 8+ decimal digits of precision (e.g., `93.83928571`, `82.55102041`). This high precision indicates these are computed output values (likely from EVI-Pro preprocessing) rather than human-specified input parameters.
**Code Location**: `defaults/ev/BEV_level_1.csv:2` (`start_soc = 93.83928571`); `defaults/ev/BEV_level_2.csv:2` (`82.55102041`); pervasive throughout all BEV/PHEV CSV files.
**Root Cause**: These SOC values were likely derived from fractional kWh computations (battery capacity / energy consumed) without rounding to practical precision.
**Impact**: While not a computational error, excessive precision suggests the data has not been reviewed/rationalized for production use. A CSV parse issue could potentially read these as integers or truncated floats depending on the parser implementation.

### Finding 7: [Severity: low]
**Description**: `pdf_Veh1_Level0.csv` defines a "Level 0" charging mode (5 columns: `day_id, start_time, duration, start_soc, weekday, temperature`). The meaning of "Level 0" is not documented. It contains 41 records. The schema matches the Level 1/Level 2 files for Veh1.
**Code Location**: `defaults/ev/pdf_Veh1_Level0.csv:1`
**Root Cause**: "Level 0" is not a standard SAE J1772 charging level. It likely represents either no charging (idle), trickle charging, or vehicle-to-grid discharge sessions.
**Impact**: Without documentation, code consuming this file may misinterpret "Level 0" data and either ignore or misapply these sessions.

### Finding 8: [Severity: low]
**Description**: CSV format inconsistency — `BEV_level_1.csv`, `BEV_level_2.csv`, `PHEV_level_1.csv`, and `PHEV_level_2.csv` have a leading empty column (empty header + empty first field in each row). The `pdf_Veh*_Level*.csv` files have no leading empty column. All `EV Profiles.csv` rows appear consistent.
**Code Location**: `defaults/ev/BEV_level_1.csv:1` (starts with `,vehicle_id...` — leading comma creates an unnamed column); `defaults/ev/pdf_Veh1_Level1.csv:1` (starts with `day_id,start_time...` — no leading empty column)
**Root Cause**: The BEV/PHEV files were generated from a different tool/pipeline than the pdf_Veh files, resulting in different CSV formatting conventions.
**Impact**: Parsers relying on column position rather than name may read wrong columns for BEV/PHEV files. Most CSV parsers handle this correctly, but a column-count-based validation could reject these files.

### Finding 9: [Severity: low]
**Description**: Battery capacity for the BEV SUV is 117.6 kWh. This is consistent with a mid-size or large electric SUV (e.g., Rivian R1S Large pack ~135 kWh, Tesla Model X ~100 kWh, F-150 Lightning ER ~131 kWh). The PHEV at 14.8 kWh is typical for a PHEV SUV (e.g., RAV4 Prime ~18.1 kWh, Mitsubishi Outlander PHEV ~13.8 kWh). Both are within expected ranges for 2030-era vehicles.
**Code Location**: `defaults/ev/BEV_level_1.csv:2` (column `Capacity (kWh)`, value `117.6`); `PHEV_level_1.csv:2` (value `14.8`)
**Root Cause**: Values match EVI-Pro/EVERMI projections for mid-size SUV class in the 2030 timeframe.
**Impact**: N/A — acceptable.

### Finding 10: [Severity: low]
**Description**: No explicit charging efficiency data is present in any CSV. Efficiency must be inferred from `total_charge` / (`duration` / 60 × `avg_power_kw`). For BEV_level_1.csv row 2: duration=345 min, avg_power_kw=1.26, total_charge=7.245 kWh. Calculated: 345/60 × 1.26 = 7.245 kWh, meaning the stored value assumes 100% efficiency between `avg_power_kw` and `total_charge`. Efficiency losses are implicitly accounted for in the `avg_power_kw` value itself (1.26 kW battery-side vs ~1.44-1.92 kW wall-side).
**Code Location**: `defaults/ev/BEV_level_1.csv:2` and all subsequent rows
**Root Cause**: The data pipeline pre-computed battery-side average power, bundling efficiency losses into a single number rather than separating wall power and efficiency.
**Impact**: Cannot independently vary charging efficiency assumptions without regenerating the entire dataset. EVI-Pro typically uses L1 efficiency of ~85-88% and L2 of ~90-93%.

## Summary

- **Total findings**: 10
- **Critical**: 0
- **High**: 2
- **Medium**: 3
- **Low**: 5

## Recommendations

1. **Create a master vehicle specification lookup table** (or document that `EV Profiles.csv` is a time-series, not a spec table) that maps each of the 50 vehicles to a vehicle type (BEV/PHEV) and provides capacity, onboard charger rating, and efficiency for each type.

2. **Clarify or adjust Level 1 charging power** to 1.44 kW or document that the 1.26 kW value represents battery-side delivered power after 85-88% charging losses at 1.44 kW wall power. If the intent is to model wall power, raise to 1.44-1.92 kW.

3. **Add L2 power tier differentiation**: consider representing multiple L2 power levels (e.g., 16 A/3.8 kW, 32 A/7.7 kW, 48 A/11.5 kW) to reflect real-world residential EVSE diversity, rather than a single 10.26 kW value for all BEV L2 sessions.

4. **Expand vehicle type diversity**: add profiles for at least compact sedan BEV (60-65 kWh, lower charger power) and long-range truck/large SUV (130-180 kWh) in addition to the existing mid-size SUV (117.6 kWh). This would enable weighted fleet composition modeling per the reference scenarios (short/medium/long commuters, fleet vehicles, etc.).

5. **Document the pdf_Veh-to-EV-Profiles mapping**: add a cross-reference file or document explaining which vehicle type each of the 50 columns in `EV Profiles.csv` represents.

6. **Document "Level 0"** charging: add a README or header comment explaining what Level 0 means (no charging, idle, V2G, etc.).

7. **Normalize CSV formats**: either add or remove the leading empty column across all BEV/PHEV files and pdf_Veh files for consistency.

8. **Round SOC values to 2-3 decimal places** (e.g., 93.84 instead of 93.83928571) to improve readability and reduce risk of floating-point parsing issues.

## References / Citations

- NREL EVI-Pro (EVERMI) vehicle database — reference for expected vehicle parameters: battery capacities, onboard charger ratings, and charging efficiencies by level
- SAE J1772 standard charging levels: Level 1 (120 V AC, 12-16 A, 1.44-1.92 kW), Level 2 (208-240 V AC, up to 80 A, up to 19.2 kW)
- Typical residential L2 deployment: NEMA 14-50 outlet with 32 A EVSE delivering 7.7 kW; hardwired 48 A delivering 11.5 kW
- EVI-Pro charging efficiencies: L1 ~85-88% (grid to battery), L2 ~90-93%
- fueleconomy.gov for vehicle specifications (battery capacities, efficiency ratings)
