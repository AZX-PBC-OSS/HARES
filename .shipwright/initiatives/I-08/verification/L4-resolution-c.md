# L4 resolution (c): fleet, runtime and memory, slop cleanup, other defects

Scope: every finding of `verify-L4-c.md`; findings 13, 14, 17, 25, 28, 29
(the RT entries), 30, 31 and 33 of `verify-L4-e.md`; the item
`L4-resolution-a.md` routed to the `10-slop-cleanup.md` owner. Files
changed: `ledger/04-fleet.md`, `ledger/06-runtime-memory.md`,
`ledger/10-slop-cleanup.md`, `ledger/11-other-defects.md`. Code read at
`afa9a071` with `git show HEAD:<path>`; OpenStudio-HPXML read at tag
v1.12.0.

Counts: 34 findings. 34 fixed (3 of them with a part only `plan.md` can
carry, routed below), 0 rejected. Two defects found while resolving are
added: `11-other-defects.md` OD-22 and the dead `_into` solves in SL-7.

- verify-L4-c: 24 (7 majors, 17 minors), 24 fixed.
- verify-L4-e: 9 touching these files, 9 fixed.
- L4-resolution-a: 1 item, fixed (SL-11).

## verify-L4-c

**1 (major), FL-2's flagged-home test needs an actor hook.** Fixed, with a
correction to the suggested route: the one-day warm-up is not reachable at
FL-2's row. At `afa9a071` `initialization_duration` is read only for being
`Some` (`dwelling/mod.rs:2920`, `if let Some(_init_dur)`) and warm-up always
runs `run_warmup_converged(0.5, 25)` (`:2935`); a bounded `max_days` exists
only from `09-numeric-fixes.md` B-2 (row 83), after FL-2 (row 51). The
degraded home is made by its inputs: "its schedule is a copy of
`BEopt_example_schedule.csv`, written to a `tempfile` directory, with one
added column `unread_load` (every value 0.0), which no mapping reads, so
the run carries `03-output-reporting.md` OR-11's always-on unmapped-column
`Warning`." The test: "the second home is `degraded` in `FleetResult.homes`
with a reason naming `unread_load`, every aggregate cell equals the first
home's period value with covered weight 1, `coverage.weight_ok` is 1 and
`weight_degraded` 2, and Python `n_succeeded == 1`." Every arm of the
mapping, the unconverged warm-up included, is pinned by a unit test of the
one mapping function: "`home_status_follows_health_and_warnings` ... each
`RunHealth` counter set to 1 alone, `WarmupOutcome::Ran { converged: false,
.. }` alone, and one warning alone are each `Degraded`". Every fleet test
first asserts that the homes it expects `ok` are `ok`. OR-11 added to
Dependencies.

**2 (major), unreachable empty-run rule.** Fixed: the "no rows is
`Degraded`" mapping and `empty_home_is_degraded` are deleted. FL-2's
Problem now says why no run reaches the fold with zero rows, and FL-1
defines the one reachable case: "A home whose restricted selection holds
no variable ... runs its own reports, gets no `fleet` report, folds no
value and enters no cell's covered weight; its status is decided as for
any other home ... its `periods_folded` is 0, and the aggregate's rows are
the periods every home that folded at least one value has a row for." New
test `home_with_no_selected_variable_is_ok_and_uncovered` (`base.xml` and
`base-battery.xml`, `Named` of one battery variable: the plain home `ok`,
counted in `weight_ok` and `n_succeeded`).

**3 (major), `NoCoveredWeight` drops every home's status.** Fixed in FL-2:
"`FleetError::NoCoveredWeight { ok, degraded, failed, homes: RecordBatch }`
... so every home's status and reason survive the error ... Python raises
`HaresSimulationError` with the counts in the message and the table as the
exception's `homes` attribute". `zero_ok_weight_raises` asserts both homes'
rows and reasons, and Python `test_zero_ok_weight_raises_with_every_home`
reads them from the exception. The `plan.md` §2.4 type is routed.

