# Gershgorin stability check vs full eigensystem for large matrices
**Review ID**: envelope-06
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
crates/hares-envelope/src/state_space.rs

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` — ad-hoc heavy ball momentum for nonlinear radiation convergence; no eigenvalue/Gershgorin-based stability checks.
- `vendors/EnergyPlus/src/EnergyPlus/Construction.cc` — Fourier number-based node sizing (lines 489–517), time-step adjustment (lines 572–615), matrix row norm scaling for matrix exponential (lines 1187–1214), CTF term count limit (`MaxCTFTerms=19`, `Construction.hh:68`). Zero instances of eigenvalue/Gershgorin/schur stability checks in any EnergyPlus source file.

## Findings

### Finding 1: [Severity: medium] False-positive instability rejection under Gershgorin-only path for non-singular A_c

**Description**: The `from_continuous` constructor uses Gershgorin circle bounds (O(n²)) as the sole stability gate, rejecting the model with `StateSpaceError::UnstableSystem` when the bound exceeds `1.0 + 1e-10` (lines 300–306). The Gershgorin theorem provides a sufficient but not necessary condition: `bound ≥ 1.0 + ε` signals true instability, but `bound < 1.0 + ε` does not guarantee stability. The code acknowledges this conservatism at line 119–122. However, for matrices where Gershgorin overestimates the spectral radius due to large off-diagonal terms, a physically stable multi-zone RC network will be incorrectly rejected with no fallback to a full eigenvalue check.

**Code Location**: `crates/hares-envelope/src/state_space.rs:295–312`

```rust
// lines 295–306
let continuous_stable = gershgorin_continuous_stable(a_c);
let gershgorin_bound = gershgorin_spectral_radius(&a_d);
let discrete_stable = gershgorin_bound < 1.0 + 1e-10;

