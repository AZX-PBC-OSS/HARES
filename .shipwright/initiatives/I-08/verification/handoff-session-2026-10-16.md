# Session handoff: 2026-10-16 (the post-checkpoint continuation)

Start here if this session dies. The engine repo is `/home/rich/src/HARES`,
branch `feat/output-results`, head `38c624bb` at this writing (the rows
below through 85 folded; the row 86 completion agent runs in
`/home/rich/src/HARES-wt/r86-b5`). The consumer is
`~/src/HARES_CALIBRATION_SYSTEM`, `main` at `78f20657`, pushed.

## What this session did (all rows independently reviewed, red-teamed,
folded with ff-only merges, every fold's gates green)

- The checkpoint rows' tail: B-16 triage (CONFIRMED), B-17 (the OS-HPXML
  wet-appliance surface; unavailable periods; checkpoint v4), D-020 (the
  MSHP stage table restored to OCHRE verbatim; the cz5a peak question
  answered: the ER-only window, the backup strip vs OCHRE's ideal fill;
  D-032 the Air Flow Ratio), the conditioned-attic triage (both remaining
  candidates reference-equivalent; the record corrected; D-031 honest),
  OD-57 (the silent-substitution sweep; the skylight rule fixed to the
  OS-HPXML conformance; the boiler's fluid absences bounded and counted),
  rows 25 (the review records leave the tree), 26 (the package rename to
  hares/hares-py), 27 (the consumer follows: the pin bumped, the gates
  green), 28-30b, 30-30a, 31-35, 36-46 (the OR block: the catalogue, the
  frozen row plan, the destinations, the reporting frequency, the batch
  callback, the diagnostics report, the HealthPolicy, PR-4, the Arrow
  Time, the metrics-on-request), 47-50 (OR-9/17, OR-10 the full checkpoint
  resume, OR-15, FL-1), 51-53 (FL-2/3/4), 55-56 (the Arrow capsule, the
  GIL), 57-60 (the catalogue binding, the build identity, the callbacks,
  the Python actors), 61-64 (RT-4/RT-2/IN-1/IN-5), 65-69 (RT-6, IN-2/7,
  IN-3/4, OR-16), 70-73 (RT-5, RT-7/8, CN-1, CN-5), 74-78 (CN-9/7/6/10/11,
  merged and pushed to the consumer's main), 79-85 (B-1a/b/c, B-2/3, B-4).

## Two rulings reserved for the operator

1. Row 97 (CN-4): the calibration losses moved. check_losses (row 74)
   now detects it: the real 13-premise fit's losses sit 2-15 percent
   above their B0 pins, bitwise-reproducible, the cause bracketed to
   `aec2e3f..cf7f2c33` (the strict-input repair's nine re-expressed
   documents + the defaults-tree moves). Rows 72/73 and the dependency
   upgrade exonerated by direct measurement. The re-baseline row runs on
   your word.
2. The consumer's two F7/GEB gate failures and the 8 new B0 paths are
   attributed to the engine's declared weather/warm-up movement (rows
   79-81, 83-84), owned by the same re-baseline row.

## The process rules this session enforced (unchanged, plus the new)

- The standing rules all hold (the review/red-team independence, the
  fold gates, no em dashes, no planning IDs in shipped text, the ledger
  accurate).
- NEW: the host OOMed twice from parallel builds. ONE build-bearing
  agent at a time; CARGO_BUILD_JOBS=2 in worktrees; the full nextest
  suite once per row, targeted tests between steps. The launch guard
  also rejects dispatch prompts shaped like recent ones: reword.
- The dead-session stash lesson: agents park WIP in the consumer's stash;
  the shared checkout's index grew a staged revert twice. Work in
  isolated worktrees; inspect `git status` before trusting any checkout.

## Unfinished at this handoff

- Row 86 (B-5): the completion agent runs in `r86-b5` (28 files in
  flight when the first attempt was cancelled; the agent judges then
  finishes or restarts).
- Rows 87-94: B-6 (null for not-applicable), B-11 (the fleet Mode fold),
  B-7 (the metrics' verbosity independence), B-8 (the fallback removal),
  B-9 (the ResStock weather year), B-10 (the packaged defaults), B-12
  (the fleet local time), B-13 (the opaque-R precedence).
- Rows 95-101a: CN-3, CN-2, CN-4 (the operator's re-baseline), CN-12,
  M-4a/b (the cross-platform agreement), CN-15 (the final pin), the push.
- The stale branches pruned this session: everything not
  `feat/output-results`/`initial-implementation`/`main` was deleted; the
  worktrees are one-per-active-row; disk 273G free.

## Stash rescue record (2026-10-16, nothing lost)

Two agents stashed row 86's in-flight work instead of reporting it; the
shared-state rule is now explicit: an agent never stashes work it did not
create. Every stash is preserved as a patch under /tmp/opencode/rescued/
before any pop or drop:

- `r86-stash0-full-snapshot.patch` (730 KB, 56 files): the first attempt's
  full session including golden re-captures. Still in the engine's
  `stash@{0}`; the pop of its coherent 16-file core is what the row 86
  completion agent builds on.
- `od35-redteam-probe.patch` (6 KB): an old red-team probe leftover on a
  landed row. Still in the engine's `stash@{1}`; droppable once the row
  86 fold is green and a diff against the landed tree shows nothing
  unique.
- `consumer-cn5-wip.patch` (14 KB): row 73's parked WIP, obsolete (row 73
  landed through its branch and the merge). Still in the consumer's
  `stash@{0}`; droppable after the re-baseline row closes, kept until
  then.

## The re-signing record (2026-10-16)

The operator's law: every commit signed; no `commit.gpgsign=false`
bypasses, ever. This session's agents committed unsigned through the
1Password agent's transient failure; both repos' spans were re-signed
and pushed:

- The ENGINE: rows 81-95's commits re-signed twice
  (`rebase --force-rebase` from the last signed commit; the second pass
  caught the span below the first base). The head `d6df89fe`. The rows'
  State cells in `00-schedule.md` name the PRE-re-sign shas for rows
  81-88: those commits' trees are byte-identical to the re-signed ones;
  the mapping: the subjects are the keys (every re-signed commit keeps
  its exact subject and tree).
- The CONSUMER: the three unsigned commits (the row 77/78 merge, the
  envelope routing, the pin bump) re-created signature-only through
  `commit-tree -S`, the trees identical, the chain
  `6f3f65b1..494b5cf3`, force-with-lease pushed.
- The deep history (the engine's pre-initiative checkpoints and the
  consumer's calibration rewrite, ~180 commits) predates the signing
  regime: untouched. The operator owns that decision.
- The stashes: all three dropped after their rescue patches landed in
  `/tmp/opencode/rescued/` and their content was verified landed or
  obsolete. The stash rule stands: an agent never stashes work it did
  not create.

## The closing sequence's state (2026-10-17)

- The engine `feat/output-results` at `92b1b80a`, pushed: the full
  schedule drained through row 99 (M-4a: the reproducibility workflow
  lands UNRUN by design: macOS arm64 needs a triggered run).
- The consumer `main` at `48547ac1`, pushed: the final pin (row 101)
  advanced and signed; every gate green on the pinned engine (383/0/7,
  ruff, the DE-trace twice identical).
- Row 100 (M-4b: the cross-platform publish) waits on a
  `workflow_dispatch` run of `.github/workflows/reproducibility.yml`
  (the branch is pushed; dispatch on GitHub or `gh workflow run`).
- Row 97 (CN-4: the re-baseline) is THE open operator ruling: the final
  B0 compare shows 275 differing paths (the fitted vectors, the losses,
  the scores: the accumulated declared movement from the strict-input
  repair, the weather/warm-up conventions, the declared-R precedence and
  the fit-path changes, every family attributed in the rows' records).
  The proposal: `verification/cn4-rebaseline-proposal.md`.
- Row 101a (the push): the consumer's main is pushed through the pin;
  the row's remaining formality: the operator's `i08-consumer` branch
  push per plan.md §3 if that branch still lives.
- The closing PR (BR-10): feat/output-results → initial-implementation,
  the same shape as PR 34. Opened on the operator's word.
