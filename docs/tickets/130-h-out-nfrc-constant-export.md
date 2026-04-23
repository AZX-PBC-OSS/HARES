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
