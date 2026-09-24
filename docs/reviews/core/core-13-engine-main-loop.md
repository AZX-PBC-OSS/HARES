# Engine main loop step sequencing and error recovery
**Review ID**: core-13
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/engine.rs`
- `crates/hares-core/src/lib.rs`
- `crates/hares-core/src/dwelling/mod.rs` (simulate, run_timestep, run_warmup_converged)
- `crates/hares-core/src/clock.rs`
- `crates/hares-core/src/invariants.rs`
- `crates/hares-core/src/checkpoint.rs`
- `crates/hares-core/src/actors/solver_feedback.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-types/src/equipment.rs` (validate_core_contract)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/SimulationManager.cc`

## Findings

### Finding 1: No adaptive time-stepping or sub-step convergence loop
**Severity**: high
**Description**: HARES uses a single-pass, fixed-time-step design. On thermal solver non-convergence (e.g., `solve_ideal_capacity_for_target` fails to solve for required HVAC capacity), the solver returns `0.0` silently and continues (stepping.rs:316-341). The equipment then receives a zero-capacity `IdealCapacity` signal with no indication of degraded operation. Failed equipment steps become warnings only -- the simulation does not retry with a shorter step, and the `ports` accumulator state is not rolled back.

EnergyPlus (`SimulationManager.cc:574-628`) employs a multi-level convergence scheme: `ZoneTimeStep` for heat balance, followed by a variable number of `SystemTimeStep` sub-iterations within each zone timestep (governed by `MinTimeStepSys`, default 1 minute). When HVAC iterations hit `MaxIter` (default 20), EnergyPlus warns about non-convergence but the heat balance has already been satisfied at the zone level. HARES has no equivalent sub-stepping or iteration.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1341-1344` (simulate loop); `crates/hares-envelope/src/thermal_solver/stepping.rs:316-341` (solver failure fallback); `crates/hares-core/src/actors/solver_feedback.rs:52-60` (capacity dispatch)
**Root Cause**: Single-pass solver architecture with no within-timestep feedback coupling between thermal solver convergence and equipment control.
**Impact**: A non-convergent thermal step produces zero HVAC capacity, causing zone temperatures to drift from setpoints. The drift cascades into subsequent steps because zone temperatures are the initial state for the next step. In a multi-zone building, one failed zone does not affect others, but repeated failures accumulate. With hourly timesteps over a year, even a 1% failure rate produces ~87 hours of uncontrolled thermal drift.

### Finding 2: Equipment failure isolation is incomplete -- ports are not cleaned up after failed steps
**Severity**: high
**Description**: In `run_timestep()` (dwelling/mod.rs:2422-2443 for thermal, 2471-2491 for non-thermal), when an equipment's `step()` returns `Err`, a warning is pushed but `step_succeeded[idx]` remains `false`. The equipment's port contributions (written to `self.ports` before the error return) are **not rolled back** for that equipment. Downstream equipment in the same execution order see the partial/stale port accumulations from the failed equipment. Only `validate_core_contract` violations (equipment.rs:943-1009) cause a hard failure; step-level errors do not.

`step_succeeded[idx]` is used at lines 2680-2682 to skip snapshotting the failed equipment's state into `latest_env.equipment_core`, which prevents the next step from reading stale telemetry. However, port-level contributions (thermal gains, electrical loads) remain in the accumulator from the failed step's partial writes.

EnergyPlus handles this differently: its `ZoneEquipmentManager` and `HVACManager` track convergence on a per-loop basis, and if the overall plant loop does not converge, the timestep state is reset to the beginning-of-step values for that loop (controlled by `MaxIter` and warning thresholds).

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2430-2443`, `crates/hares-core/src/dwelling/mod.rs:2680-2682`
**Root Cause**: `self.ports` is a shared accumulator for all equipment; a failed step's partial writes cannot be identified and reverted without per-equipment port accounting.
**Impact**: A single equipment failure can produce dirty port state that cascades into downstream equipment on the same step. If a thermal equipment fails, its `thermal_output_w` contribution may be zero or partial, affecting the thermal solver's component gains and zone temperature for that step.

### Finding 3: Invariant checks are entirely elided in production release builds
**Severity**: medium
**Description**: The `check_invariants()` call at dwelling/mod.rs:2652 is gated by `#[cfg(any(debug_assertions, feature = "check_invariants"))]` (line 3052). In a release build without the `check_invariants` feature, the entire 220-line function body compiles to nothing. This means:

- Zone temperature bounds `[-50, 80]` °C conditioned / `[-50, 120]` °C unconditioned: **not checked**
- Tank temperature bounds `[0, 100]` °C: **not checked**
- Electrical finiteness (`net_kw.is_finite()`): **not checked**
- Electrical balance solver vs port accumulation: **not checked**
- Moisture mass conservation: **not checked**
- SOC bounds: **not checked**

If the thermal solver produces NaN zone temperatures (e.g., from a singular system matrix after a weather file with extreme conditions), this propagates silently into equipment control, output recording, and downstream metrics. The NaN value persists in output Arrow batches, which downstream metric calculators cannot distinguish from valid data.

