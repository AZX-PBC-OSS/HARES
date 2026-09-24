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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Gap 1 confirmed** — `from_discrete` at `state_space.rs:161–183` (current) sets `max_discrete_eigenvalue_magnitude: None` (line 181) and performs no Gershgorin check. `gershgorin_spectral_radius` is defined at line 1043 but is never called inside `from_discrete`.
- [x] **Gap 2 partially confirmed** — `RCOND_THRESHOLD = 1.0e-12` is defined at line 7. `reciprocal_condition_estimate_1_norm` IS called in `discretize_auto` (lines 839–843), but only to branch between `discretize_zoh` and `van_loan_discretize` — no `tracing::warn!` is emitted. The ticket's description that "RCOND is never used in `from_continuous`" is imprecise: it is used indirectly via `discretize_auto`, but the warning requirement is not met.
- [x] **Documentation gap confirmed** — `max_discrete_eigenvalue_magnitude()` accessor at line 200–204 has doc comment: "None for large matrices (n > 20) where eigenvalue decomposition is skipped." There is no n > 20 size guard anywhere in the current code. The comment is stale and incorrect — `None` reflects the from_discrete construction path, not a size cut-off. This is a real documentation error.
- [x] **OCHRE cross-check**: `vendors/OCHRE/ochre/Models/StateSpaceModel.py` has no Gershgorin check anywhere. Its `to_discrete` method (lines 247–266) calls `scipy.linalg.expm` and `numpy.linalg.inv` but never checks stability of the resulting discrete matrix. HARES diverges intentionally from OCHRE by adding stability checks in `from_continuous`; the ticket proposes extending this discipline to `from_discrete`. Divergence is intentional.
- [x] **EnergyPlus cross-check**: N/A — the ticket concerns internal numerical hygiene (stability checking, conditioning), not an EnergyPlus physics formula.

### Web-Verified Citations

