# Solar position algorithm (altitude/azimuth) — verify against NOAA Solar Calculator or pvlib-python at multiple lat/lon/time grids across seasons; check equinox/solstice
**Review ID**: solar-deep-01
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/solar.rs:88-147

## Vendor/Reference Files Consulted
- vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4114–4325 (CalculateDailySolarCoeffs, CalculateSunDirectionCosines, DetermineSunUpDown)
- vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:9451–9547 (SUN3, SUN4)
- vendors/OCHRE/ochre/utils/envelope.py:137–185 (calculate_solar_irradiance via pvlib NREL SPA)
- pvlib-python 0.15.1: solarposition.py, spa.py

## Verification Methodology
A Python verification script (`docs/reviews/solar-deep/verify_solar_position.py`) compared the HARES Spencer (1971) solar position against pvlib's NREL Solar Position Algorithm (SPA; Reda & Andreas 2008) across a 4×3×4×24 grid:
- 4 latitudes: −40°, 0°, 40°, 60°
- 3 longitudes: −120°, 0°, 120°
- 4 seasonal dates: 2024-03-20 (equinox), 2024-06-20 (solstice), 2024-09-22 (equinox), 2024-12-21 (solstice)
- 24 UTC hours per date
- Total: 1152 sample points

HARES uses the Spencer (1971) 7-term Fourier series for solar declination and equation of time (EOT), with the known EOT_C0 misprint in Spencer's original paper corrected from 0.000075 to 0.0000075 (as documented in pvlib and the HARES source comment at line 66–68). HARES additionally applies an intra-day fractional correction to the day-angle γ (line 101–102) that the base Spencer day-of-year model does not.

The reference against pvlib involves two distinct comparisons:
1. Implementation fidelity: HARES Spencer constants vs pvlib `declination_spencer71()` and `equation_of_time_spencer71()` to confirm the Rust port is numerically correct.
2. Accuracy: HARES altitude/azimuth output vs NREL SPA (`pvlib.solarposition.get_solarposition(method='nrel_numpy')`), which achieves ±0.0003° accuracy (Reda & Andreas, 2008).

NREL SPA geocentric declination was extracted via `spa_module.solar_position_numpy(..., sst=True)`.

## Findings

### Finding 1: HARES Spencer implementation is numerically correct [Severity: low]
**Description**: At solar noon (UTC hour 12, where `minutes_utc = SOLAR_NOON_MINUTES = 720`), HARES' declination and EOT match pvlib's Spencer functions to within floating-point precision (declination difference < 1e-14°, EOT difference < 0.0001 min). At other hours, HARES diverges from the pvlib day-of-year-only reference by up to 0.20° in declination because HARES applies an intra-day time-of-day adjustment to the gamma angle (`gamma = 2π/365 * (day-1 + (minutes_utc-720)/1440)` at line 101–102). This is by design, not an error — HARES provides a continuous intra-day solar declination evolution rather than a daily-constant approximation.

**Code Location**: solar.rs:101–102
**Root Cause**: Architectural choice to use intraday gamma fraction. pvlib's `declination_spencer71(day_of_year)` uses only the integer day number.
**Impact**: Negligible. The intra-day adjustment is physically motivated and brings HARES closer to continuous astronomical truth than the daily-constant Spencer model. The comparison to NREL SPA (below) confirms that HARES altitude errors are small.

### Finding 2: Declination errors vs NREL SPA peak at equinoxes, nearly zero at solstices [Severity: low]
**Description**: HARES Spencer declination differs from NREL SPA geocentric declination by at most 0.237° across the 1152-point grid. Errors are seasonally structured:

| Season         | Mean abs error | Max abs error |
|----------------|---------------|---------------|
| March equinox  | 0.212°        | 0.213°        |
| June solstice  | 0.014°        | 0.016°        |
| Sept equinox   | 0.237°        | 0.237°        |
| Dec solstice   | 0.012°        | 0.014°        |

