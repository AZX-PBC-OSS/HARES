# Semi-implicit infiltration coupling LU decomposition allocation per timestep
**Review ID**: envelope-08
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/state_space.rs`
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` — OCHRE's per-timestep heat balance: state update via forward matrix multiplication `A·x + B·u` with no matrix factorization in the hot loop. Infiltration is an explicit input term added to the B-matrix input vector, not a semi-implicit coupling requiring per-step matrix modification.
- `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.cc` — EnergyPlus zone heat balance: infiltration enters via the `ΣMCp` and `ΣMCpT` terms in a direct-substitution predictor-corrector, not via a per-step matrix factorization.

## Findings

### Finding 1: Unnecessary O(n^3) LU factorization of an identity matrix every timestep
**Severity**: high
**Description**: The production path always constructs `StateSpaceModel` via `from_continuous` (`solver_builder.rs:625`), which sets `M = I` (the n×n identity matrix, `state_space.rs:293,326`). The semi-implicit infiltration coupling adds diagonal perturbations `d_diag` to zone-air state rows (at most k ≈ 1–3 entries for typical residential models). The coupled system `(M + D)·x = b` therefore has the trivial closed-form solution:
- For non-coupled rows (n − k of them): `x[i] = b[i]`
- For coupled rows (k of them): `x[i] = b[i] / (1 + d_i)`

This can be solved in **O(n)** time with a single pass over the RHS vector. Instead, `build_coupled_lu` (`state_space.rs:465–478`) calls `nalgebra`'s `.lu()` on `std::mem::take(m_scratch)`, which performs a full **O(n³)** Gaussian elimination on every timestep. For a typical residential RC model with n = 50–100 state nodes and 8,760 hourly timesteps, this is 8,760 × (50³ to 100³) ≈ 1.1 × 10⁹ to 8.8 × 10⁹ unnecessary floating-point operations per simulation year.

**Code Location**:
- `state_space.rs:465–478` — `build_coupled_lu` method that calls `.lu()` on the identity matrix with diagonal perturbations
- `stepping.rs:447–449` — `prepare_inputs_inner` calls `build_coupled_lu`
- `stepping.rs:471–474` — `integrate_inner` calls `build_coupled_lu`
- `solver_builder.rs:625` — production model construction via `from_continuous` confirming M = I

**Root Cause**: The `build_coupled_lu` method is designed as a general-purpose coupled LU builder for arbitrary `M` matrices (including non-identity `from_discrete` models). However, the `from_discrete` path is **never used in production** (`grep` confirms zero production callers). Since M = I for all production models, the heavy `.lu()` call is always unnecessary. Engineering intent was to keep the code general for mixed construction paths, but the generality carries a runtime cost on the dominant path where it provides no value.

**Impact**: For n = 100, k = 2, dt = 3600 s: each LU factorization costs ~10⁶ flops vs. the ~200 flops needed for the identity+diagonal closed-form solve. Per-timestep overhead is ~5,000× the necessary cost. With 8,760 timesteps/year, the wasted work is ~8.8 × 10⁹ flops. At modern CPU throughput (~10–20 GFlops), this is ~0.4–0.9 seconds of pure numerical work, but the allocation and memory traffic costs dominate in practice. The `std::mem::take` + alloc for the L/U matrices and the `clone_from` of the full n×n M matrix add measurable GC and cache-pressure overhead, particularly for hourly sub-annual simulations with many dwelling variants.

### Finding 2: Double LU factorization per timestep in `resolve_internal` path
**Severity**: medium
**Description**: When `resolve_internal` (`stepping.rs:686–687`) calls both `prepare_inputs_inner` and `integrate_inner` in sequence, two full LU factorizations are performed in the same timestep:
1. `prepare_inputs_inner` (`stepping.rs:447–449`) calls `build_coupled_lu` and stores the result in `last_coupled_lu` for subsequent `solve_ideal_capacity_for_target` use.
2. `integrate_inner` (`stepping.rs:471–474`) calls `build_coupled_lu` again for the actual state step.

