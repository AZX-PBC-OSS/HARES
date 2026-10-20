# L2 resolution: ledger/04, ledger/06, ledger/10, ledger/11

Scope: every finding of `verify-L2-c.md`, and every finding of
`verify-L2-e.md` that concerns `ledger/04-fleet.md`,
`ledger/06-runtime-memory.md`, `ledger/10-slop-cleanup.md` or
`ledger/11-other-defects.md`. Code claims were re-read at `ebdb1a17` (the
ledger's evidence commit) and at HEAD `872cf289` on `feat/output-results`.
Binding: `decision-log.md` (including `dl-i08-execution.*`), the
`feedback_*.md` rules, and the other owners' resolutions
(`L2-resolution-a.md`, `-b.md`, `-d.md`), whose items for these files are
applied here.

Counts: verify-L2-c 24 findings: 23 fixed (finding 5 with one part
rejected), 1 not in these files and passed on (finding 15). verify-L2-e 18
findings in scope: 17 fixed in these files (other owners' parts passed on
below), 1 fixed by another owner and not duplicated here (finding 10,
`12-inputs.md` IN-9). Items from other owners' resolutions for these files:
11, all applied.

## verify-L2-c

| # | Resolution |
|---|---|
| 1 | fixed: `04` FL-1 "*The two-stage rule.* The fleet reduces each variable twice, each stage with its own rule" with the table ("`Power` \| `Mean` \| weighted sum"; "`Quantity` \| as `VariableDef.aggregation` declares ... \| as the variable declares (`VariableKind::Quantity { across_homes }`, `Sum` or `Mean`)"); bitwise parity: "At this commit both stages keep the rules of the parent commit, keyed on `Unit::symbol()` ... `legacy_fleet_rules(Unit) -> (TimeRule, AcrossHomes)`, which FL-5 deletes"; the unknown-unit columns listed in the facts ("`fraction`, `ratio`, `%`, `bool`, `code`, `index`, `count`, `min`, `V`, `A`, `Ah`, `kg/s`, `L/day`, `J/(kg*K)`, `bytes` and the empty unit") and the changing ones moved to a new numeric entry, `04` FL-5 "fleet variables fold by their declared kind". OR-1's `Power` classification and `across_homes` declaration, §2.6 and the decision log's aggregation entry are passed on below. |
| 2 | fixed (option b, streamed): FL-1 "The fleet attaches a Rust `BatchCallback` that feeds each batch into a period reducer the fleet owns" with the parent's time rule, so "`Enum` averages in both stages today" stays until B-11; FL-5 "`Enum` stays `Mean` until B-11"; B-11's switch of both stages passed to the `09` owner. |
| 3 | fixed: FL-4 "The calendar is chosen before reduction, not after: the fleet passes each home's reducer (FL-1) a period calendar ... For `Instant(offset)` it is the declared offset's calendar ... This commit gives `03-output-reporting.md` OR-13's reducer that period-origin offset parameter"; "a home period row whose `Step Count` is below the full period's step count is null for that home"; the test rewritten with computed weights ("2.0 on the first day ... 3.0 on the five interior days, and 1.0 on the last"); "`instant_utc_equals_todays_output`: `Instant(+00:00)` at `Hourly`". |
| 4 | fixed: FL-1 "`Frequency::Minutes(n)` (`03-output-reporting.md` OR-13) carries sub-hourly fleet output: today's `FifteenMin` is `Frequency::Minutes(15)`", test `fleet_minutes_15_reduces_one_minute_steps`; the Python block (`hares.FleetReport(selection, frequency, health_policy=HealthPolicy())`, `Fleet.simulate(report, *, n_threads=None, chunk_size=64, ...)`, `FleetResult` attributes, "Removed: the `resolution=` keyword, `AggregationResolution` and `PyAggregationResolution` with its `from_str`, and `FleetResults` ..."); the three pinned sites and the rest of the census under "*Callers.*" (`test_py_enums.py:242-251`, `test_py_fleet.py:282-305`, `aggregation_check.rs` "`:271`"). `Frequency::Minutes` itself is passed to the OR-13 and §2.2 owner below. |
| 5 | fixed, with one part rejected: FL-1 "A home's `fleet` report selects the fleet selection restricted to that home's catalogue ... This is the one fleet exception to `plan.md` §2.2's rule"; "a `Pattern` that matches nothing in any home, is `FleetError::UnmatchedSelection { names }`, raised when the last home has been built and before its chunk runs"; tests `selection_missing_from_some_homes_does_not_fail_them`, `selection_matching_no_home_is_an_error`. Rejected: raising it at fleet construction. Reason, as written in FL-1: "A catalogue exists only once a home's inputs are parsed, so the check cannot run at fleet construction without parsing every home twice or holding every blueprint, which the memory bound forbids." |
| 6 | fixed: `10` SL-8 evidence lists every site (code comments at `catalog.rs:119, 323, 1168, 1229, 1242, 1255, 1266`, `coil_physics.rs:289, 1361`, `heat_pump_wh.rs:562`, `metrics.rs:2627`, `mod.rs:12323`, `ev_driver/mod.rs:7336`; the two docs; the script headers; `.gitignore:70`); the change "Each code comment above keeps the primary source the review file recorded and drops the path and any "Finding N"" with the source per comment; "`.gitignore:70` (`docs/reviews/_logs/`) is deleted"; the headers "state what each check enforces ... with no ticket ID"; the acceptance grep now covers the moved scripts too. `catalog.rs:323` was found in addition. |
| 7 | fixed: `06` RT-2 "The key holds plain values, never a `DateTime<FixedOffset>` (whose `Eq` and `Hash` compare the UTC instant alone): ... the naive local start, the resolved site UTC offset in seconds"; test `forcing_key_differs_by_utc_offset_at_equal_instant` (start `2023-01-01T00:00:00-07:00` against `2023-01-01T02:00:00-05:00`). B-1a/B-1c's key fields passed on. |
| 8 | fixed: RT-7 "`VecDwellingGymEnv.step` returns a copy of that buffer by default ... `VecDwellingGymEnv(..., copy=False)` returns the buffer itself, documented as overwritten by the next `step()` or `reset()`"; tests `batch_step_returns_a_fresh_observation_by_default`, `batch_step_copy_false_returns_the_shared_buffer`. |
| 9 | fixed: FL-1 null rule "Today one null home nulls the whole cell (`has_null`, `aggregation.rs:644, 663, 673, 693`) and one null step nulls the home's period (`Accumulator::finish`, `:235-238`)". |
| 10 | fixed: FL-1 "*Status.* Homes enter the fold as today: `Ok` and `Flagged` fold, `Failed` does not ... FL-2 narrows the fold to `Ok`, replaces `flagged` with `degraded`, and adds `skipped`"; homes table `status` "(`ok`, `flagged`, `failed`: today's `SimStatus`)". |
| 11 | fixed: FL-1 test "`base-battery.xml` (whose battery declares `{"Battery": {"bms_mode": "\"Manual\""}}`, as `11-other-defects.md` OD-8 requires)"; FL-2 fixture header the same, and FL-2 depends on OD-8. |
| 12 | fixed (own reports run): FL-1 "Each home runs the reports of its own `SimulationConfig` (a home can still write its own files) plus one report the fleet appends, named `fleet`"; the duplicate-destination rule kept with test "two homes whose configs each carry a `Parquet` report to one path"; facts "`fleet.rs:1147-1150` builds such a home". |
| 13 | fixed: FL-1 "*Columns.*": `aggregate` "`Time`, then one `Float64` column per folded variable ... The per-home `Step Count` and `[valid steps]` companions of §2.6 are not fleet columns"; `homes` "`steps` (steps the home ran) and `periods_folded`". |
| 14 | fixed: FL-1 "*Accumulation order.* Over time, step values are added in step order into one `f64` per period and variable; `Sum` returns it and `Mean` divides it once ... OR-13's reducer performs the same operations in the same order." |
| 15 | passed on (not in these files): the `aggregation.rs:509` citation is in `plan.md` §2.5, `03` OR-7 and `09` B-12. The parse is at `:505` (`parse_timestamp`, defined at `:444`) and the bucket key at `:508`. These files cite `:508` and `:636-637` only. |
| 16 | fixed by the redesign of finding e-3: RT-1 has no spare list; the slots are the state readers see ("`update_in_place` clears it for the solver domains where it drains today, and writes the schedule and mains payloads into their slots' retained vectors (the swap buffers go)"). |
| 17 | fixed: RT-1 "the crate's test binary installs the allocator under `#[cfg(test)]` ... brackets the extracted snapshot function and each domain-slot write ... Allocations elsewhere in the step (the recorder, until `03-output-reporting.md` OR-2 and OR-3) are outside the brackets"; path "`crates/hares-types/src/environment.rs:513-523`". |
| 18 | fixed: SL-7 evidence "the energy-balance branch on `m_is_identity()` (`stepping.rs:1056`) and its non-identity arm that calls `build_coupled_lu` (`:1149`)", "`CouplingData` (`:38`, re-exported at `crates/hares-envelope/src/lib.rs:34`)", "`bench_coupled_step_identity` asserts `m_is_identity()` (`:264`)", oracle tests "`state_space.rs:2017-2420`, the calls at `:2039, 2196, 2256, 2338, 2420`"; grep extended with `CouplingData`. |
| 19 | fixed: SL-8 "40 under `scripts/reviews`"; SL-2's census command gains `scripts docs` with the moved paths excluded: "returns 451 lines in 122 files: 431 lines in 112 files under the code trees and 20 lines in 10 files under `scripts` and `docs`". |
| 20 | fixed: OD-2 site 4 names every `None` ("`try_build_central_ac_config` when SEER is missing ... `Ok(None)` when the heating capacity is missing (`:851, 994, 1071`) ... `.ok()?`, `:1139, 1233, 1291, 1322` ... a mini-split cooler's builder error (`.ok().flatten()`, `:2233-2235`)"), the error for each, and the one legitimate outcome: "The one legitimate "no typed config yet" outcome is a capacity that autosizing will supply"; tests `hvac_builder_missing_input_names_the_element`, `missing_capacity_is_pending_only_when_autosizing`. |
| 21 | fixed: OD-8 "(`:131, 152, 163, 182`)" and "A fifth guard, `if len(summaries) > 0` in `TestBillingSummary` (`:277`), never enters ... crosses no billing-period boundary"; the change gives it its own run "starting `2019-06-28T00:00:00` for 7 d so it crosses the July 1 boundary" with the checks unconditional. |
| 22 | fixed: RT-7 "After `05-python-surface.md` PY-7 the Python files are `python/hares/rl/vec_env.py` and `python/hares/rl/gym_env.py`"; "A step whose observation fails ends that environment with its error: `terminated=True` ... No substitute vector is produced"; test `observation_failure_terminates_the_environment`. |
| 23 | fixed: RT-5 `warn_once_is_per_run` names `crates/hares-core/tests/grid_outage_islanding.rs`'s builders and "`add_battery_with_discharge_limit(&mut dwelling, 0.6, 0.5)` ... islanded with `set_grid_voltage(0.0)`"; "Effect on fleets: ... The fleet goldens must compare bitwise; if a golden home now carries a warning, the row stops"; RT-5 depends on FL-2. |
| 24 | fixed: OD-2 site 3 "the unconvertible-value check runs where the override payload is merged (`:906-944`) and names only override-supplied keys ... the entry's first step is a census ... listing each raw parameter the conversion drops; each resolver-emitted key it finds is either no longer emitted for raw specs or given a conversion". |

