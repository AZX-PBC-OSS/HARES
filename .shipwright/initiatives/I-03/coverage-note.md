---
coverage_note_outcome: written
---

# Coverage rounding: I-03 (EV `Immediate`/default strategy never charges)

Additional tests *were* warranted and were written; this note records the class
measurement, the tests added, what each covers, and what was deliberately left
uncovered.

## Class measurement

The defect shape — `actor_seed()` returning `None` for a default/trivial
variant, withholding the only built-in mechanism that simulates the resource's
use — occurs at exactly **2** sites, found by searching the workspace for
`fn actor_seed` implementations with early-return-`None` guards:

1. `crates/hares-equipment/src/ev/mod.rs:1369` (`Immediate` → `None`) — the
   proven defect, fixed; the fix's tests pin the corrected contract on the
   shared mechanism for all nine `ChargingStrategy` variants.
2. `crates/hares-equipment/src/battery/mod.rs:1631` (`BmsMode::Manual` →
   `None`) — deliberate: a manually driven battery needs no use-simulation.
   Already covered by `actor_seed_manual_returns_none` and
   `actor_seed_self_consumption_returns_battery_seed`
   (`crates/hares-equipment/src/battery/mod.rs:5149,5157`). No test added.

## Tests added (3)

1. **`crates/hares-equipment/src/ev/tests.rs::ev::tests::init_errors_on_malformed_charging_strategy_override`**
   — failure path adjacent to the change: a malformed `charging_strategy`
   override must fail `Ev::init()` loudly with an error naming the key
   (exercises the `require_typed::<EvConfig>()` parse and `?` at
   `crates/hares-equipment/src/ev/mod.rs:460-465`), never silently degrade to
   `Immediate`. The RCA proved this with a scratch test that was removed; this
   pins it permanently.

2. **`crates/hares-core/src/dwelling/mod.rs::dwelling::tests::auto_register_ev_dedup_is_per_equipment`**
   — boundary of the fix's canonical-name dedup: with EV1 already having a
   manually attached `EvDriver:EV1` and EV2 having none, re-registration must
   build exactly `["EvDriver:EV2"]` (exercises the
   `existing_names.contains(&actor_name)` skip at
   `crates/hares-core/src/dwelling/mod.rs:6821-6823`). The existing
   `auto_register_ev_idempotent` test would not catch dedup keyed too coarsely
   (any-`EvDriver`-exists suppressing all EV seeds).

3. **`tests/python/test_ev_tariff_integration.py::TestHpxmlDeclaredEv::test_hpxml_declared_ev_default_strategy_charges`**
   — the exact reported surface, previously covered only at unit level: an
   `<ElectricVehicle>` declared in the HPXML document with no
   `charging_strategy` must draw > 10 kWh of charging over a 3-day simulation.
   Exercises `crates/hares-io/src/hpxml/resolve_der.rs:177-242` →
   construction-time `auto_register_actors`
   (`crates/hares-core/src/dwelling/mod.rs:2539`) → `Ev::actor_seed`. A flat
   power column here means the driver was never attached.

## Verification (commands run, output observed)

- `cargo test -p hares-equipment --lib init_errors_on_malformed` →
  `test result: ok. 1 passed; 0 failed`
- `cargo test -p hares-core --lib auto_register_ev_dedup_is_per_equipment` →
  `test result: ok. 1 passed; 0 failed`
- `.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q` →
  `14 passed`
- Full gates green: `cargo fmt --check --all`;
  `cargo clippy --workspace -- -D warnings`;
  `cargo nextest run --release --no-fail-fast --profile ci -p hares-equipment`
  (2389 passed);
  `cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core`
  (1077 passed). An initial apparent failure of
  `dwelling::tests::run_timestep_panics_on_unhealthy_actor` was my own
  invocation missing the documented `-F check_invariants` flag, not a
  regression.

## Deliberately not covered

- `Ev::new()`'s silent fallback to `Immediate` on unparseable raw config
  (`crates/hares-equipment/src/ev/mod.rs:305-308`): a latent smell the RCA
  flagged as dead-in-practice (every construction path re-parses via
  `init_typed()`, which fails loudly). Pinning it with a test would lock in
  the smell; the fix-step decision on whether to remove it belongs elsewhere.
