# Water heating defaults: UEF values, tank volumes, UA values, schedules
**Review ID**: defdata-04
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
- `defaults/water_heating/WH Medium UEF Schedule.csv` (2881 lines, 2-day 1-minute schedule)
- `defaults/water_heating/default_paramters.csv` (5 lines, 4 parameter rows)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.hh` — UA units (W/K, line 494), tank volume (m³, line 483), setpoint schedules (lines 509–514), deadband (deltaC, line 515)
- `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc` — `GetSchedule` for setpoint (line 2330), which supports standard EnergyPlus day-type (Weekday/Weekend/Holiday) schedules
- `vendors/OCHRE/ochre/utils/hpxml.py` — UEF-to-UA conversion, EF-to-UEF conversion (lines 1013–1163), HPWH UA defaults (lines 1070–1075), setpoint default 125°F ≈ 51.67°C (line 1028)
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py` — default deadband 5.56°C (line 76), default capacity 4500 W (line 20)
- `vendors/OCHRE/bin/run_equipment.py` — example ERWH using `WH Medium UEF Schedule.csv` with tank volume 250 L, UA 2.17 W/K, setpoint 51°C (lines 220–249)
- `crates/hares-equipment/src/water_heater/wh_config.rs` — typed config structs documenting expected parameters: `tank_volume_m3`, `ua_w_per_k`, `setpoint_c`, `deadband_c`, `energy_factor`, `uniform_energy_factor`, etc.
- DOE 10 CFR Part 430, Subpart B, App. E (Uniform Energy Factor test procedure, effective 2015; updated 2023)
- ASHRAE Handbook — HVAC Applications, Ch. 51 (typical daily hot water draw: 240 L/day for family of four)

## Findings

### Finding 1: [Severity: medium] Schedule file misnamed — contains draw profile, not UEF values
**Description**: `WH Medium UEF Schedule.csv` is named as if it contains Uniform Energy Factor (UEF) efficiency values by fuel type and tank size category. In fact, it is a 1-minute water draw profile (L/min) with zone temperature and mains temperature columns. The file contains no UEF, EF, or efficiency data at all. DOE federal efficiency standards (e.g., gas storage UEF ≥ 0.64 for 50-gal, electric storage UEF ≥ 0.93 for 50-gal, HPWH UEF ≥ 3.30–3.70) cannot be verified against this file because no such values exist here.
**Code Location**: `defaults/water_heating/WH Medium UEF Schedule.csv:1` (header: `Time,Water Heating (L/min),Zone Temperature (C),Mains Temperature (C)`)
**Root Cause**: The file was likely imported from a legacy dataset where "UEF" in the filename referred to the test procedure draw pattern (the UEF rating test uses a standardized 24-hour draw profile), not to UEF efficiency values themselves. The name is misleading.
**Impact**: Users or developers looking for UEF efficiency defaults for water heaters will not find them in the expected location. The "Medium UEF" name does correctly identify this as a "medium" draw profile per the UEF test standard, but the distinction between "draw schedule used for UEF testing" and "UEF efficiency values" is not documented.

### Finding 2: [Severity: high] `default_paramters.csv` is critically incomplete — missing most water heater parameters
**Description**: The file contains only 4 parameter rows:
| Name | Value | Units |
|------|-------|-------|
| T_set | 49 | (empty) |
| T_db | 4 | (empty) |
| P_hw1 | 4.5 | (empty) |
| P_hw2 | 4.5 | (empty) |

The following parameters required by the HARES water heater typed configs (`wh_config.rs`) are entirely absent:
- **Tank volumes**: No volume entries for any tank size (30, 40, 50, 65, 80 gal / 114, 151, 189, 246, 303 L)
- **UA values**: No heat loss coefficients for any tank size or insulation level
- **Recovery efficiencies**: No `EnergyFactor`, `UniformEnergyFactor`, or `conversion_efficiency`
- **Tank geometry**: No `TankHeight`, `JacketRValue`
- **Heating capacity**: The file has element power (`P_hw1`/`P_hw2`) but no `HeatingCapacity`
- **Setpoint schedules**: No schedule file references or hourly schedules
- **Draw profiles**: No `avg_water_draw_l_per_day` or `draw_flow_rate_kg_s`

