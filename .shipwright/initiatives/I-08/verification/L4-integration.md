# L4 integration

Scope: the cross-owner items `verification/L4-resolution-a.md` through
`L4-resolution-d.md` hand to the files owned in this pass:
`ledger/00-schedule.md`, `plan.md`, `ledger/02-measurement.md`,
`ledger/04-fleet.md`, `ledger/06-runtime-memory.md`,
`ledger/11-other-defects.md`, `ledger/12-inputs.md`, `exploration.md`,
`review.md`, `context.md`. Each item below was checked against the
current text of its target file and, where it cites code, against
`.workstrees/audit-7b7a6a7d` (commit `7b7a6a7d`) before being applied.
This pass also carries the operator ruling received mid-round: schedule
and plan execution continues in the implementer's order rather than
waiting on verification rounds.

An item not listed under "Applied" or "Rejected" below was checked and
found already correct in the current text (most resolution items that
name `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md` or
`ledger/10-slop-cleanup.md` directly, which their own owners had already
written into those files).

## Operator ruling (mid-task)

Rewrote the readiness/gating language in `plan.md` §1 and §3 and in
`ledger/00-schedule.md`'s preamble and rules: implementation continues in
the implementer's order; the ledger follows the code, recording each
commit as it lands; verification rounds correct not-yet-executed rows
ahead of the implementer rather than gating when they start. Marked row
21 (RT-3) `in progress` in the schedule's State column, matching the
working tree.

- `ledger/00-schedule.md`: "Implementation continues in the implementer's
  order; the ledger follows the code: each commit is recorded here in
  the State column as it lands... Every committed row from row 19 on
  also names its independent audit in `verification/`; that audit's
  findings become ledger entries or follow-up commits the implementer
  picks up."
- `ledger/00-schedule.md` rule 1: "...then row 21 onward (row 21, RT-3,
  already in progress). Verification rounds keep running over rows not
  yet executed, correcting their entries ahead of the implementer, rather
  than gating the start of a row; row 94c's own verification round over
  `09-numeric-fixes.md` B-14 runs the same way."
- `ledger/00-schedule.md` row 94c: "its own verification round over B-14
  runs ahead of this row, correcting the entry rather than gating it".
- `plan.md` §1: "Implementation continues in the implementer's order
  (`decision-log.md`, `dl-i08-process.readiness`): the ledger follows the
  code, each commit recorded in the schedule's State column as it lands
  and audited independently by a reviewer who did not implement it."
- `plan.md` §3: "...then row 21 onward (row 21, RT-3, already in progress
  in the working tree). Implementation continues in the implementer's
  order: the ledger follows the code, each commit recorded in
  `00-schedule.md`'s State column as it lands and audited
  independently."

## Applied

### `ledger/00-schedule.md`

1. Row 16a commit message corrected to match `08-consumer.md` CN-17's
   current commit: `fix: the contract check reads the engine tree every
   gate resolves, and the bundle is written where the gate names`.
2. New row 47a: OR-17, before row 48 (OR-10), with its contingency note
   ("If OR-17's proof ... finds a difference ... row 48 ... moves after
   it") added to "Contingent rows".
3. Row 25 entry renamed to `SL-8 (fixes SL-11)`; SL-11 added to "Entries
   closed inside another row".
4. New row 24i already present for OD-22 (checked, correct); added rows
   24j to 24n for the found defects below (OD-23 to OD-27).
5. New rows 94j, 94k, 94l for B-15, B-16, B-17, between row 94i (OD-20)
   and row 95 (CN-3), exactly as `L4-resolution-d.md`'s "Schedule
   placements" table states.
6. Row 95 (CN-3) drift range and row 16a's gating/census checked against
   `08-consumer.md`'s current text (`CN-3 for rows 83 to 94l`; CN-17
   gated on SL-7, census rows 1 to 20): already correct, no schedule
   edit needed.
