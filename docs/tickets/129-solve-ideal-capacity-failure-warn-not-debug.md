# `solve_ideal_capacity_for_target` Failure Should Log at `warn!` Not `debug!`

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope

## Problem

When `solve_ideal_capacity_for_target` in `crates/hares-envelope` fails to converge to a setpoint-tracking capacity, the failure is logged at `tracing::debug!` level. This is too quiet: an ideal-HVAC convergence failure is a meaningful diagnostic event that affects load output (the zone will not track its setpoint that step) and should be visible in default operator logging at `info` or `warn` level, not buried at `debug`.

A user running an annual simulation and noticing that some hours have unexpected setpoint deviations has no way to discover that the ideal-capacity solver failed unless they re-run with `RUST_LOG=hares_envelope=debug`.

## Current Behavior

`solve_ideal_capacity_for_target` in `crates/hares-envelope/` (exact location: search for the function name) emits:
```rust
tracing::debug!(target: "hares_envelope::solver", "ideal capacity solve failed: ...");
```

## Required Behavior

1. Promote the failure log to `tracing::warn!` so it surfaces by default.
2. Include sufficient structured context in the log fields: zone id, target temperature, achieved temperature, outdoor temperature, capacity tried, error from convergence test.
3. Ensure the warn does not fire on every step in a pathological run — if the failure persists for many consecutive steps (e.g. the target is unreachable due to undersized equipment), throttle the warn or accumulate a count and emit one warn per N consecutive failures.

## Approach

1. Locate `solve_ideal_capacity_for_target` in `crates/hares-envelope/`.
2. Replace `tracing::debug!` with `tracing::warn!` at the failure site.
3. Add structured fields: `zone_id`, `target_c`, `achieved_c`, `oat_c`, `capacity_w`, `residual`.
4. Add a per-zone consecutive-failure counter that suppresses the warn after the first N failures in a row, with a single info-level recovery log when the solver next succeeds.
5. Add a unit test that triggers a deliberate ideal-capacity failure (e.g. impossible target) and asserts a `warn!` line is captured by `tracing-test`.

## Definition of Done

- [ ] Failure log at `solve_ideal_capacity_for_target` is `warn!`, not `debug!`
- [ ] Structured fields include zone, target, achieved, residual
- [ ] Repeat-failure throttling prevents log flood
- [ ] Recovery log fires once when the solver next succeeds after a failure run
- [ ] Unit test asserts the warn is emitted under deliberate-failure conditions

## Verification

```bash
cargo test -p hares-envelope solve_ideal_capacity
cargo test -p hares-envelope --features tracing-test
```

## References

- EnergyPlus Engineering Reference §6.3 "Ideal Loads Air System" — convergence failure is a documented diagnostic event in E+ and is reported in the Errors output file.
- `tracing` documentation on level selection: use `warn` for "an event that may be problematic but does not prevent the operation from continuing" — exactly the semantics here.

## Related Tickets

- 022-roomac-ideal-target (related ideal-target convergence)

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `tracing::debug!` is at
  `crates/hares-envelope/src/thermal_solver/stepping.rs:59` (function
  `solve_ideal_capacity_for_target`, lines 24–66). The ticket says
  "search for the function name" rather than citing a hard line number, which
  is accurate: the function is easily found.
- [x] Described logic matches current implementation — `unwrap_or_else` on the
  `Result<f64>` from `solve_for_scalar_input` / `solve_for_scalar_input_coupled`
  calls `tracing::debug!(?zone, ?e, "solve_ideal_capacity_for_target failed,
  returning 0")` and returns `0.0`. The ticket's description of the bug is
  exactly correct.
- [x] OCHRE cross-check result: **diverges (intentionally)**
  - OCHRE (`vendors/OCHRE/ochre/Equipment/HVAC.py:411–429`) calls
    `self.envelope_model.solve_for_inputs(...)` from `solve_ideal_capacity()`
    with no error handling or logging at any level; if that function raises an
    exception it propagates uncaught up to the caller.
  - HARES wraps the analogous call in a `Result` and suppresses any error
    silently at `debug!`. Neither OCHRE nor HARES emits a user-visible warning.
    The ticket is asking HARES to exceed OCHRE's behaviour here — a deliberate
    improvement, not an OCHRE parity fix.
- [x] EnergyPlus cross-check result: **partially supports the ticket's claim**
  - The EnergyPlus Engineering Reference (Big Ladder, v24.1,
    "Ideal Loads Air System" chapter) describes only one explicit warning for
    the Ideal Loads object: an outdoor-air-flow-rate exceedance. It does not
    describe a specific convergence-failure warning for the setpoint-solve
    itself.
  - EnergyPlus's module developer guide (v22.1, "Error Messages" chapter)
    describes `ShowWarningError` as the appropriate routine for conditions that
    "are worth noting but do not prevent the simulation from continuing." It
    distinguishes this from `ShowSevereError` (halting conditions) and from
    purely informational `ShowMessage`. This directly supports the ticket's
    argument that a failure-to-converge event — which causes the zone to miss
    its setpoint that timestep but does not terminate the run — should be
    reported at warning level. The EnergyPlus error philosophy aligns with the
    ticket's recommendation.
  - Quoted passage (Big Ladder EnergyPlus 24.1, Ideal Loads Air System):
    > "If outdoor air flow rate exceeds applicable maximum flow rate (heating
    > or cooling) then reduce outdoor air mass flow rate, **issue warning**, and
    > set supply air mass flow rate equal to outdoor air mass flow rate"
  - Quoted passage (Big Ladder EnergyPlus 22.1, Error Messages guide):
    > "Rather than terminating with the first illegal value, however, it is
    > better to have an 'ErrorsFound' logical that gets set to true for error
    > conditions during the main routine processing and terminates at the end."
    > … `ShowWarningError` increments the warning counter without stopping the
    > simulation.

