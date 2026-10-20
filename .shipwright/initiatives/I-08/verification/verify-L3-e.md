# Verification L3-e: traceability of plan.md and ledger/

Scope: `plan.md` and `ledger/*.md` against `decision-log.md`, the binding
rules (`feedback_*.md` in the operator's memory), `context.md` items 1 to
21, `review.md` V1 to V50, `exploration.md` requirement IDs, and every
finding of `verification/verify-L2-a.md` to `verify-L2-e.md`. Resolution
claims in `L2-resolution-*.md` and `L2-integration.md` were used only to
locate text; each was checked against the current plan and ledger text.
Code claims behind a finding were read at `ebdb1a17`, `8b09ac4a` or the
working tree with `git show`. Mechanical checks: em dash and LLM-tell
greps over `plan.md` and `ledger/` (none found), the `[writing]` pattern
over all 126 commit messages in the ledger (none match), schedule against
entry headings (every entry present).

Counts: **3 blockers, 9 majors, 17 minors.**

---

## Blockers

### 1. Nine HARES tests are skipped permanently and the gate accepts it

`ledger/pytest-skip-census.txt` fixes 9 skips as the accepted state of
`[pytest]`: three `pytest.importorskip("ochre")` modules
(`tests/python/test_ochre_parity.py:190`, `test_observer_diagnostics.py:89`,
`test_thermal_trace.py:45`), although the line runs `--all-extras
--all-groups` and so installs the `ochre` group, and six fleet tests in
`tests/python/test_py_conversions.py:164-214` skipped because "ResStock test
fixtures not available". No entry makes them run. The measurement log
records the consequence: "the census's three 'OCHRE not installed' module
skips hide the interop surface" (`02-measurement.md:709-710`); the pint
0.25 break of the vendored OCHRE passed the block unseen for that reason.

Contradicts `feedback_no_ignore_tests` ("If a test needs a fixture file,
create or find the fixture"), `feedback_no_broken_windows`, and
`feedback_ochre_parity_rules` rule 1 (OCHRE comparison required); leaves
requirement X-12's acceptance ("parity tests pass unchanged using the
preset", now the Python adapter mapping of IN-6) unprovable by the gate.

Correction: add a found-defect entry to `11-other-defects.md` (OD-13,
scheduled before PY-7, row 25a) that (a) makes `ochre` importable in the
block's venv (the vendored `vendors/OCHRE` as a path dependency of the
`ochre` group, or whatever install the parity tests need), and (b) gives
the six fleet tests a committed fixture (the `resstock_2025_fleet` buildings
of B-9a or a committed HPXML set), with acceptance "the census file is
empty" (or each remaining line carries a recorded operator decision). The
census change is written in the entry's own commit as M-0 item 2 requires.
If (a) needs the pint cap lifted first, order it after the fix of
blocker 2.

### 2. The pint cap contradicts the dependency decision and has no entry

`dl-i08-architecture.dependencies`: "Bring every outdated dependency and
tool to its latest release ... with every compile error and lint the new
versions raise fixed properly, not suppressed". BR-6c's change
(`01-baseline-repair.md:483-486`) instead lets a cap stay "with a comment
naming the incompatibility", and as executed `pint<0.25` stayed
(`dl-i08-execution.br6c-notes`, an implementer record, not an operator
decision). A retained cap is the suppression the decision forbids, and
§1's rule (a finding against a committed entry becomes a new fix entry)
was not applied.

Correction: either (a) a new fix entry in `11-other-defects.md` that
restores compatibility where HARES owns the code (define `inch_H2O_39F` in
the pint registry the parity harness and the Python adapter use before
OCHRE loads), lifts the cap to the latest pint, and passes `[pytest]` with
blocker 1's entry running the OCHRE tests; or (b) an operator decision in
`decision-log.md` amending `dependencies` to allow this cap. In both cases
rewrite BR-6c's change to drop the general "the cap stays" escape hatch
and append the as-executed caps (`maturin<2` and `pandas<3.0` removed,
`pint<0.25` kept and why).

### 3. verify-L2-e finding 2 is unresolved

- `review.md:72-73`: V49 ("An unfinished plan draft left several
  output-model gaps ... confirmed as gaps in that draft ... superseded by
  the decisions below") and V50 ("An early sink-contract draft ...") are
  history of earlier drafts, against `dl-i08-process.documents` ("hold only
  current, correct content") and `writing`.
- `initiative.md:36-40`: "The plan is ready to execute only when a round
  records zero blockers and zero majors. Implementation does not start
  before that." contradicts `dl-i08-process.readiness` status (the opening
  sequence is cleared and has executed through row 13g). The reading list
  (`initiative.md:26-34`) omits `ledger/`, `ledger/00-schedule.md`,
  `implementation.md` and `verification/`.

Correction: replace V49 with "No output-model contract may default a path
to the working directory, omit `Mode`, skip `reward_fn`, or leave the fleet
column-mismatch policy unstated | requirement | none: §2.2, §2.4, §2.6 and
PY-5 state each" and V50 with "A file destination always names its path |
requirement | none: `Destination` (§2.2)", or delete both rows and leave
the IDs unused. Rewrite the `initiative.md` Status to "The opening sequence
(schedule rows 0 to 13g) is cleared and executed. Rows 14 onward start only
after a verification round over them records zero blockers and zero majors
in `plan-critique.md`." and add the four missing documents to the list.

---

## Majors

### 4. Committed entries still state superseded facts

`dl-i08-process.documents` requires superseded content to be rewritten in
place. Six L2 findings (L2-a B1, M3, M11, m1, m17, m19) were fixed in code
or in a side section, not in the entry text; a reader of the entry gets the
wrong fact. Corrections, each appended to the named entry as its
as-executed record:

- BR-1 Verification (`01:51`): "turns `[test-observe]` and
  `[test-observe-profiling]` green".
- BR-2 (`01:102`, `:115-117`): "seven lints: the five at `ebdb1a17` and two
  `ptr_arg` in `crates/hares-core/examples/profile_sim.rs:20` added by
  PR #27, fixed in `fd23dd80` by taking `&Path`".
- BR-7 change (`01:256-262`): tests at `output_verbosity >= 4` also pass an
  `output_path` in a temporary directory, because the diagnostics writer
  ignores `write_output`; TOML-built configs (`from_toml`, synthetic TOML,
  parity `config.toml`) are sanitized too; both done in `81cc6d85`.
- BR-8 (`01:324-325`): "the ignored instances were 12 (default features)
  and 16 (`--all-features`), the bestest module compiling into two targets;
  0 after BR-8".
- BR-6c: see blocker 2. BR-6d (`01:512`): `ochre-next = { path = "HARES",
  editable = true }`.
- M-1 (`02:66`): `.gitignore:49`; (`02:145`): "updates this module in that
  entry's commit" (L2-e 44); and every other line the "golden format as
  built" section (`02:408-460`) overrides is rewritten in M-0, M-1 and M-2
  themselves, after which the section keeps only "Changed by later
  entries".
- SL-9 (`10:` SL-9): "Fixed by: `01-baseline-repair.md` BR-2 (`fd23dd80`
  gated the imports and `config`); `07-profiler.md` PR-1 deletes the
  example." Delete the "Fix" sentence that describes work already done.
- `plan.md` §3 Task 0 step 2 (`plan.md:1398-1410`) prescribes a
  fast-forward to `origin/initial-implementation` and a check that HEAD
  equals it, which the executed Task 0 (`1d60f478`, a local merge, nothing
  pushed) does not satisfy. Rewrite step 2 as executed: "`feat/output-results`
  was fast-forwarded to the local merge `1d60f478`; the GitHub merge of
  PR #27 and the matching `initial-implementation` update are operator
  checkpoints of the closing sequence."

### 5. `plan.md:1196` states a false dependency fact (L2-b 2, new error)

"hares-types already depends on serde_json": `crates/hares-types/Cargo.toml`
has `serde_json` only under `[dev-dependencies]` (checked at `8b09ac4a` and
in the working tree). `FieldSchema.default: Option<serde_json::Value>` then
does not compile. Correction: the comment reads "IN-3 adds `serde_json` to
`hares-types` `[dependencies]`", and IN-3's change states that move.

### 6. The blueprint builder's API is not defined (L2-e 4, new error)

§2.7 (`plan.md:1039-1045`) declares `DwellingBlueprintBuilder` opaque with
only `new` public, `freeze()` appears only in a comment, and
`DwellingBlueprint::from_config` returns the builder. RT-4
(`06:424`) writes `DwellingBlueprint::from_config(..)?.build()`, which does
not type-check, and "owns the mutators" names none. Correction: add to the
§2.7 impl block `pub fn freeze(self) -> Arc<DwellingBlueprint>;` and the
mutator signatures RT-4 moves (`add_equipment`, `replace_equipment`,
`remove_equipment`, `set_overrides`, as they exist on today's blueprint),
and change RT-4:424 to `DwellingBlueprint::from_config(..)?.freeze().build()`.

### 7. PR-6's allocation test needs a constructor the contract does not have (L2-a M12, new gap)

`actor_timings_record_without_allocating` (`07:362-368`) builds "an
`ActorTimings` built for 2 slots" in an integration test, but §2.9
(`plan.md:1315-1316`) gives `ActorTimings` only `record`, and §2.7 makes it
opaque. Correction: add `pub fn new(names: impl IntoIterator<Item =
Arc<str>>) -> Self` to `ActorTimings` in §2.9 and PR-6's change.

### 8. FL-5's acceptance test cannot pass on `Enum` variables (L2-c 2, new error)

FL-5 keeps `Enum` at `Mean` over time until B-11 (`04:575-580`), while each
home's own `Memory` report reduces `Enum` by `Mode` from OR-13
(`03:1481-1483`). `fleet_cells_follow_declared_kinds` (`04:587-591`)
compares "every aggregate variable" with the home's own report, so any
`Enum` cell fails. B-11's verification (`09:1231-1233`) claims the two fleet
goldens change "their mode columns", but both goldens use the verbosity-2
selection (`04:292-293`) and no evidence shows an `Enum` variable in it.
Correction: FL-5's test excludes `Enum` variables ("`Enum` cells are
checked by B-11"); B-11 names a fixture whose selection holds an `Enum`
variable (or adds the `Enum` variable to `fleet_same_offset_300s`'s
selection in its own capture commit) and states which goldens change, or
states that none does and the unit tests carry the delta.

### 9. OD-2 site 6 moves numbers outside the numeric block with no rule (L2-e 6, partial)

`plan.md` §2.8 (`:1280-1282`) and the schedule rule (`00:21-25`) put every
found defect that moves numbers in its own numeric-block row after the
consumer block. OD-2's second commit (row 31, site 6, a constant setpoint
override replacing the schedule and HPXML sources) records a delta but
stays at row 31 under check B0 (CN-13). Correction: either move site 6 to
its own row in the numeric block (between 94a and 94b, consumer check
"delta"), or add to §2.8 and the schedule rule: "OD-2 site 6 is the one
numeric change before the consumer block: no golden fixture and no
consumer path sets a constant setpoint override (`11:113-117`), so B0 and
every golden stay bitwise; its delta is recorded in its own commit."

### 10. `review.md` fleet reasoning contradicts the ledger's facts (L2-e 15, unresolved)

`review.md:327-337` says today's fleet "folds each home's result as it
completes, in completion order" and that the fix folds "by building id".
`04-fleet.md:19-20` establishes that `collect` keeps entry order, and §2.4
and FL-1 fold in entry order. Correction: rewrite the Reasoning and
Rejected-alternative paragraphs: today every home's frame is collected in
entry order before aggregation, so memory grows with the fleet; V30 is
vacuous for the reasons in its own row; the fold keeps entry order in
fixed chunks so memory is bounded and the sum is independent of chunk size
and thread count. Replace "by building id" with "in entry order".

### 11. `exploration.md` cites a document outside the initiative (L2-e 16, unresolved)

`exploration.md:320`: "Per `needs-hares-users.md` section 6.3, cross-checked
against `decision-log.md`:". Correction: "Cross-checked against
`decision-log.md`:".

### 12. Non-SI state in types the plan defines

`feedback_si_units` ("store and compute in SI units internally") and
`feedback_si_units_strict` ("no mixed units"); `dl-i08-architecture.si_units`
says kW, kWh and kVA config fields "break the SI rule" and migrates only
equipment config fields. The plan defines new serialised contracts in kW
and kWh: `BillingSnapshot` (`cumulative_import_kwh`, `peak_demand_kw`,
`period_peak_demand_kw`, `prior_peaks_kw`, `cumulative_ev_kwh`;
`plan.md:273-288`), `MetricsAccumulators` (cloned from today's
`total_electric_energy_kwh` onward, `plan.md:308-311`), and keeps
`StepResult.net_electric_power_kw` (`plan.md:208`). The checkpoint format
OR-10 creates makes these units a persisted contract. No entry and no
decision scopes them. Correction: either extend B-14 (or a new numeric
found-defect entry after it) to store billing, metrics and step power in
W and J, converting to kW, kWh and currency per kWh only at tariff pricing
and output, before OR-10's snapshot types are frozen into checkpoints
(OR-10 is row 48; reorder or carry the conversion in OR-10); or record an
operator decision in `decision-log.md` that tariff and reporting
quantities are boundary units in kW and kWh.

---

## Minors

13. **Schedule rule names non-mechanical rows as migration.**
    `00-schedule.md:16-18` says "The mechanical consumer migration (CN-1 to
    CN-11, CN-13) lands before the first numeric fix", but CN-3 (row 95)
    and CN-2 (row 96) run after it. Correction: "(CN-1, CN-5 to CN-11 and
    CN-13; CN-2, CN-3 and CN-4 follow the numeric fixes, `08-consumer.md`
    order step 5)".
