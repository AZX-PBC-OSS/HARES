---
id: THERMAL-002
title: "EnergyPlus 4-component exterior longwave radiation with \u03B2 split"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/longwave_radiation.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
references:
  - "EnergyPlus 24.1 Engineering Reference, Outside Surface Heat Balance"
  - "https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/outside-surface-heat-balance.html"
  - "Walton (1983) — original sky/ground view factor derivation"
  - "McClellan & Pedersen (1997) — β split for sky hemisphere"
verification:
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope
---

## Background/Context

HARES uses a 2-component LWR model (sky + ground at `t_ground_c`) with a
`^1.5` power on the sky view factor (line 92-94). EnergyPlus uses a
4-component model with linear view factors and a β coefficient that splits
the sky hemisphere into true-sky and air-temperature fractions.

### Current HARES formula (to be replaced)

`longwave_radiation.rs:92`: `((1.0 + cos_beta) / 2.0).powf(1.5)` — this is
an older empirical approximation. The EnergyPlus standard uses simple linear
view factors.

### EnergyPlus formula (24.1 Engineering Reference)

```
q_LWR = εσ F_gnd (T⁴_air − T⁴_surf)
      + εσ β F_sky (T⁴_sky − T⁴_surf)
      + εσ (1−β) F_sky (T⁴_air − T⁴_surf)

F_gnd = 0.5(1 − cos φ)
F_sky = 0.5(1 + cos φ)
β = √(0.5(1 + cos φ))
```

Where φ is surface tilt (0° = horizontal roof, 90° = vertical wall).

Key assumptions:
- Ground temperature = outdoor air temperature (E+ standard)
- β separates true sky radiance (cold) from near-horizon air radiance (warm)

## Work to Do

- [ ] Replace `sky_view_factor` computation (line 92-94): remove `powf(1.5)`,
      use E+ linear formula `0.5 * (1.0 + cos_phi)`.
- [ ] Add `beta: f64` field to `ExteriorSurface` struct.
- [ ] Add `pub fn beta_factor(tilt_deg: f64) -> f64` returning `√(0.5(1 + cos φ))`.
- [ ] Rewrite `exterior_longwave_w()`:
      - Rename `t_ground_c` parameter to `t_air_c` (ground = air per E+).
      - NaN fallback: if `t_sky_c` is NaN, use `t_air_c`.
      - Implement 4-component formula.
- [ ] Update `exterior_longwave_w_m2()` identically.
- [ ] Audit and update all call sites in `thermal_solver/mod.rs`:
      - `apply_exterior_longwave_inputs_iterative()`: construct `ExteriorSurface`
        with `beta`, pass `outdoor_temp_c` instead of `ground_temp_c`.
      - Non-iterative fallback path if present.
      - Grep for all references to `t_ground_c` or `ground_temp_c` in the
        LWR call chain.
- [ ] Update all 28 existing LWR tests:
      - Add `beta` field to `ExteriorSurface` construction.
      - Update expected values for linear view factors (not `^1.5`).
      - Update expected values for air-temp-for-ground.
- [ ] Add new tests:
      - `test_beta_factor_horizontal`: tilt=0° → β=1.0.
      - `test_beta_factor_vertical`: tilt=90° → β=√0.5 ≈ 0.707.
      - `test_4component_horizontal_degenerates`: horizontal roof with β=1.0
        should produce same result as pure sky-only model (F_gnd=0).
      - `test_4component_matches_energyplus_reference`: known E+ values.
      - `test_view_factors_sum_to_one`: F_gnd + β·F_sky + (1−β)·F_sky = 1.0
        for all tilts.
      - `test_beta_factor_obtuse_tilt`: tilt=135° → β ≈ 0.38 (overhangs/ceilings).
      - `test_beta_factor_inverted`: tilt=180° → β=0.0 (facing straight down).
      - `test_nan_sky_falls_back_to_air_temp`: NaN t_sky uses t_air_c (E+ standard).

## Files to Touch

- `crates/hares-envelope/src/longwave_radiation.rs`: 4-component model, beta, linear SVF
- `crates/hares-envelope/src/thermal_solver/mod.rs`: update call sites

## Measures of Success

- [ ] Vertical wall at T_surf=20°C, T_air=0°C, T_sky=-10°C matches E+ formula within 0.01%.
- [ ] Horizontal roof degenerates to sky-only model.
- [ ] View factors sum to 1.0 for all tilts.
- [ ] All 28+ existing tests updated and passing.

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope` clean
