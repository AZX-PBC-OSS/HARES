---
id: HARES-008
title: "hares-physics — Infiltration Functions"
kind: implement
depends_on: [HARES-005]
files_to_touch:
  - crates/hares-physics/src/infiltration.rs
  - crates/hares-physics/src/lib.rs
references:
  - vendors/OCHRE/ochre/Models/Envelope.py
  - docs/architecture/appendix-physics-improvements.md
verification:
  - cargo check -p hares-physics
  - cargo test -p hares-physics
  - cargo clippy -p hares-physics -- -D warnings
---

## Background/Context
Infiltration is often the dominant uncertainty in residential envelope models. Implementing ASHRAE wind-stack infiltration and terrain wind-speed correction in Rust allows the envelope solver to recalculate infiltration every timestep without Python overhead and enables sensitivity sweeps over uncertainty parameters.

## Work to Do
- [ ] Implement `ashrae_wind_stack(c_s: f64, c_w: f64, delta_t_c: f64, wind_speed_m_s: f64, n_stories: u8, shelter_coeff: f64) -> f64` using the quadrature combination: `sqrt(Q_temp² + Q_wind²)` where the temperature-driven and wind-driven components are computed separately
- [ ] Implement `ela_infiltration(ela_m2: f64, stack_coeff: f64, wind_coeff: f64, delta_t_c: f64, wind_speed_m_s: f64) -> f64` using effective leakage area method
- [ ] Implement `ach_infiltration(ach: f64, volume_m3: f64) -> f64` converting ACH to volumetric flow
- [ ] Implement `terrain_wind_speed(u_met: f64, alpha_site: f64, delta_site: f64, height: f64) -> f64` using the power-law correction for site terrain
- [ ] Define reusable constants for unit conversions and met-station defaults (`alpha_met`, `delta_met`, `h_met`) instead of inline literals
- [ ] Define a reusable terrain class enum (rural/suburban/urban) with canonical `(alpha, delta)` pairs and test it
- [ ] Implement `terrain_wind_speed_for_class(u_met: f64, class: TerrainClass, height: f64) -> f64` convenience wrapper that looks up `(alpha, delta)` from the terrain class enum and delegates to `terrain_wind_speed`
- [ ] Write tests:
  - Known `delta_t = 10`, `wind_speed = 5`: verify computed volumetric flow matches expected value
  - Terrain coefficient tests: verify output matches appendix table entries for each terrain class

## Files to Touch
- `crates/hares-physics/src/infiltration.rs`: new file — all infiltration and terrain functions
- `crates/hares-physics/src/lib.rs`: add `pub mod infiltration`

## Measures of Success
- [ ] `ashrae_wind_stack` quadrature combines temperature and wind terms correctly (test with both terms non-zero)
- [ ] `ach_infiltration(1.0, 200.0)` returns `200.0 / 3600.0` m³/s
- [ ] Terrain coefficient test: all three terrain classes (rural, suburban, urban) match appendix table values within ±0.001
- [ ] Terrain class enum and constants are reused by tests and production functions (single source of truth)

## Notes

- `shelter_class` (AIM-2 lookup from HPXML `SiteType` + `ShieldingofHome` string values) belongs in `hares-io`, not this crate. The infiltration functions in this ticket receive the numeric shelter coefficient as a pre-computed `f64` parameter — the HPXML-to-coefficient mapping is the responsibility of the `hares-io` parser layer.
- All infiltration functions in this ticket are pre-`uom` (raw `f64`). `uom`-typed wrappers are deferred to HARES-009.

## Verification
- [ ] `cargo check -p hares-physics` passes
- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo clippy -p hares-physics -- -D warnings` passes
