---
id: TARIFF-009
title: Add PriceSignal and ElectricalSummary to EnvironmentState; integrate TariffEvaluator into dwelling step
kind: implement
depends_on:
  - TARIFF-006
files_to_touch:
  - crates/hares-types/src/environment.rs
  - crates/hares-types/src/lib.rs
  - crates/hares-control/src/types.rs
  - crates/hares-core/Cargo.toml
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-core/src/actor.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-006.md
  - crates/hares-types/src/environment.rs
  - crates/hares-core/src/dwelling/mod.rs
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

The review identified a critical architectural flaw: the original design had a `TariffActor` mutating `EnvironmentState` to set prices. But `EnvironmentState` is **immutable input** to actors — actors receive `&EnvironmentState` and emit `DispatchRequest`s only. They never mutate environment directly.

**Price signal is environmental input** — like weather, it's a deterministic function of the clock. The tariff rate at time `t` is known at init time from the precomputed price array. The dwelling should populate `price_signal` during the environment preparation phase (Step 1 of `run_timestep`), before actors run.

**Electrical summary is prior-step observation** — BMS and EV actors need PV generation and net load to make charge/discharge decisions. These are results from the previous timestep's electrical solver. The dwelling populates this during Step 1 from the prior step's solver output.

**Billing accumulation is post-step accounting** — the dwelling calls `tariff_evaluator.step()` after the electrical solver runs (end of timestep), using actual metered power. This is accounting, not decision-making.

This ticket replaces the original `TariffActor` concept with clean integration into the dwelling's existing timestep phases.

## Work to Do

### 1. Extend EnvironmentState with price_signal and electrical summary

- [ ] Add `PriceSignal` to `EnvironmentState` in `crates/hares-types/src/environment.rs`:
  ```rust
  pub struct EnvironmentState {
      // ... existing fields ...
      pub price_signal: PriceSignal,
      pub electrical: ElectricalSummary,
  }
  ```
