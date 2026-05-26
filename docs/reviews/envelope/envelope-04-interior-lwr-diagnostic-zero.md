# Interior LWR diagnostic reports zero by construction under linearized method
**Review ID**: envelope-04
**Category**: envelope
**Date**: 2026-05-25

## Files Reviewed
crates/hares-envelope/src/thermal_solver/longwave.rs
crates/hares-envelope/src/thermal_solver/mod.rs
crates/hares-envelope/src/longwave_radiation.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Models/Envelope.py
vendors/EnergyPlus/src/EnergyPlus/HeatBalanceIntRadExchange.cc
vendors/EnergyPlus/src/EnergyPlus/HeatBalanceIntRadExchange.hh

## Findings

### Finding 1: Linearized interior LWR sum-of-fluxes is zero by mathematical identity — undetectable bugs [Severity: medium]
**Description**:
The linearized interior LWR method (`interior_longwave_linearised_w_into`) computes per-surface net flux as `q_i = 4·ε_i·σ·T_zone³ · A_i · (T_mrt − T_surf,i)` where T_mrt is the ε·A-weighted mean surface temperature. The sum of all signed fluxes Σ q_i is algebraically zero:

```
Σ q_i = 4σT_zone³ · [T_mrt · Σ(ε_i·A_i) − Σ(ε_i·A_i·T_surf,i)]
      = 4σT_zone³ · [T_mrt · total_ea − total_ea · T_mrt]
      = 0
```

This is not an approximate conservation property — it is an exact algebraic identity that holds regardless of whether h_r, T_mrt, emissivities, or areas are correct or even physically meaningful. A bug that corrupts any of these values will produce wrong per-surface fluxes but the signed sum will remain exactly zero, completely masking the error.

**Code Location**:
`crates/hares-envelope/src/longwave_radiation.rs:481-514` (`interior_longwave_linearised_w_into`)

**Root Cause**:
The linearized formulation uses a single `h_r` coefficient derived from zone air temperature and the same ε·A-weighted T_mrt for all surfaces. The energy balance `Σ (T_mrt − T_surf,i) × w_i = 0` reduces to a tautology when weights `w_i` are proportional to ε_i·A_i. This is inherent to the linearized approximation — there is no parameterization that preserves both the linearized form and a non-trivial energy balance residual.

**Impact**:
- Any corruption of emissivities, areas, or the T_mrt computation yields wrong per-surface fluxes but a deceptively correct Σq = 0.
- The existing diagnostic `Σ|q_i|/2` (at `longwave.rs:441-493`) reports total exchange magnitude, which *is* non-zero when surface temperatures differ, but this only catches gross magnitude errors, not subtle per-surface misattribution.
- In the normal ScriptF path, Σq is approximately zero due to floating-point arithmetic rather than algebraic identity, so a non-zero residual would indicate a real problem.

**Vendor Comparison**:
OCHRE uses a Σq residual check (`abs(h_lwr_net.sum()) > 10` → `ModelException`) at `Envelope.py:706-707`. This check is meaningful precisely because OCHRE uses the full T⁴ radiosity method (equivalent to ScriptF), where energy conservation is approximate. The same check inserted into the HARES linearized path would be vacuous — it can never fire.

### Finding 2: Constructor guard prevents linearized path but bypassable construction paths lack runtime divergence monitoring [Severity: medium]
**Description**:
`ThermalSolver::new` rejects construction when interior LWR zones have ≥2 surfaces and lack ScriptF factors (`mod.rs:403-412`). This ensures the linearized fallback is unreachable in normal production. However, if a `ThermalSolver` is constructed via an alternative path (e.g., `from_discrete`, direct struct initialization, or checkpoint restore with a config that lacks ScriptF), the fallback path at `longwave.rs:362-371` engages silently. In that scenario:

1. Per-surface LWR fluxes use the linearized approximation with ~0.1–0.5% error relative to the exact T⁴ solution (as documented in the one-time warning at lines 339-346).
2. The `Σ|q_i|/2` diagnostic reports the linearized exchange magnitude, but there is no way to determine how much this deviates from the exact ScriptF answer.
3. The linearization error is state-dependent: it grows quadratically with surface temperature spreads (error ∝ (ΔT/(2·T_mean))²). Without a reference comparison, large errors during extreme temperature gradients go undetected.

**Code Location**:
`crates/hares-envelope/src/thermal_solver/mod.rs:403-412` (constructor guard)
`crates/hares-envelope/src/thermal_solver/longwave.rs:362-371` (fallback path selection)
`crates/hares-envelope/src/thermal_solver/longwave.rs:441-493` (diagnostic computation)

**Root Cause**:
The constructor guard is the only defense. Once bypassed, the linearized path operates with no ongoing validation against the ground-truth T⁴ solution that ScriptF provides.