**Code Location**: solar.rs:105–109 (Spencer declination Fourier series)
**Root Cause**: Spencer's 7-term Fourier series approximates the solar declination with coefficients fitted to 1950s astronomical data. The series is most accurate at the solstices (where declination is near its extremes ±23.44° and the derivative is near zero) and least accurate near the equinoxes (where declination crosses zero and the derivative is maximum). This is a known limitation of the Spencer (1971) model.
**Impact**: At the equinoxes, a ~0.24° declination error propagates directly to a ~0.24° altitude error at solar noon (d(altitude_noon)/d(δ) ≈ 1 for mid-latitudes). For building energy simulation this is acceptable — the EnergyPlus 9-term Fourier equivalent has similar equinox errors. The error is well within the ±0.5° tolerance typical for hourly building energy models (ASHRAE Standard 140).

### Finding 3: Equation of time errors reach 0.57 minutes at March equinox [Severity: low]
**Description**: HARES Spencer EOT differs from NREL SPA EOT by up to 0.57 minutes, with a seasonal pattern:

| Season         | Mean abs error | Max abs error |
|----------------|---------------|---------------|
| March equinox  | 0.568 min     | 0.573 min     |
| June solstice  | 0.359 min     | 0.360 min     |
| Sept equinox   | 0.149 min     | 0.156 min     |
| Dec solstice   | 0.023 min     | 0.038 min     |

**Code Location**: solar.rs:111–116 (Spencer EOT Fourier series)
**Root Cause**: Same as Finding 2 — Spencer's Fourier coefficients approximate the equation of time with residual errors that are largest near the March equinox. An EOT error of 0.57 minutes translates to a 0.14° hour-angle error (0.57 min × 0.25°/min). At mid-latitudes away from noon, sin(H)·dH error propagates at a rate dependent on geometry but typically contributes < 0.05° to the altitude error.
**Impact**: Low. The combined effect of declination error and EOT error produces a total altitude RMSE of 0.13° against NREL SPA. The EOT error is the smaller contributor compared to the declination error at equinox.

### Finding 4: Altitude accuracy within 0.26° across all tested conditions [Severity: low]
**Description**: HARES geometric altitude (no atmospheric refraction) compared to NREL SPA geometric altitude across the full 1152-point grid:

| Metric               | Value   |
|----------------------|---------|
| Mean bias (H-SPA)    | +0.005° |
| Mean absolute error  | 0.097°  |
| Maximum absolute error| 0.257°  |
| RMSE                 | 0.130°  |
| P95 absolute error   | 0.248°  |

All 10 worst-case altitude errors occur at lat = −40° on March 20 (equinox), where the Spencer declination error is largest. The peak error of 0.257° is less than half the 0.5° threshold accepted in building energy simulation standards.

**Code Location**: solar.rs:129–133 (altitude = 90° − arccos(cos_zenith))
**Root Cause**: Combined effect of Spencer declination error (Finding 2) and EOT-derived hour-angle error (Finding 3), with declination error dominating at the equinoxes.
**Impact**: Low. Altitude errors < 0.3° are negligible for building-scale irradiance calculations. This is comparable to EnergyPlus's internal Fourier-based solar position (documented accuracy ~0.1–0.5° for hourly angles) and substantially below the conservative 1.5° tolerance used in the existing cross-check test at solar.rs:1268–1271.

### Finding 5: Azimuth errors up to 7° at geometric singularities (zenith/nadir); < 0.3° elsewhere [Severity: low]
**Description**: Azimuth comparison between HARES and NREL SPA yields:

| Condition                           | Mean abs error | Max abs error |
|-------------------------------------|---------------|---------------|
| All 1152 samples                    | 0.196°        | 7.078°        |
| Non-equatorial latitudes only       | 0.099°        | 0.309°        |
| Sun > 40° altitude (SPA)            | 0.377°        | 7.070°        |
| P95 (all samples)                   | 0.402°        | —             |

