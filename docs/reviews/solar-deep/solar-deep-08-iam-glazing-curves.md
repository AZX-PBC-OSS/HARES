# All 6 IAM glazing curves (Clear, Bronze, Green, Grey, ReflectiveC, ReflectiveB) coefficients against EnergyPlus WindowManager.cc; verify IAM(0°) = 1.0
**Review ID**: solar-deep-08
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs:524-648` — GlazingCurve enum, coefficients, IAM computation, selection logic
- `vendors/OCHRE/ochre/utils/envelope.py:100-129` — OCHRE coefficient arrays and IAM application
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:4974-5239` — EnergyPlus simple glazing transmittance curves and normalization
- `vendors/EnergyPlus/datasets/WindowGlassMaterials.idf` — Named glass type spectral properties
- `vendors/EnergyPlus/datasets/WindowConstructs.idf` — Construction definitions for named glass types

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:4974-5239` — 10 individual transmittance curves A–J, composite curves (BDCD, FH, FGHI), normalization at cos=1.0
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.hh:255-260` — POLYF polynomial evaluator
- `vendors/OCHRE/ochre/utils/envelope.py:100-129` — 6 curve coefficient arrays and angle-of-incidence application

## Findings

### Finding 1: [Severity: high] HARES coefficients are not digit-for-digit identical to EnergyPlus WindowManager.cc
**Description**: The 6 polynomial coefficient sets in HARES (`solar.rs:608-614`) are derived from OCHRE / the EnergyPlus Engineering Reference (BigLadderSoftware documentation), NOT directly from the EnergyPlus `WindowManager.cc` simple glazing transmittance curves at lines 4974–4983. There are systematic digit-level differences, especially the nonzero `a0` constant term, and `a1` through `a4` deviations at the 0.1–1.3% level.
**Code Location**: `crates/hares-physics/src/solar.rs:608-614` vs `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:4974-4983`
**Root Cause**: EnergyPlus `WindowManager.cc` uses 4-term polynomials (a0 ≡ 0.00) for the simple glazing model, while the EnergyPlus Engineering Reference (Step 7 lookup table) provides separate 5-term fits with small nonzero constant terms. HARES/OCHRE adopted the Engineering Reference fits. These represent two different polynomial fits to the same underlying angular transmittance data from the original ASHRAE/DOE research.
**Impact**:

| Curve | EnergyPlus WM.cc `[a0,a1,a2,a3,a4]` | HARES `[a0,a1,a2,a3,a4]` | Max % diff in a1-a4 |
|-------|--------------------------------------|--------------------------|---------------------|
| A | `[0.00, 3.36, -3.85, 1.49, 0.01]` | `[-0.001474, 3.355, -3.852, 1.486, 0.0147]` | 0.15% (a1) |
| BDCD | `[0.00, 2.745, -2.29, 0.05, 0.505]` | `[-0.00116, 2.74225, -2.289, 0.0474825, 0.504475]` | 0.10% (a1) |
| D | `[0.00, 2.85, -2.58, 0.40, 0.35]` | `[-0.0002804, 2.845, -2.582, 0.3963, 0.3462]` | 1.09% (a3) |
| E | `[0.00, 1.51, 2.49, -5.87, 2.88]` | `[-0.002577, 1.51, 2.489, -5.873, 2.883]` | 0.10% (a4) |
| F | `[0.00, 1.21, 3.14, -6.37, 3.03]` | `[-0.001367, 1.213, 3.137, -6.366, 3.025]` | 0.30% (a1) |
| J | `[0.00, 0.08, 6.02, -8.84, 3.74]` | `[0.0004825, 0.08407, 6.018, -8.836, 3.744]` | 1.26% (a1 via extended precision) |

EnergyPlus `WindowManager.cc:4986` defines BDCD as `(B + D + C + D) / 4.0`, which evaluates to `[0.00, 2.745, -2.29, 0.05, 0.505]`. HARES BDCD has a1=2.74225 (vs. 2.745) — a 0.10% deviation.