if !continuous_stable || !discrete_stable {
    let a_c_singular = is_singular(a_c);
    if !(a_c_singular && gershgorin_bound <= 1.0 + 1e-10) {
        return Err(StateSpaceError::UnstableSystem(StabilityResult { ... }));
    }
}
```

**Root Cause**: The construction path has no tiered fallback. Only a singular A_c with Gershgorin bound inside the tolerance (line 304) is exempted. A non-singular, well-conditioned A_c whose Gershgorin bound overestimates above `1.0 + 1e-10` — while all true eigenvalues are within the unit circle — gets rejected. This is a false positive.

The Gershgorin spectral radius (line 1066–1075) computes the row-wise infinity norm:

```
bound = max_i ( |a_ii| + Σ_{j≠i} |a_ij| )
```

For an RC thermal network after ZOH discretization, the rows of A_d are near-row-stochastic: diagonal entries are `e^{-Σ(1/(R_ij*C_i))*dt}` (decay terms) and off-diagonal terms represent inter-node coupling. With strong coupling (large off-diagonals), the bound can exceed 1 even when all true eigenvalues satisfy `|λ| < 1`.

**Impact**: Large multi-zone models with strong inter-zone thermal coupling may fail to construct, returning `StateSpaceError::UnstableSystem` despite being physically stable. This blocks the user's simulation with no workaround except reducing the model or changing discretization. The existing `eigenvalue_check` function (lines 1021–1059) and `verify_stability` method (lines 358–375) both perform full eigenvalue decompositions but are not integrated into the construction path.

### Finding 2: [Severity: low] Asymmetric handling between `from_discrete` and `from_continuous`

**Description**: `from_discrete` (line 179) emits only a `tracing::warn!` when the Gershgorin bound exceeds unity, allowing construction to proceed with an unstable model and relying on the caller to inspect. `from_continuous` (line 306) returns a hard `Err(UnstableSystem)`, blocking construction entirely. This asymmetry means the same physical system discretized differently gets different treatment at construction time.

**Code Location**: Compare `crates/hares-envelope/src/state_space.rs:178–184` vs `295–312`.

**Root Cause**: The two constructors use the same bound computation but different error policies. This is likely intentional for defense-in-depth (continuous path has more ways to go wrong), but it creates an inconsistent developer experience.

**Impact**: Users who pre-discretize externally and use `from_discrete` receive a warning for an unstable system; users who go through `from_continuous` receive an error. The warning path allows unstable systems through silently if the warning is not monitored.

### Finding 3: [Severity: low] Near-unity warning uses Gershgorin bound, not actual eigenvalue magnitude

**Description**: Line 314–319 compares the Gershgorin spectral radius bound directly against `NEAR_UNITY_EIGENVALUE_THRESHOLD` (0.99):

```rust
if gershgorin_bound > NEAR_UNITY_EIGENVALUE_THRESHOLD {
    tracing::warn!(
        gershgorin_bound,
        "Gershgorin discrete spectral bound near unity; convergence may be slow"
    );
}
```

Since Gershgorin is an upper bound, a system with all true eigenvalues at magnitude 0.5 could trigger this warning if off-diagonal coupling terms are large enough to push the Gershgorin bound above 0.99. This produces a false-positive "slow convergence" warning that may confuse users.

**Code Location**: `crates/hares-envelope/src/state_space.rs:314–319`

**Root Cause**: Mixing of Gershgorin upper-bound semantics with eigenvalue-specific thresholds. The `eigenvalue_check` function (lines 1031–1046) correctly computes true eigenvalues for its near-unity check; the construction path does not.

**Impact**: Benign — only a log warning — but may mislead users into investigating convergence issues that don't exist.

### Finding 4: [Severity: low] Full eigenvalue path exists but is not used defensively

**Description**: The codebase already has three independent full-eigenvalue pathways:

- `eigenvalue_check(a_c, a_d)` (line 1021) — standalone function computing both continuous and discrete eigenvalues
- `verify_stability()` (line 358) — method computing discrete eigenvalues via `A_d_equiv.complex_eigenvalues()`
- Tests at `state_space_tests.rs:191` and `state_space_tests.rs:218` that exercise these paths

None of these are invoked in the `from_continuous` hot path. The comment at lines 296–298 explicitly cites nalgebra's Schur QR having "unlimited iterations" and potential to "stall indefinitely" as the rationale for avoiding full eigenvalues at construction time.

**Code Location**: `crates/hares-envelope/src/state_space.rs:296–298`

```rust
// Use Gershgorin bounds (O(n)) instead of full eigenvalue decomposition.
// nalgebra's Schur QR (used by complex_eigenvalues) has unlimited iterations
// and can stall indefinitely on stiff heavyweight construction matrices.
```

**Root Cause**: Legitimate concern about nalgebra's Schur QR implementation. However, the concern is about pathological cases (matrices that cause the QR algorithm to stall). For physically-derived RC thermal networks, the A_d matrix is real, non-negative away from the diagonal, and well-behaved — Schur QR converges in O(n³) with few iterations. The risk of stalling is extremely low for these matrices.

### Finding 5: [Severity: low] Neither vendor reference implementation performs eigenvalue/Gershgorin checks

**Description**: OCHRE (`StateSpaceModel.py`) relies entirely on the physics guarantee that positive R and C values produce a stable A-matrix — it rejects models with non-positive R/C parameters at construction (`RCModel.py:110–114`) and never computes eigenvalues for stability. EnergyPlus (`Construction.cc`) uses Fourier number-based node spacing, CTF term count limits (`MaxCTFTerms=19`), and matrix row norm scaling for the matrix exponential — no eigenvalue or Gershgorin analysis.

**Impact**: The HARES approach is more rigorous than either vendor, which is a positive. However, the absence of any eigenvalue-based approach in both mature reference implementations suggests that the Gershgorin-overestimation false-positive problem may be rare in practice for physically-constructed RC networks. The conservative `Err(UnstableSystem)` rejection path may be unnecessary for the majority of realistic envelope models.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 1
- Low: 4

## Recommendations

1. **Tiered stability check in `from_continuous`**: After Gershgorin flags potential instability, attempt a full eigenvalue decomposition on A_d before rejecting. Use the existing `eigenvalue_check` infrastructure. If nalgebra's Schur QR stalls (which is unlikely for physically-derived RC matrices), fall back to the Gershgorin-based rejection. This eliminates false-positive rejections for large multi-zone models while keeping the conservative gate.

2. **Time-bounded eigenvalue fallback**: Rather than worrying about "unlimited iterations" in Schur QR, consider implementing a secondary check using the column-wise Gershgorin bound (mirror of the row-wise bound at line 1066–1075). The minimum of row-wise and column-wise Gershgorin bounds is a tighter overestimate. In practice, for physically-derived RC networks, the Gershgorin bound is already tight (center + radius ≈ 0 for continuous A_c), so this may be sufficient without a full eigenvalue decomposition.

3. **Replace near-unity Gershgorin threshold with actual eigenvalue check**: At line 314–319, instead of comparing the Gershgorin bound against `NEAR_UNITY_EIGENVALUE_THRESHOLD`, compute the maximum true eigenvalue magnitude using `complex_eigenvalues()` if the Gershgorin bound exceeds the threshold. This eliminates false-positive "slow convergence" warnings.

4. **Unify construction-path behavior**: Either downgrade `from_continuous` to warn (matching `from_discrete`), or upgrade `from_discrete` to reject on Gershgorin ≥ 1.0. Consistency improves predictability.

5. **Consider adopting OCHRE's approach of physics-guaranteed stability**: RC thermal networks with all-positive R and C are provably stable — the A_c matrix satisfies `a_ii = -Σ_{j≠i} a_ij` with negative diagonal, placing all Gershgorin discs in the closed left half-plane. If HARES validates that all R and C are positive at network construction time, the stability check at discretization time becomes redundant. This is what OCHRE does at `RCModel.py:110–114`.

## References / Citations

- Gershgorin circle theorem: Horn & Johnson, *Matrix Analysis* (2nd ed.), Theorem 6.1.1
- ZOH discretization: Franklin, Powell, & Workman, *Digital Control of Dynamic Systems* (3rd ed.), §4.3
- OCHRE StateSpaceModel.py balanced truncation: Gugercin & Antoulas, "A Survey of Model Reduction by Balanced Truncation," *Automatica* (2004)
- EnergyPlus CTF state-space method: Seem, J.E., "Modeling of Heat Transfer in Buildings," Ph.D. thesis, University of Wisconsin–Madison (1987)
- nalgebra Schur decomposition: `nalgebra::Schur` uses the Francis double-shift QR algorithm with indefinite convergence guarantee for non-normal matrices
