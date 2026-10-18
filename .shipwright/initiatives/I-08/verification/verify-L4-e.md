# Round L4, traceability (e)

Scope: `plan.md` and `ledger/*.md` checked against `decision-log.md`, the binding rules
(`memory/feedback_*.md`), `context.md` items 1 to 21, `review.md` V1 to V50, the
`exploration.md` requirement IDs, and every finding of `verification/verify-L3-a.md` to
`verify-L3-e.md`. Each L3 finding was checked against the current text, not against what
the resolution files say. Also checked: the schedule against `git log` on
`feat/output-results` and `i08-consumer`, whether the source documents are current, where
documents point outside this directory, and the writing rules.

Head checked: engine `afa9a071` (plus 51 uncommitted OR-11 paths in the tree); consumer `5c651bc`.

**Counts: 2 blockers, 13 majors, 27 minors.**

## Findings

### Blockers

**1. The decision log and the plan give opposite orders for OR-11 and rows 16a to 16h.**

- `decision-log.md` `dl-i08-process.readiness.next` says: "Finish OR-11 with the recorded
  fluid-loop resolution (bitwise golden equal), then run rows 16a to 16h before any further
  row".
- These four places say the reverse, rows 16a to 16h first and OR-11 after:
  - `ledger/00-schedule.md` lines 11-15 and 19-23;
  - `plan.md` §3 "Opening sequence" ("... and OD-15 (... rows 16a to 16h), before OR-11");
  - `initiative.md` Status;
  - `08-consumer.md` (CN-17 "before any further engine row").
- The decision log is authoritative, so the plan contradicts a binding decision.
- The tree holds 51 uncommitted OR-11 paths, and the rules forbid stash and checkout. Running
  BR-10 and BR-12 to BR-15 first would therefore commit on top of OR-11's work in progress.
  The decision log's order is the one that can actually be executed.

Correction (the operator picks one; every document then states it):

- **(a) Keep the decision log's order.**
  - Schedule line 11: "Rows 0 to 18 are committed. Row 19 (OR-11) is in progress and runs
    next, then rows 16a to 16h; rows after them follow in table order."
  - Rewrite the opening-sequence rule (lines 19-23), `plan.md` §3 "Opening sequence" ("...
    PR-6, then OR-11 (row 19), then CN-17, BR-10, ...") and `initiative.md` Status to match.
  - CN-17 then gates on OR-11's commit, its census covers rows 1 to 19, and the schedule's
    CN-17 rule already says this.
- **(b) Keep the schedule's order.**
  - Rewrite `readiness.next` to: "Run rows 16a to 16h (CN-17 gated on `afa9a071`, census
    rows 1 to 18; BR-10; BR-12 to BR-15; the row 16g re-check; OD-15), then OR-11 (row 19)
    with the fluid-loop resolution in its stop record."
  - Add a rule saying how rows 16a to 16h commit while the OR-11 tree is dirty without stash
    or checkout, for example "OR-11's uncommitted paths are disjoint from rows 16a to 16h,
    and each row stages only its own paths; the block runs in an engine-under-test worktree
    of the row's commit".

In either case, bring `readiness.status` up to date. It still says "round L3 re-checks them".

**2. A consumer gate step restores files by redirecting `git show` over them, and writes a bundle outside both repositories.**

- Where: `08-consumer.md:331-335` ("restored to their committed bytes with
  `git show HEAD:<file> > <file>`") and `02-measurement.md:586` ("`git show
  c71c542:<file>` redirection").
- Rules contradicted:
  - the redirect is a checkout-equivalent overwrite done through the shell, against
    `feedback_no_git_checkout`, `feedback_no_destructive_git` and `feedback_no_sed`;
  - the bundle overwrites `~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_<date>.zip`, which is
    outside both repositories.

Correction:

- Replace the parenthesis with: "(`build_bundle.py` runs only on a gate run that ends in a
  commit; a B0 check or a measurement runs the other gate lines and records that the bundle
  step did not run)".
- Replace "writes the bundle `~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_<date>.zip` beside the
  repository, replacing the same day's bundle" with "writes the bundle to
  `scratch/i08/<sha>/bundle.zip` (CN-17 adds the output-path argument if
  `build_bundle.py:384-398` has none)".
- Drop the redirection method from `02-measurement.md:586`.

### Majors

**3. The schedule does not match `git log` and has no executed-state column.**

Seven executed commits appear in no schedule row and no ledger entry:

| Commit | Repository | Where it is recorded today |
|---|---|---|
| `c88225df` (BR-1 formatting) | engine | decision log, implementer record |
| `469ea955` (OR-5 gate and documentation) | engine | decision log, implementer record |
| `f880a6d7` (zero-row golden read, BR-6b) | engine | decision log, implementer record |
| `4a450949` (EV soh binding) | engine | decision log, implementer record |
| `59b060d6` (golden round-trip feature set) | engine | decision log, implementer record |
| `f7131d7` (capture provenance guard) | consumer | decision log, implementer record |
| `aec2e3f` (capture provenance guard) | consumer | decision log, implementer record |

- `4a450949`, `59b060d6`, `f880a6d7`, `f7131d7` and `aec2e3f` are found defects fixed
  without a ledger entry, against `dl-i08-process.found_defects` ("tracked ... as separate
  fix entries").
- Only row 3a carries "(executed)". Rows 13a to 18 carry SHAs in their titles, and no other
  row says whether it ran.

Correction:

- Add a `State` column: `committed <sha>[, <sha>]`, `recorded` for measurement rows,
  `in progress` for row 19, and empty otherwise.
- Fill it from `git log` for rows 0 to 18. Row 1 is `aa22be6c, c88225df`; row 4 is
  `dc033cee, 469ea955`; row 11 is `f880a6d7, 872cf289`.
- Add rows 8a (`4a450949`), 9a (`59b060d6`) and 0e (consumer `f7131d7, aec2e3f`). Give each
  a short entry (problem, evidence, change, acceptance test, commit) in `02-measurement.md`
  or `08-consumer.md` respectively.
- Remove "(executed)" from row 3a's title.

**4. B-14's test calls a function that does not exist (L3-b finding 10, only partly resolved).**

- `09-numeric-fixes.md:2168`, `equipment_config_fields_are_si`, calls
  `hares_io::input_schema()`.
- `plan.md` §2.7 and IN-3 place `input_schema()` in `hares-core`, which `hares-io` cannot
  see.

Correction: split the test.
- `crates/hares-io` keeps only the walk over each config type's serialised keys.
- `crates/hares-core/tests/si_config_schema.rs::input_schema_override_classes_are_si`
  asserts on `hares_core::input_schema().overrides`.

**5. `context.md` contradicts `dl-i08-architecture.engine_performance` and RT-2.**

- Item 4 says "The (M + D) LU is rebuilt twice per step". The decision says the implicit
  matrix is always the identity, and the path is removed as dead code (SL-7).
- Item 2 says forcing "reuse is keyed on geometry". RT-2 states "Geometry is never part of a
  shared key".

Correction:
- Item 4: "**Envelope solar and incidence recomputation; dead coupled factorization.** The
  coupled (M + D) LU path (`state_space.rs:695-708`, `stepping.rs:682-685, 723-725`) is
  unreachable because the implicit matrix is always the identity; it is removed as dead code.
  `m_scratch` is reallocated each call; the solar slot map is rebuilt twice per step
  (`thermal_solver/mod.rs:1560-1568`)."
- Item 2: "... so the surface-independent forcing is shared through the parsed weather, keyed
  on the time grid and site, and per-surface values never cross geometries."

**6. `review.md` §2 "Engine performance" contradicts the same decision.**

- It says "envelope solar/incidence and coupled factorization reuse".
- Replace with: "envelope solar/incidence reuse, thermal input preparation allocations,
  thread-safe concurrent dwellings. The coupled factorization is removed as dead code (the
  implicit matrix is always the identity, ledger SL-7), not reused."

**7. `review.md` §2 "Weather" leaves out `leap_day_day_types`.**

Append: "One exception: after a skipped Feb 29, day types follow the true calendar;
EnergyPlus shifts every later date one weekday behind, which its own documentation calls
incorrect (`leap_day_day_types`). Feb 29 on a typical-year file without one, and sub-hourly
files whose interval differs from the time step, follow EnergyPlus (`weather_edge_cases`)."

**8. `review.md` §2 claims to cover every decision but leaves out 17 keys.**

- The missing keys:
  - metrics_optional, dependencies, ochre_compat, defaults_path, calibration_r_factor;
  - consumer_gate, profiler_scope, si_units, golden_storage, setpoint_sources;
  - battery_control, rl_observation_allocation, cross_platform_measurement;
  - consumer_inert_override_probe, input_schema_scope, public_planning_docs, slop_scope.
- Correction:
  - Reword the preamble to: "Each decision below states the current position of the matching
    `dl-i08-architecture` key; the keys listed here (names) are stated only in
    `decision-log.md`."
  - Add "Metrics are computed only when the run requests them" to "Output model".
  - Add the envelope-scale mechanism (IN-7) to "Inputs".

**9. `exploration.md` R1, R3 and requirement PY-7 contradict `health` and `metrics_optional`.**

- R1: replace "still populated" with "health is still recorded: an injected NaN or unresolved
  column fails the run with a typed error".
- R3: replace "a strict mode raises a typed error ... on an injected NaN" with "an injected NaN
  fails the run in every build with a typed error naming the step and the zone;
  `HealthPolicy::strict()` escalates the counted events".
- PY-7: replace "`metrics()` is identical with `retain_batches` on or off" with
  "`RunResult.metrics`, when requested, is identical whatever the selected reports and
  destinations".

**10. Requirement R7 (a fit writes nothing under scratch) is only partly covered.**

- CN-1 (`08-consumer.md` near :785) keeps `slot_dir` "which the engine reads by path", and
  CN-10 keeps per-candidate HPXML and schedule files.
- RT-4 (row 61) already accepts content inputs, and it lands before CN-1 (row 72).
- Correction: CN-1 passes the injected HPXML and schedule as text
  (`hares.ParsedHpxml.from_text`, `hares.ParsedSchedule.from_text`) and deletes `slot_dir`,
  with the test `suite_fit.fit_writes_no_slot_files` (a fit run in a fresh temporary
  directory leaves it empty; DE trace equals B0).
- Alternatively, record a decision that slot files are kept, with RT-4 as the seam, and mark
  R7 as deliberately partial.

**11. Requirement R20 (warm-up behaviour documented) has no documentation change.**

- Correction: B-2 adds a "Warm-up" section to `docs/python.md`. It covers:
  - the criteria and their defaults (0.4 C, 0.04, 1 to 25 days);
  - that warm-up rows never appear in a report;
  - what `warmup=None` does;
  - where `RunHealth.warmup` is read.
- Its example runs as `test_docs_warmup_example`, and B-2's Verification names it.

**12. The `[writing]` check does not enforce the plan §3 writing rule.**

- The `ids=` pattern misses:
  - review IDs `V1` to `V50`;
  - requirement prefixes `DV-`, `RL-`, `RS-` and `X-`;
  - decision keys `dl-i08`;
  - round IDs `L1` to `L4`;
  - `HARES#NN` in added code lines;
  - agent and LLM references.
- The consumer gate has no writing check, although §3 applies the rule to consumer commits.
- Correction:
  - Replace `ids=` with
    `'\b(BR|M|PR|OR|FL|PY|RT|CN|B|OD|SL|IN|T|CW|CFG|I|DV|RL|RS|X)-[0-9]+[a-z]?\b|\bV[0-9]{1,2}\b|\bdl-i08|\bL[1-9]-|\bTask [0-9]|\bFinding [0-9]|[Rr]eviewers?\b|\b[Tt]ickets?\b|\b[Rr]ound [0-9]|\b(LLM|Claude|subagent)\b'`.
  - Add `if git show --format= --unified=0 HEAD | grep -nP '^\+(?!\+\+).*HARES#[0-9]+'; then exit 1; fi   # [writing]`.
  - Add both checks, run with `git -C ~/src/HARES_CALIBRATION_SYSTEM`, to the consumer gate in
    `08-consumer.md`.

**13. Two recorded red-team findings have no entry.**

- The BR-6b log (`02-measurement.md:694-700`) records two red-team findings and closes
  neither:
  - quick-xml 0.42 attribute whitespace and line-ending normalization;
  - a tzf data release whose code path has only unit-test coverage.
- This breaks `feedback_fix_all_review_findings` and `feedback_no_broken_windows`.
- Correction: add `11-other-defects.md` OD-21 with:
  - an HPXML parser test pinning attribute and line-ending normalization;
  - an integration test of the resolved site UTC offset from tzf for a fixture site;
  - its own row (24h) after OD-19.
- The log sentence then names OD-21.

**14. OD-11 keeps OCHRE divergences as strict xfails without saying which value is correct.**

- This breaks `feedback_ashrae_not_ochre` and `feedback_ochre_parity_rules`.
- Correction: append to OD-11's Change, and to OD-13's last Change bullet: "A mark is kept only
  when its reason cites the reference (EnergyPlus engineering reference or source, ASHRAE)
  showing HARES's value is correct, or the documented OCHRE defect. Any other divergence is a
  defect handled by BR-12's rule: a bitwise-neutral fix here, or a new `09-numeric-fixes.md`
  entry."

**15. `decision-log.md` is stale and breaks the writing rule it states.**

- `dl-i08-execution.progress` is a snapshot from row 8. Its `next:` says "BR-6a ... M-3 (rows
  10 to 13a)", but M-3 is row 13b (L3-e finding 29, unresolved).
