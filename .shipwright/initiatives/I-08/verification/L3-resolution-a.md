# L3 resolution a: plan.md §1, §3, §5; ledger 00, 01, 02, 07; unowned files

Scope of this reviser: `plan.md` §1, §3 and §5; `ledger/00-schedule.md`;
`ledger/01-baseline-repair.md`; `ledger/02-measurement.md` (text);
`ledger/07-profiler.md`; and the files no other reviser owns
(`ledger/10-slop-cleanup.md` SL-9, `ledger/11-other-defects.md`,
`initiative.md`). Findings of `verify-L3-a.md` and `verify-L3-e.md`, plus
the operator's execution rules sent during this round.

Code state read: engine `21d90588`, then `26626710` (PR-3, committed
during this round); PR-5 work is uncommitted in the main tree.

Counts:
- `verify-L3-a.md` (19): 18 fixed, 1 fixed in part with the rest handed to
  the operator (finding 1).
- `verify-L3-e.md` (29): 14 fixed here, 7 already fixed in the current
  text by another reviser (checked), 8 belong to other owners (listed
  below), 0 rejected.
- Operator rules (5): 5 written into `plan.md` §3.

## verify-L3-a.md

1. **Execution left the schedule.** Fixed in part.
   - `00-schedule.md` now records what ran and what is next: "Rows 0 to 16
     are committed, in the order below." Rows 13a, 13b, 14, 15, 16 carry
     their commits (`8b09ac4a`, `d7f0e381`, `21d90588`, `26626710`); the
     follow-ups are rows 16a to 16h before PR-5: "then CN-17, BR-10, BR-12,
     BR-13, BR-14, BR-15 and the B0 re-check of row 16g, before PR-5."
   - New explicit step, row 16g: "rows 13a, 14 and 15 captured against B0
     through the engine-under-test mechanism, on the CN-17 consumer with
     worktrees of `8b09ac4a`, `d7f0e381` and `21d90588`; each result
     recorded". Labels: rows 13a, 14, 15 "B0 (CN-17), at row 16g"; row 16
     "B0 (CN-17): CN-17's gate at row 16a runs on this commit"; row 13
     "gate+B0, established by CN-17 at row 16a".
   - `plan.md` §3 opening sequence rewritten to the executed order, with
     the reason the earlier checks do not count: "the gate and B0 checks of
     BR-6d and BR-11 ran against an editable install of the main engine
     tree instead of an engine-under-test worktree, and PR-1 and PR-2 have
     no recorded B0 check".
   - A finding against a committed row becomes a fix entry: new contingent
     row, "If a committed engine row fails its re-check at row 16g ... it
     becomes a new `09-numeric-fixes.md` entry".
   - For the operator: "stop the row-16 work" is overtaken (PR-3 is
     committed as `26626710`), and PR-5 is now being implemented in the main
     tree ahead of rows 16a to 16h, which the schedule places before PR-5.
     `decision-log.md` (not editable here) has the execution record; its
     `executed_order` omits M-3 (it ran after BR-11 and is re-recorded at
     `8b09ac4a`) and does not name PR-3's commit `26626710`.
2. **Codecov token.** Fixed. BR-10: "The `coverage` job passes
   `token: ${{ secrets.CODECOV_TOKEN }}` to the Codecov step and keeps
   `fail_ci_if_error: true`." Operator checkpoint in `plan.md` §3: "the
   `CODECOV_TOKEN` repository secret: confirmed when BR-10 runs and created
   before the closing sequence's first push (... if the operator declines
   it, BR-10 removes the Codecov upload instead)". The closing sequence and
   row 99a name the secret; BR-10's acceptance names the coverage job.