**4 (major), `AlignmentRequired` needs offsets known only after parsing.**
Fixed in FL-4 with a named pre-pass: `hares_io::hpxml::read_site` (only
the `Site` extraction, moved into one function `parse_hpxml` also calls),
`hares_io::read_weather_meta` (format detection and header only), and
`hares_core::dwelling::site_offset(config)`, which feeds both to
`resolve_site_location` with `site_location` and the checked conversion of
OD-2 site 8. "`Fleet::simulate` runs the pre-pass over every entry before
the first chunk starts ... and raises the two errors below from it, before
any home runs." Tests: `site_offset_pre_pass_matches_built_dwellings`
(against each built dwelling's `hares:utc_offset_s`) and
`site_offset_pre_pass_reads_only_the_site_and_the_weather_header` (schedule
and defaults paths that do not exist, a weather copy holding only its
header line); `mixed_offset_fleet_requires_declared_alignment` asserts the
progress callback never ran. RT-4 carries the three functions to
`InputSource`.

**5 (major), aligned coverage impossible under `Instant`.** Fixed: "Under
`Instant(offset)` the naive starts are not compared: each home's start
instant (its naive start at its resolved offset) is, together with
duration and resolution ... each start instant must fall on a step
boundary of the declared offset's calendar ... and the homes' runs must
share at least one whole reference period". New tests
`instant_alignment_of_one_start_instant_covers_every_period` (starts
`03:00` at UTC-5, `02:00` at UTC-6, `00:00` at UTC-8; seven rows, covered
weight 3.0 in each) and `instant_start_off_the_reference_step_grid_is_an_error`.
Equal start instants are not required, so the partial-coverage test and
`instant_utc_equals_todays_output` stay as written and the `frame_golden`
manifest needs no per-home start: the aligned case is a `hares-fleet` test
whose per-home `DwellingConfig`s carry their own `start_time`.

**6 (major), RT-3 omits the ideal-capacity solve.** Fixed. Problem adds
both solves and what each allocates; Change: "The two scalar solves take a
`&mut SolveScratch` the `ThermalSolver` owns ... Each computes `N·x` into
`n_x` and `B_eff·u` into `b_u` with `mul_to` (the call the `*` operator
makes), adds them elementwise into `rhs`, then subtracts `b_col[i] * u_i`
element by element: the same operations in the same order as today".
The test is now `thermal_solver_step_allocation_free_after_first_step`,
bracketing `prepare_inputs`, every zone's
`solve_ideal_capacity_for_target` and `integrate`, coupled and uncoupled.
The surviving `_into` functions are deleted, not reused: SL-7 now removes
`solve_for_output_input_into`, `SolverScratch` and `SolveTarget`, which
have no caller and whose `gemv` accumulation with `beta = 1` is not the
used solve's operation order. The false "Zero allocation" doc comment is
named and rewritten.

**7 (major), OD-20 test against its own rule.** Fixed with a rule for
omnidirectional roofs, consistent with the no-constant rule: "An
omnidirectional roof (no `Azimuth`) has no azimuth to compare, because the
envelope averages its solar gain over every azimuth, so it matches by tilt
alone". Matching reads the envelope's registered surface geometry, so the
PV code has no orientation of its own (its roof tilt `unwrap_or(0.0)`, which
disagreed with the envelope's 90°, is named and removed).
`hpxml_pv_registers_roof_coverage` now expects both arrays on the roof
with summed coverage; `pv_on_an_oriented_roof_matches_by_azimuth` adds
`<Azimuth>180</Azimuth>` in the test and keeps the no-match warning case;
`roof_coverage_above_one_is_an_assembly_error` sums both arrays.

**8 (minor), RT-1 and the latch.** Fixed: Problem cites
`Telemetry(pub HashMap<String, f64>, Option<(String, f64)>)` at
`afa9a071`; Change: "in both cases it then copies the latch with
`self.1.clone_from(&source.1)`, which allocates nothing when both are
`None`"; the test asserts a latched source yields a latched copy on both
paths and a clean source clears a latched destination.

**9 (minor), RT-3 line labels and PR-3.** Fixed: Problem is "(lines at
`afa9a071`)" with the HEAD lines (`stepping.rs:1208`, `mod.rs:1271`,
`1512`, `1549`, `585`, `1206`); Dependencies: "PR-3 (satisfied at
`26626710` ...)".