## verify-L2-e (findings concerning these files)

| # | Resolution |
|---|---|
| 3 | fixed: RT-1 change 3 "Domain updates live in fixed slots" (`DomainSlots` with `thermal`, `humidity`, `electrical`, `fluid`, `schedule`, `mains_water` fields and `custom: Vec<DomainSlot>`), "A reader of a built-in domain reads its field (`env.domains.thermal.get()`), with no search. A custom solver's slot is resolved once, when the solver is installed at assembly"; tests `custom_domain_slot_resolved_at_installation` and "`git grep -nE 'custom_domains\|upsert_domain' -- crates tests benches` prints nothing". |
| 4 | fixed (RT-4 part; §2.7 already done by the `b` owner): RT-4 "`ParsedInputs` (the four handles), `DwellingSettings` ... and `DwellingBlueprintBuilder::new(inputs, settings)`, which parses nothing, exactly as `plan.md` §2.7 defines them", the Python handle classes, tests `blueprints_from_different_hpxml_share_weather` and `test_parsed_weather_shared_across_dwellings` ("the copy is deleted, and two dwellings ... built from the handle run"); RT-2 `blueprints_from_different_hpxml_share_forcing`. |
| 5 | fixed: RT-2 "Per-surface reuse is decided by the profile ... on `consumer_shape_900s` and on `02-measurement.md` M-3 item 5's stepped-episode loop (one blueprint built and run repeatedly ...). If either share is at or above 5% of `simulate()` (M-3's noise band ...), this commit adds a per-blueprint per-surface table"; test `per_surface_table_shared_across_builds_of_one_blueprint`. |
| 6 | fixed (OD-2 site 6 part): OD-2 "site 6 is this row's second commit, after the strictness sites, with its delta recorded"; commits listed under "**Commits:**". B-4, B-9 and IN-5 belong to other owners and are fixed there. |
| 10 | fixed by the `12` owner as `12-inputs.md` IN-9 (`L2-resolution-b.md`, new row 94c); not duplicated here. OD-2 site 7 now says "`12-inputs.md` IN-9 ... applies the same check to its converted value". The finding's "store power in W" is passed to the `12` owner below (IN-9 converts to the existing kW field). |
| 11 | fixed: new `06` RT-8 "RL reset restores the warmed-up state and reseeds without rebuilding": "`Dwelling::reseed(&mut self, master_seed: u64) -> Result<()>` re-derives every random stream ... The gym and vector environments reset by `load_state(snapshot)` and then `reseed(seed)`"; tests `reseed_resets_every_stream`, `gym_reset_restores_snapshot_and_reseeds`, `gym_reset_builds_nothing_and_runs_no_warmup`; reset time against construction time measured. |
| 14 | fixed for these files: requirement IDs are written with their source ("requirement RL-3", "requirement FL-9"); no other requirement ID appears (checked by grep). The §3 rule is the `plan.md` owner's (done in `L2-resolution-a.md`). |
| 17 | fixed: RT-3 "The zero-allocation test also carries the factorization count ... every factorization in `hares-envelope` ... factors a dynamically sized nalgebra matrix and allocates its result, so a step with zero allocations performs zero factorizations"; the test name says "and therefore zero factorizations". |
| 18 | fixed (`11` part): preamble "V21 (a malformed EV `charging_strategy` override) already fails construction and is pinned by `init_errors_on_malformed_charging_strategy_override` (`crates/hares-equipment/src/ev/tests.rs:8062`)". The `review.md` row is passed on. |
| 23 | fixed (RT-3 part): RT-3 names the sites ("The allocations at HEAD, after SL-7 removes the coupled LU path" plus the `u_buf` note) and "If the zero-allocation test reports another, the entry stops, the site is added to this list with its location, and it is removed the same way ... before the entry commits". |
| 26 | fixed (SL parts): SL-1, SL-4, SL-5, SL-6 each gain a **Check:** ("RT-1's reviewer reads the rewritten comment against `telemetry_clone_from_same_keys_allocates_nothing`"; "CN-6's reviewer confirms ... and CN-6's B0 gate shows the sampling unchanged"; "CN-5's test of the shrink"; "CN-9's test that a moved loss gives a non-zero exit code"). CN-12 is the `08` owner's. |
| 29 | fixed (RT-4 part): RT-4 measurement "M-3 item 5's three memory-growth readings ... repeated after this entry and recorded next to M-3's baseline". |
| 30 | fixed (SL-8 part): SL-8 "The `git rm --cached` steps change the index of a shared working tree, so they run at an operator checkpoint (`plan.md` §3)"; the §3 list was updated by the `a` owner. |
| 36 | fixed: FL-3 "requirement FL-9 (stepping a fleet without full telemetry) holds by construction and needs no change here"; RT-5 "`solver_builder.rs` groups interior surfaces in a `HashMap<ZoneId, _>` ... (`solver_builder.rs:1317, 1345`). Each zone's entry configures only that zone's own surfaces and inputs, so the order does not reach the numbers". |
| 38 | fixed (FL-1, RT-5 parts): FL-1 "with the bench `benches/fleet.rs`, group `fleet_fold` (replacing `benches/aggregation.rs`'s `aggregation` group)"; RT-5 "with the bench `benches/single_building.rs`, new group `concurrent_dwellings` ... `BenchmarkId` per thread count 1, 8 and 26". |
| 40 | fixed: RT-4 measurement "against a full parse of every input, both measured in the same run on `consumer_shape_900s`'s inputs"; the `context.md` figure is gone. |
| 42 | fixed: SL-8 "`.gitignore:65` (`docs/eplus/`) becomes `docs/eplus/*` with `!docs/eplus/section-mapping.md`, so the tracked citation guide is no longer inside an ignored path"; acceptance "`git ls-files -ci --exclude-standard` prints nothing". |
| 43 | fixed (FL-1 part): FL-1 "The memory bound is the chunk size times one home's reduced table. At `Timestep` frequency that table is the home's selected-variable frame"; measurement "the bytes of one home's reduced table (`RecordBatch::get_array_memory_size`) at `Timestep` and `Hourly`". §2.4 already states it (`L2-resolution-b.md` 43). |