By contrast, the EnergyPlus `WaterThermalTanks` model tracks `Volume` (m³, line 483), `LossCoeff` = UA (W/K, line 494), `MaxCapacity` (W, line 504), `Efficiency` (line 507), and requires a setpoint schedule via `GetSchedule` (line 2330). OCHRE's `hpxml.py` computes UA from UEF/EF values (lines 1102–1125) and validates tank volumes (lines 1043–1050).

**Code Location**: `defaults/water_heating/default_paramters.csv:1-5`
**Root Cause**: The file appears to be a stub or placeholder that was never fully populated. HARES likely relies on HPXML-imported parameters rather than this defaults file, but the file's existence creates confusion about what defaults are available.
**Impact**: The file cannot serve as a standalone defaults source for water heating equipment. Any code that reads this file expecting tank volumes, UA, or UEF values will fail silently or fall through to hardcoded fallbacks. If HARES has no alternative source for water heater defaults (e.g., hardcoded fallbacks in Rust), simulations may use unvalidated/inconsistent values.

### Finding 3: [Severity: medium] Daily draw total is 208 L/day — 13% below ASHRAE family-of-four benchmark
**Description**: The schedule integrates to **208.20 L/day** (55.0 gal/day). The ASHRAE-referenced benchmark for a family of four is ~240 L/day (64 gal/day). This is 86.7% of the benchmark. While consistent with a "Medium" usage label (lighter than a "High" profile), the gap should be documented. The DOE UEF test procedure for the medium draw bin uses 55.0 gal/day per the UEF draw pattern bins defined in 10 CFR 430, Subpart B, App. E — which matches exactly (55 gal/day = 208.2 L/day). The schedule's "Medium UEF" name is therefore correct for the UEF test draw bin, but it may be one usage category below what typical residential modeling expects.
**Code Location**: `defaults/water_heating/WH Medium UEF Schedule.csv` (rows 2–2881, integrated)
**Root Cause**: The schedule is derived from the UEF test standard's "medium" draw pattern bin rather than from ASHRAE or Building America typical residential profiles.
**Impact**: Simulations using this schedule will under-predict water heating energy consumption by approximately 13% compared to ASHRAE-family-of-four assumptions. This is acceptable if modeling one- or two-person households, but misleading if used as the default for a median US household (2.5 persons, ~225–250 L/day).

### Finding 4: [Severity: high] Draw schedule has an atypical temporal pattern deviating from standard US residential profiles
**Description**: Standard US residential hot water draw profiles (per Hendron 2004, ASHRAE Handbook, Building America BAHSP) have a **morning peak at 6–9 AM** and an **evening peak at 5–9 PM**, with scattered mid-day draws and near-zero overnight draws. The `WH Medium UEF Schedule.csv` shows:

| Time block | Draw (L) | Description |
|------------|----------|-------------|
| 0:00–1:48 | 98.4 | **Midnight draw** (47% of daily total) — large sustained draws at typical shower flow rates |
| 10:33–11:35 | 53.0 | Mid-morning draw |
| 12:03–16:19 | 30.3 | Scattered afternoon draws |
| 16:48–17:07 | 26.5 | Late afternoon draw |

The largest draw event (47% of daily use) occurs at **midnight**, which does not match typical US occupancy-driven profiles where water use peaks during waking hours. The expected 6–9 AM morning peak is **completely absent** — there is zero draw between 4:00 AM and 10:30 AM.

This temporal profile matches the DOE UEF test procedure draw schedule (24-hour test), which is designed to stress-test water heater performance uniformly across 24 hours, not to represent typical occupancy-driven usage patterns. The UEF test standard deliberately distributes draws throughout 24 hours to measure standby losses under controlled conditions.

**Code Location**: `defaults/water_heating/WH Medium UEF Schedule.csv:2-1441` (Day 1 events at rows 2–111 with draws at 0:00–0:08, 0:30–0:31, 1:43–1:48, 10:33–10:38, 11:33–11:35, 12:03, 12:48, 12:53, 16:03, 16:18–16:19, 16:48–16:49, 17:03–17:07)
**Root Cause**: The schedule was created from the DOE UEF test procedure 24-hour draw pattern rather than from an occupancy-based residential hot water draw model (e.g., DHWEventGenerator).
**Impact**: Simulations using this schedule will misattribute water heating energy to overnight hours rather than morning/evening peaks. This can significantly affect demand-response analysis, time-of-use tariff calculations, and coordination with other loads (e.g., EV charging at night). The total daily energy may be reasonably accurate, but the load shape timing is unrealistic for residential applications.

