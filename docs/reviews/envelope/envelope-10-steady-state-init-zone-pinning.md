# Steady-state initialization zone pinning fragile invariant
**Review ID**: envelope-10
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/initialization.rs` (459 lines)
- `crates/hares-envelope/src/thermal_solver/mod.rs` (call site at lines 413–418)
- `crates/hares-envelope/src/thermal_solver/config.rs` (wiring struct, lines 414–469)
- `crates/hares-envelope/src/state_space.rs` (steady-state / discretization, lines 248–270, 277–319)
- `crates/hares-envelope/src/rc_network.rs` (matrix assembly, lines 162–183)
- `crates/hares-envelope/src/boundary_rc.rs` (`derive_zone_capacitances`, lines 397–426; `assemble_building_rc`, lines 447–495)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py:973–1033` — `Envelope.initialize_state()`: steady-state initialization with T_LIV pinned and A-matrix partition
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:2592–2641` — `InitHeatBalance()`: uses CTF response-factor precomputation, not a state-space steady-state solve; zone air temperature initialized from schedules/inputs, not a pinned matrix solve

## Findings

### Finding 1: No zero-capacitance zone guard in initialization path
**Severity**: Medium

**Description**:
`initialize_steady_state()` partitions the state-space matrix by removing the pinned zone’s row and column, then solves the reduced system. This approach assumes the zone air node contributes a meaningful diagonal term to the A-matrix — i.e., the zone has non-zero thermal capacitance. The function performs no verification that the pinned zone’s capacitance is non-zero.

If a zero-capacitance zone were to reach the initialization:
- The reduced `(I − A_d)` or `−A_c` matrix could become singular (a massless zone node pinned at a fixed temperature creates a kinematic constraint with no energy storage to absorb it).
- The singular-matrix fallback at lines 158–162 silently returns `DVector::from_element(n, indoor_temp_c)`, which fills ALL state-vector entries (including wall mass nodes) with `indoor_temp_c`. This is physically wrong — wall nodes should converge to intermediate temperatures between indoor and outdoor — and produces an inflated step-0 ideal-capacity back-solve.
- No error, warning, or diagnostic is emitted.

**Code Location**:
`initialization.rs:127–173` — zone pinning partition and reduced-matrix solve, with singular fallback at lines 158–162 and 167–172.

**Root Cause**:
The initialization function relies entirely on the upstream `derive_zone_capacitances()` + `assemble_building_rc()` pipeline to guarantee `c_zone >= MIN_CAPACITANCE_J_K` (1,000 J/K). It has no defense-in-depth check. The wiring struct carries `c_zone_j_k` (a `HashMap<ZoneId, f64>` at `config.rs:448`) and `node_capacitances` (`config.rs:463`) precisely for this purpose, but `initialize_steady_state()` never inspects them.

**Mitigating factors**:
- The standard construction path clamps zone capacitance to ≥ 1,000 J/K via `derive_zone_capacitances` (`boundary_rc.rs:423`, `.max(MIN_CAPACITANCE_J_K)`) and again in `assemble_building_rc` (`boundary_rc.rs:492`, `cap.max(MIN_CAPACITANCE_J_K)`).
- The `StateSpaceModel::from_continuous` path detects singular A_c and uses the Van Loan fallback for discretization (`state_space.rs:864–866`), avoiding NaN/Inf propagation into A_d/B_d.
- A `mass_multiplier` of 0.0 still yields the clamped minimum (1,000 J/K).

**Impact**:
Under normal operation (standard RC pipeline), this is non-exploitable. The risk is latent: a future construction path (manual `StateSpaceModel::from_discrete`, a user who removes the capacitance clamp, integration with a model format that allows zero-mass zones, or a code refactoring that separates the capacitance clamp from the construction path) could silently produce wrong initial conditions. The diagnostic gap is the larger concern — there is no `assert!`, `tracing::warn!`, or fallback guard to catch this invariant violation.

**Vendor comparison**:
- **OCHRE** (`Envelope.py:1026`): uses `np.linalg.inv(A)` directly. If A is singular, NumPy raises `LinAlgError` — an explicit crash that forces the user to fix the model. OCHRE also has no zero-capacitance guard per se, but `np.linalg.inv` does not silently produce wrong results; it fails loudly.
- **EnergyPlus** (`HeatBalanceManager.cc:2618–2619`): uses CTF-based initialization (`InitConductionTransferFunctions`) rather than a state-space steady-state solve. Zone air temperatures are initialized from schedule inputs, not a pinned partition. The zero-capacitance question does not arise because EnergyPlus never both pins a zone AND partitions the state matrix — the two operations are architecturally separated.

### Finding 2: Singular-matrix fallback vector dimension recovery uses full state size, not reduced size
**Severity**: Low

**Description**:
When the reduced-matrix solve fails (`try_inverse()` returns `None` at lines 158–161 or 167–171), the fallback constructs `DVector::from_element(n, indoor_temp_c)` using `n = model.state_dim()` (the full original state dimension, computed at line 34). This is semantically correct — the returned vector has the right number of elements for the caller — but fills all elements uniformly with `indoor_temp_c`, including wall mass nodes that should carry intermediate temperatures. In contrast, the successful-return path at lines 176–183 assembles `x_full` from the solved reduced temperatures with the pinned zone temperature re-inserted.

The `n_reduced == 0` path (lines 147–153) handles the zero-reduced-states case correctly by building from the zone fixes directly. The singular-fallback paths do not.

**Code Location**:
`initialization.rs:158–162`, `initialization.rs:167–172`.

**Root Cause**:
The fallback branches were written as an early-exit safety valve but do not attempt any physically-grounded recovery (e.g., solving only solvable sub-blocks, falling back to the unpinned `model.steady_state(&u)` path, or propagating a diagnostic).

**Mitigating factors**:
- The `try_inverse` failure path is hard to trigger in practice because the RC-constructed A-matrix is diagonally dominant and well-conditioned.
- The unpinned fallback path at lines 112–117 (`zone_fixes.is_empty()`) also uses the same `DVector::from_element` fallback, providing consistent (if equally poor) behavior.

**Impact**:
In the unlikely event this path is hit, the resulting initial conditions are distorted but not NaN or infinite. The simulation would self-correct over a few timesteps as the wall node temperatures drift toward the correct conduction steady state.

### Finding 3: Unpinned path inherits same singular-fallback without diagnostic
**Severity**: Low

**Description**:
When `pinned_zones` is empty, the code at lines 112–117 delegates to `model.steady_state(&u)`, which returns `None` for singular systems (`state_space.rs:252–269`, `try_inverse()` failure). The fallback `DVector::from_element(model.state_dim(), indoor_temp_c)` has the same problem as Finding 2. No warning distinguishes a physics-grounded no-solve from a degenerate case with zero coupling or disconnected nodes.

**Code Location**:
`initialization.rs:112–117`; `state_space.rs:252–269`.

**Root Cause**:
`StateSpaceModel::steady_state()` returns `Option` but the initialization code treats `None` as "cannot solve → fill with indoor temperature" rather than probing the root cause (zero outdoor coupling? singular A_c? zero diagonal on a row?).

**Impact**:
Low. Same mitigating factors as Finding 2.

## Summary
- Total findings: 3
- Critical: 0
- High: 0
- Medium: 1 (Finding 1 — missing zero-capacitance guard)
- Low: 2 (Findings 2, 3 — fallback quality issues)

## Recommendations

1. **Add a zone-capacitance guard in `initialize_steady_state`** (Finding 1): Before the partitioning solve, check that each pinned zone has a non-negligible capacitance. The `wiring.c_zone_j_k` map already exists for this purpose:
   ```rust
   for zone_id in pinned_zones {
       if let Some(&c_zone) = wiring.c_zone_j_k.get(zone_id) {
           if c_zone < 1e-6 {
               return Err(ThermalSolverError::Configuration(format!(
                   "zone {} capacitance {:.3e} J/K too small for steady-state pinning",
                   zone_id.0, c_zone
               )));
           }
       }
   }
   ```
   Alternatively, emit a `tracing::warn!` and fall back to the unpinned `model.steady_state(&u)` path rather than pinning a massless node.

2. **Improve the singular-fallback recovery** (Findings 2, 3): Instead of filling the entire state vector with `indoor_temp_c`, consider:
   - Falling back to `model.steady_state(&u)` for the unpinned system (even if pinned_zones is non-empty), accepting that the initial zone temperature won't match the setpoint.
   - If `model.steady_state(&u)` also fails, constructing a physically-grounded initial guess using the conduction UA-weighted average of the boundary temperatures (`outdoor_temp_c` and `ground_temp_c`), interpolated for each state index by its coupling strength.
   - Emitting a `tracing::warn!` diagnostic when the singular-fallback path is taken, so operators can distinguish zero-capacitance model errors from genuine convergence issues.

3. **Consider OCHRE's approach**: Let the solver fail explicitly on singular systems (like `np.linalg.inv` does) rather than silently substituting uniform temperatures. A `ThermalSolverError::SingularInitialization` variant would alert the modeler that the RC network or discretization parameters need attention.

## References / Citations
- OCHRE `Envelope.initialize_state()`: `vendors/OCHRE/ochre/Models/Envelope.py:973–1033` — zone-pinned steady-state partition; uses `np.linalg.inv(A)` (explicit crash on singularity)
- EnergyPlus `InitHeatBalance()`: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:2592–2641` — CTF-based, no zone-pinned partition
- EnergyPlus `InitConductionTransferFunctions()`: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:6129–6176` — response-factor precomputation; `ShowFatalError` on construction failure (lines 6172–6176)
- ASHRAE Fundamentals 2021 Ch. 18: steady-state conduction through multi-zone envelopes (referenced in docstring at `initialization.rs:23`)
- Kusuda & Achenbach (1965): ground temperature model used in initialization at `initialization.rs:50–57`
