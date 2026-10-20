# Verification L4-c: fleet, runtime and memory, slop cleanup, other defects

Scope: `ledger/04-fleet.md`, `ledger/06-runtime-memory.md`,
`ledger/10-slop-cleanup.md`, `ledger/11-other-defects.md`, checked against
`plan.md` §1 to §3, `decision-log.md`, `ledger/00-schedule.md` and the code at
HEAD `afa9a071` (`git show HEAD:<path>`; evidence cited at `ebdb1a17`,
`21d90588` or `8b09ac4a` was read at that commit). The OpenStudio-HPXML rules
were read from <https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>;
pint behaviour from `pint/default_en.txt` and `pint/facets/plain/registry.py`
on pint's main branch.

Counts: 0 blockers, 7 majors, 17 minors.

## Findings

### Majors

**1. Major. FL-2 `flagged_home_is_not_counted_as_success` cannot be built as written.**
Evidence: the test gives a fleet home "a test actor [that] sends one
out-of-bounds control signal". A fleet home is a `DwellingConfig`
(`crates/hares-fleet/src/fleet.rs:51-54`, `FleetEntry { config, sample_weight }`),
and `DwellingConfig` (`crates/hares-core/src/dwelling/mod.rs:210-228` at HEAD)
has no actor field. Actors attach only through `Dwelling::add_actor` on a
built dwelling (`mod.rs:3267`) or are seeded from equipment
(`build_actors_from_seeds`, `mod.rs:7845`). `Fleet::simulate` builds and runs
each home internally, and no ledger entry adds a per-home actor hook.
Correction: drive the degraded home from its configuration alone. The
health events of `plan.md` §2.3 offer one: give the home a warm-up of one
day (`initialization_duration` of 86 400 s until B-2, then `warmup` with
`max_days = 1`), which ends `WarmupOutcome::Ran { converged: false, residuals: None }`
and makes the home `Degraded` by FL-2's own mapping. State the fixture
(`base.xml`, the other home with warm-up disabled), assert the `homes` row
reason names the warm-up, and keep the `n_succeeded == 1` assertion. If a
test-only actor hook is preferred instead, the ledger must add it to
`DwellingConfig` and §2.4, which is new public surface; the warm-up route
needs none.

**2. Major. FL-2's "a run with no rows is `Degraded`" is unreachable, and `empty_home_is_degraded` has no fixture.**
Evidence: today a zero-row home comes from a missing retained frame
(`aggregation.rs:293-299`, `extract_rows` returns `None` when
`retain_batches` is false). Under FL-1 the fleet appends its own `fleet`
report at `Timestep` to every home, so every home that runs one step has
rows; zero steps is rejected by validation (`config.rs:149-151`,
duration > 0; `:166-170`, duration a multiple of the step; `plan.md` §2.7's
`validate()`), and FL-4 forbids a home with a different grid. An empty
restricted selection is defined by FL-1 as "not applicable", not a failure.
Correction: delete the "no rows" mapping and `empty_home_is_degraded`, or
name the one reachable case and test it. If a home whose restricted
selection is empty is meant, say so: decide whether it is `Ok` with zero
covered weight in every column (FL-1's rule) or `Degraded`, and give the
test its fixture (for example a `Named` selection of one battery variable
over `base.xml` and `base-battery.xml`).

**3. Major. FL-2's `FleetError::NoCoveredWeight` drops every home's status and reason.**
Evidence: FL-2 raises `NoCoveredWeight { ok, degraded, failed, skipped }`
(`plan.md` §2.4) when the `Ok` weight is zero, so the `homes` table, which
holds each home's status and reason, is never returned. The decision says
"Every home's coverage and status is reported"
(`dl-i08-architecture.fleet`), and `dl-i08-criteria` forbids losing
observability or debuggability. A fleet where every home degraded is exactly
the case where the reasons are needed.
Correction: `NoCoveredWeight` carries the per-home rows
(`homes: RecordBatch`, or `Vec<(i64 bldg_id, HomeStatus)>`) next to the
counts, and Python raises with them attached (an attribute on the
exception). Update §2.4's `FleetError` and add the assertion to
`zero_ok_weight_raises`: the error lists every home with its reason.

**4. Major. FL-4's `AlignmentRequired` as a construction error needs every home's site offset, which is only known after parsing.**
Evidence: the dwelling's UTC offset is not the config's: `from_parts`
reinterprets the naive local start in the resolved site offset
(`crates/hares-core/src/dwelling/blueprint.rs:75-98`,
`resolve_site_location` over the HPXML site and the weather header). M-2's
`fleet_mixed_offset` gives all three homes `start_time = ...-06:00`
(`tests/fixtures/golden/fleet_mixed_offset.toml`) and its offsets come from
the parsed inputs. FL-1 itself says a catalogue "exists only once a home's
inputs are parsed, so the check cannot run at fleet construction without
parsing every home twice or holding every blueprint", which applies equally
here. `mixed_offset_fleet_requires_declared_alignment` expects the
construction error to name all three offsets.
Correction: state the mechanism. Either a construction-time pass that
resolves each home's site offset without a full parse (the HPXML
`Site`/time-zone elements and the weather header that
`resolve_site_location` reads, plus `SimulationConfig.site_location`),
cited and tested as reading no other input; or define the error as raised
before the first chunk runs from such a pass. Do not raise it mid-run after
chunks have folded. Name the function and add a test that the pre-pass
agrees with the offset each built dwelling records (`hares:utc_offset_s`).

