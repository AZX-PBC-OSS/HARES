---
id: HARES-071
title: "hares-core — Runtime Numerical Invariant Checks"
kind: implement
depends_on: [HARES-043]
phase: 3
crate: hares-core
files_to_touch:
  - crates/hares-core/src/invariants.rs
  - crates/hares-core/src/dwelling.rs
references:
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo test -p hares-core
  - cargo test -p hares-core --features check_invariants
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context

`07-testing-and-verification.md` specifies per-timestep runtime assertions for energy/mass conservation and physical bounds. These catch numerical drift, sign errors, and integration bugs during simulation — errors that would otherwise produce plausible-looking but incorrect outputs. The checks must be gated so they add zero overhead in release builds and a configurable overhead in debug/validation builds.

## Work to Do

- [ ] Create `crates/hares-core/src/invariants.rs` with an `InvariantChecker` struct:
  - [ ] `InvariantChecker::check_thermal(q_gains: &[f64], delta_e_storage: f64, q_loss: f64) -> Result<(), InvariantViolation>`
    - Tolerance: `|Σ(Q_gain) - ΔE_storage - Q_loss_envelope| < max(1.0, 1e-6 · |Σ Q_gain|)` (units: W)
  - [ ] `InvariantChecker::check_electrical(p_grid: f64, p_equipment_ports: &[f64]) -> Result<(), InvariantViolation>`
    - Tolerance: `|P_grid + Σ P_equipment_ports| < 0.001` (units: kW)
  - [ ] `InvariantChecker::check_moisture(delta_m_water: f64, q_latent_terms: &[(f64, f64)]) -> Result<(), InvariantViolation>`
    - Each term is `(Q_latent_i, dt_s)`; `h_fg = 2_501_000 J/kg`
    - Tolerance: `|Δm_water - Σ(Q_latent_i · dt / h_fg)| < 1e-6` (units: kg)
  - [ ] `InvariantChecker::check_soc(soc: f64, accumulated_error: f64) -> Result<(), InvariantViolation>`
    - Clamp `soc` to `[0.0, 1.0]`; warn via `tracing::warn!` if `accumulated_error.abs() > 0.001`
  - [ ] `InvariantChecker::check_temperatures(zone_temps_c: &[f64], tank_temps_c: &[f64]) -> Result<(), InvariantViolation>`
    - Zone temps: `[-50.0, 80.0]` °C; tank temps: `[0.0, 100.0]` °C
  - [ ] `InvariantViolation` error type with fields: `check_name: &'static str`, `value: f64`, `tolerance: f64`
- [ ] Gate checks in the dwelling orchestrator timestep loop (in `crates/hares-core/src/dwelling.rs`):
  - [ ] Always active when `cfg(feature = "check_invariants")`
  - [ ] Also active when `cfg(debug_assertions)` (i.e., dev builds)
  - [ ] Silent in release builds without the feature flag
- [ ] On violation: return `Err(HaresError::InvariantViolation(...))` rather than panic — the engine quarantines the offending dwelling

## Measures of Success

- [ ] A deliberately broken equipment (sign-flipped thermal gain) triggers the thermal invariant check and returns `Err`
- [ ] All invariant checks pass on the full Phase 3 test suite (HARES-043, 044, 045, 046 tests)
- [ ] Invariant checks add < 1% wall-clock overhead when enabled, measured via `cargo bench` with and without `--features check_invariants` on the 30-day single-building benchmark
- [ ] `InvariantViolation` error message identifies the violated check by name, reports the actual value and the tolerance

## Verification

- [ ] `cargo test -p hares-core` passes
- [ ] `cargo test -p hares-core --features check_invariants` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
