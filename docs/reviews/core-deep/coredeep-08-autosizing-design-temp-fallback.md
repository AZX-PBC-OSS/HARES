# Autosizing design temperature fallback chain: EPW header → ASHRAE 152 station → hardcoded defaults
**Review ID**: coredeep-08
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/autosize.rs` (primary consumer, lines 280–311)
- `crates/hares-io/src/epw.rs` (parser and struct, lines 48–57 and 862–935)
- `crates/hares-physics/src/ashrae152.rs` (station lookup, lines 88–184 and 329–339)
- `crates/hares-physics/data/ASHRAE152_climate_data.csv` (station database)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: medium]
**Description**: EPW "Extremes" parser extracts weekly extreme min/max from all weather parameters, not the ASHRAE 99.6%/0.4% design dry-bulb conditions. The parsed values are absolute record extremes (not design conditions), and the parser mixes non-dry-bulb parameters (dew point, pressure, wind speed) into the min/max calculation via a parameter-agnostic toggle.

**Code Location**:
- Struct definition: `crates/hares-io/src/epw.rs:48–57` — `DesignConditions { heating_design_db_c, cooling_design_db_c }`
- Parser: `crates/hares-io/src/epw.rs:883–935` — `parse_design_conditions()`

**Root Cause**: The `parse_design_conditions` function finds the literal token "Extremes" (case-insensitive) in EPW header line 2 and then treats EVERY subsequent numeric field as alternating (low, high) pairs regardless of which weather parameter each value belongs to. It then takes `min()` of all collected "lows" and `max()` of all "highs."

Examining the Denver TMY3 EPW header line 2:
```
DESIGN CONDITIONS,1,Climate Design Data 2009 ASHRAE Handbook,,Heating,12,-17.4,-14,...Cooling,7,15.2,34.6,...Extremes,11.9,10.4,8.8,20.7,-22.7,37.1,2.8,1.3,-24.7,38,-26.3,38.8,-27.9,39.5,-29.9,40.5
```

The proper ASHRAE 99.6% heating design for Denver is **-17.4°C** and the 0.4% cooling design is **34.6°C** — both explicitly present in the `Heating`/`Cooling` sections BEFORE the `Extremes` token. The code ignores these sections entirely.

Instead, the "Extremes"-based parser extracts -29.9°C (heating) and 40.5°C (cooling) — absolute record extremes that are 12.5°C colder and 5.9°C hotter than the true design conditions.

The parameter mixing is visible in the New Haven (CT) TMY3 EPW header:
```
Extremes,8.8,7.7,6.9,-15.2,33.4,3,2.3,-17.3,35.1,-19,36.4,-20.7,37.7,-22.9,39.3,-16.1
```

The parser's toggle treats these as (8.8=low, 7.7=high), (6.9=low, -15.2=high), (33.4=low, 3=high), etc. Value -15.2 is categorized as a cooling candidate (a "high") and 33.4 as a heating candidate (a "low") — clearly incorrect for a field meant to contain dry-bulb extremes. This works "by accident" only because the global min/max filters out these non-dry-bulb outliers.

**Impact**: Equipment sizing is *conservative* (oversized), not dangerous, because absolute extremes are more severe than design conditions. However:
1. The misleading struct name `DesignConditions` implies ASHRAE design conditions when the fields actually hold absolute extremes.
2. Equipment may be unnecessarily oversized, reducing efficiency (short-cycling, higher upfront cost).
3. The parser is fragile: EPW sources with different "Extremes" section ordering would silently produce wrong values because no parameter-type validation exists.
4. Obscures the fact that proper ASHRAE 99.6%/0.4% values are already present in the header line.

### Finding 2: [Severity: low]
**Description**: ASHRAE 152 nearest-station lookup has no distance guard. The `nearest_station` function always returns the geographically nearest station, even if it is hundreds of kilometers away across different climate zones.

**Code Location**: `crates/hares-physics/src/ashrae152.rs:174–184` (nearest_station), `ashrae152.rs:333–339` (design_temperatures_f)

**Root Cause**: The `nearest_station` function uses haversine distance to find the minimum-distance station from the 224-entry CSV database with no maximum distance threshold. The `design_temperatures_f` public API only returns `None` for the exact (0,0) lat/lon sentinel; for all other coordinates it returns the nearest station, regardless of distance or climate representativeness. There is no WMO station code matching — the EPW LOCATION header line 1 contains a WMO code (e.g., `725650` for Denver) that is not compared against any station identifier in the ASHRAE 152 CSV (which lacks WMO columns).

**Impact**: Low in practice. The 224-station database provides reasonable coverage of the contiguous US and southern Canada with typical inter-station distances of 100–300 km. Alaskan coverage is sparser (13 stations across a vast state), but no real building is likely to be >500 km from the nearest station. The greater risk is for Hawaii outer islands or very remote Canadian locations should the database be used outside its designed geography.

### Finding 3: [Severity: low]
**Description**: Hardcoded fallback defaults (-10°C heating / 35°C cooling) are labeled "conservative" in the warning message but are only conservative for mild climates. For cold climates (Fairbanks design temp -31°F = -35°C), -10°C would undersize by ~25°C. However, this code path is essentially unreachable in practice.

**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:297–305`

