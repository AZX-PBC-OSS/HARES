---
id: HARES-048
title: "hares-fleet — Aggregation and Progress"
kind: implement
depends_on: [HARES-047, HARES-038]
files_to_touch:
  - crates/hares-fleet/src/aggregation.rs
  - crates/hares-fleet/src/progress.rs
  - crates/hares-fleet/src/lib.rs
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - cargo check -p hares-fleet
  - cargo test -p hares-fleet
  - cargo clippy -p hares-fleet -- -D warnings
---

## Background/Context
ResStock samples carry sample weights that must be applied when computing fleet-level load profiles. Aggregation must produce both per-dwelling scalar metrics and a weighted-sum timeseries at a configurable resolution. Progress reporting uses structured `tracing` events with stable keys so that log consumers can parse them reliably.

## Work to Do
- [ ] Implement `aggregation.rs`: weight-aware fleet aggregation
  - [ ] `FleetResults` struct
    - [ ] `per_dwelling_metrics: Vec<DwellingMetrics>` — one row per dwelling (energy, peak, sample weight, status)
    - [ ] `aggregate_timeseries: RecordBatch` — weighted sum timeseries at configured resolution (15-min or hourly); uses HARES-038 Arrow output infrastructure
  - [ ] `aggregate(results: &[DwellingOutcome], resolution: AggregationResolution) -> FleetResults` — `DwellingOutcome` (defined in HARES-047) carries `sample_weight: f64`; do not add `sample_weight` to `SimulationResults`
    - [ ] `total_load[t] = sum(dwelling.result.timeseries[t] * dwelling.sample_weight)` for each timestep
    - [ ] Aggregate timeseries uses the intersection of all successful dwelling timestamp grids as the output index. Failed dwellings are excluded. Timesteps where a dwelling failed partway through use `null` (not 0.0).
    - [ ] Resampling arithmetic: power columns (kW suffix) are averaged, energy columns (kWh suffix) are summed, temperature columns (°C suffix) are averaged, fraction columns (unitless 0–1) are averaged. Use column name suffix matching, consistent with OCHRE's Analysis.get_agg_func() approach.
    - [ ] Exclude `Failed` dwellings from the weighted sum; include them in `per_dwelling_metrics` with a failure flag
  - [ ] `AggregationResolution` enum: `FifteenMin`, `Hourly`
- [ ] Implement `progress.rs`: fleet progress reporting
  - [ ] Report via `tracing::info!` with stable keys: `fleet_progress`, `completed`, `total`, `pct`, `elapsed_s`, `rate_dw_per_s`, `eta_s`
  - [ ] Example log line format: `[INFO] fleet progress: 850/10000 dwellings (8.5%) elapsed=42s rate=20.2 dw/s eta=453s`
  - [ ] Callback-based trigger: report every N dwellings completed or every T seconds elapsed, whichever comes first
- [ ] Re-export `FleetResults`, `DwellingMetrics`, `DwellingOutcome`, and `AggregationResolution` from `lib.rs`

## Files to Touch
- `crates/hares-fleet/src/aggregation.rs`: new file — `FleetResults`, `DwellingMetrics`, `aggregate()`
- `crates/hares-fleet/src/progress.rs`: new file — progress reporting logic
- `crates/hares-fleet/src/lib.rs`: declare and re-export new modules

## Measures of Success
- [ ] Five dwellings with known sample weights produce a weighted aggregate timeseries that matches a hand-calculated result to within floating-point precision
- [ ] Failed dwellings appear in `per_dwelling_metrics` with a failure flag and do not contribute to the aggregate timeseries
- [ ] The progress callback fires at the expected intervals (verified with a mock time source or step count)
- [ ] `tracing` output contains all required stable keys

## Verification
- [ ] `cargo check -p hares-fleet` passes
- [ ] `cargo test -p hares-fleet` passes
- [ ] `cargo clippy -p hares-fleet -- -D warnings` passes
