# Verification: 04-fleet, 06-runtime-memory, 10-slop-cleanup, 11-other-defects

HEAD `ebdb1a17` (HARES), `c71c542` (consumer). Read-only.

Counts: 0 blocker, 14 major, 8 minor.

## Findings

### 1. MAJOR: RT-1: evidence mislocated; change underspecified against a public API; a smaller bitwise-safe fix exists

Evidence:
- `dwelling/mod.rs:4922-4924` is `snapshot_equipment_state`, which runs only after `load_checkpoint` (doc at 4886-4891). It has no comment.
- The per-step site is `mod.rs:6431-6436`. The false comment is at 6432-6433.
- The `HashSet` at 6389-6399 is used to retain only `equipment_core` (6400-6402). `equipment_telemetry` is retained against `equipment_id_by_name` (6404-6407).
- The "runtime-formatted" AC keys (`air_conditioner.rs:883-905`) are registered once at init and exist only under `cfg(feature = "observe")`.
- `EnvironmentState.equipment_telemetry: HashMap<String, Telemetry>` is public (`hares-types/src/environment.rs:460`). About 40 source files read it: actors, equipment, Python, and `actor.rs:202,321`.
- `Telemetry::set` (`telemetry.rs:61-70`) already requires every key to be registered at init. Steady-state key sets are therefore fixed.

Correction:
- Cite `mod.rs:6431-6436` (per step) and `4922-4924` (restore path).
- Replace the slot-table redesign with a manual `Clone for Telemetry` whose `clone_from` overwrites values in place when the key sets are equal and falls back to `clone()` otherwise. This needs no API change and is bitwise-neutral.
- If a slot table is still wanted, name the new reader API and every reader it replaces.
- Replace the `HashSet` with a `retain` against `equipment_id_by_name` values, or with a set maintained in `refresh_equipment_caches`.
- The acceptance test must run under `-F observe` to cover the staged-AC keys.
- Add PR-3 (the real allocation counter) to the dependencies.

### 2. MAJOR: RT-2: cache key incomplete, storage and ownership unspecified

Evidence (`environment.rs:524-782`): the per-surface output also depends on:
- `weather_start_offset`, which comes from the start time (535)
- `civil_tz`, which changes `day_of_year` under `dst` (561-568)
- `weather_meta` lat/lon/elevation, which `SimulationConfig.site_location` overrides (`blueprint.rs:75-84`)
- the resample overrides and `time_res` (`environment.rs:195-198`)
- `solar_override` (621-623)

The key "(geometry digest, weather source, clock resolution)" misses every item above. Two dwellings with different start dates would collide.

Nothing says whether the cache is eager (steps x surfaces) or lazy, who owns it, or how it is looked up across threads (RT-5). At 60 s steps, a year times about 15 surfaces times 32 B is roughly 250 MB per geometry. That is unbounded memory in a memory initiative.

`azimuth_shift_deg` is a consumer-side HPXML edit (`HARES_CALIBRATION_SYSTEM/calibration/registry.py:298`), not an engine override.

Correction:
- Key on the full tuple: surface geometry, weather identity, resample policy, `time_res`, start instant or `weather_start_offset`, horizon, resolved site lat/lon/elevation, and civil timezone. Treat `solar_override` as a cache bypass.
- State the storage (eager per-step table, owned by the `Arc`-shared blueprint, immutable after build) and record its byte size as a measurement.
- Express the geometry test as two HPXMLs that differ in `<Azimuth>`.
- Add a test that differs only in start date and must produce a different key.

### 3. MAJOR: RT-3: coupled-LU path is dead by construction; allocation census wrong and incomplete

Evidence, dead path:
- `m_is_identity` is set `true` in both constructors (`state_space.rs:228`, `425`) and never assigned `false` anywhere.
- So `build_coupled_lu`, `CoupledState::LU`, `m_lu`/`m_mat` and the `m_scratch` clone are unreachable. No measurement is needed to establish this.
- The doc at `state_space.rs:266` ("caller-supplied for discrete-path") is false: `from_discrete` sets M = I (192).
- The `debug_assert` comparing `eye` to a fresh identity (211-218, 407-414) is vacuous.

