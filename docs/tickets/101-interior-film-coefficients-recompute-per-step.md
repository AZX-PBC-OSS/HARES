# Interior Film Coefficients Must Recompute Per Timestep, Not Once at Init

> **Note:** This document contains EnergyPlus Engineering Reference section-number
> citations (e.g. "EnergyPlus §3.5.4") that are unverifiable against the
> web-hosted EnergyPlus documentation. These citations are preserved for audit
> provenance. See `docs/eplus/section-mapping.md` for heading-based citations.

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-physics/film_coefficients, hares-core/dwelling

## Problem

Interior film coefficients are still computed once at init using `T_outdoor_avg − T_conditioned` despite the S1 partial fix. The S1 fix at `crates/hares-physics/src/film_coefficients.rs` and `crates/hares-core/src/dwelling/conversions.rs:131` lowered the previous 12.9°C ΔT floor to 0.1°C — but the underlying defect remains: the coefficients are frozen at init-time and never updated as zone temperatures evolve through the simulation.

Real interior convective film coefficients depend on the instantaneous surface-to-air ΔT. EnergyPlus recomputes h_conv every timestep via the TARP, MoWiTT, or Fohanno-Polidori models (per Engineering Reference §3.5). HARES freezes the value, so any departure from the assumed init ΔT injects a systematic surface-to-air heat-transfer bias.

## Current Behavior

`crates/hares-physics/src/film_coefficients.rs` and `crates/hares-core/src/dwelling/conversions.rs:131`:
- Compute h_conv_interior at init using design ΔT
- Floor previously 12.9°C; S1 lowered to 0.1°C (handles the divide-by-zero edge case)
- Value used through the entire simulation regardless of actual ΔT

In a typical residential cooling-mode simulation, the design init ΔT might be 11°C (indoor 24°C, outdoor avg 35°C summer), but the actual surface-to-air ΔT drifts as low as 1°C in steady-state — TARP h_conv at 1°C ΔT differs from h_conv at 11°C ΔT by roughly 30%.

## Required Behavior

1. h_conv_interior must be recomputed every timestep using the current zone air temperature and the current interior surface temperature.
2. Recomputation site: inside the thermal solver step, before the conduction matrix is assembled (or as part of the assembled matrix update if the solver linearises around the previous step).
3. Use the same TARP (or whichever ASHRAE-grade model is currently selected) coefficients applied at init.
4. The 0.1°C ΔT floor remains for numerical stability at exactly-equal temperatures.

## Approach

1. Identify the call site in `crates/hares-envelope/src/thermal_solver/` that currently consumes the init-time h_conv values (or where they live in the precomputed `BoundaryConductances`).
2. Replace the precomputed value with a per-step computation using the current `(T_surface, T_air)` pair from the previous step's solution.
3. The matrix that depends on h_conv must be reassembled or partially updated each step. If the solver is implicit and linearises around the previous step, this becomes a per-step update of the relevant matrix entries.
4. Add a regression test: in a free-floating simulation with a known surface-air ΔT trajectory, verify h_conv tracks the trajectory rather than remaining frozen at the init value.
5. Quantify the change in BESTEST 600/900 results — the per-step update should improve agreement with the ASHRAE 140 reference band.

## Definition of Done

- [ ] h_conv_interior recomputed per timestep using current surface and air temperatures
- [ ] 0.1°C ΔT floor preserved for numerical stability
- [ ] Per-step recomputation cost is bounded (no per-step heap allocation; pre-allocate any temporary buffers)
- [ ] Regression test: h_conv tracks ΔT trajectory in a free-floating run
- [ ] BESTEST 600/900 annual heating/cooling improved or at least unchanged
- [ ] Documentation comment cites the TARP model and the per-step update rationale

## Verification

```bash
cargo test -p hares-physics film_coefficients
cargo test -p hares-envelope thermal_solver
cargo test --test bestest
```

## References

