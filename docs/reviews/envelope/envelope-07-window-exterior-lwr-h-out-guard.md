# Window exterior LWR h_out guard threshold and NFRC fallback
**Review ID**: envelope-07
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/longwave.rs`
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-envelope/src/boundary_rc.rs`
- `crates/hares-physics/src/film_coefficients.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py`
- `vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.cc`
- `vendors/EnergyPlus/src/EnergyPlus/DataHeatBalance.hh`
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc`

## Findings

### Finding 1: [Severity: medium]
**Description**: The exterior film coefficient guard threshold `> 0.0` at `longwave.rs:86` permits near-zero but positive `h_out` values to produce physically implausible LWR corrections because `h_out` appears in the denominator of the T_eff scaling factor at `longwave.rs:106`. For example, `h_out = 0.01` at clear-night conditions (Δq ≈ −16 W/m², U = 3, A = 12 m²) yields ΔQ = −57,600 W — three orders of magnitude larger than the physically expected ~−17 W correction.

**Code Location**: `crates/hares-envelope/src/thermal_solver/longwave.rs:86-106`

**Root Cause**: The window exterior LWR correction uses an effective-temperature approach:
```
T_eff = T_air + Δq / h_out
ΔQ_zone = (U / h_out) × Δq × A
```
where `h_out` is the exterior film coefficient derived from boundary film resistance. The formula is valid when `h_out` is a physically meaningful convective coupling, but it diverges as `h_out → 0` because:
1. The boundary film resistance `r_film_exterior_m2_k_w` comes from the DOE-2 model (`film_coefficients.rs:280-300`) via `h_out = h_natural + h_forced` with `h_natural = 1.31 × ΔT^(1/3)` for vertical surfaces.
2. At zero wind and ΔT → 0, `h_natural → 0` and the DOE-2 forced component also → 0 (since `h_glass − h_natural → 0`).
3. The solver_builder already guards against zero film resistance at construction time (`solver_builder.rs:724`: `r_film_exterior_m2_k_w > 1e-9`), but this only prevents infinite `h_out`, not near-zero `h_out`.
4. At runtime, the guard at `longwave.rs:86` (`> 0.0`) accepts any positive value including `0.01`, producing correction factors >3000× relative to the NFRC fallback.

The issue is not theoretical — the DOE-2 exterior film model can produce `h_out` as low as ~0.6 W/(m²·K) at ΔT = 0.1°C with zero wind (`1.31 × 0.1^(1/3) ≈ 0.61`), and could go lower with certain roughness factors.

**Impact**: In very-low-wind, small-ΔT conditions (e.g., mild overcast nights, windows facing unconditioned buffer spaces), the T_eff correction can produce enormous erroneous heat flows that:
- Artificially inflate zone heating loads
- Cause numerical instability in the thermal solver
- Produce physically impossible results (a 12 m² window cannot lose 57 kW to sky radiation)

The impact is low-probability (requires specific edge-case conditions) but high-severity when triggered (orders-of-magnitude energy balance errors).

### Finding 2: [Severity: low]
**Description**: EnergyPlus enforces a `LowHConvLimit = 0.1` W/(m²·K) on all **interior** convection coefficients (`DataHeatBalance.hh:1790`; applied at `ConvectionCoefficients.cc:1967, 2040, 6445` and in `WindowManager.cc:2277-2279`). However, EnergyPlus does **not** apply this floor to exterior convection coefficients — its exterior heat balance does not divide by the exterior coefficient in a way that would amplify errors, so a zero-natural-convection exterior value is numerically safe in E+. HARES uses `h_out` as a divisor in the window T_eff correction, creating a uniquely dangerous division path that EnergyPlus's exterior convection floor (or lack thereof) was never designed to protect against.

**Code Location**: `crates/hares-envelope/src/thermal_solver/longwave.rs:106`

**Root Cause**: Architectural mismatch between the EnergyPlus-style exterior convection model (where h_out → 0 means zero convection, which is physically correct) and the HARES T_eff window correction (where h_out → 0 means infinite correction, which is unphysical). The HARES code inherited the E+ convention of not flooring exterior coefficients but added a division-by-h_out operation that E+ never performs.

### Finding 3: [Severity: low]
**Description**: OCHRE has no equivalent of the window exterior LWR T_eff correction and therefore no equivalent guard. OCHRE's `_solve_exterior_radiation` (`Envelope.py:125-163`) and `BoundarySurface.calculate_exterior_radiation` (`Envelope.py:274-302`) handle opaque surface LWR with an iterative surface-temperature solve — no h_out division exists in OCHRE's window path. OCHRE derives window gains from transmitted solar (SHGC × area × POA) and conductive heat transfer through the U-factor, placing the sky-temperature effect entirely in the opaque surface exterior LWR solution, not in a separate window correction. This means the HARES window T_eff approach is a HARES-specific extension with no direct OCHRE precedent to consult for the guard threshold value.

**Code Location**: `crates/hares-envelope/src/thermal_solver/longwave.rs:66-106` (HARES T_eff approach); `vendors/OCHRE/ochre/Models/Envelope.py:125-163` (OCHRE opaque-only exterior LWR)

### Finding 4: [Severity: low]
**Description**: The `H_OUT_NFRC` constant name is misleading. The value `34.0` W/(m²·K) (`film_coefficients.rs:36`) is the ASHRAE conventional combined (conv + rad) exterior coefficient for peak heating load at ~15 mph wind (ASHRAE HoF 2021 Ch. 15, Table 1), **not** the NFRC 100/ISO 15099 convective boundary condition (which is 26 W/(m²·K) convective-only at 5.5 m/s, or ~29 W/(m²·K) when radiative is included). The constant's doc comment at `film_coefficients.rs:21-35` correctly documents this distinction, but the name `H_OUT_NFRC` still implies the NFRC rating-condition value. The name was chosen during the original guard implementation when it was believed to be the NFRC value; the discovery that the 34.0 value is actually ASHRAE conventional (not NFRC) was documented but the name was not updated. This naming mismatch creates maintenance risk — a future developer seeing `H_OUT_NFRC` may assume it's the NFRC 100 rating coefficient and change dependent logic accordingly.

**Code Location**: `crates/hares-physics/src/film_coefficients.rs:36`

## Summary
- Total findings: **4**
- Critical / High / Medium / Low: **0 / 0 / 1 / 3**

## Recommendations

1. **Raise the guard threshold from `> 0.0` to a physically grounded minimum.** The current threshold of `> 0.0` is too permissive — it allows h_out values that produce non-physical corrections. Options, in order of preference:

   - **`1.0` W/(m²·K)** ([Preferred] Minimum natural convection at ΔT ≈ 0.4°C for a vertical surface. This is the natural convection floor for any surface with non-negligible indoor-outdoor temperature difference. Values below 1.0 produce correction factors >34× relative to the NFRC fallback, which exceeds the uncertainty introduced by substituting the ASHRAE tabulated value.)
   - **`0.5` W/(m²·K)** (Minimum natural convection at ΔT ≈ 0.1°C. The existing test at `longwave.rs:908` exercises this exact value. If this threshold is chosen, the test continues to pass without changes.)
   - **`0.1` W/(m²·K)** (Matches EnergyPlus's `LowHConvLimit` for interior convection. This is the most conservative option and would only trigger the fallback for values that EnergyPlus itself considers unacceptably low. However, EnergyPlus intended this limit for interior convection coefficients used in heat balance, not for coefficients used as divisors.)

   The review's suggested range of 4-5 W/(m²·K) is **not recommended**. This would reject legitimate natural-convection coefficients (vertical surface natural convection is 1.3-2.8 W/(m²·K) at typical exterior ΔT of 1-10°C in still air), forcing the 34 W/(m²·K) ASHRAE peak-load fallback in all low-wind conditions. The 34 W/(m²·K) value represents peak combined convection+radiation at ~15 mph wind, which substantially overestimates actual exterior convection in common still-air conditions and would systematically under-correct window LWR.

2. **Update the existing test** `window_lwr_uses_computed_h_out_not_nfrc_fallback` at `longwave.rs:908` if the threshold is raised above 0.5 (i.e., the test's h_out = 0.5 would then trigger the fallback). Adjust the test value to remain above the new guard threshold, or add a separate test that explicitly verifies the new guard boundary.

3. **Consider renaming `H_OUT_NFRC`** to `H_OUT_ASHRAE_PEAK` or similar, to prevent future maintenance confusion. The constant is the ASHRAE conventional combined value (34 W/(m²·K)), not the NFRC 100-2020 rating condition value. This is a rename-only change that does not affect semantics.

4. **Add a separate minimum guard at the DOE-2 exterior film calculation site** (`film_coefficients.rs:292`) as a defense-in-depth measure. The DOE-2 model's `h_natural + h_forced` result could be floored to a minimum of ~0.5-1.0 W/(m²·K) to prevent upstream production of implausibly small exterior film coefficients. This would protect all consumers of the exterior film resistance, not just the window LWR correction path.

## References / Citations
- EnergyPlus `DataHeatBalance.hh:1790`: `Real64 LowHConvLimit = 0.1;` — lowest allowed convection coefficient for detailed model (interior surfaces only).
- EnergyPlus `ConvectionCoefficients.cc:1967-1968`: Application of `LowHConvLimit` to interior convection in `CalcASHRAEDetailedIntConvCoeff`.
- EnergyPlus `WindowManager.cc:2277-2279`: Application of `LowHConvLimit` to window interior convection (`hcin`).
- EnergyPlus `ConvectionCoefficients.cc:6097-6104`: `CalcDOE2Forced` — the DOE-2 exterior forced convection model. Returns `Rf × (sqrt(Hn² + Hf²) − Hn)`, which → 0 as wind → 0 and ΔT → 0.
- EnergyPlus `ConvectionCoefficients.cc:1902-1948`: `CalcASHRAETARPNatural` — the TARP natural convection model. Returns `1.31 × ΔT^(1/3)` for vertical surfaces; goes to 0 at ΔT = 0.
- OCHRE `Envelope.py:125-163`: `_solve_exterior_radiation` — OCHRE's exterior LWR solver does not use an h_out divisor; it iterates surface temperature directly. No equivalent guard.
- OCHRE `Envelope.py:274-302`: `calculate_exterior_radiation` — per-surface exterior LWR iteration using heavy-ball damping. Windows are handled through separate solar/conduction paths without a T_eff correction.
- ASHRAE HoF 2021 Ch. 15, Table 1: Conventional combined exterior film coefficient = 34.0 W/(m²·K) for winter (peak heating load at ~15 mph wind).
- ASHRAE HoF 2021 Ch. 4 §4.2: Natural convection coefficients for vertical surfaces in still air.
- NFRC 100-2020: Exterior convection boundary condition h_cv = 26 W/(m²·K) at 5.5 m/s (convective-only), combined h_co ≈ 29 W/(m²·K).
- HARES test `window_lwr_uses_computed_h_out_not_nfrc_fallback` at `longwave.rs:908` — exercises the existing guard with h_out = 0.5, verifying a 68× ratio vs. the NFRC fallback.
- Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655: TARP natural convection model and exterior LWR Walton sky model.