**Citation 1**: Golub, G. and Van Loan, C. "Matrix Computations" 4th ed. §7.2 — Gershgorin Circle Theorem
- **Source found**: SAS DO Loop blog (https://blogs.sas.com/content/iml/2019/05/22/gershgorin-discs-location-eigenvalues.html) citing Golub & Van Loan; Cornell University archived Table of Contents PDFs for editions 1 and 2; general web sources confirming 4th ed. structure.
- **Confirmed content**: The Gershgorin Circle Theorem appears in Golub & Van Loan 4th ed. at **page 357**, under Chapter 7 "Unsymmetric Eigenvalue Problems". Multiple independent sources confirm Chapter 7's sections are: 7.1 Properties and Decompositions, 7.2 Perturbation Theory, 7.3 Power Iterations, 7.4 The Hessenberg and Real Schur Forms, 7.5 The Practical QR Algorithm, 7.6 Invariant Subspace Computations, 7.7 The Generalized Eigenvalue Problem, 7.8 Hamiltonian and Product Eigenvalue Problems, 7.9 Pseudospectra. The Gershgorin theorem at page 357 falls in **§7.1 "Properties and Decompositions"** — NOT §7.2. Section 7.2 covers perturbation theory.
- **Quoted passage**: "The Gershgorin Disc Theorem appears in Golub and van Loan (p. 357, 4th Ed; p. 320, 3rd Ed), where it is called the Gershgorin Circle Theorem." (SAS DO Loop blog, citing the book directly.)
- **Verdict**: **Incorrect section number**. The theorem is in §7.1 (Properties and Decompositions), not §7.2 (Perturbation Theory). The conceptual content (eigenvalues bounded by disk radii; applicable to stability) is correct, but the section citation is wrong.

**Citation 2**: Patankar, S. "Numerical Heat Transfer and Fluid Flow" §5.2 — coupling of large and small RC time constants; ZOH discretization accuracy
- **Source found**: Multiple sources confirming Patankar (1980) chapter structure: Routledge publisher listing, Google Books, ADS abstract, general web references.
- **Confirmed content**: Chapter 5 of Patankar (1980) is titled "Convection and Diffusion". Its sections are: 5.1 The Task, 5.2 Steady One-Dimensional Convection and Diffusion, 5.3 Discretization Equation for Two Dimensions, 5.4 Discretization Equation for Three Dimensions, 5.5 A One-Way Space Coordinate, 5.6 False Diffusion, 5.7 Closure. This chapter covers finite-difference discretization of the convection-diffusion PDE for steady flow — it is about spatial discretization of PDEs, not RC circuit time constants, stiffness of ODE networks, or Zero-Order Hold (ZOH) temporal discretization.
- **Quoted passage**: Chapter 5 covers "Steady One-dimensional Convection and Diffusion" (§5.2), not RC time constants or ZOH discretization.
- **Verdict**: **Incorrect citation**. Patankar §5.2 is about convection-diffusion spatial discretization — it does not discuss RC time constants, multi-scale stiffness of lumped-capacitance networks, or ZOH temporal discretization. This reference does not support the claim made in the ticket. A more appropriate reference for ZOH accuracy with stiff RC networks would be a numerical ODE textbook (e.g., Hairer & Wanner "Solving ODEs II: Stiff and Differential-Algebraic Problems") or a matrix exponential reference.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: Both core engineering gaps described in the ticket are real and present in the current code. Gap 1 (no Gershgorin check in `from_discrete`) is exactly as described: `from_discrete` at line 181 sets `max_discrete_eigenvalue_magnitude: None` without ever calling `gershgorin_spectral_radius`. The regression test `from_discrete_unstable_matrix_eigenvalue_magnitude_is_none` confirms the accessor returns `None` for a clearly-unstable diagonal matrix. Gap 3 (stale doc comment referencing a non-existent n > 20 size guard) is also confirmed. Gap 2 is real but the description is imprecise: `reciprocal_condition_estimate_1_norm` IS already called in `discretize_auto` (not absent from `from_continuous`), but only to choose the discretization path — the `tracing::warn!` emission is indeed missing. The two citations are both incorrect: §7.2 is Perturbation Theory (Gershgorin is in §7.1), and Patankar §5.2 covers convection-diffusion, not RC stiffness or ZOH. These citation errors do not affect the validity of the underlying code issue but would mislead an implementor looking them up.

### Proposed Fix Summary

1. **Gap 1** (`from_discrete` stability check): Inside `from_discrete` at line 172, after `validate_state_space_dimensions`, call `gershgorin_spectral_radius(&a_d)` (already exists at line 1043), store the result in `max_discrete_eigenvalue_magnitude: Some(bound)`, and emit `tracing::warn!` when `bound >= 1.0 + 1e-10`. No new helpers are needed.
2. **Gap 2** (condition warning in `from_continuous`): In `discretize_auto` (or at the call site in `from_continuous`), after computing `rcond`, emit `tracing::warn!(rcond, "A_c is severely ill-conditioned; B_d may be numerically unreliable")` when `rcond < RCOND_THRESHOLD`. No structural change needed — just add the warn branch.
3. **Doc fix**: Replace the accessor doc comment at line 200–201 with the text proposed in the ticket's Required Behavior §3, removing the false "n > 20" reference.

### Test Written

- **File**: `crates/hares-envelope/src/state_space.rs` (within `#[cfg(test)] mod tests`)
- **Test 1** (`from_discrete_unstable_matrix_eigenvalue_magnitude_is_none`): Constructs a `from_discrete` model with a clearly-unstable 2×2 diagonal matrix (entry 1.5). Asserts that `max_discrete_eigenvalue_magnitude()` returns `None`, documenting the gap. After the fix this assertion must be inverted (or the test rewritten to assert `Some(bound)` with `bound >= 1.5`).
- **Test 2** (`from_continuous_ill_conditioned_a_c_succeeds_without_panic`): Constructs a severely ill-conditioned `A_c` (condition number ~ 1e13) and asserts that `from_continuous` succeeds via the Van Loan fallback. Guards against regression where the ill-conditioned path panics or errors. After the fix a tracing subscriber could be added to assert the warn is emitted; as-is, this test documents the path succeeds.
