---
id: TARIFF-006
title: Implement BillingState and per-step demand/energy accumulation
kind: implement
depends_on:
  - TARIFF-005
files_to_touch:
  - crates/hares-tariff/src/billing.rs
  - crates/hares-tariff/src/evaluator.rs
  - crates/hares-tariff/src/lib.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-005.md
verification:
  - cargo build --workspace
  - cargo test -p hares-tariff
  - cargo clippy --workspace
---

## Background/Context

Demand charges and tiered energy rates require per-billing-period state: cumulative energy consumption for tier evaluation and peak 15-minute demand for demand charges. The billing state must be updated every simulation timestep with zero allocations in the hot loop.

The 15-minute demand window uses a pre-allocated ring buffer to compute rolling averages. On billing period close (month boundary), the evaluator produces a `BillingPeriodSummary` with all charge components broken out. Demand ratchets require tracking 12 months of historical peaks.

## Work to Do

- [ ] Create `crates/hares-tariff/src/billing.rs`
- [ ] Implement `DemandWindow` — pre-allocated ring buffer for 15-min rolling average:
  ```rust
  struct DemandWindow {
      samples: Box<[f64]>,       // pre-allocated, sized for demand_window_minutes / interval_seconds
      head: usize,
      count: usize,
      running_sum: f64,
  }
  ```
  - `push(power_kw)` — O(1), updates running sum, advances head
  - `average() -> f64` — running_sum / count
  - `new(capacity)` — pre-allocates sample buffer
- [ ] Implement `BillingState`:
  ```rust
  pub struct BillingState {
      pub period_start: DateTime<Tz>,
      pub period_end: DateTime<Tz>,
      pub cumulative_import_kwh: f64,
      pub cumulative_export_kwh: f64,
      pub peak_demand_kw: f64,
      pub prior_peaks_kw: VecDeque<f64>,   // rolling 12-month for ratchet
      demand_window: DemandWindow,
  }
  ```
- [ ] Implement `BillingState::new(period_start, billing_cycle, demand_window_minutes, interval_seconds, ratchet_config: Option<RatchetConfig>)`:
  - Compute period_end from period_start + billing_cycle
  - Pre-allocate DemandWindow with correct capacity
  - Initialize prior_peaks_kw with capacity 12
  - Store `ratchet_config` for use in period close computation
- [ ] Implement `BillingState::update(net_power_kw: f64, dt_seconds: f64)`:
  - `cumulative_import_kwh += max(0, net_power_kw) * dt_seconds / 3600.0`
  - `cumulative_export_kwh += max(0, -net_power_kw) * dt_seconds / 3600.0`
  - `demand_window.push(max(0, net_power_kw))`
  - `peak_demand_kw = max(peak_demand_kw, demand_window.average())`
- [ ] Implement `BillingPeriodSummary`:
  ```rust
  pub struct BillingPeriodSummary {
      pub period_start: DateTime<Tz>,
      pub period_end: DateTime<Tz>,
      pub energy_charge_usd: f64,
      pub demand_charge_usd: f64,
      pub fixed_charge_usd: f64,
      pub export_credit_usd: f64,
      pub net_bill_usd: f64,
      pub peak_demand_kw: f64,
      pub total_import_kwh: f64,
      pub total_export_kwh: f64,
  }
  ```
- [ ] Add `step(net_power_kw, dt_seconds, current_time) -> Option<BillingPeriodSummary>` method to `TariffEvaluator`:
  1. Call `billing_state.update(net_power_kw, dt_seconds)`
  2. Check if `current_time >= billing_state.period_end`
  3. If period crossed: compute summary, apply ratchet, reset state, advance period, return `Some(summary)`
  4. Otherwise return `None`
- [ ] Ratchet computation:
  ```
  effective_peak = max(current_peak, ratchet_fraction * max(prior_peaks))
  demand_charge = effective_peak * demand_rate_per_kw
  ```
- [ ] Add `BillingState` field to `TariffEvaluator`, initialized in `new()`
- [ ] Register module in `crates/hares-tariff/src/lib.rs`

## Files to Touch

- `crates/hares-tariff/src/billing.rs`: New file with BillingState, DemandWindow, BillingPeriodSummary
- `crates/hares-tariff/src/evaluator.rs`: Add billing_state field, step() method
- `crates/hares-tariff/src/lib.rs`: Add module declaration and re-exports

## Measures of Success

- [ ] DemandWindow push/average is O(1) with no heap allocation after init
- [ ] 15-min rolling average correctly tracks highest demand window
- [ ] Billing period closes at month boundary (e.g., Jan 1 → Feb 1)
- [ ] Cumulative energy resets to zero on period close
- [ ] Peak demand resets to zero on period close
- [ ] Prior peaks track last 12 months (VecDeque bounded at 12)
- [ ] Ratchet applies when current peak < fraction × historical peak
- [ ] BillingPeriodSummary.net_bill_usd = energy + demand + fixed - export credit
- [ ] No allocations in the step() hot path

## Tests Added

**hares-tariff:**
- `demand_window_rolling_average` — push 4 values, average equals mean of last N
- `demand_window_peak_tracking` — push sequence, peak captures highest window average
- `billing_state_energy_accumulation` — constant 1kW for 1 hour = 1 kWh import
- `billing_state_export_accumulation` — constant -2kW = 2 kWh export
- `billing_period_closes_at_month_end` — 32 days of steps, summary emitted once
- `billing_period_resets_on_close` — cumulative energy and peak demand reset after close
- `billing_ratchet_applies` — current peak < 85% of prior → billed at 85% of prior
- `billing_ratchet_not_applied_when_current_higher` — current peak > prior → billed at current
- `billing_prior_peaks_bounded` — 13 months of data → only last 12 kept
- `billing_period_summary_net_bill` — verify arithmetic: energy + demand + fixed - export = net

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-tariff` passes
- [ ] `cargo clippy --workspace` passes
