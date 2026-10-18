# L2 integration

Inputs: `verification/L2-resolution-a.md`, `-b.md`, `-c.md`, `-d.md` (their
"items for other owners", "schedule changes" and operator questions),
`decision-log.md` (binding, including `dl-i08-architecture.si_units`,
`leap_day_day_types`, the two-stage rule in `aggregation`, and the
`dl-i08-execution.*` records), `plan.md` and `ledger/*.md`. Code read at
`feat/output-results` `8b09ac4a` (BR-11 landed while this pass ran) and
consumer `i08-consumer`; both repositories only read.

Not edited: `implementation.md`, `decision-log.md`, schedule rows 0 to 13g,
and the entries of rows already committed or in progress (Task 0 through
M-3, BR-9, BR-11, CN-17), except the text corrections the resolution files
ask for in M-3 and M-4a. The measurement log and the entries of `01` were
left for their own rows' commits.

## Changes, by file

### `plan.md`

| Section | Change | Source |
|---|---|---|
| §1 ledger areas | `09` described as including the precedence change and the SI migration | si_units |
| §2.2 `Frequency` | `Minutes(NonZeroU32)` added (n divides 60, periods start on the local clock hour, TOML `{ minutes = 15 }`); OR-13 owns it; the sub-timestep sentence kept and extended with the validate rule | c (verify-L2-c 4) |
| §2.2 `VariableKind` | `Quantity { across_homes: AcrossHomes }`, `enum AcrossHomes { Sum, Mean }`; power units (`kW`, `W`, `kVAR`, `therms/hour`) declared `Power`; intensive quantities `Mean`, flows, counts and volumes `Sum` | c, decision aggregation |
| §2.4 | `FleetReport.health_policy`; `Fleet::simulate(report, n_threads, chunk_size)`; `FleetError::UnmatchedSelection { names }`; `homes` columns `steps`, `periods_folded`; `Coverage` comment (FL-1 `weight_flagged`, FL-2 renames and adds `weight_skipped`); the selection-across-homes exception paragraph | c |
| §2.5 | `aggregation.rs:509` corrected to `:505` (parse, `parse_timestamp` at `:444`), `:508` (bucket key), `:636-637` (label) | verify-L2-c 15 |
| §2.6 | two-stage table (per kind, over time and across homes), summation order, null and covered-weight rule, owners (FL-1, FL-5, B-11), and FL-4's `Instant` calendar rule | c, decision aggregation |
| §2.7 `REMOVED_KEYS` | consulting surfaces extended (`Fleet.from_resstock`, equipment constructors, override merge, synthetic TOML from B-14); rows `hpxml_dir` (B-9a) and the `<ConfigType>.<field>` equipment unit rows (B-14) | d, si_units |
| §2.7 weather rules | the one EnergyPlus departure: calendar day types after a skipped Feb 29 | leap_day_day_types |
| §2.7 `ForcingKey` | comment: time grid, naive local start, site UTC offset in seconds, horizon, lat/lon/elevation bits, civil timezone; `FirstHour` and `WeatherYear` join with B-1a and B-1c; plain values only | c (verify-L2-c 7) |
| §2.7 opaque types | `ActorTimings` added to the opaque list | a |
| §2.7 `ResolvedModel` | new field `clock: ResolvedClock` and `ResolvedClock { start, utc_offset_s, time_res_s, steps }` (IN-2; B-1c subtracts a skipped Feb 29), which B-1c and CN-14 cite and no type defined | d (IN-2 item), sweep |
| §2.7 resolved types | crate comment added before `ResolvedBoundary` (hares-core), which sat under the hares-types comment of `ResolvedParameter` | sweep |
| §2.8 | found numeric defects listed: B-14, FL-5, OD-9, IN-8, IN-9; leap-day day types in B-1c | si_units, c, b |
| §2.9 | `DwellingProfilingSummary` field list as PR-1, PR-4, PR-6 define it (eleven phases, `finish: Option<StdDuration>`, `per_actor: Vec<ActorTiming>`); `pub struct ActorTimings` | a |
| §3 opening sequence | CN-17 added after BR-6d | d |
| §3 closing sequence | the reproducibility workflow runs on the pull request (M-4a triggers) | a (verify-L2-e 41) |
| §3 consumer engine under test | worktree path, `--build-constraints`, step 4b schema regeneration, `scratch/i08/<sha>/` outputs, contract-drift check, `Generator:` line | d |

§2 holds no em dash (checked); verify-L2-e 17 is RT-3's (applied by c) and
27 was applied by b.

### `ledger/00-schedule.md` (preamble and rows 14 onward)