3. **Golden comparison on moving labels.** Fixed. BR-10: "A new `golden`
   job runs the two `[golden]` lines ... on `runs-on: ubuntu-24.04` with
   `dtolnay/rust-toolchain@1.94.1`, the platform and toolchain the committed
   goldens were captured with ... Moving that pin is a numeric event: the
   commit that moves it first runs `compare-all` locally on the new
   toolchain and, on any difference, follows M-1's numeric entry procedure".
   M-4a uses the same pinned rustc.
4. **Number-moving fix inside BR-12.** Fixed. BR-12: "a fix that is
   bitwise-neutral ... is made in this entry and pinned by a test; a fix
   that moves numbers becomes a new `09-numeric-fixes.md` entry in the
   numeric block (after `00-schedule.md` row 78a ...). The test that shows
   the defect lands in this entry asserting today's value, with a doc
   comment stating the physical expectation it violates". The test carries
   no planning ID (writing rule), so it does not "link to the entry" by ID.
   Schedule contingent row added. For the 09 owner: any such entry is
   created when BR-12 finds a defect; no entry exists yet.
5. **BR-6d's acceptance.** Fixed on this side; CN-17 text is the 08
   owner's. BR-6d: "The gate and capture of this entry's commit (consumer
   `5c651bc`) ran against an editable install of the main engine tree ...
   they do not count as this acceptance. `08-consumer.md` CN-17 establishes
   it". `plan.md` §3 consumer paragraph: "no gate or capture runs against
   an editable install of `/home/rich/src/HARES`, and the dry-run line of
   step 4 ... is recorded with every run". `02-measurement.md` M-0 item 3
   now says the base on a regenerated schema "is recorded by
   `08-consumer.md` CN-17" (the conditional is gone from this file).
   Items for the 08 owner are below.
6. **Bench compares a push with itself.** Fixed. BR-10: "from the commit
   before the push (`github.event.before`) on a push ... A push that
   creates a branch has the all-zero `before` and no baseline: the job
   takes the existing no-baseline path".
7. **JUnit path.** Fixed. BR-10: "Its artifact upload reads
   `target/nextest/ci/nextest-results.xml`, and the two stray listings drop
   the root `nextest-results.xml` exclusion."
8. **Failure at row 99a.** Fixed. BR-10 acceptance: "A job that fails
   there becomes a fix entry committed before M-4b, and the operator pushes
   again." Same rule in `plan.md` §3 (CI and closing sequence) and the
   schedule.
9. **Heating assertion for a gas home; January cooling.** Fixed. BR-12
   names `hvac_cooling` and `hvac_heating` and adds "the run's gas energy
   (`gas_energy().total_therms`) is above zero". The January cooling is
   triaged by the new `11-other-defects.md` OD-15 (row 16h), see the
   binding-rule entries below.
10. **Ignored tests with every feature.** Fixed. BR-12: "run `cargo nextest
    run --workspace --run-ignored only --no-fail-fast` (default features),
    then once more per feature that a `cfg` on an ignored test or on its
    enclosing module names, with that feature alone".
11. **Census misses repository `tests/`.** Fixed. BR-12's census now covers
    "every repository-level `tests/*.rs` and `tests/*/mod.rs` file is the
    `src_path` of a test target or the `#[path]` of a file that is one",
    with a three-line command; run at `21d90588` it prints exactly the
    seven files BR-12 deletes.
12. **BR-13 scope and `materialize`.** Fixed. "The `defaults_dir` string is
    reported, not compared"; "`materialize` ... refuses a defaults-digest
    mismatch with the same message"; new test
    `materialize_refuses_defaults_digest_mismatch`.
13. **BR-14 dependency, strictness, error tests.** Fixed.
    "`tools/frame_golden/Cargo.toml` gains `chrono-tz = { workspace = true
    }`." Strictness at the root rather than a second parser: "Every
    `Deserialize` type in `crates/hares-tariff/src/types.rs` gains
    `#[serde(deny_unknown_fields)]`", which also closes the same gap in the
    Python `Tariff` JSON constructors (`dl-i08-architecture.config`). Tests
    `manifest_tariff_rejects_invalid_file`,
    `manifest_tariff_rejects_failed_validation` (a `demand_window_minutes`
    outside `validate()`'s 5 to 60) and `tariff_json_rejects_unknown_key`.
14. **BR-15 fixture start.** Fixed: "start `2018-01-01T00:00:00-05:00` (the
    site offset of `G0900090`)", with the weather file and
    `initialization_duration_s = 0` named.
