---
coverage_outcome: adequate
new_test_paths:
  - crates/hares-core/src/actors/ev_driver/mod.rs
  - crates/hares-core/src/actors/ev_driver/departure.rs
  - crates/hares-core/src/actors/ev_driver/composer.rs
  - crates/hares-core/src/checkpoint.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-equipment/src/battery/mod.rs
failing_tests: []
---

# Coverage triage — I-05 round-out-coverage (fifth triage pass, post-ninth-pass)

This supersedes the fourth triage. New surface since: red-team pass 7 found
a third real defect (a truncated stateful actor blob restoring as silent
success through `load_state`'s empty-blob no-op convention), fix run
v2-fix-3da2f572 added the empty-blob probe rule to the restore loop, and the
coverage note's ninth pass pinned the probe rule's last unpinned arm. Prior
passes' assessments stand; the ledger below carries them forward.

## The routed item, closed

Second triage routed one specific gap: `crates/hares-equipment/src/battery/mod.rs`'s
`apply_signal` clears the opposite setpoint channel on every `PowerSetpoint`/
`SOCTarget` write (:1802-1803, :1863-1865) — the battery-side counterpart of
the EV latch-clearing this initiative added — but no test pinned it, and the
RCA had left the battery "not asserted clean."

The seventh pass added:

- **`battery::tests::soc_target_supersedes_latched_power_setpoint_and_vice_versa`**
  — both directions through observable SOC; the first direction is sensitive
  to the specific clearing line (a dropped `power_setpoint_kw = None` fails
  it), the mirror pins the supersession contract. The note is honest about
  the asymmetry (the `soc_target = None` line is not independently
  observable while Priority 1 shadows it) rather than overclaiming
  line-level air cover — verified against the code: the claim is accurate.
- **`battery::tests::self_consumption_supersedes_latched_power_setpoint`** —
  found a third instance of the same class in the same file that the triage
  itself had not named: the `SelfConsumption` arm clears both setpoints
  (verified: `battery/mod.rs:1910-1911`, exactly as cited). Genuinely
  observable, previously unpinned. This is the coverage pass out-verifying
  its triage input — the loop working.

## Independent verification (re-run this session)

- Both new tests pass individually; `cargo test -p hares-equipment --lib
  battery::tests` → 170 passed, 0 failed.
- Full gates: `cargo test -p hares-core --lib` → 970 passed, 0 failed;
  `cargo test -p hares-equipment --lib` → 2252 passed, 0 failed (counts
  match the note); `cargo clippy --workspace --all-targets -- -D warnings`
  clean; `cargo fmt --check --all` clean.
- One adjacent check prompted by reading the `SelfConsumption` arm: the
  battery also accepts `PowerLimit` into `external_power_limit_kw`
  (:1913-1915) with no runtime clearing — the battery-side instance of the
  DR `PowerLimit` latch already dispositioned as finding 4 (below). Same
  disposition applies: DR treats `PowerLimit` as a sticky, tracked action
  with an infinite-limit release idiom; no in-tree configuration targets a
  battery or EV with it outside that managed path. Not a new finding.

## The new defect and its disposition

Red-team pass 6 attacked the band-veto fix's own change and found it
incomplete: the unconditional `controller.max(deadline)` overrode an
external controller's lower target even under `ExternalAuthority` priority,
whose whole contract is that the controller's word stands. Left red on
purpose; fix run v2-fix-94655832 scoped the rule by priority
(`ev/mod.rs:630-638` — verified this session: `(Some, Some)` → `max` under
`DeadlineGuarantee`, `controller` under `ExternalAuthority`). The red team's
test pins the verbatim-setpoint pacing path; the eighth pass's test 15
(`external_authority_target_without_rate_paces_to_controller_destination`)
pins the other branch — BMS pacing with conflicting targets and no rate —
with confirmed revert sensitivity. Verified: both tests pass, plus the red
team's air-cover test
(`snapshot_round_trip_preserves_mid_trip_driving_plan`). Second time in this
initiative the red team caught a fix's own incompleteness; both times the
loop closed it. No residual action.

Red-team pass 6's trace-only items, dispositioned: the stale explicit
`soc_target_max` ceiling still capping an urgent deadline is a defensible
separate contract (a commanded health ceiling outranking a convenience
deadline) — traced, judged, named; the "minimal top-up impossible while a
ready-by is latched" trade is the fix's own documented decision; the
clamped-vs-unclamped band divergence fails in the safe direction with
bounded effect; degenerate direct-arrival sampling requires an incoherent
schedule no fixture or archetype produces. None warrants a test — each is
either a documented contract or an unreachable input, and pinning any would
assert policy, not behaviour.

## The third red-team defect and its disposition

Red-team pass 7 attacked the checkpoint v7 restore path and found that an
empty blob with a *matching* `schema_version` sails through the version
gate and hits `load_state`'s empty-blob early return — a truncated or
corrupt stateful actor blob becomes a silent no-op, `restored_count`
increments, and the resumed run diverges on freshly-constructed decision
state with nothing naming the loss. Left red on purpose; fix run
v2-fix-3da2f572 added the probe rule (`dwelling/mod.rs:4259-4278` —
verified this session: empty blob valid only for an actor that also
*saves* empty, probed against the live actor; stateful+empty → named
rejection; probe failure → named rejection). All three arms pinned: the
red team's `checkpoint_truncated_stateful_blob_must_not_restore_as_silent_success`,
the fix's `checkpoint_stateless_actor_empty_blob_restores_cleanly`, and
the ninth pass's test 16
(`checkpoint_empty_blob_probe_failure_rejected_with_named_error`) for the
probe-failure arm. Verified: all three pass. The note's class measurement
is verified accurate — the silent empty-blob convention exists exactly
once (actor side); every equipment `load_state` routes through the
loudly-rejecting `load_versioned`. Third defect the red team has caught
in this initiative's own fixes; third time the loop closed it.

Red-team pass 7's trace-only items, dispositioned: extra checkpoint
entries silently ignored (unreachable from `save_checkpoint`, which
writes exactly the live actor set; the missing direction *is* caught);
duplicate actor names last-wins (same reachability); a v7-stamped file
with a v6-shaped body (the file lies about its version — the
actionable-error contract only covers honestly-stamped files); composer
min/max folds riding onto a target from another vote (verified this
reassessment, not accepted: `min_soc`/`max_soc` are set only on the
*active discharge* arms of V2G (`v2g.rs:36`, after the stand-down guard
at :27) and V2H (`v2h.rs:47`), with in-tree floors of 0.3 — so in every
built stack the folded floor is ≤ the strategy target and the fold is
harmless; a custom stack setting `min_soc` above a strategy target would
change that, which the claim's own "built stacks" qualifier already
scopes out); idle-shaped votes dropping bounds (verified by the same
read: the stand-down path carries no bounds, so no built idle-shaped
vote exists); the equal-minute anxiety boundary (helper pinned on the
`DepartureDeadline` side, semantics agree); hold-release paths (already
pinned by four implementer equipment tests — duplication, not a gap).
None warrants action; each is unreachable, harmless-by-construction, or
already pinned.

## Findings ledger (carried forward, all dispositioned)

1. **Band veto (red-team).** Fixed and pinned from both directions;
   verified passing in the prior triage. Closed.
2. **`v2g.rs` duplicated dead soc-floor block.** Fixed; verified. Closed.
3. **Hardcoded seed inputs / literal `LowSoc 0.15`.** Operator decided
   (decision-log: proceed); residual in ticket's Known Limitations citing
   ticket 132 (exists in `docs/tickets/`). No coverage action possible —
   a test would assert known-broken behaviour.
4. **`power_limit_kw` / DR-actor × equipment latch.** Documented-and-routed;
   DR sticky-tracking plus infinite-limit release; no reachable in-tree
   unmanaged path; named for the next DR-surface change. The battery's
   `external_power_limit_kw` (above) is the same disposition.
5. **Urgency-gate 1.2× margin.** Soundly unpinned (brittle
   constant-pinning); red team concurred.
6. **Heater/preconditioning ambiguity.** Evidence-based design decision;
   inert at default config.
7. **Mid-trip day-roll freeze (red-team trace).** Superseded by landed,
   tested phase-carrying trip plans
   (`midnight_crossing_trip_continues_into_non_driving_day`, verified
   passing in the prior triage).
8. **Same-state `EvPlugIn{Disconnected}` no-op.** No observable charging
   gap; correctly untested.
9. **Operator-directed driver-response model re-baseline.** Recorded
   contract change with invariants preserved; not a quieted gate.
10. **Battery setpoint mutual-clearing unpinned (second triage's routed
    item).** Closed by seventh-pass tests 13–14, verified above.

Deliberately-not-tested claims from the coverage note, each verified rather
than accepted:

11. **Equipment-level arrival-step energy** — claimed covered by
    composition (test 1's dispatch contract + the seeded multi-day
    integration tests `nightly_start_hour_governs_hourly_charging_profile`
    and `low_soc_gate_delays_charging_beyond_immediate`). Sound: the
    integration tests assert equipment energy per hour over multi-day runs,
    which is exactly what an arrival-step energy test would measure; a
    separate unit test would repeat the composed path. No action.
12. **V2G at floor, actor level** — claimed redundant with test 4 (shared
    `resolve`/`emit_vote` choke point; V2G's only distinct input, price, is
    irrelevant at/below the floor; its floor vote shape is unit-tested in
    `v2g.rs`). Sound per the class-measurement principle; no action.
13. **v3→v4 checkpoint migration** — claimed nonexistent, with rejection
    instead of migration. Verified: `serial.rs:90-100` `load_versioned`
    returns a typed `Err` naming equipment type, expected, and found
    version on mismatch. There is no silent-misload path to pin. No action.
14. **`charging_efficiency.max(0.01)` clamp** — claimed unreachable because
    config validation rejects efficiency outside (0, 1]. Verified:
    `ev/config.rs:353-358` returns a typed error for `eff <= 0.0 || eff >
    1.0 || !is_finite()`. The clamp is defensive-only; a test would pin
    unreachable code. No action.

## Strongest case for action, and the riskiest unaddressed item

**Strongest case for action (updated, fourth triage):** red-team pass 6's
first trace item — a latched explicit `soc_target_max` ceiling still caps an
urgent ready-by deadline (`soc_limit.min(soc_target_max)`, `ev/mod.rs`
~641-644). The honest framing is not "defensible separate contract" (the
easy dismissal) but: this is an **undecided policy silently behaving one
way**. If the intended contract is "an urgent deadline outranks a stale
ceiling," the current code is observably wrong and no test pins either
answer. It is still correctly no-action-now: the behavior is consistent with
the priority contract the fix just pinned (an explicit command — a ceiling
is one — outranks a convenience deadline under both priorities), the band
fix's comment promises only that the *target* cannot veto, and writing a
test would pin a policy nobody has decided — the constitution's route for
that is a recorded decision, not a silent assertion. Named here so the next
change to destination semantics decides it explicitly rather than
inheriting it.

**Riskiest unaddressed item (unchanged):** finding 4 — the DR-actor
`PowerLimit` latch, on EV `power_limit_kw` and battery
`external_power_limit_kw` alike. Strongest case: it is the only
remaining *behavioural* surface in the latch class with no test — if DR
bookkeeping ever fails to send its infinite-limit release (event truncated,
save/restore across an event boundary, a future caller that doesn't know the
release idiom), the equipment stays capped for the rest of the simulation
and nothing fails loudly. It is still correctly dispositioned as
no-action-now: no in-tree configuration produces the unmanaged path (DR
targets exist only via the builder/registry API, verified), the release
idiom is DR's own tested behaviour, and a test would have to construct a
scenario no current code path reaches. But it is named here as the item most
likely to resurface, so the next DR-surface or checkpoint-surface change
re-checks it first.

## Verdict

`coverage_outcome: adequate`. Nine passes have added sixteen verified,
passing tests; all three red-team defects are fixed and pinned on every
arm; full gates re-run this session and match the note exactly
(hares-core 974/0, hares-equipment 2254/0, regression 18/0, clippy
`--all-targets -D warnings` clean, fmt clean); no failing test is carried;
the findings ledger is fully dispositioned. The strongest-case-for-action
item (undecided ceiling-vs-deadline policy) and the riskiest unaddressed
item (DR `PowerLimit` latch) stand as recorded in the previous section.
`failing_tests` empty.
