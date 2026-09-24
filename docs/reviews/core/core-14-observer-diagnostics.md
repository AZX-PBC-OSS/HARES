# Observer/diagnostics framework completeness and performance impact
**Review ID**: core-14
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/observer.rs crates/hares-core/src/observer_capture.rs crates/hares-core/src/diagnostics.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: high]
**Description**: The `diagnostics` module (`crates/hares-core/src/diagnostics.rs`) provides `capture()`, `write_header()`, and `write_row()` functions for per-timestep CSV diagnostic output but is never invoked from the simulation loop. The module doc comment states diagnostic output activates at `output_verbosity >= 4`, but no code exists that checks this threshold and calls these functions. Only the `EnvelopeDiag` type is consumed (in `crates/hares-core/tests/port_radiant_diag.rs:8`). The module is compiled into the library as dead code. There are no automatic post-simulation diagnostic checks for excessive unmet hours, equipment short-cycling, temperature excursions below freezing, or simultaneous heating and cooling. The observer framework captures rich per-step data but has no diagnostic analysis layer to consume it.

**Code Location**: `crates/hares-core/src/diagnostics.rs:1–150` (entire module); `crates/hares-core/src/lib.rs:8`
**Root Cause**: The diagnostic capture/writer infrastructure was implemented but never wired into the simulation loop. The `output_verbosity` field exists on the `Dwelling` struct (`crates/hares-core/src/dwelling/mod.rs:744`) and controls output schema verbosity, but no path calls `diagnostics::capture()` or the CSV writer from `run_timestep`.
**Impact**: Users and developers have no built-in automated health checks on simulation results. Common issues like equipment short-cycling, excessive unmet loads, or temperature excursions must be diagnosed manually from raw output data. This severely limits the practical utility of the observer when used for debugging.

### Finding 2: [Severity: high]
**Description**: Custom domain solvers have no observation capture point. The `run_timestep` loop captures `SolverCapture` for the four built-in solvers (thermal, humidity, electrical, fluid) at `crates/hares-core/src/dwelling/mod.rs:2598–2607`, but the subsequent loop that resolves custom domain solvers (`crates/hares-core/src/dwelling/mod.rs:2619–2628`) has no `#[cfg(feature = "observe")]` instrumentation. Any domain added via the `DomainSolver` trait is invisible to the observer, creating a blind spot for user-defined physics domains (e.g., contaminant transport, radiant panel zones).

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2619–2628` vs `crates/hares-core/src/dwelling/mod.rs:2598–2607`
**Root Cause**: The custom solver resolution loop was added after the observer capture points were defined; the observer framework's `PhaseSnapshots` struct (`crates/hares-core/src/observer.rs:28–35`) has no slot for custom domain outputs.
**Impact**: A diagnostic tool that cannot observe a particular solver domain creates a blind spot. Contaminant concentrations, custom-domain state variables, or user-defined physics results are untraceable even with the observer enabled.

### Finding 3: [Severity: medium]
**Description**: Actor decision state is not observable. The observer framework captures dispatched control signals (`DispatchCapture`) and equipment state, but there is no `post_actors` phase in `PhaseSnapshots` (`crates/hares-core/src/observer.rs:28–35`). Actors such as `BatteryManagementActor`, `EvDriverActor`, `OccupantActor`, `IdealThermostat`, `DrComplianceActor`, and `SolverFeedbackActor` each maintain internal decision state (scheduler look-ahead windows, EV trip plans, DR compliance state, thermostat comfort range adjustments) that is never captured by the observer. Only the final control signals they emit are visible.

**Code Location**: `crates/hares-core/src/observer.rs:29–35` (missing `post_actors` field); `crates/hares-core/src/dwelling/mod.rs:2341–2363` (actor decision loop, no observer instrumentation)
**Root Cause**: The observer was designed around equipment-centric phase boundaries. Actor state capture was not included in the original design, despite several actors having telemetry fields annotated with `/// Actor telemetry: observable decision state for diagnostics.`
**Impact**: Debugging actor-driven behaviors (e.g., why a battery chose to charge instead of discharge, why a thermostat held a setpoint past the schedule transition) requires ad-hoc logging or stepping through actor code manually. The observer cannot answer actor-level "why" questions.

