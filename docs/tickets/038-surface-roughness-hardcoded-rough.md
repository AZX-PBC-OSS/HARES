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
