# L3 resolution c: 04-fleet, 06-runtime-memory, 10-slop-cleanup, 11-other-defects

Resolves every finding of `verify-L3-c.md` (9 majors, 11 minors) and every
finding of `verify-L3-e.md` that touches these four files. Code facts were
re-read at `21d90588` (HEAD) with `git show`, and the OpenStudio-HPXML
"Workflow Inputs" page was read for OD-10's rules. Every new or changed
citation in the four files states its commit.

Counts: 20 of 20 `verify-L3-c` findings fixed, none rejected; 8 of 8
`verify-L3-e` findings touching these files fixed (two of them, blockers 1
and 2, were written into `11-other-defects.md` as OD-13 and OD-14 in this
round outside this resolution, as `01-baseline-repair.md` BR-6c and BR-12
cite them; they were checked and made consistent here). Two further defects
found while verifying were added (SL-10 and the OD-10 legacy flue element;
both listed below). New entries: SL-10, OD-17, OD-18, OD-19.

## verify-L3-c

### 1. MAJOR: FL-4 and B-12 label ownership: fixed

FL-4 keeps the Utf8 RFC 3339 labels for every alignment; B-12 owns the
move to the naive `Timestamp`. FL-4 "Labels": "the `Time` column keeps
FL-1's representation, a Utf8 RFC 3339 string (`Field::new("Time",
DataType::Utf8, false)`, `aggregation.rs:715` at `21d90588`), for every
alignment. `09-numeric-fixes.md` B-12 moves every alignment to a naive
`Timestamp(Millisecond, None)` and owns the local-time label tests ...
`Instant(offset)` renders each row's start instant in the declared offset
with today's formatter (`to_rfc3339_opts(SecondsFormat::Secs, true)`), so
`+00:00` renders with `Z`, byte for byte today's label ... `LocalClock`
renders the shared naive local period start with no offset". Tests now
expect `2018-01-01T00:00:00` (`LocalClock`) and
`2018-01-01T00:00:00-07:00` (`Instant(-07:00)`), and
`instant_utc_equals_todays_output` holds "the `Time` column included (Utf8,
`Z` labels)". Reference-day wording corrected: "the UTC-5 and UTC-6 homes
start at 22:00 and 23:00 on reference day 2017-12-31 and the UTC-8 home at
01:00 on reference day 2018-01-01 ... (seven rows; 2017-12-31 and
2018-01-08 each lack a home)". The Kind line now says B-12 moves "every
alignment to a naive local `Timestamp`".

### 2. MAJOR: FL-5 tests exercise no changed rule: fixed

FL-5's acceptance now names the classes and fails at its parent:
`fleet_fold_follows_declared_kinds` "with one variable of each kind whose
rule changes: a `Ratio` (unit `fraction`), a `State` (`bool`), an
intensive `Quantity` declared `AcrossHomes::Mean` (`V`), a `Quantity`
declared `Sum` over time and `AcrossHomes::Sum` (`count`) and an `Enum`
(`code`)"; `home_reducer_sums_a_quantity_declared_sum` ("reduces it to 6
(the parent averages it to 2)"); and `fleet_cells_follow_declared_kinds`
on the golden inputs "with `Selection::Pattern("*")` ... first asserts
that the selection holds at least one `Ratio`, one `Enum` and one
intensive `Quantity`". Goldens: "compare bitwise (their verbosity-2
columns keep their rules; an empty delta is the expected result and is
recorded)". The delta is recorded on the `Pattern("*")` run. The B-11
half is a note for the `09` owner (below).

### 3. MAJOR: live clipping statics, `record_clamped_actions` never called: fixed

New entry `11-other-defects.md` OD-18 (placed in `11` as instructed, not
`03`): "The two statics and the warn-once log are deleted. One function,
`clamp_action(action, action_layout) -> (Vec<f64>, u64)`, clamps each value
to `field_bounds` and counts the clamped values in every build ...
`batch_step` calls `record_clamped_actions` on each environment's
dwelling ... The pure-Python paths call a new binding,
`hares._hares.clip_action(dwelling, action, action_layout)`". Test
`batch_step_records_clamped_actions` reads `health().clamped_actions` 1 and
0 in a build without `observe`. RT-5's census row now reads "still live at
`21d90588` ... deleted by `11-other-defects.md` OD-18", and RT-5 depends on
OD-18. Placement for the schedule owner: row 30b, after PY-5 and PY-12,
before OR-12 (row 42), consumer check B0 (CN-13).

### 4. MAJOR: RT-7 and RT-8 `cfg(test)` hooks in the wrong crate: fixed

RT-7: "the batch path reads each environment's observation through one
wrapper function in `py_gym.rs` around `Dwelling::observe_into`, and that
wrapper carries a `#[cfg(test)]` fault switch ... the switch lives in
`hares-python`". RT-8: "a `#[cfg(test)]` build counter on `PyDwelling`,
incremented at its one call of `DwellingBlueprint::build_with_seed` ...
the dwelling's blueprint `Arc` is the same object before and after
(`Arc::ptr_eq`) ... Calling `reset_with_seed(7)` instead increments the
counter by one, so the counter is shown to count."

