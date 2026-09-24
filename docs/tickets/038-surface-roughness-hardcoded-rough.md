# All Exterior Boundaries Use `SurfaceRoughness::Rough` Regardless of HPXML Siding Type

**Severity**: Medium
**Priority**: P3
**Status**: Open
**Areas**: hares-core/dwelling/conversions.rs, hares-physics/film_coefficients.rs

## Problem

`building_to_boundary_inputs` (conversions.rs:138) calls `film_resistances`
with `SurfaceRoughness::Rough` for every exterior boundary, regardless of
the `<Siding>` type recorded in `bd.finish_type` from HPXML:

```rust
let (r_film_int, r_film_ext) = film_resistances(
    tilt_deg,
    interior_label,
    exterior_label,
    avg_wind_m_s,
    avg_ground_c,
    avg_ambient_c,
    SurfaceRoughness::Rough,  // ← hardcoded for all surfaces
);
```

The DOE-2 exterior convection model (film_coefficients.rs) scales forced
convection by a roughness correction factor from ASHRAE HoF 2021, Ch. 26,
Table 5:

| Surface | ASHRAE Roughness Class | DOE-2 Factor |
|---|---|---|
| Brick, rough concrete | Very Rough | 2.17 |
| Rough wood, rough plaster | Rough | 1.67 |
| Concrete block, smooth plaster | Medium Rough | 1.52 |
| Clear pine, smooth concrete | Medium Smooth | 1.13 |
| Smooth plaster, vinyl siding | Smooth | 1.11 |
| Glass, polished metal | Very Smooth | 1.00 |

Assigning `Rough` (factor 1.67) to vinyl siding (factor 1.11) overcounts the
forced convection coefficient by 50%. Assigning `Rough` to brick (factor 2.17)
undercounts by 23%.

HPXML `<Siding>` provides the finish type:
- `WoodSiding` → Medium Rough (1.52)
- `VinylSiding` → Smooth (1.11)
- `BrickVeneer` → Very Rough (2.17)
- `StuccoExterior` → Rough (1.67)
- `AluminumSiding` → Smooth (1.11)
- `NoExteriorFinish` → Medium Rough (1.52, bare OSB/sheathing)

`bd.finish_type` is already parsed from HPXML and available in
`BoundaryData`. It is used for LUT lookups (conversions.rs:177) but not for
roughness mapping.

## Evidence

```
crates/hares-core/src/dwelling/conversions.rs:131–139
    let (r_film_int, r_film_ext) = film_resistances(
        tilt_deg,
        interior_label,
        exterior_label,
        avg_wind_m_s,
        avg_ground_c,
        avg_ambient_c,
        SurfaceRoughness::Rough,  // ← bd.finish_type ignored
    );
```

`bd.finish_type` is available at this call site (same struct).

## Annual kWh Impact

**Medium.** The exterior film resistance accounts for 10–15% of total opaque
wall R-value for typical low-R assemblies. A 50% error in the forced-convection
coefficient translates to ~5–8% error in exterior film R, which for a poorly
insulated wall (R-7 total) is ~0.5–1% of total U-value. For well-insulated
walls (R-20+) the exterior film is <3% of total R; the impact is correspondingly
smaller. Effect is largest for high-wind sites and poorly insulated walls.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 26, Table 2 (exterior surface roughness multipliers)
and EnergyPlus Engineering Reference §9.5.2 (DOE-2 exterior convection
correlation), the roughness factor must be derived from the actual surface
finish, not hardcoded. HPXML `<Siding>` provides this information at parse time.

When `finish_type` is absent, the behavior must be explicit: default to
`SurfaceRoughness::MediumRough` (bare OSB/sheathing) with a `tracing::warn!`,
not silent `Rough`.

## Approach

1. Add `surface_roughness_from_finish_type(finish_type: Option<&str>) -> SurfaceRoughness`
   in `crates/hares-core/src/dwelling/conversions.rs` (or `film_coefficients.rs`).
   Map HPXML `<Siding>` values:
   - `"WoodSiding"` → `MediumRough`
   - `"VinylSiding"`, `"AluminumSiding"` → `Smooth`
   - `"BrickVeneer"` → `VeryRough`
   - `"StuccoExterior"` → `Rough`
   - `None` / `"NoExteriorFinish"` → `MediumRough` + `tracing::warn!`
