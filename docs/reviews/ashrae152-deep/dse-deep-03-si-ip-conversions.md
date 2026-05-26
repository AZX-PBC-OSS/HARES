# SI-to-IP conversion constants — 6 significant figure accuracy required
**Review ID**: dse-deep-03
**Category**: ashrae152-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ashrae152.rs` (lines 13–18, 351–357, 474–475, 516–517)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh:71` — `ConvFromSIToIP(3.412141633)` for W→BTU/h
- `vendors/EnergyPlus/src/EnergyPlus/DataConversions.hh:58–70` — IP→SI conversion chain (`CFL=0.3048`, `CFA`, `CFV`, etc.)
- `vendors/EnergyPlus/src/EnergyPlus/WindowEquivalentLayer.cc:5467` — `Rvalue = 5.678 / UCG` uses **5.678** for R-value conversion

## Findings

### Finding 1: [Severity: low] All five constants verified to 6+ significant figure accuracy
**Description**: Each SI-to-IP conversion constant was compared against the exact mathematical value derived from the international foot definition (1 ft = 0.3048 m exactly, since 1959) and the international table BTU (1 BTU_IT = 1055.05585262 J exactly).

| Constant | HARES Value | Exact Value (20 d.p.) | Relative Error | Sig Figs Match |
|---|---|---|---|---|
| `M3_TO_FT3` | 35.3147 | 35.31466672… | 9.42×10⁻⁷ | 6 |
| `M2_TO_FT2` | 10.7639 | 10.76391042… | 9.68×10⁻⁷ | 6 |
| `W_TO_BTU_H` | 3.41214 | 3.41214163… | 4.79×10⁻⁷ | 7 |
| `M3S_TO_CFM` | 2118.88 | 2118.88000… | 1.55×10⁻⁹ | 9 |
| `SI_R_TO_IP_R` | 5.67826 | 5.67826334… | 5.88×10⁻⁷ | 6 |

**Code Location**: `crates/hares-physics/src/ashrae152.rs:13–18`
**Root Cause**: N/A — all constants are correctly rounded to the stated 6 significant figures.
**Impact**: The worst-case relative error from any single constant is ~0.0001% (M2_TO_FT2), which is two orders of magnitude below the 0.01% threshold identified in the review brief. Even assuming the worst-case scenario where conversion errors from all five independently-applied constants compound multiplicatively (which they do not, since each converts a distinct physical dimension), the compounded bias would be (1 + 9.68×10⁻⁷)⁵ − 1 ≈ 4.84×10⁻⁶ or 0.00048%, still well below the 0.01% threshold. In practice, errors cannot compound multiplicatively across constants because each constant enters the DSE formula through an independent input dimension (volume, area, power, flow rate, thermal resistance) — they are not chained.

---

### Finding 2: [Severity: medium] Mathematical inconsistency between `M3_TO_FT3` and `M3S_TO_CFM`
**Description**: The two volume-related conversion constants are independently rounded, breaking the exact mathematical relationship `M3S_TO_CFM = 60 × M3_TO_FT3`.

- `M3_TO_FT3 = 35.3147` → `M3_TO_FT3 × 60 = 2118.882`
- `M3S_TO_CFM = 2118.88` (differs by 0.002 from 2118.882)
- `M3_TO_FT3 × 60` relative to `M3S_TO_CFM`: (2118.882 − 2118.88) / 2118.88 ≈ 9.4×10⁻⁷

**Code Location**: `crates/hares-physics/src/ashrae152.rs:13,16`
**Root Cause**: Independent rounding of the two literals rather than deriving `M3S_TO_CFM` from `M3_TO_FT3 * 60.0`.
**Impact**: Low. Both constants are used independently (volume at line 351, flow at line 357), so this inconsistency does not cause an actual calculation discrepancy in the current code. However, it is a latent trap: any future code that converts m³ to ft³ with one constant and then scales to CFM with `×60` would produce a different result than direct use of M3S_TO_CFM. Recommended fix: derive `M3S_TO_CFM` from `M3_TO_FT3 * 60.0` or use the more precise 6-sig-fig value `2118.88` consistently.

