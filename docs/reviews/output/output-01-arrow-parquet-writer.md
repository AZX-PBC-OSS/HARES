# Arrow/Parquet output writer: schema stability, column types, file rotation
**Review ID**: output-01
**Category**: output
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/output/writer.rs` (447 lines)
- `crates/hares-io/src/output/columns.rs` (760 lines)

Supporting context:
- `crates/hares-io/src/config.rs` (OutputFormat enum, SimulationConfig)
- `crates/hares-io/src/output/mod.rs` (OutputSummary, re-exports)
- `crates/hares-core/src/dwelling/mod.rs` (timestamp formatting at line 3141, record_row at lines 3120-3148)
- `crates/hares-core/src/clock.rs` (SimClock, offset handling)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc` (lines 1-1032+)
  - EnergyPlus uses `ReportFreq` enum with levels `EachCall`, `TimeStep`, `Hour`, `Day`, `Month`, `Simulation`, `Year` (line 347-355) for output frequency / file rotation.
  - EnergyPlus uses `Real64` (double) uniformly for variable storage.
  - No Arrow/Parquet patterns exist in the reference; comparison focuses on output strategy and frequency management.

## Findings

### Finding 1: [Severity: high] All flushed RecordBatches are retained in memory, defeating the streaming design
**Description**: The `flushed_batches` field on `StreamingRecorder` (`writer.rs:52`) accumulates every flushed `RecordBatch` via `self.flushed_batches.push(batch)` at line 154. Each `RecordBatch` holds `Arc<dyn Array>` references to the actual column data (`Float64Array`, `StringArray`). Since `build_batch()` (line 215) drains the internal buffers with `std::mem::take`, the data transitions from the per-column `Vec<f64>` / `Vec<String>` into `Array` objects inside the `RecordBatch`. After the batch is written to Parquet/CSV, it is stored in `flushed_batches`, where it remains alive until the recorder is dropped.

The doc comment on line 45 states:
> Peak memory is bounded: the buffer holds at most `chunk_size` rows.

This is false when `flushed_batches` accumulates all batches. For a 1-year simulation at 60-second timesteps with 50 Float64 columns: 525,600 rows × 50 cols × 8 bytes = ~210 MB of numeric data kept in memory indefinitely, in addition to timestamp strings and Arrow metadata.

The `flushed_batches()` accessor (line 184) is called in production code at:
- `crates/hares-core/src/engine.rs:128,217` (converts to `Vec`)
- `crates/hares-python/src/py_dwelling.rs:600,1122,1130` (converts to `Vec` for Python bindings)

**Code Location**: `crates/hares-io/src/output/writer.rs:52,154,184-186`
**Root Cause**: The `flushed_batches` Vec serves as a post-hoc accessor for all output data, but this conflicts with the stated design goal of bounded peak memory. There is no mechanism to opt out of retention.
**Impact**: For multi-year simulations or large dwellings with many equipment columns, memory can grow to hundreds of MB or GB, potentially causing OOM. Hides the true memory cost from callers who trust the documented bounded-memory contract.

### Finding 2: [Severity: high] No file rotation mechanism for long simulations
**Description**: `StreamingRecorder::new` (`writer.rs:63-108`) creates exactly one output file via `File::create(output_path)` (line 77), which truncates any existing file. There is no mechanism to rotate output by time interval (hourly, daily, monthly) or by file size. The `SimulationConfig` struct (`config.rs:35`) has no field for rotation strategy.

**Comparison with EnergyPlus**: EnergyPlus defines a `ReportFreq` enum (`OutputProcessor.cc:347-355`) with levels `EachCall`, `TimeStep`, `Hour`, `Day`, `Month`, `Simulation`, `Year`, allowing output to accumulate at different frequencies. EnergyPlus writes to separate CSV/ESO streams for each frequency. HARES has no equivalent.

