# Verification L4-b: plan.md section 2, ledger 03 (uncommitted entries), 05, 12

Scope: `plan.md` section 2; `ledger/03-output-reporting.md` every entry not
yet committed (all but OR-5), OR-11 included with its recorded fluid-loop
resolution; `ledger/05-python-surface.md`; `ledger/12-inputs.md`. Code read
at HEAD `afa9a071` through `git show`/`git grep` (the uncommitted OR-11 tree
was not used as evidence), and at the commits the entries cite
(`ebdb1a17`, `872cf289`, `21d90588`). Library claims checked against
`~/.cargo/registry` sources of the locked versions (pyo3 0.29.3,
pyo3-arrow 0.19.0, arrow-select 59.3.0, serde 1.0.229 / serde_core,
serde_derive 1.0.229, serde_json 1.0.151, uom 0.38.0). Order checked
against `ledger/00-schedule.md`; decisions against all of `decision-log.md`.

Counts: 0 blockers, 11 majors, 21 minors.

## Findings

### Majors

1. **Major. OR-11 (row 19): the "always-on `Warning`" dispositions use a
   type and a channel that do not exist at row 19.**
   Evidence: the hares-equipment rows `battery/mod.rs:892-945`,
   `:1009-1019`, `ev/mod.rs:934-956`, `pv/mod.rs:478-501`, `:840-843`; the
   hares-io rows `resolve_hvac.rs:3344-3373` ("construction `Warning`s on
   the run"), `output/metrics.rs:993-997`, `schedule_resolve.rs:459-469`;
   the hares-tariff `urdb.rs` rows. `Warning` is a hares-core type (plan
   section 2.1) created by OR-6 (row 43); today `WarningLog` stores
   `String`s (`dwelling/warnings.rs:17`). No crate below hares-core has a
   channel to the dwelling's warning log at HEAD (`git grep` finds
   `push_warning` only in hares-core and hares-python), and the only sink
   the plan defines is RT-5's `validate_core_contract` sink (row 70). The
   in-progress tree shows the gap: `urdb.rs` was turned into ungated
   `tracing::warn!`, not a run warning.
   Correction: in OR-11, define the channel for each crate before using it:
   equipment returns init warnings through a new `Equipment` method
   `take_warnings(&mut self) -> Vec<String>` (default empty, hares-equipment
   trait, the dwelling drains it after `init` and after each step into its
   `WarningLog`); hares-io resolvers return their warnings alongside their
   result (as IN-5 does for `inject_schedule_into_specs`); `MetricsCalculator`
   and `urdb::parse` return warnings in their result types. Write each as a
   `String` into today's `WarningLog`; OR-6 converts them to `Warning` when it
   introduces the type. Or reschedule those rows to RT-5 and say so.

