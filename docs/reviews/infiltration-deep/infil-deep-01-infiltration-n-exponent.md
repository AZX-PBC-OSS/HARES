# Infiltration exponent N_I_MIN=0.5, N_I_MAX=0.7, N_I_DEFAULT=0.65 — verify against ASHRAE 119-1988

**Review ID**: infil-deep-01
**Category**: infiltration-deep
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-physics/src/infiltration.rs` (lines 1–1728, full file)
- `crates/hares-envelope/src/thermal_solver/infiltration.rs` (runtime dispatch, lines 100–180)
- `crates/hares-envelope/src/thermal_solver/config.rs` (`InfiltrationMethod::AshraeWindStack`, lines 73–96)
- `crates/hares-core/src/dwelling/solver_builder.rs` (N_I_DEFAULT hardcoding, lines 926–1003)
- `crates/hares-io/src/hpxml/building.rs` (HPXML ACH-to-ACH50 conversion, lines 2252–2285)

## Vendor/Reference Files Consulted

- `vendors/EnergyPlus/src/EnergyPlus/DataHeatBalance.hh` (lines 158–163, 1100–1116): `InfiltrationModelType` enum and `Infiltration` struct with `PressureExponent` field
- `vendors/EnergyPlus/src/EnergyPlus/ZoneEquipmentManager.cc` (lines 6771–6801): Sherman-Grimsrud and AIM-2 runtime implementation
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceAirManager.cc` (lines 920–1046): `ZoneInfiltration:EffectiveLeakageArea` and `ZoneInfiltration:FlowCoefficient` input parsing
- `vendors/EnergyPlus/src/EnergyPlus/RoomAirModelManager.cc` (lines 1084–1092): crack exponent constraint for room-air models

## Findings

### Finding 1: [Severity: low] N_I_DEFAULT=0.65 is correct for typical US housing stock, matches ASHRAE 119 / ASTM E779 mixed-leakage assumption

**Description**: HARES sets `N_I_MIN=0.5` (turbulent/perfect orifice), `N_I_MAX=0.7` (laminar crack flow), and `N_I_DEFAULT=0.65` (mixed leakage). These three values span the full physically-meaningful range described in ASHRAE 119-1988 and ASHRAE HOF 2021 Ch. 16. The default of 0.65 is the standard assumption for the 4-to-50 Pa blower-door extrapolation (Q_50 = Q_4 × 12.5^n) and matches:
- OCHRE/ResStock default (0.65)
- Walker & Wilson (1998) typical range
- ASHRAE 119 / ASTM E779 residential recommendation

**Code Location**: `crates/hares-physics/src/infiltration.rs:48–52`

**Root Cause**: Design choice — correctly chosen.

**Impact**: None. The exponent values are appropriate and well-documented in the module-level doc comment (lines 3–23) and the `ach_nat_to_ach50` function documentation (lines 54–81).

---

### Finding 2: [Severity: low] `ach_nat_to_ach50` correctly implements ASHRAE 119 power-law conversion

**Description**: The function `ach_nat_to_ach50(q_nat, n)` implements `Q_50 = Q_nat × (50/4)^n`, which is the canonical ASHRAE 119 / ASTM E779 single-point blower-door extrapolation formula. With `n = NATURAL_TO_50PA_EXPONENT = 0.65`:
- (50/4)^0.65 = 12.5^0.65 ≈ 5.164
- ACH50 = ACHnat × 5.164

The function accepts any exponent `n`, making it reusable for alternative assumptions (e.g., n=0.5 for very leaky buildings, n=0.7 for tight buildings). This is a clean design that doesn't bake the exponent into the function signature.

**Code Location**: `crates/hares-physics/src/infiltration.rs:79–81`

**Root Cause**: N/A — correct implementation.

**Impact**: None. Verified by the unit test `ach_nat_to_ach50_conversion_matches_ashrae_119` (line 727) and the zero-input and linearity tests (lines 735–753).

---

### Finding 3: [Severity: low] Exponent applied correctly in AIM-2 (Walker & Wilson 1998) throughout the coefficient pipeline

**Description**: The exponent `n_i` is applied consistently in all three stages of the AIM-2 model:
1. **Flow coefficient** (line 492): `C = Q_50 / 50^n_i` — correct power-law flow coefficient from blower-door measurement
2. **Stack/wind shape factors** `f_s`, `f_w` (lines 506–544): All Walker & Wilson (1998) Eq. 9–25 expressions involving `n_i` are faithfully reproduced, including:
   - Stack neutral-plane factor `m_o` using `(2·n_i + 1)` (line 506)
   - Stack factor `f_s` using `(1 + n_i·r_i)/(n_i + 1)` and `(0.5 - ...)^(n_i + 1)` (line 524)
   - Wind factor `f_w` using `(2 - n_i)` multiplier and `r_x = 1 - r_i·(n_i/2 + 0.2)` (lines 534–543)
   - Flue correction using `n_i·y_i·(...)·(1 - 3·(x_c - x_i)²·r_i^(1 - n_i)/...)` (lines 516–518)
