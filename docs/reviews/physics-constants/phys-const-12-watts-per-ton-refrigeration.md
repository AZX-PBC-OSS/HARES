# Watts per ton of refrigeration (3516.852842066667) — verify against ASHRAE definition (1 ton = 12,000 BTU/h)

**Review ID**: phys-const-12
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:123`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXAshrae205Performance.cc:193` — rough W→ton factor (0.00028, not authoritative)
- `vendors/EnergyPlus/src/EnergyPlus/RefrigeratedCase.cc:203` — 15 W/ton ≈ 3516.17 W/ton (close but rounded)
- `vendors/OCHRE/ochre/utils/units.py` — uses Pint `UnitRegistry` (delegates to Pint's definition of `refrigeration_ton`, which computes from `BTU_IT`)
- `vendors/OCHRE/ochre/Equipment/HVAC.py:143` — uses Pint's `refrigeration_ton` unit
- `vendors/OCHRE/ochre/utils/hpxml.py:898` — same, via `convert(capacity, "W", "refrigeration_ton")`
- EnergyPlus `DataConversions.hh` — no direct ton-of-refrigeration constant

## Findings

### Finding 1: [Severity: low] W_PER_TON literal misses nearest f64 by 1 ULP

**Description**: The source literal `3_516.852_842_066_667` (ending in `667`) compiles to IEEE 754 f64 bit pattern `0x40ab79b4a7b721fa`, which is **1 ULP above** the nearest f64 to the true mathematical value. The correct f64 bit pattern is `0x40ab79b4a7b721f9`.

**Code Location**: `crates/hares-physics/src/constants.rs:123`

**Derivation**:
- ASHRAE definition: 1 ton = 12,000 BTU/h
- NIST SP 811: 1 BTU_IT = 1055.05585262 J (exact)
- Conversion: 12,000 × 1055.05585262 / 3,600 = 3516.852842066666… (6 repeating)
- Nearest f64 to true value: `0x40ab79b4a7b721f9` ≈ 3516.8528420666666534…
- HARES literal `3516.852842066667`: `0x40ab79b4a7b721fa` ≈ 3516.8528420666671082…
- Error: +4.55×10⁻¹³ W/ton (1 ULP, relative ~6.5×10⁻¹⁷)

**Root Cause**: The literal was apparently rounded from the repeating decimal 3516.852842066666… at the 14th decimal place (digit 15 = 6 → rounds 14th digit 6 to 7, producing `6667`). However, in the f64 rounding algorithm, the literal `3516.852842066667` lies above the midpoint between the two candidate f64 values `0x40ab79b4a7b721f9` and `0x40ab79b4a7b721fa`, causing the compiler to select the "above" f64 instead of the true nearest.

**Correct literal**: `3_516.852_842_066_666_7` (with the natural repeating-group followed by the rounded terminal digit) — any of `3_516.852_842_066_666_5` through `3_516.852_842_066_666_8` would also produce the correct f64.

**Vendor comparison**:
- **OCHRE**: Uses Pint's `UnitRegistry`, which computes `refrigeration_ton` from the same BTU_IT ↔ J conversion. Pint's internal representation would match the true mathematical value (via its `Quantity` arithmetic).
- **EnergyPlus**: Does not define a named constant for W/ton. The only conversion found is `CoilCoolingDXAshrae205Performance.cc:193` (`capacity_tons = gross_capacity * 0.00028`), which gives ~3571 W/ton — a rough field-specific approximation, not the ASHRAE reference value.

**Impact**: Zero practical impact on building energy simulation (error < 10⁻¹² W even for large systems). This is purely a constant-definition accuracy concern — the stored value does not equal the nearest f64 to the ASHRAE-derived quantity.

## Summary
- Total findings: 1
- Critical / High / Medium / Low: 0 / 0 / 0 / 1

## Recommendations
1. Update the literal from `3_516.852_842_066_667` to `3_516.852_842_066_666_7` to match the nearest f64 to the true value, keeping the existing Rust literal formatting style.

## References / Citations
- ASHRAE Handbook of Fundamentals, 2017, Chapter 1 — 1 ton refrigeration = 12,000 BTU/h
- NIST SP 811, Appendix B — 1 BTU_IT = 1055.05585262 J (exact)
- IEEE 754-2019 — f64 mantissa width (53 bits), round-to-nearest-even
