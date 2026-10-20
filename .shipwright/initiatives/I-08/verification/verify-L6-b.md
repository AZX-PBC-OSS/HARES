# Verification round L6-b

Scope: plan.md (S1, S3, S4, S5, S2 consistency), ledger/00-schedule.md,
ledger/01-baseline-repair.md (uncommitted entries, BR-20 in depth),
ledger/02-measurement.md, ledger/04-fleet.md, ledger/06-runtime-memory.md,
ledger/07-profiler.md, ledger/10-slop-cleanup.md, decision-log.md,
initiative.md, context.md, exploration.md, review.md, plan-critique.md,
implementation.md, and the writing rules across the whole directory
including verification/.

Independent pass; verification/ conclusions from other rounds were not
read or relied on. Code at HEAD was read from
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`; the in-progress RT-3 /
RT-3a work was read from `/home/rich/src/HARES` (the main tree, where the
uncommitted diff actually lives) by `git diff`, read only. No destructive
git command was run.

## Counts

- Blockers: 0
- Majors: 1
- Minors: 3

## Findings

### MAJOR: RT-3a is reported as "in progress" everywhere, but no RT-3a code exists anywhere in the repository

Location: `decision-log.md` `dl-i08-process.readiness` `next` (line 363:
"RT-3a (row 21a, in progress)"); `initiative.md` line 47 ("RT-3a (row 21a)
are in progress"); `plan.md` line 1911 ("RT-3 (row 21) and RT-3a (row
21a), already in progress in the working tree"); `implementation.md`
lines 22, 31, 81 (title line, "RT-3 (row 21) and RT-3a (row 21a) are in
progress in the working tree", "21a (RT-3a) are in progress and
uncommitted"); `ledger/00-schedule.md` lines 17 and 39 ("row 21 (RT-3)
and row 21a (RT-3a), in progress in the working tree" / "RT-3 (row 21)
and RT-3a (row 21a),").

Evidence: `git -C /home/rich/src/HARES status --short` and `git diff
--stat` show exactly six modified files, all under
`crates/hares-envelope/`, and every line of that diff implements RT-3
(`SolveScratch`, `output_into`, `ext_temps_save_buf`, the latent-map
round-trip, the solar-slot-map id-sequence compare, the coupled-solve
divisor check at construction, and the rewritten
`thermal_solver_step_allocation_free_after_first_step` test). There is no
trace of RT-3a's change (a per-step cache of the shared `N·x`, `B_eff·u`
and `rhs` prefix across repeated `solve_ideal_capacity_for_target` calls
in the same step, with a sequence-number or `CoupledState`-based
invalidation, and a new `shared_terms_computed_once_per_step` test): `git
diff | grep -n "divisor\|CoupledState\|coupled_solve_divisor_checked\|
shared_terms_computed_once\|solar_slot_map_rebuilt\|
thermal_solver_step_allocation_free"` finds every RT-3 marker and no
RT-3a marker. `git worktree list` shows `rt3-parent` and `rt3-rt`
worktrees with zero diff (`git status --short` and `git diff --stat`
both empty in each), and `i08-fixes` has an unrelated diff
(`dwelling/mod.rs`, `fluid_loop_declarations.rs`, `fluid_solver.rs`, 965
insertions / 392 deletions) that reads as OR-19 work, not RT-3a.

`ledger/00-schedule.md`'s own State column (the authoritative "column
State records each row that has run" field, line 26) gets this right:
row 21 (RT-3) reads `in progress`; row 21a (RT-3a) is blank. The
narrative prose in the same file, and in every other file listed above,
contradicts the table it sits next to.

Correction: in every one of the five locations, replace "RT-3a (row 21a)
... in progress" with language matching the schedule table: RT-3 (row
21) is in progress in the working tree; RT-3a (row 21a) is scheduled
next and has not started. If RT-3a work is actually happening in a
worktree or branch not covered here, that location needs to be named and
pointed to; as checked, none exists.

### MINOR: BR-20 (row 16a1) is not named in either consumer census's row range

Location: `ledger/00-schedule.md` lines 69-73: "CN-17... its contract-drift
census... covers engine rows 1 to 20. CN-13's census covers every engine
row after SL-7's commit, up to row 26: rows 16b to 16h and 21 to 26."