**10 (minor), RT-2 keep or drop rule for the surface-independent table.**
Fixed: "The table is kept only when the loop's wall time with the table is
lower than without it by more than M-3's 5% noise band ... otherwise it is
dropped ... with the per-surface table below dropped with it." The
Measurement states what a dropped row records; RT-5's `Send + Sync`
assertion covers `ForcingTable` "when RT-2 kept it".

**11 (minor), offset-only key pair.** Fixed: "A second pair isolates the
offset: the same naive local start `2023-01-01T00:00:00` with
`site_location` offsets -07:00 and -05:00 ... their keys differ only in
the offset field".

**12 (minor), RT-5 census and `Send` citation.** Fixed: census "(at
`afa9a071`)" with moved lines updated and a new row for `INSTALLED` and
`THREAD_ALLOCATIONS` ("unchanged; never feed `RunHealth`"); `Send`:
"`SteppableFleet` steps its `Vec<Dwelling>` with `par_iter_mut`
(`fleet.rs:415`, `:742`) ... (`Fleet::simulate`'s `par_iter` ... builds
and drops each dwelling inside its closure and needs no `Send`
dwelling.)"

**13 (minor), FL-1 census misses `aggregation_check.rs:153`.** Fixed: the
site is listed, and "is rewritten as one test, which replaces both
halves". FL-1 and RT-5 Verification read `cargo nextest run -p hares-core
--test regression` (`crates/hares-core/Cargo.toml:49-51`).

**14 (minor), FL-1 wall time.** Fixed in Measurement: both fleet goldens by
M-3's method at the parent and after, and `fleet_fold/1000` with 8
threads at `chunk_size` 64 against 1000; a regression beyond the band
replaces the barrier by "an ordered completion window ... which keeps the
entry fold order and the bound of `chunk_size` live results". (The bench
group is new in this commit, so its comparison is between chunk sizes, not
against the parent.)

**15 (minor), memory bound wording.** Fixed: "The frame memory is bounded
by the chunk size ... The rest of the fleet's memory grows with the number
of homes and is bookkeeping, not frames: the entry list ..., the `homes`
table ..., and the set of file destinations"; the Measurement records the
RSS slope from 1 000 to 10 000 homes as bytes per home.

**16 (minor), unreachable `Skipped`.** Fixed by dropping it: `HomeStatus {
Ok, Degraded(String), Failed(String) }`, no `weight_skipped`, with the
reason stated in FL-2. `plan.md` §2.4 routed.

**17 (minor), missing `FleetError` variants.** Fixed in FL-1:
"`FleetError::ReservedReportName { bldg_id }`", with its test asserting
the variant. FL-3 already names `NoSuchHome` and `Telemetry`. Adding the
three to `plan.md` §2.4 is routed.

**18 (minor), FL-2's bitwise premise.** Fixed: FL-1's capture commit
"records in `02-measurement.md`'s log, for every home of both fleet
goldens, its `RunResult.warnings` and every non-zero `RunResult.health`
field"; FL-2's Verification states the premise and that a home carrying
an event is listed as an expected change, re-captured with the event
named, or its cause fixed first.

**19 (minor), pint redefinition.** Fixed: "Redefinition is an error:
pint's registry warns and replaces on a redefinition by default
(`on_redefinition="warn"` ...), so the helper checks `"inch_H2O_39F" in
ureg` before defining and raises naming the unit and the installed pint
version". The test adds the already-defined case. A check in the helper is
used rather than setting the registry's private `_on_redefinition`, which
pint exposes only through the constructor OCHRE calls.

**20 (minor), OD-10 infiltration height compared with itself.** Fixed:
"the resolved height is 9.75 ft, 2.9718 m, within OpenStudio-HPXML's own
relative 0.01, the value OpenStudio-HPXML v1.12.0 defaults for its
`base.xml` with the height removed, `HPXMLtoOpenStudio/tests/test_defaults.rb`,
`test_infiltration_height_and_volume`", with the fixture's matching inputs
(2700 ft², two floors with one above grade, 8 ft ceiling, conditioned
basement, unvented attic outside the volume, `base.xml:48-54, 82-107`)
asserted first.

**21 (minor), OD-19 constant element.** Fixed: Problem names it, Change:
"A constant setpoint element that is present must parse; otherwise the
parse fails naming the element and its text", test
`setpoint_constant_with_a_bad_value_is_an_error`; the census adds "the 47
constant setpoint elements (34 files, at `afa9a071`) all hold a number".

**22 (minor), OD-20 against B-14.** Fixed: "`09-numeric-fixes.md` B-14's
equipment config commit renames the key the step reads (`capacity_kw` to
`capacity_w`) and keeps that read working, coverage unchanged, until this
entry replaces it with the typed config."

**23 (minor), RT-1 bracket mechanism.** Fixed: "a `#[cfg(test)]` per-site
allocation accumulator on the dwelling: the step path itself takes a
`thread_allocations()` reading before and after the extracted snapshot
function and each domain-slot write ..., adds each difference to that
site's accumulator, and compiles none of this outside the crate's own test
build."

