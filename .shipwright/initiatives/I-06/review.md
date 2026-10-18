---
review_outcome: clean_with_nits_applied
---

# Review round 2: I-06 — EV Nightly charging stops after the first week (gh #26/#27)

This is the second review pass. The first pass's three findings (WI-4 stand-down
reading only the resolved target; `record_step` panicking on a mid-run add at
verbosity ≥ 5; the duplicated actor-column-map derivation plus the frozen-branch
`end_use_aggregate_indices` desync it surfaced) are all fixed at the cause — I
verified each fix in source, not just in the notes. The five red-team rounds and
the fix-done interrogation round that followed are likewise landed and pinned. I
am not re-opening any of them.

This pass found **one new defect**, proven with a failing test, and — per the
operator's instruction this round ("failing tests aren't warnings, just fix it")
— **fixed it at the cause** rather than bouncing the ticket. The fix is small,
the failing test stays in the suite as the permanent pin (now green), and the
full gate set was re-run on the final tree. Details below. Everything else I
attacked held; the attacks are listed under `## Verified`.

The verdict is `clean_with_nits_applied` because nothing remains for the
implementer: the one finding this pass produced is fixed, pinned, and
gate-verified in this same review session. The finding itself was Warning-grade
(a loud regression on a public Rust API flow, not silent corruption) and is
recorded in full so the trail is honest about what the review caught.

## Finding (fixed in review): re-registering a removed equipment tripped the identity guard

**Where:** `assign_equipment_identity` in `crates/hares-core/src/dwelling/mod.rs`
(the shared entrance logic of `add_equipment` / `replace_equipment`).

**What was wrong.** The entrance called `Equipment::set_equipment_id`
unconditionally — including on the explicit-id path, where the collision check
has already passed and `assigned == requested`, so the write is a no-op by
construction. But `add_equipment` marks every registered equipment
`mark_initialized()` before the push (`dwelling/mod.rs:3429`), and
`remove_equipment` returns the box *without clearing that mark*. Re-adding the
box — a new registration of equipment that is in no dwelling at the moment of
the write — therefore hit `apply_identity_write`'s initialized-guard and failed
loudly:

```
Equipment("equipment 'Probe': identity assignment rejected: invalid state:
equipment 'Probe' is already initialized; its id cannot change")
```

The guard's invariant is "no identity mutation while registered"; the stale mark
made it read "ever registered". Concrete consequence: the public Rust
remove → re-add flow was broken for exactly the guard-tracking types (EV,
Battery — the two types with real `is_initialized` overrides) and for any
assembly-built equipment that had undergone a Dwelling-mediated LUT set (which
marks it). The error text also misattributed the cause to the equipment's
`set_equipment_id` implementation. Python could not reach it
(`PyDwelling::remove_equipment` discards the box, `py_dwelling.rs:1295-1297`),
which is why five rounds of gates didn't see it.

**Proof.** `readding_a_removed_guard_tracking_equipment_preserves_its_identity`
in the new `crates/hares-core/tests/equipment_readd_after_removal.rs` — a
guard-tracking probe equipment added (auto-assigned, marked), removed, and
re-added unchanged. Pre-fix: **FAILED** with the error above (exit 101).
Post-fix: green.

**Fix applied.** In `assign_equipment_identity`, skip the setter call when it
would not change the id (`eq.descriptor().id != assigned` gate). On the explicit
path the descriptor already holds the collision-checked value, so there is
nothing to write and the postcondition trivially holds; on the auto-assign path
(`requested == 0`) the write is real, still guard-checked, and still
postcondition-verified — the "no-op setter body fails registration loudly" arm
pinned by `equipment_identity_boundaries.rs` is unaffected (it exercises the
auto-assign path, where the write still happens). The `replace_equipment`
entrance inherits the fix through the shared helper. The entrance's doc comment
records the reasoning.

**Verification of the fix:** the proving test went red → green;
`cargo nextest run --workspace --no-fail-fast` → **6946 run, 6946 passed,
12 skipped** (the 6945-test baseline plus the new pin — every pre-existing test,
including all identity-boundary and postcondition pins, still green);
`cargo clippy --workspace --all-targets -- -D warnings` clean;
`cargo fmt --check --all` clean. The identity-call-site guard
(`identity_guard.rs`) still passes — the edit adds no call site outside the
authorized file.

## Prior-round dispositions (all verified in source, none re-opened)

- **Review round 1, Finding 1 (WI-4 stand-down)** — fixed correctly:
  `range_anxiety_override_request` (`ev_driver/mod.rs:969-983`) now requires the
  resolved target ≥ band AND the resolved rate absent-or-positive AND the
  resolved `max_soc` cap (defaulting to the target) ≥ band. All three
  `ChargingComposer::evaluate` exits set all three `last_resolved_*` fields
  (`composer.rs:154-156, 178-180, 188-190`), so no stale-field read is possible;
  NaN inputs fail every clause toward the fire side (fail-safe). The reviewer's
  red test and the new boundary pins are in the tree and green.
