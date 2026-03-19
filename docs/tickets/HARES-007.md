---
id: HARES-007
title: "hares-physics — Solar Geometry"
kind: implement
depends_on: [HARES-002, HARES-005]
files_to_touch:
  - crates/hares-physics/src/solar.rs
  - crates/hares-physics/src/lib.rs
  - crates/hares-physics/Cargo.toml
references:
  - vendors/OCHRE/ochre/utils/envelope.py
verification:
  - cargo check -p hares-physics
  - cargo test -p hares-physics
  - cargo clippy -p hares-physics -- -D warnings
---

## Background/Context
Surface irradiance on walls and windows drives both envelope conduction loads and solar gains. A self-contained solar geometry module lets the envelope model compute per-surface irradiance from standard weather file inputs (GHI, DNI, DHI) without depending on an external library.

## Work to Do
- [ ] Define `SolarPosition` struct with fields: `altitude_deg: f64`, `azimuth_deg: f64`
- [ ] Implement `solar_position(latitude_deg: f64, longitude_deg: f64, utc_datetime: DateTime<Utc>) -> SolarPosition` using the Spencer (1971) method. Note: OCHRE uses pvlib/NREL SPA (Reda & Andreas 2004), not Spencer, so this does NOT provide OCHRE parity for solar position. Spencer is simpler but less accurate (±0.5° vs ±0.0003°). A future upgrade to NREL SPA may be warranted for improved accuracy.
- [ ] Implement `angle_of_incidence(surface_tilt_deg: f64, surface_azimuth_deg: f64, solar_alt: f64, solar_az: f64) -> f64`
- [ ] Implement `surface_irradiance(surface_id: u32, ghi: f64, dni: f64, dhi: f64, aoi: f64, surface_tilt: f64) -> SurfaceIrradiance` using an isotropic diffuse sky model; `surface_id` is passed through so callers can distinguish which surface the result belongs to
- [ ] Implement `window_transmitted_solar(irradiance_w_m2: f64, shgc: f64, area_m2: f64) -> f64`
- [ ] Define named constants for solar-model coefficients and time/angle conversion factors (for example albedo, minutes-per-day, degree conversions)
- [ ] Write tests:
  - Vernal equinox solar noon at the equator: solar altitude ≈ 90°
  - Known angle-of-incidence calculation for a south-facing vertical surface
  - Night-time input (solar below horizon): `surface_irradiance` returns exactly 0 W/m² for all components (clamped to zero, not merely near-zero)

## Files to Touch
- `crates/hares-physics/src/solar.rs`: new file — `SolarPosition`, all solar geometry functions
- `crates/hares-physics/src/lib.rs`: add `pub mod solar`

## Measures of Success
- [ ] Equatorial equinox noon test: computed altitude within ±0.5° of 90°
- [ ] AOI test: computed angle matches analytic reference within ±0.1°
- [ ] Night-time irradiance test: all components = 0 W/m² (clamped to zero)
- [ ] `window_transmitted_solar` is linear in `shgc`, `area_m2`, and `irradiance_w_m2`
- [ ] No unexplained numeric literals in solar geometry equations; constants are named once and reused

## Notes

- `SolarPosition` is an intermediate computation type that lives in `hares-physics` by design — it is not a data carrier and does not belong in `hares-types`. `SurfaceIrradiance` is the data carrier type defined in `hares-types` (see HARES-002 for its type contract).
- `hares-physics` depends on `hares-types` (present in `Cargo.toml`). Returning `SurfaceIrradiance` (defined in `hares-types`) from `hares-physics` functions is consistent with this dependency and requires no special crate configuration.

## Verification
- [ ] `cargo check -p hares-physics` passes
- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo clippy -p hares-physics -- -D warnings` passes
