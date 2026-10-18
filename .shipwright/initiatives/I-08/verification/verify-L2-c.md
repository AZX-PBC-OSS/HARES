# Verification L2: 04-fleet, 06-runtime-memory, 10-slop-cleanup, 11-other-defects

HEAD `ebdb1a17` (HARES), `c71c542` (consumer). Read-only. Checked against
`plan.md` §1 to §3, `decision-log.md`, `ledger/00-schedule.md`, and the
L1 findings in `verification/verify-L1-c.md` that
`verification/L1-resolution-c.md` marks fixed.

Counts: 1 blocker, 7 major, 16 minor.

## Findings

### 1. BLOCKER: FL-1 uses the per-period aggregation kind for the cross-home fold, so its own bitwise test cannot pass

Entry: `04-fleet.md` FL-1, change bullet 3 and test `fleet_fold_matches_collect_then_aggregate`; `plan.md` §2.6.

Evidence:
- HEAD aggregates in two phases, each with its own rule:
  - Over time, per home: `ColumnAggregation`. `kWh` sums; `kW`, `W`, `C`, `-` and the rest average (`aggregation.rs:79-110`).
  - Across homes: `FleetAggregation` (`aggregation.rs:122-135`, `157-180`). `kWh`, `kW`, `therms/hour`, `kVAR` and `W` take a weighted sum Σ w·v. Only `C`, `-`, `W/m2`, `s`, `deg`, `kWh/mi` and `enum` take the weighted mean. An unknown unit falls back to the weighted sum (`:170-183`).
  - The fold is at `aggregation.rs:667-705`. A fleet `kW` cell today is the population total.
- FL-1 folds with "the kind taken from `VariableDef.aggregation`": `Sum` is Σ w·v, `Mean` is Σ w·v / Σ w.
- §2.6 and OR-1 must declare Power as `Mean`, because power averages over time. Declaring it `Sum` would add kW across steps. So under FL-1 every power column (`kW`, `W`, `kVAR`, `therms/hour`) becomes a weighted per-home average, while energy stays a population total. Fleet kWh and fleet kW then stop being consistent.
- The test requires equality with HEAD's `aggregate()` bitwise and says a mismatch "is fixed in the catalogue". The catalogue cannot fix this without breaking the per-period rule. The entry has no legal way to go green.

Correction: state a cross-home fold rule separate from `VariableDef.aggregation`, in §2.6 and FL-1:
- Weighted sum: `Energy`, `Power`, and any variable whose per-period aggregation is `Sum`.
- Weighted mean: `Temperature`, `Ratio`, `State`.
- `Mode` for `Enum` (`Mean` until B-11).
- `Quantity`: each `TelemetryField` declares extensive (sum) or intensive (mean), so irradiance `W/m2` stays a mean.

Also:
- OR-1 must classify `kVAR`, `therms/hour` and `W` as `Power`.
- FL-1 lists every HEAD column that took the unknown-unit weighted-sum fallback. Any column whose result changes becomes its own numeric entry, not part of FL-1.
- The decision log's aggregation entry states the cross-home rule for Energy and Power as well as Mode.

### 2. MAJOR: FL-1 changes enum columns inside a structural commit

Entry: FL-1 (with OR-13, B-11).

Evidence:
- FL-1 runs each home through an OR-13 period reduction ("a home's result is its reduced period table").
- OR-13 reduces `Enum` by `Mode` "from its first commit".
- HEAD reduces mode columns over time by mean (`ColumnAggregation`, `enum` unit to `Mean`), then averages across homes.
- So from FL-1 on, a fleet `Hourly` mode cell is the weighted mean of per-home hourly *modes*. HEAD gives the mean of per-step codes. Every `{name} Mode (-)` column (`columns.rs:539`) moves. FL-1's claim that `Mode` "folds as `Mean` (today's result)" until B-11 is false for any frequency coarser than the step, and `fleet_fold_matches_collect_then_aggregate` fails on those columns (`base.xml` has HVAC mode columns).

