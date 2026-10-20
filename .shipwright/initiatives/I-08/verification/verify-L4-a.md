# Verification L4-a: plan.md 1, 3, 5; BR-10, BR-12 to BR-15; PR-4; CN-17 and the row 16g re-check; the schedule

Scope: `plan.md` sections 1, 3 and 5; the entries of `ledger/01-baseline-repair.md`
not yet executed (BR-10, BR-12, BR-13, BR-14, BR-15); `ledger/07-profiler.md` PR-4;
`ledger/08-consumer.md` CN-17 and "B0 re-check" (row 16g); `ledger/00-schedule.md` in full.

Method: engine code read at HEAD `afa9a071` from an archive copy
(`git archive HEAD`, directory `l4a/`), plus `git show` at older commits;
consumer read at `5c651bc` (`i08-consumer` HEAD) with `git show`. Builds and
probes ran only in isolated copies: a compile and behaviour proof of
BR-14's `deny_unknown_fields` change (`l4a-proof/`), the BR-12 target census,
and the consumer's contract-drift check run read-only against the HEAD
archive. Nothing in either repository was edited except this report.

Counts: 1 blocker, 5 majors, 9 minors.

## Findings

### 1. Blocker: the plan's execution order contradicts the decision log, and the tree cannot run it

**Entries:** `plan.md` §3 (opening sequence), `00-schedule.md` (preamble, rule 1,
rows 16a to 16h and 19), `01-baseline-repair.md` preamble, `07-profiler.md`
preamble, `08-consumer.md` preamble item 1 and CN-17, `02-measurement.md`
"Records the schedule requires of later rows", `initiative.md` Status.

**Evidence:**
- `decision-log.md` `dl-i08-process.readiness`, `executed_order`: "OR-11 (row 19)
  is in progress uncommitted and stopped on the fluid-loop row"; `next`: "Finish
  OR-11 with the recorded fluid-loop resolution (bitwise golden equal), then run
  rows 16a to 16h before any further row: CN-17 (gated on the latest committed
  engine row, census through it), BR-10, ...". The decision log is authoritative
  (`plan.md` §2 preamble).
- `plan.md` §3: "... then CN-17, BR-10, BR-12, BR-13, BR-14, BR-15, the B0
  re-check ... and OD-15 (`ledger/00-schedule.md` rows 16a to 16h), before
  OR-11." `00-schedule.md:11-14`: "rows 16a to 16h run next, and row 19 runs
  after them"; rule at `:19-23` the same. `initiative.md` Status: "rows 16a to
  16h ... are next, before row 19."
- The main tree holds OR-11's work: `git status --porcelain` at `afa9a071`
  lists 66 modified tracked files and the new
  `crates/hares-core/tests/fluid_loop_declarations.rs` (the resolution's
  test). Rows 16b to 16f commit in `/home/rich/src/HARES`; with OR-11's
  uncommitted changes present, each of their verification blocks would test
  the OR-11 tree, not the committed row, and `git stash` is forbidden
  (`plan.md` §3 Branches). The order the plan states cannot be executed
  without inventing a procedure.

**Correction:** adopt the decision log's order everywhere. Move row 19
(OR-11) to directly after row 18, keeping its number, with consumer check
"B0 (CN-17): CN-17's gate at row 16a runs on this commit". CN-17 gates on
OR-11's commit and its drift census covers rows 1 to 19; row 18 (`afa9a071`)
gets "B0 (CN-17), at row 16g"; row 16g re-checks six commits (`8b09ac4a`,
`d7f0e381`, `21d90588`, `26626710`, `1d471677`, `afa9a071`); CN-13's census
starts after OR-11's commit and covers rows 16b to 16h and 20 to 26. Apply the
same text to `plan.md` §3 (opening sequence and the CN-17 paragraph), the
schedule preamble and rules, the 01 and 07 preambles, the 08 preamble
(items 1 and 2, the assignment list), CN-17 (Change bullet "worktree of
`afa9a071`", census bullet, Dependencies), the "B0 re-check" section, the 02
records list (rows 16a and 16g) and `initiative.md` Status. If the operator
instead wants 16a to 16h first, the decision log must say so and name how
OR-11's uncommitted work leaves the tree without `stash` (for example: it is
committed first, or moved to a separate worktree by the operator).

