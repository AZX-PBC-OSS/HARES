---
review_outcome: clean_with_nits_applied
---

# Review — I-05 (EV charging-strategy override does not govern behaviour)

## Goal, and where it is met

The ticket's core complaint is that varying `charging_strategy` produced the same
charging profile for every variant — the strategy's *content* did not govern
behaviour. Completion is defined by two observables:

1. Two `Nightly` overrides differing only in `off_peak_start_hour` produce
   measurably different hourly charging profiles.
2. `LowSoc` differs from `Immediate`.

Both are demonstrated by named tests over the real production composition path
(`EvConfig → Ev::actor_seed() → ActorSeed::Ev → build_actors_from_seeds →
EvDriverActor` stepped against a real `Ev` equipment instance, asserting
*equipment* energy per hour, not actor dispatch):

- `actors::ev_driver::tests::nightly_start_hour_governs_hourly_charging_profile`
  (`crates/hares-core/src/actors/ev_driver/mod.rs:6585`) — `Nightly 22:00–06:00`
  vs `Nightly 02:00–06:00` charge only inside their own windows; the first
  charging hour after the 18:00 plug-in is each run's own window start. Passes.
- `actors::ev_driver::tests::low_soc_gate_delays_charging_beyond_immediate`
  (`mod.rs:6702`) — `LowSoc` rests on days the gate holds and charges on strictly
  fewer days than `Immediate`. Passes.

The root-cause fix is the explicit hold: `ChargingComposer::emit_vote`
(`crates/hares-core/src/actors/ev_driver/composer.rs`) now dispatches
`PowerSetpoint{0}` for every idle-shaped vote instead of silence, and the
equipment clears that hold on a fresh `SOCTarget`/`EvSetReadyBy` and on
disconnect (`crates/hares-equipment/src/ev/mod.rs`, `apply_control`). The two
reproduction tests pinch this at the equipment level and are green:
`idle_nightly_window_does_not_charge_equipment`,
`idle_low_soc_gate_does_not_charge_equipment`.

## Wiring — every new surface has a supplier and a reader

- `EvConfig.charging_strategy` / `ActorSeed::Ev.strategy`: supplier is the
  `Equipment["EV"]` override merged field-wise into the typed config by
  `apply_equipment_overrides` (`crates/hares-core/src/dwelling/conversions.rs:843`),
  parsed by `Ev::init_typed` via `parse_charging_strategy`
  (`crates/hares-equipment/src/ev/config.rs:180`); reader is
  `hares-core`'s `build_actors_from_seeds` `ActorSeed::Ev` arm
  (`crates/hares-core/src/dwelling/mod.rs:7062`), which constructs the driver
  with the resolved strategy.
- `Actor::checkpoint_version()` (new trait method, default 1; `EvDriverActor`
  overrides to 2): writer is `Dwelling::save_checkpoint`, reader is
  `Dwelling::load_checkpoint`'s per-actor version gate.
- `ActorStateCheckpoint { name, schema_version, blob }`: written in
  `save_checkpoint`, read in `load_checkpoint`, exercised by
  `tests/regression/checkpoint_restart.rs`.
- `DwellingCheckpoint::deserialize_gated`: callers are `DwellingCheckpoint::load`
  (file path), and both Python byte-level restore paths
  (`crates/hares-python/src/py_dwelling.rs` `load_state`,
  `restore_building_state`).
- `Ev::drive_shortfall_kwh` / `DRIVE_SHORTFALL_KWH`: written in the `EvDrive`
  arm (`crates/hares-equipment/src/ev/mod.rs:1837`), read by checkpoint
  save/load (v4), default telemetry, and `telemetry_fields()`; the key is
  registered in `crates/hares-types/src/telemetry_keys.rs` (both test lists).
- `EvDriverActor::drive_cancelled` / `range_anxiety_active`: written at the
  departure-cancel branch and `populate_telemetry`, persisted through
  `EvDriverSnapshot`, observable via actor telemetry.
- `DriverPhase::{Driving, Away}` `plan: DayEvent`: written in `decide`, read by
  the phase arms and persisted/versioned (this is the actual reason for
  `checkpoint_version() == 2`).
- `minutes_until` (shared wraparound helper): readers are `DepartureDeadline`
  and the range-anxiety urgency gate — one implementation.
- The latch-clearing in `apply_control` (`power_setpoint_kw`/`soc_target`
  cleared on `SOCTarget`, `EvSetReadyBy`, and `EvPlugIn{Disconnected}`): reader
  is `compute_charging_power_kw` and the away-charging arm.
- V2G/V2H soc-floor moved from `constraint()` to `score()`: consumed by the
  shared composer choke point, which is what makes the floor stop banning
  recharging.

No declared field, flag, event, or optional dependency is orphaned.

## What I chose not to raise, and why