Evidence, allocations:
- `build_input_vector` does not return a fresh `HashMap`. It takes `self.latent_buf` (`thermal_solver/mod.rs:1282`). `prepare_inputs_inner` then drops it (`stepping.rs:673`, `_latent`), so `integrate_inner` reallocates the table every step.
- `self.last_u.clone_from(&u)` (`stepping.rs:691`) allocates every step. nalgebra 0.34.2 `Matrix` uses `#[derive(Clone)]` (`src/base/matrix.rs:163`), so `clone_from` is `*self = clone()`.
- The `exterior_surface_temps` clone at 672 is real.

Correction:
- Drop step 1's `m_is_identity` measurement.
- Add a slop item that removes the dead LU path, `m_lu`/`m_mat`, the vacuous asserts and the false doc, then restates `m_is_identity` as a type-level fact or removes it.
- Specify three fixes:
  - restore `self.latent_buf` from the prepare call
  - keep a persistent scratch `Vec<f64>` for the exterior temperatures, using `copy_from_slice` for save and restore
  - use `last_u.copy_from(&u)` with a pre-sized `last_u`
- Remove these unconditionally: the no-allocation-in-the-hot-loop rule makes them non-optional, so no profile gate. Keep the percentage share as a recorded measurement only.

### 4. MAJOR: RT-4: premise about per-build mutation is false; parse-once overstated for the fit path; `ResolvedModel` undefined

Evidence:
- `from_parts` runs once, at blueprint construction (`blueprint.rs:65`, from `from_config`). `build()` (`blueprint.rs:225`) never calls it, so the "hoist" is already the case.
- The real obstacles are:
  - `build(self)` consumes the blueprint
  - `bp.rng` is moved out (`mod.rs:1930`)
  - weather and schedule are resampled per build inside `EnvironmentManager::new_with_resample` (`environment.rs:195-203`)
- In the fit path, 9 of the 11 free factors are routed as HPXML text edits (`route='hpxml'`/`'hpxml_list'`) or defaults-CSV edits (`opaque_r_scale`, `thermal_mass_scale`) (`registry.py`, `FREE`). HPXML and defaults differ per candidate; only weather and schedule are shareable.
- `ResolvedModel` has no shape and no test.
- The blueprint mutators (`remove_equipment`, `add_equipment_spec`, `&mut self`, `blueprint.rs:183-222`) conflict with `Arc` sharing.

Correction:
- Rewrite the problem around those three obstacles.
- The change: `build(&self)` clones the rng; resampled weather and schedule at the blueprint's `time_res` are cached in the blueprint behind `Arc`; the mutators go through a builder, or `Arc::make_mut`, stated explicitly.
- State that HPXML and defaults are re-parsed per candidate until an edit seam exists, and scale the measurement expectation to match. The ~1 GB RT-5 target also depends on this.
- Either define `ResolvedModel` (fields and the source of each) with an acceptance test, or move it to its own entry.
- Use one name: `DwellingBlueprint`.

### 5. MAJOR: RT-5: the "full census" is wrong and incomplete; the `Sync` requirement is misplaced

Evidence, wrong paths:
- `mode_flow_guard.rs:16` is in `hares-types`, not `hares-equipment`.
- `pv_sizing.rs:232` is in `hares-physics`.
- `site_location.rs:51` is in `hares-io`.

Evidence, missing statics:
- Warn-once flags: `hares-core/src/telemetry.rs:61,64`, `hares-core/src/dwelling/mod.rs:144` (`ISLAND_UNSERVED_WARNED`), `hares-equipment/src/lib.rs:38`.
- Mutex statics: `hares-types/src/equipment.rs:1313` (a shared `RANGE_EVENTS` ring, so events from concurrent dwellings interleave), `panic_hook.rs:67,128`, `py_actor.rs:539-541`.
- Once/Lazy statics: `ashrae152.rs:265`, `psychrometrics.rs:46`, `battery/catalog.rs:187`, `dwelling/mod.rs:8744`.
- `thread_local`: `panic_hook.rs:38`.

