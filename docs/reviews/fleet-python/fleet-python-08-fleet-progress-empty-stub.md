# Fleet progress.rs is an empty stub
**Review ID**: fleet-python-08
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-fleet/src/progress.rs`
- `crates/hares-fleet/src/fleet.rs`

## Vendor/Reference Files Consulted
(None provided for this review area.)

## Findings

### Finding 1: [Severity: medium]
**Description**: `progress.rs` is declared as a public module in `lib.rs` but contains zero implementation — only a single doc comment line. All actual progress reporting logic lives inline in `fleet.rs` via a `ProgressCallback` type alias and two methods on `Fleet` (`with_progress`, `set_progress`). This creates a misleading development surface: a developer looking at `crates/hares-fleet/src/progress.rs` to understand progress reporting will find an empty file and must discover the real implementation by reading `fleet.rs`.

**Code Location**:
- `crates/hares-fleet/src/progress.rs:1` — entire file is a single doc comment with no code
- `crates/hares-fleet/src/lib.rs:5` — `pub mod progress;` declares the empty module
- `crates/hares-fleet/src/fleet.rs:22` — `type ProgressCallback = Arc<dyn Fn(usize, usize) + Send + Sync + 'static>;` is the actual progress type
- `crates/hares-fleet/src/fleet.rs:89` — `progress: Option<ProgressCallback>` field on `Fleet`
- `crates/hares-fleet/src/fleet.rs:170-178` — `with_progress` and `set_progress` methods
- `crates/hares-fleet/src/fleet.rs:227-264` — `simulate_parallel` method that invokes the callback

**Root Cause**: The `progress.rs` module was created as a placeholder (`pub mod progress;` in `lib.rs`) at some point with the doc comment `//! Progress reporting for fleet simulation.`, but the actual progress reporting was implemented inline in `fleet.rs` as a simple closure/callback pattern (`ProgressCallback` type alias, `Option<ProgressCallback>` field, and setter methods). The placeholder was never removed or populated.

**Impact**:
1. **Developer confusion**: Anyone looking for progress reporting logic will open `progress.rs`, find an empty shell, and must hunt through `fleet.rs` to find the actual implementation. There is no cross-reference or `#[deprecated]` attribute to guide them.
2. **Dead code in module tree**: The `pub mod progress;` declaration increases the public API surface without adding any value — `lib.rs` does not re-export anything from `progress` (line 8), but the empty module is still publicly accessible as `hares_fleet::progress`.
3. **No progress reporting for `SteppableFleet`**: The empty module is even more misleading because `SteppableFleet` (line 267) has no progress reporting whatsoever — its `step()` method (line 390) provides no callback mechanism. If progress reporting for stepped simulation were planned, `progress.rs` would be the natural place for shared traits/types, but currently nothing exists.
4. **Compile-time bloat (negligible)**: The empty module compiles but adds a file and module entry for no benefit.

### Finding 2: [Severity: low]
**Description**: The current `ProgressCallback` type (`Arc<dyn Fn(usize, usize) + Send + Sync + 'static>`) in `fleet.rs:22` is undocumented as a public type. While it is not `pub` (private to the crate), Python consumers interact with progress via `Fleet::set_progress` and `Fleet::with_progress` which accept `impl Fn(usize, usize) + Send + Sync + 'static`. The `(usize, usize)` tuple convention (completed, total) is documented only in the doc comment on `with_progress` (line 165: "called as (completed, total) from worker threads"). If progress logic is ever extracted to `progress.rs`, this convention should be formalized as a named struct or trait.

**Code Location**: `crates/hares-fleet/src/fleet.rs:22`, `crates/hares-fleet/src/fleet.rs:163-178`

**Root Cause**: The progress type is a simple type alias defined ad-hoc in `fleet.rs`. There is no `ProgressEvent` struct or `ProgressReporter` trait that would enforce the `(completed, total)` convention at the type level.

**Impact**: Minor — the current API works correctly, but future additions of richer progress data (elapsed time, ETA, per-dwelling status) would require breaking changes to the callback signature. Extracting types into `progress.rs` would centralize this contract.

## Summary
- Total findings: 2
- Critical: 0 / High: 0 / Medium: 1 / Low: 1

## Recommendations

1. **Remove the empty `progress.rs` module and its declaration.** Delete `crates/hares-fleet/src/progress.rs` and remove `pub mod progress;` from `crates/hares-fleet/src/lib.rs` (line 5). The inline `ProgressCallback` in `fleet.rs` is functional, simple, and tested (see tests at `fleet.rs:774-850`). The empty module adds confusion with no compensating value.

2. **If progress extraction is planned for a future milestone**, keep `progress.rs` but populate it with at minimum:
   - A `ProgressEvent` struct: `{ completed: usize, total: usize }` to replace the untyped `(usize, usize)` tuple.
   - A `ProgressReporter` trait with at least `fn report(&self, event: ProgressEvent)`.
   - A doc comment explaining that `Fleet` consumes this trait and `SteppableFleet` does not yet support progress reporting.
   - Update `fleet.rs` to re-export the type from `progress.rs` instead of defining the inline alias.
   - Add a test skeleton in `progress.rs`.

3. **Add progress reporting to `SteppableFleet`** if step-by-step progress is desired. The `step()` method currently has no callback mechanism. A similar `with_progress` / `set_progress` pattern could be added, or a shared `ProgressReporter` trait (recommendation 2) could be used by both `Fleet` and `SteppableFleet`.

## References / Citations
- `crates/hares-fleet/src/progress.rs:1` — empty module stub
- `crates/hares-fleet/src/lib.rs:5` — `pub mod progress;` declaration
- `crates/hares-fleet/src/fleet.rs:22` — `ProgressCallback` type alias
- `crates/hares-fleet/src/fleet.rs:89` — `progress` field on `Fleet`
- `crates/hares-fleet/src/fleet.rs:170-178` — `with_progress` and `set_progress` methods
- `crates/hares-fleet/src/fleet.rs:227-264` — `simulate_parallel` callback invocation
- `crates/hares-fleet/src/fleet.rs:267-486` — `SteppableFleet` (no progress support)
- `crates/hares-python/src/py_fleet.rs:175-187` — Python binding consuming `set_progress`
- `tests/python/test_py_fleet.py:309-317` — Python integration test for progress callback
