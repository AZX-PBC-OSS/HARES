---
id: TARIFF-004
title: Create hares-tariff crate with ElectricTariff and GasTariff types
kind: implement
depends_on:
  - TARIFF-001
files_to_touch:
  - crates/hares-tariff/Cargo.toml
  - crates/hares-tariff/src/lib.rs
  - crates/hares-tariff/src/types.rs
  - Cargo.toml
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-001.md
verification:
  - cargo build --workspace
  - cargo test -p hares-tariff
  - cargo clippy --workspace
---

## Background/Context

The utility tariff system needs its own crate because tariff evaluation involves non-trivial logic (precomputed price arrays, billing state tracking, demand charge accumulation, URDB parsing). Keeping this in a dedicated crate maintains clean dependency boundaries: `hares-tariff` depends on `hares-types` for schedule primitives but nothing else. Equipment crates do NOT depend on `hares-tariff` — the actor layer bridges them.

This ticket scaffolds the crate and adds the core data model. No evaluation logic yet.

## Work to Do

- [ ] Create `crates/hares-tariff/Cargo.toml`:
  ```toml
  [package]
  name = "hares-tariff"
  version = "0.1.0"
  edition = "2021"

  [dependencies]
  hares-types = { path = "../hares-types" }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  ```
- [ ] Add `"crates/hares-tariff"` to workspace members in root `Cargo.toml`
- [ ] Create `crates/hares-tariff/src/lib.rs` with module declarations and re-exports
- [ ] Create `crates/hares-tariff/src/types.rs` with all tariff data types:

  **Electric tariff types:**
  ```rust
  pub struct ElectricTariff {
      pub name: Option<String>,
      pub tou_schedule: Vec<TouPeriod>,
      pub energy_rates: Vec<EnergyRate>,
      pub demand_rates: Vec<DemandRate>,
      pub tiered_rates: Vec<TieredBlock>,
      pub export_rate: ExportRate,
      pub fixed_charges: FixedCharges,
      pub minimum_charge: Option<f64>,       // $/month floor
      pub billing_cycle: BillingCycle,
      pub seasonal_split: Option<SeasonalSplit>,
  }

  pub struct EnergyRate {
      pub period_name: String,
      pub season: SeasonFilter,
      pub rate_per_kwh: f64,
  }

  pub struct DemandRate {
      pub period_name: Option<String>,       // None = coincident peak
      pub season: SeasonFilter,
      pub rate_per_kw: f64,
      pub ratchet: Option<RatchetConfig>,
  }

  pub struct RatchetConfig {
      pub lookback_months: u8,               // typically 11 or 12
      pub minimum_fraction: f64,             // e.g., 0.85 = 85% of prior peak
  }

  pub struct TieredBlock {
      pub season: SeasonFilter,
      pub thresholds_kwh: Vec<f64>,          // cumulative upper bounds per tier
      pub rates_per_kwh: Vec<f64>,           // rate for each tier (len = thresholds + 1)
  }

  pub struct ExportRate {
      pub mode: ExportMode,
      pub tou_credits: Vec<EnergyRate>,
  }

  pub enum ExportMode {
      NetMetering,
      NetBilling,
      FlatRate(f64),
      None,
  }

  pub struct FixedCharges {
      pub monthly_usd: f64,
      pub daily_usd: f64,
  }
  ```

  **Gas tariff types:**
  ```rust
  pub struct GasTariff {
      pub name: Option<String>,
      pub tiered_rates: Vec<GasTieredBlock>,
      pub fixed_charges: FixedCharges,
      pub billing_cycle: BillingCycle,
      pub seasonal_split: Option<SeasonalSplit>,
  }

  pub struct GasTieredBlock {
      pub season: SeasonFilter,
      pub thresholds_therms: Vec<f64>,
      pub rates_per_therm: Vec<f64>,
  }
  ```

- [ ] All types derive `Clone, Debug, PartialEq, Serialize, Deserialize`
- [ ] `ExportMode` and `FixedCharges` derive `Default` (None and zeroes respectively)
- [ ] `ElectricTariff` and `GasTariff` derive `Default` with empty vecs and sensible zero defaults

## Files to Touch

- `Cargo.toml`: Add `hares-tariff` to workspace members
- `crates/hares-tariff/Cargo.toml`: New crate manifest
- `crates/hares-tariff/src/lib.rs`: Module declarations and re-exports
- `crates/hares-tariff/src/types.rs`: All tariff data types

## Measures of Success

- [ ] `hares-tariff` crate compiles as workspace member
- [ ] No circular dependencies in workspace
- [ ] `ElectricTariff` with TOU periods, energy rates, demand rates, tiers serializes to JSON and deserializes back
- [ ] `GasTariff` with tiered rates serializes/deserializes
- [ ] Default instances compile and produce sensible empty tariffs
- [ ] `TieredBlock` validates `rates_per_kwh.len() == thresholds_kwh.len() + 1` (document this contract)

## Tests Added

**hares-tariff:**
- `electric_tariff_default_roundtrip` — default() serializes/deserializes to empty tariff
- `electric_tariff_full_roundtrip` — construct tariff with 3 TOU periods, 3 energy rates, 1 demand rate with ratchet, 2 tiered blocks, net metering export, fixed charges; roundtrip through JSON
- `gas_tariff_roundtrip` — construct with seasonal tiered rates, roundtrip
- `export_mode_variants` — all 4 ExportMode variants roundtrip
- `tiered_block_rate_count_invariant` — document that rates_per_kwh.len() == thresholds_kwh.len() + 1

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-tariff` passes
- [ ] `cargo clippy --workspace` passes
