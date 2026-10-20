# L5-e resolution

Resolves every finding in `verify-L5-e.md` (3 blockers, 13 majors, 11 minors).
Each finding below was re-checked against the current text of its target
file before any edit, since BR-20 (row 16a1), OR-19 (row 21b at the time)
and RT-3a (row 21a) had already landed during the audit. All held as
described unless noted otherwise.

## Blockers

### B1. SL-9 orphaned entry

Held. `ledger/00-schedule.md` row 3 (BR-2) and row 14 (PR-1) now carry
"(fixes SL-9)" in the Entry column, matching the "SL-9 (rows 3 and 14,
BR-2's `fd23dd80` and PR-1's `d7f0e381`)" wording added to the preamble's
"Entries closed inside another row" list.

### B2. Preamble's "rows 16a to 16h" omits 16a1 (BR-20)

Held. The preamble (`ledger/00-schedule.md`) was rewritten to name
"CN-17 (row 16a), BR-20 (row 16a1), and rows 16b to 16h" explicitly, in
both the opening paragraph and the "Rules the order follows" bullet.

### B3. Row label "21a" assigned to two entries (OR-18 and RT-3a)

Held. RT-3a keeps row 21a. OR-18 is renumbered to row 21b and OR-19 to
row 21c; the table rows were reordered so RT-3a (21a) sits directly
after RT-3 (21), before OR-18 (21b) and OR-19 (21c). Every reference to
the old "row 21a" (OR-18) and "row 21b" (OR-19) was updated: `decision-
log.md`, `plan.md`, `plan-critique.md`, `ledger/03-output-reporting.md`,
`initiative.md` and `implementation.md`. `ledger/06-runtime-memory.md`'s
own "row 21a" citation (RT-3a's own row) needed no change.

## Majors

### M1. `stray_outputs` contradicts the actual quarantine mechanism

Held. `decision-log.md`'s `stray_outputs` key, `plan.md`'s operator-
checkpoint bullet ("quarantining the stray ... files ... to
`target/stray-quarantine/`") and `ledger/00-schedule.md`'s Task 0 title
("list and quarantine the stray files") now all name quarantine, not
removal.

### M2. Zero-blocker gate overridden without being recorded

Held. `decision-log.md`'s `dl-i08-process.readiness.status` field is
rewritten in place to state the current rule: execution is not gated on
verification rounds (operator ruling, 2026-10-02); findings are fixed as
they land rather than blocking the next row. No history of the old gate
text is kept.

### M3. `readiness.next` never names RT-3, OR-18, OR-19

Held. `decision-log.md`'s `next` field now reads "Still owed: RT-3 (row
21, in progress) and RT-3a (row 21a, in progress); OR-18 (row 21b) and
OR-19 (row 21c, being implemented on branch i08-fixes); rows 16a to
16h ...".

### M4. plan.md §3's execution-order paragraph predates OR-18/OR-19

Held. `plan.md`'s order paragraph now names RT-3 (row 21), RT-3a (row
21a), OR-18 (row 21b, on branch `i08-fixes` for OR-19) and OR-19 (row
21c), then CN-17, BR-20 and rows 16b to 16h, then row 22 onward.

### M5. Audit-scope statement contradicts the schedule

Held, resolved by widening the schedule to match plan.md rather than
narrowing plan.md: `ledger/00-schedule.md`'s preamble now states "every
committed row, with no exception for rows 0 to 18, gets an independent
audit and a red team," matching `plan.md:1907`'s "every committed row"
claim. The verify report's suggested correction (narrow plan.md to "row
19 on") is not what was applied; both documents were made to state the
same, wider scope instead, since that is the scope `plan.md` has stated
since before this round and no decision was found narrowing it.

### M6. implementation.md severely stale