**5. Major. FL-4 under `Instant` makes aligned coverage impossible for a mixed-offset fleet.**
Evidence: FL-4 requires "naive local start time, duration and resolution"
to match (`HeterogeneousTimeGrid`), and the start is reinterpreted at each
home's own offset (`blueprint.rs:90-98`). Homes at different offsets
therefore always start at different instants, and under `Instant` every
first and last reference period is partly covered and nulled for some homes
(FL-4's own test: covered weight 2.0, 3.0, 1.0). The decision defines
`instant` as "same physical moment" (`dl-i08-architecture.fleet`); a
grid-study fleet cannot be configured to cover the same physical window.
Correction: under `Instant(offset)` the grid rule compares start instants
(naive start at the home's resolved offset), duration and resolution; under
`LocalClock` and an undeclared single-offset fleet it compares naive local
starts. Add a test with three homes whose naive starts are chosen to share
one instant: every reference period has covered weight 3.0. Keep the
existing partial-coverage test (it then needs per-home starts, which the
`frame_golden` fleet manifest must allow per home).

**6. Major. RT-3 omits the ideal-capacity solve's per-step allocations, which its Measurement says it removes.**
Evidence: `solve_ideal_capacity_for_target`
(`crates/hares-envelope/src/thermal_solver/stepping.rs:366-...` at HEAD) is
called every step for every conditioned zone by `SolverFeedbackActor`
(`crates/hares-core/src/actors/solver_feedback.rs:61`). Its two arms that
survive SL-7 allocate on every call:
`solve_for_scalar_input_identity_coupled` (`state_space.rs:780-...`) builds
`&self.n_mat * x + &self.b_eff * u - self.b_eff.column(..) * u_i` (three
vector temporaries) and `vec![0.0f64; n]` for `d_agg`;
`solve_for_scalar_input` (`state_space.rs:855-900`, behind
`solve_for_output_input`) builds the same temporaries plus
`.into_owned()`, `c_row * &x_next_fixed` and `d_row * u`. RT-3's change
list names only `prepare_inputs_inner`, the latent map, `last_u`,
`model.output` and the solar slot map, and its test covers only
`ThermalSolver::resolve`, which never calls the ideal-capacity solve, so
RT-3's own "if the test reports another site, add it" rule cannot catch
these. The no-allocation rule for the hot loop is binding (RT-3: "These are
unconditional").
Correction: add both solves to the change list (scratch vectors sized at
construction; `mul_to`/`gemv` into them in the same operation order as
today so the result is bitwise equal; `d_agg` as a persistent buffer), and
extend the acceptance test to bracket `prepare_inputs`, every zone's
`solve_ideal_capacity_for_target` and `integrate` over 100 steps after the
first: zero allocations. SL-7 already deletes the unused `_into` variants'
`m_lu` solves; say whether the surviving `_into` functions are the scratch
path or are deleted.

**7. Major. OD-20's acceptance test contradicts its own no-constant rule.**
Evidence: `base-pv.xml`'s only roof has no `Azimuth` (`:110-123`, `Pitch`
6), and the envelope registers a roof with no azimuth as omnidirectional
(`crates/hares-core/src/environment.rs:3030`, "Exterior roof with missing
azimuth must set `omni_directional: true`"). OD-20 says "a roof is matched
by the orientation the envelope already uses for that roof's solar gains
... no constant stands in for a missing value", yet
`hpxml_pv_registers_roof_coverage` expects the 180° array to attach and
the 90° array not to. That outcome holds only with today's
`b.azimuth_deg.unwrap_or(180.0)` (`mod.rs:1534`), the constant the entry
deletes. Under the stated rule an omnidirectional roof has no azimuth to
compare, so either both arrays match or neither.
Correction: state the rule for an omnidirectional roof (for example: tilt
alone decides, so both arrays attach and their coverages sum; or a roof
without an azimuth is never matched, with a warning naming it) and rewrite
the test's expectations from that rule, or use a fixture whose roof
declares an `Azimuth`. Keep the summed-coverage bound test consistent with
the chosen rule (two arrays on one roof).

### Minors

**8. Minor. RT-1 describes `Telemetry` as it was at `ebdb1a17`; the hand-written `Clone` must copy the non-finite latch.**
Evidence: at HEAD `Telemetry(pub HashMap<String, f64>, Option<(String, f64)>)`
(`crates/hares-types/src/telemetry.rs:18-19`, added by OR-5). RT-1's
`clone_from` speaks only of the map ("falls back to `self.0.clone_from`").
Correction: cite the HEAD struct and state that `clone_from` also does
`self.1.clone_from(&source.1)` (no allocation when both are `None`), with
`telemetry_clone_from_same_keys_allocates_nothing` asserting that a latched
source yields a latched copy.

**9. Minor. RT-3's evidence is labelled "at HEAD" but is at `ebdb1a17`, and its PR-3 dependency text is already done.**
Evidence: `stepping.rs:1217`, `thermal_solver/mod.rs:1282, 1523, 1560-1568, 1217`
are `ebdb1a17` lines; at HEAD they are `stepping.rs:1208`, `mod.rs:1271,
1512, 1549, 585/1206`. `radiant_gain_weights_zero_alloc.rs:33-48` already
installs `hares_types::alloc_count::CountingAllocator` (PR-3, `26626710`).
Correction: write "at `ebdb1a17`" and drop the "in place of their own copy"
clause (it is done); keep PR-3 as a satisfied dependency.

**10. Minor. RT-2 has no measurement gate for the surface-independent table, and a single run gains nothing from it.**
Evidence: `context.md` item 2 measured `update_in_place` at 7.9% of
`simulate()` including the per-surface Perez loop; the surface-independent
part is a fraction of that, and a lone dwelling computes each row once
either way while paying the table's memory up front. RT-2 applies the
5% rule only to the per-surface table.
`dl-i08-architecture.engine_performance`: "An item the profile shows is
not worth it is dropped with the measurement recorded".
Correction: record the surface-independent share on `consumer_shape_900s`
and on M-3 item 5's repeated-build loop, and state the keep/drop rule for
the table itself (kept when the repeated-build saving is above the noise
band).

**11. Minor. RT-2's `forcing_key_differs_by_utc_offset_at_equal_instant` does not isolate the offset.**
Evidence: the two blueprints differ in naive local start (00:00 against
02:00) as well as offset, and the naive start is already a key field.
Correction: add a pair with the same naive local start and different site
offsets (different instants), whose keys differ only in the offset field
and whose tables differ in the solar rows.

**12. Minor. RT-5's static census misses two statics present at HEAD, and one `Send` citation is wrong.**
Evidence: the census command at HEAD also prints `INSTALLED` and
`THREAD_ALLOCATIONS` (`crates/hares-types/src/alloc_count.rs:28, 35`,
PR-3). `Fleet::simulate`'s `par_iter` (`fleet.rs:345`) builds and drops
each dwelling inside the closure, so it does not need `Dwelling: Send`;
`SteppableFleet`'s parallel step over `Vec<Dwelling>` and `#[pyclass]
PyDwelling` do.
Correction: add a row (process-wide allocator flag and per-thread counter,
unchanged, never feed `RunHealth`); cite `SteppableFleet`'s parallel step
for the `Send` requirement.

**13. Minor. FL-1's caller census omits `tests/regression/aggregation_check.rs:153`.**
Evidence: the second census pattern matches
`tests/regression/aggregation_check.rs:153` (`fleet.simulate(1)` in
`run_fleet_integration`) at `21d90588` and HEAD; the ledger lists the file
only for its synthetic `aggregation::aggregate` half (`:271`).
Correction: add the site and say the rewritten file replaces
`run_fleet_integration` as well. Also write the Verification line as
`cargo nextest run -p hares-core --test regression` (the target lives in
`crates/hares-core/Cargo.toml:49-51`); the same applies to RT-5.

**14. Minor. FL-1 is a `perf:` row but names no wall-time measurement, and the chunk barrier can regress throughput.**
Evidence: `plan.md` §3 rule 3 requires every `perf:` commit to record
before and after with M-3's method (`frame_golden run`, five fresh
constructions, median, 5% band). FL-1 records only RSS and table bytes.
Folding after a whole chunk finishes idles threads on each chunk's slowest
home.
Correction: add `fleet_same_offset_300s` and `fleet_mixed_offset` wall
time by M-3's method at the parent and after, and state the chunk-tail
effect is measured at `chunk_size` 64 with 8 threads; if it exceeds the
band, use an ordered reorder window of `chunk_size` results (fold home i as
soon as homes 0..i are done), which keeps the fold order and the bound.

**15. Minor. FL-1's memory bound statement omits the parts that grow with fleet size.**
Evidence: the `homes` table (one row per home), the entry list, the
duplicate-destination path set and per-home metrics grow with the number
of homes; the decision says memory is "bounded independent of fleet size".
Correction: state that the frame memory is bounded by `chunk_size` and
list the O(homes) bookkeeping with its bytes per home, recorded by FL-1's
10 000-home measurement.

**16. Minor. `HomeStatus::Skipped` and `Coverage.weight_skipped` are unreachable in `FleetResult`.**
Evidence: `SimError::Skipped` is produced only by `SteppableFleet::step`
(`fleet.rs:745`); `Fleet::simulate` never skips a home, and
`SteppableFleet` returns no `FleetResult`, `Coverage` or `homes` table.
Correction: either define where `Fleet::simulate` produces `Skipped`, or
drop the variant and field from §2.4 and FL-2 (dead code, `dl-i08-criteria`).

**17. Minor. Fleet error variants used by FL-1 and FL-3 are missing from `plan.md` §2.4.**
Evidence: FL-3 adds `FleetError::NoSuchHome { index, len }` and
`FleetError::Telemetry { bldg_id, source }`; FL-1's "a home whose config
already has a report named `fleet` is a construction error" names no
variant. §1: every type is defined once, in §2.
Correction: add the three variants (name the third, for example
`ReservedReportName { bldg_id }`) to §2.4's `FleetError`.

**18. Minor. FL-2's bitwise claim for `fleet_same_offset_300s` rests on an unchecked premise.**
Evidence: FL-2 says every product other than `fleet_mixed_offset`'s
compares bitwise, which holds only if every `fleet_same_offset_300s` home
is `Ok` under the new mapping (no warning, no non-zero health counter, a
converged or disabled warm-up). Fleet goldens record no health
(`fleet_mixed_offset.golden.json`: `"health": null`).
Correction: state the premise and check it at FL-1's capture: the new
`homes` product shows every home `ok` and each home's `RunResult.health`
has no event; if one does not, list its aggregate change among FL-2's
expected golden changes.

**19. Minor. OD-14's "a pint release that defines the name again makes the definition fail loudly" is false.**
Evidence: pint's registry defaults to `on_redefinition="warn"`
(`pint/facets/plain/registry.py`, `__init__`), and OCHRE builds its
registry with `UnitRegistry()` (`vendors/OCHRE/ochre/utils/units.py:5`),
so a redefinition only logs a warning.
Correction: `register_removed_units` raises when `"inch_H2O_39F" in ureg`
before defining it, and `test_inch_h2o_39f_matches_the_pint_0_24_definition`
adds a case: a registry that already defines the name makes the helper
raise.

**20. Minor. OD-10's `missing_infiltration_height_follows_openstudio_hpxml` compares the port with itself.**
Evidence: the expected value is "the ported estimate computed in the test
from the fixture's inputs", so a mistranslated rule passes.
Correction: pin the value OpenStudio-HPXML itself produces for `base.xml`
(its defaulted output, `in.xml` after `HPXMLDefaults`, or the expectation
in its `test_defaults.rb` for the same inputs), cited with the
OpenStudio-HPXML version.

**21. Minor. OD-19 leaves the constant setpoint element's parse failure silent.**
Evidence: `parse_setpoint_from_control` (`xml_helpers.rs:237-245`) returns
`None` when `SetpointTemp{Heating,Cooling}Season` is present but does not
parse, so the side falls through to no profile. OD-19's change covers only
the extension profiles and setbacks.
Correction: a present constant element that does not parse fails naming
the element and the text; add `setpoint_constant_with_a_bad_value_is_an_error`.

**22. Minor. OD-20's problem statement contradicts B-14.**
Evidence: OD-20 says that after B-14's equipment config commit
"without this entry the step would stop firing for every PV with no error";
`09-numeric-fixes.md` B-14 (lines 2025-2030) moves the same read to
`capacity_w` and keeps coverage unchanged.
Correction: drop the sentence, or say B-14 keeps the key read alive until
this entry replaces it.

**23. Minor. RT-1's allocation test brackets internals of a normal step without saying how.**
Evidence: `dwelling_snapshot_allocation_free_after_first_step` "steps the
dwelling normally and brackets the extracted snapshot function and each
domain-slot write ... with `thread_allocations()` reads"; a unit test
cannot read inside `step` without instrumentation.
Correction: name the mechanism (a `#[cfg(test)]` per-site allocation
accumulator in the dwelling, filled around those calls and read after each
step), so the test is not a re-implementation of the step.

**24. Minor. RT-1's mutator line ranges disagree within RT-4.**
Evidence: RT-4 cites the blueprint mutators as `blueprint.rs:182-222` in
its Problem and `:174-209` in its Change, both "at `ebdb1a17`"; the
functions span `:174-222` (`equipment_names` `:174` to
`add_equipment_spec` ending before `build` at `:225`).
Correction: cite `:174-222` in both places.

## Items verified correct

Fleet (`04-fleet.md`):
- Every fact in the preamble at `ebdb1a17`: `simulate(n_threads)` and the
  local pool (`fleet.rs:313-324`), ordered `par_iter().collect()`
  (`:345-409`), `DwellingOutcome` holding a full `SimulationResults`
  (`:43-48`), `retain_batches: true` for ResStock (`:797`), `SimStatus`
  (`:33-40`), both unit tables and their fallbacks (`aggregation.rs:43-50,
  63-72, 119-126, 141-150`), the accumulator and its null rule
  (`:208-243`), the per-bucket `has_null` fold (`:644-693`), the bucket
  intersection (`:588-597`), the epoch key and `Z` labels (`:507-508,
  636-637`), 60 s ResStock steps (`:787`), the `FifteenMin` Python default
  (`py_fleet.rs:180-182`). The unit census matches HEAD exactly.
- FL-1's bitwise argument holds: today's time stage adds row values in
  order into one `f64` and divides once; the cross-home fold adds `v * w`
  and `w` in entry order; with no null step value before B-6 and every home
  holding every bucket of the intersection, the new null rule and the
  covered-weight null change no number; `0.0 + a*1` equals `a` bitwise, so
  `fleet_minutes_15_reduces_one_minute_steps`'s `a·1 + b·3` is exact.
- Fold order is entry order for every chunk size and thread count; the
  determinism test varies both and compares with `to_bits()`;
  `peak_live_home_results` of exactly 1 and 2 for three homes is right.
- The two-stage table, null rule, Mode deferral to B-11, `Minutes(n)`
  sub-hourly frequency, and the `Degraded` exclusion from `weight_ok` and
  `n_succeeded` match `dl-i08-architecture.aggregation` and `.fleet`.
- FL-4's `instant_alignment_buckets_on_reference_boundaries` arithmetic:
  starts at 22:00 and 23:00 on 2017-12-31 and 01:00 on 2018-01-01 in
  -07:00; seven shared rows; covered weight 2.0, 3.0 (five days), 1.0.
  `to_rfc3339_opts(Secs, true)` renders `Z` only at +00:00, so
  `instant_utc_equals_todays_output` reproduces today's labels.
- FL-3's evidence at `ebdb1a17` and `21d90588`: the per-home "already
  reached configured end" errors (`fleet.rs:567-578`), the pinned test
  (`:1608-1614` at `ebdb1a17`), the silent accessors (`:612-636`,
  `:638-659` at `21d90588`), `unwrap_or(-1)` (`py_fleet.rs:446`), the
  binding's own bound check (`:540-547`), the index error for a failed
  snapshot (`:551-562`).
- FL-5 is correctly a numeric row: the verbosity-2 columns of the fleet
  goldens are `kW`, `therms/hour`, `kVAR` and `C` columns whose rule does
  not change, so an empty golden delta is the expected result; its unit
  tests fail at the parent by construction.
- All fixtures FL-1 to FL-5 name exist at HEAD (`bldg0176227`,
  `G0600650/0770/0790_2018.csv`, `base.xml`, `base-battery.xml`,
  `BEopt_example_schedule.csv`, `bldg0112631_schedule.csv`, the Denver EPW).
- Order: FL-1 (row 50) after OR-1, OR-3, OR-13, OR-14, OR-12, OR-6, BR-15,
  OD-8 and OD-13; FL-2 (51), FL-3 (52), FL-4 (53) after OR-7; FL-5 (87a)
  after B-6 and before B-11; B-12 (93) after FL-4.

Runtime and memory (`06-runtime-memory.md`):
- RT-1's premises: derived `Clone` gives `clone_from = *self = source.clone()`;
  the snapshot's `HashSet` rebuild and `clone_from` (`mod.rs:6389-6436` at
  `ebdb1a17`); `DomainUpdate` derived `Clone`; `upsert_domain_ref`'s false
  doc; the per-step drain and push-back (`environment.rs:705-736`); 174
  `custom_domains` sites; `CoreOutput` holds no heap data, so its per-step
  `clone` and `insert` on an existing key do not allocate. `EnvironmentState`
  is not in `DwellingCheckpoint`, so `DomainSlots` changes no checkpoint
  format.
- RT-2's dependency analysis of `update_in_place`: weather index from step
  plus offset, `compute_annual_offset` reading the start's wall clock and
  the weather's midpoint offset, solar position from the instant, the civil
  day of year under `dst`, mains from weather-derived inputs; warm-up
  replays rows `0..steps_per_day`, so the horizon rule is right.
- RT-3's listed sites and the nalgebra 0.35.0 derived `clone_from`; the
  identity-LU solves SL-7 removes; `output_into` with `mul_to` keeps the
  operation order.
- RT-4's reuse blockers (`build(self)`, the per-build resample, the
  `from_parts` writes into `weather.meta`/`building.site`, the duplicate
  parse in `Dwelling::from_config`).
