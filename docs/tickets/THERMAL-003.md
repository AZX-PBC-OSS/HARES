---
id: THERMAL-003
title: Pre-refactor cleanup of thermal solver for Crank-Nicolson readiness
kind: fix
depends_on: [THERMAL-002]
files_to_touch:
  - crates/hares-envelope/src/state_space.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-envelope/src/thermal_solver/config.rs
  - crates/hares-envelope/src/thermal_solver/infiltration.rs
  - crates/hares-envelope/src/longwave_radiation.rs
references:
  - docs/tickets/THERMAL-003.md
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core
  - cargo clippy -p hares-envelope
---

## Background/Context

The thermal solver is about to undergo a major refactor (THERMAL-003:
Crank-Nicolson implicit solver). An audit identified structural issues that
would amplify complexity during the refactor. Fix these BEFORE THERMAL-003 so
the implicit solver lands on clean foundations.

All changes are behavior-preserving refactors — no physics changes.

## Work to Do

### HIGH: Split `resolve_internal()` into phases (mod.rs:200-344)

The 145-line method orchestrates input assembly, state stepping, HVAC solving,
and output formatting in one monolith. The Crank-Nicolson refactor only needs
to replace the stepping phase — but can't without splitting the method.

- [ ] Extract `fn build_input_vector(&mut self, ports, env) -> (DVector, ComponentBreakdown)`
      Builds the `u` vector from outdoor, solar, LWR, port, and infiltration inputs.
      Returns the u vector and a breakdown struct with per-component gains for diagnostics.
- [ ] Extract `fn format_domain_update(&self, ...) -> DomainUpdate`
      Takes solver outputs, builds the DomainUpdate return value.
- [ ] Keep stepping + HVAC solving inline in `resolve_internal()` (this is what
      THERMAL-003 replaces).
- [ ] Verify the swap-buffer pattern (`std::mem::replace`) is consistent across
      all extracted methods.

### HIGH: Fix `matrix_exp()` panic (state_space.rs:488)

The Padé denominator LU solve uses `expect()` which panics on singular matrix.

- [ ] Change `matrix_exp()` return type from `DMatrix<f64>` to `Result<DMatrix<f64>>`.
- [ ] Add `StateSpaceError::SingularPadeMatrix` variant.
- [ ] Propagate `Result` through `discretize_zoh()` and `van_loan_discretize()`.
- [ ] Update `discretize_auto()` to propagate the error.

### HIGH: Stability check for large RC networks (state_space.rs:108-120)

Eigenvalue check is silently skipped for n > 20.

- [ ] Add `pub fn verify_stability(&self) -> Result<StabilityResult>` method on
      `StateSpaceModel` that always runs the full eigenvalue check (no size skip).
- [ ] Call it from `from_continuous()` only when `n <= 20` (existing behavior).
- [ ] Add Gershgorin circle bound as a cheap O(n²) check for n > 20:
      if max row sum of |A_d| > 1.0 + epsilon, warn.
- [ ] Log a `tracing::debug` when skipping full check for n > 20.

### MEDIUM: Consolidate duplicate solve methods (state_space.rs:137-209)

`solve_for_output_input()` and `solve_for_input()` share 80% of their logic.

- [ ] Extract private `fn solve_for_scalar_input(&self, x, u, y_target, output_index, input_index) -> Result<f64>`.
- [ ] Implement `solve_for_output_input()` as a thin wrapper.
- [ ] Implement `solve_for_input()` as a wrapper that finds the output index.

### MEDIUM: Separate wiring from config (config.rs)

`ThermalSolverConfig` mixes actual configuration (infiltration method,
ventilation params) with model wiring (index mappings). The wiring is derived
from the model structure and shouldn't be in user-facing config.

- [ ] Create `StateSpaceWiring` struct:
      ```rust
      pub struct StateSpaceWiring {
          pub zone_state_indices: HashMap<ZoneId, usize>,
          pub zone_output_indices: HashMap<ZoneId, usize>,
          pub zone_sensible_input_indices: HashMap<ZoneId, usize>,
          pub outdoor_temp_input_indices: Vec<usize>,
          pub indoor_temp_input_indices: Vec<usize>,
          pub solar_input_indices: HashMap<u32, usize>,
      }
      ```
- [ ] Move these fields from `ThermalSolverConfig` to `StateSpaceWiring`.
- [ ] Store `wiring: StateSpaceWiring` in `ThermalSolver` alongside `config`.
- [ ] Update all references in `mod.rs`, `infiltration.rs`, etc.

### MEDIUM: Move mutable surface temps out of config (config.rs + mod.rs)

`ExteriorSurfaceInfo.t_prev_c` is mutable state stored in immutable config.

- [ ] Add `exterior_surface_temps: Vec<f64>` to `ThermalSolver` (indexed parallel
      to `config.exterior_surfaces`).
- [ ] Remove `t_prev_c` from `ExteriorSurfaceInfo`.
- [ ] Update `apply_exterior_longwave_inputs_iterative()` to use the new field.
- [ ] Update `snapshot_state()` and `restore_state()` to include surface temps.

### MEDIUM: Replace hardcoded ZoneId(1) with configurable indoor zone

- [ ] Add `indoor_zone_id: ZoneId` to `ThermalSolverConfig` (default `ZoneId(1)`).
- [ ] Replace all `ZoneId(1)` references in `resolve_internal()` (lines 235, 254, 557)
      with `self.config.indoor_zone_id`.

### MEDIUM: Eliminate LWR function duplication (longwave_radiation.rs)

- [ ] Make `exterior_longwave_w()` delegate to `exterior_longwave_w_m2()`:
      ```rust
      pub fn exterior_longwave_w(...) -> f64 {
          exterior_longwave_w_m2(...) * surface.area_m2
      }
      ```
- [ ] Move all formula logic into `exterior_longwave_w_m2()` only.

### MEDIUM: Extract `initialize_steady_state()` to submodule

- [ ] Create `crates/hares-envelope/src/thermal_solver/initialization.rs`.
- [ ] Move `initialize_steady_state()` there.
- [ ] Make it a standalone function or method on a new `StateSpaceInitializer`.
- [ ] Add unit tests for the reduced-system solve in isolation.

## Files to Touch

- `crates/hares-envelope/src/state_space.rs`: error handling, solve consolidation, stability
- `crates/hares-envelope/src/thermal_solver/mod.rs`: split resolve_internal, remove ZoneId(1)
- `crates/hares-envelope/src/thermal_solver/config.rs`: extract wiring, remove t_prev_c
- `crates/hares-envelope/src/thermal_solver/initialization.rs`: new submodule
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`: update config refs
- `crates/hares-envelope/src/longwave_radiation.rs`: eliminate duplication

## Measures of Success

- [ ] All existing tests pass with zero behavior change.
- [ ] `resolve_internal()` is under 60 lines.
- [ ] `StateSpaceModel` has no `expect()` calls in non-test code.
- [ ] `ThermalSolverConfig` has no mutable fields.
- [ ] No hardcoded `ZoneId(1)` in production code.

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-envelope` clean