7. Schedule completeness check (every `01-baseline-repair.md` through
   `12-inputs.md` entry ID against the table): **pass**. Every ID appears
   exactly once as a row, except: `OD-2`, `OD-10` and `B-14`, which the
   preamble already documents as having rows of their own (31/94e,
   24a/94f, 94c/94g/94h); and `SL-2`, `SL-3` (standing rules, not rows,
   per the preamble) and `SL-9` (closed inside row 3, per "Entries closed
   inside another row"). No entry ID is missing and no row duplicates an
   ID outside those documented cases.

### `ledger/11-other-defects.md`

8. Five new entries for the found defects `L4-resolution-d.md` items 5 to
   9 hand over, each with Problem, Evidence (`file:line` at `7b7a6a7d`),
   Change, Acceptance test, Verification, Commit and Dependencies:
   - OD-23: "`defaults/battery/default_parameters.csv` ships beside
     `defaults/battery/*.toml` ... but battery defaults load from the
     TOML directory only."
   - OD-24: "a missing `vehicle_mapping.csv`, an unreadable file, a
     malformed row and an out-of-range field ... as warnings, not
     errors".
   - OD-25: "a load the resolver cannot size runs at exactly 0 kW for the
     rest of the simulation, with only a `tracing::warn!`".
   - OD-26: "away_charge_power_kw: 0.0` while the config-seeded path ...
     defaults the same field to 6.6 kW"; the fuel-economy quotient versus
     the catalogue; the duplicated L1 cap.
   - OD-27: "the generated stub states `params: dict[str, Any] | None =
     ...` (a default), but the Rust method carries no
     `#[pyo3(signature = ...)]`".
   Preamble's list of entries updated to cite OD-24, OD-25 (silent-path
   rule), OD-23 (dead-file deletion) and OD-26/OD-27.
9. OD-2 (site 6) and OD-10 (numeric commit) Verification sections each
   gained: "From row 94c `[golden]` also compares the `resolved` product
   ...; M-1's capture re-captures it, and [this commit] records its
   resolved delta, none expected since [it] touches no equipment
   parameter."

### `ledger/12-inputs.md`

10. IN-3's Change gained: "`09-numeric-fixes.md` B-15 adds the
    `SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod`
    elements ... to `HPXML_ELEMENTS` in its own commit".
11. IN-9's Verification gained its resolved-product delta note: "this
    entry records its resolved delta: `bldg0176775` and `bldg0442669`
    each gain an EV equipment parameter set ... that the unfixed-engine
    capture reports as absent."

### `ledger/02-measurement.md`

12. Dropped the inaccurate `git show c71c542:<file>` redirection
    description at the B0-capture log entry; replaced with
    `git checkout c71c542 -- <file>` and a note that from CN-17 on the
    bundle goes to `scratch/i08/<sha>/bundle.zip`, so nothing beside
    either repository needs restoring.
13. M-1's "Committed golden" section gained the `resolved` product's
    format: "per equipment in model order, the equipment's name and, per
    `ResolvedParameter`, the parameter's name, `Unit::symbol()` and its
    value as the 16-digit hex of `f64::to_bits`... it covers equipment
    parameters only ... From row 94c on, `compare` and `compare-all`
    compare it like any other product."
14. OR-17's proof records, IN-2's R9-target measurement, FL-1/FL-2/RT-2's
    new log records and B-15/B-16/B-14's census and measurement lines:
    checked against the owning entries (`03-output-reporting.md`,
    `12-inputs.md`, `04-fleet.md`, `06-runtime-memory.md`,
    `09-numeric-fixes.md`), which already instruct that their output
    goes into this file's log. None of these rows has executed yet, so
    no data exists to log; writing placeholder entries would be a
    speculative claim. No action taken beyond the format documentation
    above.

### `ledger/04-fleet.md`

