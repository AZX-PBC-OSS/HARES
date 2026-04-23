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
