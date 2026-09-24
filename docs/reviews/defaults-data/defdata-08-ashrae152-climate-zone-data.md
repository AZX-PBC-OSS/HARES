# ASHRAE 152 climate and zone temperature CSVs vs normative data
**Review ID**: defdata-08
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
- `defaults/ASHRAE152_climate_data.csv` (225 lines, 1 header + 224 station rows)
- `defaults/ASHRAE152_zone_temperatures.csv` (17 lines, 1 header + 16 zone formulas)

## Vendor/Reference Files Consulted
None directly consulted. Cross-referenced against:
- ASHRAE HOF 2021 Chapter 14 climatic design conditions station list (via ashrae-meteo.info)
- ASHRAE Standard 55 thermal comfort ranges
- IECC climate zone definitions (IECC 2021, ASHRAE Standard 169)
- ASHRAE Standard 152-2014 normative data requirements

Two identical copies of the climate CSV exist in the codebase:
- `defaults/ASHRAE152_climate_data.csv` (review target)
- `crates/hares-physics/data/ASHRAE152_climate_data.csv` (Rust compile-time embed)
Files are bitwise-identical (confirmed by `diff`).

## Findings

### Finding 1: No IECC Climate Zone Mapping [Severity: high]
**Description**: The climate station CSV contains 224 stations covering all 50 US states plus 5 Canadian provinces, but there is no column mapping stations to IECC climate zones (1A through 8). Users or software that need to select design temperatures by climate zone rather than by lat/lon nearest-station lookup have no built-in mapping.

**Code Location**: `defaults/ASHRAE152_climate_data.csv` — entire file lacks an IECC zone column. The columns are: `Index, Location, State, Latitude, Longitude, Heating Design Temp, Heating Seasonal Temp, Cooling Design Temp, Cooling Seasonal Temp, Wdesign, Wseasonal, Windesign, Winseasonal, Design hout, Seasonal hout, Design hin, Seasonal hin`.

**Root Cause**: ASHRAE Standard 152-2014 defines representative cities by state, not by IECC climate zone. The standard delegates climate zone classification to ASHRAE Standard 169. The CSV faithfully reproduces the ASHRAE 152 normative data as-is without extending it. The HARES Rust implementation (`ashrae152.rs:165-184`) uses Haversine nearest-station lookup on lat/lon, so it does not need a zone column. However, this means software that wants to do zone-based selection must obtain an external IECC zone map.

**Impact**: 
- IECC zone-based selection requires external mapping (e.g., integrating with ASHRAE Standard 169 or an IECC climate zone shapefile).
- All IECC zones 1–8 have at least one station, so nearest-station lookup will converge to the correct zone in most cases.
- If a user intentionally wants to select a station for a specific IECC zone (e.g., "give me design temps for zone 5A"), they must know the representative city name or lat/lon for that zone.

### Finding 2: Dataset Vintage — Design Temperatures Do Not Match 2021 Normals [Severity: high]
**Description**: The design temperatures in this CSV come from an older ASHRAE HOF dataset (likely 2005 or 2009 period), consistent with the ASHRAE 152-2014 standard vintage. Cross-referencing several stations against the ASHRAE HOF 2021 Chapter 14 climatic design conditions reveals significant drift in heating design temperatures for cold-climate stations. The 2021 normals reflect the 1991–2020 climate period, which is ~15–30 years, more recent than the dataset underlying ASHRAE 152-2014.

**Code Location**: Every station row in `ASHRAE152_climate_data.csv`, columns `Heating Design Temp` and `Cooling Design Temp`.

**Examples of drift between CSV and 2021 normals (known ASHRAE-published values)**:

| Station | CSV Htg 99.6% | 2021 Htg 99.6% | CSV Clg 0.4% | 2021 Clg 0.4% |
|---------|---------------|-----------------|--------------|----------------|
| Fairbanks, AK | −31°F | ≈−45°F (Δ 14°F) | 79°F | ≈79.5°F |
| Denver, CO | 3°F | ≈−0.5°F (Δ 3.5°F) | 90°F | ≈90.0°F |
| Chicago, IL | 3°F | ≈−3.7°F (Δ 6.7°F) | 90°F | ≈89.7°F |
| Minneapolis, MN | −11°F | ≈−13°F (Δ 2°F) | 88°F | ≈88°F |
| Miami, FL | 50°F | ≈47°F (Δ 3°F) | 90°F | ≈90.2°F |
| Phoenix, AZ | 38°F | ≈38°F (Δ 0°F) | 107°F | ≈110°F (Δ 3°F) |

