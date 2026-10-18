# L3 resolution: plan.md §2, ledger/03, ledger/05, ledger/12

Scope: every finding of `verify-L3-b.md`, and every finding of
`verify-L3-e.md` that concerns `plan.md` §2, `ledger/03-output-reporting.md`,
`ledger/05-python-surface.md` or `ledger/12-inputs.md`. Code claims were
re-read at HEAD `21d90588` (branch `feat/output-results`, which carries
PR-1 and PR-2) and, for evidence the plan cites at the baseline, at
`ebdb1a17` with `git show`. Binding: `decision-log.md`, read in full.

Counts: verify-L3-b 31 findings (14 major, 17 minor): 31 fixed, 0
rejected. verify-L3-e 9 findings in scope (5, 6, 7, 12, 19, 20, 25, 26,
28): 9 fixed, one clause of 25 rejected with its reason. Further defects
found while fixing: 9, all fixed in these files. Schedule moves required:
none (every ordering finding is fixed by moving the definition or the
Python half to its first user). Items for other owners: 7, listed at the
end, plus one cross-check that needs no change.

## verify-L3-b.md

| # | Resolution |
|---|---|
| 1 | fixed: 03 OR-4 "The equipment preamble is dropped with the CSV writer; OR-6, two rows later, carries the same information as `Provenance.equipment` (`plan.md` §2.1), which does not exist at this row", and its test removed; 03 OR-6 "`Provenance.equipment` carries the diagnostics equipment preamble OR-4 dropped (zone mapping, zone type, OCV source and `Equipment::provenance_lines()`, today's `mod.rs:2805-2830`)" with `provenance_carries_equipment_zone_mapping` against the preamble lines of OR-4's pinned `diagnostics_cz4a.csv`; OR-6 Dependencies gain OR-4. |
| 2 | fixed: 03 OR-14 test replaced by `validated_config_keeps_callback_marker` ("keeps `Destination::Callback` after `validate()`, and `to_toml()` then `from_toml()` gives the string `"callback"` back"); 03 OR-6 gains `provenance_records_callback_destination_marker` and Dependencies OR-14; 12 IN-1 states the test then reads `Provenance.settings.sim_config`. |
| 3 | fixed (Python half moved to its first user): 03 OR-15 "The Python binding is not in this commit ... PY-4 adds `hares.metrics_from_table` with its stub, its changes line and its Python test", Dependencies now OR-8 only; 05 PY-4 adds the binding ("imported with `PyTable::from_arrow_pycapsule`"), `metrics_from_table_matches_run_metrics_in_python`, Dependencies "PY-9, PY-1 (`pyo3-arrow` and `arrow()`), OR-15", commit `feat: expose the variable catalogue and metrics_from_table in Python`; 05 intro and `plan.md` §2.7 ("its Python binding needs the Python `Catalogue` and the Arrow stream import, so ... PY-4 adds it") follow. |
| 4 | fixed: 05 PY-9 pyclass list ends at "`Warning` and `Provenance`"; 05 PY-6 "the `BuildInfo` frozen pyclass (one getter per field), returned by `hares.build_info()`, and the `Provenance.build` and `Provenance.catalogue_digest` (hex string) getters on PY-9's `Provenance` pyclass, with their `_hares.pyi` stubs and their lines in the `docs/python.md` changes section"; PY-6 Dependencies gain PY-9; test `python_build_info_equals_run_provenance`. |
| 5 | fixed: `plan.md` §2.1 `Provenance` final form "`pub settings: DwellingSettings, // §2.7: the validated SimulationConfig, bldg_id, overrides, patches and warm-up the dwelling ran with, the master seed in effect included`" and `pub tariff: Option<TariffRecord>`; the field-by-entry paragraph "OR-6 creates it with `config: SimulationConfig` and `bldg_id: i64` in place of `settings` ... IN-1 replaces `config` and `bldg_id` with `settings` (the `DwellingSettings` of ... RT-4, an earlier row) and adds `inputs`; OR-16 adds `tariff`. Replay export reads only `Provenance` and the report outputs". The switch is in 12 IN-1 (mine) rather than RT-4, so no other owner changes; test `provenance_settings_carry_overrides_and_seed`. |
| 6 | fixed: `plan.md` §2.7 `InputRecords.weather_format: WeatherFormat, // the format the weather was parsed as (detected for a Path), never absent`; 12 IN-1 fills it and adds serde derives to `WeatherFormat` (`weather.rs:16`, today `Debug, Clone, Copy, PartialEq, Eq` only), test `weather_record_carries_resolved_format`; 03 OR-16 writes it and loads "the weather as `WeatherInput { source, format: Some(weather_format) }`", test `replay_of_text_weather_round_trips`. |
| 7 | fixed: `plan.md` §2.1 `BillingPeriodSummarySnapshot` (hares-tariff, the ten fields of `BillingPeriodSummary`, `billing.rs:410-421` at `ebdb1a17`, bounds as `LocalInstant`) with `from_summary` and `to_summary(&self, tz) -> Result<_, SnapshotError>`; "The closed billing summaries are the `DwellingCheckpoint` section `billing_summaries: Vec<BillingPeriodSummarySnapshot>`, because the dwelling holds them (`mod.rs:1518`)"; 03 OR-10 owner text "`hares-tariff` `BillingPeriodSummarySnapshot`, one per closed billing summary". |
| 8 | fixed (placement differs from the suggestion): `SetpointSource` moves to `hares-io` (`crates/hares-io/src/setpoint_source.rs`), where it is resolved, with two new `EquipmentSpec` fields `heating_setpoint_source` and `cooling_setpoint_source` (`EquipmentSpec` is itself in `hares-io`, `hpxml/equipment.rs:22`, so `hares-types` is not needed). 12 IN-5: "this entry adds `SetpointSource` ... and the `EquipmentSpec` fields ... which `inject_schedule_into_specs` (`schedule_resolve.rs:412`) sets ... Today the resolver stores only a typed `ScheduleSourceConfig::ColumnRef { col_idx, .. }` ... a column index, not a name"; source tests read the spec (`crates/hares-io/tests/setpoint_source.rs`), run tests the dwelling (`crates/hares-core/tests/setpoint_source.rs`); RT-6 copies the fields (item 3 below). |
| 9 | fixed: `plan.md` §2.7 comment "hares-types owns Unit, and IN-3 moves serde_json from its [dev-dependencies] to [dependencies] for FieldSchema.default"; 12 IN-3 "`hares-types` has `serde_json` only under `[dev-dependencies]` ... so this entry adds `serde_json = { workspace = true }` to its `[dependencies]`". |
| 10 | fixed: `plan.md` §2.7 "`hares_core::input_schema() -> InputSchema` ... The types live in `hares-io`; the assembly lives in `hares-core` (`crates/hares-core/src/input_schema.rs`, new), the lowest crate that sees every described type"; 12 IN-3 the same, with `DwellingConfig` and `DwellingSettings` implementing `DescribeFields` in `hares-core`; 12 IN-7 "The key deserialises into `EnvelopeScales { opaque_r_scale: f64, thermal_mass_scale: f64 }` ... defined with its `DescribeFields` impl in `hares-io` (`crates/hares-io/src/envelope_scales.rs`, new)". |
| 11 | fixed: `plan.md` §2.7 `KeyScope { SimulationConfig, DwellingTable, PythonKeyword, EquipmentConfig }`, `RemovedKey { scope, key, replacement }`, `removed_key(scope, key)`, which scopes each surface consults and in what order, and a Scope column on every table row (the `[simulation]` `initialization_duration_s` row is `SimulationConfig`; B-2's top-level row is `DwellingTable`); `InputSchema.removed_keys: Vec<RemovedKeySchema>`; 12 IN-6 creates the scoped table with `removed_key_is_scoped`; 05 PY-8 assigns each of its rows a scope and adds `synthetic_simulation_initialization_duration_names_toplevel` and `toplevel_initialization_duration_is_accepted`; PY-2 and OR-3 name their scopes. |
| 12 | fixed: 05 PY-8 `validate()` "and `site_location`, today checked only by the Python binding (`extract_site_location_override`, `py_dwelling.rs:2208-2265` at `872cf289`): each set field finite, `latitude_deg` in [-90, 90], `longitude_deg` in [-180, 180], `elevation_m` in [-500, 9000] and `utc_offset_h` in [-14, 14]"; the constructor "calls the same Rust site-location check `validate()` runs ... the Python copy of the checks is deleted"; test `toml_site_location_out_of_range_is_a_validate_error`; `plan.md` §2.7 Config states it. Ranges re-read at HEAD (`py_dwelling.rs:2243-2273`). |
| 13 | fixed, with the census corrected: SL-8 is row 25, after OR-11 (row 19), so the planning records are still tracked when OR-11 commits and the suggested `':!.shipwright'`-only census would hit `docs/reviews` and `docs/tickets`. 03 OR-11 lists every hit at HEAD `21d90588` (`README.md:49`, `CONTRIBUTING.md:90`, `docs/development.md:181-197, 461-462`, `docs/invariants-and-observability.md` eight lines including the method name, `docs/physics/exterior-surface-balance.md:116`, `docs/validation.md:215`, the three Python docstrings), rewrites each, and excludes by path what SL-8 moves (including `scripts/triage.py:191`, review tooling SL-8 moves); first acceptance criterion "`git grep -n check_invariants -- . ':!.shipwright' ':!docs/reviews' ':!docs/tickets' ':!docs/findings' ':!docs/superpowers' ':!scripts/reviews' ':!scripts/triage.py'`" prints nothing. OR-11 is close to the implementer (row 19). |
| 14 | fixed: 05 PY-11 "`across_homes` is `"sum"` or `"mean"`, required exactly when `kind == "quantity"` and a `ValueError` otherwise, and becomes `VariableKind::Quantity { across_homes }`" with `python_actor_quantity_without_across_homes_is_an_error` and `python_actor_quantity_declares_across_homes`; 05 PY-4 `Variable.across_homes` and `quantity_variables_carry_across_homes`; PY-11 Dependencies gain PY-4. |
| 15 | fixed: `plan.md` §2.1 "`hares-python` \| every crate above except `hares-envelope`" (manifest re-read at HEAD). |
| 16 | fixed: `plan.md` §2.2 "extensive ones (mass and volume flows, event counts, currents, charge capacities in Ah, byte counts) `AcrossHomes::Sum`" and "Dimensionless variables (unit `-`, `%`, `fraction`, `ratio`) are classified by meaning"; 03 OR-1 maps every unit of the census at `21d90588` (`W/m2`, `s`, `min`, `V`, `J/(kg*K)`, `kWh/mi` Mean; `kg/s`, `L/day`, `count`, `A`, `Ah`, `bytes` Sum) and the `-` variables by meaning with named examples; the test compares every variable against the pinned `crates/hares-core/tests/fixtures/schema/kinds.json`. |
| 17 | fixed: 03 OR-1 "`crates/hares-core/tests/catalogue.rs`: it needs the built-in actors and assembly, which `hares-io` cannot see", pins under `crates/hares-core/tests/fixtures/schema/`. |
| 18 | fixed: `plan.md` §2.2 "a code set that does not start at 0 declares a placeholder label at each unused code (`CHARGING_LEVEL` publishes 1 and 2, so its labels are `["none", "L1", "L2"]`)"; `State` examples are now "(defrost, on/off)" and connection is listed under `Enum` ("EV connection state, which has three values"); 03 OR-1 and test `enum_labels_cover_every_published_code`. |
| 19 | fixed: 03 OR-7 deletes `hares_verbosity`, `hares_mode_map`, `basement_lighting_created`, `garage_lighting_created` (`columns.rs:915-927` at `ebdb1a17`), their tests and `mode_ordinal_json` (`columns.rs:1008`, "whose only caller is that metadata"), test `schema_metadata_holds_only_offset_and_digest`; `plan.md` §2.5 names OR-7 and corrects the citation from `916-927`; OR-7 Dependencies gain OR-1 and OR-6. Assigned to OR-7 rather than OR-7's suggested sibling because OR-7 is the entry that rewrites the schema metadata. |
| 20 | fixed (second option): 03 OR-2 adds only `Selection` and `Frequency`, "`ReportSpec` and `Destination` are not added here: OR-3 adds them with `SimulationConfig.reports`"; OR-2's multi-selection tests "build the row plan in-crate from a selection list"; 03 OR-3 adds `ReportSpec` and `Destination::{Memory, Parquet, Csv}`; `plan.md` §2.2 variant-arrival sentence rewritten. |
| 21 | fixed: 03 OR-3 "Until OR-6 adds `finish()`, publishing happens at the end of `simulate()`, where today's `flush_and_close` runs (`mod.rs:2869`)"; 03 OR-14 "at the end of `simulate()`, today's `flush_and_close`, until OR-6, then at `finish()`". |
| 22 | fixed: `plan.md` §2.1 `RunResult.metrics` comment "until 03 OR-8, Some exactly when the run has at least one report"; 03 OR-6 "Until OR-8, `RunResult.metrics` is `Some` exactly when the run has at least one report, because metrics are still computed from the flushed batches (`metrics.rs:641`, `writer.rs:317`)"; 05 PY-9 the same. |
| 23 | fixed: 05 PY-1 "`arrow::compute::concat_batches` (`hares-python` depends on `arrow`, which re-exports `arrow-select`'s `concat_batches`, `arrow-select-59.3.0/src/concat.rs:613`) ... kept in a `OnceLock<RecordBatch>` field of the `hares-python` `ReportResult` pyclass (the Rust `ReportResult` of `plan.md` §2.1 is unchanged)"; "A report whose output is a file or a callback holds no batches: its `arrow()` and `table()` raise `HaresSimulationError` naming the report and its destination"; tests `file_report_export_raises_naming_destination` (PY-1) and `file_report_table_raises_naming_destination` (PY-9). |
| 24 | fixed: 05 PY-10 Dependencies "OR-14, PY-9, PY-1 (`pyo3-arrow` and its `PyRecordBatch`)". |
| 25 | fixed: 05 PY-8 "The constructor's signature ends in `**unexpected`: pyo3 raises its own `TypeError` for an unknown keyword before the constructor body runs"; `python_keyword_names_equal_rust_field_names` "skipping the `VAR_KEYWORD` parameter `unexpected`"; `validate()` holds only the checks on fields present at PY-8, and "The report and frequency checks join `validate()` with the fields they check (`03-output-reporting.md` OR-3 and OR-13)", which 03 OR-3 and OR-13 now state. |
| 26 | fixed: 03 OR-10 "Cargo unifies the feature for every crate in a build, so it also changes how `hares-python` parses tariff JSON (`py_tariff.rs:141,154,707,717`) and override JSON ... `tools/frame_golden` already enables the feature ... the commit body records the parsing change, and the Python suite runs in Verification"; Verification adds `maturin develop` and `pytest`. |
| 27 | fixed (tariff recorded, not refused, so replay is complete): `plan.md` §2.1 `TariffRecord { tariff: ElectricTariff, zone: String }` and `Provenance.tariff`; `plan.md` §2.7 `Replay { config, tariff, recorded_build }`, `RecordedBuild` ("BuildInfo as read back from replay.toml (owned strings)"), `load(dir, defaults_base) -> Result<Replay>`, `Replay::build`; §2.1 `BuildInfo` "Serialize only: &'static str fields cannot be deserialised"; 03 OR-16 writes `tariff.json` and the zone, tests `replay_with_attached_tariff_reproduces_billing` and `replay_records_the_exporting_build`, Python `Provenance.tariff` getter. |
| 28 | fixed: 12 IN-9 decides every vehicle element of `bldg0176775/home.xml:1814-1857`: read `FuelEconomyCombined` into `EvConfig.fuel_economy_j_per_m` with `uom` (test `vehicle_fuel_economy_read_in_joules_per_metre`); not read, each with its reason: `Battery/NominalCapacity`, `Battery/BatteryType`, `Battery/NominalVoltage`, `MilesDrivenPerYear`, `HoursDrivenPerWeek`, `FractionChargedLocation` and the schedule fractions ("trips, departures and plug-in times come from the `ev_driver` actor's trip model ... mapping that schedule onto the actor is a new driving model no decision calls for"); `docs/equipment/ev.md` lists them. |
| 29 | fixed: 12 IN-4 "The binding builds the `SiteLocation` from the dict's four values with every `*_source` field set to `FieldSource::CallerOverride` (`crates/hares-io/src/site_location.rs:100`)". |
| 30 | fixed: `plan.md` §2.9 "the reducer is a public item of `hares-io` because `hares-fleet` constructs one per home ... but neither is a stability contract or part of the documented API"; 03 OR-13 "a public item of `hares-io`, not a stability contract (`plan.md` §2.9)". |
| 31 | fixed: 03 OR-13 "an `Hourly` report at 900 s steps of a run starting at `2023-01-01T00:30` and ending at `2023-01-01T23:15` local (91 steps, the end exclusive): the first row, labelled `00:00`, has `Step Count` 2, the last, labelled `23:00`, has `Step Count` 1, and every row between has 4". |

## verify-L3-e.md (findings that concern these files)

| # | Resolution |
|---|---|
| 5 | fixed: as verify-L3-b 9. |
| 6 | fixed: `plan.md` §2.7 `DwellingBlueprintBuilder` now declares `new`, `from_config(config: &DwellingConfig) -> Result<Self>`, the mutators as they exist on today's blueprint (`equipment_names`, `add_equipment_spec`, `remove_equipment`, `remove_equipment_by_end_use`, `blueprint.rs:174-209` at `ebdb1a17`; the finding's `replace_equipment` and `set_overrides` do not exist there) and `freeze(self) -> Arc<DwellingBlueprint>`; `DwellingBlueprint::from_config(config) -> Result<Arc<DwellingBlueprint>>` is "Builder::from_config, then freeze", so RT-4's "`DwellingBlueprint::from_config(..)?.build()`" now type-checks as written; "Today's public blueprint fields (`config`, `schedule`, `defaults_path`, `equipment_specs`, `blueprint.rs:19-28`) become private; a test that reorders specs does so through `remove_equipment` and `add_equipment_spec`". |
| 7 | fixed: `plan.md` §2.9 `impl ActorTimings { pub fn new(names: impl IntoIterator<Item = Arc<str>>) -> Self; pub fn record(&mut self, slot: usize, elapsed: StdDuration, called: bool); pub fn timings(&self) -> &[ActorTiming]; }`. `07-profiler.md` PR-6 already states the same three methods and builds its allocation test with `ActorTimings::new(["probe_a", "probe_b"].map(Arc::<str>::from))` (checked). |
| 12 | fixed in these files; the engine change is item 4 below. `plan.md` §2.1 "Units" paragraph (running state SI; output records, `RunMetrics`, `BillingPeriodSummary` and its snapshot, in reporting units; one `uom` conversion where a value leaves the running state); `BillingSnapshot` shown in its final SI form (`cumulative_import_j`, `peak_demand_w`, `prior_period_peaks_w`, `cumulative_ev_j`, demand-window samples in W); "Snapshot units: a snapshot copies its live state field for field, so a restore is bitwise. OR-10 creates `BillingSnapshot` and `MetricsAccumulators` in the units `BillingState` and `MetricsCalculator` hold at that row ... B-14, a numeric row after OR-10, moves `BillingState` and the calculator's running totals to J and W ... and raises `CHECKPOINT_VERSION` to 10"; `StepResult.net_electric_power_w` final form, renamed by B-14 with the Python step dict key, "the `Total Electric Power (kW)` column keeps reading the solver's kW value, so no output moves"; §2.8 names B-14's run-state scope; 03 OR-10 "Each snapshot copies its live state in the units that state holds at this row". Converting at OR-10 is not possible: a kWh value converted to J and back is not bit-identical, so the checkpoint would not restore bitwise, and moving the live state is numeric, which belongs in the numeric block after OR-10. |
| 19 | fixed: 05 PY-8 `removed_key_messages_equal_across_toml_and_python` "parametrized over this entry's rows present on both surfaces (`duration_s` and `time_res_s`)"; the Python-only rows get `python_removed_keyword_names_replacement` on `SimulationConfig(...)`, and the TOML-only rows `synthetic_output_table_names_reports` and `synthetic_simulation_initialization_duration_names_toplevel`. |
| 20 | fixed: 12 IN-3 Problem "450 call sites (the census below, at HEAD `21d90588`) ... 67 of them are `.path(&["..."])` calls, nine of those written across lines (`building.rs:558`, `resolve_loads.rs:57,118`, `resolve_water_heater.rs:442,458,515,641,658,664`) ... `validation.rs:255-281` holds its required paths as a `&[(&[&str], &str)]` table"; Change "`path` takes `&[&'static HpxmlElement]` ... this type change is the gate, compile-enforced"; census `rg -U -n` with `\s*` ("it counts 450 sites at HEAD `21d90588` (the line-based `git grep` form counted 440)"), stated as the list of sites to convert. A generic `&\[\s*"` alternative was tried and rejected: it also matches value lists that are not element names (`resolve_hvac.rs:3397`, `&["HSPF", "COP"]`). |
| 25 | fixed, one clause rejected: 05 PY-9 "Its "Numeric changes" list starts empty; every entry that moves a number (`plan.md` §2.8) adds one line in its own commit naming what moved, why, and the golden fixtures it re-captured"; `plan.md` §2.8 makes that the rule for every numeric entry; 12 IN-8 and IN-9 add their lines; test `numeric_changes_name_existing_goldens`. Rejected: checking the list against the digests' recorded captures. A digest records `git_head` at capture (`tests/fixtures/golden/*.golden.json`), which structural re-captures also move (OR-7 re-captures every golden for the `Time` dtype), so a digest cannot tell a numeric change from a structural one; each line is checked by the entry's own review instead. |
| 26 | fixed: 05 PY-9 "Results" documents "`initialize()`, required once before the first `step()` or `simulate()`, which raise `RuntimeError` without it, and cleared by `reset_with_seed`" (`py_dwelling.rs:758-762, 813-817, 1582` at HEAD; OR-9 keeps the flag). Removing `initialize()` would be a Python API change no decision calls for. |
| 28 | fixed: `plan.md` §2.3 now cites "`crates/hares-core/src/dwelling/mod.rs:1177-1195,1228-1233,1262-1279` at `ebdb1a17`", the numbers OR-5 uses (re-read at `ebdb1a17`). |

