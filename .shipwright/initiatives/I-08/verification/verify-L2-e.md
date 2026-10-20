# Traceability audit of the I-08 ledger (L2, pass e)

Scope: `plan.md` and `ledger/00-schedule.md` to `ledger/12-inputs.md`, checked against
`decision-log.md` (every key under `dl-i08-process` and `dl-i08-architecture`, plus
`dl-i08-criteria`), the 45 `feedback_*.md` rules, `context.md` items 1 to 21, `review.md` V1 to
V50, every requirement ID in `exploration.md` (R1 to R25, PY-1 to PY-14, RL-1 to RL-8, FL-1 to
FL-11, RS-1 to RS-9, DV-1 to DV-9, X-1 to X-12), and every finding of `verification/verify-L1-e.md`.
The resolution claims in `verification/L1-resolution-*.md` and `L1-integration.md` were not
trusted: each was checked against the current plan and ledger text. The tables in the appendices
are my own. Read-only except this file.

Code spot checks (HARES at `ebdb1a17`, consumer at `c71c542`): `KNOWN_KWARGS`
(`py_dwelling.rs:2142-2165`), `DwellingConfig` and `PyDwellingConfig` fields (`mod.rs:210-238`,
`py_config.rs:550-575`), `SimulationConfig` fields (`config.rs:61-128`), `set_tariff`
(`mod.rs:4123`), `custom_domains` sites (`mod.rs:4191, 4959, 7286, 7313`;
`hares-types/src/environment.rs:500-525`), EV parsing (`resolve_der.rs:182-210`) against the
ResStock 2025.1 fixtures, gym reset (`gym_env.py:536-548`, `vec_env.py:176-182`,
`py_dwelling.rs:1518-1532`), the existing charging-strategy regression test
(`hares-equipment/src/ev/tests.rs:8062`), consumer `RESAMPLE_OVERRIDES` (`grid.py:173`, `None`)
and `build_kwargs` (`engine.py:320-346`), schedule CSVs with setpoint columns (none), factorization
call sites in `hares-envelope` (construction only after SL-7).

Severity: **blocker** = a binding decision or rule is contradicted, or a context item, confirmed
defect or in-scope must-requirement is missing. **major** = partial or unclear coverage, or a
claimed resolution that is not present in the text. **minor** = everything else.

**Counts: 3 blocker, 13 major, 30 minor (46 findings).**

---

## Findings

### Blockers

