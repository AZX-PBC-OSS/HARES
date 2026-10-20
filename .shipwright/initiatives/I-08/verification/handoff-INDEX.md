# I-08 handoff index (2026-10-06)

Start here. The engine repo is `/home/rich/src/HARES`, working branch
`feat/output-results`, head `db5efe80` at handoff. The consumer is
`~/src/HARES_CALIBRATION_SYSTEM`, branch `i08-consumer`.

## Goal

Drain the backlog to checkpoint row **24o**, then fast-forward local
`initial-implementation` (currently `1d60f478`, an ancestor of the head).
Push only on the operator's word. Gates and contents are in
`plan-critique.md`, section "Pause point: commit back to
`initial-implementation`". The fold register in the same file has the full
history of every branch; `ledger/00-schedule.md` has the rows;
`ledger/11-other-defects.md` has the OD entries.

## Rules (non-negotiable; from the operator)

- Nothing is merged knowingly broken. No deferrals, no silent defaults, no `#[ignore]`.
- Every change is implemented by one agent and then reviewed by a different, independent agent. Nobody approves their own work.
- Every committed sha builds and passes on its own; otherwise the row folds as one squashed commit. Golden provenance is re-captured from a clean tree at the folded sha (`ledger/02-measurement.md`, "The fold-process lesson").
- The full `plan.md` §3 block passes on the exact tip before any fold. Fold with `git merge --ff-only`.
- No em or en dashes, ticket IDs or agent references in shipped text or commit messages.
- An OCHRE deviation is triaged per `decision-log.md` `ochre_deviation_triage`. A class (a) deviation gets an entry in `docs/alignment/DIVERGENCES.md` and a pinning test in the same commit.
- References are OS-HPXML v1.12.0 and EnergyPlus v24.2.0, cited by file:line.
- Never run git checkout, stash, reset or revert on the main tree, and never edit its working files.
- Each agent gets a PRIVATE scratch directory. Shared `block.sh` collisions and a scratch wipe cost whole runs this session.
- The disk is tight: delete target dirs after use and watch /tmp.
- At most 5 agents at a time.

## Unfinished work at handoff

Each branch's in-progress fixes are committed as one commit on a branch
`handoff/<branch>` (for example `handoff/fix-od16`). It is cut from that
fix branch's tip and checked out in the same worktree. The fix branches
themselves are unchanged at their tips. Each per-branch handoff doc names
its handoff branch, its SHA, and what is done, partial or not started.
Finish the work on the handoff branch, then squash and reword it into
final commits before any fold. Every commit must build on its own.

## Fold order and state

| # | Branch (worktree `.workstrees/<name>`) | Tip | State | Handoff |
|---|---|---|---|---|
| 1 | `fix-headbreaks` | `cd1b5972`, work in progress on `handoff/<branch>` | review SHIP (`review-headbreaks-cd1b5972.md`); follow-up for F1 (fleet `defaults_path`), F3, F4 and a temp leak was in progress | `handoff-fix-headbreaks.md` |
| 2 | operator `row/od34-tariff`, `row/od35-ev-capacity` | on `db5efe80` | branch-ready, operator folds them after #1 | operator |
| 3 | `fix-testhyg` | `c1aca232` | partial review CHANGES-REQUIRED. At the tip: every block line green, HELICS 50/50 clean, double-free fix confirmed, mutations caught, nothing lost in the rebase. Blocker: commits `fa9699ec`, `11a35f7f`, `f31b6c35`, `0d354790`, `458a9879`, `ac151cc1` do not build (`unique_temp_name` and `TempFile` arrive later in `8ca4fbaa`), so squash or reorder; 22 commits are not yet walked. Two lows. Conflicts with #1 in 6 files (`review-headbreaks-cd1b5972.md`) | `review-testhyg-c1aca232.md` (PARTIAL, Handoff section) |
| 4 | `fix-headaudit` | `4061da95` | its pyright commit is cherry-picked into #1; rebase onto the head and drop the duplicate; needs a final review | fold register row |
| 5 | `fix-od30` | `7b86f9d4`, work in progress on `handoff/<branch>` | review CHANGES-REQUIRED (M1, M2, lows); fixes were in progress; rebase pending | `handoff-fix-od30.md` |
| 6 | `fix-or18c` | `de561181`, work in progress on `handoff/<branch>` | review CHANGES-REQUIRED (pyright stubs, attached-home gable, two untriaged ~22% residuals, orphan duct fixture); `handoff/fix-or18c` at `e3abbe70` is a partial rebase onto `db5efe80` by cherry-pick (23 of 31 non-golden commits; 8 left, listed in the doc; the goldens are dropped and are re-captured at the end; fixup `e55739e0` squashes into `442e404d`). A is fixed but pyright has not run; B, C, D and most lows are not started | `handoff-fix-or18c.md` |
| 7 | `fix-od16` + consumer migration | `160ddb61`, work in progress on `handoff/<branch>` | review CHANGES-REQUIRED (end-use widening for all signals; removal locks the roster, so evict as OR-19 does; a temp leak; `batch_step` not atomic; text); `handoff/fix-od16` = `fix-od16` at `a97fae28` (37 commits on `db5efe80`): every finding fixed with tests and mutations; removal now evicts with a warning counted in `RunHealth::evicted_actors`; Rust block lines all green; pytest had 21 setup errors from a concurrent HELICS run in another worktree, so re-run it, then census, stray, writing, pyright, then the independent review. Open question for the reviewer: a cooling `TurnOff` still lowers BESTEST 600 heating from 1313.5 to 1088.1 kWh, because the sticky `TurnOff` holds into heating. That is the behaviour before this branch; decide whether it is correct. Folds together with the consumer call-site migration listed in `review-OD-16-160ddb61.md` | `handoff-fix-od16.md` |

After each fold: run a fold-integrity audit, then rebase the remaining branches.

## Still to do after the folds (rows up to 24o)

- An independent audit of the operator commits `dc4a456d..db5efe80`.
- The silent-default sweep: OD-48, OD-49, OD-57, OD-36, OD-46. OD-57 and OD-48 gained new sites on 2026-10-06.
- OD-43, OD-52, BR-25, RT-1 (row 22).
- New finds at handoff that are not yet ledgered:
  - `Fleet::from_resstock` points every building at the dataset zip, which the engine never unpacks, and the weather FIPS is always unset, so its fleets cannot build a dwelling.
  - `blueprint.rs` (~117-131) falls back to an empty defaults store with only a warning (OD-49 class).
  - `[bench]` only compiles benchmarks, so a run-time panic in a bench went unseen. Consider a one-iteration bench smoke run in the block.
- Physics: OD-54, OD-41/51, OD-37, OD-38, OD-39, OD-44, OD-47, OD-50/56, OD-61. OD-55 last.
- The checkpoint gates in `plan-critique.md`: the full block, the roll-up report against `1d60f478`, the consumer gate. Then fast-forward `initial-implementation`.
- Clean up the leftover branches after the folds: `fix-or19`, `fold-or19c`, `fold-or19d`, `fix-testhyg-pre-rebase-*`, `or18c-pre-rebase-7b409ca5`.

## Operator-owned

- The helics upstream report (OD-59).
- The `CODECOV_TOKEN` secret.
- Any publication of a local `vendors/OCHRE` fork commit.
- Every push.