BR-20 (row 16a1) runs after CN-17 (row 16a, SL-7's commit) and before
BR-10 (row 16b), i.e. strictly "after SL-7's commit" by the same test the
sentence uses for 16b-16h, yet it is omitted from the enumerated list.
This may be intentional (BR-20 changes only the toolchain and dev tools,
not engine API surface, so it may legitimately produce no census lines),
but the text does not say so, leaving a reader to wonder whether the
omission is a gap in the enumeration or a deliberate exemption.
Correction: either add row 16a1 to CN-13's covered range, or add one
clause stating why a toolchain-pin commit is exempt from the
contract-drift census.

### MINOR: the "push" row label is reused three times, against the schedule's own uniqueness framing

Location: `ledger/00-schedule.md` rows 99a, 100a, 101a, column "Entry".

The task's row-label expectation is uniqueness; "push" appears
unqualified at three different rows. Each is distinguished by row number
and by file section (lines 328-333 explain these are operator checkpoints
recorded in the measurement log), and this is almost certainly
intentional rather than copy-paste error, but it is the only duplicate in
the table (`awk`-based uniqueness scan over the Entry column: "push" is
the sole entry with count > 1). No correction required if the intent is
confirmed; otherwise each row's push should carry a distinguishing suffix
(e.g. "push (closing sequence)", "push (consumer)") the way `Task 0` and
the other one-off operator steps do.

### MINOR: "session" terminology in measurement text brushes against the writing rule's letter, not its intent

Location: `ledger/01-baseline-repair.md` line 1287 ("same session,
interleaved"); `ledger/02-measurement.md` lines 858, 939, 957, 1026, 1028,
1037 ("same-session A/B", "in one session", "another session's cargo
builds", etc.); `plan.md` line 2122 ("measured in one session from
worktrees").

The writing rule in scope forbids references to agents, sub-agents or
sessions. Every instance found is a legitimate experimental-design term
(a contiguous wall-clock measurement window used to control for host
noise in an A/B comparison), not a reference to an AI coding session or
agent. `plan.md` line 2045 also uses "subagent" and names "LLM" and
"Claude", but that is the `[writing]` check's own specification of the
forbidden tokens, not an instance of the violation. No text in the scope
reads as an AI-session or agent reference in the sense the rule targets.
Flagged only so the distinction is on record; no change needed unless the
rule is meant to be read more literally than its evident intent.

## Checks that passed, with evidence

- Row hashes and order (check 1). `git -C /home/rich/src/HARES log
  --oneline feat/output-results` and `git -C
  /home/rich/src/HARES_CALIBRATION_SYSTEM log --oneline i08-consumer`
  were walked oldest-to-newest and compared against schedule rows 0
  through 20 (engine) and 0b, 0c, 0e, 13 (consumer). Every commit hash
  and every "then" ordering within a multi-commit row matches the actual
  commit order exactly; no row is out of sequence, no hash is wrong, and
  no row cites a commit that belongs to a different row.
- Every ledger-01/02/04/06/07/10 entry appears in the schedule exactly
  once, with the multi-row/standing exceptions the schedule itself names
  (SL-1/4/5/6/9/10/11 closed inside other rows; SL-2/SL-3 standing) and no
  others. Verified by extracting every `## <ID>` heading from each file
  and checking it against the schedule's Entry column.
- Dependencies scheduled earlier (check 1). Spot-checked every
  "Dependencies:" line in `01-baseline-repair.md` (BR-20: BR-6b row 11,
  BR-6c row 12, both before row 16a1), all of `04-fleet.md` (FL-1 through
  FL-5) and all of `06-runtime-memory.md` (RT-1 through RT-8, RT-3a).
  Every cited dependency's row number is lower than the citing entry's
  row number; none found out of order.
- BR-20's command citations (check 4). `rustup check`, `cargo search
  cargo-nextest`, `cargo search cargo-llvm-cov`, `uv self update
  --dry-run`, `cargo deny --version` and `maturin --version` were run as
  written in this environment. `rustup check` returned exactly "stable
  ... update available: 1.94.1 ... -> 1.99.0 ..." and `cargo search
  cargo-nextest` returned exactly "0.9.146" against an installed
  "0.9.137", matching the entry's cited numbers precisely.
- RT-3 citations and claims against code (check 4). `cargo check -p
  hares-envelope --all-features`, `cargo test -p hares-envelope --test
  radiant_gain_weights_zero_alloc --all-features` and `cargo test -p
  hares-envelope --lib --all-features` all pass clean (375 lib tests, 1
  integration test, 0 failures) on the uncommitted working-tree diff.
  The diff implements every item RT-3's "Change" section lists: a
  `SolveScratch` type sized at construction, `output_into` replacing the
  owned `C·x + D·u`, `ext_temps_save_buf` replacing the per-step clone,
  the latent map returned to `latent_buf`, the solar-slot-map id-sequence
  comparison (`solar_slot_map_rebuilt_only_on_surface_change` passes),
  and a construction-time check that every coupled-solve divisor `1 +
  d_i` is finite and positive, with `CouplingCoefficientInvalid` /
  `ConvectionInjectionInvalid` errors and a
  `coupled_solve_divisor_checked_at_construction` test. The rewritten
  `thermal_solver_step_allocation_free_after_first_step` test asserts
  `step_allocs == 0` and `resolve_allocs == 0` over 100 steps, both with
  and without couplings, exactly as the entry's acceptance test
  describes, and passes.
- RT-1's and PR-4's line citations (check 4, sampled for currency).
  RT-1 explicitly scopes its citations to `ebdb1a17`, not `7b7a6a7d`; `git
  show ebdb1a17:crates/hares-core/src/dwelling/mod.rs` at lines 6389-6399
  and 6428-6437 matches the entry's description exactly (the `HashSet`
  rebuild/retain and the `clone_from` comment). PR-4 cites `mod.rs:3146`
  at `7b7a6a7d`; the audit worktree shows `flush_and_close()` at that
  line. Both check out.
- `context.md` item 5's "6.8% of simulate()" figure, cited by RT-3's
  Measurement section, matches `context.md` line 65-69 verbatim.
- Writing rules (check 5). A recursive `grep -P '\x{2014}'` over every
  `.md` file in the initiative directory, including `verification/`,
  found em dashes in exactly three files; all three occurrences are
  inside backtick-quoted shell/grep command text, which the rule
  exempts. No LLM tics, no archaeology phrasing ("originally", "used to
  be", "previously was"; the "legacy" hits found are all technical,
  e.g. OCHRE-legacy exclusion rules and a doc-section title, not
  narration of past agent work), and no `/tmp` or scratchpad paths were
  found anywhere in the directory.
- Decision-log spot checks (check 3). `dl-i08-architecture`'s
  `dependencies`, `engine_performance`, `fleet`, `stray_outputs`,
  `public_planning_docs`, `slop_scope` and `rl_observation_allocation`
  sub-decisions were traced to their entries (BR-6a/b/c/d and BR-20;
  RT-1 through RT-8 and PR-1 through PR-6; FL-1 through FL-5; BR-7;
  SL-8; SL-2/SL-3; RT-7) with no contradiction found.

## Residual risk / testing gaps (not verified false, not verified true)

Given the size of the scope, the following were not individually
line-checked against the audit worktree and should be treated as open
rather than confirmed: the full citation sets of M-4a, M-4b
(`02-measurement.md`); FL-1 through FL-5 beyond their dependency lines
(`04-fleet.md`); RT-2, RT-4, RT-5, RT-6, RT-7, RT-8 beyond their
dependency lines (`06-runtime-memory.md`); the remainder of PR-4 beyond
its single evidence line; and SL-1 through SL-6, SL-8 through SL-11
(`10-slop-cleanup.md`). No command in any of these was found to be run
incorrectly or to cite a stale line where checked, but not every line
citation in every uncommitted entry in scope was individually verified
against the audit worktree. This review also did not re-derive whether
BR-20's proposed toolchain version (1.99.0) will still be the latest
stable release at the time the row actually executes; that number is
necessarily time-sensitive and the entry already accounts for this
("unless a newer stable exists when the row runs").
