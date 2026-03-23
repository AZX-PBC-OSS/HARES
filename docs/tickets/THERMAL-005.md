---
id: THERMAL-005
title: Replace explicit state-space stepping with Crank-Nicolson implicit solver
kind: implement
depends_on: [THERMAL-002, THERMAL-003, THERMAL-004]
files_to_touch:
  - crates/hares-envelope/src/state_space.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
references:
  - "EnergyPlus 24.1 Engineering Reference, Zone Air Heat Balance"
  - "EnergyPlus 9.0 CondFD: Crank-Nicolson and fully implicit schemes"
  - "ESP-r: Crank-Nicolson throughout"
  - "Crank-Nicolson / trapezoidal rule for ODEs"
  - "nalgebra 0.34 docs: LU<f64, Dyn, Dyn> implements Clone (not Copy)"
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core
  - cargo clippy -p hares-envelope
---

## Background/Context

The current explicit solver (`x[k+1] = A_d * x[k] + B_d * u[k]`) uses matrix
exponential discretization. While exact for the linear system, it produces
numerical overshoot when large infiltration loads (high-ACH attic/crawlspace
zones) create effective time constants shorter than the timestep. OCHRE patches
this with an `h_limit` clamp — a stability hack no serious tool uses.

### Industry standard: implicit methods

Every major building energy simulation tool uses implicit methods:

| Tool | Method |
|------|--------|
| EnergyPlus | 3rd-order BDF (zone air), CN or fully implicit (wall CondFD) |
| ESP-r | Crank-Nicolson throughout |
| Modelica/Buildings | Variable-step implicit ODE (DASSL/Radau) |
| IDA ICE | Implicit BDF/NDF |
| TRNSYS | Modified Euler (explicit, but hourly timestep avoids instability) |
| OCHRE | Explicit + h_limit hack |

HARES should follow industry best practice: **implicit-only, no explicit fallback**.

### Why implicit-only (no SolverMethod enum)

- One code path = half the test surface, no dispatch overhead, simpler reasoning
- The explicit path's "exactness" is irrelevant when inputs are ZOH piecewise-constant
- CN error is O(dt²) per step ≈ 10⁻⁶ per step for dt=60s, τ=10000s — unmeasurable
- `discretize_auto()`/`matrix_exp()` remain as public utilities for test validation

### Crank-Nicolson derivation (ZOH input assumption)

Given continuous system `dx/dt = A_c · x + B_c · u`, with ZOH input
(u constant over timestep — `resolve_internal` builds u once before stepping):

```
(I - dt/2 · A_c) · x[k+1] = (I + dt/2 · A_c) · x[k] + dt · B_c · u[k]
```

The `dt · B_c · u[k]` term is correct for ZOH because `u[k] = u[k+1]`, making
`dt/2 · B_c · (u[k] + u[k+1]) = dt · B_c · u[k]`.

Pre-compute at construction (O(n³) one-time cost):
- `M = I - dt/2 · A_c` (implicit system matrix)
- `m_lu = LU::new(M)` (factored once; `LU<f64, Dyn, Dyn>` implements Clone)
- `N = I + dt/2 · A_c` (explicit half)
- `B_eff = dt · B_c`

Each step: `x[k+1] = m_lu.solve(N · x[k] + B_eff · u[k])` — O(n²)

### nalgebra LU: Clone supported, Copy NOT for Dyn

`LU<f64, Dyn, Dyn>` implements `Clone` but NOT `Copy` (heap-allocated storage).
`StateSpaceModel` can derive `Clone` but not `Copy`. This is fine — the model
is constructed once and shared by immutable reference.

### LWR iteration unchanged

The implicit solver eliminates linear RC overshoot. Exterior LWR radiation is
nonlinear (T⁴) and still requires the existing iterative coupling. This ticket
does not change LWR iteration logic.

### solve_for_output_input (implicit analog)

For ideal HVAC: given x, u, find u[i] such that y[j] = target.

```
// Separate fixed vs variable parts of rhs:
rhs_fixed = N·x + B_eff·u - B_eff[:,i]·u_i_original
x_next_fixed = m_lu.solve(rhs_fixed)
g = m_lu.solve(B_eff[:,i])  // gain vector, pre-computable per input index

y_fixed = C·x_next_fixed + D·u - D[j,i]·u_i_original
effective_gain = C[j,:]·g + D[j,i]
u_i = (y_target - y_fixed) / effective_gain
```

### Steady-state initialization

Continuous steady-state: `0 = A_c · x + B_c · u` → `x_ss = -A_c⁻¹ · B_c · u`.
Independent of discretization method. For the partitioned system (zone temps
as boundary conditions), partition A_c and B_c the same way the current code
partitions A_d and B_d.

`from_discrete()` backward-compat path stores `a_c = None`, `b_c = None` and
falls back to discrete steady-state `(I - N)⁻¹ · B_eff · u` (which equals
`(I - A_d)⁻¹ · B_d · u` since M=I for that path).

## Architecture Constraints

### File organization

Keep `state_space.rs` focused on the mathematical model (struct, step, solve).
Extract `initialization.rs` for the steady-state partitioned solve (per
THERMAL-003). Tests in `#[cfg(test)]` modules or `tests/` integration files.

### Zero allocations in the hot loop

The `step()` method is called ~500k times per annual simulation. It MUST NOT
allocate:

- Pre-allocate `rhs_buf: DVector<f64>` in `ThermalSolver` (alongside existing
  `u_buf`). Reuse via `std::mem::replace` swap pattern (matching `u_buf` at
  `mod.rs:207`).
