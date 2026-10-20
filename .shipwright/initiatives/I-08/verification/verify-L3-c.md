# Verification L3: 04-fleet, 06-runtime-memory, 10-slop-cleanup, 11-other-defects

HARES HEAD `8b09ac4a`, read with `git show` so the uncommitted work in the
tree is not counted. Evidence is cited at `ebdb1a17`, except where an entry
names `8b09ac4a`. Read-only. Checked against `plan.md` §1 to §3,
`decision-log.md`, `ledger/00-schedule.md` and the OpenStudio-HPXML
workflow-inputs documentation
(<https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>).

Counts: 0 blocker, 9 major, 11 minor.

## Findings

### 1. MAJOR: FL-4 and B-12 disagree on who changes the fleet labels and their type, so `instant_utc_equals_todays_output` cannot pass as written

Entry: `04-fleet.md` FL-4 ("Labels" bullet and its tests), with `09-numeric-fixes.md` B-12.

Evidence:
- At HEAD every fleet row's label is a Utf8 RFC 3339 UTC string with `Z` (`aggregation.rs:636-637`, `Field::new("Time", DataType::Utf8, false)` at `:715`).
- FL-4 says "`Instant` labels the naive time in the declared offset" and "`LocalClock` labels the shared naive local time". Its test `local_clock_labels_are_shared_naive_local_time` expects the first row labelled `2018-01-01T00:00`.
- The same entry requires `Instant(+00:00)` at `Hourly` to reproduce `fleet_mixed_offset`'s golden bitwise. FL-4 also makes that manifest declare `Instant(+00:00)` and verifies with `compare-all`, with no re-capture.
- A naive label such as `2018-01-01T08:00:00` differs from today's `2018-01-01T08:00:00Z`, whether it is stored as Utf8 or as an Arrow `Timestamp`. A `Timestamp` would also change the schema. The `Time` column digest therefore changes.
- B-12 says "FL-4 adds the declared alignment while keeping today's labels (bug-compatible); this entry changes the labels". It then defines the `LocalClock` and `Instant` labels again, and tests them in `local_clock_fleet_is_labelled_by_local_wall_clock`.
- FL-4 never states the `Time` column's Arrow type for any alignment.

Correction: pick one owner and write it into both entries.
- Recommended: FL-4 keeps the Utf8 RFC 3339 representation for every alignment, so it moves no label.
  - `Instant(offset)` renders each row's start instant in the declared offset. `+00:00` renders as `Z`, which equals today's output, so the bitwise test holds.
  - `LocalClock` renders the shared local wall clock with the homes' common offset, or as `Z`-free naive text, stated explicitly.
  - B-12 then moves every alignment to the naive `Timestamp(Millisecond, None)` and owns the label tests.
- Otherwise, FL-4 owns the new labels, re-captures `fleet_mixed_offset`'s `Time` column with the reason recorded, and drops the bitwise claim on `Time`, and B-12 shrinks to single-offset fleets.
- Also fix the wording in `instant_alignment_buckets_on_reference_boundaries`. The UTC-5 and UTC-6 homes start at 22:00 and 23:00 on reference day 2017-12-31, not on "their first day". The arithmetic and the expected rows (2018-01-01 to 2018-01-07, covered weights 2.0, 3.0 and 1.0) check out.

### 2. MAJOR: FL-5's acceptance tests exercise no variable whose rule changes

Entry: `04-fleet.md` FL-5 (and, by the same mechanism, B-11's golden claim).

Evidence:
- FL-5's tests run "on the `fleet_same_offset_300s` inputs at `Minutes(15)`" with no stated selection. Both fleet goldens use the verbosity-2 selection (`fleet_mixed_offset.toml` `output_verbosity = 2`; FL-1's manifest).
- The columns at verbosity 0 to 2 (`crates/hares-io/src/output/columns.rs:54-58, 425-530`) carry only these units: `kW`, `therms/hour`, `kVAR`, `C` and `W`.
  - Every one is `Power` or `Temperature`.
  - Every one already takes the target rule today: a time mean, then a weighted sum for power and a weighted mean for `C` (`aggregation.rs:43-50, 119-126`).