- [ ] Move `PriceSignal` from `hares-control` to `hares-types` (it's a data type, not control logic). Keep a re-export in `hares-control` for backward compat if anything depends on it.
- [ ] Define `ElectricalSummary` in `crates/hares-types/src/environment.rs`:
  ```rust
  /// Electrical power summary from the prior timestep's solver.
  ///
  /// Provides read-only observation of the building's electrical state
  /// so actors can make informed decisions (e.g., BMS self-consumption
  /// needs to know PV generation vs home load).
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
  pub struct ElectricalSummary {
      /// Total PV generation [kW], positive = producing.
      pub pv_generation_kw: f64,
      /// Total non-dispatchable load [kW], positive = consuming.
      /// Excludes battery and EV (those are dispatchable).
      pub base_load_kw: f64,
      /// Net grid power [kW], positive = importing, negative = exporting.
      pub net_grid_kw: f64,
      /// Total battery power [kW], positive = charging, negative = discharging.
      pub battery_power_kw: f64,
      /// Total EV power [kW], positive = charging.
      pub ev_power_kw: f64,
  }
  ```
- [ ] Both new fields default to zero/empty (first step has no prior data)
- [ ] Update `TestEnvBuilder` in `crates/hares-core/src/actor.rs` to include default `price_signal` and `electrical`
- [ ] Update all existing `EnvironmentState` construction sites to include new fields (with defaults)
- [ ] `#[serde(default)]` on both new fields for backward compat with serialized checkpoints

### 2. Integrate TariffEvaluator into dwelling

- [ ] Add `hares-tariff` dependency to `crates/hares-core/Cargo.toml`
- [ ] Add `tariff_evaluator: Option<TariffEvaluator>` field to `Dwelling`
- [ ] Add `billing_summaries: Vec<BillingPeriodSummary>` field to `Dwelling` (accumulates over simulation)
- [ ] In `run_timestep()` Step 1, after `self.environment.update()`:
  ```rust
  // Step 1 (after weather/time update, before actors):
  // Populate price signal from tariff evaluator
  if let Some(ref evaluator) = self.tariff_evaluator {
      self.latest_env.price_signal = PriceSignal {
          electricity_price: Some(evaluator.current_price()),
          export_price: Some(evaluator.current_export_price()),
          ghg_intensity: self.latest_env.price_signal.ghg_intensity, // preserve external
      };
  }
  // Populate electrical summary from prior step's solver results
  self.latest_env.electrical = self.prior_electrical_summary.clone();
  ```
- [ ] After the electrical solver (end of timestep), update billing state:
  ```rust
  // Post-solver: accumulate billing state
  if let Some(ref mut evaluator) = self.tariff_evaluator {
      let net_kw = /* net grid power from solver */;
      if let Some(summary) = evaluator.step(net_kw, dt_secs, current_time) {
          self.billing_summaries.push(summary);
      }
      evaluator.advance();
  }
  // Capture electrical summary for next step's EnvironmentState
  self.prior_electrical_summary = ElectricalSummary {
      pv_generation_kw: /* from PV equipment port */,
      base_load_kw: /* from non-dispatchable equipment */,
      net_grid_kw: /* from electrical solver */,
      battery_power_kw: /* from battery equipment */,
      ev_power_kw: /* from EV equipment */,
  };
  ```
- [ ] Add `prior_electrical_summary: ElectricalSummary` field to `Dwelling` (zeroed at init)
- [ ] External `set_price_signal()` still works — it sets `self.price_signal` which is used when no tariff_evaluator is configured. The tariff evaluator overrides electricity_price/export_price but preserves externally-set ghg_intensity.
- [ ] Add `pub fn billing_summaries(&self) -> &[BillingPeriodSummary]` accessor
- [ ] Add `pub fn set_tariff(&mut self, tariff: ElectricTariff)` — builds `TariffEvaluator` from tariff + sim config
- [ ] Remove the `PriceSignalChange` `ActorInterest` variant or leave it for external price changes — actors interested in price changes now just read `env.price_signal` each step

## Files to Touch

- `crates/hares-types/src/environment.rs`: Add `PriceSignal`, `ElectricalSummary` to `EnvironmentState`
- `crates/hares-types/src/lib.rs`: Export `ElectricalSummary`
- `crates/hares-control/src/types.rs`: Move `PriceSignal` to `hares-types`, keep re-export
- `crates/hares-core/Cargo.toml`: Add hares-tariff dependency
- `crates/hares-core/src/dwelling/mod.rs`: Add tariff_evaluator field, integrate into run_timestep, add accessors
- `crates/hares-core/src/actor.rs`: Update TestEnvBuilder with new EnvironmentState fields

## Measures of Success

- [ ] `EnvironmentState.price_signal` populated before actors run each step
- [ ] `EnvironmentState.electrical` populated with prior step's solver results
- [ ] Actors receive prices via `env.price_signal` (read-only, no mutation)
- [ ] Actors receive electrical summary via `env.electrical` (read-only)
- [ ] `set_price_signal()` still works when no tariff configured (backward compat)
- [ ] Tariff evaluator overrides electricity/export price but preserves externally-set ghg_intensity
- [ ] Billing state accumulates correctly post-solver
- [ ] `BillingPeriodSummary` captured in dwelling's billing_summaries vec
- [ ] First timestep has zeroed ElectricalSummary (no prior data)
- [ ] All existing tests pass (serde defaults on new fields)
- [ ] No actor trait signature changes — actors still get `&EnvironmentState`

## Tests Added

**hares-types:**
- `environment_state_with_price_signal_roundtrip` — serde with PriceSignal
- `environment_state_default_electrical_summary` — default is all zeros
- `environment_state_backward_compat` — old JSON without price_signal/electrical deserializes

**hares-core:**
- `dwelling_tariff_evaluator_sets_price` — configure tariff, step, verify env.price_signal matches expected rate
- `dwelling_tariff_evaluator_export_price` — verify export price set
- `dwelling_electrical_summary_populated` — step, verify env.electrical has prior step's values
- `dwelling_electrical_summary_zero_first_step` — first step has zeroed summary
- `dwelling_billing_period_close` — 35-day sim, verify billing_summaries has one entry
- `dwelling_set_price_signal_without_tariff` — external set_price_signal works as before
- `dwelling_tariff_preserves_ghg_intensity` — external ghg_intensity not overwritten by tariff
- `dwelling_set_tariff_builds_evaluator` — set_tariff() creates evaluator from sim config

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
