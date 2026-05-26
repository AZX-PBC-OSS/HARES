# Fluid domain solver: loop resolution, temperature propagation, mass conservation
**Review ID**: solver-02
**Category**: solver
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/fluid_solver.rs` (425 lines)
- `crates/hares-types/src/fluid.rs` (141 lines)
- `crates/hares-types/src/ports.rs` (FluidAccumulator, PortContribution::Fluid sections)
- `crates/hares-equipment/src/hvac/boiler.rs` (temperature propagation callsite)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/Plant/LoopSide.cc` — `ResolveParallelFlows`, `UpdatePlantMixer`, `CheckPlantConvergence`
- `vendors/EnergyPlus/src/EnergyPlus/Plant/LoopSide.hh` — `HalfLoopData` member declarations
- `vendors/EnergyPlus/src/EnergyPlus/Plant/Loop.cc` — `CheckLoopExitNode` (mass flow consistency), `CalcUnmetPlantDemand` (Q=ṁ·cp·ΔT)
- `vendors/EnergyPlus/src/EnergyPlus/Plant/Loop.hh` — `PlantLoopData` including MinTemp/MaxTemp/fluid properties
- `vendors/EnergyPlus/src/EnergyPlus/Plant/SplitterData.hh` — splitter topology
- `vendors/EnergyPlus/src/EnergyPlus/Plant/MixerData.hh` — mixer topology

## Findings

### Finding 1: No mass conservation enforcement at node level
**Severity**: critical
**Description**: The solver sums flow rates from all equipment contributions on a loop via `entries.iter().map(|e| e.total_flow_kg_s).sum()` (`fluid_solver.rs:132`) but performs no mass-balance check. If a boiler reports 0.5 kg/s and a distribution coil on the same loop reports 0.3 kg/s, the aggregated `total_flow` is 0.8 kg/s with no verification that inflows equal outflows. There is no topological model of branches, nodes, splitters, or mixers, so continuity is unenforced.
**Code Location**: `fluid_solver.rs:132` — flow summation without conservation check
**Root Cause**: The solver operates on flat, per-equipment `FluidAccumulator` entries with no node-graph representation. Each equipment independently declares its own flow rate.
**Impact**: Mass can be created or destroyed at will. A hydronic loop in the simulation could produce nonsensical results (e.g., a boiler circulating 2.0 kg/s while the distribution system sees only 0.5 kg/s) with no error or warning.
**Reference**: EnergyPlus `CheckLoopExitNode` (`Loop.cc:244-265`) enforces `std::abs(Outlet.MassFlowRate - Inlet.MassFlowRate) < MassFlowTolerance` and emits a recurring warning when violated. The `ResolveParallelFlows` function (`LoopSide.cc:1279-1682`) resolves splitter/mixer flow to ensure mass conservation across all parallel branches.

### Finding 2: Parallel equipment flow is summed, not split by branch topology
**Severity**: high
**Description**: When multiple equipment report contributions on the same `LoopId`, the solver treats their flow rates as additive (`fluid_solver.rs:132`). There is no splitter, no parallel-branch model, no priority-based allocation, and no pump-curve-based flow distribution. A loop with a boiler and heat pump in parallel would have their flows simply summed, as if they operate in series. There is no mechanism to limit total loop flow to the pump's capacity.
**Code Location**: `fluid_solver.rs:125-171` — per-loop aggregation; all entries treated uniformly
**Root Cause**: The v1 solver has no hydraulic network topology model — no branches, no splitter data, no mixer data. The `FluidAccumulator` port system provides only per-equipment flow/temperature tuples with no topological constraints.
**Impact**: Cannot model realistic plant designs with parallel equipment. Flow distribution between parallel branches always produces unphysical results (series-equivalent summation rather than flow splitting).
**Reference**: EnergyPlus `ResolveParallelFlows` (`LoopSide.cc:1279-1352`) implements a multi-pass algorithm: (1) satisfy active branch requests, (2) distribute remaining to passive branches proportional to max-avail, (3) allocate to bypass, (4) distribute excess to active branches. When available flow is insufficient, it allocates proportionally by request fraction (`LoopSide.cc:1624-1660`). Splitter and mixer topology is modeled explicitly (`SplitterData.hh`, `MixerData.hh`).

