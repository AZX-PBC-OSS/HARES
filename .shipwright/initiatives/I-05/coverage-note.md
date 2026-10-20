---
coverage_note_outcome: written
---

# Coverage rounding — I-05 (EV actor/equipment silence contract)

## What changed (restated)

The fix gave "idle" an on-the-wire representation: `ChargingComposer::emit_vote`
now dispatches an explicit `PowerSetpoint{0}` hold for idle-shaped votes,
restores target-then-rate ordering, drops the zero-power early return, and
`resolve` no longer drops tied votes' targets or a lower-scored vote's
ceiling. The range-anxiety override is urgency-gated; non-driving days and
the arrival step route through `evaluate_charging`; phantom departures are
removed; the equipment clears latched holds/targets on fresh `SOCTarget`/
`EvSetReadyBy` and on disconnect.

## Class measurement

The defect shape — idle-shaped vote → actor silence → equipment charge-to-full
default — has **one shared choke point** (`emit_vote`,
`crates/hares-core/src/actors/ev_driver/composer.rs`) reachable from ~6 vote
shapes (`TimeWindowPref` outside window, `SocGate` above threshold, V2H/V2G
SOC floors, `SolarSurplus` with no surplus, `SolarTracking` zero-rate,
departure-only votes). Searched via `git diff` of the landed change and
greps for `PreferenceVote::idle`, `power_setpoint_kw`, `soc_target` across
`crates/hares-core/src/actors/ev_driver/` and
`crates/hares-equipment/src/ev/`. The fix's own tests already cover the choke
point per shape (`idle_override_dispatches_explicit_hold`,
`all_idle_scores_dispatch_explicit_hold`,
`zero_power_vote_dispatches_zero_setpoint_not_silence`,
`target_with_rate_dispatches_target_then_rate`,
`targetless_rate_winner_keeps_ceiling_target`,
`tied_vote_target_is_not_dropped`), and the equipment latch-clear is covered
per arm (`soc_target_clears_latched_zero_power_hold`,
`ready_by_clears_latched_zero_power_hold`,
`disconnect_clears_latched_hold_for_away_charging`). No per-site repetition
was warranted.

## Tests added by this pass (3)

All changes are in `#[cfg(test)]` code only; no production code was touched.

1. **`actors::ev_driver::tests::arrival_step_dispatches_strategy_hold_not_a_step_later`**
   — `crates/hares-core/src/actors/ev_driver/mod.rs` (test module).
   Edge: the arrival step itself. Exercises the fix-added
   `if doing_plugin { self.evaluate_charging(env, current_minute, out); }`
   block (~mod.rs:1161). Without it, each homecoming ran one full step at
   rated power toward a stale latched setpoint, outside the configured
   window — previously pinned by no test. Asserts the strategy's hold is in
   the arrival step's own dispatch, ordered after the plug-in (the FIFO
   contract the dwelling relies on), and no charging command outside the
   window.
2. **Equal-minute boundary appended to
   `actors::ev_driver::departure::tests::departure_midnight_wrap`** —
   `crates/hares-core/src/actors/ev_driver/departure.rs` (test module).
   Edge: departure minute equal to current minute. Exercises the
   `diff > 0 / else + 1440` branch of the new shared `minutes_until`
   helper (`crates/hares-core/src/actors/ev_driver/preference.rs`). A `0`
   here would read as "no time left" and fire the urgency override at the
   exact minute the vehicle leaves.
3. **`ev::tests::urgent_ready_by_deadline_charges_through_latched_hold`** —
   `crates/hares-equipment/src/ev/tests.rs`. Edge: deadline-guarantee
   exception. Exercises `compute_charging_power_kw`'s
   `requested = bms_power.max(requested)` deadline branch
   (`crates/hares-equipment/src/ev/mod.rs`) against a hold latched *after*
   an `EvSetReadyBy` (the composer's target-then-rate order makes this
   ordering the reachable one). The fix-plan documented this exception
   ("idle-while-a-deadline-is-urgent legitimately still charges") but no
   test pinned it.