- `br6c-notes` says "the invocation was an agent's manual sanity step", and `br6b-notes` says
  "could not be removed by the harness".
- `dl-i08-process.branch` still says the branch "is recreated from initial-implementation
  after the operator merges PR #27", but it was fast-forwarded to a local merge, and `plan.md`
  §3 forbids forced branch recreation.
- Correction:
  - Rewrite `progress.state` to rows 0 to 18 committed, with their SHAs, and delete its
    `next:`.
  - Replace "an agent's manual sanity step" with "a manual check outside the block".
  - Replace "could not be removed by the harness" with "was not removed".
  - Change `branch` to: "HARES work happens on feat/output-results, fast-forwarded to the local
    merge of PR #27 (1d60f478); the GitHub merge is an operator checkpoint of the closing
    sequence."

### Minors

**Ledger and plan text**

16. **The plan text tells the history of its own earlier drafts.**
    - `08-consumer.md:33-37`: delete "CN-17 ... was placed directly after BR-6d (consumer
      `5c651bc`). It did not land there: ... while the consumer stayed at `5c651bc`." Keep the
      current rule.
    - CN-17's Dependencies: delete "The position first required was directly after PR-2 ...".
      Keep "Position: directly after row 18 (PR-6, `afa9a071`), gated on that commit; if
      another engine row commits first, CN-17 moves directly after it and that row's B0 check
      joins row 16g. In any case before CN-13."
