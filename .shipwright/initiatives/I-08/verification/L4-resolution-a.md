# L4 resolution (a): plan.md 1, 3, 5; the schedule; 01, 02 records, 07; CN-17 and row 16g run order

Scope: every finding of `verify-L4-a.md`, and findings 1, 3, 12, 13 and
minors 19 to 24, 26 and 27 of `verify-L4-e.md`. Binding: `decision-log.md`
(`dl-i08-process.readiness`, `executed_order` and `next`). Code read at
`afa9a071` and the consumer at `5c651bc`, read-only.

The binding run order, stated the same way in every place below: OR-11
(row 19) finishes and commits first, because its uncommitted work is in
the working tree; then rows 16a to 16h in order (CN-17 gated on OR-11's
commit with its census over rows 1 to 19, BR-10, BR-12 to BR-15, the row
16g re-check, OD-15); then row 20 onward after a clean verification round;
row 94c after its own round over B-14.

Counts: 27 findings. 26 fixed, 1 routed in full, 0 rejected.

- verify-L4-a: 15 findings; 14 fixed, 1 routed to the `08-consumer.md`
  owner (finding 13).
- verify-L4-e: 12 findings; 12 fixed. Three carry a part another owner
  must apply: finding 1 (the stale `readiness.status` in the decision
  log), finding 3 (the refusal test for BR-19, in CN-17) and finding 12
  (the `[writing]` line in the consumer gate block).

## verify-L4-a

**1 (blocker), run order.** Fixed everywhere it is stated.
- `plan.md` §3 "Opening sequence": "The order from there is fixed
  (`decision-log.md` `dl-i08-process.readiness`): OR-11 (row 19) finishes
  and commits first, because its uncommitted work is in the working tree
  and no other row can commit while it is there; then rows 16a to 16h, in
  order: CN-17 (gated on OR-11's commit, its contract-drift census over
  engine rows 1 to 19), BR-10, BR-12, BR-13, BR-14, BR-15, the B0 re-check
  (row 16g) and OD-15 (row 16h); then row 20 onward, which starts only
  after a verification round over rows 20 onward records zero blockers and
  zero majors in `plan-critique.md`."
- `00-schedule.md` preamble, first rule and table: row 19 moved directly
  after row 18 (number kept), consumer check "B0 (CN-17): CN-17's gate at
  row 16a runs on this commit", state "in progress"; row 18 "B0 (CN-17),
  at row 16g"; row 16a "gate+B0, on OR-11's engine", census "engine rows 1
  to 19"; CN-13 rule "covers every engine row after OR-11's commit, up to
  row 26: rows 16b to 16h and 20 to 26".
- `01-baseline-repair.md` preamble: "Then `03-output-reporting.md` OR-11
  (row 19) finishes and commits, `08-consumer.md` CN-17 (row 16a) lands
  gated on OR-11's commit, and BR-10, BR-12, BR-13, BR-14 and BR-15 follow
  in that order".
- `07-profiler.md` preamble: "all five are checked in the B0 re-check step
  (row 16g) ... That step runs after `03-output-reporting.md` OR-11 (row
  19) commits and `08-consumer.md` CN-17 (row 16a) lands gated on OR-11's
  commit."