Evidence, `Send`/`Sync`:
- "Nothing today proves `Dwelling: Send`" is false. `PyDwelling { dwelling: Mutex<Dwelling> }` as a `#[pyclass]` (`py_dwelling.rs:617-619`) and `Fleet`'s `par_iter` (`fleet.rs:345`) already require it.
- Threads that each own a dwelling need `Dwelling: Send` and `DwellingBlueprint: Send + Sync`, not `Dwelling: Sync`.
- Warn-once statics are already shared across `hares-fleet` rayon threads today.

Correction:
- Regenerate the census with a stated grep: `static`, `thread_local!`, `OnceLock`/`LazyLock`/`Once` across `crates/*/src`. Classify each item as immutable, process diagnostic counter, per-run state or warn-once.
- Assert `Dwelling: Send` and `DwellingBlueprint: Send + Sync` under every feature set. Drop step 3 (replacing the `observe` `Cell`s) unless a shared-`&Dwelling` use case is named.
- State that no process-global counter may feed `RunHealth` (OR-5).

### 6. MAJOR: FL-1: fold order not chunk-size independent; order test vacuous; memory test non-deterministic; null semantics missing

Evidence:
- Sorting within each chunk gives a global order that depends on chunk boundaries unless the entries are sorted first. `bldg_id`s are not guaranteed unique: `tests/regression/determinism.rs:25-26` builds `bldg_id` 1 twice.
- `RAYON_NUM_THREADS` is read once, when the global pool is initialised. `Fleet::simulate(n_threads)` (`fleet.rs:313-324`) is the real control.
- An RSS-ratio test at 10,000 simulated homes is slow and allocator-dependent.
- `Accumulator::finish` returns `None` whenever any value is null (`aggregation.rs:235-238`). After B-6 introduces nulls, every affected fleet cell becomes null.
- Aggregation kind is still inferred from unit suffixes (`ColumnAggregation::for_column`, `aggregation.rs:312`), contrary to §2.6.

Correction:
- Sort entries by `(bldg_id, entry index)` before chunking, and say the fold order is that global order.
- The test varies `Fleet::simulate(n_threads)` over {1, 2, 8} and chunk size over {1, 3, 64}, then compares the full `FleetResult` bitwise.
- Replace the RSS assertion with a deterministic peak-live-`DwellingOutcome` counter, or peak bytes from the PR-3 counting allocator, asserted `<= chunk_size`. RSS at 100/1,000/10,000 becomes a recorded measurement only.
- Define the fold's per-cell null rule (null contributes nothing and is excluded from that cell's weight, with coverage recorded) and take the aggregation kind from `VariableDef`.
- Make the accumulator Mode-capable for B-11 (per-code weight maps).

### 7. MAJOR: FL-2: flagged homes remain counted as successes; the status type does not exist; not-applicable conflated with degraded

Evidence:
- The fleet type is `SimStatus { Ok, Flagged, Failed }` (`fleet.rs:33-40`). There is no `HomeStatus`.
- `CoreSimStatus::Flagged` (`engine.rs:147,277`) is aggregated like `Ok`: only `Failed` is skipped (`aggregation.rs:274`), `failed: false` (268), and it is counted in `n_succeeded` (`py_fleet.rs:285`). The decision says "a flagged home is never counted as a success."
- A home without a battery legitimately lacks battery columns. Under FL-2 as written it would become `Degraded`.
- The test path is `tests/python/test_py_fleet.py:404-462`, not `crates/hares-fleet/...`.

Correction:
- Replace `SimStatus` with `HomeStatus` and state the mapping: `Flagged` or a `RunHealth` policy flag becomes `Degraded`.
- State whether `Degraded` rows enter the aggregate. Either way they count in `weight_degraded`, never in `weight_ok`, and never in `n_succeeded`.
- A column absent from a home's catalogue is not-applicable: it is excluded from that column's coverage, and the home stays `Ok`. Only a declared-but-missing variable, or zero rows, gives `Degraded`.
- Add the test `flagged_home_is_not_counted_as_success`.