The `check_temperatures()` function (invariants.rs:148-191) correctly uses `!t.is_finite()` guards, so it *would* catch NaN if enabled. But `check_thermal()` (invariants.rs:30-55) and `check_moisture()` (invariants.rs:82-112) use simple comparison operators (`residual >= tolerance`), where `NaN >= tolerance` evaluates to `false`, bypassing the check even when enabled.

EnergyPlus runs all balance checks unconditionally; its `DataConvergParams` and `ShowSevereError`/`ShowFatalError` mechanism is always active regardless of build configuration.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3048-3267` (check_invariants), `crates/hares-core/src/invariants.rs:30-55` (check_thermal NaN bypass)
**Root Cause**: Invariant checking is treated as a debugging aid, not a production safeguard.
**Impact**: Silent numerical corruption in production runs. A NaN from one step can render the entire timeseries file invalid without any warning.

### Finding 4: Warmup is well-implemented but RNG state is not isolated
**Severity**: medium
**Description**: `run_warmup_converged()` (dwelling/mod.rs:2143-2207) correctly implements EnergyPlus warmup convergence:
- Threshold: 0.5 °C (matching EnergyPlus default)
- Max iterations: 25 (matching EnergyPlus default)
- Comparison metric: max |ΔT_zone| across all conditioned zones
- Non-convergence behavior: warning + proceed with current thermal state (matches EnergyPlus)

However, the warmup loop runs `self.run_timestep(false)` using the **same RNG instance** that will be used for the production run. The RNG state is not saved before warmup and restored after. Clock reset (line 2159: `self.clock.current_step = 0`) resets the time index for weather replay, but the RNG stream continues from wherever warmup left it. After warmup, a new `SimClock` is constructed (lines 1321-1326) but `dwelling.rng` is not touched.

Consequence: if the same dwelling config is simulated twice with the same seed, but warmup converges in a different number of iterations (e.g., due to different initial building state), the production RNG stream will be at different positions, producing different stochastic outcomes (occupant behavior, equipment derating, DR compliance randomization). This violates deterministic reproducibility.

The comment at line 2156-2158 correctly notes that thermal state carries forward from previous warmup day (per EnergyPlus §"Warmup Convergence"), but the RNG needs equal treatment: it should also carry forward. Instead, resetting the RNG to its pre-warmup state ensures reproducibility regardless of warmup iteration count.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1319-1326` (post-warmup clock reset without RNG reset), `crates/hares-core/src/dwelling/mod.rs:2159` (warmup clock reset)
**Root Cause**: The warmup loop resets the clock for weather replay but does not isolate RNG state.
**Impact**: Non-deterministic simulation results across runs with identical inputs. Monte Carlo ensemble studies will see additional variance.

### Finding 5: Simulation time boundary silently truncates partial remainder
**Severity**: medium
**Description**: `SimClock::total_steps()` (clock.rs:65-75) uses integer division: `u64::try_from(duration_secs / res).unwrap_or(0)`. If the simulation duration is not an exact multiple of the time resolution, the remainder is silently dropped. For example, a 24-hour duration with a 55-minute timestep yields `(86400 / 3300) = 26` steps (23:50:00 simulated), silently dropping 10 minutes. No warning is emitted.

The main loop condition `current_step() < total_steps()` is a standard half-open `[0, total_steps)` interval and is correct -- no off-by-one. The final step executes. After exhaustion, `current_time()` returns one step past the last simulated time (clock.rs:111-117 test confirms).

The `run_timestep()` guard at lines 2210-2214 redundantly checks `>= total_steps()` before executing, but this cannot be reached from the `simulate()` loop (which already has the `<` check).

EnergyPlus handles boundaries with explicit `EndHourFlag`, `EndDayFlag`, `EndEnvrnFlag` cascading (SimulationManager.cc:603-611), and its day-loop condition `(DayOfSim < NumOfDayInEnvrn) || WarmupFlag` (line 516) correctly handles the warmup-to-production transition.

**Code Location**: `crates/hares-core/src/clock.rs:65-75`, `crates/hares-core/src/dwelling/mod.rs:1342`
**Root Cause**: The silent integer division truncation is not documented as a user-visible constraint.
**Impact**: Users who configure non-dividing durations get fewer simulation steps than expected with no diagnostic. For an 8760-hour year with a non-dividing timestep, the cumulative truncation could be significant.

### Finding 6: Engine-level panic catch provides coarse failure isolation but no recovery
**Severity**: low
**Description**: `SimulationEngine::run()` (engine.rs:116) wraps `dwelling.simulate()` in `panic::catch_unwind(AssertUnwindSafe(|| ...))`. Three outcomes are handled:
1. `Ok(Ok(results))`: normal completion, metrics computed
2. `Ok(Err(err))`: simulation returned error → `SimStatus::Failed`
3. `Err(payload)`: panic → `SimStatus::Failed` with stringified payload