Correction: pick one and write it into FL-1 and B-11:
- (a) The fleet's per-home report reduces `Enum` by mean until B-11, which switches both the per-home reduction and the cross-home fold in one numeric commit.
- (b) Fleet homes run at `Timestep` and the fleet performs the time reduction itself, bug-compatible until B-11.

### 3. MAJOR: `Instant` alignment cannot be built on per-home local-calendar reductions

Entry: FL-4 (with FL-1, OR-13).

Evidence:
- FL-1 has each home reduce to periods before folding.
- OR-13 reduces on "calendar periods in local time" of that home.
- FL-4's `Instant(offset)` buckets every home "on period boundaries of the declared offset", and a period a home covers in part "is null for that home".
- A home at UTC-5 that has already reduced to local days cannot be re-bucketed onto UTC-7 days. The partial-coverage rule also contradicts §2.6, where a partial period is a row with a smaller `Step Count`, not null.
- The acceptance test `instant_alignment_buckets_on_reference_boundaries` uses `Daily` with `Instant(-07:00)` on `fleet_mixed_offset` (UTC-5, -6, -8; no home at -7). It is not implementable as written.
- `instant_utc_equals_todays_output` holds only for `Hourly` or finer with whole-hour offsets.

Correction:
- The fleet passes each home a reduction calendar: the declared reference offset for `Instant`, and the home's own local calendar for `LocalClock` or a single-offset fleet. Give OR-13's reducer a period-origin offset parameter, stated in §2.6 and OR-13.
- State that the fold treats a home period row whose `Step Count` is below the full period's step count as null for that home.
- Pin `instant_utc_equals_todays_output` to `Hourly`.

### 4. MAJOR: replacing `AggregationResolution` drops 15-minute fleet output and leaves the Python surface and its tests unspecified

Entry: FL-1 (with `plan.md` §2.2 `Frequency` and §2.4 `FleetReport`).

Evidence:
- `AggregationResolution::{Hourly, FifteenMin}` (`aggregation.rs:16`). Python's `Fleet.simulate` defaults to `FifteenMin` (`py_fleet.rs:180-182`).
- ResStock fleets run at `time_res: Duration::minutes(1)` (`fleet.rs:787`). Their default output today is a 15-minute aggregate of 1-minute runs.
- `Frequency` has no sub-hourly period (`Timestep, Hourly, Daily, Monthly, Annual, RunPeriod`), so that output is gone.
- No entry specifies:
  - the Python fleet API change (`resolution=` argument, `PyAggregationResolution`, `FleetResults.per_dwelling_metrics`/`failures`, the chunk size, the alignment argument, `covered_weight`, the homes table);
  - the tests pinned to the old API: `tests/python/test_py_enums.py:244-249`, `tests/python/test_py_fleet.py:300-304`, `tests/regression/aggregation_check.rs:271` (calls `aggregation::aggregate` with `DwellingOutcome`).

Correction:
- Add a sub-hourly frequency to §2.2 (for example `Frequency::Minutes(u32)`, dividing 60 and a whole multiple of the step), or record an operator decision dropping 15-minute fleet output together with the new Python default.
- Name in FL-1 (or a PY entry it depends on) the Python fleet signature, `FleetResult`'s Python shape, the removal of `PyAggregationResolution`, and the rewrite of the three test sites above.

### 5. MAJOR: a fleet selection naming a variable some homes lack fails those homes

Entry: FL-1, FL-2 (with OR-3, `plan.md` §2.2).

Evidence:
- FL-1 runs each home "with a `Memory` report of that selection".
- §2.2 and OR-3 make a `Named` name absent from the catalogue "an error listing them", and a `Pattern` matching nothing an error.
- FL-2 says a variable absent from a home's catalogue is not applicable and "the home stays `Ok`".
- With `Named(["Battery ..."])` or `Pattern("Battery*")`, `base.xml` would fail construction, not stay `Ok`. FL-2's test `battery_home_and_plain_home_aggregate_together` depends on this.

