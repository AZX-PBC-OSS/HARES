---
id: HARES-046
title: "hares-core — Engine (Batch Runner)"
kind: implement
depends_on: [HARES-044, HARES-039]
files_to_touch:
  - crates/hares-core/src/engine.rs
  - crates/hares-core/src/lib.rs
  - crates/hares-core/tests/engine.rs
references:
  - docs/architecture/08-operations.md
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
`SimulationEngine` is the entry point for CLI and batch use cases where no Python layer is involved. It wraps the `Dwelling` construction and `simulate()` call behind a single `run()` method, collects structured results and timing metrics, and surfaces warnings without aborting the run.

## API Contract
- [ ] `pub struct SimulationEngine`
  - [ ] `pub fn new() -> Self`
  - [ ] `pub fn run(&self, config: DwellingConfig) -> anyhow::Result<SimulationResults>`
- [ ] `pub enum SimStatus { Ok, Flagged(String), Failed(String) }`
- [ ] `pub struct SimulationResults`
  - [ ] `pub timeseries_path: Option<PathBuf>`
  - [ ] `pub timeseries: Option<Vec<RecordBatch>>`
  - [ ] `pub metrics: SimulationMetrics`
  - [ ] `pub warnings: Vec<String>`
  - [ ] `pub status: SimStatus`
  - [ ] `pub elapsed: std::time::Duration`
- [ ] `pub struct KernelTimer`
  - [ ] `pub fn start(kernel_name: &'static str) -> Self`
  - [ ] `pub fn stop(self)`

## Work to Do
- [ ] Implement `engine.rs`: `SimulationEngine`, `SimStatus`, `SimulationResults`, and `KernelTimer`
  - [ ] `SimulationEngine::run`
    - [ ] Read HPXML, schedule, and weather paths from `DwellingConfig` and validate each path exists before construction. Return descriptive `Err` values that include the missing path and field name.
    - [ ] Construct `Dwelling` via `Dwelling::from_hpxml(...)` with timing fields from `SimulationConfig`.
    - [ ] Wrap `Dwelling::simulate()` in `std::panic::catch_unwind(AssertUnwindSafe(...))`; map panics to `SimulationResults { status: SimStatus::Failed(msg), ... }` so caller process stays alive.
    - [ ] Pull non-fatal warnings via `Dwelling::take_warnings() -> Vec<String>` and copy to `SimulationResults::warnings`.
    - [ ] Populate `SimulationResults::metrics` from `hares_io::output::metrics::SimulationMetrics` (HARES-039 contract). Do not use an untyped map.
    - [ ] Record wall time using `std::time::Instant`.
    - [ ] `SimStatus` semantics:
      - [ ] `Ok`: simulation completed with no warnings
      - [ ] `Flagged(reason)`: simulation completed with warnings or soft-limit violations
      - [ ] `Failed(reason)`: panic or hard failure; no process abort
  - [ ] Implement `KernelTimer` for profiling mode
    - [ ] Use string constants for kernel names in this module: `KERNEL_TOTAL`, `KERNEL_CONSTRUCT_DWELLING`, `KERNEL_SIMULATE`.
    - [ ] Under `#[cfg(feature = "profiling")]`, emit timing entries to `tracing::info!`.
    - [ ] At minimum wrap engine phases (`construct_dwelling`, `simulate`, `total`) and surface a summary compatible with architecture profiling output (`envelope_solve`, `hvac`, `water_heater`, `schedule_load`, `io`) when underlying accumulators are available.
- [ ] Re-export from `lib.rs`
  - [ ] `SimulationEngine`
  - [ ] `SimulationConfig` (re-export from `hares-io`)
  - [ ] `SimulationResults`
  - [ ] `SimStatus`

## Files to Touch
- `crates/hares-core/src/engine.rs`: new file — engine entrypoint and result/status types
- `crates/hares-core/src/lib.rs`: declare and re-export engine API and `SimulationConfig`
- `crates/hares-core/tests/engine.rs`: integration tests for success/failure/warning paths

## Measures of Success
- [ ] `SimulationEngine::run` with a valid test fixture produces `SimStatus::Ok` or `SimStatus::Flagged(_)`, non-empty metrics, and `elapsed > Duration::ZERO`
- [ ] Passing a missing HPXML path returns a descriptive `Err` rather than panicking
- [ ] Passing a missing schedule or weather file likewise returns a descriptive `Err`
- [ ] `SimulationResults.metrics` contains at minimum `annual_energy_kwh` and `peak_electric_kw`
- [ ] `SimulationResults.warnings` is populated when `EnvironmentManager` emits a warning
- [ ] A deliberate panic inside a test equipment produces `SimStatus::Failed`, not a process abort
- [ ] `timeseries_path` is `Some(...)` and `timeseries` is `None` in default production mode

## Scope Boundaries
- [ ] In scope: single-dwelling batch execution orchestration, status mapping, warning propagation, elapsed timing, engine-level profiling hooks.
- [ ] Out of scope: fleet threading/progress behavior (HARES-047), Python-facing APIs (HARES-049+), output schema definitions (HARES-038), and metric formula implementation details (HARES-039).

## Source Parity Expectations
- [ ] Add at least one fixture-based parity-style assertion against architecture expectations from `docs/architecture/08-operations.md`: a successful run returns structured status/metrics/warnings without panicking, and a forced panic is isolated into `SimStatus::Failed`.

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
