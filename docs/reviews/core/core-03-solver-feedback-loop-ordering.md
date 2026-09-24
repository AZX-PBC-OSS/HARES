# Solver feedback loop ordering and convergence within time step
**Review ID**: core-03
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actors/solver_feedback.rs` (lines 1-187)
- `crates/hares-core/src/dwelling/mod.rs` (lines 2209-2815, `run_timestep`)
- `crates/hares-envelope/src/thermal_solver/mod.rs` (lines 1-1202, solver struct and `build_input_vector`)
- `crates/hares-envelope/src/thermal_solver/stepping.rs` (lines 1-620, `prepare_inputs_inner`, `integrate_inner`, `resolve_internal`)
- `crates/hares-envelope/src/electrical_solver.rs` (lines 1-389)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc` (lines 140-238, `ManageHeatBalance`)
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc` (lines 149-234, `ManageSurfaceHeatBalance`)
- `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.cc` (lines 198-247, `ManageZoneAirUpdates` and `PredictorCorrectorCtrl` enum)

## Findings

### Finding 1: Single-pass equipment-solver coupling with no within-timestep convergence iteration
**Severity**: high

**Description**: The HARES simulation loop (`run_timestep`) uses a single-pass design: thermal solver `prepare_inputs` runs before equipment steps (Step 1d), equipment steps and deposits gains to ports (Steps 3a-3b), then thermal solver `integrate` runs with the updated ports (Step 4). There is no within-timestep feedback loop that iterates equipment-solver coupling until convergence. Equipment decides its output (Step 1c, Step 2a) based on the solver state from `prepare_inputs`, which was built using the *previous* step's equipment port contributions, not the current step's.

By contrast, EnergyPlus's `ManageSurfaceHeatBalance` → `ManageAirHeatBalance` path (HeatBalanceSurfaceManager.cc:149-234) uses an explicit predictor-corrector architecture: `PredictSystemLoads` computes the predicted HVAC load, the HVAC system runs, and then `correctZoneAirTemp` corrects zone temperatures based on actual HVAC delivery. The `ZoneTempPredictorCorrector` (ZoneTempPredictorCorrector.cc:225-247) exposes separate `PredictStep`, `CorrectStep`, and `PushZoneTimestepHistories` phases with the `PredictorCorrectorCtrl` enum. Additionally, EnergyPlus calls `UpdateFinalSurfaceHeatBalance` (HeatBalanceSurfaceManager.cc:188) for radiant systems that require a final averaging pass.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2209-2815` (entire `run_timestep` method)

**Root Cause**: The architecture intentionally separates the equipment-solver phases but does not include a convergence check or iteration loop. The solver feedback actor (`collect_and_solve` at line 2337-2338, using the `ThermalSolver::solve_ideal_capacity_for_target` method at stepping.rs:252-344) serves as a predictor — it computes the required HVAC capacity based on the `prepare_inputs` weather/solar/infiltration state plus occupancy gains, but the final integration at Step 4 uses whatever the equipment actually delivered (which may differ from the predicted capacity due to equipment constraints like cycling limits, capacity limits, or COP derating).

**Impact**: 
1. When equipment has fast response relative to the thermal time constant (e.g., high-capacity heat pumps in lightweight construction with time constants under 1 hour), the one-step lag between equipment decision and thermal state can cause oscillations: equipment over-corrects based on previous state, the thermal solver integrates the overshoot, and the cycle repeats every 2-4 timesteps.
2. Equipment that runs at partial capacity due to constraints (minimum runtime, short-cycling protection) injects less thermal energy than the ideal capacity calculation predicted, yet there is no correction pass to account for this discrepancy within the same step.
3. The "two-phase" design (`prepare_inputs_inner` at stepping.rs:370-388 followed by `integrate_inner` at stepping.rs:392-607) computes a coupled LU during prepare that is consumed by the solver feedback actor, but this LU is recomputed during `integrate_inner` (line 400-404) rather than reused — meaning the coupled system solved during ideal capacity computation is not exactly the same system integrated in the final step.

**Acceptability for Residential Simulation Time Scales**: For 1-minute timesteps with heavyweight construction (BESTEST Case 600 ~3.5 hour time constant), the one-step lag is acceptable because the thermal time constant is 210× the timestep, so the step-to-step temperature change is small and equipment cannot overshoot meaningfully. However, for 5-minute timesteps with lightweight construction (low-mass envelope, minimal thermal capacitance), the thermal time constant can be as low as 15-30 minutes (3-6 timesteps), making the lag of 1 timestep potentially significant. The same concern applies to 15-minute and 60-minute timestep configurations where the timestep approaches or exceeds the thermal time constant of low-mass zones.

