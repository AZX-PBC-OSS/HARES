# Window transmitted solar (SHGC-based): verify Q = I·SHGC·A; check whether SHGC accounts for IAM at normal incidence
**Review ID**: solar-deep-07
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs:487-522` — isotropic tilted irradiance (fallback)
- `crates/hares-physics/src/solar.rs:524-527` — `window_transmitted_solar` (flat SHGC)
- `crates/hares-physics/src/solar.rs:530-665` — `window_iam`, `GlazingCurve`, angular model
- `crates/hares-physics/src/solar.rs:799-823` — `window_transmitted_solar_angular`
- `crates/hares-physics/src/solar.rs:1886-1975` — unit tests for IAM and angular vs flat

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:6309-6382` — `EvalNominalWindowCond` (SHGC at normal incidence)
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:4888-4969` — `TransAndReflAtPhi` (simple-glazing angular model)
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:7263-7553` — runtime beam solar transmission
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.hh:255` — `POLYF` (6th-order angle-dependent polynomial)
- `vendors/OCHRE/ochre/utils/envelope.py:54-134` — `calculate_plane_irradiance` (IAM curves)
- `vendors/OCHRE/ochre/utils/envelope.py:405-431` — `calculate_window_parameters`
- `vendors/OCHRE/ochre/Models/Envelope.py:215-218` — window optical property derivation

## Findings

### Finding 1: [Severity: low]
**Description**: `window_transmitted_solar` computes Q = I · SHGC · A without any angle-of-incidence modifier (IAM). The function is retained solely for testing and is never called from production runtime code.

**Code Location**: `crates/hares-physics/src/solar.rs:525-527`

```rust
pub fn window_transmitted_solar(irradiance_w_m2: f64, shgc: f64, area_m2: f64) -> f64 {
    irradiance_w_m2 * shgc * area_m2
}
```

**Root Cause**: This is intentional. The function is a "flat" reference implementation — analogous to the `isotropic_tilted_irradiance` fallback at lines 489-522 — retained for linearity tests (lines 1104-1110) and as a baseline comparator for the angular model in tests (lines 1899, 1918, 2070). Every runtime call path uses `window_transmitted_solar_angular` (line 812), which correctly applies EnergyPlus step-7 IAM curves.

**Impact**: None at runtime. If a future caller mistakenly used `window_transmitted_solar` instead of `window_transmitted_solar_angular`, the result would overestimate transmitted solar at off-normal incidence angles — by ~10% at 60° and ~50–60% at 80° per the pinned IAM values at lines 1960-1975.

---

### Finding 2: [Severity: low]
**Description**: The SHGC value is correctly interpreted as the normal-incidence (rated) SHGC, consistent with EnergyPlus and NFRC conventions. A separate IAM correction is correctly applied in the angular function.

**Code Location**:
- SHGC-as-normal-incidence: `crates/hares-physics/src/solar.rs:806` (parameter docstring: "solar heat gain coefficient at normal incidence")
- IAM normalization: `crates/hares-physics/src/solar.rs:660-664` (divides raw polynomial by `normal_incidence_raw()` so `window_iam(0) = 1.0`)
- Angular application: `crates/hares-physics/src/solar.rs:820-822`

```rust
// Line 806: shgc documented as normal-incidence value
shgc: f64,  // solar heat gain coefficient at normal incidence

