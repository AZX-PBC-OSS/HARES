# Per-Step Heap Allocation in Radiant Gain Weight Computation

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope

## Problem

`distribute_radiant_lwr_surfaces` and `distribute_radiant_solar_surfaces` in `hares-envelope/src/thermal_solver/ports.rs` each allocate a `Vec<f64>` weights buffer on every call. Both functions are reached every timestep via `build_input_vector` → `apply_port_radiant_inputs`, called from both `prepare_inputs_inner` (Step 1d) and `integrate_inner` (Step 4) — two heap allocations per timestep in the hot loop.

Project policy `feedback_hot_loop_minimal.md`: no sorts or allocations in the hot loop; preprocess at init, pre-allocate buffers.

## Current Behavior

`hares-envelope/src/thermal_solver/ports.rs:87`:
```rust
let mut weights = Vec::with_capacity(surfaces.len());
```
inside `distribute_radiant_lwr_surfaces` — allocated per call.

`hares-envelope/src/thermal_solver/ports.rs:133`:
```rust
let mut weights = Vec::with_capacity(surfaces.len());
```
inside `distribute_radiant_solar_surfaces` — allocated per call.

`ThermalSolver` already owns pre-allocated fields (`solar_absorbed_buf`, `lwr_net_flux_buf`, etc.) added precisely to avoid this pattern. The `radiant_weights_buf` field is absent.

## Required Behavior

Zero per-timestep heap allocations in `distribute_radiant_lwr_surfaces` and `distribute_radiant_solar_surfaces`. Follow the existing `compute_solar_distribution_into` pattern: caller owns a buffer, callee receives `buf: &mut Vec<f64>` and calls `buf.clear(); buf.resize(n, 0.0);` at entry.

Reference: project policy `feedback_hot_loop_minimal.md`; existing pattern at `solar_absorbed_buf` in `hares-envelope/src/thermal_solver/mod.rs`.

## Approach

1. Add `radiant_weights_buf: Vec<f64>` to `ThermalSolver` alongside `solar_absorbed_buf`, `lwr_net_flux_buf`, etc.
2. Pre-allocate `radiant_weights_buf` to `max_interior_surfaces` (or the largest surface count across all zones) at `ThermalSolver` construction.
3. Change the signature of `distribute_radiant_lwr_surfaces` and `distribute_radiant_solar_surfaces` to accept `buf: &mut Vec<f64>` in place of the internal `Vec::with_capacity` call.
4. At the top of each function, call `buf.clear(); buf.resize(n, 0.0);` — matching the `compute_solar_distribution_into` pattern.
5. At all call sites in `build_input_vector` and `apply_port_radiant_inputs`, pass `&mut self.radiant_weights_buf`.

## Definition of Done

- [ ] `ThermalSolver` has `radiant_weights_buf: Vec<f64>` pre-allocated at construction
- [ ] `distribute_radiant_lwr_surfaces` accepts `buf: &mut Vec<f64>`; no `Vec::with_capacity` inside the function body
- [ ] `distribute_radiant_solar_surfaces` accepts `buf: &mut Vec<f64>`; no `Vec::with_capacity` inside the function body
- [ ] `cargo test -p hares-envelope` passes; no regression in radiant distribution tests
- [ ] Grep confirms zero `Vec::with_capacity` calls remain inside either function

## Verification

```bash
cargo test -p hares-envelope
```

## References

- Project policy `feedback_hot_loop_minimal.md` — no sorts/allocs in hot loop; preprocess at init
- Existing pattern: `solar_absorbed_buf` in `hares-envelope/src/thermal_solver/mod.rs`
- `hares-envelope/src/thermal_solver/ports.rs:87` and `:133` — current allocation sites