15. **CN-17 drift census overlaps CN-13.** Fixed in the schedule rules:
    "The contract-drift census ... of CN-17 covers every engine row up to
    the commit it gates on (rows 1 to 16, through PR-3); CN-13's covers the
    engine rows after it, from row 16b to row 26." The 08 preamble wording
    is the 08 owner's (below).
16. **Output changes before the consumer block.** Fixed. OD-2 site 6 moved
    into the numeric block as row 94e (the decision log's consumer
    ordering); `11-other-defects.md` OD-2: "site 6 is this entry's second
    commit, in its own row of the numeric block (`00-schedule.md` row
    94e)". FL-2 stays (it is fleet structure, not a numeric fix) and is
    named as an exception with its change recorded: "FL-2 (row 51), whose
    `fleet_mixed_offset` `aggregate` and `covered_weight` products stop
    being empty ... No consumer capture and no consumer-shaped golden reads
    a fleet product". The schedule's "exactly once" rule names OD-2's two
    rows.
17. **Profiler line references.** Fixed, re-cited at `21d90588` (after
    BR-6a, PR-1 and PR-2; the preamble says so): `synthetic.rs:2153`,
    `mod.rs:6786-6795` (the assert block), `:5079` and `:6722` (telemetry
    clones), `benches/rl_episode.rs:19-38` and `:280-306`, `mod.rs:2994`
    (`flush_and_close`), `py_dwelling.rs:722-742`, `mod.rs:1828-1831`,
    `:5726-5730`, `:5795-5796`, `:5811-5812`, `:4215-4231`.
18. **Elidable allocations.** Fixed: "7 `black_box(Box::new(black_box(n)))`
    allocations give a delta of exactly 7 (the outer `black_box` makes each
    box escape ...)". This matches the committed test in `26626710`.
19. **PR-6 Python probe actors.** Fixed: "the test's own `#[cfg(test)]`
    module in `hares-python` defines two `Actor` implementations named
    `"probe_a"` and `"probe_b"` ... adds them through its inner `Dwelling`
    ... under the interpreter that `pyo3`'s `auto-initialize` feature ...
    provides".

## verify-L3-e.md

1. **Nine permanently skipped tests.** Fixed: new `11-other-defects.md`
   OD-13 (row 24d, before PY-7). It puts `vendors/OCHRE` on pytest's path
   (`pythonpath = ["vendors/OCHRE"]`; checked: OCHRE 0.9.2 imports in the
   block's venv), deletes the tests that assert nothing (the five skipped
   `test_observer_diagnostics.py` tests, `test_print_comparison`,
   `test_7day_benchmark`), turns the missing-column skips into failures,
   gives the six fleet tests a committed fixture (`Fleet.from_buildings`
   over `tests/fixtures/resstock/2025.1/bldg0000002`; `from_resstock` cannot
   build a runnable fleet until B-9a), adds the submodule step to CI's
   `slow-python-test`, and empties the census: "the block's `[pytest]` line
   passes with an empty census and its log holds no `SKIPPED` line".
2. **pint cap.** Fixed: new OD-14 (row 24e). pint 0.25 renamed the unit to
   `inch_H2O_4C` with the same density (999.972 kg/m^3, checked in pint's
   `default_en.txt` and `constants_en.txt`); "`register_removed_units(ureg)`
   defines `inch_H2O_39F = inch_H2O_4C`" on OCHRE's registry in every
   HARES-environment module that runs OCHRE; the cap goes. BR-6c's change
   no longer has the escape hatch and states the holds: "Two packages stay
   below their latest release ...: pytest-timeout at 2.4.0 (2.5.0 is yanked
   upstream) and pint ... The pint cap is a suppression the dependency
   decision does not allow; `11-other-defects.md` OD-14 registers the unit
   on the HARES side and lifts it."
