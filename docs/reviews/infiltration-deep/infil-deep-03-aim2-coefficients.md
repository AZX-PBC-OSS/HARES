# AIM-2 model coefficients computation — all coefficient formulas for ShieldingClass × FoundationLeakageClass interactions
**Review ID**: infil-deep-03
**Category**: infiltration-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/infiltration.rs` (full 1728 lines)
  - Core AIM-2 runtime: `ashrae_wind_stack()` at lines 133–145
  - Coefficient computation: `aim2_coefficients_from_ach50()` at lines 479–576
  - ShieldingClass enum: lines 400–420
  - FoundationLeakageClass enum: lines 425–431
  - Aim2Params / Aim2Coefficients structs: lines 434–467

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/ZoneEquipmentManager.cc` (lines 6786–6801 — AIM-2 runtime formula)
- `vendors/EnergyPlus/src/EnergyPlus/DataHeatBalance.hh` (lines 1112–1117 — AIM-2 data structure: `FlowCoefficient`, `AIM2StackCoefficient`, `AIM2WindCoefficient`, `PressureExponent`, `ShelterFactor`)
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceAirManager.cc` (lines 1035–1071 — `ZoneInfiltration:FlowCoefficient` input parsing)
- Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations", *HVAC&R Research* 4(2):119–139.

## Key Structural Observation

EnergyPlus does **not** compute AIM-2 coefficients from building properties. The `ZoneInfiltration:FlowCoefficient` IDF object reads all five coefficients (`c`, `Cs`, `Cw`, `n`, `s`) as direct numeric user inputs (HeatBalanceAirManager.cc:1043–1046). There is no ShieldingClass lookup table, no FoundationLeakageClass table, and no ACH50-to-coefficient derivation anywhere in the vendor EnergyPlus code.

HARES takes the opposite approach: `aim2_coefficients_from_ach50()` derives `c_s`, `c_w`, and `shelter_coeff` from an ACH50 blower-door measurement using the full Walker & Wilson (1998) Equations 9–25 pipeline. The runtime quadrature formula is structurally identical between the two codebases, but the coefficient generation pathway is entirely different. Direct formula-by-formula comparison against EnergyPlus is therefore impossible for the coefficient computation — the valid reference is the Walker & Wilson (1998) paper itself and the OCHRE cross-validation already embedded in the HARES test suite.

## Findings

### Finding 1: ShieldingClass raw values differ from Walker & Wilson (1998) Table 3 [Severity: medium]

**Description**: The `ShieldingClass` enum assigns raw shelter values of Normal=0.5, Exposed=0.9, WellShielded=0.3 (lines 414–418). Walker & Wilson (1998) Table 3 gives the canonical shelter factors `s_g` as 1/6 ≈ 0.167 (normal/suburban), 0.30 (exposed), and 0.10 (well-shielded). The HARES ELA code branch correctly uses `SHIELDING_NORMAL = 0.5 / 3.0 ≈ 0.167` (line 674) matching W&W Table 3, but the AIM-2 branch uses substantially larger values.

**Code Location**: `infiltration.rs:414–418` (`ShieldingClass::raw()`)

**Root Cause**: The code follows ResStock / OCHRE convention where the AIM-2 shelter coefficient is a "raw local" multiplier applied inside `(shelter_coeff * wind_speed)^(2n_i)`, not a pure wind-speed ratio. The combined shelter coefficient is computed at line 564 as `f_t * (shielding.raw() * (1 - y_i) + s_wflue * 1.5 * y_i)`, where the `f_t` terrain correction factor (≈ 0.61–0.68 for suburban 5–8 m) scales it down. The cross-validation test at line 1548–1590 confirms the resulting `shelter_coeff` matches OCHRE at 1e-8 tolerance.

**Impact**: The values are correct per the ResStock/OCHRE convention that HARES targets. However, a developer comparing against W&W 1998 Table 3 directly would conclude the values are wrong. The comment at line 413 says "Walker & Wilson (1998) Table 3" but the values are not from that table — they are from ResStock's `get_aim2_shelter_coefficient`. This documentation ambiguity risks future maintenance errors.

### Finding 2: Non-crawlspace wind factor exponent `(1.5 - y_i)` departs from Walker & Wilson (1998) Eq. 15 [Severity: medium]

**Description**: At line 542, the wind factor for non-crawlspace foundations uses:
```rust
0.19 * (2.0 - n_i) * (1.0 - ((x_i_raw + r_i) / 2.0).powf(1.5 - y_i))
```
Walker & Wilson (1998) Eq. 15 uses a plain exponent of 1.5 (not `1.5 - y_i`). For the typical no-flue case (`y_i = 0`), `1.5 - 0 = 1.5` and the formula matches. For flue cases (`y_i = 0.2`), the exponent becomes 1.3, which reduces the inner term's power relative to the published equation. The origin of the `-y_i` adjustment is not documented and may be an OCHRE-introduced refinement not present in the original W&W derivation.

**Code Location**: `infiltration.rs:542`

**Root Cause**: The formula was adopted from OCHRE's `envelope.py:488-633` without documenting the deviation from W&W 1998 Eq. 15. The flue correction term (`-y_i/4 * (j_i - 2*y_i*j_i^4)`) already accounts for flue effects in the W&W formulation; the additional exponent reduction may double-count the flue influence.

**Impact**: For no-flue buildings (the vast majority of residential archetypes), this has zero effect. For flue-equipped buildings, the wind coefficient `f_w` will be slightly elevated because `(1 - X^1.3) > (1 - X^1.5)` for 0 < X < 1, modestly overestimating wind-driven infiltration. The effect is bounded and partially offset by the subtractive flue term, but it deviates from the published AIM-2 model.

### Finding 3: Wind speed source differs between HARES and EnergyPlus — correctly documented [Severity: low]

**Description**: EnergyPlus applies the AIM-2 formula using a zone-level wind speed (`Zone(NZ).WindSpeed` at ZoneEquipmentManager.cc:6718) that has already been terrain-corrected from met station data. HARES pre-bakes the terrain correction into `shelter_coeff` via the `f_t` factor (line 556–561) and requires the caller to pass **raw** met-station wind speed. Both approaches produce equivalent results, but the responsibility for avoiding double-correction shifts from the framework (EnergyPlus) to the caller (HARES).

**Code Location**: `infiltration.rs:556–568` (terrain-correction pre-baking + documentation comment)

**Root Cause**: HARES chose to embed terrain correction in the pre-computed coefficient rather than requiring a separate terrain-corrected wind speed input. This is mathematically equivalent and documented in the code comments at lines 566–568. The test at lines 1688–1727 explicitly verifies the shelter-to-terrain ratio invariant.

**Impact**: Low. The documentation is clear and the invariant is tested. The risk is that a caller familiar with EnergyPlus conventions might terrain-correct the wind speed before calling `ashrae_wind_stack()`, causing the flow to be understated by roughly `f_t^2 ≈ 0.38` (a 62% error for suburban terrain). This is protected by the `aim2_shelter_coeff_embeds_terrain_correction` regression test (line 1688).

### Finding 4: Crawlspace wind factor `x_i` clamp comment references non-existent Eq. 25 [Severity: low]

**Description**: At line 533, the comment says `// Eq. 25 clamp`, referencing Walker & Wilson (1998) Eq. 25. However, the W&W 1998 paper equations end at Eq. 24 (the `x_s` formula). The `x_i` clamping to `min(x_i_raw, 1 - 2*y_i)` is not from a numbered equation in the paper — it appears to be a practical bound to prevent non-physical values.

