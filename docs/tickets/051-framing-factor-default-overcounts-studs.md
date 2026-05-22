# Framing Factor Default 0.25 Incorrect; Steel Frame Uses Wrong Method

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml/building.rs, hares-envelope/boundary_rc.rs

## Problem

Two independent defects exist in the default branch of `parse_framing_factor`
(`crates/hares-io/src/hpxml/building.rs:1153–1160`), reached when HPXML
provides no explicit `<FramingFactor>` and no `<StudSpacing>`/`<StudWidth>`:

**Defect 1 — wrong wood-stud default.** The function returns `Some(0.25)` for
`WoodStud`. ASHRAE Handbook of Fundamentals 2021, Ch. 27, Table 6 gives 0.23
for 2×4 at 16" OC assembly (plates, headers, and corner assemblies included).
The 0.25 value likely originates from an informal rule of thumb; it is not
cited in ASHRAE. For R-13 fibreglass (k_cavity = 0.04 W/m·K, k_wood = 0.144 W/m·K):

```
k_eff(ff=0.25) = 0.25 × 0.144 + 0.75 × 0.04 = 0.066 W/m·K
k_eff(ff=0.23) = 0.23 × 0.144 + 0.77 × 0.04 = 0.064 W/m·K  (ASHRAE)
```

The 0.25 default raises effective conductivity by 4.7% on every LUT-miss wall.

**Defect 2 — steel frame uses parallel-path method.** `SteelFrame` is assigned
`Some(0.25)` and then routed through `parallel_path_conductivity`
(`crates/hares-envelope/src/boundary_rc.rs:1394–1399`), the same branch as
wood. Steel studs are high-conductance thermal bridges; ASHRAE HoF 2021, Ch. 27
§3.2 requires the zone method (series-parallel) for metal framing. The parallel-path
method can underestimate steel-framed wall U-values by 2–5×, understating
heat loss by the same factor. See ticket 037 for the related defect in the
stud-geometry (explicit spacing) branch.

## Current Behavior

`crates/hares-io/src/hpxml/building.rs:1153–1160`:

```rust
// Default by construction type per ASHRAE Handbook of Fundamentals.
// 25% for 16" OC (standard), 22% for 24" OC (advanced framing).
match construction_type {
    Some("WoodStud") => Some(0.25),
    Some("SteelFrame") => Some(0.25),
    _ => None,
}
```

`crates/hares-envelope/src/boundary_rc.rs:1394–1399`:

```rust
pub fn parallel_path_conductivity(k_cavity_w_m_k: f64, framing_factor: Option<f64>) -> f64 {
    match framing_factor {
        Some(ff) if ff > 0.0 && ff < 1.0 => {
            ff * SOFTWOOD_CONDUCTIVITY_W_M_K + (1.0 - ff) * k_cavity_w_m_k  // applied to steel too
        }
```

OCHRE does not apply framing correction to raw material layers; it uses
pre-computed LUT R-values derived from ASHRAE HoF assemblies. These defects
affect HARES on the LUT-miss path (non-standard or BESTEST-style assemblies).

## Required Behavior