17. **`11-other-defects.md` OD-20 (:1316, :1327) mentions a review round.**
    - Replace "recorded by the code verification of round L3" with "found while verifying the
      ledger".
    - Replace "The code verification of round L3 found that the coverage step" with "The
      coverage step".
18. **The OR-11 stop record keeps the rejected operator options.** In
    `03-output-reporting.md:761-767`, the "(a)/(b)" options and "Resolution ... neither
    option" are draft history. Keep the facts (the row, the input, the error) and the
    re-disposition only, and write "undeclared" and "declared" in lower case.
19. **A schedule note tells the history of an earlier draft.** `00-schedule.md:272-273`:
    replace "Row 54 has no entry: PY-9, which it held, runs at row 43a, directly after OR-6."
    with "There is no row 54."
20. **Schedule rows 16b to 16f carry no commit message.** They read "see BR-10" to "see
    BR-15". Copy each entry's commit message into the Commit column, as every other row has
    one.
21. **The extra verification round for B-14 is not in the schedule.** The decision log's
    "B-14 gets its own verification round before row 94c" appears nowhere in the schedule or
    B-14. Add to row 94c: "starts only after a verification round over B-14 records zero
    blockers and zero majors in `plan-critique.md`".
22. **The scratch-deletion checkpoint is broader than CN-8.**
    - `plan.md` §3 says "deleting the consumer's `scratch/` output (7.0 GB across 61
      top-level directories ...)", but CN-8 deletes only `scratch/fitrun`.
    - Replace with "deleting the consumer's `scratch/fitrun` (6.2 GB of the 7.0 GB `scratch/`
      at `c71c542`, re-measured at execution), by named directory only ...".
    - `context.md` item 19's "9 968 directories" should read "`scratch/fitrun`, one directory
      per worker process".
23. **The thermal-solver scope list in `plan.md` §3 is incomplete.**
    - It lists OR-11 as touching only "the `fluid_solver.rs` and construction checks". Replace
      with "its `hares-envelope` census rows and the `DomainSolver::resolve` signature in
      every domain solver". The uncommitted work touches `humidity_solver.rs`,
      `electrical_solver.rs`, `state_space.rs`, `boundary_rc.rs` and `thermal_solver/*`.
    - It omits the committed OR-5, BR-6a and BR-6b, which also touched `hares-envelope`.
