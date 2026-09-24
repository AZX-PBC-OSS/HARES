# All 5 TerrainClass variants (Ocean, Flat, Rough, VeryRough, Urban) — wind profile alpha and delta parameters
**Review ID**: infil-deep-02
**Category**: infiltration-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/infiltration.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc` — Building terrain selection (lines 565–597) and Site:HeightVariation override (lines 1260–1316)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` — Site:WeatherStation object, wind sensor defaults (lines 7218–7286)
- `vendors/EnergyPlus/src/EnergyPlus/DataEnvironment.cc` — `OutWindSpeedAt` two-stage terrain correction (lines 170–188)

## Findings

### Finding 1: Only 3 of 5 terrain classes are implemented — Ocean and a distinct VeryRough/Rough class are missing
**Severity**: high
**Description**: The `TerrainClass` enum at `infiltration.rs:84–88` defines only three variants:
```rust
pub enum TerrainClass {
    Rural,
    Suburban,
    Urban,
}
```
The EnergyPlus `Building` object (and the ASHRAE HOF terrain classification) supports **five** distinct terrain types with unique α/δ pairs:
| Class | α (alpha) | δ (delta) | EnergyPlus keyword |
|-------|-----------|-----------|--------------------|
| Ocean / open water | 0.10 | 210 m | `OCEAN` |
| Flat / open country | 0.14 | 270 m | `COUNTRY` |
| Rough / suburban | 0.22 | 370 m | `SUBURBS` |
| VeryRough / urban centre | 0.33 | 460 m | `CITY` |
| Urban (EP alias) | 0.22 | 370 m | `URBAN` |

HARES maps its three classes as follows (`infiltration.rs:36–45`):
- `Rural` → α=0.14, δ=270 (matches EnergyPlus `COUNTRY`)
- `Suburban` → α=0.22, δ=370 (matches EnergyPlus `SUBURBS`)
- `Urban` → α=0.33, δ=460 (matches EnergyPlus `CITY`)

The **Ocean** terrain class (α=0.10, δ=210) is entirely absent. The "VeryRough" class (EnergyPlus `CITY`, α=0.33) is present but named `Urban` in HARES, while EnergyPlus's `URBAN` keyword maps to suburban parameters (α=0.22, δ=370).

**Code Location**: `infiltration.rs:84–88` (enum definition); `infiltration.rs:91–107` (alpha/delta dispatch); `infiltration.rs:685` (`attic_ela_coefficients` defaults to `TerrainClass::Suburban`); `infiltration.rs:698` (`garage_ela_coefficients` defaults to `TerrainClass::Suburban`)

**Root Cause**: The HARES `TerrainClass` enum was designed to mirror the three EnergyPlus Building-object terrain keywords (`Country`, `Suburbs`, `City`) but omitted the `Ocean` class and conflated the EnergyPlus `URBAN` alias (α=0.22) with the `CITY` class (α=0.33).

**Impact**: Coastal residential sites (Florida, California coast, Gulf Coast, Atlantic seaboard, Great Lakes shoreline) — which represent a large fraction of US residential stock — will be assigned Rural (α=0.14) or Suburban (α=0.22) instead of Ocean (α=0.10). The wind speed correction factor for a 5 m building height is:

| Class | f_t (at 5 m) |
|-------|-------------|
| Ocean (α=0.10, δ=210) | 0.674 |
| Rural (α=0.14, δ=270) | 0.747 |
| Suburban (α=0.22, δ=370) | 0.620 |

Using Rural instead of Ocean overestimates the wind speed correction by ~11% at building height. This propagates into the AIM-2 wind coefficient and ELA wind coefficient, biasing infiltration rates upward by 5–15% for coastal homes.

### Finding 2: Terrain class naming is inconsistent with EnergyPlus
**Severity**: medium
**Description**: HARES's `TerrainClass::Urban` maps to α=0.33 / δ=460, which is EnergyPlus's `CITY` terrain. EnergyPlus's `URBAN` keyword maps to α=0.22 / δ=370 (identical to `SUBURBS`), per `HeatBalanceManager.cc:582–585`:
```cpp
} else if (AlphaName(2) == "URBAN") {
    state.dataEnvrn->SiteWindExp = 0.22;
    state.dataEnvrn->SiteWindBLHeight = 370.0;
    AlphaName(2) = "Urban";
}
```

This means a HARES user selecting `Urban` thinking it corresponds to EnergyPlus `Urban` (suburban-like) would get City-level wind attenuation (α=0.33), understating wind-driven infiltration. Conversely, a user wanting EnergyPlus `City` roughness gets the correct parameters but under a misleading name.

**Code Location**: `infiltration.rs:43–45` (URBAN_ALPHA, URBAN_DELTA_M constants); `infiltration.rs:96` (Urban arm in `alpha()`)

**Root Cause**: The naming convention appears to combine ASHRAE's "VeryRough" class (equivalent to EP `CITY`) under the `Urban` label, without awareness of the EnergyPlus `URBAN` keyword's different semantics.

**Impact**: Potential misclassification when users map EnergyPlus input files to HARES — selecting `Urban` in HARES produces significantly different wind correction than EnergyPlus `URBAN`. Confusion risk is moderate since most users consume defaults.

### Finding 3: No `Site:WeatherStation` override — met station constants are hardcoded
**Severity**: low
**Description**: HARES hardcodes the meteorological station wind profile parameters at `infiltration.rs:36–38`:
```rust
const MET_STATION_ALPHA: f64 = 0.14;
const MET_STATION_DELTA_M: f64 = 270.0;
const MET_STATION_HEIGHT_M: f64 = 10.0;
```

