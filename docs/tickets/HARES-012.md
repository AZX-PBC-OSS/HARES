---
id: HARES-012
title: "hares-envelope — State-Space Solver (CRITICAL PATH)"
kind: implement
depends_on: [HARES-002]
files_to_touch:
  - crates/hares-envelope/src/state_space.rs
  - crates/hares-envelope/src/lib.rs
references:
  - vendors/OCHRE/ochre/Models/StateSpaceModel.py
  - docs/architecture/01-sim-core-and-solver.md
verification:
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

The RC envelope solver is the computational heart of HARES. Every thermal simulation timestep resolves to `x[k+1] = A_d · x[k] + B_d · u[k]`, where `A_d` and `B_d` are precomputed once at init via zero-order hold (ZOH) discretization of the continuous-time RC matrices. This is a direct Rust reimplementation of OCHRE's `StateSpaceModel` (vendors/OCHRE/ochre/Models/StateSpaceModel.py, 367 lines). Blocking for HARES-013 (RC construction) and HARES-014 (ThermalSolver).

## Work to Do

- [ ] Define `StateSpaceModel` struct with fields: `a_d: DMatrix<f64>`, `b_d: DMatrix<f64>`, `c: DMatrix<f64>`, `d: DMatrix<f64>`
- [ ] Populate `C` and `D` during construction from the RC network's node-to-zone mapping: `C` maps state variables (node temperatures) to observable outputs (zone temperatures); `D` maps inputs directly to outputs. Both must be computed at construction time and stored on the struct.
- [ ] Implement `StateSpaceModel::output(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64>` — returns `C * x + D * u`
- [ ] Implement `discretize_zoh(a_c: &DMatrix<f64>, b_c: &DMatrix<f64>, dt: f64) -> Result<(DMatrix<f64>, DMatrix<f64>)>` — computes `A_d = expm(A_c * dt)`, then `B_d = A_c⁻¹ * (A_d - I) * B_c` via LU solve
- [ ] Implement `matrix_exp(m: &DMatrix<f64>) -> DMatrix<f64>` using Padé approximant scaling-and-squaring (match scipy's `expm` output to within f64 rounding)
- [ ] Implement `van_loan_discretize(a_c: &DMatrix<f64>, b_c: &DMatrix<f64>, dt: f64) -> (DMatrix<f64>, DMatrix<f64>)` — augmented matrix fallback for singular `A_c`. Build the block matrix `[[A_c, B_c], [0, 0]] * dt`, call `matrix_exp`, and extract `A_d` (top-left block) and `B_d` (top-right block). Use OCHRE `StateSpaceModel.py:261-264` as the authoritative reference for this construction; do not restate or re-derive the formula here. The Van Loan reference is in `vendors/OCHRE/ochre/utils/StateSpaceModel.py:261-264` — confirm the vendored tree is present before implementation.
- [ ] Implement `StateSpaceModel::step(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64>` — returns `A_d * x + B_d * u`
- [ ] Implement `StateSpaceModel::solve_for_input(&self, x: &DVector<f64>, u: &DVector<f64>, y_target: f64, input_index: usize) -> Result<f64>` — for a single unknown scalar input, rearrange `y_target = C * (A_d * x + B_d * u) + D * u` analytically: let `x_next_fixed = A_d * x + B_d_without_col * u_without_input`, then solve `y_target = C * (x_next_fixed + B_d_col * u_i) + D_col * u_i` for `u_i`. This is direct scalar algebra (one division), NOT a matrix inversion; panics or `Err` if the effective gain coefficient is zero.
- [ ] Define `StabilityResult` struct with fields: `continuous_stable: bool`, `discrete_stable: bool`, `near_unity_eigenvalues: Vec<(usize, nalgebra::Complex<f64>)>` (index + full complex eigenvalue, for eigenvalues with magnitude > 0.99)
- [ ] Implement `eigenvalue_check(a_c: &DMatrix<f64>, a_d: &DMatrix<f64>) -> Result<StabilityResult, StabilityResult>` — all continuous eigenvalues must have negative real parts; all discrete eigenvalues must have magnitude < 1.0; warn via `tracing::warn!` if any discrete eigenvalue magnitude > 0.99 (near-unity, risk of slow convergence). Returns `Ok(StabilityResult)` when stable (caller may still want to inspect near-unity eigenvalues) or `Err(StabilityResult)` when unstable — not `assert!` — so the caller decides whether to abort or degrade gracefully; document this choice in `docs/PHYSICS_DECISIONS.md` (see note below). Use `nalgebra`'s `DMatrix::complex_eigenvalues()` for the decomposition (correct for non-symmetric real matrices; do not use symmetric-only routines).
- [ ] Auto-select between `discretize_zoh` and `van_loan_discretize` at construction time: attempt LU decomposition of `A_c`; use Van Loan if singular (rcond below threshold)
- [ ] Use `nalgebra` `DMatrix`/`DVector` throughout; no `ndarray`
- [ ] Add entry to `docs/PHYSICS_DECISIONS.md`: `eigenvalue_check` returns `Result<StabilityResult, StabilityResult>` rather than `assert!`-ing (as shown in arch doc `01-sim-core-and-solver.md`). `Ok` carries the result when all stability criteria pass; `Err` carries it when any criterion fails. Both variants contain the full `StabilityResult` so callers can always inspect near-unity eigenvalues. Rationale: callers (e.g. test harnesses, config validation tools) need a recoverable error path; a panic from a construction-time assertion makes it impossible to report which network parameter caused instability. This is a deliberate improvement over the arch doc's illustrative `assert!`. (Note: standardized path is `docs/PHYSICS_DECISIONS.md`, not `docs/architecture/PHYSICS_DECISIONS.md`.)

## Files to Touch

- `crates/hares-envelope/src/state_space.rs`: new file — `StateSpaceModel`, `StabilityResult`, `matrix_exp`, `discretize_zoh`, `van_loan_discretize`, `eigenvalue_check`
- `crates/hares-envelope/src/lib.rs`: add `pub mod state_space` and re-export public types

## Measures of Success

- [ ] 1R1C golden case: `R=1.0`, `C=1000.0`, `dt=60s`, 100-step simulation from `T_init=20°C`, `T_ext=0°C` — final temperature matches analytical `T(t) = T_ext + (T_init - T_ext) * exp(-t / (R*C))` to within 0.01°C
- [ ] 3R2C network produces `A_d`, `B_d`, `C`, and `D` matching known scipy `cont2discrete(method='zoh')` output and OCHRE golden values to within 1e-10 (provide all four matrices as constants in the test; validate `C` maps node temperatures to zone temperatures and `D` has the correct input-to-output coefficients)
- [ ] Singular `A_c` fallback tested with a degenerate RC network where one node has zero capacitance, forcing the Van Loan path; result matches expected discretization
  - Note: The Van Loan path requires a relaxed continuous stability check. A singular `A_c` has zero eigenvalues which fail the strict `re < 0.0` check. The implementation correctly accepts marginally stable discrete eigenvalues (magnitude <= 1.0) when `A_c` is singular.
- [ ] Unstable network (`A_c` with a positive eigenvalue) causes `eigenvalue_check` to return `Err(StabilityResult)` with `continuous_stable: false`
- Note: ThermalSolver validates stability via `StateSpaceModel::from_continuous` at construction time — no separate eigenvalue check is needed at ThermalSolver construction.
- [ ] `solve_for_input` recovers the correct HVAC capacity for a known `A_d`, `B_d`, `C`, `D`, state vector, and input vector — round-trip error < 1e-9
- [ ] `solve_for_input` test includes a case where `D_col` is non-zero, verifying correct gain coefficient
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes with no suppressed warnings

## Performance Notes
- **P2 — Reusable solver buffers**: `DomainSolver` implementations (ThermalSolver, HumiditySolver) pre-allocate working buffers at construction and reuse them each step rather than allocating fresh `DVector`/`HashMap` per call. This is the general pattern; see HARES-014 and HARES-015 for solver-specific details.

## Verification

- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