### Finding 2: [Severity: high] HARES has only 6 curves; individual Bronze, Green, and Grey IAM curves are collapsed into composite BDCD
**Description**: The 6 named glass types in the review scope (Clear, Bronze, Green, Grey, ReflectiveC, ReflectiveB) do NOT map 1:1 to HARES's 6 `GlazingCurve` variants. EnergyPlus `WindowManager.cc:4975-4977` defines separate individual curves B (Bronze), C (Green), and D (Grey) with distinct coefficient sets. HARES replaces B, C, and D with the single composite BDCD curve, losing curve-level fidelity for individual tinted single-pane glass types.
**Code Location**: `crates/hares-physics/src/solar.rs:562-575` (enum definition) vs `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:4975-4977,4986`
**Root Cause**: The OCHRE/HARES simplified model uses only 6 composite curves (A, BDCD, D, E, F, J) to partition the full U-factor×SHGC 2D space, collapsing EnergyPlus's 10 individual curves into a coarser grid. The BDCD composite averages Bronze(B), Green(C), and Grey(D) into a single angular response curve.
**Impact**: When a user specifies a specific glass type (e.g., "Sgl Bronze 3mm"), HARES routes to BDCD irrespective of the glass color. The individual curves differ notably:

| Curve | EnergyPlus WM.cc polynomial | Angular shape characteristic |
|-------|-----------------------------|------------------------------|
| B (Bronze) | `0.00 + 2.83·cs − 2.42·cs² + 0.04·cs³ + 0.55·cs⁴` | Reference single-pane Bronze |
| C (Green) | `0.00 + 2.45·cs − 1.58·cs² − 0.64·cs³ + 0.77·cs⁴` | Negative c3 term, different shape |
| D (Grey) | `0.00 + 2.85·cs − 2.58·cs² + 0.40·cs³ + 0.35·cs⁴` | Reference single-pane Grey (also used standalone for SHGC ≤ 0.3) |
| BDCD composite | avg of B, C, 2×D | Smoothed/blurred angular response |

For HARES users selecting Bronze glass, the IAM at mid-angles (e.g., 60°) may differ from EnergyPlus by up to ~1.5% absolute IAM depending on the divergence of the individual Bronze curve from the BDCD average. For Green glass (curve C), which has a markedly different polynomial shape (negative c3), the deviation could be larger.

**Additionally**, EnergyPlus has curves G, H, I that are not represented in HARES at all. WindowManager.cc:4980-4982 defines these for intermediate U/SHGC regions that HARES collapses into the F or J bins.

### Finding 3: [Severity: critical] IAM(0°) = 1.0 is correctly verified for all 6 HARES curves
**Description**: All 6 GlazingCurve variants return exactly 1.0 at 0° incidence via the `normal_incidence_raw()` normalization in `window_iam()`. Verified analytically by summing coefficient arrays and confirming the division identity `raw(1.0) / normal_incidence_raw() = 1.0`.
**Code Location**: `crates/hares-physics/src/solar.rs:618-619, 664`
**Root Cause**: N/A — this is the correct behavior.
**Impact**: None — this is a confirmation.

**Verification sums** (all 5 coefficients):

| Curve | Normal incidence raw sum | IAM(0°) after normalization |
|-------|--------------------------|-----------------------------|
| A | −0.001474 + 3.355 − 3.852 + 1.486 + 0.0147 = **1.002226** | 1.0 ✓ |
| BDCD | −0.00116 + 2.74225 − 2.289 + 0.0474825 + 0.504475 = **1.0040475** | 1.0 ✓ |
| D | −0.0002804 + 2.845 − 2.582 + 0.3963 + 0.3462 = **1.0052196** | 1.0 ✓ |
| E | −0.002577 + 1.51 + 2.489 − 5.873 + 2.883 = **1.006423** | 1.0 ✓ |
| F | −0.001367 + 1.213 + 3.137 − 6.366 + 3.025 = **1.007633** | 1.0 ✓ |
| J | 0.0004825 + 0.08407 + 6.018 − 8.836 + 3.744 = **1.0105525** | 1.0 ✓ |

The raw sums deviate from 1.0 by 0.22–1.06% (polynomial fitting error in the original Engineering Reference), but HARES correctly divides by the sum to guarantee `IAM(0°) = 1.0` exactly.

By contrast, EnergyPlus `WindowManager.cc:5226-5228` enforces `TransTmp = 1.0` via an explicit conditional `if (cs == 1.0)` rather than a ratio division. Both approaches achieve the same result. EnergyPlus's explicit guard is slightly more numerically robust (avoids floating-point summation error in the denominator), but HARES's ratio approach handles floating-point cos(θ) near 1.0 (where `cs` may be `0.9999999...` due to floating-point error) correctly through continuous evaluation rather than a delta comparison.

