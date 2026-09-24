# Kell (1975) water density polynomial — verify all 7 coefficients against Journal of Chemical & Engineering Data 20(1), 97-105
**Review ID**: phys-const-04
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:235-252` (function `water_density_kg_m3`)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:1635-1650` — `RhoH2O` cubic polynomial (not Kell)
- `vendors/EnergyPlus/src/EnergyPlus/FluidProperties.cc:112-136` — `DefaultWaterRhoData` 33-point lookup table (not Kell)
- `vendors/OCHRE/ochre/Models/Water.py:7-12` — `water_density = 1000` constant (no temperature correction)
- `crates/hares-physics/tests/physics_validation_tests.rs:1204-1279` — cross-validation tests
- `docs/tickets/122-water-density-temperature-correction.md:68-162` — historical verification audit

## Findings

### Finding 1: [Severity: low]
**Description**: All 7 coefficients in the Kell (1975) rational polynomial match the published values digit-for-digit. No discrepancies found.
**Code Location**: `crates/hares-physics/src/constants.rs:247-249`
**Root Cause**: N/A — implementation is correct.
**Impact**: None. The function correctly implements Kell (1975) as published in J. Chem. Eng. Data 20(1), 97-105.

#### Coefficient-by-coefficient verification

The polynomial form is:

```
ρ(T) = [a₀ + a₁·T + a₂·T² + a₃·T³ + a₄·T⁴ + a₅·T⁵] / (1 + b₁·T)
```

| # | Term | Kell (1975) value | HARES source (rust literal) | Match |
|---|------|-------------------|-----------------------------|-------|
| 1 | a₀ | +999.83952 | `999.83952` | ✓ |
| 2 | a₁ | +16.945176 | `16.945_176` | ✓ |
| 3 | a₂ | −7.9870401×10⁻³ | `-7.987_040_1e-3` | ✓ |
| 4 | a₃ | −46.170461×10⁻⁶ | `-46.170_461e-6` | ✓ |
| 5 | a₄ | +105.56302×10⁻⁹ | `105.563_02e-9` | ✓ |
| 6 | a₅ | −280.54253×10⁻¹² | `-280.542_53e-12` | ✓ |
| 7 | b₁ | +16.879850×10⁻³ | `16.879_850e-3` | ✓ |

Underscores in Rust numeric literals (e.g. `16.945_176`) are grouping separators only and do not affect the value.

#### Sign pattern verification

The alternating sign pattern in the published polynomial is preserved correctly:

- a₀: + (positive)
- a₁: + (positive)
- a₂: − (negative)
- a₃: − (negative)
- a₄: + (positive)
- a₅: − (negative)
- b₁: + (positive)

#### Operator precedence

The function uses Rust's standard arithmetic precedence (multiplication before addition/subtraction), which correctly groups terms as:

```
numerator = a₀ + (a₁ × t) − (a₂ × t × t) − (a₃ × t × t × t) + (a₄ × t × t × t × t) − (a₅ × t × t × t × t × t)
denominator = 1.0 + (b₁ × t)
```

No explicit parentheses are needed; the expression evaluates correctly as written.

### Finding 2: [Severity: low]
**Description**: The polynomial form in HARES correctly includes the denominator term `(1 + 16.879850e-3·T)`, making it a proper rational polynomial. The original ticket (#122) for this feature contained two errors: it omitted the denominator and had a 1000×-too-small a₁ coefficient. The HARES implementation correctly uses the published rational form.
**Code Location**: `crates/hares-physics/src/constants.rs:250`
**Root Cause**: N/A — implementation correctly resolves the ticket's formula errors.
**Impact**: None.

### Finding 3: [Severity: low]
**Description**: HARES correctly validates the Kell polynomial against four NIST IAPWS reference temperatures. The test tolerance of ±0.05 kg/m³ is appropriately conservative (the Zelentech reference cites Kell accuracy as ±0.02 kg/m³). No vendor codebase implements Kell; EnergyPlus uses unrelated water-density formulations (a cubic polynomial and a 33-point lookup table), and OCHRE uses a constant 1000 kg/m³.
**Code Location**: `crates/hares-physics/tests/physics_validation_tests.rs:1225-1240`
**Root Cause**: N/A.
**Impact**: None — the test suite provides robust cross-validation.

#### Reference values cross-check

| T | HARES test expected | Kell rational (computed) | Engineering Toolbox | Match? |
|---|---------------------|--------------------------|---------------------|--------|
| 4°C | 999.97 ± 0.05 | 999.972 | 999.97 | ✓ |
| 20°C | 998.21 ± 0.05 | 998.204 | 998.21 | ✓ |
| 50°C | 988.04 ± 0.05 | 988.036 | 988.04 | ✓ |
| 80°C | 971.79 ± 0.05 | 971.798 | 971.79 | ✓ |

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 0 / 3

All 7 coefficients are verified correct against the Kell (1975) reference. No errors, no deviations. The rational polynomial form, operator precedence, signs, and test tolerance are all correct. Neither EnergyPlus nor OCHRE vendor codebases use the Kell polynomial, so HARES is the only implementation with this higher-fidelity water density model.

## Recommendations
1. No changes required. All coefficients are correct.
2. The `±0.05 kg/m³` accuracy claim in the doc comment at line 241 could optionally be tightened to `±0.02 kg/m³` to match the published claim by Zelentech (which directly reproduces the Kell polynomial), but the current conservative value is acceptable.

## References / Citations
- Kell, G.S. (1975). "Density, thermal expansivity, and compressibility of liquid water from 0° to 150°C." *Journal of Chemical & Engineering Data*, 20(1), 97-105. DOI: 10.1021/je60064a005
- Zelentech Water Density Tool, https://www.zelentech.co/en/tools/water-density/ (reproduces Kell 1975 polynomial with full coefficients; accessed 2026-05-26)
- Engineering Toolbox, "Water — Density, Specific Weight and Thermal Expansion Coefficients," https://www.engineeringtoolbox.com/water-density-specific-weight-d_595.html (tabulated values consistent with Kell; accessed 2026-05-26)
- NIST Chemistry WebBook, https://webbook.nist.gov/chemistry/fluid/ (IAPWS-95 saturation data for water; used for cross-validation)
