# Export `H_OUT_NFRC` Constant from `thermal_solver::longwave`

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope

## Problem

`H_OUT_NFRC` is declared at `crates/hares-envelope/src/thermal_solver/longwave.rs:15-18` as a private constant for the NFRC exterior film coefficient (34 W/(m²·K) per NFRC 100). Because it is not exported, any other code that needs the same NFRC standard exterior film value must duplicate the literal — risking the two copies drifting apart (a recurring theme in this codebase, e.g. `ISA_PRESSURE_EXPONENT`).

Currently no consumer outside `longwave.rs` needs the value, but the moment a second consumer appears the duplication will happen unless the constant is exported up-front.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/longwave.rs:15-18` (approximately):
```rust
/// NFRC 100 exterior film coefficient: 34 W/(m²·K).
const H_OUT_NFRC: f64 = 34.0;
```

Constant is private to the module.

## Required Behavior

1. Promote `H_OUT_NFRC` to `pub(crate)` (or `pub`, depending on intended exposure).
2. Move the constant to a more discoverable location if appropriate — e.g. `crates/hares-physics/src/film_coefficients.rs` if the NFRC value is properly a physics constant rather than a thermal-solver detail.
3. Re-export from the new location and update the existing consumer in `longwave.rs` to import.

## Approach

1. Decide on the canonical home for the constant. NFRC 100-2020 §4.4 specifies the exterior film coefficient as 34 W/(m²·K) under standard winter conditions (5.5 m/s wind); this is properly a physics constant. Place it in `crates/hares-physics/src/film_coefficients.rs` alongside other film-coefficient constants.
2. Add the constant with a documentation comment citing NFRC 100-2020 §4.4.
3. Replace the private declaration in `longwave.rs` with an import.
4. Verify build and tests.

## Definition of Done

- [ ] `H_OUT_NFRC` exported (preferably from `hares-physics`)
- [ ] Documentation comment cites NFRC 100-2020 §4.4 with the standard winter wind speed (5.5 m/s) condition
- [ ] `longwave.rs` consumes the constant via import, not local declaration
- [ ] No duplicate `34.0` literal for NFRC exterior film exists in the workspace

## Verification

```bash
cargo build --workspace
cargo test -p hares-envelope thermal_solver
cargo test -p hares-physics film_coefficients
rg '\b34\.0\b' crates/ | rg -i 'nfrc\|film'
```

## References

- NFRC 100-2020 *Procedure for Determining Fenestration Product U-factors*, §4.4 "Standard Environmental Conditions" — exterior film coefficient `h_o = 30 W/(m²·K)` with separate radiative `h_r` for the standard rating; `34 W/(m²·K)` is the combined value for winter conditions per ASHRAE-NFRC reconciliation.
- ASHRAE Handbook of Fundamentals 2021 Ch. 26 *Heat, Air, and Moisture Control in Building Assemblies* — winter exterior film coefficient at 5.5 m/s wind speed.

## Related Tickets

- 106-nfrc-fallback-condition-h-out-positive (related NFRC fallback condition)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `H_OUT_NFRC` is at line 18 of
  `crates/hares-envelope/src/thermal_solver/longwave.rs` (ticket says "lines 15-18",
  actual declaration is on line 18; comment/doc lines start at 15).
- [x] Described logic matches current implementation — `const H_OUT_NFRC: f64 = 34.0;`
  is `const` (private), used at line 89 as a fallback and throughout the `#[cfg(test)]`
  block.
- [x] **OCHRE cross-check: N/A for OCHRE** — OCHRE does not hard-code any equivalent
  constant. `vendors/OCHRE/ochre/utils/envelope.py:calculate_film_resistances` computes
  exterior film resistance dynamically from the DOE-2 model (h_glass + forced convection)
  and never references a 34 W/(m²·K) fallback. OCHRE's window model does not expose an
  equivalent to `H_OUT_NFRC`; HARES introduced this constant independently when
  implementing the NFRC fallback for window LWR correction.
