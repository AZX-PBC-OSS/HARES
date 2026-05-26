# Wet-bulb specific heat constants (2.381, 2.006 kJ/kg.K) vs PsychroLib (2.326)
**Review ID**: phys-const-02
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/psychrometrics.rs:30-42` — constant definitions
- `crates/hares-physics/src/psychrometrics.rs:83-99` — `humidity_ratio_from_twb` (uses the constants)
- `crates/hares-physics/src/psychrometrics.rs:337-351` — regression test with relaxed tolerance
- `crates/hares-physics/src/psychrometrics.rs:507-539` — regression tests making incorrect claims

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.cc:492,494` — inverse wet-bulb (2003 re-engineered, uses 2.326 / 0.24)
- `vendors/EnergyPlus/src/EnergyPlus/Psychrometrics.hh:1448` — forward humidity ratio (1976 original, uses 2.381 / 1.805)
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py:180,184` — OCHRE/PsychroLib (uses 2.326 / 0.24)

## Findings

### Finding 1: Below-freezing psychrometer coefficient is wrong (2.006 vs 0.24) [Severity: high]
**Description**: `SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE` is set to `2.006` kJ/(kg·K) with a comment claiming that `0.24` was "an IP unit value in BTU/lb/°F — incorrect for SI." Both claims are false.

**Code Location**: `crates/hares-physics/src/psychrometrics.rs:37-42`

**Root Cause**: The standard ASHRAE sub-freezing psychrometer coefficient is `0.24` kJ/(kg·K), derived from `c_p_ice − c_p_v = 2.1 − 1.86 = 0.24`. This is the PHYSICAL SI value, not an IP unit value. The comment at line 41 makes two errors:
1. `0.24` is already the correct SI coefficient (in kJ/(kg·K)), not an IP value.
2. Even if it were in IP units, `0.24` BTU/(lb·°F) converts to `1.005` kJ/(kg·K), not `2.006`.

The value `2.006` corresponds to neither the correct psychrometer coefficient nor any physically meaningful `c_p` difference in the sub-freezing derivation. It was introduced as part of a "fix" that incorrectly assumed `0.24` was an IP unit value needing conversion.

**Impact**: The error magnitude in W depends on conditions. At `Tdb = −2°C, Twb = −5°C`, the relative error in humidity ratio is approximately 0.55%. At colder wet-bulb temperatures the error grows linearly with the term `(2.006 − 0.24) × |Twb| × W_sat`. While the absolute error is constrained by the inherently low humidity ratios at sub-freezing temperatures, the coefficient itself is wrong by a factor of ~8.4× (`2.006 / 0.24 ≈ 8.36`), and the test at line 509 contains only weak validity checks (positivity, bounds) that do not validate against a known reference.

**Vendor Consistency**: Both re-engineered, actively maintained vendor implementations agree:
- EnergyPlus `PsyTwbFnTdbWPb` (re-engineered 2003): uses `0.24` (`Psychrometrics.cc:494`)
- OCHRE/PsychroLib: uses `0.24` (`psychrolib_jit.py:184`)

**Recommendation**: Replace `2.006` with `0.24` and update the comment to reflect the correct derivation (`c_p_ice − c_p_v`).

---

### Finding 2: Above-freezing psychrometer constant mixes two inconsistent ASHRAE formulations [Severity: medium]
**Description**: `SPECIFIC_HEAT_WET_BULB_ABOVE_FREEZE` = `2.381` is adopted from the ASHRAE HOF **1972** simplified formulation but paired with companion constants from the ASHRAE HOF **2017** standard formulation. This mixes two incompatible constant sets.

**Code Location**: `crates/hares-physics/src/psychrometrics.rs:30-35, 87-90`

**Root Cause**: The psychrometer coefficient `c_s_wb` in the numerator term `(h_fg0 − c_s_wb × Twb)` derives from `c_pw − c_pv`. Two editions of ASHRAE HOF provide two consistent constant sets:

| Edition | `c_s_wb` | `c_pv` (denominator) | `c_pa` coeff on `(Tdb−Twb)` |
|---------|----------|---------------------|-----------------------------|
| ASHRAE 1972 (EnergyPlus .hh:1448) | **2.381** | **1.805** | **1.0** (implicit) |
| ASHRAE 2017 (EnergyPlus .cc:492, PsychroLib) | **2.326** | **1.86** | **1.006** (explicit) |

HARES (`psychrometrics.rs:87-90`) uses `2.381` (1972 set) with `1.86` (2017 set) and `1.006` (2017 set). This is an inconsistent mix.

EnergyPlus itself confirms the issue: the 1976 function `PsyWFnTdbTwbPb` (`.hh:1448`) still uses the 1972 set (`2.381`, `1.805`, no `1.006` multiplier), while the 2003-re-engineered function `PsyTwbFnTdbWPb` (`.cc:492`) switched to the 2017 set (`2.326`, `1.86`, explicit `1.006`). The 1976 function was never re-engineered alongside the 2003 update.

**Impact**: The practical error magnitude is small — approximately 0.062% at the standard test condition (Tdb=30°C, Twb=25°C, P=95461Pa) — because the term `c_s_wb × Twb` is subtracted from `h_fg0 = 2501`, and the difference between `2.381 × Twb` and `2.326 × Twb` is at most ~2 kJ/kg at 35°C wet-bulb. However, the code comments (lines 33–34, 524–525, 340–341) suggest a deliberate departure from PsychroLib based on an ASHRAE HOF 2021 reference, but:

1. The value `2.381` pre-dates ASHRAE 2021 by decades (used in EnergyPlus since 1976).
2. Even if ASHRAE HOF 2021 re-adopted `2.381`, it would need to be paired with consistent companion constants (`c_pv = 1.805` or an updated `c_s_wb` derivation) for mathematical correctness.
3. The test at line 343 acknowledges the discrepancy with PsychroLib and relaxes tolerance to 0.5% — but the 0.5% tolerance is far larger than the actual divergence (~0.06%), suggesting the test was designed around an expected larger error that does not materialize.

**Vendor Consistency**: The re-engineered, validated vendor implementations use the 2017 set:
- EnergyPlus `PsyTwbFnTdbWPb` (re-engineered 2003): `2.326` + `1.86` + `1.006`
- OCHRE/PsychroLib: `2.326` + `1.86` + `1.006`

**Recommendation**: Use the internally consistent ASHRAE HOF 2017/2021 standard set: `c_s_wb = 2.326` with `c_pv = 1.86` and explicit `1.006 × (Tdb − Twb)`. This aligns with all modern vendor implementations and eliminates formulation mixing.

---

## Summary
- **Total findings**: 2
- **High**: 1 (below-freezing coefficient is factually wrong due to incorrect IP/SI unit conversion claim)
- **Medium**: 1 (above-freezing coefficient mixes two inconsistent ASHRAE editions)
- **Critical / Low**: 0

## Recommendations
1. **Fix the below-freezing constant**: Replace `SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE = 2.006` with `0.24` (`psychrometrics.rs:42`). Update the comment at lines 37–41 to note the correct derivation: `c_p_ice − c_p_v = 2.1 − 1.86 = 0.24` (ASHRAE HOF Ch. 1 Eq. 37).

2. **Align the above-freezing constant with the modern standard**: Replace `SPECIFIC_HEAT_WET_BULB_ABOVE_FREEZE = 2.381` with `2.326` (`psychrometrics.rs:35`). Update the comment at lines 31–34 to cite ASHRAE HOF 2017 Ch. 1 Eq. 35 and note the derivation: `c_pw − c_pv = 4.186 − 1.86 = 2.326`. This makes the constant set internally consistent and matches all actively maintained vendor implementations.

3. **Remove or correct the relaxed tolerance** in `humidity_ratio_from_twb_matches_psychrolib_reference` (`psychrometrics.rs:343`): after adopting `2.326`, the tolerance can be tightened from 0.5% to 0.03%, matching the other PsychroLib validation tests.

4. **Update the regression test comments** at `psychrometrics.rs:507-539` to reflect the correct rationale (the old values were from the correct ASHRAE 2017 formulation, not "IP unit errors" or "old wrong constants").

## References / Citations
- ASHRAE Handbook of Fundamentals 2017, Ch. 1, Eq. 35 (above-freezing psychrometer equation, SI): `W = ((2501 − 2.326·Twb)·W_sat − 1.006·(Tdb − Twb)) / (2501 + 1.86·Tdb − 4.186·Twb)`
- ASHRAE Handbook of Fundamentals 2017, Ch. 1, Eq. 37 (below-freezing psychrometer equation, SI): `W = ((2830 − 0.24·Twb)·W_sat − 1.006·(Tdb − Twb)) / (2830 + 1.86·Tdb − 2.1·Twb)`
- EnergyPlus `PsyTwbFnTdbWPb` (2003 re-engineered): `Psychrometrics.cc:492,494` — uses the ASHRAE 2017 formulation
- EnergyPlus `PsyWFnTdbTwbPb` (1976 original, never re-engineered): `Psychrometrics.hh:1448` — uses the ASHRAE 1972 formulation (`2.381`, `1.805`, simplified numerator)
- OCHRE/PsychroLib: `psychrolib_jit.py:180,184` — uses the ASHRAE 2017 formulation
- Derivation: `c_s_wb = c_pw − c_pv = 4.186 − 1.86 = 2.326` (above-freezing); `c_s_wb = c_p_ice − c_pv = 2.1 − 1.86 = 0.24` (below-freezing)
- IP→SI conversion: `1 BTU/(lb·°F) = 4.1868 kJ/(kg·K)`; `0.24 BTU/(lb·°F) ≈ 1.005 kJ/(kg·K)` — NOT `2.006`