### 5. MAJOR: OD-3 targets the wrong defect: fixed

OD-3 rewritten to the autosize rebuild: "Autosizing then rebuilds the
boiler's typed config (`autosize.rs:593-594`) through builders that set
`loop_id: None` (`resolve_hvac.rs:957` for the gas boiler, `:1040` for the
electric boiler), and the boiler falls back to `LoopId::default()` at
construction (`boiler.rs:169-170` and `:549-550`)". Change: "The loop id
lives in the spec's raw parameters, which the post-autosize rebuild reads
... A boiler pending autosizing is not an error". Tests:
`autosized_boiler_keeps_its_indirect_tank_loop` on `base-dhw-multiple.xml`
with `HeatingCapacity` removed, `indirect_tank_wiring_errors` (wrong type,
not a boiler, tank payload), `boiler_without_loop_id_is_a_construction_error`.
Verified: the one golden boiler (`bldg0000002` 2025.1, `HeatingCapacity`
37881.0) is not autosized, so goldens stay bitwise. Commit message changed
(schedule note below).

### 6. MAJOR: OD-7 forwarding cannot be complete: fixed

"`_sim_config_to_kwargs` is deleted, and the gym passes
`config=sim_config` to `from_hpxml` unchanged". Test: "a gym built with a
`SimulationConfig` whose every field differs from its default ... the
config read back from the built dwelling (`Dwelling.config()`,
`py_dwelling.rs:1624`, built by `PySimulationConfig::from_sim_config`)
equals the one passed on every field the class exposes".

### 7. MAJOR: OD-9 fixture cannot show the fix: fixed

Fixture `tests/fixtures/resstock/2025.1/bldg0000002/home.xml` (weekday and
weekend heating profiles differ; verified by census, `cz4a_ashp_hpwh` and
`BEopt_example.xml` have identical weekday and weekend profiles), start
Friday 2018-01-05, 3 d: "the scheduled heating setpoint at 03:00 on Friday
is 70 F, at 06:00 on Friday 67 F and at 06:00 on Saturday 70 F ... At the
parent every step reads 67 F". The delta is that run. The two parser
fallbacks are new entry OD-19: "A profile element that is present must
hold 24 values that each parse ... A thermostat that declares a setback or
setup element fails the parse naming the element". Setback is rejected,
not applied: applying it is a new capability (scope), and no fixture
declares one. OD-9 depends on OD-17 and OD-19. Placement: OD-19 at row
24g, B0 (CN-17).

### 8. MAJOR: OD-10 neighbouring fallbacks undecided: fixed