3. **review.md V49/V50, initiative.md.** V49/V50 and the reading list were
   already corrected in the current text. `initiative.md` Status fixed:
   "Schedule rows 0 to 16 ... are committed on `feat/output-results`; rows
   16a to 16h ... are next. A row runs only after a verification round over
   it records zero blockers and zero majors in `plan-critique.md`."
4. **Committed entries stating superseded facts.** Fixed, each in the
   entry's own text:
   - BR-1: "turns `[test-observe]` and `[test-observe-profiling]` green".
   - BR-2: "seven lints: five at `ebdb1a17` and two that PR #27 adds ...
     `profile_sim.rs:20` (Task 0 head): two `ptr_arg`".
   - BR-7: "Construction paths that read a fixture's TOML (`from_toml`, the
     synthetic TOML constructors, the parity harness's `config.toml`) set
     `write_output: false`"; "every test at that verbosity also passes an
     `output_path` in a temporary directory".
   - BR-8: "12 ignored test instances under default features and 16 under
     `--all-features`", and the alignment helper keeping its recorder.
   - BR-6c: see 2. BR-6d: "`ochre-next = { path = "HARES", editable = true
     }`".
   - M-1: "`.gitignore:49`"; "updates this module in that entry's commit";
     every as-built fact moved into M-0, M-1 and M-2 themselves (products,
     metrics flattening, the sorted defaults digest, `SIMULATION_KEYS`, RT-4
     for the defaults tree, the census comparison, the 2026-09-22 schema,
     the TMY3 weather years); the section now reads "Entries that change
     the golden format" and keeps only that list.
   - SL-9: "**Fixed by:** `01-baseline-repair.md` BR-2 (`fd23dd80` gates
     the imports and `config` on `profiling` ...); `07-profiler.md` PR-1
     (`d7f0e381`) then deletes the example." The superseded "Fix" line is
     gone.
   - `plan.md` §3 Task 0 step 2: "`feat/output-results`, which had no
     commits of its own, is fast-forwarded to `1d60f478`, the Task 0 head.
     Nothing is pushed: the GitHub merge of PR #27 and the matching
     `initial-implementation` update are operator checkpoints of the
     closing sequence."
5. **`plan.md:1196` serde_json.** Other owner (§2, `12-inputs.md` IN-3).
   The current §2.7 comment reads "IN-3 moves serde_json from its"
   dev-dependencies; IN-3's text is the 12 owner's to confirm.
6. **Blueprint builder API.** Already fixed by the §2 owner: §2.7 defines
   `DwellingBlueprintBuilder` with `freeze(self) -> Arc<DwellingBlueprint>`,
   and `DwellingBlueprint::from_config` returns the frozen blueprint, so
   RT-4's `DwellingBlueprint::from_config(..)?.build()` type-checks.
7. **`ActorTimings` constructor.** Fixed, consistent with §2.9 as the §2
   owner wrote it (`new`, `record`, `timings`). PR-6: "`pub fn new(names:
   impl IntoIterator<Item = Arc<str>>) -> Self` ... `pub fn record(&mut
   self, slot: usize, elapsed: StdDuration, called: bool)` ... `pub fn
   timings(&self) -> &[ActorTiming]`"; the allocation test builds
   "`ActorTimings::new(["probe_a", "probe_b"].map(Arc::<str>::from))`".
8. **FL-5 on `Enum`.** The FL-5 half is already fixed in `04-fleet.md` by
   another reviser ("for `Enum`, the mean until B-11, not the home
   report's `Mode`"). The B-11 half (which fixture holds an `Enum`, which
   goldens change) is the 09 owner's.
9. **OD-2 site 6.** Fixed by moving it (see L3-a 16): row 94e, consumer
   check "delta". For the §2 owner: §2.8 needs no exception sentence now;
   if it lists numeric rows, it should include OD-2's site 6.
10. **review.md fleet reasoning.** Already fixed in the current text
    ("collects every home's full result in fleet entry order").
11. **exploration.md citation.** Already fixed ("From the needs analysis of
    HARES users, cross-checked against `decision-log.md`:").
12. **Non-SI state in plan types.** Other owners (§2 and `09-numeric-fixes.md`
    B-14, or an operator decision).
13. **"CN-1 to CN-11" rule.** Fixed: "The mechanical consumer migration
    (CN-13, then CN-1, CN-5, CN-9, CN-7, CN-6, CN-10, CN-11 and the CN-8
    checkpoint) lands before the first numeric fix ... CN-14, CN-3, CN-2,
    CN-4 and CN-12 follow the numeric fixes they depend on".
14. **B0 labels.** Fixed in the schedule (the legend's parenthetical is
    gone; labels per L3-a 1). The `08-consumer.md` order step 2 sentence is
    the 08 owner's (below).
15. **SL-9 in two rows.** Fixed: "SL-9 (row 3, BR-2's `fd23dd80`)"; PR-1's
    entry already states the deletion. SL-10, added by another reviser
    during this round, is placed: "SL-10 (row 52, FL-3)".
16. **CN-6 leftover.** 08 owner.
17. **CN-3 dependencies.** 08 owner.
18. **BR-10 wording.** Fixed: row 5 "CI runs clippy on all targets and
    features and the default, `dst`, `profiling` and plain-release test
    builds"; row 16b "every cargo line of the block in CI"; the first CI run
    is checked at row 99a ("Row 99a's record holds the first CI run of the
    closing sequence's pull request").
19. **PY-8 test over Python-only keys.** 05 owner.
20. **IN-3 census grep.** 12 owner.
21. **Time-relative wording.** Fixed: "crates.io on 2026-09-29" and "the
    latest release on 2026-09-29".
22. **Memory-file references in RT-1.** Already fixed in `06` ("no
    per-step allocation, search or sort in the step path: `dl-i08-criteria`,
    `dl-i08-architecture.engine_performance`").
23. **Thermal solver.** Fixed, `plan.md` §3 "Scope of the thermal solver":
    "The operator's scope decision (`dl-i08-process.scope` ...) authorizes
    editing the thermal solver, `crates/hares-envelope`, in this initiative
    ... The entries that edit it are ... RT-1 ..., RT-3 and RT-5, ... SL-7,
    ... OR-11 ..., ... PR-3 ... and ... B-8 and B-13; ... IN-7 changes the
    envelope parameters it is given." (entry list checked by grep.)
24. **Ideal-capacity compute rule.** Already fixed in RT-3's Measurement by
    another reviser.
25. **R13 numeric-changes list.** 05 owner (PY-9).
26. **`initialize()` documentation.** 05 owner (PY-9).
27. **Open interpreter question.** Fixed: `plan.md` §5 "One question is
    open: the supported Python interpreter range", with the recommendation
    "keep `>=3.12,<3.14` for this initiative" and its reasons.
28. **Two citations for the same sites.** 03 and §2 owner.
29. **Stale row label in the decision log.** Operator (`decision-log.md` is
    not edited by revisers): `dl-i08-execution.progress` `next:` "M-3 (rows
    10 to 13a)" should name the entries (M-3 is row 13b).

## Operator rules for §3 (sent during this round)

All five are in `plan.md` §3, with commands checked to exist
(`Makefile` target `check-ashrae-reference`; `hares-core` test targets
`bestest`, `parity`, `conditioned_oracle`, `freefloat_oracle`,
`structural_envelope_oracle`, `envelope_oracle`; `hares-physics` targets
`physics_validation_tests`, `solar_parity`; `ruff` in the dev group):
1. "A row that is not a numeric entry is bitwise-neutral ... Any drift
   stops the row; it is never re-baselined inside that row", with the
   listed exceptions.
2. "A numeric entry ... records, at its parent commit and after its
   change, its golden delta ... and the physics comparison suites ... No
   metric may move away from its reference unless the entry names the
   reference ... The entry's reviewer checks this evidence."
3. "A performance row ... records before and after with M-3's method ...
   A regression beyond the noise band stops the row."
4. "every fix goes back to a review by a party that did not write it". The
   plan says "party", not "agent": `dl-i08-process.writing` forbids
   references to agents in these documents.
5. "The review criteria include ... idiomatic Rust and Python,
   maintainability and extensibility ... `clippy::pedantic` is enabled
   nowhere in the workspace ...; a new `#[allow(...)]` or `#[expect(...)]`
   carries a comment stating why ... Python files an entry changes gain no
   `ruff check` finding" (the tree has 438 ruff findings today, so the bar
   is "no more than at the parent, none on a changed line").

