---
id: HARES-042
title: "hares-core — Environment Manager"
kind: implement
depends_on: [HARES-041, HARES-033, HARES-034, HARES-035, HARES-007]
files_to_touch:
  - crates/hares-core/src/environment.rs
  - crates/hares-core/src/lib.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
At every timestep the simulation must assemble a complete `EnvironmentState` before any equipment runs. This involves indexing weather data, computing per-surface solar irradiance, loading schedule values, incorporating zone temperature feedback from the previous envelope solve, and setting default grid state. `EnvironmentManager` owns these data sources and produces a ready-to-consume `EnvironmentState` on each call to `update()`.

## Work to Do
- [ ] Implement `environment.rs`: `EnvironmentManager` struct
  - [ ] Owns `WeatherTimeSeries`, `ScheduleTimeSeries`, `WeatherMeta` (for location and timezone), and building surface geometry (`Vec<SurfaceGeometry>` — orientation, tilt, area per surface, sourced from parsed `Building` struct)
  - [ ] When constructing `EnvironmentManager`, call `schedule.resample(time_res.as_secs() as u32)` on the parsed `ScheduleTimeSeries`. Fail with error if resampling fails (e.g., non-integer divisor).
  - [ ] Initial zone temperatures: initialise from `HPXML Building/BuildingDetails/BuildingSummary/Site/...` indoor design temperature if present; fall back to 20 °C when absent
  - [ ] Method `update(clock: &SimClock, zone_states: &[ZoneState]) -> EnvironmentState`
  - [ ] Step 1: index into weather data using `clock.current_step` to retrieve current ambient conditions; populate `WeatherState.wind_dir_deg` from the `wind_dir_deg` column of `WeatherTimeSeries`
  - [ ] Step 2: compute per-surface solar irradiance using `hares-physics` solar functions for each building surface orientation; `WeatherMeta` provides latitude, longitude, and timezone for solar angle calculations
  - [ ] Step 3: load schedule values for the current timestep from `ScheduleTimeSeries`
  - [ ] Step 4: merge `zone_states` feedback (zone temperatures from the previous envelope solve)
  - [ ] Step 5: set grid state defaults (`voltage = 1.0 p.u.`, `frequency = 60 Hz`) unless a grid override is active
  - [ ] `set_grid_override(grid: GridState)`: public method that sets an override `GridState`; subsequent calls to `update()` use this override instead of defaults until `clear_grid_override()` is called
  - [ ] `clear_grid_override()`: public method that restores default grid state
  - [ ] Return a fully populated `EnvironmentState` ready for equipment `step()` calls
- [ ] Re-export `EnvironmentManager` from `lib.rs`

## Files to Touch
- `crates/hares-core/src/environment.rs`: new file — `EnvironmentManager` struct and `update()` impl
- `crates/hares-core/src/lib.rs`: declare and re-export new module

## Measures of Success
- [ ] At timestep 0, `env.weather.outdoor_temp_c` equals the first EPW record dry-bulb to within 1e-6 °C
- [ ] At timestep 59 (1-hour mark for 1-min resolution), weather values match the replicated first EPW hour record
- [ ] `WeatherState.wind_dir_deg` is populated from `WeatherTimeSeries` on every call to `update()`
- [ ] Schedule value at timestep 0 for a known column matches the first row of the fixture CSV to within 1e-6
- [ ] Per-surface solar irradiance for a south-facing surface at solar noon exceeds north-facing surface irradiance
- [ ] Zone state feedback: after ThermalSolver updates zone temp to 22 °C, next `EnvironmentManager::update()` returns `env.zones[0].temperature_c == 22.0`
- [ ] Grid state defaults to `voltage_pu = 1.0`, `frequency_hz = 60.0` when no override is set
- [ ] After `set_grid_override(GridState { voltage_pu: 0.95, .. })`, next `update()` returns `grid.voltage_pu = 0.95` and `grid.frequency_hz` matches the supplied override value
- [ ] After `clear_grid_override()`, `update()` returns `grid.voltage_pu = 1.0`
- [ ] An HPXML fixture with no indoor design temperature produces initial zone temperatures of 20 °C

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