2. Replace the hardcoded `SurfaceRoughness::Rough` at `conversions.rs:131–139`
   with `surface_roughness_from_finish_type(bd.finish_type.as_deref())`.
3. `bd.finish_type` is already available at the call site.

## Definition of Done

- [ ] `surface_roughness_from_finish_type` function implemented and tested.
- [ ] Hardcoded `SurfaceRoughness::Rough` removed from `conversions.rs:131–139`.
- [ ] All HPXML `<Siding>` enum values mapped without panic.
- [ ] Test: `VinylSiding` produces lower exterior h_c than `BrickVeneer` at
      same wind speed (different roughness factor — factor 1.11 vs 2.17).
- [ ] Test: absent `finish_type` → `MediumRough` → warning logged.

## Verification

```bash
cargo test -p hares-core surface_roughness
cargo test -p hares-physics film_coefficients
```

Expected: `film_resistances(..., SurfaceRoughness::Smooth)` produces larger
exterior film R than `film_resistances(..., SurfaceRoughness::VeryRough)` at
the same wind speed.

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 26, Table 2 (DOE-2 exterior surface
  roughness correction factors by roughness class).
- EnergyPlus Engineering Reference §9.5.2 (DOE-2 exterior convection correlation —
  roughness multiplier applied to forced convection coefficient).
- HPXML Specification v4.2 §4.4.1.2 (`<Siding>` element enumeration).

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match: `conversions.rs:131–139` — confirmed at
  lines 131–139. `SurfaceRoughness::Rough` is hardcoded at line 138.
- [x] Described logic matches current implementation: `film_resistances(…,
  SurfaceRoughness::Rough)` is called for every exterior boundary regardless of
  `bd.finish_type`. `bd.finish_type` is available in scope (same struct field) and
  is used for LUT lookups at line 176, but not passed to `film_resistances`.
- [x] OCHRE cross-check: **matches OCHRE's bug** — `vendors/OCHRE/ochre/utils/envelope.py:393`
  contains `r_f = 1.67  # ROUGHNESS_BY_FINISH_TYPE.get(boundary.get('Finish Type'), 1.67)`.
  OCHRE also hardcodes 1.67 (Rough). The mapping dictionary was written but then
  commented out (`vendors/OCHRE/ochre/utils/hpxml.py:49–62`) with a `# FUTURE:` note.
  HARES reproduces OCHRE's known limitation. This is not an intentional HARES
  correction — it inherited the unfinished behaviour.
- [x] EnergyPlus cross-check: **confirmed**. EnergyPlus Engineering Reference ("DOE-2
  Model" subsection under "Outdoor/Exterior Convection", BigLadder v9.3/v9.6):
  > "Surface Roughness Multipliers (Walton 1981).
  > Roughness Index 1 (Very Rough): Rf = 2.17; 2 (Rough): Rf = 1.67;
  > 3 (Medium Rough): Rf = 1.52; 4 (Medium Smooth): Rf = 1.13;
  > 5 (Smooth): Rf = 1.11; 6 (Very Smooth): Rf = 1.00."
  > "For less smooth surfaces: hc = hn + Rf(hc,glass − hn)"
  The HARES implementation of `SurfaceRoughness::factor()` (film_coefficients.rs:60–69)
  exactly matches these Rf values. The bug is only in always passing `Rough`.

### Web-Verified Citations

**Citation 1**
- **Citation**: "ASHRAE HoF 2021, Ch. 26, Table 2 (DOE-2 exterior surface roughness
  correction factors)"
- **Source found**: ASHRAE Handbook Fundamentals 2021 Table of Contents
  (https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals);
  EnergyPlus Engineering Reference DOE-2 section
  (https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/outside-surface-heat-balance.html)
- **Quoted passage**: EnergyPlus attributes the roughness table to "Walton 1981" not
  ASHRAE HoF: *"Surface Roughness Multipliers (Walton 1981)."* The ASHRAE HoF 2021
  Ch. 26 covers "Heat, Air, and Moisture Control in Building Assemblies — Material
  Properties." No "Table 2" with roughness correction factors for exterior convection
  was found in Ch. 26. The EnergyPlus Engineering Reference separately cites
  "ASHRAE 1989, p. 22.4" for a related roughness correlation (MoWiTT coefficients),
  not Ch. 26.