- Mode columns start at verbosity 3 (`columns.rs:532-540`). Ratio, state, `code` and intensive-quantity columns are higher still.
- Consequences:
  - `fleet_cells_follow_declared_kinds` passes at FL-5's parent, so the fix is not tested.
  - `fleet_ratio_and_state_are_weighted_means` is vacuous (no `Ratio` or `State` cell).
  - The promised golden re-capture has nothing to re-capture.
  - B-11's "their mode columns change" fails for the same reason.

Correction:
- Give FL-5's tests an explicit selection, `Verbosity(8)` or a `Named` list, that contains at least one of each:
  - a `Ratio` (`fraction`, `ratio` or `%`);
  - a `State` (`bool`);
  - an intensive `Quantity` (`V` or `J/(kg*K)`);
  - a `Quantity` declared `Sum` over time.
- Each test must fail at FL-5's parent on those columns.
- State that the verbosity-2 fleet goldens are expected to show an empty delta. Alternatively, add a higher-verbosity fleet golden that records the change.
- Apply the same fix to B-11's golden claim, which should name a selection carrying an `Enum` column.

### 3. MAJOR: RT-5 records the gym's clipping statics as deleted by OR-5; they are live at HEAD, and nothing calls `record_clamped_actions`

Entry: `06-runtime-memory.md` RT-5 census row `ACTIONS_CLIPPED`, `ACTIONS_CLIPPED_WARNED` (with committed `03-output-reporting.md` OR-5, `dc033cee` + `469ea955`).

Evidence:
- At `8b09ac4a`, `crates/hares-python/src/py_gym.rs:208,212` still declare both `observe`-only statics, and `:277-284` still increment them process-wide.
- `Dwelling::record_clamped_actions` exists (`crates/hares-core/src/dwelling/mod.rs:4534`), but `git grep record_clamped_actions -- crates/hares-python python` finds no caller.
- So `RunHealth.clamped_actions` is always 0 (every golden records `"clamped_actions": 0`). `HealthPolicy.max_clamped_actions` (OR-12, row 42) cannot fire, and `health()` in Python (PY-9, row 43a) under-reports.
- OR-5's own ledger text (`03-output-reporting.md:461-463`) promised the deletion and the call. The other two statics OR-5 claimed (`REJECTED_SIGNAL_COUNT`, `NON_FINITE_REJECTIONS`) are gone at HEAD.

Correction:
- Per `plan.md` §1 (a finding against a committed entry becomes a new fix entry in the owning area), add a fix entry in `03-output-reporting.md`, scheduled right after the current position and before OR-12 (row 42). It:
  - deletes the two statics;
  - counts clipped values in `map_action_to_signals` in every build;
  - calls `record_clamped_actions` from both the batch and the Python gym paths.
- Its test steps a gym with one out-of-bounds action and reads `health().clamped_actions == 1` in a non-`observe` build.
- Rewrite the RT-5 row's disposition to cite that entry.

### 4. MAJOR: RT-8's (and RT-7's) `#[cfg(test)]` hooks sit in `hares-core` but are read from `hares-python` unit tests, where they do not exist

Entry: `06-runtime-memory.md` RT-8 `gym_reset_builds_nothing_and_runs_no_warmup`; RT-7 `observation_failure_terminates_the_environment`.

Evidence:
- RT-8 asks a `hares-python` unit test to read "a `#[cfg(test)]` counter on `DwellingBlueprint::build_with_seed`". `DwellingBlueprint` is in `hares-core`.
- `cfg(test)` is set only for the crate under test. When `hares-python`'s tests compile (they do run: `crate-type = ["cdylib", "rlib"]`, pyo3 `auto-initialize`, about 170 `#[test]`s), `hares-core` is built without `cfg(test)`, so the counter is not compiled and the test cannot build.
- RT-7's "`#[cfg(test)]` fault injected into one environment's `observe_into`" has the same problem if the fault is placed in `Dwelling::observe_into` (`hares-core`), the only `observe_into` the plan defines.

Correction: put both hooks in `hares-python`.
- A `#[cfg(test)]` counter at the `PyDwelling` site that calls `build_with_seed`, which is the only Python-side builder.
- A `#[cfg(test)]` fault switch in the batch path's wrapper around `observe_into`.
- Or, for RT-8, assert that the dwelling's blueprint `Arc` and a construction serial are unchanged across `reset`.

