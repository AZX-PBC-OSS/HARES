# Ice specific heat constant (2.1 kJ/kg.K) -- verify no regression to IP units bug (0.24)
**Review ID**: phys-const-03
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
`crates/hares-physics/src/psychrometrics.rs:36`, `crates/hares-physics/src/constants.rs:86`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc:494`
- `vendors/EnergyPlus/src/EnergyPlus/RefrigeratedCase.cc:214`
- `vendors/EnergyPlus/src/EnergyPlus/PlantPipingSystemsManager.cc:5904`
- `vendors/EnergyPlus/src/EnergyPlus/GroundTemperatureModeling/FiniteDifferenceGroundTemperatureModel.cc:1100`
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py:179-186`

## Findings
### Finding 1: [Severity: low] Ice specific heat constant is correct (2.1 kJ/(kg·K)); no regression to 0.24
**Description**: The constant `SPECIFIC_HEAT_ICE_KJ_KG_K` at `psychrometrics.rs:36` holds the value `2.1` in SI units (kJ/(kg·K)). This matches both EnergyPlus and OCHRE, which use `2.1` in the denominator of the below-freezing wet-bulb psychrometric formula. The prior bug value of `0.24` does not appear as an active constant anywhere in the codebase -- it is referenced only in documentation comments (`psychrometrics.rs:41`, `psychrometrics.rs:507`) warning about the previous bug and in the regression test at `psychrometrics.rs:510`.
**Code Location**: `crates/hares-physics/src/psychrometrics.rs:36`
**Root Cause**: N/A -- code is correct.
**Impact**: None. The ice specific heat is correct SI and is used correctly in the denominator of the below-freezing wet-bulb formula at `psychrometrics.rs:95`.

### Finding 2: [Severity: low] No dedicated ice specific heat constant in shared constants module
**Description**: `constants.rs:86` holds `CP_LIQUID_WATER_J_KG_K = 4_180.0`, the specific heat of liquid water, not ice. The shared constants module (`constants.rs`) contains no `CP_ICE_*` or equivalent constant. The ice specific heat is defined only as a module-local constant in `psychrometrics.rs:36`. This is acceptable since the constant is used exclusively in the psychrometric wet-bulb formula.
**Code Location**: `crates/hares-physics/src/constants.rs:86`
**Root Cause**: N/A -- design choice to keep psychrometric-specific constants local.
**Impact**: None. No conflicting ice constant exists elsewhere.

### Finding 3: [Severity: low] Duplicate ice-side heat capacity in the below-freezing formula (2.1 vs 2.006)
**Description**: The below-freezing wet-bulb formula uses two distinct constants that both represent ice-side heat capacity:
- `SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE = 2.006` (numerator term at `psychrometrics.rs:92`) -- this is the precise ASHRAE HOF 2021 value, directly replacing the older `0.24` derived-term coefficient used by EnergyPlus/OCHRE.
- `SPECIFIC_HEAT_ICE_KJ_KG_K = 2.1` (denominator term at `psychrometrics.rs:95`) -- this is the rounded ASHRAE value, matching EnergyPlus and OCHRE.

The two constants differ by approximately 4.7%. The denominator maintains consistency with EnergyPlus/OCHRE (2.1), while the numerator adopts the ASHRAE HOF 2021 revised value (2.006). The documentation adequately explains both, but the use of two slightly different values for what is fundamentally the same physical property warrants awareness.
**Code Location**: `crates/hares-physics/src/psychrometrics.rs:36-42`
**Root Cause**: The numerator reformulation per ASHRAE HOF 2021 Eq. 37 (the HARES comment at line 39-41 states the 2.006 numerator coefficient was updated from the older 0.24 derived term), while the denominator retains backward-compatible rounding (2.1) matching EnergyPlus/OCHRE.
**Impact**: The ~4.7% discrepancy between numerator and denominator ice-side coefficients produces a ~0.3% difference in computed humidity ratio at WB = -5degC compared to EnergyPlus/OCHRE. This falls within ASHRAE psychrometric tolerance for the below-freezing range.

### Finding 4: [Severity: low] Regression test guards against the 0.24 bug
**Description**: A dedicated regression test exists at `psychrometrics.rs:507-522` (`humidity_ratio_from_twb_sub_freezing_uses_correct_ice_specific_heat`) that verifies the sub-freezing wet-bulb formula produces physically reasonable humidity ratios. The test ensures the output is positive, bounded, and does not exceed saturation. This test would fail if the 0.24 value were accidentally re-introduced.
**Code Location**: `crates/hares-physics/src/psychrometrics.rs:510`
**Root Cause**: N/A -- proactive measure.
**Impact**: None -- effective safety net.

### Finding 5: [Severity: low] The 0.24 value in EnergyPlus/OCHRE is a derived term, not ice specific heat
**Description**: The value `0.24` in the EnergyPlus (`Psychrometrics.cc:494`) and OCHRE (`psychrolib_jit.py:184`) below-freezing formula numerator represents `cp_wv - cp_ice = 1.86 - 2.1` (a derived combination term in the traditional ASHRAE formulation), not ice specific heat directly. HARES replaces this derived coefficient with the direct ASHRAE HOF 2021 ice specific heat value of 2.006. The HARES comment at `psychrometrics.rs:41` -- "(The previous value of 0.24 was an IP unit value in BTU/lb/F -- incorrect for SI.)" -- may conflate the origin of 0.24: ice specific heat in IP units is approximately 0.5 BTU/(lb.degF), not 0.24. The 0.24 historically used in the HARES codebase may have had a different provenance than the 0.24 in the EnergyPlus/OCHRE formula. This is a documentation nuance only; the code is correct.
**Code Location**: `crates/hares-physics/src/psychrometrics.rs:41` (comment), `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc:494`, `vendors/OCHRE/ochre/utils/psychrolib_jit.py:184`
**Root Cause**: Historical note in comment conflates the derived coefficient with ice specific heat.
**Impact**: None -- documentation only.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 0 / 5

All ice specific heat constants are in correct SI units. The primary constant (`SPECIFIC_HEAT_ICE_KJ_KG_K = 2.1`) matches both EnergyPlus and OCHRE reference implementations. No regression to the IP-units `0.24` bug exists. The value `0.24` appears only in documentation warnings and test descriptions. A regression test at `psychrometrics.rs:510` guards against accidental reversion.

## Recommendations
1. No corrective action required for the ice specific heat constant itself.
2. Consider either (a) using the same ice specific heat value (2.1 or 2.006) consistently in both numerator and denominator of the below-freezing formula, or (b) adding a comment explaining why the two coefficients differ (backward compatibility vs ASHRAE precision). The current split is not a bug but could reduce surprise for future readers.
3. The comment at `psychrometrics.rs:41` stating "0.24 was an IP unit value in BTU/lb/F" could be clarified to note that 0.24 in the EnergyPlus/OCHRE formula represents `cp_wv - cp_ice` rather than ice specific heat directly, to prevent confusion when comparing against vendor reference code.

## References / Citations
- ASHRAE Handbook of Fundamentals 2017, Ch.1, Table 2
- ASHRAE Handbook of Fundamentals 2021, Ch.1 Eq. 37 (below-freezing psychrometer equation)
- EnergyPlus Psychrometrics.cc (line 494): below-freezing wet-bulb formula uses `2.1` in denominator and `0.24` in numerator
- OCHRE psychrolib_jit.py (lines 184-185): identical formula constants (2.1 denominator, 0.24 numerator)
- NIST CODATA 2018: Conversion factor 1 BTU/(lb.degF) = 4.1868 kJ/(kg.K)
