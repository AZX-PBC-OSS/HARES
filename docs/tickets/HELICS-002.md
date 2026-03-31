---
id: HELICS-002
title: "Add Fleet::step() to Rust fleet crate"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-fleet/src/fleet.rs
  - crates/hares-fleet/src/lib.rs
references:
  - docs/tickets/HARES-065.md
  - docs/tickets/HARES-047.md
  - crates/hares-core/src/dwelling/mod.rs
verification:
  - cargo check -p hares-fleet
  - cargo test -p hares-fleet
  - cargo clippy -p hares-fleet -- -D warnings
---

## Background/Context
HELICS co-simulation requires stepping all dwellings in a fleet one timestep at a time, synchronized by the HELICS broker. Currently `Fleet` only has `simulate()` which runs all dwellings to completion — there is no per-timestep interface. A `SteppableFleet` is needed that owns initialized `Dwelling` instances and advances them one step at a time using Rayon parallelism.

The existing `Fleet` constructs dwellings inside `run_entry()` via `SimulationEngine::run()` which builds and fully simulates in one call. `SteppableFleet` needs a different lifecycle: construct dwellings once at init, then call `step()` repeatedly.

## Work to Do
- [ ] Add `SteppableFleet` struct to `fleet.rs` that owns `Vec<Dwelling>` (constructed and initialized, not yet simulated)
- [ ] `SteppableFleet::from_configs(configs: Vec<DwellingConfig>, n_threads: usize) -> Result<(Self, Vec<DwellingBuildError>)>`: constructs and initializes all dwellings. Construction may fail per-dwelling — return `(fleet_of_successful_dwellings, vec_of_per_dwelling_errors)`. If ALL dwellings fail, return `Err`. Each `DwellingBuildError` carries `bldg_id` and error message. This allows the caller to proceed with partial success (e.g., a fleet of 100 where 2 fail to parse HPXML).
- [ ] `SteppableFleet::step(&mut self) -> Vec<Result<StepResult, SimError>>`: advances all dwellings one timestep in parallel using Rayon. Each dwelling's `step()` is called within `par_iter_mut()`. Panic isolation per dwelling (same pattern as `simulate_parallel`)
- [ ] `SteppableFleet::set_grid_voltage(&mut self, dwelling_index: usize, voltage_pu: f64)`: sets grid voltage for a specific dwelling before the next step
- [ ] `SteppableFleet::set_grid_voltage_all(&mut self, voltage_pu: f64)`: sets the same grid voltage for all dwellings
- [ ] `SteppableFleet::apply_control(&mut self, dwelling_index: usize, name: &str, signal: ControlSignal)`: queues a control signal for a specific dwelling
- [ ] `SteppableFleet::telemetry(&self, dwelling_index: usize) -> DwellingTelemetry`: returns telemetry for one dwelling
- [ ] `SteppableFleet::len(&self) -> usize` and `is_empty(&self) -> bool`
- [ ] `SteppableFleet::is_finished(&self) -> bool`: returns true when all dwellings have reached the end of their simulation horizon
- [ ] `SteppableFleet::time_res_s(&self) -> f64`: returns the timestep resolution in seconds (from the first dwelling's config). Required by HELICS-005 to drive the federate time loop.
- [ ] `SteppableFleet::total_steps(&self) -> u64`: returns total number of timesteps in the simulation horizon
- [ ] `SteppableFleet::current_step(&self) -> u64`: returns the current step index (0-based, increments after each `step()` call)
- [ ] Re-export `SteppableFleet` from `lib.rs`
- [ ] Tests: construct 3 dwellings, step all once, verify all return `Ok(StepResult)` with valid timestamps; step to completion; verify `is_finished()` transitions

## Files to Touch
- `crates/hares-fleet/src/fleet.rs`: add `SteppableFleet` struct with step/control/telemetry methods
- `crates/hares-fleet/src/lib.rs`: re-export `SteppableFleet`

## Measures of Success
- [ ] `SteppableFleet::step()` advances all dwellings exactly one timestep in parallel
- [ ] Per-dwelling panic isolation: one dwelling panicking does not crash others
- [ ] `set_grid_voltage()` and `apply_control()` affect the next `step()` for the targeted dwelling
- [ ] `is_finished()` returns `true` after all timesteps have been consumed
- [ ] No allocations in the hot step loop beyond what `Dwelling::step()` itself requires

## Verification
- [ ] `cargo check -p hares-fleet` passes
- [ ] `cargo test -p hares-fleet` passes
- [ ] `cargo clippy -p hares-fleet -- -D warnings` passes
