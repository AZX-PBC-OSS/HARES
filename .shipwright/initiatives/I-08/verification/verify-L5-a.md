# Verification round L5-a

Scope: `plan.md` §1, §3, §5; `ledger/00-schedule.md`; `ledger/01-baseline-repair.md`
BR-10, BR-12, BR-13, BR-14, BR-15 (not yet committed); `ledger/02-measurement.md`;
`ledger/07-profiler.md` PR-4; `decision-log.md` `dl-i08-process`. Checked at
HEAD `7b7a6a7d` in `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`, with
evidence pinned at the commits each entry names (`872cf289`, `afa9a071`) read
with `git show <sha>:<path>`.

Blockers: 0. Majors: 0. Minors: 2. Testing gaps: 1.

## Minor findings

1. **`ledger/07-profiler.md` PR-4, evidence citation has no commit pin.**
   PR-4's Evidence reads "`Dwelling::simulate` calls `flush_and_close()` at
   `mod.rs:2994`", naming no commit. The line number is correct only at
   `21d90588` (PR-2's commit), confirmed by `git show 21d90588:crates/hares-core/src/dwelling/mod.rs`
   line 2994. At every later committed row the line moves: `26626710`/`1d471677`
   line 3004, `afa9a071` line 3064, `b8195c83`/HEAD `7b7a6a7d` line 3146. The
   file's own preamble ("PR-1 and PR-2 cite their evidence at `ebdb1a17` plus
   PR #27... PR-3 to PR-6 cite theirs at `21d90588`") does not mention PR-4,
   so there is no stated pin anywhere for this citation, against the
   convention every other entry in scope follows (BR-10 through BR-15 all
   state the exact commit their `file:line` evidence is read at). Correction:
   add "`21d90588`" ( or the actual parent commit when PR-4 starts, per the
   readiness process that re-verifies rows ahead of execution) to PR-4's
   Evidence line, or add PR-4 to the preamble's citation-commit sentence.
   Not a blocker: the content is correct at a determinable commit and the
   row is scheduled at row 44, far after the current row 21, so the
   stop-before-start readiness process will have a further chance to correct
   the line number before PR-4 executes; still, as written, a reader at HEAD
   checking `mod.rs:2994` will find a different statement (an `rng_seed`
   read) and may wrongly conclude the citation is broken.

2. **`ledger/02-measurement.md` M-0 item 1, `[tools]` line omitted from the
   recorded table.** The verification block (`plan.md` §3) lists `[tools]`
   as the first line (`{ rustc -V; cargo nextest --version; cargo deny
   --version; uv --version; } | tee target/tools.txt`), and `plan.md` §3
   states "`[tools]`: records the toolchain and tool versions the run used.
   A row's record in the `ledger/02-measurement.md` log names them". M-0's
   own table of block-line results (`02-measurement.md` lines 561-581) has
   no `[tools]` row, so the one explicit instance of "a row's record ...
   names them" that should establish the convention does not. Every later
   dated log entry that needs a tool-version comparison (per BR-10's "a
   version that differs from the previous row's is stated with the row that
   first ran under it") has nothing to diff against from M-0. Correction:
   add a `[tools]` row to M-0's table with the Task-0-head `rustc -V`,
   `cargo nextest --version`, `cargo deny --version` and `uv --version`
   output (the raw record `.shipwright/runs/.../m0-block-results.md` likely
   already has it; it only needs to be pulled into the table).

## Testing gap

`BR-10`'s Evidence cites GitHub-side facts this environment could not
independently verify: "PRs #24, #25 and #27 (base `initial-implementation`)
have no CI run after their base was set" and "the repository has no Actions
secret (`gh api repos/:owner/:repo/actions/secrets`: `total_count` 0)" and
the specific failed run id `36028888222`. `gh api` calls against this
repository were blocked by the harness's permission system in this session
(network/credential exploration denial), so these claims are reported as
unverified rather than confirmed; they should be re-checked by whoever runs
BR-10, since BR-10's own Acceptance test already re-derives the secret
presence at execution time ("the operator confirms the secret when this
entry runs") and does not depend on the historical claim being correct.

## What was checked and held (no finding)

- `plan.md` §1 ledger-area index, §3 execution rules (Task 0, opening
  sequence, verification block, golden/measurement/consumer rules, writing
  rules, operator checkpoints), §5 open operator decisions: self-consistent,
  and consistent with `decision-log.md` (`dl-i08-process`,
  `dl-i08-architecture`, `dl-i08-process.readiness`) and
  `ledger/00-schedule.md`'s row table and rules paragraph.
- `ledger/00-schedule.md`: every row number, commit hash and "Consumer
  check" cell cross-referenced against `01-baseline-repair.md` BR-10 through
  BR-15's own Dependencies lines resolves consistently (BR-10 row 16b, BR-12
  row 16c, BR-13 row 16d, BR-14 row 16e, BR-15 row 16f; each entry's "Runs
  after X (row N)" names its own row, not X's row, and that reading is
  consistent across all five entries). Rows 0-20 commit hashes match `git
  log` on the branch. PR-4's schedule position (row 44, after OR-6 row 43
  and PY-9 row 43a) matches its own Dependencies line.
- `01-baseline-repair.md` BR-10: every `.github/workflows/ci.yml` and
  `.config/nextest.toml` citation checked against `afa9a071` by `git show`:
  lines 3-8 (`on:` block), 150-156 (the seven nextest runs), 167-171 (the
  JUnit upload), 249-255 (Codecov step), 271 and 281-283 (the `bench` job's
  `main`-SHA and checkout) all match. The entry's own caveat that OR-11
  (`b8195c83`) deletes line 155 (`check_invariants`) before BR-10 starts was
  confirmed by diff against HEAD `7b7a6a7d`; the entry already tells the
  implementer to re-read lines at the parent commit, so this is not a
  defect. `cargo deny --all-features check advisories`,
  `cargo bench --workspace --no-run`, and `cargo nextest run ... --profile
  ci -F observe,profiling` all parse against the installed tool help output
  (`cargo-deny 0.20.2`, `cargo-nextest 0.9.137`); `rustc -V` on this host is
  `1.94.1`, matching the toolchain BR-10 pins for the new `golden` CI job.
- BR-11: `tools/frame_golden/src/adapter.rs:174,274-276`,
  `crates/hares-core/src/dwelling/mod.rs:2753`,
  `crates/hares-python/src/py_dwelling.rs:2473-2479` all confirmed at
  `872cf289`.
- BR-12: `crates/hares-core/Cargo.toml:50-84` ([[test]] targets),
  `tests/resstock_smoke.rs` shim shape and the seasonal-test line ranges
  confirmed at `872cf289`; the census script in the acceptance test
  (`cargo metadata` / shim-following `comm -13`) was run verbatim at HEAD
  `7b7a6a7d` and printed exactly the seven files BR-12 says it deletes,
  matching the stated behavior at the (later) row 20 parent cited by
  `00-schedule.md`'s rule text ("At `21d90588` it prints exactly the seven
  files").
- BR-13: `tools/frame_golden/src/golden.rs:47-48`, `capture.rs:43-44`,
  `defaults.rs:71-82`, `compare.rs:172-200` confirmed at `872cf289`;
  `compare_products`' signature indeed takes no defaults digest.
- BR-14: `adapter.rs:216-223`, `mod.rs:4152` (`set_tariff`),
  `manifest.rs:53-67` confirmed at `872cf289`; `ElectricTariff`
  (`hares-tariff/src/types.rs:263-306`, `validate()` near `:340`) and the
  four `hares-types/src/schedule.rs` types (`TimeWindow:56`,
  `DistributionKind:175`, `SeasonalSplit:932`, `TouPeriod:1146`) confirmed at
  `afa9a071` to derive `Deserialize` with no `deny_unknown_fields`, matching
  the stated defect exactly. The `month_boundary_billing_3600s` fixture's
  day/row count (12 d at 3600 s = 288 rows) checks out arithmetically.
- BR-15: `manifest.rs:38-51` (`HomeEntry` has no weight field),
  `adapter.rs:282,304-310` (`Fleet::from_buildings`, the step-derived
  resolution rule), `crates/hares-fleet/src/fleet.rs:263`
  (`with_sample_weights`) and `aggregation.rs`'s `AggregationResolution`
  variants confirmed at `872cf289`. The acceptance test's named weather file
  `G0900090_2018.csv` corresponds to an EPW (`G0900090.epw`) whose `LOCATION`
  line states UTC offset `-5.0`, matching BR-15's claimed
  `2018-01-01T00:00:00-05:00` site offset.
- `02-measurement.md`: M-1/M-2/M-3/M-4a/M-4b, the golden-format-change list,
  and the "records the schedule requires of later rows" list are internally
  consistent with `00-schedule.md`'s row numbering and with the entries they
  name (OR-11, SL-7, BR-11 sections already present in the measurement log
  match the schedule's recorded commit hashes). No em dashes, no banned
  tokens (`load-bearing`, `stated plainly`, agent/session/reviewer/ticket
  references in a disallowed sense) found in any file in scope by a direct
  grep; the few matches for "session" are measurement-session language
  ("same-session A/B"), not references to an agent or chat session, and are
  not covered by the writing rule's banned-token list in `plan.md` §3.
- `07-profiler.md` PR-4 (apart from the citation-pin finding above): its
  planned `DwellingProfilingSummary.finish` field, Python `finish_s` field,
  and `emit_dwelling_profiling_summary` function all name real, currently
  existing symbols (`crates/hares-core/src/dwelling/mod.rs:258-283`,
  `crates/hares-core/src/engine.rs:354`, `python/ochre_next/_hares.pyi:53-78`)
  with no field-name collision, and `Dwelling::finish()` correctly does not
  exist yet at HEAD (it is added by OR-6, a dependency of PR-4 that has not
  landed), so PR-4's placement after OR-6/PY-9 in the schedule is sound.
  PR-4's acceptance test correctly avoids asserting a timing value, per the
  "recorded measurements, never timing assertions" rule in `plan.md` §3.
- `decision-log.md` `dl-i08-process` (and its sibling entries
  `dl-i08-process.readiness`, `dl-i08-architecture`): restates the same
  purpose, found-defects, plan-form, readiness, independence, documents,
  writing and branch rules as `plan.md` §1 and §3 with no contradiction
  found.

## Residual risk

The testing gap above (GitHub-side facts in BR-10) is the only unverified
claim significant enough to matter before BR-10 executes; everything else
in scope was confirmed against committed code, installed tool `--help`
output, or the real weather/fixture files. Neither minor finding blocks
execution of any row in scope; both are citation-hygiene corrections.