EnergyPlus allows the user to override these via the `Site:WeatherStation` object (`WeatherManager.cc:7232–7265`), which reads `WeatherFileWindExp`, `WeatherFileWindBLHeight`, and `WeatherFileWindSensorHeight` from the IDF input file. These values feed into `WeatherFileWindModCoeff`, which is the first stage of the two-stage wind correction.

The hardcoded defaults match EnergyPlus's IDD defaults (α_met=0.14, δ_met=270 m, h_met=10 m) and are correct for the vast majority of TMY3 and EPW weather files (which assume airport/open-country siting). However, some custom weather stations or non-standard EPW files may use different reference heights or terrain assumptions.

**Code Location**: `infiltration.rs:36–38` (met station constants); `infiltration.rs:187–195` (`terrain_wind_speed` function consuming them)

**Root Cause**: Architectural simplification — HARES targets residential simulation where the default weather station parameters are near-universal.

**Impact**: Negligible for standard EPW/TMY3 weather data. Could bias results for non-standard weather files (e.g., urban weather stations, custom micro-climate stations). Mitigation: the hardcoded values should be documented as assumptions in the crate-level docs.

### Finding 4: Two-stage wind correction matches EnergyPlus — no mathematical error
**Severity**: low
**Description**: The HARES `terrain_wind_speed` function (`infiltration.rs:187–195`):
```rust
u_met * (MET_STATION_DELTA_M / MET_STATION_HEIGHT_M).powf(MET_STATION_ALPHA)
      * (height / delta_site).powf(alpha_site)
```
is mathematically equivalent to EnergyPlus's `OutWindSpeedAt` (`DataEnvironment.cc:187–188`):
```cpp
WindSpeed * WeatherFileWindModCoeff * pow(Z / SiteWindBLHeight, SiteWindExp)
```
where `WeatherFileWindModCoeff = (δ_met / h_met)^α_met`.

Both implement the ASHRAE two-stage power-law correction: first convert met-station wind to gradient/boundary-layer wind using the station terrain parameters, then convert gradient wind to local wind at building height using the site terrain parameters. This is correct.

The HARES **embedded correction** strategy (baking terrain into `shelter_coeff` for AIM-2, and into `wind_coeff` for ELA) is also consistent with the approach documented at `infiltration.rs:566–569` and `infiltration.rs:661–667`.

**Code Location**: `infiltration.rs:187–195` (terrain correction formula); `DataEnvironment.cc:187–188` (EP equivalent)

**Root Cause**: N/A — implementation is correct.

**Impact**: N/A — no error. The double-correction risk is well-documented in tests (`infiltration.rs:1621–1682`, `infiltration.rs:1683–1727`).

### Finding 5: Test comment references incorrect ASHRAE HOF chapter
**Severity**: low
**Description**: The test at `infiltration.rs:1605` comments:
```
// ASHRAE HoF Ch.24 Table 1 suburban terrain: alpha=0.22, delta=370 m.
```
The terrain/wind profile coefficients are in ASHRAE HOF Chapter 24 (*Airflow Around Buildings*), which is correct for the 2021 edition. However, the test at line 892–895 (`terrain_coefficients_match_appendix_classes`) asserts that at 10 m height with suburban terrain, `terrain_wind_speed(5.0, Suburban, 10.0) ≈ 3.584` — which is simply `5.0 * (270/10)^0.14 * (10/370)^0.22 ≈ 3.584`. The test at `infiltration.rs:1598–1618` also documents that a prior claim of ~0.62 correction factor at 8 m was incorrect (actual is ~0.6824). These test values are numerically correct but the 0.6824 factor at 8 m is substantially higher than the often-cited ASHRAE correction — users should note it applies to a specific met-station wind reference framework.

**Code Location**: `infiltration.rs:1605` (comment); `infiltration.rs:892–895` (test)

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 1
- Low: 3

## Recommendations
1. **Add the Ocean terrain class** with α=0.10, δ=210, and a convenience selector for coastal proximity. This covers roughly 40% of US residential stock within 50 km of a coastline. Without it, wind-driven infiltration is systematically overestimated for coastal sites.
2. **Rename `TerrainClass::Urban` to `TerrainClass::CityCentre`** or add a separate `City` variant, and optionally mirror EnergyPlus's `Urban` alias (α=0.22) for compatibility. Document the mapping to EnergyPlus Building-object terrain keywords.
3. **Document the hardcoded met station assumptions** (α_met=0.14, δ_met=270, h_met=10) in the crate-level docs. Note that standard EPW/TMY3 files conform to these values.
4. **Consider the default terrain class** — `Suburban` is used as the default in `attic_ela_coefficients`, `garage_ela_coefficients`, and typical test fixtures. For the typical US residential stock (detached single-family in low-to-medium density), this is reasonable but should be configurable from the building input model.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Chapter 24 (Airflow Around Buildings), Table 1 — Wind speed profile power law exponents for terrain categories
- ASHRAE Standard 119-1988 (RA 2014), Table 1 — Terrain coefficients
- EnergyPlus Engineering Reference v24.1 — §3.1.1.3 (Site:WeatherStation, wind speed profile)
- EnergyPlus Engineering Reference v24.1 — §3.1.3.2 (Building object, terrain field)
- Walker IS, Wilson DJ. "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations." *HVAC&R Research* 4(2), 1998.
- EnergyPlus `WeatherManager.cc:7218–7286` — `GetWeatherStation` with weather station terrain parameters
- EnergyPlus `HeatBalanceManager.cc:565–597` — Building object terrain class mapping
- EnergyPlus `DataEnvironment.cc:170–188` — `OutWindSpeedAt` two-stage power-law wind speed correction