- **The ticket's literal `LowSoc { threshold: 0.15 }` still does not differ from
  `Immediate`.** This is real, and I did not hide it (see Acknowledged
  limitations). I did not file it as a finding because it is not a defect in
  this change: the cause is the pre-existing hardcoded
  `daily_drive_miles = 30` / `range_anxiety_miles = 20` in
  `build_actors_from_seeds` (`dwelling/mod.rs:~7078`) plus the separately
  diagnosed non-standard HPXML EV element, both scoped to
  `docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`
  (verified present). The operator was put this exact ship/hold choice
  (fix-plan `blocked`) and chose to proceed. Re-opening it would re-litigate an
  operator decision under a real named constraint.
- **Held EV's heater can still run during a strategy hold** — the heater gate
  (`would_charge_underated`) cannot distinguish cold-derate from strategy hold
  in the current control vocabulary; `DEFAULT_HEATER_POWER_W = 0.0` makes it
  inert for every fixture and the completion bar. Documented, conceded.
- **Urgent `EvSetReadyBy` deadline charges through a `PowerSetpoint{0}` hold** —
  this is the intended `DeadlineGuarantee` contract, pinned by
  `urgent_ready_by_deadline_charges_through_latched_hold`. Not a defect.
- **Idle-hold widens the last-write-wins window vs the occupant actor's
  higher-tier `PowerSetpoint`** — the occupant re-sends every present step at a
  higher tier; the workspace-wide dispatcher contract owns this. Conceded.
- **Completion-bar tests hand-build `EvConfig` rather than driving the HPXML
  parser** — the HPXML→`EvConfig` link is a separate subsystem (and where
  ticket 132 lives); the override-merge mechanism
  (`typed_equipment_override_applies_known_keys`), the `charging_strategy`
  string parse/validation (`crates/hares-equipment/src/ev/tests.rs:5936-6270`),
  and the `build_actors_from_seeds` wiring are each independently tested. An
  Observation, not a gap worth a round.
- **Stale "charge to full" comments** — the override now charges minimally to
  the anxiety band, not full; four comments said otherwise. Contradictory
  documentation, but comment-only. Fixed below, not filed.

## Acknowledged limitations (documented, tracked, real)

- **Literal `LowSoc { threshold: 0.15 }` is governed by the range-anxiety
  override, not its own gate.** `anxiety_soc ≈ 0.25–0.28` (from the hardcoded
  seed inputs) sits above 0.15, so the ticket's own example cannot be exercised
  until ticket 132 resolves the HPXML parser element and threads the
  driving-behaviour parameters through `EvConfig`. The completion-bar test
  demonstrates the mechanism at `threshold: 0.65` instead. This is the one
  place the ticket's literal wording is not met, and it is operator-approved
  and tracked, not silently dropped.
- Heater/preconditioning ambiguity (above), and the held-EV
  idle-hold/dispatcher window (above) carry the same disposition from the
  initiative's Known Limitations.

## Why I found nothing blocking

I attacked the high-risk seams and they held under direct reading:

- **The latch.** The RCA's fact (c) — "every setpoint latches until
  `init_typed`" — is the trap the naive hold fixes itself with. The
  `apply_control` clears are present in exactly the three arms that need them
  (`SOCTarget`, `EvSetReadyBy`, `Disconnected`), and the discharge fast-path in
  `compute_charging_power_kw` (`power_setpoint_kw < 0.0`) is untouched by the
  idle hold (`0.0` is not `< 0.0`).
- **The floor-domain math.** `apply_soc_and_thermal`
  (`crates/hares-equipment/src/ev/mod.rs:1286`) draws DC at
  `ac_discharge_kw / charging_efficiency`; the new v2l/v2g cap
  (`available_kwh * efficiency / dt`) is the correct AC equivalent that lands
  on the floor instead of undershooting it. The reversal would be a real bug;
  the arithmetic is right and pinned by
  `v2l_discharge_lands_on_soc_reserve_floor` / `v2g_…`.
- **The resolve rewrite.** `resolve()` no longer lets a targetless rate winner
  erase a ceiling (`best_target_score` fold), and the `>=` tie-break no longer
  drops a tied vote's target. The composer tests pin all three shapes
  (`targetless_rate_winner_keeps_ceiling_target`, `tied_vote_target_is_not_dropped`,
  `target_with_rate_dispatches_target_then_rate`).
- **The V2G/V2H floor deadlock.** Moving the floor from `constraint()` to
  `score()` is the correct shape: a floor `Override(idle)` would short-circuit
  the stack's own `SocTarget` and, under the new hold contract, latch the
  vehicle at its floor forever. Pinned by
  `v2g/v2h_soc_floor_override_does_not_deadlock_stack_target_recovery`.
- **The checkpoint gate.** `deserialize_gated` probes `format_version` before
  the schema-dependent parse, so a pre-v7 file is rejected as a version
  mismatch, not a field-parse failure. The per-actor version gate and the
  empty-blob probe close the two remaining silent-restore shapes. All four arms
  (stateful round-trip, version mismatch, truncated stateful, stateless empty,
  probe-failure, drained-state) are tested.
- **Determinism.** `minutes_until` gives one wraparound implementation; the
  completion-bar tests use a fixed seed, no wall-clock or ambient state; no
  log-capture assertions anywhere in the touched tests.
