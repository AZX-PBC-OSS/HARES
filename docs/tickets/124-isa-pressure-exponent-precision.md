# ISA Pressure Exponent Constant Precision Mismatch

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-physics/constants, hares-io/resstock_csv

## Problem

`ISA_PRESSURE_EXPONENT = 5.2559` is declared at `crates/hares-physics/src/constants.rs:83`, but `crates/hares-io/src/resstock_csv.rs` uses an inline literal `5.25588` instead of the constant. The two values differ in the fifth decimal place (5.2559 vs 5.25588), so a future contributor reading either file cannot tell which is the "correct" value or whether the difference is intentional.

The U.S. Standard Atmosphere 1976 derives the exponent as `g·M / (R*·L)` where `g = 9.80665 m/s²`, `M = 0.0289644 kg/mol`, `R* = 8.31446 J/(mol·K)`, and `L = 0.0065 K/m` for the troposphere. The exact value to higher precision is `5.255876329...`, so `5.25588` (resstock_csv) is the correctly-rounded 6-significant-figure value, while `5.2559` (constants.rs) is an over-rounded 5-significant-figure value.

## Current Behavior

`crates/hares-physics/src/constants.rs:83`:
```rust
pub const ISA_PRESSURE_EXPONENT: f64 = 5.2559;
```

`crates/hares-io/src/resstock_csv.rs` (inline):
```rust
let exponent = 5.25588;
```

Two different values for the same physical constant in two different files; the io file does not import the physics constant.

## Required Behavior

1. `ISA_PRESSURE_EXPONENT` in `hares-physics/constants.rs` must be updated to the higher-precision value `5.25587611` (8 significant figures, matching the derivation precision of the input constants).
2. `crates/hares-io/src/resstock_csv.rs` must remove the inline literal and import `ISA_PRESSURE_EXPONENT` from `hares-physics`.
3. Any other file with an inline `5.2559x` literal for this constant must also be migrated.

## Approach

1. Compute the exponent from primary constants: `5.25587611...` (truncated to 8 sig figs).
2. Update `ISA_PRESSURE_EXPONENT` in `crates/hares-physics/src/constants.rs:83` with a comment citing the U.S. Standard Atmosphere 1976 derivation.
3. `grep` the workspace for any inline `5.2558` or `5.2559` literals; replace each with the imported constant.
4. Add an opt-in `cargo test` that asserts the constant equals the derivation from the underlying primary constants to within `f64` epsilon.

## Definition of Done

- [ ] `ISA_PRESSURE_EXPONENT` updated to `5.25587611` with derivation comment
- [ ] `crates/hares-io/src/resstock_csv.rs` imports the constant; no inline literal
- [ ] Workspace `grep` for `5.2558` and `5.2559` finds only the canonical declaration
- [ ] Unit test verifies the constant matches the derivation from `g`, `M`, `R*`, `L`

## Verification

```bash
cargo test -p hares-physics constants
cargo test -p hares-io resstock_csv
rg '5\.2559|5\.2558' crates/
```

## References

- U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562), Part 1 §1.2.5 "Pressure" — derivation of the pressure exponent from gravitational acceleration, mean molar mass of dry air, universal gas constant, and tropospheric temperature lapse rate.
- NIST CODATA 2018 — values of universal gas constant `R* = 8.314462618 J/(mol·K)`, standard gravity `g = 9.80665 m/s²`.
- ICAO *Manual of the ICAO Standard Atmosphere* (Doc 7488) — independent derivation matching USSA 1976.

## Related Tickets

- 029-resstock-csv-constant-pressure (related resstock pressure handling)
