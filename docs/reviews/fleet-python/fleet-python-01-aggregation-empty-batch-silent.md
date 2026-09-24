# Fleet aggregation: silent empty-batch on column mismatch
**Review ID**: fleet-python-01
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-fleet/src/aggregation.rs`

## Vendor/Reference Files Consulted
None (no vendor reference provided).

## Findings

### Finding 1: [Severity: high]
**Description**: `build_aggregate_batch()` silently returns an empty `RecordBatch` (zero rows, only a `"Time"` column) when it detects that the column name sets from different dwellings differ (`aggregation.rs:324-326`). No warning, error, or diagnostic is emitted.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:322-326`
```rust
for (_, columns, buckets) in successful.iter().skip(1) {
    if columns != first_columns {
        return empty_batch();  // silent — no log, no warning, no error
    }
    // ...
}
```

**Root Cause**: The function was designed with a hard equality check for column sets but without any diagnostic reporting path. The `tracing` crate is already a workspace dependency (`Cargo.toml:13`), but no `tracing::warn!` or `tracing::error!` call exists anywhere in the aggregation module.

**Impact**:
- **Full call chain affected**: `Fleet.simulate()` (Python) → `PyFleet::simulate` → `aggregate()` → `build_aggregate_batch()`. The empty `RecordBatch` is passed through `record_batches_to_polars_df()` in `crates/hares-python/src/conversions.rs:52-93` and returned to the Python caller as a polars DataFrame with only a `"Time"` column and zero rows.
- **Downstream consumers expect non-empty output**: Python integration tests assert `df.height > 0` after aggregation (`tests/python/test_py_conversions.rs:169-174`, `tests/python/test_py_fleet.py:281-288`). Production code using the aggregate timeseries for plotting, statistical analysis, or CSV export would produce empty plots or zero-valued statistics with no indication that the data was silently dropped.
- **The only existing guard is insufficient**: `PyFleet::simulate` (`crates/hares-python/src/py_fleet.rs:222-226`) returns a `PyValueError` only when *all* dwellings failed — it does not check whether the aggregate timeseries is empty after partial success.

---

### Finding 2: [Severity: medium]
**Description**: `extract_rows()` silently returns `None` when a single dwelling's own batches have inconsistent schemas (`aggregation.rs:191-196`). This is a related intra-dwelling variant of the same pattern: schema mismatches are consumed silently, causing that dwelling's data to be dropped from the aggregation pool without any diagnostic.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:191-196`
```rust
if !batches
    .iter()
    .all(|batch| batch.schema().fields() == schema.fields())
{
    return None;
}
```

**Root Cause**: Same design pattern — mismatch detection exits early with no logging or error reporting.

**Impact**: If one dwelling's year-long simulation is split across multiple chunks (due to pausing, resume, or streaming write) and a schema change occurs between chunks, that dwelling's full output is silently excluded from the fleet aggregate. The user sees `per_dwelling_metrics` with the dwelling marked `Ok` but no aggregate contribution — a subtle data integrity issue.

---

### Finding 3: [Severity: medium]
**Description**: The bucket-intersection logic (`aggregation.rs:328-332`) can silently produce an empty intersection when dwellings have non-overlapping time ranges, resulting in zero-row output with no diagnostic.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:328-332`
```rust
let mut bucket_intersection: BTreeSet<i64> = first_buckets.keys().copied().collect();
for (_, columns, buckets) in successful.iter().skip(1) {
    // column check (Finding 1) ...
    let keys: BTreeSet<i64> = buckets.keys().copied().collect();
    bucket_intersection = bucket_intersection
        .intersection(&keys)
        .copied()
        .collect::<BTreeSet<_>>();
}
```

**Root Cause**: Intersection-based alignment is reasonable for handling mismatched time ranges, but there is no check after the loop to verify the intersection is non-empty or to warn the user that all timesteps were dropped.

**Impact**: If dwelling A simulates 2021-01-01 through 2021-06-30 and dwelling B simulates 2021-07-01 through 2021-12-31 (unlikely but possible with custom time ranges), the intersection is empty. The returned `RecordBatch` has the correct schema but zero rows. This is harder to diagnose than the column mismatch case because the error pattern (empty DataFrame) is identical.

---

