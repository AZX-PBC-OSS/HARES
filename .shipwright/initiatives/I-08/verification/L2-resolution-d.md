# L2 resolution, pass d: consumer and numeric fixes

Files revised in place: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`.
Findings resolved: every finding in `verify-L2-d.md`, and every finding in
`verify-L2-e.md` that concerns these two files. Findings that belong to other
files are handed on below ("Schedule changes", "Items for other owners").

Checked against: the consumer branch `i08-consumer` (`824805e` CN-0, `a6d1345`
CN-16, `f7131d7` and `aec2e3f` the capture provenance guard, `5c651bc` BR-6d,
which landed while this pass ran); `decision-log.md` including
`dl-i08-execution.*`; `baselines/` (`B0-de-trace.json`, `B0-losses.json`,
`B0-record.json`); `ledger/02-measurement.md` M-0 and B0; HARES at
`feat/output-results` (`75a182a0`); the vendored EnergyPlus source and IDD;
the consumer's `results/engine_schema.json`, venv and cache. Both code
repositories were only read.

Counts:
- `verify-L2-d.md`, 22 findings: 18 fixed in these files; 2 already done in
  committed work and recorded with evidence, with the remaining text or
  follow-up fixed here (1, 19); 2 schedule-only, handed to the schedule
  integrator (15, 16). Of the 18 fixed, 3 were partly done in committed
  work (3, 5) or by another owner (6), recorded below. 0 rejected.
- `verify-L2-e.md`, 9 findings concern these files (4, 6, 9, 12, 14, 24, 26,
  37, 45): 8 fixed, 1 checked with no change needed (14). 0 rejected.
- New entries: `08` CN-17 (follow-up to CN-0/B0, before CN-13), `09` B-9a
  (found defect: `Fleet::from_resstock` cannot build a runnable fleet, and
  the fixture B-9 records its delta on).
- 3 schedule changes, 9 items for other owners, 1 operator question.

## What the committed consumer work already did

- CN-0 (`824805e`): `tools/capture_de_trace.py` `cmd_compare` runs
  `diff_fields(a["premises"], b["premises"], "premises")`; the module
  docstring says the header "is NOT compared". Exit 2 on unreadable input.
  `RULES` is empty, so `--rule n-eval-without-parent-probes` is an error
  until CN-6. Missing: a case that pins the header rule (the two committed
  cases never vary the header) -> CN-17.
- `f7131d7`, `aec2e3f`: `capture` refuses an installed engine whose
  `engine_so` is not inside `paths.HARES_REPO`.
- CN-16 (`a6d1345`): M-0's base gate at `c71c542` against `1d60f478` read
  329 passed, 0 failed, 7 skipped; no engine-side cause exists.
- B0: `02-measurement.md` M-0 records the tree comparison ("`tree(0ec757cb)` =
  `81dc6655...`, `tree(1d60f478)` = `b48a53cb...`. The diff is exactly the
  `cargo fmt` commit (`89f32822`) reformatting `profile_sim.rs` and
  `py_dwelling.rs`"). Re-checked here: `git diff --stat 0ec757cb 1d60f478`
  lists those two files, 18 insertions and 4 deletions, and its hunks equal
  `git diff 3703b941 89f32822` apart from blob ids and line offsets.
  `B0-record.json`: gate 332 passed, 0 failed, 7 skipped; three captures
  bitwise identical.
- Engine schema: through B0, `results/engine_schema.json` was the copy dated
  2026-09-22 13:04 (before Task 0). It was rewritten 2026-10-02 06:01 with
  `engine_sha256` `0e673f6cd097168b`, the sha256 prefix of
  `/home/rich/src/HARES/python/ochre_next/_hares*.so` built at 06:00, which
  the consumer venv holds as an editable install (`direct_url.json`:
  `file:///home/rich/src/HARES`) at `5c651bc`. So BR-6d's gate most likely
  ran on a regenerated schema; no written step required it.
- Gate outputs: no `gate.txt` and no `scratch/i08/` exist in the consumer;
  `~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_2026-09-30.zip` exists beside the
  repository (the side effect the gate text never mentioned).