### Finding 4: [Severity: medium]
**Description**: When observation is active, every equipment step triggers two full `PortSlots::clone()` calls: an initial clone at the start of the equipment phase (`crates/hares-core/src/dwelling/mod.rs:2416–2419`) and a re-clone after each equipment's step to preserve the accumulator state for the next equipment's diff (`crates/hares-core/src/dwelling/mod.rs:2453`). `PortSlots` contains multiple `Vec` allocations (thermal accumulators, fluid accumulators, fuel HashMap). For a dwelling with 20 pieces of equipment at a 1-minute timestep over one year (525,600 steps), this produces approximately 21 * 525,600 = 11 million full `PortSlots` clones distributed across ~11 million heap allocations. The allocation churn alone could dominate runtime for year-long sub-hourly simulations.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2416–2419`, `crates/hares-core/src/dwelling/mod.rs:2453`
**Root Cause**: The observer needs before-and-after accumulator snapshots to compute per-equipment port contributions via `diff_ports()`. The current approach clones the full port state on each transition rather than snapshotting only the diff-relevant accumulator fields incrementally.
**Impact**: A simulation that takes 30 seconds in release mode without observation could take minutes with observation enabled at sub-hourly resolution, potentially rendering the observer prohibitive for production use. The per-equipment clone loop is O(N * M) where N=equipment count, M=PortSlots allocation size.

### Finding 5: [Severity: medium]
**Description**: No explicit `disable_observer()` method exists. The observer can be enabled at runtime via `enable_observer(capacity)` (`crates/hares-core/src/dwelling/mod.rs:1955–1958`), but there is no corresponding method to set `observer_buf` back to `None`. Draining the buffer (`drain_observations()`) retrieves snapshots but leaves the buffer in-place with zero entries — subsequent steps will still execute the full observation capture path (cloning ports, diffing, allocating observation structs) and push snapshots into the empty buffer.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1952–1974`
**Root Cause**: The observer was designed with an enable-only lifecycle. The `observer_buf` field is `Option<ObserverBuffer>`, which naturally supports `None` disabling, but no public API exposes this.
**Impact**: Once observation is enabled during a simulation run, it cannot be turned off. For batch simulations where only a subset of dwellings or timesteps need observation (e.g., first 1000 steps for debugging, then release), there is no way to stop the performance overhead mid-run.