This correctly prevents a single dwelling panic from crashing the batch runner. The engine docstring (engine.rs:289) states "the engine then quarantines this dwelling rather than propagating a panic." However, there is no retry, no partial-results recovery, and no checkpoint-based restart. A single step failure terminates the entire run.

The `run_dwelling()` variant (engine.rs:204-274) is identical in structure but presumes a pre-configured dwelling.

**Code Location**: `crates/hares-core/src/engine.rs:116-187`
**Root Cause**: The engine is designed for batch-processing many dwellings, prioritizing containment over per-dwelling recovery.
**Impact**: A single step-level invariant violation terminates the entire annual simulation, discarding all prior output. For a year-long simulation (8760 hourly steps), a failure at step 8759 loses all data.

### Finding 7: Checkpoint infrastructure exists but is never called in the main loop
**Severity**: low
**Description**: `save_checkpoint()` (dwelling/mod.rs:1991-2018) and `load_checkpoint()` (dwelling/mod.rs:2022-2068) provide complete state snapshot/restore capability (RNG seed, thermal solver state, humidity, fluid, equipment states, LWR temperatures). However, neither is called during `simulate()` or `run_timestep()`. The checkpoint mechanism exists solely as a public API for external orchestration.

There is no periodic auto-save (e.g., every N steps or every simulated day) and no crash-recovery path that restores from the last checkpoint. In EnergyPlus, `ManageSimulation` does per-day SQLite transactions (SimulationManager.cc:522-523, 634-636), which provides crash recovery at day boundaries.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1341-1352` (simulate, no checkpoint calls), `crates/hares-core/src/checkpoint.rs` (save/load)
**Root Cause**: Checkpoint is designed as an externally-driven API, not integrated into the simulation loop.
**Impact**: Long-running simulations have no automated crash recovery. No incremental results are saved if the process is killed.

## Summary
- Total findings: 7
- Critical: 0 / High: 2 / Medium: 3 / Low: 2

## Recommendations

1. **Implement adaptive fallback for thermal solver non-convergence** (Finding 1). Instead of returning 0.0 and continuing, accumulate a per-zone non-convergence counter and, after a configurable threshold (e.g., 3 consecutive failures), either (a) use the last-good capacity value with a degradation flag, or (b) emit a `SimStatus::Flagged` error that aborts the run with partial results preserved.

2. **Add per-equipment port accounting** (Finding 2). Before each equipment's `step()` call, snapshot the relevant port accumulator sections. On failure, restore the snapshot so downstream equipment sees clean state. This is the minimum viable failure isolation for equipment within a single timestep.

3. **Enable at least electrical finiteness and zone temperature NaN checks in release builds** (Finding 3). These are O(n_equipment + n_zones) per step and cheap. The full invariant suite can remain behind the feature flag, but NaN propagation through temperatures and electrical power should be caught unconditionally -- it represents unrecoverable data corruption, not a recoverable numerical issue.

4. **Snapshot and restore RNG state around warmup** (Finding 4). Save `self.rng.get_seed()`, `get_stream()`, and `get_word_pos()` before `run_warmup_converged()`, and restore them afterward. This ensures that two runs with the same initial seed produce identical production-phase stochastic output regardless of warmup iteration count.

5. **Warn on truncated duration** (Finding 5). In `SimClock::total_steps()`, compute the remainder and emit a `tracing::warn!` when `duration_secs % res != 0`, reporting the truncated time. Alternatively, document this as a hard requirement in the `SimClock::new()` docstring.

6. **Add periodic checkpoint auto-save** (Finding 6/7). Call `self.save_checkpoint()` every simulated day (or every N steps) and write to a `.checkpoint` file. On engine-level failure (panic or error), attempt `load_checkpoint` recovery in `SimulationEngine::run()`. For batch runners, this allows partial result recovery and incremental analysis on long-running simulations.

## References / Citations
- EnergyPlus Engineering Reference, "Warmup Convergence" (Basis for HARES warmup logic at dwelling/mod.rs:2132-2207)
- EnergyPlus `SimulationManager.cc:454-643` — Environment → Day → Hour → TimeStep nested loops with adaptive system sub-stepping
- EnergyPlus `SimulationManager.cc:516` — Day loop condition `(DayOfSim < NumOfDayInEnvrn) || WarmupFlag` handles warmup-production transition
- EnergyPlus `SimulationManager.cc:574-628` — TimeStep loop with `EndHourFlag`/`EndDayFlag` cascading and `ExternalInterfaceExchangeVariables`
- HARES `dwelling/mod.rs:1341-1344` — Single-pass `[0, total_steps)` loop with `?` propagation (no retry)
- HARES `stepping.rs:252-343` — `solve_ideal_capacity_for_target` returns 0.0 on failure with log-level escalation (warn → debug)
- HARES `invariants.rs:30-55` — `check_thermal` NaN bypass via `residual >= tolerance` comparison
- HARES `clock.rs:61-75` — `total_steps()` integer division truncation
- HARES `checkpoint.rs:15-30` — Full state checkpoint schema (format version 3)