## Items for other owners

**08 owner (`08-consumer.md`):**
- CN-17 gates on PR-3's engine `26626710` (schedule row 16a), lands before
  PR-5, and drops the conditional "The base on a regenerated schema is the
  BR-6d gate's if ...": its gate, run through the full mechanism on a
  worktree of `26626710`, records the base skip set on a regenerated schema
  and establishes BR-6d's acceptance (consumer gate and DE trace equal to
  B0 for the upgraded lock). If the trace differs, BR-6d's text now says:
  capture the same consumer against a worktree of `1d60f478`; a difference
  there is the lock (split rule), equality puts the cause in an engine row
  (row 16g order).
- Add row 16g to 08 as the step it owns: capture the CN-17 consumer
  against worktrees of `8b09ac4a`, `d7f0e381` and `21d90588`, compare
  with B0, record each.
- Preamble drift-census assignment: "CN-17 for engine rows 1 to 16 (through
  PR-3, the commit it gates on)"; "CN-13 for the engine rows from row 16b
  to row 26". Order step 1: CN-17 lands after PR-3, not "directly after
  BR-6d". Order step 2: "the consumer at CN-16 runs rows 1 to 12; the
  consumer at CN-17, which includes BR-6d, runs rows 13a to 25 (rows 13a,
  14 and 15 at row 16g)".