**24 (minor), RT-4 mutator lines.** Fixed: `blueprint.rs:174-222` in both
places.

## verify-L4-e

**13, OD-21.** Checked against the current file: the preamble lists OD-21,
its row (24h) and commit match the schedule, and its evidence is at
`afa9a071`. No change; OD-22 joins the same preamble list.

**14, strict xfails without a reference.** Fixed in OD-11's Change ("A mark
is kept only when its reason cites the reference showing that HARES's
value is the correct one ... or the documented OCHRE defect ... Any other
divergence is a HARES defect handled by `01-baseline-repair.md` BR-12's
rule") and in OD-13's last Change bullet.

**17, review-round wording in OD-20.** Fixed: "found while verifying the
ledger"; the second sentence now reads "The coverage step never registers
coverage for an HPXML PV".

**25, `legacy_fleet_rules`.** Fixed: `unit_keyed_fleet_rules` in FL-1 and
FL-5 (three places). `09-numeric-fixes.md:1488` still uses the old name
(routed).

**28, OD-2 Kind.** Fixed: "Fixes V11, V35, V36, ...".

**29, V-IDs in Kind lines (this file's entries).** Fixed: RT-1 "Fixes V27";
RT-5 "Fixes V25, and V30 for single dwellings (`04-fleet.md` FL-1 fixes it
for fleets)". The other entries of the table are in other owners' files.

**30, V44's `master_seed=0`.** Fixed in OD-7: "`test_gym_zero_master_seed_reaches_the_dwelling`
(a `master_seed` of 0, which the truthiness test dropped for the default,
is the seed the built dwelling's `Dwelling.config()` reports; at the
parent it reports the default)".

**31, line conventions in the OD preamble.** Fixed: "(each test cited by its
`fn` line at `afa9a071`)", `rc_network.rs:1038`, `boundary_rc.rs:2385`;
`ev/tests.rs:8062` is already its `fn` line.

**33, X-5 fleet schema test.** Fixed in FL-1:
"`fleet_aggregate_columns_match_home_report_schema`: on the
`fleet_same_offset_300s` run, every `aggregate` column after `Time` has the
name and the `unit` metadata of the variable of that name in the first
home's own `Timestep` `Memory` report of the same selection, and the type
`Float64`".

## L4-resolution-a item

**"Ticket tolerance" at `ev_driver/mod.rs:6759`.** Fixed as
`10-slop-cleanup.md` SL-11. A census with `plan.md` §3's `[writing]`
pattern at `afa9a071` found what SL-2's narrower pattern missed: 138 lines,
all legitimate (R-values, resistor and diagram labels, IDD versions,
"Table B-1", verbosity-8 comments, the user-documented "Rule R1", `V0`)
except three module docstrings with old ticket IDs
(`test_py_fleet.py:1`, `test_py_lut_injection.py:1`, `test_py_stubs.py:1`)
and an orphan rule number "R3" in `ideal_hvac.rs`. SL-11 also takes the
fourteen other SL-2 matches in `ev_driver/mod.rs`. Fixed by SL-8 (row 25),
which already edits `ev_driver/mod.rs`, with a `git grep` check added to
SL-8's acceptance.