The 7° maximum azimuth errors occur exclusively at two geometric singularities:
1. Sun near **nadir** (altitude ≈ −88°): When the sun is far below the horizon, azimuth is poorly defined and small angle differences produce large azimuth deltas. This affects zero simulation results (no direct beam reaches surfaces when the sun is that far below the horizon).
2. Sun near **zenith** at the equator during equinox: At lat = 0° with declination ≈ 0°, the sun passes directly overhead at solar noon. The atan2-based azimuth formula in HARES (line 138–139) and the SPA's azimuth formula both become singular at the zenith. Slight differences in the exact declination value shift the singularity point, producing a large apparent error that is geometrically meaningless.

Excluding these physically-irrelevant singularities, HARES azimuth accuracy is < 0.3°.

**Code Location**: solar.rs:135–140
**Root Cause**: Geometric singularity — azimuth is undefined when the solar vector is aligned with the local vertical (zenith or nadir). This is not an algorithm defect but a fundamental property of the spherical coordinate system.
**Impact**: Negligible. The large errors all occur when irradiance on tilted surfaces is zero or dominated by diffuse sky radiation where azimuth matters minimally. For surfaces that receive direct beam (sun above horizon, away from zenith), the azimuth error is ≤ 0.3°.

### Finding 6: No atmospheric refraction — HARES reports geometric altitude only [Severity: medium]
**Description**: HARES computes geometric (unrefracted) solar altitude using pure astronomical geometry. It does not apply an atmospheric refraction correction. NREL SPA provides both geometric elevation and apparent (refraction-corrected) elevation. The refraction correction from pvlib SPA shows:

| Altitude range    | Mean refraction | Max refraction |
|-------------------|----------------|----------------|
| alt < 5°          | 0.033°         | 0.605°         |
| 5° ≤ alt < 15°    | 0.093°         | 0.156°         |
| alt ≥ 15°         | 0.024°         | 0.061°         |

**Code Location**: solar.rs:88–146 (entire solar_position function — no refraction step)
**Root Cause**: HARES models the geometric sun position only. Atmospheric refraction bends sunlight near the horizon, making the sun appear up to 0.6° higher than its geometric position at 0° geometric altitude.
**Impact**: At sunrise/sunset, the geometric altitude underestimates the apparent solar elevation by 0.3–0.6°. This shifts the perceived sun-up/sun-down boundary by approximately 2–4 minutes at mid-latitudes. During these transition minutes, direct irradiance is already very low (near-zero at the horizon due to airmass extinction), so the practical effect on daily integrated irradiance is small. However, for simulations that depend on precise sunrise/sunset timing (e.g., lighting controls, blind automation), this systematic offset should be noted.

EnergyPlus also does not apply atmospheric refraction in its standard solar position calculation — it uses geometric zenith only and applies a sun-up threshold of cos(zenith) ≥ 0.00001 (DataEnvironment.hh:81), effectively treating the sun as up when the geometric center is at the horizon. OCHRE/pvlib computes both geometric and apparent positions by default.

### Finding 7: DAYS_PER_YEAR = 365.0 means no leap-year compensation [Severity: low]
**Description**: The day-angle computation uses `DAYS_PER_YEAR = 365.0` (solar.rs:74), which matches Spencer's original (1971) formula exactly. During leap years, the ordinal day of year ranges from 1–366, but the denominator remains 365. This creates a ~1/365 (0.27%) phase error in the gamma angle after February 29 of a leap year, equivalent to ~1 day of accumulated seasonal shift.

**Code Location**: solar.rs:74, 101–102
**Root Cause**: Spencer's original formula used 365 days/year. The pvlib `declination_spencer71()` also uses 365. EnergyPlus uses 366 (`X = 0.017167 * DOY` where 0.017167 ≈ 2π/366) for its Fourier series. Neither approach is definitively "correct" — Spencer fitted coefficients assuming a 365-day denominator, so changing it would require re-fitting the Fourier coefficients.
**Impact**: Extremely low. In a leap year, the maximum additional error is ~0.04° in declination on December 31 (the accumulated phase shift of ~1 day in gamma). This is well below the base Spencer model error of ~0.24°. For multi-year simulations, the floating-point representation of time handles leap years correctly; the only effect is a slightly different gamma-phase for dates after February 29.

