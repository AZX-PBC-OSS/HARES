---
id: PARITY-019
title: "Quality: Eliminate hot-path allocations in thermal solver"
kind: fix
depends_on: [PARITY-001]
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - docs/architecture.md (Hot-Path Memory section)
  - feedback_code_quality.md
verification:
  - cargo build --workspace --features profiling
  - cargo test --workspace
---

## Prerequisites

THERMAL-005 (Crank-Nicolson implicit solver) and THERMAL-006a (semi-implicit infiltration) will have completed. THERMAL-005 implements a zero-allocation `step_into()` hot-loop. THERMAL-006a adds per-step diagonal perturbation of the CN matrices for implicit infiltration coupling — this introduces a per-step LU factorization that must be verified allocation-free. This ticket is a **final sweep** to catch any remaining allocations from both the THERMAL chain and PARITY tickets 005-018.

## Status: COMPLETE

Audit and fix of hot-path allocations in the thermal solver.

### Fixed
- Interior LWR surface temperature Vec → reusable `interior_surf_temps_buf` on ThermalSolver (clear+push instead of collect)
- Changed `apply_interior_longwave_inputs` from `&self` to `&mut self` to allow buffer reuse

### Accepted (unavoidable or negligible)
- `DomainUpdate` return type requires owned `Vec<(ZoneId, f64)>` for zone temperatures — consumed by caller, can't pre-allocate without changing trait
- `format_domain_update` latent pairs Vec — small (1-4 zones), bounded
- `compute_solar_distribution` absorbed Vec — small (4-8 surfaces per zone), bounded
- `lwr_by_zone` diagnostic Vec — small, bounded by zone count
- Interior LWR fallback path `Vec<InteriorSurface>` — only runs when `compute_scriptf()` not called at init (legacy path)

### Not applicable
- THERMAL-005 CN `step_into()` already zero-allocation
- THERMAL-006a LU factorization reuses `m_scratch` pre-allocated buffer
- `u_buf` swap pattern verified correct
- `latent_buf` HashMap take/clear pattern verified correct

## Background/Context

Post-THERMAL/PARITY sweep to minimize per-timestep heap allocations.

## Work to Do

- [ ] **Post-PARITY audit of thermal_solver resolve pipeline:**
  - Verify THERMAL-006a's per-step LU factorization reuses pre-allocated workspace (no alloc per step)
  - Verify PARITY-015 (interior LWR ScriptF) uses pre-allocated surface temperature buffers
  - Verify PARITY-018 (solar distribution) uses pre-allocated surface gain buffers
  - Verify any remaining `.collect()` calls from resolve_internal() are eliminated
  - Verify latent_buf, zone_temps_buf use take/swap pattern

- [ ] **dwelling run_timestep():**
  - Pre-allocate `sorted_indices: Vec<usize>` on Dwelling, reuse each step
  - Verify observation cloning is behind `#[cfg(feature = "observe")]` (should be)

- [ ] **Add allocation tracking test:**
  - Under `#[cfg(feature = "profiling")]`, assert zero allocations during a single timestep
  - Use `GlobalAlloc` wrapper to count allocations in test
  - Fail test if any allocation occurs between `ports.zero()` calls

### Quality Requirements

- [ ] Zero heap allocations per timestep in release mode (excluding observation feature)
- [ ] All pre-allocated buffers use the `std::mem::replace` / `std::mem::take` swap pattern
- [ ] Buffer sizes validated at init, not checked per-step

## Measures of Success

- [ ] Allocation tracking test passes with 0 allocations per timestep
- [ ] No regression in simulation results
- [ ] Memory usage is constant regardless of simulation duration (no leaks)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --workspace --features profiling` passes
