---
id: TARIFF-012
title: Auto-register built-in BMS/EV actors during simulation init
kind: implement
depends_on:
  - TARIFF-008
  - TARIFF-009
  - TARIFF-010
  - TARIFF-011
files_to_touch:
  - crates/hares-core/src/dwelling/mod.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-009.md
  - docs/tickets/TARIFF-010.md
  - docs/tickets/TARIFF-011.md
verification:
  - cargo build --workspace
  - cargo test -p hares-core
  - cargo clippy --workspace
---

## Background/Context

Built-in actors should be auto-registered during simulation initialization based on the dwelling configuration. This keeps the Python API simple — users set a tariff and BMS mode, and the actors are wired up automatically. Users who want custom control can set `BmsMode::Manual` or `ChargingStrategy::Immediate` to disable built-in actors and register their own Python actors instead.

Note: there is no `TariffActor` — tariff evaluation is handled directly by the dwelling in the environment preparation phase (TARIFF-009). This ticket only auto-registers `BatteryManagementActor` and `EVChargingActor`.

For `TouOptimization` and `TouAware` strategies, the actors need a reference to the precomputed price array from the `TariffEvaluator`. The dwelling passes `Arc<[f64]>` slices at actor construction time. If no tariff is configured and a TOU-dependent mode is set, log a warning and fall back to a simpler mode.

## Work to Do

- [ ] In the dwelling initialization path (after equipment is constructed and tariff evaluator is built):
  1. For each battery equipment with `bms_mode != BmsMode::Manual`:
     - If mode is `TimeOfUseOptimization` and `tariff_evaluator.is_none()`: log warning, fall back to `SelfConsumption` with same `reserve_soc` as `min_soc`
     - Construct `BatteryManagementActor` with the battery's name, bms_mode, and optional `Arc<[f64]>` price array reference from tariff evaluator
     - Register it before user actors
  2. For each EV equipment with `charging_strategy` requiring tariff awareness (TouAware, V2G):
     - If tariff-dependent strategy and `tariff_evaluator.is_none()`: log warning, fall back to `Immediate` or `Scheduled`
     - Construct `EVChargingActor` with the EV's name, strategy, and optional price array reference
     - Register it before user actors
- [ ] Actor registration order: BatteryManagementActor(s) → EVChargingActor(s) → user-registered actors
- [ ] Registration should be idempotent — if user already registered an actor with the same name, skip
- [ ] Actors registered via this path should be distinguishable from user actors (e.g., via a `built_in: bool` flag on actor registration metadata)

## Files to Touch

- `crates/hares-core/src/dwelling/mod.rs`: Auto-register BMS/EV actors in init path

## Measures of Success

- [ ] Config with battery TOU mode + tariff → BatteryManagementActor registered with price array
- [ ] Config with EV TouAware + tariff → EVChargingActor registered with price array
- [ ] Config with `BmsMode::Manual` → no BatteryManagementActor registered
- [ ] Config with `ChargingStrategy::Immediate` → no EVChargingActor registered
- [ ] TOU mode without tariff → warning logged + fallback to SelfConsumption
- [ ] TouAware EV without tariff → warning logged + fallback to Immediate
- [ ] Built-in actors registered before user actors
- [ ] Idempotent: duplicate registration skipped

## Tests Added

**hares-core:**
- `auto_register_bms_actor` — battery with TOU mode + tariff → BatteryManagementActor registered
- `auto_register_ev_actor` — EV with TouAware + tariff → EVChargingActor registered
- `manual_mode_no_bms_actor` — BmsMode::Manual → no BatteryManagementActor
- `immediate_no_ev_actor` — ChargingStrategy::Immediate → no EVChargingActor
- `tou_without_tariff_warns_and_falls_back` — TOU mode + no tariff → warning + SelfConsumption
- `tou_aware_ev_without_tariff_warns` — TouAware + no tariff → warning + Immediate fallback
- `actor_order_before_user_actors` — built-in actors precede user-registered actors
- `auto_register_multiple_batteries` — 2 batteries with different BMS modes → 2 separate BatteryManagementActors
- `auto_register_multiple_evs` — 2 EVs with different strategies → 2 separate EVChargingActors

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy --workspace` passes
