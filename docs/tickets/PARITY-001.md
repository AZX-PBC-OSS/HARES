---
id: PARITY-001
title: "Refactor: Split thermal_solver/mod.rs into focused modules"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/src/thermal_solver/solar.rs
  - crates/hares-envelope/src/thermal_solver/longwave.rs (new)
  - crates/hares-envelope/src/thermal_solver/infiltration.rs
  - crates/hares-envelope/src/thermal_solver/ports.rs (new)
  - crates/hares-envelope/src/thermal_solver/stepping.rs (new)
references:
  - docs/architecture.md (Thermal State Solver section)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Prerequisites

THERMAL-003 (pre-refactor cleanup), THERMAL-005 (Crank-Nicolson implicit solver), and THERMAL-006a (semi-implicit infiltration) will have completed before this ticket. THERMAL-003 splits `resolve_internal()` into phases and fixes structural issues. THERMAL-005 replaces the explicit stepper with Crank-Nicolson `step_into()`. THERMAL-006a changes infiltration from explicit q-injection to semi-implicit conductance coupling (h_inf modifies the CN step matrices per-timestep). After all three, `thermal_solver/mod.rs` will be cleaner but still oversized.

## Background/Context

After THERMAL-003/005, `thermal_solver/mod.rs` will still mix unrelated physics domains: solar application, longwave radiation, infiltration, port aggregation, and zone state management. It must be split into focused modules before PARITY-015 (iterative LWR) and PARITY-018 (solar distribution) add more complexity.

## Work to Do

Extract into focused modules while preserving the zero-allocation hot-path pattern (u_buf swap, latent_buf swap):

- [ ] **`thermal_solver/stepping.rs`** (~200 lines): State-space integration
  - After THERMAL-005: wraps the Crank-Nicolson `step_into()` and steady-state initialization
  - Owns `x: DVector<f64>`, `last_u: DVector<f64>`, `u_buf: DVector<f64>`
  - No new physics — just extraction of the implicit solver into its own module

- [ ] **`thermal_solver/longwave.rs`** (~300 lines): Interior + exterior LWR
  - Move `apply_exterior_longwave_inputs_iterative()` and `apply_interior_longwave_inputs()`
  - Owns `ExteriorSurfaceInfo` state (t_prev_c warm-start temperatures)
  - Will be the landing zone for PARITY-001 ScriptF implementation

- [ ] **`thermal_solver/solar.rs`** (extend existing ~83 lines → ~200 lines):
  - Already has `apply_solar_inputs()` and `apply_exterior_solar_inputs()`
  - Will be the landing zone for PARITY-002 and PARITY-003

- [ ] **`thermal_solver/ports.rs`** (~100 lines): Port-to-input-vector aggregation
  - `fn apply_port_sensible_inputs(ports, config, u)` — read thermal accumulators, write to u
  - `fn apply_occupancy_gains(ports, config, occupancy)`

- [ ] **`thermal_solver/infiltration.rs`** (already exists, will be modified by THERMAL-006a):
  - After THERMAL-006a: returns per-zone `(h_inf_w_k, q_forcing_w)` instead of injecting into u
  - Keep as standalone module; verify semi-implicit interface is clean

- [ ] **`thermal_solver/mod.rs`** (~400 lines): Orchestrator only
  - `ThermalSolver` struct with buffer ownership
  - `resolve_internal()` orchestrates: build_input → apply_outdoor → apply_solar → apply_lwr → apply_ports → collect_infiltration_conductances → step_with_infiltration → output
  - Note: after THERMAL-006a, the step includes implicit infiltration coupling (diagonal perturbation of CN matrices), not just a simple `step(u)` call
  - No physics equations — only sequencing and buffer management

### Quality Requirements

- [ ] No function > 80 lines in any extracted module
- [ ] All extracted modules take `&mut DVector<f64>` for input vector (no allocation)
- [ ] All methods are `pub(super)` or `pub(crate)` — no public API change
- [ ] Zero new allocations in hot path (verify with profiling feature)
- [ ] All existing tests pass unchanged

## Files to Touch

- `crates/hares-envelope/src/thermal_solver/mod.rs`: Reduce from 3107 → ~400 lines (orchestrator)
- `crates/hares-envelope/src/thermal_solver/stepping.rs`: New — state-space integration
- `crates/hares-envelope/src/thermal_solver/longwave.rs`: New — LWR physics
- `crates/hares-envelope/src/thermal_solver/ports.rs`: New — port aggregation
- `crates/hares-envelope/src/thermal_solver/solar.rs`: Extend with extracted methods

## Measures of Success

- [ ] No file in thermal_solver/ exceeds 500 lines
- [ ] `resolve_internal()` is < 60 lines (pure orchestration)
- [ ] All 47 envelope tests pass
- [ ] Simulation results are bit-identical before and after refactor

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] Smoke test produces identical output to pre-refactor baseline
