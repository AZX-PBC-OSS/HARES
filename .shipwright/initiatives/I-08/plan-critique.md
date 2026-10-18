# I-08 plan verification record

`plan.md` and `ledger/` are ready to execute only when one full verification
round finds zero blockers and zero majors. Each round uses reviewers who did
not write the plan and have not seen earlier rounds' conclusions: code
verification per area, traceability against every source, and a red team.

## Current status: implementation proceeding; round L5 in progress; per-commit audits recorded below

## Round L1 (ledger, first verification)

| Scope | Blockers | Majors | Minors |
|---|---|---|---|
| plan.md 1-3, baseline repair, measurement, profiler | pending | pending | pending |
| output reporting, Python surface | 2 | 16 | 12 |
| fleet, runtime and memory, slop cleanup, other defects | 0 | 14 | 8 |
| consumer, numeric fixes, schedule | 4 | 15 | 17 |
| traceability (decisions, rules, context items, defects, requirements) | 17 | 26 | 13 |

Main themes to resolve:

- Decided capabilities without an entry: package rename (hares-py, hares,
  ~/.cache/hares), validate(), content inputs, input schema, replay export,
  metrics_from_table, resample_weather, reporting frequency, the callback
  destination and its Python surface, HealthPolicy, the Python RunResult
  surface.
- Decisions contradicted: weather lag declared correct instead of fixed;
  numeric changes folded into structural commits before the consumer
  migration; invariant checks still stubs in release builds; process-global
  counters kept; reward function changed instead of honoured; a twin config
  type kept; ignored tests kept.
- Premises corrected by verification: the telemetry clone site
  (`mod.rs:6431-6436`), the dead coupled-LU path, blueprint `from_parts`
  running once, the fleet status and alignment behaviour at HEAD, existing
  sort and multi-depth tests for the ground network.
- Verification mechanics: the verification block is red for a span of
  entries, the pytest line cannot run, the stray-file check is missing, some
  tests depend on timing or RSS.
- Slop census incomplete: 204 T-NNNN ticket IDs in 55 files and other
  reviewer references.

Detailed findings with evidence and corrections are in
`verification/verify-L1-a.md` to `verification/verify-L1-e.md`; every one is
resolved in the ledger before the next round.

## Re-review of the ledger form (2026-09-28, two independent verifiers)

Both verifiers walked the ledger end to end: ~95 file:line citations spot-checked with zero
material drift; the block commands validated against installed tooling; the vendored
EnergyPlus source confirmed to support B-1a/B-1b/B-1c/B-2 (weights, solar arithmetic, row
counts recomputed); the capability map (prior design → ledger entry) judged better or
equal-with-mechanics in every row, rotation a deliberate documented repudiation; the
deliberately-not-built items (§2.10) right-defers with real seams; HARES#28 solved with no
unaccounted per-run write (all three non-test writes at HEAD owned: OR-3, OR-4, OR-9); every
V1-V50 defect and every review item mapped to an entry or a reasoned repudiation.

**Verdicts:** verifier 1: ACTIONABLE, conditional on one major; verifier 2: READY-WITH-FIXES,
no blockers. Converged findings, ALL APPLIED to the ledger before execution:

- **The pin provenance (the one major):** `0ec757cb` exists only as the rebased PR #27 branch
  in the consumer's nested engine clone, not on HARES origin; B0's tree comparison now names
  the fetch-or-push rule (`ledger/08-consumer.md`, preamble + B0).
- **CN-1's census holes:** `suite_coverage.py:149-165` (the kwargs-list and rotation pins) +
  the stale `Dwelling.metrics()` doc text (`p4_data_quality.py:13`, `adapter.py` docstring)
  added; census grep extended (`resample_overrides`, `Dwelling\.metrics\(\)`, `\.results\(\)`,
  `finalize_billing`); the new acceptance test `removed_kwargs_leave_the_surface`.
- **Spec wording:** OR-1's unit census (no `"ordinal {ordinal}"`; `""` → `Unit::None`); OR-5's
  `WarmupResiduals` = the worst zone's four quantities (matches §2.3's cardinality); FL-1's
  fold rule pinned bug-compatible until FL-2; §2.4 names `FleetReport` as the fleet
  `HealthPolicy` carrier; §2.2's `Aggregation` notes Min/Max/Last were never in the binding
  decision; the RowPlan freeze-once rule stated (equipment mutation after freeze =
  `InvariantViolation`); CN-1's `health_<field>` recording fixed (`reduce_summary` + the
  `WarmupOutcome` JSON form, not `provenance()`); decision-log's `engine_performance` states
  the coupled factorization is REMOVED as dead code (SL-7), not reused; schedule contingency
  named in the `[writing]`-matchable form (`B-0`); citation nits (`.gitignore:50`,
  `metrics.rs:230`, Deref `245-250`).

Residual risk, stated: the verification block's first full run happens at M-0 on a tree the
verifiers did not build (research-only mandate); M-0 item 1 is the ledger's catch-all if any
block line fails without a covering entry.

## Round L2 (fresh verification of the ledger, 2026-09-30)

Five reviewers who had not written or seen the ledger's earlier reviews checked
it against the code and every source.

| Area | Blockers | Majors | Minors |
|---|---|---|---|
| plan.md 1, 3, 5; baseline repair; measurement; profiler | 2 | 10 | 22 |
| plan.md 2; output reporting; Python surface; inputs | 0 | 8 | 14 |
| fleet; runtime and memory; slop cleanup; other defects | 1 | 7 | 16 |
| consumer; numeric fixes; schedule | 0 | 6 | 16 |
| traceability | 3 | 13 | 30 |

Status: the opening sequence (Task 0 through M-3) is executing. Rows 14 onward
are NOT READY: every L2 finding is being fixed in the ledger before the
implementer reaches the affected row, and findings that still hold against
already committed rows become follow-up entries scheduled right after M-3.
Reports: `verification/verify-L2-a.md` to `verify-L2-e.md`; resolutions:
`verification/L2-resolution-a.md` to `-d.md`. A fresh round re-checks rows 14
onward after the fixes land.

## Round L3 (fresh verification of rows not yet executed)

| Area | Blockers | Majors | Minors |
|---|---|---|---|
| plan.md 1, 3, 5; follow-ups BR-10..15, CN-17; profiler; schedule | 2 | 3 | 14 |
| plan.md 2; output reporting; Python surface; inputs | 0 | 14 | 17 |
| fleet; runtime and memory; slop cleanup; other defects | 0 | 9 | 11 |
| consumer; numeric fixes | 4 | 31 | 22 |
| traceability | 3 | 9 | 17 |

Execution ran ahead of this round: PR-1 and PR-2 were committed and PR-3
started before CN-17, BR-10 and BR-12 to BR-15. The decision log records the
executed order and the required next rows. Every L3 finding is being fixed;
resolutions go to `verification/L3-resolution-*.md`, then a fresh round
re-checks.

## Round L4 (fresh verification of rows not yet executed)

| Area | Blockers | Majors | Minors |
|---|---|---|---|
| plan.md 1, 3, 5; follow-ups; profiler; schedule | 1 | 5 | 9 |
| plan.md 2; output reporting (incl. OR-11); Python surface; inputs | 0 | 11 | 21 |
| fleet; runtime and memory; slop cleanup; other defects | 0 | 7 | 17 |
| consumer; numeric fixes (incl. B-14) | 0 | 8 | 13 |
| traceability | 2 | 13 | 27 |

The run-order contradiction is resolved: OR-11 commits first, then rows 16a
to 16h, then row 20 onward after a clean round. OR-11's text carries the
resolutions its implementation needs (fluid-loop declarations from ports, a
warning channel, the schedule-column decisions). Found defects added as
entries: OD-22 (equipment dropped on init failure), applied unavailable
periods, wet-appliance hot-water triage, OR-17 (exact JSON float parsing).
Resolutions: `verification/L4-resolution-*.md`.

Status: implementation proceeds (operator ruling). Round L5 corrects the
entries for rows 21 onward ahead of the implementer; row 94c also needs its own clean round over
B-14.

## Round L5 (fresh verification of rows not yet executed)

| Area | Blockers | Majors | Minors |
|---|---|---|---|
| plan.md 1, 3, 5; schedule; follow-ups; measurement; profiler | 0 | 0 | 2 |
| plan.md 2; output reporting (incl. OR-18, OR-19); Python surface; inputs | 2 | 5 | 19 |
| fleet; runtime and memory; slop cleanup; other defects | 0 | 1 | 1 |
| consumer; numeric fixes (incl. B-14, B-15 to B-17) | 1 | 2 | 8 |
| traceability | 3 | 13 | 11 |

Area a: PR-4's evidence line now names its commit; M-0's record carries
its `[tools]` line. BR-10's GitHub facts (no CI runs on PRs 24, 25 and 27,
no Actions secrets, run 36028888222) could not be re-checked (`gh api`
blocked); BR-10's acceptance re-derives them when it runs. Found while
fixing: the toolchain was never upgraded (stable 1.94.1, latest 1.99.0;
nextest 0.9.137, latest 0.9.146) and nothing pins it; BR-20 (row 16a1)
pins the latest and runs before BR-10 sets CI's pin.

Area c, second pass (OD-1 to OD-21 line by line,
`verification/verify-L5-c2.md`): 0 blockers, 6 majors, 11 minors. OD-20's
Problem misdescribed a silent clamp as an unbounded value; the rest are
citation drift and OD-10's census undercount (twelve sites, not ten).

Area c: RT-3's stop-and-add record cited the wrong lines (corrected); RT-3's
comment rewrite and test-header rewrite are owed before its commit. OD-1
to OD-21 were spot-checked, not read line by line; they get a full pass
before round L5 closes.

## Round L6 (fresh verification after the L5 resolutions)

| Area | Blockers | Majors | Minors |
|---|---|---|---|
| output reporting (OR-18, OR-19), Python surface, other defects, inputs | 1 | 1 | 3 |
| plan, schedule, status, traceability, BR-20, runtime, profiler, slop | 0 | 2 | 3 |
| consumer, numeric fixes | 0 | 2 | 1 |