- Use `gemv` for matrix-vector products into pre-allocated buffers:
  `rhs_buf.gemv(1.0, &n_mat, x, 0.0)` then `rhs_buf.gemv(1.0, &b_eff, u, 1.0)`.
  This writes `N·x + B_eff·u` into `rhs_buf` with zero allocation.
- Use `m_lu.solve_mut(&mut rhs_buf)` to solve in-place. The result overwrites
  `rhs_buf` which becomes the new `x`.
- Swap `x` and `rhs_buf` after solving so the buffer is ready for the next step.

### Immutable pre-computed data

`StateSpaceModel` is immutable after construction. All matrices (`m_lu`, `n_mat`,
`b_eff`, `c`, `d`) are `&self` reads. Mutable state (`x`, `rhs_buf`, `u_buf`)
lives in `ThermalSolver`, not `StateSpaceModel`.

## Work to Do

### StateSpaceModel struct (state_space.rs)

- [ ] Replace internal fields:
      ```rust
      #[derive(Clone)]
      pub struct StateSpaceModel {
          a_c: Option<DMatrix<f64>>,  // None for from_discrete() path
          b_c: Option<DMatrix<f64>>,  // None for from_discrete() path
          m_lu: LU<f64, Dyn, Dyn>,   // pre-factored (I - dt/2 * A_c)
          n_mat: DMatrix<f64>,        // I + dt/2 * A_c
          b_eff: DMatrix<f64>,        // dt * B_c
          pub c: DMatrix<f64>,
          pub d: DMatrix<f64>,
      }
      ```
- [ ] Add accessor methods: `state_dim()`, `input_dim()`, `output_dim()`.
- [ ] Add `a_c()` → `Option<&DMatrix<f64>>`.
- [ ] Add `b_c()` → `Option<&DMatrix<f64>>`.
- [ ] Add `StateSpaceError::ImplicitMatrixSingular`.
- [ ] Rewrite `from_continuous()`:
      - Compute M, N, B_eff from A_c, B_c, dt.
      - LU factorize M. Fail with `ImplicitMatrixSingular` if singular.
      - Store `a_c = Some(a_c.clone())`, `b_c = Some(b_c.clone())`.
      - Keep eigenvalue stability check on equivalent `A_d = M⁻¹N` for n ≤ 20.
      - Add Gershgorin circle bound for n > 20 (cheap O(n²) check).
- [ ] Rewrite `from_discrete()` for backward compat:
      - Set `m_lu = LU::new(I)`, `n_mat = A_d`, `b_eff = B_d`.
      - Set `a_c = None`, `b_c = None`.
      - Step degenerates to `I.solve(A_d*x + B_d*u) = A_d*x + B_d*u`.
- [ ] Rewrite `step()` — zero-allocation version:
      ```rust
      pub fn step_into(&self, x: &DVector<f64>, u: &DVector<f64>, buf: &mut DVector<f64>) {
          buf.gemv(1.0, &self.n_mat, x, 0.0);   // buf = N·x
          buf.gemv(1.0, &self.b_eff, u, 1.0);    // buf += B_eff·u
          self.m_lu.solve_mut(buf);               // buf = M⁻¹·buf (in-place)
      }
      ```
      Keep `step()` returning `DVector` for convenience in tests; have it
      delegate to `step_into()` with an internal allocation.
- [ ] `output()`: No change (`y = C*x + D*u`).
- [ ] Rewrite `solve_for_output_input()` with implicit derivation.
- [ ] Consolidate `solve_for_input()` as wrapper (per THERMAL-003 cleanup).
- [ ] Add `steady_state(u: &DVector<f64>) -> Option<DVector<f64>>`:
      If `a_c` is Some: solve `A_c · x = -B_c · u`.
      If `a_c` is None: solve `(I - n_mat) · x = b_eff · u`.
- [ ] Keep `discretize_auto`, `matrix_exp`, `van_loan_discretize` as public
      test/validation utilities. They are no longer called from `from_continuous()`.

### ThermalSolver (thermal_solver/mod.rs)

- [ ] Add `rhs_buf: DVector<f64>` field to `ThermalSolver`.
- [ ] In `resolve_internal()`, use `model.step_into(x, u, &mut rhs_buf)`
      then swap `x` and `rhs_buf`.
- [ ] Update `model_dims()` to use new accessors.
- [ ] Update `initialize_steady_state()` to use `model.steady_state(&u)`.
- [ ] Update `#[cfg(test)]` helpers or remove if unused.

### No changes to infiltration

The implicit solver eliminates infiltration overshoot by construction.
No h_limit clamp needed. `infiltration.rs` is unchanged by this ticket.

### Rollback strategy

If validation fails after this ticket:
1. Revert `state_space.rs` and `thermal_solver/mod.rs` to pre-THERMAL-005.
2. THERMAL-001/002/003/004 are independent and preserved.
3. Re-run THERMAL-004 tests to confirm baseline still passes.

## Files to Touch

- `crates/hares-envelope/src/state_space.rs`: Implicit-only solver
- `crates/hares-envelope/src/thermal_solver/mod.rs`: Zero-alloc stepping + accessors

## Measures of Success

- [ ] All existing state_space.rs tests pass (updated for new internals).
- [ ] All existing thermal_solver tests pass.
- [ ] `from_discrete()` backward compat: tests produce identical results.
- [ ] New test: 1R1C exponential decay matches analytical within 0.05°C at 24h.
- [ ] New test: system that overshoots with explicit converges with implicit.
- [ ] New test: `step_into()` produces zero allocations (use `#[global_allocator]`
      counting allocator in a test).
- [ ] 1-hour OCHRE parity test passes (<5% total energy diff).

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-envelope` clean
- [ ] `uv run pytest tests/python/test_ochre_parity.py::test_print_comparison -v -s`
      shows all equipment within 5%