Correction:
- In FL-1, each home's report selection is the fleet selection restricted to that home's catalogue.
- A `Named` name absent from every home's catalogue, or a `Pattern` matching nothing in any home, is a fleet construction error listing it.
- State this as the one fleet exception to §2.2's rule.

### 6. MAJOR: SL-8's acceptance grep fails on references its change does not remove

Entry: `10-slop-cleanup.md` SL-8.

Evidence: after the move, `git grep -n -E 'docs/(reviews|tickets|findings|superpowers)|scripts/reviews'` still matches:
- `.gitignore:70` (`docs/reviews/_logs/`, now a dead rule);
- code comments that cite review files as the source of data values:
  - `crates/hares-equipment/src/ev/catalog.rs:119,1168,1229,1242,1255,1266`
  - `crates/hares-equipment/src/hvac/coil_physics.rs:289,1361`
  - `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:562`
  - `crates/hares-io/src/output/metrics.rs:2627`
  - `crates/hares-core/src/dwelling/mod.rs:12323`
  - `crates/hares-core/src/actors/ev_driver/mod.rs:7336`
- prose in user documentation (`docs/eplus/section-mapping.md:27-28`, `docs/references/ashrae-hof-2021-chapters.md:67`), which is not a link;
- header comments in the CI scripts SL-8 edits (`scripts/check-ashrae-chapters.sh:13`, `scripts/check-citations.sh:12`).

The Change covers only exclusion lists and links. SL-3's rule leaves untouched files alone, so none of these would be edited.

Correction:
- Add these sites to SL-8's Change.
- Rewrite each code comment to carry the primary source the review file cited (for example fueleconomy.gov and the manufacturer specification for the EV catalog values, and the ASHRAE or EnergyPlus section for the coil and HPWH notes), dropping the review path.
- Delete `.gitignore:70`.
- Rewrite the two script headers and the two docs passages.

### 7. MAJOR: RT-2's `ForcingKey` omits the local offset, so two blueprints can share the wrong table

Entry: `06-runtime-memory.md` RT-2 (with `plan.md` §2.7 `ForcingKey`).

Evidence: the cached quantities depend on the local wall clock as well as the instant.
- `compute_annual_offset` reads `start_time.ordinal0()`, `.hour()`, `.minute()` in the start's own offset (`environment.rs:823-846`).
- `day_of_year = now.ordinal()` (`:568`).
- `solar_position` converts to UTC (`hares-physics/src/solar.rs:97`).
- The key is "`time_res`, start instant, horizon, lat/lon/elevation bits, civil timezone". chrono's `DateTime<FixedOffset>` implements `Eq` and `Hash` on the UTC instant alone.
- Two blueprints over one `ParsedWeather`, with the same start instant but different resolved UTC offsets, would share one table and read the wrong weather rows and day of year. For example, one site with an overriding `site_location`, or HPXML time zones differing within a county. There is no error.

Correction:
- Key on the naive local start plus the resolved site UTC offset in seconds (and `RecordLabel` once B-1a replaces `midpoint_offset_secs`), and never on a `DateTime<FixedOffset>`.
- Add a test `forcing_key_differs_by_utc_offset_at_equal_instant`.

### 8. MAJOR: RT-7 returns one mutable array object every step

Entry: RT-7, change bullet 2 and test `batch_step_fills_the_preallocated_observation_array`.

Evidence:
- RT-7 fills one preallocated `(n_envs, n_fields)` array in place and returns the same object each step. The test asserts `obs_t is obs_{t+1}`.
- Any caller that keeps `obs` across a step (`prev_obs = obs`, rollout code that stores references) then sees its stored observation overwritten, and nothing reports it.
- Gymnasium's own vector environments copy observations by default for this reason (`SyncVectorEnv(copy=True)`).