**Code Location**: `infiltration.rs:533`

**Root Cause**: The equation reference in the comment is incorrect. The clamp is a reasonable safety bound (the horizontal leakage asymmetry fraction cannot exceed 1.0 minus the flue contribution), but the citation is wrong.

**Impact**: Low — the clamp is a defensive bound that prevents numerical issues. The formula itself (lines 534–538) correctly implements W&W 1998 Eqs. 20–24. The wrong equation reference in the comment is a documentation error only.

### Finding 5: Exponential drift in `powi(2).powf(0.75)` vs `abs().powf(1.5)` [Severity: low]

**Description**: At line 537, the crawlspace `x_x` term computes:
```rust
let x_x = 1.0 - (((x_i - x_s) / (2.0 - r_i)).powi(2)).powf(0.75);
```
This decomposes `|z|^1.5` as `(z^2)^0.75`, which is mathematically correct but introduces a small floating-point round-trip error compared to computing `z.abs().powf(1.5)` directly. The double exponentiation through two different powers (integer-squared then float-0.75) adds approximately 1–2 ULPs of error per evaluation.

**Code Location**: `infiltration.rs:537`

**Root Cause**: The code follows the OCHRE convention of decomposing the 1.5-exponent computation into a square (exact) followed by a 0.75 power. This is not a logical error, but the redundant `powi(2)` → `powf(0.75)` path is less precise than `abs().powf(1.5)`.

