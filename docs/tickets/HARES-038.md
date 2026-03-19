---
id: HARES-038
title: "hares-io — Output Writer (Arrow Streaming)"
kind: implement
depends_on: [HARES-002, HARES-001, HARES-036]
files_to_touch:
  - crates/hares-io/src/output/mod.rs
  - crates/hares-io/src/output/writer.rs
  - crates/hares-io/src/output/columns.rs
  - crates/hares-io/src/lib.rs
references:
  - docs/architecture/06-input-output.md
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
HARES simulations can span a full year at 1-minute resolution, producing ~500 000 rows and potentially hundreds of columns. Accumulating all rows in memory before writing would make fleet-scale runs infeasible. The output layer therefore uses Arrow RecordBatch streaming: rows are buffered up to a configurable chunk size and flushed to Parquet or CSV incrementally, keeping peak memory proportional to chunk size rather than simulation length. Column naming must be OCHRE-compatible at every verbosity level so that downstream analysis tooling works without modification.

## Work to Do
- [ ] Implement `output/columns.rs`: schema and column name definitions
  - [ ] Define verbosity level semantics matching the arch doc exactly:
    - Level 0: total electric power (kW) and total gas power (kW) only
    - Level 1: level 0 + per-equipment electric and gas power (HVAC, WH, appliances, lighting, EV, PV, battery)
    - Level 2: level 1 + zone temperatures (°C) and unmet loads (kW) per zone
    - Level 3: level 2 + equipment modes, setpoints, SOC
    - Level 4: level 3 + energy (kWh) per end-use
    - Level 5: level 4 + reactive power (kVAR) and power factor per equipment
    - Level 6: level 5 + component loads per boundary (envelope diagnostics)
    - Level 7: level 6 + schedule inputs
    - Level 8: level 7 + all individual equipment state variables
  - [ ] Multi-instance column naming convention: `"Battery #1 SOC (-)"`, `"PV South Electric Power (kW)"` — unit and orientation encoded in name, matching OCHRE
  - [ ] Mode columns: encode operating mode as an integer ordinal in `push_row` values; document the enum-to-ordinal mapping as a code comment in `columns.rs` so that output consumers can decode it. Also embed the mapping as Parquet custom metadata (key: `hares_mode_map`, value: JSON) so output files are self-describing without requiring users to find the Rust source comment.
  - [ ] `build_schema(equipment_list: &[EquipmentSpec], verbosity: u8) -> arrow::datatypes::Schema`
  - [ ] Column name registry: `expected_columns_at_verbosity(verbosity: u8) -> Vec<&'static str>` for use in tests
- [ ] Implement `output/writer.rs`: `StreamingRecorder` struct
  - [ ] Constructor: `StreamingRecorder::new(schema: Schema, chunk_size: usize, format: OutputFormat, output_path: &Path) -> Result<Self>`
  - [ ] `push_row(&mut self, values: &[f64]) -> Result<()>` — append row to in-memory buffer; flush when buffer reaches `chunk_size`; mode columns are passed as integer ordinals cast to `f64`
  - [ ] `flush(&mut self) -> Result<()>` — write current buffer as a RecordBatch and reset; no-op if buffer is empty
  - [ ] `finish(self) -> Result<OutputSummary>` — flush remaining rows, close file, return row count and byte size
  - [ ] Parquet writer: use `parquet` crate with `WriterProperties` set to `SNAPPY` compression by default
  - [ ] CSV writer: use `arrow-csv` crate; write header on first flush only
  - [ ] Peak memory must be bounded: buffer holds at most `chunk_size` rows at any time; no accumulation across flushes
- [ ] Implement `output/mod.rs`: module declaration and `OutputSummary` struct
- [ ] Re-export `StreamingRecorder`, `build_schema`, `OutputSummary` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/output/mod.rs`: new file — module declarations, `OutputSummary`
- `crates/hares-io/src/output/writer.rs`: new file — `StreamingRecorder` streaming flush logic
- `crates/hares-io/src/output/columns.rs`: new file — verbosity-level column definitions, `build_schema`
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] Writing 100 000 rows with `chunk_size = 10000` produces exactly 10 flush operations and no unbounded memory growth (verify via buffer length assertions in tests)
- [ ] Column names at verbosity 0 match the OCHRE output header for total electric/gas power exactly
- [ ] Column names at verbosity 1 include all expected per-end-use names from the OCHRE column registry
- [ ] A Parquet file written and then read back with `parquet` produces identical `f64` values (bit-exact round-trip for non-NaN finite values)
- [ ] `finish()` on an empty recorder produces a valid zero-row output file without panicking

## Performance Notes
- **P0 — Telemetry by-ref downstream**: The output writer now reads `Equipment::telemetry()` as `&Telemetry` (borrowed reference) rather than receiving a cloned `HashMap`. This eliminates one `HashMap` clone per equipment per timestep during output recording. See HARES-018 Performance Notes for the trait-level change.

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
