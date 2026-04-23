# Stability Eigenvalue Check Missing for from_discrete Path; Condition Number Not Checked

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope

## Problem

`StateSpaceModel` has two stability gaps:

### Gap 1: `from_discrete` performs no stability check

`from_continuous` (`hares-envelope/src/state_space.rs:283–285`) always computes a Gershgorin spectral radius bound for the discrete matrix `A_d`:

```rust
let gershgorin_bound = gershgorin_spectral_radius(&a_d);
let discrete_stable = gershgorin_bound < 1.0 + 1e-10;
let max_discrete_eigenvalue_magnitude = Some(gershgorin_bound);
```

`from_discrete` (`state_space.rs:161–183`) sets `max_discrete_eigenvalue_magnitude: None` and performs no stability check on the caller-supplied `a_d`. Any unstable discrete matrix provided via `from_discrete` is accepted silently. The RC network builder currently always uses `from_continuous`, but the gap remains for any future `from_discrete` caller and creates an ambiguous `None` return from the accessor.

Golub and Van Loan "Matrix Computations" 4th ed. §7.2 (Gershgorin Circle Theorem): every eigenvalue of a square matrix lies in at least one Gershgorin disk; the spectral radius is bounded by `max_i(|a_ii| + Σ_{j≠i} |a_ij|)`. For discrete stability, all eigenvalues must lie within the unit circle; the Gershgorin bound ≥ 1.0 is a conservative but reliable flag.

### Gap 2: Matrix condition number not checked during ZOH discretization

`RCOND_THRESHOLD: f64 = 1.0e-12` is defined at `state_space.rs:7` but is never used during `from_continuous`. A building with small thermal capacitances (e.g., thin plywood surface, τ ≈ 60 s) produces an RC network whose `A_c` condition number can span 7+ orders of magnitude relative to heavy-mass nodes. The matrix inversion in `B_d = A_c⁻¹ (A_d − I) B_c` accumulates significant floating-point error for ill-conditioned `A_c`. The `reciprocal_condition_estimate_1_norm` helper at `state_space.rs:819` is available but not called in `from_continuous`.

### Documentation mismatch

`max_discrete_eigenvalue_magnitude()` at `state_space.rs:202–204` returns `None` for all `from_discrete` paths with no documentation explaining that `None` means "not computed" rather than "stable". Callers cannot distinguish absent check from confirmed stability.

## Current Behavior

`hares-envelope/src/state_space.rs:181`: `from_discrete` sets `max_discrete_eigenvalue_magnitude: None` — no stability validation.
`hares-envelope/src/state_space.rs:7`: `RCOND_THRESHOLD = 1.0e-12` — defined but not used in `from_continuous`.
`hares-envelope/src/state_space.rs:202–204`: accessor is undocumented regarding the Gershgorin-bound vs. exact-spectral-radius vs. None distinction.

## Required Behavior

1. **`from_discrete` stability check**: Add a Gershgorin spectral radius check on the caller-supplied `a_d`. Store the result in `max_discrete_eigenvalue_magnitude` (making it `Some` for all construction paths). Emit `tracing::warn!` when the bound exceeds 1.0 + 1e-10 (matching the `from_continuous` threshold). Reference: Golub and Van Loan §7.2.

2. **Condition number check in `from_continuous`**: After assembling `A_c` and before ZOH discretization, call `reciprocal_condition_estimate_1_norm(&a_c)`. Emit `tracing::warn!` when the estimate is below `RCOND_THRESHOLD` (1e-12), naming the zone or surface producing the degenerate network. This catches thin-surface RC networks before they produce numerically unreliable `B_d`.

3. **Documentation**: Add a doc comment to the `max_discrete_eigenvalue_magnitude` field:
   "Gershgorin spectral radius upper bound for the discrete state matrix `A_d`. `Some(bound)` for all construction paths after this ticket is applied; `bound < 1.0` does not guarantee stability (Gershgorin is conservative) but `bound >= 1.0` is a strong instability signal. Emit `tracing::warn!` when `bound >= 1.0 + 1e-10`."

## Approach

1. In `from_discrete` (`state_space.rs:161–183`), after `a_d` is accepted, call `gershgorin_spectral_radius(&a_d)` (function already exists), store in `max_discrete_eigenvalue_magnitude: Some(...)`, and emit `tracing::warn!` if bound ≥ 1.0 + 1e-10.
2. In `from_continuous` (`state_space.rs`), before calling the ZOH expm routine, call `reciprocal_condition_estimate_1_norm(&a_c)` and emit `tracing::warn!` when result < `RCOND_THRESHOLD`.
3. Add a doc comment to the field and update the accessor doc.

## Definition of Done

- [ ] `from_discrete` computes Gershgorin bound on `a_d`, stores in `max_discrete_eigenvalue_magnitude`
- [ ] `from_discrete` emits `tracing::warn!` when Gershgorin bound ≥ 1.0 + 1e-10
- [ ] `from_continuous` calls `reciprocal_condition_estimate_1_norm` on `A_c` before ZOH and emits `tracing::warn!` when estimate < 1e-12
- [ ] `max_discrete_eigenvalue_magnitude` field has accurate doc comment; `None` is no longer a valid post-construction state
- [ ] `cargo test -p hares-envelope` passes
- [ ] Test: `from_discrete` with a known unstable matrix (e.g., diagonal entries > 1) emits warning and stores bound
- [ ] Test: `from_continuous` with a severely ill-conditioned `A_c` (rcond < 1e-12) emits condition warning

## Verification

```bash
cargo test -p hares-envelope
```

## References

- Golub, G. and Van Loan, C. "Matrix Computations" 4th ed. §7.2 — Gershgorin Circle Theorem: eigenvalues bounded by disk radii; applicable to discrete stability check
- Patankar, S. "Numerical Heat Transfer and Fluid Flow" §5.2 — coupling of large and small RC time constants; ZOH discretization accuracy
- `hares-envelope/src/state_space.rs:7` — `RCOND_THRESHOLD = 1.0e-12`
- `hares-envelope/src/state_space.rs:819` — `reciprocal_condition_estimate_1_norm` available but not called in `from_continuous`