3. **Coefficient scaling** (lines 528, 548): `Cs = f_s × (ρ·g·H/T)^n_i`, `Cw = f_w × (ρ/2)^n_i`
4. **Runtime flow** at `ashrae_wind_stack()` (lines 142–143): `Q_stack = c_s × |ΔT|^n_i`, `Q_wind = c_w × (shelter·v)^(2·n_i)`

**Code Location**: `crates/hares-physics/src/infiltration.rs:479–576`

**Root Cause**: N/A — correct implementation; OCHRE cross-validation at `aim2_ochre_cross_validation` (line 1548) confirms <0.5% relative error in c_s, c_w, and exact match for shelter_coeff.

**Impact**: None. The exponent is applied correctly throughout the pipeline.

---

### Finding 4: [Severity: medium] EnergyPlus Sherman-Grimsrud model uses fixed square-root exponent (n=0.5), EnergyPlus AIM-2 uses user-specified `PressureExponent`; HARES AIM-2 hardcodes 0.65

**Description**: EnergyPlus implements two distinct infiltration models:
- **Sherman-Grimsrud** (`ZoneInfiltration:EffectiveLeakageArea`): `Q = (ELA/1000) × √(Cs·|ΔT| + Cw·v²)` — fixed square-root exponent, no `n_i` parameter. This is equivalent to an exponent of 0.5.
- **AIM-2** (`ZoneInfiltration:FlowCoefficient`): `Q = √((C·Cs·|ΔT|^n)² + (C·Cw·(sf·v)^(2n))²)` where `n` is `PressureExponent` from user input field 3. No default value is assigned — it's user-specified.

HARES does not implement EnergyPlus's Sherman-Grimsrud model directly. HARES has:
- **AIM-2** via `ashrae_wind_stack()` with configurable `n_i` (clamped to [0.5, 0.7]) — equivalent to EnergyPlus's AIM-2 path but with `N_I_DEFAULT=0.65` hardcoded in the solver builder
- **ELA** via `ela_infiltration()` with fixed exponent 0.5 — equivalent to EnergyPlus's Sherman-Grimsrud path

The naming in the HARES codebase is slightly confusing: `ashrae_wind_stack()` is the AIM-2 model (not Sherman-Grimsrud), and the doc comment on line 26 references "ASHRAE HOF 2017, Ch. 16" rather than explicitly naming "AIM-2 Enhanced" as EnergyPlus does. However, the implementation is correct.

**Code Location**:
- EnergyPlus: `ZoneEquipmentManager.cc:6771–6801`, `HeatBalanceAirManager.cc:954, 1044`
- HARES: `infiltration.rs:133–145` (AIM-2 runtime), `solver_builder.rs:995` (hardcoded N_I_DEFAULT)

**Root Cause**: EnergyPlus treats `PressureExponent` as a user-input field; HARES treats it as a physics constant. This is a deliberate simplification for HARES's stock-modelling use case (batch simulation of existing housing stock where blower-door exponents are typically unknown).

**Impact**: Low direct impact (0.65 is the right default for the US stock), but users modelling tight or very leaky buildings cannot adjust the exponent to match site-specific blower-door test data. The ~15–30% error claimed in the doc comment (line 18) cannot be avoided for individual buildings.

---

### Finding 5: [Severity: medium] n_i is a global constant with no per-building override path

**Description**: Although the entire `n_i` plumbing is fully configurable throughout the stack (`Aim2Params.n_i` → `aim2_coefficients_from_ach50()` → `Aim2Coefficients.n_i` → `InfiltrationMethod::AshraeWindStack.n_i` → `ashrae_wind_stack()`), the value is hardcoded to `N_I_DEFAULT` (0.65) at the point where input data maps to solver configuration:

```rust
// crates/hares-core/src/dwelling/solver_builder.rs:995
n_i: N_I_DEFAULT,
```

There is no HPXML field, no building attribute, and no configuration option that allows a per-building override. The HPXML `<AirLeakage>` parser similarly uses `ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT)` without the ability to specify a custom exponent:

```rust
// crates/hares-io/src/hpxml/building.rs:2284
let converted = raw.map(|v| ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT));
```

For comparison, EnergyPlus allows per-building specification via the `PressureExponent` field on `ZoneInfiltration:FlowCoefficient`.