24. **Rule 2 does not require a physics-decision record.** Append to `plan.md` §3 rule 2:
    "Each numeric entry records its physics decision and its reference in
    `docs/PHYSICS_DECISIONS.md` in its commit." (B-2, B-11, B-12, OD-9 and FL-5 do not cite
    it today.)
25. **FL-1 ships a function named `legacy_fleet_rules`**, which lives until FL-5 deletes it.
    Rename it `unit_keyed_fleet_rules`.
26. **The per-entry review does not ask for untested behaviour changes.** After "and no slop."
    in `plan.md` §3 Per-entry review, add: "The reviewer lists every behaviour change the
    diff makes that no test pins, with the test file it belongs in; each is a finding."
27. **The verification block never runs before a commit.** It runs only after each commit
    (`feedback_tests_every_ticket`). Change the first sentence to: "Run ... on the staged
    change before the entry commits, and again on the commit before the next entry starts
    (`[writing]` on the commit run only)."
28. **OD-2 omits V11 from its Fixes list.** Change OD-2's Kind line "Fixes V35, V36," to
    "Fixes V11, V35, V36,".
29. **Several entries fix V-items without naming them.** Add each V-ID to the entry's Kind
    line:

    | Entry | V-IDs to add |
    |---|---|
    | OR-5 | V1, V2 |
    | B-1a, B-1b | V4 |
    | B-1c | V5 |
    | B-4 | V6 |
    | B-8 | V8 |
    | B-5 | V9, V10 |
    | OR-4 | V13 |
    | B-6 | V14 |
    | B-10 | V19 |
    | B-2 | V22, V24 |
    | B-3 | V23 |
    | RT-5 | V25, V30 |
    | RT-1 | V27 |
    | B-7, OR-8 | V29 |
    | OR-3 | V34, V45, V50 |
    | B-11 | V49 |
    | B-9 | V42 |

30. **No test pins V44's `master_seed=0`.** OD-7 adds
    `test_gym_zero_master_seed_reaches_the_dwelling`.
31. **The OD preamble cites line numbers by two conventions.**
    - It cites `boundary_rc.rs:2387`, but the function is at `:2385` at `afa9a071`.
    - `rc_network.rs:1037` is the `#[test]` line, not the function.
    - Use one convention (the `fn` line) for both.
32. **RL-1 has no test that a default gym writes no file.** OR-3 adds
    `test_gym_default_writes_no_file`: a default `DwellingGymEnv` episode in a fresh temporary
    working directory leaves it empty.
33. **X-5 has no fleet schema test.** FL-1 adds
    `fleet_aggregate_columns_match_home_report_schema`.
34. **R9's 100 ms latency target is neither measured nor waived.** IN-2 records the wall time
    of `validate()` as a Measurement.

**Source and record documents**

35. **`review.md` §2 "Aggregation" omits the two-stage fleet rule.** Append the decision's
    sentence: "Fleets combine in two stages: ...; every quantity variable declares its
    across-homes rule."
36. **Eight evidence paths in `review.md` do not exist.** Correct them:

    | V | Cited path | Correct path |
    |---|---|---|
    | V4, V5 | `crates/hares-core/src/dwelling/environment.rs` | `crates/hares-core/src/environment.rs` |
    | V8 | `thermal_solver/fluid_solver.rs` | `crates/hares-envelope/src/fluid_solver.rs` |
    | V9, V10 | `crates/hares-core/src/dwelling/columns.rs` | `crates/hares-io/src/output/columns.rs` |
    | V16 | `crates/hares-core/src/fleet/aggregation.rs` | `crates/hares-fleet/src/aggregation.rs` |
    | V29 | `metrics.rs` | `crates/hares-io/src/output/metrics.rs` |
    | V31 | `thermal_solver/rc_network.rs` | `crates/hares-envelope/src/rc_network.rs` |
    | V46 | `crates/hares-core/src/hpxml/equipment.rs` | `crates/hares-io/src/hpxml/equipment.rs` |
    | V21 | `.../ev/config.rs` | `crates/hares-equipment/src/ev/config.rs` |

37. **Four `review.md` Status and Ledger cells are stale.**
    - V8 Status: "partial: the arm is unreachable; removed with zero delta (B-8)".
    - V9 Status: "partial: latent (B-5)".
    - V31 Ledger: "none (already pinned; `11-other-defects.md` preamble)".
    - V42 Ledger: "B-9, with B-9a".
38. **`exploration.md` still describes an OCHRE preset in the core.** X-12 and §4 describe "an
    OCHRE-parity preset". Replace with "OCHRE comparison through the Python adapter's mapping;
    no OCHRE preset in the Rust core (`ochre_compat`)".
