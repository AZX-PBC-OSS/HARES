# L4 resolution: plan.md §2, ledger/03, ledger/05, ledger/12

Scope: every finding of `verify-L4-b.md`; the findings of `verify-L4-e.md`
that concern these files (4, 18, 32, 34, 40); and the coordinator's
request on the `.github/workflows/ci.yml:155` step. Code claims were
re-read at HEAD `afa9a071` with `git show` and `git grep`; the uncommitted
OR-11 tree was read only to see what the implementer has built, never as
evidence. Binding: `decision-log.md`, read in full.

Counts: verify-L4-b 32 findings (11 major, 21 minor): 32 fixed, 0
rejected. verify-L4-e 5 findings in scope: 3 fixed (18, 32, 34), 1 fixed
with the change in another owner's file (4: wording coordinated below), 1
rejected as a duplicate (40). The CI request: fixed. Found while fixing: 3
defects outside these files, handed to their owners below. Schedule
changes required: 1 (a new row for OR-17).

## OR-11 first (row 19, in implementation)

OR-11's text now carries everything the implementer needs, and ends with
a dated "Implementer note" listing what changed:

1. **Warning channel** (finding 1). `crates/hares-types/src/warning.rs`:
   `Warning { step_index, source: Arc<str>, message }` (`plan.md` §2.1's
   type, moved to `hares-types` because `hares-equipment` and `hares-io`
   produce it). Equipment: `fn drain_warnings(&mut self, out: &mut
   Vec<Warning>) {}`, drained after `init`, after `add_equipment` and
   `replace_equipment`, and per step beside `take_health_counts`
   (`mod.rs:6335`) into one reused scratch `Vec`. Resolvers:
   `resolve_equipment` and `inject_schedule_into_specs` take `&mut
   Vec<Warning>`; the blueprint holds the resolver warnings and the build
   moves them into the log. HPXML: `Building.parse_warnings`. Tariff:
   `ElectricTariff.parse_warnings: Vec<String>`, pushed by `set_tariff`
   with source `tariff` (the tariff is parsed outside any dwelling, so its
   warnings travel with it). Until OR-6 the log stores strings through
   `WarningLog::push_warning(Warning)`, formatting `"{source}: {message}"`.
   OR-6, RT-5 and IN-5 build on it; none adds a second channel.
2. **Metrics row** (`metrics.rs:993-997, 1017-1029`): re-dispositioned to
   delete, not warn: the result already carries `simulation_duration_hours`
   and `coverage`, and a warning would fire on every legitimately short run
   (a stepped run finished early, a skipped Feb 29), marking such fleet
   homes `Degraded`.
3. **Schedule columns** (finding 2, minor 27). `WaterHeater` category with
   `hot_water_fixtures` and `hot_water_mains_temperature` (read at
   `:748`); the occupancy lookup moved into `hares-io` and shared, so
   `Occupancy (Persons)` (parity and BEopt files) counts as read;
   `Ignore` becomes `NotUsed { reason }`. Per column: `hot_water_dishwasher`
   and `hot_water_clothes_washer` are legitimately unused (the wet
   appliance draws its own hot water on the DHW demand loop during the
   cycles the `dishwasher` and `clothes_washer` columns schedule,
   `event_load.rs:1162-1189`; consuming them would count the draw twice);
   `electric_vehicle_charging` and `electric_vehicle_discharging` are
   unused because trips come from the `ev_driver` model (IN-9's decision).
   Census of all 19 schedule column names in the fixture tree: each read or
   `NotUsed`, so no golden, parity or ResStock home raises a schedule
   warning. Tests: `fixture_schedule_columns_are_all_known`,
   `unknown_schedule_column_warns_naming_it`,
   `resstock_homes_carry_no_schedule_warning`
   (`crates/hares-core/tests/schedule_columns.rs`).
4. **Fluid loops** (minor 21). Tests in
   `crates/hares-core/tests/fluid_loop_declarations.rs` over ResStock
   2025.1 `bldg0000007` (the file the tree already holds) and
   `tests/fixtures/hpxml/ochre_samples/base-dhw-multiple.xml` (the only
   fixture with tankless, indirect tank and boiler together; the tree's
   test lacks tankless). cp is not configurable:
   `FluidSolverConfig.fluid_specific_heats` is deleted and the solver uses
   `hares_physics::constants::cp_j_kg_k`, the existing exhaustive `match`
   the boilers use, so solver and equipment share one cp per fluid (what
   the tree already does); `net_power_from_flow_and_temp_delta`
   (`fluid_solver_tests.rs:116`, cp 4186) is rewritten. Boilers: `init`
   sets the fluid port's declared type from the configured `fluid_type`
   where it sets the loop id (`boiler.rs:271, 691`), and the loop map is
   read after `init`; test `boiler_port_declares_configured_fluid_type`.
5. **CI step** (coordinator): `ci.yml:155` is deleted, not edited. With
   the flag gone it would be exactly `ci.yml:154`
   (`cargo nextest run --workspace --release --no-fail-fast --profile ci`);
   the `-p hares-equipment` step at `:156` stays (different feature
   unification).
6. **Stop record** (L4-e 18): the rejected operator options are gone; the
   section keeps the row, the input, the error, the cause and the
   operator's disposition, under "Fluid-loop declarations (the stop
   condition fired here, 2026-10-02)".