### 5. MAJOR: OD-3 makes "boiler spec with no typed config" an error, which is the legitimate autosize-pending state, and the real loss of the loop id is in the autosize rebuild

Entry: `11-other-defects.md` OD-3 (with OD-2 site 4).

Evidence:
- `resolve_loop_wiring` runs at resolve time (`crates/hares-io/src/hpxml/equipment.rs:197-236`).
- A gas boiler with no `HeatingCapacity` and `autosize_heating` gets `typed_config = None` on purpose (`resolve_hvac.rs:2105-2115`). OD-2 site 4 extends that "pending autosizing" outcome to every capacity-missing builder.
- OD-3 turns that `None` (`equipment.rs:247-249`) into an error, so a valid autosized boiler with an indirect tank fails construction. The acceptance case "a boiler spec with no typed config ... returns an error" pins exactly that.
- The actual defect is downstream. Autosizing rebuilds the boiler's typed config (`crates/hares-core/src/dwelling/autosize.rs:593-594`) through builders that set `loop_id: None` (`resolve_hvac.rs:957, 1040`). So the wiring is lost for every autosized boiler, whether it was wired or not. It only works by accident when both sides fall back to `LoopId(1)`.

Correction:
- Carry the loop id in the spec's raw parameters, which the post-autosize rebuild reads (or wire after autosizing). Fail only when the boiler config exists and does not deserialize, or when the name is not a boiler.
- Add tests:
  - `autosized_boiler_keeps_its_indirect_tank_loop`: `base-dhw-multiple.xml` with `HeatingCapacity` removed and autosizing on; the boiler and the tank share the loop id after build.
  - `indirect_tank_wiring_errors`, keeping only the wrong-type and non-boiler cases.

### 6. MAJOR: OD-7's "forward every field" test cannot hold against the `from_hpxml` kwargs at HEAD

Entry: `11-other-defects.md` OD-7, test `test_gym_forwards_every_sim_config_field`.

Evidence:
- The Python `SimulationConfig` (`crates/hares-python/src/py_config.rs:48-64`) has 17 fields, including `retain_batches`, `duration` and `time_res`.
- `Dwelling.from_hpxml` accepts only the names in `KNOWN_KWARGS` (`py_dwelling.rs:~2185-2206`). Those names include `duration_s` and `time_res_s`. They do not include `retain_batches`, which `build_config` hard-sets to `true` (`:2449`).
- So "the forwarded keyword set equals the `SimulationConfig` field set" is impossible, and forwarding `retain_batches` is an unknown-keyword error.
- `from_hpxml` already accepts `config=SimulationConfig` (`:2296-2300`).

Correction: `_sim_config_to_kwargs` is replaced by passing `config=sim_config` unchanged. The test asserts that a gym built with `write_output=False` records no output, and that every field of a non-default `SimulationConfig` reaches the dwelling (read back through `PySimulationConfig::from_sim_config` on the built dwelling).

### 7. MAJOR: OD-9's acceptance fixture cannot show the fix, and its recorded delta is zero by construction

Entry: `11-other-defects.md` OD-9.

Evidence:
- `base.xml` has only `SetpointTempHeatingSeason` 68 and `SetpointTempCoolingSeason` 78, with no weekday or weekend profile (`base.xml:357-358`).
- `parse_setpoint_from_control` turns a constant into 24 equal values (`crates/hares-io/src/hpxml/xml_helpers.rs`, the fallback after the extension lookup).
- So hour 0 equals every hour. `generated_schedule_keeps_the_hpxml_setpoint_profile` passes on values at HEAD, and fails only through the `SetpointSource` label.
- "the delta on a generated schedule run of `base.xml`" is identically zero.
- The parser also silently drops a `Weekday/WeekendSetpointTemps*` extension whose length is not 24, and ignores `SetbackTemp*Season`. These are silent paths in code this fix depends on, and are not recorded in any area.