39. **`exploration.md` points to the wrong file for decisions.** Lines 5-6 and 12 say they are
    in `review.md`. Point to `decision-log.md`, and to `review.md` only for the defect table.
    Line 335: "a always-available" should be "an always-available".
40. **The requirement IDs have no traceability table.** 79 of the 94 IDs are cited nowhere.
    Add the requirement-to-entry table below as `plan.md` §2.11, writing the PY and FL IDs as
    "requirement PY-n" and "requirement FL-n".
41. **`plan-critique.md` is stale and breaks the writing rule.**
    - Its two em dashes (around :57 and :61) break the writing rule.
    - The L1 table holds "pending" cells.
    - Round L3 says "PR-3 started", but PR-3, PR-5 and PR-6 are committed.
    - Rewrite the status lines to the current state.
42. **`implementation.md` is stale and contradicts the rules.**
    - It covers rows 0 to 8 at `7643b4e7`, but rows 0 to 18 are committed.
    - It says "by agents that did not write the code". Write "by parties that did not write
      the code".
    - Its `passing_commands` run the consumer gate with `HARES_REPO=/home/rich/src/HARES`,
      which is the editable main tree that `plan.md` §3 forbids. Replace it with the
      engine-under-test form (`HARES_REPO=/home/rich/src/HARES/.workstrees/<sha>`).

## Tables

### Decisions (`decision-log.md`)

| Decision | Implemented by | State |
|---|---|---|
| criteria, process.purpose/found_defects/plan_form/independence/documents/scope | plan §1, §3 | found_defects broken by commits with no entry (3) |
| process.readiness (status, next) | plan §1, schedule | contradicted (1); stale (15) |
| process.writing | plan §3 Writing, `[writing]` | enforcement gap (12); breaches (15, 16, 17, 19, 41, 42) |
| process.branch | plan §3 Branches, Task 0 | text stale (15) |
| results | OR-6, OR-9, PY-9 | covered |
| output_model | OR-1, OR-2, OR-3, OR-13 | covered |
| destinations | OR-3, OR-14, PY-10 (`callback_exception_fails_run_with_cause_chained`) | covered |
| python_export | PY-1 | covered |
| config | PY-8, PY-2, IN-6 (`REMOVED_KEYS`) | covered |
| inputs | RT-4, RT-6 | covered |
| new_capability | OR-13, RT-6, IN-2, OR-12, OR-15, IN-4, OR-16, IN-3; §2.10 seams | covered |
| health | OR-5, OR-11, OR-12, OD-18, CN-2 | covered |
| time | OR-7, B-6 | covered |
| weather, weather_edge_cases, leap_day_day_types | B-1a, B-1b, B-1c | covered |
| aggregation | OR-13, FL-5, B-11 | covered |
| fleet | FL-1 to FL-4, B-12 | covered |
| numeric_fixes | B-1a to B-13 (plan §2.8) | covered |
| precedence | OD-2, B-13 | covered |
| reproducibility, cross_platform_measurement | RT-5, FL-1, M-4a, M-4b | covered |
| package | PY-7, CN-13 | covered |
| engine_performance | RT-1 to RT-5, SL-7 | covered in plan; contradicted in context (5) and review (6) |
| dependencies | BR-6a to BR-6d, OD-14, `[advisories]` | covered |
| metrics_optional | OR-8, CN-7 | covered |
| ochre_compat | IN-6 | covered; exploration stale (38) |
| defaults_path | B-10 | covered |
| calibration_r_factor | IN-7, CN-3 | covered |
| consumer_gate | CN-16, 08 gate, CN-15 | covered; bundle step breaks a rule (2) |
| profiler_scope | PR-1 to PR-6 | covered (PR-1, 2, 3, 5, 6 committed) |
| si_units | B-14 (four commits) | covered; test defect (4) |
| golden_storage | M-1 | covered |
| setpoint_sources | IN-5, CN-1 (`setpoint_factor_edits_the_active_source`) | covered |
| battery_control | OD-8 | covered |
| rl_observation_allocation | RT-7 | covered |
| consumer_inert_override_probe | CN-1 (`wh_energy_factor` typed rejection) | covered |
| input_schema_scope | IN-3 | covered |
| public_planning_docs | SL-8 (`git mv`, then `git rm --cached`; nothing deleted locally) | covered |
| slop_scope | SL-2, SL-3, plan §3 | covered |
| stray_outputs | Task 0, BR-7, `[stray]` | covered |
| consumer | schedule rules, CN-4 | covered |
| execution.worktrees | plan §3, 08 mechanism | covered |
| python_support | plan §5 | covered |

