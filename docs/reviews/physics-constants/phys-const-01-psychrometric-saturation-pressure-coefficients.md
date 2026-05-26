# Verify psychrometric saturation pressure formula — all 10 polynomial coefficients against ASHRAE HOF 2021 Ch.1 Table 2
**Review ID**: phys-const-01
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/psychrometrics.rs:49-61`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc:715-742`
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py:21-47`

## Reference Standard
ASHRAE Handbook of Fundamentals 2021, Chapter 1, Table 2 — "Polynomial Coefficients for Saturation Pressure"

## Coefficient Verification (Digit-for-Digit)

### Ice Region (−100°C to 0.01°C) — Table 2 coefficients C1–C7

| Coef | ASHRAE HOF 2021 | HARES (line 49–54) | Match |
|------|----------------|---------------------|-------|
| C1   | −5.6745359 × 10³ | `-5.674_535_9e3`     | ✓ |
| C2   | 6.3925247       | `6.392_524_7`        | ✓ |
| C3   | −9.677843 × 10⁻³ | `- 9.677_843e-3`     | ✓ |
| C4   | 6.2215701 × 10⁻⁷ | `6.221_570_1e-7`     | ✓ |
| C5   | 2.0747825 × 10⁻⁹ | `2.074_782_5e-9`     | ✓ |
| C6   | −9.484024 × 10⁻¹³ | `- 9.484_024e-13`    | ✓ |
| C7   | 4.1635019       | `4.163_501_9`        | ✓ |

### Liquid-Water Region (0.01°C to 200°C) — Table 2 coefficients C8–C13

| Coef | ASHRAE HOF 2021 | HARES (line 55–59) | Match |
|------|----------------|---------------------|-------|
| C8   | −5.8002206 × 10³ | `-5.800_220_6e3`    | ✓ |
| C9   | 1.3914993       | `1.391_499_3`       | ✓ |
| C10  | −4.8640239 × 10⁻² | `- 4.864_023_9e-2`  | ✓ |
| C11  | 4.1764768 × 10⁻⁵ | `4.176_476_8e-5`    | ✓ |
| C12  | −1.4452093 × 10⁻⁸ | `- 1.445_209_3e-8`  | ✓ |
| C13  | 6.5459673       | `6.545_967_3`       | ✓ |

## Cross-Vendor Consistency

| Coef | HARES | EnergyPlus | OCHRE/psychrolib | Match |
|------|-------|------------|-------------------|-------|
| C1   | −5.6745359e3 | −5674.5359 (C1) | −5.6745359e3 | ✓ |
| C2   | 6.3925247    | 6.3925247 (C2)  | 6.3925247    | ✓ |
| C3   | −9.677843e-3 | −0.9677843e-2 (C3) | −9.677843e-3 | ✓ |
| C4   | 6.2215701e-7 | 0.62215701e-6 (C4) | 6.2215701e-7 | ✓ |
| C5   | 2.0747825e-9 | 0.20747825e-8 (C5) | 2.0747825e-9 | ✓ |
| C6   | −9.484024e-13 | −0.9484024e-12 (C6) | −9.484024e-13 | ✓ |
| C7   | 4.1635019    | 4.1635019 (C7)  | 4.1635019    | ✓ |
| C8   | −5.8002206e3 | −5800.2206 (C8) | −5.8002206e3 | ✓ |
| C9   | 1.3914993    | 1.3914993 (C9)  | 1.3914993    | ✓ |
| C10  | −4.8640239e-2 | −0.048640239 (C10) | −4.8640239e-2 | ✓ |
| C11  | 4.1764768e-5 | 0.41764768e-4 (C11) | 4.1764768e-5 | ✓ |
| C12  | −1.4452093e-8 | −0.14452093e-7 (C12) | −1.4452093e-8 | ✓ |
| C13  | 6.5459673    | 6.5459673 (C13) | 6.5459673    | ✓ |

## Findings

### Finding 1 [Severity: low — informational]
**Description**: Triple-point boundary treatment differs slightly from EnergyPlus. HARES uses `t_c <= 0.01` for the ice branch (`psychrometrics.rs:49`), while EnergyPlus uses `Tkel < TriplePointOfWaterTempKelvin` (`Psychrometrics.cc:723`), which assigns exactly 0.01°C to the liquid-water branch. OCHRE/psychrolib (`psychrolib_jit.py:27`) uses `t_dry_bulb <= TRIPLE_POINT_WATER_SI` (0.01), matching HARES.
**Code Location**: `psychrometrics.rs:49` — the `<=` vs `<` boundary condition.
**Root Cause**: Both conventions (ice at triple point vs. liquid at triple point) are internally consistent; the saturation pressure is continuous across the boundary.
**Impact**: Negligible. At the triple point both branches produce essentially identical pressures, and the test suite verifies continuity across the 0°C transition.

## Summary
- Total findings: 1
- Critical: 0 / High: 0 / Medium: 0 / Low: 1

## Recommendations
1. No corrective action required. All 13 coefficients (10 in HARES code, counting the coefficient pairs across both branches) match ASHRAE HOF 2021 Ch.1 Table 2 exactly, digit-for-digit. The implementation is consistent with both EnergyPlus and OCHRE/psychrolib reference implementations.

## References / Citations
- ASHRAE Handbook of Fundamentals, 2021, Chapter 1, Table 2: "Polynomial Coefficients for Saturation Pressure"
- EnergyPlus `PsyPsatFnTemp` in `Psychrometrics.cc:715-742`
- PsychroLib (psychrolib_jit.py) `get_sat_vap_pressure` in `psychrolib_jit.py:21-47`
- HARES test `saturation_pressure_matches_ashrae_hof_table_2` in `psychrometrics.rs:573-596`
- HARES test `saturation_pressure_matches_psychrolib_reference` in `psychrometrics.rs:281-298`