Correction:
- Run both the test and the delta on a fixture with a varying 24-hour profile that differs between weekday and weekend (`tests/fixtures/parity/cz4a_ashp_hpwh/building.xml` or `data/examples/BEopt_example.xml`, both of which carry the extension). Assert the value at a setback hour.
- Record the two parser fallbacks in `11-other-defects.md`, or in `12-inputs.md` under IN-5. A malformed profile is an error. A declared setback is either applied or rejected, never ignored.

### 8. MAJOR: OD-10 leaves its neighbouring fallbacks undecided, though the OpenStudio-HPXML documentation decides them, and the decided rules move numbers in a non-numeric row

Entry: `11-other-defects.md` OD-10 (row 24a, consumer check B0, "a change stops the entry").

Evidence: from the OpenStudio-HPXML workflow inputs documentation.
- `NumberofConditionedFloorsAboveGrade`: required, no default.
  - HARES parses it as optional (`crates/hares-io/src/hpxml/building.rs:557-563`) and the solver substitutes 1.0 (`solver_builder.rs:1435, 1444`).
  - Every checked-in HPXML carries it.
- `InfiltrationHeight`: "If InfiltrationHeight not provided, it is estimated from other inputs (e.g., ConditionedFloorArea, NumberofConditionedFloorsAboveGrade, attics/foundations with WithinInfiltrationVolume=true, etc.)."
  - HARES uses `ceiling_height * floors_above_grade`.
- `HasFlueOrChimneyInConditionedSpace`: defaults to true if any of these holds, otherwise false: a non-electric furnace, boiler, wall or floor furnace, stove or space heater in conditioned space with AFUE below 0.89; a fireplace; or the water heater condition.
  - HARES uses `false`, and parses only the literal `true`, so an xsd `1` reads as false (`building.rs:594-596`).
- `ConditionedBuildingVolume`: OS-HPXML defaults it to `ConditionedFloorArea * AverageCeilingHeight + ConditionedCrawlspaceVolume`; `AverageCeilingHeight` defaults to 8 ft.
  - HARES's parser makes it an error instead.
- `ShieldingofHome`: "normal" for single-family detached or manufactured homes, "well-shielded" for attached homes or apartments. `SiteType`: suburban.
  - HARES maps any missing or unrecognised shielding string to `Normal` (`conversions.rs:1063-1072`) and any other site type to suburban, with a warning (`solver_builder.rs:1396-1408`).
- The same block also defaults `ceiling_height_m` to 2.5 m (`solver_builder.rs:1381`) and the zone id to `ZoneId(zone_idx)` (`:1418-1421`). OD-10 does not list either.
- 15 checked-in HPXML files lack `InfiltrationHeight` and `HasFlueOrChimneyInConditionedSpace` (`base.xml` among them), so adopting the documented rules moves their numbers. The goldens' HPXMLs carry all of these elements, so only the goldens stay put.
- OD-10 defers every decision to "this entry's first step". That is a change the ledger does not state (`dl-i08-process.plan_form`), and a numeric change hidden inside a structural row.

Correction: state the decisions in OD-10.
- In the structural commit, which stays bitwise on every fixture:
  - the volume error (code-built `Building`);
  - `NumberofConditionedFloorsAboveGrade` becomes a parser `MissingField`;
  - the 2.5 m ceiling height and the zone-id fallback become errors (both reachable only from code-built inputs);
  - an unknown shielding or site-type string is a parse error;
  - the boolean parse accepts the xsd forms `true`, `false`, `1` and `0`.
- In a second commit with its own numeric row after the consumer block and its recorded delta:
  - the OS-HPXML `InfiltrationHeight` rule;
  - the `HasFlueOrChimneyInConditionedSpace` rule;
  - the facility-type shielding default;
  - each cited in code and in `docs/PHYSICS_DECISIONS.md`.