14. **B0 labels (L2-d 16, partial).** Rows 13a to 13g read `B0 (CN-16)` and
    the legend carries an exception for them; `08:38-40` says the
    CN-0/CN-16 consumer runs every engine row before PY-7, while BR-6d and
    CN-17 change the consumer at rows 13 and 13-1. Correction: relabel rows
    13a to 13g `B0 (CN-17)`, delete the legend's parenthetical, and in
    `08` order step 2 write "the consumer at CN-16 runs rows 1 to 12; the
    consumer at CN-17, which includes BR-6d, runs rows 13a to 25".
15. **SL-9 appears in two schedule rows.** `00:47-48` closes SL-9 in row 3
    and row 14. "Every ledger entry appears exactly once": write "SL-9
    (row 3, BR-2's `fd23dd80`)" and move "row 14, PR-1, deletes the
    example" into PR-1's own entry.
16. **CN-6 leftover (L2-d 7).** `08:1037-1038` "reports a difference of
    exactly 2 per fit" contradicts the variable `s`. Correction: "exactly
    `s` per fit (2 when both standalone probes were submitted)".
17. **CN-3 dependencies (L2-d, new).** Order step 5 (`08:62-64`) runs CN-3
    after FL-5; CN-3's Dependencies line does not list `04-fleet.md` FL-5.
    Add it.
18. **BR-10 wording (L2-a M4, new).** Row 13c's title (`00:86`) "runs the
    full block" should read "runs every cargo line of the block"; row 5's
    title likewise; BR-10's acceptance clause that the first CI run of the
    closing-sequence pull request passes (`01:578-581`) cannot be checked
    at row 13c: move it to row 99a's record.
19. **PY-8 test over Python-only keys (L2-e 8, new).**
    `removed_key_messages_equal_across_toml_and_python` (`05:185-187`) runs
    over the four site keywords, which have no TOML form (`plan.md:907`).
    Restrict it to rows present on both surfaces; the site keywords are
    covered by PY-2's `removed_keyword_names_replacement`.
20. **IN-3 census grep (L2-b 17, partial).** The line-based `git grep` misses
    nine multi-line `.path(&[` calls (`building.rs:558`,
    `resolve_loads.rs:57,118`, `resolve_water_heater.rs:442,458,515,641,658,664`)
    and the `required_paths` table (`validation.rs:255-281`), and "58" is 67.
    Use `git grep -P -z` (or `rg -U`) with `\s*` between `&[` and `"`, add
    a pattern for `&[&str]` literals in `hpxml/`, and correct the count; or
    state that the `&'static HpxmlElement` accessors are the gate and the
    grep is advisory.
21. **Time-relative wording (L2-e 28, unresolved).** `01:411` "when this plan
    was written" and `01:481` "at plan time": write "at 2026-09-29".
22. **References to memory files.** RT-1's Kind (`06:12-13`) cites
    "binding rules `feedback_custom_domains_perf` and
    `feedback_hot_loop_minimal`", files outside the initiative directory.
    Replace with the rule itself: "no per-step allocation, search or sort
    in the step path (`dl-i08-criteria`; `dl-i08-architecture.engine_performance`)".
23. **The thermal-solver rule and the scope decision.** `feedback_no_touch_thermal_solver`
    forbids editing the thermal solver; `dl-i08-process.scope` ("including
    the thermal solver") overrides it for I-08. Only `06`'s preamble says
    so, and only for RT-3 and SL-7. Add to `plan.md` §3: "`dl-i08-process.scope`
    puts the thermal solver in scope for I-08; the entries that edit
    `hares-envelope` are RT-1 (domain readers), RT-3, SL-7, OR-11
    (`fluid_solver.rs` checks), B-8, IN-7 and B-13; each is bitwise against
    the goldens or a declared numeric entry."
24. **Context item 5, the ideal-capacity solve.** `solve_ideal_capacity_for_target`
    (6.8% of `simulate()`) is only measured by RT-3; its compute (not its
    allocations) has no profile rule. Add to RT-3's measurement: "if, after
    this entry, the ideal-capacity phase is at or above 5% of `simulate()`
    on `consumer_shape_900s`, the next row adds an entry that removes the
    repeated resolve; otherwise the measurement is recorded as the drop
    `dl-i08-architecture.engine_performance` requires."
25. **Requirement R13.** Numeric changes are recorded in the measurement
    log, which does not ship. Add to PY-9's "Changes in this release" a
    "Numeric changes" list (each numeric entry's commit subject and the
    golden fixtures whose digests changed), checked by a test against the
    golden manifests' recorded captures.
26. **Requirement PY-14.** PY-9's "Results" documentation covers
    `simulate()`, stepping, `finish()` and `health()` but not
    `initialize()`, which OR-9 keeps. Add `initialize()` and when it is
    required, or state its removal if OR-9 removes it.
27. **An open interpreter question is not listed.** `dl-i08-execution.br6c-notes`
    leaves `requires-python ">=3.12,<3.14"` as "a test-matrix decision for
    the operator", while `plan.md` §5 says "No open operator decisions".
    List it in §5, or record the decision.
28. **Two citations for the same sites (L2-b 14).** OR-5 cites
    `mod.rs:1177-1195,1228-1233,1262-1279`; §2.3 cites `1195-1210,1250-1302`.
    Use one commit's line numbers in both.
29. **Stale row label in the decision log.** `dl-i08-execution.progress`
    `next:` says "M-3 (rows 10 to 13a)"; M-3 is row 13b and 13a is BR-11.
    Correct the row numbers or name the entries without row numbers.

---

## Appendix A: decisions (`decision-log.md`)

| Decision | Where implemented | Acceptance | Status |
|---|---|---|---|
| criteria | plan §3 rules; every area | per entry | covered (see 1, 2, 12) |
| process.purpose / found_defects | plan §1; 11, 09 B-9a, PY-12 | per entry | covered |
| process.plan_form | every entry (Problem with file:line, Change, Acceptance, Commit, Dependencies) | named tests | covered; SL-2/SL-3 are standing rules |
| process.readiness | plan §1, `initiative.md` | n/a | contradicted by `initiative.md` (3) |
| process.independence | plan §3 Independence, Per-entry review | n/a | covered |
| process.documents | all files | n/a | contradicted (3, 4) |
| process.writing | plan §3 Writing, `[writing]` | `[writing]` | covered in plan/ledger; `review.md` (3), `01` (21) |
| process.branch | plan §3 Task 0 | n/a | covered; step 2 stale (4) |
| process.scope | `06` preamble | n/a | partial (23) |
| architecture.results | OR-6, PY-9 | `stepped_finish_equals_simulate`, `python_stepped_finish_equals_simulate` | covered |
| output_model | OR-1, OR-2, OR-8 | OR-1/OR-2 tests | covered |
| destinations | OR-3, OR-14, PY-10 | OR-3 file tests, `callback_*` | covered |
| python_export | PY-1 | `second_export_does_not_recopy` | covered |
| config | PY-8, PY-2, OD-2, IN-6 | `python_keyword_names_equal_rust_field_names` | covered (19) |
| inputs | RT-4, RT-6 | `blueprints_from_different_hpxml_share_weather` | covered (6) |
| new_capability | OR-1/PY-4, OR-13, RT-6, IN-2, OR-12, OR-15, IN-4, OR-16, IN-3; seams §2.10 | per entry | covered |
| health | OR-5, OR-11, OR-12, RT-5 | OR-5 tests | covered |
| time | OR-7, B-6 | `docs` time example | covered |
| weather | B-1a, B-1b, B-1c | per entry | covered |
| aggregation | OR-13, FL-1, FL-5, B-11 | per entry | covered (8) |
| fleet | FL-1 to FL-4, B-12 | per entry | covered |
| numeric_fixes (12 items) | B-1a to B-12 | per entry | covered |
| precedence | OD-2, B-13, IN-9 | per entry | covered |
| reproducibility | RT-5, FL-1, M-4a/b | `concurrent_dwellings_match_sequential`, spawned-process test | covered |
| package | PY-7, CN-13 | rename census | covered |
| engine_performance | RT-1 to RT-3, SL-7, RT-5 | allocation/factorization tests | covered (24) |
| dependencies | BR-6a to BR-6d | block, `[advisories]` | contradicted for pint (2) |
| metrics_optional | OR-8, CN-7 | OR-8 tests | covered |
| weather_edge_cases | B-1a, B-1c | per entry, E+ citations | covered |
| ochre_compat | IN-6 | IN-6 tests | covered (parity tests skipped, 1) |
| defaults_path | B-10 | B-10 tests | covered |
| calibration_r_factor | IN-7, B-13, CN-3 | per entry | covered |
| consumer_gate | CN-16, gate text | gate | covered |
| profiler_scope | PR-1 to PR-6 | per entry | covered (7) |
| leap_day_day_types | B-1c | `day_types_after_a_skipped_feb29_follow_the_calendar` | covered |
| si_units | B-14 | B-14 tests | covered for config; see 12 |
| golden_storage | M-1 | M-1 tests | covered |
| setpoint_sources | IN-5, IN-8, OD-9, CN-3 | per entry | covered |
| battery_control | OD-8 | OD-8 test | covered |
| rl_observation_allocation | RT-7, RT-8 | allocation test | covered |
| cross_platform_measurement | M-4a, M-4b | workflow | covered |
| consumer_inert_override_probe | OD-2, CN-1 (`08:670`) | CN-1 | covered |
| input_schema_scope | IN-3 | IN-3 tests | covered (20) |
| public_planning_docs | SL-8 | SL-8 grep | covered |
| slop_scope | SL-3, plan §3 | review pass | covered |
| stray_outputs | Task 0, BR-7, `[stray]` | `[stray]` | covered |
| consumer | 08 order, schedule | B0 gates | covered (13, 14) |
| execution.worktrees | plan §3, 08 | n/a | covered (no `hares-eut` left in plan/ledger) |
| execution.task0-state | plan §3 step 1 | n/a | covered; step 2 stale (4) |
| execution.paths-summary | 08 (`08:221-225`) | n/a | covered |
| execution.progress findings | OD-7 (gym `write_output`), OR-3/OR-4 (diagnostics), OR-11 census (hares-io, hares-envelope) | per entry | covered |
| execution.br6b-notes | measurement log | n/a | covered |
| execution.br6c-notes | OD-11, OD-12; pint, requires-python | per entry | OD-11/12 covered; pint (2), interpreter (27) |

## Appendix B: binding rules

| Rule | Where | Status |
|---|---|---|
| actor_telemetry | PY-11, `Actor::output_variables`, undeclared key fails | covered |
| always_review, no_self_review, fix_all_review_findings | plan §3 | covered |
| ashrae_not_ochre, best_physics, not_ochre_parity_target | 09 E+ citations, PHYSICS_DECISIONS sections | covered |
| code_quality, hot_loop_minimal, custom_domains_perf | RT-1, RT-3, RT-7, OR-2 | covered |
| design_for_all_users | plan §2, consumer logic in 08 | covered |
| deterministic_tests, test_quality | area test rules, plan §3 | covered |
| equipment_self_contained | §2.3 rejected signals, actors dispatch only | covered |
| fix_existing_not_new_features | overridden by `new_capability`; seams §2.10 | covered |
| local_time_only | §2.5; UTC offsets only where `fleet` decides | covered |
| max_5_agents, opencode_stateless, no_sed, no_parallel_same_files | execution practice; rows run one at a time | covered |
| no_ai_slop_public, no_ticket_refs, no_useless_comments | plan §3 Writing, `[writing]`, SL-2/SL-3 | covered (21, 22) |
| no_backward_compat, no_legacy_shims, no_silent_defaults | PY-8, IN-6, OD-2, OD-10, B-10 | covered |
| no_broken_windows | §1 found-defect rule | contradicted (1, 2) |
| no_code_fixes_from_reviews | n/a to plan | n/a |
| no_destructive_git, no_git_checkout, no_git_revert, no_git_stash, no_blanket_kill | plan §3 Branches, operator checkpoints | covered |
| no_ignore_tests | BR-8/BR-12 handle ignored helpers | contradicted by the skip census (1) |
| no_timelines | none found | covered |
| no_touch_thermal_solver | overridden by `process.scope` | stated partially (23) |
| ochre_parity_rules | tracing per entry; OCHRE comparison | parity tests never run (1) |
| pay_for_what_you_need | OR-3, OR-8, OR-5 | covered |
| shipwright_files_authoritative | all files | contradicted (3, 4) |
| si_units, si_units_strict, unit_boundary, use_uom | B-14, IN-8, IN-9 (`uom`) | partial (12) |
| test_coverage_reporting, tests_every_ticket | block after every row | covered |

## Appendix C: `context.md` items

| Item | Entry | Status |
|---|---|---|
| 1 telemetry clone, HashSet rebuild | RT-1 (SL-1) | covered |
| 2 forcing per dwelling | RT-2 | covered |
| 3 parse once | RT-4 | covered |
| 4 envelope solar, LU, `m_scratch`, slot map | RT-2, RT-3, SL-7 | covered |
| 5 thermal input preparation | RT-3 | partial (24) |
| 6 threads | RT-4, RT-5, CN-10 | covered |
| 7 unused checkpoint | OR-9 | covered |
| 8 output pipeline cost | OR-2, OR-3, PY-1 | covered |
| 9 HARES#28 | OR-3 | covered |
| 10 health only under observe | OR-5, OR-11 | covered |
| 11 stepped runs lose output | OR-6 | covered |
| 12 diagnostics CSV | OR-4 | covered |
| 13 fleet memory and status | FL-1, FL-2 | covered |
| 14 time as strings | OR-7, B-12 | covered |
| 15 profiler | PR-1 to PR-6 | covered (7) |
| 16 migrate call sites, `contract()` | CN-1 | covered |
| 17 worker pools, dead shrink | CN-5 (SL-5) | covered |
| 18 duplicate parent runs | CN-6 (SL-4) | covered (16) |
| 19 fit path, scratch output | CN-7, CN-8 | covered |
| 20 provenance and replay | CN-11 | covered |
| 21 surrogate trial | CN-12 | covered |

## Appendix D: `review.md` V1 to V50

| V | Entry or explanation | Status |
|---|---|---|
| V1, V2 | OR-5 | covered |
| V3 | OD-1 | covered |
| V4 | B-1a, B-1b | covered |
| V5 | B-1c | covered |
| V6 | B-4 (BR-14 fixture) | covered |
| V7 | OR-9 | covered |
| V8 | B-8 | covered |
| V9, V10 | OR-1 (seam), B-5 | covered |
| V11 | OD-2 site 1 | covered |
| V12 | OR-6 | covered |
| V13 | OR-4 | covered |
| V14 | OR-2/OR-6 structure, B-6 values | covered |
| V15 | PY-5 | covered |
| V16, V17 | FL-1, FL-2 | covered |
| V18 | PY-6 | covered |
| V19 | B-10 | covered |
| V20 | PY-2 | covered |
| V21 | refuted; `11` preamble names the pinning test | valid |
| V22, V23, V24 | B-2, B-3 | covered |
| V25 | OR-5, RT-5 census | covered |
| V26 | OR-6 | covered |
| V27 | RT-1 | covered |
| V28 | OR-10 | covered |
| V29 | OR-8, B-7 | covered |
| V30 | FL-1, RT-5 | covered |
| V31 | `11` preamble: already pinned by `from_elements_external_node_positions_match_build_matrices_columns` (`rc_network.rs:1037`, verified) and `multi_depth_ground_cols_match_assembled_matrix` | valid |
| V32 | PY-3 | covered |
| V33 | PY-4 | covered |
| V34 | OR-3 | covered |
| V35 to V39, V43 | OD-2 (V39 also IN-5) | covered |
| V40 | OD-2 (EV), OD-8 (battery), IN-9 | covered |
| V41 | FL-3 | covered |
| V42 | B-9a, B-9, B-12 | covered |
| V44 | OD-7 | covered |
| V45 | OR-6, PY-1 | covered |
| V46 | OD-3 | covered |
| V47 | refuted | valid |
| V48 | partial, addressed by OR-1/OR-8 by construction | valid |
| V49, V50 | planning-draft gaps | substance valid, wording is draft history (3) |

## Appendix E: `exploration.md` requirements

| ID | Entry (acceptance) | Status |
|---|---|---|
| R1 | OR-3, OR-5 | covered |
| R2 | OR-2 (`Named`, nearest names) | covered |
| R3 | OR-5, OR-12, PY-9 | covered |
| R4 | OR-1, OR-7 digest, B-5, PY-4 | covered |
| R5 | OD-2, IN-9 | covered |
| R6 | RT-6, IN-2 | covered |
| R7 | RT-4 content inputs, CN-1/CN-10 | covered |
| R8 | IN-3 | covered (20) |
| R9 | IN-2 | covered |
| R10 | OD-1; taxonomy not built, justified in §2.10 | covered |
| R11 | PY-2, B-10, OR-3 | covered |
| R12 | RT-5 spawned-process test (below-grade home) | covered |
| R13 | goldens, measurement log | partial (25) |
| R14 | PY-6 | covered |
| R15 | PY-9 changes section | covered |
| R16 | PR-1, PR-2, M-3 | covered |
| R17 | PY-3, RT-5, CN-10 | covered |
| R18 | OR-6 measurement | covered |
| R19 | not built; seam (§2.10, `new_capability`) | justified |
| R20 | OR-7 docs, B-2 | covered |
| R21 | OR-7 docs, IN-4 | covered |
| R22 | OR-16, CN-11 | covered |
| R23 | OD-2, OD-8, IN-9 | covered |
| R24 | PY-12 | covered |
| R25 | OR-10, PY-9 (fleet does not rewind) | covered |
| PY-1 to PY-13 | OR-6; OR-3; OR-3/§2.2; OR-7; B-6; PY-2/PY-8/PY-9 stubs; OR-8/B-7; OR-2; OR-13; PY-4; PY-1; OR-15; OR-16/PY-6 | covered |
| PY-14 | PY-9, OR-7 | partial (26) |
| RL-1, RL-2 | OR-3, OR-6 | covered |
| RL-3 | RT-8 | covered |
| RL-4 | OR-10 | covered |
| RL-5 | PY-5 | covered |
| RL-6, RL-8 | RT-7 | covered |
| RL-7 | BR-7 (bench temporary paths), OR-3 (nothing written unless requested) | covered |
| FL-1 to FL-8 | FL-1; B-9a/B-9/FL-2; FL-1; FL-2; FL-1; B-9/B-12; FL-1; OR-7/FL-1 | covered |
| FL-9 | FL-3 | covered |
| FL-10 | FL-1 `fleet_fold` bench at 100, 1 000, 10 000 | covered |
| FL-11 | not built; seam (§2.10) | justified |
| RS-1 to RS-8 | OR-3/OR-6; OR-3/OR-4; PY-8/OD-2; §2.1 `run` returns `HaresError`; `RunResult.metrics`/`ReportOutput`; OR-5; OR-6; RT-4 | covered |
| RS-9 | not built; seam (§2.10) | justified |
| DV-1 to DV-9 | OR-5/RT-5; RT-5/FL-1/OR-10; OR-4; BR-7/`[stray]`; OR-4 (`Diagnostics` with `Memory`); OR-3 bench groups; PR-5/PR-6; OR-1; OD-2 (`wall_area_m2`) | covered |
| X-1 to X-11 | OR-8; OR-3; OR-6; OR-6; OR-3 (Parquet/CSV schema test); OR-10/RT-8; PY-8; FL-2; OR-13; OR-3 measurement; PY-11 | covered |
| X-12 | IN-6 (Python adapter mapping) | not provable while OCHRE tests skip (1) |

## Appendix F: verify-L2 findings

Every finding was checked against the current text. Rows not listed are
resolved.

| Finding | Status | Here |
|---|---|---|
| L2-a B1, M3, M11, m1, m17, m18, m19 | fixed in code or a side section; entry text stale | 4, 2 |
| L2-a M4 | resolved; new wording problems | 18 |
| L2-a M12 | resolved; new constructor gap | 7 |
| L2-a, other | decision-log row label | 29 |
| L2-b 2 | resolution added a false fact | 5 |
| L2-b 14 | resolved; citation nit | 28 |
| L2-b 17 | partial | 20 |
| L2-c 2 | resolved; new test and golden problems | 8 |
| L2-d 7 | resolved; leftover sentence | 16 |
| L2-d 16 | partial | 14 |
| L2-d, other | CN-3 dependency | 17 |
| L2-e 2 | not resolved | 3 |
| L2-e 4 | resolved; new type error | 6 |
| L2-e 6 | partial | 9 |
| L2-e 8 | resolved; new test problem | 19 |
| L2-e 15 | not resolved | 10 |
| L2-e 16 | not resolved | 11 |
| L2-e 28 | not resolved | 21 |
| L2-e 44 | partial | 4 |

Totals checked: L2-a 34 (25 resolved), L2-b 24 (22), L2-c 24 (23), L2-d 22
(20), L2-e 46 (38).

## Appendix G: schedule and references

- Every entry in `ledger/01` to `ledger/12` appears in `00-schedule.md`:
  BR-5 inside row 4; SL-1, SL-4, SL-5, SL-6 inside their rows; SL-2 and
  SL-3 as standing rules; SL-9 in two rows (15). No row names an entry
  that does not exist; row 54 is explained.
- References outside the initiative directory: consumer repository paths,
  `~/.cache` paths, `.workstrees` and `.shipwright/runs/...` run records are
  allowed. Not allowed: `needs-hares-users.md` (11) and the memory file
  names in RT-1 (22).
- Writing: no em dash, no listed tic and no agent or LLM reference in
  `plan.md` or `ledger/`; no planning ID in any of the 126 commit messages;
  time-relative wording at `01:411`, `01:481` (21).