**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:995`, `crates/hares-io/src/hpxml/building.rs:2284`

**Root Cause**: HARES targets batch stock modelling and omits this parameter for simplicity; the HPXML schema does not define a dedicated flow exponent field.

**Impact**: Buildings with atypical leakage characteristics (e.g., very tight Passivhaus construction at n≈0.7, or very leaky older stock at n≈0.5) cannot use site-specific exponents. For a tight building with n=0.7 modelled at n=0.65, the Q_50 → C conversion error alone is ~3–5%; combined with the runtime ΔT^n and v^(2n) shape errors, the total natural infiltration estimate error can reach the ~15–30% range cited in the doc comment. This is acceptable for stock-level analysis but limits use for detailed single-building calibration.

---

### Finding 6: [Severity: low] ELA path correctly uses fixed exponent of 0.5 (square root) per ASHRAE 62.2

**Description**: The `ela_infiltration()` function uses `driver.sqrt()` with a fixed exponent of 0.5, as documented in the comment on line 150: "Uses a fixed square-root (exponent = 0.5) per ASHRAE 62.2 and OCHRE's `_ela` path." This is correct — ASHRAE 62.2's ELA method assumes orifice flow (n=0.5) because the ELA is defined at 4 Pa with Cd=1.0. The distinction between the n_i-configurable AIM-2 path and the fixed-exponent ELA path is clearly documented (lines 3–23, 148–153).

The EnergyPlus ShermanGrimsrud model (ZoneInfiltration:EffectiveLeakageArea) also uses a fixed square-root exponent (no configurable `PressureExponent`), making this consistent with vendor practice.

**Code Location**: `crates/hares-physics/src/infiltration.rs:173`

**Root Cause**: N/A — correct by design.

**Impact**: None.

---

### Finding 7: [Severity: low] No sensitivity analysis documented for the exponent choice

**Description**: The module doc comment (line 18) states "Using 0.5 instead of 0.65 for a tight building causes ±15–30 % error in infiltration airflow." While this claim is broadly correct per the literature (Walker & Wilson 1998, Sherman 1998), no sensitivity analysis is presented to support the specific 15–30% magnitude or to characterise the impact under different climate conditions, building heights, or shelter classes.

The unit tests verify:
- Monotonicity: higher n_i → higher flow when ΔT>1 (line 786), lower flow when ΔT<1 (line 800)
- Clamping: values outside [0.5, 0.7] are clamped (line 814)
- OCHRE cross-validation: <0.5% error vs OCHRE reference at n_i=0.65 (line 1548)

But no parametric sweep quantifies the sensitivity of natural ACH to n_i variation. A 3×3 sensitivity matrix (n_i ∈ {0.50, 0.65, 0.70} × ΔT ∈ {5, 15, 30} K × v ∈ {0, 4, 8} m/s) would provide evidence for the 15–30% claim and help users understand when the default is or isn't adequate.

**Code Location**: `crates/hares-physics/src/infiltration.rs:17–18`

**Root Cause**: Sparse documentation — the claim is cited from known literature but not quantified within the codebase's parameter space.

**Impact**: Low — the default value is correct and the claim is directionally accurate. A formal sensitivity analysis would increase confidence in the stock-modelling approach.

---

## Summary

- **Total findings**: 7
- **Critical**: 0
- **High**: 0
- **Medium**: 2 (Finding 4: EnergyPlus model naming/classification; Finding 5: no per-building n_i override)
- **Low**: 5

## Recommendations

1. **Add per-building n_i override**: Expose a `pressure_exponent` field in the building configuration (e.g., HPXML extension or solver builder parameter) that, when specified, overrides the hardcoded `N_I_DEFAULT` at `solver_builder.rs:995`. This would enable calibration against site-specific blower-door test data. Keep the default at 0.65 for stock modelling.

2. **Rename `ashrae_wind_stack` → `aim2_wind_stack` or document the distinction clearly**: The current name is ambiguous — it is the AIM-2 model, not the Sherman-Grimsrud model. The module-level doc (line 111) says "Implements the AIM-2 model" so the function doc is correct, but the function name is misleading relative to EnergyPlus naming where "ASHRAE" → Sherman-Grimsrud and "AIM-2" is explicit.

3. **Add sensitivity analysis test**: A parametric test sweeping n_i over {0.50, 0.65, 0.70} with typical US climate conditions would quantify the error bounds on the 15–30% claim and serve as a reference for users considering custom exponents.

4. **Consider exposing `PressureExponent` in the HPXML natural-to-50 Pa conversion**: The HPXML parser at `building.rs:2284` could accept an optional `<extension>` field to specify the flow exponent for blower-door extrapolation, defaulting to 0.65. This is low priority since HPXML schema doesn't standardise this field.

## References / Citations

- ASHRAE Standard 119-1988, "Air Leakage Performance for Detached Single-Family Residential Buildings"
- ASTM E779-19, "Standard Test Method for Determining Air Leakage Rate by Fan Pressurization"
- Walker, I.S. and Wilson, D.J. (1998). "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations." *HVAC&R Research*, 4(2), pp. 119–139.
- ASHRAE Handbook of Fundamentals (2017), Chapter 16: Ventilation and Infiltration
- ASHRAE Handbook of Fundamentals (2021), Chapter 16: Ventilation and Infiltration
- EnergyPlus Engineering Reference, §15.4: Infiltration (AIM-2 Enhanced Model)
- Sherman, M.H. (1998). "The use of blower-door data." *Indoor Air*, 8(2), pp. 71–80.
- OCHRE (NREL): `ochre/utils/envelope.py:488-633` — `calculate_ashrae_infiltration_params`
- EnergyPlus source: `HeatBalanceAirManager.cc:920–1046`, `ZoneEquipmentManager.cc:6771–6801`, `DataHeatBalance.hh:158–163,1100–1116`