If the coupling terms are identical between the two calls (which they are, since they're rebuilt from the same weather and ports), the first LU factorization is identical to the second — it's wasted work.

**Code Location**: `stepping.rs:686–689` — `resolve_internal` calling both phases, triggering two `build_coupled_lu` calls per timestep.

**Root Cause**: `prepare_inputs_inner` exists to support a two-phase dispatch pattern (prepare first, then solve ideal capacity, then integrate). When the single-phase `resolve_internal` wrapper is used, it calls both phases but `integrate_inner` redundantly rebuilds and re-factorizes the same coupling matrix instead of reusing the `last_coupled_lu` already computed by `prepare_inputs_inner`.

**Impact**: Doubles the per-timestep LU cost for single-phase callers. In a typical HARES simulation, the `resolve()` entry point (`DomainSolver::resolve`, `mod.rs:970–984`) maps to `resolve_internal`, so this affects all non-dwelling callers.

### Finding 3: Per-step heap allocation via `clone_from` after `std::mem::take`
**Severity**: low
**Description**: `build_coupled_lu` (`state_space.rs:465–478`) uses `std::mem::take(m_scratch)` to move the n×n matrix data into `.lu()`, which leaves `m_scratch` as a 0×0 matrix. On the next call, `m_scratch.clone_from(&self.m_mat)` must allocate a new n×n heap buffer via nalgebra's internal `clone_from` (which assigns `*self = source.clone()`). For n = 100, this is ~80 KB allocated and freed each timestep. With 8,760 timesteps, that's ~700 MB of cumulative heap allocations. While not a correctness issue, this churn increases GC pressure and cache thrash.

The m_scratch pointer field itself is reused (struct field), but the backing heap buffer is re-allocated each step because `std::mem::take` destroys the allocation and `clone_from` creates a new one. A zero-allocation path would overwrite `m_scratch` in-place from `m_mat` without taking ownership, then factorize in-place without moving the backing storage.

**Code Location**: `state_space.rs:470` (`clone_from`) and `state_space.rs:477` (`std::mem::take`)

**Root Cause**: `std::mem::take` + `.lu()` is a clean ownership-transfer pattern to factorize without cloning, but the subsequent `clone_from` on a 0×0 matrix requires a fresh allocation rather than overwriting an existing allocation in-place.

**Impact**: Moderate cumulative allocation pressure across an annual simulation; not a bottleneck in isolation but compounds with Finding 1's unnecessary factorization work.

### Finding 4: `step_with_coupled_lu_into` does not use the model's pre-factored `m_lu`
**Severity**: low
**Description**: When there are no infiltration couplings (`coupling_buf` is empty), `integrate_inner` (`stepping.rs:486`) calls `self.model.step_into()`, which uses the pre-factored `self.model.m_lu` (identity LU, computed once at construction, `state_space.rs:321,340`). When couplings exist, `step_with_coupled_lu_into` uses a freshly-built LU from `build_coupled_lu` instead. This is architecturally correct (the coupling-modified matrix differs from M), but since M = I and the modification is diagonal-only, the `m_lu` solve could be inlined as a simple scalar division per coupled row, avoiding both the new LU and the non-coupled-row back-substitution.

**Code Location**: `stepping.rs:471–483` — coupled path vs. `stepping.rs:486` — uncoupled path

**Root Cause**: Same as Finding 1 — general-purpose LU code path executes on identity-matrix data.

## Summary
- Total findings: 4
- Critical: 0
- High: 1 (Finding 1)
- Medium: 1 (Finding 2)
- Low: 2 (Findings 3, 4)

## Recommendations
1. **Implement a closed-form solve for the identity M + diagonal D case (Finding 1).** Since M = I in all production models, replace `build_coupled_lu` + `step_with_coupled_lu_into` with a trivial O(n) solver:
   ```rust
   buf.gemv(1.0, &self.n_mat, x, 0.0);
   for &(idx, d, _) in couplings { buf[idx] -= d * x[idx]; }
   buf.gemv(1.0, &self.b_eff, u, 1.0);
   for &(idx, _, forcing) in couplings { buf[idx] += forcing; }
   // Closed-form solve: buf[i] /= 1.0 + d_i for coupled rows
   for &(idx, d_diag, _) in couplings { buf[idx] /= 1.0 + d_diag; }
   ```
   This is identical to `build_coupled_rhs` followed by diagonal scaling — zero allocations, O(n) time. Gate the full LU path behind a `#[cfg(test)]` or `from_discrete` flag if generality is needed for test models.

2. **Reuse `last_coupled_lu` in `integrate_inner` (Finding 2).** In `resolve_internal`, call `build_coupling` only once. Pass the already-computed `last_coupled_lu` from `prepare_inputs_inner` into `integrate_inner` instead of rebuilding it. For safety, verify that `coupling_buf` has not changed between the two phases (or just skip the rebuild when both phases share the same ports).

3. **Pre-allocate an in-place scratch buffer (Finding 3).** If the general LU path is retained, use an in-place factorization method (`nalgebra`'s `linalg::factorization::LU` can be constructed from a view) instead of `std::mem::take` + `.lu()`. Keep an `m_scratch` that is overwritten in-place rather than consumed-and-reallocated.

4. **Consider EnergyPlus-style explicit infiltration (vendor alignment).** OCHRE and EnergyPlus treat infiltration as an explicit input term added to the input vector rather than a matrix-modifying semi-implicit coupling. The explicit approach avoids per-timestep matrix modification entirely at the cost of a marginal stability reduction for very large infiltration rates. For typical residential infiltration (<< 10 ACH), the stability difference is negligible. The semi-implicit approach is theoretically more stable but, given M = I in production, provides no stability benefit over explicit treatment — it only adds computational overhead without improving solution quality.

## References / Citations
- EnergyPlus Engineering Reference (2024), "Basis for the Zone and Air System Integration" — infiltration enters the zone air heat balance as ΣMCp terms (explicit), not as matrix modifications
- OCHRE Envelope.py `update_infiltration()` (line 1281) — infiltration heat gains added to `inputs_init` vector (explicit)
- OCHRE StateSpaceModel.py `update_model()` (line 318) — `A·x + B·u` forward multiplication, no per-step factorization
- nalgebra 0.33 `LU::new()` / `.lu()` — performs full Gaussian elimination regardless of matrix structure
- ASHRAE HoF 2021 Ch.16 (Ventilation and Infiltration) — AIM-2 and ELA infiltration models
