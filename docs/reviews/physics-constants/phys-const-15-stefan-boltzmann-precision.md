# Stefan-Boltzmann constant precision — verify f64 mantissa preserves full CODATA 2018 precision
**Review ID**: phys-const-15
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:202`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DataGlobalConstants.hh:618` — central `StefanBoltzmann` constant (and 11 other local/nested copies across the codebase)
- `vendors/OCHRE/ochre/Models/Envelope.py:238` — inline `5.6704e-8`
- `vendors/OCHRE/ochre/utils/schedule.py:182` — inline `5.6697e-8`

## Findings

### Finding 1: [Severity: low]
**Description**: f64 mantissa fully preserves CODATA 2018 precision. The constant `5.670_374_419e-8` has 10 significant decimal digits, requiring ~33 bits of mantissa. The f64 53-bit mantissa provides ~15.95 decimal digits of precision — over 5 extra guard digits beyond the CODATA value. The ULP (unit in last place) for values near 5.67×10⁻⁸ is ≈6.62×10⁻²⁴, while the last significant digit's place value is 1×10⁻¹⁸. The representational error (≤0.5 ULP ≈ 3.3×10⁻²⁴) is therefore ~6 orders of magnitude smaller than the precision floor of CODATA 2018. The constant is stored without any loss of significant digits.

**Code Location**: `crates/hares-physics/src/constants.rs:202`
**Root Cause**: N/A — this is a confirmation, not a defect.
**Impact**: None. The constant is correct and complete.

### Finding 2: [Severity: medium]
**Description**: HARES constant materially exceeds vendor precision. EnergyPlus uses `5.6697×10⁻⁸` (CODATA ~2010, 5 significant figures), a deviation of −6.74×10⁻¹² from CODATA 2018. OCHRE uses two inconsistent inline literals with no named constant: `5.6704×10⁻⁸` in Envelope.py (deviation +2.56×10⁻¹³) and `5.6697×10⁻⁸` in schedule.py (deviation −6.74×10⁻¹²). A single 1 K temperature error at 300 K translates to a radiative flux error of roughly 4σT³ΔT ≈ 6×10⁻³ W/m² for the EnergyPlus constant versus ~3×10⁻¹¹ W/m² for the HARES constant — negligible for building simulation in both cases, but HARES uses the authoritative source value.

**Code Location**:
- HARES: `crates/hares-physics/src/constants.rs:202`
- EnergyPlus: `vendors/EnergyPlus/src/EnergyPlus/DataGlobalConstants.hh:618`
- OCHRE: `vendors/OCHRE/ochre/Models/Envelope.py:238` and `vendors/OCHRE/ochre/utils/schedule.py:182`
**Root Cause**: Vendors use pre-2014 CODATA or truncated values. HARES uses CODATA 2018 directly.
**Impact**: Low practical impact on simulation results (<0.12% radiative flux difference between HARES and EnergyPlus). The primary benefit is traceability to the authoritative source, not numerical accuracy.

### Finding 3: [Severity: low]
**Description**: The unit test at line 282–284 is tautological. `STEFAN_BOLTZMANN` and `sigma_nist` are both the same literal `5.670_374_419e-8` (Rust underscores are visual separators only). The assertion `(STEFAN_BOLTZMANN - sigma_nist).abs() < 1e-18` is always `0.0 < 1e-18` — it verifies that the compiler parses the same token consistently, not that the constant matches CODATA 2018 independently. The test provides no external validation.

**Code Location**: `crates/hares-physics/src/constants.rs:278-285`
**Root Cause**: The test uses the same decimal literal for both the constant and the expected value, making the comparison self-referential.
**Impact**: The test cannot catch transcription errors (e.g., if a digit were mistyped in both places identically). Since both are the same source token, they will always match. The true CODATA value is independently confirmed by the numerical analysis in Finding 1.

## Summary
- **Total findings**: 3
- **Critical**: 0
- **High**: 0
- **Medium**: 1 (vendor precision gap)
- **Low**: 2 (f64 precision confirmation, tautological test)

## Recommendations
1. The f64 type is adequate for this constant; no change to the type is warranted.
2. Consider replacing the tautological test with an independent verification — e.g., assert against a hardcoded integer representation (`STEFAN_BOLTZMANN.to_bits() == N`) derived offline from the IEEE 754 encoding of the CODATA 2018 value. This would catch transcription errors that the current test cannot.
3. No change to the constant value itself is needed; it is the most precise among all three codebases reviewed.

## References / Citations
- NIST CODATA 2018: σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴
- IEEE 754-2019 binary64: 53-bit mantissa, ≈15.95 decimal digits of precision
- EnergyPlus Engineering Reference: historical use of 5.6697×10⁻⁸ (pre-2014 CODATA)
- OCHRE Envelope.py / schedule.py: inconsistent inline literals with no named constant