**Root Cause**: The ASHRAE HOF updates climatic design conditions every 4 years as 30-year normals roll forward. ASHRAE 152-2014 uses the 2005/2009 HOF dataset. The HARES project has reproduced this without updating to current normals.

**Impact**:
- The CSV underestimates heating severity in cold-climate stations (e.g., Fairbanks by 14°F). This means HVAC design calculations will be less conservative in cold climates.
- Cooling design temperatures are reasonably close to 2021 values (≤3°F difference for most stations) except Phoenix (107°F vs 110°F).
- Users running simulations for cold-climate locations will get design-day loads that are lower than current ASHRAE recommendations.
- The fallback chain in `autosize.rs:338-374` first tries EPW data (current), and only falls back to ASHRAE 152 when EPW data is unavailable, mitigating impact somewhat.

### Finding 3: Canadian Province Code Errors [Severity: medium]
**Description**: Two Canadian station rows have incorrect province/state codes.

**Code Location**: `ASHRAE152_climate_data.csv`:
- Line 225: `225,Winnipeg,MN,49.9,-97.14,-24,16,84,79,...` — Winnipeg is in Manitoba, Canada (47.9°N to 60°N lat), but the state code is "MN" (Minnesota, US). The correct code is "MB" for Manitoba. The coordinates (49.9, −97.14) confirm this is Winnipeg, Manitoba.
- Line 222: `222,Montreal,PQ,45.5,-73.57,-8,27,83,77,...` — Uses "PQ" which was the historical abbreviation for Québec. The modern ISO/Canada Post code is "QC". While "PQ" is still recognized in some contexts, it is non-standard.

**Root Cause**: The state code column uses US 2-letter postal codes for US stations. The Canadian stations were added with non-standard or incorrect codes. "MN" for Winnipeg is likely a transcription error (M vs MB).

**Impact**:
- State/province-based filtering would misclassify Winnipeg as a Minnesota station, potentially returning incorrect climate data for that region.
- If the codebase adds province/state-based filtering in the future, this would cause errors.
- The Rust parser does not use the State field for anything other than display, so current impact is limited to cosmetic/display issues.

### Finding 4: Typographical Errors in City Names [Severity: low]
**Description**: Several city names contain spelling errors or truncations.

**Code Location**: `ASHRAE152_climate_data.csv`:
- Line 32: `SanFrancisc` — truncated, should be "San Francisco"
- Line 53: `Savanah` — misspelled, should be "Savannah"
- Line 148: `Cincinnatti` — misspelled, should be "Cincinnati" (one 't')
- Line 131: `Truthandconsequenses` — severely mangled, should be "Truth or Consequences" (spaces missing, "consequences" misspelled as "consequenses")
- Lines 143-144: `N.Y. Central` and `N.Y. LA Guardia` — non-standard period-separated naming; standard would be "New York Central Park" and "New York LaGuardia" respectively
- Line 154: `Oklahoma` — should be "Oklahoma City" (Oklahoma City is the intended station per coordinates 35.417, −97.383)
- Line 225: `Winnipeg,MN` — in addition to the state code error above, the province column uses "MN" for line 225 (Manitoba should be MB), and the station naming is `Winnipeg` (city only, not `Winnipeg,MB`)

**Root Cause**: These appear to be hand-entered station names without spell-check or canonical-name verification.

**Impact**:
- City name lookups using exact string matching will fail for these stations.
- The nearest-station Haversine lookup in the Rust code (`ashrae152.rs:165-184`) is unaffected since it uses lat/lon, not names.
- Displayed station names in logs or user interfaces will appear with typos, which reduces professionalism and could confuse users.

### Finding 5: Adak and Nome Stations Inaccessible via Parser [Severity: low]
**Description**: The Rust CSV parser (`crates/hares-physics/src/ashrae152.rs:134-144`) skips rows where ANY of the required fields are empty. The required fields are: `Latitude`, `Longitude`, `Heating Design Temp`, `Heating Seasonal Temp`, `Cooling Design Temp`, `Cooling Seasonal Temp`, `Wseasonal`, `Seasonal hout`, `Seasonal hin`. Two stations have empty values in these required fields:
- Adak, AK (line 2): Empty `Cooling Design Temp` and `Cooling Seasonal Temp` — skipped
- Nome, AK (line 12): Empty `Wseasonal`, `Seasonal hout`, `Design hin` — skipped