// Line 820-822: separate IAM applied to beam and diffuse
let beam_gain = beam_irradiance_w_m2 * shgc * window_iam(angle_of_incidence_rad, curve);
let diffuse_gain = diffuse_irradiance_w_m2 * shgc * curve.diffuse_iam();
```

**Root Cause**: Design correctly follows the EnergyPlus convention. In EnergyPlus (`EvalNominalWindowCond`, `WindowManager.cc:6309-6382`), SHGC is computed at normal incidence: `SHGC = Σ(AbsBeamNorm[i] × thermal_ratio[i]) + TSolNorm`. The rated (NFRC) SHGC = SHGC at normal incidence. At runtime, EnergyPlus applies angle-dependent transmittance via `POLYF(CosInc, TransSolBeamCoef)` — a 6th-order polynomial ratio that serves as the IAM. HARES expresses this same separation: SHGC encodes the normal-incidence optical+thermal performance, and `window_iam()` (the EnergyPlus step-7 degree-4 polynomial normalized to 1.0 at cos=1) encodes the angular modifier.

**Verification**: The test at lines 1887-1904 confirms `window_transmitted_solar_angular(beam, 0, shgc, area, 0.0, curve)` equals `window_transmitted_solar(beam, shgc, area)` at normal incidence (within 1e-9), because `window_iam(0) = 1.0`. The test at lines 1906-1923 confirms angular < flat at θ = 60° (off-normal reduction).

**Impact**: Correct behavior — no error. The implementation is consistent with both EnergyPlus and OCHRE on the semantic meaning of SHGC and the necessity of a separate IAM.

---

### Finding 3: [Severity: low]
**Description**: The `window_iam` function applies an IAM that is a *transmittance* modifier (ratio T(θ)/T(0)), not a full *SHGC* modifier. This is appropriate for beam radiation gain but slightly overestimates the inward-flowing absorbed component at off-normal angles. OCHRE applies the same simplification.

**Code Location**: `crates/hares-physics/src/solar.rs:820`

```rust
let beam_gain = beam_irradiance_w_m2 * shgc * window_iam(angle_of_incidence_rad, curve);
```

**Root Cause**: In EnergyPlus, SHGC = T_sol + Σ(N_i × A_sol_i). The angular dependence differs between the transmittance component (T_sol) and the absorbed+re-emitted component (N_i × A_sol_i). EnergyPlus tracks these separately at runtime via per-layer absorptance polynomials (`POLYF(CosInc, AbsSolBeamCoef[i])`). OCHRE — and HARES following OCHRE — applies the same transmittance-IAM to the full SHGC, which implicitly assumes the absorbed component has the same angular profile as the transmitted component. For most glazing types this approximation is within the simple-window model's tolerance.

**Impact**: Minor overestimation of beam solar gain at high incidence angles (likely < 5% for typical glazing). This is consistent with OCHRE's approach and is inherent to using the simple window model rather than EnergyPlus's full layered glass model. The alternative would require the EnergyPlus detailed window module, which is outside HARES's current scope.

---

## Summary
- **Total findings**: 3
- **Critical**: 0
- **High**: 0
- **Medium**: 0
- **Low**: 3

## Recommendations

1. **Consider marking `window_transmitted_solar` as `#[cfg(test)]` or `#[doc(hidden)]`** to prevent accidental runtime use. Currently it's a public API without runtime callers, but no compile-time guard prevents future misuse.

2. **Document the transmittance-IAM-as-SHGC-IAM approximation** in the docstring of `window_transmitted_solar_angular` or `window_iam`. Note that while EnergyPlus tracks absorbed and transmitted solar with separate angular polynomials, HARES follows OCHRE in applying the transmittance IAM to the full SHGC — a simplification justified for the simple window model.

3. **Consider a future enhancement** to split SHGC into transmittance and absorbed components (already available via `calculate_window_parameters` at `solar.rs:726-797`) and apply the IAM only to the transmittance portion of beam gain, while using a hemispherical-weighted IAM for the absorbed portion. This would more closely match EnergyPlus's layered approach. The current approximation is adequate for simple glazing systems.

## References / Citations

1. EnergyPlus Engineering Reference, Window Calculation Module, Step 7 "Determine Angular Performance": polynomial coefficients for curves A, BDCD, D, E, F, J as functions of U-factor and SHGC. HARES `GlazingCurve::from_u_shgc` (solar.rs:579-598) and `coefficients()` (solar.rs:606-615).

2. EnergyPlus `EvalNominalWindowCond` (WindowManager.cc:6309-6382): SHGC computed at normal incidence using `AbsBeamNorm[i]` (layer i beam absorptance at normal incidence) and `TSolNorm` (normal transmittance). This establishes `SHGC_rated = SHGC at normal incidence`.

3. EnergyPlus `POLYF` (WindowManager.hh:255): 6th-order polynomial in cos(θ) that returns tau(θ)/tau(0) — the IAM for transmittance. At runtime in SolarShading.cc:7263, `TBmBm = POLYF(CosInc, TransSolBeamCoef)` computes angle-dependent beam transmittance.

4. OCHRE `calculate_window_parameters` (envelope.py:405-431): Steps 4-5 of EnergyPlus simple window model, deriving transmittance from SHGC and U-factor. HARES implements the same at solar.rs:726-797.

5. OCHRE `calculate_plane_irradiance` (envelope.py:105-123): Applies IAM polynomial to beam irradiance using `np.dot(t_params, [cos^0, cos^1, cos^2, cos^3, cos^4])`. HARES equivalent at solar.rs:653-665.

6. ASHRAE Fundamentals, Chapter 15: `SHGC = T_sol + A_sol × N_i`, where `N_i` is the inward-flowing fraction of absorbed solar. EnergyPlus tracks angular dependence of T and A separately; HARES/OCHRE apply the transmittance IAM to the full SHGC.