## Verification (commands and output)

```
$ cargo test -p hares-core --lib actors::ev_driver::tests::arrival_step
test result: ok. 1 passed; 0 failed; 0 measured; 950 filtered out
$ cargo test -p hares-core --lib actors::ev_driver::departure
test result: ok. 10 passed; 0 failed; 941 filtered out
$ cargo test -p hares-equipment --lib urgent_ready_by
test result: ok. 1 passed; 0 failed; 2242 filtered out
$ cargo test -p hares-core --lib
test result: ok. 951 passed; 0 failed
$ cargo test -p hares-equipment --lib
test result: ok. 2243 passed; 0 failed
$ cargo clippy --workspace --all-targets -- -D warnings   # clean
$ cargo fmt --check --all                                  # clean
```

No new test fails — no latent defects found in the covered ground.

## Second pass — V2G/V2H soc-floor refactor (new surface since the first pass)

`v2g.rs` and `v2h.rs` changed after the first coverage pass: the soc floor
moved from a `constraint()` Override to an idle vote inside `score()`, so
the floor constrains only the preference's own discharge/export vote
instead of short-circuiting the whole stack. Both files' rewritten unit
tests (`declines_export_at_soc_floor_even_at_high_price`,
`declines_discharge_at_soc_floor_even_with_deficit`) pin the vote shape and
`Constraint::Inactive` — but the refactor's stated *purpose* (the stack's
own `SocTarget` must still charge a vehicle sitting at the floor, because a
latching hold there is self-reinforcing) is an actor-dispatch-level claim
that was pinned nowhere. Added one test on the shared composer mechanism:

4. **`actors::ev_driver::tests::v2h_stack_charges_off_soc_floor`** —
   `crates/hares-core/src/actors/ev_driver/mod.rs` (test module). SOC 0.15
   below `min_soc` 0.2, 19:00 with departure ~13 h out (anxiety override
   stands down), asserts the stack dispatches `SOCTarget(0.9)` and no
   explicit zero-power hold. V2G routes through the identical
   `resolve`/`emit_vote` path and its floor vote is already unit-tested in
   `v2g.rs`, so no per-site repetition.

**Finding (production code, not fixed — this role adds tests only):**
`crates/hares-core/src/actors/ev_driver/v2g.rs`, `V2GExport::score`,
contains the floor comment block and the
`if ctx.current_soc <= self.min_soc { return PreferenceVote::idle("v2g:soc_floor"); }`
check **twice** (the second occurrence is dead — the first returns
unconditionally). `v2h.rs` has the block only once. Harmless at runtime
but a broken window; flagged for the next production-touching pass.

Verification this pass: `v2h_stack_charges_off_soc_floor` passes;
`cargo test -p hares-core --lib` → 955 passed, 0 failed;
`cargo test -p hares-equipment --lib` → 2243 passed, 0 failed; clippy
workspace `--all-targets -D warnings` clean; `cargo fmt --check --all`
clean. No new test fails — no latent defect surfaced.

## Third pass — V2L/V2G floor-landing efficiency budgeting (new surface)

The previously reported `v2g.rs` duplicated floor-check block is fixed
(one occurrence remains). New production surface since the second pass:
`compute_v2l_discharge` and `compute_v2g_discharge`
(`crates/hares-equipment/src/ev/mod.rs` ~942, ~1022) now budget the
floor-landing discharge cap in the DC domain
(`available_kwh * efficiency / dt` instead of `available_kwh / dt`), so a
discharge landing on the reserve floor no longer undershoots it by one
step's (1/efficiency − 1). Existing floor tests
(`v2l_respects_soc_reserve_floor`, `v2g_respects_soc_reserve`) only pin
"zero power at/below the floor" — the landing behavior itself was pinned
nowhere. The cap computation is a separate copy in each of the two
functions (no shared choke point), so both sites get a test:

5. **`ev::tests::v2l_discharge_lands_on_soc_reserve_floor`** —
   `crates/hares-equipment/src/ev/tests.rs`. SOC 0.12, reserve 0.1,
   efficiency 0.9, 5 kW request over 1 h: the energy cap binds, and the
   step must land on 0.1 (≥ floor − 1e-9, ≤ floor + 1e-6) — the old
   AC-domain cap would land at ≈0.0978.
6. **`ev::tests::v2g_discharge_lands_on_soc_reserve_floor`** — same
   contract for the V2G copy (SOC 0.22, reserve 0.2).

Verification this pass: both new tests pass; `cargo test -p hares-core
--lib` → 956 passed, 0 failed; `cargo test -p hares-equipment --lib` →
2245 passed, 0 failed; clippy workspace `--all-targets -D warnings` clean;
`cargo fmt --check --all` clean. No new test fails.

## Fourth pass — drive-shortfall accounting (new surface)

New since the third pass: `Ev::drive_shortfall_kwh`
(`crates/hares-equipment/src/ev/mod.rs`, EvDrive arm ~1812) — a drive
dispatch exceeding the pack's remaining energy delivers what the pack
holds (SOC lands exactly at 0) and accounts the undeliverable remainder as
cumulative observable state, published via the new
`tk::DRIVE_SHORTFALL_KWH` telemetry key (`hares-types`), default telemetry
channel (`ev/telemetry.rs`), and checkpoint schema v4 field
(`ev/checkpoint.rs`). Actor side: trips longer than the observed pack
energy are cancelled (`drive_cancelled` counter,
`crates/hares-core/src/actors/ev_driver/mod.rs`).

Already covered by the landing change: truncation/landing/telemetry
publication (`soc_clamps_at_boundaries`, ev/tests.rs ~3114), cancellation
and counting (`drive_shortfall_is_accounted_not_silently_dropped`,
`cancelled_trip_counts_once_across_departure_window`,
`range_anxiety_active_reports_short_driver`, mod.rs), telemetry-key
registration (telemetry_keys.rs test lists). The stated contract elements
pinned nowhere were (a) cumulative (`+=`, not last-write) semantics and
(b) the new checkpoint field surviving save/load. Added:

7. **`ev::tests::drive_shortfall_accumulates_and_survives_checkpoint`** —
   `crates/hares-equipment/src/ev/tests.rs`. Two successive overdraws
   (1.0 + 2.0 kWh) must sum to 3.0; a checkpoint round-trip into a fresh
   `Ev` must preserve the total (a dropped save/load field would silently
   reset the accounting); double round-trip bytes identical.

Verification this pass: new test passes; `cargo test -p hares-core --lib`
→ 960 passed, 0 failed; `cargo test -p hares-equipment --lib` → 2246
passed, 0 failed; `cargo test -p hares-types` → all suites pass; clippy
workspace `--all-targets -D warnings` clean; `cargo fmt --check --all`
clean. No new test fails.

## Fifth pass — actor snapshot schema versioning (new surface)

New since the fourth pass: the actor-side checkpoint contract
(`crates/hares-core/src/actor.rs`, `checkpoint.rs`, `dwelling/mod.rs`,
`hares-python/src/py_dwelling.rs`). `actor_states` entries became
`ActorStateCheckpoint { name, schema_version, blob }` records;
`DwellingCheckpoint::deserialize_gated` probes `format_version` before the
schema-dependent parse (so a pre-v7 file is rejected as a version mismatch,
not a field-parse failure); the dwelling validates each actor's saved
`schema_version` against its live `checkpoint_version()` before
`load_state`; both Python byte-level restore paths now route through the
gate; `EvDriverSnapshot` gained the `drive_cancelled` counter under the
same v6→v7 bump.