**Code Location**: `crates/hares-io/src/output/writer.rs:77`, `crates/hares-io/src/config.rs:35-83`
**Root Cause**: The output infrastructure was designed for single-dwelling, short-duration simulations. No requirement for multi-file output was captured.
**Impact**: For year-long or multi-year simulations at fine timesteps, a single Parquet/CSV file can exceed practical limits (e.g., 500+ MB for a single dwelling). Downstream analysis tools may struggle to open such files. Restarting a simulation truncates prior output (no append mode).

### Finding 3: [Severity: medium] All numeric columns use Float64 — no Float32 for temperatures, no Int32 for integer-coded values
**Description**: Every non-timestamp column in `build_schema` (`columns.rs:65-315`) is declared as `Field::new(..., DataType::Float64, true)`. In the writer, all value buffers are `Vec<f64>` (line 48) and batches are built from `Float64Array` (line 226).

Specific type-appropriateness concerns:
- **Temperatures** (outdoor dry bulb, zone/attic/ground temps, hot water mains, setpoints): these are rendered in °C with 0.01°C being more than sufficient for building simulation. Float32 provides ~7 decimal digits of precision, far exceeding the 0.01°C requirement. Float64 doubles storage size for no benefit.
- **Mode ordinals** (operating mode, defrost state, schedule inputs): these are integer-coded categorical values (0, 1, 2, …). Storing them as Float64 wastes 4 bytes per value and loses semantic type information. Int32 would be both smaller and semantically correct.
- **Power factor** and **SOC** are dimensionless ratios in [0, 1]; Float32 is more than sufficient.
- **Cumulative energy** (kWh): Float64 is appropriate here due to large cumulative sums over long simulations.

**Comparison with EnergyPlus**: EnergyPlus stores all values as `Real64` (C++ `double`), making the same tradeoff. However, EnergyPlus predates columnar formats and does not benefit from Parquet's columnar compression. HARES should take advantage of narrower types since Parquet's dictionary encoding and run-length encoding work better with integer types and smaller floats.

**Code Location**: `crates/hares-io/src/output/columns.rs:68-303` (all `DataType::Float64`), `crates/hares-io/src/output/writer.rs:48,226`
**Root Cause**: Uniform `f64` was chosen for implementation simplicity and OCHRE CSV compatibility (OCHRE uses float64 throughout).
**Impact**: Parquet files are up to 2× larger for temperature/ordinal columns than necessary. Integer-valued columns lose semantic type information, making schema introspection less useful for downstream tools. Memory usage during simulation is higher.

### Finding 4: [Severity: medium] No schema stability regression tests
**Description**: While `build_schema` is deterministic in practice (the internal `HashMap` in `instance_qualified_names` at `columns.rs:396-401` is used only for counting and lookup; output order is driven by the input `specs` slice), there are no tests that verify:
1. Calling `build_schema` twice with identical inputs produces identical schemas.
2. The field count, field names, field types, and field order are identical across repeated runs.
3. Schema metadata (`hares_verbosity`, `hares_mode_map`) is identical across repeated runs.

The existing tests in `columns.rs:464-760` verify specific columns appear at specific verbosity levels but do not assert full schema identity. The writer tests in `writer.rs:234-447` test round-trip correctness but not schema consistency across separate construction calls.

**Code Location**: `crates/hares-io/src/output/columns.rs:396-401` (HashMap usage), test module at lines 464-760
**Root Cause**: Testing focused on column presence and content correctness, not schema-level identity guarantees required by downstream Parquet consumers.
**Impact**: A refactor that accidentally changes column ordering, field nullable flags, or metadata insertion order could produce schemas that differ from previously written files. Downstream Parquet tools that merge or compare files by schema would reject or misread data.

### Finding 5: [Severity: medium] Timestamps stored as Utf8 strings instead of Parquet native timestamp type
**Description**: The timestamp column is declared as `Field::new(TIMESTAMP_COL, DataType::Utf8, false)` at `columns.rs:65` and built as `StringArray` at `writer.rs:221`. The actual values are RFC 3339 strings with offset (e.g., `2024-01-01T00:00:00-07:00`) formatted at `crates/hares-core/src/dwelling/mod.rs:3141` via chrono's `%+` specifier.

