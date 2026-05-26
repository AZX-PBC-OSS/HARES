# Thermal balance invariant registration unwired in engine
**Review ID**: core-01
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/invariants.rs` (lines 1–374)
- `crates/hares-core/src/engine.rs` (lines 1–477)
- `crates/hares-core/src/dwelling/mod.rs` (relevant sections: lines 53, 707–742, 1216–1290, 2566, 2652, 3046–3268)
- `crates/hares-types/src/error.rs` (lines 1–89)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: high]
**Description**: The `check_thermal` invariant is fully implemented and tested in `invariants.rs` (`crates/hares-core/src/invariants.rs:31–55`) but is never invoked anywhere in the simulation engines runtime code path. The per-timestep invariant check method `check_invariants` in `dwelling/mod.rs` (`crates/hares-core/src/dwelling/mod.rs:3051–3268`) explicitly defers thermal balance with a comment (`crates/hares-core/src/dwelling/mod.rs:3135–3142`) stating that a zone-air-only balance has an ~6 kW residual and a proper system-level audit requires ThermalSolver API changes.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3135–3142`
**Root Cause**: The multi-node RC state-space model distributes thermal energy across zone-air and wall-mass nodes. The current `check_thermal` API accepts a flat scalar `delta_e_storage` and a single `q_loss` value, which is insufficient to capture the full energy redistribution across nodes. The solver would need to expose per-node capacitances and previous-step state vectors. The `zone_capacitances_j_k` field exists on the `Dwelling` struct (`crates/hares-core/src/dwelling/mod.rs:707–708`) but is annotated `#[expect(dead_code)]` and never consumed by any invariant check.

**Impact**: Energy non-conservation bugs in the thermal solver (e.g., a sign error in a heat-gain term, incorrect capacitance, or solver divergence) will go undetected at runtime. The thermal domain is the largest and most complex numerical subsystem in HARES; without a monitoring invariant, regressions can persist silently until noticed in output data.

### Finding 2: [Severity: medium]
**Description**: The electrical balance tolerance in `check_electrical` is a hardcoded absolute value of `0.001 kW` (1 W) (`crates/hares-core/src/invariants.rs:70`). This does not scale with system size. A large commercial building or multi-dwelling fleet with 1000+ kW loads will have floating-point accumulation error proportional to gross power magnitude, but the tolerance remains fixed at 1 W. Conversely, for very small systems (e.g., a single 100 W plug load), 1 W is 1% relative tolerance and may be too loose to catch small imbalances. The thermal balance check uses a well-designed relative tolerance (`max(1.0, 1e-6 * gross_flux)` at `crates/hares-core/src/invariants.rs:45`), but this relative approach was not applied to the electrical check.

**Code Location**: `crates/hares-core/src/invariants.rs:70`
**Root Cause**: The electrical balance check was implemented with the simpler absolute tolerance model; the relative-tolerance pattern from `check_thermal` was not replicated.
**Impact**: At scale, false positives in debug builds will halt simulations unnecessarily; at small scale, real imbalances may pass undetected.

### Finding 3: [Severity: medium]
**Description**: The failure mode for all invariant violations is a hard stop -- any violation returns `Err(HaresError::InvariantViolation)`, which propagates via `?` through `check_invariants → run_timestep → simulate`, terminating the simulation (`crates/hares-core/src/dwelling/mod.rs:2652`, `crates/hares-core/src/dwelling/mod.rs:1343`). In `engine.rs`, this becomes `SimStatus::Failed` (`crates/hares-core/src/engine.rs:168–177`). There is no configurable severity level that would allow development (panic/abort) vs. production (warn-and-continue) behavior.

**Code Location**: `crates/hares-core/src/invariants.rs:46–52`, `crates/hares-core/src/dwelling/mod.rs:2652`, `crates/hares-core/src/engine.rs:168–177`
**Root Cause**: The invariant checks were designed as `Result<(), HaresError>` with only two outcomes: pass or hard-error. There is no middle path (e.g., a `warn` severity that logs via `tracing::warn!` and continues). Only `check_soc` (`crates/hares-core/src/invariants.rs:119–138`) uses warn-and-continue behavior -- the four other checks all halt.
**Impact**: During development, halting on violation is desirable for rapid feedback. But in production or long-running fleet simulations, a single stray numerical blip in one dwelling kills the entire run. A configurable threshold (e.g., `--invariant-severity=warn` vs `--invariant-severity=abort`) would allow operational flexibility.