Already covered by the landing change: the old-format rejection naming
both versions (`old_version_checkpoint_rejected_as_version_mismatch_not_parse_error`,
checkpoint.rs), the per-actor round-trip and the tampered-version named
rejection (`checkpoint_actor_state_round_trips_with_schema_version`,
`checkpoint_actor_schema_version_mismatch_rejected_with_named_error`,
dwelling/mod.rs), and the regression runner's actor-state round-trip
(`tests/regression/checkpoint_restart.rs`, updated to the record shape).
The Python paths share `deserialize_gated` with the tested file-load path —
but on reassessment, "same function, no second test warranted" covered only
the happy path: the function's two *error* branches are reachable from the
Python byte-level calls with no SHA-256 pre-check in front of them, and no
test touched either — bytes that are not JSON at all (the probe's
`map_err`), and a body whose probe matches the current version but fails
the full parse. Test 9 below closes both.

The one contract element pinned nowhere: `EvDriverSnapshot`'s round-trip
tests (`save_state_load_state_round_trip_estimated_soc_preserved`,
`post_restore_estimated_soc_not_reset_to_default`) assert only
`estimated_soc`/`current_day_ordinal`/`phase`. A field added to the
snapshot but forgotten in `load_state` compiles cleanly and silently
resets to the constructor default across every checkpoint/restart —
exactly the failure mode the new per-actor version gate is designed to
catch at schema-change time, but with no test pinning the current schema's
fields, the gate would never be exercised by an actual omission. Added:

8. **`actors::ev_driver::tests::save_state_load_state_round_trip_full_snapshot_preserved`**
   — `crates/hares-core/src/actors/ev_driver/mod.rs` (test module).
   Populates every mutable decision field (estimated SOC, day ordinal,
   phase, `needs_away_charge`, `todays_event`, and the new cumulative
   `drive_cancelled` counter) with non-default values, round-trips through
   `save_state`/`load_state` into a freshly-constructed actor, and asserts
   each survives — most pointedly that a cancelled trip stays counted
   across a restart (a forgotten `drive_cancelled` assignment would
   silently re-zero it, letting a cancelled over-long trip re-depart).
9. **`checkpoint::tests::deserialize_gated_rejects_malformed_bytes_loudly`**
   — `crates/hares-core/src/checkpoint.rs` (test module). Exercises
   `deserialize_gated`'s two error branches directly (the entry point the
   Python byte-level restore paths use): non-JSON bytes must fail loudly
   with `checkpoint parse failed` (probe branch), and a current-version
   but structurally invalid body must likewise fail at the full parse —
   not panic, not substitute a default, and not be misreported as a
   version mismatch (the versions agree).

Verification this pass: both new tests pass; `cargo test -p hares-core --lib`
→ 966 passed, 0 failed; `cargo test --test regression` (includes the
checkpoint suites) → 18 passed, 0 failed; `cargo clippy -p hares-core
--lib --tests -- -D warnings` clean; `cargo fmt --check --all` clean. No
new test fails.

## Sixth pass — band-veto fix and phase-carried trip plans (new surface)

New since the fifth pass (fix run v2-fix-3845041d): (a)
`compute_charging_power_kw`'s effective destination became
`controller.max(deadline)` when both a `soc_target` and a `ready_by_soc`
are latched (`crates/hares-equipment/src/ev/mod.rs`), closing the red
team's band-veto finding; (b) `DriverPhase::{Driving, Away}` became
plan-carrying so midnight-crossing trips complete on their own plan, and
`EvDriverActor::checkpoint_version()` was overridden to 2 — the first
real use of the per-actor version gate.

Already covered by the landing change: the band veto in the
controller-below-deadline direction (the red team's
`band_soc_target_must_not_veto_urgent_ready_by_charging`, plus the fix's
sibling `band_soc_target_ceiling_must_not_stop_charging_short_of_urgent_deadline`
covering the early-return branch), the midnight-crossing trip contract
(`midnight_crossing_trip_continues_into_non_driving_day`), and the plan
payload in the snapshot round-trip (test 8 from the fifth pass, updated
to the `Away { plan }` shape). Two contract elements were pinned nowhere:

10. **`dwelling::tests::ev_driver_v1_snapshot_blob_rejected_by_named_version_gate`**
    — `crates/hares-core/src/dwelling/mod.rs` (test module). The
    initiative narrative claims a v1 EvDriver blob is rejected at the
    checkpoint boundary with a named error, but the existing gate tests
    use the stub actor — they would still pass if `EvDriverActor`'s
    `checkpoint_version()` override were dropped back to the trait
    default of 1 (a v1-stamped blob would then decode as v2 without
    error, or postcard-fail deep in `load_state`). This test runs the
    real actor through the dwelling save/restore path, asserts the saved
    entry carries version 2, tampers it to 1, and asserts the named
    mismatch error (actor, `blob=1`, `expected=2`) rather than a
    postcard failure.
11. **`ev::tests::controller_target_above_ready_by_raises_destination`** —
    `crates/hares-equipment/src/ev/tests.rs`. The `max()` rule's mirror
    direction: a controller target (0.95) above the deadline's (0.6)
    raises the destination, so at SOC 0.7 (deadline already satisfied)
    charging must continue toward the ceiling. A future "deadline target
    always wins" simplification of the `max()` would early-return zero
    here; the two band tests pin only the opposite inequality.

12. **`actors::ev_driver::tests::trip_completion_step_publishes_away_phase_telemetry`**
    — `crates/hares-core/src/actors/ev_driver/mod.rs` (test module).
    Added on reassessment: the same fix round gave the `Driving` arm's
    `steps_left == 0` completion exit (~mod.rs:1203) the
    `populate_telemetry` call every other `decide` exit path already had,
    and no test pinned it. Drives one mid-trip step (phase channel reads
    1.0), then the completion step with steps exhausted at arm entry:
    the `phase` channel must publish Away (2.0) on that same step, not
    leave the stale Driving reading. Asserts observable telemetry, not
    the call's presence.

Verification this pass: all three new tests pass; `cargo test -p hares-core
--lib` → 970 passed, 0 failed; `cargo test -p hares-equipment --lib` →
2250 passed, 0 failed; `cargo clippy -p hares-core -p hares-equipment
--lib --tests -- -D warnings` clean; `cargo fmt --check --all` clean. No
new test fails — no latent defects surfaced.

## Seventh pass — battery setpoint mutual-clearing (triage-routed)

`coverage-note-quality.md` (second triage) returned `tests_missing` for one
named gap: the battery's two setpoint channels clear each other on every
write (`apply_signal`'s `PowerSetpoint` arm clears `soc_target`,
`SOCTarget` arm clears `power_setpoint_kw` —
`crates/hares-equipment/src/battery/mod.rs:1802-1803, 1863-1865`), the
battery-side counterpart of the EV latch-clearing this initiative added —
and the RCA's bounded audit had left it "not asserted clean." The triage
verified by reading that no same-class defect exists; what was missing was
the pin. Added:

13. **`battery::tests::soc_target_supersedes_latched_power_setpoint_and_vice_versa`**
    — `crates/hares-equipment/src/battery/mod.rs` (test module). Asserts
    both directions through observable SOC: a fresh `SOCTarget{0.6}` after
    a latched 5 kW charge setpoint stops at the target instead of riding
    Priority 1 toward `max_soc` (this direction pins the
    `power_setpoint_kw = None` clearing line directly — without it the
    setpoint governs forever), and a fresh `PowerSetpoint{-2.0}` after a
    latched target discharges back below it (the supersession contract).