### Finding 3: Net power conflates source and load contributions
**Severity**: high
**Description**: The `net_power_w` computation sums `cp * flow * (supply_temp - return_temp)` across all entries on the loop (`fluid_solver.rs:133-140`). A boiler contributes positive power (heating the fluid), while a distribution coil also contributes positive power (extracting heat). These are algebraically opposite but both produce positive values because the ΔT sign convention is the same for both equipment types. Consider: a boiler with flow 0.5, supply 60°C, return 50°C → +20930 W; a radiant floor with flow 0.5, supply 50°C, return 40°C → +20930 W. `net_power_w` = +41860 W, but the loop's net power change should be near zero (boiler heat injected ≈ distribution heat extracted).
**Code Location**: `fluid_solver.rs:133-140`
**Root Cause**: The `PortContribution::Fluid` structure uses a single sign convention (`supply_temp_c - return_temp_c`) that does not distinguish between heat sources (injectors) and heat sinks (extractors). Both types report positive ΔT in their respective flow directions.
**Impact**: `net_power_w` is physically meaningless for loops with both sources and sinks. The value is used as a first-class output on `FluidLoopState` (`fluid.rs:12`) and consumed by downstream equipment (e.g., `loop_return_temp_c` in `boiler.rs:695-707` reads from this state, though currently only reads `mean_return_temp_c`).
**Reference**: EnergyPlus `CalcUnmetPlantDemand` (`Loop.cc:90-213`) distinguishes between heating and cooling demand and computes `LoadToLoopSetPoint = MassFlowRate * Cp * (SetPointTemp - InletTemp)`, where the sign of the result determines heating vs. cooling. Plant loads are reported separately as `CoolingDemand` and `HeatingDemand` (`Loop.hh:165-166`).

### Finding 4: No within-timestep convergence iteration
**Severity**: medium
**Description**: Equipment reads the previous timestep's `mean_return_temp_c` from the fluid domain state in `EnvironmentState` (`boiler.rs:226-227`, `loop_return_temp_c` at `boiler.rs:695-706`) to compute its supply temperature via `return_temp_c + thermal_output_w / (flow * cp)` (`boiler.rs:228-229`). The fluid solver then averages all contributions. There is no iterative re-simulation loop within a single timestep to converge supply/return temperatures across equipment that depend on each other's temperatures.
**Code Location**: Execution order in `dwelling/mod.rs:2685-2690` (fluid solver runs once, after all equipment). Temperature reads happen in equipment step, e.g., `boiler.rs:226-227`.
**Root Cause**: Single-pass architecture where equipment runs, then domain solvers run, then state is merged. Equipment sees only the previous timestep's fluid state. No predictor-corrector or iterative loop within a timestep.
**Impact**: Temperatures lag by one timestep. In rapidly changing conditions, equipment may compute supply temperatures from stale return temperatures, producing physically inconsistent loop temperatures. For slow-changing loops with large thermal mass this is acceptable; for fast dynamics it can cause significant errors.
**Reference**: EnergyPlus `DoFlowAndLoadSolutionPass` (`LoopSide.cc:1212-1277`) performs two passes: an "unlocked" flow pass where components request flow, then a "locked" pass after `ResolveParallelFlows` has set corrected branch flows. Each half loop (`HalfLoopData::solve`, `LoopSide.cc:70-173`) can trigger re-simulation of the other half loop. Convergence is checked via `CheckPlantConvergence` (`LoopSide.cc:352-407`) which verifies inlet/outlet temps and flows have stabilized across HVAC iterations. Up to `MaxPlantLoopIterations` iterations are allowed.

### Finding 5: No unphysical temperature bounds checking
**Severity**: medium
**Description**: The solver outputs `mean_supply_temp_c` and `mean_return_temp_c` with no validation against physical limits. Temperatures below 0°C (freezing for pure water), above 100°C (boiling at atmospheric pressure for pure water), or exceeding material limits are written to the loop state without detection.
**Code Location**: `fluid_solver.rs:161-170` — FluidLoopState is built without any temperature validation
**Root Cause**: The solver has no `MinTemp`/`MaxTemp` configuration per loop and no bounds-checking logic in the resolve path.
**Impact**: Unphysical simulated temperatures can propagate downstream through equipment that reads `mean_return_temp_c` (`boiler.rs:695-706`), leading to cascading errors. Sub-freezing water temperatures could be reported without warning, invalidating COP calculations or heat transfer physics that assume liquid water.
**Reference**: EnergyPlus `PlantLoopData` (`Loop.hh:125-126`) defines `Real64 MinTemp` and `Real64 MaxTemp` per loop, with `MinTempErrIndex` / `MaxTempErrIndex` for recurring error reporting when temperatures violate these bounds. These are initialized with defaults and configurable.