Parquet supports native `Timestamp` logical types (`TimestampNanosecond`, `TimestampMicrosecond`, etc.) with optional UTC adjustment. These enable:
- Efficient timestamp range queries (`WHERE time BETWEEN ...`)
- Automatic timezone normalization in query engines
- Columnar compression (dictionary encoding for repetitive date prefixes)
- Smaller storage footprint (8 bytes vs ~25 bytes per value)

**Code Location**: `crates/hares-io/src/output/columns.rs:65`, `crates/hares-io/src/output/writer.rs:221`
**Root Cause**: String timestamps are simpler to implement and are the OCHRE CSV convention. The `FixedOffset` from `SimClock` is preserved in the string format.
**Impact**: Downstream tools (Pandas, DuckDB, Polars) must parse timestamps from strings, which is slower than reading native Parquet timestamps. Storage is ~3× larger for the timestamp column. Timezone-aware queries require extra parsing steps.

### Finding 6: [Severity: low] `build_batch` hardcodes type assumptions with no defensive column-type validation
**Description**: `build_batch` (`writer.rs:215-231`) constructs columns as: 1× `StringArray` (timestamp) + (N-1)× `Float64Array` (values). It derives the number of value columns from `self.schema.fields().len() - 1` (line 74), implicitly assuming the first field is always `Utf8` and all remaining fields are `Float64`. If `build_schema` were modified to add a non-`Float64` column at index > 0, `build_batch` would silently create a `Float64Array` for it, and Arrow's `RecordBatch::try_new` (line 229) would catch the type mismatch. However, the error message would say "Invalid argument error: column types must match" with no indication of which column mismatched or what the expected vs actual types were.

**Code Location**: `crates/hares-io/src/output/writer.rs:74-75,215-231`
**Root Cause**: The batch construction is tightly coupled to the "one Utf8 + N Float64" schema convention without asserting it at construction time.
**Impact**: Low — `build_schema` currently produces only Utf8 and Float64 columns. If that changes, the error will still be caught at `RecordBatch::try_new`. The unclear error message is a minor debugging inconvenience.

### Finding 7: [Severity: low] No upper bound validation on `chunk_size`
**Description**: `StreamingRecorder::new` (`writer.rs:69-71`) validates `chunk_size > 0` but does not enforce an upper bound. Each value buffer pre-allocates `Vec::with_capacity(chunk_size)` (line 101), so a malicious or mistyped config value like `output_chunk_size = 2_000_000_000` would allocate ~2 billion × 8 bytes = 16 GB per value column at construction time.

**Code Location**: `crates/hares-io/src/output/writer.rs:69-71,101`
**Root Cause**: The `SimulationConfig` accepts any `usize` for `output_chunk_size` (`config.rs:69`) without range validation.
**Impact**: An accidentally large chunk size causes an immediate OOM at recorder construction, before any simulation work begins. The error is a standard Rust allocation panic with no domain-specific message.

### Finding 8: [Severity: low] `instance_qualified_names` uses `HashMap` — deterministic in practice but fragile
**Description**: `instance_qualified_names` (`columns.rs:395-418`) uses `HashMap` for two purposes:
1. Counting name occurrences (`counts` HashMap, line 399)
2. Assigning instance indices (`indices` HashMap, line 405)

The output `result` vector is built by iterating over the input `specs` slice in order (line 407), so column ordering is deterministic regardless of HashMap iteration order. However, this relies on the invariant that HashMap lookups for counts and indices produce correct values regardless of internal iteration. While this is mathematically correct, the use of a non-deterministic data structure for lookups inside a function whose output must be deterministic is a code smell — `IndexMap` from `indexmap` crate or `BTreeMap` would make the intent explicit.

