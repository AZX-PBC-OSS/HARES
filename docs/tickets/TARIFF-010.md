---
id: TARIFF-010
title: Implement BatteryManagementActor with all BMS modes
kind: implement
depends_on:
  - TARIFF-002
  - TARIFF-008
  - TARIFF-009
files_to_touch:
  - crates/hares-core/src/actors/bms.rs
  - crates/hares-core/src/actors/mod.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-002.md
  - docs/tickets/TARIFF-009.md
  - crates/hares-core/src/actor.rs
  - crates/hares-core/src/actors/dr_compliance.rs
  - crates/hares-types/src/environment.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-core
  - cargo clippy --workspace
---

## Background/Context

The `BatteryManagementActor` reads the battery's configured `BmsMode`, the current `PriceSignal` from `env.price_signal` (populated by the dwelling's tariff evaluator in the env prep phase), and electrical state from `env.electrical` (PV generation, base load, net grid) to emit appropriate `ControlSignal` dispatch requests. This actor implements the same operating modes found in real residential battery systems (Tesla Powerwall, Enphase IQ, etc.).

The actor reads environment state — it never mutates it. It only emits `DispatchRequest`s at `PriorityTier::Schedule` so Python users can override with custom actors at `UserOverride` or `Grid` tiers. In `BmsMode::Manual`, the actor emits nothing — external actors have full control.

## Work to Do

- [ ] Create `crates/hares-core/src/actors/bms.rs`
- [ ] Implement `BatteryManagementActor` struct:
  ```rust
  pub struct BatteryManagementActor {
      battery_name: String,
      bms_mode: BmsMode,
      grid_export_rule: GridExportRule,
      // Pre-computed TOU price percentiles for the current day (for TouOptimization)
      charge_price_threshold: f64,
      discharge_price_threshold: f64,
      current_day_ordinal: u32,
  }
  ```