- Add the one line of L3-a finding 5: no gate or capture runs against an
  editable install of `/home/rich/src/HARES`; step 4's dry-run line is
  recorded with every run (`plan.md` §3 now states it).
- L3-e 16 (CN-6 "exactly 2 per fit") and 17 (CN-3 depends on FL-5).

**09 owner (`09-numeric-fixes.md`):**
- BR-12, OD-13 and the row 16g re-check can each produce a new numeric
  entry (schedule "Contingent rows"); none exists yet. OD-15 can produce
  OD-16, which lives in `11-other-defects.md`.
- The numeric block now includes OD-2's site 6 at row 94e, under 09's
  rules.
- L3-e 8 (B-11's fixture and golden statement) and L3-e 12 (SI units with
  §2).
- `plan.md` §3 rule 2 asks every numeric entry to record the physics
  suites before and after; 09's preamble may cite it.

**§2 / 03 / 05 / 12 owner:** L3-e 5 (IN-3's `serde_json` move), 12 (SI),
19 (PY-8), 20 (IN-3 grep), 25 and 26 (PY-9), 28 (OR-5 and §2.3 line
numbers). §2.8: OD-2 site 6 is now a numeric-block row (94e).

**Operator:** `decision-log.md` `executed_order` and `dl-i08-execution.progress`
row labels (L3-a 1, L3-e 29); PR-5 is being implemented ahead of rows 16a
to 16h; the `CODECOV_TOKEN` decision at BR-10; the §5 interpreter question.