## Further defects found while fixing (all fixed)

1. OR-10's billing test checkpoints inside the open January period, so the
   closed-summary section was never exercised: 03 OR-10 adds
   `restore_after_a_closed_billing_period_keeps_its_summary` (checkpoint at
   `2023-02-03T12:00`).
2. A `Provenance` holding the blueprint's settings would record the wrong
   seed after `build_with_seed` or an RL reseed: §2.1, 03 OR-6, 03 OR-16 and
   12 IN-1 record "the master seed in effect", tested by
   `provenance_settings_carry_overrides_and_seed`.
3. `WeatherFormat` derives no serde traits (`weather.rs:15`), so it could
   not be written to `replay.toml`: 12 IN-1 adds them.
4. OR-15's tests need a run but its Verification ran only `hares-io`: it
   now runs `-p hares-io -p hares-core metrics`, with the run tests in
   `crates/hares-core/tests/metrics_from_table.rs`.
5. IN-5's tests mixed `hares-io` spec assertions and dwelling-run
   assertions under a `hares-io` filter: split into two named test files
   and run with `--test setpoint_source`.
6. Missing dependencies: OR-6 on OR-4 and OR-14, OR-7 on OR-1 and OR-6,
   PY-6 on PY-9, PY-11 on PY-4, IN-1 on OR-6 and PY-9.