## Summary
- Total findings: 7
- Critical: 0
- High: 0
- Medium: 1 (Finding 6 — no atmospheric refraction)
- Low: 6

The HARES solar position algorithm implements the Spencer (1971) model correctly with the known C0 misprint corrected. Accuracy against the NREL SPA reference is within 0.26° for altitude and 0.3° for azimuth (outside of geometric singularities), which is well within the ±0.5° tolerance commonly accepted for hourly building energy simulation. The algorithm is comparable in accuracy to EnergyPlus's 9-term ASHRAE/Threlkeld Fourier method and is an appropriate choice for the HARES residential energy simulation use case.

## Recommendations
1. **Document the lack of atmospheric refraction** in the solar position docstring (solar.rs:85–87) and note that sunrise/sunset timing may be offset by ~2–4 minutes relative to observed apparent sunrise. This is consistent with EnergyPlus behavior but users coming from pvlib/OCHRE may expect refraction-corrected values.

2. **Consider adding a sun-up threshold guard** similar to EnergyPlus's `SunIsUpValue = 0.00001` (cos(zenith) ≥ 1e-5) to prevent spurious near-zero negative altitudes from being treated as "sun up" due to floating-point roundoff. Currently, HARES has no explicit sun-up threshold — it returns negative altitudes naturally.

3. **Clarify the Spencer EOT_C0 correction** by adding a citation to pvlib's documentation of the misprint (pvlib-python issue tracker or source comment). The current comment at solar.rs:66–68 is clear but could reference the primary source (Spencer 1971 p.172) for traceability.

4. **The azimuth singularity at zenith/nadir is geometrically unavoidable** but could be guarded with an altitude threshold: when altitude > 89.5° (near zenith), azimuth could be clamped to a sentinel value (e.g., 180°) or returned as NaN to signal "undefined." This would prevent downstream code from using a numerically unstable azimuth value. The EnergyPlus `DetermineSunUpDown` function uses a different azimuth formula (arccos-based) that has the same singularity issue and does not guard against it either.

5. **DAYS_PER_YEAR = 365.0 is consistent with Spencer's original derivation** and should not be changed without also re-fitting the Fourier coefficients. If improved leap-year accuracy is desired in the future, the preferred upgrade path is to the full NREL SPA (as OCHRE does via pvlib) rather than attempting to patch the Spencer model.

## References / Citations
- Spencer, J.W. (1971). "Fourier series representation of the position of the sun." *Search*, 2(5), p.172.
- Reda, I. and Andreas, A. (2008). "Solar Position Algorithm for Solar Radiation Applications." NREL/TP-560-34302. Revised January 2008.
- Duffie, J.A. and Beckman, W.A. (2013). *Solar Engineering of Thermal Processes*, 4th Edition. Wiley.
- pvlib-python 0.15.1: `pvlib.solarposition` module. [https://pvlib-python.readthedocs.io/](https://pvlib-python.readthedocs.io/)
- EnergyPlus 24.1: `WeatherManager.cc`, `SolarShading.cc` (ASHRAE/Threlkeld Fourier solar model)
- OCHRE: `envelope.py` — delegates solar position to pvlib NREL SPA
- NOAA Solar Calculator: [https://gml.noaa.gov/grad/solcalc/](https://gml.noaa.gov/grad/solcalc/)

## Verification Script
`docs/reviews/solar-deep/verify_solar_position.py` — Python script that replicates HARES' Rust solar position in Python and compares against pvlib Spencer and NREL SPA across the full lat/lon/time grid. Reproducible with `python3 verify_solar_position.py` (requires pvlib ≥ 0.15).