- EnergyPlus Engineering Reference §3.5.4 "Interior Convection Algorithms" — TARP, MoWiTT, Fohanno-Polidori; per-step recomputation requirement.
- ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.2 "Free Convection" and §4.3 "Forced Convection at Surfaces" — h_conv as a function of ΔT.
- TARP method: Walton, G. N. (1983) *Thermal Analysis Research Program Reference Manual*, NBSIR 83-2655, National Bureau of Standards.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation (StarMesh derivation depends on consistent h_conv)
- 090-rederive-per-surface-ua-from-first-principles (per-surface UA computed from h_conv)
- 044-lwr-fallback-linearised-not-scriptf

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `conversions.rs:131` is the exact call site of `film_resistances(...)` inside `building_to_boundary_inputs()` (confirmed at lines 131–139).
- [x] Described logic matches current implementation — `ashrae_simple_interior_h_conv` (film_coefficients.rs:148–168) returns a fixed h_conv by surface orientation only; it receives `_t_ext_c` and `_t_int_c` as dead parameters (underscore-prefixed), confirming ΔT independence. The computed `r_film_int` is embedded in the RC network A-matrix at discretisation and never recalculated during the simulation loop. `BoundaryDiagnosticInfo::RCNode` stores `r_film_int_m2_k_w: f64` as an immutable scalar used verbatim each step (stepping.rs:161–168).
- [x] No 12.9°C floor in current code — the S1 partial fix confirmed: the 0.1°C floor now applies only to the *exterior* TARP natural convection path (film_coefficients.rs:231), not to the interior ASHRAE Simple path (which is ΔT-independent by construction). The ticket's description of the S1 fix is accurate.
- [x] OCHRE cross-check result: **matches** — OCHRE (`vendors/OCHRE/ochre/utils/envelope.py:342–402`, `Models/Envelope.py:341–410`) also freezes film resistances at init. OCHRE uses TARP at the 12.9°C ΔT floor anchor to derive the "typical" interior h_conv, then bakes the result into the static RC `resistors` dict. The HARES ASHRAE Simple value (3.076 W/(m²·K) for vertical surfaces at ΔT≈12.9°C) matches OCHRE's TARP-at-12.9°C result to within 0.12%, confirming HARES intentionally mirrors OCHRE's init-time snapshot. However, both share the same architectural defect the ticket describes.
- [x] EnergyPlus cross-check result: **diverges** — EnergyPlus (Engineering Reference, "Interior Convection / TARP Algorithm", Eqs. 90–92) recomputes `h_conv` every timestep via `CalcASHRAEDetailedIntConvCoeff` / `InitIntConvCoeff`, passing the current surface and zone-air temperatures at each heat-balance evaluation. HARES/OCHRE freeze the value at init; EnergyPlus does not.

  Quoted EnergyPlus passage (ConvectionCoefficients.cc methodology comment, confirmed via GitHub raw fetch):
  > "METHODOLOGY EMPLOYED: Determine the temperature difference between the surface and the zone air for the last time step and then base the calculation of the convection coefficient on that value and the surface tilt."

  Quoted EnergyPlus Engineering Reference (v9.3 "Interior Convection / TARP Algorithm", Eqs. 90–92):
  > "h = 1.31|ΔT|^(1/3)  [vertical surface, Eq. 90]"
  > "h = 9.482|ΔT|^(1/3) / (7.238 − |cosΣ|)  [enhanced, Eq. 91]"
  > "h = 1.810|ΔT|^(1/3) / (1.382 + |cosΣ|)  [reduced, Eq. 92]"

### Web-Verified Citations

**Citation 1**
- **Citation**: "EnergyPlus Engineering Reference §3.5.4 'Interior Convection Algorithms' — TARP, MoWiTT, Fohanno-Polidori; per-step recomputation requirement."
- **Source found**: EnergyPlus Engineering Reference v9.3 "Interior Convection" section, https://bigladdersoftware.com/epx/docs/9-3/engineering-reference/inside-heat-balance.html; also v9.6 and v24.1 TOC confirmed.
- **Quoted passage**: TOC entry (v24.1): `"[Inside Heat Balance](inside-heat-balance.html#inside-heat-balance) → [Interior Convection](inside-heat-balance.html#interior-convection)"` — no formal §3.5.4 number is assigned; it is a named anchor under "Surface Heat Balance Manager / Processes".
- **Verdict**: **Partially correct** — the content described (TARP, Fohanno-Polidori, per-step recomputation) is real and correctly attributed to EnergyPlus, but the section reference "§3.5.4" does not exist. The EnergyPlus Engineering Reference uses named HTML anchors, not numbered subsections; the interior convection content lives in the "Inside Heat Balance" page. MoWiTT is an *exterior* convection algorithm, not an interior one — it does not appear in the interior convection section of any EnergyPlus version examined.

**Citation 2**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.2 'Free Convection' and §4.3 'Forced Convection at Surfaces' — h_conv as a function of ΔT."
- **Source found**: ASHRAE Handbook—Fundamentals 2021 Table of Contents, https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
- **Quoted passage**: The TOC lists "Chapter 4: Heat Transfer" under the Principles section. The chapter-level TOC does not publish subsection §4.2/§4.3 titles publicly; the document is paywalled.
- **Verdict**: **Cannot fully confirm section numbers** — Chapter 4 covering convective heat transfer is confirmed. The §4.2/§4.3 numbering cannot be verified without a subscription; section numbers in ASHRAE Handbooks change between editions. The underlying physics (h_conv ∝ ΔT for free convection) is well-established and confirmed by the EnergyPlus TARP derivation cited in Walton 1983. The citation is physically correct even if section numbers may be imprecise.