### 2. Major: the CN-17 re-gating rule moves the wrong row's check into row 16g

**Entries:** `00-schedule.md` rule at `:33-40`; `08-consumer.md` CN-17
Dependencies; the same rule in the 08 preamble item 1.

**Evidence:** schedule: "If another engine row commits before CN-17 lands,
CN-17 gates on that row's commit instead, its census extends to that row, and
that row's B0 check moves into row 16g." CN-17: "it lands directly after that
row, gated on it, ... and that row's B0 check joins the re-check step." When
CN-17 gates on the new row, CN-17's gate is the new row's B0 check; the row
that loses its check is the one CN-17 was gated on before (row 18,
`afa9a071`), which row 16g does not list. Finding 1 makes this case the
actual one (OR-11 commits first).

**Correction:** reword both to: "CN-17 gates on that row's commit (its gate is
that row's B0 check), its census extends to that row, and the row CN-17 was
previously gated on (row 18, `afa9a071`) joins the row 16g re-check." Add
`afa9a071` to row 16g's worktree list and to the 02 record list for row 16g.

### 3. Major: BR-14's strict tariff parsing misses the types a tariff period is made of

**Entry:** `01-baseline-repair.md` BR-14 (Change bullet 1; acceptance
`tariff_json_rejects_unknown_key`).

