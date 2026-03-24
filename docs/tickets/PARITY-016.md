---
id: PARITY-016
title: "Window transmittance: EnergyPlus polynomial curves"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/solar.rs
  - crates/hares-envelope/src/thermal_solver/solar.rs
  - crates/hares-envelope/src/thermal_solver/config.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 2)
  - vendors/OCHRE/ochre/utils/envelope.py (calculate_window_parameters, lines 107-134)
  - EnergyPlus Engineering Reference Ch. 3.11.3 (Window Optical Properties)
  - ASHRAE Handbook of Fundamentals Ch. 15
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Prerequisites

WEATHER-008 (dynamic ground albedo) will have modified `solar.rs` to accept `ground_albedo: f64` as a parameter in `perez_tilted_irradiance()` and `liu_jordan_isotropic()`. This ticket modifies the same file to add window transmittance polynomials — coordinate to avoid conflicts.

## Status: COMPLETE (verified — already fully implemented)

Audit confirms the full EnergyPlus window transmittance model is already implemented:

| Feature | Implementation | Tests |
|---------|---------------|-------|
| Angular IAM polynomials | `window_iam()` with 6 GlazingCurve variants (A, B/D, D, E, F, J) | `window_iam_is_unity_at_normal_incidence`, `window_iam_is_zero_at_grazing_incidence`, `window_iam_decreases_monotonically_with_theta`, `window_iam_curve_e_at_60deg_is_in_expected_range` |
| Curve selection from U/SHGC | `GlazingCurve::from_u_shgc()` | `glazing_curve_selection_matches_energyplus_table` |
| Diffuse hemispherical IAM | `GlazingCurve::diffuse_iam()` — pre-computed per curve | `window_diffuse_iam_is_in_physical_range` |
| SHGC decomposition | `calculate_window_parameters()` — EnergyPlus Steps 4-5 | Tests for transmittance + radiation_frac computation |
| Inward-flowing fraction | `N_i = (R_ext + R_glass/2) / (R_ext + R_glass + R_int)` — computed in `calculate_window_parameters()` | Stored as `radiation_frac` in `WindowSolarProperties` |
| Solar gain formula | `apply_solar_inputs()`: beam×IAM + diffuse×diffuse_IAM, then transmitted + absorbed-inward | Full pipeline tested in solver tests |

The gap analysis claimed "simple SHGC-based split" but the actual code uses:
- Degree-4 polynomial IAM per EnergyPlus curve family
- Separate beam and diffuse IAM corrections
- `(SHGC - transmittance) × POA` for absorbed-inward, which equals `A_sol × N_i × POA` by ASHRAE decomposition

## Background/Context

Implemented during prior work. The gap analysis was based on an earlier state of the code.

**Target**: EnergyPlus-grade — implement the full angular transmittance/absorptance decomposition per EnergyPlus Engineering Reference, not just the OCHRE simplification. This includes:
- Per-angle beam transmittance T(θ) via polynomial
- Separate glass absorptance A(θ) calculation
- Inward-flowing fraction of absorbed solar based on resistance ratios
- Hemispherical diffuse transmittance (integral over hemisphere, not just 0.854× fudge)

### EnergyPlus Window Transmittance Model (from Engineering Reference 25.1)

**Normalized angular transmittance**:
```
tau(phi) = a + b*cos(phi) + c*cos^2(phi) + d*cos^3(phi) + e*cos^4(phi)
T(phi) = T(0) * tau(phi)
```

**Coefficient table** (EnergyPlus curves A-J):

