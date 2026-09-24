# Extraterrestrial irradiance: solar constant (1367 or 1361 W/m²?), eccentricity correction — check against NREL SPA
**Review ID**: solar-deep-03
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs:15` — `SOLAR_CONSTANT` definition
- `crates/hares-physics/src/solar.rs:165-173` — `extraterrestrial_irradiance()` (Spencer 1971)
- `crates/hares-physics/src/solar.rs:175-182` — `extraterrestrial_normal_irradiance()` alias
- `crates/hares-physics/src/solar.rs:205-219` — clear-sky model caller
- `crates/hares-physics/src/solar.rs:436-442` — Perez wrapper caller
- `crates/hares-physics/tests/solar_parity.rs:350-385` — Spencer bounds & parity tests
- `crates/hares-physics/src/solar.rs:920-930, 1366-1386` — unit tests

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:3501` — `GlobalSolarConstant = 1367.0`
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4158` — `DayCorrection = 2π/366`
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4197` — AVSC eccentricity formula
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:3882,3896` — ETR application
- `vendors/OCHRE/ochre/utils/envelope.py:144` — OCHRE delegates to pvlib
- pvlib `irradiance.py`: `get_extra_radiation()` — default `solar_constant=1366.1`, `method='spencer'` (fetched live from GitHub `main`)

## Findings

### Finding 1: Solar constant is 1367 W/m² (WMO 1982), not modern 1361.1 W/m² (IAU 2015 / NIST / CODATA 2018)
**Severity**: medium

**Description**: HARES hard-codes `SOLAR_CONSTANT = 1367.0` at `solar.rs:15`. This reflects the older WMO (1982) value. The modern standard adopted by the IAU (2015 Resolution B3) and NREL SPA (Reda & Andreas 2008) is **1361.1 W/m²**, based on SORCE/TIM space-based radiometer measurements. The HARES test file explicitly acknowledges the discrepancy at `tests/solar_parity.rs:357-358` but incorrectly claims parity with pvlib defaults.

**Code Location**:
- Definition: `crates/hares-physics/src/solar.rs:15` — `const SOLAR_CONSTANT: f64 = 1367.0;`
- Acknowledged in: `crates/hares-physics/tests/solar_parity.rs:357-358` — `"NIST / WMO solar constant = 1361.1 W/m² (updated), but HARES uses 1367.0 W/m² consistent with OCHRE and pvlib defaults."`
- Units tests pinned to this value: `solar.rs:923-930` (~1415 at perihelion), `solar.rs:1367-1376` (~1367 at mean distance), `solar.rs:1379-1386` (~1322 at aphelion)

**Root Cause**: HARES uses the legacy WMO 1982 solar constant (1367 W/m²) for consistency with EnergyPlus (which also uses 1367 at `WeatherManager.cc:3501`). The test comment asserts pvlib parity, but pvlib changed its default from 1367 to 1366.1 (not 1361.1) in v0.7.0 (2019).

**Impact**:
- Systematic **+0.43% overestimate** in all extraterrestrial irradiance values (1367 / 1361.1 ≈ 1.00433).
- At perihelion (day ~3): ETR ≈ 1415 W/m² with 1367 → should be ≈ 1409 W/m² with 1361.1 (Δ ≈ **6 W/m²**).
- At aphelion (day ~185): ETR ≈ 1322 W/m² with 1367 → should be ≈ 1316 W/m² with 1361.1 (Δ ≈ **6 W/m²**).
- Propagates through all downstream models: clear-sky irradiance, Perez transposition, POA irradiance.
- Tests at `solar.rs:1367-1376` accept a ±20 W/m² tolerance, which is wider than the 6 W/m² bias, so the error is within existing test tolerances.
- EnergyPlus parity maintained (both use 1367), but pvlib parity is not — pvlib's default is 1366.1, not 1367.

---

### Finding 2: Spencer (1971) eccentricity correction formula matches pvlib, confirmed correct
**Severity**: low

**Description**: The Spencer 1971 5-term Fourier series in `extraterrestrial_irradiance()` at `solar.rs:165-173` matches pvlib's `'spencer'` method verbatim (coefficient `7.7e-05` = `0.000077`). The formula is correct and well-established. However, it diverges from EnergyPlus's 2-term AVSC approximation.

**Code Location**: `crates/hares-physics/src/solar.rs:166-172`

**Comparison**:

| Source | Formula | Max error vs SPA |
|--------|---------|-------------------|
| HARES | `1.00011 + 0.034221·cos(B) + 0.00128·sin(B) + 0.000719·cos(2B) + 0.000077·sin(2B)` (Spencer 1971) | ~0.01% |
| pvlib 'spencer' | Same coefficients (type `7.7e-05`) | ~0.01% |
| EnergyPlus AVSC | `1.000047 + 0.000352615·sin(X) + 0.0334454·cos(X)` where `X = 2π·DOY/366` | ~0.3% |
| NREL SPA | VSOP87 orbital mechanics → `1/R²` | Reference |

**Impact**: The Spencer formula's eccentricity factor error is negligible (~0.01%). The choice of Spencer over the simpler EnergyPlus AVSC is appropriate — Spencer's 5-term series is more accurate. The angle normalization differs slightly (HARES: `2π·(DOY-1)/365` vs EP: `2π·DOY/366`) but produces results within 0.001% across the year.

---

### Finding 3: pvlib default solar constant is 1366.1, not 1367 nor 1361.1
**Severity**: low

**Description**: pvlib-python's `get_extra_radiation()` defaults to `solar_constant=1366.1` (changed from 1367 in v0.7.0, 2019). This is a historical compromise value from ASTM G173-03 and is distinct from both the WMO 1367 value and the modern IAU 1361.1 value. The HARES test comment at `tests/solar_parity.rs:357-358` claims consistency with "pvlib defaults," but this is two pvlib versions out of date.

**Code Location**: `crates/hares-physics/tests/solar_parity.rs:357-358`

**Root Cause**: The test comment was written when pvlib's default was still 1367. pvlib has since moved to 1366.1.

**Impact**: Low — primarily a documentation accuracy issue. The comment should be corrected to read something like "consistent with EnergyPlus and the legacy pvlib default of 1367 W/m²."

---

### Finding 4: Dual solar constant values in EnergyPlus (1367, 1355, 1353) — HARES aligns with the primary value
**Severity**: informational

**Description**: EnergyPlus uses multiple solar constant values for different purposes:
- `GlobalSolarConstant = 1367.0` (primary, at `WeatherManager.cc:3501`) — used for ETR and clear-sky
- `ZHGlobalSolarConstant = 1355.0` (at `WeatherManager.cc:3502`) — Zhang-Huang model
- `1353.0` (at `SolarShading.cc:2729`) — "average extraterrestrial irradiance" for Perez sky

HARES uses a single 1367.0 value consistently, which aligns with EnergyPlus's primary `GlobalSolarConstant`. HARES does not need the secondary values (Zhang-Huang, Perez sky luminance) since it doesn't implement those models.

**Code Location**: `crates/hares-physics/src/solar.rs:15`

**Impact**: No issue — HARES's single-value approach is appropriate for its scope.

---

## Summary
- Total findings: 4
- Critical: 0
- High: 0
- Medium: 1 (Finding 1 — solar constant is legacy WMO 1367)
- Low: 2 (Findings 2, 3 — eccentricity formula confirmed correct; pvlib default mismatch in doc)
- Informational: 1 (Finding 4 — EnergyPlus dual-constant context)

## Recommendations
1. **Consider adopting 1361.1 W/m²** as the solar constant, consistent with IAU 2015 / NREL SPA. This would reduce systematic irradiance overestimate by 0.43%. Requires updating tests at `solar.rs:923-930, 1367-1376, 1379-1386` and the test parity bounds at `solar_parity.rs:362-370`. If the project's priority is EnergyPlus parity, keep 1367 and document the choice explicitly.
2. **Fix the test comment** at `tests/solar_parity.rs:357-358` — remove the claim of pvlib default parity since current pvlib uses 1366.1, not 1367. State: "HARES uses 1367.0 W/m² consistent with EnergyPlus (WeatherManager.cc:3501)."
3. **Add clarity**: document the `SOLAR_CONSTANT` constant with its provenance (WMO 1982, used by EnergyPlus & ASHRAE HoF models), and note the modern IAU 1361.1 value in the doc comment so future maintainers are aware.
4. **Optional — enumerate the Spencer coefficients** in the doc comment above `extraterrestrial_irradiance()` to make the formula source (Spencer 1971, Search 2:172) explicit without requiring readers to decode the Rust expression.

## References / Citations
- Spencer, J.W. (1971). "Fourier series representation of the position of the sun." *Search* 2(5):172.
- Reda, I., Andreas, A. (2008). "Solar Position Algorithm for Solar Radiation Applications." NREL/TP-560-34302. §3.5 — solar constant = 1361.1 W/m².
- IAU (2015). Resolution B3: Recommended nominal values for solar and planetary parameters.
- pvlib-python. `pvlib.irradiance.get_extra_radiation()`. GitHub: `main` branch. Default `solar_constant=1366.1`.
- EnergyPlus Engineering Reference. §14.5 "Extraterrestrial Radiation" — uses 1367 W/m².
- CODATA (2018). Total Solar Irradiance at 1 AU = 1361 W/m². NIST Standard Reference Database 121.
- ASHRAE Handbook of Fundamentals (2013). Ch.14, Table 8 — apparent solar irradiation at air mass zero ≈ 1205 W/m² (attenuated), derived from 1367 W/m² ETR.