### Finding 5: [Severity: low] Filename typo: "paramters" vs "parameters"
**Description**: The file `default_paramters.csv` has a spelling error — "paramters" is missing the "e" before the "t" in "parameters". This is noted in the review instructions.
**Code Location**: `defaults/water_heating/default_paramters.csv` (filename)
**Root Cause**: Typo at file creation.
**Impact**: Code that references the file by exact name will break if the filename is corrected. No code in the HARES Rust codebase directly references this file path (water heater defaults in `hares-io/src/defaults.rs` use TOML subdirectories, lines 846–882). The OCHRE `run_equipment.py` script uses `WH Medium UEF Schedule.csv` but does not reference `default_paramters.csv`. Low impact, but should be corrected for clarity.

### Finding 6: [Severity: medium] Missing units in `default_paramters.csv`
**Description**: The Units column is empty for all 4 data rows. The parameters are:
- `T_set` = 49 — likely °C (consistent with 49°C = 120.2°F, a typical tempering valve delivery temperature per ASHRAE 90.2)
- `T_db` = 4 — likely delta °C (= K), which is 7.2°F. OCHRE default deadband is 5.56°C (10°F). EnergyPlus default deadband is typically 2–5°C. 4°C is within range.
- `P_hw1` = 4.5 — likely kW (4.5 kW = 4500 W, standard residential electric water heater element). This matches OCHRE `default_capacity = 4500` (W).
- `P_hw2` = 4.5 — likely kW (same as above for upper element)

Without explicit units, downstream code must guess whether `P_hw1`/`P_hw2` are in W or kW, potentially causing 1000× errors.
**Code Location**: `defaults/water_heating/default_paramters.csv:2-5` (empty Units column)
**Root Cause**: Units column populated in header but left blank in data rows.
**Impact**: If any parser reads these values without pre-baking the knowledge that they are kW, results will be off by factor 1000. The HARES OCHRE-based equipment uses W internally (see `wh_config.rs` validation ranges), so the conversion from kW to W must be explicit.

### Finding 7: [Severity: low] HARES CSV schedule format does not differentiate weekdays/weekends
**Description**: EnergyPlus schedules inherently support day-type differentiation (Weekday, Weekend, Holiday, SummerDesignDay, etc.) via its `Schedule:Day:Interval` / `Schedule:Week:Daily` / `Schedule:Year` hierarchy. EnergyPlus water heater setpoint schedules use `Sched::GetSchedule` (WaterThermalTanks.cc:2330), which loads a full schedule object with day-type resolution. HARES uses a flat 2-day CSV timeline (`WH Medium UEF Schedule.csv`) with identical day-1 and day-2 draw totals (208.20 L/day each) and only a 3-minute time offset between day 1 and day 2 events (e.g., day 2's draw events are shifted +3 minutes from day 1's pattern). There is no documentation explaining whether the two days represent different day types (e.g., day 1 = weekday, day 2 = weekend) or simply provide two consecutive identical days for simulation startup.
**Code Location**: `defaults/water_heating/WH Medium UEF Schedule.csv:2-2881` (two nearly identical days with 3-min time offset)
**Root Cause**: HARES adopted OCHRE's schedule format, which uses CSV time series rather than EnergyPlus's hierarchical schedule object model.
**Impact**: Weekend water use patterns (typically shifted later in the morning, with different appliance usage patterns) are not captured. For annual simulations with weekday/weekend differentiation, water heating energy misestimation could be on the order of 2-5% of the water heating load (weekend vs. weekday delta × fraction of days that are weekends).

### Finding 8: [Severity: medium] Setpoint temperature of 49°C may be too low for storage tank safety
**Description**: `T_set = 49°C` (120.2°F). While ASHRAE 90.2 and plumbing codes typically recommend **120°F (48.9°C)** at the **fixture** (via tempering valve), storage tank setpoints are typically higher — **125–140°F (51.7–60.0°C)** — to prevent Legionella growth (bacteria stops multiplying at 122°F/50°C and is killed at 140°F/60°C) and to provide a buffer above the tempering valve setpoint. Values from reference implementations:
- EnergyPlus default: 60°C (140°F) — `WaterThermalTanks.hh:514`
- OCHRE default (HPXML parser): 125°F / 51.67°C — `hpxml.py:1028`
- OCHRE example script (`run_equipment.py:233`): 51°C
- CDC/ASHRAE guidance: storage ≥ 60°C (140°F) or ≥ 49°C (120°F) with anti-Legionella measures