### 8. MAJOR: FL-3: problem misstated; the acceptance test passes at HEAD

Evidence: `Failed` is returned only when `step()` is called after `is_finished()` (`fleet.rs:567-578`). The existing test `fleet.rs:1600-1604` already shows every step `Ok` up to the end, and `fleet.rs:1608-1614` pins the post-end `Failed`. The stepper reports `Result<StepResult, SimError>`, not a per-home status.

Correction:
- `step()` past the end returns one fleet-level error, for example `FleetError::AlreadyFinished`, not per-home `SimError::Failed`. Map it in Python (`py_fleet.rs:214,489`).
- The test calls `step()` after `is_finished()` and asserts that fleet-level error. Rewrite `fleet.rs:1608-1614`.

### 9. MAJOR: FL-4: today's behaviour and `LocalClock` misdescribed

Evidence: buckets are keyed on the UTC epoch (`bucket_start(ts).timestamp()`, `aggregation.rs:508`) and labelled UTC `Z` (636-637). Today is `Instant` alignment labelled in UTC, not "closest to `LocalClock`." "`LocalClock` keeps every home's own local labels" is incoherent for a single aggregate row.

Correction:
- `LocalClock`: bucket by each home's naive local wall-clock time; the row label is that shared naive local time.
- `Instant(offset)`: bucket by the epoch; the label is the naive time in the declared fixed UTC offset.
- Say "fixed offset" (local-time rule) and reconcile the decision's "reference zone" wording.
- Record that today's behaviour equals `Instant(+00:00)`.

### 10. MAJOR: SL-2/SL-3: census incomplete; the acceptance grep contradicts the deferral

Evidence:
- `T-NNNN` ticket IDs: 204 occurrences in 55 files (for example `hares-equipment/src/lib.rs:211,982`, `dwelling/synthetic.rs:141` "T-0226", `tests/si_guard.rs:636-697`).
- Review references: "Finding N" at `heat_pump_wh.rs:562`, `battery/degradation.rs:733`, `si_guard.rs:637`; "future reviewer" at `hvac_tests.rs:3499`.
- Another 49 matches sit under `python/`, `tests/`, `benches/`, `examples/`.
- SL-2 defers `dwelling/mod.rs:4909` ("I-06") to a later entry, but SL-3 (row 8) asserts the grep over all of `crates/` is empty, so it fails at its own row.

Correction:
- Pattern `\bT-[0-9]{3,4}\b|\bI-0[0-9]\b|Task [0-9]|Finding [0-9]|reviewer`, over `crates python tests benches examples`. Exclude citations such as NREL/CP-, TP-, FHWA-PL-, `IH-002` fixture ids, and SHA-.
- SL-3 fixes every match, including `mod.rs:4909`.

### 11. MAJOR: OD-1: hard-codes a module path the design renames; two unchosen mechanisms; "equal exception" untestable

Evidence:
- The frame (§2.7) renames the import to `hares`, but no ledger entry performs the rename. `05-python-surface.md` has none.
- OD-1 would register `ochre_next._hares`.
- Python exceptions compare by identity, so "reconstructs to an equal exception" cannot pass.

Correction:
- Choose `create_exception!(<pkg>._hares, ...)` (pyo3 stringifies the module expression).
- Add the missing rename entry, and either sequence OD-1 after it or have the rename entry update the path.
- The test asserts type identity and `args` across a `multiprocessing` spawn.

### 12. MAJOR: OD-2: wrong paths, unspecified mechanism, undecided per-site behaviour, and one non-defect

Evidence, wrong paths:
- `HeatPumpConfig` is `pub type HeatPumpConfig = HeatPumpHeaterConfig` at `crates/hares-equipment/src/hvac/heat_pump_config.rs:700`. The flatten sites are 262, 297 and 483. `hares-io/src/hpxml/heat_pump_config.rs` does not exist.
- The override sites are in `crates/hares-core/src/dwelling/conversions.rs:937-942,1085-1099`.
- `schedule_resolve.rs` is `crates/hares-io/src/schedule_resolve.rs`.