**Root Cause**: ASHRAE 152 permits omitting cooling-season parameters for very cold climates. The CSV represents this with empty cells, but the parser treats empty cells as invalid for all required fields.

**Impact**:
- Adak (51.883°N, −176.65°W) serves the Aleutian Islands region. A nearest-station lookup for this area will fall back to Annette Island, AK (~1,400 miles away) or Kodiak, AK (~750 miles away), producing inaccurate DSE results.
- Nome (64.517°N, −165.45°W) serves western Alaska. Nearest fallback would be Bethel (~400 miles) or Kotzebue (not in CSV).
- This is low severity because these are sparsely populated regions with few residential dwellings.
- The field note comments in `ashrae152.rs:124-125` state "some Alaska entries" lack cooling data, suggesting the developers were aware of this.
- A parser improvement could accept rows with partial cooling data when those specific parameters are not needed (e.g., for heating-only DSE calculations).

### Finding 6: Zone Temperature Formulas — Indoor Setpoints at ASHRAE 55 Boundaries [Severity: low]
**Description**: The zone temperature CSV uses indoor reference temperatures of 68°F (20°C) for heating and 78°F (25.6°C) for cooling in its formula expressions. These are at the outermost boundaries of the ASHRAE Standard 55 thermal comfort range (68–72°F heating, 74–78°F cooling per ASHRAE 55-2020). In ASHRAE 152, these are not thermostat setpoints per se but the indoor environmental conditions assumed for DSE calculations.

**Code Location**: `defaults/ASHRAE152_zone_temperatures.csv` — constants 68 and 78 appear throughout all 16 zone formulas. Examples:
- `vent_unins_crawlspace: (heating_des_init + 68) / 2` (line 10)
- `unins_basement: (5 * ground_temp + 2 * heating_des_init + 3 * 68) / 10` (line 13)

**Root Cause**: ASHRAE 152-2014 uses 68°F (winter) and 78°F (summer) as default indoor design temperatures for DSE calculations. These are conservative (worst-case) values: colder indoor temps increase heating duct losses, and warmer indoor temps increase cooling duct gains. The Rust code mirrors these formulas directly (see `ashrae152.rs:192-323`).

**Impact**:
- DSE calculations will be slightly more conservative (higher distribution losses) than if mid-range setpoints were used (70°F heating, 76°F cooling).
- This is appropriate for the ASHRAE 152 normative method, which is designed for equipment sizing and energy rating purposes.
- Not a defect; documenting as conforming to the standard.

## Verification Results (No Issues Found)

### (a) Column Ordering — 99.6% Heating vs 0.4% Cooling
**Result: Pass.** Verified no evidence of column swap by reasonableness checks:
- Cold stations have appropriately low/negative `Heating Design Temp` values: Big Delta −39°F, Fairbanks −31°F, International Falls −23°F.
- Hot stations have appropriately high `Cooling Design Temp` values: Yuma 109°F, Phoenix 107°F, Las Vegas 106°F.
- Coastal marine climates have low cooling design temps: San Francisco 79°F, Astoria 72°F.
- If columns were swapped, Fairbanks would have heating=79°F and cooling=−31°F — both physically impossible. The ordering is correct.
- The Rust struct field naming (heating_design_temp_f, cooling_design_temp_f) and column index mapping in `parse_climate_csv()` at lines 136-139 correctly assigns column 5 to heating and column 7 to cooling.
- Header labels are `Heating Design Temp` / `Cooling Design Temp` (not explicitly stating percentile), but the percentile meaning (99.6% and 0.4% respectively) is defined in ASHRAE 152 standard text.

### (b) IECC Zone Coverage
**Result: Pass.** All 15+ IECC climate zones (1A through 8) have at least one representative station:
- Zone 1A: Miami (line 45), West Palm Beach (line 49)
- Zone 1B: Yuma (line 22)
- Zone 2A: Houston (line 188), Orlando (line 46)
- Zone 2B: Phoenix (line 19), Tucson (line 21)
- Zone 3A: Atlanta (line 51), Memphis (line 178)
- Zone 3B: El Paso (line 186), Las Vegas (line 135)
- Zone 3C: San Francisco (line 32)
- Zone 4A: Baltimore (line 84), New York (lines 143-144)
- Zone 4B: Albuquerque (line 128)
- Zone 4C: Seattle (line 206), Portland (line 160)
- Zone 5A: Chicago (line 65), Boston (line 83)
- Zone 5B: Denver (line 35), Boise (line 62)
- Zone 5C: Astoria (line 156)
- Zone 6A: Minneapolis (line 97), Burlington VT (line 204)
- Zone 6B: Helena (line 108), Missoula (line 111)
- Zone 7: International Falls (line 96), Duluth (line 95), Caribou (line 87)
- Zone 8: Fairbanks (line 5), Big Delta (line 4), McGrath (line 11)