Correction:
- Fill the internal buffer in place, with no per-row lists and no Python objects per field.
- Return a copy by default: one contiguous `n_envs × n_fields` allocation per step.
- Add an explicit opt-in `copy=False` that returns the shared buffer, documented as overwritten on the next step.
- The test asserts the default returns a new array equal to the pure-Python path, and that `copy=False` returns the same object.

### 9. MINOR: FL-1 cites the per-home accumulator for the cross-home null rule

`Accumulator::finish` (`aggregation.rs:235-238`) is the per-home time accumulator. The cross-home "one null home nulls the cell" behaviour is `has_null` at `aggregation.rs:663,672,693`. Cite both.

### 10. MINOR: FL-1 relies on FL-2's status type before FL-2 exists

FL-1 (row 50) says "Only homes with status `Ok` (FL-2) are folded", and its tests compare the homes table and `Coverage`. FL-2 (row 51) introduces `HomeStatus`.

Correction: state that FL-1 folds today's `SimStatus::{Ok, Flagged}` homes and records `SimStatus` in the homes table, and that FL-2 narrows the fold to `Ok` and adds `Degraded`/`Skipped`. Alternatively, move the status type into FL-1.

### 11. MINOR: fleet tests use a battery fixture that OD-8 makes a construction error

FL-1 and FL-2 fixtures include `base-battery.xml`. OD-8 (row 32) makes it a construction error without a declared control.

Correction: state in FL-1 and FL-2 that the battery home declares `{"Battery": {"bms_mode": "\"Manual\""}}` (or a control those tests need), as OD-8 does for its callers.

### 12. MINOR: FL-1's duplicate-destination rule has no source of file destinations

FL-1 has each home run only the fleet's `Memory` report, so no home has a file destination and `fleet_homes_with_same_file_destination_are_rejected` has nothing to reject.

Correction: state whether each home's own `SimulationConfig.reports` also run in a fleet. If they do, say so and keep the rule. If not, reject any file destination in a fleet home's config and drop the rule.

### 13. MINOR: per-home coverage is missing, and the aggregate's columns are unstated

The decision says "every home's coverage and status is reported". The homes table has bldg_id, weight, status and reason, with no per-home coverage (periods or steps covered, which matters for `Instant` edges). `fleet_report_selects_columns_and_frequency` says the aggregate holds only `Time` and the two variables, which leaves unstated whether §2.6's `Step Count` and `[valid steps]` companions appear.

Correction: add a per-home covered-period (or step) count to the homes table, and state the fleet aggregate's column set.

### 14. MINOR: FL-1's bitwise test depends on an unstated summation order in OR-13

`fleet_fold_matches_collect_then_aggregate` is bitwise, which holds only if OR-13's `Sum`/`Mean` accumulate in step order and divide once, as HEAD's `Accumulator` does (`aggregation.rs:222-243`).

Correction: state that summation rule in OR-13 or FL-1.

### 15. MINOR: `aggregation.rs:509` is not the parse site

The citation appears in `plan.md` §2.5, `03-output-reporting.md` OR-7 (line 656) and `09-numeric-fixes.md` B-12. Parsing is at `aggregation.rs:505` (`parse_timestamp`, defined at `:444`); the bucket key is at `:508`.

### 16. MINOR: RT-1 puts per-step scratch on a public, serialised state type

RT-1 holds the spare `DomainUpdate` list "on `EnvironmentState`". That struct is public, `Serialize`/`PartialEq` (`hares-types/src/environment.rs:449`), and built by struct literal at about 229 sites.

Correction: keep the spare list on `EnvironmentManager`, which does the drain, and pass it to `Dwelling`, or keep it on `Dwelling` next to the update buffers. Readers see the same entries either way.

### 17. MINOR: RT-1's allocation test boundary is unspecified; one path is ambiguous