- **Review round 1, Finding 2 (v≥5 duct-losses panic)** — fixed correctly:
  `duct_losses` resolves via `resolve_col(HVAC_DUCT_LOSSES_COL, …)` ungated on
  schema membership (`dwelling/mod.rs:522-524`), and the schema emits that
  column unconditionally at verbosity ≥ 5 (`columns.rs:605-610`), so the
  ungated demand in `record_step` (`dwelling/mod.rs:6684`) is always
  satisfiable. The reviewer's red test is green.
- **Review round 1, Findings 3–4 (map-derivation duplication / frozen-branch
  end-use desync)** — fixed correctly: `build_actor_column_map`,
  `equipment_descriptor_specs`, and `build_end_use_aggregate_indices`
  (`dwelling/mod.rs:570-641`) are single homes called from assembly, the
  fresh-schema branch, and the frozen-schema branch (`mod.rs:4047-4072`).
- **Red-team rounds 1–5 and the fix-done interrogation** — spot-verified the
  riskiest: the evictee-exemption predicate is `replace_name.is_none_or(|evictee|
  name != evictee)` (`mod.rs:3503`) — add path exempts nobody, replace path
  exempts exactly the evictee; the round-5 red test is green. The
  `is_none_or(|power| power > 0.0)` rate boundary has a dedicated zero-hold pin.
- **Round-1 review's acknowledged limitations and observations** (warm-climate
  second fixture; process-wide `RangeViolationEvent` id scope; the
  replace-with-explicit-evictee-id one-step stale core read; `apply_overrides`
  called only with the empty object) — conceded again, unchanged.

## Attacks run this pass that did NOT produce findings

- **Stand-down ordering vs. the urgency gate.** The stand-down is checked before
  the urgency gate, so a band-covering plan with a *slow but positive* rate
  (SolarSurplus at partial surplus) stands the override down even when the plan
  cannot reach the band before departure. Killed: this is not a regression —
  pre-WI-4 the urgency gate made the identical max-rate optimism
  (`needed_charge_hours_to_target` uses `ctx.max_charge_kw`), so the same step
  stood down the same way; and every rate-bearing production stack carries
  `DepartureDeadline`, whose own constraint arms (`departure.rs:71-104`) fire a
  full-rate override when time actually runs short, making the resolved plan
  legitimately band-covering. `build_preferences` (`ev_driver/mod.rs:129`)
  is an exhaustive match over 8 strategy variants with no custom-preference
  path into the actor's composer, so no production stack lacks that backstop.
- **End-use aggregate survival under the reserved-namespace filter.** An
  equipment named "Total" loses its per-equipment columns but must still count
  toward its end-use aggregate. Verified: `build_schema` emits aggregates from
  the unfiltered `equipment_list` (`columns.rs:495-506`), and
  `build_end_use_aggregate_indices` maps the live vector, so the aggregate is
  emitted and populated. Pinned end-to-end by
  `equipment_named_total_at_build_time_never_silences_the_aggregate_total`.
- **Nested `equipment_id` override bypass.** The reserved-key check
  (`conversions.rs:857-878`) covers the three override sources' top-level keys;
  a nested `{"common": {"equipment_id": …}}` cannot reach identity because every
  reader (`equipment_id_from_config`, `explicit_equipment_id`) reads only the
  top-level key of the merged/typed maps. No bypass.
- **Parameters-mirror vs. typed-payload divergence in the io classifier** —
  red-team round 5 item 6 already established the two channels are written
  together (`set_spec_equipment_id`) or derived from the same typed data at
  parse; a divergence would fail loudly at assembly anyway. Conceded.
- **`assign_equipment_ids` saturating counter vs. the dwelling's checked
  counter** — the io pass's `saturating_add` can pin `next_id` at `u32::MAX`
  behind an explicit `u32::MAX`, handing a later absent spec the same id — but
  the assembly duplicate check (`mod.rs:2391-2397`) rejects that build loudly
  with both names. Loud, not silent; not a finding.
- **Hot-path cost of the new membership rule** — `is_schema_known_equipment`'s
  `format!` runs per-equipment per-step only inside the
  `debug_assertions`/`check_invariants` invariant block of `record_step`;
  release hot path reads pre-resolved indices only. No new per-timestep
  allocation in release.
- **Checkpoint v8 restore** — name-keyed map + id validation + symmetric count
  check (`mod.rs:4632-4683`); a duplicate name in the checkpoint collapses the
  map but then some live equipment's name is missing → loud error. Pinned by
  `checkpoint_restore_rejects_spec_reorder_with_named_error` /
  `checkpoint_restore_rejects_a_shrunk_equipment_set`.
- **The syn-based telemetry guard** — detection (`receiver_is_telemetry_channel`)
  and the three `// allowed:` positions are pinned by the vacuity test
  `telemetry_read_guard_flags_and_excludes_by_syntax`; the five newly contracted
  production reads each name a real constraint (verified: `CoreState` has no
  defrost/tank-temperature/capacity field; away-charge power is off-balance-sheet
  by design). `syn`/`proc-macro2` landed as **dev-dependencies** of hares-core
  (`Cargo.toml:47-48`), not production deps.

