---
id: TARIFF-001
title: Add SeasonFilter, BillingCycle, and TouPeriod to hares-types
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-types/src/schedule.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/tickets/TARIFF-INDEX.md
verification:
  - cargo build --workspace
  - cargo test -p hares-types
  - cargo clippy --workspace
---

## Background/Context

HARES needs a utility tariff system at the dwelling/simulation level to support time-of-use rates, seasonal pricing, and demand charges. Before building the tariff evaluator, we need foundational types that describe when rates apply. These types extend the existing `TimeWindow`/`DayFilter` schedule infrastructure with season awareness and billing period configuration.

These are pure data types with no evaluation logic — they will be consumed by the `hares-tariff` crate (TARIFF-004+) and the actor system.

## Work to Do

- [ ] Add `SeasonFilter` enum to `crates/hares-types/src/schedule.rs`:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
  pub enum SeasonFilter {
      #[default]
      All,
      Summer,
      Winter,
  }
  ```
- [ ] Implement `SeasonFilter::contains_month(self, month: u8) -> bool`:
  - Summer = months 6..=9 (June through September)
  - Winter = months 1..=5 and 10..=12
  - All = always true
  - Debug-assert on month outside 1..=12; in release, return `false` for invalid months
- [ ] Add `SeasonalSplit` struct:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct SeasonalSplit {
      pub summer_start_month: u8,  // inclusive, 1-indexed
      pub summer_end_month: u8,    // inclusive, 1-indexed
  }
  ```
- [ ] Implement `SeasonalSplit::is_summer(month: u8) -> bool` with wrapping support (e.g., Nov–Feb winter in southern hemisphere)
- [ ] Add `BillingCycle` enum:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
  pub enum BillingCycle {
      #[default]
      Monthly,
      Custom(u32),  // days per billing period
  }
  ```
- [ ] Add `TouPeriod` struct:
  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct TouPeriod {
      pub name: String,
      pub schedule: Vec<TimeWindow>,
      pub season: SeasonFilter,
  }
  ```
- [ ] Export all new types from `crates/hares-types/src/lib.rs`

## Files to Touch

- `crates/hares-types/src/schedule.rs`: Add `SeasonFilter`, `SeasonalSplit`, `BillingCycle`, `TouPeriod` types
- `crates/hares-types/src/lib.rs`: Re-export new types

## Measures of Success

- [ ] `SeasonFilter::All.contains_month(m)` returns `true` for all months 1..=12
- [ ] `SeasonFilter::Summer.contains_month(7)` returns `true`
- [ ] `SeasonFilter::Summer.contains_month(12)` returns `false`
- [ ] `SeasonFilter::Winter.contains_month(1)` returns `true`
- [ ] `SeasonFilter::Winter.contains_month(7)` returns `false`
- [ ] `BillingCycle` defaults to `Monthly`
- [ ] `TouPeriod` serializes/deserializes correctly including nested `TimeWindow` and `SeasonFilter`
- [ ] All types derive Clone, Debug, PartialEq, Serialize, Deserialize
- [ ] Existing schedule tests still pass

## Tests Added

**hares-types:**
- `season_filter_contains_month_all` — All returns true for every month
- `season_filter_contains_month_summer` — Summer covers June–September only
- `season_filter_contains_month_winter` — Winter covers October–May only
- `season_filter_debug_asserts_invalid_month` — month 0 or 13 debug-panics, returns false in release
- `seasonal_split_is_summer` — custom split with wrapping support
- `billing_cycle_default_is_monthly` — Default trait returns Monthly
- `billing_cycle_serde_roundtrip` — Monthly and Custom(14) serialize/deserialize
- `tou_period_serde_roundtrip` — Full roundtrip with TimeWindow schedule and SeasonFilter

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy --workspace` passes