- At row 22 the step still allocates elsewhere (the recorder before OR-2, OR-3), so "the end-of-step snapshot and the domain upserts perform zero allocations" needs a stated boundary. State that the test brackets the extracted snapshot function and each `upsert_domain_ref` call with `thread_allocations()` reads in an in-crate test with the allocator installed under `cfg(test)`.
- Write the path of `EnvironmentState::upsert_domain_ref` in full: `crates/hares-types/src/environment.rs:513-523`.

### 18. MINOR: SL-7 evidence is incomplete

- It misses the energy-balance arms `thermal_solver/stepping.rs:1056` (`else if self.model.m_is_identity()`) and the non-identity arm that calls `build_coupled_lu` at `:1149`.
- The oracle tests reach `state_space.rs:2420`, not only `2017, 2150-2400`.
- The public re-export `CouplingData` at `hares-envelope/src/lib.rs:34` goes too.
- `bench_coupled_step_identity` asserts `m_is_identity()` (`benches/rc_solver.rs:264`), which is not part of `bench_coupled_step_lu`.

### 19. MINOR: SL-8's manifest count is wrong; the SL-2 census scope misses two trees

- `scripts/reviews` holds 40 files at `ebdb1a17`, not 43.
- SL-2's census paths omit `scripts/` and the remaining user docs, about 20 matching lines. That includes `scripts/check-ashrae-chapters.sh`, `check-citations.sh` and `check-energyplus-sections.sh`, which SL-8 edits, so SL-3's rule does not reach them. Add `scripts docs` (minus the moved paths) to SL-2's census command.

### 20. MINOR: OD-2 site 4 does not say what each builder's `None` becomes

The HVAC builders return `None` or `Ok(None)` for missing inputs, not only on error:
- `try_build_central_ac_config` returns `None` "Skipping AC: ... SEER not found" (`resolve_hvac.rs:1150-1155`) and on missing capacity (`:1155`);
- `Ok(None)` at `:851`, `:994`, `:1071`;
- the ideal and dehumidifier builders return `Option`.

Returning `Result<EquipmentConfig, HpxmlError>` needs each of these cases stated (a typed error naming the field). The alternative is a legitimate "no typed config" outcome, which the plan should then name.

### 21. MINOR: OD-8's guard lines are off

The guards in `test_tariff_integration.py` are at `:131, 152, 163, 182`, not 183. There is also `:277` (`if len(summaries) > 0`); state whether it becomes unconditional.

### 22. MINOR: RT-7's paths and one silent fallback

- RT-7 runs at row 71, after PY-7 (row 26). Its paths are `python/hares/rl/vec_env.py` and `python/hares/rl/gym_env.py`.
- The `observation_error` path substitutes `dwelling.observation().unwrap_or_default()`, a different vector, when the field observation fails (`py_gym.rs:486-491`). RT-7 touches this code. With the plan failing at `reset()` on unknown fields, a step-time observation failure should end that environment with its error, and RT-7 should say so.

### 23. MINOR: RT-5's warn-once test and its effect on fleet status

- `warn_once_is_per_run` names no fixture or outage configuration that makes a run islanded. Name them.
- RT-5 (row 70) moves plausibility warnings into `RunResult.warnings` in every build. Under FL-2 (row 51) any home with such a warning becomes `Degraded` and leaves the aggregate. RT-5 should state this effect and confirm the fleet golden (`fleet_mixed_offset`) is unchanged, or record it.

### 24. MINOR: OD-2 site 3's error for unconvertible values may reach resolver output

The error for an unconvertible untyped value (`conversions.rs:683`) applies to all of `spec.parameters`, including values the resolver produced, not only override values.

Correction: state that the error names override-supplied keys, and census any resolver-emitted `null`, object or mixed array on raw specs so the goldens do not stop the entry unexpectedly.

## Items verified correct

