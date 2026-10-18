# Verify L5-e: I-08 traceability audit

Independent, read-only traceability verification of the I-08 initiative ledger
(decision-log.md, initiative.md, context.md, exploration.md, review.md,
plan.md, plan-critique.md, implementation.md, ledger/00-12, verification/).
Code checked at committed HEAD `7b7a6a7d` in
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`; ledger/schedule state
checked against the live repository at `/home/rich/src/HARES` (git log,
git status). No files other than this report were edited.

## Counts

- Blocker: 3
- Major: 13
- Minor: 11

Total findings: 27.

Five areas were audited in parallel against the full text of every named
file: decision-log to plan/ledger mapping (57 individual decisions checked);
context.md items 1-21, review.md V1-V50, exploration.md section 1.8, and the
four verification/audit/redteam/research files; ledger/00-schedule.md against
ledger/01-12 and git history; status consistency across initiative.md,
decision-log.md, plan.md, plan-critique.md, implementation.md and the
schedule; and a writing-rules scan of every file in the tree.

---

## Blocker findings

### B1. `SL-9` is an orphaned entry: claimed resolution never appears in the schedule table

**Location:** `ledger/00-schedule.md` preamble (line ~115) vs. row 3 (line ~139); definition at `ledger/10-slop-cleanup.md:369-383`.

**Evidence:** The preamble asserts "SL-9 (row 3, BR-2's `fd23dd80`)". Row 3's actual text is:
`| 3 | BR-2 | clippy clean under \`observe\` | 01 | ... | committed \`fd23dd80\` |`
, with no "(fixes SL-9)" marker, unlike every other "closed inside another row" entry (SL-1, SL-4, SL-5, SL-6, SL-10, SL-11, BR-5 all carry their marker correctly). `grep -n SL-9 ledger/00-schedule.md` finds zero occurrences in the table, only the preamble claim. Worse, SL-9's own ledger definition says it is resolved across **two** commits/rows ("`01-baseline-repair.md` BR-2 (`fd23dd80`)... `07-profiler.md` PR-1 (`d7f0e381`) then deletes the example"), so even the preamble's single-row attribution is incomplete.

**Why blocker:** An implementer or auditor checking "is SL-9 closed" from the schedule table alone finds no trace of it; the only evidence is a preamble assertion that doesn't match the row it points to, and that assertion is itself wrong about how many rows resolve SL-9.

**Correction:** Add an explicit "(fixes SL-9)" marker to row 3 and to row 14 (PR-1), matching SL-9's own two-commit resolution text; or correct the preamble to name both rows.

### B2. Schedule preamble's "rows 16a to 16h" enumeration is stale, omitting the newly inserted row 16a1 (BR-20)

**Location:** `ledger/00-schedule.md` preamble (lines 11, 14, 36-37) vs. table rows 163-171; `ledger/01-baseline-repair.md:1251` (BR-20).

**Evidence:** The preamble names exactly 8 entries for "rows 16a to 16h" (CN-17, BR-10, BR-12, BR-13, BR-14, BR-15, the B0 re-check, OD-15). The table now contains 9 rows in that span, with a new row `16a1` (BR-20, "Rust toolchain pinned at the latest stable...", dependencies "BR-6b, BR-6c; runs before BR-10 (row 16b)") inserted between 16a and 16b. The preamble was never updated after BR-20 was added to the table.

**Why blocker:** This is exactly the class of error the schedule's own stated invariant exists to prevent; an implementer reading only the preamble's row-span summary will not know BR-20/row 16a1 exists or where it sits in the dependency chain.

**Correction:** Update the preamble's enumeration of rows 16a-16h to include BR-20/16a1, or renumber the span description (e.g., "rows 16a to 16h, plus 16a1").

### B3. Row label "21a" is assigned to two different entries (OR-18 and RT-3a)

**Location:** `ledger/00-schedule.md` lines 172 and 174-175.

**Evidence:** `| 21a | OR-18 | the three census rows OR-11 left as logs... | | |` and, two rows later, `| 21a | RT-3a | one shared-term resolve per step for the ideal-capacity solves (RT-3's stop-rule entry, added 2026-10-02) | | |`. Both carry identifier "21a". The schedule's preamble states "a row keeps its number when it moves, so references to it stay valid", implying row numbers are unique stable identifiers. RT-3a's own text notes it was "added 2026-10-02," i.e. inserted after OR-18 already held 21a, and should have received a fresh label (21b is already OR-19, so the next free slot is 21c).