**1. (blocker) Renamed and removed Python keywords do not name their replacement.**
- Source: `dl-i08-architecture.config` ("Strict everywhere: unknown or removed keys are errors that
  name the replacement"); `feedback_no_silent_defaults`.
- Text: PY-2 gives `Dwelling.from_hpxml(hpxml, schedule, weather, *, config, bldg_id=0,
  initialization_duration=None, defaults=None, overrides=None)`; "a key in `REMOVED_KEYS` raises the
  removed-key message; any other unknown keyword is Python's own `TypeError`". At `ebdb1a17`
  `KNOWN_KWARGS` holds `defaults_path`, `latitude`, `longitude`, `elevation_m` and `utc_offset_h`,
  and `PyDwellingConfig` takes `defaults_path` (`py_config.rs:572`). `defaults_path` becomes
  `defaults` (PY-2, B-10, CN-1) and the four site keywords become `SimulationConfig.site_location`,
  yet none is in the `plan.md` §2.7 removed-key table, so each falls through to a generic
  `TypeError` that names nothing. PY-2's rule for simulation keywords ("a `TypeError` naming
  `config=SimulationConfig(master_seed=...)`") would name a field that does not exist for
  `latitude=`.
- Correction: add rows to the §2.7 table: `defaults_path` (Python keyword, `DwellingConfig`
  argument) names `defaults`; `latitude`, `longitude`, `elevation_m`, `utc_offset_h` name
  `config=SimulationConfig(site_location=...)`. In PY-2, add a disposition table for all 22
  `KNOWN_KWARGS` entries at `ebdb1a17` (kept as dwelling keyword, moved into `SimulationConfig`
  under which name, or removed naming what), and parametrize
  `removed_keyword_names_replacement` over every removed or renamed entry of that table, on both
  `Dwelling.from_hpxml` and `DwellingConfig`.

**2. (blocker) Initiative documents carry draft history and stale status.**
- Source: `dl-i08-process.documents` ("hold only current, correct content"), `writing`;
  `feedback_shipwright_files_authoritative` ("do not leave superseded designs or history").
- Text: `review.md` V49 "An unfinished plan draft left several output-model gaps ... confirmed as
  gaps in that draft ... superseded by the decisions below" and V50 "An early sink-contract draft
  kept `path: Option<PathBuf>` ... confirmed as a gap in that draft". `initiative.md` Status:
  "`plan.md` is being rewritten as the ledger", which is no longer true, and its document list
  omits `ledger/` and `verification/`.
- Correction: rewrite V49 and V50 as current design requirements with no draft reference (for
  example V49: "No file destination may default to a working-directory path; enum columns need a
  `Mode` reducer; `reward_fn` must reach Python on every path; a fleet column mismatch needs a
  defined policy. Status: requirement, not a HARES defect. Ledger: `plan.md` §2.2, §2.6, PY-5,
  FL-2"), or delete the two rows and renumber nothing (leave the IDs unused). Rewrite
  `initiative.md` Status to the current state ("`plan.md` and `ledger/` hold the ledger; not ready
  until `plan-critique.md` records a round with zero blockers and zero majors") and list
  `ledger/00-schedule.md` and `verification/` in the reading order.

**3. (blocker) RT-1 keeps the `custom_domains` vector and its per-step linear searches, which a
binding rule forbids.**
- Source: `feedback_custom_domains_perf` ("When refactoring the dwelling step loop, replace
  custom_domains Vec with pre-allocated domain slots indexed by DomainId. Built-in domains should
  be fixed fields, not dynamically searched"); `feedback_hot_loop_minimal`.
- Text: RT-1 change 3 refactors exactly this path (spare list, `upsert_domain_ref` reuse) and
  states "The presence and order of the entries every reader sees during a step are unchanged":
  `upsert_domain`/`upsert_domain_ref` still `iter_mut().find(|u| u.domain_id == ...)`
  (`hares-types/src/environment.rs:500-525`) and readers still scan with `.iter().find(...)` at
  `mod.rs:4191, 4959, 7286, 7313` every step. The allocations go; the dynamic search the rule
  names stays.
- Correction: in RT-1, replace `EnvironmentState::custom_domains: Vec<DomainUpdate>` by fixed
  fields for the built-in domains (thermal, humidity, electrical, fluid, schedule) plus an
  `Vec<Option<DomainUpdate>>` indexed by a dense `DomainId` for custom domains, allocated at
  assembly; readers index directly. Keep the bitwise-golden and zero-allocation tests and add
  `domain_lookup_is_indexed` (a structural test that `EnvironmentState` exposes no search API, or
  a compile-time check that the built-in accessors are field reads).

### Majors

**4. (major) Sharing parsed inputs across blueprints is not specified, and CN-10 depends on an
API no entry builds.**
- Source: `dl-i08-architecture.inputs` ("parsed data shared immutably across dwellings"),
  `engine_performance` ("exogenous forcing reuse shared through the parsed weather"); context items
  2, 3 and 6.
- Text: `DwellingConfig` holds sources (`hpxml: InputSource`, `weather: WeatherInput`, ...), and
  `DwellingBlueprint::from_config` parses them; `builds_share_parsed_inputs` covers only dwellings
  built from one blueprint. RT-4 itself says "Only weather, schedule and all but one defaults file
  are shareable across candidates", and every calibration candidate has its own HPXML, so its own
  blueprint. No API builds a blueprint from an existing `Arc<ParsedWeather>` (or
  `ParsedSchedule`/`ParsedDefaults`), so RT-2's forcing tables cached "on the `ParsedWeather` they
  came from" are never shared across candidates, RT-5's 26-dwelling thread measurement cannot share
  weather, and CN-10 ("Each distinct weather file of a batch is parsed once into the engine's
  shared weather content ... RT-4 (content inputs and shared parsed handles, including their
  Python constructors)") has no engine surface to call. Passing weather as `Text` content re-parses
  it per blueprint.
- Correction: in `plan.md` §2.7 and RT-4, define how a blueprint takes already-parsed handles (for
  example `DwellingBlueprintBuilder::new(hpxml: Arc<ParsedHpxml>, schedule:
  Option<Arc<ParsedSchedule>>, weather: Arc<ParsedWeather>, defaults: Arc<ParsedDefaults>, ...)`
  plus the Python classes `hares.ParsedWeather.from_path/from_text`, `ParsedSchedule`,
  `ParsedDefaults` accepted by `from_hpxml`), and add
  `blueprints_from_different_hpxml_share_weather_and_forcing` (two HPXMLs, one `ParsedWeather`:
  `Arc::ptr_eq` on the weather handle and on the `ForcingTable`, frames bitwise equal to separate
  parses) plus the Python counterpart CN-10 relies on.

**5. (major) RT-2 drops per-surface solar and incidence reuse before the profile, on a
calibration-only argument.**
- Source: `dl-i08-architecture.engine_performance` ("envelope solar/incidence ... reuse ... An
  item the profile shows is not worth it is dropped with the measurement recorded");
  `feedback_design_for_all_users`; context item 4.
- Text: RT-2 decides "Per-surface values are not cached ... which buys nothing when every
  candidate's geometry differs" and records the remaining share afterwards "as the measurement for
  which per-surface reuse is dropped". Every dwelling built from one blueprint (RL resets through
  `build_with_seed`, RS-8 repeated evaluation, the batched-evaluation seam, fleet homes sharing an
  archetype) has identical geometry, so a per-blueprint per-surface table is reusable for them. The
  drop does not follow from the profile, and it is shaped around one consumer.
- Correction: make RT-2's per-surface item profile-driven with a stated rule: after the
  surface-independent table lands, record the per-surface Perez and incidence share on
  `consumer_shape_900s` and on a repeated-build loop of one blueprint (M-3 item 5's stepped
  episodes); if the share is at or above the program's threshold (state it, for example 2% of
  `simulate()`), add a per-blueprint `Arc` per-surface table with
  `per_surface_table_shared_across_builds_of_one_blueprint` and bitwise goldens; otherwise record
  the share and drop it. Either way the entry names both measurements.

**6. (major) Several changes that move numbers have no fixture to record their delta on.**
- Source: `dl-i08-architecture.numeric_fixes` ("each in its own commit with golden re-capture
  and a recorded delta"); `09-numeric-fixes.md` preamble.
- Text:
  - B-4 depends on "a golden with billing summaries spanning a month boundary" and records "per-
    period billing delta on the month-boundary golden". No M-2 manifest configures a tariff, the
    manifest format has no tariff key, and a tariff is set only through `Dwelling::set_tariff`
    (`mod.rs:4123`), not `SimulationConfig`. M-1's `billing` product is therefore never produced.
  - B-9 records a "delta on the ResStock golden". No manifest runs `Fleet::from_resstock`;
    `fleet_mixed_offset` gives explicit homes and an explicit start, so B-9 changes no golden.
  - IN-5 reads schedule setpoint columns in Fahrenheit (a numeric change for every schedule with
    such a column) and OD-2 site 6 makes a constant setpoint override take effect (a numeric
    change for every run with that override). Both land outside the numeric block, record no
    delta and add no `docs/PHYSICS_DECISIONS.md` section; OD-2 also bundles site 6 with eight
    strictness sites in one commit.
- Correction: M-1 manifest gains an optional `[tariff]` table (a tariff file path and zone) that
  the adapter passes to `set_tariff`; M-2 adds `month_boundary_tariff_3600s` (cz2a inputs,
  2023-01-25 to 2023-02-05, flat and time-of-use tariffs) so B-4, OR-10 and the `billing` product
  have a fixture. M-2 adds `resstock_2025_fleet` (kind `fleet`, built through
  `Fleet::from_resstock` over three `tests/fixtures/resstock/2025.1/` buildings) so B-9 has a
  golden. IN-5 records the delta of a `base.xml` run with a Fahrenheit setpoint column before and
  after, in the log, and adds its section to `docs/PHYSICS_DECISIONS.md` (or lands after B-1a in
  the numeric block). OD-2 site 6 becomes its own entry (or its own commit inside OD-2's row) with
  the `constant_setpoint_override_applies` run's delta recorded.

**7. (major) Ownership of the Python replay binding is conditional, and PY-9 lists a test that
cannot pass at its commit.**
- Source: `dl-i08-process.plan_form` (ready to execute; acceptance test per entry).
- Text: PY-9 adds "`RunResult.export_replay(dir)`, `hares.replay(dir, defaults) -> RunResult`" and
  the test `replay_round_trip_in_python`, then says "the `export_replay`/`replay` binding lands
  with OR-16 (this entry adds it if OR-16 has landed, otherwise OR-16's commit adds it)". The
  schedule fixes the order: PY-9 is row 54 and OR-16 row 69, so the Rust function does not exist
  at PY-9's commit. OR-16's own acceptance tests are Rust only.
- Correction: remove `export_replay`, `hares.replay` and `replay_round_trip_in_python` from PY-9;
  add them to OR-16's change and acceptance tests ("Python: `RunResult.export_replay(dir)`,
  `hares.replay(dir, defaults) -> RunResult`; test `replay_round_trip_in_python`"), and add
  `_hares.pyi` coverage there.

**8. (major) The Python `SimulationConfig` keyword names are not defined against the Rust field
names.**
- Source: `dl-i08-architecture.config` ("One Rust config type ... Python builds it from typed
  keyword arguments"); X-7.
- Text: PY-8 says "`start_time` and `duration_s` are required, as in Rust" and "Getters for every
  field", while the Rust fields are `duration` and `time_res` in seconds (§2.7 removed-key rows
  for synthetic TOML name `time_res`, `duration`). Whether the Python constructor takes
  `duration_s` or `duration`, and `time_res_s` or `time_res`, is not stated, nor what
  `SimulationConfig(site_location=...)`, `reports=`, `warmup=` accept as Python values. CN-1,
  B-3, the stubs and `removed_key_messages_equal_across_toml_and_python` all depend on it.
- Correction: in PY-8, state that the Python keyword names are exactly the Rust field names
  (`duration` and `time_res` as integer seconds or `timedelta`), list the Python type accepted per
  field (including `site_location` as a `SiteLocation` pyclass), and add `duration_s` and
  `time_res_s` (Python keywords at `ebdb1a17`) to `REMOVED_KEYS` naming `duration` and
  `time_res`; test `python_keyword_names_equal_rust_field_names` (the constructor's signature
  names equal the serde field names).

**9. (major) Many acceptance tests name no real fixture.**
- Source: `dl-i08-process.plan_form` ("acceptance test (named, deterministic, real fixture)").
- Text: tests that say "a fixture" or describe one without naming it: OR-5
  `curve_index_clamp_counted` ("an HVAC config"), `warmup_nonconvergence_recorded_and_warned` ("a
  fixture that does not converge in two"), `warmup_residuals_recorded_without_changing_days_run`
  ("a fixture with a three-day warm-up"); OR-6 `stepped_run_finish_equals_simulate`; OR-10 "a
  tariff fixture", "a time-of-use fixture", "a sub-hourly fixture"; OR-15 "a run"; B-2 "a
  heavyweight fixture" (three tests), "a fixture whose zone temperatures are held by the
  thermostat", "a fixture that converges on day 2"; B-4 "a dwelling run ... with a flat tariff";
  B-5 (the misnamed-equipment fixture); B-6 "a heat-pump fixture"; B-7 "one fixture"; B-9 "a
  2025.1 fixture"; B-12 "a UTC-5 fleet"; PY-5 "two environments"; OR-13's test actor is named but
  its pattern is not.
- Correction: name the inputs for each (path under `tests/fixtures/`, start, duration, step and
  any override), and for the warm-up tests pick and name the fixture whose day-2 residual lies
  between 0.4 and 0.5 C (record the measured residuals in the log when the entry starts, then pin
  the fixture).

**10. (major) A found defect is not tracked: HPXML v4 vehicles and chargers are ignored.**
- Source: `dl-i08-process.found_defects`; `dl-i08-architecture.precedence` ("accepted means
  applied"); `feedback_no_silent_defaults`, `feedback_no_broken_windows`, `feedback_si_units`.
- Evidence: the parser reads EVs only from `Systems/ElectricVehicles/ElectricVehicle` with a
  `MaxChargingPower` in kW (`resolve_der.rs:182-210`). The ResStock 2025.1 fixtures
  `bldg0176775` and `bldg0442669` declare their EVs as `Systems/Vehicles/Vehicle/VehicleType/
  BatteryElectricVehicle` with `ElectricVehicleChargers/ElectricVehicleCharger/ChargingPower` in W
  (`bldg0176775/home.xml:1835-1858`, 5690.0 W). Those EVs are silently not simulated. OD-8 already
  notes that these `<Battery>` elements "belong to vehicles" but no entry records the defect. The EV
  config also stores power in kW (`max_charging_power_kw`), against the SI rule.
- Correction: add an entry in `11-other-defects.md` (numeric, in the numeric block, own commit):
  parse `Vehicles/Vehicle` battery-electric vehicles and their connected
  `ElectricVehicleCharger/ChargingPower` (W) into the EV config, store power in W, keep OD-2 site
  7's range check on the converted value, and declare the elements in IN-3's `HPXML_ELEMENTS`;
  tests `resstock_vehicle_ev_is_simulated` (`bldg0176775`: an EV end use with non-zero charging
  energy) and `charger_power_read_in_watts`; record the delta.

**11. (major) RL-3 (MUST) is only partly covered, and the gym reset path discards the state it
restores.**
- Source: `exploration.md` RL-3 ("Reset restores a captured initialised state and re-seeds without
  rebuilding"; acceptance: reset well under 10% of construction, identical observations);
  `dl-i08-architecture.results`, `engine_performance`.
- Evidence: `DwellingGymEnv.reset` calls `load_state(initial_snapshot)` and then
  `reset_with_seed(seed)` (`gym_env.py:546-548`; `vec_env.py:180-182`), and `reset_with_seed`
  rebuilds the dwelling from its config, including warm-up (`py_dwelling.rs:1518-1532`), so the
  loaded state is thrown away. RT-4 makes `reset_with_seed` call `build_with_seed` (no re-parse),
  but `build()` still runs warm-up (IN-2), so each episode reset still repeats warm-up. No entry
  changes the gym reset, adds a re-seed without rebuild, or tests RL-3's acceptance.
- Correction: add an entry (after OR-10 and RT-4): `Dwelling::reseed(&mut self, master_seed)`
  re-derives every RNG stream from the seed without rebuilding; the gym and vec env reset by
  `load_checkpoint` of the post-warm-up snapshot plus `reseed`, and drop the `reset_with_seed`
  call; tests `gym_reset_restores_snapshot_and_reseeds` (two resets with one seed and the same
  actions give bitwise-equal observations) and `gym_reset_runs_no_warmup` (a step counter reads 0
  warm-up steps during reset); record reset time against construction time in the log.

**12. (major) R21's documentation half (MUST) has no entry.**
- Source: `exploration.md` R21 ("MUST (docs): documented time semantics (instant vs. interval,
  weather-to-step mapping, schedule indexing, local-time convention)"); `dl-i08-architecture.time`.
- Text: B-1a documents the weather mapping in `docs/PHYSICS_DECISIONS.md`, and `Sampling` makes
  instant versus interval machine-readable. Nothing documents for users that `Time` is naive local
  standard time labelled at interval start, the `hares:utc_offset_s` metadata, the `Step Count`
  and valid-steps columns, or schedule row indexing (including the leap-day rule of B-1c).
- Correction: OR-7 (and B-1c for the leap-day rule) adds a "Time" section to `docs/python.md`
  stating those conventions, with a doc test that runs its example (the `Time` of the first row
  equals `start_time`'s wall clock).

**13. (major) R15 is neither covered nor justified.**
- Source: `exploration.md` R15 (SHOULD: "A documented Python API stability policy, including the
  package-name decision"; acceptance: a changelog section listing every added, changed or removed
  callable); the task rule that an uncovered requirement is justified by the decision log's purpose
  and scope.
- Text: PY-7 renames the package and PY-2, PY-8, PY-9, OR-3 remove or change most Python
  callables, but no entry writes the list a Python user needs, and no document says why R15 is
  not built.
- Correction: add to PY-9 (the last Python surface row that removes callables) a
  `docs/python.md` "Changes in this release" section listing every added, changed and removed
  callable and keyword, generated from the stub diff between `ebdb1a17` and the entry's commit,
  with `stub_changes_are_listed` (every name removed from or added to `_hares.pyi` appears in the
  section). Or state in `plan.md` §2.10 why the decision log excludes it.

**14. (major) The identifier and reviewer rules in `plan.md` §3 contradict the documents
themselves.**
- Source: `dl-i08-process.writing` ("no planning IDs ... in these documents"), `plan_form` ("Each
  entry: ID"); `plan.md` §3 Writing ("no reference to reviewers or review rounds ... in these
  documents ... in these documents only the ledger's own entry IDs appear").
- Text: the ledger cites review IDs ("Fixes V12 ... and V26", "Fixes V16, V17 and V41", "Fixes V3"),
  requirement IDs ("V15; RL-5", "V18; R14", "R3, X-3, RS-1"), `context.md` item numbers, decision
  keys and issue numbers; `plan.md` §1 and §3 themselves speak of "a fresh, independent round of
  review" and "reviewer". Requirement IDs collide with ledger IDs (exploration PY-1 to PY-14 and
  FL-1 to FL-11 against ledger PY-1 to PY-11 and FL-1 to FL-4), so "PY-5" or "FL-1" is ambiguous
  in any cross-document table, including this audit.
- Correction: rewrite the §3 Writing sentence to what the documents need and the decision log
  allows: planning documents may cite the ledger's entry IDs and, as cross-references, the source
  documents' identifiers (review V-IDs, requirement IDs, `context.md` item numbers, decision keys);
  nothing that ships may. Drop "no reference to reviewers or review rounds ... in these documents"
  (the readiness and independence rules require both words). In ledger text, write requirement IDs
  with their source ("requirement PY-1", "requirement FL-1") wherever a ledger ID with the same
  spelling exists.

**15. (major) `review.md` states a fleet premise the ledger has shown to be false.**
- Source: `dl-i08-process.documents` ("current, correct content").
- Text: `review.md` Fleet reasoning: "Fleet aggregation today folds each home's result as it
  completes, in completion order ... this is the mechanism behind the determinism test being
  vacuous, V30". `04-fleet.md` "Facts every entry below relies on, at HEAD": `simulate_parallel`
  collects in entry order and `aggregate` runs only after every home finished; V30's actual
  mechanism is setting `RAYON_NUM_THREADS` after the pool exists and comparing sorted totals. The
  rejected-alternative paragraph repeats the wrong premise.
- Correction: rewrite the Fleet reasoning in `review.md` from the `04-fleet.md` facts: every home's
  full frame is collected before aggregation (memory grows with homes), the fold order is entry
  order today, and the new design keeps entry order while folding in fixed chunks so memory is
  bounded; V30 is vacuous for the reasons in its own row.

**16. (major) `exploration.md` cites a document outside the initiative.**
- Source: the self-containment check of this audit; `dl-i08-process.documents`.
- Text: `exploration.md` §4: "Per `needs-hares-users.md` section 6.3, cross-checked against
  `decision-log.md`". No such file exists in the initiative directory.
- Correction: replace with "Cross-checked against `decision-log.md`:" and keep the list, which
  already states each item's basis.

### Minors

17. (minor) No test counts factorizations, although `engine_performance` says "allocation and
    factorization counts are tests". After SL-7 no per-step factorization remains, which SL-7's
    grep shows only structurally. Add to RT-3 `thermal_solver_step_performs_no_factorization` (a
    `#[cfg(test)]` counter on the one LU helper, zero over 100 steps) or state in §2.9 that SL-7's
    removal discharges the factorization-count requirement.
18. (minor) V21 is recorded stale: `review.md` says "A regression test is still owed", but
    `init_errors_on_malformed_charging_strategy_override` exists (`ev/tests.rs:8062`). Update the
    V21 row, and add V21 to the `11-other-defects.md` preamble next to V31 as needing no entry.
19. (minor) PY-1's default `arrow(rechunk=True)` concatenates on every call, so repeated reads
    re-copy O(rows), against requirement PY-11 and context item 8. Cache the rechunked batch on
    `ReportResult` (built once, shared) and add `second_export_does_not_recopy` (the two exported
    tables share buffers).
20. (minor) Requirement PY-8's acceptance ("An unknown name raises and names close matches") is
    not met: OR-2's error lists the unknown names only. Add the three nearest catalogue names by
    edit distance to the error and to `unknown_selection_name_errors_listing_unknown_names`.
21. (minor) PR-4's `finish > Duration::ZERO` is a timing assertion (`feedback_deterministic_tests`)
    and can read zero on a coarse clock. Assert structurally that the finish phase is recorded
    (`finish` is `Some` or the phase counter incremented once) and record the duration.
22. (minor) IN-5's test asserts "21.111 C", which 70 F is not exactly, and the entry does not say
    the conversion uses `uom` (`feedback_use_uom`). State the expected value as the `uom`
    conversion of 70 F and compare to it on bits.
23. (minor) OR-11 leaves its per-site table "filled in below during implementation", and RT-3 says
    "Any other allocation the counter reports ... is removed the same way; the acceptance test
    decides". Both are ready-to-execute gaps (`plan_form`); run OR-11's census now and fill the
    table, and name RT-3's remaining allocation sites or state that none remain beyond the four
    listed.
24. (minor) `frame_starts_at_start_time_with_exact_row_count` is an acceptance test of both OR-7
    and B-2. Keep it in OR-7; in B-2 say it is re-run.
25. (minor) BR-8 deletes eleven ignored tests without showing that none fails when run.
    `feedback_no_ignore_tests` forbids deleting a failing test. Add to BR-8's verification:
    `cargo nextest run --workspace --all-features --run-ignored only` passes before the deletion,
    recorded in the log.
26. (minor) SL-1, SL-4, SL-5 and SL-6 have no acceptance test and CN-12 has a commit with none.
    Name the check for each (for example SL-1: the rewritten comment is reviewed against RT-1's
    `telemetry_clone_from_same_keys_allocates_nothing`; CN-12: `surrogate_trial_reproduces_report`
    re-runs the command on one premise and matches the recorded table row).
27. (minor) `plan.md` §2.10 closes with filler ("The overall bar for every entry below is the
    same: solid, robust, ready to execute ..."). Delete the sentence; keep the pointer to
    `08-consumer.md`.
28. (minor) `01-baseline-repair.md` says "Latest releases on crates.io when this plan was written"
    and "maturin 1.15.0 is the latest release at plan time". State the date or commit of the
    check instead ("at 2026-09-29").
29. (minor) M-3 item 5 records memory growth only as a baseline. No entry records the after value
    for requirements R18, RL-1, RL-2 or X-4. Add the same three measurements to OR-6's and RT-4's
    Measurement lines.
30. (minor) SL-8's `git rm -r --cached` and the untracking of `.shipwright/constitution.md` change
    the index of a shared tree and are not listed as operator checkpoints
    (`feedback_no_destructive_git`). Add them to `plan.md` §3's checkpoint list.
31. (minor) OR-3's commit message carries `(HARES#28)`. State in `plan.md` §3 Writing whether a
    public GitHub issue reference is allowed in commit messages (it is not a planning ID, but
    `feedback_no_ticket_refs_in_comments` and SL-2's census treat ticket references as slop).
32. (minor) `review.md` New capability says `SteppableFleet` "loading remains a typed 'not
    rewindable' error, documented", while `plan.md` §2.10 says "`SteppableFleet` gains no save or
    restore method now". Make them agree, and document in `docs/python.md` that fleet stepping
    does not rewind (requirement R25's acceptance).
33. (minor) The destinations decision says files are "renamed on finish"; §2.2 publishes with
    `hard_link` plus removal when `overwrite` is false. Add one sentence to §2.2 that this is the
    no-clobber form of the rename the decision requires.
34. (minor) OR-10's `TariffSnapshot` stores each period bound as a UTC instant, against
    `feedback_local_time_only`. Store the naive local time and the zone name instead, or state why
    the tariff's `DateTime<Tz>` needs the instant.
35. (minor) `plan.md` §2.7 shows `DefaultsSource.base: Option<PathBuf>` with a comment that B-10
    makes it required. The design frame should show the final contract and say RT-4 introduces it
    optional until B-10 (`feedback_no_backward_compat`).
36. (minor) Two rejections live only in verification files, not in the plan: requirement FL-9
    ("`SteppableFleet` without full telemetry", satisfied by construction after OR-3 and OR-6) and
    the `solver_builder.rs:1317,1345` `HashMap` iteration (does not reach the numbers). Add one
    sentence for each to FL-3 and RT-5.
37. (minor) Context item 18 names a precondition ("the probe equals nominal only when no ABSOLUTE
    factor is free"). CN-6 states a different one (slot purity) and never mentions ABSOLUTE
    factors. Add a sentence that CN-6 reuses `fx[0]`, the same vector as the standalone call, and
    never substitutes the reachability probe's run, so the ABSOLUTE-factor caveat does not apply.
38. (minor) Requirements DV-6 (bench groups: no output, in memory, CSV, Parquet), R17 (8-thread
    throughput) and FL-10 (a named bench target) are covered only by measurements with no named
    bench. Name the bench file and groups in OR-3, RT-5 and FL-1.
39. (minor) COULD requirements R24, FL-11, RS-9 and PY-14 are neither covered nor justified. Add a
    line each to `plan.md` §2.10 giving the decision-log basis for not building them now, or an
    entry.
40. (minor) RT-4's Measurement cites "`context.md`: 45 to 48 ms per parse"; `context.md` holds no
    such figure. Cite M-3 or remove the figure.
41. (minor) M-4a triggers the new `reproducibility.yml` on pushes to `feat/output-results`, a
    branch name that is wrong once merged. Trigger on `workflow_dispatch` and `pull_request`.
42. (minor) `L1-integration.md` leaves a found item for "any later documentation change" (a file
    tracked under the ignored `docs/eplus/`). Deferral is not allowed
    (`feedback_no_broken_windows`); add it to SL-8 (untrack it, or drop the ignore rule for that
    file) or record why it stays.
43. (minor) FL-1 bounds memory by chunk size, but at `Timestep` frequency each live home result is
    its full selected-column frame. State this bound in §2.4 and record bytes per home result in
    FL-1's measurement, so "never retain full frames" is read as "never all homes' frames".
44. (minor) `plan.md` §3 and M-1 say an entry that changes how results are obtained updates the
    `frame_golden` adapter "in its own commit", while OR-3 migrates it "in this commit" and every
    commit must compile. Say "in the entry's commit".
45. (minor) Context item 16 asks that `contract()` "asserts the schema version"; CN-1 resolves
    required names and units and only records the catalogue digest. Say that the name-and-unit
    resolution is the assertion, or assert the digest against a pinned value.
46. (minor) The `[pytest]` skip census keys on file and line, so any edit above a skip in a test
    file changes the census. Key the census on node ID and reason (`-rs` output with line numbers
    stripped by the comparison).

---

## Appendix A: decision log

| Decision | Where in the plan | Status | Finding |
|---|---|---|---|
| dl-dcbe0baa (flow recipe) | n/a | n/a | |
| dl-i08-criteria | whole ledger | partial | 3, 5, 10 |
| process.purpose | §1 | covered | |
| process.found_defects | §1, `11-other-defects.md` | partial (untracked found defects) | 10, 11, 42 |
| process.plan_form | every entry | partial | 7, 9, 23, 26 |
| process.readiness | §1, `plan-critique.md` | covered | |
| process.independence | §3 | covered | |
| process.documents | all documents | contradicted | 2, 15, 16 |
| process.writing | §3 `[writing]`, SL-2, SL-3 | partial (no em dash or tell found; draft history; ID rule) | 2, 14, 27, 28 |
| process.branch | §3 Task 0, closing sequence | covered | |
| process.scope | §2.10, RT-3, SL-7 | covered | 42 |
| architecture.results | §2.1, OR-6, OR-9, PY-9 | covered | 7 |
| architecture.output_model | §2.2, OR-1, OR-2, OR-8, OR-13 | covered | 20 |
| architecture.destinations | §2.2, OR-3, OR-4, OR-14, PY-10 | covered | 33 |
| architecture.python_export | PY-1 | covered | 19 |
| architecture.config | §2.7, PY-8, PY-2, OD-2, OR-3 | contradicted (removed keywords) | 1, 8 |
| architecture.inputs | §2.7, RT-4, RT-6, IN-1 | partial (cross-blueprint sharing) | 4 |
| architecture.new_capability | OR-13, RT-6, IN-2, OR-12, OR-15, IN-4, OR-16, IN-3, §2.10, PY-11 | covered | 7 |
| architecture.health | §2.3, OR-5, OR-11, OR-12, RT-5 | covered | |
| architecture.time | §2.5, OR-7, B-6 | covered (docs half of R21 missing) | 12, 34 |
| architecture.weather | `09` weather section, B-1a, B-1b, B-1c | covered | |
| architecture.aggregation | §2.6, OR-13, FL-1, B-11 | covered | |
| architecture.fleet | §2.4, FL-1 to FL-4, B-12 | covered | 43 |
| architecture.numeric_fixes | §2.8, B-1a to B-13, OD-9 | partial (deltas without fixtures) | 6 |
| architecture.precedence | OD-2, B-13 | partial (vehicles) | 10 |
| architecture.reproducibility | RT-5, FL-1, B-8, M-4a, M-4b | covered | |
| architecture.package | §2.7, PY-7, OD-1, CN-13, CN-15 | covered | |
| architecture.engine_performance | §2.9, RT-1, RT-2, RT-3, RT-5, SL-7 | partial | 3, 5, 17 |
| architecture.dependencies | §2.7, BR-6a to BR-6d, `[advisories]` | covered | 28 |
| architecture.metrics_optional | §2.2, OR-8, CN-7, B-7 | covered | |
| architecture.weather_edge_cases | B-1a, B-1c (EnergyPlus citations) | covered | |
| architecture.ochre_compat | IN-6 | covered | |
| architecture.defaults_path | B-10 | covered | 1 |
| architecture.calibration_r_factor | IN-7, CN-3 | covered | |
| architecture.consumer_gate | `08-consumer.md` preamble and gate, CN-16, CN-15 | covered | |
| architecture.profiler_scope | PR-1 to PR-6 | covered | 21 |
| architecture.golden_storage | M-1 | covered | |
| architecture.setpoint_sources | IN-5, OD-9, RT-6, CN-1 | covered | 6, 22 |
| architecture.battery_control | OD-8 | covered | |
| architecture.rl_observation_allocation | RT-7 | covered | |
| architecture.cross_platform_measurement | M-4a, M-4b | covered | 41 |
| architecture.consumer_inert_override_probe | CN-1, OD-2 | covered | |
| architecture.input_schema_scope | IN-3 | covered | |
| architecture.public_planning_docs | SL-8, PY-7 | covered | 30 |
| architecture.slop_scope | SL-3, §3 | covered | |
| architecture.stray_outputs | Task 0, BR-7, `[stray]` | covered | |
| architecture.consumer | `00-schedule.md`, `08-consumer.md`, CN-4 | covered | |

## Appendix B: binding rules (`feedback_*.md`, 45)

| Rule | Status | Finding |
|---|---|---|
| actor_telemetry | covered (OR-1 actor declarations, PY-11) | |
| always_review | covered (§3 per-entry review) | |
| ashrae_not_ochre | covered (`09` EnergyPlus basis) | |
| best_physics | covered (`docs/PHYSICS_DECISIONS.md`) | 6 |
| code_quality | partial | 3, 19 |
| custom_domains_perf | contradicted | 3 |
| design_for_all_users | partial | 5 |
| deterministic_tests | partial | 21 |
| equipment_self_contained | covered | |
| fix_all_review_findings | covered (§3 Independence) | |
| fix_existing_not_new_features | covered (built-now list is the operator's) | |
| hot_loop_minimal | partial | 3 |
| local_time_only | partial | 34 |
| max_5_agents | n/a to the ledger | |
| no_ai_slop_public | partial | 27, 31 |
| no_backward_compat | partial | 35 |
| no_blanket_kill | n/a | |
| no_broken_windows | partial | 10, 11, 42 |
| no_code_fixes_from_reviews | n/a | |
| no_destructive_git | partial | 30 |
| no_git_checkout | covered (CN-15 nested checkout is an operator checkpoint) | |
| no_git_revert | covered (§3) | |
| no_git_stash | covered (§3) | |
| no_ignore_tests | partial | 25 |
| no_legacy_shims | covered (IN-6, PY-7) | |
| no_parallel_same_files | covered (rows are sequential) | |
| no_sed | n/a to the ledger | |
| no_self_review | covered (§3) | |
| no_silent_defaults | partial | 1, 10 |
| no_ticket_refs_in_comments | covered (SL-2, SL-3, `[writing]`) | 31 |
| no_timelines | covered | |
| no_touch_thermal_solver | superseded by `process.scope`, cited in `06` preamble | |
| no_useless_comments | covered (§3 slop rule) | |
| not_ochre_parity_target | covered | |
| ochre_parity_rules | partial (IN-5 lacks the physics note) | 6 |
| opencode_stateless | n/a | |
| pay_for_what_you_need | covered | |
| shipwright_files_authoritative | contradicted | 2, 15 |
| si_units | partial | 10 |
| si_units_strict | covered | |
| test_coverage_reporting | n/a | |
| test_quality | partial | 22 |
| tests_every_ticket | covered (block after every row) | |
| unit_boundary | covered | |
| use_uom | partial | 22 |

## Appendix C: `context.md` items 1 to 21

| # | Item | Entry | Status | Finding |
|---|---|---|---|---|
| 1 | Telemetry clone, `HashSet` rebuild | RT-1, SL-1 | covered | 3 |
| 2 | Exogenous forcing reuse | RT-2 | partial (not shared across blueprints) | 4 |
| 3 | Parse once per home | RT-4 | partial (within one blueprint only) | 4 |
| 4 | Envelope solar/incidence, LU, `m_scratch`, slot map | RT-2, RT-3, SL-7 | partial (per-surface dropped a priori) | 5, 17 |
| 5 | Thermal input preparation | RT-3 | covered | 23 |
| 6 | Threads, per-run statics | RT-5, CN-10 | partial (CN-10's shared weather) | 4 |
| 7 | Unused checkpoint | OR-9 | covered | |
| 8 | Output pipeline cost | OR-2, OR-3, PY-1, PY-9, PR-4 | covered | 19 |
| 9 | HARES#28 | OR-3 | covered | |
| 10 | Health only in `observe` builds | OR-5, OR-11 | covered | |
| 11 | Stepped runs lose output | OR-6, PY-9 | covered | |
| 12 | Diagnostics CSV | OR-4 | covered | |
| 13 | Fleet memory and status | FL-1, FL-2 | covered | 43 |
| 14 | Time strings | OR-7, B-12 | covered | |
| 15 | Profiler (HARES#31) | PR-1 to PR-6 | covered | |
| 16 | Consumer call sites, `contract()` | CN-1 | covered | 45 |
| 17 | Worker pools | CN-5 | covered | |
| 18 | Duplicate parent runs | CN-6 | covered | 37 |
| 19 | Fit path computes only what it reads | CN-7, CN-8 | covered | |
| 20 | Per-evaluation provenance and replay | CN-11 | covered | |
| 21 | Surrogate search trial | CN-12 | covered | 26 |

## Appendix D: `review.md` V1 to V50

| V | Entry | Status | V | Entry | Status |
|---|---|---|---|---|---|
| V1 | OR-5 | covered | V26 | OR-6 | covered |
| V2 | OR-5 (counter deleted; NaN fails) | covered | V27 | RT-1 | covered |
| V3 | OD-1, PY-7 | covered | V28 | OR-10 | covered |
| V4 | B-1a, B-1b | covered | V29 | OR-8, B-7 | covered |
| V5 | B-1c (typical wraps as EnergyPlus; actual errors) | covered | V30 | FL-1, RT-5 | covered |
| V6 | B-4 | covered (no billing golden, 6) | V31 | none; existing tests cited in `11` preamble | explained |
| V7 | OR-9 | covered | V32 | PY-3 | covered |
| V8 | B-8 | covered | V33 | PY-4 | covered |
| V9 | OR-1 seam, B-5 | covered | V34 | OR-3 | covered |
| V10 | B-5 | covered | V35 | OD-2 site 2 | covered |
| V11 | OD-2 site 1 | covered | V36 | OD-2 site 8 (ambiguity unreachable for a fixed offset) | covered |
| V12 | OR-6 | covered | V37 | OD-2 sites 3 and 4 | covered |
| V13 | OR-4 | covered | V38 | OD-2 site 5 | covered |
| V14 | OR-1 `nullable`, B-6 | covered | V39 | OD-2 site 6, IN-5 | covered (delta, 6) |
| V15 | PY-5 | covered | V40 | OD-2 site 7, OD-8 | covered (vehicles, 10) |
| V16 | FL-2 | covered | V41 | FL-3 | covered |
| V17 | FL-1, FL-2 | covered | V42 | B-9 | covered (no golden, 6) |
| V18 | PY-6 | covered | V43 | OD-2 site 9 | covered |
| V19 | B-10 | covered | V44 | OD-7 | covered |
| V20 | PY-2 | covered (renamed keywords, 1) | V45 | PY-1, OR-3 | covered |
| V21 | refuted; existing test `ev/tests.rs:8062` | explained in no document (18) | V46 | OD-3 | covered |
| V22 | B-2 | covered | V47 | refuted | n/a |
| V23 | B-3 | covered | V48 | design (catalogue/row plan) | n/a |
| V24 | B-2 | covered | V49 | planning artefact | archeology (2) |
| V25 | OR-5, RT-5 | covered | V50 | planning artefact | archeology (2) |

## Appendix E: requirements in `exploration.md`

Calibration consumer (R):

| R | Pri | Entry | Status | Finding |
|---|---|---|---|---|
| R1 | MUST | OR-3, OR-5 | covered | |
| R2 | SHOULD | OR-2 | covered | |
| R3 | MUST | OR-5, OR-12, PY-9 | covered | |
| R4 | MUST | OR-1, PY-4, PY-6, B-5 | covered (metrics not in the catalogue; digest detects renames) | |
| R5 | MUST | OD-2, IN-3, existing EV test | covered | 18 |
| R6 | MUST | §2.7, RT-6, IN-2 | covered (AFUE edit tested; HSPF-to-EIR not tested) | |
| R7 | SHOULD | RT-4 content inputs, CN-3 | covered (engine side) | 4 |
| R8 | SHOULD | IN-3 | covered | |
| R9 | SHOULD | IN-2 | covered | |
| R10 | MUST | OD-1, PY-9, §2.10 (no taxonomy, justified) | covered | |
| R11 | MUST | PY-2, B-10 | partial | 1 |
| R12 | MUST | RT-5, B-8 | covered | |
| R13 | SHOULD | `09` preamble (commit body states the delta) | covered | |
| R14 | MUST | PY-6 | covered | |
| R15 | SHOULD | none | missing, unjustified | 13 |
| R16 | SHOULD | PR-1, PR-2, M-3 | covered | |
| R17 | SHOULD | PY-3, RT-5, CN-10 | partial | 4, 38 |
| R18 | SHOULD | M-3 item 5 (baseline only) | partial | 29 |
| R19 | COULD | §2.10 seam | justified (not now) | |
| R20 | SHOULD | OR-7, B-2 | covered | |
| R21 | MUST docs / SHOULD API | IN-4 (API); docs partial | partial | 12 |
| R22 | SHOULD | OR-16, CN-11 | covered | 7 |
| R23 | SHOULD | OD-2 site 7, OD-8 | partial (PV and battery ranges) | 10 |
| R24 | COULD | none | missing, unjustified | 39 |
| R25 | COULD | OR-10, §2.10 | partial (fleet rewind not documented) | 32 |

Other user classes:

| Req | Entry | Status | Finding | Req | Entry | Status | Finding |
|---|---|---|---|---|---|---|---|
| PY-1 | OR-6, PY-9 | covered | | FL-1 | FL-1 | covered | 43 |
| PY-2 | OR-3, OR-4 | covered | | FL-2 | FL-2 | covered | |
| PY-3 | §2.2, OR-3 | covered | | FL-3 | FL-2 | covered | |
| PY-4 | OR-7, B-12 | covered | | FL-4 | FL-2 | covered | |
| PY-5 | B-6 | covered | | FL-5 | FL-1 | covered | |
| PY-6 | PY-2, PY-8, PY-9 stubs | partial | 8 | FL-6 | B-9 | covered | |
| PY-7 | OR-8, B-7 | covered | | FL-7 | FL-1 | covered | |
| PY-8 | OR-2 | partial | 20 | FL-8 | OR-7 | covered | |
| PY-9 | OR-13 | covered | | FL-9 | OR-3, OR-6 by construction | justified only in a verification file | 36 |
| PY-10 | PY-4 | covered | | FL-10 | FL-1 measurement | partial | 38 |
| PY-11 | PY-1 | partial | 19 | FL-11 | none | unjustified (COULD) | 39 |
| PY-12 | OR-15 | covered | | RS-1 | OR-6, OR-8 | covered | |
| PY-13 | OR-16 | covered | | RS-2 | OR-3, OR-4 | covered | |
| PY-14 | none | unjustified (COULD) | 39 | RS-3 | OD-2, PY-8 | covered | |
| RL-1 | OR-3, M-3 | partial | 29 | RS-4 | OR-6 | covered | |
| RL-2 | OR-6, M-3 | partial | 29 | RS-5 | §2.1 | covered | |
| RL-3 | RT-4 | partial (MUST) | 11 | RS-6 | OR-5 | covered | |
| RL-4 | OR-10 | covered | | RS-7 | OR-6 | covered | |
| RL-5 | PY-5 | covered | | RS-8 | RT-4 | covered | |
| RL-6 | RT-7 | covered | | RS-9 | none (PR-1 deletes the example) | unjustified (COULD) | 39 |
| RL-7 | BR-7, OR-3 | covered | | DV-1 | OR-5, RT-5 | covered | |
| RL-8 | RT-7 | partial (COULD) | | DV-2 | RT-5, FL-1, B-8, OR-10 | covered | |
| X-1 | OR-8 | covered | | DV-3 | OR-4 | covered | |
| X-2 | OR-3 | covered | | DV-4 | BR-7, `[stray]` | covered | |
| X-3 | OR-6, PY-9 | covered | | DV-5 | OR-4 | covered | |
| X-4 | OR-6 | partial (no after measurement) | 29 | DV-6 | OR-3 measurement | partial | 38 |
| X-5 | OR-3, PY-1 | covered | | DV-7 | PR-1, PR-6 | covered | |
| X-6 | OR-10 | covered | | DV-8 | OR-1 | covered | |
| X-7 | PY-8 | partial | 8 | DV-9 | OD-2 site 9 | covered | |
| X-8 | FL-2 | covered | | X-10 | OR-8 measurement | covered | |
| X-9 | OR-2, OR-13 | covered | | X-11 | PY-11 | covered | |
| X-12 | IN-6 (preset leaves the core by decision) | justified | | | | | |

## Appendix F: `verify-L1-e.md` findings, checked against the current text

| # | Resolution claimed | Present in the text | Status | Finding |
|---|---|---|---|---|
| 1 | PY-7, OD-1, CN-13, CN-15 | yes | resolved | |
| 2 | B-1a/B-1b/B-1c, IN-6, CN-14 | yes | resolved | |
| 3 | OR-8/B-7 and FL-4/B-12 split; B0 on Task 0 head; consumer golden | yes | resolved | |
| 4 | OR-5 table, latch, OR-12, RT-5 | yes | resolved | |
| 5 | IN-2, OR-15, IN-4, OR-16, IN-3, RT-4, §2.10 | yes; cross-blueprint handles missing | partial | 4 |
| 6 | OR-13, OR-2 tests | yes | resolved | |
| 7 | OR-14, PY-10 | yes | resolved | |
| 8 | PY-8, §2.7 table, OR-3 | yes; table incomplete | partial | 1 |
| 9 | RT-3 slot map, SL-7, RT-2 measurement | yes; per-surface dropped a priori | partial | 5, 17 |
| 10 | FL-1 `n_threads`, RT-5 spawned processes, M-4a/b | yes (the resolution-c quote about RT-5's measurement is stale; the item moved to M-4) | resolved | |
| 11 | PY-5 | yes | resolved | |
| 12 | OD-2 site 8 | yes | resolved | |
| 13 | BR-8 | yes | resolved | 25 |
| 14 | PY-3 hook, FL-1 peak count, B-2 behaviour tests | yes | resolved | |
| 15 | `[stray]`, `[python-build]`, `[pytest]`, census | yes | resolved | 46 |
| 16 | history and dangling cites removed | plan and ledger yes; `review.md` V49/V50 and RT-4's context cite no | partial | 2, 40 |
| 17 | PY-9, OR-6 engine | yes | resolved | |
| 18 | Task 0 fast-forward, closing sequence | yes | resolved | |
| 19 | `08-consumer.md` mechanism, gate, CN-9, CN-15 | yes | resolved | |
| 20 | `metrics: bool` (decision `metrics_optional`) | yes | resolved | |
| 21 | §2.3 policy fields; B1 attribution | yes | resolved | |
| 22 | RT-4 defaults, PR-1, CN-1, CN-5 | yes | resolved | 45 |
| 23 | §2.1 types, warnings cursor, OR-5 | yes | resolved | |
| 24 | §2.7 `ResolvedModel`, RT-6 | yes | resolved | |
| 25 | OR-1 item 4 and tests | yes | resolved | |
| 26 | B-2 defaults 25, loads, `WarmupOutcome`, OR-7 test | yes | resolved | 24 |
| 27 | B-13 evidence and rule | yes | resolved | |
| 28 | B-8 envelope test | yes | resolved | |
| 29 | B-10, M-1 defaults digest | yes | resolved | |
| 30 | B-9 unresolved weather error | yes | resolved | 6 |
| 31 | OR-10 `step_index`, warnings | yes | resolved | |
| 32 | §2.2 publish rules, OR-3 tests, FL-1 duplicates | yes | resolved | |
| 33 | FL-1, FL-2, FL-4 | yes; FL-9 reason only in a verification file | resolved | 36, 43 |
| 34 | RT-1 domains, RT-5 census, RT-7 | allocation yes; indexed slots no | partial | 3, 36 |
| 35 | M-3 item 5, RT-4 `reset_with_seed` | baseline only; gym reset unchanged | partial | 11, 29 |
| 36 | M-1 products, adapter, row criterion, consumer fixture, ordering | yes; no fixture produces `billing` | partial | 6 |
| 37 | each entry decided | yes except OR-11 table and RT-3 open item | partial | 23 |
| 38 | PY-6 rerun rules, synthetic `.git` tests | yes | resolved | |
| 39 | opening sequence, `[test-profiling]` | yes | resolved | |
| 40 | `09` preamble | yes; IN-5 and OD-2 site 6 outside it | partial | 6 |
| 41 | OR-3, OR-7, PY-9, PY-11, PY-4, §2.5 | yes | resolved | |
| 42 | `Selection::Diagnostics`, OR-4 test | yes | resolved | |
| 43 | BR-6b no ignores, CI job | yes | resolved | |
| 44 | schedule rows | yes | resolved | |
| 45 | `Arc<RunResult>` | yes | resolved | |
| 46 | RT-2 roof coverage outside the key | yes | resolved | |
| 47 | `06` preamble cites `process.scope` | yes | resolved | |
| 48 | FL-4 `HeterogeneousTimeGrid` | yes | resolved | |
| 49 | OD-7, OD-2 site 7 test | yes | resolved | |
| 50 | B-5 classifier list | yes | resolved | |
| 51 | PY-1 deletions | yes | resolved | |
| 52 | BR-4, BR-6b, BR-7 CI | yes | resolved | |
| 53 | OR-2 validity bitset | yes | resolved | |
| 54 | CN-1 probe and docstring | yes | resolved | |
| 55 | rejected by resolution a; `09` preamble commit-body rule covers R13 | yes | resolved | |
| 56 | §2.10 taxonomy sentence | yes | resolved | |

## Appendix G: schedule (`ledger/00-schedule.md`)

| Check | Result |
|---|---|
| Every ledger entry appears exactly once | yes: 116 entry IDs in `ledger/01` to `12` against 116 in the schedule. BR-5 appears as "OR-5 (closes BR-5)", SL-1/SL-4/SL-5/SL-6 as "(fixes ...)" in the rows that fix them, SL-2 and SL-3 as standing rules in the preamble; Task 0 and the three push rows are operator rows. No duplicates. |
| Stated dependencies respected by row order | yes, except PY-9 (row 54) listing OR-16 (row 69) conditionally (finding 7). The other apparent inversions are "must precede" statements, not dependencies. |
| Opening sequence equals `plan.md` §3 | yes |
| Numeric changes only in the numeric block | no: IN-5 (row 64) and OD-2 site 6 (row 31) move numbers outside it (finding 6) |
| Consumer checks consistent with `08-consumer.md` preamble | yes (B0 through row 27 with the CN-0/CN-16 consumer, rows 28 to 33 with the CN-13 consumer, golden from row 34) |

## Appendix H: self-containment and writing sweep

| Check | Result |
|---|---|
| Em dash or en dash in `plan.md`, `ledger/`, `decision-log.md`, `context.md`, `exploration.md`, `review.md`, `initiative.md`, `plan-critique.md` | none |
| The tells `dl-i08-process.writing` names, agent or LLM references | none (the `AGENTS.md` hits are a consumer file name) |
| Draft or round history | `review.md` V49, V50; `initiative.md` status (finding 2); "when this plan was written", "at plan time" (finding 28) |
| References outside the initiative directory | `exploration.md` `needs-hares-users.md` (finding 16). All others are the two code repositories (including `vendors/` submodules and the `.shipwright/` tree inside HARES), engine-under-test worktrees of the HARES repository under `~/src/hares-eut/`, the runtime cache path `~/.cache/hares`, and two public OpenStudio-HPXML URLs. |
| Planning IDs in anything that ships | commit messages in the schedule carry none; OR-3 carries a public issue number (finding 31) |