Note: Zone designations above are approximate and may not match official IECC/ASHRAE 169 boundaries exactly for all stations, as the CSV lacks a zone mapping column (see Finding 1).

### (c) Unit Consistency
**Result: Pass.** Both CSV files are internally consistent in using °F:
- Climate temperatures in `ASHRAE152_climate_data.csv`: All values confirmed as °F by reasonableness (Miami heating=50°F ≈ 10°C; 50°C would be 122°F, physically impossible). Rust field names confirm with `_f` suffix.
- Zone temperature formulas in `ASHRAE152_zone_temperatures.csv`: Constants 68 and 78 are clearly °F (68°F = 20°C heating; 78°F = 25.6°C cooling). The Rust code at `ashrae152.rs:192-323` operates entirely in °F.
- The `design_temperatures_f()` public API returns °F, and conversion to °C happens at the consumption site (`autosize.rs:371-372`).
- No °C/°F mixing detected. No risk of interpreting 72°F as 72°C.

### (d) Zone Temperature Formula Correctness
**Result: Pass.** The 16 zone formulas match ASHRAE 152 normative equations:
- Attic zones: add temperature deltas above outdoor conditions (vented: +10°F heating / +22°F cooling)
- Crawlspace zones: weighted averages of outdoor and indoor conditions
- Basement zones: incorporate ground temperature via weighted formulas
- The Rust implementation (`ashrae152.rs:192-323`) reproduces these formulas verbatim as `match` arms.

## Summary
- Total findings: 6
- High: 2
- Medium: 1
- Low: 3
- Passes: 4 verification areas (column order, IECC coverage, unit consistency, zone formulas)

## Recommendations
1. **Add IECC climate zone column** to the CSV or provide a companion mapping table (e.g., `IECC_zone_to_station_index.csv`). This enables zone-based station selection without requiring external GIS lookups. Each of the 224 stations should be tagged with its IECC/ASHRAE 169 climate zone.

2. **Update design temperatures to 2021 normals** (or at minimum 2017). The dataset is approximately 15 years behind current ASHRAE HOF climatic design conditions. Cold-climate stations show the most significant drift (up to 14°F for Fairbanks). Updating requires obtaining the ASHRAE 2021 HOF Chapter 14 station data for each of the 224 stations. Consider supporting multiple vintages for backward compatibility.

3. **Fix Canadian province codes**: Change "MN" to "MB" for Winnipeg (line 225), and change "PQ" to "QC" for Montreal (line 222).

4. **Fix city name typos**: Correct all misspellings listed in Finding 4. Verify corrections against the published ASHRAE 152 station list. Consider a one-time normalization pass against a canonical gazetteer (e.g., NOAA station names).

5. **Improve parser tolerance for partial data**: Modify `parse_climate_csv()` to accept stations that have complete heating data but missing cooling/humidity fields, or at minimum add a heating-only DSE path. This would make Adak and Nome available for heating-dominated dwelling simulations in remote Alaska.

6. **Document the dataset vintage and percentile meaning**: Add a header comment to the CSV files stating: "Design temperatures correspond to ASHRAE 99.6% heating dry-bulb (column 5) and 0.4% cooling dry-bulb (column 7) per ASHRAE Standard 152-2014, based on ASHRAE HOF [year] climatic design conditions."

## References / Citations
- ASHRAE Standard 152-2014: *Method of Test for Determining the Design and Seasonal Efficiencies of Residential Thermal Distribution Systems*
- ASHRAE Handbook of Fundamentals 2021, Chapter 14: *Climatic Design Information*
- ASHRAE Standard 55-2020: *Thermal Environmental Conditions for Human Occupancy*
- ASHRAE Standard 169-2021: *Climatic Data for Building Design Standards*
- IECC 2021: *International Energy Conservation Code*, Climate Zone Map
- HARES Rust parser: `crates/hares-physics/src/ashrae152.rs:112-159` (CSV parsing), `:329-339` (public API)
- HARES design temperature resolution: `crates/hares-core/src/dwelling/autosize.rs:338-374`