### Finding 6: [Severity: low]
**Description**: The `observing` boolean (`crates/hares-core/src/dwelling/mod.rs:2285`) is computed once per step as `self.observer_buf.is_some()`, then reused for guard checks throughout the step. When `observing` is `false`, the capture code is skipped via `if observing { ... }` branches, which is a single boolean check. However, the compiler cannot eliminate these branches at compile time because `observer_buf` is a runtime field. The feature flag (`#[cfg(feature = "observe")]`) correctly eliminates all observer code at compile time, but when the feature is enabled and the buffer is simply not allocated, the hot path still evaluates ~8 boolean checks per step. This is negligible for most simulations but worth noting for the "zero overhead when disabled" claim in `observer.rs:1-5`.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2285`; `crates/hares-core/src/observer.rs:1–5`
**Root Cause**: The file-level doc comment in `observer.rs` claims "zero-cost" and "compiler eliminates every observation site" — this is true only when the `observe` feature flag is off. When the feature is on but the buffer is `None`, the check overhead is ~1-2 nanoseconds per guard, which is functionally zero for practical purposes but not compile-time-zero.
**Impact**: Minimal. The branch predictor will perfectly predict `observing == false` after the first iteration. The claim in the module docs is slightly misleading but not incorrect for the common case (feature off entirely).

### Finding 7: [Severity: low]
**Description**: `FuelType::Electric` is excluded from per-equipment contribution tracking in `diff_ports()` (`crates/hares-core/src/observer_capture.rs:77–81`) but included in the full-port snapshot in `capture_ports()` (`crates/hares-core/src/observer_capture.rs:137–143`). This means per-equipment fuel contribution captures only `[Gas, Propane, Oil]`, omitting electric fuel-tracking from the equipment-level diff. Electric consumption is still captured via the dedicated `electrical_load_kw` field, so this does not cause data loss — but it creates an asymmetry where `EquipmentContribution::fuel_consumption_w` has a different fuel type set than `PortsCapture::fuel_consumption_w`. A consumer of the observation data might incorrectly assume Electric fuel consumption is zero for individual equipment when it is actually aggregated into the electrical accumulator.

**Code Location**: `crates/hares-core/src/observer_capture.rs:77–81` vs `crates/hares-core/src/observer_capture.rs:137–143`
**Root Cause**: `FuelAccumulator` tracks `Electric` fuel separately from `ElectricalAccumulator`. The diff logic skips it because the electrical accumulator already captures this value with different units (kW vs W), but this distinction is not documented.
**Impact**: Low risk of misinterpretation. Consumers who iterate `fuel_consumption_w` exhaustively may miss electric fuel flows, but the dedicated electrical fields in `EquipmentContribution` cover the same data.

## Summary
- Total findings: 7
- Critical: 0 / High: 2 / Medium: 3 / Low: 2

## Recommendations
1. Wire `diagnostics::capture()` into the `run_timestep` loop when `output_verbosity >= 4`, or remove the dead code. Add post-hoc diagnostic checks for excessive unmet hours, equipment short-cycling (mode change frequency), temperature excursions below freezing, and simultaneous heating and cooling from the observer buffer data.
2. Add a `post_custom_solvers` slot to `PhaseSnapshots` and instrument the custom solver resolution loop (`crates/hares-core/src/dwelling/mod.rs:2619–2628`) with observer capture.
3. Add a `post_actors` phase to `PhaseSnapshots` and capture actor telemetry during the actor decision loop (`crates/hares-core/src/dwelling/mod.rs:2341–2363`).
4. Add a `disable_observer()` method that sets `observer_buf` to `None` and drops the buffer.
5. Optimize the per-equipment port snapshotting in the observer path: instead of cloning the full `PortSlots` per equipment, snapshot only the accumulator subset needed for diff computations (thermal `Vec`, electrical accumulator, fuel accumulator), or use a pre-allocated scratch buffer and swap semantics.
6. Document the `FuelType::Electric` exclusion in `diff_ports()` to avoid consumer confusion about the asymmetry between `EquipmentContribution::fuel_consumption_w` and `PortsCapture::fuel_consumption_w`.

## References / Citations
- `crates/hares-core/src/observer.rs:1–5` — zero-cost claim in module docs
- `crates/hares-core/src/observer.rs:28–35` — `PhaseSnapshots` struct (6 phase slots, no actors/custom solvers)
- `crates/hares-core/src/dwelling/mod.rs:2285` — `observing` boolean computed once per step
- `crates/hares-core/src/dwelling/mod.rs:2416–2453` — per-equipment PortSlots clone loop
- `crates/hares-core/src/dwelling/mod.rs:2598–2628` — solver capture vs. custom solver blind spot
- `crates/hares-core/src/dwelling/mod.rs:2619–2628` — custom domain solver loop (no observer instrumentation)
- `crates/hares-core/src/diagnostics.rs:1–150` — dead diagnostic CSV module
- `crates/hares-core/src/diagnostics.rs:3–4` — `output_verbosity >= 4` claim with no implementation
- `crates/hares-core/src/observer_capture.rs:77–81` — Electric fuel type excluded from equipment diff
- `crates/hares-core/src/observer_capture.rs:137–143` — Electric fuel type included in port snapshot