## verify-L4-b.md

| # | Resolution |
|---|---|
| 1 | fixed: OR-11 "Warning channel" section (above); census rows battery `:892-945`, `:1009-1019`, ev `:934-956`, pv `:478-501`, `:840-843`, resolve_hvac `:3344-3373`, validation `:94-100`, schedule `:459-469`, urdb `:312-318`, `:364-369` now name their channel; `plan.md` §2.1 "`Warning` is one type from the first warning a crate below `hares-core` raises: ... OR-11 creates it ...; OR-6 makes `WarningLog` store `Warning` values ...; OR-10 adds their serde derives"; §2.3 owners "OR-11 creates `Warning` and its channel"; OR-6 "`Warning` is OR-11's, in `hares-types`", Dependencies gain OR-11; 12 IN-5 "The setpoint warnings go into the `&mut Vec<Warning>` collector that `inject_schedule_into_specs` takes since ... OR-11 ...; no second channel is added". |
| 2 | fixed: OR-11 "Schedule columns" section (above), census row "`find_unknown_schedule_columns`"; tests listed above. |
| 3 | fixed (owner split): `Dwelling::catalogue(&self) -> Arc<Catalogue>` is OR-1's ("readable from build on, before the first step"; test `catalogue_queryable_before_run` now on a built, unstepped dwelling); `DwellingBlueprint::catalogue()` moves to 12 IN-2, which separates assembly from warm-up ("The same assembly gives `DwellingBlueprint::catalogue(&self)` ... variables of actors a caller registers or `set_tariff` adds after build are not in it"), with `blueprint_catalogue_equals_built_dwelling_catalogue` and the Python accessor; 05 PY-4 keeps `Dwelling.catalogue()` and `RunResult.catalogue`, test `catalogue_available_before_first_step`; `plan.md` §2.2 and §2.7 state both owners. No schedule move: IN-2 (row 66) is after PY-4 (row 57). |
| 4 | fixed: `plan.md` §2.2 `Enum { labels: EnumLabels }`, `pub enum EnumLabels { Static(&'static [&'static str]), Runtime(Arc<[Arc<str>]>) }` ("serialises as a list of strings; deserialises as Runtime"), and "Static tables hold only `'static` data ... `Arc` appears only in what is declared at runtime"; §2.1 serde list names `EnumLabels::Runtime`; OR-1 item 3 "the per-equipment `Mode (-)` column declares `EnumLabels::Static` labels, so the table compiles as a `static`", test `static_tables_are_statics`; PY-11 labels become `EnumLabels::Runtime`. |
| 5 | fixed: `plan.md` §2.2 adds `Aggregation::Last` and the table "a stock or level ... `Quantity`, `Mean` / a configuration constant ... `Mean` / a running total since init ... `Last` / a count of the step ... `Sum` / a timer ... `Mean`", and "A variable in an energy unit is `Energy` only when it is the energy of the step"; §2.6 defines `Last`; OR-1 item 1 decides every such variable at `afa9a071` by name (five stocks and levels, nine constants, nine running totals including `DRIVE_SHORTFALL_KWH` and `CYCLE_COUNT`, two latest-event values, four actor step counts, six timers), pinned in `kinds.json` with aggregation and sampling; tests `last_aggregation_on_a_non_quantity_is_rejected`, OR-13 `hourly_running_total_is_last`; `ReducerSnapshot.sums` holds the last value for `Last`. |
| 6 | fixed: OR-6 "two bounded buffers ...: the run record, the first `CAPACITY` warnings of the run plus `warnings_dropped` ...; and the pending drain, every warning pushed since the previous `take_warnings()`, bounded at `CAPACITY` per drain with its own dropped count"; test `gym_drain_sees_warnings_past_run_capacity` (1 100 rejected signals, one drain per step); `plan.md` §2.3 "The warning log has two bounds"; OR-10 checkpoints both buffers. |
| 7 | fixed: OR-7 "The fleet aggregation reads each home's `Time` as naive local milliseconds and subtracts the home's `hares:utc_offset_s` to recover today's UTC epoch key ...; its output `Time` stays a Utf8 RFC 3339 `Z` string until `09-numeric-fixes.md` B-12"; test `fleet_aggregate_unchanged_by_timestamp_time` (`fleet_mixed_offset`, before any re-capture); fleet goldens not re-captured; `plan.md` §2.5 "Order of the change" paragraph. |
| 8 | fixed (in OR-13, no schedule move): OR-13 "`Time` is still today's Utf8 column at this row ... a period row's `Time` is its nominal period start rendered by the same function as the step rows ..., and `push_batch` reads a batch's Utf8 `Time` with `DateTime::parse_from_rfc3339` ... OR-7 switches ... and re-runs this entry's tests"; the reducer buckets on the row plan's `i64` local timestamp (OR-2); OR-7 Dependencies gain OR-13. |
| 9 | fixed: new entry OR-17 "JSON floats parse exactly in every build", its own commit directly before OR-10 (`build: parse JSON floats exactly in every build`), with a proof on every golden and consumer input: compare-all; the consumer gate and B0 DE-trace check on a worktree of the B0 engine `1d60f478` with only the manifest change; the Python tariff values. "If any comparison differs, the entry stops ...; it becomes a numeric row (`plan.md` §2.8 ...) ... and OR-10 waits for it." OR-10 no longer carries the feature; `plan.md` §2.1 states it. |
| 10 | fixed: OR-16 adds `Provenance.replay_blockers`, `ReplayBlocker` (`BlueprintEdited`, `EquipmentChanged`, `CallerActor`, `ExternalInput`, `StateRestored`, each with the methods that set it) and `HaresError::NotReplayable`; "`export_replay` returns `NotReplayable` naming every recorded category before it writes anything"; test `replay_export_rejects_mutated_run`, one case per category; `plan.md` §2.1 and §2.7 carry the types. |
| 11 | fixed: 12 IN-7 "Its bounds are enforced by `EnvelopeScales::validate()` ... `DescribeFields` does not exist yet at this row: IN-3, the next row, creates it and adds the impl"; IN-3 adds `impl DescribeFields for EnvelopeScales` and the `envelope` reserved key, test `envelope_scales_described_with_bounds`; `EnvelopeScales` shape in `plan.md` §2.7. |
| 12 | fixed: OR-10 "`SnapshotError` ... this commit adds it with `cargo add -p hares-tariff thiserror`, which inherits the workspace entry (`Cargo.toml:66`)"; `plan.md` §2.1 `SnapshotError { UnknownZone, ZoneMismatch, UnresolvedLocalInstant }`; test `snapshot_errors_name_the_zone_and_instant`. |
| 13 | fixed: `EnvelopeScales` in `plan.md` §2.7; `PeriodReducer`, `PeriodCalendar`, `TimeRule`, `PeriodRow`, `ReduceError` with `new`, `push_row`, `push_batch`, `finish` in §2.6; OR-13 constructs it by those names; `SnapshotError` variants in §2.1. |
| 14 | fixed: `plan.md` §2.2 "except `Destination.callback()`, which arrives with the Python attach in `ledger/05-python-surface.md` PY-10". |
| 15 | fixed: 05 PY-8 "`to_toml()` wraps the Rust `SimulationConfig::to_toml(&self) -> Result<String, ConfigError>`, which this entry adds"; `plan.md` §2.7 states it; OR-12 and OR-14 tests cite it. |
| 16 | fixed: 12 IN-6 "this entry ends that signature in `**unexpected`, looks each key up in the `DwellingTable` scope ... `05-python-surface.md` PY-2 later replaces the constructor"; test `dwelling_config_unknown_keyword_is_type_error`. |
| 17 | fixed: OR-15 "the year of the first `Time` value decides the leap-year coverage (`metrics.rs:1001` ...), and the duration is the row count times `time_res` (`metrics.rs:992`), so the result equals the in-run value on bits. An empty table is an error naming it." |
| 18 | fixed: `plan.md` §2.1 "the fields from `total_electric_energy_kwh` (`metrics.rs:391`) through `nan_step_count` (`:431`), less the two that are not running state: `has_envelope_columns` (`:415`) and `config` (`:428`). The `observe`-only `end_use_equipment_counts` after them (`:434`) is a diagnostic and is not carried." |
| 19 | fixed: digest "for `Quantity`, the across-homes tag (`sum` or `mean`)", "The description is not hashed"; `VariableDef.description` added; `unit` "serialises as `Unit::symbol()` (here, in `FieldSchema` and `HpxmlElementSchema`)"; 05 PY-4 `Variable.description` and test `variable_descriptions_come_from_declarations`; PY-11's `description` reaches `VariableDef.description`. |
| 20 | fixed: OR-1 item 2 "`Telemetry::from_fields(&[TelemetryField])` ... registers exactly the declared keys and seals the map: a later `insert` of a new key, like `set` on an unregistered key, latches with OR-11's unknown-key latch ... Registration ... rejects an actor whose telemetry is not sealed ... so no per-step check is needed"; tests `undeclared_actor_telemetry_key_fails_the_step` (covers the swapped-key case), `unsealed_actor_telemetry_is_rejected_at_registration`; `plan.md` §2.2 states the mechanism; PY-11 builds the wrapper's telemetry with it. |
| 21 | fixed: see "OR-11 first", item 4. |
| 22 | fixed: OR-4 "written by the writer of the entry's parent commit, which every row before it leaves bitwise unchanged". |
| 23 | fixed: 05 PY-6 "The git directory is resolved once, as for `HEAD`: `../../.git` when it is a directory, and the directory its `gitdir:` line names when `.git` is a file ..., so `HEAD` and `index` are that worktree's"; test `rerun_paths_follow_the_worktree_gitdir`. |
| 24 | fixed: 05 PY-8 Dependencies "`12-inputs.md` IN-6 (creates `REMOVED_KEYS`, `KeyScope` and `removed_key`, to which this entry adds rows)". |
| 25 | fixed: 05 PY-11 "`aggregation=None` means the aggregation `dl-i08-architecture.aggregation` gives the kind ..., and it is required for `kind == "quantity"` ... (`None` there is a `ValueError` naming the choices)"; test `python_actor_quantity_without_aggregation_is_an_error`. |
| 26 | fixed: 12 IN-4 names the ten channels and their `WeatherState` fields ("`Outdoor Dry Bulb (C)` ... `Hot Water Mains Temperature (C)`"), adds the seven missing ones to the dwelling-level table in no verbosity preset, with kinds and sampling, and excludes the wind direction ("a direction has no mean over a period"). |
| 27 | fixed: 12 IN-9 "not read, the schedule-file columns `electric_vehicle_charging` and `electric_vehicle_discharging` ... for the same reason: they stay `NotUsed` entries ... (`03-output-reporting.md` OR-11)"; its test asserts no `schedule` warning. |
| 28 | fixed: `plan.md` §2.2 "mutating equipment or registering an actor after the row plan freezes is an `HaresError::InvariantViolation` ... before the first row, the freeze is rebuilt on either"; OR-1 rebuilds the catalogue on actor registration. |
| 29 | fixed in §2.2 ("On a filesystem without hard links the publish fails with `HaresError::Io` naming the target and stating that `overwrite: false` needs hard links there and `overwrite: true` publishes by rename"); the decision log wording is another owner's (below). |
| 30 | fixed: IN-3 `mod.rs:210`; `plan.md` §2.2 and OR-1 cite the function `zone_display_name` at `mod.rs:7790` (`afa9a071`); §2.7 `building.rs:60`; OR-1 Verification "the tool pins names, Arrow types, nullability and order ..., so it shows no difference. The `unit` field-metadata change ... is listed in the commit body, and the kind table pins each variable's `Unit`"; OR-8 "fleet homes whose config requests metrics (`plan.md` §2.4: metrics are a per-home config request)". |
| 31 | fixed: IN-7 `tests/fixtures/hpxml/ochre_samples/base.xml`; OR-16's two tests name the `consumer_shape_900s` inputs over 1 h and what each changes. |
| 32 | fixed: `plan.md` §2.1 "`Provenance` and its parts derive no serde: replay export writes its own manifest from them, so no row before OR-16 adds a derive to a type only because `Provenance` holds it." |