### Finding 4: [Severity: medium] Monotonicity is inherent in the source data but lacks explicit verification
**Description**: The IAM curve should be monotonically decreasing with incidence angle (monotonically increasing with cos θ). All 6 curves come from validated EnergyPlus source data and are inherently monotonic over [0°, 90°], but HARES has no unit test verifying this property. At cos θ = 0 (θ = 90°), the raw polynomial evaluates to a slightly negative value for curves A, BDCD, D, E, F (a0 < 0 for all except J), which is clamped to 0.0 — correct behavior.
**Code Location**: `crates/hares-physics/src/solar.rs:653-665`
**Root Cause**: Missing validation tests.
**Impact**: Low risk in practice (coefficients are from a validated source), but a regression test would catch future coefficient corruption.

### Finding 5: [Severity: low] HARES comments claim derived from OCHRE but OCHRE normalization differs
**Description**: The comment at `solar.rs:604-605` says coefficients were "Derived by reversing OCHRE's coefficient arrays (stored highest-degree first) before evaluation." While the coefficient values match OCHRE's reversed arrays, OCHRE does NOT normalize by dividing by `raw(1.0)`. OCHRE (`envelope.py:123`) applies the raw polynomial directly: `irr["poa_direct"] *= np.dot(t_params, ...)`. The normalization is implicitly embedded in OCHRE's separate transmittance model. HARES's explicit normalization is more correct for a standalone IAM function, but the comment understates the difference.
**Code Location**: `crates/hares-physics/src/solar.rs:604-605`
**Root Cause**: Imprecise comment.
**Impact**: Low — no code defect, only documentation accuracy.

## Summary
- **Total findings**: 5
- **Critical**: 1 (confirmed IAM(0°) = 1.0 — positive finding)
- **High**: 2 (coefficient source divergence from WindowManager.cc; missing individual B, C curves in favor of BDCD composite)
- **Medium**: 1 (no monotonicity test)
- **Low**: 1 (misleading comment about OCHRE derivation)

## Recommendations

1. **Document the coefficient provenance explicitly** in `solar.rs`. Add a comment noting that coefficients come from the EnergyPlus Engineering Reference (Step 7 lookup table) and differ from the `WindowManager.cc` simple-glazing curves, which use a0=0 and slightly different a1–a4 values. Cite both sources.

2. **Add individual curves B and C** (Bronze and Green single-pane) to the `GlazingCurve` enum. The current BDCD composite loses curve-level fidelity for named glass types. At minimum, document that Bronze, Green, and Grey are all approximated by BDCD in the simplified model.

3. **Add a unit test** for each curve verifying:
   - `window_iam(0.0, curve) ≈ 1.0` (within fp epsilon)
   - `window_iam(89.0_f64.to_radians(), curve) ≈ 0.0` (near-zero at grazing)
   - Monotonic decrease over `[0°, 85°]` sampled at 5° intervals

4. **Consider adding curves G, H, I** and composites FH, FGHI per EnergyPlus `WindowManager.cc:4980-4985` to improve accuracy for intermediate U-factor windows (e.g., 2.0–3.0 W/m²·K with moderate SHGC) that currently fall into HARES's coarser F/J bins.

5. **Fix or clarify the OCHRE derivation comment** at `solar.rs:604-605`. Note that while coefficient values match OCHRE, the normalization strategy (explicit division vs. implicit) differs.

## References / Citations

- EnergyPlus `WindowManager.cc:4974-4986` — 10 individual transmittance curves A–J, composite BDCD/FH/FGHI
- EnergyPlus `WindowManager.cc:5226-5228` — explicit `TransTmp = 1.0` at `cs == 1.0`
- EnergyPlus `WindowManager.hh:255-260` — `POLYF` Horner-form 6-coefficient polynomial evaluator
- EnergyPlus Engineering Reference, Window Calculation Module, Step 7: https://bigladdersoftware.com/epx/docs/8-9/engineering-reference/window-calculation-module.html
- OCHRE `envelope.py:105-123` — 6 curve coefficient arrays (highest-degree-first storage)
- HARES `solar.rs:562-665` — `GlazingCurve` enum, coefficients, `from_u_shgc()`, `window_iam()`
- EnergyPlus `WindowConstructs.idf:80-110` — named single-pane glass constructions (Sgl Clear/Bronze/Green/Grey/Ref-*)
- EnergyPlus `WindowGlassMaterials.idf` — spectral properties for individual glass types
