# `beam_floor_fraction` Minimum Clamp of 0.3 Is Physically Wrong at Low Solar Altitude

> **Note:** This document contains EnergyPlus Engineering Reference section-number
> citations (e.g. "EnergyPlus §14.5") that are unverifiable against the
> web-hosted EnergyPlus documentation, which uses heading-based navigation
> without numeric section designators. These citations are preserved for audit
> provenance. For heading-based citations, see `docs/eplus/section-mapping.md`.

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/solar.rs

## Problem

`beam_floor_fraction` (solar.rs:10–12) computes the fraction of transmitted
beam solar radiation that strikes the floor vs. the walls:

```rust
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)
}
```

The lower clamp of 0.3 is physically wrong. At low solar altitude angles
(sunrise, sunset, winter morning/evening) the beam enters through windows
nearly horizontally. A horizontal beam strikes vertical walls, not the floor;
the true floor fraction approaches zero as altitude → 0°.

`sin(0°) = 0.0` is the physically correct floor fraction at solar altitude 0°.
The clamp forces a minimum of 0.3, assigning 30% of all low-angle beam to the
floor even at sunrise. This phantom floor gain is distributed per the floor's
`area × absorptance` weight and deposited to the floor RC node, bypassing the
wall nodes where the energy physically arrives.

The upper clamp of 0.9 is arguably justified — the sun is rarely directly
overhead in residential buildings (overhead sun at high altitude strikes mainly
the floor through skylights, but glazing on vertical walls means some wall
absorption always occurs). A ceiling of 0.9 is defensible.

The `sin(altitude)` base formula itself is an approximation. EnergyPlus uses
ASHRAE interior solar distribution with view-factor-based geometry (Engineering
Reference §14.5). For the approximation to be acceptable, it must at least be
physically monotone and pass through zero.

## Evidence

```
crates/hares-envelope/src/thermal_solver/solar.rs:10–12
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)
}
```

Called at solar.rs:116 and solar.rs:149 for both ScriptF (LWR) and StarMesh
solar distribution paths.

## Annual kWh Impact

**Medium.** The error is concentrated in winter mornings and evenings when
solar altitude is low and beam solar is non-trivial. In heating-dominated
climates (CZ 5–7), this artificially cools walls (less solar gain to wall
nodes) and heats the floor (excess floor gain), resulting in some cancellation.
The net effect on zone air temperature is moderated by the interior LWR
exchange, but the spatial distribution of absorbed solar heat is wrong. For
BESTEST Case 600 the error is visible in morning peak zone temperatures.

## Required Fix

1. Remove the lower clamp of 0.3. The physically correct lower bound is 0.0.
2. Retain the upper clamp of 0.9 (or make it configurable per ASHRAE interior
   solar distribution coefficients).
3. Corrected function:
   ```rust
   fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
       solar_altitude_deg.to_radians().sin().clamp(0.0, 0.9)
   }
   ```
4. Add a unit test verifying:
   - `beam_floor_fraction(0.0) == 0.0` (sunrise: beam hits walls, not floor)
   - `beam_floor_fraction(90.0) <= 0.9` (overhead: mostly floor but clipped)
   - `beam_floor_fraction(30.0) ≈ 0.5` (mid-altitude: ~half floor)
5. Long-term: replace the sinusoidal approximation with ASHRAE SHGC-weighted
   area fractions per surface orientation (EnergyPlus §14.5).

## References

- EnergyPlus Engineering Reference §14.5 (Interior Solar Distribution).
- ASHRAE Handbook of Fundamentals 2021, Ch. 15, §5 (Solar Heat Gain through
  Fenestration — interior distribution).
- BESTEST ASHRAE Standard 140-2017, Case 600 (interior solar gain distribution
  validation).

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-22

### Code Confirmation
- [x] Referenced line numbers still match — `beam_floor_fraction` is at **solar.rs:10–12** exactly as cited; called at **lines 116 and 149** as cited.
- [x] Described logic matches current implementation — `solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)` is present verbatim.
- [x] Bug is NOT already fixed — `clamp(0.3, 0.9)` is the live production code.
- [x] OCHRE cross-check result: **diverges** — OCHRE (`vendors/OCHRE/ochre/Models/Envelope.py:548–558`) distributes transmitted solar using `area × absorptivity / Σ(area × absorptivity)` view factors applied uniformly to all surfaces, with no altitude-angle-dependent floor/wall split and no lower clamp. HARES's `sin(altitude)` approximation is a HARES-specific addition absent from OCHRE.
- [x] EnergyPlus cross-check result: **partially matches on direction; section citation is wrong** — see below.