Held. Rewritten: title now reads "I-08 rows 0 to 20 (through OR-11 and
SL-7); RT-3 and RT-3a in progress". The engine and consumer commit
tables are rebuilt from `git -C /home/rich/src/HARES log --oneline`
(HEAD `7b7a6a7d`, `feat/output-results`) and `git -C
/home/rich/src/HARES_CALIBRATION_SYSTEM log --oneline` (HEAD `5c651bc`,
`i08-consumer`), covering every row through SL-7. The "Verification
block state at HEAD" section now cites `7b7a6a7d` and references
`verification/audit-OR-11-b8195c83.md`, `verification/audit-SL-7-
7b7a6a7d.md` and `verification/redteam-OR-11-SL-7.md`. The stale
"`[advisories]` is red pending BR-6b" Known Limitation is removed: BR-6b
(row 11, `872cf289`) is committed and its own acceptance test is
"`[advisories]` exits 0 with no ignores," so the row's committed state
confirms it is fixed.

### M7. plan-critique.md banner says NOT READY

Held. The banner now reads "implementation proceeding; round L5 in
progress; per-commit audits recorded below".

### M8. BR-10 mislabels CN-17's row

Held. `ledger/01-baseline-repair.md`'s BR-10 dependencies field now
reads "Runs after `08-consumer.md` CN-17 (`00-schedule.md` row 16a)".

### M9. review.md V36 has no confirmed ledger entry

Held: OD-21 is a different parser path (HPXML attribute handling and
`site_location.rs`'s time-zone lookup), not blueprint.rs's fallback.
Confirmed directly in the audit worktree (`/home/rich/src/HARES/
.workstrees/audit-7b7a6a7d`, HEAD `7b7a6a7d`) that
`crates/hares-core/src/dwelling/blueprint.rs:95-102` still has an
out-of-range UTC offset silently become UTC and a DST gap/ambiguity
silently resolved by a non-DST-aware reinterpretation. Added `OD-28` to
`ledger/11-other-defects.md` in the entry format (Problem, Evidence at
`7b7a6a7d`, Change, Acceptance test, Verification, Commit,
Dependencies), a schedule row `24o` in `ledger/00-schedule.md`, and
updated `review.md`'s V36 disposition from "separate fix (input
strictness)" to `11-other-defects.md` OD-28.

### M10. implementation.md:29 "agents"

Held and already superseded by M6's rewrite; the sentence now reads "by
parties that did not write the code".

### M11. Em-dash violations in three verification documents