- RT-5's dispositions for every listed static, and the fixtures and helpers
  its tests name (`grid_outage_islanding.rs:86, 110`,
  `base-foundation-basement-garage.xml`, `base-hvac-multiple.xml`).
- RT-7's evidence (`observation_for_fields`, the
  `observation().unwrap_or_default()` substitute at `py_gym.rs:491`) and
  RT-8's (`reset_with_seed` after `load_state` in `gym_env.py:546-548` and
  `vec_env.py:180-182`).
- Every RT entry is bitwise against the goldens with allocation counts as
  assertions and timings only recorded; dependencies and rows (21, 22, 61,
  62, 65, 70, 71, 71a) are consistent with `00-schedule.md`.

Slop cleanup (`10-slop-cleanup.md`):
- SL-1, SL-2 (451 lines at `ebdb1a17`, reproduced), SL-8 (285, 114, 28, 3
  and 40 tracked files, reproduced), SL-4, SL-5 and SL-6 (the consumer's
  `fit.py:319-321`, `scale.py:630-633`, `check_losses.py` returning 0 at
  `c71c542`), SL-10 are genuine, and each fix is right.
- SL-7: both constructors set `m_is_identity: true` and nothing sets it
  false; the LU path is unreachable; removing the identity solves can change
  at most the sign of a zero, and the entry stops on any golden difference.