- **Verdict**: **Partially correct**. The six roughness classes and Rf values are
  real and confirmed in EnergyPlus. However, the primary source is **Walton (1981)**,
  not ASHRAE HoF 2021 Ch. 26. The table number "Table 2" in Ch. 26 is unverified.
  The ticket should cite Walton 1981 (via EnergyPlus Engineering Reference) as the
  authoritative source for the Rf values.

**Citation 2**
- **Citation**: "EnergyPlus Engineering Reference §9.5.2 (DOE-2 exterior convection
  correlation — roughness multiplier applied to forced convection coefficient)"
- **Source found**: EnergyPlus Engineering Reference "Outside Surface Heat Balance"
  chapter, multiple versions (v9.3, v9.6) at bigladdersoftware.com
- **Quoted passage**: The DOE-2 Model appears as a heading-level subsection under
  "Outdoor/Exterior Convection" within the Outside Surface Heat Balance chapter.
  The document uses heading-based navigation rather than explicit numerical section
  identifiers; no "§9.5.2" label appears in the rendered HTML.
- **Verdict**: **Partially correct**. The content (DOE-2 exterior convection with Rf
  roughness multiplier) is confirmed in EnergyPlus Engineering Reference. The section
  reference "§9.5.2" is a plausible but unverifiable section number — the BigLadder
  HTML rendering does not expose explicit §x.y.z numbers. Correct approach: cite as
  "EnergyPlus Engineering Reference, 'DOE-2 Model' section under Outside Surface
  Heat Balance."