**Fleet**
- Facts at HEAD all verified:
  - `Fleet::simulate` local pool and global-pool rule (`fleet.rs:313-324`);
  - `par_iter().map().collect()` keeps entry order (`:346-409`);
  - `DwellingOutcome` holds full `SimulationResults` (`:43-48`);
  - ResStock `retain_batches: true` (`:797`);
  - `SimStatus` (`:31-40`) and `map_status` (`:771-776`);
  - `Flagged` from warnings (`engine.rs:146-150, 276-280`);
  - aggregation skips only `Failed` (`aggregation.rs:274`, `failed:` at `:268`);
  - `n_succeeded` counts flagged homes (`py_fleet.rs:254, 285`);
  - the column-mismatch emptying (`aggregation.rs:572-586`), pinned by `test_py_fleet.py:404-462`;
  - the bucket intersection (`:588-597`);
  - epoch bucketing at `:508` and the `Z` label at `:636-637`.
- FL-4's premise holds: `from_parts` reinterprets the naive start in the site offset (`blueprint.rs:90-98`), so today is `Instant` alignment labelled at `+00:00`.
- FL-3 is correct: `step()` past the end returns one `SimError::Failed` per home (`fleet.rs:567-578`), pinned at `:1609-1615`. `HaresSimulationError` exists (`py_dwelling.rs:32-36`).
- The fold is independent of threads and chunks by construction: folded one home at a time in entry order, bounded by chunk size, with a deterministic `peak_live_home_results` test and RSS as a recorded bench only.
- The null rule, the `Mode` accumulator form, flagged homes never counted as success, `NoCoveredWeight`, local-time labelling deferred to B-12, and the `HeterogeneousTimeGrid` rule all match `dl-i08-architecture.fleet` and §2.4.

**Runtime and memory**
- RT-1:
  - `Telemetry` derives `Clone` (`hares-types/src/telemetry.rs:11-12`), so `clone_from` reallocates. The false comment is at `mod.rs:6431-6433`.
  - The per-step `HashSet` is at `:6389-6402`; the restore path at `:4892-4930`.
  - `DomainUpdate` derives `Clone` (`domain_solver.rs:14-19`), and `update_in_place` drains `custom_domains` (`environment.rs:705`), so the thermal, humidity, electrical, fluid and custom upserts push fresh clones each step.
  - `refresh_equipment_caches` runs after every mutation (`mod.rs:3085...3679`, defined at `:3794`).
  - The change is bitwise-safe (`f64` copies, entry order unchanged).
- RT-2: every listed dependency is real (`environment.rs:195-203, 240, 243, 548-575, 605-610, 621-676`; `blueprint.rs:74-86`). The eager table, per-surface work left per step, mutex off the step path, and recorded sizes are sound apart from finding 7.
- RT-3:
  - `exterior_surface_temps` is cloned and restored (`stepping.rs:672-674`), and the vector is never resized, so `copy_from_slice` is safe.
  - The latent map is dropped by `prepare_inputs_inner` (`stepping.rs:673`, taken at `mod.rs:~1275`, returned only at `:1522`).
  - `last_u.clone_from` is at `:691, 1217`.
  - `model.output` allocates (`state_space.rs:471-473`), and `output_into` with `C·x` then `D·u` then elementwise add is bitwise-equal.
  - `refresh_solar_slot_map` runs twice per step (`mod.rs:1217` via both phases; `:1560-1568`).
  - The 11-per-step bound is in `radiant_gain_weights_zero_alloc.rs:239-276`.
- RT-4: every obstacle verified (`blueprint.rs:40-65, 74-86, 183-225`; `mod.rs:1749, 1925-1930, 2605`; `py_dwelling.rs:1522`), and the fit-path factor split matches `c71c542`.
- RT-5: the static census is exact against the stated grep at HEAD (every hit is in the table). The `Send` evidence holds (`py_dwelling.rs:617-619` `#[pyclass]` over `Mutex<Dwelling>`), and out-of-bounds control signals are counted as rejected (`hares-equipment/src/lib.rs:40-52`).
- RT-6: sources exist (`building.rs` `infiltration_ach50`/`infiltration_ela_cm2`, `thermal_solver/config.rs:277-281`, `EquipmentRegistry::known_names` at `registry.rs:161`).
- RT-7: premise verified (`telemetry.rs:92, 250, 261`; `mod.rs:4160`; `py_gym.rs:34-41, 449-545`; `vec_env.py:226-247`).