## verify-L2-d

1. already done in committed work: `capture_de_trace.py` `cmd_compare`
   compares `premises` only (evidence above). fixed: `08` "DE-trace capture"
   "A capture "equals" a baseline when `compare` exits 0: `compare` diffs
   only the per-premise payload (`premises[*]`, every recorded field); the
   header ... is provenance, written and never compared"; CN-0 change text
   now states the committed behaviour; follow-up CN-17
   "`compare_ignores_the_header`: two payloads with identical `premises` and
   headers that differ in every header field ... make `compare` exit 0";
   CN-4 "a second capture on the same commits compared with B1 exits 0";
   CN-15 "the DE-trace `compare` against B1 exits 0 (per-premise payload
   only; the header differs by design)".
2. fixed: `08` mechanism "Every output a gate or a capture writes in the
   consumer goes to `scratch/i08/<sha>/` ... so no output can appear as an
   UNTRACKED file or be zipped into the bundle"; gate commands write
   `scratch/i08/<sha>/gate.txt`, `contract.txt`, `bundle.txt`; captures
   write `scratch/i08/<sha>/capture.json`; gate text "Side effects, expected
   and recorded: it rewrites `INDEX.txt`, `MANIFEST.txt` and `SETUP.txt` ...
   and it writes the bundle `~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_<date>.zip`
   beside the repository" (`build_bundle.py:384-398` at `aec2e3f`). Not done
   in committed work (no record of where B0's gate output went).
3. fixed (partly done, see above): `08` mechanism step 4b
   "`HARES_REPO=... uv run --no-sync python -m calibration.schema --emit
   --all-cached`" with its reason; gate text "a skip of any of the eight
   `suite_coverage` cases is a gate failure (step 4b did not run or
   failed), never part of the base"; the base on a regenerated schema is
   recorded by "the first gate that ran step 4b (CN-17 states which)".
   CN-17: "The base on a regenerated schema is the BR-6d gate's if its
   record ... shows the schema was regenerated against its engine under
   test (the 06:01 rewrite above); otherwise this entry's gate records it."
   CN-15 regenerates the schema against the pinned engine before `run_all`.
4. fixed: `08` preamble "Contract-drift rule" lists all six categories with
   their source files and fields, then "the first consumer row gated after
   one or more engine rows runs `... python calibration/hares_contract_check.py`
   ... and, in that row's commit, updates `hares_contract.py`, the adapter
   code that writes the item and the `coverage.py` rules for every DRIFT
   item it reports, in every category". Assignment: CN-17 rows 1 to 12;
   CN-13 BR-11 to row 26; CN-1 rows 28 to 71 (OR-3 `output_verbosity`, OR-1
   column literals, RT-4 `DwellingConfig` path fields, PY-8, IN-5 setpoint
   config, IN-6); CN-14 rows 79 to 81; CN-3 rows 83 to 94c (B-2
   `initialization_duration`, B-5 classifiers, OD-9 generated setpoint
   literals, IN-8 Fahrenheit unit, IN-9 `EvConfig`). "Where the engine
   publishes a category, CN-1 moves the check from the source scan to the
   published surface ... `hares.input_schema()` (IN-3) ... the catalogue
   (PY-4)", with the case `contract_drift_reads_the_published_schema`. The
   census "`hares_contract_check.py` exits 0 on this row's engine" is in the
   verification of CN-17, CN-13, CN-1, CN-14 and CN-3. The script form is
   used because the module imports `hares_contract` as a top-level module
   (`python -m calibration.hares_contract_check` would fail).
5. fixed: `08` preamble "Pin provenance (facts)": "`0ec757cb` is `ebdb1a17`
   plus three commits, `4fa9bc43`, `6ec4fead` and `0ec757cb`: the PR #27
   changes ... rebased onto `ebdb1a17`, without the fmt commit ... on no
   `origin` ref"; "PR #27 on `origin` (`refs/pull/27/head` = `89f32822` ...)
   is four commits on `2ee83a86`: `88d8aaa4`, `a20f8372`, `3703b941` and
   `89f32822`"; "The Task 0 head `1d60f478` is the local merge of `89f32822`
   into `ebdb1a17`". Verified with `git log` in `~/src/HARES` (the fetched
   `0ec757cb` is now present there). B0: the command (`git -C ~/src/HARES
   fetch ~/src/HARES_CALIBRATION_SYSTEM/HARES 0ec757cb`, then `diff --stat
   0ec757cb 1d60f478`) and "Expected result: formatting-only differences in
   `crates/hares-core/examples/profile_sim.rs` and
   `crates/hares-python/src/py_dwelling.rs`, whose hunks equal those of
   `git diff 3703b941 89f32822` ... Any other difference stops B0"; already
   done in committed work: "**Recorded:** ... Tree ids `81dc6655`
   (`0ec757cb`) and `b48a53cb` (`1d60f478`)" (02 M-0). The diffstat is in
   `02-measurement.md`'s M-0 entry already.
6. fixed by the 12-inputs owner, coordinated here: `12-inputs.md` IN-8
   "schedule-file setpoint columns are read in degrees Fahrenheit" is a
   numeric-block entry (row 94b, after OD-9), IN-5 states "the Fahrenheit
   reading is IN-8" and that a generated column "is never read as
   Fahrenheit"; IN-8 "A generated schedule is not affected:
   `11-other-defects.md` OD-9, an earlier row, has removed its setpoint
   columns." Placed directly after OD-9 rather than before it, which the
   finding's purpose allows: the conversion then never meets a generated
   column. `08` reflects it: order step 5 "CN-3 (after B-13, OD-9 and
   `12-inputs.md` IN-8 and IN-9)", CN-3 and CN-4 dependencies, the CN-3
   drift item "IN-8 gives the two setpoint columns the unit degree
   Fahrenheit". The consumer writes no setpoint column
   (`git grep` over `calibration/`), so IN-8 moves no consumer number.
7. fixed: `08` CN-6 "`evaluate` counts in `n_eval` only the specs it submits
   and in `n_rejected` only submitted specs that come back `None` or
   `FailedRun`; a vector whose spec fails to build is `PENALTY` and counted
   in neither"; "`n_eval` drops by `s` ... `n_rejected` drops by `r`" with
   `s`, `r` measured at the parent and passed as `--declared <file>`;
   "`flag_codes` may differ only by the presence of
   `fit_infeasible_corners`, and only for a premise where the rejection
   share test of `flag.py:161` ... gives a different result on B0's counts
   and on the adjusted counts"; "`flag` may differ only where that code is
   the premise's only code"; the rule lands in `RULES`; test
   `compare_accepts_only_the_declared_probe_counts`.
8. fixed: `09` EnergyPlus section "The "next hour" of hour 24 is hour 1 of
   the same day's records ... (`WeatherManager.cc:3060-3064`)"; B-1b "After
   the half hour of hour 24, the next-hour value is hour 1 of the same day,
   as `WeatherManager.cc:3060-3064` does"; test
   `solar_hour24_takes_hour1_of_the_same_day_after_the_half_hour`.
9. fixed: `09` EnergyPlus "Clock" bullet citing
   `group-location-climate-weather-file-access.tex:830`; B-1c "Records are
   addressed by the local standard time date and hour (the site's standard
   offset), never by the civil time of a `civil_timezone` with daylight
   saving"; test `weather_is_read_by_standard_time_under_daylight_saving`
   (`full_year_900s_dst` inputs).
10. fixed: `09` EnergyPlus section "a file whose data period is not a
    complete year runs only when the run's start and end dates lie inside
    that period, and is fatal otherwise (`:942-991`)"; B-1c "A typical file
    whose data period is not a complete year is a construction error ...
    when the run's first or last step lies outside that period"; fixture
    `tests/fixtures/weather/cz4a_january.epw`; test
    `partial_year_typical_file_outside_its_period_is_an_error`.
11. fixed by following EnergyPlus (the decision log is binding and this pass
    may not edit it): `09` EnergyPlus section "The day of week advances by
    one per simulated day (`CurDayOfWeek = mod(CurDayOfWeek, 7) + 1` for
    each day read, `:2856-2859`)"; B-1c "Day types ... follow EnergyPlus
    ... After a skipped Feb 29, every later date of that run is one weekday
    behind its calendar weekday, which the Input Output Reference states:
    "the leap day will be skipped if it is not present, resulting in
    incorrect day types for the remainder of the year"
    (`group-location-climate-weather-file-access.tex:597-601`)"; tests
    `day_types_after_a_skipped_feb29_follow_energyplus` (2024-03-01 is a
    Thursday, its calendar weekday Friday) and
    `typical_file_with_feb29_uses_it`. Operator question below.
12. fixed: `09` EnergyPlus "Timesteps" bullet ("Must be evenly divisible
    into 60", `idd/Energy+.idd.in:925-936`); B-1a
    "`SimulationConfig::validate()` rejects a `time_res` under one hour that
    does not divide 3600 s, naming the value and the rule"; test
    `subhour_step_must_divide_the_hour` (420 s fails; 900 and 3600 pass).
13. fixed: `09` B-13 "A declared `AssemblyEffectiveRValue` at or below the
    sum of the boundary's interior and exterior film resistances ... is a
    construction error naming the boundary, its declared R and its film
    sum; the clamp goes from the declared-R path" (clamp verified at
    `conversions.rs:147-150`); "this entry's normalisation by `k` runs
    before it, so a layer's resolved resistance is the row's times `k`
    times `opaque_r_scale`"; tests `declared_r_at_or_below_the_films_is_an_error`
    and `assembly_r_and_opaque_r_scale_compose`; dependency on IN-7.
14. fixed: `09` preamble "B-5 depends only on OR-1 and shares no code with
    B-6 or B-11, so its place before them is an order, not a pairing";
    B-11's "Adjacent to B-5" replaced by "B-6 (nulls), directly before this
    entry". No schedule change.
15. handed to the schedule integrator (`00-schedule.md` is not this pass's
    file): "rows 1 to 10 each keep" becomes "rows 1 to 11 each keep" (BR-6b
    is row 11, and the schedule now also has row 3a).
16. handed to the schedule integrator: the consumer under test changes at
    BR-6d (`5c651bc`) and again at CN-17, so the column label for the rows
    after them is "B0 (BR-6d)" until CN-17 and "B0 (CN-17)" from CN-17 to
    row 26, each defined in the column legend.
17. fixed: `09` B-1c dependencies "`12-inputs.md` IN-2 (`validate()`, which
    reports the start-on-Feb-29 error and the simulated step count)"; B-2
    dependencies "`12-inputs.md` IN-2 (`validate()`)".
18. fixed: `09` B-1c "The resolved clock that
    `DwellingBlueprint::validate()` reports (`12-inputs.md` IN-2) carries
    the run's simulated step count" and "OR-7's
    `frame_starts_at_start_time_with_exact_row_count` expects the simulated
    step count from this commit on"; B-2 re-runs it ("the row count equals
    the simulated step count (B-1c)"); `08` CN-14 "The row-count check of
    `Runner.run` (`engine.py:371-376` ...) compares the table's row count
    with the simulated step count the engine reports" (the finding's
    `:361-366` is `:371-376` at `aec2e3f`), test
    `row_check_uses_the_engines_step_count`.
19. already done in committed work: M-0 "`run_all`: `329 passed, 0 failed,
    7 skipped, 336 total` ... The three failures in the committed
    `docs/Test_Report.md` do not reproduce on the Task 0 engine". fixed:
    `08` CN-16 "None of the three fails on the Task 0 engine ... No
    engine-side change is needed" (the contingent engine commit is gone);
    no contingent row is needed in `00`.
20. fixed: `08` CN-13 "`c="${XDG_CACHE_HOME:-$HOME/.cache}"; test ! -e
    "$c/hares" && mv "$c/ochre_next" "$c/hares"`. If `$c/hares` exists, the
    row stops and the operator decides".
21. fixed: `08` mechanism step 3 writes `scratch/i08/<sha>/build-constraints.txt`
    with `maturin==<version>` read from the worktree's `uv.lock` and passes
    `--build-constraints` (the `uv pip install` flag is plural,
    `-b/--build-constraints`, env `UV_BUILD_CONSTRAINT`), then
    "`grep -h '^Generator:' ... WHEEL` prints "Generator: maturin (<the
    version in build-constraints.txt>)"", recorded with
    `calibration.paths.summary()`. Checked: the extraction prints
    `maturin==1.15.0` on HARES's lock; the consumer venv's engine WHEEL
    reads `Generator: maturin (1.15.0)`.
22. fixed: `08` CN-16 "The consumer's record attributes the three failures
    to "upstream HARES#26" (`docs/Red_Team_Record.md:97-98`: "Full gate
    unchanged: 325 pass / 3 fail (upstream HARES#26) / 7 named skips"), and
    its Round 4 entry, headed "engine `ebdb1a17` (...), build
    `13a4d73d9786a092`", reads "Gate: 329 passed, 0 failed, 7 skipped, 336
    total." (`:144-155`). Build `13a4d73d9786a092` is the release build of
    `0ec757cb` (measured), so that gate ran on `0ec757cb`".

Other corrections made while here: the mechanism's paths follow
`dl-i08-execution.worktrees` (`/home/rich/src/HARES/.workstrees/<sha>`,
shared target `.workstrees/target`); `paths.summary()` is described as the
module function with its seven fields (`dl-i08-execution.paths-summary`);
the seed citation is `scale.py:569` (was `:563`); CN-5 updates the capture
tool's `_workers_used()` mirror, whose committed comment asks for it; CN-1
uses the Python keyword names `duration`, `time_res` and
`site_location=SiteLocationOverride(...)`; CN-10 uses `hares.ParsedWeather`;
CN-13 records that PY-12 (row 30a) does not affect the consumer
(`upgrade0_2025_1.parquet` carries `weight`, 549,971 rows); the em dash in
the old B0 text is gone.

## verify-L2-e (findings that concern these files)

| # | Resolution |
|---|---|
| 4 | fixed: `08` CN-10 "Each distinct weather file of a batch is parsed once into a `hares.ParsedWeather` (`plan.md` §2.7, built by `06-runtime-memory.md` RT-4) and that handle is passed to every job's `from_hpxml`"; the API itself is `plan.md` §2.7 (`L2-resolution-b.md` 4) and RT-4's to build. |
| 6 | fixed for B-4: `09` B-4 uses BR-14's `month_boundary_billing_3600s` ("`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`, start 2023-01-25T00:00:00-07:00, 12 days at 3600 s, tariff `tests/fixtures/golden/flat_tariff.json` in `America/Denver`"), dependency on BR-14, measurement on that golden. fixed for B-9: new `09` B-9a adds `resstock_2025_fleet` ("metadata `tests/fixtures/resstock/2025.1/metadata_fleet3.parquet`, the rows of `bldg_id` 19694, 162486 and 352714 ... weather `G5300330_2018.csv` ... duration 172800 s"), captured on the unfixed engine, and B-9 records its delta there. B-9 needed B-9a because `Fleet::from_resstock` cannot build a runnable fleet: its HPXML and schedule paths are the dataset's zip archives (`resstock.rs:204-227, 307-317`) and `weather_fips` is never set (`:317`; `parse_weather_station_fips` at `:424` is never called), so no `from_resstock` golden could exist. IN-5 is the 12-inputs owner's IN-8 (finding 6 above). OD-2 site 6 is not this pass's file. |
| 9 | fixed: named inputs for B-1a (new `tests/fixtures/weather/psm3_15min_2019_0101.csv` with stated content; cz4a weather for the hourly tests; stated series for unit tests), B-1b (stated series), B-1c (three new weather fixtures derived from named files; every test names its building, start and step), B-2 (candidates A to D and a recorded, pinned selection rule: "Each test below pins, by name, the first candidate in the order A to D that meets its stated property; the test asserts the property"), B-3 (cz4a inputs, 2 days, 3600 s), B-4 (BR-14's fixture), B-5 (`base-pv.xml`, `base-misc-loads-large-uncommon.xml` with `test_location.epw`), B-6 (cz4a inputs, 2 days, 900 s), B-7 (cz2a inputs, 7 days, 3600 s), B-9 (B-9a's fixture), B-12 (`bldg0000002`, `bldg0000017`, `bldg0000003` at UTC-5; `fleet_mixed_offset`; FL-4's fixture), B-13 (`base.xml` `Wall1`); `08` CN-2, CN-3, CN-6, CN-11 (`suite_fit`'s `bldg0050945`, county `G4804390`, `STUDY_GRID`), CN-7, CN-14 (`engine._default_spec`, `bldg0008590`, plus the cached TMY3 `G4802010.epw`). The `09` preamble gains the rule "Every acceptance test names its inputs". |
| 12 | fixed (B-1c half): `09` B-1c "the "Time" section OR-7 adds to `docs/python.md` gains the leap-day rule: on a typical file without Feb 29, Feb 29 is not simulated, no row is labelled Feb 29, and later day types follow EnergyPlus as above". |
| 14 | checked, no change: `08` and `09` cite only ledger entry IDs, `context.md` item numbers and decision keys; no requirement ID with a colliding spelling appears (grep for `V\d`, `R\d`, `RL-`, `RS-`, `DV-`, `X-`; the `FL-` hits are ledger entries). |
| 24 | fixed: `09` B-2 "`03-output-reporting.md` OR-7's `frame_starts_at_start_time_with_exact_row_count` is re-run here with warm-up enabled". |
| 26 | fixed: `08` CN-12 acceptance "`surrogate_trial_reproduces_report`, a check mode of the tool rather than a testkit case (a case would skip whenever the `trial` group is absent and break the gate's exact skip set) ... exits 0 only when its loss, out-of-sample scores and engine-run count equal the report's table row for that premise on bits". |
| 37 | fixed: `08` CN-6 "`context.md` item 18's caveat ("the probe equals nominal only when no ABSOLUTE factor is free") concerns substituting the reachability probe's run for the nominal evaluation; this entry never does: it reuses `fx[0]` and `fx[1]`, built from the same vectors". |
| 45 | fixed: `08` CN-1 "That resolution is the schema assertion (`context.md` item 16): a missing name or a different unit raises naming the name, the expected unit and the catalogue's. It records `build_info()` and the catalogue digest (PY-6) without pinning the digest". |

Items from other resolution files addressed here: `L2-resolution-a.md` (B-4
names `month_boundary_billing_3600s`; B-2's manifest migration maps `0` to
disabled and `604800` to the default; `0` now means one thing everywhere
because the key is removed); `L2-resolution-b.md` (B-2
`warmup_toml_forms_round_trip`, `replay.toml` `warmup`, PY-2's
`removed_keyword_names_replacement` row, OR-7 test re-run; B-1a's
sub-hourly rule covers EPW once IN-6 reads it; B-1c updates OR-7's "Time"
section; CN-1 and CN-10 keywords and `ParsedWeather`; PY-12's weight column
checked against the consumer's metadata).

## Schedule changes (`ledger/00-schedule.md`, for its integrator)

- New row directly after row 13 (BR-6d, landed as consumer `5c651bc`) and
  before row 13a (BR-11): CN-17, file 08,
  `test: pin that a capture compare ignores the provenance header`,
  consumer check "gate+B0". It must in any case precede row 27 (CN-13).
  The opening-sequence rule gains CN-17 after BR-6d.
- New row directly before row 91 (B-9): B-9a, file 09,
  `fix: build ResStock fleets from extracted buildings and their weather stations`,
  consumer check "golden" (it adds `resstock_2025_fleet`; every existing
  golden, `consumer_shape_900s` included, is unchanged). It is a defect fix
  that moves no number, so the "Each numeric fix" rule does not list it.
- Findings 15 and 16 above (the "rows 1 to 11" wording; the "B0 (BR-6d)" and
  "B0 (CN-17)" labels for the consumer check column). Finding 19 needs no
  row: CN-16 had no engine-side cause.
- Order checks (no change): CN-3 (row 95) now depends on IN-8 and IN-9
  (rows 94b, 94c), B-2, B-5 and OD-9, all earlier; B-1c and B-2 depend on
  IN-2 (row 66) and OR-7 (row 45); B-13 depends on IN-7 (row 66a); B-4 on
  BR-14 (row 13f); B-9a on BR-15 (row 13g), FL-1 and PY-2.

## Items for other owners

- `ledger/01-baseline-repair.md` BR-6d (implementer's record): state whether
  its gate ran on the regenerated schema (the 06:01 rewrite, build
  `0e673f6cd097168b`) and where its gate output went. The consumer venv
  holds an editable install of the main tree `/home/rich/src/HARES`, not of
  a `.workstrees/<sha>` worktree, so its engine follows any uncommitted edit
  in the main tree; the mechanism in `08` requires one worktree per gated
  commit. BR-6d predates `--build-constraints`; its engine's WHEEL reads
  `Generator: maturin (1.15.0)`, the locked version, which can be recorded.
- `ledger/02-measurement.md`: M-0's and B0's gate counts were measured on
  the 2026-09-22 engine schema; the regenerated base is recorded by BR-6d's
  or CN-17's gate (CN-17 states the rule). M-1's manifest format gains
  `kind = "resstock_fleet"` (B-9a) and, with B-2, `warmup` in place of
  `initialization_duration_s`.
- `ledger/03-output-reporting.md` OR-7: from B-1c on, its
  `frame_starts_at_start_time_with_exact_row_count` expects the simulated
  step count, and B-1c adds the leap-day paragraph to its "Time" section.
  OR-16: B-2 replaces `initialization_duration` in `replay.toml` with
  `warmup`.
- `ledger/04-fleet.md`: B-9a changes `Fleet::from_resstock` (extracted
  building layout, `buildings_dir`, FIPS from the HPXML weather station).
  B-12's `local_clock_fleet_is_labelled_by_local_wall_clock` uses FL-4's
  mixed-offset fixture (the one `L2-resolution-a.md` asks FL-4 to name).
- `ledger/05-python-surface.md` PY-2: `REMOVED_KEYS` gains `hpxml_dir`
  (Python `Fleet.from_resstock`) naming `buildings_dir` (B-9a), and the
  `initialization_duration` row B-2 completes.
- `ledger/06-runtime-memory.md` RT-4: the Python `hares.ParsedWeather`
  handle CN-10 passes to `from_hpxml`.
- `ledger/12-inputs.md`: IN-2's resolved clock gains the simulated step
  count B-1c fills (B-1c depends on IN-2 for it). IN-8 is consistent with
  this pass. Please verify whether OpenStudio-HPXML's `water_heater_setpoint`
  schedule column is also in degrees Fahrenheit (`schedule_resolve.rs`
  reads it as a setpoint column like the two IN-8 converts); this pass did
  not add it.
- `plan.md` §3 (owner of the opening sequence text): it repeats the
  engine-under-test mechanism (`plan.md:1488-1517`); it should carry step
  4b, the `--build-constraints` step and the `scratch/i08/<sha>/` outputs as
  `08` now states them.
- Schedule integrator: the section above.

## Operator question

- `dl-i08-architecture.weather_edge_cases` says "Follow EnergyPlus behaviour
  for Feb 29 on a typical-year file without a Feb 29". B-1c now does so in
  full, including EnergyPlus's day types, which put every date after a
  skipped Feb 29 one weekday behind its calendar weekday for the rest of
  that run (EnergyPlus's own documentation calls them incorrect). If calendar
  day types are wanted instead, the decision entry must say so; B-1c then
  states the deviation in `docs/PHYSICS_DECISIONS.md` and its test expects
  the calendar weekday.