Held. All prose em dashes in `verification/OR-11-stopped-rows-
research.md` (27), `verification/audit-OR-11-b8195c83.md` (15) and
`verification/audit-SL-7-7b7a6a7d.md` (4 of the 5 originally cited
lines; line 22 carried none) were replaced with commas, semicolons or
colons. The one literal em dash inside a quoted shell command in
`audit-SL-7-7b7a6a7d.md:159` (verifying no em dashes were added) is
correctly left untouched. A full-directory sweep after this pass found
further em dashes outside the three named files, in `flow.yaml` (11),
`verify-L1-a.md` (1, a literal inside a grep pattern searching for em
dashes, left untouched), `verify-L5-a.md` (2), `verify-L5-b.md` (9),
`verify-L5-c.md` (4) and `verify-L5-c2.md` (2), and in `verify-L5-e.md`
itself (the finding document's own prose, 13). All of these were fixed
as well, since the task's writing-rule instruction covers "any other
initiative file". The directory now has zero em dashes outside the two
literal, correctly-exempt shell-quote occurrences.

### M12. "session scratchpad" references

Held. Rewritten in `verification/verify-L1-a.md:7`, `verify-L2-a.md:14-
15`, `verify-L3-d.md:11`, `verify-L4-a.md:8,10` and `L3-resolution-
d.md:17` to describe the method without naming a scratchpad or session
("an isolated `git archive HEAD` copy", "isolated scratch copies", "an
isolated copy"). One further scratchpad reference found in the same
sweep, `verify-L5-d.md:63`, was fixed the same way ("output redirected
elsewhere").

### M13. "the implementer's session" references

Held. `verification/L2-integration.md:15` and `:250` and
`L2-resolution-a.md:396` are rewritten to defer to specific rows'
commits instead of a session ("left for their own rows' commits",
"Deferred to BR-6c's and BR-6d's own rows").

## Minors

1. Five `dl-i08-process` sub-rules never cited by tag. Fixed: added
   `(decision-log.md dl-i08-process.branch)`, `.independence` and
   `.writing` at `plan.md`'s "Branches", "Independence" and "Writing"
   headers; `.plan_form` at `plan.md`'s opening paragraph; `.documents`
   at `initiative.md`'s "Documents" heading.

2. Seven `dl-i08-architecture` sub-decisions have no plan.md §2
   citation. Fixed: added a paragraph at the end of `plan.md` §2 naming
   all seven (`consumer_gate`, `unavailable_periods`,
   `wet_appliance_hot_water`, `setpoint_sources`, `battery_control`,
   `consumer_inert_override_probe`, `cross_platform_measurement`) and
   stating they are covered at the ledger-entry level.

3. review.md V38 / OD-2. No longer held: OD-2's body (item 5) names
   `resolve_water_heater.rs:131-141` and the UA-error-to-default
   fallback explicitly, confirming the claim. No edit needed.

4. review.md V22 / B-2. No longer held: B-2's Problem list (item 3)
   states "`initialization_duration` is read only for being `Some`; its
   value is ignored, and the maximum is the literal 25," and its Change
   section replaces the value-ignoring path with a configured
   `Warmup`/`WarmupConfig`. B-2 does confirm V22's specific claim. No
   edit needed.

5. exploration.md RL-7/FL-10 map to files, not entry IDs. Fixed:
   `exploration.md`'s requirement-mapping table now cites `PR-3` for
   RL-7 (confirmed as the entry containing `benches/rl_episode.rs`) and
   `FL-1` for FL-10 (confirmed as the entry containing `benches/
   fleet.rs`'s `fleet_fold` peak-RSS measurement at 100, 1 000 and
   10 000 homes).

6. Census disposition gap at `03-output-reporting.md:1123`. Fixed: the
   `hvac/helpers.rs:59-90` row's disposition cell now states the two
   separate dispositions (water-heater ambient-source resolution,
   dehumidifier conditioned-zone resolution) rather than one shared
   line, per OR-18.

7. initiative.md Status section names outstanding work generically.
   Fixed: now names OR-18 (row 21b) and OR-19 (row 21c, branch
   `i08-fixes`) by ID alongside the generic description.

8. flow.yaml em dashes. Fixed as part of M11's directory-wide sweep (11
   instances, all in `description`/`inject`/`note` strings).

9. Rule-name citations containing "agent" (`max_5_agents`,
   `opencode_stateless`). No longer held as a defect: these quote the
   literal rule identifiers verbatim in process-compliance checklists
   (`verify-L1-e.md`, `verify-L2-e.md`, `verify-L3-e.md`,
   `verify-L4-e.md`), which the writing rule's own exemption for
   verbatim rule names covers. No edit made.

10. `dl-i08-architecture.fleet` single tag citation. No longer held as a
    gap: the dedicated `ledger/04-fleet.md` file's coverage is
    unambiguous by content; confirmed, no edit needed.

11. Ledger files observed changing during the audit. Addressed: this
    resolution pass re-read `ledger/00-schedule.md` and
    `ledger/01-baseline-repair.md` fresh before editing (per this
    file's opening note) and confirmed BR-20/row 16a1 and OR-19/row 21b
    (now 21c) were present and accounted for in the B1-B3 fixes above.

## Counts

- Fixed: 23 (3 blockers, 13 majors, 7 minors).
- No longer held: 4 (minors 3, 4, 9, 10).
- Rejected: 0.
