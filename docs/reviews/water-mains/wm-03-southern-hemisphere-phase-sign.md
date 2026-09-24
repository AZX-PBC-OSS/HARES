# Southern hemisphere phase sign verification: sign=-1 for Northern, +1 for Southern hemisphere
**Review ID**: wm-03
**Category**: water-mains
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/water_mains.rs:102-105

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/schedule.py:235-239
vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:7199-7204

## Findings

### Finding 1: [Severity: low] Formulation convention diverges from EnergyPlus but is mathematically equivalent

**Description**: HARES and OCHRE place the hemisphere sign *inside* the sine argument (`sin(A + sign × 90°)`), while EnergyPlus multiplies the entire sine result by `latitude_sign` (`latitude_sign × sin(A − 90°)`). Both formulations are mathematically equivalent:
- `sin(A + 90°) = −sin(A − 90°) = sin(90° − A)` — verified via the cosine identity `sin(90° − x) = cos(x)` and `sin(90° + x) = cos(x)`.

**Code Location**: `water_mains.rs:102-108` (HARES) vs `WeatherManager.cc:7199-7204` (EnergyPlus) vs `schedule.py:235-239` (OCHRE).

**Root Cause**: HARES followed OCHRE's convention of embedding `sign` inside the angle calculation, which is arguably more readable (a single sine call with the sign baked into the phase shift), compared to EnergyPlus's approach of post-multiplying the sine result.

**Impact**: None — numerical results are identical for equivalent inputs. The choice of convention has no effect on model output. However, anyone cross-referencing the HARES source against the EnergyPlus Engineering Reference's formula presentation may be momentarily confused by the different placement of the sign.

### Finding 2: [Severity: low] Equator boundary handling differs across implementations

**Description**: The three implementations handle the equator (latitude = 0°) differently:
- **EnergyPlus** (`WeatherManager.cc:7199`): `latitude >= 0` → Northern (equator treated as NH)
- **OCHRE** (`schedule.py:235`): `latitude > 0` → Northern, else Southern (equator treated as SH)
- **HARES** (`water_mains.rs:102-105`): explicit `Hemisphere` enum — caller chooses

OCHRE's `latitude > 0` condition means latitude = 0 resolves to Southern Hemisphere (`sign = 1`), which is arguably a bug for equatorial sites where Northern-hemisphere seasonal patterns are more appropriate. EnergyPlus correctly treats the equator as Northern by including the `=`.

**Code Location**: `water_mains.rs:121-125` (Hemisphere enum), `schedule.py:235` (OCHRE), `WeatherManager.cc:7199` (EnergyPlus)

**Root Cause**: Simple off-by-inequality-sign bug in OCHRE that HARES avoids by using an explicit `Hemisphere` enum rather than deriving sign from latitude directly.

**Impact**: Negligible in practice — equatorial sites have near-zero seasonal variation (ΔT ≈ 0), so the hemisphere sign term has no practical effect on the result. The HARES `Hemisphere` enum design is superior because it forces an explicit choice, avoiding automated latitude-based misclassification entirely.

## Summary
- Total findings: 2
- Critical / High / Medium / Low: 0 / 0 / 0 / 2

## Recommendations

1. **No changes required.** The sign convention (`Northern → −1`, `Southern → +1`) matches OCHRE exactly and produces results mathematically equivalent to EnergyPlus. The southern hemisphere test (`southern_hemisphere_peaks_in_northern_winter` at line 365) correctly verifies the phase inversion, confirming peak mains temperature in Southern hemisphere summer (~mid-February, day 40–60).

2. For documentation consistency, consider noting in the `water_mains_temperature_c` doc comment that while EnergyPlus uses `latitude_sign × sin(A − 90)`, HARES and OCHRE use the equivalent `sin(A + sign × 90)` formulation.

## References / Citations

- Burch, J. and Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." Proceedings of the 2007 ASES National Solar Conference.
- EnergyPlus `WeatherManager.cc:7199-7204` — `latitude_sign × sin(0.986 × (DayOfYear − 15 − Lag) − 90)`
- OCHRE `schedule.py:235` — `sign = -1 if location.get("latitude") > 0 else 1`
- HARES `water_mains.rs:102-105` — `Hemisphere::Northern => -1.0, Hemisphere::Southern => 1.0`
- HARES `water_mains.rs:365-383` — `southern_hemisphere_peaks_in_northern_winter` test verifying phase inversion
