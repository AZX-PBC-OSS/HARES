# Solver feedback actor: ideal capacity dispatch bridge between solvers and equipment
**Review ID**: agents-04
**Category**: agents
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-core/src/actors/solver_feedback.rs` (187 lines)
- `crates/hares-envelope/src/thermal_solver/stepping.rs` (466 lines)
- `crates/hares-core/src/dwelling/mod.rs` (lines 2290–2409, timestep dispatch)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py` (371 lines)

## Findings

### Finding 1: [Severity: medium]
**Description**: Multiple equipment sharing the same zone can independently report `ideal_target()`, causing the solver to compute the full zone load for each equipment. When both dispatch `IdealCapacity`, the thermal injection doubles the zone requirement.

**Code Location**:
- `crates/hares-core/src/actors/solver_feedback.rs:77–83` — `collect_with` iterates all equipment independently
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:2170–2176` — `ideal_target()` returns `Some` for any heater with `use_ideal`, without checking operating mode or deduplicating against other heating equipment in the same zone
- `crates/hares-equipment/src/hvac/furnace.rs:356–362` — furnace: same pattern
- `crates/hares-equipment/src/hvac/baseboard.rs:232–238` — baseboard: same pattern
- `crates/hares-equipment/src/hvac/boiler.rs:346–352` — boiler: same pattern

**Root Cause**: `solve_ideal_capacity_for_target` computes the *total* capacity required to drive a zone's air temperature to `target_c`, given the full state vector and input vector. It does not partition the load across equipment. When two heating devices in the same zone both report `ideal_target()`, each receives the full zone load from independent solver calls because `last_u` and `x` are not modified between `collect_with` iterations. Neither the `SolverFeedbackActor` nor the individual `ideal_target()` implementations contain a deduplication or zone-load-partitioning guard.

**Impact**: In a multi-equipment-per-zone scenario (e.g., heat pump with backup resistance heater in the same zone), the dwelling over-injects heat by the number of active heating devices. The zone overheats and the thermostat FSM will disengage on the next timestep, creating on/off oscillation. This cannot happen for cooling because `AirConditioner::ideal_target()` (air_conditioner.rs:1469–1479) gates on `operating_mode == Cooling`, and multi-stage systems typically use a single `IdealHvac` model that reports one target. But dedicated heating equipment (furnace, boiler, baseboard, heat pump heater) has no mode gate — only a `use_ideal` check, which is true at coarse timesteps for all simultaneously.

**Vendor reference**: OCHRE explicitly prevents this at `Dwelling.py:143–148`:
```python
if end_use in ["HVAC Heating", "HVAC Cooling", "Water Heating"]:
    raise OCHREException(f"More than 1 equipment defined for {end_use}: {eq}")