- `08-consumer.md` preamble item 1 ("CN-17 ... lands directly after OR-11,
  gated on OR-11's commit (`00-schedule.md` row 16a), before any further
  engine row and before the next consumer gate"), item 2, the census
  assignment ("rows 1 to 19 (through OR-11 ...)"; CN-13 "rows 16b to 16h
  and 20 to 26"); CN-17's gate bullet ("on a worktree of OR-11's
  commit"), census bullet ("rows 1 to 19, on OR-11's commit") and
  Dependencies ("Position: directly after OR-11 (row 19), the latest engine
  row committed before it, gated on OR-11's commit"); the B0 re-check
  section.
- `02-measurement.md` records list: rows 19, 16a and 16g rewritten.
- `initiative.md` Status: "Row 19 (OR-11) is in progress in the working
  tree; it finishes and commits first. Rows 16a to 16h follow, in order".

**2 (major), the CN-17 re-gating rule.** Fixed by stating the decision
log's rule once, with its only instance under the binding order. Schedule:
"CN-17 gates on the latest engine row committed before it, which under the
order above is OR-11 (row 19)". The contingency for another row
committing first is removed from the schedule, CN-17's Dependencies and the
08 preamble: the binding order leaves no row between OR-11 and CN-17, so
the rule could only mislead. Row 18 (`afa9a071`), which lost CN-17's gate,
is in row 16g's worktree list and in the 02 record for row 16g.

**3 (major), BR-14 strict parsing misses the period types.** Fixed. Change:
"Every `Deserialize` type in `crates/hares-tariff/src/types.rs`, and
`TouPeriod`, `TimeWindow`, `DistributionKind` and `SeasonalSplit` in
`crates/hares-types/src/schedule.rs`, gain `#[serde(deny_unknown_fields)]`
... Every committed input any of these readers parses (tariff JSON,
equipment and actor TOML and JSON, test strings) is checked for extra
keys, and each one found is corrected in this commit." `DistributionKind`
(a window's `noise`) is added beyond the report's three because a key
inside it is equally dropped. Acceptance: "one extra key placed, in three
separate cases, at the top level, inside its `tou_schedule` period and
inside that period's `schedule` window; each fails naming the key." The
commit message becomes `fix: read tariff files strictly and give golden
manifests a tariff and a month-boundary billing fixture`, since the entry
changes parsing behaviour.

**4 (major), BR-12 cannot list the shadowed ignored test.** Fixed: "In the
archive copy only, a temporary `[[test]] name = "resstock_smoke_shadow"`,
`path = "tests/resstock_smoke.rs"` is added to
`crates/hares-core/Cargo.toml` and that target is run with `-F observe
--run-ignored only`. Its result, or its compile error, is the recorded
result for that test". The acceptance names it.

**5 (major), BR-12's gas accessor.** Fixed (the frame column, read with the
file's own helper): "The engine's `SimulationMetrics` carries no gas total,
so the gas energy is the sum of the output file's `Total Gas Power
(therms/hour)` column, read with the file's `parse_csv_columns` helper,
times the 1 h step." End uses are read from
`SimulationResults.metrics.total_energy_kwh.per_end_use`.

**6 (major), rows 1 to 12 and the re-check's attribution.** Fixed in
`plan.md` §3, the schedule rule, the 08 preamble and the row 16g section:
"BR-11 changed only `tools/frame_golden`, so the engine at `8b09ac4a` is
row 12's and its capture is the cumulative check of rows 1 to 12. A
difference there is bisected over the engine rows' commits
`1d60f478`..`75a182a0` (worktrees, in row order) before any row is named".
Row 16g's acceptance: "no row is named before that". The schedule's
consumer check for rows 1 to 12 reads "B0 (CN-17), at row 16g,
cumulative"; `01-baseline-repair.md` BR-6d's text routes an engine-caused
difference to the same bisection.

**7 (minor), commit subjects of rows 16b to 16f.** Fixed: each row carries
its entry's commit message (BR-14's as changed under finding 3).

**8 (minor), multi-commit rows and missing hashes.** Fixed with a State
column: every committed row carries every hash, e.g. row 1 "committed
`aa22be6c`, `c88225df`", row 4 "committed `dc033cee`, `469ea955`", and the
found-defect commits have rows of their own (verify-L4-e 3).

**9 (minor), no log record for PR-5 and PR-6.** Fixed in the records list:
"Rows 17 and 18 (`07-profiler.md` PR-5 `1d471677` and PR-6 `afa9a071`),
appended from the implementer's row records before row 19 commits: the
`[tools]` line and the verification block's result at each commit, each
record stating that the row's B0 check is the row 16g re-check." The
records themselves are the implementer's (item for others, below); writing
them here without the run's data would invent results.

**10 (minor), PR-4's open details.** Fixed: `finish` is "timed around the
first, finalising call of `finish()` ... Later calls ... leave `finish` as
the first call measured it"; the adapter reads the summary after
`finish()` and the profiling line at `cli.rs:695` prints `finish`; a
second `finish()` is asserted to keep the value; `profiling_summary_reports_finish`
pins `finish_s`; Verification adds `[clippy]` and `[pytest]`; PY-9 added
to Dependencies.

**11 (minor), where BR-14 reads the tariff file.** Fixed: "`GoldenManifest::parse(path,
text)` has no repository root, so it checks the table's shape ... and
parses `zone`. One function, `GoldenManifest::tariff(&self, root: &Path)`,
reads `file` ..., runs `validate()` ...; `GoldenManifest::load` calls it
with `repo_root(path)` ..., and the adapter calls it with the run's
repository root". The acceptance names which tests call `parse` and which
call `load`, and the latter use an absolute tariff path in a `tempfile`
directory.

**12 (minor), BR-13's replacement file.** Fixed: "The replacement source is
an absolute path in a `tempfile` directory (a copy of the `defaults/` file
it replaces, then changed by one byte), which `repo_path` returns as given
(`Path::join` with an absolute path), so the test writes nothing into the
tree and edits no committed file."

**13 (minor), CN-17's contract-check test patch target.** Routed to the
`08-consumer.md` owner: this edit is inside CN-17 but not part of the run
order, which is the only CN-17 text this resolution may change. Text to
add to CN-17's Change and its
`contract_check_resolves_the_tree_the_gate_resolves` case: "The script
imports `paths` flat (as it imports `hares_contract`) and reads
`paths.HARES_REPO` at call time; the test patches `HC.paths.HARES_REPO`
(the module object the script holds), not `calibration.paths.HARES_REPO`."

**14 (minor), BR-12's test count.** Fixed: "Every test line the block has
at the parent commit (OR-11 deleted `[test-release-invariants]`) runs four
more tests than at the parent, plus one per defect pinned in this entry."

**15 (minor), BR-10 at its parent; JUnit report overwritten.** Fixed:
evidence "at `afa9a071` ... This entry's parent is OR-11's commit (row 19),
which edits the release `check_invariants` run at `ci.yml:155` ...; the
implementer re-reads the lines below at that commit before starting";
"Each nextest step is followed by a step with `if: success() ||
failure()` that copies `target/nextest/ci/nextest-results.xml` to
`target/nextest/ci/junit-<run>.xml` ... The artifact upload reads
`target/nextest/ci/junit-*.xml`". OR-11 added to Dependencies.

## verify-L4-e

**1 (blocker), opposite orders.** Fixed as verify-L4-a 1, choosing (a), the
decision log's order. The stale `readiness.status` sentence is in
`decision-log.md`, which this resolution may not edit: item for others.

**3 (major), schedule against git; found-defect commits without entries.**
Fixed.
- State column on every row: `committed` with every hash, `recorded`,
  `done` (Task 0, "local merge `1d60f478`"), `in progress` (row 19);
  "(executed)" removed from row 3a; hashes removed from titles.
- New entries in `01-baseline-repair.md`, each with Kind, Problem,
  Evidence, Change, Acceptance test, Verification, Commit, Landed and
  Dependencies, and an executed row: BR-16 (`4a450949`, row 8a: "In a
  plain release build the binding is unused, so the build warns, and CI's
  `test` job, which sets `RUSTFLAGS: "-D warnings"` and runs the plain
  release nextest line, fails to compile"); BR-17 (`59b060d6`, row 9a: the
  round-trip tests' `features = []` manifest rejected under `-F dst`);
  BR-18 (`f880a6d7`, row 10a: "`read_frame` collects one batch per row
  group, so it returns no batch, and `delta` then fails"); BR-19
  (consumer `f7131d7`, `aec2e3f`, row 0e, before B0: "a capture's header
  could name a tree the running engine was not built from"). Row 11 is now
  `872cf289` alone.
- Two of these commits landed without a test that pins them. BR-18's pin,
  `zero_row_product_reads_back_as_its_schema`, is added to BR-13 (row 16d,
  the next golden-tool row). BR-19's pin is routed (below).
- 01 placement: these fixes have no entry anywhere; 01 already holds the
  opening sequence's follow-up fixes and one consumer-repository entry
  (BR-6d), and is owned here.

**12 (major), `[writing]` coverage.** Fixed in `plan.md` §3: the `ids`
pattern adds `DV|RL|RS|X` prefixes, `\bR[0-9]{1,2}\b`, `\bV[0-9]{1,2}\b`,
`\bdl-i08`, `\b[Rr]ound L?[0-9]`, `\bL[0-9]-([a-e]\b|resolution|integration)`
and `\b(LLMs?|Claude|[Ss]ubagents?|AI agents?|coding agents?|language
models?)\b` (the reinforcement-learning "agent" is not matched); added
lines also fail on `HARES#[0-9]+`; the check is one `writing()` function
taking the repository. Added lines under `tests/fixtures/` and lockfiles
are excluded as data, so the broader pattern does not fail on fixture
bytes. Checked against every commit message `1d60f478..afa9a071` (no
match) and every added line of those commits (no match from the new
patterns). The bullet: "Every consumer commit runs the same `ids` and
`writing` definitions as `writing ~/src/HARES_CALIBRATION_SYSTEM` in its
gate", and the §3 consumer summary lists it. Adding that line to the gate
block in `08-consumer.md` is routed (below).

**13 (major), red-team notes without an entry.** Fixed: `11-other-defects.md`
OD-21, appended at the end of the file, row 24h directly after OD-19,
commit `fix: fail the HPXML parse on a malformed attribute and pin
attribute normalization and the time-zone lookup`. It pins quick-xml 0.42's
XML 1.0 attribute and line-end normalization
(`xml_attribute_and_line_end_normalization_follow_xml_1_0`) and the tzf
lookup on a real fixture site
(`site_offset_from_the_time_zone_database_for_a_fixture_site`: `base.xml`
with a ResStock CSV gives -7.0 from `FieldSource::TimezoneLookup`). While
checking the parser, a found defect in the same lines joins it: a rejected
attribute is dropped by `.flatten()` and an unescapable value becomes ""
by `unwrap_or_default()` (`building.rs:366-371`, `:384-389`);
`malformed_hpxml_attribute_is_a_parse_error` pins the fix. The 02 BR-6b
note now ends "Both notes are closed by `11-other-defects.md` OD-21 (row
24h)." The 11 preamble's list of entries that turn silent paths into
errors gains OD-21 (one-word edit, listed below for the 11 owner).

**19 (minor), schedule history.** Fixed: "There is no row 54."

**20 (minor), rows 16b to 16f commit messages.** Fixed (verify-L4-a 7).

**21 (minor), B-14's verification round.** Fixed: row 94c "starts only
after a verification round over B-14 records zero blockers and zero majors
in `plan-critique.md`", the schedule's first rule, `plan.md` §3 and
`initiative.md` Status.

**22 (minor), scratch-deletion checkpoint.** Fixed: "deleting the
consumer's `scratch/fitrun` (`ledger/08-consumer.md` CN-8: 6.2 GB of the
7.0 GB `scratch/` at `c71c542`, re-measured at execution), by that named
directory only". (`context.md` item 19 is outside this resolution's files.)

**23 (minor), thermal-solver scope list.** Fixed: committed OR-5
(`dc033cee`), BR-6a (`39488581`), BR-6b (`872cf289`) and PR-3
(`26626710`) listed; OR-11 is "its `crates/hares-envelope` census rows in
`thermal_solver/`, `boundary_rc.rs`, `state_space.rs`, `fluid_solver.rs`,
`humidity_solver.rs` and `electrical_solver.rs`, the fallible
`DomainSolver` resolve signature every domain solver implements, and the
fluid-loop declarations of its stop record" (checked against the
uncommitted tree's `crates/hares-envelope` diff).

**24 (minor), physics decision record.** Fixed in rule 2: "Each numeric
entry records its physics decision and the reference it rests on in
`docs/PHYSICS_DECISIONS.md` (created by `ledger/09-numeric-fixes.md`
B-1a), in its own commit."

**26 (minor), untested behaviour changes.** Fixed in Per-entry review:
"The reviewer lists every behaviour change the diff makes that no test
pins, with the test file it belongs in; each is a finding."

**27 (minor), block before the commit.** Fixed: "Run ... twice per entry:
on the staged change before the entry commits, and again on the commit
before the next entry starts. The `[writing]` line reads the commit, so it
runs on the second run only." The schedule's green rule says the same.

## `plan.md` §1 and §5

§1 now says the schedule gives "each row's commit message and, for a row
that has run, its state and every commit hash it landed as", and the 01
area line names BR-16 to BR-19. §5 needed no change: no operator decision
is open, since the decision log fixes the order.

## Edits outside the files owned here (all minimal)

- `ledger/08-consumer.md`, run order only: preamble items 1 and 2; the
  census assignment bullets for CN-17 and CN-13; CN-17's gate bullet, its
  BR-6d bullet (one sentence: "This gate's capture is also OR-11's B0
  check."), its census bullet and its Dependencies; the "B0 re-check"
  section (Problem, Change, Acceptance, Dependencies). The rewrite also
  removes the draft history verify-L4-e minor 16 named in those passages.
- `ledger/11-other-defects.md`: OD-21 appended at the end; the preamble's
  list of silent-path entries gains "OD-21". The file was last written at
  15:30, before this resolution; nothing else in it changed.

## Items for other owners

- `08-consumer.md` owner:
  - verify-L4-a 13: the CN-17 text quoted above.
  - BR-19's missing pin: add to CN-17's Change and acceptance
    `suite_scale.capture_refuses_an_engine_built_outside_the_resolved_tree`
    (an `engine_so` outside `paths.HARES_REPO` makes `capture` exit 2 and
    write no file). `01-baseline-repair.md` BR-19 states it lands with
    CN-17.
  - verify-L4-e 12: add to the consumer gate block, after the `ruff`
    line, the `ids` and `writing` definitions of `plan.md` §3 and
    `writing ~/src/HARES_CALIBRATION_SYSTEM   # [writing]`, run on a gate
    that ends in a commit.
  - CN-0's "Landed" line may cite `01-baseline-repair.md` BR-19 for
    `f7131d7` and `aec2e3f`.
- `decision-log.md` owner: `dl-i08-process.readiness.status` still says
  "round L3 re-checks them" (verify-L4-e 1); `dl-i08-execution.progress`
  is a row-8 snapshot (verify-L4-e 15).
- `03-output-reporting.md` owner: OR-11's "Also outside the tables" says
  the commit "removes that job's `-F check_invariants`", which would leave
  a second plain release run in the `test` job; say whether the step is
  deleted. BR-10 is written to work either way.
- `10-slop-cleanup.md` owner: `crates/hares-core/src/actors/ev_driver/mod.rs:6759`
  at `afa9a071` still reads "// Ticket tolerance is <1% drift; ...", a
  planning reference in code (BR-6a re-indented the line without
  rewording it); it needs an SL item for the next entry that touches the
  file.
- Implementer: append the PR-5 and PR-6 records the 02 records list now
  requires, before row 19 commits.
- `implementation.md` is stale (verify-L4-e 42); outside this resolution.
