# ASHRAE 152 climate station design temperature lookup — CSV parsing, interpolation, and fallback logic
**Review ID**: dse-deep-01
**Category**: ashrae152-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ashrae152.rs` (lines 86–184: ClimateStation, CSV parsing, Haversine lookup; lines 329–396: design_temperatures_f and climate usage within calculate_dse)
- `crates/hares-physics/data/ASHRAE152_climate_data.csv` (224 rows, 17-column header)
- `crates/hares-core/src/dwelling/autosize.rs` (lines 275–298: caller-side fallback usage)
- `crates/hares-physics/src/constants.rs` (line 168: `BOILER_AUXILIARY_HOURS_PER_YEAR`)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/equipment.py` (lines 161–467: `calculate_duct_dse()`; lines 216–242: nearest-station climate lookup)
- `vendors/OCHRE/ochre/defaults/ASHRAE152_climate_data.csv` (identical header and data to HARES)
- `vendors/OCHRE/ochre/utils/hpxml.py` (lines 889–892: 2080-hour boiler auxiliary convention)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` (lines 5922–6039: DesignDay parsing; lines 4411–4487: location resolution)
- `vendors/EnergyPlus/src/EnergyPlus/OutputReportTabular.cc` (lines 5460–5799: .stat file design condition reporting)
- `vendors/EnergyPlus/src/EnergyPlus/DataSizing.hh` (lines 463–464, 630–635, 768–797: ZoneHVACSizingData)

## Findings

### Finding 1: [Severity: informational] CSV column mapping is correct — no column-shift error
**Description**: The `parse_climate_csv()` function at `ashrae152.rs:112–158` maps 0-based split indices to the 17-column CSV header without any offset error. Each parsed field index was verified against the actual CSV header and sample data rows.
**Code Location**: `ashrae152.rs:134–144`
**Root Cause**: N/A (confirmatory finding)
**Impact**: None. All climate station values are correctly assigned:

| Field | CSV Header (0-based col) | Parse Index | Sample (Birmingham, AL) | Parsed Value |
|---|---|---|---|---|
| Latitude | `Latitude` (3) | `parse(3)` | `33.567` | 33.567 |
| Longitude | `Longitude` (4) | `parse(4)` | `-86.75` | -86.75 |
| Heating Design Temp | `Heating Design Temp` (5) | `parse(5)` | `21` | 21 °F |
| Heating Seasonal Temp | `Heating Seasonal Temp` (6) | `parse(6)` | `43` | 43 °F |
| Cooling Design Temp | `Cooling Design Temp` (7) | `parse(7)` | `91` | 91 °F |
| Cooling Seasonal Temp | `Cooling Seasonal Temp` (8) | `parse(8)` | `82` | 82 °F |
| Wseasonal | `Wseasonal` (10) | `parse(10)` | `0.0143` | 0.0143 |
| Seasonal hout | `Seasonal hout` (14) | `parse(14)` | `35` | 35 |
| Seasonal hin | `Seasonal hin` (16) | `parse(16)` | `29` | 29 |

The HARES CSV file is bitwise-identical to the OCHRE vendor reference file (224 lines including header, verified via `diff`).

### Finding 2: [Severity: medium] Single-nearest-neighbor without distance threshold — no inverse-distance-weighted interpolation
**Description**: `nearest_station()` at `ashrae152.rs:174–184` uses the Haversine great-circle formula (Earth radius = 6,373 km, matching OCHRE) and `min_by` to select the single geographically closest station. There is no inverse-distance-weighted interpolation across multiple nearby stations, and no maximum distance threshold beyond which the result is considered unreliable.
**Code Location**: `ashrae152.rs:174–184`
**Root Cause**: Design choice consistent with the OCHRE reference implementation (`vendors/OCHRE/ochre/utils/equipment.py:217–227`), which also uses `argmin()` on the Haversine distance array. ASHRAE 152 (ANSI/ASHRAE 152-2004) does not specify an interpolation method; it presumes the user selects a representative climate station.
**Impact**: Low for the continental US (station density is ~6 stations per state, typical nearest-station distance < 100 km). Moderate for remote Alaska and international locations where station gaps can exceed 500 km. A single-nearest-neighbor selection in a sparse region can use a station with substantially different climate (e.g., coastal vs. interior) despite a nearer-but-topographically-different station being only slightly farther.

**Accuracy assessment**: Single-nearest-neighbor is a defensible first-order approach. Inverse-distance-weighted (IDW) interpolation across the 3–5 nearest stations would reduce sensitivity to station placement and is standard practice in microclimate estimation. The 198-line lookup here could be upgraded to IDW at minimal computational cost (<1 ms for 225 stations) by replacing `min_by` with a weighted average of the _k_ nearest stations.

### Finding 3: [Severity: high] No fallback protection in `calculate_dse()` — distant stations accepted silently
**Description**: `calculate_dse()` at `ashrae152.rs:376` calls `nearest_station()` directly without any distance check. The `.expect()` call at `ashrae152.rs:183` guarantees a result as long as at least one row was parsed, so the DSE will always compute with _some_ station's data, regardless of how far away it is. Unlike the public `design_temperatures_f()` function (which returns `None` for the (0,0) sentinel but has no distance guard either), `calculate_dse()` has no escape hatch — it cannot return an error or warn about an implausible climate match.
**Code Location**: `ashrae152.rs:376` (the `nearest_station` call within `calculate_dse`), `ashrae152.rs:183` (the unconditional `.expect()`)
**Root Cause**: The DSE path and the design-temperature-only path share the same `nearest_station()` function, but only the caller in `autosize.rs:284–292` provides a fallback (conservative defaults of -10 °C heating / 35 °C cooling). The `calculate_dse()` path has no equivalent guard.
**Impact**: For a project site in the Aleutian Islands, the nearest valid station (after Adak is excluded for missing cooling data) could be Annette or Anchorage — over 1,500 km away — yet the DSE would be computed silently with that distant station's design temperatures. This could bias the heating/cooling DSE by an unpredictable amount depending on the climate mismatch. At minimum, a warning should be emitted when the nearest-station distance exceeds a threshold (e.g., 200 km).

### Finding 4: [Severity: low] Alaska stations with empty cooling fields excluded — undocumented geographic gaps
**Description**: The CSV parser at `ashrae152.rs:119–157` skips rows where any required field is empty. Some Alaska stations in the climate dataset (e.g., Adak at index 1, Nome partially at index 12) have empty cooling design temperatures, cooling seasonal temperatures, and seasonal enthalpy columns. These rows are correctly excluded from `CLIMATE_DATA`, but the consequence is that locations physically near these stations have no representative station in the dataset.
**Code Location**: `ashrae152.rs:125–144` (the `parse` closure and `continue` guards)
**Root Cause**: The raw ASHRAE 152 climate dataset (sourced from OCHRE) omitted cooling data for some remote Alaska stations. HARES's parser correctly handles the missing data by skipping those rows, which is preferable to OCHRE's approach (which would produce `NaN` and propagate garbage through the DSE formula). However, this leaves geographic holes in station coverage.
**Impact**: Low. These are remote locations with very few buildings. The `autosize.rs` fallback (-10 °C / +35 °C) provides a reasonable safety net for design temperature lookups, but the DSE calculation itself has no such guard. In practice, a building in Adak is unlikely to be modeled with HARES, but the silent gap is worth documenting.

### Finding 5: [Severity: informational] 2080 auxiliary hours convention correctly separated from DSE — overridable for variable-speed equipment
**Description**: The ASHRAE 152 DSE calculation is a dimensionless seasonal efficiency; it does not directly consume annual operating hours. The 2080-hour convention (ANSI/RESNET/ICC 301-2019 Equation 4.4-5) for auxiliary fan/pump loads is correctly defined as `BOILER_AUXILIARY_HOURS_PER_YEAR = 2_080.0` in `constants.rs:168` and is applied in the HVAC energy calculation layer (e.g., `hares-io/src/hpxml/resolve_hvac.rs:1647` for boiler auxiliary power). The DSE itself operates purely on steady-state heat transfer physics (duct conduction loss factors `bs_high`/`br_high`, leakage fractions `as_high`/`ar_high`, and enthalpy ratios). Multi-speed equipment with non-continuous fan operation is handled via:
- `n_speeds > 1` triggering the low-speed branch (lines 473–492 for duct factors, lines 502–506 for heating uncorrected DE, lines 515–523 for cooling uncorrected DE)
- `capacity_low_w` and `fan_flow_low_m3_s` providing the low-speed operating point
- Part-load equipment factor adjustments for heat pumps (`0.44 + 0.56 * seas_uncorr_de`) and variable-speed cooling (`0.82 + 0.18 * seas_uncorr_de`)

**Code Location**: `ashrae152.rs:345–578` (full `calculate_dse`), `constants.rs:161–168` (`BOILER_AUXILIARY_HOURS_PER_YEAR`)
**Root Cause**: N/A — correct design.
**Impact**: No bias in the DSE value itself. The 2080-hour convention is applied at the correct architectural layer (annual energy calculation), not within the dimensionless seasonal efficiency. EnergyPlus's `Duct:Loss:Conduction` and `Duct:Loss:Leakage` objects (in `DuctLoss.cc:77–79`) take a fundamentally different approach (explicit duct geometry and hourly simulation) and do not use a DSE abstraction at all, so direct comparison is not meaningful.

### Finding 6: [Severity: low] Haversine Earth radius differs from WGS-84 by 0.3%
**Description**: The Haversine function at `ashrae152.rs:171` uses an Earth radius of 6,373 km. The WGS-84 mean radius is 6,371.009 km. The OCHRE reference uses 6,373 km identically (`equipment.py:226`), so this is a faithful port, but the 2 km difference introduces a systematic 0.03% distance error that is negligible for climate station selection.
**Code Location**: `ashrae152.rs:171`
**Root Cause**: OCHRE used 6,373 km (possibly the volumetric mean radius); HARES preserved this value exactly during the port.
**Impact**: Negligible. A 200 km station distance would be computed as 200.06 km. This does not affect nearest-station selection order for any station pair in the 224-station dataset.

## Summary
- Total findings: 6
- Critical: 0
- High: 1 (Finding 3: no distance fallback in `calculate_dse()`)
- Medium: 1 (Finding 2: single-nearest-neighbor without interpolation)
- Low: 2 (Findings 4, 6: excluded Alaska stations, Haversine radius)
- Informational: 2 (Findings 1, 5: confirmed-correct CSV mapping, 2080-hour separation)

## Recommendations

1. **Add a distance guard in `calculate_dse()`**: After `nearest_station()`, compute the Haversine distance and log a warning (or return an error) if it exceeds a configurable threshold (e.g., 200 km). This would mirror the safety net already present in `autosize.rs:284–292`.

2. **Consider inverse-distance-weighted interpolation across k-nearest stations**: Replace the single-nearest-neighbor selection with IDW over k=3–5 stations. This reduces sensitivity to station placement and provides smoother spatial coverage. The computational cost is negligible for 225 stations. Weight = 1/d² where d is Haversine distance; normalize by sum of weights.

3. **Document the geographic gaps from excluded Alaska stations**: Add a comment in the CSV parser noting that stations with empty cooling data (Adak, Nome, etc.) are excluded from the lookup and that locations near excluded stations will fall back to the next-nearest valid station, which may be far away.

4. **Document Earth radius choice**: Add a brief comment at `ashrae152.rs:171` noting that 6,373 km matches the OCHRE reference and is the volumetric mean radius, explaining the ~2 km discrepancy from WGS-84.

5. **Add a unit test for distant-station edge case**: A test that queries a remote Pacific location (lat=0, lon=-170) should verify that `nearest_station()` returns a station and that the distance is logged/reported, establishing a baseline for future distance-threshold behavior.

## References / Citations

- ASHRAE 152-2004: Method of Test for Determining the Design and Seasonal Efficiencies of Residential Thermal Distribution Systems
- ANSI/RESNET/ICC 301-2019: Standard for the Calculation and Labeling of Energy Performance of Dwelling and Sleeping Units using an Energy Rating Index — Equation 4.4-5 (boiler auxiliary hours)
- OCHRE: `ochre/utils/equipment.py:161–467` — reference DSE implementation using single-nearest-neighbor Haversine lookup
- EnergyPlus: `WeatherManager.cc:4411–4487` — location resolution; `OutputReportTabular.cc:5460–5799` — .stat file ASHRAE design condition reporting
- EnergyPlus `DuctLoss.cc:77–79` — alternative duct loss modeling approach (conduction, leakage, makeup air), not DSE-based
- WGS-84 mean Earth radius: 6,371.009 km (NGA.STND.0036_1.0.0_WGS84)
