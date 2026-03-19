---
id: HARES-005
title: "hares-physics — Psychrometrics and Air Properties"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/psychrometrics.rs
  - crates/hares-physics/src/air_properties.rs
  - crates/hares-physics/src/lib.rs
references:
  - vendors/OCHRE/ochre/utils/psychrolib_jit.py
  - docs/architecture/appendix-physics-improvements.md
verification:
  - cargo check -p hares-physics
  - cargo test -p hares-physics
  - cargo clippy -p hares-physics -- -D warnings
---

## Background/Context
Psychrometric calculations underpin HVAC modelling, infiltration latent loads, and zone humidity tracking. Implementing them in pure Rust eliminates the Python psychrolib dependency at the inner simulation loop and enables verification against ASHRAE reference tables.

## Work to Do
- [ ] Implement in `psychrometrics.rs`:
  - `saturation_pressure_pa(t_c: f64) -> f64`
  - `humidity_ratio_from_tdp(t_dp_c: f64, p_pa: f64) -> f64`
  - `humidity_ratio_from_twb(t_db_c: f64, t_wb_c: f64, p_pa: f64) -> f64`
  - `relative_humidity(t_db_c: f64, w: f64, p_pa: f64) -> f64`
  - `wet_bulb_from_humidity_ratio(t_db_c: f64, w: f64, p_pa: f64) -> f64` using iterative bisection
  - `dew_point(w: f64, p_pa: f64) -> f64`
  - `moist_air_enthalpy(t_db_c: f64, w: f64) -> f64`
- [ ] Implement in `air_properties.rs`:
  - `standard_pressure_pa(elevation_m: f64) -> f64` using ISA formula: `101325.0 * (1.0 - 2.25577e-5 * z).powf(5.2559)`
  - `moist_air_density_kg_m3(p_pa: f64, t_db_c: f64, w: f64) -> f64` using `p / (287.0 * (t + 273.15) * (1.0 + 1.6077687 * w_eff))` where `w_eff = w.max(1e-5)` (floor guard prevents divide-by-zero at zero humidity; matches EnergyPlus `PsyRhoAirFnPbTdbW` — see appendix §1)
  - `dry_air_density_kg_m3(p_pa: f64, t_c: f64) -> f64`
- [ ] Write tests with 20+ conditions validated against ASHRAE psychrometric tables
- [ ] Include a test confirming Denver altitude (~1609 m) yields air density ~18% below sea-level density
- [ ] Add a test for `moist_air_density_kg_m3` at `w=0` — must return a finite positive value (floor guard active)
- [ ] Add a test for `wet_bulb_from_humidity_ratio` at 100% RH (saturated air): wet-bulb must equal dry-bulb within ±0.01 °C
- [ ] Replace repeated unit-conversion and physical coefficients with named module-level constants; no unexplained numeric literals in core equations
- [ ] All physics constants should import from the shared `hares_physics::constants` module rather than being redefined locally. Each constant in that module must cite its authoritative source (ASHRAE Handbook of Fundamentals year/chapter/table, NIST CODATA, ISA 1976, etc.) in a doc comment.

## Files to Touch
- `crates/hares-physics/src/psychrometrics.rs`: new file — all psychrometric functions
- `crates/hares-physics/src/air_properties.rs`: new file — ISA pressure, moist/dry air density
- `crates/hares-physics/src/lib.rs`: add `pub mod psychrometrics` and `pub mod air_properties`

## Measures of Success
- [ ] At least 20 test cases pass against published ASHRAE values (within acceptable tolerance)
- [ ] Denver density test: `moist_air_density_kg_m3` at 1609 m is within ±1% of expected ~18% reduction vs sea level
- [ ] Bisection in `wet_bulb_from_humidity_ratio` converges to within 0.01 °C
- [ ] All functions handle edge cases (0% RH, 100% RH, 0 °C, 40 °C) without panic
- [ ] Unit conversion and model constants are defined once as named constants and reused across functions/tests

## Notes

- `hares-physics` depends on `hares-types` (confirmed in `Cargo.toml`). The psychrometrics and air-properties functions in this ticket operate on plain `f64` values, but later tickets (e.g. HARES-007 solar geometry) use types from `hares-types` (such as `SurfaceIrradiance`), so the crate-level dependency is present from the start. `uom`-typed wrappers are deferred to HARES-009, which is the gate for `uom` adoption across the physics layer.
- The gas constant `287.058 J/(kg·K)` (NIST-derived: R / M_air = 8314.462 / 28.9647) is used for improved precision over EnergyPlus/OCHRE's `287.0` approximation. Document this decision in `docs/PHYSICS_DECISIONS.md` with the test tolerance set accordingly.

## Verification
- [ ] `cargo check -p hares-physics` passes
- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo clippy -p hares-physics -- -D warnings` passes