Every fallback is decided against the documentation, quoted in the entry.
Structural commit (row 24a, bitwise): volume error; "`NumberofConditionedFloorsAboveGrade`
is a parser `MissingField`, and `Building.floors_above_grade` becomes
`f64`"; `ceiling_height_m` becomes `f64` and the zone-id fallback is an
assembly error; unknown shielding and site-type strings are parse errors
(missing `SiteType` resolves to the documented suburban, today's value);
"One helper, `parse_xsd_boolean` ... `true` and `1` are true, `false` and
`0` are false" at all ten boolean sites; and "`ConditionedBuildingVolume`
stays required by the parser" with its reason recorded. Numeric commit
(own row, recorded delta on `base.xml` and the two
`base-bldgtype-multifamily*` files): the `InfiltrationHeight` estimate, the
`HasFlueOrChimneyInConditionedSpace` rule and the facility-type shielding
default, each cited in code and in `docs/PHYSICS_DECISIONS.md`. Found while
verifying: thirteen files declare the flue as `<HasFlueOrChimney>`, which
the parser ignores; the structural commit makes it "a parse error naming
its current name" and migrates the thirteen files (all `false`, today's
value). Placement: numeric commit at row 94f, after OD-2 site 6 (94e) and
before CN-3 (95), consumer check "delta".

### 9. MAJOR: untracked generated-schedule default: fixed

New entry OD-17: "`load_default_profiles(dir) -> Result<DefaultProfiles,
HaresError>` ... The generator takes `&DefaultProfiles` in place of
`Option<&Path>` ... The constant `occupants` column is deleted ... columns,
names and aggregations are pushed together". Also found and fixed in the
same code: the 14 columns against 13 aggregations would drop
`cooling_setpoint` on a downsample (`schedule.rs:262`); the loader's
weekend and monthly substitutions (the CSV gains the missing `Ceiling Fan`
`month_multipliers` row of 1.0, today's value); the resolver's 0.0 kW and
constant-power fallbacks become errors. RT-4 now reads the profiles from
`ParsedDefaults` and depends on OD-17. Placement: row 24f (after OD-6 and
OD-14, before SL-8, RT-4 and OD-9), B0 (CN-17).

### 10. MINOR: `SteppableFleet::telemetry` fallback and dead `unwrap_or(-1)`: fixed

New `10-slop-cleanup.md` SL-10, fixed by FL-3. FL-3: "`telemetry(index)`
returns `Result<DwellingTelemetry, FleetError>`: `FleetError::NoSuchHome
{ index, len }` ... `FleetError::Telemetry { bldg_id, source }`", the same
for `set_grid_voltage` and `apply_control` (found: they also only log an
out-of-range index), and "the unreachable `bldg_id(idx).unwrap_or(-1)`
(`py_fleet.rs:446`) goes". Tests `steppable_fleet_accessors_reject_a_missing_home`
and `fleet_error_maps_to_python_exception`.

### 11. MINOR: FL-1 caller census: fixed

"The second, `git grep -nE '\bfleet(_clone)?\.simulate\(' -- benches tests
crates tools docs`, names the callers of `Fleet::simulate(n_threads)`
... `benches/fleet.rs:36`; `tests/regression/fleet_scale.rs:30`; the
`hares-fleet` unit tests (`fleet.rs:1221, 1318, 1341, 1378, 1395, 1414,
1431`); `py_fleet.rs:199, 201`; `tools/frame_golden/src/adapter.rs:299`;
`docs/architecture.md:629`". One bench decision: "`benches/aggregation.rs`
and its `[[bench]] aggregation` entry (`crates/hares-core/Cargo.toml:109-112`)
are deleted; the fold is benchmarked by the `fleet_fold` group of
`benches/fleet.rs`". `test_py_fleet.py` citations moved to `21d90588`.

### 12. MINOR: identical homes in the 15-minute test: fixed

"the second with `data/examples/bldg0112631_schedule.csv` (a different load
profile) and weight 3 ... The test first asserts that `a` and `b` differ in
at least one row".

### 13. MINOR: RT-1 custom-solver install API: fixed (API defined)

"Custom solvers are installed through one method,
`Dwelling::install_domain_solver(&mut self, solver: Box<dyn DomainSolver>)
-> Result<CustomDomainSlot, HaresError>` ... installing after the
dwelling's first step, is an error naming the id". The Problem now states
that no production code installs one, and both existing tests install
through the method. Defined rather than removed: `DomainSolver` is public
and the step loop already runs custom solvers, so RT-1's slot design needs
the one entry point.

### 14. MINOR: RT-2 horizon: fixed

"The horizon is `max(duration steps, steps_per_day)` when warm-up is on
and the duration steps otherwise", citing `mod.rs:5207-5209` at `21d90588`.

### 15. MINOR: `py_dwelling.rs` citations: fixed

RT-4 and RT-8 cite `py_dwelling.rs:1570-1584` at `21d90588` (the current
HEAD; `:1557` was `8b09ac4a`).

### 16. MINOR: SL-9 contradicts the schedule: fixed

SL-9's fix line had already been rewritten when this file was re-read; it
reads "**Fixed by:** `01-baseline-repair.md` BR-2 (`fd23dd80` gates the
imports and `config` on `profiling` ...); `07-profiler.md` PR-1
(`d7f0e381`) then deletes the example", and the "Fix" sentence is gone.
Verified: `d7f0e381` deletes `crates/hares-core/examples/profile_sim.rs`.

### 17. MINOR: OD-11 `strict=True` and a passing test: fixed

"A test that passes in this commit loses its `xfail` mark in this commit,
and its measured agreement ... is recorded". OD-13's acceptance now reads
each test "as OD-11 left it (an `XFAIL` raised by its assertion, or a
pass)". OD-11's acceptance command names the environment
(`PYTHONPATH=vendors/OCHRE uv run --all-groups --no-sync pytest ...`).

### 18. MINOR: OD-12 missed consumer and run hints: fixed

Adds `tests/python/test_py_solar_override.py:18-24`, the hints at
`tests/freefloat_oracle.rs:1232` and the docstring `:8`, both to read
"`uv run --group ochre python tests/python/generate_pvlib_solar_override.py
--out <dir>`"; "It parses its arguments before any third-party import";
the unused `vendors/OCHRE` `sys.path` insertion goes; the test runs with
an environment whose `pvlib` import raises.

### 19. MINOR: FL-2 impossible case: fixed

The clause is deleted: "a run with no rows is `Degraded`; a run error or
panic ...".

### 20. MINOR: RT-3 counts and nalgebra version: fixed

"nalgebra 0.35.0 (`Cargo.lock` at `21d90588`) `Matrix` is
`#[derive(Clone, Copy)]` (`src/base/matrix.rs:163`)" and "two vectors, one
per product; the owned-plus-owned sum reuses the left operand's buffer".