14. **`battery::tests::self_consumption_supersedes_latched_power_setpoint`**
    — `crates/hares-equipment/src/battery/mod.rs` (test module). Added on
    reassessment: the `SelfConsumption` arm clears both setpoints
    (`:1910-1911`) — a third instance of the same supersession class in
    the same file, and genuinely observable (a latched 5 kW setpoint
    would keep Priority 1 charging through a 2 kW net load the battery
    was told to offset). The existing self-consumption tests never latch
    a setpoint first, so the clearing was unpinned. Note on the mirror
    direction of test 13: the `soc_target = None` line in the
    `PowerSetpoint` arm (`:1803`) is not independently observable —
    Priority 1 shadows a stale `soc_target` for as long as the setpoint
    is latched, and every path that unlatches the setpoint clears both —
    so test 13's mirror assertion pins the supersession *contract*, and
    only its first direction is sensitive to a specific clearing line.
    Recorded here so a future pass does not mistake the mirror for
    line-level air cover.

Verification this pass: both new tests pass; `cargo test -p hares-equipment
--lib` → 2252 passed, 0 failed; `cargo test -p hares-core --lib` → 970
passed, 0 failed; `cargo clippy -p hares-equipment --lib --tests --
-D warnings` clean; `cargo fmt --check --all` clean. No new test fails.
The triage's other ledger items (1–9) required no coverage action and are
dispositioned there; nothing re-scoped beyond the flagged gap.

## Eighth pass — priority-scoped destination match (fix run v2-fix-94655832)

New surface since the seventh pass: red-team pass 6 found the band-veto
fix's `controller.max(deadline)` leaking into `ChargingPriority::ExternalAuthority`
(whose contract is the opposite), and the fix scoped the destination rule
by priority — `compute_charging_power_kw`'s target match is now
`(Some, Some) → max` under `DeadlineGuarantee`, `controller` under
`ExternalAuthority` (`crates/hares-equipment/src/ev/mod.rs:630-638`). The
red team's `external_authority_deadline_target_must_not_raise_controller_destination`
pins the ExternalAuthority arm, but commands a rate alongside the target —
the verbatim-setpoint pacing path. The same priority-scoped destination
also governs the no-setpoint BMS-pacing path (`requested = bms_power`,
mod.rs:761-762), which no test exercised with conflicting targets. Added:

15. **`ev::tests::external_authority_target_without_rate_paces_to_controller_destination`**
    — `crates/hares-equipment/src/ev/tests.rs`. Latches a 0.9 ready-by
    deadline, commands `SOCTarget{0.3}` with NO rate under
    `ExternalAuthority`, steps inside the deadline window: asserts the
    pack paces off the start SOC (BMS pacing still runs without a
    setpoint) AND stops at the controller's 0.3, not the deadline's 0.9.
    A revert to the unconditional `max()` raises `soc_limit` to 0.9 and
    fails the ceiling assertion — same sensitivity as the sibling, on the
    other pacing branch. The remaining three match arms ((Some, None),
    (None, Some), (None, None)) are priority-independent and already
    pinned by the generic `soc_target` / ready-by / default-destination
    tests; the ExternalAuthority controller-above-deadline direction is
    the same trivial `controller` return and adds no new branch.