Independently, the status-consistency audit flagged this same duplication and concluded it leaves "the actual next-row order after RT-3 ambiguous (is 21a OR-18 or RT-3a?)".

**Why blocker:** Two independent audit passes converged on this; it directly creates the kind of ambiguity that would make an implementer pick the wrong next row, or conflate two distinct required fixes under one citation.

**Correction:** Relabel RT-3a's row to a unique identifier (e.g. 21c), and fix any text elsewhere that cites "row 21a" meaning RT-3a.

---

## Major findings

### M1. `dl-i08-architecture.stray_outputs` contradicts the actual (infeasible-deletion) mechanism, and downstream documents were never corrected

**Location:** `decision-log.md:307-311` (stray_outputs decision) vs. `decision-log.md:396-397` (dl-i08-execution.task0-state) and `ledger/02-measurement.md:547-550` (M-0 log); also `plan.md:2210-2212` and `ledger/00-schedule.md:131` (Task 0 title).

**Evidence:** The architecture decision and plan.md's operator checklist both say the stray `dwelling_*.csv`/`dwelling_*_diagnostics.csv` files are "listed... then removed." The execution log for the same initiative records that deletion is actually blocked on the implementation host, and the files were "moved to `target/stray-quarantine/`" instead, with `target/` excluded from the stray check. This violates `dl-i08-process.documents`'s own rule that superseded content is rewritten in place: the stray_outputs key, plan.md's checklist, and the schedule's Task 0 title were never updated to reflect that quarantine, not removal, is the real mechanism.

**Correction:** Rewrite `decision-log.md:307-311` to say "quarantined to `target/stray-quarantine/` (deletion is blocked on the implementation host)"; update `plan.md:2210-2212` and the Task 0 row title in `ledger/00-schedule.md:131` to say "quarantine" rather than "remove."

### M2. `dl-i08-process.readiness.status`'s zero-blocker/zero-major gate was overridden without the override being recorded in decision-log.md

**Location:** `decision-log.md:330-339` vs. `plan-critique.md` Round L4 (lines 121-127, 137).

**Evidence:** decision-log states row 20 onward is cleared "only when a fresh verification round over it records zero blockers and zero majors in plan-critique.md." plan-critique.md's Round L4 table shows nonzero blockers/majors in every area, and line 137 records "implementation proceeds (operator ruling)", an explicit gate override. decision-log.md, which initiative.md calls the authoritative record, never records this override.

**Correction:** Add the operator-ruling override to `dl-i08-process.readiness.status` in decision-log.md, citing plan-critique.md's Round L4/L5.

### M3. `dl-i08-process.readiness.next` never names the entries it is actually about (RT-3, OR-18, OR-19)

**Location:** `decision-log.md:348-362`.

**Evidence:** This field speaks only generically of "the OR-11 stopped-row fixes" and "rows 16a to 16h," never using the ledger IDs RT-3 (currently in progress per git status and three other documents) or OR-18/OR-19 (the specific not-started follow-on rows that `plan-critique.md` names explicitly).

**Correction:** Update `readiness.next` to name RT-3, OR-18 and OR-19 by ID.

### M4. plan.md §3's execution-order paragraph is stale, predates OR-18/OR-19

**Location:** `plan.md:1879-1911`.

**Evidence:** Ends at "row 21 onward (row 21, RT-3, already in progress)" with no mention of OR-18 or OR-19, which were created only after plan-critique.md Round L4's "Committed-row audits" findings (`plan-critique.md:169-198`). plan.md was not updated after those rows were added to the schedule.

**Correction:** Extend plan.md §3's order paragraph to mention rows 21a (OR-18) and 21b (OR-19).

### M5. Audit-scope statement in plan.md contradicts ledger/00-schedule.md and actual verification/ contents

**Location:** `plan.md:1907` ("Every committed row gets an audit...") vs. `ledger/00-schedule.md:24` ("Every committed row from row 19 on also names its independent audit...").

**Evidence:** Only rows 19 (OR-11) and 20 (SL-7) have audit files in `verification/` (`audit-OR-11-b8195c83.md`, `audit-SL-7-7b7a6a7d.md`); no audit files exist for rows 0-18, confirming the schedule's narrower claim and contradicting plan.md's unqualified claim.

**Correction:** Correct plan.md:1907 to state the audit requirement begins at row 19, matching the schedule and reality.