## verify-L4-e.md (items touching these files)

| # | Resolution |
|---|---|
| 4 | coordinated: `hares_core::input_schema()` stays in `hares-core` (`plan.md` §2.7, 12 IN-3, unchanged). The 09 owner applies L4-e's split as written: `crates/hares-io` keeps only the walk over each config type's serialised keys, and `crates/hares-core/tests/si_config_schema.rs::input_schema_override_classes_are_si` asserts on `hares_core::input_schema().overrides`; nothing in `hares-io` calls `input_schema()`. |
| 18 | fixed (see "OR-11 first", item 6). |
| 32 | fixed: OR-3 `test_gym_default_writes_no_file` ("a `DwellingGymEnv` built with its defaults ... one episode of 20 steps with the process's working directory set to a fresh temporary directory, leaves that directory empty"). |
| 34 | fixed: IN-2 Measurement "recorded in `02-measurement.md` against requirement R9's target (an infeasible input rejected in under 100 ms); a time above it is reported to the operator". |
| 40 | rejected: `exploration.md` §1.8 ("Requirement-to-ledger-entry table") already maps every requirement ID to its entry; a copy in `plan.md` §2 would be a second source that drifts (`dl-i08-process.documents`). Its RL-1 and R9 cells are stale after this round (below). |

## Schedule changes (owner: `ledger/00-schedule.md`)

