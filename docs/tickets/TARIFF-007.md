---
id: TARIFF-007
title: URDB v7 JSON importer for ElectricTariff
kind: implement
depends_on:
  - TARIFF-004
files_to_touch:
  - crates/hares-tariff/src/urdb.rs
  - crates/hares-tariff/src/lib.rs
  - tests/fixtures/urdb/flat_rate.json
  - tests/fixtures/urdb/pge_e_tou_c.json
references:
  - docs/tickets/TARIFF-INDEX.md
  - docs/tickets/TARIFF-004.md
  - https://openei.org/services/doc/rest/util_rates/?version=7
verification:
  - cargo build --workspace
  - cargo test -p hares-tariff
  - cargo clippy --workspace
---

## Background/Context

NREL's Utility Rate Database (URDB) is the canonical open database of US utility rates, containing thousands of tariff definitions in structured JSON. Supporting URDB import gives HARES users access to real utility rates without manual construction. The URDB v7 JSON schema uses 12×24 matrices (month × hour) to map time periods, with separate structures for energy charges, demand charges, and metadata.

This ticket implements a parser that converts URDB v7 JSON into HARES `ElectricTariff`. Unsupported fields (reactive power charges, coincident demand lookback windows) produce warnings, not errors. Only structurally invalid JSON returns `Err`.

## Work to Do

- [ ] Create `crates/hares-tariff/src/urdb.rs`
- [ ] Define `UrdbParseError` error type with descriptive messages naming the missing/malformed field
- [ ] Implement `pub fn parse(json: &str) -> Result<ElectricTariff, UrdbParseError>`:
  1. Parse JSON into `serde_json::Value`
  2. Extract `energyweekdayschedule` and `energyweekendschedule` — both are `Vec<Vec<u32>>` (12×24 matrices mapping `[month][hour]` → period index)
  3. Convert schedule matrices into `Vec<TouPeriod>`:
     - Deduplicate period indices to get unique period names ("period_0", "period_1", etc.)
     - For each period index, scan the matrix to build `Vec<TimeWindow>` entries with appropriate `DayFilter` (Weekdays/Weekends) and hour ranges
     - Determine `SeasonFilter` by checking which months use each period
  4. Extract `energyratestructure` — `Vec<Vec<{rate, adj, max, unit, sell}>>` — convert to `Vec<EnergyRate>` and `Vec<TieredBlock>`
  5. Extract `flatdemandstructure` and `demandratestructure` → `Vec<DemandRate>`
  6. Extract `demandweekdayschedule`/`demandweekendschedule` if present (demand TOU periods)
  7. Extract `fixedchargefirstmeter` and `fixedchargeunits` → `FixedCharges`
  8. Extract `minmonthlycharge` → `minimum_charge`
  9. Extract `demandratchetpercentage` → `RatchetConfig` on demand rates
  10. Extract `dgrules` → `ExportMode` mapping:
      - "Net Metering" → `NetMetering`
      - "Net Billing Instantaneous"/"Net Billing Hourly" → `NetBilling`
      - "Buy All Sell All" → `FlatRate` (using sell rate)
      - absent/unknown → `ExportMode::None`
  11. Log warnings (via `tracing::warn!`) for unsupported fields: `reactivepowercharge`, `voltagecategory`, `phasewiring`
- [ ] Create test fixtures:
  - `tests/fixtures/urdb/flat_rate.json` — simple flat rate (single period, no TOU)
  - `tests/fixtures/urdb/pge_e_tou_c.json` — PG&E E-TOU-C with summer/winter TOU, tiered blocks, demand charges
  - Obtain real URDB data via OpenEI API or construct representative fixtures matching the schema
- [ ] Register module in `crates/hares-tariff/src/lib.rs`

## Files to Touch

- `crates/hares-tariff/src/urdb.rs`: New file with URDB v7 parser
- `crates/hares-tariff/src/lib.rs`: Add module declaration and re-exports
- `tests/fixtures/urdb/flat_rate.json`: Test fixture — simple flat rate
- `tests/fixtures/urdb/pge_e_tou_c.json`: Test fixture — complex TOU rate

## Measures of Success

- [ ] Flat rate fixture parses to `ElectricTariff` with single period covering all hours
- [ ] Complex TOU fixture parses with correct number of TOU periods (3-4), energy rates, and demand rates
- [ ] Parsed rate values match source JSON within 0.1% (rounding only)
- [ ] Missing optional fields (dgrules, demand) produce valid tariff with None/empty defaults
- [ ] Structurally invalid JSON (missing energyratestructure) returns `Err` with descriptive message
- [ ] Unsupported fields logged as warnings, not errors
- [ ] Unknown JSON fields are tolerated (no deny_unknown_fields)

## Tests Added

**hares-tariff:**
- `urdb_flat_rate_parses` — flat_rate.json produces single TouPeriod, correct energy rate
- `urdb_pge_tou_c_parses` — pge_e_tou_c.json produces expected period count and rate values
- `urdb_pge_tou_c_demand_rates` — demand charges parsed with ratchet config
- `urdb_pge_tou_c_seasonal_split` — summer/winter periods correctly assigned
- `urdb_missing_energy_structure_errors` — JSON without energyratestructure → Err
- `urdb_missing_optional_fields_ok` — JSON without dgrules/demand → Ok with defaults
- `urdb_unknown_fields_tolerated` — extra JSON keys don't cause parse failure
- `urdb_rate_values_match_source` — spot-check parsed rates against fixture values

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-tariff` passes
- [ ] `cargo clippy --workspace` passes
