# CFM_TO_M3_S (0.0004719474432) — verify 1 ft³/60s derivation
**Review ID**: phys-const-11
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:130`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/OutputReportTabular.cc:19109` — uses `2118.6438` as m³/s→ft³/min multiplier
- `vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXAshrae205Performance.cc:192` — uses `2118.88` as mass-flow→CFM conversion constant
- `vendors/OCHRE/ochre/utils/units.py:27` — uses Pint `convert(1, "cubic_feet/min", "m^3/s")` for runtime conversion

## Derivation Verification

The constant is derived from the exact definition of the international foot (1959 agreement):

```
1 ft = 0.3048 m              (exact, by international definition)
1 ft³ = 0.3048³ m³
      = 0.028316846592 m³    (exact)
1 CFM = 1 ft³/min
      = 0.028316846592 / 60 m³/s
      = 0.0004719474432 m³/s (exact)
```

Computed with arbitrary-precision decimal arithmetic:

| Step | Value |
|------|-------|
| `0.3048³` | `0.028316846592` |
| `÷ 60` | `0.0004719474432` |
| Constant in code | `0.0004719474432` |
| Difference | `0` (exact match) |

The constant is correct to all 13 significant digits shown.

## Findings

### Finding 1: [Severity: low]
**Description**: EnergyPlus uses a slightly different conversion for m³/s ↔ ft³/min
**Code Location**: Not applicable to HARES; `vendors/EnergyPlus/src/EnergyPlus/OutputReportTabular.cc:19109`
**Root Cause**: EnergyPlus defines `ort->UnitConv(51).mult = 2118.6438` for the m³/s→ft³/min multiplier. The exact value (from 1 / `CFM_TO_M3_S`) is `2118.880003...`. The EnergyPlus value differs by ~0.011%.
**Impact**: When comparing HARES results against EnergyPlus simulations that use tabular unit conversions, airflow values expressed in IP (ft³/min) units will differ by approximately 0.011% — negligible for engineering purposes but worth noting for traceability. The OCHRE Pint-based conversion produces the same exact value as HARES since Pint derives conversions from the same 0.3048 m/ft definition.

## Summary
- **Total findings**: 1
- **Critical / High / Medium / Low**: 0 / 0 / 0 / 1

## Recommendations
1. No change needed. The constant `CFM_TO_M3_S` is mathematically exact and matches the derivation `0.3048³ / 60` to all digits.
2. Consider adding a source comment citing the 1959 international foot definition (0.3048 m/ft exact) as the basis, similar to the NIST citation used for `BTU_PER_HR_PER_W` on line 126.

## References / Citations
- International Yard and Pound Agreement (1959): 1 yd = 0.9144 m (exact) → 1 ft = 0.3048 m (exact)
- NIST Special Publication 811: Guide for the Use of the International System of Units (SI)
- EnergyPlus v24.1.0, `OutputReportTabular.cc:19109` — unit conversion multiplier `2118.6438`
- OCHRE `utils/units.py:27` — Pint-based runtime conversion