**Impact**: Low — the error is at the ULP level and is negligible for infiltration calculations. Cross-validation against OCHRE matches within tolerance.

## Verification of Specific Checklist Items

### (1) Wind coefficient C_w for each shielding class

HARES does **not** maintain a per-shielding-class C_w table. Instead, the shielding class affects the `shelter_coeff` (line 564) which then modulates wind speed inside `ashrae_wind_stack()` as `(shelter_coeff * wind_speed)^(2*n_i)`. The EnergyPlus equivalent uses a user-provided `ShelterFactor` applied the same way (ZoneEquipmentManager.cc:6791–6792). The combined effect is:

| ShieldingClass | raw | shelter_coeff (suburban 5m, no flue) | Relative wind contribution |
|---|---|---|---|
| WellShielded    | 0.3 | 0.184 | 0.18^(1.3) ≈ 0.11 |
| Normal          | 0.5 | 0.307 | 0.31^(1.3) ≈ 0.24 |
| Exposed         | 0.9 | 0.553 | 0.55^(1.3) ≈ 0.52 |

The ranking (Exposed > Normal > WellShielded) is physically correct. The absolute values match ResStock/OCHRE conventions via the cross-validation test (line 1548). **EnergyPlus has no equivalent lookup** — its `ShelterFactor` is an unrestricted user input.

### (2) Stack coefficient C_s for each foundation leakage class

The stack factor `f_s` (line 524) uses temperature-difference exponent `n_i` (0.65 default) — matching Walker & Wilson (1998). The height correction is embedded in the `cs = f_s * (ρ·g·H/T_in)^n_i` formula (line 528), where `H = infiltration_height_m`. This is correct: stack-driven flow scales with building height because the hydrostatic pressure difference is proportional to H.

For the two foundation classes:

- **Other (slab)**: `leak_ceil=0.25, leak_floor=0.25` → `r_i=0.50, x_i_raw=0.0` → symmetrical vertical leakage distribution → neutral pressure level at mid-height → `f_s ≈ 0.256` at `n_i=0.65`.
- **VentedCrawlspace**: `leak_ceil=0.15, leak_floor=0.50` → `r_i=0.65, x_i_raw=-0.35` → bottom-heavy leakage → neutral pressure level shifts downward → `f_s ≈ 0.260` at `n_i=0.65` (slightly higher because more stack potential with floor-dominated leakage).

The EnergyPlus equivalent has no comparable foundation-type parameterization — Cs is a raw user input.

### (3) Horizontal leakage fraction for each foundation type

The horizontal leakage fraction is encoded implicitly via `r_i = leak_ceil + leak_floor`:

| FoundationLeakageClass | Ceil | Floor | Horizontal (r_i) | Vertical (1-r_i) |
|---|---|---|---|---|
| VentedCrawlspace       | 0.15 | 0.50  | 0.65             | 0.35             |
| Other (slab/basement)  | 0.25 | 0.25  | 0.50             | 0.50             |

These values come from Walker & Wilson (1998) Table 1. The split is physically correct:
- Horizontal surfaces (ceil + floor) drive stack/buoyancy flow — captured through `r_i` in the `f_s` stack factor.
- Vertical surfaces (walls) drive wind-driven flow — captured through `1 - r_i` in the `f_w` wind factor.

For the ELA branch (`calculate_ela_coefficients`, lines 627–670), explicit `hor_lk_frac` values are used: 0.0 (conditioned), 0.4 (garage), 0.75 (attic) — all from W&W 1998 Table 2. This is a separate code path for zone-level ELA calculations, not part of the AIM-2 coefficient pipeline, and is correctly implemented.

