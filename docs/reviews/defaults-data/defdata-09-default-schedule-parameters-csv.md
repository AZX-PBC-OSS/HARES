# Default Schedule Parameters.csv: schedule types, seasonal multipliers, patterns
**Review ID**: defdata-09
**Category**: defaults-data
**Date**: 2026-05-25

## Files Reviewed
- `defaults/Default Schedule Parameters.csv` (88 lines, 30 distinct schedule types)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py` (654 lines) — OCHRE schedule loading/parsing logic, including `SCHEDULE_NAMES` mapping, `create_simple_schedule()` factory, and `import_occupancy_schedule()` which reads `Default Schedule Parameters.csv`.

## Findings

### Finding 1: [Severity: critical]
**Description**: No default HVAC thermostat heating or cooling setpoint schedules are defined in the CSV. The OCHRE code at `schedule.py:408-426` handles "Setpoint" category entries. If neither an HPXML schedule file column nor equipment-level "Weekday Setpoints (C)" or "Setpoint Temperature (C)" properties are present, the code raises `OCHREException(f"Must specify {ochre_name} setpoints...")`. Since the CSV provides no default `heating_setpoint` or `cooling_setpoint` rows, any simulation that lacks a user-provided schedule file or HPXML-defined setpoints will panic at startup.

**Code Location**:
- `defaults/Default Schedule Parameters.csv`: Entire file — no `heating_setpoint` or `cooling_setpoint` Schedule Name entries exist.
- `vendors/OCHRE/ochre/utils/schedule.py:55-56` — `SCHEDULE_NAMES["Setpoint"]` defines `heating_setpoint` and `cooling_setpoint`.
- `vendors/OCHRE/ochre/utils/schedule.py:408-426` — `import_occupancy_schedule()` requires setpoints or raises.

**Root Cause**: The CSV was ported from the NREL OpenStudio-HPXML default schedules file (cited at `schedule.py:395`), which itself does not include thermostat schedules (those are generated separately in OpenStudio-HPXML). However, OCHRE's architecture combines all schedule types into a single CSV, requiring setpoint schedules here as well.

**Impact**: Non-recoverable simulation failure for any dwelling that does not explicitly provide thermostat setpoints via HPXML properties or an external schedule file. This prevents the default schedule CSV from serving as a standalone fallback.

### Finding 2: [Severity: high]
**Description**: All 30 schedule types define identical weekday and weekend fraction patterns — every `WeekdayScheduleFractions` row has exactly the same 24 values as its corresponding `WeekendScheduleFractions` row. This violates the ASHRAE 90.2/HERS reference home assumption of distinct weekday/weekend occupancy. The `create_simple_schedule()` function at `schedule.py:282-304` fully supports separate weekday/weekend fractions (the `weekend` boolean key is used to join against `df.index.weekday < 5` at `schedule.py:440`), but the data never exercises that capability.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:2-3` — `occupants` weekday/weekend fractions identical.
- `defaults/Default Schedule Parameters.csv:8-9` — `lighting_interior` weekday/weekend identical.
- `defaults/Default Schedule Parameters.csv:79-80` — `hot_water_fixtures` weekday/weekend identical.
- (All other schedule types follow this pattern.)
- `vendors/OCHRE/ochre/utils/schedule.py:284-285` — `create_simple_schedule()` accepts separate params.
- `vendors/OCHRE/ochre/utils/schedule.py:440` — join uses `df.index.weekday < 5` to select weekday vs. weekend.

**Root Cause**: The source ANSI/RESNET/ICC 301-2022 Addendum C tables provide single schedules (not split by day type). The data was imported verbatim without deriving weekend-specific fractions.

**Impact**: Simulations using only the default CSV produce no behavioral difference between weekdays and weekends. Expected ASHRAE 90.2/HERS patterns — weekday occupancy low during 9 AM–5 PM with evening peaks, weekend occupancy high all day — are not represented. This reduces energy prediction accuracy for utility rate analysis (TOU rates often distinguish weekday/weekend) and for demand-response studies.

