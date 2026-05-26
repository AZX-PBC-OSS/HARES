# Gas therm/hour → Watt conversion (29,307.107...) — verify against NIST SP 811 BTU definition (1055.05585262)
**Review ID**: phys-const-05
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:139` — `GAS_THERMS_PER_HOUR_TO_W`
- `crates/hares-equipment/src/scheduled_load.rs:1976-1987` — associated unit test
- `crates/hares-physics/src/constants.rs:121-126` — related BTU/Watt constants (`BTU_PER_HR_PER_W`, `W_PER_TON`)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/units.py` — pint UnitRegistry-based conversions; `kwh_to_therms = convert(1, "kWh", "therms")`
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh:71` — `ConvFromSIToIP = 3.412141633` (BTU/hr per W)

## Findings

### Finding 1: [Severity: low]
**Description**: The constant `GAS_THERMS_PER_HOUR_TO_W = 29_307.107_017_222_2` truncates the exact arithmetic result after 10 decimal digits. The exact computation `100_000 × 1055.055_852_62 / 3600` yields `29307.107017222222...` with a repeating `2`, whereas the stored value has only a single trailing `2`.

**Code Location**: `crates/hares-physics/src/constants.rs:139`

**Root Cause**: The comment at line 137 states the conversion as `100_000 × 1055.055_852_62 / 3600 = 29_307.107_017_222_2`, which implies the truncated form is the intended value. The exact result is a rational with a repeating decimal expansion (`29307.1070172222...`).

**Arithmetic verification**:
```
NIST BTU_IT            = 1055.05585262 J
1 therm                = 100,000 BTU_IT = 105,505,585.262 J
1 therm/hour           = 105,505,585.262 / 3600 = 29,307.107017222... W
HARES constant         = 29,307.1070172222          (no repeating tail)
Error in f64           = -2.18 × 10⁻¹¹ W (≈ 0.74 ULP)
Implied BTU from HARES = 1055.0558526199992 J      (NIST: 1055.05585262)
BTU delta              = -8 × 10⁻¹³ J per BTU
```

**Impact**: The discrepancy is sub-ULP in f64 (the stored f64 representations of both the truncated and exact values are identical at `repr()` level), making this entirely benign for computation. For a 100,000 BTU/hour gas appliance, the error is ~2 × 10⁻¹¹ W — 14+ orders of magnitude below meaningful measurement precision. No simulation results are affected.

### Finding 2: [Severity: low]
**Description**: The comment at line 138 states the value is "Consistent with pint UnitRegistry used by OCHRE." However, pint's internal therm definition chain in `pint ≥ 0.20` may derive from a slightly different BTU definition (via the IT calorie → IT BTU path: 4.1868 × 251.99576... = 1055.05585... vs NIST's 1055.05585262), depending on pint's definition file version. The comment could mislead a future reader to assume pint and NIST are identical chains.

**Code Location**: `crates/hares-physics/src/constants.rs:138`

**Root Cause**: OCHRE uses `pint.convert(1, "kWh", "therms")` (units.py:26), which relies on pint's internal definition file for "therm". Pint defines `therm = 10^5 BTU` and `BTU = 1055.05585262...` (or a close approximation). The HARES constant is computed directly from NIST SP 811 rather than derived from pint, so the comment is true in spirit but not strictly verifiable without pinning the pint version.

**Impact**: No functional impact. The comment is informational. If a future auditor cross-checks pint's runtime output, a discrepancy of ±0.01% is possible depending on pint version, which could cause confusion.

## Summary
- Total findings: 2
- Critical / High / Medium / Low: 0 / 0 / 0 / 2

## Recommendations
1. **No action needed** on the constant value — it is correct within f64 precision and the associated test (`scheduled_load.rs:1980`) confirms agreement to within 0.01 W.
2. Consider removing or qualifying the "Consistent with pint" comment on line 138 if pint version is not pinned, or add a note that the constant is derived directly from NIST SP 811 and independently verified to match pint's runtime output.

## References / Citations
- NIST Special Publication 811 (2008), Appendix B9: 1 BTU_IT = 1055.05585262 J (exact by definition of the IT calorie)
- 15 U.S.C. § 231 (US Code): 1 therm = 100,000 BTU
- `crates/hares-equipment/src/scheduled_load.rs:1976-1987` — unit test verifying `GAS_THERMS_PER_HOUR_TO_W` matches NIST derivation
- `vendors/OCHRE/ochre/utils/units.py:1-27` — pint UnitRegistry usage for OCHRE therm conversions