**Slop cleanup**
- SL-1 is correct.
- SL-2's census is exact: 431 lines in 112 files, and every cited example checked.
- SL-3 matches `dl-i08-architecture.slop_scope`.
- SL-4 (`fit.py:319-321`), SL-5 (`scale.py:630-633`) and SL-6 (`check_losses.py`, `main()` always returns 0) are real at `c71c542`.
- SL-7's premise holds: both constructors set `m_mat` to the identity (`state_space.rs:193-231, 320-428`) and nothing sets `m_is_identity` to `false`. The sign-of-zero caveat is correct, since an identity LU solve through `axpy` can flip only `-0.0`.
- The SL-8 mechanism is correct and non-destructive. Reproduced in a scratch repository:
  - `git mv` into a missing parent fails;
  - with `mkdir -p` it succeeds into an ignored directory and carries ignored contents (`_logs/`);
  - `git rm -r --cached` then untracks the files and leaves them on disk;
  - `.shipwright/constitution.md` untracks the same way.
- SL-8's file counts (285, 114, 28, 3), the script citations and `.gitignore:63` are correct.

**Other defects**
- OD-1 is real (`py_dwelling.rs:30-37` against `pyproject.toml:77`). `create_exception!(hares._hares, ...)` is supported (pyo3 0.29.2 test at `exceptions.rs:1036`, `stringify!($module)`).
- OD-2 sites 1 to 9 are real at the cited lines, including `time_res_secs_u32`'s `unwrap_or(3600)`, the site-offset fallbacks and `wall_area_m2` (25 files). `EquipmentConfig` is a struct, so the flattened `RejectUnknownKeys` does not interact with untagged enum matching. `MaxChargingPower` is a HARES-specific element read in kW, so the 19.2 kW limit is the right check.
- OD-3, OD-6, OD-7 and OD-9 evidence is verified. OD-8's evidence and caller list are complete for parity (`tests/parity/corpus.rs` discovers fixtures), apart from findings 11 and 21.

**Ordering** (`00-schedule.md`):
- SL-7 (20) before RT-3 (21).
- PR-3 (16) before RT-1, RT-3 and RT-7.
- OD-6 (24) before RT-4 (61), which is before RT-2 (62) and RT-6 (65).
- IN-6 (33) before RT-2.
- IN-5 (64) before RT-6 and OD-9.
- PY-7 (26) before OD-1 and OD-7.
- OD-2 (31) before OD-8 (32) and PY-8 (34), with the consumer's probe assertion in CN-1 (72).
- SL-8 (25) after M-3 and before PY-7.
- FL-1 to FL-4 (50 to 53) after OR-1, OR-3, OR-5, OR-6, OR-7, OR-12 and OR-13.
- B-11 and B-12 after FL-4.
- Commit messages match the ledger.

## L1 findings marked fixed (verify-L1-c)

All 22 are fixed in the current text. In c-6, c-10 and c-17 the resolution was later superseded by a binding decision, and the text follows the decision:
- c-6: the fold order is entry order, not a sort. `collect` keeps entry order, so this satisfies the finding.
- c-10: SL-3 now applies only to touched items, per `dl-i08-architecture.slop_scope`. It has no whole-tree grep, so the contradiction the finding raised is gone.
- c-17: RT-3 now uses PR-3's shared allocator.

The other findings are fixed as follows:
- c-1 to c-5, c-7 to c-9, c-11 and c-12 are fixed; c-12 now carries the operator decisions (OD-2 site 6, OD-8).
- OD-4 (c-13) and OD-5 (c-14) are removed.
- c-15, c-16 and c-18 to c-22 are fixed.

Residual items: finding 20's `:509` citation survives outside FL-4 (finding 15 above), and the RT-2 key issue (finding 7) is new.
