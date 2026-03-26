---
id: TARIFF-005
title: Implement TariffEvaluator with precomputed price array
kind: implement
depends_on:
  - TARIFF-004
files_to_touch:
  - crates/hares-tariff/Cargo.toml
  - crates/hares-tariff/src/evaluator.rs
  - crates/hares-tariff/src/lib.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-004.md
verification:
  - cargo build --workspace
  - cargo test -p hares-tariff
  - cargo clippy --workspace
---

## Background/Context

The tariff evaluator is the core runtime component that translates a static `ElectricTariff` definition into per-timestep price signals. The key design decision is to precompute energy prices into a flat array at initialization — this gives O(1) lookup per timestep with zero allocations in the hot loop.

TOU energy prices depend only on time-of-day, day-of-week, month, and season — all known at init time. Tiered rates depend on cumulative energy and cannot be fully precomputed, so the evaluator provides a separate `tier_multiplier()` method that applies a scalar adjustment based on current billing period energy.

The evaluator needs `chrono-tz` for DST-aware timestamp mapping (using the existing `civil_timezone` from `SimulationConfig`).

## Work to Do

- [ ] Add `chrono` and `chrono-tz` dependencies to `crates/hares-tariff/Cargo.toml`
- [ ] Create `crates/hares-tariff/src/evaluator.rs`
- [ ] Implement `TariffEvaluator`:
  ```rust
  pub struct TariffEvaluator {
      tariff: ElectricTariff,
      price_array: Vec<f64>,        // $/kWh per simulation interval
      export_array: Vec<f64>,       // $/kWh export credit per interval
      interval_seconds: u32,
      simulation_start: DateTime<Tz>,
      step_index: usize,            // current position in price_array
  }
  ```
- [ ] Implement `TariffEvaluator::new(tariff, simulation_start, simulation_end, interval_seconds, timezone)`:
  1. Compute total interval count from duration / interval_seconds
  2. Pre-allocate `price_array` and `export_array` with exact capacity
  3. For each interval, convert UTC timestamp to civil time using timezone
  4. Determine active `TouPeriod` by matching civil time against `TimeWindow` schedule + `SeasonFilter`
  5. Look up `EnergyRate` by matching `period_name` + `season`
  6. Store rate in `price_array[i]`
  7. Similarly compute `export_array[i]` from `ExportRate.tou_credits` or flat rate
  8. If no matching period: use rate 0.0 (log warning at init, not per-step)
- [ ] Implement `TariffEvaluator::current_price(&self) -> f64` — returns `price_array[self.step_index]`
- [ ] Implement `TariffEvaluator::current_export_price(&self) -> f64` — returns `export_array[self.step_index]`
- [ ] Implement `TariffEvaluator::current_period_name(&self) -> &str` — O(1) from a parallel precomputed `period_names` array or by re-evaluating (prefer precomputed)
- [ ] Implement `TariffEvaluator::advance(&mut self)` — increments `step_index`
- [ ] Implement `TariffEvaluator::tier_multiplier(&self, cumulative_kwh: f64) -> f64`:
  - Find current season from step_index → civil time → month
  - Find matching `TieredBlock` by season
  - Walk thresholds to find current tier
  - Return `tier_rate / base_rate` as multiplier (or just the tier rate directly)
- [ ] Implement `TariffEvaluator::price_slice(&self, start_idx: usize, end_idx: usize) -> &[f64]`:
  - Returns slice of price_array for lookahead (used by EVChargingActor for TOU-aware scheduling)
- [ ] Register module in `crates/hares-tariff/src/lib.rs`

## Files to Touch

- `crates/hares-tariff/Cargo.toml`: Add chrono, chrono-tz dependencies
- `crates/hares-tariff/src/evaluator.rs`: New file with TariffEvaluator
- `crates/hares-tariff/src/lib.rs`: Add module declaration and re-exports

## Measures of Success

- [ ] Evaluator builds price array for a 1-year hourly simulation (8760 entries) without panic
- [ ] Array length exactly matches `(end - start).num_seconds() / interval_seconds`
- [ ] Summer weekday peak hour returns summer peak rate
- [ ] Winter weekend off-peak hour returns winter off-peak rate
- [ ] Midnight returns correct off-peak/super-off-peak rate
- [ ] DST transition hours map correctly (spring forward: no duplicate, fall back: no gap)
- [ ] `tier_multiplier` returns correct tier rate at tier boundaries
- [ ] `price_slice` returns valid subslice for lookahead queries
- [ ] No heap allocations after `new()` (verify arrays are pre-allocated)

## Tests Added

**hares-tariff:**
- `evaluator_hourly_array_length` — 365 days × 24 hours = 8760 entries
- `evaluator_15min_array_length` — 365 × 96 = 35040 entries
- `evaluator_summer_peak_price` — July weekday 5pm returns summer peak rate
- `evaluator_winter_offpeak_price` — January weekend midnight returns winter off-peak rate
- `evaluator_season_boundary` — June 1 vs May 31 returns different seasonal rates
- `evaluator_dst_spring_forward` — March DST transition produces correct array length
- `evaluator_tier_multiplier_first_tier` — cumulative 0 kWh returns tier 1 rate
- `evaluator_tier_multiplier_second_tier` — cumulative above threshold returns tier 2 rate
- `evaluator_price_slice` — slice returns correct subarray
- `evaluator_flat_rate_tariff` — no TOU periods → uniform price array

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-tariff` passes
- [ ] `cargo clippy --workspace` passes
