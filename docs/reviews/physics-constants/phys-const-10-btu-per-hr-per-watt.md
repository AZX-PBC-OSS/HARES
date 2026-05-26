# BTU_PER_HR_PER_W (3.412141633) — verify against NIST exact conversion
**Review ID**: phys-const-10
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:126`
- `crates/hares-io/src/hpxml/resolve_hvac.rs:27,930,975,1061,1146,1147,1313,1314` (usage sites)
- `crates/hares-equipment/tests/si_guard.rs:33` (guard test reference)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh:71` — `ConvFromSIToIP(3.412141633)`
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc:14709` — identical constant
- `vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDX.cc:950,976` — identical constant
- `vendors/EnergyPlus/src/EnergyPlus/HVACVariableRefrigerantFlow.cc:1334,12147` — usage via `vrf.SCHE`
- `vendors/EnergyPlus/src/EnergyPlus/OutputReportTabular.cc:19124` — rounded to `3.412`
- `vendors/OCHRE/ochre/` — searched for BTU/watt conversions; OCHRE uses `pint` unit registry for on-the-fly conversions rather than hard-coded constants

## Findings

### Finding 1: [Severity: medium]
**Description**: The constant `BTU_PER_HR_PER_W = 3.412_141_633` is rounded to 10 significant digits and does **not** match the full f64 representation of the NIST exact conversion ratio `3600 / 1055.05585262`. The value stored in the binary is `3.412141633000000...` rather than the correct `3.412141633127941...`. The comment on line 125 calls this the "NIST exact conversion," which is misleading—it is the NIST *published rounded* value.

**Code Location**: `crates/hares-physics/src/constants.rs:125-126`
```rust
/// BTU/h per Watt. NIST exact conversion.
pub const BTU_PER_HR_PER_W: f64 = 3.412_141_633;
```

**Root Cause**: The literal `3.412141633` has only 9 decimal digits (10 significant digits). The full-f64 representation of the NIST ratio (`3600 / 1055.05585262`) requires the value `3.4121416331279417`. The truncation drops 8 decimal digits of precision, causing an absolute error of `1.28 × 10⁻¹⁰` (relative error `3.75 × 10⁻¹¹`) and a ULP error of ~168,867 — a substantial data loss within f64's representable range.

NIST SP 811 defines 1 BTU_IT = 1055.05585262 J (exact). Therefore:
```
1 W = 1 J/s = 3600 J/h
BTU/h per W = 3600 / 1055.05585262 = 3.4121416331279419...
```

**Impact**:
- For building energy simulation, the relative error of `3.75 × 10⁻¹¹` is negligible in practice. At 100,000 BTU/h the error is ~0.000013 BTU/h.
- The constant is used in HVAC efficiency calculations (`resolve_hvac.rs`) to convert SEER, HSPF, EER ratings into EIR, so the practical impact on simulation results is zero.
- The primary issue is correctness-by-label: the comment claims "NIST exact" but the constant is rounded. This could mislead future maintainers.
- EnergyPlus uses the same rounded value (`3.412141633`) at `StandardRatings.hh:71` and elsewhere, so HARES is consistent with that vendor reference.

**Verification computation**:
```
python3:
  exact = 3600 / 1055.05585262    # = 3.41214163312794172001...
  hares = 3.412141633
  error = exact - hares           # = 1.2794e-10
  rel   = error / exact           # = 3.75e-11
  ulps  = error / (exact * eps)   # ≈ 168,866 ulps (eps = 2.22e-16)
```

## Summary
- Total findings: 1
- Critical / High / Medium / Low: 0 / 0 / 1 / 0

## Recommendations
1. If full-f64 precision is desired, change the literal to:
   ```rust
   pub const BTU_PER_HR_PER_W: f64 = 3.412_141_633_127_942;
   ```
   This is the nearest f64 to `3600 / 1055.05585262` and preserves the maximum available precision. Alternatively, compute it from the NIST BTU definition already present in the same file (line 136, `1055.055_852_62`):
   ```rust
   pub const BTU_PER_HR_PER_W: f64 = 3600.0 / 1_055.055_852_62;
   ```
   This would make the derivation self-documenting and guarantee consistency.
2. Update the comment to note that this is derived from NIST SP 811 (1 BTU_IT = 1055.05585262 J), not an independently published "exact" value.
3. Optionally add a const-eval assertion verifying the round-trip consistency with `GAS_THERMS_PER_HOUR_TO_W` (line 139), which already uses the NIST BTU value correctly.

## References / Citations
- NIST SP 811 (2008): 1 BTU_IT = 1055.05585262 J (exact definition)
- EnergyPlus StandardRatings.hh:71: `ConvFromSIToIP(3.412141633)` — identical 10-digit rounded value
- EnergyPlus OutputReportTabular.cc:19124: `ort->UnitConv(66).mult = 3.412;` — even coarser rounding