### Binding rules (`feedback_*.md`)

| Rule | Embodied | Contradicted |
|---|---|---|
| no_destructive_git, no_git_checkout, no_git_stash, no_git_revert, no_sed | plan §3 Branches; operator checkpoints | yes (2) |
| no_ai_slop_public, no_ticket_refs_in_comments | plan §3 Writing, SL-2, SL-3, SL-8 | enforcement gap (12) |
| shipwright_files_authoritative | decision `documents` | yes (15, 41, 42) |
| fix_all_review_findings, no_broken_windows | plan §3 Independence | yes (13) |
| ashrae_not_ochre, ochre_parity_rules, best_physics | plan §3 rule 2 | partly (14, 24) |
| tests_every_ticket | plan §3 block | partly (27) |
| test_coverage_reporting | plan §3 Per-entry review | partly (26) |
| no_legacy_shims, no_backward_compat | IN-6, PY-7, `REMOVED_KEYS` | minor (25) |
| no_touch_thermal_solver (scoped by the decision) | plan §3 scope list | list incomplete (23) |
| always_review, no_self_review | plan §3 Independence | no |
| no_ignore_tests | BR-8, BR-12, OD-11, OD-13 | no (but see 14) |
| no_silent_defaults | OD-2, 3, 6, 8, 10, 17, 19, 20; B-10 | no |
| si_units, use_uom | B-14, §2.1, `Unit::from_si` | no |
| local_time_only | §2.5, `LocalInstant`, FL-4, B-12 | no |
| hot_loop_minimal, custom_domains_perf, code_quality | §2.9, RT-1, RT-3, RT-7, OR-2, OR-11 | no |
| pay_for_what_you_need | OR-3, OR-8, metrics_optional | no |
| deterministic_tests, test_quality | §3 recorded measurements; RT-5; M-1 | no |
| equipment_self_contained, actor_telemetry | OD-2, RT-8, OR-1, PY-11 | no |
| design_for_all_users, fix_existing_not_new_features | §2, §2.10 seams | no |
| no_timelines, no_blanket_kill, no_useless_comments, no_ticket_refs | no hits | no |
| max_5_agents, opencode_stateless, no_code_fixes_from_reviews | not applicable | no |

### Context items 1 to 21

| Item | Entry |
|---|---|
| 1 | RT-1, SL-1 |
| 2 | RT-2 (context text stale, 5) |
| 3 | RT-4 |
| 4 | RT-3, SL-7 (context text stale, 5) |
| 5 | RT-3 |
| 6 | RT-5, CN-10 |
| 7 | OR-9 |
| 8 | OR-2, OR-6, PY-1 |
| 9 | OR-3 |
| 10 | OR-5 (committed), OR-11 |
| 11 | OR-6 |
| 12 | OR-4 |
| 13 | FL-1, FL-2 |
| 14 | OR-7 |
| 15 | PR-1, PR-2, PR-3, PR-5, PR-6 (committed), PR-4 |
| 16 | CN-1, CN-13, CN-2 (`contract()` becomes a schema assertion) |
| 17 | CN-5 |
| 18 | CN-6 (ABSOLUTE precondition) |
| 19 | CN-7, CN-8 |
| 20 | CN-11 |
| 21 | CN-12 |

Every item has an entry with an acceptance test, or a recorded measurement where the item is a measurement.

### review.md V1 to V50