**Citation 3**
- **Citation**: "HPXML Specification v4.2 §4.4.1.2 (`<Siding>` element enumeration)"
- **Source found**: HPXML Data Dictionary v4.0.0 and v4.2.0 at hpxml.nlr.gov
  (https://hpxml.nlr.gov/datadictionary/4.0.0/Building/BuildingDetails/Enclosure/Walls/Wall/Siding)
- **Quoted passage**: "Valid enumerated values: none, other, masonite siding,
  composite shingle siding, fiber cement siding, asbestos siding, brick veneer,
  aluminum siding, vinyl siding, synthetic stucco, stucco, wood siding."
- **Verdict**: **Partially correct — with a critical error in the ticket**. The
  `<Siding>` element exists and contains the enumeration described. However, HPXML
  uses **lowercase space-separated strings** ("vinyl siding", "brick veneer",
  "stucco"), **not CamelCase** ("VinylSiding", "BrickVeneer", "StuccoExterior") as
  listed in the ticket's mapping table and proposed `surface_roughness_from_finish_type`
  function. The HARES parser at `hpxml/building.rs:1375–1378` reads the raw XML text
  verbatim: `node.child("Siding").map(|n| n.text.trim().to_string())`. Therefore
  `bd.finish_type` will contain "vinyl siding" not "VinylSiding". The proposed fix
  must match against lowercase strings. Additionally, "StuccoExterior" is not an
  HPXML value — the correct values are "stucco" and "synthetic stucco".
  "NoExteriorFinish" is also not an HPXML value; the correct value is "none".
  The ticket's HPXML mapping table is incorrect in its string identifiers.

**Citation 4 (ticket mapping values)**
- **Citation**: Ticket Table mapping HPXML values to ASHRAE roughness classes:
  `WoodSiding → MediumRough (1.52)`, `VinylSiding → Smooth (1.11)`,
  `BrickVeneer → VeryRough (2.17)`, `StuccoExterior → Rough (1.67)`,
  `AluminumSiding → Smooth (1.11)`
- **Source found**: OCHRE hpxml.py commented-out ROUGHNESS_BY_FINISH_TYPE dict
  (vendors/OCHRE/ochre/utils/hpxml.py:49–62); EnergyPlus Engineering Reference
  roughness table; HPXML Data Dictionary v4.2
- **Quoted passage (OCHRE dict)**:
  ```python
  # 'vinyl siding': 1.67,  # (OCHRE maps to Rough, NOT Smooth)
  # 'brick veneer': 1.67,  # (OCHRE maps to Rough, NOT VeryRough)
  # 'stucco': 2.17,        # (OCHRE maps to VeryRough)
  # 'wood siding': 1.13,   # (OCHRE maps to MediumSmooth)
  # 'aluminum siding': 1.13, # (OCHRE maps to MediumSmooth)
  ```
- **Verdict**: **Cannot be fully verified from public sources**. The ticket's proposed
  roughness assignments (e.g., BrickVeneer → VeryRough, VinylSiding → Smooth) differ
  from OCHRE's assignments (brick veneer → Rough, vinyl siding → Rough). The
  EnergyPlus Engineering Reference lists "Stucco" as a VeryRough example and "Brick"
  as a Rough example — consistent with the EnergyPlus material library. The ticket's
  assignment of VinylSiding to Smooth (1.11) aligns with EnergyPlus material
  examples (smooth plaster/vinyl = Smooth). The brick assignment differs: EnergyPlus
  lists Brick as "Rough" (1.67), while the ticket maps BrickVeneer to "VeryRough"
  (2.17). Independent verification of per-material assignments requires access to
  the proprietary ASHRAE HoF or DOE-2 reference manuals.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real and confirmed: `SurfaceRoughness::Rough` is
  hardcoded at `conversions.rs:138` for every exterior boundary, while `bd.finish_type`
  (populated from HPXML `<Siding>`) is ignored at that call site. The DOE-2 Rf values
  (2.17, 1.67, 1.52, 1.13, 1.11, 1.00) are confirmed by EnergyPlus Engineering
  Reference. The OCHRE cross-check confirms this is a known limitation in OCHRE as
  well (commented-out mapping). The bug is therefore genuine. However, the ticket
  contains two material errors: (1) the ASHRAE citation is incorrect — the roughness
  table originates from Walton (1981), not ASHRAE HoF 2021 Ch. 26 Table 2; and
  (2) the proposed fix uses CamelCase identifiers ("VinylSiding", "BrickVeneer",
  "StuccoExterior") that do not match the actual HPXML string values stored in
  `bd.finish_type` ("vinyl siding", "brick veneer", "stucco"). Additionally,
  "StuccoExterior" and "NoExteriorFinish" are not valid HPXML v4.2 values.
  The per-material roughness assignments (especially BrickVeneer → VeryRough vs.
  EnergyPlus's brick → Rough) need reconciliation with the EnergyPlus material
  library before implementation.

### Proposed Fix Summary

The fix is as described in the ticket with two required corrections:

1. Implement `surface_roughness_from_finish_type(finish_type: Option<&str>) ->
   SurfaceRoughness` matching against lowercase HPXML strings: "vinyl siding" →
   `Smooth`, "aluminum siding" → `Smooth`, "stucco" → `VeryRough`, "synthetic
   stucco" → `VeryRough`, "brick veneer" → `Rough` (per EnergyPlus material
   library, not `VeryRough` as the ticket states), "wood siding" → `MediumRough`
   (or `MediumSmooth` per EnergyPlus — needs decision), "fiber cement siding" →
   `MediumRough`, "none" → `MediumRough` + `tracing::warn!`.
   Do NOT use CamelCase identifiers.

2. Replace `SurfaceRoughness::Rough` at `conversions.rs:138` with
   `surface_roughness_from_finish_type(bd.finish_type.as_deref())`.

The fix does not require any changes to `film_coefficients.rs` — the `SurfaceRoughness`
enum and `factor()` implementation are correct.

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs`
- **Test name**: `roughness_class_changes_exterior_film_resistance`
- **What it tests**: Verifies that all six `SurfaceRoughness` variants produce strictly
  ordered exterior film resistances (VeryRough lowest, VerySmooth highest) at a
  fixed wind speed. Also explicitly asserts the two ticket-identified failure modes:
  (a) using `Rough` instead of `Smooth` for vinyl siding gives lower R_ext (more
  forced convection than the surface warrants), and (b) using `Rough` instead of
  `VeryRough` for brick veneer gives higher R_ext (less forced convection than
  warranted). The test passes as written because it tests the physics layer, not
  the callsite bug in `conversions.rs`. A companion integration test that exercises
  the full `building_to_boundary_inputs` path with a HPXML file specifying
  `<Siding>vinyl siding</Siding>` would directly catch the `conversions.rs:138`
  hardcoding — such a test would live in `crates/hares-core/tests/`.
- **Status**: Passes (cargo test confirms).
