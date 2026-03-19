---
id: HARES-040
title: "hares-io — ResStock Metadata"
kind: implement
depends_on: [HARES-002, HARES-001]
files_to_touch:
  - crates/hares-io/src/resstock.rs
  - crates/hares-io/src/lib.rs
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
ResStock releases (2024.1, 2024.2, 2025.1) store per-building metadata in a Parquet file (`results_up00.parquet`) with versioned column schemas. The fleet runner needs to enumerate buildings, resolve file paths for HPXML, schedule, and weather inputs, and carry `sample_weight` through to aggregated outputs. Column names differ across ResStock versions, so a versioned `ColumnMapper` trait abstracts the differences without branching in fleet logic.

## Work to Do
- [ ] Implement `resstock.rs`: ResStock metadata types and loader
  - [ ] `ResStockVersion` enum: `V2024_1`, `V2024_2`, `V2025_1`
  - [ ] `ResStockBuilding` struct:
    - `bldg_id: i64`
    - `upgrade: i64`
    - `sample_weight: f64`
    - `hpxml_path: PathBuf`
    - `schedule_path: PathBuf`
    - `weather_path: PathBuf`
    - `characteristics: HashMap<String, String>` — all remaining string columns as key/value pairs
  - [ ] `ColumnMapper` trait:
    - `fn bldg_id_col(&self) -> &str`
    - `fn sample_weight_col(&self) -> &str`
    - `fn upgrade_col(&self) -> &str`
    - `fn map_characteristics(&self, row: &RecordBatch, row_idx: usize) -> Result<HashMap<String, String>>`
  - [ ] Implement `ColumnMapper` for each `ResStockVersion`: map to the correct column names for that release
  - [ ] `parse_resstock_metadata(path: &Path, version: ResStockVersion, dataset_root: &Path) -> Result<Vec<ResStockBuilding>>`
    - [ ] Read Parquet file using `parquet` + `arrow` crates
    - [ ] Use the appropriate `ColumnMapper` to extract `bldg_id`, `upgrade`, `sample_weight`
    - [ ] Construct `hpxml_path`, `schedule_path`, `weather_path` from `dataset_root` and building ID fields using the upgrade-aware path template: `building_energy_models/{state}/up{upgrade:02d}-baseline/bldg{id}.zip`. Path template must be configurable per `ResStockVersion` via the `ColumnMapper` trait. Verify template against ResStock 2024.1, 2024.2, and 2025.1 release directories.
    - [ ] Propagate `sample_weight` exactly as stored (no rounding)
    - [ ] Range string conversion: strings matching `\d+-\d+` are parsed as numeric ranges (midpoint = (lower+upper)/2.0) and stored as strings in `characteristics` (e.g. `"1500-1999"` → `"1749.5"`). All other strings are stored verbatim.
    - [ ] Exclude `out.*` columns from `characteristics`: these are ResStock simulation output metrics, not building input characteristics, and must not appear in the `characteristics` map
    - [ ] Version mismatch detection: if the `ColumnMapper` for the configured version cannot find its expected columns (e.g. a V2024_1 mapper applied to a V2024_2 file), return a typed error with a message identifying the version mismatch and naming the missing column
  - [ ] Return typed errors for missing expected columns, with the column name in the error message
- [ ] Re-export `ResStockBuilding`, `ResStockVersion`, `parse_resstock_metadata` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/resstock.rs`: new file — `ResStockBuilding`, `ResStockVersion`, `ColumnMapper` trait and impls, Parquet loader
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] Parsing a ResStock 2024.1 fixture Parquet produces the correct `bldg_id` and `sample_weight` for every row
- [ ] Column mapping for `V2024_2` and `V2025_1` fixtures correctly resolves alternate column names without error
- [ ] A Parquet file missing the `sample_weight` column returns a typed error naming `sample_weight_col()`
- [ ] `hpxml_path` for building ID 12345, upgrade 3, state "CO" resolves to `building_energy_models/CO/up03-baseline/bldg12345.zip` relative to `dataset_root`
- [ ] `in.*` column with value `"1500-1999"` is stored in `characteristics` as `"1749.5"`
- [ ] A malformed range string like `"N/A"` is stored verbatim, not parsed
- [ ] `out.*` columns are absent from the `characteristics` map
- [ ] Applying a `V2024_2` `ColumnMapper` to a `V2024_1` Parquet file (where a V2024_2-specific column is missing) returns a typed error identifying the version mismatch

## Architecture Alignment Notes
- **`ColumnMapper` trait**: Extends the architecture's trait definition with `upgrade_col()` and uses `(&RecordBatch, row_idx)` instead of `&ArrowRow` for `map_characteristics`. This is an intentional improvement — `RecordBatch` is the native Arrow columnar type and avoids materializing per-row wrappers, which aligns better with Arrow's columnar access patterns.
- **`ResStockBuilding` struct**: Extends the architecture's struct definition with `upgrade: i64` (needed for upgrade-aware path templates) and `characteristics: HashMap<String, String>` (captures all remaining string columns as key/value pairs for downstream use). These fields are required by the implementation but were not in the original architecture spec.

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