## verify-L3-e findings touching these files

- **Blocker 1** (permanent skips): fixed in `11-other-defects.md` OD-13,
  written outside this resolution. Checked here: it runs the three OCHRE
  modules through `pythonpath = ["vendors/OCHRE"]`, deletes the
  assertion-free tests, gives the six fleet tests a committed fixture and
  empties the census. FL-1's rewrite of those six tests now builds on that
  fixture ("on the committed `fleet` fixture `11-other-defects.md` OD-13
  gives them").
- **Blocker 2** (pint cap): fixed in OD-14 (likewise), option (a);
  BR-6c already cites it.
- **Major 4** (SL-9 bullet): fixed, as finding 16 above.
- **Major 8** (FL-5 and `Enum`): fixed. FL-5's end-to-end test computes the
  expected `Enum` cell "by the declared time rule (for `Enum`, the mean
  until B-11, not the home report's `Mode`)", so it no longer compares
  against OR-13's `Mode`; its unit test includes an `Enum` on `code`.
- **Major 9** (OD-2 site 6 outside the numeric block): fixed outside this
  resolution in OD-2 (site 6 "in its own row of the numeric block
  (`00-schedule.md` row 94e)"); checked.
- **Minor 22** (memory file names in RT-1): fixed: "no per-step allocation,
  search or sort in the step path: `dl-i08-criteria`,
  `dl-i08-architecture.engine_performance`".
- **Minor 24** (ideal-capacity compute): fixed in RT-3's Measurement: "If,
  after this entry, the ideal-capacity phase is at or above 5% of
  `simulate()` on `consumer_shape_900s` ... the row stops and a new entry
  that removes the repeated resolve is added".
- **Minor 15** (SL-9 in two schedule rows): the entry text is consistent;
  the schedule row text is the `00` owner's (below).

## Items for other owners

- **`00-schedule.md`:**
  - New rows: 24f OD-17 (`fix: missing or malformed default schedule
    profiles are errors, not full load`, B0 (CN-17)); 24g OD-19 (`fix:
    reject malformed HPXML setpoint profiles and setbacks HARES does not
    model`, B0 (CN-17)); 30b OD-18 (`fix: count clamped RL actions in each
    run's health in every build`, B0 (CN-13)); 94f OD-10 numeric commit
    (`fix: default infiltration height, flue and shielding by the
    OpenStudio-HPXML rules`, delta), placed after 94e and before CN-3.
  - Changed commit messages: row 23 OD-3 `fix: keep an autosized boiler on
    its indirect tank's loop and fail when wiring cannot apply` (title
    "an autosized boiler keeps its indirect tank's loop; wiring fails
    loudly"); row 24a OD-10 `fix: infiltration set-up fails on missing or
    unknown inputs instead of assuming defaults` (title "infiltration
    set-up fails on missing or unknown inputs; OD-10's numeric commit is
    row 94f").
  - Rule text: add OD-10's numeric commit to the numeric-fix list
    (`00-schedule.md:21-24`), and SL-10 to "Entries closed inside another
    row" (row 52, FL-3).
  - Verify-L3-e minor 15: SL-9 is listed once, as row 3 (`fd23dd80`); the
    deletion belongs to PR-1's own row text.
- **`09-numeric-fixes.md` B-12:** FL-4 now keeps Utf8 RFC 3339 labels for
  every alignment (`Instant(offset)` rendered in the declared offset, `Z`
  at `+00:00`; `LocalClock` as naive text with no offset). B-12's "FL-4
  adds the declared alignment while keeping today's labels
  (bug-compatible)" should state those renderings, and B-12 owns the move of
  every alignment to `Timestamp(Millisecond, None)` and the naive-label
  tests; its measurement is a label delta from those strings.
- **`09-numeric-fixes.md` B-11:** "the `fleet_same_offset_300s` and
  `fleet_mixed_offset` goldens re-captured (their mode columns change)" is
  false: both select the verbosity-2 tier, which holds no `Enum` column.
  B-11's tests should use FL-5's selections (a fold unit test with an
  `Enum` on `code`, and `Pattern("*")` on the `fleet_same_offset_300s`
  inputs, whose equipment `enum` modes are `Enum`), state that the goldens
  compare bitwise, and record the delta on the `Pattern("*")` run.
- **`03-output-reporting.md` OR-5:** its as-executed record
  (`03:452`, `:504-506`) says the two gym statics were deleted and
  `record_clamped_actions` is called; neither happened. Rewrite it to cite
  `11-other-defects.md` OD-18, and add OD-18 to OR-12's dependencies.
- **`05-python-surface.md`:** PY-8 (`05:185-191`) names
  `_sim_config_to_kwargs`, which OD-7 now deletes (the gym passes
  `config=sim_config`); PY-9's `health()` test can assert
  `clamped_actions` after one out-of-bounds gym action on both paths,
  which OD-18 makes non-zero.
- **`12-inputs.md`:** IN-3's element list carries OD-19's rejected setback
  and setup elements and OD-10's legacy `HasFlueOrChimney` as rejected
  elements; IN-5 follows OD-19 (row 24g), which makes the profiles it reads
  strict.
- **`02-measurement.md`:** log entries this resolution adds: FL-5's delta on
  its `Pattern("*")` run and the empty golden delta; OD-9's delta on the
  `bldg0000002` generated-schedule run; OD-10's numeric delta; RT-3's
  ideal-capacity share and the 5% decision.