Verification this pass: new test passes (`cargo test -p hares-equipment
--lib external_authority_target_without_rate` → 1 passed); `cargo test -p
hares-equipment --lib` → 2254 passed, 0 failed (the +2 over the seventh
pass's 2252 are the fix run's own red-team test and air-cover test);
`cargo test -p hares-core --lib` → 971 passed, 0 failed (the +1 is the
red team's `snapshot_round_trip_preserves_mid_trip_driving_plan`);
clippy `-D warnings` clean; `cargo fmt --check --all` clean. No new test
fails.

## Ninth pass — empty-blob probe error arm (fix run v2-fix-3da2f572)

New surface since the eighth pass: red-team pass 7 found a truncated
stateful actor blob (empty `blob`, matching `schema_version`) restoring as
silent success via `load_state`'s empty-blob no-op convention; the fix
added the empty-blob probe rule to `load_checkpoint`'s restore loop
(`crates/hares-core/src/dwelling/mod.rs:4259-4278`): an empty blob is
valid only for an actor that also saves empty, else a named rejection —
and a probe failure is itself a named rejection (`"cannot verify empty
state blob"`). Class measurement: the silent-empty-blob-no-op convention
exists exactly once, on the actor side (`Actor::save_state` default +
stateful impls' empty early return); the equipment side has no such
convention — every equipment `load_state` routes through
`load_versioned` (`crates/hares-equipment/src/serial.rs:100`), which
rejects a short/empty blob loudly. So one occurrence, one fix site, and
the rule's three arms: (1) stateful + empty → rejected (red team's
`checkpoint_truncated_stateful_blob_must_not_restore_as_silent_success`),
(2) stateless + empty → clean restore (`checkpoint_stateless_actor_empty_blob_restores_cleanly`),
(3) probe itself fails → named rejection. Arm (3) was unpinned. Added:

16. **`dwelling::tests::checkpoint_empty_blob_probe_failure_rejected_with_named_error`**
    — `crates/hares-core/src/dwelling/mod.rs` (test module). Saves a
    checkpoint with a stateless `StubActor` (legitimately empty blob),
    then loads into a dwelling whose same-named `FailingProbeActor`
    errors from `save_state`: asserts the restore is rejected (not a
    panic or silent pass) with a message naming both the probe failure
    ("cannot verify") and the actor. Without the `map_err` arm the probe
    error would propagate unframed (or worse, if unwrapped, panic) — the
    test pins the named-rejection contract on the restore path's last
    unpinned branch.

Verification this pass: new test passes (`cargo test -p hares-core --lib
checkpoint_empty_blob_probe_failure` → 1 passed); `cargo test -p
hares-core --lib` → 974 passed, 0 failed (the +3 over the eighth pass's
971 are the fix run's two tests plus this one); `cargo test -p
hares-equipment --lib` → 2254 passed, 0 failed; clippy `-D warnings`
clean; `cargo fmt --check --all` clean. No new test fails. The
non-empty-blob path needs no dedicated test: every existing checkpoint
round-trip exercises it.

## Paths deliberately not tested (named, not silently skipped)

- **Urgency-gate exact 1.2× equality** (`hours_left >= needed * 1.2`,
  `mod.rs` ~838): both sides are pinned by
  `range_anxiety_override_preempts_strategy_when_time_runs_short`; an
  equality test would pin the policy margin constant through
  temperature/efficiency-dependent arithmetic — brittle, not observable
  behavior.
- **Equipment-level arrival-step energy**: covered by composition — the
  dispatch contract (test 1 above) plus the seeded multi-day integration
  tests (`nightly_start_hour_governs_hourly_charging_profile`,
  `low_soc_gate_delays_charging_beyond_immediate`).
- **`power_limit_kw` latch**: deliberately retained by design (genuine
  config ceiling; fix-plan's documented decision), so no clearing behavior
  exists to test.
- **V2G at floor, actor level**: the V2H test pins the shared composer
  mechanism; V2G's only distinct input (price) is irrelevant at/below the
  floor, and its floor vote shape is unit-tested in `v2g.rs`. A second
  actor-level test would repeat the same code path.
- **`EvDriverActor::checkpoint_version()` override**: superseded by the
  sixth pass — the actor now overrides the trait to `2` (plan-carrying
  phases), and that participation is pinned end-to-end by test 10
  (`ev_driver_v1_snapshot_blob_rejected_by_named_version_gate`).
- **v3→v4 checkpoint migration**: none exists — `load_versioned`
  (`crates/hares-equipment/src/serial.rs:94`) rejects version-mismatched
  blobs loudly rather than migrating, so there is no silent-misload path
  to pin.
- **`charging_efficiency.max(0.01)` clamp at near-zero efficiency** (both
  discharge fns): config validation already rejects efficiency outside
  (0, 1] (`config.rs:354-357`), so the clamp is unreachable defensive
  code — no observable behavior to pin.