- **No silenced gates.** No `#[ignore]`, no `#[should_panic]` introduced to
  mask a fix; the two `#[should_panic]` in the diff are pre-existing invariant
  diagnostics. Clippy `-D warnings` is clean; no `TODO`/`FIXME`/`HACK` added.

## fixes_applied

Trivial, comment-only corrections (the override target changed from "charge to
full" to the minimal anxiety-band top-up in an earlier operator-directed round;
four comments still said "charge to full"):

- `crates/hares-core/src/actors/ev_driver/mod.rs:1019` — `evaluate_charging`
  doc comment now says "top up minimally to the anxiety band".
- `crates/hares-core/src/actors/ev_driver/mod.rs:1092` — step-start refresh
  comment now says "(anxiety-band) estimate".
- `crates/hares-core/src/actors/ev_driver/mod.rs:2078` — test doc comment.
- `crates/hares-core/src/actors/ev_driver/mod.rs:2111` — inline test comment.

No behaviour changed.

## Verified

- `cargo test -p hares-core --lib` → **975 passed, 0 failed**.
- `cargo test -p hares-equipment --lib` → **2254 passed, 0 failed**.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean, no warnings.
- `cargo fmt --check --all` → exit 0.
- `cargo test --test regression` → **18 passed, 0 failed, 4 ignored**.
- `cargo test -p hares-core --lib actors::ev_driver::tests::nightly_start_hour_governs_hourly_charging_profile` → ok.
- `cargo test -p hares-core --lib actors::ev_driver::tests::low_soc_gate_delays_charging_beyond_immediate` → ok.
- `cargo test -p hares-core --lib actors::ev_driver::tests::idle_` → both
  reproduction tests ok (equipment SOC held at 0.5 through the idle window).
- **Fix is causal, not coincidental.** I temporarily reverted the idle-hold
  branch in `emit_vote` back to pre-fix silence (edit tool, marker
  `REVIEW-PROBE`, then restored and confirmed the marker gone) and ran the
  reproduction test: it **FAILED with `instead it charged to 0.932`** — the
  exact pre-fix charge-to-full signature. Restoring the fix returned it to ok.
- `cargo test -p hares-core --tests` → full integration set green (43 test
  binaries; includes `checkpoint_regressions`, `actor_telemetry_regressions`,
  `dispatch_ordering_regressions`, `engine`).
- `uv run pytest tests/python/test_ev_archetypes.py
  tests/python/test_ev_tariff_integration.py` → **56 passed**;
  `uv run pytest tests/python/test_py_dwelling_integration.py
  tests/python/test_bms_modes.py` → **61 passed** (the four Python suites the
  plan's Verification widened the gate to include).
- `cargo test -p hares-types` → all four result blocks ok (432 passed total).
- `maybe_push_range_anxiety_override` (`crates/hares-core/src/actors/ev_driver/mod.rs`)
  pushes `SOCTarget { target_soc: anxiety_soc.clamp(0.0, 1.0) }` — the band, not
  1.0, confirming the "charge to full" comments I corrected were stale.
- `apply_soc_and_thermal` (`crates/hares-equipment/src/ev/mod.rs:1286`) draws
  discharge DC at `ac_discharge_kw / charging_efficiency`, confirming the
  DC-domain floor cap `available_kwh * efficiency / dt` is the correct inverse.
- `docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`
  exists and names the parser element, the hardcoded seed inputs, and the
  threading steps — the deferral is real, not an abandoned item.
- `apply_equipment_overrides` (`crates/hares-core/src/dwelling/conversions.rs:843`)
  merges the `Equipment["EV"]` override object into the typed config data map,
  which is how `charging_strategy` reaches `EvConfig`.

### Similar-issue sweep (searched fresh, not assumed)

- `grep -rln "power_setpoint_kw" crates/hares-equipment/src/` → only
  `battery`, `ev`, `generator` carry the field.
- Battery (`crates/hares-equipment/src/battery/mod.rs`): absent a setpoint it
  falls to context-sensitive self-consumption, not a fixed extreme, and the
  mutual `soc_target`/`power_setpoint_kw` clearing is now pinned by this
  initiative's own coverage pass (`soc_target_supersedes_latched_power_setpoint_and_vice_versa`,
  `self_consumption_supersedes_latched_power_setpoint`).
- Generator (`crates/hares-equipment/src/generator.rs:1259`): a silent default
  falls back to `current_power_kw` (its own last output), not charge-to-max,
  and has an explicit disarm path (`:1211`, `:2023`) — not the same class.
- Water heater / HVAC: thermostatic self-regulation around a setpoint band
  (prior RCA bounded check, re-confirmed as cited). A sweep of
  `unwrap_or(self.*)` across `crates/hares-equipment/src/` shows only
  "previous value" defaults (`min_soc`, `max_soc`, `setpoint_c`); the EV's
  `ready_soc` fixed-target default is the sole charge-to-extreme case, and it
  is the one the actor-side hold closes.