Per ASHRAE HoF 2021, Ch. 27, Table 6:
- `WoodStud` default framing fraction: **0.23** (16" OC assembly).
- `SteelFrame`: must not use parallel-path. ASHRAE HoF Ch. 27 §3.2 mandates
  the zone method: `U_eff = (A_insul × U_insul + A_stud × U_stud) / A_total`
  where the area fractions are derived from stud geometry.

## Approach

1. Change `Some("WoodStud") => Some(0.25)` to `Some(0.23)`.
2. Remove `Some("SteelFrame") => Some(0.25)`. In the solver boundary construction,
   when `construction_type == SteelFrame` and the LUT path was not taken, return
   a hard `Err`: the zone method requires explicit stud dimensions
   (`<StudSpacing>`, `<StudWidth>`) which are absent in this branch. Error message
   must reference the required HPXML elements.
3. Implement `steel_frame_u_zone_method(stud_width_m: f64, stud_spacing_m: f64, r_cavity_m2_k_w: f64, r_stud_m2_k_w: f64) -> f64`
   in `boundary_rc.rs` for use when stud geometry is available.
4. No fallback constants for steel frame; fail loudly.

## Definition of Done

- [ ] `parse_framing_factor` returns `Some(0.23)` for `WoodStud` default.
- [ ] `parse_framing_factor` does not return a value for `SteelFrame` default;
      the steel-frame LUT-miss path errors with a message citing `<StudSpacing>` and `<StudWidth>`.
- [ ] `steel_frame_u_zone_method` function implemented and tested.
- [ ] Test: `WoodStud` default produces `k_eff` within 1% of ASHRAE Table 6 reference.

## Verification

```bash
cargo test -p hares-io parse_framing_factor
cargo test -p hares-envelope parallel_path_conductivity
cargo test -p hares-envelope steel_frame_u_zone_method
```

Expected: default wood-stud `k_eff` with ff=0.23 matches ASHRAE HoF Ch. 27 Table 6
assembly U-value within 1%.

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 27, §3.1, Table 6 (Parallel-path method;
  framing fractions for wood-stud walls).
- ASHRAE Handbook of Fundamentals 2021, Ch. 27, §3.2 (Zone method for metal-framed
  assemblies).
- ISO 6946:2017 §6.9.2 (Combined method for thermal bridging elements).
- EnergyPlus Engineering Reference §25.2 (Opaque conduction — assembly U-factor
  including parallel-path and zone methods).

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `parse_framing_factor` is at
  `crates/hares-io/src/hpxml/building.rs:1128–1161`; the default `match` block
  is at lines 1153–1160. (Ticket says 1153–1160 — confirmed.)
- [x] Described logic matches current implementation — both `WoodStud` and
  `SteelFrame` arms return `Some(0.25)`; `parallel_path_conductivity` at
  `crates/hares-envelope/src/boundary_rc.rs:1396–1403` uses
  `SOFTWOOD_CONDUCTIVITY_W_M_K = 0.144` regardless of framing type. Confirmed.
- [x] OCHRE cross-check — OCHRE does **not** compute framing factors from first
  principles; it reads `<FramingFactor>` directly from HPXML. In
  `vendors/OCHRE/ochre/defaults/Input Files/BEopt_example.xml` lines 296–298,
  2×4 @ 16" OC walls are given `<FramingFactor>0.25</FramingFactor>` — matching
  the HARES default of 0.25, not 0.23 as the ticket claims. OCHRE has no
  `parallel_path_conductivity` or `zone_method` equivalent; it uses
  pre-computed assembly R-values.
- [x] EnergyPlus cross-check — N/A for this specific algorithm. EnergyPlus
  ingests pre-built construction assemblies (via the `Construction` object) and
  applies conduction transfer functions; it does not implement a stand-alone
  `parallel_path` or `zone_method` function in its engineering reference section
  §25.2. The cited section number does not correspond to an identifiable
  framing-factor algorithm in EnergyPlus 25.2 Engineering Reference (bigladdersoftware.com/epx/docs/25-2).

### Web-Verified Citations

**Citation 1**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021, Ch. 27, §3.1, Table 6 gives
  0.23 for 2×4 at 16″ OC assembly (plates, headers, and corner assemblies included)."
- **Source found**: ASHRAE HoF F17 Ch. 27 (Examples, SI), fetched directly from
  `handbook.ashrae.org/Handbooks/F17/IP/f17_ch27/f17_ch27_ip.aspx`; 2021 edition
  has the same chapter structure (Ch. 25 Fundamentals / Ch. 27 Examples per
  confirmed TOC at ashrae.org).
- **Quoted passage**: *"For stud walls 16 in. on center (OC), the fraction of
  insulated cavity may be as low as 0.75, where the fraction of studs, plates, and
  sills is 0.21 and the fraction of headers is 0.04."* (total framing = 0.21 + 0.04
  = **0.25**). Example 3 uses 75% cavity / 21% studs+plates / 4% headers.
- **Additional corroboration**: OCHRE BEopt_example.xml `<FramingFactor>0.25</FramingFactor>`
  for 2×4 @ 16" (lines 296–298). California Energy Code JA4.1.6 table: "Walls |
  16″ o.c. | 25%". Multiple code-compliance references (IECC, REScheck, ASHRAE
  90.1-based tools) use 25% for 16″ OC.
- **Verdict**: **INCORRECT**. The ticket claims ASHRAE Table 6 gives 0.23 but the
  authoritative ASHRAE source gives 0.21 (studs+plates) + 0.04 (headers) = **0.25**
  for a 16″ OC 2×4 assembly. The 0.25 currently in the code is correct.

**Citation 2**
- **Citation**: "ASHRAE HoF 2021, Ch. 27 §3.2 requires the zone method
  (series-parallel) for metal framing."
- **Source found**: ASHRAE HoF F17 Ch. 27 Examples, fetched from
  `handbook.ashrae.org/Handbooks/F17/IP/f17_ch27/f17_ch27_ip.aspx`; Ch. 25
  Fundamentals, fetched from
  `handbook.ashrae.org/Handbooks/F17/SI/F17_Ch25/f17_ch25_si.aspx`.
- **Quoted passage** (Ch. 25 Fundamentals): *"For assemblies with large differences
  in material thermal conductivities (e.g., assemblies using metal structural
  elements), the zone method is recommended (see Chapter 27)."*
  Ch. 27 Examples: *"For these metal stud/wall constructions, the recommended
  approach is the modified zone method (see Example 7) or two-dimensional analysis
  software such as THERM."*
- **Verdict**: **PARTIALLY CORRECT**. The substance is right — ASHRAE does require
  the **modified zone method** (not the standard parallel-path method) for steel
  framing, and applying `parallel_path_conductivity` with `SOFTWOOD_CONDUCTIVITY_W_M_K`
  to a steel-framed wall is wrong. However, the section reference "Ch. 27 §3.2" is
  imprecise: the 2021 edition's Chapter 27 is titled "Examples" and the relevant
  guidance appears in Examples 5 and 7 (modified zone method), not in a numbered
  "§3.2" subsection. Chapter 25 contains the conceptual statement.