## Items from other owners' resolutions, applied

- `L2-resolution-a.md`: FL-2 is the entry that makes `fleet_mixed_offset`'s
  aggregate non-empty: FL-2 verification "those products are re-captured in
  this commit and the change is recorded"; FL-1 migrates BR-15's
  `resolution` ("BR-15's `resolution` key of both fleet manifests becomes the
  `[fleet]` table") and uses BR-15's `weight`; FL-4's acceptance restricts
  covered-weight assertions to "every variable all three homes declare".
- `L2-resolution-b.md`: RT-4 builds `ParsedInputs`, `DwellingSettings`,
  `DwellingBlueprintBuilder::new` and the Python handles, `base:
  Option<PathBuf>` until B-10; RT-2 `blueprints_from_different_hpxml_share_forcing`;
  RT-5's census row "each rejection a `Warning`"; RT-6
  "`ResolvedParameter`, `crates/hares-types/src/resolved.rs`" and
  "`SetpointSource::GeneratedConstant` ... until `11-other-defects.md` OD-9
  deletes it"; RT-3's factorization statement; the RL-3 entry (RT-8, after
  RT-4 and OR-10); OD-9 deletes `GeneratedConstant` and "Lands before
  `12-inputs.md` IN-8 (row 94b)"; OD-1 "`HaresCallbackError` (added by
  `05-python-surface.md` PY-10)"; OD-2 site 7 and IN-9; FL-1 bytes and
  bench; FL-3's FL-9 sentence. The gym's dropped `write_output` is fixed in
  OD-7 itself ("`_sim_config_to_kwargs` forwards every field of the typed
  `SimulationConfig`"), the function OD-7 already edits, and PY-8 later
  replaces it.
- `L2-resolution-d.md`: B-12 uses FL-4's mixed-offset inputs
  (`fleet_mixed_offset`, named in FL-4's acceptance header); RT-4 provides
  `hares.ParsedWeather` for CN-10.

## Schedule changes (`ledger/00-schedule.md`, for its owner)

- Row 31 (OD-2): two commits, `fix: reject unknown keys and inapplicable
  overrides instead of dropping them`, then
  `fix: a constant setpoint override replaces the schedule and HPXML setpoint sources`.
- Row 50 (FL-1): two commits,
  `test: capture a golden same-offset fleet at 15-minute aggregation`, then
  `perf: fold fleet homes into a bounded, deterministic streaming aggregate`.
  FL-1 depends on BR-15, OR-12 and OR-14, all earlier.
- Row 51 (FL-2): consumer check "golden, `fleet_mixed_offset` re-captured";
  FL-2 depends on OD-8 (row 32).
- New row 71a after RT-7: RT-8, file 06,
  `perf: reset RL environments from the warmed-up state and reseed them without rebuilding`,
  consumer check "golden".
- New row 87a, between B-6 (row 87) and B-11 (row 88): FL-5, file 04,
  `fix: fold fleet variables by their declared kind, not by unit string`,
  consumer check "delta". The preamble rule "Each numeric fix
  (`09-numeric-fixes.md`, and `11-other-defects.md` OD-9)" gains
  `04-fleet.md` FL-5.
- RT-5 (row 70) now depends on FL-2 (row 51); RT-2 (row 62) on M-3; both
  orders already hold.

## Items for other owners

- `plan.md` §2.2 and `03-output-reporting.md` OR-13: add
  `Frequency::Minutes(NonZeroU32)` (the minute count divides 60 and the
  period is a whole multiple of `time_res`; periods start on the local
  clock hour), its `validate()` rule and its Python constructor; the §2.2
  sentence "HARES has no sub-timestep reporting interval" stays true. OR-13's
  reducer is constructed by `hares-fleet` with a calendar and a per-variable
  rule and is fed by record batches as well as rows (FL-1), with OR-13's
  summation order (step order, divide once) stated; FL-4 adds the
  period-origin offset in its own commit.
- `plan.md` §2.2 and `03` OR-1: `VariableKind::Quantity { across_homes:
  AcrossHomes }` with `enum AcrossHomes { Sum, Mean }` declared by every
  `Quantity` (irradiance, durations, efficiencies, voltage and specific heat
  `Mean`; flows, counts and volumes `Sum`); variables in `kW`, `W`, `kVAR`
  and `therms/hour` declared `Power`. OR-1's canonical `unit` symbol merges
  `enum`, `code` and `index` (and `fraction`, `ratio`), and the fleet path
  still reads that string until FL-1: if the merged symbol changes what
  `aggregation.rs:119-126` infers for any column (for `code` and `index`
  today's weighted sum becomes a weighted mean if the symbol is `enum`),
  OR-1 moves fleet numbers that no golden pins (`fleet_mixed_offset`'s
  aggregate has zero rows), so OR-1 must keep the fleet's inference
  bitwise or record it.
- `plan.md` §2.4: `FleetReport` lists the `health_policy` its prose names;
  `Fleet::simulate(report, n_threads, chunk_size)`;
  `FleetError::UnmatchedSelection { names }`; the selection exception of
  FL-1; `FleetResult.homes` columns (`steps`, `periods_folded`);
  `Coverage` gains `weight_flagged` at FL-1, renamed `weight_degraded` by
  FL-2.
- `plan.md` §2.6: the two-stage table of FL-1 (over time per home; across
  homes: `Energy` and `Power` weighted sum, `Temperature`, `Ratio` and
  `State` weighted mean, `Enum` Mode, `Quantity` as declared), and the
  `Instant` calendar rule of FL-4.
- `plan.md` §2.7: `ForcingKey`'s comment reads "time grid, naive local
  start, site UTC offset in seconds, horizon in steps, site lat/lon/elevation
  bits, civil timezone" (RT-2), not "start instant".
- `plan.md` §2.5, `03` OR-7, `09` B-12: `aggregation.rs:509` becomes `:505`
  (parse; `parse_timestamp` at `:444`) and `:508` (bucket key)
  (verify-L2-c 15).
- `05-python-surface.md` PY-1: export all three `FleetResult` tables
  (`aggregate`, `covered_weight`, `homes`), not only the aggregate.
- `09-numeric-fixes.md` B-11: it switches `Enum` to `Mode` in both fleet
  stages (the fleet's per-home time rule and the cross-home fold), depends
  on `04` FL-5, and its evidence (the unit tables) is deleted by FL-1 and
  FL-5. B-1a and B-1c add `FirstHour` and `WeatherYear` to RT-2's
  `ForcingKey` in their commits.
- `02-measurement.md` M-1 and M-2: the fleet manifest's `[fleet]` table
  (a `FleetReport`, with an optional `alignment` from FL-4) replaces BR-15's
  `resolution`; the fleet products become `aggregate`, `covered_weight`,
  `homes` and `metrics`; M-2's table gains `fleet_same_offset_300s` (added
  by FL-1's first commit).
- `12-inputs.md` IN-9: verify-L2-e 10 asks that charger power be stored in
  W (`feedback_si_units`; `plan.md` §2.7 already names
  `max_charging_power_w` for RT-6's read-back); IN-9 converts to the
  existing kW field. Either IN-9 stores W or RT-6's read-back converts kW to
  W; this pass leaves the choice to the `12` owner.
- `review.md` owner: the V21 row (verify-L2-e 18).
- Operator: `decision-log.md` `dl-i08-architecture.aggregation` should
  state the cross-home rule for energy and power (summed) and for
  temperature, ratios and binary states (weighted mean), as well as Mode
  (verify-L2-c 1).
- Found while verifying, not added (no finding asks for it, and the scope
  rule of this pass excludes new entries): `solver_builder.rs` substitutes
  `building.conditioned_volume_m3.unwrap_or(400.0)` in the infiltration
  set-up (`crates/hares-core/src/dwelling/solver_builder.rs:1371, 1440` at
  `872cf289`), a silent default under `feedback_no_silent_defaults`, while
  `resolve_hvac.rs:247` treats the same missing value as an error. Under
  `dl-i08-process.found_defects` it is a fix entry for whoever owns that
  decision.