7. The setpoint warnings had no path from `hares-io` to the run's warning
   log: 12 IN-5 "The setpoint warnings are returned by
   `inject_schedule_into_specs` alongside the specs, and the dwelling
   pushes them into its warning log at assembly".
8. `Provenance` getters had no owner after PY-9: 05 PY-9 "each entry that
   adds a `Provenance` field adds its getter in its own commit", with
   PY-6, IN-1 and OR-16 each naming theirs.
9. IN-9 read a fuel economy element with no test: 12 IN-9
   `vehicle_fuel_economy_read_in_joules_per_metre`.

## Schedule moves

None required. Text changes for the schedule's owner are item 1 below.

## Items for other owners

1. `ledger/00-schedule.md` row 57 (PY-4): title "the catalogue and
   `metrics_from_table` in Python", commit
   `feat: expose the variable catalogue and metrics_from_table in Python`.
2. `ledger/06-runtime-memory.md` RT-4: the line "`DwellingBlueprint::from_config`
   is `ParsedInputs::parse` followed by `new`" becomes
   "`DwellingBlueprintBuilder::from_config` is `ParsedInputs::parse`
   followed by `new`, and `DwellingBlueprint::from_config` is that followed
   by `freeze()`" (`plan.md` §2.7); the builder's mutators and `freeze` are
   the §2.7 list; today's public blueprint fields become private, and
   `crates/hares-core/tests/equipment_id_regressions.rs:1037`
   (`blueprint_b.equipment_specs.rotate_left(1)`) is rewritten as
   `remove_equipment` of the first spec then `add_equipment_spec` of it.