---

### Finding 2: Same-timestep temporal ordering: PV generation unavailable for BMS self-consumption decisions
**Severity**: medium

**Description**: The simulation loop ordering creates a one-timestep data dependency gap for electrical systems. The `EnvironmentState.electrical` field is populated at Step 1 with `self.prior_electrical_summary` (dwelling/mod.rs:2253), which was computed at the **end** of the previous timestep (lines 2785-2806). Battery management actors run in Step 1f (line 2358-2363), consuming `latest_env` which contains the *prior* step's PV generation, not the current step's. PV equipment steps in Step 3b (non-thermal, ExecutionStage::Independent, rank 0), depositing generation data to ports. The electrical solver then runs in Step 4 (line 2583-2588) and correctly aggregates PV + battery + load, but this information arrives too late for the BMS actor's decision in the same step.

The `stage_rank` function (conversions.rs:461-467) orders execution as Independent (0) → Electrical (1) → Thermal (2). This ensures PV (Independent) runs before Battery (Electrical) within Step 3b, so the battery's `step()` call could conceptually see PV contributions if it reads `ports.electrical`. However, the BMS actor's **decision** (charge power setpoint) is already fixed before equipment steps. The battery equipment's `step()` method receives the actor's pre-computed setpoint; it cannot revise the decision based on same-step PV availability.

**Code Location**: 
- Prior electrical summary computation: `crates/hares-core/src/dwelling/mod.rs:2785-2806`
- BMS actor decide: `crates/hares-core/src/dwelling/mod.rs:2358-2360` (actor loop in Step 1f)
- PV step execution: `crates/hares-core/src/dwelling/mod.rs:2471-2503` (non-thermal equipment loop, Step 3b)
- Electrical solver resolve: `crates/hares-core/src/dwelling/mod.rs:2583-2588` (Step 4)

**Root Cause**: The architecture performs actor decisions (Step 1f) before equipment execution (Steps 3a-3b), creating a temporal ordering where control decisions are based on stale state. This is structurally necessary for the single-pass design but creates a tension: equipment needs actor-set directives (charge power, SOC targets) before stepping, but actors need equipment output (PV generation) to compute optimal directives.

**Impact**: For BMS strategies that depend on real-time PV availability (SelfConsumption, solar-only charging), the one-step lag in PV data means the battery may fail to capture transient PV generation spikes (e.g., cloudy/sunny transitions on sub-10-minute timescales) or may attempt to charge when PV is no longer available. With typical residential PV ramp rates (<1% per second), 1-minute and 5-minute timestep configurations will see modest impact. 15-minute and 60-minute timesteps are more vulnerable because a PV ramp from 0% to 100% of rated power can occur entirely within one timestep, causing the BMS to miss the entire self-consumption opportunity for that step.

---

### Finding 3: Thermal solver `prepare_inputs` build-then-integrate split uses stale infiltration coupling in solve path
**Severity**: low

**Description**: In `collect_and_solve` (solver_feedback.rs:52-59), the `solve_ideal_capacity_for_target` method (stepping.rs:252-344) uses `self.last_u` and `self.last_coupled_lu`, which were set by `prepare_inputs_inner` (stepping.rs:370-388). However, `prepare_inputs_inner` saves and restores `exterior_surface_temps` (line 371-373) to avoid mutating the solver's exterior surface temperature state during the ideal capacity solve. This means that any change in exterior surface temperatures caused by the `build_input_vector` call during prepare (e.g., iterative exterior LWR convergence affecting `exterior_surface_temps`) is discarded after prepare. The final `integrate_inner` call at Step 4 will re-compute these temperatures, potentially producing different boundary conditions than those used during the ideal capacity solve.

**Code Location**: `crates/hares-envelope/src/thermal_solver/stepping.rs:370-373`

**Root Cause**: The exterior surface temperature persistence is intentionally reset after `prepare_inputs` to maintain consistency between the prepare phase (which is used for ideal capacity solving) and the integrate phase. However, this creates a discrepancy: the ideal capacity solve uses surface temperatures from `prepare_inputs`, while the final integration uses surface temperatures from `integrate_inner`, which may differ due to different port contributions (occupancy gains only vs. occupancy + equipment gains).

**Impact**: Minor — typically less than 1% of heating/cooling capacity for well-insulated buildings. The discrepancy primarily affects exterior LWR exchange at windows, where surface temperature differences of 1-2°C could shift the net radiative balance by tens of watts. For residential simulation scales, this is within engineering tolerance.

---

### Finding 4: Electrical solver resolve ordering relative to humidity/thermal solvers is correct but undocumented
**Severity**: low

