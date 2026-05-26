# Python binding: dead code with #[allow(dead_code)]
**Review ID**: fleet-python-05
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
crates/hares-python/src/conversions.rs

## Vendor/Reference Files Consulted


## Findings
### Finding 1: `obs_to_numpy` is dead code — never wired into the RL API [Severity: low]
**Description**: The function `obs_to_numpy()` at `crates/hares-python/src/conversions.rs:133-136` converts a `Vec<f64>` to a `numpy::PyArray1<f64>` and is annotated `#[allow(dead_code)]`. It has zero callers across the entire codebase. The `batch_step_py` function (`crates/hares-python/src/py_gym.rs:154-157`) produces observation vectors as `Vec<f64>` and embeds them directly into the returned `PyDict` via `d.set_item("obs", item.obs)`, relying on PyO3's automatic `Vec<f64>` → Python list conversion. The NumPy path is not wired in, nor does it need to be — the existing list-based approach works correctly and is idiomatic for the Gymnasium return protocol.

**Code Location**: `crates/hares-python/src/conversions.rs:133-136`
**Root Cause**: The function was likely added during initial development as a planned utility but was never integrated into `batch_step_py` because PyO3 already handles `Vec<f64>` → Python list conversion automatically. It was retained with `#[allow(dead_code)]` rather than being removed or wired.
**Impact**: Minimal — the function is a trivial one-liner (`values.into_pyarray(py)`). It does not represent incomplete feature wiring; the Gymnasium API returns correctly via the existing list path. However, the `#[allow(dead_code)]` annotation suppresses a useful compiler warning and creates a maintenance burden (the `numpy::IntoPyArray` import at line 10 remains unused except for this dead function). The import of `IntoPyArray` and `PyArray1` in the crate would become unused if this function were removed, which is a stronger signal that the dead code has no purpose.

### Finding 2: `record_batch_arc_to_polars_df` is dead code — convenience wrapper with no callers [Severity: low]
**Description**: The function `record_batch_arc_to_polars_df()` at `crates/hares-python/src/conversions.rs:139-145` wraps `record_batches_to_polars_df()` for callers that hold an `&Arc<RecordBatch>`. It is annotated `#[allow(dead_code)]` and has zero callers. The only Arrow-to-DataFrame conversion path in the Python bindings is the `aggregate_timeseries` getter (`crates/hares-python/src/py_fleet.rs:278-281`), which calls `record_batches_to_polars_df` directly with `std::slice::from_ref(&self.results.aggregate_timeseries)`. The `FleetResults.aggregate_timeseries` field (`crates/hares-fleet/src/aggregation.rs:34`) is typed as `RecordBatch`, not `Arc<RecordBatch>`, and no other code path in the `hares-python` crate holds an `Arc<RecordBatch>` — a grep for `Arc<RecordBatch>` in the crate confirms the only occurrence is this dead function's own signature.

**Code Location**: `crates/hares-python/src/conversions.rs:139-145`
**Root Cause**: This is a forward-compatibility convenience function that was added speculatively for a future code path where Arrow batches might be held behind an `Arc`. That code path never materialized — the `aggregate_timeseries` call site passes a bare `RecordBatch` reference using `std::slice::from_ref`, which is equally concise.
**Impact**: Minimal. The function body delegates to the existing `record_batches_to_polars_df`, so there is no duplicated logic. However, the `#[allow(dead_code)]` annotation masks the fact that the function is unused, preventing the compiler from flagging it for cleanup. The `std::sync::Arc` import (line 3 of conversions.rs) would remain in use even without this function (it is not used elsewhere in the file), so removing the function would trigger an unused-import warning for `Arc` — confirming it has no other purpose in this module.

## Summary
- Total findings: 2
- Critical / High / Medium / Low: 0 / 0 / 0 / 2

## Recommendations
1. **Remove both dead functions** (`obs_to_numpy` and `record_batch_arc_to_polars_df`) from `conversions.rs`. Neither represents incomplete feature wiring — the existing code paths handle observation returns and Arrow-to-DataFrame conversion correctly without them. Removing them eliminates the `#[allow(dead_code)]` annotations, which are otherwise suppressing legitimate compiler diagnostics.
2. **Remove the associated now-unused imports:** After removing `obs_to_numpy`, the `numpy::IntoPyArray` and `numpy::PyArray1` imports (line 10) and the `std::sync::Arc` import (line 3) will become unused and trigger compiler warnings. Remove them as well.
3. **If forward-compatibility is desired** for the Arc-based Arrow path or NumPy observations, file a tracking issue and reference it in a comment on the `record_batches_to_polars_df` and `batch_step_py` code paths respectively, rather than retaining dead wrapper functions. A comment like `// TODO(#XXX): consider Arc-based dispatch for X` is clearer than a dead function with `#[allow(dead_code)]`.

## References / Citations
- `crates/hares-python/src/conversions.rs:133-136` — `obs_to_numpy` definition with `#[allow(dead_code)]`
- `crates/hares-python/src/conversions.rs:139-145` — `record_batch_arc_to_polars_df` definition with `#[allow(dead_code)]`
- `crates/hares-python/src/py_gym.rs:152-170` — `batch_step_py` returns observations as `Vec<f64>` via PyDict, not NumPy arrays
- `crates/hares-python/src/py_fleet.rs:278-281` — `aggregate_timeseries` getter calls `record_batches_to_polars_df` directly, not the Arc wrapper
- `crates/hares-fleet/src/aggregation.rs:34` — `FleetResults.aggregate_timeseries` is `RecordBatch`, not `Arc<RecordBatch>`