3. `ledger/06-runtime-memory.md` RT-6: `ResolvedEquipment.heating_setpoint`
   and `cooling_setpoint` are copied from `EquipmentSpec.heating_setpoint_source`
   and `cooling_setpoint_source`; `SetpointSource` is a `hares-io` type
   created by IN-5 (`plan.md` §2.7).
4. `ledger/09-numeric-fixes.md` B-14 (verify-L3-e 12): extend the entry to
   the run state `plan.md` §2.1 names: `BillingState`'s energy, peak and
   demand-window fields and the metrics calculator's running totals (gas
   energy included) stored in J and W, converted with `uom` only when
   priced or written into `BillingPeriodSummary` or `RunMetrics`; the
   `BillingSnapshot` and `MetricsAccumulators` field renames shown in §2.1;
   `CHECKPOINT_VERSION` 9 to 10 with a test that a version 9 checkpoint is
   rejected; `StepResult.net_electric_power_kw` renamed
   `net_electric_power_w` with the Python step dict key, its stub and
   changes line. Its "Not in scope" paragraph keeps equipment state,
   ports, control signals and tariff rates out and drops the run state
   above from the exclusion. Its `REMOVED_KEYS` rows take scope
   `KeyScope::EquipmentConfig`. Its delta and "Numeric changes" line
   (`plan.md` §2.8) follow.
5. `ledger/09-numeric-fixes.md` B-2: its `REMOVED_KEYS` rows take scopes:
   `initialization_duration` and the top-level `initialization_duration_s`
   are `KeyScope::DwellingTable` (`plan.md` §2.7 table).
6. `ledger/09-numeric-fixes.md` B-9a: the `hpxml_dir` row is
   `KeyScope::PythonKeyword`.
7. Every numeric entry outside these files (`09-numeric-fixes.md` B-1a to
   B-14, `04-fleet.md` FL-5, `11-other-defects.md` OD-9): add its
   `docs/python.md` "Numeric changes" line to its Change or Verification,
   as `plan.md` §2.8 now requires.
8. `ledger/07-profiler.md` PR-6: none; it already matches `plan.md` §2.9.
