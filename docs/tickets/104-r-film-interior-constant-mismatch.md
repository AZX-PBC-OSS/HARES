# `R_FILM_INTERIOR_M2_K_W` Constant Mismatches Production Use

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/boundary_rc

## Problem

`R_FILM_INTERIOR_M2_K_W = 0.12` at `crates/hares-envelope/src/boundary_rc.rs:33-35` is the ISO 6946 *combined* (convection + radiation) interior surface resistance value. After the S1 fix promoted convection and radiation to separate paths, production code computes interior film coefficients via TARP (convection-only ~0.447 m²·K/W at ΔT=5°C) plus an explicit linearised h_rad branch. The combined-resistance constant is no longer used by the production path, but tests still pass `R_FILM_INTERIOR_M2_K_W = 0.12` to setup helpers — a 3.7× mismatch with what production actually uses.

The constant is misleading: a future contributor will read it as authoritative and either (a) recompute production using it (regressing S1) or (b) write a new test against the constant that diverges from production.

## Current Behavior

`crates/hares-envelope/src/boundary_rc.rs:33-35`:
```rust
pub const R_FILM_INTERIOR_M2_K_W: f64 = 0.12;  // ISO 6946 combined
```

Tests pass `R_FILM_INTERIOR_M2_K_W` to setup helpers. Production calls TARP and h_rad separately, ignoring this constant.

## Required Behavior

Choose one:

A. **Rename and re-purpose** — rename the constant to `R_FILM_INTERIOR_COMBINED_ISO6946_M2_K_W` and document explicitly that it is the ISO 6946 reference value provided for unit-test convenience and never used by production. Add a comment block explaining that production uses TARP (convection-only) plus explicit h_rad.

B. **Remove and migrate tests** — delete the constant. Update every test that passes it to compute its own expected value from TARP at the test's design ΔT plus a documented h_rad. This is the more correct path because it removes the misleading constant entirely.

Recommended path: B (remove). The constant has no production use and its existence is a footgun.

## Approach