2. **Major. OR-11 `schedule_resolve.rs:459-469`: the unmapped-column warning
   fires on every ResStock schedule, including a golden home, and flags a
   column the resolver reads.**
   Evidence: `find_unmapped_csv_columns` (`schedule_resolve.rs:1435-1447`)
   checks only `COLUMN_MAPPINGS`, which lacks `hot_water_fixtures`
   although `inject_water_heater_schedule_columns` reads it (`:748`).
   `tests/fixtures/resstock/2025.1/bldg0176227/in.schedules.csv` (the
   `consumer_shape_900s` home) carries `hot_water_dishwasher`,
   `hot_water_clothes_washer`, `hot_water_fixtures`; bldg0176775 and
   bldg0442669 add `electric_vehicle_charging`,
   `electric_vehicle_discharging`. No code reads `hot_water_dishwasher` or
   `hot_water_clothes_washer` from a CSV (they appear only in
   `hpxml_schedule.rs:25-26`). With an always-on warning, every ResStock run
   carries warnings: RT-5's rule stops the row when a golden home carries a
   warning, and plan section 2.4 makes every such fleet home `Degraded`.
   Correction: in OR-11, (a) make the unmapped check use the set of columns
   the resolver actually reads (add `hot_water_fixtures` and
   `hot_water_mains_temperature` to the mapping or derive the read set from
   the resolver's own lookups); (b) for each column the ResStock 2025.1
   fixtures carry that HARES does not read, decide it in the entry (read, or
   a named "not modelled" table with the reason, listed by IN-3's schema)
   so the warning fires only for truly unknown columns; (c) add a test that
   `consumer_shape_900s` and the bldg0176775 run carry no schedule warning.

3. **Major. OR-1 test `catalogue_queryable_before_run` and PY-4 depend on
   `DwellingBlueprint::catalogue()`, which no entry owns, and at row 36 a
   blueprint cannot produce one without building and warming up.**
   Evidence: plan section 2.7 lists `DwellingBlueprint::catalogue(&self)`
   in the final API; the section 2.7 owners paragraph assigns RT-4, RT-2,
   RT-6, IN-1, IN-2, not `catalogue()`; `grep 'catalogue()'` over the plan
   finds only PY-4's Python accessor. The catalogue needs instantiated
   equipment descriptors and actor declarations; at HEAD
   `DwellingBlueprint::build(self)` consumes the blueprint
   (`dwelling/blueprint.rs:225`) and construction runs
   `run_warmup_converged(0.5, 25)` (`dwelling/mod.rs:2935`). The
   assembly/warm-up split is IN-2's (row 66). The Rust `Dwelling::catalogue()`
   that PY-4's `Dwelling.catalogue()` wraps is not defined either.
   Correction: make OR-1 own `Dwelling::catalogue(&self) -> Arc<Catalogue>`
   and `DwellingBlueprint::catalogue(&self) -> Result<Arc<Catalogue>, HaresError>`,
   and state the mechanism at row 36 (instantiate the equipment and the
   auto-registered actors without warm-up through a factored assembly
   function that IN-2 later reuses), stating that actors a caller adds after
   build are not in a blueprint's catalogue. Or move
   `catalogue_queryable_before_run` and PY-4's blueprint accessor after IN-2
   and name IN-2 as the owner.

4. **Major. OR-1: `VariableKind::Enum { labels: Arc<[Arc<str>]> }` cannot
   appear in the `&'static [VariableDef]` tables OR-1 item 3 specifies.**
   Evidence: OR-1 item 3 ("today's `LEVEL_*_COLUMNS` as
   `&'static [VariableDef]`", "a static per-capability table for
   core-output columns"); the per-capability core-output set includes each
   equipment's `Mode (-)` column (`columns.rs:98,535`), an `Enum`. `Arc` has
   no const constructor, so a `static` holding an `Enum` variable does not
   compile.
   Correction: change the label type in plan section 2.2 to
   `Cow<'static, [Cow<'static, str>]>` (const-constructible as
   `Cow::Borrowed(&[Cow::Borrowed("off"), ...])`, deserialises owned, Python
   actors build `Cow::Owned`), and update section 2.1's serde `rc` list
   accordingly; or keep `Arc` and replace "static tables" with functions
   building the tables once at assembly.

5. **Major. OR-1 does not decide the kind and over-time aggregation of
   stock and cumulative variables; `Energy` forces `Sum`, which sums stocks.**
   Evidence: telemetry declared in `kWh` at `21d90588` includes stocks
   (`CAPACITY_KWH`, `CAPACITY_KWH_RATED`, `DECLARED_CAPACITY_KWH`,
   `IMPLIED_CAPACITY_KWH`, `EBM_ENERGY_KWH`, `EBM_MAX_ENERGY_KWH`,
   `EBM_MIN_ENERGY_KWH`) and a cumulative energy (`DRIVE_SHORTFALL_KWH`,
   "Cumulative EV drive energy", `telemetry_keys.rs:495`); counts include
   run totals (`TOTAL_COMMANDS_PARSED`) and configuration constants
   (`N_SERIES`, `N_PARALLEL`). OR-1 names kWh nowhere in its Quantity list,
   so they default to `Energy` (`Sum`); an `Hourly` report at 900 s would
   report four times a battery's capacity. Plan section 2.2 declares no
   `Last`.
   Correction: in OR-1 and plan section 2.2, classify stocks (capacities,
   stored energy, EBM bounds, configuration counts) as
   `Quantity { across_homes: Sum }` with `Mean` aggregation and
   `Sampling::InstantAtEnd`; for cumulative run totals either publish the
   per-step increment (then `Sum`) or declare `Mean` with the meaning stated
   in the description; pin each in `kinds.json`; extend
   `every_quantity_declares_across_homes_and_power_units_are_power` to
   assert no `kWh` variable whose description says capacity, state or
   cumulative is `Energy`.

6. **Major. OR-6 changes the gym's per-step warning stream although it says
   the contract is kept.**
   Evidence: today `WarningLog::take` drains and resets both entries and the
   dropped count (`dwelling/warnings.rs`), so a gym draining every step
   (`py_gym.rs:475-479`) sees every warning and the 1000-entry bound applies
   per drain. OR-6 keeps warnings for the whole run, bounded at `CAPACITY`,
   and serves `take_warnings()` from a cursor; after the first 1000 warnings
   of a run (one per rejected control signal, plan section 2.3) the gym
   receives nothing more.
   Correction: state in OR-6 that the cursor stream and the run record have
   separate bounds: `take_warnings()` returns every warning pushed since the
   previous call (bounded per drain at `CAPACITY`, with its own dropped
   summary as today), while `RunResult.warnings` keeps the first `CAPACITY`
   of the run plus `warnings_dropped`; add the test
   `gym_drain_sees_warnings_past_run_capacity` (1001+ rejected signals over
   several drains, each drain returns its own).

7. **Major. OR-7 contradicts the fleet entries on the fleet `Time` column and
   leaves the old fleet path's reading of the new home `Time` unspecified.**
   Evidence: OR-7 "The fleet path uses the same representation"; FL-1
   (`04-fleet.md` line 193) and FL-4 keep the fleet `Time` a Utf8 RFC 3339
   UTC `Z` label until B-12, and B-12 says so (`09-numeric-fixes.md`
   B-12). Between OR-7 (row 45) and FL-1 (row 50) today's fleet aggregation
   parses each home's Utf8 `Time` (`aggregation.rs:505`, `parse_timestamp`
   `:444`) and keys buckets by epoch second (`:508`); after OR-7 a home's
   `Time` is a naive local `Timestamp`.
   Correction: replace the sentence with: "The fleet aggregation reads each
   home's `Time` as naive local milliseconds and subtracts
   `hares:utc_offset_s` to recover today's UTC epoch key; its output `Time`
   stays a Utf8 RFC 3339 `Z` string until `09-numeric-fixes.md` B-12", and
   add a test that a two-home fleet's aggregate after OR-7 equals the
   pre-OR-7 aggregate on bits (or name the `fleet_mixed_offset` products).

8. **Major. OR-13 (row 39) runs before OR-7 (row 45) but does not say how its
   period rows are labelled or how its batch feed reads `Time` while `Time`
   is a Utf8 string.**
   Evidence: `Time` is `DataType::Utf8` written with chrono `"%+"`
   (`columns.rs:425`, `mod.rs:7009-7010` at `ebdb1a17`) until OR-7. OR-13
   requires rows "labelled at period start", the batch feed for FL-1, and
   `batch_fed_reducer_equals_row_fed_reducer`; none states the label format
   or that the batch feed parses strings until OR-7.
   Correction: either schedule OR-7 before OR-13 (OR-7's dependency on OR-6
   is only the justification for deleting metadata keys; move that deletion
   into OR-6 or keep it in OR-7 after OR-6) or state in OR-13 that until
   OR-7 the period label is the period start as `DateTime<FixedOffset>` at
   the step's offset formatted `"%+"`, the batch feed parses it with the
   recorder's format, and OR-7 converts both and re-runs OR-13's tests.

9. **Major. OR-10 enables `serde_json/float_roundtrip` for every build, which
   moves numbers for the Python wheel and the consumer, inside a structural
   commit whose consumer check is only `golden`.**
   Evidence: at HEAD only `tools/frame_golden/Cargo.toml:24` enables the
   feature, so `cargo` builds of the workspace that include frame_golden
   unify it, but `maturin` builds of `hares-python` do not. The wheel parses
   URDB and tariff JSON (`py_tariff.rs:134,141,154,707,717`) and checkpoint
   JSON (`checkpoint.rs:159-167`). The consumer at `c71c542` calls
   `ElectricTariff.from_urdb_json` (`calibration/ami2/tariff.py:73,87`) and
   `save_state`/`load_state` (`calibration/cosim.py:74,90,102`,
   `coupling.py:399`). Schedule row 48's consumer check is "golden"; plan
   section 2.8 forbids folding a numeric change into a structural commit.
   Correction: split the feature into its own commit and row (before OR-10,
   `build: parse JSON floats exactly in every build`) whose verification
   runs the consumer gate and the B0 DE-trace check and records the delta
   over the Python tariff tests; if any number moves, the row moves into the
   numeric block per plan section 2.8 with a "Numeric changes" line.

10. **Major. OR-16: replay export is silently inexact for any run whose
    dwelling differs from what `DwellingSettings` and the inputs rebuild.**
    Evidence: `Replay::build` rebuilds from `DwellingConfig` plus the
    tariff. A run that used `DwellingBlueprintBuilder` mutators
    (`add_equipment_spec`, `remove_equipment`,
    `remove_equipment_by_end_use`, plan section 2.7), `Dwelling` equipment
    mutation (`mod.rs:3788`), caller-registered actors
    (`add_actor_by_name`), queued external control signals or gym actions is
    not reproduced, and nothing records or rejects that.
    Correction: in OR-16, record in `Provenance` a flag per category
    (blueprint mutated, equipment mutated after build, caller actors,
    external control signals) set by the dwelling, and make
    `export_replay` return a typed error naming the category when any is
    set; add `replay_export_rejects_mutated_run` (one case per category).

11. **Major. IN-7 (row 66a) uses `DescribeFields`, which IN-3 (row 67)
    creates.**
    Evidence: IN-7 "`EnvelopeScales` ... defined with its `DescribeFields`
    impl in `hares-io`" and "bounds (0, 10], declared through
    `DescribeFields`"; plan section 2.7 places `FieldSchema`, `ValueType`,
    `DescribeFields` in `hares-types` `schema.rs` (new) under IN-3, and
    IN-3 adds `serde_json` to hares-types for it. IN-3 depends on IN-7, so
    the rows cannot simply swap.
    Correction: IN-7 defines `EnvelopeScales` with its bounds enforced by
    its own deserialisation and range check (OD-2's rule) and no
    `DescribeFields`; IN-3 adds `impl DescribeFields for EnvelopeScales`
    with the other impls and lists the `envelope` reserved key. Move the
    `EnvelopeScales` shape into plan section 2.7 (see minor 13).

### Minors

12. **Minor. OR-10: `SnapshotError` is a `thiserror` enum in hares-tariff,
    which has no `thiserror` dependency** (`crates/hares-tariff/Cargo.toml`
    at HEAD: hares-types, serde, serde_json, chrono, chrono-tz, tracing).
    Correction: OR-10 adds `thiserror = { workspace = true }` with
    `cargo add`, and plan section 2.1 names `SnapshotError`'s variants.

13. **Minor. Types used across crates or entries without a section 2 shape:**
    `EnvelopeScales` (defined only in IN-7), the public period reducer of
    hares-io that hares-fleet constructs (OR-13, FL-1, FL-4: no name or
    constructor signature), `SnapshotError` variants. Plan section 1 requires
    every named type in section 2. Correction: add `EnvelopeScales` to
    section 2.7 and the reducer's name and constructor
    (`PeriodReducer::new(frequency, calendar, rules: &[TimeRule])`, its
    `push_row`/`push_batch`/`finish`) to section 2.6 or 2.9.

14. **Minor. Plan section 2.2 says every variant's Python constructor lands
    in the same commit; OR-14 defers `Destination.callback()` to PY-10.**
    Correction: add "except `Destination.callback()`, which arrives with the
    Python attach in PY-10" to section 2.2.

15. **Minor. OR-14 `validated_config_keeps_callback_marker` and OR-12
    `health_policy_toml_round_trip` call `to_toml()`, which Rust
    `SimulationConfig` lacks** (`config.rs` at HEAD has `from_toml` only;
    PY-8 adds `to_toml()` to the Python class). Correction: PY-8 adds
    `SimulationConfig::to_toml(&self) -> Result<String, ConfigError>` in
    Rust (the Python method wraps it), or the tests use `toml::to_string`.

16. **Minor. IN-6's Python `DwellingConfig(..., resample_overrides=...)`
    test cannot name the adapter**: `PyDwellingConfig.__new__`'s signature
    (`py_config.rs:582`) has no `**kwargs`, so removing the parameter makes
    pyo3 raise its own `TypeError` before the body runs (the mechanism PY-8
    explains). Correction: IN-6 adds `**unexpected` to that signature and
    looks it up in the `DwellingTable` scope; PY-2 later replaces the
    constructor.

17. **Minor. OR-15: `metrics_from_table` has no start year or duration,
    which `MetricsCalculator::finish` reads from its stored config**
    (`metrics.rs:997` duration, `:1001` `start_time.year()` for leap-year
    coverage). Correction: state that the table's first `Time` gives the
    year and the row count times `time_res` the duration, so the result
    equals the in-run value on bits.

18. **Minor. Plan section 2.1 `MetricsAccumulators` = "the fields from
    `total_electric_energy_kwh` on"**, which sweeps in
    `config: SimulationConfig`, `has_envelope_columns` and the
    observe-only `end_use_equipment_counts` (`metrics.rs:427-447`), none an
    accumulator. Correction: list the excluded fields (config and plan state
    are rebuilt at freeze; observe counters are diagnostics).

19. **Minor. Plan section 2.2 catalogue: the digest omits `across_homes`
    and `VariableDef` drops `description`.** Two catalogues that differ only
    in a `Quantity`'s fleet rule hash equal; `TelemetryField.description` and
    PY-11's `OutputVariable(description=...)` reach no `VariableDef` or
    Python `Variable`. `Unit`'s serde form (symbol or variant name) is not
    stated, though `Catalogue::to_json`, `FieldSchema.unit` and
    `HpxmlElementSchema.unit` serialise it. Correction: add the
    `across_homes` tag (`sum`/`mean`) after the kind tag in the digest rule;
    add `description` to `VariableDef` and `Variable` (or state it is
    dropped); state that `Unit` serialises as its symbol.

20. **Minor. OR-1's undeclared-key check compares telemetry length with the
    declared count**, which misses an actor that publishes an undeclared key
    in place of a declared one it has not written yet. Correction: build each
    actor's `Telemetry` from `output_variables()` at registration so that
    `Telemetry::set` on an unregistered key latches (OR-11's mechanism) and
    the length check is not needed.

21. **Minor. OR-11 fluid-loop resolution is under-specified in three
    places.** (a) Its three tests name no target file or fixture (the
    untracked `crates/hares-core/tests/fluid_loop_declarations.rs` in the
    working tree is not cited). (b) "an exhaustive per-`FluidType` mapping
    (a `match`)" leaves open whether cp stays configurable;
    `crates/hares-envelope/tests/fluid_solver_tests.rs:126` builds a custom
    table. (c) Boilers declare their fluid port as `FluidType::Water`
    (`hvac/boiler.rs:197,580`) while `init` takes `fluid_type` from config
    (`:258,669`) and contributes with it (`:364`), so a non-Water boiler
    fails at the first step with "undeclared fluid loop"; ports-derived loop
    types keep that. Correction: name the test file and fixtures; state the
    cp type (a struct with one field per `FluidType`, defaults as today,
    which the test constructs); make boiler `init` set the fluid port's
    `fluid_type` from config, or reject non-Water at config validation,
    with a test.

22. **Minor. OR-4 pins `diagnostics_cz4a.csv` "from `ebdb1a17`"**, which
    requires building a pre-upgrade tree; every row before 41 is bitwise
    neutral, so the parent commit's writer (still present until OR-4)
    produces the same file. Correction: pin at the entry's parent commit.

23. **Minor. PY-6 `build.rs` emits `cargo:rerun-if-changed=../../.git/index`
    only**; in a worktree (the engine-under-test mechanism,
    `.workstrees/<sha>`) `.git` is a `gitdir:` file and the index lives
    under that directory, so `dirty` goes stale after edits outside
    `crates/`. Correction: resolve the index path through `gitdir:` as HEAD
    is resolved.

24. **Minor. PY-8 Dependencies omits IN-6**, though its change adds rows to
    `REMOVED_KEYS`, which IN-6 creates. Correction: add `12-inputs.md` IN-6.

25. **Minor. PY-11 `aggregation=None` "means the kind's default"; plan
    section 2.2 gives no default for `Quantity`.** Correction: make
    `aggregation` required for `kind="quantity"` (a `ValueError` otherwise).

26. **Minor. IN-4 returns "every dwelling-level weather variable of the
    catalogue", but `VariableDef` has no weather marker** (source
    `Dwelling` covers non-weather variables too). Correction: list the
    catalogue names IN-4 returns, or add the selection as a named list in
    the entry.

27. **Minor. IN-9 does not decide the schedule columns
    `electric_vehicle_charging` and `electric_vehicle_discharging`**, which
    `bldg0176775` and `bldg0442669` `in.schedules.csv` carry (the
    OpenStudio-HPXML EV charging schedule; unmapped in `COLUMN_MAPPINGS`),
    now that the vehicle is simulated by the actor model. Correction: add
    them to IN-9's read/not-read decisions with the reason, consistent with
    the fix for major 2.

28. **Minor. Plan section 2.2's freeze rule covers equipment mutation only.**
    Actor registration after the first recorded row changes the catalogue
    (actor columns) the same way. Correction: "mutating equipment or
    registering an actor after the row plan freezes is an
    `InvariantViolation`".

29. **Minor. Destinations: the decision log says files are "renamed on
    finish"; plan section 2.2 publishes `overwrite: false` by
    `std::fs::hard_link` plus removal**, which fails on filesystems without
    hard links. Correction: record the no-clobber form in the decision log
    entry, and state the error (naming the path) when `hard_link` is
    unsupported.

30. **Minor. Citation and wording slips.** IN-3 cites `DwellingConfig` at
    `mod.rs:206` (the struct is at `:210`); plan section 2.2 cites
    `zone_display_name` at `mod.rs:806` (the call site; the function is at
    `:7497` at `ebdb1a17`) and `BoundaryType` at `building.rs:59` (`:60`).
    OR-1 Verification says compare-all shows the `unit` metadata change as
    "the only difference", but frame_golden pins names, types, nullability
    and order only (`tools/frame_golden/src/digest.rs:550`), so compare-all
    shows none. OR-8 says "fleet homes whose fleet requests metrics" while
    plan section 2.4 and FL-1 make metrics a per-home config request.
    Correction: fix each citation and phrase.

31. **Minor. IN-7 test `opaque_r_scale_never_reselects_the_row` names
    "`base.xml`" without a path, and OR-16
    `replay_export_fails_if_input_file_changed_since_parse` and
    `replay_load_fails_on_defaults_digest_mismatch` name no inputs.**
    Correction: `tests/fixtures/hpxml/ochre_samples/base.xml`; the
    `consumer_shape_900s` inputs of the entry's other tests.

32. **Minor. OR-16 `HpxmlDataPatches` serde derives arrive at row 69, but
    IN-1 (row 63) already puts `DwellingSettings` (holding
    `Option<HpxmlDataPatches>`) into `Provenance`.** Nothing requires
    `Provenance` to be serialisable before OR-16, so this compiles; state
    in plan section 2.1 that `Provenance` derives no serde (only `BuildInfo`
    serialises, for `replay.toml`) so no earlier row adds derives by
    accident.

## Items verified correct

- Crate dependency table (plan section 2.1) matches every `[dependencies]`
  table at HEAD; `hares-core` names `hares-fleet` only in
  `[dev-dependencies]`; `hares-python` depends on every crate except
  `hares-envelope`; `hares-io` has no pyo3. Every section 2 type sits in a
  crate that sees the types it names (`TelemetryField`/`Unit`/
  `HealthEvent`/`EquipmentHealthCounts`/`ResolvedParameter`/`FieldSchema`
  in hares-types below hares-equipment; `HealthPolicy`, `Selection`,
  `Destination`, `BatchCallback` (arrow present) in hares-io; snapshots in
  hares-tariff with chrono-tz; `TariffRecord` in hares-core, which depends
  on hares-tariff). `uom` is absent from hares-types today and B-14 adds it
  (`09-numeric-fixes.md:2085`); `sha2` is absent from hares-io and OR-7
  adds it; `serde_json` is dev-only in hares-types and IN-3 moves it.
- serde: `Arc<T: ?Sized>` deserialises through `Box<T>` only under `rc`
  (`serde_core-1.0.229/src/de/impls.rs:2042-2088`), so `Arc<[Arc<str>]>`
  needs the feature OR-1 adds; `deny_unknown_fields` on an externally
  tagged enum applies to struct variants; `Cow<'static, str>` deserialises
  owned; `HaresError` derives `Serialize, Deserialize, Clone, PartialEq`
  and `HealthEvent` already derives them (`hares-types/src/health.rs:11`).
- serde_json 1.0.151 has `float_roundtrip`; only frame_golden enables it
  at HEAD (`tools/frame_golden/Cargo.toml:24`), and frame_golden is a
  workspace member, as OR-10 states.
- pyo3 0.29.3: `Python::detach<T: Ungil, F: Ungil + FnOnce() -> T>`
  (`marker.rs:562`) and `Python::attach` (`:414`) exist; the extension is
  not abi3, so `pyo3::ffi::PyGILState_Check` (`pyo3-ffi cpython/pystate.rs:80`)
  is available for PY-3's test; `#[new]` signatures generate a class
  `__text_signature__` (`pyo3-macros-backend pymethod.rs:1603-1612`), so
  PY-8's `inspect.signature` test works.
- pyo3-arrow 0.19.0: `PyTable` is `#[pyclass(module = "arro3.core._core",
  name = "Table", subclass, frozen)]` at `src/table.rs:36`;
  `PyTable::from_arrow_pycapsule` and `try_new(Vec<RecordBatch>, SchemaRef)`
  exist; `PyRecordBatch::new` and `__arrow_c_array__`
  (`record_batch.rs:183`) exist; it depends on arrow 59, pyo3 0.29, numpy
  0.29 and thiserror 1. `concat_batches` is at
  `arrow-select-59.3.0/src/concat.rs:613`. Locked versions match plan
  section 2.7 (arrow 59.3.0, pyo3 0.29.3, numpy 0.29.0).
- OR-5's committed shapes equal plan section 2.3 (`RunHealth`,
  `WarmupOutcome`, `WarmupResiduals` at `hares-core/src/health.rs`;
  `record_clamped_actions -> Result<()>` at `mod.rs:4685`;
  `take_health_counts` on the trait at `hares-equipment/src/lib.rs:282`);
  `ActorTiming`/`ActorTimings` and `DwellingProfilingSummary` equal plan
  section 2.9; `CountingAllocator`/`thread_allocations` exist in
  `hares-types/src/alloc_count.rs`.
- Snapshot sources: `BillingState` (`billing.rs:75-100`), `DemandWindow`
  (`:9-17`), `BillingPeriodSummary` (`:410-421`) and `TariffEvaluator`
  (`evaluator.rs:8-29`, `step_index`, `finished`, `finalized`) hold exactly
  the fields plan section 2.1 snapshots; `CHECKPOINT_VERSION = 8`
  (`checkpoint.rs:22`), `deserialize_gated` at `:154`.
- Metrics: `FullSimulationMetrics` (`metrics.rs:237`), its `Deref`
  (`:244-249`), `MetricsCalculator` (`:361`), `total_electric_energy_kwh`
  (`:391`), `accumulate` (`:641`), and the existing `MetricsError` (`:32`).
- Output evidence at `ebdb1a17`: `extract_unit_from_name` (`columns.rs:27`),
  `field_with_unit` (`:43`), the classifiers (`:178, 321, 374, 382, 1089,
  1140`, `is_ev`'s `contains("ev")`), metadata keys (`:915-927`), Utf8
  `Time` (`:425`), `"%+"` (`mod.rs:7009-7010`), writer `File::create`
  (`writer.rs:123`), `check_rotation` (`:190`), `default_output_path`
  (`conversions.rs:590`), engine items (`engine.rs:49,102,251,411,428,472,512`),
  diagnostics writer and preamble (`mod.rs:2790-2838`), fleet time
  handling (`aggregation.rs:444,505,508,636-637`). No reader of the deleted
  metadata keys exists outside `columns.rs`.
- Config evidence: `SimulationConfig` derives `Serialize, Deserialize`;
  `from_toml`'s five checks; `time_res_secs_u32`'s `unwrap_or(3600)`;
  `RotationPolicy`; synthetic `SyntheticSimulationConfig` (`:125`) and
  `SyntheticOutputConfig` (`:291`); `bestest/600.toml`'s `[simulation]`
  and `[output]` tables.
- Python evidence: exceptions under `_hares` (`py_dwelling.rs:30-37`), the
  six `initial_state` sites, `KNOWN_KWARGS` (22 entries at `872cf289`, all
  dispositioned in PY-2), `build_config`, the Python defaults
  (`py_config.rs:17-18, 201, 211`), conversions (`:20, 33, 52, 158, 165`),
  `py_fleet.rs:303`, `py_gym.rs:494`, `vec_env.py:47,119,236`,
  `RewardContext` (`gym_env.py:88-92`), ResStock weight lookups
  (`resstock.py:737,747,768,1055,1095`), cache paths, and the 787-count
  rename census.
- Inputs evidence: `WeatherFormat` (`weather.rs:16`, derives as stated;
  snake_case gives `res_stock_csv`), `detect_weather_format` (`:43`),
  `ochre_compat` (`:431`), `resample` (`:568`), `ensure_hourly_data_period`
  (`epw.rs:468`, called at `:172`), the IN-6 census (189 lines in 39 files),
  the IN-3 literal census (450 matches, 67 `.path(&["..."])`, same at HEAD),
  `inject_setpoint_schedules` (`:655`), `set_typed_setpoint_source`
  (`:695`), `inject_schedule_into_specs` (`:412`), the unread
  `water_heater_setpoint` mapping, `generate_schedule_from_hpxml`'s constant
  setpoint columns, `GasFurnaceConfig`'s AFUE check, `SiteLocationOverride`
  (`:134`), `SiteLocation` (`:159`), `FieldSource::CallerOverride` (`:102`),
  `HpxmlDataPatches { number_of_bedrooms: Option<f64> }`, `EquipmentSpec`
  (`equipment.rs:22`). The bldg0176775 vehicle block (63.433 kWh usable,
  5690.0 W, level 2, 0.22002 kWh/mile) matches IN-9; its `electric vehicle
  charging` entry is a `ServiceFeeder` `LoadType`, not a `PlugLoad`, so IN-9's
  double-count error does not fire on its own fixture.
- OR-11 resolution facts: `extract_fluid_loops_from_specs` at
  `solver_builder.rs:763` matches on equipment names and omits tankless,
  indirect tank, wet appliances and `DHW_DEMAND_LOOP`
  (`water_heater/mod.rs:128`); every `DHW_DEMAND_LOOP` port declares
  `FluidType::Water`; today an undeclared loop takes its accumulators'
  fluid type and the water-cp fallback only covers a fluid type missing
  from the table, so the ports-derived map is bitwise neutral for every
  runnable configuration. The two found defects (`warn_unwritten` key
  mismatch, `ports.rs:713` versus `:858`; the gated
  `restored_from_checkpoint` reset at `872cf289` `mod.rs:5449`) are real.
- Fixtures named by the tests exist at HEAD (cz4a and cz2a parity cases,
  BESTEST 600, both URDB files, `base.xml`, the Denver TMY3 and BEopt
  schedule, the ResStock 2025.1 homes and weather files), and the golden
  manifests carry the starts, steps, offsets, `bldg_id` 176227, the
  `Gas Water Heater` override and the replaced defaults file the tests
  rely on; OR-7's and IN-2's expected values (UTC times, 8760 steps,
  -25200 s) follow from them; OR-13's step counts (91 steps, 2/4/1 per
  hour, tie rows) and PY-10's 40/40/16 batches are arithmetically right.
- Schedule order, apart from majors 1, 3, 8 and 11: each other 03, 05 and 12
  entry uses only types and bindings created at or before its row
  (REMOVED_KEYS from IN-6 at row 33 before PY-8/PY-2/OR-3; `validate()`
  from PY-8 before OR-3/OR-13; OD-18 before OR-12/PY-9; OR-13 before
  OR-10; `sha2` from OR-7 before IN-1; RT-4 before IN-1/IN-5; RT-6 before
  IN-2/IN-7; B-14's SI fields before IN-9).
- No new type name collides with an existing one at HEAD; the scoped
  documents carry no em dashes or listed tics; commit subjects carry no
  planning IDs.
- Decision log: results, output model, destinations (apart from minor 29),
  python export, config strictness, health, time, aggregation, metrics
  optional, package, input schema scope, ochre_compat, calibration
  R factor and the OR-11 next step are reflected as decided.