## Found while resolving

- `11-other-defects.md` OD-22: a non-critical equipment whose `init` fails
  is dropped from the model with only a warning
  (`dwelling/mod.rs:2554-2575`), reachable from input (a
  `sensible_gain_fraction` above 1 on a `ScheduledLoad`). Structural fix
  with a fixture census, commit `fix: an equipment whose initialisation
  fails stops construction instead of being dropped`.
- SL-7: `solve_for_output_input_into`, `SolverScratch` and `SolveTarget`
  have no caller and are deleted with the LU path (above, finding 6).

## Schedule changes (for the `00-schedule.md` owner)

- New row 24i, directly after row 24h: OD-22, "an equipment whose
  initialisation fails stops construction", file 11, commit
  `fix: an equipment whose initialisation fails stops construction instead of being dropped`,
  consumer check B0 (CN-17).
- Row 25: entry "SL-8 (fixes SL-11)", file "10".
- Row 62 (RT-2): its state may end as "measured, dropped" under the keep
  or drop rule; no ordering change.
- No row moves. FL-2 (row 51) keeps its position; its degraded fixture
  depends on OR-11 (row 19), already earlier.

## Items for other owners

- `plan.md` owner, §2.4:
  - `Coverage` loses `weight_skipped` (its comment: "FL-1 carries
    `weight_flagged`, which FL-2 renames `weight_degraded`").
  - `pub enum HomeStatus { Ok, Degraded(String), Failed(String) }`.
  - `FleetError`: `NoCoveredWeight { ok: usize, degraded: usize, failed:
    usize, homes: RecordBatch }`; add `ReservedReportName { bldg_id: i64 }`,
    `NoSuchHome { index: usize, len: usize }`, `Telemetry { bldg_id: i64,
    source: HaresError }`.
  - `AlignmentRequired` and `HeterogeneousTimeGrid` are raised by
    `simulate` from the site-offset pre-pass before any home runs; add the
    signatures `hares_io::hpxml::read_site(path) -> Result<Site,
    HpxmlError>`, `hares_io::read_weather_meta(path) -> Result<WeatherMeta,
    WeatherError>`, `hares_core::dwelling::site_offset(&DwellingConfig) ->
    Result<FixedOffset, HaresError>`, and the grid rule by alignment
    (naive starts under `LocalClock` and an undeclared single offset; start
    instants on the declared offset's step grid, sharing a reference
    period, under `Instant`).
  - §2.7: `ForcingKey` and `ForcingTable` exist only if RT-2's measurement
    keeps the table. §2.9, if it lists envelope types: `SolveScratch`
    (`n_x`, `b_u`, `rhs`, `gain`, `d_agg`).
- `09-numeric-fixes.md` owner: B-11 (line 1488) names `legacy_fleet_rules`;
  it is `unit_keyed_fleet_rules`.
- `03-output-reporting.md` owner: FL-2's degraded-home fixture relies on
  OR-11's unmapped schedule-column `Warning` reaching `RunResult.warnings`
  by row 51. If verify-L4-b finding 1 is resolved by moving that row to
  RT-5, tell the `04-fleet.md` owner. `data/examples/BEopt_example_schedule.csv`
  carries `hot_water_dishwasher`, `hot_water_clothes_washer` and
  `hot_water_fixtures`; under OR-11's census row as written they warn,
  which would make every FL-2 fixture home and every ResStock golden home
  `degraded`. Verify-L4-b finding 2's fix needs to cover these columns.
- `02-measurement.md` owner: new log records named by FL-1 (each golden
  fleet home's warnings and health; wall times; bytes per home), RT-2 (the
  surface-independent shares and the keep or drop decision) and FL-2 (any
  golden home found degraded).