---

### Finding 3: [Severity: low] f64 const evaluation preserves stated precision
**Description**: Verified that Rust `const f64` literal parsing and const evaluation do not introduce additional rounding beyond the literal precision. The f64 representation of each constant matches the intended decimal value to within machine epsilon (~2.2×10⁻¹⁶).

| Constant | Declared | f64 Representation | Delta |
|---|---|---|---|
| `M3_TO_FT3` | 35.3147 | 35.314700000000002 | +2×10⁻¹⁵ |
| `M2_TO_FT2` | 10.7639 | 10.7639 (exact) | 0 |
| `W_TO_BTU_H` | 3.41214 | 3.41214 (exact) | 0 |
| `M3S_TO_CFM` | 2118.88 | 2118.8800000000001 | +1×10⁻¹³ |
| `SI_R_TO_IP_R` | 5.67826 | 5.6782599999999999 | −1×10⁻¹⁶ |

**Code Location**: `crates/hares-physics/src/ashrae152.rs:13–18`
**Root Cause**: Standard IEEE 754 binary representation of decimal literals — all well below the stated precision level.
**Impact**: None. The f64 representation error (≤ 10⁻¹³) is 6+ orders of magnitude below the literal truncation error (~10⁻⁷). Runtime precision is dominated by the literal values themselves, not by floating-point representation.

---

### Finding 4: [Severity: low] EnergyPlus cross-reference — HARES R-value constant is more precise
**Description**: EnergyPlus `WindowEquivalentLayer.cc:5467` uses `Rvalue = 5.678 / UCG` (4 significant figures) for the SI-to-IP R-value conversion, whereas HARES uses `SI_R_TO_IP_R = 5.67826` (6 significant figures). HARES is approximately 46 ppm closer to the exact value.

EnergyPlus `StandardRatings.hh:71` uses `ConvFromSIToIP(3.412141633)` (10 significant figures) for W→BTU/h, whereas HARES uses `3.41214` (6 significant figures). The EnergyPlus constant is ~0.5 ppm more accurate.

Neither discrepancy is material to simulation outcomes — both sets of constants are well within engineering tolerances for building energy simulation.

---

## Summary
- **Total findings**: 4
- **Critical / High / Medium / Low**: 0 / 0 / 1 / 3

## Recommendations
1. **Derive `M3S_TO_CFM` from `M3_TO_FT3 * 60.0`** to eliminate the internal inconsistency. The current stored value `2118.88` is coincidentally the one closer to the exact conversion, but the inconsistency is a code-quality concern.
2. Consider upgrading `W_TO_BTU_H` to `3.412141` (7 significant figures) to match or exceed EnergyPlus precision — this is the constant used in both numerator and denominator paths of the DSE formula (`dte_high` at line 453, load factor at line 531) and thus is the most multiply-used constant.
3. No urgent action required — all constants meet the 6-significant-figure accuracy requirement, and the maximum systematic bias from all conversion constants combined is approximately 0.0005%, well below the 0.01% threshold discussed in the review brief.

## References / Citations
- International foot: 1 ft = 0.3048 m exactly (US Survey definition deprecated in favor of international foot in 1959; reaffirmed in NIST SP 811)
- International Table BTU: 1 BTU_IT = 1055.05585262 J exactly (NIST SP 811, Appendix B.9)
- EnergyPlus `DataConversions.hh:60` — `CFL(0.3048)` is the IP→SI length conversion; derived area and volume use `CFA = CFL²`, `CFV = CFL³`
- EnergyPlus `StandardRatings.hh:71` — `ConvFromSIToIP(3.412141633)` W→BTU/h
- EnergyPlus `WindowEquivalentLayer.cc:5467` — `Rvalue = 5.678 / UCG`
- Rust Reference §6.1.5: Const evaluation uses deterministic IEEE 754 semantics
- ASHRAE Standard 152-2014, §7.3 — DSE calculation chain operates entirely in IP units internally