Evidence, mechanism and pinning tests:
- serde `flatten` cannot combine with `deny_unknown_fields`; the doc at 254-256 and 476-478 says so.
- Unlisted tests pin the permissive behaviour: `heat_pump_config.rs:1062,1078` and `config_round_trip.rs:820-833` (the TOML twin).

Evidence, mis-described site: `resolve_hvac.rs:315-351` discards typed-config build errors via `.ok().flatten()`. It is not an "untyped override value dropped".

Evidence, undecided behaviour: nothing says what should happen for a battery with no BMS (`resolve_der.rs:163`; `bms_mode` None becomes `Manual`, and `actor_seed` returns None at `battery/mod.rs:1641`), for EV power units, or for thermostat precedence.

Evidence, non-defect: `master_seed` defaults to 0 in both Rust (`config.rs:99`, and the test at `config.rs:240`) and Python (`py_config.rs:205`). Omitting 0 is therefore not observable, and `master_seed_zero_is_honored_not_dropped` passes at HEAD.

Correction:
- Fix every path.
- Name the strict mechanism: a trailing `#[serde(flatten)] unknown: BTreeMap<String, Value>` rejected when non-empty, or unflatten.
- List every pinning test.
- Replace `.ok().flatten()` with error propagation.
- Decide each site's behaviour per `dl-i08-architecture.precedence`: the override wins or is rejected; HPXML battery gets a named default control mode or an error; `MaxChargingPower` is converted from W with its unit checked.
- Reclassify `master_seed` as slop (use `is not None`), with no defect test.
- Require a golden comparison, because the water-heater UA and thermostat changes can move numbers.

### 13. MAJOR: OD-4: premise false; the regression coverage already exists

Evidence: `rc_network.rs:1020` is the doc of a deterministic test, `from_elements_external_node_positions_match_build_matrices_columns` (1037-1038). `boundary_rc.rs:2386-2504` has `multi_depth_ground_cols_match_assembled_matrix` with depths of 0.3 m and 2.0 m, including a B_ext wiring check.

Correction: drop OD-4. Or, if a full-dwelling end-to-end run is wanted, name the observable output column per depth, which does not exist today. Also remove the planning IDs in those tests under SL-3.

### 14. MAJOR: OD-5: the test is not a fleet test; the filter matches nothing; scheduling ambiguous

Evidence:
- `tests/regression/determinism.rs:43` uses a single-dwelling `engine.run`, and the thread env var has no effect on it.
- The only test function is `regression_full_suite` (`tests/regression/mod.rs:33`), so `regression::determinism` selects no tests and nextest errors.
- Row 31 always follows row 30 (FL-1), so the "own commit" branch is dead.

Correction:
- State that OD-5 folds into FL-1's commit, which replaces the suite with the finding-6 test.
- Keep single-dwelling determinism under RT-5's concurrent test.
- Fix the verification filter to a real test name in `hares-fleet`.

### 15. MINOR: SL-1 and `plan.md` §3 cite the wrong line

The comment is at `mod.rs:6431-6433` and there is no twin comment. Cite 6431-6433 in both.

### 16. MINOR: RT-1/OR-2 interplay not stated

OR-2's `RowPlan` producer slots are resolved at row 20. RT-1 (row 43) changes telemetry storage. State whether the telemetry-sourced producer slots are re-resolved, so the two index layers do not duplicate.

### 17. MINOR: RT-3 dependencies

The allocation harness comes from PR-3, not from RT-1. List PR-3.

### 18. MINOR: OD-3 misses branches; test shape unclear

Also make these silent branches fail:
- `typed_config == None` early return (`equipment.rs:247-249`, 289)
- the silent `_ => {}` (277)

The test is a unit test of the wiring pass over an `EquipmentSpec` whose typed payload type mismatches.

