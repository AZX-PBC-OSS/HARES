# Missing .ddy and .stat weather file format support
**Review ID**: weather-07
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/epw.rs` — EPW parser, design-conditions extraction
- `crates/hares-io/src/weather.rs` — Weather format detection, `WeatherTimeSeries`, resampling
- `crates/hares-core/src/dwelling/autosize.rs` — HVAC capacity autosizing from design conditions

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` — Design day setup (line 3485), design-day data retrieval (line 5922), STAT-based monthly dry-bulb for water mains (line 8660)
- `vendors/EnergyPlus/weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.ddy` — Representative DDY file with multiple `SizingPeriod:DesignDay` objects
- `vendors/EnergyPlus/weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.stat` — Representative STAT file with heating/cooling/extremes stats and monthly temperature summaries
- `vendors/OCHRE/ochre/defaults/ASHRAE152_climate_data.csv` — OCHRE's ASHRAE 152 station database
- `crates/hares-physics/data/ASHRAE152_climate_data.csv` — HARES's ASHRAE 152 station database (224 stations, same schema)

## Findings

### Finding 1: No support for standalone DDY or STAT file formats [Severity: medium]
**Description**: HARES auto-detects and parses EPW, PSM3, TMY3, and ResStock CSV weather files, but does not recognize `.ddy` (Design Day) or `.stat` (Annual Statistics) EnergyPlus auxiliary weather files. The `WeatherFormat` enum (`weather.rs:16-30`) lists only `Epw`, `Psm3`, `ResStockCsv`, and `Tmy3`. The `detect_weather_format` function (`weather.rs:43-65`) maps `.epw` to `WeatherFormat::Epw` and `.csv` to a header-sniffing path; any other extension (including `.ddy` and `.stat`) produces an `UnsupportedWeatherFileExtension` error.

**Code Location**: `crates/hares-io/src/weather.rs:16-30`, `weather.rs:43-65`

**Root Cause**: The HARES weather subsystem was built around the primary EPW time-series file. The DDY and STAT companion files were not included in the initial format support scope. The codebase is aware of their existence — `epw.rs:879-881` explicitly documents that "The authoritative design-day data lives in separate DDY (Design Day) files. The 'Extremes' summary here provides a fallback design temperature when ASHRAE 152 climate station data is unavailable." — but no parser was implemented.

**Impact**:
- **No ability to use DDY-derived design conditions for HVAC sizing.** DDY files provide 6–12 distinct design periods per location covering multiple percentiles (99.6% and 99.0% heating dry-bulb, humidification dewpoint, wind-driven infiltration; 0.4%, 1.0%, and 2.0% cooling dry-bulb, wet-bulb, dewpoint, and enthalpy conditions). The EPW header "Extremes" section provides only 2 values (min of extreme lows, max of extreme highs), losing the humidity, wind, and pressure context that accompanies each design condition in a DDY file.
- **No ability to use STAT-derived monthly statistics.** EnergyPlus reads STAT files to extract monthly daily-average dry-bulb temperatures for water mains temperature autosizing (`WeatherManager.cc:8660-8729`). HARES currently has no water mains temperature autosizing, so this gap is latent but will become relevant when water heater sizing is added.
- **Nuisance error when users try to point HARES at a DDY or STAT file instead of (or alongside) the EPW.** The error message lists only `.epw` and `.csv` as supported extensions and does not guide users toward the EPW file.

### Finding 2: EPW header design conditions lack humidity, wind, and pressure context used in Manual J sizing [Severity: medium]
**Description**: The `DesignConditions` struct (`epw.rs:48-57`) captures only `heating_design_db_c` and `cooling_design_db_c` from the EPW "Extremes" section. The `parse_design_conditions` function (`epw.rs:883-935`) extracts these by finding the "Extremes" token, collecting alternating low/high numeric values, and taking the min of lows and max of highs. In contrast, a DDY file for the same location (e.g., Denver-Aurora-Buckley TMY3) provides 16 distinct `SizingPeriod:DesignDay` objects, each carrying maximum dry-bulb, humidity condition type (wetbulb/dewpoint), humidity value at max dry-bulb, barometric pressure, wind speed, wind direction, clear-sky optical depth (taub/taud), and a daily temperature range profile.

**Code Location**: `crates/hares-io/src/epw.rs:48-57`, `epw.rs:883-935`

**Root Cause**: The EPW header "Extremes" section is a compact, lossy summary of the full design-condition data that resides in the companion DDY file. The DDY format preserves the full ASHRAE Handbook design condition table including humidity and wind at each design percentile.