### Web-Verified Citations

**Citation 1**: *EnergyPlus Engineering Reference §14.5 (Interior Solar Distribution)*
- **Source found**: Big Ladder Software EnergyPlus 9.5 Engineering Reference, Shading Module (https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/shading-module.html); EnergyPlus 8.0 Engineering Reference Table of Contents (https://bigladdersoftware.com/epx/docs/8-0/engineering-reference/)
- **Quoted passage**: The interior solar distribution topic is documented under the subsection "Details of the Interior Solar Distribution Calculation" within the **Shading Module** chapter — not §14.5. EnergyPlus docs use HTML-page organisation, not fixed paragraph numbers. For the `MinimalShadowing`/`FullExterior` modes the reference states: *"All beam solar radiation entering the zone is assumed to fall on the floor, where it is absorbed according to the floor's solar absorptance."* For `FullInteriorAndExterior` the program projects ray geometry rather than applying a floor-fraction heuristic. No section numbered "§14.5" could be located in any version of the EnergyPlus Engineering Reference examined (versions 8.0–9.6).
- **Verdict**: **Partially correct** — the referenced subject matter (how EnergyPlus distributes interior beam solar) is real and covered in the Shading Module; however the section identifier "§14.5" does not correspond to any numbered section in the EnergyPlus Engineering Reference. EnergyPlus's full-interior mode uses explicit ray-tracing geometry, not a `sin(altitude)` floor-fraction approximation; HARES's sinusoidal heuristic is an independent simplification, not derived from EnergyPlus §14.5.

**Citation 2**: *ASHRAE Handbook of Fundamentals 2021, Ch. 15, §5 (Solar Heat Gain through Fenestration — interior distribution)*
- **Source found**: ASHRAE Handbook of Fundamentals 2017 Chapter 15 online (https://handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx); ASHRAE 2021 Fundamentals table of contents (https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals)
- **Quoted passage**: Chapter 15 (Fenestration) covers U-factors, SHGC definitions, inward-flowing fraction of absorbed solar, and angular transmittance properties. The accessible content states: *"The second component is the inward flowing fraction of 'absorbed solar radiation', radiation that is absorbed in the glazing and framing materials of the fenestration, some of which is subsequently conducted, convected, or radiated to the interior of the building."* No §5 or any sub-section in the accessible portions of Chapter 15 addresses how transmitted beam solar is *distributed across interior surfaces* (floor vs. walls) after it enters the zone. Interior surface distribution is an energy-balance / zone model topic, not a fenestration-product characterisation topic.
- **Verdict**: **Incorrect citation** — Ch. 15 §5 of ASHRAE HoF addresses the solar heat gain coefficient and glazing characterisation, not interior-surface solar distribution. The concept cited (how much beam solar reaches the floor vs. walls) is not found in this reference. The correct place to look in ASHRAE literature would be ASHRAE HoF Chapter 18 (Nonresidential Cooling and Heating Load Calculations, Radiant Time Series method) or related load-calculation guidance, but even there no `sin(altitude)` floor-fraction heuristic with a 0.3 minimum is present in publicly accessible sources.

**Citation 3**: *BESTEST ASHRAE Standard 140-2017, Case 600 (interior solar gain distribution validation)*
- **Source found**: ANSI/ASHRAE Standard 140-2017 preview (https://webstore.ansi.org/preview-pages/ASHRAE/preview_ANSI+ASHRAE+Standard+140-2017.pdf — 403 restricted); ResearchGate figure (https://www.researchgate.net/figure/BESTEST-case-600-geometry-ASHRAE-2017-ASHRAE-standard-140-2017-a-standard-method-of_fig3_360655472); LBL EnergyPlus BESTEST report (https://simulationresearch.lbl.gov/dirpubs/epl_bestest_ash.pdf — binary)
- **Quoted passage**: Case 600 is confirmed as the "Base Case" for low-mass buildings in ASHRAE 140-2017, featuring south-facing unshaded windows. The LBL BESTEST report confirms EnergyPlus results fell within the acceptable range for all Case 600 metrics. However, ASHRAE 140 specifies output tolerance bands and input geometry — it does not specify *how* a simulator must distribute interior beam solar. The standard leaves the distribution algorithm to the tool implementer.
- **Verdict**: **Partially correct** — BESTEST Case 600 is a real validation benchmark relevant to interior solar gain; however ASHRAE 140-2017 does not validate or specify the `sin(altitude)` floor-fraction formula or a minimum of 0.3. The citation is legitimate as a regression-test motivator but not as a normative source for the 0.3 clamp value.

### Physics Verification

The ticket's core physical claim is independently verifiable from first principles:

- `sin(0°) = 0.0`: a beam at the horizon is horizontal; it cannot illuminate a horizontal floor, only vertical walls.
- `sin(5°) ≈ 0.087`: at 5° altitude, a simple geometric argument gives a floor-impact fraction of ~8.7%, not 30%.
- The `clamp(0.3, 0.9)` lower bound overrides `sin` for all altitudes below `arcsin(0.3) ≈ 17.5°`, which covers a significant fraction of winter morning and evening hours.

These values were confirmed with Python: `math.sin(math.radians(0)) = 0.0`, `math.sin(math.radians(5)) ≈ 0.0872`, `math.sin(math.radians(30)) = 0.5`.

### Legitimacy
- **Verdict**: **Legitimate** (with minor citation corrections)
- **Rationale**: The core bug is real and confirmed by direct code inspection: `beam_floor_fraction` at `crates/hares-envelope/src/thermal_solver/solar.rs:11` uses `clamp(0.3, 0.9)`, which forces a 30% floor fraction at solar altitude 0°. Basic trigonometry (`sin(0°) = 0`) establishes that a horizontal beam cannot strike a horizontal floor, so the physical floor fraction at 0° must be 0.0. The upper clamp of 0.9 is defensible for the same geometric reason given in the ticket. OCHRE uses no such altitude-dependent split at all (pure area×absorptivity view factors), confirming that the 0.3 lower bound is not inherited from a reference model. EnergyPlus's simplified modes assume *all* beam falls on the floor (100% floor fraction), while its full-geometry mode ray-traces the actual beam path — neither uses a `sin(altitude)` heuristic with a 0.3 minimum. The citation of "EnergyPlus §14.5" is an incorrect section number but refers to real subject matter; the citation of "ASHRAE HoF Ch. 15 §5" points to the wrong chapter topic. Neither citation provides normative support for the 0.3 clamp. The ticket's description, evidence, required fix, and physical reasoning are all accurate.

### Proposed Fix Summary

Change `clamp(0.3, 0.9)` to `clamp(0.0, 0.9)` in `beam_floor_fraction` at `crates/hares-envelope/src/thermal_solver/solar.rs:11`. No other changes to production code are required. The upper clamp of 0.9 should be retained. Do NOT implement this fix in this audit.

### Test Written
- File: `crates/hares-envelope/src/thermal_solver/solar.rs` (within existing `#[cfg(test)] mod tests`)
- **Tests already exist**: Two regression tests were found in the existing test module at lines 751–786:
  1. `beam_floor_fraction_zero_altitude_must_be_zero` (lines 755–763): marked `#[should_panic]` because it asserts `beam_floor_fraction(0.0) == 0.0` which currently fails (the clamp returns 0.3). This test will start passing (without `should_panic`) after the fix.
  2. `beam_floor_fraction_low_altitude_walls_dominate` (lines 772–786): asserts `beam_floor_fraction(5.0) < 0.15`. This test is **actively failing** — confirmed by running `cargo test -p hares-envelope beam_floor_fraction`, which reports: `beam_floor_fraction(5°) should be near sin(5°) ≈ 0.087, not inflated by the 0.3 clamp; got 0.3`.
- No additional regression test needs to be written; the existing `beam_floor_fraction_low_altitude_walls_dominate` test is the canonical failing regression for this bug.