- New row **47a**, between OR-9 (47) and OR-10 (48): OR-17, "JSON floats
  parse exactly in every build", file 03, commit
  `build: parse JSON floats exactly in every build`, consumer check
  "golden; consumer gate and B0 DE-trace on a `1d60f478` worktree with
  only the manifest change". If its proof finds a difference it moves into
  the numeric block and row 48 moves after it (OR-17's own text).
- Row 19's content grows (warning channel, schedule columns, `ci.yml:155`
  deleted); its commit message is unchanged.
- No other move: every new dependency points to an earlier row (OR-6 on
  OR-11; OR-7 on OR-13; OR-13 on OR-1 and OR-2; OR-10 on OR-1 and OR-17;
  OR-16 on RT-4; IN-2 on OR-1 and PY-4).

## Items for other owners

1. `ledger/06-runtime-memory.md` RT-5: its sink and warn-once rows use
   OR-11's channel. "`validate_core_contract` passes each warning to a
   sink the caller supplies" becomes "pushes each warning into a `&mut
   Vec<Warning>` the caller supplies (`03-output-reporting.md` OR-11)";
   `ISLAND_UNSERVED_WARNED`'s first occurrence goes through
   `push_warning`; `NO_AMPACITY_WARNED`'s builder warning goes into the
   blueprint's resolver collector.