49°C is at the absolute minimum of safe storage temperature and allows Legionella proliferation if tank stratification causes lower temperatures at the tank bottom.
**Code Location**: `defaults/water_heating/default_paramters.csv:2`
**Root Cause**: The setpoint may have been chosen as a tempering valve delivery temperature rather than a storage tank temperature.
**Impact**: Marginal Legionella risk if used as a tank setpoint. Energy predictions slightly underestimate tank standby losses (lower ΔT between tank and ambient). Tank capacity (usable hot water before recovery) is reduced because the temperature differential between stored hot water and cold mains is smaller.

## Summary
- Total findings: 8
- Critical: 0 / High: 2 / Medium: 4 / Low: 2

## Recommendations
1. **Rename `WH Medium UEF Schedule.csv`** to `WH Medium Draw Schedule.csv` or `WH UEF Medium Draw Profile.csv` to clearly indicate it contains a draw profile, not UEF efficiency values.
2. **Populate `default_paramters.csv`** (correct the filename spelling) with complete water heater defaults per water heater type: tank volumes for 30, 40, 50, 65, 80 gal sizes; UA values scaled by surface area (target: 1.0–2.0 W/K for typical residential tanks with R-10 to R-16 insulation); recovery efficiencies / UEF values; setpoint schedules; and average daily draw volumes.
3. **Add an occupancy-based draw schedule** (e.g., "WH Medium Residential" or "WH High Residential") with a morning peak (6–9 AM), evening peak (5–9 PM), and low overnight usage, integrating to ~240 L/day for a family of four per ASHRAE. Keep the DOE UEF test schedule but clearly label it as a test-standard profile, not a residential occupancy profile.
4. **Add weekday/weekend differentiation** to water heater schedules. At minimum, document that the current 2-day schedule does not differentiate day types. Ideally, support a weekly 7-day schedule similar to EnergyPlus `Schedule:Week:Daily`.
5. **Fill the Units column** in `default_paramters.csv` for all parameters. Add a header comment or metadata row describing the expected units and typical ranges.
6. **Increase the default storage setpoint** from 49°C to 51.67°C (125°F) to match OCHRE, or to 60°C (140°F) to match EnergyPlus, with a tempering valve delivery setpoint of 49°C (120°F) separately specified. The `fixture_delivery_temp_c` field in `wh_config.rs` (defaulting to 40.6°C per the comment) already supports this separation.
7. **Compute and document UA values per tank size** that verify internal consistency. For example, a 50-gal (0.189 m³) cylindrical tank with 1.2 m height has surface area ~2.3 m². With R-12 insulation (R = 2.11 K·m²/W), UA ≈ 2.3/2.11 = 1.09 W/K. A 80-gal tank (0.303 m³) with same height has surface area ~2.9 m² and UA ≈ 1.37 W/K. These are within the expected 1.0–2.0 W/K range and should scale consistently. Current file has zero UA values to verify.

## References / Citations
- DOE 10 CFR Part 430, Subpart B, Appendix E — Uniform Energy Factor Test Procedure (2015, updated 2023)
- ASHRAE Handbook — HVAC Applications, Chapter 51 (Service Water Heating), typical daily draw 240 L/day for family of four
- EnergyPlus Engineering Reference, Section 14.5 — Water Heater Model (`WaterThermalTanks.cc`)
- Maguire, J. and Roberts, D. (2020). "Converting Uniform Energy Factor to Energy Factor for Residential Water Heaters." ASHRAE Building Performance Conference. Available at line 1023 of `vendors/OCHRE/ochre/utils/hpxml.py`
- RESNET EF Calculator (2017). Available at line 1099 of `vendors/OCHRE/ochre/utils/hpxml.py`
- CDC (2023). "Legionella (Legionnaires' Disease and Pontiac Fever): Water Management Programs." Recommends storage temperature ≥ 60°C (140°F)