### Finding 3: [Severity: medium]
**Description**: The `permanent_spa_heater` monthly multiplier values (line 78) are byte-for-byte identical to the `refrigerator` monthly multipliers (line 24): `"0.837, 0.835, 1.084, 1.084, 1.084, 1.096, 1.096, 1.096, 1.096, 0.931, 0.925, 0.837"`. However, the `permanent_spa_pump` (line 75, same equipment system) uses a different summer-peaked pattern: `"0.921, 0.928, 0.921, 0.915, 0.921, 1.160, 1.158, 1.158, 1.160, 0.921, 0.915, 0.921"`. Spa pumps and spa heaters should logically share the same seasonal profile since they operate together. The heater’s multipliers appear to be a copy-paste of the refrigerator pattern rather than the intended spa heater pattern.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:24` — refrigerator multipliers.
- `defaults/Default Schedule Parameters.csv:75` — permanent_spa_pump multipliers (summer peak, different pattern).
- `defaults/Default Schedule Parameters.csv:78` — permanent_spa_heater multipliers (matches refrigerator, not pump).

**Root Cause**: Likely copy-paste error during CSV construction. The refrigerator multipliers were reused instead of generating (or using) a spa-appropriate seasonal profile.

**Impact**: The spa heater’s seasonal energy profile is distorted — it shows a summer peak (from the refrigerator compressor pattern) rather than following the spa pump’s usage pattern. Total spa energy consumption remains correct (monthly multipliers are normalized factors that redistribute annual total), but seasonal timing of energy use is wrong, which affects peak-demand studies and utility bill calculations.

### Finding 4: [Severity: medium]
**Description**: Pool pump and pool heater monthly multipliers (lines 69, 72) show highest values in winter months (Jan=1.161, Dec=1.154) and lowest in summer (Jul=0.883, Aug=0.883). This winter-peaked seasonal pattern appears inverted compared to typical residential pool operation, where swimming pool pumps run predominantly during summer months. While freeze-protection cycling in some climates could cause winter pump use, the HERS rating reference home assumption would typically place pool operation in the swimming season.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:69` — pool_pump MonthlyScheduleMultipliers.
- `defaults/Default Schedule Parameters.csv:72` — pool_heater MonthlyScheduleMultipliers.
- Data source: Figure 24 of the 2010 BAHSP.

**Root Cause**: The multipliers are cited from Figure 24 of the 2010 Building America House Simulation Protocols. The source may model pool pumps for freeze protection or may contain a different operational assumption. Verification against the original source is recommended.

**Impact**: Pool pump/heater energy is concentrated in winter rather than summer, which inverts the seasonal load shape. This could affect summer peak demand analysis and DER sizing studies where pool loads should contribute to summer peaks. Since monthly multipliers only redistribute the annual total, annual energy remains unaffected.

### Finding 5: [Severity: medium]
**Description**: The `cooking_range`, `clothes_washer`, `dishwasher`, and `clothes_dryer` schedules all have monthly multipliers of 1.0 (no seasonal variation). While some appliances may genuinely have flat seasonal profiles, clothes dryers typically see reduced use in summer (line-drying) and dishwashers and cooking ranges could have modest winter increases. The presence of a seasonal profile for lighting (winter = 1.20, summer = 0.80) but complete absence for major appliance categories suggests the seasonal multipliers were not applied to all schedule types with seasonal dependencies.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:21` — cooking_range multipliers all 1.0.
- `defaults/Default Schedule Parameters.csv:37` — dishwasher multipliers all 1.0.
- `defaults/Default Schedule Parameters.csv:40` — clothes_washer multipliers all 1.0.
- `defaults/Default Schedule Parameters.csv:43` — clothes_dryer multipliers all 1.0.

**Root Cause**: The source ANSI/RESNET/ICC 301-2022 Addendum C Table C.3(1) provides single normalized appliance profiles without seasonal decomposition. The BAHSP source for some other schedules includes monthly variation; appliances from the ANSI tables do not.

**Impact**: Appliance loads show no seasonal variation. Clothes dryers in particular produce identical energy in summer and winter, which may not reflect real-world line-drying behavior. Peak demand studies (especially winter peak for dryers) may be slightly over-estimated in non-winter months.

### Finding 6: [Severity: low]
**Description**: Multiple rows use the literal string `"N/A"` in the "OCHRE Name" and "OCHRE Element" columns instead of leaving them empty/pandas-NA. The OCHRE code at `schedule.py:397` filters with `df_default["OCHRE Name"].notna()`, which does NOT catch the string `"N/A"` (it is a non-null string). If these rows were ever processed via the current pivot logic, multiple rows with OCHRE Name=`"N/A"` and OCHRE Element=`"N/A"` would collide, causing `ValueError: Index contains duplicate entries, cannot reshape`. Currently safe because these entries (`hot_water_recirculation_pump_*`, `lighting_exterior_holiday`) are not referenced in `SCHEDULE_NAMES` and are never looked up. However, this is fragile and would break if the mapping were extended.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:17-18` — lighting_exterior_holiday: OCHRE Name="N/A", OCHRE Element="N/A".
- `defaults/Default Schedule Parameters.csv:82-88` — hot_water_recirculation_pump_*: OCHRE Name="N/A", OCHRE Element="N/A".
- `vendors/OCHRE/ochre/utils/schedule.py:397-398` — filter and pivot.

**Root Cause**: Placeholder convention — `"N/A"` used to mean "not applicable" in the OCHRE namespace, but OCHRE's `notna()` filter treats it as valid data.

