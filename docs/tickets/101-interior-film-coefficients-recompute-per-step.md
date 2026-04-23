# Interior Film Coefficients Must Recompute Per Timestep, Not Once at Init

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-physics/film_coefficients, hares-core/dwelling

## Problem

Interior film coefficients are still computed once at init using `T_outdoor_avg − T_conditioned` despite the S1 partial fix. The S1 fix at `crates/hares-physics/src/film_coefficients.rs` and `crates/hares-core/src/dwelling/conversions.rs:131` lowered the previous 12.9°C ΔT floor to 0.1°C — but the underlying defect remains: the coefficients are frozen at init-time and never updated as zone temperatures evolve through the simulation.

Real interior convective film coefficients depend on the instantaneous surface-to-air ΔT. EnergyPlus recomputes h_conv every timestep via the TARP, MoWiTT, or Fohanno-Polidori models (per Engineering Reference §3.5). HARES freezes the value, so any departure from the assumed init ΔT injects a systematic surface-to-air heat-transfer bias.

## Current Behavior

`crates/hares-physics/src/film_coefficients.rs` and `crates/hares-core/src/dwelling/conversions.rs:131`:
- Compute h_conv_interior at init using design ΔT
- Floor previously 12.9°C; S1 lowered to 0.1°C (handles the divide-by-zero edge case)
- Value used through the entire simulation regardless of actual ΔT

In a typical residential cooling-mode simulation, the design init ΔT might be 11°C (indoor 24°C, outdoor avg 35°C summer), but the actual surface-to-air ΔT drifts as low as 1°C in steady-state — TARP h_conv at 1°C ΔT differs from h_conv at 11°C ΔT by roughly 30%.

## Required Behavior

1. h_conv_interior must be recomputed every timestep using the current zone air temperature and the current interior surface temperature.
2. Recomputation site: inside the thermal solver step, before the conduction matrix is assembled (or as part of the assembled matrix update if the solver linearises around the previous step).
3. Use the same TARP (or whichever ASHRAE-grade model is currently selected) coefficients applied at init.
4. The 0.1°C ΔT floor remains for numerical stability at exactly-equal temperatures.

## Approach

1. Identify the call site in `crates/hares-envelope/src/thermal_solver/` that currently consumes the init-time h_conv values (or where they live in the precomputed `BoundaryConductances`).
2. Replace the precomputed value with a per-step computation using the current `(T_surface, T_air)` pair from the previous step's solution.
3. The matrix that depends on h_conv must be reassembled or partially updated each step. If the solver is implicit and linearises around the previous step, this becomes a per-step update of the relevant matrix entries.
4. Add a regression test: in a free-floating simulation with a known surface-air ΔT trajectory, verify h_conv tracks the trajectory rather than remaining frozen at the init value.
5. Quantify the change in BESTEST 600/900 results — the per-step update should improve agreement with the ASHRAE 140 reference band.

## Definition of Done

- [ ] h_conv_interior recomputed per timestep using current surface and air temperatures
- [ ] 0.1°C ΔT floor preserved for numerical stability
- [ ] Per-step recomputation cost is bounded (no per-step heap allocation; pre-allocate any temporary buffers)
- [ ] Regression test: h_conv tracks ΔT trajectory in a free-floating run
- [ ] BESTEST 600/900 annual heating/cooling improved or at least unchanged
- [ ] Documentation comment cites the TARP model and the per-step update rationale

## Verification

```bash
cargo test -p hares-physics film_coefficients
cargo test -p hares-envelope thermal_solver
cargo test --test bestest
```

## References

- EnergyPlus Engineering Reference §3.5.4 "Interior Convection Algorithms" — TARP, MoWiTT, Fohanno-Polidori; per-step recomputation requirement.
- ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.2 "Free Convection" and §4.3 "Forced Convection at Surfaces" — h_conv as a function of ΔT.
- TARP method: Walton, G. N. (1983) *Thermal Analysis Research Program Reference Manual*, NBSIR 83-2655, National Bureau of Standards.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation (StarMesh derivation depends on consistent h_conv)
- 090-rederive-per-surface-ua-from-first-principles (per-surface UA computed from h_conv)
- 044-lwr-fallback-linearised-not-scriptf
