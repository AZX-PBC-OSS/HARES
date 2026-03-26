---
id: TARIFF-002
title: Add BmsMode and GridExportRule to hares-types
kind: implement
depends_on:
  - TARIFF-001
files_to_touch:
  - crates/hares-types/src/equipment.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-001.md
verification:
  - cargo build --workspace
  - cargo test -p hares-types
  - cargo clippy --workspace
---

## Background/Context

Residential battery systems (Tesla Powerwall, Enphase IQ, SolarEdge, Sonnen) expose a common set of operating modes: self-consumption, time-of-use optimization, backup reserve, demand response, scheduled windows, and storm watch. These modes determine how the battery charges/discharges relative to PV production, grid prices, and backup needs.

The mode is *configuration* on the battery — it tells the `BatteryManagementActor` (TARIFF-010) how to generate `ControlSignal` dispatches. The battery physics model itself does not read these types.

## Work to Do

- [ ] Add `GridExportRule` enum to `crates/hares-types/src/equipment.rs`:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
  pub enum GridExportRule {
      SolarOnly,
      #[default]
      Unrestricted,
      Disabled,
  }
  ```
- [ ] Add `StormWatchTrigger` enum:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
  pub enum StormWatchTrigger {
      ManualEnable,
      WeatherSignal,
  }
  ```
- [ ] Add `BmsAction` enum:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub enum BmsAction {
      Charge { rate_fraction: f64 },
      Discharge { rate_fraction: f64 },
      Idle,
      Hold { target_soc: f64 },
  }
  ```
- [ ] Add `BmsScheduleWindow` struct:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct BmsScheduleWindow {
      pub time_window: TimeWindow,
      pub action: BmsAction,
  }
  ```
- [ ] Add `BmsMode` enum:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
  pub enum BmsMode {
      SelfConsumption {
          min_soc: f64,
          max_soc: f64,
          solar_only_charging: bool,
      },
      TimeOfUseOptimization {
          reserve_soc: f64,
          charge_threshold_percentile: f64,
          discharge_threshold_percentile: f64,
          solar_only_charging: bool,
      },
      BackupReserve {
          target_soc: f64,
          charge_from_grid: bool,
          charge_rate_fraction: f64,
      },
      DemandResponse {
          base_mode: Box<BmsMode>,
          dr_discharge_rate: f64,
          min_soc_during_dr: f64,
      },
      Scheduled {
          windows: Vec<BmsScheduleWindow>,
      },
      StormWatch {
          target_soc: f64,
          trigger: StormWatchTrigger,
          base_mode: Box<BmsMode>,
      },
      #[default]
      Manual,
  }
  ```
- [ ] Export all new types from `crates/hares-types/src/lib.rs`

## Files to Touch

- `crates/hares-types/src/equipment.rs`: Add `BmsMode`, `BmsAction`, `BmsScheduleWindow`, `GridExportRule`, `StormWatchTrigger`
- `crates/hares-types/src/lib.rs`: Re-export new types

## Measures of Success

- [ ] All 7 `BmsMode` variants construct and compile
- [ ] `Box<BmsMode>` nesting works (StormWatch wrapping SelfConsumption wrapping Manual)
- [ ] `BmsMode` defaults to `Manual`
- [ ] `GridExportRule` defaults to `Unrestricted`
- [ ] All types derive Clone, Debug, PartialEq, Serialize, Deserialize
- [ ] Serde roundtrip works for every variant including nested Box variants
- [ ] Existing equipment tests still pass

## Tests Added

**hares-types:**
- `bms_mode_default_is_manual` — Default trait returns Manual
- `bms_mode_self_consumption_serde` — roundtrip with all fields
- `bms_mode_tou_optimization_serde` — roundtrip with percentile thresholds
- `bms_mode_backup_reserve_serde` — roundtrip with charge_from_grid flag
- `bms_mode_demand_response_nested_serde` — Box<BmsMode> nesting roundtrips
- `bms_mode_scheduled_serde` — Vec<BmsScheduleWindow> with TimeWindow roundtrips
- `bms_mode_storm_watch_nested_serde` — StormWatch wrapping SelfConsumption roundtrips
- `bms_mode_deep_nesting` — 3 levels of Box nesting (StormWatch > DemandResponse > SelfConsumption)
- `grid_export_rule_default_is_unrestricted` — Default trait check
- `grid_export_rule_serde` — all 3 variants roundtrip

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy --workspace` passes