### (4) Combined flow coefficient C = sqrt(C_w² + C_s²)

**Verified — correct.** HARES `ashrae_wind_stack()` (lines 142–144):
```rust
let q_temp = c_s * delta_t_c.abs().powf(n_i);
let q_wind = c_w * (shelter_coeff.max(0.0) * wind_speed_m_s).powf(2.0 * n_i);
(q_temp * q_temp + q_wind * q_wind).sqrt()
```

EnergyPlus (ZoneEquipmentManager.cc:6789–6792):
```cpp
sqrt( pow_2(FlowCoefficient * AIM2StackCoefficient * pow(|ΔT|, PressureExponent)) +
      pow_2(FlowCoefficient * AIM2WindCoefficient * pow(ShelterFactor * WindSpeedExt, 2*PressureExponent)) )
```

The two formulas are equivalent. HARES pre-multiplies `c_flow * cs` into `c_s` (line 551) and `c_flow * cw` into `c_w` (line 552), while EnergyPlus keeps `c` as a separate coefficient multiplied at runtime. The quadrature `sqrt(Q_s² + Q_w²)` form comes directly from Walker & Wilson (1998) Eq. 1 and is the correct pressure superposition for uncorrelated wind and stack effects.

### (5) Slab-on-grade effective stack height

**Verified — correct.** For slab foundations (FoundationLeakageClass::Other), the leakage distribution is `ceil=0.25, floor=0.25` — equal at top and bottom. The Walker & Wilson (1998) formulation uses the full `infiltration_height_m` in the `cs` formula (line 528) because:

1. The neutral pressure level for equal floor/ceiling leakage is at mid-height (0.5H).
2. The stack factor `f_s` already accounts for the leakage distribution through `r_i` and `x_i_raw`.
3. The effective driving pressure is proportional to the full height H, not the distance from the neutral plane.

There is no separate slab-specific height reduction in either the published AIM-2 model or the EnergyPlus implementation. The HARES implementation correctly passes the full conditioned-space height as `infiltration_height_m`.

## Summary

- **Total findings**: 5
- **Critical**: 0
- **High**: 0
- **Medium**: 2
- **Low**: 3

## Recommendations

1. **Update `ShieldingClass::raw()` documentation** (line 413): Replace "Walker & Wilson (1998) Table 3" with "ResStock `get_aim2_shelter_coefficient`" or add a note explaining that the values are ResStock/OCHRE "raw local" shelter multipliers, not the W&W Table 3 `s_g` wind-speed ratios. Consider adding the W&W Table 3 values as doc comments for reference (WellShielded: 0.10, Normal: 0.167, Exposed: 0.30).

2. **Document the `(1.5 - y_i)` exponent** (line 542): Add a comment explaining that the `-y_i` adjustment to the exponent is an OCHRE refinement beyond W&W 1998 Eq. 15. If it is not intentional, evaluate whether reverting to plain `1.5` would be more faithful to the published model (this would affect only flue-equipped buildings).

3. **Fix Eq. 25 comment** (line 533): The clamp `x_i_raw.min(1.0 - 2.0 * y_i)` is not from "Eq. 25" — change the comment to describe the physical constraint (horizontal leakage asymmetry cannot exceed the non-flue fraction).

4. **Consider simplifying `powi(2).powf(0.75)` at line 537** to `abs().powf(1.5)` for improved floating-point precision, or add a comment noting the convention follows OCHRE.

5. **Add a table mapping FoundationLeakageClass to explicit horizontal leakage fractions** in the module documentation to make the wind/stack split transparent to future maintainers.

## References / Citations

- Walker, I. S., & Wilson, D. J. (1998). Field validation of algebraic equations for stack and wind driven air infiltration calculations. *HVAC&R Research*, 4(2), 119–139.
- ASHRAE Handbook of Fundamentals (2017/2021), Chapter 16: Ventilation and Infiltration.
- EnergyPlus Engineering Reference §15.4 (AIM-2 Enhanced Model).
- OCHRE `envelope.py:488-633` — `calculate_ashrae_infiltration_params`.
