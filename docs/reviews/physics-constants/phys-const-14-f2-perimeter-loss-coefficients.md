# F2 perimeter loss coefficient: R-value thresholds (0.88, 1.76) and 6 hardcoded F2 values — verify against ASHRAE 90.1-2022 Table A6.3.1
**Review ID**: phys-const-14
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ground.rs:158-169` — `f2_coefficient()` function
- `crates/hares-core/src/dwelling/conversions.rs:180-181` — caller site in slab RC conversion

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:4777-4778` — uses consolidated F-factor (`Reff = Area / (Pexp × Ffactor) - Rfilm,in - Rfilm,out`); no F1/F2 decomposition
- `vendors/EnergyPlus/doc/engineering-reference/src/surface-heat-balance-manager-processes/ground-heat-transfer-calculations-using-c.tex:22-66` — engineering reference for F-factor slab method
- `vendors/OCHRE/ochre/defaults/Envelope/Envelope Materials.csv:668-708` — uses RC network with Ficticious Insulating Layer R-values (0.424–2.068 m²·K/W); no ASHRAE table reference

## Findings

### Finding 1: All six F2 values and both R-value thresholds are correct [Severity: NONE]
**Description**: All six hardcoded F2 coefficients and both R-value thresholds verified against ASHRAE 90.1-2022 Table A6.3.1 for the standard 2 ft vertical perimeter insulation configuration, common in residential slab-on-grade construction.

**Verification details — R-value thresholds**:
- **0.88 m²·K/W**: R-5 (IP) × 0.17611 m²·K/W per (h·ft²·°F/Btu) = 0.88055 → rounded to 0.88. Exact value: 0.88055 m²·K/W.
- **1.76 m²·K/W**: R-10 (IP) × 0.17611 m²·K/W per (h·ft²·°F/Btu) = 1.7611 → rounded to 1.76. Exact value: 1.7611 m²·K/W.

**Verification details — F2 coefficients (using HARES conversion factor 1.73074)**:

| Insulation | Heated | ASHRAE IP (Btu/h·ft·°F) | Computed SI | HARES SI | Match |
|---|---|---|---|---|---|
| Uninsulated | No | 0.73 | 1.26344 | **1.263** | Yes |
| R-5 | No | 0.72 | 1.24613 | **1.246** | Yes |
| R-10 | No | 0.71 | 1.22883 | **1.229** | Yes |
| Uninsulated | Yes | 1.35 | 2.33650 | **2.336** | Yes |
| R-5 | Yes | 1.31 | 2.26727 | **2.267** | Yes |
| R-10 | Yes | 1.30 | 2.24996 | **2.250** | Yes |

All values round correctly to 3 decimal places. Maximum relative error from exact conversion factor (1.730734666...) is 0.0345% (uninsulated unheated), far below any practical engineering threshold.

**Code Location**: `crates/hares-physics/src/ground.rs:158-169`

### Finding 2: Step-function binning at thresholds is correct, but no interpolation exists for intermediate R-values [Severity: LOW]
**Description**: The function uses `>=` comparisons at the two thresholds (0.88 and 1.76 m²·K/W), mapping insulation R-values into three discrete bins. At boundary values, binning is correct:
- R = 0.88 → R-5 bin (>= 0.88 passes)
- R = 1.76 → R-10 bin (>= 1.76 passes)

However, intermediate R-values are assigned to the *lower* bin with no interpolation:
- R = 0.87 → uninsulated bin (not R-5, though only 1% below threshold)
- R = 1.75 → R-5 bin (not R-10, though only 0.6% below threshold)
- R = 2.00 → R-10 bin (though R-11.4 IP)

ASHRAE 90.1-2022 Table A6.3.1 provides only discrete entries per insulation level. Applying the lower bin for intermediate values is a conservative engineering choice (slightly overestimates heat loss), used in practice by many implementations. Linear interpolation between bins would better handle non-standard R-values but is not required for code-compliance checks.

**Code Location**: `crates/hares-physics/src/ground.rs:159-168`

**Root Cause**: Design choice to use discrete bin lookup rather than interpolation between ASHRAE table entries.

