# ISA pressure exponent precision (5.2558761 vs EnergyPlus 5.2559) — quantify pressure difference at 0-3000m
**Review ID**: phys-const-09
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/constants.rs:113` — `pub const ISA_PRESSURE_EXPONENT: f64 = 5.255_876_1;`
- `crates/hares-physics/src/constants.rs:90-113` — doc comment block with USSA 1976 derivation
- `crates/hares-physics/src/air_properties.rs:11-13` — `standard_pressure_pa()` consumer
- `crates/hares-physics/tests/physics_validation_tests.rs:1183-1201` — `isa_pressure_exponent_matches_ussa76_derivation` test

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4467` — uses `5.2559`
- `vendors/EnergyPlus/src/EnergyPlus/Autosizing/Base.cc:176` — uses `5.2559`
- `vendors/EnergyPlus/src/EnergyPlus/MicroturbineElectricGenerator.cc:252` — uses `5.2559`
- `vendors/OCHRE/ochre/` — does **not** compute ISA pressure; uses constant 101.325 kPa regardless of elevation

## Findings

### Finding 1: HARES exponent is more accurate than EnergyPlus — extra digits justified by derivation [Severity: low]
**Description**: The USSA 1976 tropospheric pressure exponent derives from primary physical constants as E = g₀·M₀ / (R*·L) = 9.80665 × 0.0289644 / (8.31432 × 0.0065) ≈ **5.2558761133**. HARES stores 5.2558761 (8 sig figs, 1.3×10⁻⁸ from derivation). EnergyPlus stores 5.2559 (5 sig figs, 2.4×10⁻⁵ from derivation). The HARES value is ~1,800× closer to the USSA 1976 derivation value than EnergyPlus'.

**Code Location**: `crates/hares-physics/src/constants.rs:107-113` (HARES) vs `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4467` (EnergyPlus)

**Root Cause**: EnergyPlus uses a 5-significant-figure rounding (5.2559) that discards three meaningful digits. HARES stores the derivation value rounded to f64 precision, which is the correct engineering choice given that super-float computing is free and the derivation inputs have 6+ significant figures.

**Impact**: The practical pressure difference is negligible:

| Elevation (m) | HARES (Pa)       | EnergyPlus (Pa)  | Δ (Pa) | Relative Δ |
|---------------|-----------------:|-----------------:|-------:|-----------:|
| 0             | 101325.000000    | 101325.000000    | +0.00  | 0.00%      |
| 500           | 95460.838252     | 95460.812373     | +0.03  | 2.7×10⁻⁵%  |
| 1000          | 89874.568425     | 89874.519416     | +0.05  | 5.5×10⁻⁵%  |
| 2000          | 79495.211748     | 79495.124038     | +0.09  | 1.1×10⁻⁴%  |
| 3000          | 70108.539571     | 70108.422159     | +0.12  | 1.7×10⁻⁴%  |

The maximum difference of ~0.12 Pa at 3000 m is 3–5 orders of magnitude below the accuracy of any building simulation sensor (typical barometric transducers: ±50–100 Pa; differential pressure sensors for HVAC: ±25 Pa at best). The difference is well within numerical noise for all practical purposes.

### Finding 2: EnergyPlus truncation propagates to multiple call sites [Severity: low]
**Description**: The over-rounded 5.2559 is hard-coded in at least three independent EnergyPlus locations (WeatherManager, Autosizing::Base, MicroturbineElectricGenerator), creating a latent inconsistency risk if one site is updated but another is not. HARES uses a single named constant, which is the better practice.

**Code Location**:
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4467`
- `vendors/EnergyPlus/src/EnergyPlus/Autosizing/Base.cc:176`
- `vendors/EnergyPlus/src/EnergyPlus/MicroturbineElectricGenerator.cc:252`

**Root Cause**: EnergyPlus uses magic-number literals rather than a named constant for the ISA pressure exponent.

**Impact**: No current bug — all three sites agree on 5.2559. But the duplication is a maintenance hazard.

### Finding 3: OCHRE has no ISA pressure model — fixed 101.325 kPa everywhere [Severity: low]
**Description**: OCHRE does not compute barometric pressure from elevation at all. It reads "Ambient Pressure (kPa)" from its input schedule, defaulting to 101.325 kPa (sea level) regardless of site elevation. This means OCHRE simulations at altitude (e.g., Denver at 1609 m) overestimate air density and pressure-dependent HVAC performance unless the user manually provides an altitude-corrected pressure schedule.

**Code Location**: `vendors/OCHRE/ochre/Models/Envelope.py:1070,1209,1248`; `vendors/OCHRE/ochre/Models/Humidity.py:25`

**Root Cause**: OCHRE relies entirely on user-provided input for ambient pressure with no elevation-based model.

**Impact**: For OCHRE users who do not manually adjust pressure schedules, altitude-dependent air density errors (up to ~17% at 1600 m) affect ventilation heat recovery, infiltration mass flow, and psychrometric calculations. HARES avoids this issue by computing ISA pressure from site elevation.

### Finding 4: Existing code comment and test are accurate and self-documenting [Severity: low]
**Description**: The doc comment on `ISA_PRESSURE_EXPONENT` correctly states the derivation, identifies the rounding level (8 sig figs from 5.2558761133…), and acknowledges the EnergyPlus/ASHRAE discrepancy. The validation test (`isa_pressure_exponent_matches_ussa76_derivation`) independently recomputes the derivation and checks within 1×10⁻⁵ — a conservative tolerance that would pass both HARES and EnergyPlus values but has been correctly tightened by the stored precision.

**Code Location**: `crates/hares-physics/src/constants.rs:97-112` (doc comment), `crates/hares-physics/tests/physics_validation_tests.rs:1183-1201` (test)

**Root Cause**: Good engineering practice — the comment and test are already present and correct.

**Impact**: No action needed. The documentation is thorough and accurate.

## Summary
- Total findings: 4
- Critical: 0 / High: 0 / Medium: 0 / Low: 4

All four findings are low-severity. The extra digits in HARES' ISA pressure exponent (5.2558761) are justified by direct derivation from USSA 1976 physical constants, producing a pressure that differs from the derivation by at most 6.5×10⁻⁵ Pa — far below any measurable threshold. The maximum difference vs EnergyPlus' 5.2559 is 0.12 Pa at 3000 m, which is 4–5 orders of magnitude below typical building simulation measurement accuracy.

## Recommendations
1. **No change needed** to `ISA_PRESSURE_EXPONENT`. The current value is correct, well-documented, and traceable to USSA 1976.
2. **No change needed** to the validation test. The 1e-5 tolerance is conservatively loose (the actual deviation is 1.3e-8) but adequate.
3. **Document the OCHRE gap** in future cross-validation notes: OCHRE's lack of ISA pressure modeling is a known difference in physical fidelity, not HARES being "wrong."

## References / Citations
- U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562) Part 1 §1.2.5 — tropospheric pressure exponent derivation
- Wikipedia "Barometric formula" — model equations table, layer 0 (confirms g₀=9.80665, M₀=0.0289644, R*=8.31432, L=0.0065)
- EnergyPlus Engineering Reference, "Standard Barometric Pressure" — uses StdPressureSeaLevel × (1 − 2.25577×10⁻⁵ × Elevation)^5.2559
- ASHRAE Handbook of Fundamentals 2021, Ch. 1 §1.8 — psychrometric pressure correction (also uses 5.2559)
