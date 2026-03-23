---
id: PARITY-014
title: "Ground coupling: RC-based foundation model"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/ground.rs (new)
  - crates/hares-envelope/src/boundary_rc.rs
  - crates/hares-io/src/hpxml/building.rs
  - crates/hares-io/src/envelope_lut.rs
  - crates/hares-core/src/dwelling/conversions.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
  - crates/hares-core/src/environment.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 4)
  - EnergyPlus Engineering Reference Ch. 3.17 (Ground Heat Transfer)
  - Winkelmann (1998) "Underground Surfaces: How to Get a Better Underground Surface Heat Transfer Calculation"
  - vendors/OCHRE/ochre/utils/hpxml.py (get_slab_insulation, get_fnd_wall_insulation)
  - ASHRAE Handbook of Fundamentals Ch. 18.31 (Below-Grade Heat Transfer)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES treats ground as a fixed-temperature boundary condition. Real ground temperature varies with depth, season, and soil properties. EnergyPlus uses the Winkelmann 3D ground heat transfer model; a simpler but effective approach is the Winkelmann-simplified or Kusuda-Achenbach ground temperature model with perimeter and underslab insulation effects.

**Target**: Better than OCHRE — implement Kusuda-Achenbach ground temperature profile (depth + time dependent) combined with ASHRAE perimeter conduction factor (F2) method for slabs. This exceeds OCHRE (fixed ground temp) while being simpler than full Kiva 3D.

### Kusuda-Achenbach Model (EnergyPlus Engineering Reference)

```
T(z,t) = T_mean - T_amplitude * exp(-z * sqrt(pi / (alpha * tau))) * cos(2*pi*t/tau - theta - z * sqrt(pi / (alpha * tau)))
```

Where:
- `T_mean` = average annual soil surface temperature [C]
- `T_amplitude` = amplitude of yearly soil temperature variation [C]
- `z` = depth below surface [m]
- `alpha` = soil thermal diffusivity [m^2/day] (typical 0.04-0.07)
- `tau` = 365 days
- `theta` = phase shift (day of minimum surface temperature, typically day 35 for northern hemisphere)
- `t` = day of year

Parameters can be determined from weather data: `T_mean` = annual average dry-bulb, `T_amplitude` = (max monthly avg - min monthly avg) / 2.

### Sky Temperature (EnergyPlus Clark & Allen 1978)

```
epsilon_sky_clear = 0.787 + 0.764 * ln(T_dp / 273)       # T_dp in Kelvin
epsilon_sky = epsilon_clear * (1 + 0.0224*N - 0.0035*N^2 + 0.00028*N^3)  # N = opaque sky cover tenths
T_sky = (IRH / sigma)^0.25 - 273.15                       # from horizontal infrared [W/m2]
```

## Work to Do

- [ ] Create `crates/hares-physics/src/ground.rs`:
  - `fn kusuda_achenbach_temp(depth_m, day_of_year, t_mean_annual_c, t_amplitude_c, phase_day, diffusivity_m2_per_day) -> f64`
  - `fn slab_perimeter_loss_w(perimeter_m, f2_w_per_m_k, t_indoor_c, t_ground_surface_c) -> f64`
  - `fn foundation_wall_loss_w(height_below_grade_m, area_m2, r_wall_m2_k_w, t_indoor_c, t_ground_c) -> f64`
- [ ] In `hpxml/building.rs`, parse slab/foundation details:
  - Slab perimeter insulation (R-value, depth)
  - Underslab insulation (R-value, coverage)
  - Foundation wall insulation (R-value, height)
  - Exposed perimeter length
- [ ] In `envelope_lut.rs`, add foundation-specific LUT entries for common slab/basement assemblies
- [ ] In `boundary_rc.rs`, enhance ground node handling:
  - Instead of single GROUND_NODE as constant, compute per-boundary ground temperature using Kusuda-Achenbach at appropriate depth
  - Slab boundaries: depth = 0 (surface) to 0.5m (typical underslab)
  - Foundation walls: depth varies with below-grade height
- [ ] In `environment.rs`, compute time-varying ground temperature profile:
  - Use annual mean temp and amplitude from weather data
  - Update ground node driving temperature each timestep
- [ ] In `solver_builder.rs`, configure depth-dependent ground coupling for slab/foundation boundaries
- [ ] Add tests: verify Kusuda-Achenbach matches ASHRAE tabulated values, verify slab loss matches F2 method

## Files to Touch

- `crates/hares-physics/src/ground.rs`: New ground heat transfer module
- `crates/hares-physics/src/lib.rs`: Export new module
- `crates/hares-envelope/src/boundary_rc.rs`: Enhanced ground node handling
- `crates/hares-io/src/hpxml/building.rs`: Parse slab/foundation insulation details
- `crates/hares-io/src/envelope_lut.rs`: Foundation LUT entries
- `crates/hares-core/src/dwelling/conversions.rs`: Pass foundation details to envelope
- `crates/hares-core/src/dwelling/solver_builder.rs`: Depth-dependent ground coupling
- `crates/hares-core/src/environment.rs`: Time-varying ground temperature

## Measures of Success

- [ ] Slab heat loss varies seasonally (higher in winter, lower in summer)
- [ ] Ground temperature at 3m depth is nearly constant (annual mean ± 1°C)
- [ ] Perimeter-insulated slab loses less heat than uninsulated (physical validation)
- [ ] Kusuda-Achenbach matches ASHRAE tabulated ground temps within 1°C

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
