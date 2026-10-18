---
red_team_outcome: clean
failing_tests: []
---

# Red-team pass 9: I-03 — attacking the class fix itself

Ninth traversal. Since pass 8 the fix round landed the class remedy the
operator asked for: the `Equipment` trait now carries the enforcement
itself. `apply_control_unchecked` became a default method that runs
`ControlSignal::validate_numeric_bounds()` and delegates to a new required
per-equipment arm, `apply_signal`; `apply_control` validates (with
rejection accounting) and calls `apply_signal` directly;
`delegate_equipment!` forwards `apply_signal`; every arm was renamed; and a
syn-based guard test (`crates/hares-equipment/tests/control_gate_guard.rs`)
fails if any `impl Equipment` in that crate's `src` overrides either gated
method. All 14 pass-8 findings are green (verified: 2438 passed before my
edits).

This pass attacked the fix's own surface — the enforcement mechanism, not
the arms it fronts — and the fix held. Every hypothesis was either closed
with read evidence or pinned with a test:

## Attacks that landed as permanent pins (both green)

1. **The guard's scope hole** — `control_gate_guard.rs` scans only
   `hares-equipment/src`, but the `Equipment` trait is public: an
   `impl Equipment` in any sibling crate overriding
   `apply_control`/`apply_control_unchecked` would silently drop the
   central bounds enforcement for that equipment, and nothing scanned for
   it. No live instance exists today (workspace grep: the only
   `fn apply_control_unchecked` outside the trait is the heat-pump
   heater's *inherent* method, reached through its gated `apply_signal`
   forwarder — and all cross-crate `impl Equipment` sites are test stubs
   that implement `apply_signal`). The avenue is now guarded:
   `crates/hares-equipment/tests/workspace_gate_guard.rs` scans every
   sibling crate's `src` and `tests` with the same syn-based collector
   (plus a vacuity self-test proving it flags a synthetic override and
   ignores the heater's inherent-method pattern). Green — zero violations
   across the workspace.
2. **The third raw-store arm** — the heat-pump heater core keeps its own
   arm match (an inherent `apply_control_unchecked` method reached through
   its gated `apply_signal` forwarder) — distinct from the AC core arm
   from the AC core arm (pinned in pass 8) and from the shared
   heating-helpers path (pinned via the furnace test in pass 8). No test
   had ever driven it. Pin:
   `hvac::heat_pump::heater::tests::out_of_domain_control_values_rejected_on_unchecked_path`
   — six out-of-domain probes (DutyCycle 1.5, LoadFraction 5.0,
   PowerLimit −5.0, MaxCapacityFraction 5.0, deadband 100, IdealCapacity
   NaN) through `apply_control_unchecked` on an `ASHPHeater`. Green — the
   trait default fronts the arm; the one untested arm family now has air
   cover.

## Hypotheses closed with evidence (declined, no test needed)

- **Remaining gate overrides** — workspace grep for
  `fn apply_control_unchecked`/`fn apply_control` inside `impl Equipment`:
  none (heater's is inherent and gated via its forwarder; dwelling's
  `apply_control` hits are Dwelling/Fleet-level methods, not trait
  overrides).
- **Direct `apply_signal` callers** bypassing both gates — every caller is
  the trait default, the delegation macro, a wrapper's own `apply_signal`
  override, or a `#[cfg(test)]` stub (dwelling/mod.rs:10419).
- **Central validator completeness** — it is now the single line of
  defense on both paths, so its coverage was re-measured: every `f64`
  field in `ControlSignal` (enumerated against the enum definition) has a
  validator arm — ThermalSetpoint (+h+db<c), HumiditySetpoint (+ordering),
  PowerSetpoint (active/reactive/min_soc/max_soc +ordering), PowerLimit
  (+ramp), SOCTarget (+ordering), DutyCycle (+period), LoadFraction,
  CurtailmentPercent, ReactiveSetpoint, PowerFactorSetpoint, IdealCapacity
  (finiteness — negative is legitimate cooling), ThermalSetpointDelta,
  DemandResponse duration, EventDelay, MaxCapacityFraction, EvDrive,
  EvAwayCharge, EvSetReadyBy. The only unvalidated payloads are enums and
  bools.
- **Double validation on the checked path** — `apply_control` validates
  once and calls `apply_signal` directly (lib.rs:227), not the unchecked
  wrapper.
- **Protocol-bridge replay bypass** — drained bridge commands re-enter as
  `DispatchRequest`s routed through the dispatcher's
  `eq.apply_control(&request.signal)` (dwelling/mod.rs:1091, 1186) — the
  validated, capability-gated path.
- **Check-ordering change on the unchecked path** (numeric errors now
  precede state errors) — message-content assertions across the suite
  still pass; no observable contract broken.
- **The arms' now-redundant finiteness guards** (EV/battery/PV kept their
  arm-level checks beneath the central gate) — harmless
  defense-in-depth, not a defect; their disposition is the fixer's call.

## Suite state (verified fresh this pass)

- `cargo nextest run --release --no-fail-fast --profile ci -p hares-equipment`
  → **2441 run: 2441 passed, 0 skipped** (2438 pre-existing + the two
  guard tests + the heater pin, all green).
- `cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core`
  → 1078 passed, 21 skipped.
- `cargo nextest run --release --no-fail-fast --profile ci -p hares-types`
  → 420 passed.
- `tests/python/test_ev_tariff_integration.py` → 21 passed.
- `cargo fmt --check --all` → clean;
  `cargo clippy --workspace --all-targets -- -D warnings` → clean.

No production code was touched; the edits are one test function in
`crates/hares-equipment/src/hvac/heat_pump/heater.rs`, the new
`crates/hares-equipment/tests/workspace_gate_guard.rs`, and this note.
The outcome is `clean`: the class fix was attacked at its enforcement
mechanism and survived; the two attacks that could not be closed by
reading are now permanent green pins in the suite.
`failing_tests: []` is the designed declaration — nothing is red, nothing
needs the fix traversal.