### Web-Verified Citations

**Citation 1**: "EnergyPlus Engineering Reference §6.3 'Ideal Loads Air System'
— convergence failure is a documented diagnostic event in E+ and is reported in
the Errors output file."

- **Source found**: Big Ladder Software, EnergyPlus 24.1 Engineering Reference —
  "Ideal Loads Air System" (https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/ideal-loads-air-system.html)
- **Quoted passage**: The chapter contains no section numbered §6.3 in the HTML
  rendering and does not mention convergence failure as a documented diagnostic
  event. The only warning reference is the outdoor-air flow exceedance passage
  quoted above.
- **Verdict**: **Partially correct** — the EnergyPlus Ideal Loads Air System
  chapter is real, EnergyPlus does write diagnostics to its `.err` file, and
  the general philosophy of warning on notable-but-non-fatal events is
  well-established in the module developer guide. However, the specific claim
  that E+ §6.3 documents "convergence failure … reported in the Errors output
  file" is not verifiable from the Engineering Reference text: no convergence
  failure warning for the setpoint-solve is explicitly described there. The
  section number "§6.3" does not match the HTML-rendered chapter structure.
  The substantive engineering argument (warn on non-fatal diagnostic events)
  is sound even though the precise citation is imprecise.

**Citation 2**: "`tracing` documentation on level selection: use `warn` for
'an event that may be problematic but does not prevent the operation from
continuing' — exactly the semantics here."

- **Source found**: docs.rs/tracing, `Level` struct documentation
  (https://docs.rs/tracing/latest/tracing/struct.Level.html)
- **Quoted passage**: `Level::WARN` — *"The 'warn' level. Designates hazardous
  situations."*
- **Verdict**: **Partially correct** — the tracing crate does define `warn` as
  the appropriate level for hazardous situations and `debug` for lower-priority
  diagnostics. However, the ticket paraphrases the definition as "may be
  problematic but does not prevent the operation from continuing," which is not
  the verbatim wording ("designates hazardous situations"). The paraphrase is a
  reasonable interpretation and the substantive argument is correct: an
  ideal-capacity solve failure is more than a debug-level curiosity. The
  exact quoted phrase is not found in the tracing docs, but the conclusion
  (promote to `warn!`) is fully supported.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The bug is real and confirmed at
  `crates/hares-envelope/src/thermal_solver/stepping.rs:59` — the failure
  path of `solve_ideal_capacity_for_target` calls `tracing::debug!` and
  silently returns `0.0`, giving no user-visible signal that the ideal-capacity
  solve failed and that the zone will miss its setpoint. OCHRE has no
  equivalent warning either, so this is not an OCHRE-parity concern but a
  genuine HARES improvement. EnergyPlus's error-handling philosophy
  (ShowWarningError for non-fatal notable events) directly supports promoting
  this log to `warn!`. Both ticket citations are imprecise in their wording
  (the tracing paraphrase and the E+ section number) but substantively correct.
  The proposed fix is straightforward and the Definition of Done is clear.
  Throttling (item 3 in the Approach) is desirable but adds complexity; the
  core fix (promote to `warn!` with structured fields) is unambiguously correct.

### Proposed Fix Summary

In `crates/hares-envelope/src/thermal_solver/stepping.rs` at the
`unwrap_or_else` closure (currently line 58–65):

1. Replace `tracing::debug!` with `tracing::warn!`.
2. Add structured fields: `?zone`, `target_c`, `?e` (already present: `?zone`,
   `?e`); add `target_c` as a field so operators know what temperature was
   requested.
3. Optionally add a per-zone consecutive-failure counter in `ThermalSolver`
   state to suppress repeated warns, emitting a recovery `info!` when the
   solver next succeeds — this is a desirable enhancement but not required for
   the core fix.
4. Add `tracing-test` as a dev-dependency and annotate the regression test
   with `#[traced_test]` + `assert!(logs_contain(...))`.

Do NOT change any production code under `crates/*/src/` as part of this audit.

### Test Written

- **File**: `crates/hares-envelope/src/thermal_solver/mod.rs`
- **Test name**: `solve_ideal_capacity_failure_returns_zero_without_warn`
  (added at line ~2028 in the `#[cfg(test)] mod tests` block)
- **What it tests**: Constructs a `ThermalSolver` with a B matrix whose HVAC
  sensible-input column is zero (so `solve_for_scalar_input` returns
  `ZeroEffectiveGain`), then asserts the function returns `0.0`. This
  demonstrates the silent-failure path is exercised. The test comment
  explicitly documents the missing `warn!` assertion and the steps needed
  (add `tracing-test` dev-dependency, `#[traced_test]` attribute, and
  `logs_contain` assertion) once the ticket fix is applied. Confirmed
  passing with `cargo test -p hares-envelope
  solve_ideal_capacity_failure_returns_zero_without_warn`.