### Finding 4: [Severity: low]
**Description**: `RecordBatch::try_new()` failure at `aggregation.rs:398` silently falls back to `empty_batch()` via `unwrap_or_else(|_| empty_batch())`, discarding the Arrow error context.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:398`
```rust
RecordBatch::try_new(schema, arrays).unwrap_or_else(|_| empty_batch())
```

**Root Cause**: Defensive fallback pattern without error propagation or logging.

**Impact**: If column/array length mismatches occur during schema construction (likely indicating a logic bug in the builder code), the error is swallowed. The only diagnostic is an empty output DataFrame that matches the same symptom as Findings 1-3.

---

### Finding 5: [Severity: medium]
**Description**: Column mismatch is a **realistic** scenario, not merely theoretical. Three independent code paths produce heterogeneous column sets across dwellings:

**Evidence**:

1. **Equipment-dependent columns** (`crates/hares-io/src/output/columns.rs:89-297`): Each dwelling's HPXML file determines its equipment list. A dwelling with a gas furnace produces columns like `"Gas Furnace Electric Power (kW)"` and `"Gas Furnace Gas Power (therms/hour)"`, while a dwelling with an ASHP heater+cooler produces `"ASHP Heater Electric Power (kW)"`, `"ASHP Cooler Electric Power (kW)"`, `"ASHP Heat/Cool Defrost State (-)"`, `"ASHP Cooler SHR (-)"`, etc. ResStock datasets routinely mix equipment types across dwellings.

2. **Actor telemetry columns** (`crates/hares-core/src/dwelling/mod.rs:228-245`): `extend_schema_with_actor_columns()` only adds `actor:*` columns for the actors actually present in each dwelling. A dwelling with a Battery gets `actor:BatteryManagementActor:Battery:soc` and related columns; a dwelling with an EV gets `actor:EvDriver:EV:target_soc`. A dwelling with neither gets zero actor telemetry columns.

3. **Python custom equipment** (`crates/hares-python/src/py_actor.rs`): The `PyActorWrapper` allows Python subclasses to provide custom `telemetry()` output. While the default `telemetry()` returns `None`, an overridden Python actor could inject arbitrary additional columns into a single dwelling's output.

No schema union, merge, or compatibility validation exists anywhere in the codebase between dwelling simulation and fleet aggregation.

---

## Summary
- Total findings: 5
- Critical: 0
- High: 1  (Finding 1: silent empty batch on column mismatch)
- Medium: 3  (Findings 2, 3, 5)
- Low: 1  (Finding 4: swallowed RecordBatch construction error)

## Recommendations

1. **Add `tracing::warn!` diagnostics in `build_aggregate_batch()`** (Finding 1): When column mismatch is detected at `aggregation.rs:324-326`, emit a warning listing the columns present in the first dwelling and the names of the mismatched columns. Example:
   ```rust
   tracing::warn!(
       "Fleet aggregation: column set mismatch detected. \
        First dwelling has {} columns; subsequent dwelling has {} different columns. \
        Mismatched columns present in subsequent dwelling but not first: {:?}. \
        Returning empty aggregate timeseries.",
       first_columns.len(),
       columns.len(),
       columns.iter().filter(|c| !first_columns.contains(c)).collect::<Vec<_>>()
   );
   ```

2. **Add `tracing::warn!` in `extract_rows()`** (Finding 2): When intra-dwelling batch schemas differ, log the differing field sets so users can diagnose resume/streaming issues.

3. **Add a post-intersection emptiness check** (Finding 3): After the `bucket_intersection` loop (`aggregation.rs:332`), emit a warning if the intersection is empty (and the inputs were not empty), or if the intersection is significantly smaller than any single dwelling's bucket count.

4. **Propagate `RecordBatch::try_new` errors** (Finding 4): Replace the `unwrap_or_else(|_| empty_batch())` at `aggregation.rs:398` with either a `tracing::error!` before falling back, or propagate the error upward (changing the return type to `Result<RecordBatch>`). The caller `aggregate()` has no error path either, so this would require a modest API change.

5. **Add Python-level diagnostics** (Findings 1-3): In `PyFleet::simulate` (`crates/hares-python/src/py_fleet.rs:228`), after calling `aggregate()`, check `aggregate_timeseries.num_rows() == 0` when `success.len() > 0`, and either emit a Python warning via `pyo3::PyWarning` or include the condition in the returned `PyFleetResults` so Python callers can inspect it.

6. **Add test coverage for column mismatch**: Add a Rust unit test in `aggregation.rs` tests that verifies a column mismatch returns an empty batch (confirming current behavior) and that the `tracing::warn!` is observable. Add a Python integration test that verifies column-mismatch-aware behavior (either a warning emission or a non-empty DataFrame with a subset of shared columns).

7. **Consider schema union as an alternative to empty-batch**: If heterogeneous fleets are a supported use case, implement a schema union strategy where only columns shared by *all* dwellings are aggregated (the intersection of column sets rather than requiring exact equality). This would drop the mismatched columns instead of the entire timeseries, providing partial but useful output. The `tracing::warn!` from recommendation 1 should still fire so users know some columns were dropped.

## References / Citations
- `crates/hares-fleet/src/aggregation.rs:311-399` — `build_aggregate_batch()` definition
- `crates/hares-fleet/src/aggregation.rs:131-184` — `aggregate()` caller
- `crates/hares-fleet/src/aggregation.rs:186-199` — `extract_rows()` silent schema mismatch
- `crates/hares-fleet/src/aggregation.rs:401-405` — `empty_batch()` helper
- `crates/hares-python/src/py_fleet.rs:185-232` — `PyFleet::simulate()` consumer of `aggregate()`
- `crates/hares-python/src/py_fleet.rs:278-280` — `aggregate_timeseries` getter
- `crates/hares-python/src/conversions.rs:52-93` — `record_batches_to_polars_df()` Arrow→polars conversion
- `crates/hares-io/src/output/columns.rs:60-297` — per-dwelling schema construction (equipment-dependent columns)
- `crates/hares-core/src/dwelling/mod.rs:228-245` — `extend_schema_with_actor_columns()` (actor telemetry columns)
- `crates/hares-equipment/src/registry.rs:17-88` — 87 built-in equipment types
- `crates/hares-io/src/hpxml/equipment.rs:33-52` — HPXML-driven equipment resolution per dwelling
- `crates/hares-fleet/Cargo.toml:13` — `tracing` already a dependency
- `tests/python/test_py_conversions.py:169-174` — Python test asserts non-empty aggregate
- `tests/python/test_py_fleet.py:281-288` — Python test asserts non-empty aggregate
