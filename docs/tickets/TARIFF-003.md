---
id: TARIFF-003
title: Extend ChargingStrategy with V2H, V2G, SolarSurplus and DepartureConstraint
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

The existing `ChargingStrategy` enum has basic variants (Immediate, Nightly, LowSoc, QuickThenWait, PreDeparture, TouAware) but lacks realistic bidirectional modes (V2H, V2G), solar surplus charging, and structured departure constraints with day-of-week filtering. Real EVSEs (Tesla Wall Connector, Wallbox, Emporia) support these strategies, and the `EVChargingActor` (TARIFF-011) needs them to make tariff-aware decisions.

This ticket extends the existing enum rather than replacing it, preserving backward compatibility for existing variants.

## Work to Do

- [ ] Add `DepartureConstraint` struct to `crates/hares-types/src/equipment.rs`:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct DepartureConstraint {
      pub day_filter: DayFilter,
      pub departure_minute: u32,   // minute of day, 0..1440
      pub target_soc: f64,
  }
  ```
- [ ] Add new variants to `ChargingStrategy`:
  ```rust
  // Add to existing enum:
  SolarSurplus {
      min_charge_rate_kw: f64,
      departure_schedule: Vec<DepartureConstraint>,
  },
  V2H {
      discharge_threshold_soc: f64,
      min_soc: f64,
  },
  V2G {
      min_soc: f64,
      max_export_kw: f64,
      price_threshold: f64,
  },
  ```
- [ ] Update existing `TouAware` variant to include departure schedule and buffer:
  ```rust
  TouAware {
      target_soc: f64,
      #[serde(default)]
      departure_schedule: Vec<DepartureConstraint>,
      #[serde(default = "default_charge_buffer_hours")]
      charge_buffer_hours: f64,  // default: 2.0
  },
  ```
  Backward compat: `#[serde(default)]` on new fields ensures old JSON `{"TouAware":{"target_soc":0.85}}` still deserializes because serde's externally-tagged enum representation includes all struct fields, and new ones get defaults. Verify this with an explicit backward compat test.
- [ ] Update `PreDeparture` variant to use `DepartureConstraint`:
  ```rust
  PreDeparture {
      target_soc: f64,
      #[serde(default)]
      departure_schedule: Vec<DepartureConstraint>,
  },
  ```
  Same serde default approach.
- [ ] Export `DepartureConstraint` from `crates/hares-types/src/lib.rs`
- [ ] Update existing `ChargingStrategy` serde tests to cover new variants
- [ ] Update existing serde tests to verify backward compat (old JSON without new fields still parses)

## Files to Touch

- `crates/hares-types/src/equipment.rs`: Add `DepartureConstraint`, extend `ChargingStrategy` with V2H/V2G/SolarSurplus, update TouAware/PreDeparture
- `crates/hares-types/src/lib.rs`: Re-export `DepartureConstraint`

## Measures of Success

- [ ] All existing `ChargingStrategy` variants still compile and roundtrip
- [ ] Old JSON without departure_schedule/charge_buffer_hours deserializes (serde defaults)
- [ ] `V2H` and `V2G` variants construct and roundtrip
- [ ] `SolarSurplus` with departure constraints roundtrips
- [ ] `DepartureConstraint` with various `DayFilter` values roundtrips
- [ ] `departure_minute` range is documented as 0..1440
- [ ] Existing ChargingStrategy tests still pass

## Tests Added

**hares-types:**
- `charging_strategy_v2h_serde` — V2H roundtrip with soc thresholds
- `charging_strategy_v2g_serde` — V2G roundtrip with price threshold
- `charging_strategy_solar_surplus_serde` — SolarSurplus with departure constraints
- `charging_strategy_tou_aware_backward_compat` — old JSON `{"TouAware":{"target_soc":0.85}}` still parses
- `charging_strategy_pre_departure_backward_compat` — old JSON still parses
- `departure_constraint_serde` — roundtrip with DayFilter::Weekdays and specific day
- `charging_strategy_tou_aware_with_departures` — full roundtrip with departure schedule

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy --workspace` passes
