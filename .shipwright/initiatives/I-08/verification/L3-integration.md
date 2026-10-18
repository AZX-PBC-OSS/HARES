# L3 integration

Integrates `L3-resolution-a.md` to `-d.md` into the plan. Inputs:
`decision-log.md` (authoritative, read in full; not edited), `plan.md`,
`ledger/*.md`, the four resolution files. Code read with `git show` at
`afa9a071` (HEAD of `feat/output-results`) and `21d90588`; consumer read
at `5c651bc`. Neither code repository was edited. `implementation.md` and
`decision-log.md` were not edited.

## Execution state found

`git log feat/output-results` shows rows 17 and 18 committed after PR-3:
PR-5 `1d471677` and PR-6 `afa9a071`. The working tree holds uncommitted
changes to 48 engine files that remove `check_invariants` gates (row 19,
OR-11). Rows 16a to 16h have not run. The decision log's
`executed_order` ("PR-5 is in progress") and `next` ("Commit PR-5 only
after rows 16a to 16h") predate these commits.

## Conflict resolutions

1. **CN-17's position.** Resolution a placed CN-17 at row 16a, gated on
   PR-3 (`26626710`). Resolution d required it after PR-2 and before PR-3,
   with the rule that a later committed row moves it after that row and
   extends its range. The instruction for this pass applied that rule to
   PR-3. Applied to the repository as it stands, the same rule puts CN-17
   after PR-6: CN-17 keeps label 16a, gates on `afa9a071`, and its drift
   census covers rows 1 to 18. Rows 17 and 18 are listed where they ran,
   directly after row 16 and before 16a, keeping their numbers. This
   departs from the instruction's "census rows 1 to 16" only because PR-5
   and PR-6 are committed. If OR-11 commits before CN-17 lands, the rule as
   written in the schedule and in CN-17 moves CN-17 after row 19 and
   extends the census to it.
2. **Consumer-check labels.** Resolution d proposed "B0 (BR-6d)" for rows
   13a, 14 and 15. The check that counts for those rows runs at row 16g on
   the CN-17 consumer (BR-11's recorded check ran against an editable
   install; PR-1 to PR-5 have none), so the labels are "B0 (CN-17), at row
   16g" for rows 13a, 14, 15, 16 and 17, and row 18 is "B0 (CN-17): CN-17's
   gate at row 16a runs on this commit". CN-17's entry states that it
   changes no capture path, so the CN-17 consumer captures what `5c651bc`
   would.
3. **Where the B0 checks of committed rows run.** Resolution d asked for
   them before CN-17's gate on `5c651bc`; resolution a put them at row 16g
   on the CN-17 consumer. The schedule follows a (row 16g, after CN-17),
   with the reason in point 2. The step is now a section of
   `08-consumer.md` ("B0 re-check").
4. **B-14 scope.** Resolution d rejected the run-state extension and put
   three questions to the operator; resolution b extended B-14 to billing,
   metrics and `StepResult`. The decision log's widened
   `dl-i08-architecture.si_units` answers all three: everything HARES
   stores or computes is SI, output columns keep their reporting units.
   B-14 now carries it out in four commits (below).
5. **FL-5 and `Enum`.** Resolution c kept `Enum` in FL-5's end-to-end test
   (mean until B-11); resolution d asked FL-5 to exclude it. FL-5's
   end-to-end test now excludes `Enum`, so it holds unchanged after B-11;
   its unit test keeps the interim `Enum` case, which B-11 deletes; B-11
   adds `fleet_enum_cells_follow_mode` on the same `Pattern("*")` run.

## Changes by file

### `ledger/00-schedule.md` (rewritten in place)

- Header: entries with more than one row are OD-2 (31, 94e), OD-10 (24a,
  94f) and B-14 (94c, 94g, 94h). Rows 0 to 18 committed; rows 17 and 18
  ran before 16a and are listed there.
- Rules: opening sequence through PR-6 and OD-15 before OR-11; B0 rule
  names PR-1 to PR-6 and the five 16g worktrees; the CN-17 placement rule
  (gated on the last committed engine row; at `afa9a071`, rows 1 to 18;
  CN-13 covers every later engine row to 26); numeric list adds B-14's
  three commits, OD-2 site 6, OD-10's numeric commit and OD-20; B-14's
  first commit noted as adding fixtures inside the numeric block.
- Rows: 16 to "B0 (CN-17), at row 16g"; 17 and 18 moved up with their
  commits; 16a CN-17 with d's commit subject, "drift census of engine rows
  1 to 18", check on `afa9a071`; 16g re-checks 13a, 14, 15, 16 and 17 on
  five worktrees; 23 OD-3 and 24a OD-10 new titles and commits; new 24f
  OD-17, 24g OD-19, 30b OD-18, 94f OD-10 numeric, 94g B-14 commit 3, 94h
  B-14 commit 4, 94i OD-20; 57 PY-4 title and commit with
  `metrics_from_table`; 90 B-8 new title, `refactor:` commit and "golden
  (zero delta)"; 94c B-14 commits 1 and 2 with the new capture subject;
  95 CN-3 title names the SI renames.
- Contingent rows: row 18 failing CN-17's gate is handled like a 16g
  failure.
- Checked by script: every ledger entry appears exactly once (SL-2, SL-3
  are standing rules, SL-9 is closed inside row 3); every schedule commit
  subject appears verbatim in its owning ledger file; every "row N"
  reference in the plan and ledger resolves to a schedule row.

### `ledger/09-numeric-fixes.md`

- B-14 retitled "HARES stores and computes in SI units"; Kind lists four
  commits (1: capture, row 94c; 2: equipment config, row 94c; 3: run
  state, row 94g; 4: control layer, row 94h).
- Evidence at `afa9a071` for commit 3 (`BillingState`, metrics totals,
  `StepResult` and the Python and fleet step dicts, `ElectricalSummary`,
  `CoreFlows`, ports, `DwellingTelemetry`, island and ports dicts, gym
  observation keys and `RewardContext`, equipment run state and
  checkpoints, scheduled-load gas rates, premise ZIP weights, PV sizing,
  `CHECKPOINT_VERSION`) and commit 4 (actor parameters, the DR
  `PowerLimit` string, `ControlSignal` fields and their Python
  constructors, the HELICS signal key, gym action keys, BMS and EV-driver
  state and telemetry, actor blobs).
- "Not in scope" replaced by "Boundaries that keep their units" (output
  records, HELICS publications, tariff rate data, datasheet catalogues,
  defaults files, `heating_capacity_kbtu_h`, PySAM adapters, any fleet
  per-home rollup).
- Change for commits 3 and 4: SI names; one conversion at the row plan
  (`Unit::from_si`), the tariff evaluator and the metrics calculator;
  `metrics_from_table` converts with `Unit::to_si` and OR-15's equality
  tests move to a 1e-12 relative tolerance; `uom` added to `hares-types`
  and `hares-tariff`; snapshots renamed and `CHECKPOINT_VERSION` 9 to 10;
  `StepResult.net_electric_power_w` and the Python key; gas rates and PV
  sizing with `REMOVED_KEYS` rows; actor parameters (`ActorParameter`
  scope), control-signal fields (`ControlSignal` scope), observation
  names (`Observation` scope); actor telemetry published in SI to
  variables that keep their reporting names and units; actor
  `schema_version` raised.
- Commit 2 gains: the `register_pv_roof_shading` key read follows the
  rename (handed to OD-20); the test-scope sentence of
  `equipment_config_fields_are_si` corrected; its Numeric changes line.
- Acceptance tests for commits 3 and 4, including `si_census`;
  Verification and the guard for commits 3 and 4 (relative difference
  above 1e-9, or a unit factor, stops the entry; a flipped discrete
  decision is recorded with its first step); four commit subjects;
  Dependencies (PY-9, RT-7, OR-1, OR-10, FL-3; CN-3 drift including
  `ami2/dr_actuation.py:257`); Measurement per numeric commit.
- B-2: the two removed-key rows are scope `KeyScope::DwellingTable`.
  B-9a: `hpxml_dir` is `KeyScope::PythonKeyword`.
- Preamble: B-14 scope sentence; a rule that each number-moving commit
  adds its `docs/python.md` "Numeric changes" line (B-8 and B-9a move no
  number); a rule citing `plan.md` §3's numeric-entry evidence; B-14's
  placement sentence.
- Each of B-1a, B-1b, B-1c, B-2, B-3, B-4, B-5, B-6, B-7, B-9, B-10, B-11,
  B-12 and B-13 names its "Numeric changes" line in Verification.
- B-11: deletes FL-5's interim `Enum` unit case; new
  `fleet_enum_cells_follow_mode` on FL-5's `Pattern("*")` run; every golden
  compares bitwise; delta recorded on that run.
- B-12: starts from FL-4's three Utf8 renderings; owns the move to the
  naive `Timestamp` and the rewrite of FL-4's label tests; tests state the
  FL-4 strings they replace; measurement names the label delta.

### `plan.md`

- §1: the 09 area line names the full SI scope.
- §2.1 Units: everything stored or computed is SI, per the widened
  decision; three commits of B-14; the three conversion points.
  `StepResult` paragraph: the electrical solver moves to W, the column
  keeps its name and unit and the row plan converts. Snapshot units:
  B-14 commit 3 raises `CHECKPOINT_VERSION` to 10; commit 4 raises actor
  `schema_version`s only.
- §2.2: `Unit::from_si` and `Unit::to_si`; the published-value contract
  (SI in, declared reporting unit out, from B-14 commit 3).
- §2.7: `KeyScope` gains `ActorParameter`, `ControlSignal` and
  `Observation` (added by B-14 commits 3 and 4); surfaces that consult
  each scope; the qualified key formats (`EquipmentConfig`
  `<ConfigType>.<key path>` with `[]`, variant paths and `<n>`,
  reconciled with B-14; `ActorParameter`; `ControlSignal`;
  `Observation`); table rows for the new B-14 rows; `InputSchema` gains
  `rejected_hpxml_elements` with `RejectedHpxmlElementSchema`,
  `RejectedHpxmlElement` and `HPXML_REJECTED_ELEMENTS` (crate `hares-io`).
- §2.8: B-14's three numeric commits; OD-2 site 6, OD-10's numeric commit
  and OD-20 listed; the Numeric changes rule is per number-moving commit.
- §3: opening sequence and executed order through PR-6, CN-17 on
  `afa9a071`, the five 16g worktrees; the thermal-solver scope list adds
  B-14 (`electrical_solver.rs`).
- §5: no open question; the interpreter range recorded as decided
  (`python_support`); the SI scope recorded.

### `ledger/08-consumer.md`

- Order step 1: the six engine rows committed while the consumer stayed
  at `5c651bc`; CN-17 lands after the last committed engine row (row 18).
- Order step 2: rows 13a to 18 have no valid B0 check; row 18 through
  CN-17's gate, 13a to 17 at row 16g; no gate or capture against an
  editable install; step 4's dry-run line recorded with every run.
- Drift assignment: CN-17 rows 1 to 18; CN-13 rows 16b to 16h and 19 to
  26; CN-3 rows 83 to 94i, naming B-14's step-key and control-signal
  renames. Order step 5 and CN-3's Dependencies add B-14's three commits,
  OD-2 site 6, OD-10 numeric and OD-20.
- CN-17: gate on a worktree of `afa9a071`; records the base skip set (the
  conditional on BR-6d's schema is gone); establishes BR-6d's acceptance
  with the `1d60f478` fallback; census rows 1 to 18; no capture-path
  change; Dependencies state the position rule and the move from PR-2.
- New section "B0 re-check (`00-schedule.md` row 16g)".
- Consumer gate base-skip sentence names CN-17's gate.
- CN-3 Change: every consumer read of `net_electric_power_kw` from the
  engine step dict (`dr_actuation.py:257`, `cosim.py:94, 107`,
  `coupling.py:412`, `suite_ami2.py:822, 879, 1767`) and of
  `mean_power_kw` (`cvr_actuation.py:137`) moves to the W keys through one
  helper; census command; the consumer's own `IntervalRecord` stays in kW.

### `ledger/11-other-defects.md`

- New OD-20 (row 94i, numeric): PV roof coverage from the typed PV
  config, no substituted orientation or area, coverage above 1 an error,
  an unattached PV a warning; triage test run at the parent first.
- OD-2 site 6, OD-9 and OD-10's numeric commit name their Numeric changes
  lines; OD-10's numeric commit names row 94f; preamble lists the numeric
  entries.

### `ledger/04-fleet.md`

- FL-5: end-to-end test excludes `Enum` (B-11 checks it on the same run);
  the unit test's `Enum` case is deleted by B-11; Numeric changes line.

### `ledger/03-output-reporting.md`

- OR-5 (committed): the `clamped_actions` bullet now states what was
  committed (`record_clamped_actions` added, never called; the two statics
  still live at `21d90588`) and cites OD-18.
- OR-12: Dependencies add OD-18.

### `ledger/05-python-surface.md`

- PY-8: no longer names `_sim_config_to_kwargs`; OD-7 deleted it and the
  gym passes `config=sim_config`.
- PY-9: `billing_summaries()` and `dwelling_fatal` stay; new tests
  `health_counts_clamped_gym_actions` (both gym paths) and
  `billing_summaries_survive_finish`; Dependencies add OD-18.
- PY-2: B-14's rows named by scope.

### `ledger/06-runtime-memory.md`

- RT-4: `DwellingBlueprintBuilder::from_config` then `freeze()`; the
  builder's mutators; private blueprint fields; the rewrite of
  `equipment_id_regressions.rs:1037`.
- RT-6: the read-back name to config field mapping before and after
  B-14; setpoint sources copied from `EquipmentSpec.heating_setpoint_source`
  and `cooling_setpoint_source`; `SetpointSource` in `hares-io`, created by
  IN-5.

### `ledger/12-inputs.md`

- IN-3: `HPXML_REJECTED_ELEMENTS` with OD-19's four setback and setup
  elements and OD-10's `HasFlueOrChimney` (context
  `BuildingConstruction/extension`, checked in the fixtures), B-14's
  `active_power_kw` to `active_power_w` rename; test
  `rejected_elements_fail_the_parse_as_listed`; Dependencies add OD-10
  and OD-19.
- IN-4: its per-step weather function is the one B-2's
  `warmup_days_repeat_the_first_hour_rule` calls.
- IN-5: the precedence no longer lists setback and setup; the profiles are
  strict from OD-19; Dependencies add OD-19 and OD-17.

### `ledger/02-measurement.md`

- B-14's fixtures: `der_pv_battery_900s` with its self-consumption
  override, `der_pv_battery_ev_900s`, the resolved-model JSON files.
- New section "Records the schedule requires of later rows": CN-17's gate
  (PR-6's B0 check, BR-6d's acceptance, the base skip set, the census),
  the five 16g records, OD-15, RT-3's ideal-capacity share and 5%
  decision, FL-5's and B-11's `Pattern("*")` deltas, OD-9's delta, B-14's
  per-commit records, OD-10's numeric delta, OD-20's triage and delta.

### `ledger/07-profiler.md`, `ledger/01-baseline-repair.md`, `initiative.md`

- 07: commits of PR-1, PR-2, PR-3, PR-5 and PR-6 and where their B0
  checks run.
- 01: BR-10 and BR-12 to BR-15 run after PR-6 and CN-17.
- `initiative.md` Status: rows 0 to 18 committed; 16a to 16h next, before
  row 19.

## Sweep

- IDs: every entry ID referenced in `plan.md`, `initiative.md` and the
  ledger exists, except the contingent OD-16 and B-0 and the requirement
  IDs FL-9, FL-11 and PY-14, which `plan.md` §3 spells with their source.
- Types: no type is defined twice in `plan.md` §2; the new types
  (`RejectedHpxmlElement`, `RejectedHpxmlElementSchema`, the three
  `KeyScope` variants, `Unit::from_si` and `to_si`) have crate homes
  (`hares-io`, `hares-types`) that can see what they name; `hares-types`
  and `hares-tariff` gain `uom` in B-14 commit 3.
- Writing: no em or en dashes in `plan.md`, `initiative.md` or the
  ledger; no LLM tells; the only "agent" strings are the consumer's
  `AGENTS.md` file name; no planning ID in a commit subject.

## Unresolved, for the operator

1. `decision-log.md` `executed_order` and `next`, and
   `dl-i08-execution.progress` `next`, predate PR-5 (`1d471677`) and PR-6
   (`afa9a071`). They should record rows 17 and 18 as committed, CN-17
   gated on `afa9a071`, and the five 16g worktrees.
2. OR-11 (row 19) is being implemented in the main tree although
   `next` holds rows after PR-6 until rows 16a to 16h run and
   `plan-critique.md` records a clean round over them. If OR-11 commits
   first, the CN-17 rule moves CN-17 after row 19 and its B0 check joins
   the census and gate.
3. B-14 now spans the whole engine in three numeric commits. It needs a
   verification round before row 94c; the size of commits 3 and 4 is the
   main risk.
4. OD-20's cause is not pinned. The L3 code verification reported that
   roof coverage never registers for HPXML PV, but the code read for this
   pass suggests `base-pv.xml`'s 180° array would attach. The entry's
   first step is a test run at the parent; its delta may be empty.
5. The `CODECOV_TOKEN` checkpoint at BR-10 is unchanged (an operator step,
   not a decision).
