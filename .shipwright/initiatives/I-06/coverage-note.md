---
coverage_note_outcome: written
---

# Coverage round-out: I-06 equipment identity (gh #26/#27) — re-measurement on the post-fix tree

This note supersedes the prior `coverage-note.md` (2026-09-23 12:42), which is
now stale in one material respect: it documents the `EquipmentModeChange`
dead-wiring defect as red-pinned behind a `#[should_panic]` carrier. A later
fix phase fixed the wiring (`prev_equipment_modes` is captured from the
pre-snapshot `equipment_core`, one generation behind the map the comparison
reads), removed the carrier, and left the test as the permanent green
contract — verified on the current tree: the only `should_panic` occurrence
in `interest_filtering_regression.rs` is the doc comment recording the
removal, and the test passes in the full suite.

## Verdict: no additional tests warranted

This round measured the class fresh, on the current tree, and found every
occurrence of the defect shape already carries a behavioral pin through a
real path. The changed surface is the most thoroughly attacked part of the
codebase — it has survived the fix phase's own coverage plan (fix-plan
coverage items 1–12), two prior coverage rounds, four red-team passes, and a
review — and each of those rounds' findings is fixed and pinned green on
this tree (verified by execution, not by trusting the artifacts: the three
review findings and the red-team round-4 findings' previously-red tests all
pass; the WI-4 stand-down predicate now checks all three dimensions —
target, rate, cap — and `range_anxiety_must_fire_when_strategy_target_is_
not_backed_by_charging` is green; the `record_step` verbosity ≥ 5 panic is
closed — `duct_losses` now resolves for schema-unknown equipment because it
is a global aggregate column; the duplicated `actor_column_map` derivation
is extracted into `build_actor_column_map` with two call sites).

Adding tests here would duplicate assertions already covered by broader
integration tests — the exact performative-test shape the test-quality
concern forbids. What follows is the measurement that justifies that claim.

## Class measurement

The shape of the proven defect: **an `EquipmentId`-keyed consumer mechanism
wired into the real dwelling loop**, plus the identity mechanisms at the
equipment entrances that feed them. Searched by grepping `equipment_core`,
`equipment_id_by_name`, `prev_equipment_modes`, `EquipmentId(` consumers,
and the entrance functions (`add_equipment`, `replace_equipment`,
`remove_equipment*`, `clear_equipment`) across `crates/*/src`. Ten consumer
occurrences and seven mechanism surfaces; each with its pin:

| # | Occurrence | Behavioral pin (all green in the suite) |
|---|---|---|
| 1 | `equipment_core` end-of-step snapshot | `equipment_core_has_one_entry_per_equipment`; `midrun_added_equipment_is_observable_before_its_first_step` |
| 2 | `equipment_id_by_name` | `assembly_equipment_ids_are_distinct_and_nonzero`; `same_config_builds_identical_name_to_id_mapping` |
| 3 | `EvDriverActor` observation | `nightly_strategy_keeps_ev_charged_over_long_horizon` + `_on_a_second_fixture` (45-day, real assembly, both fixtures) |
| 4 | `BatteryManagementActor::read_soc` | `bms_actor_observes_its_own_battery_through_real_assembly` |
| 5 | Actor equipment bindings (`Actor::resolve_equipment_id` on refresh) | `actor_rebinds_to_replacement_equipment_after_replace`; `user_added_actor_resolves_its_equipment_binding_on_add`; `actor_binding_recovers_when_its_equipment_arrives_later` |
| 6 | `prev_equipment_modes` → `equipment_mode_changed` interest filter | `equipment_mode_change_interest_wakes_the_changed_equipments_watcher` (transition, attribution, no-spurious); `mode_change_interest_wakes_on_first_observation_of_midrun_added_equipment` (first-observation, `ByEndUse`, unresolved-name arms) |
| 7 | Checkpoint restore (identity-keyed, format v8) | `checkpoint_round_trip_preserves_equipment_identity`; `checkpoint_restore_rejects_spec_reorder_with_named_error`; `checkpoint_restore_rejects_a_shrunk_equipment_set` |
| 8 | Output column maps / end-use aggregates / actor columns | `equipment_identity_misattribution.rs`; `midrun_equipment_removal_keeps_output_columns_aligned`; `midrun_equipment_removal_keeps_actor_columns_attributed`; `end_use_aggregates_match_per_equipment_columns_after_midrun_remove`; `midrun_added_equipment_contributes_to_its_end_use_aggregate`; high-verbosity variants |
| 9 | Port slots for post-construction equipment | `added_equipment_gains_port_slot_coverage_before_first_step`; `add_equipment_after_stepping_rejects_undeclared_port`; `replace_equipment_after_stepping_rejects_undeclared_port`; unknown-zone rejections at both entrances |
| 10 | Active-id retain (departed ids dropped) | pinned observably by occurrence 8's removal tests (the evicted equipment's channels go quiet) |