- SL-8's `git mv` then `git rm -r --cached` keeps files on disk and is an
  operator checkpoint; it lands before PY-7.

Other defects (`11-other-defects.md`):
- OD-2 sites checked at HEAD: no `deny_unknown_fields` on
  `SimulationConfig`; `time_res_secs_u32`'s 3600 fallback; the site offset
  fallback to UTC for out-of-range and NaN offsets (`blueprint.rs:90-91`);
  `wall_area_m2` parsed and unread (25 files).
- OD-3's wiring and fallback lines at `21d90588` (`equipment.rs:197, 247-249,
  277`; `boiler.rs:169-170, 549-550`; `resolve_hvac.rs:957, 1040`).
- OD-9's fixture: `bldg0000002`'s heating profile and the three assertions
  (70 F, 67 F, 70 F; the first and third fail at the parent); 2018-01-05 is
  a Friday.
- OD-10 matches the OpenStudio-HPXML documentation: SiteType choices and
  the suburban default; ShieldingofHome choices and the facility-type
  default; NumberofConditionedFloorsAboveGrade required;
  ResidentialFacilityType required; InfiltrationHeight estimated from other
  inputs; the HasFlueOrChimneyInConditionedSpace rule quoted verbatim
  (AFUE/Percent below 0.89, fireplace, water heater EF below 0.59). The
  stricter ConditionedBuildingVolume rule is a declared, recorded deviation.
  Every golden HPXML carries InfiltrationHeight, ShieldingofHome, SiteType
  and floors, and the parity files carry the legacy `HasFlueOrChimney`
  false that the structural commit migrates, so the structural commit is
  bitwise. `base.xml`'s water heater is electric, so the flue test turns on
  the furnace AFUE alone. The solver lines (`solver_builder.rs:1371-1445` at
  `21d90588`) show every default the entry names.
- OD-11 (three `strict=False` marks, undefined `VENDOR_OCHRE`), OD-12,
  OD-13, OD-17, OD-18 (`record_clamped_actions` has no caller; the two
  statics), OD-19 (the token filter and constant fallback; no setback
  element read) are real at HEAD.
- OD-14: pint's current registry defines `inch_H2O_4C` and no
  `inch_H2O_39F`; the value check is right.
- OD-15's triage is sound: 3.45 kWh over 72 h is 48 W against a declared
  50 W crankcase heater; the plan compares with the EnergyPlus
  `Coil:Cooling:DX` crankcase rules and metering and OpenStudio-HPXML's
  defaults, and the contingent OD-16 row is in the schedule.
- Ordering: OD-3 (23) before OD-2 (31); OD-6 (24), OD-17 (24f), OD-19 (24g)
  before RT-4 and OD-9; OD-11, OD-12, OD-13, OD-14 (24b to 24e) before
  PY-7; OD-8 (32) after OD-2; OD-18 (30b) before OR-12 and RT-5; the
  numeric commits OD-9 (94a), OD-2 site 6 (94e), OD-10 (94f), OD-20 (94i)
  are in the numeric block after their dependencies; commit messages match
  `00-schedule.md`; no em dash in the four files.
