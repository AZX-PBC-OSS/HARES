---
id: THERMAL-006
title: Synthetic box analytical validation tests
kind: implement
depends_on: [THERMAL-005, THERMAL-006a]
files_to_touch:
  - crates/hares-envelope/tests/synthetic_box.rs
references:
  - crates/hares-envelope/src/state_space.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
verification:
  - cargo test -p hares-envelope --test synthetic_box
---

## Background/Context

Validate the implicit Crank-Nicolson solver against analytical solutions for
simple geometries. These tests verify HARES against physics, not against a
reference implementation. They serve as a permanent regression guard for the
thermal solver core.

Tests use the existing `one_zone_solver()` helper pattern from
`thermal_solver/mod.rs` tests, extended for synthetic RC parameters.

## Work to Do

- [ ] Create `crates/hares-envelope/tests/synthetic_box.rs`

- [ ] `test_1r1c_exponential_decay`:
      Single zone, R=0.05 K/W (UA=20 W/K), C=200 kJ/K. Zone at 20°C,
      outdoor constant 0°C. No HVAC, no solar, no infiltration. dt=60s, 24h.
      Analytical: `T(t) = T_out + (T_0 - T_out) · exp(-t/RC)`.
      τ = RC = 10000s. T(24h) ≈ 0.003°C.
      Assert HARES matches analytical within 0.05°C at every hour.

- [ ] `test_1r1c_steady_state_with_hvac`:
      Same box + constant heating setpoint 20°C. Outdoor 0°C.
      Steady-state HVAC = UA × ΔT = 20 × 20 = 400 W.
      Run 24h (>>5τ). Assert final-step HVAC power within 1% of 400 W.

- [ ] `test_1r1c_solar_step_response`:
      500 W constant solar gain to zone. No HVAC. Outdoor 0°C.
      Steady-state: T_ss = T_out + Q_solar × R = 0 + 500 × 0.05 = 25°C.
      Assert HARES reaches 25°C ± 0.1°C within 5τ (50000s ≈ 14h).

- [ ] `test_1r1c_with_moderate_infiltration`:
      ACH=0.05, V=400 m³ with HVAC setpoint 20°C, outdoor 0°C.
      With semi-implicit infiltration (THERMAL-006a), the infiltration
      coupling is part of the implicit solve, so the ideal HVAC correctly
      compensates for both conduction and infiltration losses.
      Steady-state HVAC = (UA + ρ·cp·ACH·V/3600) × ΔT ≈ 534 W.
      Assert HVAC converges to analytical within 2%.

- [ ] `test_implicit_stability_extreme_ach`:
      Zone at 20°C, outdoor at -10°C, ACH=50 (vented attic), C=50kJ/K, dt=60s.
      With semi-implicit infiltration (THERMAL-006a), the infiltration coupling
      coefficient is part of the implicit solve, making this unconditionally stable.
      Assert solver keeps zone in [-10, 20]°C after one step.
      Assert zone converges toward outdoor temp monotonically (no oscillation).

- [ ] `test_implicit_vs_explicit_agreement_stable_case`:
      Build same 1R1C system. Step with both matrix-exponential (via
      `discretize_auto`) and Crank-Nicolson for 1000 steps with same inputs.
      Assert both reach same steady state within 0.01°C.
      Assert transient trajectory differs by < 0.1°C at each step
      (CN is O(dt²) vs exact for matrix-exp, so small transient diff expected).

- [ ] `test_2r2c_eigenvalue_verification`:
      Two-node wall+zone with known R1, R2, C1, C2.
      Construct ThermalSolver, extract equivalent `A_d = M⁻¹N`.
      Hand-compute A_c, compute expected A_d via `matrix_exp(A_c·dt)`.
      Compare eigenvalues within 1e-4 (CN approximation differs from exact
      matrix exponential; eigenvalues converge as dt/τ → 0).

- [ ] `test_near_zero_thermal_mass`:
      R=0.05 K/W, C=1 J/K (near-zero mass, e.g. uninsulated metal surface).
      dt=60s. Assert no NaN, no division-by-zero, zone converges rapidly to
      steady state without oscillation.

- [ ] `test_step_into_zero_allocations`:
      Use a counting global allocator. Run 100 steps via `step_into()`.
      Assert zero heap allocations during the stepping loop.

## Files to Touch

- `crates/hares-envelope/tests/synthetic_box.rs`: New test file

## Measures of Success

- [ ] All analytical tests match within specified tolerances.
- [ ] Stability test demonstrates no overshoot with extreme infiltration.
- [ ] Implicit/explicit agreement validates the CN discretization.
- [ ] Zero-allocation test confirms hot-loop performance.

## Verification

- [ ] `cargo test -p hares-envelope --test synthetic_box` passes