**Description**: The electrical solver runs concurrently with humidity and fluid solvers (Step 4, lines 2577-2594) but the ordering *between* these solvers within the step is not explicitly documented or justified. Currently:
1. Thermal solver `integrate()` (line 2553-2554)
2. Humidity solver `resolve()` (line 2577-2582)
3. Electrical solver `resolve()` (line 2583-2588)
4. Fluid solver `resolve()` (line 2589-2594)

Thermal → humidity ordering is justified because the humidity solver reads zone temperatures from the updated `latest_env` (which was updated from the thermal update at line 2557). However, the electrical solver reads only `ports.electrical` and `env.grid.voltage_pu` — neither of which depends on thermal or humidity solver output. The inter-solver ordering is functionally correct (no hidden data dependencies are violated), but the lack of explicit documentation means future solver additions or refactors could inadvertently introduce ordering dependencies.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2550-2628`

**Root Cause**: The solver ordering emerged organically from the codebase evolution rather than from a documented design contract. The `DomainSolver` trait enforces a domain-aware resolve interface but does not specify dependencies between domains.

**Impact**: Currently none — all inter-solver data flow is through `latest_env` which is updated synchronously. Risk is latent: if a future solver reads from a domain that hasn't been updated yet (e.g., electrical solver reading thermal comfort metrics), stale data would silently propagate.

## Summary
- **Total findings**: 4
- **Critical**: 0
- **High**: 1 (single-pass coupling with no within-step convergence)
- **Medium**: 1 (PV generation unavailable for same-step BMS decisions)
- **Low**: 2 (stale surface temps in ideal solve path, undocumented solver ordering)

## Recommendations

1. **Document the single-pass design decision explicitly** in a Solver Feedback Loop Architecture document. State the expected thermal time constant range (e.g., 2-10 hours for typical US residential construction) and justify that a 1-minute timestep provides >120 steps per time constant, making the one-step lag acceptable. Define the construction types and timestep ranges for which the single-pass approach is validated.

2. **Add a residual check between predicted and delivered HVAC capacity.** After the thermal solver `integrate()` at Step 4, compare the ideal capacity computed by `solve_ideal_capacity_for_target` (Step 1e) against the actual HVAC port contribution recorded in `component_gains`. When the difference exceeds a threshold (e.g., 10% of predicted capacity), emit a `tracing::debug!` diagnostic. This provides observational data on the single-pass error without changing the algorithm.

3. **Consider adding an optional within-step PV-BMS re-evaluation pass** for timesteps ≥5 minutes: after PV equipment steps in Step 3b (Independent), run a lightweight BMS actor re-evaluation that can adjust battery charge power based on actual (not predicted) PV generation. This would require the BMS actor to support an `adjust_for_pv(pv_kw: f64)` method that runs after equipment has deposited generation data but before the battery's `step()`. Alternatively, move the electrical solve into a two-phase flow: `prepare_ports` → equipment step → `integrate` where equipment contributions to electrical ports could be observed by later-running equipment through the accumulator pattern already present in `PortSlots`.

4. **Document the domain solver dependency graph** in a comment block near `run_timestep` Step 4. List each solver, what domain data it reads from `latest_env`, and what it writes. This prevents future regressions when solvers are added, removed, or reordered. Example:

```
// Step 4: domain solver resolution
// Dependency order: Thermal → Humidity (reads zone temps from thermal update)
//   → Electrical (reads ports.electrical and env.grid, independent of thermal/humidity)
//   → Fluid (reads ports.fluid and thermal output, independent of electrical)
```

5. **For high-fidelity simulations or validation runs**, consider adding an optional iteration loop (`configure_iterations(max_iter: usize)`) that repeats Steps 1d-4 within a single timestep until the zone temperature change between iterations falls below a convergence threshold. This would bring HARES into alignment with EnergyPlus's predictor-corrector approach for studies requiring tight energy balance closure.

## References / Citations

- EnergyPlus Engineering Reference (2024), "Basis for the Zone and Air System Integration" — describes the predictor-corrector approach for the zone air heat balance.
- EnergyPlus Engineering Reference, "Warmup Convergence" — iterative convergence over weather days with a maximum zone temperature change threshold.
- ANSI/ASHRAE Standard 140-2017, Case 600 — lightweight construction test case with ~3.5 hour time constant used for validation.
- HARES thermal solver `integrate_inner` at `crates/hares-envelope/src/thermal_solver/stepping.rs:392-607` — documents the per-step energy balance closure check and semi-implicit infiltration coupling.
- HARES `run_timestep` at `crates/hares-core/src/dwelling/mod.rs:2209-2815` — complete per-timestep simulation sequence.
