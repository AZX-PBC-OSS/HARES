---
coverage_outcome: adequate
failing_tests: []
---

# Coverage triage: I-06 round-out re-measurement — coverage is adequate

**Verdict: `adequate`.** The coverage note under triage re-measured the
equipment-identity class on the post-fix tree and concluded no additional
tests are warranted. I verified that conclusion by execution and by code
reading rather than trusting the note: every load-bearing claim I checked
held, the prior round's `defect_found` defect is genuinely fixed, and each
of the note's five "examined and not warranted" adjudications survives the
"what goes wrong if this isn't addressed?" test with the answer "nothing."
`tests_missing` is wrong — the class measurement maps every occurrence to
a named, green behavioral pin. `defect_found` is wrong — the one defect
this class surfaced (last round) is fixed and its red pin is now the
permanent green contract.

## What this triage verified independently (not trusted from the note)

- **The gates, re-run on this tree.** `cargo nextest run --workspace
  --no-fail-fast` → **6943 passed, 12 skipped** — the note's exact
  arithmetic. `cargo clippy --workspace --tests -- -D warnings` → clean.
  `cargo fmt --check --all` → clean.
- **The prior round's defect is fixed, by code reading.** The prior
  triage (the superseded `coverage-note-quality.md`) proved
  `ActorInterest::EquipmentModeChange` never fired: `prev_equipment_modes`
  was rebuilt *after* the end-of-step snapshot, so the comparison read a
  copy of itself. On the current tree (`dwelling/mod.rs:6335-6342`) the
  capture now happens **before** the snapshot population, with a comment
  stating the one-generation-behind contract and why — the exact fix the
  prior triage prescribed ("populate it from the pre-snapshot
  `equipment_core`"). The comparison (`:5091-5096`, both `ByName` and
  `ByEndUse` arms) now reads two distinct generations.
- **The carrier is gone and the contract is green, by execution.** The
  only `should_panic` occurrence in
  `crates/hares-core/tests/interest_filtering_regression.rs` is the doc
  comment (`:434`) recording the removal. `cargo nextest run -p hares-core
  --test interest_filtering_regression` → **10/10 pass**, including both
  new pins (transition/attribution/no-spurious and
  first-observation/`ByEndUse`/unresolved-name).
- **The class measurement's pins exist.** I grep-verified ~25 of the
  note's cited pin names — every one present in the file the note names
  (`equipment_core_has_one_entry_per_equipment`,
  `nightly_strategy_keeps_ev_charged_over_long_horizon`,
  `bms_actor_observes_its_own_battery_through_real_assembly`,
  `actor_rebinds_to_replacement_equipment_after_replace`,
  `user_added_actor_resolves_its_equipment_binding_on_add`,
  `actor_binding_recovers_when_its_equipment_arrives_later`,
  `checkpoint_round_trip_preserves_equipment_identity`,
  `checkpoint_restore_rejects_spec_reorder_with_named_error`,
  `checkpoint_restore_rejects_a_shrunk_equipment_set`,
  `same_config_builds_identical_name_to_id_mapping`,
  `assembly_equipment_ids_are_distinct_and_nonzero`,
  `midrun_equipment_removal_keeps_output_columns_aligned`,
  `midrun_equipment_removal_keeps_actor_columns_attributed`,
  `end_use_aggregates_match_per_equipment_columns_after_midrun_remove`,
  `midrun_added_equipment_contributes_to_its_end_use_aggregate`,
  `added_equipment_gains_port_slot_coverage_before_first_step`,
  `add_equipment_after_stepping_rejects_undeclared_port`,
  `replace_equipment_after_stepping_rejects_undeclared_port`,
  `low_target_strategy_pack_is_raised_to_the_anxiety_band_before_departure`,
  `range_anxiety_must_fire_when_strategy_target_is_not_backed_by_charging`,
  `delegate_equipment_forwards_guard_methods_and_identity_to_inner`).
  All are green in the full-suite run above. The mechanism-surface test
  files all exist (`equipment_identity_config.rs`, `identity_guard.rs`,
  `equipment_id_assignment.rs`, `equipment_identity_misattribution.rs`,
  `equipment_identity_boundaries.rs`,
  `replace_equipment_name_collision.rs`).
