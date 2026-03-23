---
id: THERMAL-001
title: Use dry air density for infiltration mass flow
kind: fix
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/infiltration.rs
references:
  - "ASHRAE Fundamentals 2021, Ch. 6: rho_da = rho_moist / (1 + W)"
  - vendors/OCHRE/ochre/Models/Humidity.py (lines 60-65)
verification:
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope
---

## Background/Context

Infiltration sensible load uses `q = m_dot * cp_dry * ΔT`. The mass flow rate
should be dry air mass per second: `m_dot_dry = rho_moist * Q / (1 + W)` where
W is the humidity ratio [kg_water/kg_dry_air]. HARES currently uses moist air
density directly, overestimating mass flow by ~0.5-1%.

OCHRE (`Humidity.py:60-65`) and ASHRAE both use `rho_dry = rho_moist / (1 + w)`.

This affects BOTH sensible and latent mass flows (lines 123-124 in
`infiltration.rs`) since `rho` is used for both `m_dot_sens` and `m_dot_lat`.

## Work to Do

- [ ] In `infiltration.rs` line 31, change:
      ```rust
      let rho = moist_air_density_kg_m3(p_pa, t_out, w_out);
      ```
      to:
      ```rust
      let rho = moist_air_density_kg_m3(p_pa, t_out, w_out) / (1.0 + w_out);
      ```
      This single change fixes both `m_dot_sens` (line 123) and `m_dot_lat`
      (line 124) since both derive from `rho`.
- [ ] Add inline `#[cfg(test)]` test `test_dry_density_lower_than_moist`:
      At 20°C, w=0.010, 101325 Pa: assert `rho_dry < rho_moist` and
      `assert_relative_eq!((rho_moist - rho_dry) / rho_moist, w / (1.0 + w), epsilon = 1e-6)`.

## Files to Touch

- `crates/hares-envelope/src/thermal_solver/infiltration.rs`: density fix + test

## Measures of Success

- [ ] Infiltration sensible AND latent gains are ~1% lower at typical humidity.
- [ ] No regression in existing tests.

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope` clean