| Curve | Type | a | b | c | d | e |
|-------|------|------|------|-------|-------|------|
| A | 1P 3mm clear | 0.00 | 3.36 | -3.85 | 1.49 | 0.01 |
| B | 1P 3mm bronze | 0.00 | 2.83 | -2.42 | 0.04 | 0.55 |
| C | 1P 6mm bronze | 0.00 | 2.45 | -1.58 | -0.64 | 0.77 |
| D | 1P 3mm coated | 0.00 | 2.85 | -2.58 | 0.40 | 0.35 |
| E | 2P clear/clear | 0.00 | 1.51 | 2.49 | -5.87 | 2.88 |
| F | 2P coated/clear | 0.00 | 1.21 | 3.14 | -6.37 | 3.03 |
| G | 2P tinted/clear | 0.00 | 1.09 | 3.54 | -6.84 | 3.23 |
| H | 2P 6mm coated/clear | 0.00 | 0.98 | 3.83 | -7.13 | 3.33 |
| I | 2P 6mm tinted/clear | 0.00 | 0.79 | 3.93 | -6.86 | 3.15 |
| J | 3P coated/clear/coated | 0.00 | 0.08 | 6.02 | -8.84 | 3.74 |

**Diffuse hemispherical transmittance** (Simpson's rule, 0-90deg in 10deg steps):
```
T_diffuse = 2 * integral_0^(pi/2) T(phi)*cos(phi)*sin(phi) dphi
```

**Inward-flowing fraction** (absorbed solar reaching interior):
```
Frac_inward = (R_o,s + 0.5*R_l,w) / (R_o,s + R_l,w + R_i,s)
```
where R_o,s = summer outside film, R_i,s = summer inside film, R_l,w = glass-to-glass resistance.

## Work to Do

- [ ] In `solar.rs`, add transmittance polynomial coefficients to `GlazingCurve`:
  - Each curve family gets `t_coeffs: [f64; 5]` for `[a, b, c, d, e]` (4th-order in cos(phi))
  - Store as const arrays per curve variant
- [ ] Add `fn beam_transmittance_normalized(theta_rad: f64, curve: GlazingCurve) -> f64` — evaluate polynomial
- [ ] Add `fn beam_absorptance(theta_rad: f64, curve: GlazingCurve) -> f64` — from reflectance polynomial (1 - T - R)
- [ ] Add `fn diffuse_transmittance(curve: GlazingCurve) -> f64` — Simpson's rule integration at init (cache result)
- [ ] Add `fn inward_flowing_fraction(r_glass: f64, r_film_ext: f64, r_film_int: f64) -> f64` per EnergyPlus formula
- [ ] Update `WindowSolarProperties` in `config.rs`:
  - Add `glazing_curve: GlazingCurve`
  - Add `inward_flowing_fraction: f64`
  - Remove simple `transmittance` and `radiation_frac` fields (replace with curve-based computation)
- [ ] Update `apply_solar_inputs()` in `thermal_solver/solar.rs`:
  - For each window: compute `T(θ) × area × POA` for beam, `T_diffuse × area × (DHI + reflected)` for diffuse
  - Compute `A(θ) × N_i × area × POA` for absorbed-inward
  - Total window gain = transmitted + absorbed-inward
- [ ] Update solver_builder to populate new fields from HPXML window properties
- [ ] Add unit tests: verify T(0°) = SHGC for each curve family, verify T(90°) ≈ 0, verify energy conservation

## Files to Touch

- `crates/hares-physics/src/solar.rs`: Transmittance/absorptance polynomial coefficients and evaluation
- `crates/hares-envelope/src/thermal_solver/solar.rs`: Updated window solar gain calculation
- `crates/hares-envelope/src/thermal_solver/config.rs`: Extended `WindowSolarProperties`
- `crates/hares-core/src/dwelling/solver_builder.rs`: Populate new window fields

## Measures of Success

- [ ] Beam transmittance at normal incidence matches SHGC to within 2% for each curve family
- [ ] Beam transmittance at 60° is 70-90% of normal (typical glass behavior)
- [ ] Diffuse transmittance is lower than normal-incidence transmittance (expected physically)
- [ ] Total window heat gain (transmitted + absorbed-inward) matches EnergyPlus reference values for ASHRAE standard windows

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