**Citation 3**
- **Citation**: "Walton, G. N. (1983) *Thermal Analysis Research Program Reference Manual*, NBSIR 83-2655, National Bureau of Standards."
- **Source found**: NIST publications https://www.nist.gov/publications/thermal-analysis-research-program-reference-manual; PDF available at https://nvlpubs.nist.gov/nistpubs/Legacy/IR/nbsir83-2655.pdf; Internet Archive full text at https://archive.org/stream/thermalanalysisr8326walt/thermalanalysisr8326walt_djvu.txt; onebuilding.org mirror at https://onebuilding.org/historical/TARP/TARP-nbsir83-2655.pdf; also indexed by Semantic Scholar.
- **Quoted passage** (Internet Archive djvu text, Section III.J table of contents): `"J. SURFACE INSIDE HEAT BALANCES / 1. Simple Convection Coefficient / 2. Detailed Natural Convection Coefficient"` — the two approaches (Simple and Detailed/TARP) are confirmed. The EnergyPlus Engineering Reference states: `"Walton, G. N. 1983. Thermal Analysis Research Program Reference Manual. NBSSIR 83-2655. National Bureau of Standards (now NIST). This is documentation for 'TARP.'"` (Note: the designation appears as both NBSIR and NBSSIR in different sources; both refer to the same document.)
- **Verdict**: **Confirmed** — the citation is accurate. The document exists, is publicly accessible, and is the canonical source for the TARP natural convection algorithm. The HARES source file (film_coefficients.rs:15) cites "NBSSIR 83-2655" (the alternate spelling also used in EnergyPlus), which refers to the same document.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core defect is real and confirmed: HARES computes interior film coefficients once at initialisation using the ASHRAE "Simple" algorithm (fixed h_conv by orientation, ΔT-independent), bakes the result into the RC network matrices, and never recalculates it during the simulation. EnergyPlus, by contrast, recomputes h_conv every timestep using the ΔT-dependent TARP formula (confirmed from ConvectionCoefficients.cc and Engineering Reference Eqs. 90–92). However, two details in the ticket require correction:

  1. **Model name**: HARES uses the ASHRAE "Simple" algorithm (constant by orientation), not the TARP model frozen at an init ΔT. The ticket says "TARP … coefficients applied at init" but the code actually uses the simpler orientation-only model. Switching to per-step TARP would be a model upgrade, not merely a timing change.
  2. **Section reference**: The ticket cites "EnergyPlus Engineering Reference §3.5.4" — this section number does not exist in EnergyPlus documentation (which uses named anchors). MoWiTT is an *exterior* algorithm and is not present in the interior convection section of any examined EnergyPlus version.

  The quantitative error claim ("~30% difference at 1°C vs 11°C ΔT") is verified: `h_tarp(1°C) ≈ 1.31`, `h_tarp(11°C) ≈ 2.88`, vs. frozen ASHRAE Simple 3.076 — the frozen value overestimates by ~135% at 1°C and underestimates by ~6% at 11°C, so the magnitude in the ticket is if anything conservative for the low-ΔT case.

  OCHRE shares the same frozen-at-init architecture (confirmed), so addressing this ticket would create a HARES/OCHRE divergence that should be documented.

### Proposed Fix Summary

Replace `ashrae_simple_interior_h_conv` in `film_resistances` (film_coefficients.rs:204–225) with a call to `tarp_h_natural` using the current zone-air and surface temperatures. Move the per-surface h_conv computation out of `building_to_boundary_inputs` (conversions.rs:131) and into the thermal solver's per-step loop, where `T_surface` (from the previous step's state vector) and `T_zone` (from the zone output) are available. Update `BoundaryDiagnosticInfo::RCNode` to store the surface/zone temperature indices rather than a pre-computed `r_film_int_m2_k_w`, and recompute `1/h_tarp(ΔT)` at each step before using it to drive the zone-to-surface convection flux. The 0.1°C ΔT floor already in film_coefficients.rs:231 should be reused. No heap allocation is required — the update touches only the scalar computation path. The RC matrix A_c does NOT need reassembly per step; only the explicit convective flux term used in `BoundaryDiagnosticInfo` diagnostics and future UA telemetry needs the per-step value.

  **Important caveat**: The conduction path (A-matrix resistors) also embeds `r_film_int` as a static conductance. Full correctness requires reassembling or parameterising those matrix entries per step, which is a more invasive change involving the state-space discretisation. A minimal first fix could apply per-step h_conv only to the diagnostic/telemetry output, deferring the matrix update to a follow-on ticket.

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs` (appended as test `ticket_101_ashrae_simple_interior_h_conv_is_dt_independent`)
- **What it tests**: Demonstrates that `ashrae_simple_interior_h_conv` returns the same frozen value (3.076 W/(m²·K)) across ΔT = 1°C, 11°C, and 20°C, and quantifies the divergence from the TARP ΔT^(1/3) formula at each point. Also confirms that TARP at 12.9°C matches the ASHRAE Simple value to within 0.2% (the OCHRE anchor). The test passes in the current broken state and documents the expected contrast once per-step TARP is implemented.