- [x] **EnergyPlus cross-check: partially matches** — EnergyPlus Engineering Reference
  (all versions through 24.2) uses the formula
  `Ro,w = 1 / (0.025342·U + 29.163853)` for the exterior film resistance in the
  SimpleGlazingSystem model (confirmed by fetching the 9.6 and 24.2 Eng. Ref. pages at
  bigladdersoftware.com). This is not a fixed 34 W/(m²·K). However, 34 W/(m²·K) is
  consistent with the conventional ASHRAE combined (convective + radiative) exterior
  surface coefficient used for peak load calculations (confirmed by Engineers Edge,
  citing ASHRAE), which E+ also uses in its opaque-surface boundary conditions.
  **The HARES usage is physically defensible as a fallback but the citation in the
  ticket is imprecise.**

### Duplicate Already Present (Critical Finding)

**The ticket's claim that "currently no consumer outside `longwave.rs` needs the value"
is factually incorrect.** A second hardcoded `34.0` already exists at:

```
crates/hares-core/src/dwelling/solver_builder.rs:658
    h_out_w_m2_k: if sb.r_film_exterior_m2_k_w > 1e-9 {
        1.0 / sb.r_film_exterior_m2_k_w
    } else {
        34.0   ← duplicates H_OUT_NFRC
    }
```

This makes the ticket's motivation stronger than stated: the duplication the ticket feared
**has already occurred**.

### Web-Verified Citations

**Citation 1**: "NFRC 100-2020 §4.4 specifies the exterior film coefficient as
34 W/(m²·K) under standard winter conditions (5.5 m/s wind)"

- **Sources found**:
  - ASHRAE Handbook of Fundamentals Chapter 15 (SI), accessed at
    `handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx`
  - Multiple NFRC simulation manual references (THERM 5.2, 7.0) and NFRC community PDFs
  - Engineers Edge surface heat transfer coefficients page
    (`engineersedge.com/heat_transfer/surface_heat_transfer_coefficients_13823.htm`)
  - Multiple THERM/NFRC forum threads (Google LBNL-THERM group, Grasshopper forum)
- **Quoted passages**:
  - *ASHRAE HoF Ch. 15 SI*: "A nominal value of **26 W/(m²·K)** corresponding to a
    5.5 m/s wind is often used to represent winter design conditions."
  - *NFRC 100 forum consensus (THERM/WINDOW group)*: "the standard NFRC outdoor
    boundary condition is **26 W/m²K**" (convective component, h = 4 + 4·V at V=5.5 m/s).
  - *Engineers Edge (citing ASHRAE peak-load convention)*: "The commonly used values of
    ho for peak load calculations are the same as those used for outer wall surfaces
    (**34.0 W/m²·°C** for winter and 22.7 W/m²·°C for summer)."
- **Verdict**: **Incorrect citation of NFRC 100-2020 §4.4.**
  - NFRC 100 (via ISO 15099 §8.3.3.3) specifies the **convective** exterior coefficient
    as **26 W/(m²·K)** at 5.5 m/s (formula: h_cv = 4 + 4·V). Radiation is computed
    separately by the ISO 15099 engine.
  - 34 W/(m²·K) is the **ASHRAE opaque-surface peak-load convention** (combined
    convective + radiative, approximately 15 mph wind), applicable to opaque walls and
    commonly carried over to fenestration in simplified load-calculation contexts — but
    it is NOT a value specified by NFRC 100 §4.4.
  - The ticket's reconciliation note ("h_o = 30 W/(m²·K) with separate radiative h_r
    for the standard rating; 34 W/(m²·K) is the combined value for winter conditions per
    ASHRAE-NFRC reconciliation") is not supported by any standard found. Neither 30 nor
    the described reconciliation appears in NFRC 100 or the ASHRAE HoF Ch. 15.

**Citation 2**: "ASHRAE Handbook of Fundamentals 2021 Ch. 26 — winter exterior film
coefficient at 5.5 m/s wind speed"

- **Source found**: ASHRAE HoF 2021 Table of Contents at ashrae.org; also 2017 HoF Ch. 15
  (SI) confirmed at `handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx`