2. `ledger/09-numeric-fixes.md` B-14: L4-e finding 4 as coordinated above.
3. `ledger/04-fleet.md` FL-1 and FL-4: name the reducer by `plan.md` §2.6
   (`PeriodReducer::new(frequency, PeriodCalendar, rules)`, `push_batch`,
   `finish`; FL-4 adds the period-origin offset to `PeriodCalendar`), and
   note that from OR-7 the fleet reads each home's `Time` as naive local
   milliseconds minus `hares:utc_offset_s` (OR-7's text).
4. `decision-log.md` `dl-i08-architecture.destinations`: "Files are
   written to a temporary name and renamed on finish" should read "written
   to a temporary name and published on finish without replacing an
   existing file (a hard link and removal; a rename when `overwrite` is
   set)", matching `plan.md` §2.2 (verify-L4-b minor 29).
5. `exploration.md` §1.8: RL-1 "test gap until
   `test_gym_default_writes_no_file` lands" becomes "OR-3
   (`test_gym_default_writes_no_file`)"; R9 "measurement only; latency
   target not yet confirmed or waived" becomes "IN-2 (measured against the
   100 ms target)".
6. Found defect, `ledger/11-other-defects.md` owner: OpenStudio-HPXML
   unavailable periods are accepted and not applied. The schedule columns
   `No Space Heating`, `No Space Cooling`, `Vacancy` and `Power Outage`
   are on the ignore list (OCHRE ignores them too), but they are real
   inputs: `bldg0176227/in.schedules.csv`, the `consumer_shape_900s` golden
   home, marks 96 hours of no space heating, and `bldg0000001` 672. HARES
   heats through them. By `dl-i08-architecture.precedence` (accepted means
   applied) this needs a numeric entry that applies them, or a decision
   that rejects such a file. OR-11 keeps them `NotUsed` with the truthful
   reason "unavailable periods are not modelled" so its commit stays
   bitwise.
7. Found defect candidate, `ledger/11-other-defects.md` or
   `09-numeric-fixes.md` owner: the wet appliances draw a fixed hot water
   volume per cycle (`resolve_loads.rs:258` clothes washer 15 L, `:432`
   dishwasher 6 L), where OpenStudio-HPXML and OCHRE derive a daily volume
   from the HPXML appliance (`vendors/OCHRE/ochre/utils/hpxml.py:1264,
   1356`). Not measured here; it moves domestic hot water energy if
   confirmed.
8. `ledger/02-measurement.md` owner: OR-17's proof records and IN-2's
   R9-target measurement have their lines there.
