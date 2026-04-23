# Garage Infiltration Uses `ach50 / 20` Rule-of-Thumb Instead of AIM-2 Physics

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-core/dwelling/solver_builder.rs

## Problem

Garage zone infiltration is computed as `building.infiltration_ach50 / 20.0`
with `unwrap_or(0.5)` fallback (`crates/hares-core/src/dwelling/solver_builder.rs:941–947`).
This has three independent physics errors:

**1. Fixed N=20 divisor ignores climate.** The AIM-2 N-factor (ACH50 → natural ACH)
depends on stack and wind coefficients, which are functions of building geometry,
terrain, and shielding class. Per Walker & Wilson 1998, N ranges from ~10 in
windy southern climates (CZ1) to ~22 in sheltered northern climates (CZ7). A
fixed divisor of 20 introduces 67–100% error in natural ACH for CZ1 buildings.

**2. Building ACH50 applied to garage.** `building.infiltration_ach50` is measured
for the conditioned envelope. The garage is a separate, typically less airtight
zone with its own leakage characteristics. Applying the conditioned-zone ACH50 to
the garage conflates two distinct air barriers.

**3. `InfiltrationMethod::Ach` used.** The result is a constant annual natural ACH,
bypassing wind and stack dynamics entirely. All other zones use physics-based methods
(`AshraeWindStack`, `Ela`) that vary by timestep.

Note: ticket 052 documents that `building.infiltration_ach50` may already be wrong
(CFM50 parsed as ACH50). Fix ticket 052 first; this ticket assumes correct ACH50
is available after that fix.

## Current Behavior

`crates/hares-core/src/dwelling/solver_builder.rs:941–947`:

```rust
ZoneType::Garage => {
    let garage_ach = building
        .infiltration_ach50
        .map(|ach50| ach50 / 20.0)  // fixed N, wrong zone leakage source
        .unwrap_or(0.5);
    InfiltrationMethod::Ach { ach: garage_ach }
}
```

The conditioned zone (same file, ~line 870–928) calls `aim2_coefficients_from_ach50`
with height, shielding, and terrain parameters. The garage zone does not.

OCHRE assigns garage `ach50 / n_factor` where `n_factor` is climate-specific,
not a fixed 20.

## Required Behavior

Per Walker & Wilson 1998 (AIM-2) and ASHRAE HoF 2021, Ch. 16 §4.3, the
N-factor must be derived from the AIM-2 stack and wind coefficients for the
zone. Per EnergyPlus Engineering Reference §27.4.3, the ELA-based model
produces time-varying infiltration from SLA and zone geometry. Both are
superior to a constant ACH.

When no garage-specific leakage measurement is available, ASHRAE 152-2004
(Residential HVAC Performance) and the EnergyPlus residential template specify
`SLA = 3.0 × 10⁻⁴` for an attached unconditioned garage. There is no silent
`unwrap_or(0.5)` fallback — if no leakage data is available and the ASHRAE
default is not applied, the error must surface loudly.

## Approach

1. Implement `garage_infiltration_method(building, zone, building_height_m, terrain_class) -> Result<InfiltrationMethod, HaresError>`
   analogous to `foundation_infiltration_method` and `attic_infiltration_method`.
2. If `infiltration_cfm50` is available for the garage zone (or as a building-level
   measurement with explicit garage attribution from HPXML), derive ELA and
   compute AIM-2 coefficients using garage floor area and height.
3. If no garage-specific leakage measurement is available, apply the ASHRAE 152
   default `SLA = 3.0e-4` with a `tracing::warn!` naming the source standard.
   Convert to ELA: `ela_m2 = SLA × garage_floor_area_m2`. Use
   `InfiltrationMethod::Ela` with attic-style coefficients appropriate to garage
   height.
4. Remove `ach50 / 20.0` and `unwrap_or(0.5)` entirely. If the ASHRAE 152
   default is also unacceptable for a given run configuration, error loudly
   referencing the missing HPXML element.
5. Use `InfiltrationMethod::Ela` throughout; not `InfiltrationMethod::Ach`.

## Definition of Done

- [ ] `garage_infiltration_method` function exists, returns `InfiltrationMethod::Ela`.
- [ ] `ach50 / 20.0` and `unwrap_or(0.5)` paths removed.
- [ ] ASHRAE 152 SLA default applied with `tracing::warn!` when no zone-level
      leakage data is present.
- [ ] Test: CZ1 climate (N≈10) garage produces ≈2× higher natural ACH than a
      fixed-N=20 calculation would give.
- [ ] Test: missing leakage data → ASHRAE 152 default applied → ELA non-zero.

## Verification

```bash
cargo test -p hares-core garage_infiltration
cargo test -p hares-physics aim2_coefficients_from_ach50
```

## References

- Walker, I.S. and Wilson, D.J. (1998), "Field Validation of Algebraic Equations
  for Stack and Wind Driven Air Infiltration Calculations," HVAC&R Research 4(2):
  119–139. (AIM-2 model; N-factor climate dependence).
- ASHRAE Handbook of Fundamentals 2021, Ch. 16 §4.3 (Residential infiltration
  models — LBL model N-factor limitations).
- ASHRAE Standard 152-2004 §6.2 (Unconditioned attached garage default SLA).
- EnergyPlus Engineering Reference §27.4.3 (Effective Leakage Area model).
- `crates/hares-physics/src/infiltration.rs` — `aim2_coefficients_from_ach50`
  provides correct N-factor computation.
