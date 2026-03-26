---
id: ENVELOPE-005
title: Wire boundary diagnostics for all boundary types
kind: fix
depends_on: [ENVELOPE-001]
files_to_touch:
  - crates/hares-core/src/dwelling/solver_builder.rs
  - crates/hares-envelope/src/thermal_solver/config.rs
  - crates/hares-envelope/src/thermal_solver/stepping.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
references:
  - crates/hares-envelope/src/boundary_rc.rs (BoundaryDiagnostic, BoundaryDiagnosticInfo)
  - vendors/OCHRE/ochre/Models/Envelope.py (BoundarySurface heat gain calculation)
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core --features observe --test conditioned_oracle -- --nocapture
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

The conditioned oracle shows zeros for roof, window, and internal mass conduction
diagnostics. This is because `BoundaryDiagnosticInfo` is only populated in
`build_solver_boundaries()` for boundaries that have `inner_wiring` — i.e., boundaries
with RC interior nodes (solver_builder.rs line ~602: `if let Some(ref iw) = sb.inner_wiring`).

Boundaries without `inner_wiring` (fallback-R boundaries, windows with no RC nodes,
some roof configurations) still transfer heat through the state-space A matrix and
B_ext, but their conduction isn't tracked in the per-component diagnostic breakdown.

**Critical constraint**: The diagnostic computation must be **non-injecting** (read-only
from state vector and environment). It must NOT add to the B matrix or input vector.
The heat is already flowing through the state-space model — we're only computing a
diagnostic decomposition for observability.

## Work to Do

- [ ] Audit all boundary types to determine which ones lack `inner_wiring` and why:
  - Fallback-R boundaries (no material layers, only aggregate R-value)
  - Windows (simple U-factor, no RC thermal mass)
  - Some roof configurations
- [ ] Extend `BoundaryDiagnosticInfo` (or create a parallel `FallbackBoundaryDiag` variant)
  to handle boundaries without `inner_state_index`:
  - For fallback-R boundaries: `Q = (T_zone - T_driving) * UA` using zone air temp
    from output vector and driving temp from environment (outdoor or ground)
  - For windows: `Q = (T_zone - T_outdoor) * U_window * area`
- [ ] The implementation in `stepping.rs` post-solve loop needs access to:
  - Zone air temperature (already available from `y_next[zone_output_idx]`)
  - Driving temperature: outdoor temp or ground temp from environment. This may need
    to be cached from the input vector or passed into `resolve_internal()`. The
    `EnvironmentState` is not currently available in `stepping.rs` — either pass it
    through or cache the needed temperatures in the `ThermalSolver` struct.
- [ ] Add computed conduction to `EnvelopeComponentGains` fields:
  - `window_heat_gain_w`: U-factor conduction
  - `roof_heat_gain_w`: fallback-R roof conduction
  - `internal_mass_heat_gain_w`: if internal mass boundaries lack inner_wiring
- [ ] Ensure NO double-counting: boundaries WITH `inner_wiring` continue using the
  existing surface-temperature-based calculation in the `for diag in boundary_diagnostics`
  loop. New entries are for boundaries WITHOUT inner_wiring only.

## Files to Touch

- `crates/hares-core/src/dwelling/solver_builder.rs`: Populate diagnostic info for all boundaries
- `crates/hares-envelope/src/thermal_solver/config.rs`: Extend `BoundaryDiagnosticInfo` or add variant for fallback boundaries
- `crates/hares-envelope/src/thermal_solver/stepping.rs`: Compute fallback conduction in post-solve diagnostic loop
- `crates/hares-envelope/src/thermal_solver/mod.rs`: Cache driving temperatures if needed

## Measures of Success

- [ ] `boundary_diagnostics_count()` matches total number of conditioned-zone boundaries
- [ ] Conditioned oracle shows non-zero values for roof, window, and internal mass heat gains
- [ ] Window heat gain (conduction) matches OCHRE within 20%
- [ ] Roof heat gain matches OCHRE within 20%
- [ ] No energy balance violations (sum of component gains unchanged from current)
- [ ] Diagnostic computation is provably non-injecting (read-only)

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core --features observe --test conditioned_oracle -- conditioned_ideal_winter --nocapture` — roof/window/mass gains non-zero
- [ ] `cargo clippy --all-targets -- -D warnings` passes
