# Consolidate `h_rad = 4εσT³` Linearisation Into Shared Helper

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-physics, hares-envelope

## Problem

The linearised radiation conductance `h_rad = 4·ε·σ·T³` (Stefan-Boltzmann linearisation) is computed inline at three locations:

- `crates/hares-physics/src/film_coefficients.rs:186-189`
- `crates/hares-envelope/src/boundary_rc.rs:696-697`
- `crates/hares-envelope/src/rc_network.rs:871-872`

DRY violation. A future change to ε, σ, or the linearisation pivot temperature must touch three files. The constant 4 (the derivative coefficient) and the σ value (Stefan-Boltzmann constant) are at risk of subtle drift.

## Current Behavior

Three independent inline computations of `4 * emissivity * STEFAN_BOLTZMANN * t.powi(3)`. Each site uses its own σ symbol (one uses `STEFAN_BOLTZMANN_W_M2_K4`, another uses `SIGMA`, the third may use a literal). Verifying agreement requires reading three files.

## Required Behavior

Add a single shared helper in `hares-physics`:

```rust
/// Linearised radiation conductance `h_rad = 4·ε·σ·T³` (Stefan-Boltzmann derivative).
///
/// `t_kelvin` is the linearisation pivot temperature in K (typically the mean of the
/// two surface temperatures involved in the radiation exchange).
#[inline]
pub fn linearised_h_rad(emissivity: f64, t_kelvin: f64) -> f64 {
    4.0 * emissivity * STEFAN_BOLTZMANN_W_M2_K4 * t_kelvin.powi(3)
}
```

All three callsites consume this helper. The σ constant has a single canonical definition in `hares-physics/src/constants.rs` (verify the existing constant matches NIST CODATA 2018: σ = 5.670374419e-8 W·m⁻²·K⁻⁴).

## Approach

1. Add `linearised_h_rad` to `crates/hares-physics/src/film_coefficients.rs` (or a new `radiation.rs` if more appropriate).
2. Replace the inline computations at:
   - `crates/hares-physics/src/film_coefficients.rs:186-189`
   - `crates/hares-envelope/src/boundary_rc.rs:696-697`
   - `crates/hares-envelope/src/rc_network.rs:871-872`
3. Verify the σ constant matches NIST CODATA 2018; update if necessary.
4. Add a unit test for `linearised_h_rad` at a representative T (e.g. T=295 K, ε=0.9): expected ~5.79 W/(m²·K).
5. Verify no behavioural change via existing `cargo test -p hares-envelope` and `cargo test -p hares-physics`.

## Definition of Done

- [ ] `linearised_h_rad` helper exists in `hares-physics`
- [ ] Three callsites consume the helper; no inline `4 * .. * sigma * t.powi(3)` pattern remains
- [ ] σ constant verified against NIST CODATA 2018
- [ ] Unit test for `linearised_h_rad` at T=295 K, ε=0.9 passes
- [ ] All existing tests pass with no behavioural change

## Verification

```bash
cargo test -p hares-physics
cargo test -p hares-envelope
rg "4\.0 \* .* STEFAN" crates/   # should return zero hits after fix
```

## References

- NIST CODATA 2018: Stefan-Boltzmann constant σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴.
- Incropera, DeWitt, Bergman, Lavine *Fundamentals of Heat and Mass Transfer* 7th ed. §1.2.3 — Stefan-Boltzmann law and linearisation about a pivot temperature.
- ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.3 "Radiation Heat Transfer" — linearised h_rad formulation.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation
- 044-lwr-fallback-linearised-not-scriptf
- 101-interior-film-coefficients-recompute-per-step