**Impact**: No current runtime impact (these schedules are in the "Ignore" category or unlisted). Could cause a crash if future code adds these to `SCHEDULE_NAMES` without also fixing the CSV.

### Finding 7: [Severity: low]
**Description**: Line 4 (`occupants,MonthlyScheduleMultipliers`) has a trailing comma after the final value in the Values column: `"1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,"`. This creates an extra empty string element when `eval()` is applied. While `eval("1.0, 1.0, ")` technically evaluates to a tuple of `(1.0, 1.0)` in Python (trailing comma is valid in tuple literals since it's actually `eval("1.0, 1.0,")` which truncates the trailing comma... wait, let me reconsider.

Actually, `eval("1.0, 1.0,")` — in Python, a single-element trailing comma creates a tuple literal: `(1.0,)` and a multi-element list with trailing comma is fine: `(1.0, 1.0,)`. So `eval("1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,")` evaluates to a 12-element tuple which is correct. However, the trailing comma is non-standard CSV formatting and could confuse other parsers.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:4` — trailing comma in Values column.
- `vendors/OCHRE/ochre/utils/schedule.py:433` — `eval(val)` parses the string.

**Impact**: No current impact — Python's `eval()` handles trailing commas. Non-Python parsers (or external tools consuming this CSV) may produce a 13th empty element.

### Finding 8: [Severity: low]
**Description**: Several schedule rows have empty or missing Data Source annotations:
- `plug_loads_vehicle` (lines 52-54): Data Source entirely blank.
- `lighting_exterior_holiday` (lines 17-18): Data Source blank.
- `hot_water_recirculation_pump_no_control` (lines 82-83): Data Source blank.
- `hot_water_recirculation_pump_demand_control` (lines 84-85): Data Source blank.
- `hot_water_recirculation_pump_temperature_control` (lines 86-87): Data Source blank.
- `hot_water_recirculation_pump` (line 88): Data Source blank.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:17-18, 52-54, 82-88` — empty Data Source fields.
- (By contrast, most other rows properly cite ANSI/RESNET/ICC 301-2022 or 2010 BAHSP.)

**Root Cause**: Unattributed data may have been derived from the same source documents (e.g., Equation 4.2-43a of ANSI/RESNET/ICC 301-2022 is cited on line 83 but not uniformly across all recirculation entries), or may be internally derived values without a publication reference.

**Impact**: Traceability gap — reviewers and auditors cannot verify the provenance of these schedule values. Low practical impact since the fraction values are simple.

## Summary
- **Total findings**: 8
- **Critical**: 1 (Missing HVAC thermostat schedules)
- **High**: 1 (Identical weekday/weekend patterns)
- **Medium**: 3 (Spa heater copy-paste, pool seasonal inversion, missing appliance seasonality)
- **Low**: 3 (CSV "N/A" values, trailing comma, missing data sources)

## Recommendations
1. **Add default HVAC thermostat setpoint schedules** (heating and cooling) to the CSV, either as constant 24-value hourly fractions (e.g., 20°C heating, 24°C cooling for HERS reference) or as separate Setpoint category entries with seasonal variation if desired. Without this, the default schedule CSV cannot serve as a standalone fallback.
2. **Derive distinct weekend schedule fractions** for occupancy-driven schedules at minimum (occupants, lighting, hot water, appliances). Use the ASHRAE 90.2/HERS reference pattern: weekday occupancy with 9 AM–5 PM gap, weekend occupancy high all day.
3. **Verify and correct permanent_spa_heater monthly multipliers** — they should match the spa_pump seasonal pattern, not the refrigerator pattern.
4. **Audit pool pump/heater monthly multipliers** against the original BAHSP Figure 24 source to confirm the winter-peaked profile is intentional (freeze protection) rather than inverted.
5. **Fix CSV data hygiene**: replace literal `"N/A"` strings with empty/null values in the OCHRE Name/Element columns (use blank cells or actual NaN), remove trailing commas from Values, and backfill missing Data Source citations.
6. Consider adding seasonal multipliers for major appliances (particularly clothes dryer for line-drying in summer, and cooking range for holiday cooking peaks) if supported by the source standards.

## References / Citations
- ANSI/RESNET/ICC 301-2022 Addendum C Tables C.3(1)–C.3(5) — primary source for most schedule fractions.
- 2010 Building America House Simulation Protocols (BAHSP), Figures 16, 23, 24 — source for refrigerator, freezer, pool, spa, well pump, gas grill, and fireplace schedules.
- NREL OpenStudio-HPXML `default_schedules.csv` — upstream reference for the CSV structure.
- OCHRE `vendors/OCHRE/ochre/utils/schedule.py` — reference implementation for schedule parsing, `create_simple_schedule()` at line 282, `import_occupancy_schedule()` at line 372.
- ASHRAE 90.2 / HERS Reference Home — occupancy assumption: 5 PM–8 AM weekdays, all day weekends.