## Observations (no action required)

- `flow.yaml` still records-but-does-not-encode the python gate for work beneath
  `hares-python` — a CI-policy decision the decision log assigns to the flow
  owner. Process item, not code; noted so it stays visible.
- My fix makes the explicit-path `set_equipment_id` call skippable rather than
  required; if a future entrance *needs* the write on that path (e.g. a setter
  with side effects), the postcondition alone will not catch a skipped side
  effect. No such implementation exists or should — the setter's contract is a
  pure descriptor stamp (`lib.rs:196-215`).
- `assign_equipment_identity`'s skipped-write condition and its postcondition
  are now the same predicate; the postcondition retains force only on the
  auto-assign path, which is exactly the path whose write can fail to land.

## Known Limitations — disposition

Both entries were checked and are conceded: the superseded diagnostic harness
`i06_ev_nightly_diag.rs` is genuinely absent from the tree (`ls
crates/hares-core/tests` — five other `*_diag.rs` files remain, none for I-06),
and the I-07/I-08 stubs are gone (`ls .shipwright/initiatives` → I-01..I-06
only). The warm-climate second-fixture entry names a real physical constraint
(the cold-pack charge derate legitimately blocks January charging on
bldg0000002); the identity mechanism is pinned fixture-independently by the
structural gates, and the behavioral breadth test runs on bldg0000006 with the
constraint documented in the test.

## Verified

- Full suite on the final tree (with my fix and pin): `cargo nextest run
  --workspace --no-fail-fast` → **6946 run, 6946 passed, 12 skipped** (pre-fix
  baseline this session: 6945 passed, same skips).
- The proving test: `cargo test -p hares-core --test
  equipment_readd_after_removal` → **FAILED pre-fix** ("identity assignment
  rejected: invalid state: equipment 'Probe' is already initialized"), **ok
  post-fix**. Left in the suite as the permanent pin.
- Identity-focused files re-run post-fix: `cargo nextest run -p hares-core
  --test equipment_id_regressions --test equipment_identity_boundaries --test
  equipment_identity_misattribution --test replace_equipment_name_collision
  --test add_equipment_empty_name_exemption --test equipment_readd_after_removal`
  → 52 passed, 0 failed — the postcondition, collision, counter-boundary, and
  aggregate-namespace pins all survive the skipped-write change.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean, exit 0
  (post-fix). `cargo fmt --check --all` → clean (post-fix).
- Reproduction gates green on this tree, run by name with the fix plan's
  verbatim commands: `cargo test -p hares-core --test equipment_id_regressions
  equipment_core_has_one_entry_per_equipment` → ok (0.18 s);
  `... nightly_strategy_keeps_ev_charged_over_long_horizon` → ok (1.63 s, the
  45-day ticket scenario asserting ≥30 charge-days and zero cancellations).
  The pre-fix red is not re-derivable in this session — re-running it would
  require reverting the working tree, which the operating rules forbid; it is
  recorded verbatim in `reproduction.md` (exit 101, "12 equipment collapsed
  into 1 entries", "got 7 charge-days") and the RCA's diagnostic run.
- WI-4 predicate read at `crates/hares-core/src/actors/ev_driver/mod.rs:969-983`;
  all three `ChargingComposer::evaluate` exits set all three resolved fields at
  `composer.rs:154-156, 178-180, 188-190`.
- `HVAC Duct Losses (W)` emission is unconditional at verbosity ≥ 5:
  `crates/hares-io/src/output/columns.rs:605-610`; the dwelling demand is
  `record_step` `dwelling/mod.rs:6684`; the map resolution is ungated at
  `dwelling/mod.rs:522-524`.
- End-use aggregates are emitted from the unfiltered spec list:
  `columns.rs:495-506`; the dwelling-side index derives from the live equipment
  vector: `dwelling/mod.rs:599-618` with callers at assembly (`2534`),
  fresh branch (`3917`), frozen branch (`4068`).
- `remove_equipment` returns the box without clearing the initialized mark
  (`dwelling/mod.rs:3576-3590` — body contains no `unmark_initialized`); the
  mark is applied at `dwelling/mod.rs:3429` (add) and `3664` (replace);
  assembly-built equipment is never marked (no `mark_initialized` call in
  `build_from_blueprint`) — which is why the defect only bit
  entrance-registered equipment.
- Python cannot reach the defect: `PyDwelling::remove_equipment` returns `()`
  and drops the box (`py_dwelling.rs:1295-1297`); every Python `add_*`
  constructs fresh equipment. The recorded Python-gate run (1172 passed) was
  not re-run — my change is Rust-only, on a path Python cannot reach, and is
  behaviorally identical for fresh equipment (the auto-assign write still
  happens whenever the id changes).
- Known-limitation file checks: `ls crates/hares-core/tests | grep i06` → no
  match; `ls .shipwright/initiatives` → I-01..I-06 only.
- `syn`/`proc-macro2` are `[dev-dependencies]` in `crates/hares-core/Cargo.toml:47-48`
  (section header at line 39), not production dependencies.
