---
coverage_outcome: adequate
new_test_paths:
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-core/src/dwelling/mod.rs
  - tests/python/test_ev_tariff_integration.py
  - crates/hares-types/src/equipment.rs
  - crates/hares-types/src/control_signal.rs
  - crates/hares-equipment/src/config.rs
  - crates/hares-equipment/tests/control_gate_guard.rs
failing_tests: []
---

# Triage: coverage rounding for I-03

## What I checked

Read `rca.md`, `reproduction.md`, and `coverage-note.md` in full. The note now
has **four** passes: the original three tests, strict-validation coverage,
red-team/silent-fallback hardening coverage, and a new fourth pass after the
control-gate refactor. Every verifiable claim was re-checked against the
current tree by running the tests myself:

**Pass 1 (3 tests):** `ev::tests::init_errors_on_*` → green (5 tests);
`dwelling::tests::auto_register_ev_dedup_is_per_equipment` → green;
`pytest test_ev_tariff_integration.py` → 21 passed, matching the note.
All four `ev::tests::actor_seed_*` tests green; the buggy-behaviour pin
`actor_seed_immediate_returns_none` is gone — replaced, not weakened.

**Pass 2 (3 tests):** the `charging_strategy_validate_*` /
`plug_in_policy_validate_bounds_threshold` tests → green; validators exist at
the cited lines (`equipment.rs:490, 508, 615`).

**Pass 3 (4 tests):** `power_setpoint_soc_window_*` (3) → green, including
the `min >= max` arm no other test reaches;
`config::tests::normalize_config_name_...` → green. Background claims
verified on a prior challenge pass: strict-parser `init_errors_on_unknown_*`
/ `accepted_*_spellings_parse` pairs exist across water heaters and EV;
eviction/dedup is exercised by the python integration class
(`test_ev_tariff_integration.py:469-583`).

**Pass 4 (2 tests, new this run):** `cargo test -p hares-equipment --test
control_gate_guard` → 2 passed. Structural claims verified: the trait
boundary exists as described (`apply_signal` required at lib.rs:176,
`apply_control_unchecked` default at :184, `apply_control` at :194); the
inherent method the guard deliberately ignores exists at
`hvac/heat_pump/heater.rs:2628`; and the migrated-arm rejection tests are
real — `cargo test -p hares-equipment --lib -- unchecked` → 18 passed across
ventilation, scheduled_load, battery, EV, event_load, and water heaters.
(Count imprecision, resolved on challenge: the note says 51 migrated arms.
Actual count: 46 `fn apply_signal` workspace-wide excluding `tests/` dirs,
of which ~14 are test mocks in `dwelling/mod.rs` and 3 are the trait
definition plus test stubs in `lib.rs` — so ~29-30 production equipment
arms. The material claim — every migrated module carries unchecked-path
rejection tests — I verified concretely: 21 `*_unchecked*` test functions
exist and every one of the 22 source files containing an `apply_signal` impl
also contains unchecked-path test coverage (pv, protocol_bridge, dehumidifier,
boiler, furnace, cooler, etc. included). 18 of them pass in a single filtered
run; the rest pass in module suites. The coverage claim is substantiated;
only the number is off.)

**Class measurement — accurate:** exactly two `actor_seed` implementations
(EV, Battery); Battery's `Manual → None` is deliberate and already pinned.

## Value judgment

Each pass's tests answer "what goes wrong without it?" with something real:

- Passes 1–3: as assessed in prior triages (error paths, dedup boundary,
  reported surface, fail-closed validators, the unreached `min >= max` arm,
  the canonicaliser contract behind all strict parsers).
- Pass 4 is the strongest pass conceptually: the control-gate refactor made
  numeric-bounds enforcement a *convention* (doc comment: "Do NOT override"),
  and the constitution is explicit that "a standard that nothing enforces
  erodes." The AST-scanning guard converts that convention into a failing
  test, and the second test keeps the guard non-vacuous. Without it, a future
  equipment implementor overriding `apply_control` would silently bypass the
  entire sweep — the exact defect class I-03 exists to kill.

## Deliberately-uncovered items

Both exclusions justified: pinning `Ev::new()`'s silent fallback would lock in
a smell whose removal is a fix-step decision, and the `debug_assert!` →
`assert!` change is cfg-gated and unobservable.

## Gaps the findings and the work both miss

One candidate, settled previously: whether `Nightly`/`LowSoc` parameters are
consumed (identical 332.1 kWh across variants) — existing `ev_driver` tests
(`ev_nightly_off_peak_charges`, `ev_nightly_peak_idles`,
`ev_low_soc_below_threshold_charges`, `ev_low_soc_above_threshold_idles`)
assert configured window/threshold/target_soc drive decisions. No follow-up.

## Reassessment (challenge pass)

- **Overreach:** none. No fix, no test written, no implementation touched —
  all verification read-only (grep/sed-print/cargo test/pytest).
- **Riskiest unaddressed finding:** `Ev::new()`'s silent fallback to
  `Immediate` (ev/mod.rs:305-308) remains in production — a literal "a
  failure never looks like a success" instance. Re-verified dead-in-practice:
  every production construction path (`create_equipment_from_spec`,
  `add_ev`, `add_ev_with_driver`) calls `init()` immediately after
  `create()`, and `init_typed()` fails loudly. No bypassing caller exists.
  Flagged in both `rca.md` and `coverage-note.md`; removal is a
  production-code decision outside the coverage phase. If a future caller
  ever uses an `Ev` without `init()`, it becomes a live silent-degradation
  defect — that precondition is the documented mitigation.

## Verdict

**Coverage is adequate.** No defect surfaced by any of the four coverage
passes — every added test passes, verified by me on the current tree. No
manufactured action items.
