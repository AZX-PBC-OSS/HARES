---
id: HARES-006
title: "hares-physics — Biquadratic Curves"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/biquadratic.rs
  - crates/hares-physics/src/lib.rs
references:
  - vendors/OCHRE/ochre/Equipment/HVAC.py
verification:
  - cargo check -p hares-physics
  - cargo test -p hares-physics
  - cargo clippy -p hares-physics -- -D warnings
---

## Background/Context
HVAC capacity and efficiency modifiers in EnergyPlus-derived models are expressed as biquadratic polynomials in two independent variables (typically entering wet-bulb and outdoor dry-bulb temperatures). Centralising these in one module avoids duplication across heat pump, chiller, and DX coil models.

## Work to Do
- [ ] Implement `biquadratic(coeffs: &[f64; 6], x1: f64, x2: f64) -> f64` evaluating: `a + b*x1 + c*x1² + d*x2 + e*x2² + f*x1*x2`
- [ ] Implement `quadratic(coeffs: &[f64; 3], x: f64) -> f64` evaluating: `a + b*x + c*x²`
- [ ] Define `BiquadraticCurve` struct holding:
  - coefficient array `[f64; 6]`
  - clamping bounds for `x1`: `(x1_min, x1_max)`
  - clamping bounds for `x2`: `(x2_min, x2_max)`
- [ ] Implement `BiquadraticCurve::evaluate(&self, x1: f64, x2: f64) -> f64` that clamps inputs before evaluating
- [ ] Write tests using known HVAC coefficient sets from OCHRE and verify output matches OCHRE's `_biquadratic` function
- [ ] Define reusable constants for tolerances/default bounds used in tests and clamping behavior; avoid duplicated magic numbers

## Files to Touch
- `crates/hares-physics/src/biquadratic.rs`: new file — free functions and `BiquadraticCurve` struct
- `crates/hares-physics/src/lib.rs`: add `pub mod biquadratic`

## Measures of Success
- [ ] `biquadratic` and `quadratic` results match reference Python implementation within ±1e-12 relative error for all test inputs (exact IEEE 754 match is not required — Rust and Python may use FMA differently)
- [ ] `BiquadraticCurve::evaluate` clamps inputs: values outside bounds produce the same result as the boundary value
- [ ] At least one test covers simultaneous out-of-bounds clamping on both `x1` and `x2` (both inputs beyond their respective bounds at once)
- [ ] At least one test uses a real EnergyPlus HVAC coefficient set from OCHRE source
- [ ] Test and implementation literals for tolerance/bounds are named constants, not inline unexplained numbers

## Verification
- [ ] `cargo check -p hares-physics` passes
- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo clippy -p hares-physics -- -D warnings` passes