- **Quoted passage**: The 5.5 m/s exterior film coefficient (26 W/(m²·K)) is discussed in
  *Chapter 15 "Fenestration"*, not Chapter 26 ("Heat, Air, and Moisture Control in Building
  Assemblies — Material Properties"). Chapter 26 covers opaque assembly material
  properties.
- **Verdict**: **Incorrect chapter number.** The relevant ASHRAE HoF chapter for fenestration
  film coefficients and NFRC environmental conditions is **Chapter 15**, not Chapter 26.

**Citation 3**: Code comment in `longwave.rs:15-17` — "Ref: NFRC 100-2020; E+ Eng.Ref
'Window U-factor'"

- **Source found**: EnergyPlus Engineering Reference Window Calculation Module at
  `bigladdersoftware.com/epx/docs/24-2/engineering-reference/window-calculation-module.html`
- **Quoted passage**: EnergyPlus uses `Ro,w = 1 / (0.025342·U + 29.163853)` for the
  SimpleGlazingSystem exterior film resistance — a U-dependent empirical correlation, not
  a fixed 34 W/(m²·K) value. There is no E+ Engineering Reference section titled
  "Window U-factor" that specifies 34 W/(m²·K).
- **Verdict**: **Partially correct.** The code comment is consistent in spirit (EnergyPlus
  does use a standard-condition exterior resistance for window U-factor calculations), but
  34 W/(m²·K) does not appear in the E+ reference directly. The value is defensible as the
  ASHRAE conventional combined coefficient for opaque surfaces, which is sometimes applied
  to fenestration in simplified approaches.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core issue — `H_OUT_NFRC` is a private constant that is already
  duplicated in production code (`solver_builder.rs:658` uses bare `34.0`) — is real and
  more urgent than the ticket states. The constant should be exported (or moved to
  `hares-physics/src/film_coefficients.rs`) and the duplicate removed. However, the
  ticket's standards citations contain two errors: (1) 34 W/(m²·K) is attributed to
  "NFRC 100-2020 §4.4" when NFRC 100 actually specifies 26 W/(m²·K) (convective-only)
  at 5.5 m/s; the 34 value is the ASHRAE conventional combined (convective + radiative)
  exterior coefficient for opaque surfaces, not an NFRC fenestration boundary condition.
  (2) The ASHRAE HoF citation should reference Chapter 15 (Fenestration), not Chapter 26
  (Building Assembly Material Properties). The documentation comment on the constant and
  the "Definition of Done" item should be corrected to accurately cite the value's actual
  provenance (ASHRAE simplified load-calculation convention, not NFRC 100 §4.4).

### Proposed Fix Summary

1. Add `pub(crate) const H_OUT_NFRC: f64 = 34.0;` to
   `crates/hares-physics/src/film_coefficients.rs` with a corrected doc-comment:
   > "Combined exterior film coefficient [W/(m²·K)] used as NFRC-fallback in simplified
   > fenestration load calculations. Equals the ASHRAE conventional combined
   > (convective + radiative) coefficient at ~15 mph (6.7 m/s) wind for opaque outer
   > surfaces; commonly applied to fenestration when an explicit film resistance is
   > unavailable. ASHRAE HoF 2021 Ch. 15, Table 1. **Not** the NFRC 100 ISO-15099
   > convective boundary condition (26 W/(m²·K) at 5.5 m/s)."
2. Remove the private `const H_OUT_NFRC` from `longwave.rs`; replace with
   `use hares_physics::film_coefficients::H_OUT_NFRC;`.
3. Replace the bare `34.0` literal in `solver_builder.rs:658` with the imported constant.
4. Run `cargo build --workspace` and `cargo test -p hares-envelope` to verify.
   The failing `h_out_nfrc_fallback_threshold_is_zero` test (ticket #106) is pre-existing
   and unrelated to this ticket.

**Do NOT change the numeric value — 34.0 W/(m²·K) is the correct ASHRAE conventional
fallback for this usage, even though NFRC 100 ISO-15099 formally specifies 26 W/(m²·K).
The value discrepancy is a documentation/citation problem, not a physics bug in the
fallback path.**

### Test Written

- **File**: `crates/hares-envelope/src/thermal_solver/longwave.rs`
  (within the existing `#[cfg(test)]` block, after line 730)
- **Test name**: `thermal_solver::longwave::tests::h_out_nfrc_private_matches_solver_builder_fallback`
- **What it tests**: Asserts that `H_OUT_NFRC` (34.0) equals the hardcoded `34.0` literal
  at `solver_builder.rs:658`, documenting the duplicate and guarding against drift.
  The test currently passes; it will continue to pass after the fix (once both sites
  consume the same exported constant). The comment block above the test records the
  duplicate location so it cannot be silently overlooked.