**Code Location**: `crates/hares-io/src/output/columns.rs:396,405`
**Root Cause**: `HashMap` is the idiomatic Rust default for maps, but schema construction is a context where determinism is critical.
**Impact**: None today — the HashMap is only used for `.entry().or_insert()` and `.get()` lookups, not iteration. A future developer who adds iteration over `counts` or `indices` would introduce non-deterministic column ordering. The fragility is mitigated by the tests that verify specific columns appear at specific positions.

## Summary
- Total findings: 8
- Critical: 0
- High: 2
- Medium: 4
- Low: 2

## Recommendations

1. **Make batch retention optional** (Finding 1): Add a `retain_batches: bool` parameter to `StreamingRecorder::new`. When `false`, skip `self.flushed_batches.push(batch)` in `flush()`. Set to `false` by default for simulation engine use; enable only in Python bindings and tests that need post-hoc batch access.

2. **Add file rotation support** (Finding 2): Introduce a `RotationPolicy` enum (e.g., `None`, `Hourly`, `Daily`, `Monthly`) in `SimulationConfig`. When set, `StreamingRecorder` should create new files at rotation boundaries, naming them with a timestamp suffix (e.g., `dwelling_42_2024-01-01.parquet`). Each rotated file gets its own schema header. EnergyPlus's `ReportFreq` enum (`OutputProcessor.cc:347-355`) provides a reference design.

3. **Use narrower numeric types** (Finding 3): Migrate temperature columns to `DataType::Float32`, mode ordinals and state columns to `DataType::Int32`, power factor and SOC to `DataType::Float32`. Keep energy (kWh) as `DataType::Float64`. Update `build_batch` to use a column-type dispatch (match on schema field DataType) so that `Int32Array`, `Float32Array`, and `Float64Array` are constructed appropriately from the value buffer.

4. **Add schema stability regression tests** (Finding 4): Add a test that calls `build_schema` twice with identical arguments and asserts `schema1 == schema2` using Arrow's schema equality. Also test that field count, name list, and metadata are identical.

5. **Use Parquet native timestamps** (Finding 5): Change the timestamp column from `DataType::Utf8` to `DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()))`. Convert the `DateTime<FixedOffset>` to nanoseconds since epoch before pushing to a dedicated `Vec<i64>` timestamp buffer. This requires updating `build_batch` to produce `TimestampNanosecondArray` and updating `push_row` to accept a nanosecond timestamp instead of a string, or adding a separate method.

6. **Add defensive type assertion in `build_batch`** (Finding 6): Before constructing columns, assert that the schema has exactly one `Utf8` column at index 0 and all others are numeric (`Float64`, `Float32`, `Int32`). Produce a clear `OutputError` variant like `UnsupportedColumnType { column_name: String, data_type: DataType }` on mismatch.

7. **Add upper bound to `chunk_size`** (Finding 7): Enforce a reasonable maximum (e.g., 1_000_000 rows) in either `SimulationConfig` validation or `StreamingRecorder::new`, producing a clear error message if exceeded.

8. **Replace `HashMap` with deterministic map** (Finding 8): Use `std::collections::BTreeMap` (ordered, no extra dependency) in `instance_qualified_names` to make the determinism explicit and future-proof against accidental iteration over the map.

## References / Citations
- EnergyPlus `ReportFreq` enum with output frequency levels: `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:347-355`
- EnergyPlus `determineFrequency` function mapping string input to `ReportFreq`: `OutputProcessor.cc:357-422`
- HARES timestamp formatting via chrono `%+` (RFC 3339): `crates/hares-core/src/dwelling/mod.rs:3141`
- HARES output path resolution (single file): `crates/hares-core/src/engine.rs:324-339`
- Arrow `RecordBatch::try_new` validation: `crates/hares-io/src/output/writer.rs:229`
- `FlushedRecordBatch` accessor usage in engine: `crates/hares-core/src/engine.rs:128,217`