- Numeric-fix rule: covers `09`, `04` FL-5, `11` OD-9, `12` IN-8 and
  IN-9, each after the consumer migration of the row before it.
- Opening-sequence green rule: `[test-observe-profiling]` added to the
  Task 0 failing list and to BR-1's lines; "rows 1 to 10" became "rows 1
  to 11 (row 3a included)" (verify-L2-a minor 2, verify-L2-d 15).
- New paragraph listing entries closed inside another row: BR-5 (row 4),
  SL-1 (22), SL-5 (73), SL-6 (74), SL-4 (76), SL-9 (row 3, `fd23dd80`; PR-1
  deletes the example).
- Consumer-check legend: **B0 (CN-17)** added; the label names the consumer
  entry the consumer under test is taken from (verify-L2-d 16).
- Rows 14 to 25: consumer check `B0 (CN-17)`; row 14 title names the
  deleted example.
- New rows 24a OD-10, 24b OD-11, 24c OD-12; 30a PY-12; 43a PY-9 (moved from
  54, which now has no entry); 71a RT-8; 87a FL-5; 90a B-9a; 94b IN-8; 94c
  B-14 (two commits); 94d IN-9.
- Row 31 OD-2: two commits named. Row 39 OR-13: minute frequency in title
  and commit. Row 50 FL-1: two commits, `fleet_same_offset_300s` added.
  Row 51 FL-2: "golden, `fleet_mixed_offset` re-captured". Row 81 B-1c:
  calendar day types. Row 88 B-11: both stages.
- Closing note: rows without a commit read 13b (M-3), not 13a; row 54
  explained.