15. FL-1's reducer construction named explicitly: "the per-home reducer
    is OR-13's `PeriodReducer` (`plan.md` §2.6): `PeriodReducer::new(frequency,
    calendar, rules)` ... the attached callback feeds each batch to
    `push_batch`, and the fold closes each home's reducer with `finish`".
16. FL-1's Dependencies gained OR-7's text: "from this row on the fleet
    reads each home's `Time` as naive local milliseconds and subtracts
    the home's `hares:utc_offset_s` to recover today's UTC epoch key".
17. FL-4's period-origin offset named on `PeriodCalendar` explicitly:
    "This commit adds that period-origin offset to
    `03-output-reporting.md` OR-13's `PeriodCalendar`"; its Dependencies
    line on OR-7 reworded to match.

### `ledger/06-runtime-memory.md`

18. RT-5's census row for `RANGE_WARNING_COUNT`/`RANGE_EVENTS`: "pushes
    each warning into a `&mut Vec<Warning>` the caller supplies
    (`03-output-reporting.md` OR-11)" (replacing "a sink the caller
    supplies").
19. RT-5's `ISLAND_UNSERVED_WARNED` and `NO_AMPACITY_WARNED` rows:
    "goes through `03-output-reporting.md` OR-11's `WarningLog::push_warning`"
    and "the blueprint's resolver warning collector (`03-output-reporting.md`
    OR-11) records a `Warning`".

### `plan.md`

20. §2.4: `Coverage` loses `weight_skipped` (comment updated to name
    only the rename); `HomeStatus` loses `Skipped`; `FleetError::NoCoveredWeight`
    carries `homes: RecordBatch` and gains `ReservedReportName`,
    `NoSuchHome`, `Telemetry`; added the pre-pass signatures
    (`hares_io::hpxml::read_site`, `hares_io::read_weather_meta`,
    `hares_core::dwelling::site_offset`) and the alignment-dependent grid
    rule for `AlignmentRequired`/`HeterogeneousTimeGrid`.
21. §2.7: noted `ForcingKey`/`ForcingTable` "exist only if RT-2's
    measurement keeps the surface-independent table"; added the
    `REMOVED_KEYS` rows for `estimate_pv_capacity`, `TariffEvaluator.step`,
    `EvConfig.max_charging_kw` and the three control-signal surfaces (the
    Rust gym action reader and the protocol bridge included); added the
    keyword-only rule for every B-14-renamed parameter.
22. §2.9: checked for an envelope-type listing to add `SolveScratch` to
    (`L4-resolution-c.md`'s conditional item); §2.9 lists no envelope
    types at all, so the condition does not apply and nothing was added.

### `exploration.md`

23. §1.8 R9: "`IN-2` (measured against the 100 ms target)" (was
    "measurement only; latency target not yet confirmed or waived").
24. §1.8 RL-1: "`OR-3` (`test_gym_default_writes_no_file`)" (was "test gap
    until `test_gym_default_writes_no_file` lands").
25. §1.8 R5 gained OD-22 and B-15 ("accepted implies applied" is exactly
    what both close). §1.8 DV-2 gained OR-17 (build/platform
    reproducibility of golden JSON floats).

## Not changed, checked and found already correct

- `ledger/08-consumer.md` CN-17: run order, gating on SL-7, rows 1-20
  census, the `[writing]` gate line, the patch-target wording, BR-19's
  pin, the `--out` bundle fix: all already present, matching
  `L4-resolution-a.md` and `L4-resolution-d.md`.
- `ledger/04-fleet.md` FL-2/FL-1: `HomeStatus`, `weight_degraded`,
  `ReservedReportName`, `NoCoveredWeight` with `homes`: already present.
- `ledger/06-runtime-memory.md` RT-2: "measured, dropped" keep-or-drop
  rule text and the `ForcingTable` `Send + Sync` assertion: already
  present. Row 62's schedule State column stays blank (RT-2 has not run).
- `ledger/11-other-defects.md` OD-21, OD-22: already present with the
  preamble listing them.
- `plan.md` `EnumLabels`, `Aggregation::Last`, `Warning`/`WarningLog`,
  `ReplayBlocker`/`NotReplayable`, `SnapshotError`, the publish-failure
  wording: already present.

## Rejected

None. Every item this pass's resolution files hand to these files either
applied cleanly or was already satisfied by the owning entry's existing
text.

## Not applicable here (other owners' files, confirmed consistent)

- `09-numeric-fixes.md` B-14 (si_census wording), B-11 (`unit_keyed_fleet_rules`
  rename): not this pass's files; `04-fleet.md`'s own use of
  `unit_keyed_fleet_rules` already matches.
- `03-output-reporting.md`'s note to the `04-fleet.md` owner (FL-2's
  degraded fixture depending on OR-11's schedule-column warning reaching
  `RunResult.warnings`): informational; OR-11's "Schedule columns"
  section already marks `hot_water_dishwasher`, `hot_water_clothes_washer`
  and `hot_water_fixtures` `NotUsed`, so FL-2's fixture does not trip it.
  No `04-fleet.md` edit needed.
- `09-numeric-fixes.md` B-16/B-17: these have no row in
  `exploration.md`'s requirement table, consistent with several other
  numeric entries that serve no stated consumer requirement (B-1a, B-1b,
  B-1c, B-3, B-4, B-7, B-11, B-13), so none was added.

## Counts

- Items applied: 25 (across `00-schedule.md`, `11-other-defects.md`,
  `12-inputs.md`, `02-measurement.md`, `04-fleet.md`,
  `06-runtime-memory.md`, `plan.md`, `exploration.md`), plus the
  operator-ruling rewrite (4 passages) addressed separately above.
- Items rejected: 0.
- Items checked and already correct (no edit needed): 10.
- New ledger entries added: 5 (`OD-23` to `OD-27`).
- New schedule rows added: 8 (`47a`, `24j` to `24n`, `94j` to `94l`).
- Schedule completeness check: pass, no entry missing or duplicated
  outside the documented multi-row and closed-inside-another-row cases.