### M6. implementation.md is severely stale relative to the repository and to itself

**Location:** `implementation.md:22` (title), `implementation.md:63` ("Verification block state at HEAD (`7643b4e7`)"), commit table lines 32-56.

**Evidence:**
- Title says "schedule rows 0 to 8 plus BR-9," but its own commit table already runs through row ~16 (M-2, `59b060d6`); the title undercounts the document's own body.
- Its self-declared "HEAD" (`7643b4e7`) is *older* than the table's own last-listed commit (`59b060d6`); internally inconsistent.
- Relative to actual git HEAD (`7b7a6a7d`), it omits 12+ commits/rows: BR-6a-d, BR-11, M-3, PR-1 through PR-6, OR-11, SL-7, and the in-progress RT-3.
- It references none of the verification artifacts that now exist and define current status (`audit-OR-11-b8195c83.md`, `audit-SL-7-7b7a6a7d.md`, `redteam-OR-11-SL-7.md`, `OR-11-stopped-rows-research.md`).

**Why major, not blocker:** initiative.md, decision-log.md, plan.md and the schedule independently carry the correct, current status, so an implementer who checks those is not misled, but implementation.md is explicitly the implementer's own live record (initiative.md:41) and currently looks like the project stopped at M-2.

**Correction:** Update implementation.md's title and "Verification block state at HEAD" section to the actual current HEAD and commit table, and add references to the four verification files above.

### M7. plan-critique.md's top-level status banner contradicts its own later sections

**Location:** `plan-critique.md:3-8` ("## Current status: NOT READY") vs. lines 129-139, 141-217 (Round L4/L5, "implementation proceeds (operator ruling)", committed-row audits for rows past Round L1).

**Evidence:** The banner was written at Round L1 and never revised through L2-L5, despite the document's later, current sections describing two further rows committed, audited and red-teamed since. A reader who weights the banner over the narrative gets the opposite of the real situation.

**Correction:** Revise the top banner to reflect current status (e.g. "implementation proceeds under operator ruling; see Round L5"), or remove the banner in favor of a status line that is kept current.

### M8. `BR-10`'s own text mislabels CN-17's schedule row

**Location:** `ledger/01-baseline-repair.md:699-700` (BR-10 "Dependencies" field).

**Evidence:** "Runs after `08-consumer.md` CN-17 (`00-schedule.md` row 16b)." CN-17 is actually row 16a; row 16b is BR-10 itself: the text cites its own row number as if it were CN-17's. Other entries in the same span (BR-12, BR-13) correctly cite their own row when using this phrasing pattern, so BR-10 is the outlier.

**Correction:** Change "row 16b" to "row 16a" in BR-10's dependencies field.

### M9. review.md V36 (UTC/DST ambiguity silent fallback) has no confirmed ledger entry addressing it

**Location:** `review.md` V36 disposition ("separate fix (input strictness)"); candidate `ledger/11-other-defects.md:1473-1557` (OD-21).

**Evidence:** OD-21's stated scope ("HPXML attribute handling and the time-zone lookup have no pinning test, and a malformed attribute is dropped") is the closest content match, but no entry was found that explicitly cites `blueprint.rs:90-97` or names "UTC/DST ambiguity resolves to a silent fallback" by V36's own description. This is the one review defect out of 50 that this audit could not confirm is substantively addressed.

**Correction:** The implementer or next review pass should open OD-21 in full and confirm it covers blueprint.rs:90-97's UTC/DST fallback; if it does not, V36 needs its own ledger entry before it can be marked resolved.

### M10. `implementation.md:29` still reads "by agents that did not write the code," a previously-identified, unfixed writing-rule violation

**Location:** `implementation.md:29`.

**Evidence:** `verification/verify-L4-e.md:389` already identified this exact defect and prescribed the fix ("Write 'by parties that did not write the code'"), but implementation.md still reads "agents." This is a live reference to AI-assistant tooling process in a document that is meant to describe the initiative's engineering process, and it is a known, specified, unapplied fix.

**Correction:** Apply the already-specified fix: replace "agents" with "parties" at implementation.md:29.

### M11. Em-dash violations in three verification documents (47 instances total)

