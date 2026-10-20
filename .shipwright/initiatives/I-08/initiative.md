---
"id": "I-08"
"title": "Runtime, telemetry reporting, memory and performance (HARES#28)"
"status": "active"
"kind": "chore"
"description": "Optimize HARES's runtime, telemetry and output reporting, memory overhead, performance and efficiency, starting from HARES#28 (results without paying for a disk write). Defects found along the way are tracked and fixed as separate ledger entries."
"created_at": "2026-09-26T21:37:14.819Z"
"created_by": "operator"
---

# I-08: Runtime, telemetry reporting, memory and performance

## Purpose

HARES pays for output it is not asked for, reports health and telemetry only
in special builds, loses the tail of stepped runs, and spends a large share of
each simulated step on avoidable allocation and copying. I-08 makes output
and telemetry reporting pay-for-what-you-use, correct for every run mode and
cheap, and removes the measured runtime and memory overhead, without changing
the physics. Every optimization is proven bitwise frame-equal and measured
before and after.

Defects found along the way that are outside this purpose are fixed in I-08
as separate ledger entries, each in its own commit.

## Documents (read in this order, `decision-log.md` `dl-i08-process.documents`)

1. `decision-log.md`: the binding decisions and rules. Authoritative.
2. `plan.md`: the approach (design frame, contracts, execution rules) and
   the index of ledger areas.
3. `ledger/`: the entries, one file per area; `ledger/00-schedule.md` is the
   single execution order and records each executed row's state and
   commits.
4. `context.md`: the problem, the users and the goal.
5. `exploration.md`: what HARES users need and how the current pipeline
   behaves, with evidence.
6. `review.md`: the architecture review and the confirmed defect table.
7. `plan-critique.md`: the verification record; `verification/` holds each
   round's reports and resolutions.
8. `implementation.md`: the implementer's record of executed rows and
   measurements.

## Status

Schedule rows 0 to 21b (through OR-18 `1c18543a`), OD-16 (`fe804b59`),
BR-13 (row 16d, `32044ab5`), BR-14 (row 16e, `c6889589`) and BR-15
(row 16f, `50a5c261`) are committed on
`feat/output-results` and `i08-consumer`; OR-18's red team found
three majors and its audit is running. Fixes are made in worktrees cut
from the engine head, reviewed and red-teamed, and folded back onto
`feat/output-results`: `fix-or18` (OR-18's findings), `fix-or19` (OR-19
ported from `i08-fixes` with its review and red-team findings fixed),
`i08-head-fixes` (BR-21); RT-9 follows. Implementation
proceeds in the implementer's
order; the ledger records each commit as it lands and each commit is
audited independently and red-teamed (`verification/audit-*.md`,
`verification/redteam-*.md`), with findings fixed as they are found.
Owed: rows 16a to 16h (consumer gate corrections,
the follow-up fixes to the opening rows, the B0 re-check, one triage);
and B-14's own verification round before row 94c.