Checks run on the result: every entry of `ledger/01` to `12` is in the
schedule exactly once (SL-2 and SL-3 are the standing rules of the
preamble; SL-9 and BR-5 are listed as closed inside rows 3 and 4); no
dependency named in an entry's **Dependencies** line is scheduled after
it (the parser's remaining hits are "lands before X" sentences).

### `ledger/02-measurement.md`

- M-3 item 1: the baseline variants are the tool's default run (the
  engine's own Parquet file and retained batches) and `--discard-output`;
  `--output-dir` (the tool's writer) is not a variant (verify-L2-a
  minor 11).
- M-3 item 4: the Python run uses the manifest's replacement defaults tree,
  override and no warm-up (verify-L2-a minor 10).
- M-4a: triggers `workflow_dispatch` and pull requests into
  `initial-implementation`, as BR-10 (verify-L2-e 41); M-4b dependency
  names the pull request.
- New section "The golden format as built, and the entries that change
  it", before the log: the committed tool's fleet products, defaults
  digest rule, metrics flattening, health layout, `SIMULATION_KEYS`,
  `.gitignore:49`, "until RT-4", "in that entry's commit", census without
  line numbers, M-0/B0 gate counts on the 2026-09-22 schema, EPW source
  years; then the format changes of BR-11, BR-13, BR-14, BR-15, FL-1, FL-2,
  FL-4, B-2, B-9a and B-14 (resolution a's M-0, M-1, M-2 items, c's M-1/M-2
  `[fleet]` items, d's M-1 items), written as follow-up text because M-0,
  M-1 and M-2 are committed.

### `ledger/03-output-reporting.md`

- OR-1: `"enum"` stays its own `Unit::Enum` so the fleet's unit-keyed rules
  stay bitwise in this structural commit (FL-5 merges it); `Power` for
  `kW`, `W`, `kVAR`, `therms/hour`; every `Quantity` declares
  `across_homes`; tests `fleet_unit_rules_unchanged_by_canonical_symbols`
  and `every_quantity_declares_across_homes_and_power_units_are_power` (c).
- OR-7: `aggregation.rs` citation corrected; `frame_starts_at_start_time_with_exact_row_count`
  expects the simulated step count from B-1c, which updates the "Time"
  section; B-2 re-runs the test (d).
- OR-13: `Minutes(n)` with its rule and Python constructor; the reducer is
  constructible by `hares-fleet` with a calendar and per-variable rules,
  fed by batches or rows, with the stated summation order; tests
  `minutes_15_energy_sum_equals_timestep_sum`,
  `batch_fed_reducer_equals_row_fed_reducer`,
  `minutes_not_dividing_the_hour_is_a_validate_error`; commit message
  names minute frequency (c).

### `ledger/04-fleet.md`

- Intro: B-9a's change to `Fleet::from_resstock` and B-12's use of FL-4's
  inputs (d).
- FL-5: merges `Unit::Enum` into `Unit::Code` and lists the metadata
  change (follows OR-1's choice).

### `ledger/05-python-surface.md`

- PY-1: all three `FleetResult` tables export through the capsule; test
  `fleet_tables_export_through_capsule`; dependency on FL-1 (c).
- PY-2: the `removed_keyword_names_replacement` rows of B-2 (completed),
  B-9a (`hpxml_dir`) and B-14 (d, si_units).

### `ledger/06-runtime-memory.md`

- RT-4: the M-3 item 5 repeat uses `duration` and `reports=[]` (b).
- RT-6: kW/kWh fields converted to SI with `uom` when reported until B-14
  deletes the conversions; `clock` arrives with IN-2 (si_units, sweep).

### `ledger/07-profiler.md`

- PR-6: the per-actor vector is `per_actor` (field and Python key), because
  PR-1's phase field is already `actors`; resolution a's §2.9 list named
  both `actors`, which no struct can hold.

### `ledger/08-consumer.md`

- Order step 5 and CN-4: FL-5 and B-14 among the engine rows CN-3 and B1
  follow.
- Contract-drift assignment: CN-1 for rows 28 to 71a; CN-3 for rows 83 to
  94d, with B-14's renamed config fields and the consumer's DER code
  (`calibration/hares_der_remeasure.py:85`, `hares_contract.py:102-107`).
- CN-3: the drift step covers B-14 (DER spec to engine conversion at one
  boundary); dependency on B-14.

### `ledger/09-numeric-fixes.md`

- Preamble: scope names B-13, B-14 and B-9a; adjacency B-6, FL-5, B-11 and
  B-14 before IN-9; golden list adds `fleet_same_offset_300s` and
  `der_pv_battery_900s`.
- Weather section: the HARES departure on day types noted twice (365-day
  year bullet and the day-of-week sentence).
- B-1a: `first_hour_interpolation` joins `ForcingKey`; test
  `forcing_key_differs_by_first_hour` (c).
- B-1c: day types follow the true calendar (decision
  `leap_day_day_types`), with `docs/PHYSICS_DECISIONS.md` recording the
  departure; test renamed `day_types_after_a_skipped_feb29_follow_the_calendar`
  (2024-03-01 Friday, 2024-03-02 Saturday); the "Time" section wording;
  `weather_year` joins `ForcingKey`, test `forcing_key_differs_by_weather_year`.
- B-9a: the golden kind carries the `[fleet]` table (it depends on FL-1),
  not BR-15's `resolution`; test `test_from_resstock_hpxml_dir_names_buildings_dir`.
- B-11: Mode in both fleet stages, after FL-5; evidence marked as at
  `ebdb1a17` and deleted by FL-1/FL-5; test
  `home_mode_over_time_is_the_most_frequent_code`; both fleet goldens
  re-captured; dependency on FL-5 (c).
- B-12: citation corrected; `instant_fleet_is_labelled_in_the_reference_zone`
  rewritten: the manifest start is read per home in its own offset
  (`blueprint.rs:92-98`), so the first common hour in UTC-6 is 02:00, not
  00:00 as the old text said.
- New B-14 (SI units migration, `dl-i08-architecture.si_units`): census
  table of every typed config field and raw key in kW, kWh, kVA, kWh/mi at
  `8b09ac4a` (PV, PV array, battery, EV, generator, crankcase heaters,
  event and scheduled loads, synthetic event load), the Python
  constructors, the writers; scope boundary (state, ports, control
  signals, tariff rates, the compiled catalogues and the defaults CSVs keep
  their units and are converted at the boundary); `uom` conversions
  (`kilowatt`, `kilowatt_hour`, kWh/mi to J/m through `newton`);
  `REMOVED_KEYS` rows `<ConfigType>.<field>`; OD-2 site 7 in W; RT-6
  conversions deleted; IN-3 units; golden `der_pv_battery_900s` captured
  in a first commit; delta on every golden and on the EV regression
  dwelling.

### `ledger/11-other-defects.md`

- Preamble: OD-10 among the silent-path entries; OD-11, OD-12 named.
- OD-2 site 7: the check moves to W with B-14; IN-9 applies it to the
  value read.
- New OD-10: `conditioned_volume_m3.unwrap_or(400.0)` at
  `solver_builder.rs:1371, 1440` (verified at `8b09ac4a`) becomes the
  `MissingField` error `resolve_hvac.rs:247` returns; the neighbouring
  defaults at `:1435-1447` are decided against OpenStudio-HPXML's
  documented defaults in the entry's first step; test builders set a
  volume; goldens unchanged (every fixture HPXML carries the volume, and
  the parser already requires it).
- New OD-11 (from `dl-i08-execution.br6c-notes`): `VENDOR_OCHRE` undefined
  in `tests/python/test_thermal_trace.py:83-84`; the three
  `xfail(strict=False)` marks become `raises=AssertionError, strict=True`.
- New OD-12 (from `dl-i08-execution.br6c-notes`): the pvlib generator
  writes into `tests/fixtures/freefloat/` by default
  (`generate_pvlib_solar_override.py:29, 174-175`); it requires `--out`.

### `ledger/12-inputs.md`

- Intro: IN-9 follows B-14.
- IN-2: adds `ResolvedModel.clock`; test `resolved_clock_reports_steps`.
- IN-6: the never-compiled `crates/hares-core/tests/resstock_smoke.rs`
  leaves the census list (BR-12 deletes it; resolution a).
- IN-9: `ChargingPower` read in W into `max_charging_power_w`, capacity
  converted to `capacity_j` with `uom`; OD-2's check at 19 200 W; tests
  assert the SI fields and the read-back; dependencies B-14, RT-6
  (si_units, resolutions b and c).

### `review.md`

- V21 row: already fixed and tested (`ev/tests.rs:8062`), no entry
  (verify-L2-e 18).
- New capability: `SteppableFleet` has no save or restore method and
  `docs/python.md` states that fleet stepping does not rewind, matching
  `plan.md` §2.10 (verify-L2-e 32).

## Choices where inputs conflicted

1. IN-9 storage unit: resolution b converted to the EV's kW field; c left
   the choice open. `dl-i08-architecture.si_units` decides: B-14 moves the
   field to W first, and IN-9 writes W. B-14 therefore lands before IN-9,
   so IN-9 is row 94d, not 94c as b and the request labelled it; `08`'s
   "94c" ranges became "94d".
2. Leap-day day types: resolution d followed EnergyPlus's shifted
   weekdays; `leap_day_day_types` decides the calendar, applied in B-1c,
   the weather section and §2.7.
3. OR-1 unit merge: c asked OR-1 to keep the fleet's inference bitwise or
   record the change. A structural commit may not move numbers (§2.8), so
   OR-1 keeps `"enum"` as its own unit and FL-5 (numeric) merges it.
4. Profiling summary: resolution a's §2.9 list named two fields `actors`;
   the per-actor vector became `per_actor`.
5. B-9a's manifest key: d wrote BR-15's `resolution`, while c has FL-1
   replace it with `[fleet]` before B-9a runs; B-9a uses `[fleet]`.
6. B-12's mixed-offset expectation contradicted FL-4 and the engine's
   start-time rule; rewritten to the engine's rule.

## Unresolved

- Schedule rows 13a to 13g carry the consumer-check label `B0 (CN-16)`
  although they run after CN-17; the rows are frozen for this pass, so the
  legend states the meaning instead. The implementer may relabel them
  `B0 (CN-17)`.
- Deferred to BR-6c's and BR-6d's own rows, in `ledger/01-baseline-repair.md`:
  BR-6c's `requires-python` note and BR-6d's consumer-source quote
  (`L2-resolution-a.md` minors 17, 18); "when this plan was written"
  (BR-6b) and "at plan time" (BR-6c) to "at 2026-09-29" (verify-L2-e 28);
  BR-6d's record of whether its gate ran on the regenerated schema, where
  its gate output went and its maturin `Generator:` line (`L2-resolution-d.md`).
- For the operator: `decision-log.md` `dl-i08-process.readiness` still holds
  em dashes and the verdict wording against the L2 findings.
- `ledger/02-measurement.md`'s log cites raw records under
  `.shipwright/runs/` (M-0, M-3), outside this directory; they are the
  implementer's records of executed rows and were not moved.
- M-3 was re-measured at `8b09ac4a` before its item 1 and item 4 text was
  corrected: its "file written" figure (1.559 s) includes the tool's own
  writer and is not a baseline variant; later before/after rows compare
  against "in memory" (1.533 s, the engine's file and retained batches)
  and "nothing kept" (1.433 s). Whether item 4's Python run used the
  replacement defaults tree is not recorded; the export time it measures
  does not depend on the defaults.
- OD-11 pins the three OCHRE-divergence expectations strictly; whether
  those parity comparisons should exist at all, given that HARES targets
  EnergyPlus and not OCHRE, is a question for the operator.
- OD-10's three neighbouring defaults are decided inside the entry against
  the OpenStudio-HPXML documentation, which this pass did not read.
- Between PY-8 (row 34) and B-2 (row 83), `initialization_duration_s = 0`
  in TOML still warms up while Python's `0` does not (pre-existing); B-2
  removes the key, which ends the difference. No interim fix was added,
  because changing either meaning there would move numbers in a structural
  row.