Mechanism surfaces: assembly injection + validation (duplicate ids, id 0,
malformed from raw/typed/null payloads and all three override sources —
`equipment_id_regressions.rs` + `equipment_identity_boundaries.rs` +
`equipment_id_assignment.rs`); the tri-state id reader and
preserve-when-absent init sites (`equipment_identity_config.rs`); entrance-2
auto-assignment with the never-reused counter, explicit-collision
rejection, landed-write postcondition, `u32::MAX` boundary and exhaustion
(`equipment_identity_boundaries.rs`, `equipment_id_regressions.rs`); the
replace entrance's full contract including name collision
(`replace_equipment_name_collision.rs`); the setter population contract over
all 63 canonical names, the guard-engaged error path, and the
`delegate_equipment!` forwarding
(`equipment_identity_config.rs` + `delegate_equipment_forwards_guard_
methods_and_identity_to_inner` in `hares-equipment/src/lib.rs`); the static
call-site guard (`identity_guard.rs`, with its own non-vacuity proof). The
WI-4 precedence predicate is pinned on all six arms — stand-down and fire,
for each of target / rate / cap (`ev_driver/mod.rs` tests at :4288, :4331,
:4385, :4454, :4512, :4561) plus the dwelling-level raise side
(`low_target_strategy_pack_is_raised_to_the_anxiety_band_before_departure`).

## Error paths, boundaries, concurrency

- **Error paths**: id 0 and duplicates at assembly; malformed ids from raw,
  typed, and `null` payloads; `equipment_id` in user overrides rejected from
  all three sources; explicit collisions at add and replace; name collisions
  at add, assembly, and replace; sabotaged setter body; guard-engaged setter;
  checkpoint reorder and shrunk set; unknown-zone and undeclared-port
  declarations at both post-construction entrances; non-critical init
  failure skip. All pinned on returned typed `Err`s or observable state.
- **Boundary values**: `u32::MAX` in-use explicit id (no reissue) and counter
  exhaustion (loud error); the id-0 sentinel; the 45-day horizon (load
  bearing per the RCA); the second-fixture climate constraint documented
  in-test (bldg0000006 — cold-pack derate on bldg0000002 is the thermal
  model working).
- **Concurrency**: no new surface. Dwellings step single-threaded; the
  static guard covers the cross-crate source surface; the process-wide
  `RangeViolationEvent` ring is `#[cfg(observe)]` diagnostics, conceded in
  the review.

## Candidates examined this round and adjudicated as not warranting tests

- **`remove_equipment_by_end_use`** (no direct test): it is a thin
  aggregation over the same `evict_actors_targeting` +
  `refresh_equipment_caches` mechanism that `remove_equipment` exercises,
  and that mechanism is pinned end-to-end by the removal tests above. One
  pin on the shared mechanism is the correct shape; a per-site repeat would
  assert nothing new.
- **Checkpoint restore with a grown equipment set**: the count-mismatch
  check the shrunk-set test pins is symmetric — a dwelling larger than the
  checkpoint fails the same guard. Already covered.
- **Replace with an explicit id equal to the evictee's** (one-step stale
  core-output seed via `or_insert_with`, `dwelling/mod.rs:3785`): the review
  examined exactly this path and adjudicated it a minor observation, not a
  finding — auto-assignment always mints a fresh id, only a contrived
  explicit reuse reaches it, and the staleness lasts one step before the
  end-of-step snapshot overwrites it. This round re-read the code *and*
  traced the surrounding mechanism end to end, and concurs on stronger
  grounds than the review's: the transient is **documented accepted design**
  — `actor_rebinds_to_replacement_equipment_after_replace`'s own comment
  states "the first post-replace step can still see the evictee's snapshot
  entry before the end-of-step retain drops its id" and deliberately
  asserts the sustained final-row value instead. A test pinning the joining
  step would be red against a deliberate, in-suite-documented contract —
  re-litigating a settled adjudication, not covering a gap.
- **The end-of-step departed-id retain** (`dwelling/mod.rs`, the retain that
  drops ids which have left the vector): traced this round — after a
  removal, the departed id's `equipment_core` entry survives until the next
  step's end, but **no actor can be bound to it**: eviction is name-based
  and comprehensive (`evict_actors_targeting` retains out every actor whose
  `dispatch_target_name` matches, and `clear_equipment` retains out every
  equipment-targeted actor), and every surviving actor's binding is
  re-resolved from the rebuilt name→id map on the same refresh. The stale
  window is therefore unreachable through public behavior; pinning the
  retain itself would require reaching into `latest_env` internals — an
  implementation-detail test the constitution's testing rules forbid. The
  retain's observable effect (departed equipment's channels going quiet) is
  already pinned by the removal tests in occurrence 8.
- **Cross-process id determinism**: ids derive from spec order (BTreeMap
  iteration), so the in-process `same_config_builds_identical_name_to_id_
  mapping` pin plus the construction-order guarantee covers the contract; a
  cross-process test is not expressible in one harness and would assert
  nothing the ordering construction doesn't already guarantee.

## Gates (run this round, on arrival state — this round changed no files)

- `cargo nextest run --workspace --no-fail-fast` → **6943 passed, 12
  skipped** (includes every pin listed above).
- `cargo clippy --workspace --tests -- -D warnings` → clean.
- `cargo fmt --check --all` → clean.
- `git status --porcelain` → 60 entries, all the fix phase's uncommitted
  work; this round added no files and edited no source.

## Post-writing reflection

Completeness: error paths, boundary values, and concurrency audited above,
not just the obvious variation; the four adjudicated candidates are recorded
with grounds rather than silently dropped. Test quality: no tests were
forced — the note's claim is that the existing ~70 pins across nine test
files cover the class, and each cited pin was verified present and green by
execution (the full-suite run) and by reading its assertions. Honesty: the
stale prior note is superseded explicitly rather than left to contradict
this one. No gaps remained after the pre-writing reflection, and re-reading
this note end-to-end found nothing to fix.
