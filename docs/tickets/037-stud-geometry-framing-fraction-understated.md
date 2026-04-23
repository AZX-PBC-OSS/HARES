# Stud-Geometry Framing Fraction Counts Stud Faces Only — 41% of ASHRAE Assembly Value

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/building.rs

## Problem

When HPXML provides `<StudSpacing>` and `<StudWidth>`, `parse_framing_factor`
(`crates/hares-io/src/hpxml/building.rs:1143–1150`) computes:

```rust
return Some(width_in / spacing_in);
```

For 2×4 studs at 16" OC this yields `1.5 / 16 = 0.094`.

ASHRAE Handbook of Fundamentals 2021, Ch. 27, Table 6 specifies a framing
fraction of **0.23** for the same assembly. The ASHRAE value covers all wood
members in the plane, not just stud faces:

| Member | Area Fraction (typical) |
|---|---|
| Studs (1.5" at 16" OC) | 0.094 |
| Double top plate (3" total) | 0.035 |
| Single bottom plate (1.5") | 0.018 |
| Headers over openings | 0.040 |
| Corner assemblies and partition intersections | 0.043 |
| **Total ASHRAE assembly** | **0.230** |

`width / spacing` produces **0.094** — 41% of the correct value.

## Current Behavior

`crates/hares-io/src/hpxml/building.rs:1143–1150`:

```rust
if let (Some(spacing_in), Some(width_in)) = (
    find_descendant_f64(node, "StudSpacing", ValueKind::Raw),
    find_descendant_f64(node, "StudWidth", ValueKind::Raw),
) {
    if spacing_in > 0.0 && width_in > 0.0 && width_in < spacing_in {
        return Some(width_in / spacing_in);  // stud faces only
    }
}
```

Effect on effective conductivity via `parallel_path_conductivity` for R-13
fibreglass (k_cavity = 0.04 W/m·K, k_wood = 0.144 W/m·K):

```
k_eff(ff=0.094) = 0.094 × 0.144 + 0.906 × 0.04 = 0.050 W/m·K
k_eff(ff=0.230) = 0.230 × 0.144 + 0.770 × 0.04 = 0.064 W/m·K
```

The stud-geometry path makes the wall appear 22% more insulating than the
ASHRAE assembly value. This path is reached when `<StudSpacing>` is provided,
overriding the default framing factor. See ticket 051 for the related defect
in the default (0.25 constant) branch.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 27, Table 6, the framing fraction for a wood-stud
wall must include studs, plates, headers, and corner assemblies. The assembly
method formula:

```
ff_studs  = stud_width_in / stud_spacing_in
ff_plates = (3.0 × stud_width_in) / (wall_height_in)   -- double top + single bottom
ff_misc   = 0.043                                        -- headers, corners (ASHRAE Table 6 constant)
ff_assembly = ff_studs + ff_plates + ff_misc
```

`wall_height_in` defaults to 96 in (2.44 m, standard 8 ft) when absent from HPXML.

For 2×4 at 16" OC with 8 ft wall: ff = 0.094 + (4.5/96) + 0.043 = 0.094 + 0.047 + 0.043 = 0.184.
The remaining gap to 0.230 is accounted for by plate thickness variation and
corner details; calibrate against ASHRAE Table 6 and clamp to [0.10, 0.35].

## Approach

1. In `parse_framing_factor` (`building.rs`), replace `width_in / spacing_in`
   with the assembly formula above. Accept an optional `wall_height_in` parameter
   from the HPXML `<WallHeight>` element; default to 96.0 in.
2. Add `assembly_framing_factor(stud_width_in: f64, stud_spacing_in: f64, wall_height_in: f64) -> f64`
   as a pure function in `building.rs` or a shared geometry module, covered by unit tests.
3. No fallback to `width / spacing`; emit a hard error if stud dimensions are
   physically implausible (width >= spacing).

## Definition of Done

- [ ] `parse_framing_factor` stud-geometry branch uses the ASHRAE assembly formula.
- [ ] `assembly_framing_factor(1.5, 16.0, 96.0)` returns a value within 5% of 0.23.
- [ ] `assembly_framing_factor(1.5, 24.0, 96.0)` returns a value within 5% of 0.15
      (ASHRAE HoF Ch. 27 Table 6, advanced framing).
- [ ] No path returns `width / spacing` without the plate/misc correction.

## Verification

```bash
cargo test -p hares-io parse_framing_factor
cargo test -p hares-io assembly_framing_factor
```

Expected: `assembly_framing_factor(1.5, 16.0, 96.0)` in [0.219, 0.242].

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 27, §3.1, Table 6 (Framing fractions for
  wood-stud walls — assembly-level values including plates and corner details).
- EnergyPlus Engineering Reference §25.2 (Opaque Conduction — Parallel Path Method,
  assembly framing fraction).
- RESNET HERS Method Ch. 3 §3.3.2 (Framing fraction calculation for wood-frame walls).
