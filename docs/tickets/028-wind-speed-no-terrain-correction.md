# Infiltration Uses Raw Met-Station Wind Speed Without Terrain/Height Correction

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/infiltration.rs, hares-physics/infiltration.rs

## Problem

`apply_infiltration_and_ventilation` (`crates/hares-envelope/src/thermal_solver/infiltration.rs:96, 109, 142`)
passes `env.weather.wind_speed_m_s` directly to `ashrae_wind_stack`, `ela_infiltration`,
and `natural_ventilation_flow_m3_s` without any terrain or height correction.

EPW and weather file wind speeds are measured at 10 m above open-country terrain
(ASHRAE standard meteorological station conditions). The wind speed at the building
envelope in suburban, urban, or rural terrain differs by the power-law profile:

```
u_building = u_met × (δ_met / H_met)^α_met × (H_building / δ_site)^α_site
```

Per ASHRAE HoF 2021, Ch. 24, Table 1 (Outdoor Design Conditions — Wind Speed
Adjustment) and Walker & Wilson 1998, this correction must be applied before
computing wind-driven infiltration terms. The net effect of omitting it is
30–50% overestimation in urban terrain and up to 20% underestimation in exposed
rural terrain.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/infiltration.rs:96`:

```rust
ashrae_wind_stack(c_s, c_w, delta_t, env.weather.wind_speed_m_s, shielding_coeff, n_i)
```

`crates/hares-envelope/src/thermal_solver/infiltration.rs:109`:

```rust
ela_infiltration(ela_m2, stack_coeff, wind_coeff, delta_t, env.weather.wind_speed_m_s)
```

The terrain correction is fully implemented in `hares_physics::infiltration::terrain_wind_speed`
and `terrain_wind_speed_for_class` (with correct constants: `MET_STATION_ALPHA = 0.14`,
`MET_STATION_DELTA_M = 270.0 m`, `MET_STATION_HEIGHT_M = 10.0 m`) and is tested
in isolation but never called from the stepping path.

**AshraeWindStack double-correction risk.** The `Aim2Coefficients` initialisation
(`crates/hares-physics/src/infiltration.rs:526–535`) bakes terrain correction
into `shelter_coeff` via `terrain_wind_speed(1.0, ...)`. When the stepping code
subsequently applies terrain correction to the raw met-station wind, the correction
would be applied twice for the `AshraeWindStack` branch. The fix must correct the
`Ela` branch unconditionally and document clearly that `AshraeWindStack` wind
coefficients already embed terrain correction and must not receive a second
correction at step time.

OCHRE also passes uncorrected wind speed; OCHRE is the floor, not the target.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 24 § "Wind data correction" and Walker & Wilson 1998
(AIM-2 model), the wind speed at the building envelope must be terrain- and
height-corrected before use in any infiltration model. The `Ela` branch must
apply this correction. The `AshraeWindStack` branch must document that its
pre-computed coefficients already embed it.

## Approach

1. Add `terrain_class: TerrainClass` and `building_height_m: f64` as fields on
   `ThermalSolverConfig` (shared across infiltration methods).
2. In `apply_infiltration_and_ventilation`, for the `Ela` branch only, compute:
   ```rust
   let u_site = terrain_wind_speed(
       env.weather.wind_speed_m_s,
       terrain_class.alpha(),
       terrain_class.delta_m(),
       building_height_m,
   );
   ```
   and pass `u_site` instead of `env.weather.wind_speed_m_s`.
3. For `AshraeWindStack`, pass `env.weather.wind_speed_m_s` unchanged (correction
   already embedded in coefficients). Add an inline comment explaining this invariant.
4. Do not apply the correction in `natural_ventilation_flow_m3_s` unless its
   derivation is confirmed to require it separately.

## Definition of Done

- [ ] `terrain_class` and `building_height_m` available in `ThermalSolverConfig`.
- [ ] `Ela` branch in `apply_infiltration_and_ventilation` applies `terrain_wind_speed`
      before calling `ela_infiltration`.
- [ ] `AshraeWindStack` branch passes uncorrected wind speed with a comment confirming
      the double-correction invariant.
- [ ] Test: suburban terrain (α=0.22, δ=370 m) at 8 m height produces wind speed
      ≈62% of met-station value and correspondingly lower `Ela` infiltration.

## Verification

```bash
cargo test -p hares-envelope infiltration
cargo test -p hares-physics terrain_wind_speed
```

Expected: `terrain_wind_speed(u_met, 0.22, 370.0, 8.0)` ≈ 0.62 × u_met
(suburban at 8 m per ASHRAE HoF Ch. 24 Table 1).

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 24 § "Wind data correction for
  terrain and height" (power-law profile, Table 1 terrain parameters).
- ASHRAE Handbook of Fundamentals 2021, Ch. 16 §4 (Air leakage — terrain-corrected
  wind speed in AIM-2 model).
- Walker, I.S. and Wilson, D.J. (1998), "Field Validation of Algebraic Equations
  for Stack and Wind Driven Air Infiltration Calculations," HVAC&R Research 4(2):
  119–139.
- EnergyPlus Engineering Reference §15.4 (AIM-2 Infiltration Model — terrain-
  corrected wind speed).