**Impact**:
- **Humidity conditions at design temperature are absent.** ACCA Manual J-2016 requires outdoor design humidity for latent load calculations. The DDY provides wetbulb/dewpoint at max dry-bulb for heating (99.6%) and cooling (0.4%) conditions. HARES's autosizing path (`autosize.rs:204-218`) uses `autosize_capacity_cooling` with peak solar but zero humidity context, meaning latent loads from outdoor air infiltration at design dewpoint are not accounted for in autosizing.
- **Design wind speed is absent.** Manual J infiltration calculations depend on wind speed at design conditions. DDY files provide wind speed for heating (99.6%) and cooling (0.4%) conditions; HARES has no source for design wind speed except the ASHRAE 152 station data, which does not include wind.
- **Barometric pressure at design is absent.** Pressure affects air density, fan power, and psychrometric calculations. The EPW header does not carry barometric pressure; the DDY does. HARES does not use design pressure in autosizing, but it would matter for high-altitude locations where the standard-atmosphere assumption is incorrect.
- **Clear-sky optical depth (taub/taud) are absent.** For cooling design, the DDY provides ASHRAE clear-sky optical depths for beam and diffuse irradiance. `autosize_capacity_cooling` (`autosize.rs:204-218`) currently uses the Perez (1990) model with location-independent clear-sky assumptions, which may differ from the location-calibrated ASHRAE taub/taud values in the DDY. For Denver at 1726 m elevation, the difference in clear-sky beam irradiance between Perez-default and ASHRAE-taub could be ~5–10%.

### Finding 3: ASHRAE 152 station fallback covers only 224 US locations vs. 1,000+ TMY3/EPW locations [Severity: low]
**Description**: When EPW design conditions are unavailable (PSM3, TMY3, ResStock CSV formats), `resolve_design_temperatures` (`autosize.rs:343-373`) falls back to a nearest-neighbor lookup in the ASHRAE 152 station database (`ashrae152.rs:333-338`). The database contains 224 stations, primarily at US airport locations. TMY3/EPW files cover over 1,000 locations across the US and international sites. For a residence far from the nearest ASHRAE 152 station (e.g., rural areas), the haversine-nearest lookup can produce design temperatures from a station 50–150 km away, which may differ from local conditions by 3–5°C for heating and 2–4°C for cooling. The EPW header "Extremes" values for the local EPW file would be more representative.

**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:357-368`, `crates/hares-physics/src/ashrae152.rs:333-338`

**Root Cause**: The ASHRAE 152 station database was created for OCHRE's duct DSE calculations and covers a representative but limited set of locations. HARES reuses it unchanged.

**Impact**: Autosizing inaccuracy for sites without EPW-format weather files. The fallback priority (EPW "Extremes" → ASHRAE 152 → hardcoded defaults) is reasonable for locations near a station, but degrades for rural sites. For the common case where an EPW file is available, the EPW header "Extremes" are preferred (Tier 1), so this issue only affects the PSM3/TMY3/ResStock paths.

### Finding 4: STAT file could replace EPW-based ground temperature computation with measured soil data [Severity: low]
**Description**: EnergyPlus reads STAT files to extract monthly daily-average dry-bulb for water mains temperature autosizing (`WeatherManager.cc:8660-8729`). The STAT file also contains detailed monthly statistics (hourly average dry-bulb profiles, extreme max/min with timestamps, dew-point statistics) that could validate or replace the DOE-2 sinusoidal ground temperature model (`epw.rs:489-503`) when EPW ground temperature header data is absent. The ASHRAE 152 station database provides only design and seasonal temperatures, not monthly profiles.

**Code Location**: `crates/hares-io/src/epw.rs:282-287`, `epw.rs:489-503`

**Root Cause**: STAT parsing was not implemented. The DOE-2 model is used as a fallback when EPW ground temperature data is missing from the header.

**Impact**: Currently latent — HARES has no water mains temperature model, and the DOE-2 ground temperature model works adequately for most soil boundary conditions. When water heater or hydronic system sizing is added, STAT-derived monthly dry-bulb profiles would improve water mains temperature estimation over the current hardcoded assumption (`autosize.rs:463`: `mains_temp_c: 15.0` in test fixtures).

### Finding 5: WeatherManager.cc DDY-processing design is documented but not mirrored [Severity: low]
**Description**: The EnergyPlus reference implementation processes DDY design days through `GetDesignDayData` (line 5922) → `SetUpDesignDay` (line 3485). `SetUpDesignDay` generates a full 24-hour weather profile from each design-day definition using ASHRAE daily temperature range multiplier profiles, humidity-condition interpolation, and ASHRAEClearSky solar model with site-specific taub/taud. HARES could adopt a similar approach for a design-day simulation mode — running a single 24-hour period at design conditions instead of requiring a full-year EPW for sizing. This would decouple autosizing from the annual weather file entirely.

**Code Location**: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:3485-3713`, `WeatherManager.cc:5922-6119`

