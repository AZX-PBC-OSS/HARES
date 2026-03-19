---
id: HARES-043
title: "hares-core — Dwelling Orchestrator (CRITICAL)"
kind: implement
depends_on: [HARES-041, HARES-042, HARES-014, HARES-015, HARES-016, HARES-017, HARES-018, HARES-019, HARES-037, HARES-038, HARES-011]
files_to_touch:
  - crates/hares-core/src/dwelling.rs
  - crates/hares-core/src/lib.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
`Dwelling` is the central orchestrator of a single-building simulation. It owns all equipment, solvers, the clock, the environment manager, and the recorder, and coordinates the five-step timestep loop described in the architecture. It is the primary integration point for the entire HARES system and must be correct before higher-level crates (fleet, Python bindings) can be built.

## Work to Do
- [ ] Define `DwellingConfig` struct with fields: `hpxml_path: PathBuf`, `schedule_path: PathBuf`, `weather_path: PathBuf`, `sim_config: SimulationConfig`, `overrides: Option<serde_json::Value>`, `bldg_id: i64`. This is the stable contract between the fleet runner (HARES-047) and the dwelling orchestrator.
- [ ] Implement `dwelling.rs`: `Dwelling` struct
  - [ ] Fields: `Vec<Box<dyn Equipment>>`, `ThermalSolver`, `HumiditySolver`, `ElectricalSolver`, `FluidSolver`, `SimClock`, `EnvironmentManager`, `PortSlots`, `StreamingRecorder`, `ChaCha8Rng`, `warnings: Vec<String>`
  - [ ] `Dwelling::take_warnings(&mut self) -> Vec<String>`: drains and returns the accumulated warnings vector
  - [ ] `Dwelling::push_warning(&mut self, msg: String)`: appends a warning; for use by equipment and solvers during simulation
- [ ] Implement the five-step timestep loop (called by both `simulate()` and `step()`)
  - [ ] Step 1: advance `SimClock`, call `EnvironmentManager::update()` to obtain `EnvironmentState`
  - [ ] Step 2: distribute queued control signals to equipment via `ControlDispatcher` — route by equipment name or end-use category; implement `ControlDispatcher` inline in this module
  - [ ] Step 3: equipment update — sort by `ExecutionStage`, run sequentially in two sub-steps:
    - [ ] Step 3a: Non-ideal equipment (scheduled loads, battery, PV, EV, generator) runs in stage order (Independent → Electrical → Thermal) — each writes PortContributions to shared PortSlots. After Stage 1 equipment runs, snapshot the accumulated `PortSlots` totals into a read-only `StageSnapshot` buffer. Stage 2 equipment (Battery, Generator) reads from this snapshot, not from the live `PortSlots`, to prevent Stage 2 from seeing its own contributions as Stage 1 input.
    - [ ] Step 3b: Ideal-capacity HVAC uses the envelope's `solve_for_input(x_prev, u_known, y_target=setpoint, input_index)` to compute the exact heating/cooling power needed to reach the setpoint from the previous-step state. This is the same one-step temperature lag OCHRE uses — acceptable for 30+ minute thermal time constants at 1-minute timesteps. HVAC then writes its PortContributions with the solved capacity.
  - [ ] Step 4: Envelope resolution — `ThermalSolver::resolve()` consumes ALL accumulated thermal ports (including ideal HVAC from 3b) and updates zone temperatures; `ElectricalSolver::resolve()` sums power; `HumiditySolver::resolve()` updates moisture; `FluidSolver::resolve()` balances hydronic loop flow and temperature; iterate all registered `DomainSolver` implementations for any `PortSlots::custom` entries; zero `PortSlots` after resolution
  - [ ] Step 5: record results to `StreamingRecorder`
- [ ] Add `debug_assert` runtime numerical invariants: thermal energy balance, electrical balance, SOC bounds, temperature sanity checks, moisture mass balance
- [ ] Public API
  - [ ] `from_config(config: DwellingConfig) -> Result<Dwelling>`
  - [ ] Optional warm-up period: support `initialization_duration: Option<Duration>` in `DwellingConfig`. If set, run the timestep loop for that duration before the official simulation period using the same schedule and weather, with output recording suppressed. This brings envelope and equipment states to physically realistic starting conditions (matching OCHRE's `initialization_time` kwarg). Note: this feature is an OCHRE-compat requirement — the compat layer (HARES-051) must expose `initialization_time` as a constructor kwarg and map it to this field.
  - [ ] `simulate() -> SimulationResults` — runs all timesteps
  - [ ] `results() -> SimulationResults` — returns accumulated results for the completed simulation (needed by Python binding HARES-049)
  - [ ] `step() -> StepResult` — single timestep for RL/control use
  - [ ] `apply_control(name: &str, signal: ControlSignal)` — queues a control signal
  - [ ] `set_price_signal(signal: PriceSignal)` — stores current price signal accessible to equipment controllers
  - [ ] `set_grid_voltage(voltage_pu: f64)` — updates `GridState::voltage_pu` for HELICS co-simulation (see arch doc `03-control-interfaces.md` HELICS integration pattern)
  - [ ] `telemetry() -> DwellingTelemetry` — current observable state
- [ ] Re-export `Dwelling` and related public types from `lib.rs`

## Files to Touch
- `crates/hares-core/src/dwelling.rs`: new file — `Dwelling` struct, timestep loop, `ControlDispatcher`, public API
- `crates/hares-core/src/lib.rs`: declare and re-export new module

## Measures of Success
- [ ] 24h simulation with a constant 1.0 kW `ScheduledLoad` produces total electric energy = 24.0 kWh ± 0.001 kWh
- [ ] Thermal energy balance holds at every timestep: `|ΣQ_gain - ΔE_storage - Q_loss| < max(1.0 W, 1e-6·|ΣQ_gain|)` — verified by an explicit test assertion, not just `debug_assert`
- [ ] Electrical balance holds: `|P_grid + ΣP_equipment| < 0.001 kW` — explicit test assertion
- [ ] Moisture balance holds: `|Δm_water - Σ(Q_latent·dt/h_fg)| < 1e-6 kg` — explicit test assertion
- [ ] Stage ordering verified by port values: after Stage 1 runs, `PortSlots` contains PV contribution; Battery (Stage 2) reads from the `StageSnapshot` (not live slots)
- [ ] Note: stage ordering tests using real Battery + PV equipment should be in HARES-061 Phase 3 gate (not here) since HARES-043 does not depend on HARES-028/029
- [ ] `take_warnings()` returns a non-empty `Vec<String>` after a simulation that triggers a temperature-range clamp
- [ ] Equipment executes in `Independent → Electrical → Thermal` stage order, verifiable via a test hook or log
- [ ] Control signal routed by equipment name reaches the correct device
- [ ] Control signal routed by end-use category reaches all matching devices
- [ ] Energy balance `debug_assert`s do not fire during a clean simulation run
- [ ] Temperature values remain within sanity bounds throughout the run

## Architecture Alignment Notes
- **`StageSnapshot` mechanism**: This is a deliberate refinement over the architecture. Stage 1 accumulation is frozen into a read-only snapshot before Stage 2 runs, preventing Stage 2 equipment (Battery, Generator) from seeing its own output as Stage 1 input. This is critical for correct self-consumption behavior — without it, a battery could "see" its own discharge as available generation and enter a feedback loop. The architecture doc `01-sim-core-and-solver.md` should be updated to include this mechanism.

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
