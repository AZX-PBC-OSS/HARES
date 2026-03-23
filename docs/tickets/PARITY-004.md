---
id: PARITY-004
title: "Quality: Unit test coverage for physics modules"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/solar.rs
  - crates/hares-physics/src/psychrometrics.rs
  - crates/hares-physics/src/air_properties.rs
  - crates/hares-physics/src/film_coefficients.rs
  - crates/hares-envelope/src/longwave_radiation.rs
  - crates/hares-envelope/src/rc_network.rs
  - crates/hares-envelope/src/state_space.rs
  - crates/hares-envelope/src/boundary_rc.rs
references:
  - feedback_test_quality.md
  - EnergyPlus Engineering Reference (for reference values)
verification:
  - cargo test --workspace
---

## Background/Context

Audit found all 324 tests are in `tests/` directories (integration/parity tests). There are zero `#[test]` functions inside `src/` files. Physics modules like solar position, psychrometrics, film coefficients, and longwave radiation have no unit tests verifying individual functions against known analytical solutions.

Unit tests are critical for:
- Catching regressions when refactoring (PARITY-100, 101)
- Validating individual physics functions before integration
- Documenting expected behavior at boundary conditions

## Work to Do

### hares-physics unit tests

- [ ] **solar.rs**: Solar position at known dates/locations (equinox at equator = 90° altitude), window IAM at 0° = 1.0, IAM at 90° = 0.0, Perez model at known conditions
- [ ] **psychrometrics.rs**: Wet-bulb at known conditions (ASHRAE tables), saturation pressure at 0°C = 611 Pa, humidity ratio at 100% RH
- [ ] **air_properties.rs**: Dry air density at STP = 1.225 kg/m³, moist air corrections
- [ ] **film_coefficients.rs**: TARP interior at vertical surface matches DOE-2 values, exterior at known wind speed

### hares-envelope unit tests

- [ ] **longwave_radiation.rs**: Stefan-Boltzmann at 300K, sky view factor at tilt=0 = 1.0, tilt=90 ≈ 0.5
- [ ] **rc_network.rs**: 2-node network produces correct A_c matrix, parallel resistance combination
- [ ] **state_space.rs**: Identity system (A=0, B=I) → x stays constant, eigenvalue check on known unstable system
- [ ] **boundary_rc.rs**: Single-layer wall R-value matches manual calculation, zone capacitance formula verification

### Test quality standards

- [ ] Each test validates ONE behavior
- [ ] Tests use `approx::assert_relative_eq!` with explicit tolerance (not `assert_eq!` on floats)
- [ ] Tests include boundary conditions (0, max, NaN guard)
- [ ] Tests are named descriptively: `test_solar_altitude_at_equinox_equator_is_90_degrees`

## Measures of Success

- [ ] At least 50 new unit tests across physics + envelope crates
- [ ] Every public function in hares-physics has at least one unit test
- [ ] Every physics constant is validated against a reference source (ASHRAE, EnergyPlus)

## Verification

- [ ] `cargo test --workspace` passes
- [ ] `cargo test --workspace -- --ignored` runs extended validation tests
