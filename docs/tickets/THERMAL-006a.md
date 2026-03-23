---
id: THERMAL-006a
title: Semi-implicit infiltration treatment per EnergyPlus zone air heat balance
kind: fix
depends_on: [THERMAL-005]
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/infiltration.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/src/state_space.rs
references:
  - "EnergyPlus 24.1 Engineering Reference, Basis for Zone and Air System Integration"
  - "https://bigladdersoftware.com/epx/docs/22-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html"
  - "OCHRE Envelope.py: update_infiltration + h_limit clamp (lines 629-653)"
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core
  - cargo clippy -p hares-envelope
---

## Background/Context

Infiltration is currently computed as an explicit heat gain
`q = m_dot * cp * (T_out - T_zone)` and injected into the state-space input
vector. This is how OCHRE does it, and OCHRE patches the resulting instability
with an `h_limit` clamp (a stability hack).

With the Crank-Nicolson solver from THERMAL-005, the A-matrix decay is
unconditionally stable, but infiltration is still an explicit per-step forcing
term. For extreme ACH (vented attics, crawlspaces), the explicit infiltration
gain can exceed the zone's thermal capacity in a single timestep, causing
overshoot even with the implicit solver.

### EnergyPlus approach (correct)

EnergyPlus treats infiltration **semi-implicitly** in the zone air heat balance:

```
C_z dT/dt = ... + m_inf*cp*(T_out - T_zone)
```

The `m_inf*cp*T_zone` term goes on the left side (implicit, evaluated at new
timestep T_zone^{k+1}), while `m_inf*cp*T_out` stays as explicit forcing.
This is equivalent to adding `m_inf*cp` to the effective UA coefficient in
the A-matrix diagonal:

```
A_c[zone,zone] -= h_inf / C_zone     (increase decay rate)
u[outdoor_idx]  += h_inf * T_out / C_zone  (or add to sensible input as T_out forcing)
```

Where `h_inf = m_dot * cp` is the infiltration conductance [W/K].

This makes infiltration **unconditionally stable** regardless of ACH or timestep.

### What needs to change

The CN matrices (M, N, B_eff) are pre-computed at construction and currently
immutable per step. Infiltration varies per-step (ACH depends on wind speed
and temperature difference). Two approaches:

**Approach A: Modify CN matrices per step (correct but complex)**
- Each step: compute h_inf, rebuild M_eff and N_eff with modified A_c diagonal,
  re-factor LU. O(n^3) per step instead of O(n^2). For n=10-30 this is ~1-5 us
  per step, acceptable for 1-min timesteps.

**Approach B: Split infiltration into implicit coefficient + explicit forcing (simpler)**
- Keep CN matrices fixed (pre-computed from conduction-only A_c).
- Each step: compute h_inf per zone.
- Modify the CN step to include the infiltration coupling implicitly:
  `(M + dt/2 * diag(h_inf/C)) * x[k+1] = (N - dt/2 * diag(h_inf/C)) * x[k] + B_eff*u + dt*diag(h_inf*T_out/C)`
- This requires a per-step LU factorization of the modified M matrix. Same O(n^3)
  cost as Approach A but the base M/N are reused and only the diagonal perturbation changes.

**Approach C: Additive correction to pre-factored solve (most efficient)**
- Use the Sherman-Morrison-Woodbury formula to update the LU solve with
  rank-k diagonal perturbation. For k=number_of_zones (typically 1-3), this
  is O(n^2) per zone per step. Preserves the zero-allocation hot-loop goal.

**Recommended: Approach B** — clearest, matches EnergyPlus derivation directly,
and O(n^3) for n<30 is sub-microsecond.

## Work to Do

- [ ] Add `step_with_infiltration()` method to `StateSpaceModel` or `ThermalSolver`:
      Takes the base CN matrices plus per-zone infiltration conductances h_inf[zone].
      Modifies the implicit/explicit matrices with the diagonal perturbation and solves.

- [ ] Rewrite `apply_infiltration_and_ventilation()` to return per-zone
      `(h_inf_w_k: f64, q_forcing_w: f64)` instead of injecting watts into u.
      - `h_inf = m_dot * cp` (infiltration conductance, W/K)
      - `q_forcing = h_inf * T_out` (explicit forcing in watts·K... actually
        the term is `h_inf * T_out / C` in the ODE, but in the u vector it's
        just `h_inf * T_out` since B_c already has 1/C).
      - Latent loads remain unchanged (explicit per-step, not temperature-dependent).

- [ ] In `resolve_internal()`, after `build_input_vector()`:
      - Collect per-zone h_inf values
      - Build diagonal perturbation matrix (or sparse representation)
      - Call the modified step that includes the implicit infiltration coupling

- [ ] Update ideal HVAC solve to account for the modified system matrices
      (infiltration coupling affects the gain computation).

- [ ] Remove the explicit `q_sensible` injection into `u[idx]` for infiltration.
      The implicit coupling replaces it entirely.

- [ ] Update all infiltration tests to validate the semi-implicit behavior:
      - Extreme ACH (50+) should converge without overshoot
      - Moderate ACH should match analytical steady-state HVAC power

## Files to Touch

- `crates/hares-envelope/src/thermal_solver/infiltration.rs`: return h_inf instead of injecting q
- `crates/hares-envelope/src/thermal_solver/mod.rs`: modified stepping with implicit infiltration
- `crates/hares-envelope/src/state_space.rs`: step method variant with diagonal perturbation

## Measures of Success

- [ ] ACH=50, dt=60s, C=50kJ/K: zone stays in [T_out, T_initial] after one step.
- [ ] ACH=50: monotonic convergence toward outdoor temp (no oscillation).
- [ ] ACH=0.05 with HVAC: steady-state HVAC matches (UA+UA_inf)*dT within 2%.
- [ ] No h_limit clamp needed — stability by construction.
- [ ] Performance: < 2x regression from pre-infiltration-fix stepping rate.

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-envelope` clean
