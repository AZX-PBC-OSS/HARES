---
id: HARES-039
title: "hares-io — Output Metrics"
kind: implement
depends_on: [HARES-038]
files_to_touch:
  - crates/hares-io/src/output/metrics.rs
references:
  - docs/architecture/06-input-output.md
  - vendors/OCHRE/ochre/Analysis.py
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
OCHRE's `Analysis.calculate_metrics()` (in `Analysis.py`, 797 lines) derives post-simulation summary statistics from the full timeseries output. HARES must produce the same metrics so that fleet aggregation scripts and comparison workflows continue to work without modification. Metrics are computed from the same Arrow RecordBatch stream used by the output writer, avoiding a second pass over raw simulation state.

## Work to Do
- [ ] Implement `output/metrics.rs`: `MetricsCalculator` and `SimulationMetrics` structs
  - [ ] `annual_energy_kwh`: total and per-end-use kWh sums; computed as `sum(power_kw) * timestep_h` where `timestep_h = time_res_secs / 3600.0`
  - [ ] `peak_power_kw`: maximum instantaneous power value per end-use across the simulation
  - [ ] Split metrics into two groups:
    - Always-available: `annual_energy_kwh` (total and per-end-use), `peak_power_kw` (per end-use), and demand metrics — computable at any verbosity level
    - Verbosity-gated: `comfort_hours` and `unmet_load_hours` require verbosity ≥ 3 because setpoint columns first appear at level 3. These return `None` when required columns are absent in the schema.
  - [ ] `MetricsCalculator::new` must succeed at any verbosity level; gated metrics return `None` when required columns are absent — do not error or silently return zero.
  - [ ] `comfort_hours`: count of timesteps where all conditioned zone temperatures fall within the active setpoint deadband; convert to hours via `count * timestep_h`. Requires setpoint columns (verbosity ≥ 3).
  - [ ] `unmet_load_hours`: count of timesteps where HVAC output power equals capacity limit and zone temperature is outside setpoint; convert to hours. Requires verbosity ≥ 3 for the same reason.
  - [ ] `comfort_hours` and `unmet_load_hours` use the setpoint deadband from `SimulationConfig` (passed at construction) or from equipment telemetry if config value is absent
  - [ ] `renewable_energy_fraction`: `abs(pv_generation_kwh) / total_consumption_kwh`; PV generation is stored as a negative value in the port model (generation convention), so take the absolute value; clamp result to [0, 1]; return `None` when no PV column is present in the schema
  - [ ] `grid_interaction_metrics`: `peak_import_kw` (max grid draw), `peak_export_kw` (max grid feed-in, positive convention)
  - [ ] `MetricsCalculator::new(schema: &Schema, time_res_secs: u32, config: &SimulationConfig) -> Result<Self>` — returns `Err` only when universally-required columns (e.g., `total_electric_power_kw`) are absent from the schema; verbosity-gated columns (setpoint columns, PV column) are optional and their absence causes the corresponding metrics to return `None`, not a construction failure
  - [ ] `MetricsCalculator::accumulate(&mut self, batch: &RecordBatch)` — update running aggregates from each flushed RecordBatch; no full-timeseries retention
  - [ ] `MetricsCalculator::finish(self) -> SimulationMetrics` — finalise all metrics and return
  - [ ] All metrics fields use `f64`; optional metrics use `Option<f64>`

## Files to Touch
- `crates/hares-io/src/output/metrics.rs`: new file — `MetricsCalculator`, `SimulationMetrics`, all metric computations

## Measures of Success
- [ ] A synthetic 8760-row timeseries with constant 1 kW electric load produces `annual_energy_kwh.total = 8760.0`
- [ ] Peak power correctly identifies the maximum row across the full synthetic series
- [ ] Comfort hours equals simulation duration in hours when all zone temperatures are always within deadband
- [ ] `MetricsCalculator::new` with a verbosity-0 schema succeeds; `comfort_hours` and `unmet_load_hours` return `None` (not an error)
- [ ] `renewable_energy_fraction` with a PV column carrying negative values (generation convention) produces a positive fraction
- [ ] `renewable_energy_fraction` returns `None` for a series with no PV column present
- [ ] `peak_export_kw` is zero when the grid import column is always non-negative

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