### Finding 4: [Severity: low]
**Description**: The moisture balance tolerance (`check_moisture`) is hardcoded at `1e-6 kg` (`crates/hares-core/src/invariants.rs:97`). Like the electrical balance, this absolute tolerance does not scale with zone volume or simulation duration. A 1000 m³ zone with high humidity flux could accumulate floating-point error exceeding this fixed limit even when the physics is correctly balanced.

**Code Location**: `crates/hares-core/src/invariants.rs:97`
**Root Cause**: Same pattern as Finding 2 -- the `check_thermal` relative-tolerance design was not replicated for the moisture check.
**Impact**: Potential false positives in large-zone or high-humidity scenarios when invariants are enabled.

### Finding 5: [Severity: low]
**Description**: The code comment at `crates/hares-core/src/invariants.rs:30` states the thermal balance check asserts `|Σ(Q_gain) − ΔE_storage − Q_loss_envelope| < tolerance`, but the tolerance formula in the comment says `max(1.0, 1e-6 · |Σ Q_gain|)` while the actual implementation at `crates/hares-core/src/invariants.rs:44` uses `gross_flux` (sum of absolute values of individual gains, not the absolute net sum). The comment is slightly misleading -- the implementation is actually better (using gross magnitude) than what the comment describes, but the discrepancy could confuse maintainers.

**Code Location**: `crates/hares-core/src/invariants.rs:27–30` vs `crates/hares-core/src/invariants.rs:44`
**Root Cause**: Comment not updated after the code was refined to use gross flux instead of net sum.
**Impact**: Minor maintenance confusion only; the code itself is correct.

### Finding 6: [Severity: low]
**Description**: The `check_invariants` method in `dwelling/mod.rs` is gated by `#[cfg(any(debug_assertions, feature = "check_invariants"))]`. This means the entire body of invariant checking compiles to nothing in production release builds without the `check_invariants` feature. This is explicitly documented (`crates/hares-core/src/invariants.rs:4–9`) and is a deliberate design choice. However, there is no mechanism for selectively enabling individual invariant categories (e.g., enabling thermal but not moisture). All checks are either fully on or fully off.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3052`
**Root Cause**: The `#[cfg(...)]` gates are coarse-grained; individual checks within the block are called unconditionally when the gate is active.
**Impact**: If one invariant category produces false positives in a given scenario, all categories must be disabled to continue.

## Summary
- Total findings: 6
- Critical: 0 / High: 1 / Medium: 2 / Low: 3

## Recommendations
1. **Wire the thermal balance invariant** by either (a) extending `ThermalSolver` to expose per-node capacitances and prior state, computing a proper system-level `delta_e_storage` from the full state-space model, or (b) exposing a `ThermalSolver::energy_balance_residual()` method that internally verifies the full RC network and returns a single residual value. The existing `zone_capacitances_j_k` field and the `_residual` field already flowing through the thermal custom payload (`crates/hares-core/src/dwelling/mod.rs:3182`) suggest the infrastructure is partially in place.

2. **Make invariant severity configurable**: Add an `InvariantPolicy` enum (`Abort | Warn | Suppress`) configurable per check or globally via `DwellingConfig`. When set to `Warn`, violations should emit `tracing::warn!` and continue rather than returning `Err`. This mirrors the existing `check_soc` pattern (`crates/hares-core/src/invariants.rs:119–138`).

3. **Apply relative tolerance to electrical and moisture checks**: Replicate the `gross_flux`-based relative tolerance pattern from `check_thermal` for `check_electrical` and `check_moisture`. As a minimum, add a `max(absolute_floor, relative_fraction * sum(|term|))` tolerance formula.

4. **Update the thermal balance doc-comment** at `crates/hares-core/src/invariants.rs:27–30` to match the implementation (gross flux, not net sum).

## References / Citations
- `crates/hares-core/src/invariants.rs:31–55` — `check_thermal` implementation with relative tolerance
- `crates/hares-core/src/invariants.rs:61–80` — `check_electrical` with hardcoded 0.001 kW tolerance
- `crates/hares-core/src/invariants.rs:89–112` — `check_moisture` with hardcoded 1e-6 kg tolerance
- `crates/hares-core/src/dwelling/mod.rs:3135–3142` — thermal balance explicitly deferred
- `crates/hares-core/src/dwelling/mod.rs:707–708` — `zone_capacitances_j_k` field reserved but unused
- `crates/hares-core/src/dwelling/mod.rs:2652` — `check_invariants(dt)?` call site with hard-stop error propagation
- `crates/hares-core/src/dwelling/mod.rs:3051–3268` — full `check_invariants` method body
- `crates/hares-core/src/engine.rs:168–177` — engine converts invariant violation to `SimStatus::Failed`
- `crates/hares-types/src/error.rs:23–28` — `HaresError::InvariantViolation` variant definition