**Root Cause**: Not a bug — HARES currently couples autosizing to either the EPW header "Extremes" summary or ASHRAE 152 station data. A design-day simulation mode is a feature gap, not a defect.

**Impact**: Autosizing always requires a weather file (for location metadata and atmospheric parameters) even though the annual time series is not used during sizing. A standalone design-day mode would allow users to run HARES for sizing purposes without an EPW file, using only DDY data or ASHRAE 152 station data. This would be useful for building retrofit assessments where local weather data is unavailable.

## Summary

- **Total findings**: 5
- **Critical**: 0
- **High**: 0
- **Medium**: 2
- **Low**: 3

## Recommendations

1. **Add DDY parsing as the preferred design-condition source (medium priority).**
   A DDY parser would extract `SizingPeriod:DesignDay` objects into a structured format. For residential autosizing, the primary design days of interest are:
   - `Ann Htg 99.6% Condns DB` — heating dry-bulb with wetbulb humidity
   - `Ann Clg 0.4% Condns DB=>MWB` — cooling dry-bulb with mean coincident wet-bulb
   These would supersede the EPW header "Extremes" when a `.ddy` file is co-located with the `.epw` file (standard EnergyPlus convention: same basename, different extension).

2. **Augment `DesignConditions` with humidity and wind fields (medium priority).**
   Add `design_wetbulb_c`, `design_wind_speed_m_s`, and `design_pressure_kpa` to the `DesignConditions` struct. The DDY parser would populate these; when only EPW header data is available, the current dry-bulb-only behavior would be the fallback. The `autosize_capacity_cooling` path should pass humidity conditions for latent load calculation.

3. **Add STAT parsing for annual statistics (low priority).**
   A STAT parser would extract the monthly daily-average dry-bulb profile for water mains temperature estimation and could validate/cross-check the DOE-2 ground temperature model. This can wait until water heater sizing is implemented.

4. **Expand ASHRAE 152 station coverage or add DDY-based lookup (low priority).**
   The 224-station database is adequate for the duct DSE calculation (its primary purpose) but has coarser geographic coverage than the TMY3/EPW station network. Consider augmenting with the 1,000+ station design conditions from ASHRAE Handbook Chapter 14 tables, or providing a fallback that reads design conditions from the EPW header even for non-EPW weather sources when both files are available.

5. **Document the DDY/STAT gap clearly in weather format selection (low priority).**
   When `detect_weather_format` encounters a `.ddy` or `.stat` extension, the error message should direct users to the companion `.epw` file rather than reporting an unsupported format. This improves UX for users accustomed to the EnergyPlus convention of downloading all three files together.

## References / Citations

- **ACCA Manual J-2016 (8th Ed.)**, Residential Load Calculation, §4 (Outdoor Design Conditions): Requires outdoor dry-bulb, wet-bulb (or dewpoint), and wind speed at design percentile. The DDY format provides all three per ASHRAE Handbook Chapter 14 percentiles.
- **ACCA Manual S-2017 (4th Ed.)**, Residential Equipment Selection, §4: Oversizing factors applied to Manual J design loads. Referenced in `autosize.rs:31-38`.
- **ASHRAE Handbook—Fundamentals (2009/2017)**, Chapter 14 (Climatic Design Information): Tables provide percentiles for heating (99.6%, 99.0%) and cooling (0.4%, 1.0%, 2.0%) with mean coincident wet-bulb/dewpoint, wind speed, and clear-sky optical depth. DDY files are machine-readable extracts of these tables. The ASHRAE 152 station database uses the same source for its `Heating Design Temp` and `Cooling Design Temp` fields.
- **EPW Data Dictionary v9.6**, §2 (Header Fields): The "Extremes" summary on line 2 provides only extreme high/low dry-bulb temperatures. The EPW standard explicitly notes that the DDY file is the authoritative design-day source.
- **EnergyPlus WeatherManager.cc**: `GetDesignDayData` (line 5922) reads `SizingPeriod:DesignDay` objects; `SetUpDesignDay` (line 3485) generates hourly profiles; `CalcAnnualAndMonthlyDryBulbTemp` (line 8660) reads STAT files for water mains autosizing.