- **The review-findings claims.** `build_actor_column_map`
  (`dwelling/mod.rs:621`) exists with exactly two call sites (`:3893`,
  `:4044`) — review finding 3's duplication is extracted as claimed. The
  WI-4 stand-down predicate reads all three dimensions — target, rate,
  and the folded cap (`last_resolved_max_soc`, `composer.rs:46,86,156,190`,
  consumed at `ev_driver/mod.rs:977`) — and the fire-side pin is green.
- **This round changed no files.** `git status --porcelain` shows only
  the fix phase's uncommitted work; nothing new from the coverage round.

## The adjudicated candidates, re-judged

The note examines five candidate gaps and declines each. Applying "what
will go wrong if this isn't addressed?" — and checking the two strongest
arguments against the code rather than the note:

- **`remove_equipment_by_end_use` (no direct test)** — verified a thin
  aggregation over the `evict_actors_targeting` + `refresh_equipment_caches`
  mechanism the removal tests pin end to end. A per-site repeat asserts
  nothing new (the test-quality concern's performative-test shape). No
  action.
- **Grown checkpoint set** — the count guard at `dwelling/mod.rs:4618`
  is `cp.equipment_states.len() != self.equipment.len()` — symmetric, as
  the note claims; the shrunk-set test pins the same check. No action.
- **Replace with the evictee's explicit id (one-step stale core-output
  seed)** — a documented, accepted transient
  (`actor_rebinds_to_replacement_equipment_after_replace`'s own comment
  states and deliberately asserts around it), adjudicated twice now. A
  pin would be red against a settled contract. No action.
- **End-of-step departed-id retain** — the note's unreachability argument
  checks out: `evict_actors_targeting` (`:3085-3088`) retains out every
  actor whose `dispatch_target_name` matches, `clear_equipment` (`:3536`)
  retains out every equipment-targeted actor, and survivors re-resolve
  from the rebuilt name→id map on the same refresh. The stale window is
  unreachable through public behavior; its observable effect (departed
  channels going quiet) is pinned by the removal tests. No action.
- **Cross-process id determinism** — not expressible in one harness, and
  the in-process determinism pin plus BTreeMap-ordered construction
  covers the contract. No action.

## No test added by this triage

The role's instruction is to settle any unresolvable gap by adding a
permanent test. One candidate curiosity arose — whether the fix's
generation semantics are exactly right (capture-before-snapshot yields
prev = t−1, current = t at step t+1's ActorDecide, and the first
post-snapshot step fires on first observation for every equipment).
This was settleable by reading: the ordering is stated in the code
comment, the first-observation firing is the same convention every other
interest uses, and both behaviors are pinned by the two green tests
above. Nothing remained that required a new test to resolve.

## Items assessed, no action needed

- **Artifact-alignment nit (observation, not an action item):**
  `initiative.md`'s Implementation Notes meticulously record every fix
  round but do not mention the `prev_equipment_modes` wiring fix; it is
  recorded in the code comment (self-contained, per the constitution's
  commentary rule), the superseded triage, and the coverage note, which
  explicitly supersedes the stale prior note. The chain is coherent — no
  silent contradiction — so nothing goes wrong; noted so a future reader
  of `initiative.md` alone knows where to look.
- **The prior round's flaky zero-alloc test**
  (`radiant_gain_weight_distribution_zero_allocations`) — passed in this
  round's full no-fail-fast run; pre-existing, environment-sensitive,
  outside this round's scope. It does not change the coverage routing,
  but the reassessment below found the prior triage's tracking
  recommendation was dropped — it is named there as the riskiest
  unaddressed finding, with the concrete follow-up.
- **The stale prior `coverage-note-quality.md` this document replaces**
  routed `defect_found` for its round — correct then (the wiring was
  provably dead), superseded now (the wiring is fixed and its pin green).

## The red-team note (read this pass; my first draft had not)

`red-team-note.md` (round 4, 186 lines, read in full on reassessment)
carries a findings lifecycle: rounds 1–2 fixed and pinned; round 3 clean;
round 4's three findings (replace-entrance name collision, WI-4 `max_soc`
blind spot, telemetry-guard false positive) all recorded fixed at the
cause in `initiative.md` (fix-phase-9) with named pins — and all three
previously-red tests are green in the full-suite run above. Its
"Attacked and traced — no defect, no test" section lists five items, each
with a verifiable outcome; I re-judged the one with any residual:

- **Classifier/reader disagreement on integral floats** — the io
  classifier marks `7.0` Malformed while the constructor reader accepts
  it as 7, so the build silently succeeds where the pass's contract
  comment implies loud rejection. The red team adjudicated it benign
  (the id delivered equals the numerically intended id; no misbinding is
  reachable; typed payloads fail loudly), leaving a doc-comment
  inaccuracy as the whole residual. Re-judged: correct. What goes wrong
  if ignored: a reader of the comment mis-predicts behavior for a
  config shape no user can reach with intent (explicit ids arrive only
  through typed specs). Cosmetic, not coverage. No action.
- The other four (explicit-id collision → loud assembly rejection;
  `u32::MAX` pre-pass boundary → loud rejection with the dwelling-side
  boundary pinned; counter burning an id on a rejected add → sparse ids
  are documented; override truncating composer dispatch → documented
  path exclusivity) are adjudications of correct behavior, each
  consistent with code I read for other reasons this session. No action.

## Reassessment (overreach / shortfall / abdication)

**Overreach — none, verified.** `git status --porcelain` shows 60
entries, identical to arrival state; I wrote no test, applied no fix,
and edited no source file this session (my only writes were this triage
document). Unlike the prior triage, I did not even need a carrier-removal
probe — the carrier was already gone; execution alone settled the
defect-fixed question.

**Shortfall — two found in my first draft, both closed before this
filing.** (1) I had assessed the red-team passes on the coverage note's
summary and the green suite without reading `red-team-note.md`; it is
now read in full and assessed above — no conceded-but-unpinned defect
hides in it. (2) I had asserted the `ByEndUse` arm of
`equipment_mode_changed` was "covered by the same root-cause pin" on a
partial reading; I have now read the full arm (`:5098-5119`): its
`current`/`prev` derivation and `!=` comparison are textually identical
to the `ByName` arm the transition test pins, and the arm's own
machinery (the end-use loop, the name→id resolution, the early return)
is exercised by the first-observation pin. A `ByEndUse` transition test
would re-assert the same comparison through a second door — the
per-site-repeat shape the test-quality concern forbids. The claim
stands, now on a full reading.

**Every finding assessed, with its reason:** the coverage note's five
adjudicated candidates (above, each re-judged); the review's three
findings (all three fixes verified in code — the three-dimension
stand-down predicate, the `duct_losses` global-column resolution, the
`build_actor_column_map` extraction); the red team's three round-4
findings plus its five attacked-and-dropped items (above); the prior
triage's two carryovers (below). Nothing was dismissed without a named
ground.

**Abdication — the strongest case for action, and the riskiest
unaddressed finding, are the same item: the flaky zero-allocation test.**
`radiant_gain_weight_distribution_zero_allocations` failed once under
parallel load two rounds ago, and the prior triage explicitly recommended
"the same tracking-ticket treatment as the wiring defect rather than
silent dismissal." Reassessment finding: **that recommendation was
dropped** — the test appears in `initiative.md` (as a one-line parenthesis
in a verification footnote) and in the two triage documents, but in no
ticket, no initiative, and no tracked list anywhere under `.shipwright/`.
What could go wrong if this stays ignored: a test that fails
intermittently under the workspace's standard gate teaches every future
agent that a red run can be re-rolled, and the cheapest way to silence it
is the constitution's forbidden non-fix — widening the zero-allocation
assertion — which would convert a hot-path-discipline guard into a
permanent blind spot over per-timestep allocation regressions. **The
concrete follow-up (for the flow owner or next initiative-planning step,
not this schema): create a tracking ticket to either deflake the test
(isolate the global-allocator measurement from parallel binaries) or
document it as a known-flaky with a tracked reason — before someone's
"fix" is to weaken the assertion.** This does not change the routing:
`coverage_outcome` routes this round's coverage of the I-06 class, the
flake is pre-existing and outside that class, and it passed in this
round's full no-fail-fast run. But it is named here, with the specific
work, because the prior naming was allowed to evaporate.

The strongest case for `defect_found` — that the first-observation
firing semantics might be an unwanted spurious-wakeup behavior — was
settled by reading: it is the same first-comparison convention every
other interest filter uses, it is documented in the fix comment, and it
is pinned by a green test that must survive any future change to the
wiring. Not a defect. The strongest case for `tests_missing` — that one
of the five declined candidates is a rationalized-away gap — fails
because each declination names an already-pinned mechanism and I verified
the two load-bearing ones against the code. `adequate` is the routing.