### 19. MINOR: SL-2 overstates the untouched-file list

OD-2 touches `resolve_hvac.rs` and OD-4 touches `rc_network.rs`. This is harmless, because SL-3 (row 8) lands first. Reword the sentence.

### 20. MINOR: FL-1's `aggregation.rs:509` citation is off by one

The bucket key is at 508.

### 21. MINOR: RT-2 problem lists the schedule-row gather without handling it

The schedule-row gather is listed as recomputed, but it is per-dwelling and not cached. Remove it from the problem statement or say it stays per dwelling.

### 22. MINOR: 11-other-defects preamble claims no entry moves a number

The claim "none moves a physics number" is unproven for OD-2 (UA fallback, thermostat precedence). Require `frame_golden compare-all` in OD-2's verification.

## Verified correct

- The RT-1 core premise is correct. `Telemetry` derives `Clone` (`hares-types/src/telemetry.rs:11-12`), so `clone_from` reallocates every step at `mod.rs:6436`. The per-step `HashSet` rebuild at 6389-6399 is real.
- The RT-2 premise that `update_in_place` recomputes solar position, Perez, psychrometrics and mains every step (`environment.rs:524-782`) is correct. PV roof coverage is applied after Perez (667-676), so read-time application is bitwise-safe.
- The RT-3 `exterior_surface_temps` save/restore clone (`stepping.rs:672-674`) is real, and the `m_is_identity` citation (`state_space.rs:262-263`) is correct.
- RT-4 `build(self)` consuming the blueprint (`blueprint.rs:225`) is correct.
- RT-5 `clock.rs` `Cell`s under `observe` (`clock.rs:50-56`) are correct. The listed atomics at `panic_hook.rs:71,75,101`, `telemetry.rs:20`, `hares-equipment/src/lib.rs:35`, `equipment.rs:1236,1239`, `state_space.rs:181` and `py_gym.rs:208,212` exist.
- FL-1: `DwellingOutcome` holds full `SimulationResults` (`fleet.rs:43-48`); `par_iter().map().collect()` (345-409); `aggregate(&[DwellingOutcome])` (`aggregation.rs:248`); ResStock `retain_batches: true` (`fleet.rs:797`).
- FL-2: status recorded before the skip (`aggregation.rs:257-270`); empty rows skipped (294-299); a mismatch empties the whole aggregate (571-585); `test_py_fleet.py` pins that emptying.
- SL-2: the `I-0N` census is exact (25 matches, 12 files), and the examples at `boundary_rc.rs:2495,2500`, `mod.rs:4909`, `hares-equipment/src/config.rs:472` and `resolve_hvac.rs:6497,6536` are accurate.
- SL-4 (`fit.py:319-321`, uniform sampling rather than Latin-hypercube), SL-5 (`scale.py:630` defaults `workers` before the guard at 633) and SL-6 (`check_losses.py:44` always returns 0) are real.
- OD-1's defect is real (`py_dwelling.rs:30-37` against `pyproject.toml:77`).
- OD-2 sites confirmed real, some at other paths: `time_res_secs_u32`'s `unwrap_or(3600)` (`config.rs:139-141`) and the missing `deny_unknown_fields` on `SimulationConfig`; the non-object and `all`/`*` override merges (`dwelling/conversions.rs:937-942`); `json_value_to_config_value` dropping mixed arrays and nulls (1085-1099); the water-heater UA fallback (`resolve_water_heater.rs:131-141`); the CSV setpoint column overwriting other sources (`schedule_resolve.rs:673-690`); the dead `wall_area_m2` (`synthetic.rs:137-143`); the gym `_coerce_seconds` 60.0 fallback (`gym_env.py:100-105`).
- OD-3's defect is real (`hpxml/equipment.rs:240-302`, warn and continue).
- OD-5: the sorted-totals, epsilon comparison is vacuous (`determinism.rs:45-80`).
- Schedule: RT-4 comes before RT-2, RT-1 before RT-3, and RT-5 last. SL-3 lands before the OD entries that touch the same files.