1. Audit all callsites of `R_FILM_INTERIOR_M2_K_W`. Every call should be in test code; no production callsite should remain (verify against the S1 fix).
2. For each test callsite, compute the expected value from TARP (or from the test's intended ΔT and design conditions) and replace the constant reference with an inline computation or a test-local constant.
3. Delete `R_FILM_INTERIOR_M2_K_W` from `boundary_rc.rs`.
4. If any production callsite is found during the audit, migrate it to the TARP/h_rad path immediately (do not defer — `feedback_no_broken_windows`).

## Definition of Done

- [ ] Audit complete; no production code references `R_FILM_INTERIOR_M2_K_W`
- [ ] All test callsites migrated to compute expected values from first principles (or from documented test-local constants)
- [ ] `R_FILM_INTERIOR_M2_K_W` deleted from `boundary_rc.rs`
- [ ] No grep hit for `R_FILM_INTERIOR_M2_K_W` anywhere in the workspace after the fix

## Verification

```bash
cargo test -p hares-envelope boundary_rc
cargo test -p hares-envelope thermal_solver
rg R_FILM_INTERIOR_M2_K_W   # should return zero hits after fix
```

## References

- ISO 6946:2017 *Building components and building elements — Thermal resistance and thermal transmittance* — combined surface resistance values (0.13 m²·K/W interior, 0.04 m²·K/W exterior).
- ASHRAE Handbook of Fundamentals 2021 Ch. 26 §26.4 "Surface Resistances" — comparison of combined vs convection-only film models.
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — production uses convection (TARP) plus explicit longwave network, not combined film.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation
- 090-rederive-per-surface-ua-from-first-principles
- 101-interior-film-coefficients-recompute-per-step

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `R_FILM_INTERIOR_M2_K_W = 0.12` is at `boundary_rc.rs:35` (ticket says lines 33–35; actual declaration is line 35, comment on line 34 — minor off-by-one, substance correct).
- [x] Described logic matches current implementation — the constant is defined as `0.12` with comment "ISO 6946 combined" and is referenced in 13 locations all within `#[cfg(test)]` (`make_boundary`, `make_precomputed_boundary`, and inline test struct literals at lines 1439, 1998, 2250, 2305, 2320, 2543, 2582, 2620, 2655, 2713) plus one integration test file (`bestest_900ff_root_cause.rs:179`). No production callsite uses `R_FILM_INTERIOR_M2_K_W` directly.
- [x] OCHRE cross-check result: **matches ticket's description, not the constant** — `vendors/OCHRE/ochre/utils/envelope.py:342–402` (`calculate_film_resistances`) calls TARP with `delta_t = max(12.9, ...)` and returns `1/h_natural` for the interior, explicitly documented as "constant for vertical boundaries default to h = 3.076 W/m²·K, based on Simple Natural Convection Algorithm" (line 373 comment). OCHRE never uses a 0.12 constant; the production HARES path at `hares-physics/src/film_coefficients.rs:187` (`film_resistances`) mirrors OCHRE exactly. Production sets `BoundaryInput.r_film_interior_m2_k_w` via `conversions.rs:278` using this function, not the constant.
- [x] EnergyPlus cross-check result: **matches ticket** — EnergyPlus Eng. Ref. §3.6.4 "Interior Convection" (inside heat balance is §3.6, not §3.5 as the ticket says — see correction below) confirms TARP and ASHRAE Simple are convection-only. Fetched from `https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/inside-heat-balance.html`: *"The comprehensive natural convection model, accessed using the keyword 'TARP,' correlates the convective heat transfer coefficient to the surface orientation and the difference between the surface and zone air temperatures"* and *"it permits separating the radiant and convective parts of the heat transfer at the surface, which is an important attribute of the heat balance method."* For the ASHRAE Simple algorithm: *"The radiative heat transfer component was estimated at 1.02 × 0.9 = 0.918 BTU/h-ft²-F and then subtracted off"* — confirming h = 3.076 W/(m²·K) for vertical surfaces is convection-only.

### Web-Verified Citations

**Citation 1**
- **Citation**: ISO 6946:2017 — combined surface resistance values (0.13 m²·K/W interior, 0.04 m²·K/W exterior).
- **Source found**: ISO 6946 Table 1 values confirmed via `https://pdfcoffee.com/iso-6946-pdf-free.html` and ISO 6946:2007 sample (same table structure, confirmed unchanged in 2017 edition by multiple secondary sources).
- **Quoted passage**: *"The surface resistance is given by R_s = 1/(h_c + h_r) where h_c is the convective coefficient; h_r is the radiative coefficient"*; Table 1: Rsi (horizontal heat flow) = **0.13 m²·K/W**, Rse = 0.04 m²·K/W. Note: for upward heat flow Rsi = 0.10; for downward Rsi = 0.17.
- **Verdict**: **Partially correct** — the ticket cites 0.13 m²·K/W for the ISO 6946 interior value, which is correct for horizontal (wall) heat flow. However, the constant in code is **0.12**, not 0.13. The 0.12 value corresponds to ASHRAE Standard 90.1 Appendix A for interior vertical walls (R-0.68 hr·ft²·°F/Btu ≈ 0.1197 m²·K/W), not ISO 6946's 0.13. The comment in code says "ISO 6946 combined" but the value 0.12 is actually the ASHRAE combined value. This is a minor inaccuracy in the ticket's attribution (the substantive point — it is a combined resistance — is correct).

**Citation 2**
- **Citation**: ASHRAE Handbook of Fundamentals 2021 Ch. 26 §26.4 "Surface Resistances" — comparison of combined vs convection-only film models.
- **Source found**: Unmet Hours discussion `https://unmethours.com/question/55666/what-is-the-basis-for-air-film-resistances-used-to-calculate-u-factor-with-film/`; ASHRAE 90.1 addenda.
- **Quoted passage**: Interior wall film resistance ≈ **R-0.68 hr·ft²·°F/Btu ≈ 0.12 m²·K/W** (ASHRAE Standard 90.1 Appendix A, A3.1.1) — a combined convection+radiation value. Chapter 26 (2009 and 2021 editions) is cited as background for these surface coefficient tables.
- **Verdict**: **Confirmed in substance** — the 0.12 value is indeed a combined ASHRAE film resistance. The ticket's claim that this is a combined value is correct. Access to the 2021 HoF itself is paywalled, but the derivation and values are confirmed from ASHRAE 90.1 and secondary sources. Chapter number (26) and §26.4 designation cannot be independently verified for the 2021 edition specifically, but the substance of the citation is supported.

**Citation 3**
- **Citation**: EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — production uses convection (TARP) plus explicit longwave network, not combined film.
- **Source found**: `https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/inside-heat-balance.html`; EnergyPlus 25.2.0 Engineering Reference table of contents.
- **Quoted passage**: *"The heart of the heat balance method is the internal heat balance involving the inside faces of the zone surfaces. This heat balance is generally modeled with four coupled heat transfer components: 1) conduction through the building element, 2) convection to the air, 3) short wave radiation absorption and reflectance and 4) longwave radiant interchange."* TARP section: *"The comprehensive natural convection model, accessed using the keyword 'TARP,' correlates the convective heat transfer coefficient to the surface orientation..."*
- **Verdict**: **Section number incorrect** — the table of contents from EnergyPlus 25.2.0 shows the Inside Heat Balance is **§3.6**, not §3.5. The substance of the citation (production uses TARP + separate longwave, not combined film) is **confirmed correct**.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core issue is real and confirmed: `R_FILM_INTERIOR_M2_K_W = 0.12` is a combined (convection + radiation) surface resistance derived from ASHRAE 90.1 conventions (~0.12 m²·K/W for vertical walls), while production `film_resistances()` returns ASHRAE Simple convection-only resistance of ~0.325 m²·K/W for vertical surfaces — a ~2.7× mismatch (not 3.7× as the ticket states; the ticket's 3.7× would only apply if comparing to TARP at ΔT≈5°C giving ~0.446 m²·K/W, which is not the actual production formula). The constant is confirmed to appear only in test helpers within `boundary_rc.rs`'s `#[cfg(test)]` module and in one integration test, not in any production callsite. Two details need refinement: (a) the EnergyPlus section number is §3.6, not §3.5; (b) the mismatch ratio of "3.7×" should be approximately 2.7× (comparing 0.12 to the ASHRAE Simple value 0.325) — the ticket uses TARP at ΔT=5°C which is not the actual production formula (HARES uses ASHRAE Simple fixed h_conv, identical to TARP at ΔT=12.9°C per OCHRE clamp); (c) the constant value 0.12 is most accurately attributed to ASHRAE 90.1 Appendix A, not ISO 6946 (which specifies 0.13). The footgun concern is legitimate: a future contributor reading the constant and its comment "ISO 6946 combined" could misuse it in production RC assembly.

### Proposed Fix Summary

Rename `R_FILM_INTERIOR_M2_K_W` to `R_FILM_INTERIOR_COMBINED_ASHRAE_M2_K_W` (Option A), updating its doc-comment to state it is ASHRAE 90.1 Appendix A combined (not ISO 6946) and is provided for test convenience only; or delete it and replace all test callsites with inline `0.12` or a test-local named constant (Option B). The ticket recommends Option B. Either way, add a comment that production uses ASHRAE Simple convection-only via `film_resistances()` (~0.325 m²·K/W for vertical walls). No production code changes required.

### Test Written
- **File**: `crates/hares-envelope/src/boundary_rc.rs` (within `#[cfg(test)]` module, appended at line 2840)
- **What it tests**: Asserts that (a) `R_FILM_INTERIOR_M2_K_W` still equals 0.12 (guard against silent value changes), and (b) the production ASHRAE Simple convection-only resistance for a vertical wall (`1/3.076 ≈ 0.325 m²·K/W`) is greater than `2 × R_FILM_INTERIOR_M2_K_W`, confirming the mismatch between the constant and the production film model. The test passes currently (documents existing state) and will alert if the constant is ever promoted to production use.
