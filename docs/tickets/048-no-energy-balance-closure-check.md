# No Energy Balance Closure Check at Zone Level

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope, hares-core

## Problem

There is no per-timestep check that the zone energy balance closes. The thermal solver integrates `x[k+1] = A_d·x[k] + B_d·u[k]`; the first-law expectation for each conditioned zone is:

```
C_zone × (T_zone[k+1] - T_zone[k]) / dt = Σ Q_in - Σ Q_out
```

where `C_zone` is the zone air thermal capacitance (J/K) and `Σ Q_in - Σ Q_out` sums all port sensible contributions (HVAC, occupancy, solar, LWR, infiltration). Without this check, sign errors in port injection, incorrect `B_d` entries, or mis-wired port indices accumulate silently over thousands of timesteps.

EnergyPlus Engineering Reference §13.5 "Zone Energy Balance" reports a per-zone closure metric each timestep and warns when it exceeds 0.001 W. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 states that any valid heat balance method must demonstrate closure. The ZOH state-space formulation is algebraically exact; residuals above 1 W at release precision indicate a wiring or port-injection bug, not floating-point roundoff.

## Current Behavior

`hares-core/src/dwelling/mod.rs:2335`: `check_invariants(dt)?` contains equipment-contract and output checks but no zone thermal balance closure.

`hares-core/src/invariants.rs`: no zone thermal balance check.

`hares-envelope/src/thermal_solver/stepping.rs:146–208`: `integrate_inner` computes `y_next` but does not verify `C_zone × dT/dt ≈ Σ Q_in`.

Boundary diagnostics at `stepping.rs:147–202` are `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` — not available in release builds, and do not perform a closure check.

## Required Behavior

1. Add a zone energy balance closure check to `integrate_inner` (`stepping.rs`), gated on `debug_assertions` or a `check_energy_balance` cargo feature. For each conditioned zone:
   - Compute `delta_stored = C_zone_j_k × (T_next - T_curr) / dt` (W).
   - Compute `q_net = Σ Q_in` from all port sensible contributions for that zone.
   - Assert `|delta_stored - q_net| < threshold_w`.
   - Thresholds: debug builds → 0.01 W; release with feature → 1.0 W (per EnergyPlus §13.5).

2. Add `C_zone_j_k: f64` (J/K) to `StateSpaceWiring` so `integrate_inner` can access it without re-deriving from the RC network each timestep.

3. Publish the residual as telemetry key `ENERGY_BALANCE_RESIDUAL_W` per conditioned zone in the `DomainUpdate` custom payload, available in all build configurations.

4. Add a test in `hares-envelope/tests/solver_energy_conservation.rs` (file already exists) verifying closure to 0.01 W over 100 timesteps under realistic boundary conditions (non-zero HVAC, solar, and infiltration gains simultaneously active).

Reference: EnergyPlus Engineering Reference §13.5 "Zone Energy Balance"; ASHRAE HoF 2021 Ch. 18 §18.2; Patankar "Numerical Heat Transfer and Fluid Flow" §3.4.

## Approach

1. In `StateSpaceWiring`, add `pub c_zone_j_k: f64` populated from the zone air node's capacitance during RC network construction.
2. In `integrate_inner`, after computing `y_next`, loop over conditioned zones: extract `T_curr` and `T_next` from the state vector, compute `delta_stored` and `q_net` from the port accumulator totals, compute residual, emit `tracing::warn!` if residual > 1.0 W in any build, assert in debug.
3. Add telemetry emission for `ENERGY_BALANCE_RESIDUAL_W`.

## Definition of Done

- [ ] `StateSpaceWiring` has `c_zone_j_k: f64` populated at construction
- [ ] `integrate_inner` computes energy balance residual per conditioned zone
- [ ] Debug builds assert `|residual| < 0.01 W`; release emits `tracing::warn!` when `|residual| > 1.0 W`
- [ ] Telemetry key `ENERGY_BALANCE_RESIDUAL_W` published per zone each timestep
- [ ] `solver_energy_conservation.rs` test: closure to < 0.01 W over 100 timesteps with HVAC + solar + infiltration active
- [ ] `cargo test -p hares-envelope` passes

## Verification

```bash
cargo test -p hares-envelope solver_energy_conservation
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §13.5 "Zone Energy Balance" — closure criterion 0.001 W, per-timestep reporting
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — energy closure requirement
- Patankar, S. "Numerical Heat Transfer and Fluid Flow" §3.4 — conservation check for finite-difference schemes
- `hares-envelope/tests/solver_energy_conservation.rs` — existing partial conservation tests
- `hares-envelope/src/thermal_solver/stepping.rs:146–208` — `integrate_inner` integration path