**Evidence:** `ElectricTariff.tou_schedule: Vec<TouPeriod>`, `seasonal_split:
Option<SeasonalSplit>` and `EnergyRate.season: SeasonFilter` are defined in
`crates/hares-types/src/schedule.rs` (`TouPeriod` :1146, `TimeWindow` :56,
`SeasonalSplit` :932, `DistributionKind` :175), not in
`crates/hares-tariff/src/types.rs`, which the entry limits the attribute to.
Compile and behaviour proof in a scratch copy: `#[serde(deny_unknown_fields)]`
on all eleven `Deserialize` types of `types.rs` compiles and keeps
`hares-tariff`'s 184 tests green; parsing a flat tariff then rejects an extra
top-level key and an extra key in an `energy_rates` entry, but accepts an
extra key inside a `tou_schedule` entry and inside its `schedule` window
(`Ok(())` for both). The acceptance test's "one extra key inside a period"
(the flat tariff's one all-hours TOU period) therefore still passes silently
after the change, so the test fails on the fixed code, and the problem the
entry states ("a misspelled key in a tariff file is dropped silently")
remains for every key under `tou_schedule`, `demand_tou_schedule` and
`seasonal_split`.

**Correction:** the change also adds `#[serde(deny_unknown_fields)]` to
`TouPeriod`, `TimeWindow` and `SeasonalSplit` in `hares-types` (none
flattens), after checking every other reader of those types (equipment
schedule TOML and the actor configs that embed `TimeWindow`) for committed
inputs with extra keys, corrected in the same commit. The acceptance test
names the three placements it covers: a top-level key, a key inside a
`tou_schedule` period, and a key inside that period's `schedule` window;
each fails naming the key.

### 4. Major: BR-12's ignored-test run cannot record a result for one of the eleven tests

**Entry:** `01-baseline-repair.md` BR-12 (Change bullet 3; acceptance "a
result per test and feature set").

**Evidence:** at `81cc6d85` (`b674a767^`) the eleven `#[ignore]` attributes
are at `alignment_oracles_regressions.rs:515, 709, 770`,
`crates/hares-core/tests/resstock_smoke.rs:299`, `hares-physics/src/solar.rs:2461`,
`tests/bestest/bestest_diagnostic.rs:114` and `tests/bestest/mod.rs:404, 506,
619, 665, 882` (`git grep -n '#\[ignore' 81cc6d85`). The fourth,
`debug_bldg0000002_winter_heating` (`#[cfg(feature = "observe")]`), lives in
the shadowed file this entry exists to fix: no target compiles it, so
`cargo nextest run --workspace --run-ignored only` (with or without
`-F observe`) never lists it, and the acceptance cannot be met as written.

**Correction:** add to the Change: in the archive copy only, add a temporary
`[[test]] name = "resstock_smoke_shadow"`, `path = "tests/resstock_smoke.rs"`
to `crates/hares-core/Cargo.toml`, run it with `-F observe --run-ignored
only`, and record either its result or its compile error (a compile error is
the result: the test never built). State that this one test is recorded that
way.

### 5. Major: BR-12's gas assertion names an accessor the reused runner does not return

**Entry:** `01-baseline-repair.md` BR-12 (Change bullet 2).

**Evidence:** the entry moves the seasonal tests into the compiled
`tests/resstock_smoke.rs` "reusing that file's helpers", whose runner is
`SimulationEngine::run` (`tests/resstock_smoke.rs:162-194`). Its result is
`engine::SimulationResults` with `metrics: SimulationMetrics`
(`crates/hares-core/src/engine.rs:55`), built from `calculator.finish().metrics`
(`engine.rs:469`), which drops `gas_energy`. `gas_energy()` exists only on
`FullSimulationMetrics` (`crates/hares-io/src/output/metrics.rs:254`),
reachable through `Dwelling::take_streamed_metrics` (`dwelling/mod.rs:4775`)
or a `MetricsCalculator` over batches. The winter assertion as written does
not compile against the runner the entry tells the implementer to reuse.
(The four positivity claims themselves hold: a scratch probe of the four
cases recorded `hvac_cooling` 24.5 and 43.6 kWh in July, `hvac_heating` 7.6
and 2.37 kWh in January, and a nonzero `Total Gas Power` column.)

**Correction:** name the source: run with `retain_batches: true` and compute
`MetricsCalculator` over `result.timeseries`, asserting
`gas_energy().expect(..).total_therms > 0.0`; or sum the frame's `Total Gas
Power (therms/hour)` column times the step length in hours. Pick one in the
entry.

### 6. Major: rows 1 to 12's B0 checks also ran on an editable install, and the re-check attributes a difference to the wrong row

**Entries:** `00-schedule.md` rule at `:24-32`; `plan.md` §3 opening-sequence
paragraph; `08-consumer.md` preamble item 2, CN-17 (BR-6d acceptance bullet),
"B0 re-check".

**Evidence:** the schedule says only "BR-6d's gate and the B0 checks recorded
for BR-11 did not" use the engine-under-test mechanism, and 08 says "The
consumer at CN-16 (`aec2e3f`) ran engine rows 1 to 12" as valid checks.
`implementation.md:17-19` records the passing commands for every opening row
as `HARES_REPO=/home/rich/src/HARES uv run --no-sync ...` (the main tree's
editable install), and `02-measurement.md`'s log holds a B0 check record only
for rows 1 to 3 (":614"); OR-5, BR-4, BR-7, BR-8, M-1, M-2, BR-6a, BR-6b and
BR-6c have none. The row 16g procedure stops "at the first commit that shows"
a difference and blames that row; its first commit is `8b09ac4a` (BR-11),
whose engine equals `75a182a0`'s (BR-11 changed only `tools/frame_golden`),
so a difference introduced anywhere in rows 1 to 12 would be attributed to
BR-11. CN-17's BR-6d bullet routes an engine-caused difference to the same
re-check.

**Correction:** state that rows 1 to 12 have no recorded mechanism-based B0
check, that the `8b09ac4a` capture is their cumulative check (its engine is
row 12's), and that a difference at `8b09ac4a` is bisected over the engine
rows' commits `1d60f478`..`75a182a0` (worktrees, in row order) before any
row is named; amend the schedule rule, §3 and the 08 preamble accordingly.

### 7. Minor: the schedule's commit column omits the follow-up rows' commit subjects

**Entry:** `00-schedule.md` rows 16b to 16f ("see BR-10" .. "see BR-15").
`plan.md` §1 says the schedule gives each entry's commit. **Correction:**
write the subjects: BR-10 `ci: run on initial-implementation and its pull
requests, and add the observe-profiling and golden lines`; BR-12 `test:
compile the seasonal ResStock tests and delete test files no target builds`;
BR-13 `fix: golden comparison fails when the defaults tree differs from the
capture`; BR-14 `test: add a tariff to golden manifests and a month-boundary
billing fixture`; BR-15 `fix: fleet golden manifests state each home's weight
and the aggregation resolution`.

### 8. Minor: committed rows with more than one commit, and rows 1 to 13 without hashes

**Entry:** `00-schedule.md` rows 0b, 1, 4, 8, 9, 11 and the header ("Rows 0
to 18 are committed"). **Evidence:** `git log 1d60f478..afa9a071` and the
consumer log show BR-1 = `aa22be6c` + `c88225df`, OR-5 = `dc033cee` +
`469ea955`, M-1 = `979b8773` + `4a450949`, M-2 = `7643b4e7` + `59b060d6`,
BR-6b = `f880a6d7` + `872cf289`, CN-0 = `824805e` + `f7131d7` + `aec2e3f`
(`decision-log.md` `dl-i08-execution.progress`, br6b-notes). The schedule
lists one subject per row and no hash for rows 0b to 13. Every subject that is
listed matches git. **Correction:** add the hashes of every committed row
(rows 0b to 18), including the extra commits above, so the state is checkable
against `git log`.

### 9. Minor: the measurement log has no record for PR-5 and PR-6

**Entry:** `02-measurement.md` log; `plan.md` §3 `[tools]` ("A row's record
in the `02-measurement.md` log names them"). **Evidence:** records exist for
PR-1, PR-2, PR-3 and BR-11; none names `1d471677` or `afa9a071`.
**Correction:** append both rows' records (tools line, block result), with
their B0 checks marked as deferred to rows 16g and 16a.

### 10. Minor: PR-4 leaves three details open

**Entry:** `07-profiler.md` PR-4. **Evidence:** §2.1 makes later `finish()`
calls return the same `Arc`; a timer "around the whole of `finish()`" would
overwrite the measured value on a second call. The Measurement reads
`finish` for `consumer_shape_900s` through `frame_golden run`, whose
profiling line (`tools/frame_golden/src/cli.rs:695`) and summary read
(`adapter.rs:254`) do not carry the new field. Verification names only
`[test-profiling]` although the change edits the Python binding and
`_hares.pyi`. **Correction:** record `finish` on the first (finalising) call
only; add the field to `frame_golden`'s profiling line and read the summary
after `finish()` in the adapter; Verification adds `[clippy]` and `[pytest]`.

### 11. Minor: BR-14 does not say where the tariff file is read

**Entry:** `01-baseline-repair.md` BR-14. **Evidence:**
`GoldenManifest::parse(path, text)` has no repository root
(`tools/frame_golden/src/manifest.rs:108`), and the `manifest.rs` tests parse
text; reading and validating the repository-relative `file` needs the root.
**Correction:** state that `parse` checks shape and `zone`, and that the file
read, unknown-key parse and `validate()` run in `GoldenManifest::load` with
`repo_root(manifest_path)` (or in the adapter before construction), and which
of the four `manifest_tariff_*` tests call which.

### 12. Minor: BR-13's test needs a replacement file outside the tree

**Entry:** `01-baseline-repair.md` BR-13 (`compare_fails_on_defaults_digest_mismatch`).
**Evidence:** `[defaults_files]` sources resolve with `root.join(relative)`
(`manifest.rs:205-207`). A test that "changes one byte" of an in-repo
replacement writes into the tree (`[stray]`) or edits a committed file.
`Path::join` with an absolute path returns that path, so a temporary file
works. **Correction:** name it: the replacement source is an absolute path in
a `tempfile` directory, copied from `defaults/` and then changed by one byte.

### 13. Minor: CN-17's contract-check test must patch the module the script reads

**Entry:** `08-consumer.md` CN-17 (`contract_check_resolves_the_tree_the_gate_resolves`).
**Evidence:** the gate runs `python calibration/hares_contract_check.py` as a
script (sys.path[0] is `calibration/`; the module already does `import
hares_contract` flat, `hares_contract_check.py:25`), so it can only import
`paths` flat; the suite imports the package module (`from calibration import
paths`, `suite_calibration.py:273`). These are two module objects; patching
`calibration.paths.HARES_REPO` does not reach `_repo()`. **Correction:** state
that the script imports `paths` flat and reads `paths.HARES_REPO` at call
time, and that the test patches `HC.paths.HARES_REPO`.

### 14. Minor: BR-12's test-count statement is not exact

**Entry:** `01-baseline-repair.md` BR-12 Verification ("every other test
line run four more tests"). A bitwise-neutral defect fix made in the entry
adds its pinning test, and `[test-release-invariants]` no longer exists once
OR-11 is committed first (finding 1). **Correction:** "four more tests, plus
one per defect pinned in this entry, on every test line the block still has".

### 15. Minor: BR-10 needs re-verifying at its parent and leaves the JUnit report partial

**Entry:** `01-baseline-repair.md` BR-10. **Evidence:** the evidence lines
(`ci.yml:150-156` naming the release `check_invariants` run at `:155`) are
correct at `afa9a071`, but OR-11 deletes that run (its census row for
`ci.yml:155`), so with finding 1 they shift. All seven `--profile ci` runs
write the same `target/nextest/ci/nextest-results.xml`; the uploaded artifact
holds only the last run. **Correction:** cite the evidence at the parent
commit the row runs on; either accept the last-run report explicitly or give
each run its own JUnit path (`--config-file` override or a copy step after
each run) and upload them all.

## Verified correct

- Every entry in `ledger/01` to `12` appears in the schedule exactly once,
  apart from the declared multi-row entries (OD-2, OD-10, B-14) and the
  entries closed inside another row (BR-5; SL-1, SL-4, SL-5, SL-6, SL-9,
  SL-10) or standing rules (SL-2, SL-3); M-4a and M-4b are one ledger heading
  with two rows; row 54 is intentionally empty. Checked by extracting every
  `## <ID>` heading and every schedule row.
- No entry's Dependencies field names a row scheduled after it: every forward
  reference found is a "lands before" or "precedes" statement and the target
  row is later (scripted over all ledger files).
- Numeric rows (79 to 94i, including FL-5, OD-9, IN-8, IN-9, OD-2 site 6,
  OD-10's numeric commit, B-14's three commits and OD-20) each have their own
  row and commit, all after the consumer block (rows 72 to 78a); the
  contingent B-0, OD-16 and BR-12/OD-13 numeric entries are placed after row
  78a.
- Committed rows 0b to 18: every listed subject matches `git log` in both
  repositories; `8b09ac4a`, `d7f0e381`, `21d90588`, `26626710`, `1d471677`,
  `afa9a071` match their rows.
- BR-10 evidence at `afa9a071`: `on:` at `ci.yml:3-8` lists only `main`; the
  `test` job (`:132-171`) has no `-F observe,profiling` run and no golden
  run, uploads `nextest-results.xml` from the root, and excludes it from the
  stray listings (`:149`, `:162`); `.config/nextest.toml` sets `junit.path =
  "nextest-results.xml"` under `[profile.ci]`; the bench job hard-codes
  `main` (`:271`, `:281`); the Codecov step has no token (`:251-255`). The
  local host is Ubuntu 24.04.5, glibc 2.39, rustc 1.94.1, matching the pin
  BR-10 gives the golden job. The bench targets it names exist on
  `origin/initial-implementation`. Every golden manifest's inputs are tracked
  files (no submodule or download needed in the golden job).
- BR-12 evidence: `crates/hares-core/Cargo.toml:49-91` declares the explicit
  targets; the census command, run on the HEAD archive, prints exactly the
  seven files the entry deletes; the six shims are two-line `#[path]` files;
  the shadowed file's seasonal tests and inputs are as the entry states.
- BR-13 evidence: `golden.rs:47-48`, `capture.rs:43-44` and `:110-144`
  (materialize compares products, metrics and health only), `defaults.rs`
  `digest_tree`, `compare_products` (`compare.rs:172-180`) takes no defaults
  digest. The change is executable as written.
- BR-14: no `types.rs` type flattens a field; `deny_unknown_fields` compiles on
  every type there (unit and data enums included) and keeps the crate's tests
  green; `validate()` rejects `demand_window_minutes = 2`; `chrono-tz` is a
  workspace dependency (`Cargo.toml:56`); the adapter's billing product path
  exists; warm-up runs at construction (`dwelling/mod.rs:2935`), so a tariff
  set after construction bills only the run. The consumer builds tariffs only
  through `from_urdb_json`, whose parser reads `serde_json::Value`
  (`urdb.rs:859`), so the change cannot touch the consumer's DE trace.
- BR-15 evidence: `HomeEntry` has no weight (`manifest.rs:42-58`); the adapter
  uses `Fleet::from_buildings` (weight 1.0) and derives the resolution from
  the step (`adapter.rs:293`, `:315-321`); `with_sample_weights`
  (`fleet.rs:263`) validates and stores the weights the `weights` product
  reads (`adapter.rs:363-381`).
- PR-4: the problem holds at HEAD (`DwellingProfilingSummary`,
  `dwelling/mod.rs:257-285`, has no finalisation field; `flush_and_close()`
  at `mod.rs:3064` is untimed); it runs after OR-6 (row 43) and PY-9 (43a),
  matching its dependencies and §2.1's `finish(&mut self)` contract.
- CN-17 evidence at `5c651bc`: `hares_contract_check.py:27-32` (`DEFAULT_REPO`,
  `_repo()`), `:55-58` (SKIP returns 0), `:142-144` (PASS);
  `suite_calibration.py:264-278`; `paths.py:24-41`; `suite_coverage.py:23-31`;
  `schema.py:508-550`; `capture_de_trace.py` `cmd_compare` diffs only
  `premises`. `results/engine_schema.json` is dated 2026-10-02 06:01. The
  contract-drift check run read-only against the `afa9a071` archive prints
  `[hares-contract] PASS` (21 EndUse variants), so the census over rows 1 to
  18 has no DRIFT item to fix.
- Engine-under-test mechanism: the maturin extraction yields 1.15.0 at all
  six commits the re-check and CN-17 use; `.workstrees/` is gitignored
  (`.gitignore:72`); worktrees for `1d60f478` and `fd23dd80` exist, none
  for the re-check commits yet (an operator checkpoint, `plan.md` §3).
- `plan.md` §3: the physics comparison commands name targets that exist
  (`make check-ashrae-reference`, `--test physics_validation_tests`,
  `--test solar_parity`, the two Python files, `ratchet_baseline` in
  `tests/bestest/mod.rs`); the thermal-solver scope list covers every
  not-yet-executed entry that names `crates/hares-envelope`; operator
  checkpoints cover the PR #27 GitHub merge, `i08-consumer`, the
  `CODECOV_TOKEN` secret (at BR-10 and before the first push), stray removal,
  every push, the pin checkout, worktree creation and removal, SL-8's index
  changes, the cache move, scratch deletion, new consumer dependencies and the
  surrogate trial.
- `plan.md` §5: `requires-python = ">=3.12,<3.14"` (`pyproject.toml:9`), CI's
  `python-test` matrix is 3.12 and 3.13 (`ci.yml:179`), the vendored OCHRE
  declares `>=3.10, <3.13`, and the decision log's `python_support` says the
  same; no operator decision is open beyond finding 1.