**Root Cause**: The `resolve_design_temperatures` function calls `design_temperatures_f(lat, lon).unwrap_or_else(|| ...)`. The `design_temperatures_f` function (`ashrae152.rs:333–339`) only returns `None` when both lat and lon are approximately 0.0 (the unset-location sentinel). Since `nearest_station` always returns a station for any other coordinates and the function does not validate coordinate ranges, the hardcoded defaults are only reached when no location information is available at all. The warning message `"using conservative defaults: heating -10 °C, cooling 35 °C"` is misleading because:
1. The defaults aren't conservative for extreme climates.
2. The message is emitted at `warn!` level, which may be ignored in production log filtering.

**Impact**: Very low — the code path is only triggered for the (0,0) sentinel, and in that case no meaningful location data exists so any default is arbitrary. The misleading message text is a documentation concern.

### Finding 4: [Severity: low]
**Description**: Site latitude/longitude from `building.site` override weather file coordinates, but both are treated as equally authoritative for the ASHRAE 152 station lookup.

**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:295–296`

**Root Cause**: The latitude/longitude used for ASHRAE 152 station proximity matching is:
```rust
let lat = site_lat.unwrap_or(weather_lat);
let lon = site_lon.unwrap_or(weather_lon);
```
When `site_lat` is `Some(...)`, it overrides `weather_lat` from the EPW AUTOSIZE_CONTEXT. If the HPXML site coordinates differ from the weather file coordinates (e.g., a building located at a different site than the weather station), the nearest ASHRAE 152 station will match the building site rather than the weather station location. This is arguably correct for building-centric design, but no warning is emitted when the site and weather coordinates differ significantly.

**Impact**: Low. This is a design decision (building-centric vs weather-station-centric). A mismatch large enough to matter would indicate a data quality issue that should be flagged.

### Finding 5: [Severity: informational]
**Description**: Heating and cooling design temperatures are correctly separated and used exclusively for their respective autosizing paths. No cross-contamination between heating design temp and cooling autosizing, or vice versa.

**Code Location**: `crates/hares-core/src/dwelling/autosize.rs:120` (heating path uses `heating_design_c`), `autosize.rs:198` (cooling path uses `cooling_design_c`)

**Verification**: The `resolve_design_temperatures` function returns a tuple `(heating_design_c, cooling_design_c)` sourced from the same fallback chain for both values. The heating autosizing block (lines 118–186) uses only `heating_design_c`, and the cooling autosizing block (lines 188–267) uses only `cooling_design_c`. Temperature conversions (°F → °C via `temperature_f_to_c`) are applied consistently in both paths. No findings — this is correct.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 0
- **Medium**: 1
- **Low**: 3
- **Informational**: 1

## Recommendations

1. **Rename `DesignConditions` struct** to `ExtremeConditions` or `EpwExtremes` to accurately reflect that fields contain absolute extreme values, not ASHRAE 99.6%/0.4% design conditions. Update the field documentation at `epw.rs:48–57` accordingly.

2. **Parse ASHRAE design conditions from EPW header when available.** The TMY3 EPW header contains explicit 99.6% heating and 0.4% cooling design dry-bulb temperatures in the `Heating` and `Cooling` sections BEFORE the `Extremes` token. A proper parser for these sections would extract the true ASHRAE design values (position 3 after each section marker) and use them in preference to the Extremes-based heuristic. Fall back to the current Extremes parser for non-TMY3 EPW formats that lack these sections.

3. **Add a distance guard to `nearest_station`.** Emit a `warn!`-level log when the nearest ASHRAE 152 station is >200 km from the target coordinates, indicating that the matched station's climate data may not be representative. Consider adding WMO-station-based matching by including WMO codes in the ASHRAE 152 CSV and matching against the WMO code parsed from the EPW LOCATION header.

4. **Clarify the hardcoded defaults message.** Change the warn message at `autosize.rs:299–303` from "conservative defaults" to "arbitrary defaults" to avoid implying these values are safe for all climates.

5. **Add a distance check between site and weather coordinates.** When `site_lat` and `weather_lat` differ by more than ~1° (~111 km), emit a debug-level diagnostic noting the mismatch, as it affects which ASHRAE 152 station is selected.

## References / Citations
- EPW Data Dictionary v9.6: Header line 2 design conditions format
- ASHRAE Handbook — Fundamentals 2017, Chapter 14: Climatic Design Conditions
- ACCA Manual J (8th ed.): Design temperature selection for residential load calculations
- ACCA Manual S (2nd ed., 2017): Equipment oversizing factors (1.4× heating, 1.15× cooling)
- EnergyPlus Engineering Reference: EPW format specification