**Location:**
- `verification/OR-11-stopped-rows-research.md`: 27 prose em dashes (lines include 37, 54, 72-80, 93, 101, 116, 127, 143, 159-160, 199, 222, 235, 238, 240, 255, 326, 353, 376). Fixed in this pass; see `L5-resolution-e.md`.
- `verification/audit-OR-11-b8195c83.md`: 15 prose em dashes (lines include 38, 49, 89, 95, 119, 151, 154, 206, 211, 233, 308, 314, 322, 325, 359). Fixed in this pass.
- `verification/audit-SL-7-7b7a6a7d.md`: of the 5 lines originally cited (22, 82, 129, 133, 140), 4 (82, 129, 133, 140) actually carried a prose em dash and were fixed in this pass; line 22 carried none at the time of this pass. A sixth hit at line 159 is a literal character inside a quoted shell command verifying no em dashes were added, and is correctly not a violation; it is unchanged.

**Evidence:** These are substantive initiative documents (root-cause research and committed-row audits, both cited elsewhere in this report as load-bearing evidence for traceability), not scratch notes, so the no-em-dash rule applies to them.

**Correction:** Rewrite the flagged sentences in all three files to remove em dashes, consistent with the rest of the corpus (no em dashes were found anywhere else in decision-log.md, initiative.md, context.md, exploration.md, review.md, plan.md, plan-critique.md, implementation.md, or any ledger/*.md file).

### M12. Six "session scratchpad" references are a double violation (AI-session reference + scratchpad path)

**Location:** `verification/verify-L1-a.md:7`, `verification/verify-L2-a.md:14-15`, `verification/verify-L3-d.md:11`, `verification/verify-L4-a.md:8,10`, `verification/L3-resolution-d.md:17`.

**Evidence:** All six read some variant of "ran... under the session scratchpad," referring to the AI verifier's own working/scratchpad directory used to run builds and tests. This is simultaneously a reference to the AI-assistant tooling process (prohibited) and a scratchpad/tmp-path reference (prohibited).

**Correction:** Rewrite each to describe the verification method without naming the scratchpad/session mechanism, e.g. "in an isolated `git archive HEAD` copy."

### M13. Process-mapping gap: dependency `max_5_agents`-style literal "agent" substrings aside, three "the implementer's session" references are an unresolved judgment call that should default to violation

**Location:** `verification/L2-integration.md:15,250`, `verification/L2-resolution-a.md:396`.

**Evidence:** "The measurement log... was left to the implementer's session" and similar phrasing. Given the project is implemented by an AI across discrete work sessions, and the writing rule's purpose is to keep the initiative's own documents free of narration about the AI tooling process, these should be read as violations rather than a neutral "whoever implements this": the ambiguity itself indicates the phrasing needs to be fixed, not left as acceptable.

**Correction:** Rewrite to avoid "session," e.g. "was left for a later commit" or "is deferred to BR-1 / row N."

---

## Minor findings

1. **Five `dl-i08-process` sub-rules are never cited by their literal tag anywhere in plan.md or the ledger** (`plan_form`, `independence`, `documents`, `writing`, `branch`), though their content is structurally honored throughout. Suggested: add explicit tags at plan.md:2108 (`independence`), plan.md:2234 (`writing`), plan.md:1828 (`branch`), matching the pattern already used for `.readiness`, `.scope`, `.purpose`, `.found_defects`.

2. **Seven `dl-i08-architecture` sub-decisions have no plan.md §2 citation**, only ledger-level coverage: `consumer_gate`, `unavailable_periods`, `wet_appliance_hot_water`, `setpoint_sources`, `battery_control`, `consumer_inert_override_probe`, `cross_platform_measurement`. All seven are substantively and correctly implemented in the relevant ledger entry (verified directly); this is a traceability-tag gap, not a content gap.

3. **review.md V38** (water-heater UA resolution silent fallback, `resolve_water_heater.rs:131-141`) is plausibly folded into OD-2's "strict configuration" scope but this audit could not independently confirm OD-2's body text names this specific claim. Needs a direct check of `ledger/11-other-defects.md` OD-2 text against the V38 evidence lines.

4. **review.md V22** (`initialization_duration` value never read) is dispositioned to B-2 per review.md's own decision section, but B-2's body text was confirmed to address the convergence-criterion claim (V24) clearly; the "duration value is read" claim specifically (V22) was not independently confirmed in B-2's text.

5. **exploration.md §1.8 items RL-7 and FL-10** map to benchmark files (`benches/rl_episode.rs`, benches under `06-runtime-memory.md`) inside existing ledger entries rather than to a dedicated entry ID; plausible but not independently re-opened and verified in this pass.

6. **Cross-row disposition gap for `helpers.rs` census table**, independently flagged by both `verification/audit-OR-11-b8195c83.md` (minor m1) and `verification/OR-11-stopped-rows-research.md`'s "Cross-row process note" (lines 403-418): the census table row at `ledger/03-output-reporting.md:1123` still reads as a single disposition even though both verification documents agree the water-heater-ambient-source and dehumidifier-conditioned-zone fixes need two separate dispositions. Not fixed by OR-18. A one-cell correction.

7. **`initiative.md`'s Status section** names outstanding work only generically ("OR-11's three unresolved census rows," "rows 16a to 16h") rather than by the specific ledger IDs OR-18/OR-19 that plan-critique.md and the schedule use. No practical impact since the substance matches, but less traceable than the other documents.

8. **`flow.yaml`** (not a `.md`/`.txt` initiative document, a workflow-automation config) contains 7 prose em dashes in description/note strings (lines 51, 107, 132, 150, 187, 207, 216). Likely out of scope for the writing rule (tooling config, not initiative prose) but flagged for the operator to confirm applicability.

9. **Rule-name citations containing the literal substring "agent"** (e.g. `max_5_agents`, `opencode_stateless`) appear in several `verify-L*-e.md` process-compliance checklists. These are internal rule identifiers being checked against, not narrative references to AI agents; recommend treating as exempt bookkeeping unless strict literal enforcement is wanted.

10. **`dl-i08-architecture.fleet`** has only one literal tag citation (in `ledger/09-numeric-fixes.md:1594`) with the bulk of its coverage being the dedicated `ledger/04-fleet.md` file matched by content rather than tag. Not a gap (the dedicated file is unambiguous), but noted for completeness since the task asked for exhaustive tag-mapping verification.

11. **Ledger files were observed changing during this audit** (`ledger/00-schedule.md` grew between the start and end of the parallel audit passes; `ledger/01-baseline-repair.md` gained a new entry, BR-20, mid-audit). This report reflects the state as read by each audit pass; given active concurrent editing, a follow-up pass should re-diff `ledger/00-schedule.md` and `ledger/01-baseline-repair.md` against their current content before treating B2 (the 16a1 preamble gap) as still open, since it may since have been corrected or further changed.

---

## Checks that passed cleanly (exhaustive, not sampled)

- Every row in `ledger/00-schedule.md` references an entry ID that exists in ledger/01-12 (162 rows checked against 164 known IDs). No nonexistent/typo'd entry ID found.
- Every dependency named in any ledger entry's "Dependencies" field (~140 fields checked) is scheduled at an earlier row than the dependent entry. No ordering violations found, including the more intricate multi-position chains (B-14, OD-2, OD-10).
- Every committed row's claimed commit hash exists in `git -C /home/rich/src/HARES log --oneline` and its message matches exactly (27 hashes checked, zero mismatches), plus the consumer-repo commits named in BR-19/BR-6d/CN-0/CN-16 verified against `git -C /home/rich/src/HARES_CALIBRATION_SYSTEM log --oneline` (5 hashes, zero mismatches).
- `dl-i08-process.readiness`'s `executed_order` list (PR-1 through SL-7) matches the live git log exactly, in the same order.
- Every one of context.md's 21 items maps to a substantive ledger entry; no gaps in that list.
- 48 of 50 review.md defects (V1-V50) map to substantive ledger entries; only V36 (Major M9 above) and the softer V22/V38 notes (Minor 3/4) are unresolved to full confidence.
- Every ledger ID cited in exploration.md §1.8's requirement-mapping table exists as a real heading in the corresponding ledger file (zero dangling references), and ~45% of citations were spot-verified as substantively on-topic with no mismatches found.
- All blocker/major findings in the four verification files (`audit-OR-11-b8195c83.md`, `audit-SL-7-7b7a6a7d.md`, `redteam-OR-11-SL-7.md`, `OR-11-stopped-rows-research.md`) map to substantive ledger fixes (OR-18, OR-19, or direct text additions inside RT-3/`02-measurement.md`'s SL-7 section), with the caveat that OR-18 and OR-19 are themselves still "not started" in implementation state, a landing-status fact, not a traceability gap.
- Em dashes, LLM-tic phrases ("load-bearing", "stated plainly"), archaeology language, and agent/session references were found to be clean (no violations) across decision-log.md, initiative.md, context.md, exploration.md, review.md, plan.md, plan-critique.md, implementation.md (aside from M10 above), and every ledger/*.md file.