```
HARES relies on upstream HPXML parsing to enforce one-at-most, but has no defensive check at the solver-feedback level. If that enforcement fails or is bypassed, the solver silently produces incorrect results.

**Recommendation**: Either (a) add a per-zone, per-end-use uniqueness check during dwelling construction (matching OCHRE's approach), or (b) in `collect_with`, skip equipment whose zone already has a pending ideal target for the same end use, or (c) partition `last_u[input_idx]` among equipment in the same zone by accumulating prior solved capacities.

---

### Finding 2: [Severity: low]
**Description**: NaN values can propagate from the solver through to `IdealCapacity` signals without being caught by the error-handling path.

**Code Location**:
- `crates/hares-envelope/src/thermal_solver/stepping.rs:144` — `total.map(|raw| raw - capacity_value)` retracts `last_u[input_idx]` from the solved total
- `crates/hares-core/src/actors/solver_feedback.rs:81` — `self.pending.push((idx, capacity_w))` stores the raw result
- `crates/hares-core/src/actors/solver_feedback.rs:110` — `ControlSignal::IdealCapacity { capacity_w }` dispatches without validation

**Root Cause**: `solve_for_scalar_input` (state_space.rs:545–594) wraps the solve result in `Result::Ok`. If the solve produces a NaN (e.g., from a degenerate state matrix with NaN eigenvalues, or from a NaN input in `x`/`u`/`y_target`), it passes through `Ok` because the NaN is not an error according to Rust's `Result` type. The error guards only cover:
- Index out of bounds (line 553–564)
- Singular LU decomposition (line 574, 578)
- Zero effective gain (line 589–590)

NaN in the result passes through `Ok(total)` → `Ok(raw - capacity_value)` → `Ok(NaN)`, which is returned as `f64::NaN`. Equipment receiving `ideal_capacity_w = NaN` will compute NaN PLR and inject NaN thermal output into the ports, propagating NaN into the next timestep's state vector.

**Impact**: Low-likelihood (requires a pathological model or input corruption), high-impact (total simulation contamination once NaN enters). The 0.0 fallback for errors is correct; the gap is that NaN is not classified as an error.

**Recommendation**: After the solve, check `capacity.is_finite()` and fall back to 0.0 with a `warn!` if the result is non-finite:
```rust
Ok(capacity) if capacity.is_finite() => capacity,
Ok(_) => { tracing::warn!(...); 0.0 }
```

---

### Finding 3: [Severity: low]
**Description**: `SolverFeedbackActor::decide()` silently drops `IdealCapacity` signals when `dispatch_targets` is shorter than the equipment index, producing only a `warn!` log.

**Code Location**: `crates/hares-core/src/actors/solver_feedback.rs:100–106`
```rust
let Some(target) = self.dispatch_targets.get(idx) else {
    tracing::warn!(idx, len = self.dispatch_targets.len(),
        "dispatch_targets not synced with equipment");
    continue;
};
```

**Root Cause**: The `decide()` drain loop uses equipment indices stored during `collect_and_solve` but looks them up in `dispatch_targets`. If the two fall out of sync (e.g., equipment was removed but `collect_and_solve` captured a stale index, or `refresh_equipment_caches` was interrupted), the signal is silently dropped. The equipment receives no `IdealCapacity`, operates at 0 capacity instead of the solver-computed value, and the dwelling diverges from the intended thermal trajectory.

**Impact**: Low — `refresh_equipment_caches` (dwelling/mod.rs:1684–1692) recomputes both `equipment_execution_order` and `dispatch_targets` from the same `&self.equipment` vec in a single call, so they cannot drift in normal operation. The guard is a defensive fallback, but the recovery (silent drop) is worse than a panic because it produces silently incorrect results.

**Recommendation**: Consider upgrading this to a panic or at minimum an error-level log, since an out-of-sync state is a programming error, not a recoverable runtime condition. Alternatively, store the dispatch target inside the `pending` tuple `(target, capacity_w)` during `collect_with` so that sync is guaranteed at capture time.

---

### Finding 4: [Severity: low]
**Description**: The log-throttling mechanism in `solve_ideal_capacity_for_target` suppresses `warn!` for consecutive solver failures after the first, making persistent solve failures invisible at default log levels.

**Code Location**: `crates/hares-envelope/src/thermal_solver/stepping.rs:162–188`
```rust
Err(e) => {
    let count = self.ideal_capacity_failure_counts
        .entry(zone).and_modify(|c| *c += 1).or_insert(1);
    if self.ideal_capacity_warned_zones.insert(zone) {
        tracing::warn!(...);
    } else {
        tracing::debug!(...);
    }
    0.0
}
```

**Root Cause**: After the first failure, `ideal_capacity_warned_zones` already contains the zone, so subsequent failures log at `debug!` level (suppressed in default `info`-level logging). The counter increments but is invisible. If a zone's solver is persistently broken (e.g., singular matrix, miswired indices), the only visible signal is one `warn!` message followed by silence — while every timestep silently returns 0.0 capacity.

**Impact**: Makes debugging persistent solver failures unnecessarily difficult. A run with 8760 hourly steps and a broken zone would produce exactly one warning at default log level, masking the scope of the problem. The ZoneId in the failure map also leaks memory for the duration of the run (the HashSet and HashMap entries are never removed for zones that permanently fail).

**Vendor reference**: OCHRE raises exceptions for configuration errors rather than silently returning 0.0, making failures immediately visible. HARES' defensive design (continue simulation even with partial model failures) is reasonable for long-running simulations, but the log throttling makes it harder to detect when failures are persistent rather than transient.

**Recommendation**: Log a summary at the end of the simulation (or periodically, e.g., every 50 consecutive failures) that reports the cumulative failure counts per zone, making persistent issues detectable at default log levels without flooding.

---

## Summary
- **Total findings**: 4
- **Critical / High / Medium / Low**: 0 / 0 / 1 / 3

### Ordering verification

The collect-and-solve ordering is correct. The dwelling timestep at `dwelling/mod.rs:2324–2338` runs equipment `update_control()` (Step 1c) first to determine modes and target setpoints, then `prepare_inputs` (Step 1d) to build the current-step input vector with weather/solar/internal gains, then `collect_and_solve` (Step 1e) to collect all ideal targets and compute capacities. The solver sees the complete picture of all zones and all targets before dispatching any signals.

### Multi-zone verification

Multi-zone is correctly handled. `collect_with` (solver_feedback.rs:77–83) iterates all equipment independent of zone. Each equipment's `ideal_target()` returns `(ZoneId, target_c)` for its assigned zone. `solve_ideal_capacity_for_target` (stepping.rs:98–104) uses per-zone input/output indices from `StateSpaceWiring` to isolate each zone's solve. The solver reads from `self.last_u` and `self.x` without modifying them, so one zone's solve does not interfere with another's.

### No-equipment-provides-ideal-target verification

When no equipment reports `ideal_target()` (all in deadband or `use_ideal = false`), `collect_with` produces an empty `pending` buffer. `decide()` drains an empty buffer → zero `DispatchRequest` values → no `IdealCapacity` signals dispatched → equipment steps with `ideal_capacity_w = 0.0` → free-floating thermal response. No NaN, no crash. This path is correct.

### Feedback-loop prevention verification

The `IdealCapacity` → equipment dispatch path is one-way within a timestep:
1. `ideal_target()` is queried once (Step 1e), before `IdealCapacity` signals exist
2. `IdealCapacity` signals are dispatched (Step 2), setting `ideal_capacity_w` on equipment
3. `update_control()` re-runs (Step 2a) but does not call `ideal_target()` — only re-evaluates the thermostat FSM
4. Equipment `step()` (Step 3a) consumes `ideal_capacity_w`, then clears it to 0.0
5. The `ControlDispatcher` priority ledger (`begin_step` at line 2292) prevents double-writes to the same target within the same step

No feedback loop exists. The actor-solve-dispatch pipeline is a directed acyclic graph within each timestep.

## Recommendations
1. **Add a per-zone, per-end-use uniqueness check** during dwelling construction (matching OCHRE's validation) or add a deduplication guard in `collect_with` to prevent multiple equipment in the same zone from reporting independent ideal targets for the same end use (Finding 1).
2. **Guard against NaN capacity** from the solver by checking `capacity.is_finite()` after the solve and falling back to 0.0 with a warning (Finding 2).
3. **Upgrade the `dispatch_targets` sync warning** from `warn!` to either a panic or an error log, since out-of-sync indices represent a programming error (Finding 3).
4. **Periodically report cumulative solver failure counts** at `info!` level (e.g., every Nth consecutive failure or at simulation end) so that persistent solver failures are detectable without enabling debug logging (Finding 4).

## References / Citations
- OCHRE Dwelling.py:176–180 — forces ideal HVAC to run last among sub-simulators so all internal gains are known before HVAC runs
- OCHRE Dwelling.py:143–148 — raises exception when >1 equipment exists for the same HVAC/WH end use
- HARES dwelling/mod.rs:2209–2608 — full per-timestep orchestration with numbered steps
- HARES solver_feedback.rs:52–84 — `collect_and_solve` and `collect_with` implementations
- HARES stepping.rs:98–190 — `solve_ideal_capacity_for_target` with log-throttling and failure-count tracking
- HARES state_space.rs:488–593 — coupled and uncoupled `solve_for_scalar_input` implementations