- The `debug_assert!` → `assert!` change in
  `crates/hares-fleet/src/aggregation.rs:644` is a cfg-gated invariant check,
  not observable behaviour; triggering it means panicking under
  `check_invariants`, which no test can usefully assert beyond the existing
  invariant-test infrastructure.

## Second pass (after the fix grew strict validation)

The fix later added strict validation to `Ev::init_typed` and
`Battery::init_typed` (`crates/hares-equipment/src/ev/mod.rs:418-484`,
`crates/hares-equipment/src/battery/mod.rs:998-1003`) backed by new shared
validators in `crates/hares-types/src/equipment.rs` (`ChargingStrategy::validate`
:615, `PlugInPolicy::validate` :490, `DepartureConstraint::validate` :508).
By this pass, EV/Battery init error paths were already covered
(`init_errors_on_domain_invalid_charging_strategy_override`,
`init_errors_on_unparseable_initial_connection_state_override`,
`init_errors_on_unparseable_chemistry_override` in
`crates/hares-equipment/src/ev/tests.rs`;
`init_errors_on_unparseable_chemistry_override` in
`crates/hares-equipment/src/battery/mod.rs`), and hares-types already had
variant-specific validator tests (`equipment.rs:2762-2920`: departure
constraint reject/accept, V2H inverted, V2G invalid, SolarSurplus invalid +
nested, TouAware buffer, Nightly hours, all-variants acceptance).

Genuinely uncovered cases, now tested on the shared mechanism
(`crates/hares-types/src/equipment.rs`, 3 tests):

1. `charging_strategy_validate_rejects_out_of_domain_values` — NaN
   `target_soc`, `QuickThenWait` `partial_soc` out of [0,1], NaN Nightly hour,
   `LowSoc` `target_soc` > 1, and an invalid `DepartureConstraint` nested in a
   `PreDeparture` schedule (that arm's loop was the only nested-schedule loop
   without a rejection test).
2. `charging_strategy_validate_accepts_boundary_valid_values` — acceptance at
   the exact bounds (0.0/1.0 fractions, hour 23.999, zero charge buffer, V2H
   `min_soc == discharge_threshold_soc`, zero `max_export_kw`); prior
   acceptance tests used only mid-range values, so an off-by-one in a bound
   would have stayed green.
3. `plug_in_policy_validate_bounds_threshold` — `PlugInPolicy::validate` had
   no direct test (only indirect via EV init): pins 0.0/1.0 acceptance and
   1.5/NaN rejection.

Cases I first drafted and then dropped as duplicates of existing tests
(`equipment.rs:2762-2920`): departure-minute bounds, V2H inverted SoC, V2G
negative export/NaN price, SolarSurplus negative rate, Nightly hour 25.0.

Verified: `cargo test -p hares-types --lib -- charging_strategy_validate_rejects_out_of_domain charging_strategy_validate_accepts_boundary plug_in_policy_validate_bounds`
→ `test result: ok. 3 passed; 0 failed`; full gates green
(`cargo fmt --check --all`, `cargo clippy --workspace -- -D warnings`,
`cargo nextest run --release --no-fail-fast --profile ci -p hares-types -p hares-equipment`
→ 2811 passed,
`cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core`
→ 1077 passed,
`.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q`
→ 18 passed).

## Third pass (after red-team pass 7 / the silent-fallback hardening round)

The tree grew substantially: strict `normalize_config_name`-based parsers
across PV/water-heater/HVAC configs, `dispatch_target_name` actor eviction,
and an SOC-window validation block added to `ControlSignal::PowerSetpoint` in
`validate_numeric_bounds`
(`crates/hares-types/src/control_signal.rs:310-334`). Most of that surface
landed with its own tests (each new strict parser has an
`init_errors_on_unknown_*` / `accepted_*_spellings_parse` pair; eviction is
covered by the python integration class). Two gaps remained:

1. **SOC-window validation shape: 2 occurrences, 1 tested.** The
   min_soc/max_soc window rules appear twice in `validate_numeric_bounds` —
   `SOCTarget` (fully tested: `soc_target_valid`, `soc_target_out_of_range`,
   `soc_target_ordering`) and the just-added `PowerSetpoint` block, which had
   no test with a non-`None` window. Added 3 tests in
   `crates/hares-types/src/control_signal.rs` mirroring the SOCTarget
   conventions: `power_setpoint_soc_window_valid` (inclusive 0.0/1.0 bounds,
   single-sided window), `power_setpoint_soc_window_out_of_range` (−0.1, 1.5,
   NaN min; 1.1, −∞ max), `power_setpoint_soc_window_inverted_or_empty_rejected`
   (min 0.8 > max 0.2; min == max — the `min >= max` arm at :327-333, which no
   other test reaches).
2. **`normalize_config_name` (`crates/hares-equipment/src/config.rs:93`): 8
   call sites, 0 tests of its documented contract.** Every consumer's
   spelling tests used only canonical case. Added one test on the shared
   helper, `config::tests::normalize_config_name_is_case_space_and_underscore_insensitive`,
   pinning case/whitespace/underscore insensitivity ("Level 1", "level_1",
   "LEVEL1", padded/doubled separators) and that distinct names still
   normalise distinctly — covering all 8 parsers without per-site
   repetition.

Verified: `cargo test -p hares-types --lib -- power_setpoint` → 5 passed;
`cargo test -p hares-equipment --lib -- config::tests::normalize` → 1 passed.
Full gates green: `cargo fmt --check --all`;
`cargo clippy --workspace -- -D warnings`;
`cargo nextest run --release --no-fail-fast --profile ci -p hares-types -p hares-equipment`
→ 2842 passed;
`cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core`
→ 1078 passed;
`.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q`
→ 21 passed.

## Fourth pass (after the control-gate refactor)

The hardening sweep restructured the control boundary:
`Equipment::apply_control_unchecked` became a default trait method that
centrally enforces `ControlSignal::validate_numeric_bounds()` before
delegating to the new per-equipment `apply_signal` arm
(`crates/hares-equipment/src/lib.rs:166-190`, 51 arm implementations
migrated). Every migrated arm landed with its own unchecked-path rejection
tests (`*_rejects_*_on_unchecked_path` across ev, battery, event_load,
scheduled_load, ventilation, ideal_hvac, furnace, air_conditioner, water
heaters), so the per-site surface is covered.

What nothing enforced: the gate itself. Both `apply_control` and
`apply_control_unchecked` remain overridable trait methods; a future
implementor overriding either silently bypasses the central numeric-bounds
enforcement the entire sweep relies on — the exact defect class this
initiative exists to kill. The only protection was a doc comment
("Do NOT override"). Following the `si_guard.rs` precedent, added
`crates/hares-equipment/tests/control_gate_guard.rs` with two tests:

1. `no_equipment_impl_overrides_the_gated_control_boundary` — AST-scans
   every `impl Equipment for …` block under `src/` (syn, so comments and
   strings cannot false-positive) and fails if any defines `apply_control`
   or `apply_control_unchecked`. Currently 0 overrides: the only other
   occurrence of the name is an inherent method on the heat-pump heater
   (`hvac/heat_pump/heater.rs:2628`), reached via the trait's `apply_signal`
   forwarder, which the guard deliberately ignores.
2. `guard_flags_synthetic_override_and_ignores_compliant_code` — keeps the
   guard non-vacuous: proves the collector flags a synthetic override and
   passes both a compliant `apply_signal`-only impl and the inherent-method
   pattern.

Verified: `cargo test -p hares-equipment --test control_gate_guard` → 2
passed. Full gates green: `cargo fmt --check --all`;
`cargo clippy --workspace --all-targets -- -D warnings`;
`cargo nextest run --release --no-fail-fast --profile ci -p hares-types -p hares-equipment`
→ 2858 passed;
`cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core`
→ 1078 passed;
`.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q`
→ 21 passed.

No production code was changed in this phase; all edits are test code.