### Finding 6: Fluid-type agnostic specific heat capacity
**Severity**: low
**Description**: The solver uses a single configurable `cp_water_j_kg_k` (default 4186.0 J/kg·K) for all fluid types including Glycol and Refrigerant (`fluid_solver.rs:15-16`, line 136). Glycol-water mixtures have significantly lower specific heats (e.g., 40% ethylene glycol at 60°C has cp ≈ 3550 J/kg·K). Refrigerants have entirely different thermodynamic properties governed by pressure-enthalpy relations. The `FluidType` enum (`equipment.rs:242-246`) distinguishes Water, Glycol, and Refrigerant, but this distinction is unused in energy calculations.
**Code Location**: `fluid_solver.rs:135-139` — `self.config.cp_water_j_kg_k` used regardless of `e.fluid_type`
**Root Cause**: The `FluidSolverConfig` has only one cp value; no per-fluid-type property table.
**Impact**: Glycol-loop net power and ΔT calculations will be systematically biased (e.g., ~15% overestimate of temperature change for a given heat rate on a glycol loop). Refrigerant loops will produce meaningless results since refrigerant phase-change behavior requires enthalpy, not single-phase cp.
**Reference**: EnergyPlus `PlantLoopData` (`Loop.hh:115-116`) holds pointers to `Fluid::GlycolProps *glycol` and `Fluid::RefrigProps *steam`. `CalcUnmetPlantDemand` (`Loop.cc:136-137`) calls `this->glycol->getSpecificHeat(state, TargetTemp, ...)` which uses temperature-dependent property lookups. Water and glycol share the same property lookup path but with different fluid indices.

### Finding 7: Zero-flow zombie state persists stale temperatures indefinitely
**Severity**: low
**Description**: When `total_flow.abs() <= MIN_FLOW_KG_S` (1e-12 kg/s), the solver falls back to `last_known_temps` (`fluid_solver.rs:142-147`), which is set from the previous non-zero flow step. If the loop stops flowing for an extended period (e.g., summer months for a heating-only loop), the reported temperatures remain frozen at their last value. Real loops would slowly drift toward ambient temperature.
**Code Location**: `fluid_solver.rs:142-147`
**Root Cause**: There is no thermal decay model for stagnant loops. The `last_known_temps` map (`fluid_solver.rs:32`) stores the last value indefinitely with no time-based decay to ambient.
**Impact**: Stale temperatures could mislead diagnostics or optimizations that assume active temperature readings. Equipment that re-activates after a long idle period would see the old return temperature instead of ambient.
**Reference**: EnergyPlus doesn't have this specific issue since it uses fixed flow rates during off periods and handles stagnation through node history and plant capacitance tracking.

## Summary
- Total findings: 7
- Critical: 1 (mass conservation not enforced)
- High: 2 (parallel flow splitting, net power conflation)
- Medium: 2 (no convergence iteration, no temperature bounds)
- Low: 2 (fluid-agnostic cp, zombie state)

## Recommendations
1. **Implement node-level mass conservation**: Add a hydraulic network model with nodes, branches, splitters, and mixers. Verify that sum of inflows equals sum of outflows at each node within a tolerance. This is the foundational requirement for any meaningful hydronic simulation.
2. **Implement flow-splitting logic**: For loops with multiple parallel equipment, implement flow distribution based on branch resistance or pump curves (similar to EnergyPlus `ResolveParallelFlows`), not additive summation.
3. **Fix net power sign convention**: Distinguish heat source contributions from heat sink contributions. Use signed power values or separate source/sink fields so `net_power_w` reflects the loop's actual energy balance.
4. **Add temperature bounds**: Per-loop `min_temp_c` and `max_temp_c` configuration with error reporting when exceeded. Prevent water loops from reporting sub-freezing or super-boiling temperatures.
5. **Add per-fluid heat capacity**: Expand `FluidSolverConfig` to include distinct cp values for Water, Glycol, and Refrigerant. Use fluid type from `FluidAccumulator` in energy calculations.
6. **Consider within-timestep iteration**: If temporal accuracy becomes important, add a predictor-corrector pass or at minimum ensure equipment reads its own return temperature contribution rather than the loop-averaged value.

## References / Citations
- EnergyPlus Engineering Reference, "Plant/Condenser Loops" — Flow solver description
- EnergyPlus `HalfLoopData::ResolveParallelFlows` (`LoopSide.cc:1279-1682`) — five-pass flow splitting algorithm
- EnergyPlus `HalfLoopData::UpdatePlantMixer` (`LoopSide.cc:2202-2275`) — mass-flow-weighted mixer outlet temperature
- EnergyPlus `PlantLoopData::CheckLoopExitNode` (`Loop.cc:215-268`) — mass flow continuity check
- EnergyPlus `PlantLoopData::CalcUnmetPlantDemand` (`Loop.cc:90-213`) — `Load = MassFlowRate * Cp * DeltaTemp`
- Incropera, F. et al., *Fundamentals of Heat and Mass Transfer* — mass conservation and energy balance on control volumes
