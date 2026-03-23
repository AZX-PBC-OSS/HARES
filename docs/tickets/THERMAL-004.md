---
id: THERMAL-004
title: Pre-refactor test coverage for thermal solver
kind: implement
depends_on: [THERMAL-003]
files_to_touch:
  - crates/hares-envelope/tests/multi_zone_coupling.rs
  - crates/hares-envelope/tests/interior_lwr.rs
  - crates/hares-envelope/tests/solver_energy_conservation.rs
references:
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/src/longwave_radiation.rs
verification:
  - cargo test -p hares-envelope
---

## Background/Context

The thermal solver audit identified critical test coverage gaps that must be
filled before the Crank-Nicolson refactor. These tests establish the baseline
behavior that the implicit solver must preserve.

## Work to Do

### Multi-zone thermal coupling test

- [ ] Create `crates/hares-envelope/tests/multi_zone_coupling.rs`
- [ ] `test_two_zone_coupled_wall_heat_direction`:
      Zone 1 at 25°C, Zone 2 at 15°C, connected by a wall (R=0.1 K/W).
      Step once. Assert zone 1 cools and zone 2 warms.
- [ ] `test_two_zone_coupled_wall_steady_state`:
      Same setup with outdoor at 0°C. Run 24h with HVAC setpoint 20°C in zone 1.
      Assert both zones converge. Assert HVAC power > 0 (zone 1 loses heat to
      zone 2 which loses to outdoor).

### Interior LWR energy balance test

- [ ] Create `crates/hares-envelope/tests/interior_lwr.rs`
- [ ] `test_interior_lwr_net_flux_is_zero`:
      Create 4 interior surfaces forming a zone. All at same temperature.
      Assert sum of interior LWR fluxes = 0 (energy conservation).
- [ ] `test_interior_lwr_hot_surface_loses_heat`:
      One surface at 30°C, three at 20°C. Assert the hot surface has negative
      net LWR (losing heat) and the cold surfaces have positive (gaining heat).
- [ ] `test_interior_lwr_identical_surfaces_symmetric`:
      Two identical surfaces at different temps. Assert |flux_A_to_B| = |flux_B_to_A|.

### Energy conservation over long runs

- [ ] Create `crates/hares-envelope/tests/solver_energy_conservation.rs`
- [ ] `test_energy_conservation_1r1c_no_hvac`:
      Zone at 20°C, outdoor at 0°C, no HVAC. Run 24h.
      Compute: ΔE_thermal = C × (T_final - T_initial).
      Compute: Q_loss = Σ(UA × (T_zone[k] - T_out) × dt) over all steps.
      Assert |ΔE_thermal + Q_loss| < 0.1% of Q_loss (energy balance).
- [ ] `test_energy_conservation_with_hvac`:
      Same setup + HVAC setpoint 20°C. Run 24h.
      Q_hvac = Σ(hvac_power[k] × dt).
      Q_loss = Σ(UA × (T_zone[k] - T_out) × dt).
      Assert |Q_hvac - Q_loss| < 1% at steady state.

## Files to Touch

- `crates/hares-envelope/tests/multi_zone_coupling.rs`: New
- `crates/hares-envelope/tests/interior_lwr.rs`: New
- `crates/hares-envelope/tests/solver_energy_conservation.rs`: New

## Measures of Success

- [ ] All tests pass with the current explicit solver (baseline).
- [ ] Tests will also pass with the implicit solver (THERMAL-003 regression guard).

## Verification

- [ ] `cargo test -p hares-envelope` passes