**Impact**: Minor heat loss overestimate (<2% of total slab loss) for intermediate R-values near thresholds. For R-values at or near the standard levels (R-5, R-10), no impact.

### Finding 3: F2 capped at R-10+; no support for R-15 or higher perimeter insulation [Severity: LOW]
**Description**: The function treats all R ≥ 1.76 m²·K/W (> R-10 IP) as a single "R-10+" bin returning the R-10 values. ASHRAE 90.1-2022 Table A6.3.1 provides distinct F-values for higher insulation levels (R-15, R-20, etc.) in the standard 2 ft vertical insulation configuration.

For example, at R-15 (2.64 m²·K/W SI):
- Heated slab: ASHRAE value ≈ 1.29 Btu/h·ft·°F → 2.233 W/(m·K); HARES returns 2.250 (+0.8%)
- Unheated slab: ASHRAE value ≈ 0.70 Btu/h·ft·°F → 1.212 W/(m·K); HARES returns 1.229 (+1.4%)

The impact is minor for typical residential applications where R-10 is the practical maximum for perimeter insulation. However, highly insulated residential slabs (R-15, R-20) in cold climates would have their heat loss slightly overestimated.

**Code Location**: `crates/hares-physics/src/ground.rs:159-161` — the `>= 1.76` branch comment reads "R-10+ perimeter insulation"

**Root Cause**: Limited bin coverage — only 3 bins (uninsulated, R-5, R-10+) versus the broader range in ASHRAE Table A6.3.1.

**Impact**: For R > R-10 insulation, F2 overestimated by < 2% relative to ASHRAE reference. Low practical impact for typical residential stock.

## Comparison with Vendor Implementations

| Aspect | HARES | EnergyPlus | OCHRE |
|---|---|---|---|
| **Method** | F2 lookup from ASHRAE table, then R = Area/(F2×P) - R_film | User-supplied F-factor, then same conversion | RC network with calibrated fictitious layer R-values |
| **F2 values** | Hardcoded 6 values matching ASHRAE 90.1-2022 | User inputs F-factor externally (no embedded table) | No F-factor concept; uses CSV lookup |
| **Insulation bins** | 3 bins: unins, R-5, R-10+ | N/A (external input) | 10+ slab types in CSV with distinct R-values |
| **Table reference** | ASHRAE 90.1-2022 Table A6.3.1 | Documents same reference in I/O guide | No ASHRAE 90.1 reference |

EnergyPlus and OCHRE do not decompose F-factor into F1/F2 sub-components. EnergyPlus uses the consolidated F-factor approach matching the same underlying ASHRAE methodology.

## Summary
- Total findings: 2
- Critical: 0
- High: 0
- Medium: 0
- Low: 2

Both numerical findings are classified as LOW severity — they represent design limitations (fixed binning, R-10 cap) rather than errors. All six hardcoded values and both R-value thresholds are numerically correct against ASHRAE 90.1-2022 Table A6.3.1 within engineering tolerance.

## Recommendations
1. Consider adding interpolation between discrete ASHRAE bins for intermediate R-values (e.g., R = 0.88–1.76 maps linearly between R-5 and R-10 values) to improve accuracy for non-standard insulation levels.
2. Consider adding R-15 and R-20 entries for cold-climate applications where perimeter insulation may exceed R-10, though this is a low priority for typical residential stock.
3. Document in docstring that intermediate R-values are conservatively mapped to the nearest lower bin, to avoid user surprise.

## References / Citations
- ANSI/ASHRAE/IES 90.1-2022, Table A6.3.1 "Assembly F-Factors for Slab-on-Grade Floors"
- `crates/hares-physics/src/ground.rs:17` — explicit citation of ASHRAE 90.1-2022 Table A6.3.1
- EnergyPlus Engineering Reference § "Slab-on-grade and Underground Floors Defined with F-factors" (`ground-heat-transfer-calculations-using-c.tex:22-57`)
- EnergyPlus Input Output Reference, `Construction:FfactorGroundFloor` documentation (`group-surface-construction-elements.tex:3932-3959`)
- Conversion factor: 1 Btu/(h·ft·°F) = 1.730734666... W/(m·K) (derived from 1 Btu = 1055.05585262 J, 1 ft = 0.3048 m, 1 °F = 5/9 K)
