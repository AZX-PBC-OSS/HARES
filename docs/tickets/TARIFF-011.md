---
id: TARIFF-011
title: Implement EVChargingActor with all charging strategies
kind: implement
depends_on:
  - TARIFF-003
  - TARIFF-008
  - TARIFF-009
files_to_touch:
  - crates/hares-core/src/actors/ev_charging.rs
  - crates/hares-core/src/actors/mod.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-003.md
  - docs/tickets/TARIFF-009.md
  - crates/hares-core/src/actor.rs
  - crates/hares-equipment/src/ev/mod.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-core
  - cargo clippy --workspace
---

## Background/Context

The `EVChargingActor` translates EV `ChargingStrategy` configuration into per-timestep `ControlSignal` dispatches. Unlike batteries which are always available, EVs have event-driven availability (plug-in/departure schedule). The actor must check EV connection state before emitting any control signals.

The actor reads `env.price_signal` (populated by the dwelling's tariff evaluator during env prep) and `env.electrical` (PV generation, base load from prior step). It never mutates environment — only emits `DispatchRequest`s.

The `TouAware` strategy requires price lookahead — sorting future intervals by cost to charge during cheapest periods while meeting a departure deadline. The actor holds a reference to the precomputed price array (passed at construction from the dwelling's tariff evaluator). The sort happens once at plug-in time (O(n log n)) and is cached until the next plug-in event. The `charge_buffer_hours` parameter triggers max-rate charging near the deadline regardless of price.

## Work to Do

- [ ] Create `crates/hares-core/src/actors/ev_charging.rs`
- [ ] Implement `EVChargingActor` struct:
  ```rust
  pub struct EVChargingActor {
      ev_name: String,
      strategy: ChargingStrategy,
      // Cached TOU-aware charge plan (interval indices sorted by price)
      cached_charge_plan: Option<CachedChargePlan>,
      was_plugged_in: bool,  // for detecting plug-in transitions
  }

  struct CachedChargePlan {
      intervals_by_price: Vec<usize>,  // sorted cheapest-first
      energy_needed_kwh: f64,
      departure_step_index: usize,
  }
  ```
- [ ] Implement `Actor` trait for `EVChargingActor`:
  - `name()` → `"EVChargingActor:{ev_name}"`
  - `decide(env, dispatch_buffer)`:
    1. Read EV telemetry: `plugged_in`, `soc`, `connection_state`, `departure_time` (from equipment telemetry, not env)
    2. Read `env.price_signal` for current price, `env.electrical` for PV/load data
    3. If not plugged in: clear cached plan, return (no dispatch)
    4. Detect plug-in transition (`!was_plugged_in && plugged_in`): trigger plan computation for TOU-aware
    5. Match on strategy:

  **Immediate:**
  - Emit `PowerSetpoint { active_power_kw: max_charge_rate }` (respects existing equipment power limits)

  **Nightly:**
  - If current civil hour is within `[off_peak_start_hour, off_peak_end_hour)`: charge at max rate
  - Otherwise: idle

  **LowSoc:**
  - If `soc < threshold`: emit `SocTarget { soc: target_soc }`
  - Otherwise: idle

  **PreDeparture:**
  - Find matching `DepartureConstraint` for today's day-of-week
  - Compute time to departure, energy needed
  - Compute minimum charge rate: `energy_needed / time_remaining`
  - If `time_remaining < 2h` or rate > 0.8 × max_rate: charge at max rate
  - Otherwise: defer (idle until closer to departure)

  **TouAware:**
  - On plug-in: build `CachedChargePlan`:
    1. Get price slice from TariffEvaluator for `[now, departure]`
    2. Sort interval indices by price (ascending)
    3. Compute energy needed: `(target_soc - current_soc) * capacity_kwh / efficiency`
    4. Mark cheapest intervals until energy target met
  - Each step: if current interval is in the charge plan → charge at max rate, else idle
  - Buffer fallback: if `time_to_departure < charge_buffer_hours` → charge at max rate regardless of price
  - If `price_signal` differs from precomputed price by >10%: invalidate and rebuild plan (dynamic tariff detection)

  **SolarSurplus:**
  - Read `env.electrical.pv_generation_kw` and `env.electrical.base_load_kw`
  - `surplus = max(0, pv_generation_kw - base_load_kw)`
  - If `surplus >= min_charge_rate_kw`: emit `PowerSetpoint { active_power_kw: surplus }`
  - If `surplus < min_charge_rate_kw`: idle (avoid low-current charging)
  - Check departure constraints: if departure imminent, switch to max rate

  **QuickThenWait:**
  - If `soc < partial_soc`: charge at max rate
  - Otherwise: idle

  **V2H:**
  - If `soc > discharge_threshold_soc` AND home load > PV generation: emit `PowerSetpoint { active_power_kw: -(home_load - pv_generation) }` (clamped to max discharge rate and min_soc)
  - Otherwise: idle

  **V2G:**
  - If `soc > min_soc` AND `price > price_threshold`: emit `PowerSetpoint { active_power_kw: -max_export_kw }`
  - V2G export is constrained by the EV's own `max_export_kw` parameter. Dwelling-level export limits (if any) are enforced by the equipment's existing `export_limit_w` clamp in the electrical model, not by the actor.
  - Otherwise: idle

- [ ] All dispatches use `PriorityTier::Schedule` and target `ByName(ev_name)`
- [ ] Register module in `crates/hares-core/src/actors/mod.rs`

## Files to Touch

- `crates/hares-core/src/actors/ev_charging.rs`: New actor implementation
- `crates/hares-core/src/actors/mod.rs`: Add module declaration and re-exports

## Measures of Success

- [ ] `EVChargingActor` implements `Actor` trait
- [ ] All existing ChargingStrategy variants (Immediate, Nightly, LowSoc, QuickThenWait, PreDeparture, TouAware) produce correct ControlSignal
- [ ] New variants (SolarSurplus, V2H, V2G) produce correct ControlSignal
- [ ] No dispatch emitted when EV not plugged in
- [ ] TouAware cached plan recomputed only on plug-in transition (not every step)
- [ ] TouAware charge_buffer_hours fallback works near departure deadline
- [ ] V2H/V2G only activate under correct SOC and price conditions
- [ ] SolarSurplus respects min_charge_rate_kw threshold

## Tests Added

**hares-core:**
- `ev_immediate_max_rate` — plugged in → max rate PowerSetpoint
- `ev_not_plugged_in_no_dispatch` — disconnected → empty dispatch buffer
- `ev_nightly_off_peak_charges` — during off-peak hours → charge
- `ev_nightly_peak_idles` — during peak hours → no dispatch
- `ev_low_soc_below_threshold_charges` — soc < threshold → SocTarget
- `ev_low_soc_above_threshold_idles` — soc ≥ threshold → no dispatch
- `ev_tou_aware_charges_cheapest` — known price array → charging in cheapest intervals
- `ev_tou_aware_buffer_fallback` — near departure → max rate regardless of price
- `ev_tou_aware_plan_cached` — plan not recomputed between steps (assert via counter)
- `ev_tou_aware_plan_rebuilt_on_plugin` — new plug-in event → plan recomputed
- `ev_solar_surplus_modulates_rate` — 3kW surplus → 3kW charge rate
- `ev_solar_surplus_below_min_idles` — 0.5kW surplus, min=1.4kW → idle
- `ev_pre_departure_defers_start` — 8h until departure, 2h charge needed → idle initially
- `ev_pre_departure_charges_near_deadline` — <2h to departure → max rate
- `ev_v2h_discharges_during_deficit` — soc > threshold, load > pv → negative power
- `ev_v2h_idles_with_low_soc` — soc ≤ threshold → no dispatch
- `ev_v2g_discharges_above_price` — price > threshold → negative export power
- `ev_v2g_idles_below_price` — price ≤ threshold → no dispatch

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy --workspace` passes