**Impact**:
- In practice low, because the constructor prevents the fallback from the documented API.
- If a future refactor removes the guard, or if someone uses `unsafe` construction, the degradation would be silent — the `Σ|q_i|/2` diagnostic would still be plausible (it is physically reasonable for the linearized case) but systematically biased relative to the true T⁴ exchange.

### Finding 3: OCHRE and EnergyPlus do not implement a linearized-to-exact divergence diagnostic — HARES has an opportunity for innovation [Severity: low]
**Description**:
Neither OCHRE nor EnergyPlus provides a mode that compares a linearized LWR computation against an exact T⁴ reference at runtime:

- **OCHRE** supports `internal_radiation_method` values `"full"`, `"linear"`, and `"none"`, but these are mutually exclusive — the linearized method (`add_radiation_resistances`, lines 1048–1061) bakes linear conductances into the RC network and never computes a T⁴ reference. The `"full"` method uses Σq validation but has no linear comparison.
- **EnergyPlus** uses ScriptF (Gebhart) or CarrollMRT, both of which are "exact" methods based on enclosure radiosity theory; no linearized approximation exists in the interior LWR path.

HARES is unique among the references in maintaining both a linearized fallback and a full ScriptF path within the same solver, creating the opportunity to report a divergence metric that neither vendor provides.

**Code Location**:
`crates/hares-envelope/src/longwave_radiation.rs:444-478` (linearized function)
`crates/hares-envelope/src/longwave_radiation.rs:363-399` (ScriptF net_flux_w)

**Impact**:
Informational — this is an enhancement opportunity rather than a defect.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 2 / 1

## Recommendations

1. **Add a one-step ScriptF reference comparison to the linearized fallback path.**
   In `apply_interior_longwave_inputs` at `longwave.rs:414-424`, after computing the linearized net fluxes, perform a single ScriptF `net_flux_w_into` call using the same converged surface temperatures. Compute a divergence metric (e.g., `max_i |q_linearized_i − q_scriptf_i|` or per-zone `Σ|q_linearized − q_scriptf| / Σ|q_scriptf|`) and emit it as a `tracing::debug!` event. This would:

   - Make linearization error observable without adding significant computational cost (one extra T⁴ evaluation per timestep, which the ScriptF production path already performs multiple times per iteration).
   - Match OCHRE's philosophy of `abs(sum) > 10 → exception` but adapted to the linearized context (where sum is always zero by identity).
   - Allow operators to detect when surface temperature spreads exceed the linearized model's validity range (~20–40 K ΔT).

2. **Add a construction-time `debug_assert!` or compile-time guard ensuring ScriptF is present when ≥2 interior surfaces exist.**
   The existing `Err(...)` return at `mod.rs:403-412` is the correct behavior, but addition of `debug_assert!(zone_cfg.scriptf.is_some(), ...)` in the `ThermalSolver` struct itself (in `apply_interior_longwave_inputs`) would catch any bypass of the constructor guard in debug/test builds.

3. **Consider a Σq sanity check on the ScriptF (production) path for parity with OCHRE.**
   OCHRE validates `abs(h_lwr_net.sum()) > 10 → ModelException` (line 706-707). The HARES ScriptF path does not currently validate Σq. Adding a `tracing::error!` or configurable assertion when `|Σ q_i| > threshold` would catch numerical issues in the view-factor or matrix-vector computations before they manifest as incorrect physics.

## References / Citations

- OCHRE `_solve_interior_radiation`: `vendors/OCHRE/ochre/Models/Envelope.py:90-121` — iterative T⁴ solver with heavy-ball damping (matches HARES `apply_interior_longwave_inputs` at `longwave.rs:356-412`).
- OCHRE Σq validation: `vendors/OCHRE/ochre/Models/Envelope.py:706-707` — `if abs(h_lwr_net.sum()) > 10: raise ModelException(...)`.
- OCHRE linearized method: `vendors/OCHRE/ochre/Models/Envelope.py:1048-1061` (`add_radiation_resistances`) — bakes h_r ≈ 4εσT³ into the RC network at T=20°C reference; no comparison against full method.
- EnergyPlus ScriptF computation: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceIntRadExchange.cc:1800-1900` — `CalcScriptF` implements the Gebhart method with matrix inversion and LU decomposition.
- EnergyPlus CarrollMRT alternative: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceIntRadExchange.cc:2003-2050` — Oppenheim resistance approximation, still a full radiosity method (not linearized h_r).
- Siegel & Howell, *Thermal Radiation Heat Transfer*, 4th ed., Ch. 4 — linearization error grows as (ΔT/(2·T_mean))², ~0.12% at ΔT=20 K, ~0.5% at ΔT=40 K. Cited in HARES `longwave.rs:338`.