| V | Entry | V | Entry |
|---|---|---|---|
| V1, V2 | OR-5 | V26 | OR-6 |
| V3 | OD-1 | V27 | RT-1 |
| V4 | B-1a, B-1b | V28 | OR-10 |
| V5 | B-1c | V29 | B-7, OR-8 |
| V6 | B-4 | V30 | FL-1, RT-5 |
| V7 | OR-9 | V31 | none (already pinned; 11 preamble) |
| V8 | B-8 | V32 | PY-3 |
| V9, V10 | B-5 | V33 | PY-4 |
| V11 | OD-2 site 1 | V34 | OR-3 |
| V12 | OR-6 | V35 | OD-2 site 2 |
| V13 | OR-4 | V36 | OD-2 site 8 |
| V14 | B-6 | V37 | OD-2 sites 3, 4 |
| V15 | PY-5 | V38 | OD-2 site 5 |
| V16, V17 | FL-2 | V39 | OD-2 site 6, IN-5 |
| V18 | PY-6 | V40 | OD-2 site 7, OD-8 |
| V19 | B-10 | V41 | FL-3 |
| V20 | PY-2, PY-8 | V42 | B-9, B-9a |
| V21 | none (pinned, `ev/tests.rs:8062`) | V43 | OD-2 site 9 |
| V22, V24 | B-2 | V44 | OD-7 (test gap, 30) |
| V23 | B-3 | V45 | OR-3, PY-1 |
| V25 | RT-5, OR-5 | V46 | OD-3 |
| | | V47 | refuted (single `StepResult`) |
| | | V48 | design, OR-1 and OR-8 |
| | | V49 | OR-3, B-10, OR-4, B-11, PY-5, FL-1, FL-2 |
| | | V50 | OR-3 |

No confirmed defect is missing.

### exploration.md requirements (94 IDs)

| Group | Entries | Gaps |
|---|---|---|
| R1 to R25 | OR-3, OR-5, OR-1, OR-7, PY-6, B-5, OD-2, RT-6, IN-2, IN-3, OD-1, PY-2, PY-8, B-10, RT-5, B-8, plan §2.8, PY-9, PR-1, BR-10, PY-3, CN-10, M-3, OR-16, OD-8, IN-9, B-14, PY-12, OR-10; R19 is a §2.10 seam | R7 partial (10); R20 partial (11); R9 latency (34) |
| PY-1 to PY-14 | OR-6, OR-3, OR-4, OR-7, PY-1, B-6, PY-2, PY-8, PY-9, OR-8, B-7, OR-2, PY-4, OR-13, OR-15, OR-16, PY-6 | text stale for PY-7 (9) |
| RL-1 to RL-8 | OR-3, OD-7, OR-6, RT-8, OR-10, PY-5, RT-7, BR-7 | RL-1 test (32) |
| FL-1 to FL-11 | FL-1 to FL-3, B-9a, B-9, B-12, OR-7; FL-11 is a §2.10 seam | none |
| RS-1 to RS-9 | OR-6, OR-8, OR-3, OR-4, OD-2, PY-8, §2.1, OR-5, RT-4; RS-9 is a §2.10 seam | none |
| DV-1 to DV-9 | RT-5, OR-5, B-8, OR-10, FL-1, OR-3, OR-4, BR-7, PR-5, PR-6, OR-1, OD-2 | none |
| X-1 to X-12 | OR-8, OR-3, OR-4, B-10, OR-6, OR-10, PY-8, FL-2, OR-2, OR-13, PY-11, IN-6, PY-1, FL-1 | X-5 test (33); X-12 text (38) |

No in-scope must-requirement is missing.

### L3 findings, checked against the current text

| Report | Findings | Present in text | Not present |
|---|---|---|---|
| verify-L3-a | 19 | 18 | a1: present in the schedule and plan, contradicted by the decision log (1) |
| verify-L3-b | 31 | 30 | b10: B-14 still names `hares_io::input_schema` (4) |
| verify-L3-c | 20 | 20 | 0 |
| verify-L3-d | 57 | 57 | 0 |
| verify-L3-e | 29 | 28 | e29: decision-log row label (15) |

### Schedule against git

| Check | Result |
|---|---|
| Every ledger entry listed exactly once (allowed exceptions: OD-2, OD-10, B-14) | yes. OD-4, OD-5 and OD-16 are gaps or contingent, not entries; BR-5 and SL-1, 4, 5, 6, 9, 10 are closed inside rows |
| Commit subjects of rows 1 to 18 equal `git log` | yes, for the commit each row names |
| Every executed commit is in a row | no: seven are not (3) |
| Executed-state column | absent (3) |
| Consumer rows 0b, 0c, 13 | subjects equal `824805e`, `a6d1345`, `5c651bc` |

### Outside references

Every reference outside this directory falls into one of these groups:
- the engine and consumer repositories, including `.workstrees/` and `.shipwright/archive` as
  SL-8 targets;
- runtime cache paths;
- public issue numbers;
- the implementer's records.

Two notes:
- The bundle path outside both repositories is covered by finding 2.
- `flow.yaml` and the first decision entry name `.shipwright/flows/chore.yaml`, a workspace
  tool file. It is the tool's own event record and needs no change.