- [ ] Implement `Actor` trait for `BatteryManagementActor`:
  - `name()` → `"BatteryManagementActor:{battery_name}"`
  - `decide(env, dispatch_buffer)`: match on `self.bms_mode` and dispatch:

  **SelfConsumption:**
  - Read `env.electrical.pv_generation_kw` and `env.electrical.base_load_kw`
  - If PV surplus (generation > load): emit `SelfConsumption { enabled: true }` — battery charges from surplus
  - If deficit (load > generation): emit `SelfConsumption { enabled: true }` — battery discharges to offset
  - Respect `min_soc` / `max_soc` bounds via `PowerLimit` if SOC near bounds
  - If `solar_only_charging`: emit `GridConnect { connected: false }` when PV = 0 to prevent grid charging

  **TimeOfUseOptimization:**
  - Read `env.price_signal.electricity_price` for current price
  - On day boundary (detect via current_day_ordinal change): recompute price thresholds. The actor holds a reference to the precomputed price array (passed at construction from the dwelling's tariff evaluator) and slices today's 24h window, computing charge/discharge percentile thresholds.
  - If `current_price < charge_price_threshold` AND `soc < 1.0 - reserve_soc`: emit `PowerSetpoint` at max charge rate
  - If `current_price > discharge_price_threshold` AND `soc > reserve_soc`: emit `PowerSetpoint` at max discharge rate
  - Otherwise: idle (no dispatch)
  - If `solar_only_charging`: only charge from PV during cheap periods (combine SelfConsumption with price gate)

  **BackupReserve:**
  - If `soc < target_soc`: emit `SocTarget { soc: target_soc }` (charge to target)
  - If `charge_from_grid` is false: gate charging with `GridConnect` when no PV
  - `charge_rate_fraction` limits charge power to fraction of rated capacity
  - If `soc >= target_soc`: idle

  **DemandResponse:**
  - Detect active DR via the existing `DrComplianceActor` pattern: the dwelling's `DrCompliance` actor emits `ControlSignal::DemandResponse` dispatches at `Grid` priority. The `BatteryManagementActor` checks if a `DemandResponse` signal was dispatched to this battery in the current step by reading a `dr_active` flag on the equipment's telemetry state (set by the DR compliance actor's dispatch). Alternatively, the BMS actor can cooperate with DR by checking `env.price_signal` for DR-level pricing spikes.
  - If DR active: emit `PowerSetpoint` at `dr_discharge_rate × rated_power` while `soc > min_soc_during_dr` at `Schedule` priority (DR compliance actor at `Grid` tier will override if needed)
  - If DR inactive: delegate to `base_mode` (recursive mode evaluation)

  **Scheduled:**
  - Find matching `BmsScheduleWindow` for current civil time (use `TimeWindow::contains()`)
  - Map `BmsAction` to `ControlSignal`:
    - `Charge { rate_fraction }` → `PowerSetpoint { active_power_kw: rate_fraction * max_charge_kw }`
    - `Discharge { rate_fraction }` → `PowerSetpoint { active_power_kw: -(rate_fraction * max_discharge_kw) }`
    - `Idle` → no dispatch (equipment idles by default)
    - `Hold { target_soc }` → `SocTarget { soc: target_soc }`
  - If no matching window: idle

  **StormWatch:**
  - If trigger is active (ManualEnable flag or WeatherSignal detected): emit `SocTarget { soc: target_soc }` at max charge rate
  - If trigger inactive: delegate to `base_mode`

  **Manual:**
  - Emit nothing. External actor handles all control.

- [ ] All dispatches use `PriorityTier::Schedule` and target battery `ByName(battery_name)`
- [ ] Emit actor telemetry: `BmsTelemetry { mode_name, active_action, soc, price }` each step
- [ ] Register module in `crates/hares-core/src/actors/mod.rs`

## Files to Touch

- `crates/hares-core/src/actors/bms.rs`: New actor implementation
- `crates/hares-core/src/actors/mod.rs`: Add module declaration and re-exports

## Measures of Success

- [ ] `BatteryManagementActor` implements `Actor` trait
- [ ] SelfConsumption mode: charges when PV surplus, discharges when deficit
- [ ] TouOptimization mode: charges during low-price periods, discharges during high-price
- [ ] BackupReserve mode: charges to target_soc, respects charge_from_grid flag
- [ ] DemandResponse mode: discharges during DR events, falls back to base_mode
- [ ] Scheduled mode: maps BmsAction to correct ControlSignal
- [ ] StormWatch mode: charges to target when triggered, delegates otherwise
- [ ] Manual mode: no dispatch requests emitted
- [ ] All dispatches at PriorityTier::Schedule
- [ ] Price percentile thresholds recomputed at day boundary only (not every step)
- [ ] DemandResponse base_mode delegation works with arbitrary nesting depth

## Tests Added

**hares-core:**
- `bms_self_consumption_pv_surplus_charges` — PV > load → charge signal emitted
- `bms_self_consumption_deficit_discharges` — load > PV → discharge signal emitted
- `bms_self_consumption_solar_only_blocks_grid` — no PV → grid charging disabled
- `bms_tou_low_price_charges` — price < charge_threshold → charge at max rate
- `bms_tou_high_price_discharges` — price > discharge_threshold → discharge
- `bms_tou_respects_reserve_soc` — soc ≤ reserve → no discharge
- `bms_backup_reserve_charges_to_target` — soc < target → SocTarget emitted
- `bms_backup_reserve_idle_above_target` — soc ≥ target → no dispatch
- `bms_demand_response_active` — DR signal → discharge at configured rate
- `bms_demand_response_delegates_to_base` — no DR → base_mode dispatch
- `bms_scheduled_charge_window` — matching Charge window → positive PowerSetpoint
- `bms_scheduled_no_matching_window` — no match → no dispatch
- `bms_storm_watch_active_full_charge` — trigger active → SocTarget(1.0)
- `bms_storm_watch_inactive_delegates` — trigger inactive → base_mode dispatch
- `bms_manual_no_dispatch` — Manual mode → empty dispatch buffer

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy --workspace` passes