Area c (`verification/verify-L6-c.md`): B-14 left four current documents
(`docs/architecture.md`, `docs/invariants-and-observability.md`,
`docs/outage-behavior.md`, `docs/rust-primer.md`) naming renamed fields;
they are now in its update list, found by a stated search re-run at each
commit. B-14's Btu/h factor differs from OpenStudio-HPXML by 5.9e-10
relative, not the therm's 1.4e-7; corrected. CN-17's reason for not using
`maturin develop` no longer cites a different ruff (both lock files
resolve 0.16.10); the dev-group install remains the reason. CN-1 to CN-15
citations into the consumer were sampled, not read in full; a full-depth
pass reads every one and lists any consumer call site the entries miss
(`verification/verify-L6-c2.md`): 1 major, fixed (four CN-1 citations
into `suite_ami2.py` 41 lines stale after CN-16's insertions); the
`ochre_next` census (53 hits) and every non-test `Runner(` site are
complete. CN-17's gate engine is now defined as the latest
`feat/output-results` commit when it runs, with its census and row 16g's
re-check list derived from it, so the text no longer goes stale with each
engine commit.

Area b, full-depth pass (`verification/verify-L6-b2.md`; M-4a, M-4b,
FL-1 to FL-5, RT-1, RT-2, RT-3a, RT-4 to RT-8, SL-1 to SL-6, SL-8 to
SL-11): 0 blockers, 1 major. RT-1's hand-written `Telemetry` clone was
specified for two fields; OR-11 added the unknown-key latch. RT-1 now
copies all three, pins the latch in its test, guards the field count at
compile time, and takes `active_equipment_ids` from OR-19's roster plan.
SL-2's and SL-11's census counts are re-run when those rows start.

Area a, full-depth pass (`verification/verify-L6-a2.md`; uncommitted
OR entries, all of PY and IN): 0 blockers, 1 major, 1 minor. Nineteen
`dwelling/mod.rs` citations in OR-1, OR-2, OR-3, OR-6, OR-7, OR-13 and
plan.md §2.1 and §2.5 point 150 to 550 lines off, several at unrelated
code, because committed rows inserted code there. They are re-pinned, and
every `dwelling/mod.rs`, `blueprint.rs`, `environment.rs` and thermal
solver citation in every uncommitted entry is swept against `7b7a6a7d`
(`verification/L6-resolution-a2.md`: 98 changed, 28 confirmed; two were
wrong even at their pinned commit; OR-3's test precedent named code that
does not exist and now names `FailingPortEquipment`). Every changed
citation is re-checked independently (`verification/verify-L6-a3.md`):
every changed citation read (the per-file counts sum to 109; the sweep's
closing total of 98 was an arithmetic slip), one wrong (a second
occurrence of B-1c's mains leap-year line) and fixed, and RT-1's latch
line moved from the loop header to the check.
No design defect found.

Area a: the entry added for `review.md` V36 described an unreachable
branch as a daylight-saving defect (`FixedOffset` has no gaps or repeated
hours; the civil zone only converts instants) and duplicated OD-2 site 8.
V36 is OD-2 site 8, which now states why the daylight-saving half needs no
change and names its two tests; the duplicate entry and its schedule row
are gone. OD-22's guard is described as the sensible plus latent sum. The
OpenStudio-HPXML citations this pass could not fetch were fetched and
confirmed by the L5 resolutions (`verification/L5-resolution-b.md`).

## Fix-branch reviews

OR-19 `9ef347e0` (branch `i08-fixes`): review CHANGES-REQUIRED (1 major,
1 minor, `verification/review-OR-19-9ef347e0.md`); red team
ATTACK-SUCCEEDED (1 major, `verification/redteam-OR-19-9ef347e0.md`).
Assembly kept its own cache path, so equipment added before the first
step lacks the telemetry-unit schema metadata assembled equipment has; a
rejected add or replace advances the equipment id counter; planning moves
the roster out, so a panic would leave it empty. One follow-up commit
fixes all three, then a fresh review and red team.

OR-19 follow-up `ffa9051f`: review CHANGES-REQUIRED (0 blockers, 1
major, 4 minors, 2 nits, `verification/review-OR-19-ffa9051f.md`). The
major is confirmed against the code: assembly now reaches the recorder
through `install_roster_caches`, whose creation failure is a warning, so
`from_config` with an unwritable output path returns Ok and records
nothing (at `7b7a6a7d` it returned `output recorder init failed`); the
same arm, since `9ef347e0`, keeps the old recorder beside the new column
index on a runtime re-init failure. Fix: the recorder (with metrics) is
created in `plan_roster_caches` and carried in the fresh-schema plan, so
creation failure is a plan error and install stays infallible; a test
pins `from_config` failing on an unwritable output directory. Minors:
assembly still builds ports, rollback ports and the fluid solver that
the step-0 plan rebuilds; about fifteen placeholder fields instead of
one empty-caches value; the planner is generic over four instantiations
where `&[&dyn _]` suffices, which also removes two public `AsRef` impls
added only for it. Nits: history-narrating test comments; a cloned name
set and a double roster filter in `auto_register_actors`. Coverage
gaps: no test for `write_output = false` with zone temperatures in the
step result; the parity test covers only the battery fixture. All are
fixed on the port to `i08-head-fixes`.

Red team of `ffa9051f` (`verification/redteam-OR-19-ffa9051f.md`):
ATTACK-SUCCEEDED, 2 majors, 3 minors, 3 pre-existing. Both majors have
the review's root cause and fix: an unwritable output path makes
`from_config` return Ok and record nothing; and an `add_equipment`
before the first step with an uncreatable output path returns Ok with
a 90-column schema beside the old 85-column recorder, so the next step
fails on row width with the roster already committed (install is not
infallible). Minors: `set_tariff` pushes the parse warnings and assigns
the evaluator before `auto_register_actors()?`, so a rejection leaves
the tariff attached (confirmed); a rejected add or replace keeps the
candidate's init warnings (returned in the error instead); comments
still name the deleted `refresh_equipment_caches` (`mod.rs` 789, 1837,
1911, 3075, 4211, 5480; `tests/equipment_id_regressions.rs:2275`) and
the old "Refreshes internal caches" doc block survives at 4222-4235.
Pre-existing, fixed with the rest (nothing deferred): removing and
re-adding an EV loses its driver actor and actor columns
(`add_equipment` does not re-register actors); replacing equipment under
a different name leaves actors targeting the old name; a removed
equipment's `latest_env.equipment_core` entry survives until the next
step ends. Held: atomic rejection over about 25 observable fields at
all six entrances, post-step checkpoint identity, byte-identical
results against `7b7a6a7d` on 14 fixtures under both output and tariff
settings, schema metadata after remove and re-add, step path unchanged
and allocation-free.

RT-3 `44da330c` and RT-3a `69950a98`: audit
(`verification/audit-RT-3-RT-3a-69950a98.md`) 0 blockers, 0 majors; every
requirement implemented, goldens identical in three builds, physics
suites pass, 7130 debug and 7087 release tests. It found that release
clippy fails on a debug-only import (pre-existing) and that the ASHRAE
check needs a manual sync: BR-21 (row 21d) fixes both. Red team
(`verification/redteam-RT-3-RT-3a-69950a98.md`): ATTACK-FAILED on both,
bitwise identity held under NaN, signed zero, negative and duplicate
couplings and several solves per zone. Three pre-existing findings
(checkpoint restore leaves the coupling state stale; `config_mut()`
exposes geometry the scratch buffers depend on; the allocation test
skips several solves per zone) are RT-9 (row 21e).

## Pause point: commit back to `initial-implementation` (proposed 2026-10-05)

`initial-implementation` is at `1d60f478`, an ancestor of
`feat/output-results`, so a fast-forward lands it (`origin` is at
`ebdb1a17`). The schedule's own push rows (99a, 100a, 101a) come after
the numeric and consumer blocks, far off. The natural earlier pause is
the end of the found-defect band, row 24n, before row 25 starts the
planned output, Python, fleet and runtime work:
- in flight: OR-18c with OD-31 and OD-53, OD-30, OD-32 with BR-24, the
  OD-16 fixes, the head-audit fixes;
- the remaining non-physics found defects: RT-1 (row 22), OD-19,
  OD-21, OD-24 to OD-27 (rows 24g to 24n, with the operator), and
  OD-34, OD-35, OD-36, OD-43, OD-48, OD-49, OD-52, OD-55, OD-57 and
  BR-25;
- the physics finds (OD-37, OD-38, OD-39, OD-41, OD-44, OD-47, OD-50,
  OD-51, OD-54, OD-56), fixed before the checkpoint, each with its delta
  recorded in `02-measurement.md` and its deviations in the register.
  The operator ruled (2026-10-05): "no broken windows", "nothing merged
  knowingly broken". So no known defect is carried into the
  commit-back, and none is deferred to the numeric block. OD-16 folds
  together with its consumer migration, never without it.

At that point the engine carries no known defect: no silent default,
skip or discard; contracts (roster, control, checkpoint) enforced;
physics triaged against the references with every deviation
registered; tests deterministic and leaving nothing behind. Anything
found while draining joins the band, and the checkpoint waits for it.

Gates, in order:
1. The OR-19 fold-integrity audit, and the independent audit of the 13
   operator commits since `8082592e`.
2. The full block on the roll-up tip, every line green, `[stray]`
   included.
3. A roll-up report against `1d60f478`: BESTEST, parity, oracles, every
   golden delta with its cause, step time and peak memory on the bench
   set, and the D-entries added. Proof of no regression and of what
   improved.
4. The consumer gate: `HARES_CALIBRATION_SYSTEM` against the roll-up
   tip, through the contract check and its suites.
5. A fast-forward of local `initial-implementation`. A push to `origin`
   only on the operator's word.

## Fold register (2026-10-05, engine head `db5efe80`; operator landed `dc4a456d` .. `db5efe80`, 8 commits, after `1b708ccb`, independent audit owed)

Every fix lives on its own branch cut from the engine head, is
reviewed and red-teamed by agents that did not write it, is folded
onto `feat/output-results` by a fold branch built on the current head
(the operator fast-forwards), and after each fold an integrity audit
compares every source hunk and test with the head
(`verification/fold-integrity-*.md`). A branch is deleted only after
its fold is verified. This table is updated at every state change.

| Work | Branch (base) | Commits | Review state | Fold state |
|---|---|---|---|---|
| OR-18 fixes | folded | `f719895b`, `b7ea5d0a`, `b3807202` | review and red team SHIP | folded; integrity NOTHING-LOST (`fold-integrity-5d66dafc.md`) |
| OD-29 | folded | `3005ee1c`, `a4b29269`, `59465557` | review and red team SHIP | folded; integrity NOTHING-LOST |
| RT-9 | folded | `2a39a8ee`, `89eae0ba`, `d09a3b64`, `39113e2d` | red-team fixes verified | folded; integrity NOTHING-LOST |
| BR-21 | folded | `91815306`, `19ca1069`, `0c0bece9`, `ede151da`, `398d21a6` | APPROVED-FOR-FOLD | folded (operator fast-forward); integrity NOTHING-LOST, 2 low (`fold-integrity-BR-21.md`) |
| OR-19 | `fix-or19` (`1c18543a`) | `12f753be`, `a3a21a92`, `1bb045c6`, `f77054df`, `ba44d9f1`, `21c4fac0` | check of `21c4fac0` PASS (0 blockers, 0 majors, 0 minors, 2 nits; `review-OR-19-21c4fac0.md`) | `fold-or19c` on `8082592e` (`edb83678` ... `fe5fdfeb`, 7 commits, `CHECKPOINT_VERSION` 11) is stale. `fold-or19d` on `249912f9` (tip `2d491be7`; three import-only conflicts, both intents hold; `CHECKPOINT_VERSION` 11; block green: nextest debug 7321, observe 7379, dst 7334, profiling 7327, release 7278, goldens three modes identical, pytest 1417 passed 0 skipped, physics suites pass; `[stray]` TMPDIR line fails from leaks that predate it, which BR-24 fixes; `verification/fold-or19d.md`). The head then moved to `13881b48` (OD-6); Rebased: `fold-or19d` tip `2f85338d` on `13881b48` (7 OR-19 commits, `17bd7291` a rejected roster add on a fluid-type wiring conflict leaves roster, ids and checkpoint byte-identical, `2f85338d` fixes a `py_dwelling.rs` test that fails to compile on the head since `13881b48` made `schedule_path` optional); block green on every line (nextest debug 7326, observe 7384, dst 7339, profiling 7332, observe+profiling 1454, release 7283; goldens three modes identical; pytest 1417, 0 skipped, census clean; physics suites pass) except `[stray]` TMPDIR (BR-24, on `fix-testhyg`). FOLDED 2026-10-05 (fast-forward `13881b48..2f85338d` at the operator's instruction "fold"). Mutation proof done (operator approved, 2026-10-05): with `plan_loop_types(...)?` turned into `.unwrap_or_default()` in `plan_roster_caches`, `roster_add_wiring_failure` fails at `:107`; reverted clean (`verification/mutation-OR-19-roster-plan.md`). Owed: the fold-integrity audit; the assembly-time measurement. `fold-or19c`'s block was green (release 7243, release `observe` 7295, `observe,profiling` 1427, goldens three modes, `make check`); the assembly-time measurement owed |
| BR-22 (operator) | on head | `8082592e` | independent audit: 0 high, 2 medium (M4 deprecated `save_versioned` shim kept by `allow(deprecated)`; M5 `zone_state_ordering.rs:254` test asserts nothing), 10 low (`audit-OD-13-BR-22-head.md`) | landed; audit findings go to fix branch `fix-headaudit` |
| OD-16 fixes | `fix-od16` (`50a5c261`) | `cae2ab0a` ... `9713fe20` (5), `feaff388`, `211b530e`, `a1173a4a`, `55854486`, `287b8385`, `db644111` | every review finding fixed (DR sender single-axis by envelope load with release at event end; Ideal band class [0, 10] °C, BESTEST byte-identical to `50a5c261`; tank band to 44.4 °C per HPWHsim; explicit release restoring the configured band; terminal state after the budget error; solar override validated); `2689f2af` (PV tilt and azimuth from the input, OS-HPXML orientation map with a warning, or a typed error; multi-array PVs register every array; no golden moved); final review (`review-OD-16-2689f2af.md`) CHANGES-REQUIRED: 1 major (the DR pre-conditioning axis from outdoor minus zone temperature picks heating in 36-58% of cooling hours on four ResStock homes; the axis must come from the target's end use, as OCHRE addresses controls and EnergyPlus decides from the zone balance), 7 minors (release restores a tank's band but not its setpoint; checkpoint load after the budget error; gym deadband bounds per device class; three unpinned fixes and kept state columns; one `[writing]` failure on `55854486`); terminal state honoured by every step path; consumer change list recorded in the review (release constructor, single-axis GEB and DR deltas, `rejected_control_signals == 0`, budget error fatal per home, calibration budget 0). Follow-up `071affc9` .. `53c536ba` (9 commits): the pre-conditioning axis comes from the target (end use, or a unit's active, last active, then declared mode; the axis is checkpointed; a target with no direction is counted in telemetry, not guessed); release restores a water heater's configured setpoint and band; `load_checkpoint` resumes an ended run, `restore_building_state` on one is refused; gym and vector-env deadband bounded by the target's thermostat class; IdealHvac reads its zone once; a failed step zeroes PV irradiance; BESTEST byte-identical, goldens identical, nextest all lines green; two failures known before (release-clippy unused import, fixed on the head by BR-21; OCHRE `test_7day_benchmark` pandas 3, fixed on the head by OD-33); two commits fail `[writing]` alone, so the fold rewords them. Review of the follow-up (`review-OD-16-53c536ba.md`) CHANGES-REQUIRED: 1 major (a dual-mode unit keeps the first axis's override after its mode flips, so the band is squeezed 8747 of 8760 h on BESTEST 600 ideal), 5 minors, 5 info; 5 of 5 mutation checks reproduced. Decision: a dual-mode target needs an explicit direction on the event or the event is rejected at registration; an axis change releases the previous axis first; the axis comes from declared capability, never the per-step end use. Round 3 `34323f70` .. `2fb9f63b` (7 commits): equipment declares `thermostat_axes()`; an event names its direction (`preheat`/`precool`); `add_actor` rejects a dual-mode target without one (`HaresError::PreconditioningAxis`); one axis displaced, with a release on any axis change; a checkpoint load clears `step_failed`; `batch_step` takes bounds resolved once; telemetry resets; year test on BESTEST 600 ideal asserts one axis displaced on every step; BESTEST byte-identical; goldens identical; nextest all lines green; the two known failures predate it and are fixed on the head. Review `review-OD-16-2fb9f63b.md` CHANGES-REQUIRED: 1 major (the registration refusal is bypassable: an actor added before its target, `add_equipment_with_actors`, a misspelt name or a replaced unit leave an unserveable event silently doing nothing), 4 minor (spurious rejections on a dual-mode `ByEndUse`; `batch_step` panics on bad bounds; a misplaced doc; test gaps), 6 info; the dual-mode squeeze confirmed fixed (0 of 8760 steps with both axes displaced, preheat and precool); 4 mutations reproduced. A scratch fold onto `2f85338d` conflicts in 24 files and needs `CHECKPOINT_VERSION` 12 and the hooks moved into OR-19's `install_roster_caches`. Round 4: rebased onto `2f85338d` (tip `160ddb61`, 32 commits; `CHECKPOINT_VERSION` 12). Validation is inside `plan_roster_caches`, so every roster path refuses an unserveable event atomically; an unknown target is a typed error; removing or replacing a named pre-conditioning target is refused, not evicted; a dual-mode `ByEndUse` gives 0 rejections over 2000 steps; `batch_step` bounds are validated with no panic; axes are checked against band class for every thermostat type; the `HeatPumpWH` ramp branch was dead and is removed (HPWH checkpoint 3); `Dwelling.thermostat_axes`; DR actor checkpoint 2. Block green on every line (nextest debug 7405, release 7362; BESTEST byte-identical; pytest 1445, 0 skipped, with the pinned OCHRE). Review `review-OD-16-160ddb61.md` CHANGES-REQUIRED: 1 major (`addressed_by_end_use` widens end-use addressing for every signal type, so a DR `TurnOff` on `ByEndUse(HVAC_COOLING)` cuts BESTEST 600 heating from 1313.5 to 166.0 kWh; untested), 4 minors (removal refusal locks the roster with no `remove_actor` and disagrees with OR-19's eviction rule, with a false message; a branch test leaks a diagnostics CSV into TMPDIR; `batch_step` not atomic on apply-stage errors; stale `PreconditioningAxis` doc, a `Some(Heating)` fragment in a message, `add_actor_by_name` raises a plain `ValueError`), 6 info; the old bypass major is fixed; rebase lost nothing; BESTEST byte-identical; the cherry-pick onto `db5efe80` needs `max_consecutive_step_failures` in three head-added `SimulationConfig` literals; consumer call-site list in the review. Decision: refuse at registration and supply; on removal, evict with a dwelling warning counted in `health()`, as OR-19 does; end-use widening for thermostat-setpoint signals only. Round 5 implementer fixing all and rebasing onto the head | after a passing review: fold onto the head with the consumer migration (call sites listed in the review) | after the final review; consumer migration lands with it |
| OR-18 review items + OD-31 | `fix-or18c` (`5d66dafc`) | `9ba22b42`, `680b98ca`, `afcd981a`, `b68a8aca` | review CHANGES-REQUIRED (3 medium, 7 low; `review-OR-18c-b68a8aca.md`); all fixed in `83015bf2` (setbacks per OS-HPXML, conditioned volume a plain value, table provenance with a regeneration check and the BSD-3 notice), `d4d24518` (one OS-HPXML geometry for foundation and garage zones; vented crawlspace ACH from SLA, height and WSF; 0.1 ACH unvented; OS-HPXML SLA defaults with warnings), `1e443c48`, `a2ac4991` (goldens: furnace gas 89.17 to 114.99 to 120.75 therm, fan 47.4 to 64.2 kWh, cooling 1115.7 to 1083.9 kWh; fleet +0.047 kWh); block and physics green, sweep unchanged. `eb050372` (attics and garages by one OS-HPXML rule each: square-hip attic volume, OS-HPXML garage leakage) and seven clean golden re-captures (`132ed237` .. `e903016e`; largest move `full_year_900s_dst` cooling +0.8%); block green except two `conditioned_oracle` tests under `observe`: attic MAE against OCHRE 2.93 to 3.09 °C against a 3.0 °C bound (attic volume 1/3 instead of 1/2 of A·h on a gable roof). Not loosened. Source verification (`attic-volume-os-hpxml.md`): OS-HPXML v1.12.0 uses the square-hip volume for every attic (`geometry.rb:1315-1330`, written to the IDF Zone Volume); the hip rule is 27-31% low for a gable attic; the ~3 °C gap mostly comes from HARES's attic air capacitance multiplier 1.0 (OS-HPXML and OCHRE use 7 for every zone) and its attic wind coefficient (ASHRAE terrain law instead of OS-HPXML `f_t_SG`, 15-20% less airflow than OCHRE). Follow-up: `50f3761a` (every zone's temperature capacitance multiplier per OS-HPXML), `5a1edeae` (attic and garage leakage with OS-HPXML's terrain factor), `9010067c` (gable attic geometric volume), HEAD `caa47532`; Rust, goldens (three modes), ASHRAE and physics green; pytest has 2 new deterministic failures: `test_finite_thermostat_param_still_attaches_and_simulates` (heating 0.0 W over 5 steps at 21 °C) and `test_simulation_plan_swap_furnace_to_ashp` (ASHP segment 0.415 kW against the furnace's 0.799 kW); triaged: the thermostat failure is a HARES defect (an `IdealThermostat` defaulted its target to "HVAC", matching nothing, so the override was dropped silently; now the target is required and a missing one fails loudly); the ASHP swap failure is a test-window artefact (the zone needs 7 min to reach the heat pump's call, and over an hour the ASHP heats correctly). Attic oracle passes the UNCHANGED 3.0 °C bound in all six scenarios (summer 3.09 to 1.55). Deviation classes: capacitance multiplier and terrain factor were HARES errors; the remaining terrain gap is OCHRE's hardcoded rural assumption. The gable volume taken from gable-wall area inflates it with the eaves (OCHRE's formula); now being derived from the footprint and pitch. Roof roughness silent default (OD-53) fixed here. ASHP COP about 0.9 at -17 °C filed as OD-54. Round done, HEAD `7b409ca5` (20 commits on `caa47532`): the IdealThermostat target required (unhealthy with a named reason when missing); gable volume ½ × footprint × (span/2)·tan(pitch), with the span from the footprint and conditioned perimeter (BEopt 127.4 m³), else OS-HPXML's hip with a warning; roughness from the material (Engineering Reference table and EnergyPlus `ASHRAE_2005_HOF_Materials.idf`), with no material taking OS-HPXML 'Rough' and a warning; an unknown site type or boundary type is an error; register entries; seven clean golden re-captures (largest: `consumer_shape_900s` cooling -4.2%). Attic oracle passes the unchanged 3.0 °C bound everywhere, worst 1.31 °C winter ideal. Caution recorded: Very Rough shingles bring summer from 1.48 to 0.89 °C by departing from OCHRE's 1.67 and may compensate an untriaged attic difference. Remaining attic gap (summer +0.5, winter -1.2 °C) and free-float indoor MAE (summer 2.06, winter 1.94 °C) not triaged: filed as OD-56. Register collision: this branch and `fix-od30` both add D-005 to D-007; the later fold renumbers. Block green except the four OCHRE-side crashes already fixed on the head. Review `review-OR-18c-7b409ca5.md` CHANGES-REQUIRED: 7 medium (roughness rows not matching their EnergyPlus dataset records; an absent material not following OS-HPXML's defaults; the geometric attic height feeding the stack coefficient, unregistered; `9cff224b` loosened OCHRE bands without a measured cause; the winter attic bias of -1.26 °C untouched by roughness, so the MAE is not evidence for it (OD-56); goldens captured dirty with no stated causes; the 7× equivalent-battery capacity unrecorded), 9 low; multiplier and terrain confirmed correct; no performance regression. Follow-up done: rebased onto `2f85338d`, tip `de561181` (38 commits; pre-rebase kept as `or18c-pre-rebase-7b409ca5`). D-008 to D-011 each folded into its change commit. Roughness follows the EnergyPlus dataset record for each material; an absent material takes OS-HPXML's default. A found defect is fixed: the parser read `ShieldingOfHome`, but the schema spells it `ShieldingofHome`, so shielding was ignored on every HPXML home. The geometric leakage height is registered (0.008 °C). Band triage, class (d): the parity harness counted HARES HVAC energy twice and added therms to kW, and OCHRE's start temperature was an unseeded random draw. Both fixed, references regenerated; the cz5a mini-split residual is class (a), OCHRE charging duct losses to a ductless unit; OD-61 filed for what remains. Goldens re-captured clean with causes; equivalent battery 7× recorded as D-011; block green on every line. Review `review-OR-18c-de561181.md` CHANGES-REQUIRED: 1 major (pyright 2 errors: the rebase kept the head's `IndirectTank` and `TanklessWaterHeater` stubs beside the branch's), 3 medium (the gable guard halves gable-wall area assuming two outside ends, so `base-bldgtype-attached*` with one outside gable passes at a 6.12 ft rise instead of 9.18 ft and the attic is about 33% low, contradicting D-009's corpus claim; `cz6b_resistance_res_wh` and `cz4a_ashp_hpwh` sit at about 22% under the 25% default band, untriaged, `cz4a` unpinned, a candidate cause the fork's `WaterHeater.py:456` reading `hp_cop` before it is set; the cz5a mini-split class (a) band rests on an orphan duct system in the fixture, so the root cause is class (d)), 5 low (OD-53 text stale; `398097d2` message wording; ticket IDs in `tests/bestest/mod.rs`; a stale comment; the parity generator does not seed numpy); rebase lost nothing; previous findings fixed at root; nextest debug 7333, release 7290; the `db5efe80` fold conflicts heavily. Round 4 implementer fixing all, triaging the two residuals (a fork fix stays a local submodule commit, with publication the operator's call) and rebasing onto the head | after a passing review: rebase onto the head and fold | after the follow-up and review |
| OD-30 | `fix-od30` (`38398bd4`) | `5543134a`, `6006c05d`, `1ec68ee2` (strict test restored), `a2502f68` (replay by date), `4429f059` (radiant split), `013e9dbd`, `4b8abc8d` (golden provenance ignores golden outputs), `392f4b8f` (clean re-capture) | parity: all eleven 1-hour fixtures improve, envelope oracle 1.16 to 1.04 °C, 48 h trace 0.80 to 0.757 °C; `b2cc1ff2` (one vendored-OCHRE import helper; the comparison suites run without `PYTHONPATH`), `2d3a9076` (lighting 0.6 radiant, 0.2 visible absorbed like diffuse solar per EnergyPlus, through the existing diffuse path), `528ebc6c` (clean re-capture); three parity fixtures move away from OCHRE (OCHRE lighting is all convective), one within 0.007 of its ratchet; review (`review-OD-30-528ebc6c.md`) CHANGES-REQUIRED, 1 high (an override of a load's sensible fraction drops the appliance through the dwelling's silent "init failed, skipping" path, or skews its split: a regression on valid input; the silent skip itself is fixed at the root, every assembly skip becomes a typed error), 2 medium (observer and diagnostics miss radiant and short-wave heat; the newly running OCHRE suites leave temp files), 6 low; physics and both golden deltas independently confirmed; the lighting parity moves (cz5a minisplit 1.622 to 1.743 against a 1.75 limit) are correct per OS-HPXML and EnergyPlus; follow-up round 1: `80f6315e` (H1: radiant and visible parts stored as shares of sensible and resolved after every override; the root flaw: an equipment init failure, the autosize duct DSE error, an unreadable ventilation config and a failed basement schedule copy each fail construction; goldens identical), `0a73dd8d` (M1, L1, L2: one `ZoneHeat` accessor that the observer, diagnostics and Python export read; breakdown closes with short-wave), `8bfc451c` (M2, L5, L6: OCHRE suites write under pytest temp dirs; all 8 end uses compared, 11 pass, 3 xfail); L4 (one `xs:boolean` reader) half-done and uncommitted. The tool's permission check denied its one-line import edit and is waiting for the operator, so `hares-io` does not compile in the worktree. Not started: L3 (schedule row as a typed field), ratchet tightening, the block on the new commits. Resolver silent skips in `hares-io` filed as OD-48; the missing-defaults-file fallback filed as OD-49 | operator approved the import edit (2026-10-05). Round 2: `1da9d150` (ratchets tightened on ten fixtures from the appliance radiant split), `ca3e35d2` (cause notes beside the ratchets). Every move away from OCHRE triaged class (a): OCHRE lumps radiant gains into air (`Equipment.py:80-85`, "FUTURE: separate convection and radiation") and gives lighting radiative 0 ("TODO: get default fractions"); HARES follows OS-HPXML `model.rb:249-251` and EnergyPlus `HeatBalanceSurfaceManager.cc:3991-3996`; no band loosened. Block green after a disk-full re-run (release, goldens three modes, pytest 1390, `[stray]`); the 11 skips are the ones OD-13 removes on the head. Tip `8ded384b`: `8a632640` (L4: one `xs:boolean` reader at all 12 boolean elements, each pinned by its own failing-before test), `9b1e631f` (L3: typed `EnvironmentState::schedule_row`), `f28e1b9d` (cause notes, subject corrected; citations at EnergyPlus v24.2.0 lines), `8ded384b` (DIVERGENCES.md D-005 to D-007, each pinned by tests that fail when reverted to OCHRE's all-convective split). Review `review-OD-30-8ded384b.md` CHANGES-REQUIRED: 2 medium (gain overrides silently ignored: HPXML spellings shadowed by injected defaults, unknown keys accepted, wrong types become 0; remaining silent paths in assembly and stepping, filed as OD-57), 8 low; all 9 earlier findings fixed; conservation exact on 3 ResStock homes; step time within noise; bands are measured values plus margin, none loosened; citations check. Follow-up: rebased onto `2f85338d` (22 commits, tip `8ea79461`), D-005 to D-007 folded into their change commits, the commit claims corrected (seven of nine tightened bands catch a return to all-convective), the OD-30 measurement section in `02-measurement.md`; block green except one release test failing in its git-commit setup on a WSL vsock error, passing alone. The implementer's own check of the override commit `a8870d8a` found it rejects overrides that loads really read (`month_multiplier_<n>`, `gas_schedule_is_w`), and that `all` silently shadows `*`. Being fixed into `a8870d8a` with one key list owned by the equipment crate, plus the lows, the flaky-setup fix and the zone-ordering mutation; then an independent re-review. Done: rebased onto `1b708ccb`, tip `7b86f9d4` (23 commits). One override list per load kind in `hares-equipment` (`raw_params.rs`) shared by readers and validator, a debug read off the list panics; wrong-kind values rejected; `all` and `*` through one resolver, both at once an error, numbered keys range-checked, unread wildcard keys an error; `month_multiplier_<n>` now composes with a daily profile's month factors; the release flake was a failing commit signer, now isolated from global git config. The rebase keeps one implementation where the head did the same thing (head's init-failure code, the branch's typed `xs:boolean` reader, head's wording). Its own internal review (2 medium, 6 low) does not count as independent. Block on `4acf4c4a` green; on `7b86f9d4` pytest 1417 with 4 xfailed and pyright 0 (fleet tests now pass `defaults_path`, needed since the head's `952e5486`). Owed: the zone-ordering mutation, blocked because it tried to edit another branch's worktree, so the reviewer runs it in its own scratch worktree; the Rust lines on `7b86f9d4`; a rebase onto the head. `resolve_hvac.rs` `.ok()?` typed-config builders added to OD-48. Incident: its block runs used a shared scratchpad `block.sh` another agent had overwritten, so a stray block ran in the `fix-od16` worktree (no source changed); private script paths are now required. Review `review-OD-30-7b86f9d4.md` CHANGES-REQUIRED, no high: M1 the debug off-list read panic has no test (disabling it fails 0 tests); M2 an equipment-less schedule column skipped at `debug!` (must be a dwelling warning, matching the head's `7ac4d4d9` and `fix-headbreaks` `c71a360b`); lows (negative `month_multiplier` silently clamped, newly reachable by override; `apply_overrides` bypasses the list; appliance zone id first-of-type against the zone map's last; a wrong OCHRE citation; the `appliance_site` index mutation uncaught in hares-core; event-replay golden leaves moved without a cause, water heating +39% and +31%; no corpus-wide wildcard test). Block green on every line of `7b86f9d4` (nextest debug 7408, release 7365; goldens 7/7; pytest 1417, 0 skipped; pyright 0); no branch temp leaks; zone-ordering and five other mutations each caught; rebase decisions sound, nothing of the head lost; the scratch merge onto `db5efe80` has 9 textual conflicts and 5 compile errors in head tests, resolved compile clean. Round 10 implementer fixing all; rebases once `fix-testhyg` and `fix-headbreaks` fold | after review (the strict test passes; OD-38 no longer blocks it, OD-38 still owed) |
| OD-32 + BR-24 | `fix-testhyg` (`38398bd4`) | `a025464d`, `d2d74874`, `6128246d`, `2555f562`, `db043dae` | HELICS 50/50 under load (root causes: ports drawn from the kernel ephemeral range, a failed bind undetected, library closed under a live federate thread); `nextest` temp output 1,117 entries to 0; tests and benches start at a fixed local time instead of `Utc::now()`; bench temp files 850 to 0; nextest default 7238 and release 7195 green. `0216a35a`: one ResStock fetch path (the async path removed), an offline guard failing any unmarked test that reaches beyond loopback (proven to catch the old design); pytest 1390 green under the guard; review (`review-testhyg-0216a35a.md`) CHANGES-REQUIRED, 1 high (13 unbounded `federate_ready.wait()` calls hang until the worker is killed when a federate fails first: OD-32's original symptom), 3 medium (an obsolete stale-process guard false-positives on concurrent sessions; the offline guard is bypassable by module-scoped fixtures, raw `_socket`, UDP, `gethostbyname` and subprocesses; the fleet fetch lost its shared client and 120 s timeout), 9 low; every block line green, 26 HELICS runs under load all pass. Follow-up to `6c2e34d5` (`d9dbd0d9` a federate failing before ready fails the test promptly; `0c19b06b` zmq port pairs avoid HELICS defaults; one shared fixed start and temp-file helper; the `fix-or18c` HELICS worker crash traced to the base's early `helicsCloseLibrary()` and released-port races, both absent on the branch; 400/400 targeted and 20 x 28/28 whole-file stress under load; nextest debug 7236, release 7193; isolated TMPDIR empty) | re-review `review-testhyg-6c2e34d5.md` CHANGES-REQUIRED (0 high, 1 medium: an unknown ResStock version in a fleet fetch returns [] instead of raising; 7 low: shared client built per thread, offline guard hostname/`-E`/ctypes bypasses, swallowed attempts unreported under xdist, failed-test temp retention, one pyright stub gap, Ctrl-C with in-flight downloads, unretried federate ports, fixed-start spellings left); all 13 earlier findings confirmed fixed; nothing left in temp on any line, including a private-tmpfs `/tmp` pass; HELICS 22/22 under load 53-66. Follow-up done: rebased onto `2f85338d`, tip `c318e431` (22 commits), every finding fixed with a test (version refused before any fetch; one client under a lock; hostname, `-E` and xdist guard holes closed, native-code limits documented; retention bounded at 3; port retry with fresh core names; Ctrl-C ends a fleet fetch in 0.11 s, root cause polars' SA_RESTART SIGINT handler; one Denver offset helper; head-only temp leaks fixed); full block green on the tip (nextest debug 7326, release 7283; pytest 1473, 0 skipped; `[pyright]` 0; `[stray]` TMPDIR empty); HELICS 50/50 under load. Final review `review-testhyg-c318e431.md`: 0 high, 0 medium, 4 low (the `uname -p` allowance matches argv only; the federate retry fires on any HELICS error and can orphan cores; Ctrl-C does not cut a connect or TLS handshake in flight; the README still says async). Rebase lost nothing; 6 mutations each fail; nothing left in temp on any line, including a private `/tmp`. The head moved to `1b708ccb`, so it is no longer a fast-forward. Follow-up: the 4 lows fixed; block green on every Rust and lint line (nextest debug 7342, release 7299; pyright 0). Two branch-caused failures remain: the offline guard refuses `readelf -d` in the head's new `test_helics_wheel_reports_its_cxx_runtime` (`dc4a456d`); and 3 of 50 HELICS runs fail because `helicsCoreConnect` can raise HELICS error -3 on a bind failure, and `_release` re-raises it, skipping the retry. Being fixed with a deterministic injected-error test; 50/50 required. Done: rebased onto `db5efe80`, tip `c1aca232` (29 commits): `readelf -d <one file>` allowed by real path; a `HelicsException` on connect retries like a False return, cleanup never hides the real failure; the remaining loop failures were a double free this branch introduced (our `helicsCoreFree` plus the wrapper's own finalizer freed a reused address, invalidating the fresh core), now freed once through the wrapper's finalizer, with a test red on the old code; a fixture restores the real helics modules after fake-module tests. Rust lines all green (nextest debug 7348, release 7305); pytest 1493, 4 xfailed; pyright 0. HELICS loop: 32 runs shown clean, runs 27-44 lost when the scratch directory was wiped mid-run by another run, so 50/50 is not proven. Independent review running with its own 50-run loop (`review-testhyg-c1aca232.md`); scratch directories are now private per agent. `plan.md` §3's `[stray]` TMPDIR line did not prune pytest's retained basetemp contents; corrected (2026-10-05) to `-prune`. Then then a fold onto the head (the branch predates the pyright gate, so the fold must also pass `[pyright]`) || OD-13 (operator) | on head | `d9133308` | independent audit: 0 high, 4 medium (M1 ochre-marked tests run only on workflow_dispatch; M2 Lighting xfail cites a test in another file; M3 thermal-trace xfail reasons uncited and stale; M3b duplicate OD-34 heading, fixed in the ledger by renumbering Lighting to OD-47), 5 low (`audit-OD-13-BR-22-head.md`) | landed; audit findings go to `fix-headaudit` |
| Head audit fixes (OD-13, BR-22, BR-21 lows) | `fix-headaudit` (`249912f9`) | `6a499504` .. `7a3cf56c` (15) | every finding fixed except L15, shown wrong (unsigned `is_multiple_of` stable since Rust 1.87): OCHRE ochre-marked tests run on push and PR; OCHRE runs seeded, so the thermal-trace records are bit-repeatable strict xfails that fail on 2% drift; parity tolerance 1e-9 from measured agreement; `save_versioned` shim deleted; the empty ordering test replaced; kept lint census 29 with reasons; no production `expect`; `PvRoofSiting` cannot carry ignored panel fields; block green (nextest debug 7271, release 7228; goldens identical; pytest 1413, 0 skipped) except `[stray]` TMPDIR (BR-24); new entries OD-50, OD-51, OD-52, BR-25 | review `review-headaudit-7a3cf56c.md` CHANGES-REQUIRED: 2 medium (the thermal-trace xfails carry no triage class or cause, now owned by OD-50 as untriaged; the CI OCHRE job can pass with every test skipped through `importorskip`), 5 low; L15, CI selection, seeding, tolerances in debug and release, removals, goldens and allocation tests all verified; cherry-picks cleanly onto `2f85338d`. The zone-ordering mutation was denied by the tool's permission check (operator). Follow-up done: rebased onto `2f85338d` (15 commits clean, `ee12383c..5f31d549`) plus `24e5432c` (xfail reasons say untriaged; one pinned inclusive 2% record rule), `17d721df` (plain OCHRE imports; `--fail-on-skip` gate; the CI OCHRE job fails on a broken import, proven), `ac9703f0` (diagnostic scratch move and interior-surface pairs pinned), `ea2a48df` (en dash); tip `ea2a48df`; block green except `[stray]` TMPDIR (BR-24). Re-review `review-headaudit-ea2a48df.md`: SHIP on content, 4 lows (the default-run claim overstated; no zero or NaN record test; a private pytest API and a skip gate silent without a terminal reporter; 3.12 unverified); OR-19 and R6 mutations reproduced; rebases onto `1b708ccb` with no conflict; three conflicts with `fix-testhyg` mapped. Lows fixed (tip `4061da95`): a record holds only finite values (an infinite record had held every measurement); collection rule and skip gate on public pytest only, working serial, under xdist and without a terminal reporter; all five scratch buffers pinned. The 3.12 run found a broken window on the head: `787487df` imports `typing.TypeIs` (3.13 only) in `resstock.py` and `helics/_types.py`, so the package does not import on 3.12, which CI's 3.12 legs run; fixed in `4061da95` from `typing_extensions`. Waits for `fix-testhyg` to fold, then rebase and the full block |
| OD-40, OD-42 + OD-45 (operator) | on head | `a7a9253d`, `ee7bee85` | operator's | landed; independent audit owed |
| BR-23, OD-12, OD-14, OD-3, OD-6 (operator) | on head | `787487df`, `901354e0`, `699cfad2`, `249912f9`, `13881b48` | operator's | landed; independent audit owed |
| Audit of the OR-19 fold and the operator commits to `1b708ccb` | `audit-OR-19-fold-and-operator-1b708ccb.md` | n/a | OR-19 fold: NOTHING-LOST; assembly time within noise (−22% to +14% medians under load 20-47). Operator commits: H-1 goldens fail `compare-all` on the defaults digest since `952e5486` (frames bitwise); H-2 `schedule_integration.rs` did not compile at `952e5486`/`1b708ccb` (fixed by the operator's `a416b901`); the eight OR-19 fold commits inherit `13881b48`'s compile break (fixed at `2f85338d`); M-1 bag overrides of unset optional fields dropped (to OD-46); M-2 dirty golden provenance from `6c33e689`; M-3 pyright has no `pythonVersion`, so 3.13-only imports pass; M-4 `952e5486` drops default schedule profiles when no defaults path is given; lows | `fix-headbreaks` on `db5efe80`, tip `cd1b5972` (20 commits): H-1 and H-2 already resolved on the head (`dc4a456d`, `a416b901`); goldens re-captured clean one per commit, only `git_head`/`git_dirty` moved, digests bitwise (M-2); `pythonVersion = "3.12"`, pyright 0, with `fix-headaudit`'s `4061da95` cherry-picked byte-identical (M-3); default profiles only from the configured `defaults_path`, a missing one is an error naming it (M-4); lows fixed with tests (a real bug in `e113a61f`: an override changing backup fuel left the heater's FUEL capability stale); M-1 stays with OD-46; block green except `[stray]` TMPDIR from head-test leaks (`fix-testhyg`'s class); six further silent paths found, added to OD-57. Review `review-headbreaks-cd1b5972.md` SHIP, every count reproduced, mutations caught; findings fixed before the fold anyway: F1 medium `Fleet::from_resstock` and the benches hard-code `defaults_path: None` with no way to pass one, which M-4 makes a hard failure for a ResStock schedule lacking a mapped column; F3 the dwelling-level backup-fuel tests do not catch the `e113a61f` revert; F4 the garage and basement lighting warnings fire on every home without those spaces; a `mkdtemp` leak in `test_thermal_trace.py`; F2 (the `DefaultsStore` relative fallback) stays with OD-49. Consumer: `HaresForwardModel` (`forward_model.py:257,289`) and `run_calibration.py` without `--defaults` build with no defaults path. It conflicts with `fix-testhyg` in 6 files; it folds first, as a fast-forward that unblocks the operator's two branches, and `fix-testhyg` rebases onto it. Follow-up round running |
| OD-24, OD-26, OD-34 (operator) | worktrees `.workstrees/od24`, `od26`, `od34` seen 2026-10-05 | in progress | operator's | not dispatched by the orchestrator, to avoid duplicate work |
| OD-34 tariff, OD-35 EV capacity (operator, 2026-10-06) | `row/od34-tariff`, `row/od35-ev-capacity`, both on `db5efe80` | branch-ready | operator's: OD-34 mid-run tariff billing with the evaluator checkpointed, review and red team SHIP; OD-35 the negative fade from Smith 2017 BOL levels above nameplate, missing SSC's only-decreases guard, fixed at root, reference curve bit-pinned | the operator folds both straight after `fix-headbreaks` lands; OD-35 leaves the orchestrator's drain list. Fold rule from now on (`02-measurement.md`, the fold-process lesson): every committed sha builds and passes on its own, or the row lands as one squashed commit; golden provenance re-captured from a clean tree at the folded sha. Operator-owned items: the helics upstream report (OD-59) and the `CODECOV_TOKEN` secret before the closing push |
| OD-10 structural, OD-22, OD-23, OD-17 (operator) | on head | `18fbf72e`, `8c034e4f`, `6c33e689`, `1aa077ff`, `952e5486`, `1b708ccb` | operator's | landed after the OR-19 fold; independent audit owed. `8c034e4f` is the same root fix as OD-30's `80f6315e`, which keeps one implementation at its rebase |

## Open queue (2026-10-03)

0. Working rule (operator, 2026-10-03): each fix is made in its own
   worktree cut from the engine head, by an implementer; separate
   agents verify, review and red-team it; once clean and proven by the
   full block and the physics suites it is folded back onto
   `feat/output-results`. Worktrees and branches are removed once
   nothing uses them. The orchestrator dispatches and keeps the ledger;
   it does not verify findings itself.
   State at engine `d09a3b64` (2026-10-04): the operator folded OR-18's
   fixes (`f719895b`, `b7ea5d0a`, `b3807202`), OD-29 (`3005ee1c`,
   `a4b29269`, `59465557`) and RT-9 with its red-team fixes
   (`2a39a8ee`, `89eae0ba`, `d09a3b64`, `CHECKPOINT_VERSION` 10), each
   after an in-worktree review and red team (rows 21b, 21e, 21f), and
   landed BR-10 (`4a1f2c19`), BR-12 (`6646c801`, `aab05da0`) and OD-15
   (`3ff4583a`, no defect). Still to fold: BR-21 (fold branch
   `fold-br21` from `d09a3b64`, being built and verified for a
   fast-forward), OR-19 (`fix-or19`, follow-up for its final check in
   implementation; its checkpoint bump becomes 11 at the fold), OD-16's
   fixes (`fix-od16`, in implementation). The independent OR-18 review
   of `7aaaf53e` still running checks the folded code; its findings
   are fixed from the head. A fold-integrity audit
   (`verification/fold-integrity-5d66dafc.md`) compares every source
   fix commit with its folded counterpart hunk by hunk and re-runs the
   source commits' tests on the head, so no fix is lost in a
   cherry-pick: NOTHING-LOST (OR-18's three commits identical; OD-29
   equivalent, the duplicated registration block `a4b29269` carried
   removed by `59465557`; RT-9 differs only by the version bump to 10;
   all 72 source tests present and passing on the head; every absent
   test accounted for); `i08-fixes` is fully contained in `fix-or19`
   and was removed. Engine head `38398bd4` (the vendored OCHRE sentinel
   writes at the column's own resolution). Independent review of OR-18's fixes
   (`verification/review-OR-18-7aaaf53e.md`): CHANGES-REQUIRED, 0
   blockers, 0 majors, 2 medium (the water heater default-location
   order and the zone-less HVAC capacitance fallback are unpinned), 5
   low (dead default zones and `unwrap_or(20.0)` setpoints; no IECC
   zone from the weather station when the HPXML has none, unlike
   OS-HPXML; the Python stub types `zone_id` optional; ledger
   bookkeeping, fixed; release observe clippy, fixed by RT-9's fold);
   the humidity blend, pressure, floors, wall share and location tables
   verified against OS-HPXML and EnergyPlus; a sweep of 456 HPXML and
   ResStock files is byte-identical before and after, so the new errors
   reject nothing that built. Follow-up on `fix-or18c` (from
   `5d66dafc`) together with OD-31 (row 21i), implementer running.
   Next free slots, in order: OD-30 (now unblocked, OR-18's
   fixes are folded), then OD-32, BR-23, BR-24, then BR-22 after
   BR-21 folds; the independent audit owed on the consumer commit
   `18a8ec3` (CN-17).
1. OR-19 (row 21c): `9ef347e0` and `ffa9051f` are cherry-picked from
   `i08-fixes` onto branch `fix-or19` (`.workstrees/fix-or19`, cut from
   `1c18543a`), conflicts with OR-18 resolved, and every finding of the
   `ffa9051f` review (1 major, 4 minors, 2 nits) and red team (2 majors,
   3 minors, 3 pre-existing) fixed with a test that fails first.
   Committed `12f753be`, `a3a21a92` (no conflicts), `1bb045c6`
   (fixes). Review (`verification/review-OR-19-1bb045c6.md`):
   CHANGES-REQUIRED, 1 major (four em dashes in moved error strings
   fail `[writing]`), 6 minors (no test pins the driver cursor and its
   streams can alias the event-load range; the `RejectedEquipment`
   Python mapping untested and its use depends on warnings;
   `add_equipment_with_actors` atomicity and the `add_ev` change
   untested, docstrings silent on eviction; the four deleted fluid tests
   unreplaced; rejection untested through remove, remove by end use,
   clear and `auto_register_actors`, and on the live output file; the
   OR-19 entry's State stale, now updated), 1 nit. Every
   `ffa9051f`-round finding verified fixed and pinned by reverting it;
   full block green (nextest default 7168, `observe` 7227, `dst` 7181,
   `profiling` 7174, `observe,profiling` 1368, release 7125; pytest 1378
   with the census matching; goldens identical). Follow-up `f77054df`
   fixes every review and red-team finding (the cursor checkpointed,
   driver streams bounded below the event-load range with exhaustion an
   error; a restored dwelling reports its last step's environment; a
   tariff rebuild keeps each EV driver's state and stream;
   `RejectedEquipment` for every rejection; `add_pv` registers its
   surface only on acceptance; equipment validated before supplied
   actors; actors evicted when a replacement lacks a capability they
   require, via `Actor::required_control_capabilities`; panic-safe
   planning; the four fluid cases, all eight entrances and the live
   output file pinned). Full block green: nextest default 7186,
   `observe` 7245, `dst` 7199, `profiling` 7192, `observe,profiling`
   1381, release 7143; advisories; goldens identical in three builds;
   pytest 1383 with the census matching; `[writing]` and `[stray]`
   clean. Final check (`verification/redteam-OR-19-f77054df.md`):
   CHANGES-REQUIRED, 1 major, 5 minors, 2 nits; every claimed fix
   verified by revert, full block green. Major: a mid-run tariff
   rebuild restarts the battery management actor (7 of its 10 saved
   fields do not derive from prices; re-attaching the identical tariff
   after 40 steps flips net power from -3.44 to +4.70 kW; not a
   regression, the earlier commits restarted both actors). Minors: a
   checkpoint cursor below a restored driver's stream is accepted; a
   driver kept across an in-kind replacement keeps the old EV's
   parameters; required capabilities are a second hand-kept list; a
   stale cursor doc; resume through the checkpoint file differs by one
   ULP on three fixtures (`serde_json` without `float_roundtrip`,
   pre-existing). Follow-up `ba44d9f1` fixes all of them and found the
   real cause of the major: a tariff attached mid-run started its price
   index at step 0, so every later step was priced and billed at the
   wrong time (pre-existing billing defect); the evaluator now advances
   to the current step, and the battery manager and EV driver take over
   their predecessors' state and telemetry; a restored cursor behind a
   driver is rejected; an in-kind replace keeps an actor only when the
   actor seed matches; an actor sending an undeclared signal fails the
   step; `serde_json` `float_roundtrip` in hares-core makes file
   resume bitwise (it was hidden in workspace builds by frame_golden
   enabling the feature). Full block green (7193 debug, 7150 release,
   pytest 1383). Adversarial check (`verification/review-OR-19-ba44d9f1.md`):
   CHANGES-REQUIRED, 3 majors, 5 minors, 2 nits; the fold not started.
   Split by ownership so OR-19 can close: OR-19 fixes what its own code
   introduced (the EV driver take-over misses the SOC gate's hysteresis
   latch, also absent from checkpoints; the EV seed carries live
   degraded capacity, so an identical replacement never keeps its
   driver; three fixes unpinned, `float_roundtrip` only in a
   `-p hares-core` build; the `estimate_error` latch cleared on
   rebuild; the declared-signal check ignores the target and runs
   twice), implementer running. Pre-existing defects it exposed become
   entries: OD-34 (row 21m: a mid-run tariff bills from the run start,
   the first period is not prorated, a replaced tariff loses its open
   period, and the evaluator is not checkpointed), OD-35 (row 21n: EV
   capacity rises above its rating), and a note on B-4 (the Monthly
   anniversary anchor drifts after a short month).
   Fold note: the head is at `CHECKPOINT_VERSION` 10 (RT-9 and OD-29
   reconciled); OR-19 and OD-16's fixes each change the checkpoint
   shape, so each fold bumps it once more (11, 12, in fold order) and
   re-pins `checkpoint_shape_is_pinned_to_its_version`. OD-16's fixes
   change the consumer contract (its DR release `(None, None, 0.0)` and
   its GEB pre-event two-axis delta are rejected): the consumer
   migration lands with that fold, and the consumer gate runs on it.
   Red team (`verification/redteam-OR-19-1bb045c6.md`):
   ATTACK-SUCCEEDED, 1 major, 4 minors, 4 info. Major: the EV driver
   stream cursor is not checkpointed, so after remove and re-add a
   resumed dwelling gives a new EV the stream a restored driver holds
   (two live drivers share one stream; resumed diverges from
   continuous). Minors: a rejected Python `add_pv` still registers its
   irradiance surface (pre-existing); a duplicate `add_ev_with_driver`
   now raises `HaresSimulationError` instead of `HaresEquipmentError`;
   replacing an EV by a different kind under the same name keeps its
   driver bound and warns every step; whether a Rust caller sees
   `RejectedEquipment` depends on whether the candidate raised
   warnings. Info: install is not panic-safe (no current equipment
   panics there); `set_tariff` and `auto_register_actors` mid-run
   rebuild every EV driver from word 0 and reset the cursor
   (pre-existing); equipment added after rows are recorded gets no
   columns (documented); event loads ignore their sub-stream index
   (pre-existing, new entry OD-29, row 21f, own worktree `fix-od29`,
   implementer running). Held: rejected plans leave the output file
   (inode, mtime, size), directory, recorder and state unchanged for
   CSV, Parquet and rotation; re-plans never truncate recorded rows;
   post-step changes with either `retain_batches`; the cursor within
   one dwelling; the fluid conflicts the deleted validators caught are
   still rejected; plan-time panics leave the roster intact; goldens
   identical in three builds, allocation-free. Every OR-19 finding
   (the major, the minors, the install panic safety and the mid-run
   driver rebuild) is fixed in one follow-up once the review reports;
   then fold. `i08-fixes` is removed after
   the fold. Fold order: BR-21, OR-19, RT-9 and OR-18's fixes each fix
   or touch the same release-clippy import; the first fold carries it
   and the later ones are reconciled on top.
2. OR-18 (row 21b), `1c18543a`: red team ATTACK-SUCCEEDED (3 majors, 5
   minors, see Committed-row audits); every finding fixed on branch
   `fix-or18` (`.workstrees/fix-or18`, cut from `1c18543a`), each with
   a test that fails first, together with the audit's (4 majors, 9
   minors). Committed `9d599117` (all findings: a per-placement
   ambient dry-bulb and wet-bulb computed once per step for occupied
   placements, per OS-HPXML `waterheater.rb` inlet humidity with the
   humidity ratio blended; every `ZoneId(1)` fallback and silent zone
   temperature default removed, typed errors instead; ambient
   placements declare no thermal port; NaN outdoor rejected; closure
   check corrected; heat pump wall share 0.5 only in the living zone,
   per OS-HPXML and OCHRE; several conditioned zones rejected;
   `docs/python.md` states behaviour only) and `f1caa8da` (fleet golden
   re-captured from a clean tree, values identical). Goldens identical;
   7185 debug and 7142 release tests; ASHRAE, oracle 25/25, physics
   53/53, Python 1217 pass. Audit M3 (one commit for four) is history
   and stays as recorded, mitigated by the audit's isolation run.
   Follow-up running on the same branch for what the implementer found
   still there: the heat pump cooler's capacitance pass-through,
   scheduled loads dropping gains on a missing zone role, an invented
   zone for a zoneless building, the solver config's default zone, and
   the OS-HPXML default location for a water heater with no
   `<Location>` (now a build error, which would reject valid HPXML).
   Follow-up `7aaaf53e` fixes all five (heat pump cooler capacitance
   passed through; a heat-giving scheduled load without its zone role
   is a typed error; `NoZones`; `ThermalSolverConfig::new(indoor_zone)`
   required, no `Default`; the OS-HPXML default water heater location,
   `defaults.rb:6104-6132`, recorded as a warning); 7192 debug, 7149
   release; goldens identical; physics suites and Python pass. It also
   found two more defects, new entries OD-30 (HPXML appliance event
   loads drop their internal gains, row 21h) and OD-31 (zone volume
   fallback, row 21i). Review running
   (`verification/review-OR-18-7aaaf53e.md`); then red team, and the
   per-step cost measurement the audit found missing (`69950a98`
   against the fixed branch, profiling phases, recorded in
   `02-measurement.md`); then fold.
3. BR-21 (row 21d): committed `d41646e9` on branch `i08-head-fixes`
   (cut from `1c18543a`); the block gains `[clippy-default]`,
   `[clippy-dst]`, `[clippy-profiling]`, `[clippy-observe]`,
   `[clippy-observe-profiling]` and `[clippy-release]`. Review and red
   team running; the implementer flagged a remaining
   `#[cfg_attr(not(test), allow(unused_imports))]` in
   `dwelling/mod.rs`, fixed in the follow-up with the review's
   findings. Red team (`verification/redteam-BR-21-d41646e9.md`):
   ATTACK-SUCCEEDED, 0 blockers, 0 majors, 5 minors; 58 clippy
   configurations clean, test lists identical to the parent in every
   build, the parent's `--release -F observe` E0425 reproduced and
   fixed, the gate passes from a pristine worktree. Minors: the gate
   runs without `--locked` and rewrote a stale `uv.lock`; the
   `allow(unused_imports)` sites hide imports unused in every build
   (`gas.rs`, `heat_pump_wh.rs`, `tankless.rs`, `indirect_tank.rs`;
   redundant in `resistance.rs`; also `dwelling/mod.rs:100` and five
   `observe` `cfg_attr` sites); no lint line for `--release -F observe`
   or `--release -F dst`; `make check` still fails fresh in
   `check_ocv.sh` (no `pybamm` extra), so the ASHRAE gate never runs
   through it; `conditioned_oracle` is gated out of release+observe as a
   whole when only one diagnostic line needs the cfg. Informational:
   `gen_ashrae_reference.py` docstring, the gate builds the extension
   for a pure-Python script, CI clippy runs seven uncached builds.
   Review (`verification/review-BR-21-d41646e9.md`): APPROVED, 2
   minors, 2 nits; every block line passes (seven clippy lines plus
   six more release and `observe_detailed` combinations; nextest
   default 7151, `observe` 7210, `dst` 7164, `profiling`,
   `observe,profiling`, release, release `observe`; bench; goldens);
   the block's stale `[test-release-invariants]` line is removed from
   `plan.md` §3 and `implementation.md` (OR-11 deleted the feature),
   and `[clippy-release-dst]` and `[clippy-release-observe]` are added.
   Follow-up `4872f244` fixes every red-team and review item: the gate
   runs `--locked` (a stale lock fails, `uv.lock` untouched);
   `make check` passes from a fresh checkout (`check_ocv.sh` locked
   with the `pybamm` extra and no longer rewrites the tracked OCV
   tables, `gen_ocv.py --toml-dir`); every unused-import and five
   observe suppressions removed, which exposed a real defect (the
   fluid solver's conservation-violation count and peak imbalance were
   never recorded before the error return, so `observe` telemetry
   always reported 0; now recorded and tested); CI lints nine
   configurations; `conditioned_oracle` runs in release `observe`
   (7167 tests); the script type errors fixed (pyright 0). Block lines
   green, goldens identical. A second follow-up is running for the OCV
   tables (stale PyBaMM headers, no gate compares them) and a pyo3
   `seed` parameter accepted and ignored (`py_equipment.rs:771`).
   Second follow-up `4e54d56b`: the OCV tables regenerated (headers
   only, values byte-identical) and `check_ocv.sh` diffs them, shown
   failing on a one-digit change; the ignored pyo3 `seed` on
   `EV.from_vehicle_with_archetype` removed (it seeded nothing; the
   driver's seed is on `add_ev_with_driver`), so no `allow(unused_*)`
   remains. Block green (nine clippy lines, nextest default 7151,
   `observe` 7210, release 7108, goldens, `make check` fresh) except
   `[pytest]`: the HELICS completion test crashed its worker (1 failed,
   1377 passed; alone it passes 2 of 4 under load): new entry OD-32
   (row 21j). Python type-check errors seen in the editor (gym env,
   HELICS dwelling, stub overrides) with no block line: new entry BR-23
   (row 21k). Final check (`verification/review-BR-21-4e54d56b.md`)
   APPROVED-FOR-FOLD (1 minor, 2 nits). Fold branch `fold-br21c` on
   `38398bd4`: `91815306`, `19ca1069` (message reworded to behaviour
   only), `0c0bece9`, `ede151da` (the review's leftovers: the `ocv.rs`
   doc states the generator command, a mixer case in the fluid
   violation test); conflicts resolved to the head's forms (RT-9 had
   already fixed the shared import and the oracle gating; OR-18 had
   already rewritten scheduled-load resolution). Fast-forwards; full
   block green (nextest default 7237, `observe` 7295, `dst` 7250,
   `profiling` 7243, release 7194, release `observe` 7252; `make
   check`; pytest 1381 with the census empty) except `[golden]`: the
   OCV TOML header regeneration changes the defaults digest BR-13
   (`32044ab5`) now checks, so all 7 goldens report the digest only,
   every product bitwise identical. The goldens' provenance is
   re-captured in its own commit from the clean committed tree, then
   the branch goes to the operator to fast-forward.
   `scripts/pybamm-requirements.txt` (pins `pybamm==26.3.0` against the
   lock's 26.9.0.0, referenced nowhere) is dead: its removal is an
   operator decision (deletion). The review's census of the other 78 suppressions is new
   entry BR-22 (row 21g), after the follow-up folds. RT-9 (row 21e): committed
   `3c734c44` on branch `fix-rt9`; review
   (`verification/review-RT-9-3c734c44.md`) CHANGES-REQUIRED, 1 high,
   3 medium, 1 low: `tests/freefloat_oracle.rs` still calls
   `config_mut()` and no longer builds under `observe` (the default
   gate never builds it, so the free-float oracle suite has not run in
   any gate); `last_coupled_state` is redundant with
   `last_coupling.is_empty()` and made `CoupledState` public only for
   the snapshot; `ThermalSnapshot` is now an unguarded checkpoint
   schema (no tripwire against a shape change without a version bump);
   `resumed_run_equals_continuous_run` passes at the parent and its
   doc does not state why. Passed: 7155 debug and 7112 release tests
   with OCHRE present, goldens identical, restore validated before
   mutation. Follow-up `d7fe5684` fixes all four (the free-float
   oracle disables natural ventilation through its HPXML before
   construction; `CoupledState` and the flag deleted, the path derived
   from `last_coupling.is_empty()`;
   `checkpoint_shape_is_pinned_to_its_version` pins the serialized leaf
   paths and types to the version; the resume test's doc states the
   step order) and makes `--release -F observe` build; 7156 debug,
   7215 observe, 7113 release, 7172 release observe; physics and
   oracles under `observe` 80/80; goldens identical. Red team running
   (`verification/redteam-RT-9-d7fe5684.md`), probing also the tripwire (not shown failing by mutation) and the
   hard-coded line numbers in the test's doc comment; then fold after BR-21 (both fix the same release-clippy
   import). Its run had 6 `hpxml_parsing_tests` failures from the
   uninitialised `vendors/OCHRE` submodule in its worktree; the review
   runs with the submodule present. Its report also confirms
   `hares-core -F observe` does not build at `1c18543a`
   (`solver_builder.rs:512`), which BR-21's commit fixes.
4. New engine commits on `feat/output-results` (operator): `fe804b59`
   (OD-16, the actuate release signal and the tolerated-failure
   consistency sums), `32044ab5` (BR-13, row 16d, the golden
   defaults digest checked) and `c6889589` (BR-14, row 16e, strict
   tariff reading, golden tariff manifests, the month-boundary billing
   fixture), `50a5c261` (BR-15, row 16f, fleet manifest weights and
   aggregation resolution; the operator's review and red team done),
   and consumer `18a8ec3` (CN-17, row 16a, on `i08-consumer`;
   the operator's own review pass is done): each gets an independent
   audit and red team as slots free. OD-16 audited, red team running;
   the rest queued. Every
   fix branch is cut from `1c18543a` and is reconciled onto the head
   at its fold.
5. Round L6's areas are resolved; a fresh round over the whole ledger
   follows the next batch of commits.
6. Done 2026-10-04 (operator instruction): the vendored OCHRE commits
   `35c5edd` and `8681122` (OD-33's sentinel fix) pushed to
   `AZX-PBC-OSS/OCHRE` branch `hares-pandas3-sentinel`, so the
   submodule pin is fetchable from a fresh clone and CI.
   Owed by the operator: whether to delete the dead
   `scripts/pybamm-requirements.txt` (BR-21's follow-up); the SL-7
   timing re-run on a quiet host; the
   OCHRE parity and thermal-trace pytest files at `b8195c83` and
   `7b7a6a7d` (OCHRE and the built extension in an audit environment);
   `rustup update stable` for BR-20; the CODECOV_TOKEN secret before the
   first push.

## Committed-row audits

SL-7 (row 20, `7b7a6a7d`) was committed after OR-11 (`b8195c83`) and
before rows 16a to 16h and round L5, against the readiness order. Both
commits are audited by auditors independent of their implementer and
reviewers: `verification/audit-OR-11-b8195c83.md`,
`verification/audit-SL-7-7b7a6a7d.md`. Round L5 covers rows 21 onward.

| Commit | Entry | Blockers | Majors | Minors |
|---|---|---|---|---|
| `b8195c83` | OR-11 | 3 | 0 | 3 |
| `7b7a6a7d` | SL-7 | 0 | 1 | 1 |
| `1c18543a` | OR-18 (red team) | 0 | 3 | 5 |
| `1c18543a` | OR-18 (audit) | 0 | 4 | 9 |
| `fe804b59` | OD-16 (audit) | 0 | 2 | 5 |

OD-16 audit (`verification/audit-OD-16-fe804b59.md`): the release form
is not a pure pass-through in the thermostat FSM when its deadband is
positive (the deadband auto-correct, `thermostat.rs` about 585-615,
still uses it: a release with 5.0 against 20/24 installs a 26.0
cooling override); the water heaters (heat pump, resistance, gas)
write the release deadband permanently and the indirect tank rejects
it, so the contract differs across equipment. Minors: the per-zone
consistency skip has no test; two of three zero-deadband rejection
sites unpinned (the IdealHvac one unreachable behind the validator) and
the FSM's non-finite branch unpinned; IdealHvac's `<= 0.0` lets NaN
through and the predicate and message are written three times, with
IdealHvac a second hysteresis write site; recorded outputs still read
a failed equipment's stale power after a tolerated failure
(pre-existing: `mod.rs:4589`, `:7080`, `:7244`); comments narrate the
old behaviour, repeated rationale, a one-line forwarder with an unused
parameter, a test hard-coding the default hysteresis. Held: the
electrical skip is exact (`step_succeeded` set only on success, ports
restored by swap), the HVAC family shares one application path, the
pinned tests fail without the fix, goldens identical in three builds,
block and physics suites green except release clippy (BR-21).

OD-16 red team (`verification/redteam-OD-16-fe804b59.md`):
ATTACK-SUCCEEDED, 1 blocker, 4 majors, 5 minors. Blocker: a tiny
positive named deadband (1e-17 to subnormal) passes every guard, the
EBM window collapses (capacitance x hysteresis <= epsilon), and since
the commit skips failed equipment in reconciliation with no failure
budget, a dwelling runs on with the HVAC dead (bestest s54, -5 °C: 720
tolerated failures, every step Ok, zone 19.4 to 0.35 °C); before the
commit the run failed loudly, so this is a regression. Majors: a
configured `hysteresis_c = 0` passes validation; a release with no
deadband installs a frozen cooling override when the base gap is at or
below twice the hysteresis (init's reconciliation produces that gap);
the hysteresis a named event writes is not checkpointed for furnaces,
baseboard and boilers (resumed differs by up to 1.07 °C); a
deadband-only override is silently dropped. Minors: gym bounds and the
HELICS guard; auto-correct rewriting a named cooling pre-cool; a
failed HVAC step leaves partial state; `EnvelopeResolution` equipment
treated as failed; a test helper. Held: signed zero and non-finite
rejected, defrost, consistency sums through consecutive and
first-step failures, every producer through the validator, goldens
identical. Fix branch `fix-od16` (cut from `50a5c261`) for the audit
and red team together, implementer running, the blocker first (an
explicit failure policy and one deadband validity rule). Fixes
`cae2ab0a`, `10951bed`, `d960b82b`, `b6b4caa1`, `9713fe20`; independent
review (`verification/review-OD-16-9713fe20.md`): CHANGES-REQUIRED, 0
blockers, 2 majors, 11 minors; every audit and red-team finding fixed
and pinned (two exceptions, now minors); a 944-run sweep (472 HPXML
and ResStock files, January and July) byte-identical to `50a5c261`.
Majors: the engine's own `DrCompliance` sends a two-axis delta the new
rule rejects, so a DR event on a typical home does nothing; IdealHvac
was forced to a 0.1 °C band it does not need, moving BESTEST
unrecorded. Contract adopted from the review: a crossed or too-narrow
two-axis command stays rejected (EnergyPlus stops on crossed
setpoints; the Nest API rejects them) and the senders are fixed
(`DrCompliance` single-axis by mode, the consumer likewise); IdealHvac
accepts a zero band and BESTEST returns to its prior bits; tank water
heaters get their own justified band maximum through the same
predicate; an explicit release constructor; a terminal state after the
failure-budget error. Implementer fixing all on `fix-od16`.

RT-9 red team (`verification/redteam-RT-9-d7fe5684.md`):
ATTACK-SUCCEEDED, 1 high, 3 medium, 3 low. High: heat recovery
ventilation's derated effectiveness, written at step k and read at
k+1, is neither snapshotted nor recomputed, so a resumed dwelling with
an HRV diverges on 32 columns (no fixture has non-zero effectiveness).
Medium: the ideal-capacity fallback state is not restored; the shape
tripwire misses numeric retypes, tuple arity, unit-variant renames and
Option inner types; `load_checkpoint` mutates clock, RNG and equipment
before failing (pre-existing). Low: restore accepts finite couplings
that break the solve; drifting file:line refs in a test doc; the solver
is a pub field. Held: every other non-snapshot field, 20 malformed
snapshots rejected without panic, v8 rejected, the free-float oracle
change, allocation, goldens. Implementer fixing on `fix-rt9`.

OR-18 audit (`verification/audit-OR-18-1c18543a.md`): the numeric
delta and golden claims hold. Goldens identical at `1c18543a` (default
5/5, `dst` 1/1, `profiling` 5/5); per-home hashes against `69950a98`
show only fleet home 1 (bldg0000004) moved; with the water heater's
ambient resolution disabled in a disposable tree, `compare-all` against
`69950a98`'s goldens is identical, so the dehumidifier, closure,
IdealHvac and plumbing changes are bitwise neutral and the ambient path
is the only source of the delta. Home 1 binds at the 20 °C floor in a
-12 °C week: water heating +0.217 kWh, HVAC heating +2.70 kWh (tank
loss no longer enters the zone), matching the golden; weights and
floors re-confirmed against `geometry.rb` v1.12.0. ASHRAE reference
PASS; oracle/BESTEST/parity 25/25; 7157 debug and 7114 release tests;
fmt and dev clippy clean; release clippy fails on the pre-existing
import (BR-21). Majors: the `ZoneId(1)` fallback inlined at 18 sites
and silent (and a water heater with neither zone id nor zone type runs
on zone 1); the heat pump wall-heat default moved 0.0 to 0.5 for
conditioned basement, conditioned garage and kitchen with no test or
recorded delta; one commit for four (mitigated by the isolation run);
`docs/python.md:136-147` links gitignored bookkeeping. Minors: release
clippy; ledger accuracy (the "three fleet homes" claim: only
bldg0000004 is in a golden; corrected in `03-output-reporting.md` and
row 21b); no per-step measurement record; duplication across the five
water heaters, a dead tankless field and forwarder wrappers; silent
substitutions (non-finite housing-unit temperature, no conditioned
zone); `end_use` lag at the `ByEndUse` dispatch and thermal-grouping
read sites; telemetry naming; the dehumidifier port struct-update; the
fleet golden captured from a dirty tree. Every code finding is added to
`fix-or18`'s scope; the release clippy item is BR-21's.

OR-18 red team (`verification/redteam-OR-18-1c18543a.md`):
ATTACK-SUCCEEDED, three majors, each confirmed against the code. (1) A
heat pump water heater in an ambient placement feeds its dry-bulb
series to the COP and capacity curves as the wet-bulb
(`heat_pump_wh.rs:373-381`): COP 2.555 against 2.130 for the same
22 °C / 14 °C wet-bulb air; 'other housing unit' had read the zone's
real wet-bulb before, and outside placements ignore the available
outdoor wet-bulb (`base-dhw-tank-heat-pump-outside.xml` water heating
30.9 to 116.8 kWh over 48 h). Fix: a per-placement wet-bulb in the
ambient series, computed once per step. (2) The water heater and HVAC
constructors still fall back to `ZoneId(1)` when no zone resolves
(`unwrap_or(ZoneId(1))` in resistance, heat pump, gas, indirect, tankless,
ideal HVAC, furnace, baseboard and others; `solver_builder.rs:125`, `:899`,
`:1572`), and the commit removed the only warning, so the fallback is now
silent; an added water heater with no zone succeeds on zone 1. Fix: an
unresolved zone is a typed `init` error, as the dehumidifier's now is.
(3) An ambient water heater cannot be added after stepping begins: its
thermal port has no zone, the frozen port table's check requires one,
so the Python add paths that worked at `69950a98` now fail. Fix: an
ambient placement declares no thermal port. Minors: the ambient manager
falls back to outdoor when no conditioned zone exists, and 'other
housing unit' falls back on a non-finite zone temperature (both silent
defaults); `f64::max` turns an outdoor NaN into the floor value; the
synthetic closure check rejects closed Wall+Slab and
FoundationWall+Slab+Roof envelopes, accepts zero-area surfaces and
counts other zones' boundaries; the heat pump wall-heat default moved to
0.5 for conditioned basement, conditioned crawlspace and non-freezing
space without being stated (code reading, not run); with several
conditioned zones the first wins silently. Held: location string
variants and floors, non-first conditioned zones, checkpoint restore,
end-use sums on 18 files (control files bit-identical to `69950a98`),
IdealHvac sign handling including signed zero, the dehumidifier's
errors. All are fixed on `i08-head-fixes` together with the audit's
findings (audit running).

OR-11: the fluid-loop resolution, the warning channel, the schedule-column
reasons and the feature's removal are implemented as written; debug 7133
and release 7090 tests pass, clippy clean, golden `compare-all` identical
in the default build (`dst` not yet run). Three blockers: rows the census
dispositions as typed errors or `debug_assert!` were committed as
debug-only logs. The six zone-unresolved checks (five water heaters and
the dehumidifier) still warn under `debug_assertions` and fall back to
`ZoneId(1)`; the synthetic closed-envelope check and the IdealHvac
end-use sign check are debug-only `tracing::error!` with "STOPPED ROW"
comments, never resolved in OR-11's entry. The decision log's
`or11_resolution` described them as resolved; it is corrected. Minors:
the `helpers.rs` `ZoneId(1)` fallback row's ledger text and code
disagree (folded into blocker 1); two audit-environment notes (submodule
init, a concurrent-build race), no code defect. The audit's fmt finding
does not reproduce at `7b7a6a7d` (`cargo fmt --all --check` clean there);
it came from uncommitted edits in the main tree. The right fix for each
blocker is researched in `verification/OR-11-stopped-rows-research.md`
and is entry OR-18 (schedule row 21b).

Red team (`verification/redteam-OR-11-SL-7.md`): OR-11 ATTACK-SUCCEEDED,
2 blockers, 1 minor, both reproduced. `add_equipment` keeps a candidate
the fluid-loop check rejected; `replace_equipment` loses the original
equipment and installs the rejected one. Every roster entrance mutates
before the now-fallible refresh. Fixed by OR-19 (schedule row 21c): the
refresh splits into a pure plan and an infallible install, every entrance
plans against the prospective roster before mutating, and the pre-step
loop-type map reads live declarations. SL-7 ATTACK-FAILED: no break; one
pre-existing minor (the `1 + d_i` division in the identity-coupled solve
is unguarded, unchanged by SL-7; RT-3 adds a construction-time check that
every divisor is finite and positive, with a test).

SL-7: unreachability proven structurally (no implicit-matrix field
survives); hares-envelope 438 debug and 435 release tests pass; golden
`compare-all` identical in default and `dst`; clippy clean. Open: the
entry's Measurement (share of `simulate()` in `step_into` and
`solve_for_output_input`, before and after) was never recorded (major).
Taken since, independently (`02-measurement.md` SL-7 section): envelope
phase share 38% to 37%, `simulate()` 1.352 s to 1.374 s, inside a 16 to
18% spread on a host at load average 18 to 145, so no measurable change
and a re-run on a quiet host is owed; `perf` cannot run on this WSL2
kernel, so symbol shares come from the profiling phases. ASHRAE reference,
BESTEST, parity and oracle suites (25/25), physics validation and solar
parity (53/53) pass at both `b8195c83` and `7b7a6a7d`; goldens identical
in default, `dst` and `profiling`. Not run: the OCHRE parity and thermal
trace pytest files (OCHRE and the built extension not installed in the
audit worktrees' environments) and the B0 check (row 16g); the stale
"CN implicit path remains well-behaved" comment at `state_space.rs:271`
(minor) is fixed by `06-runtime-memory.md` RT-3, the next row to edit
that file.