**Citation 3**
- **Citation**: "ISO 6946:2017 §6.9.2 (Combined method for thermal bridging elements)."
- **Source found**: ISO 6946:2017 scope description via
  `www.normsplash.com/ISO/119976344/ISO-6946` and
  `standards.iteh.ai/catalog/standards/cen/3c0b7c72-4e13-4e21-9298-8a933cbc74c2/en-iso-6946-2017`.
- **Quoted passage**: *"Other cases where insulation is bridged by metal are outside
  the scope of ISO 6946:2017."* (Scope section.) The standard does provide *"an
  approximate method…for elements containing inhomogeneous layers, including the
  effect of metal fasteners, by means of a correction term given in Annex F"* but
  this covers fasteners only, not metal stud framing.
- **Verdict**: **INCORRECT**. ISO 6946:2017 explicitly excludes metal-framed wall
  assemblies from scope. Citing this standard for the combined method as applied
  to steel studs is wrong. The correct standard for metal framing is ASHRAE HoF
  Ch. 27 (modified zone method) or AISI/CFSEI procedures, not ISO 6946.

**Citation 4**
- **Citation**: "EnergyPlus Engineering Reference §25.2 (Opaque conduction —
  assembly U-factor including parallel-path and zone methods)."
- **Source found**: EnergyPlus 25.2 Engineering Reference TOC at
  `bigladdersoftware.com/epx/docs/25-2/engineering-reference/`.
- **Quoted passage**: No section matching "§25.2 — opaque conduction assembly
  U-factor" exists. The TOC lists "Conduction Through The Walls" (covering
  Conduction Transfer Functions and CTF calculations) and "Conduction Finite
  Difference Solution Algorithm". There is no framing-factor, parallel-path, or
  zone-method section. EnergyPlus accepts pre-assembled constructions and does
  not implement these ASHRAE methods internally.
- **Verdict**: **CANNOT VERIFY** — the cited section does not appear to exist.
  EnergyPlus does not expose a framing-factor or parallel/zone-method calculation
  as a documented algorithm in its Engineering Reference.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: **Defect 1 (WoodStud 0.25 → 0.23) is NOT a real defect.** The
  ASHRAE HoF F17/F21 Ch. 27 Examples chapter explicitly gives total framing =
  0.21 (studs+plates+sills) + 0.04 (headers) = 0.25 for a 2×4 @ 16″ OC wall,
  which matches both the current HARES code and OCHRE's BEopt example XML. The
  0.23 figure cited in the ticket has no traceable ASHRAE source. Defect 1 should
  be **rejected**; the current code is correct. **Defect 2 (steel frame uses
  parallel path with softwood conductivity) IS a real defect.** Applying
  `SOFTWOOD_CONDUCTIVITY_W_M_K` (0.144 W/m·K) to a steel framing member (≈50
  W/m·K) under-corrects the thermal bridge by a factor of ~350, and ASHRAE clearly
  mandates the modified zone method for metal framing. The section and standard
  citations supporting this claim (Ch. 27 examples, modified zone method) are
  substantively correct even if the exact section numbers are imprecise. The ISO
  6946:2017 §6.9.2 citation is wrong (that standard excludes metal framing from
  scope). The EnergyPlus §25.2 citation cannot be verified. Severity is real but
  the impact is limited to the LUT-miss path for steel-framed assemblies.

### Proposed Fix Summary

**Do NOT change WoodStud from 0.25 to 0.23** — the current value is correct per
ASHRAE. Only fix Defect 2:

1. Change `Some("SteelFrame") => Some(0.25)` to `Some("SteelFrame") => None` in
   the default branch of `parse_framing_factor` (or, if the caller can propagate
   errors, return a structured error). This prevents the steel wall from silently
   using softwood conductivity.
2. In the boundary solver, add a guard: when `construction_type == SteelFrame`
   and `framing_factor.is_none()` and the assembly requires a conductivity
   correction, return an error citing `<StudSpacing>` and `<StudWidth>` as
   required elements, and reference the ASHRAE modified zone method.
3. Implement `steel_frame_u_zone_method` for use when stud geometry IS available
   (explicit `<StudSpacing>` + `<StudWidth>`).

The WoodStud default (0.25) and the parallel-path formula for wood are correct
as-is. Do not change them.

### Test Written

- **File**: `crates/hares-io/tests/hpxml_parsing_tests.rs`
- **Tests added**:
  - `wood_stud_default_framing_factor_is_0_25_per_ashrae` — asserts that the
    WoodStud default returns 0.25 (confirming the current value is correct and
    should NOT be changed to 0.23 as Defect 1 claims). **Passes now and should
    continue to pass.**
  - `steel_frame_default_silently_applies_softwood_conductivity` — documents the
    Defect 2 bug: SteelFrame currently returns `Some(0.25)`. The test asserts
    this buggy value; once the fix is applied (SteelFrame should return `None` or
    error), this assertion must be inverted / replaced. **Passes now as a
    documentation test of current broken behaviour.**