- State that the parser keeps requiring `ConditionedBuildingVolume` (a stricter departure from OS-HPXML's default, recorded with its reason), or adopt the OS-HPXML default as part of that numeric commit.

### 9. MAJOR: an untracked silent default in the generated-schedule path that OD-6 and OD-9 build on

Entry: `11-other-defects.md` (no entry); related to OD-6, OD-9 and B-10.

Evidence:
- `load_default_profiles` (`crates/hares-io/src/schedule_resolve.rs:257-270`) warns and returns an empty map when `Default Schedule Parameters.csv` cannot be read.
- `generate_schedule_from_hpxml` then fills every load and hot-water column with a constant 1.0, full load around the clock (`hpxml_schedule.rs:43, 56-63`). It does the same for any profile name missing from the file.
- The generator is also called with `config.defaults_path.as_deref()` (`blueprint.rs:46-56`), so before B-10 no defaults path gives the same result.
- B-10 makes a `DefaultsStore::load` failure an error. This CSV is read separately, so B-10 does not catch it.
- The same function pushes two columns named `occupants` (the constant at `:50-52` and the `("occupants", "Occupancy")` row of `COLUMN_TO_PROFILE`). The name index keeps the later one, so the doc comment "occupancy is constant 1.0" is false.

Correction: add a found-defect entry, or extend OD-9 if its delta rules are acceptable.
- `load_default_profiles` returns `Result`. An unreadable file or a missing required profile is an error naming it.
- The generator reads profiles through the `ParsedDefaults` handle (RT-4).
- The duplicate `occupants` column is removed and the doc corrected.
- Test: a generated schedule with the CSV removed from a copied defaults directory fails naming the file.
- If any run's numbers move, the delta is recorded.

### 10. MINOR: a silent fallback added by committed OR-5 in `SteppableFleet::telemetry` is not recorded

`crates/hares-fleet/src/fleet.rs:638-659` (added in `dc033cee`) maps a failed telemetry snapshot to `None`, "the same signal as an out-of-bounds index", and logs it. `py_fleet.rs` also has an unreachable `fleet.bldg_id(idx).unwrap_or(-1)`.

Correction: record both in `10-slop-cleanup.md`, fixed by FL-3, which edits `SteppableFleet`. `telemetry` returns `Result<Option<_>, _>`, and the dead fallback goes.

### 11. MINOR: FL-1's caller census misses the `Fleet::simulate(n)` callers

The census grep does not match `simulate(<n>)`. It misses:
- `benches/fleet.rs:36`;
- `tests/regression/fleet_scale.rs:30`;
- the `hares-fleet` unit tests (`fleet.rs:1221-1431`);
- `docs/architecture.md:629`;
- the `[[bench]] aggregation` entry in `crates/hares-core/Cargo.toml:109-112`, which goes when `benches/aggregation.rs` does.

FL-1 also says `benches/aggregation.rs` is "rewritten as the fold bench" in one place and replaced by a group in `benches/fleet.rs` in another. Pick one.

### 12. MINOR: `fleet_minutes_15_reduces_one_minute_steps` uses two identical homes

Both homes are `base.xml` with the same schedule and weather, so `a·1 + b·3` equals `4a`, and a home or weight mix-up cannot fail the test. Give the second home a different weather or schedule file.

### 13. MINOR: RT-1 assumes a custom-solver installation point that does not exist

`custom_domain_solvers` is initialised empty at construction (`mod.rs:2613`, `ebdb1a17`), and only unit tests push to the private field (`:14564`). There is no install API, so `DomainSlots::install_custom` has no production caller, and the cited test installs by direct push.

Correction: name the one `Dwelling` method that installs a solver and resolves its slot, and route the existing tests through it.

### 14. MINOR: RT-2's horizon over-sizes the table and needlessly splits keys

Warm-up resets `clock.current_step` to 0 each day (`mod.rs:5094`), so warm-up reads rows `0..steps_per_day`. "duration plus warm-up" adds up to 25 days of unused rows (B-2's `max_days`), and gives a different key to runs that differ only in warm-up.

Correction: horizon = `max(duration steps, steps per day when warm-up is on)`.

### 15. MINOR: `py_dwelling.rs` citations are still at `ebdb1a17` although Task 0 is done

RT-4 and RT-8 cite `reset_with_seed` at `py_dwelling.rs:1518-1532`. At the Task 0 head `1d60f478` it is at `:1559`; at `8b09ac4a`, `:1557`.

`plan.md` §1 requires re-verification at the Task 0 head; that head now exists. Update the citations.

### 16. MINOR: SL-9 contradicts the schedule

`00-schedule.md` says SL-9 closed at row 3, since `fd23dd80` gated the imports (verified: `profile_sim.rs:14-24` at HEAD). SL-9 still says "Fixed by: PR-1 ... gate the two imports and `config`".

Correction: rewrite SL-9's fix line to name `fd23dd80`, and say PR-1 deletes the example.

### 17. MINOR: OD-11 does not say what happens if a parity test now passes

With `VENDOR_OCHRE` defined, the tests run their comparison for the first time since the pint break. `strict=True` turns a pass into a failure.

Correction: state that a test that passes loses its mark in this commit, with the measured agreement recorded.

### 18. MINOR: OD-12 misses a consumer and two run hints

`tests/python/test_py_solar_override.py:19-23` also reads the fixtures. The "Run:" hint in `tests/freefloat_oracle.rs:1232` and the generator's docstring (`:8`) invoke it without `--out`, and must be updated.

The no-argument test imports a module that runs `import pvlib` and `sys.path.insert` at import time. Parse arguments before those imports, so the test checks `--out` and not the environment.

### 19. MINOR: FL-2's "a run missing a variable its own catalogue declares" cannot occur

Under `plan.md` §2.2, a non-nullable missing value fails the run (`Failed`), and a nullable one is null. Drop the clause, or name the case it covers.

### 20. MINOR: RT-3 counts three allocations in `model.output`

`&c * x + &d * u` allocates two vectors, because owned + owned reuses the left buffer. The fix is unaffected. The `nalgebra` note should read 0.35.0 (`Cargo.lock` at HEAD; `Matrix` is still `#[derive(Clone, Copy)]`, `src/base/matrix.rs:163`, so `clone_from` still allocates).

## Items verified correct

**Fleet (04)**
- The facts at `ebdb1a17` hold; `aggregation.rs` is unchanged since apart from a let-chain edit:
  - the time table (`:43-50`, fallback `:63-72`) and the cross-home table (`:119-126`, fallback `:141-150`);
  - `Accumulator` sums in row order and divides once by the non-null count (`:206-243`);
  - the fold adds `v * w` and `w` per home in `successful` (entry) order and divides once (`:667-705`);
  - `has_null` per missing or null home;
  - the bucket intersection and the epoch key.
- FL-1's two-stage table matches `dl-i08-architecture.aggregation`. Its legacy-rule bitwise plan reproduces those operations in the same order, so the fold is bitwise. Fold order is independent of thread count and chunking.
- The memory bound is chunk size times one reduced table, with an exact `peak_live_home_results` test. RSS is a recorded bench only.
- The null rule (no step is null before B-6), `Mode` across homes, flagged homes never counted as success (FL-2), UTC `Z` labels kept until B-12, the multi-offset `AlignmentRequired`, and `Minutes(n)` for sub-hourly output all match the decision log.
- FL-1's fixture exists and works:
  - `bldg0176227/home.xml` (UTC-8, no battery) and its `in.schedules.csv`;
  - `G0600770`, `G0600650` and `G0600790_2018.csv`.
- `fleet_mixed_offset`'s three homes are `Ok` at HEAD. The golden `status` digest `17ef56bc…` equals SHA-256 over the validity byte and three `"Ok"` strings. So a 7-day warm-up does not make any of them `Degraded` under FL-2, and FL-4's covered weights rest on three folded homes.
- The curve-index clamp counter fires only on a configuration mismatch (`hvac_core.rs:1003-1043`), not routinely, so FL-2 does not empty ResStock fleets.
- FL-4's reference-day arithmetic checks out for UTC-5, -6 and -8 against `-07:00`.
- FL-3's evidence holds: `fleet.rs:567-578, 1600-1614`; `py_fleet.rs:444, 485-493`.
- The census of `AggregationResolution` and its relatives is complete apart from finding 11. The `der_parity.rs` hits are false positives, and the `docs/reviews` hits move in SL-8.

**Runtime (06)**
- RT-1:
  - The snapshot evidence holds (`mod.rs:6389-6436, 4892-4930`).
  - `CoreOutput` is plain data, so `equipment_core.insert` does not allocate.
  - The domain drain and push (`environment.rs:705, 728, 733`), the upsert sites (`mod.rs:6010, 6120-6123, 6138`) and the readers are all as cited.
  - `custom_domains` matches 174 lines; `refresh_equipment_caches` runs at the seven cited sites.
- RT-2:
  - Every listed dependency of the cached values is real (`environment.rs:524-676`).
  - The key holds plain values. Warm-up replays rows from step 0, so a per-step table indexed by `current_step` is bitwise.
- RT-3: the allocation sites (`stepping.rs:672-674, 691, 744, 1217`) are as cited; `output_into` is bitwise (the same gemv, then an elementwise add); the existing bound is in `radiant_gain_weights_zero_alloc.rs:241-282`.
- RT-4: the blueprint obstacles (`blueprint.rs:39-88, 183-225`; `mod.rs:1928-1930, 2605`) and fixtures (`defaults/envelope/Envelope Materials.csv`, `base-pv.xml`) exist.
- RT-5:
  - The census matches HEAD except finding 3. `REJECTED_SIGNAL_COUNT`, `WARNED_REJECTED_SIGNAL` and `NON_FINITE_REJECTIONS` are gone.
  - The test helpers exist (`grid_outage_islanding.rs:86, 110`; batteries added after build, outside OD-8's HPXML rule).
  - The spawned-process fixtures exist.
- RT-6, RT-7 and RT-8:
  - The evidence is verified: `base.xml` has a gas furnace (AFUE 0.92) and a central AC; `telemetry()`, `to_observation_vec`, `index_map` and `observation_for_fields`; the substituted observation (`py_gym.rs:486-491`); the reset path (`gym_env.py:546-548`, `vec_env.py:180-182`).
  - The RNG streams are as cited (`rng.rs:16-23, 31`), and the `cz5a_ev_charging` fixtures exist.

**Slop (10)**
- SL-2's census reproduces exactly: 451 lines in 122 files.
- SL-8's counts reproduce (285, 114, 28, 3 and 40). Its reference census at HEAD equals the listed sites, with line drift only. `git ls-files -ci` lists exactly the two files named.
- SL-7's premise holds: `m_is_identity: true` at both constructors (`state_space.rs:228, 425`); no `false` anywhere; no use outside `hares-envelope`; the cited symbols are at their lines.
- SL-1 and SL-3 to SL-6 are unchanged from L2 and correct.

**Other defects (11)**
- OD-1 is real at HEAD: `create_exception!(_hares, …)` (`py_dwelling.rs:30-37`) against `module-name = "ochre_next._hares"` (`pyproject.toml:81` at HEAD, `:77` at `ebdb1a17`).
- OD-2's site 4 lines check out (`resolve_hvac.rs:851, 994, 1071, 1139, 1150-1155, 1233, 1240, 1244, 1291, 1322, 2234`; `mod.rs:2039-2049`).
- OD-6 is real (`blueprint.rs:46-56`, `mod.rs:1756-1766`).
- OD-8 is unchanged from L2.
- OD-11 is real: `VENDOR_OCHRE` is undefined in `test_thermal_trace.py`, and the only three `strict=False` marks are at `:231, 326, 377`.
- OD-12 is real (`generate_pvlib_solar_override.py:29, 174-175`).

**Ordering and commits**
- Every dependency in scope precedes its entry in `00-schedule.md`:
  - SL-7 (20) before RT-3 (21);
  - PR-3 (16) before RT-1, RT-3 and RT-7;
  - OD-6 (24) before RT-4 (61), which is before RT-2 (62);
  - IN-6 (33) before RT-2;
  - IN-5 (64) before RT-6 (65), with IN-2 (66) the next commit;
  - PY-7 (26) before OD-1 and OD-7;
  - OD-2 (31) before OD-8 (32);
  - FL-1 to FL-4 (50 to 53) after OR-1, OR-3, OR-6, OR-7 and OR-12 to OR-14;
  - FL-5 (87a) after B-6 and before B-11;
  - OD-9 (94a) before IN-8;
  - OD-10 to OD-12 (24a to 24c) after M-2 and before PY-7.
- Every commit message matches its schedule row and passes the `[writing]` pattern.
