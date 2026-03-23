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

THERMAL-005 (Crank-Nicolson implicit solver) will have completed. THERMAL-005 implements a zero-allocation `step_into()` hot-loop that supersedes most of the thermal solver allocation concerns. This ticket becomes a **final sweep** to catch any remaining allocations introduced by PARITY tickets 005-018.

## Background/Context

After THERMAL-005, the core state-space stepping is zero-allocation. However, other PARITY tickets (015 interior LWR, 018 solar distribution, 009 ventilation) may introduce new allocations in the resolve pipeline. This ticket audits the full hot path after all physics features are in place and eliminates any remaining per-timestep allocations.

## Work to Do

- [ ] **Post-PARITY audit of thermal_solver resolve pipeline:**
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
