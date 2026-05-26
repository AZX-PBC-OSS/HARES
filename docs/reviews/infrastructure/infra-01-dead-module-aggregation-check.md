# Dead module reference aggregation_check.rs causes compile failure
**Review ID**: infra-01
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
- `tests/regression/mod.rs` (98 lines)

## Vendor/Reference Files Consulted
- `crates/hares-fleet/src/aggregation.rs` (730 lines) — fleet aggregation implementation with weighted metrics
- `crates/hares-fleet/src/lib.rs` (8 lines) — public API surface for fleet aggregation
- `tests/regression/determinism.rs` (88 lines) — sibling regression sub-suite for comparison
- `tests/regression/fleet_scale.rs` (82 lines) — sibling regression sub-suite that exercises Fleet but not aggregation
- `tests/regression/helpers.rs` (126 lines) — shared test helpers

## Findings

### Finding 1: [Severity: critical]
**Description**: `tests/regression/mod.rs` line 16 declares `mod aggregation_check;` and line 77 invokes `aggregation_check::run_aggregation_check()`, but no `aggregation_check.rs` file exists in `tests/regression/`. This causes an unconditional compilation failure (`error[E0583]: file not found for module 'aggregation_check'`), making the entire regression test harness unbuildable.

**Code Location**:
- Module declaration: `tests/regression/mod.rs:16` — `mod aggregation_check;`
- Invocation: `tests/regression/mod.rs:77` — `aggregation_check::run_aggregation_check()`
- Missing file: `tests/regression/aggregation_check.rs` (never existed in repository)

**Root Cause**: The `aggregation_check` module reference was included in the very first commit that created the regression test harness (`67f28b7`, "panic checkpoint", 2026-03-19). All six other sibling module `.rs` files (`determinism.rs`, `fleet_scale.rs`, `checkpoint_restart.rs`, `multi_instance.rs`, `helpers.rs`, `smoke_test.rs`) were successfully committed in that same commit. The `aggregation_check.rs` source file was never created or committed, and has never existed at any point in the repository's git history. No deletion commit exists for this file path, confirming the file was never present.

**Impact**:
1. **Compilation blocked**: `cargo test --test regression` (and any attempt to compile `tests/regression/mod.rs`) fails with `E0583`.
2. **Entire regression suite is dead code**: Because no crate in the workspace (all 10 crates are in `crates/` with their own `tests/` directories) references `tests/regression/`, the entire regression harness at the workspace root `tests/` directory is orphaned infrastructure — even if the dead reference were removed, the test suite would not be discoverable by Cargo as a test target in the current virtual workspace layout.
3. **Fleet aggregation regression gap**: The `hares-fleet` crate provides a production `aggregation::aggregate()` function (`crates/hares-fleet/src/aggregation.rs:131`) that computes weighted fleet-level timeseries and per-dwelling metrics from `DwellingOutcome` vectors. This function has synthetic unit tests (`aggregation.rs:407-730`) but no integration/regression test that validates it against real simulation outputs. The `fleet_scale.rs` regression sub-suite exercises `Fleet::simulate()` at 1000-dwelling scale but only checks outcome counts and total energy validity — it never calls `aggregation::aggregate()` and therefore provides zero coverage of fleet-level aggregation correctness.

## Summary
- **Total findings**: 1
- **Critical / High / Medium / Low**: 1 / 0 / 0 / 0

## Recommendations

1. **Remove the dead references** (minimum fix): Delete line 16 (`mod aggregation_check;`) and lines 76-84 (the "Weighted aggregation" block) from `tests/regression/mod.rs` to eliminate the compilation error. This alone will not make the suite functional because it remains unlinked from any workspace crate.

2. **Implement and wire up the aggregation check module** (preferred): Create `tests/regression/aggregation_check.rs` with a `pub fn run_aggregation_check() -> Result<(), Vec<String>>` that:
   - Runs a multi-dwelling simulation via `hares_fleet::Fleet`
   - Calls `hares_fleet::aggregation::aggregate()` on the outcomes
   - Validates that `FleetResults.per_dwelling_metrics` length matches dwelling count
   - Validates that weighted timeseries columns have expected suffix-based aggregation rules (kW → weighted sum, C/°C/(-) → weighted mean)
   - Validates that sample weights are correctly applied (leveraging patterns from the existing unit tests in `aggregation.rs:486-729`)
   - Follows the same error-collection pattern used by sibling modules (`determinism.rs`, `fleet_scale.rs`, etc.)

3. **Relocate or link the regression suite to a workspace crate** (broader fix): Move `tests/regression/` into `crates/hares-core/tests/` (or add appropriate `[[test]]` entries) so Cargo discovers the regression test harness. Currently no package owns these test files, making the entire suite unreachable regardless of whether the compilation error is fixed.

## References / Citations
- Initial commit adding `aggregation_check` reference: `67f28b7` (2026-03-19, "panic checkpoint") — committed 7 files to `tests/regression/` but omitted `aggregation_check.rs`
- Compilation error: `error[E0583]: file not found for module 'aggregation_check'` at `tests/regression/mod.rs:16:1`
- Fleet aggregation implementation: `crates/hares-fleet/src/aggregation.rs:131` (`pub fn aggregate(results: &[DwellingOutcome], resolution: AggregationResolution) -> FleetResults`)
- Existing fleet scale regression (no aggregation validation): `tests/regression/fleet_scale.rs:29-31`
- Workspace manifest confirming virtual workspace (no root package for test discovery): `Cargo.toml:1-25`
