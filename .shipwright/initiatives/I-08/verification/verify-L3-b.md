# Verification L3-b: design frame, output reporting, Python surface, inputs

Scope: `plan.md` §2 (design frame and public contracts), `ledger/03-output-reporting.md`,
`ledger/05-python-surface.md`, `ledger/12-inputs.md`, against the code at HEAD `8b09ac4a`
(branch `feat/output-results`; evidence cited "at `ebdb1a17`" or "at `872cf289`" checked at
that commit with `git show`), the binding decisions in `decision-log.md` (read in full), the
order in `ledger/00-schedule.md`, and the crate sources under `~/.cargo/registry`
(pyo3 0.29.3, pyo3-macros-backend 0.29.3, pyo3-arrow 0.19.0, arrow / arrow-select 59.3.0,
serde_derive 1.0.229, uom 0.38.0).

Counts: 0 blocker, 14 major, 17 minor.

## Findings

### Major

**1. OR-4 moves the diagnostics preamble into `Provenance.equipment` five rows before `Provenance` exists.**
Entry: `03` OR-4 (schedule row 41). Evidence: OR-4 Change "The equipment preamble moves to
`Provenance.equipment` (`plan.md` §2.1)" and test `provenance_carries_equipment_zone_mapping`;
`Provenance` and `EquipmentProvenance` are created by OR-6 (`03` OR-6 Change, first bullet),
schedule row 43. The code has no `Provenance` (`git grep -n 'struct Provenance' HEAD` is empty).
Correction: move the preamble half of OR-4 (the `Provenance.equipment` sentence and
`provenance_carries_equipment_zone_mapping`) into OR-6's Change and Acceptance tests, and state
in OR-4 that until OR-6 the preamble is dropped with the CSV writer (or move OR-4 to directly
after OR-6 in `00-schedule.md` and add OR-6 to OR-4's Dependencies).

**2. OR-14's acceptance test reads `Provenance` three rows before it exists.**
Entry: `03` OR-14 (row 40). Evidence: test `provenance_records_callback_destination_marker`;
`Provenance` arrives with OR-6 (row 43). Correction: move that test to OR-6 (which then asserts
`Provenance.config.reports` keeps the `Callback` marker), or replace it in OR-14 with
`validated_config_keeps_callback_marker` over `SimulationConfig` after `validate()`.

**3. OR-15's Python binding needs PY-1 and PY-4, which run after it.**
Entry: `03` OR-15 (row 49). Evidence: OR-15 Python half: `hares.metrics_from_table(table,
catalogue, ...)` where `table` is "any object with `__arrow_c_stream__`" and `catalogue` is the
Python `Catalogue`; test `metrics_from_table_matches_run_metrics_in_python` uses "the report's
exported table". The stream import needs `pyo3-arrow` (added to `crates/hares-python/Cargo.toml`
by PY-1, row 55; `hares-python` has no `pyo3-arrow` today), the export is PY-1's `arrow()`, and
the `Catalogue` pyclass is PY-4's (row 57). Correction: move OR-15's Python half (the binding,
its stub, its docs line and `metrics_from_table_matches_run_metrics_in_python`) into PY-4's
commit (which follows PY-1), and add "the Python `metrics_from_table` binding" to PY-4's Change
and Acceptance tests; or move OR-15 in `00-schedule.md` to directly after PY-4 and add PY-1 and
PY-4 to its Dependencies.

**4. PY-9 wraps `BuildInfo` fifteen rows before PY-6 creates it.**
Entry: `05` PY-9 (row 43a). Evidence: PY-9 Change "Frozen pyclasses over the Rust types:
`RunResult`, ... `Provenance`, `BuildInfo`"; `BuildInfo` and `Provenance.build` are added by
PY-6 (`05` PY-6 Change; `03` OR-6 "`Provenance.build` ... added by PY-6"), row 58; PY-6 also
adds `hares.build_info()` "as a frozen pyclass". Correction: delete `BuildInfo` from PY-9's
pyclass list and add to PY-6's Change "the `BuildInfo` pyclass and `Provenance.build` getter,
with their stubs and `docs/python.md` changes line".

**5. `export_replay(&self)` cannot write the dwelling settings it must write: `Provenance` does not carry them.**
Entry: `plan.md` §2.1 `Provenance`; `03` OR-16. Evidence: OR-16 writes "every `DwellingSettings`
field other than `overrides`: `bldg_id`, `patches` ... `initialization_duration`" into
`replay.toml` and `overrides` into `overrides.json`, from `RunResult::export_replay(&self, ...)`;
`RunResult` holds only `reports, metrics, health, warnings, warnings_dropped, catalogue,
provenance`, and `Provenance` holds `build, catalogue_digest, config, bldg_id, inputs,
site_utc_offset_s, equipment`: no overrides, patches or warm-up duration. Correction: in §2.1
replace `pub bldg_id: i64` with `pub settings: DwellingSettings` (the §2.7 type, `hares-core`,
which already holds `sim_config`, `bldg_id`, `overrides`, `patches`, `initialization_duration`)
and drop the duplicated `config` field (or keep `config` and add `overrides:
Option<serde_json::Value>`, `patches: Option<HpxmlDataPatches>`, `initialization_duration:
Option<StdDuration>` until B-2); name OR-6 as adding `bldg_id`/`config` and RT-4 (which creates
`DwellingSettings`) as switching to `settings`.

**6. A replayed content-sourced weather input has no format.**
Entry: `plan.md` §2.7 `InputRecords`; `12` IN-1; `03` OR-16. Evidence: `WeatherInput.format` is
"required for Text/Bytes"; `InputRecords.weather: InputRecord { source, sha256 }` drops it;
OR-16 writes "the weather content verbatim" and `replay::load` must rebuild a `WeatherInput`.
Format detection works only from a path's extension plus CSV header sniffing
(`crates/hares-io/src/weather.rs:22-43`, `detect_weather_format(path)`), so the exporter cannot
name the file and the loader cannot supply `format`. Correction: in §2.7 add
`pub weather_format: WeatherFormat` (the resolved format, never `None`) to `InputRecords`,
filled by IN-1 from the parsed handle; OR-16 writes it to `replay.toml` and passes it as
`WeatherInput.format` on load; add IN-1 test `weather_record_carries_resolved_format` and an
OR-16 test `replay_of_text_weather_round_trips` (the cz4a EPW passed as `Text`).

**7. The closed billing summaries have no serialisable checkpoint type.**
Entry: `plan.md` §2.1 checkpoint sections; `03` OR-10. Evidence: OR-10 "`hares-core` the closed
billing summaries"; `BillingPeriodSummary` (`crates/hares-tariff/src/billing.rs:409-420` at HEAD)
derives nothing and holds `period_start`/`period_end: DateTime<Tz>`; workspace `chrono-tz` has no
`serde` feature. §2.1 defines `TariffSnapshot`, `BillingSnapshot`, `LocalInstant` but no summary
snapshot. Correction: add to §2.1 (crate `hares-tariff`, `snapshot.rs`)
`pub struct BillingPeriodSummarySnapshot { pub period_start: LocalInstant, pub period_end:
LocalInstant, pub energy_charge_usd: f64, pub demand_charge_usd: f64, pub fixed_charge_usd: f64,
pub export_credit_usd: f64, pub net_bill_usd: f64, pub peak_demand_kw: f64, pub
total_import_kwh: f64, pub total_export_kwh: f64 }` and a `billing_summaries:
Vec<BillingPeriodSummarySnapshot>` section (in `TariffSnapshot` or `DwellingCheckpoint`), and
change OR-10's owner text from `hares-core` to `hares-tariff`.

**8. IN-5 records and asserts `SetpointSource`, a `hares-core` type that RT-6 creates after it.**
Entry: `12` IN-5 (row 64), `plan.md` §2.7. Evidence: §2.7 places `SetpointSource` in
`crates/hares-core/src/resolved.rs` with `ResolvedModel` (RT-6, row 65); IN-5's tests assert
"`SetpointSource` is `ScheduleColumn { column: "heating_setpoint" }`" and
"`SetpointSource::GeneratedConstant { value_c }`" at IN-5's commit; IN-5 records the source in
the resolved equipment configuration built in `hares-io` (`schedule_resolve.rs:695-713`
`set_typed_setpoint_source`, which stores `ScheduleSourceConfig::ColumnRef { col_idx, .. }`: a
column index, not a column name, and nothing that marks a generated constant). `hares-io` cannot
name a `hares-core` type. Correction: move `SetpointSource` in §2.7 to `hares-types`
(`crates/hares-types/src/resolved.rs`, beside `ResolvedParameter`); IN-5 creates it and stores the
per-side value on the thermostat/HVAC config it already writes, exposed through an `Equipment`
accessor or the `EquipmentSpec`; RT-6 only copies it into `ResolvedEquipment`. IN-5's tests read
it from that accessor.

**9. `hares-types` does not depend on `serde_json`, contrary to §2.7.**
Entry: `plan.md` §2.7 ("hares-types already depends on serde_json and owns Unit"); `12` IN-3.
Evidence: `crates/hares-types/Cargo.toml` lists `serde_json` only under `[dev-dependencies]`.
`FieldSchema.default: Option<serde_json::Value>` therefore does not compile in `hares-types`.
Correction: replace the §2.7 clause with "IN-3 adds `serde_json = { workspace = true }` to
`hares-types` `[dependencies]`", and add that manifest change to IN-3's Change.

**10. `hares_io::input_schema()` cannot describe `DwellingConfig`, which lives in `hares-core`.**
Entry: `plan.md` §2.7 `InputSchema`; `12` IN-3 (and IN-7's `envelope` fields). Evidence:
`InputSchema.dwelling_config: Vec<FieldSchema>` is built by `hares_io::input_schema()`; IN-3's
rule "each config ... implements `DescribeFields` next to its definition" puts the
`DwellingConfig`/`DwellingSettings` impl in `hares-core` (`crates/hares-core/src/dwelling/mod.rs:206`),
which `hares-io` cannot see (`hares-io` has no `hares-core` dependency; the graph in §2.1). IN-7
does not say which crate defines the `envelope` override struct; the override map is applied in
`hares-core` (`crates/hares-core/src/dwelling/conversions.rs:858,906`). Correction: make the
function `hares_core::input_schema() -> InputSchema` (the types stay in `hares-io`), and state in
IN-7 that the envelope scales struct and its `DescribeFields` impl live in `hares-io`
(`crates/hares-io/src/envelope_scales.rs`) so both the schema and the override validation see it.

**11. One flat `REMOVED_KEYS` table cannot express `initialization_duration_s` removed in `[simulation]` but valid at top level.**
Entry: `plan.md` §2.7 table; `12` IN-6 (lookup); `05` PY-8. Evidence: the §2.7 row "synthetic
TOML `[simulation]` `initialization_duration_s` -> top-level `initialization_duration_s`" (PY-8);
IN-6: "the TOML loaders check each top-level key of a `SimulationConfig` table and of a synthetic
TOML file's dwelling table against it"; after PY-8 the dwelling-table key
`initialization_duration_s` is the valid replacement (`tests/fixtures/bestest/*.toml` carry it,
migrated by PY-8), so the same lookup rejects it with a message naming itself. The table's type
`&[(&str, &str)]` and `removed_key(key)` carry no scope. Correction: give each row a scope:
`pub static REMOVED_KEYS: &[RemovedKey]` with `pub struct RemovedKey { pub scope: KeyScope,
pub key: &'static str, pub replacement: &'static str }` and `pub enum KeyScope {
SimulationConfig, DwellingTable, PythonKeyword, EquipmentConfig }`, lookup
`removed_key(scope, key) -> Option<&'static str>`; the `[simulation]` row is
`KeyScope::SimulationConfig`, B-2's top-level row is `KeyScope::DwellingTable`; add test
`toplevel_initialization_duration_is_accepted_after_py8`.

**12. Site-location range checks live only in the Python constructor, so TOML and Rust configs skip them.**
Entry: `05` PY-8. Evidence: PY-8 "`SiteLocationOverride` ... a frozen pyclass ... whose
constructor applies today's finiteness and range checks (`py_dwelling.rs:2208-2265` at
`872cf289`)"; those checks exist only in `extract_site_location_override`
(`crates/hares-python/src/py_dwelling.rs:2208-2265` at `872cf289`), and PY-8's
`validate()` list does not include them. `dl-i08-architecture.config`: "One Rust config type with
one validate() ... Strict everywhere". Correction: add to PY-8's `validate()` list "site_location:
each set field finite; latitude in [-90, 90], longitude in [-180, 180], elevation_m in
[-500, 9000], utc_offset_h in [-14, 14]"; the Python constructor calls the same Rust check; add
test `toml_site_location_out_of_range_is_a_validate_error` beside
`site_location_override_round_trips`.

**13. OR-11 deletes the `check_invariants` feature but leaves user documentation telling users to build with it.**
Entry: `03` OR-11. Evidence: census and acceptance cover `'*.rs' '*.toml'` and `ci.yml` only.
At HEAD, `git grep -n check_invariants` also hits `README.md:49` (`cargo build --release -F
check_invariants`), `CONTRIBUTING.md:90`, `docs/development.md:181-197,461-462` (seven lines),
`docs/invariants-and-observability.md` (eight), `docs/physics/exterior-surface-balance.md`,
`docs/validation.md`, `scripts/triage.py:191`, and the docstrings of
`tests/python/test_actor_param_boundary.py:13`, `test_non_finite_input_boundaries.py:8`,
`test_solar_override_boundary.py:12`. After the commit those commands fail. Correction: OR-11's
census becomes `git grep -n check_invariants -- . ':!.shipwright'` (the moved planning records are
already untracked by SL-8), its Change rewrites each file above to the always-on behaviour, and
its first acceptance criterion is that census printing nothing.

**14. A Python actor cannot declare a `Quantity` variable: `OutputVariable` has no across-homes rule.**
Entry: `05` PY-11 (and PY-4's `Variable`). Evidence: `plan.md` §2.2 `VariableKind::Quantity {
across_homes: AcrossHomes }`, "Every `Quantity` declares its rule across homes";
`dl-i08-architecture.aggregation`: "Every quantity variable declares its across-homes rule".
PY-11's `OutputVariable(name, unit, kind, aggregation=None, sampling=..., nullable=False,
labels=None, description="")` has no such parameter. Correction: add `across_homes=None`
(`"sum"` or `"mean"`), required exactly when `kind == "quantity"` and a `ValueError` otherwise
(the `labels` rule's shape); add test `python_actor_quantity_without_across_homes_is_an_error`;
add `across_homes` (or `None`) to PY-4's `Variable` attributes.

### Minor

**15. §2.1 crate table: `hares-python` does not depend on `hares-envelope`.** Evidence:
`crates/hares-python/Cargo.toml` `[dependencies]` lists core, fleet, control, equipment, io,
physics, tariff, types. Correction: the `hares-python` row reads "every crate above except
`hares-envelope`".

**16. OR-1's across-homes list does not cover every unit in its census.** Evidence: the
census at `ebdb1a17` also yields `"A"` (`CURRENT_A`), `"Ah"` (`AH_CELL`), `"%"`
(`CAPACITY_FADE_PCT`), `"bytes"` (`PAYLOAD_SIZE_BYTES`) and `"-"` (96 sites), none placed by "irradiance,
durations, efficiencies, voltage and specific heat Mean; flows, counts and volumes Sum", so
`every_quantity_declares_across_homes_and_power_units_are_power` has no expected value for them.
Correction: extend the list: `%` and `-` ratios are kind `Ratio`; `A` and `Ah` `AcrossHomes::Sum`;
`bytes` `AcrossHomes::Sum`; or state each `"-"` variable's kind in the pinned table.

**17. OR-1's catalogue test sits in `hares-io` but needs `hares-core` actors and assembly.**
Evidence: `catalogue_schema_equals_pinned_schema_at_every_verbosity` in
`crates/hares-io/tests/catalogue.rs` includes "one of each built-in actor" (`hares-core`) and the
assembled catalogue; `hares-io` has no `hares-core` dependency. Correction: place it in
`crates/hares-core/tests/catalogue.rs` with pins under `crates/hares-core/tests/fixtures/schema/`.

**18. Enum codes that are not zero-based, and a three-valued "connection" called binary.**
Evidence: `CHARGING_LEVEL` publishes codes 1 and 2 (`ev/telemetry.rs:51-53` at `ebdb1a17`),
while `VariableKind::Enum` decodes code `i` to `labels[i]`; `CONNECTION_STATE` is 0, 1, 2
(`ev/telemetry.rs:45-48`) while §2.2 lists "connection" as a binary `State`. Correction: OR-1
states that a non-zero-based code declares a placeholder label at each unused index (for
`CHARGING_LEVEL`: `["none", "L1", "L2"]`), and §2.2's `State` examples drop "connection".

**19. §2.5's deletion of three schema metadata keys has no owning entry.** Evidence: §2.5
deletes `hares_mode_map`, `basement_lighting_created`, `garage_lighting_created`
(`columns.rs:911-923` at HEAD, tested at `:1321-1344`); no ledger entry names them, and
`hares_verbosity` (`:911`) is not addressed. Correction: add both to OR-7's Change (deleted with
their tests; `hares_verbosity` deleted since a schema can mix selections) with test
`schema_metadata_holds_only_offset_and_digest`.

**20. OR-2 defines `ReportSpec` (which holds a `Destination`) before OR-3 adds `Destination`.**
Correction: OR-2 adds `Destination::{Memory, Parquet, Csv}` and states the implicit report's
destination is derived from `write_output`/`output_format`/`retain_batches` exactly as today's
recorder chooses, or OR-2 keeps the row plan's selections internal and `ReportSpec` lands in OR-3.

**21. OR-3 and OR-14 publish at `finish()`, which OR-6 introduces later.** Correction: both say
"at the end of `simulate()` (today's `flush_and_close`) until OR-6, then at `finish()`".

**22. "Metrics always computed until OR-8" is false without a recorder.** Evidence: PY-9 text;
metrics come from flushed batches (`metrics.rs:641`, `writer.rs:317`), so a run with
`reports=[]` has none. Correction: OR-6 and PY-9 state `RunResult.metrics` is `Some` exactly when
the run has at least one report until OR-8.

**23. PY-1 details.** The `OnceLock<RecordBatch>` is not a field of §2.1's `ReportResult`; say it
lives on the `hares-python` `ReportResult` pyclass. `hares-python` depends on `arrow`, not
`arrow-select`; name `arrow::compute::concat_batches` (re-export of
`arrow-select-59.3.0/src/concat.rs:613`). State what `arrow()`/`table()` do for `File` and
`Callback` outputs (raise `HaresSimulationError` naming the destination).

**24. PY-10 uses `pyo3-arrow`'s `PyRecordBatch` but does not list PY-1 among its Dependencies.**
Correction: add PY-1.

**25. PY-8's Python constructor and validate list.** The removed-key messages for `duration_s`,
`time_res_s` and the site keywords need a `**unexpected` catch on `SimulationConfig(...)` (pyo3
otherwise raises its own `TypeError` first); `python_keyword_names_equal_rust_field_names` must
skip the `VAR_KEYWORD` parameter. PY-8's `validate()` list includes report and frequency checks
whose fields arrive in OR-3 and OR-13; move them to those entries' text.

**26. Workspace `float_roundtrip` (OR-10) changes wheel JSON parsing.** `tools/frame_golden`
already enables it (its `Cargo.toml`), so goldens are unaffected, but `hares-python` builds
(tariff JSON at `py_tariff.rs:141,154,707,717`, override JSON) parse differently afterwards.
Correction: OR-10 records that difference and runs the Python suite in its Verification.

**27. OR-16 replay limits.** `BuildInfo` (`&'static str` fields) cannot be deserialised, so
`replay::load` must read `replay.toml` into a type without it (say so); a tariff attached with
`Dwelling::set_tariff` is not exported. Correction: state that `export_replay` fails naming the
tariff when one is attached (or records it, with the tariff JSON and zone).

**28. IN-9 does not say which further vehicle elements are read.** `FuelEconomyCombined`
(`bldg0176775/home.xml`, kWh/mile) maps naturally to `EvConfig.fuel_economy_kwh_per_mi`;
`MilesDrivenPerYear`, `HoursDrivenPerWeek` and the schedule fractions are present too.
Correction: list each as read (with its `EvConfig` field and unit) or not read (and why), so
`HPXML_ELEMENTS` is decided.

**29. IN-4's Python `site` dict lacks `FieldSource` values that `SiteLocation` requires.**
Correction: state the sources the binding sets (`CallerOverride`) when building `SiteLocation`
from the dict.

**30. OR-13's reducer is `pub` in `hares-io` (for `hares-fleet`) while §2.9 calls it not part of
any public contract.** Correction: §2.9 says "public items of `hares-io`, not a stability contract".

**31. OR-13 `partial_first_and_last_period_carry_step_count` names no frequency.** Correction:
`Hourly`, 900 s steps, start 00:30, end 23:15, expecting `Step Count` 2 in the first row and 1 in
the last.

## Verified correct

- Crate graph in §2.1 matches every manifest except finding 15; `hares-io` has no pyo3; the
  `Equipment` trait is `crates/hares-equipment/src/lib.rs:173`; `HaresError` at
  `crates/hares-types/src/error.rs:9` derives `Clone, PartialEq, Serialize, Deserialize`.
- Committed OR-5 state matches §2.3: `HealthEvent` and `EquipmentHealthCounts` in
  `crates/hares-types/src/health.rs`; `RunHealth`, `WarmupOutcome`, `WarmupResiduals` in
  `crates/hares-core/src/health.rs` with the §2.3 fields; `Dwelling::health()` and
  `record_clamped_actions(&mut self, u64) -> Result<()>`; `take_health_counts` default on the trait.
- `TelemetryField` at `equipment.rs:1869-1874`, `EquipmentDescriptor.telemetry_fields`,
  `ser_arc_str`/`de_arc_str` at `dispatch.rs:102-110`, `Actor` at `actor.rs:44`, `TestActor` at
  `:477`, and every `Actor` implementor OR-1 lists (plus `PyActorWrapper`, `py_actor.rs:99`).
- serde: `deny_unknown_fields` on an enum applies to its struct variants (serde_derive 1.0.229
  `de/struct_.rs` uses the enum's container attributes); `rc` gives `Arc<str>` and
  `Arc<[Arc<str>]>`; `#[default]` on `Frequency::Timestep` is valid.
- pyo3 0.29.3: `Python::attach` (`marker.rs:414`), `Python::detach` (`:562`),
  `ffi::PyGILState_Check` (`pyo3-ffi cpython/pystate.rs:80`, no abi3 in this build); `#[new]`
  constructors get an auto-generated `__text_signature__` (`pymethod.rs:1603-1612`).
- pyo3-arrow 0.19.0: depends on arrow 59, pyo3 0.29, numpy 0.29, thiserror 1; `PyTable` is
  `#[pyclass(module = "arro3.core._core", name = "Table", subclass, frozen)]` at `table.rs:36`,
  `from_arrow_pycapsule` public at `:57`, repeatable `__arrow_c_stream__` at `:263`;
  `PyRecordBatch::__arrow_c_array__` at `record_batch.rs:183`.
- uom 0.38: `degree_fahrenheit` and `degree_celsius` exist on `ThermodynamicTemperature`; `hares-io`
  depends on `uom`.
- `sha2` 0.11 is in the workspace and in `hares-core`; `hares-io` gains it in OR-7 as stated.
- `SimulationConfig` (`config.rs:62` at `ebdb1a17`), `RotationPolicy` (`:37`), the
  `unwrap_or(3600)` (`:141`), the five `from_toml` checks; `DwellingConfig` fields
  (`mod.rs:210-224`); `StepResult` (`mod.rs:846`); `FullSimulationMetrics` (`metrics.rs:237`),
  `Deref` (`:244`), `metrics_reliability` (`:230`), `MetricsCalculator` (`:361`),
  `total_electric_energy_kwh` (`:391`); `BillingState` fields equal `BillingSnapshot`'s list;
  `TariffEvaluator.step_index` (`evaluator.rs:26`); `CHECKPOINT_VERSION = 8` at HEAD;
  `SiteLocationOverride` (`site_location.rs:134`), `SiteLocation` (`:159`); `BoundaryType`
  (`building.rs:59`); `EndUse` in `hares-types`.
- The OR-7 Time evidence (`columns.rs:425`, `mod.rs:7009-7010`, `writer.rs:379`,
  `aggregation.rs:444,505,508,636-637`, `conversions.rs:165-169`).
- OR-11's census (504 lines in 117 files at `872cf289`; HEAD adds one comment line at
  `mod.rs:6467`, covered by the rename row) and `ci.yml:155`.
- OR-3's diagnostics citation (`mod.rs:2819-2830` at `872cf289`); OR-9's six sites at
  `ebdb1a17`; PY-2's 22 `KNOWN_KWARGS` at `872cf289:py_dwelling.rs:2183-2206`; PY-8's and PY-3's
  citations at `ebdb1a17`.
- IN-3's literal-name grep prints 440 lines at `872cf289` and at HEAD; accessor locations
  (`building.rs:303-336`, `xml_helpers.rs:61-173`).
- IN-6's census (189 lines in 39 files outside the planning records), `ensure_hourly_data_period`
  (`epw.rs:468`, called at `:172`), `ochre_compat` (`weather.rs:431`).
- IN-9's fixture evidence (`bldg0176775/home.xml:1814-1857`: 63.433 kWh usable, 5690.0 W,
  level 2); the `electric vehicle charging` entries in both EV fixtures are panel
  `ServiceFeeder`s, not plug loads, so the double-count rule does not reject them.
- Golden manifests: `full_year_hourly` and the cz4a fixtures are Denver (`-7.0` in the EPW
  header), so `utc_offset_s == -25200` and 8760 steps hold; `consumer_shape_900s` carries
  `bldg_id` 176227, the `Gas Water Heater` override and a replaced defaults file; frame_golden
  pins field name, type and nullability only (`digest.rs:407-417`), so OR-1's `unit` metadata
  change passes `compare-all`; `frame_golden compare --columns` exists.
- Named fixtures exist: `tests/fixtures/urdb/{flat_rate,pge_e_tou_c}.json`,
  `tests/fixtures/bestest/600.toml`, `bestest_dwelling()` and `FailingPortEquipment` in
  `mod.rs`, `tests/fixtures/hpxml/ochre_samples/base.xml` (the only `base.xml`), the PY-5 inputs,
  the IN-9 weather files, `python/ochre_next/_hares.pyi` at `ebdb1a17`.
- Schedule order holds for every other dependency in scope (OR-1 after OR-11 and M-2; OR-3 after
  IN-6, PY-8, PY-2; PY-9 after OR-6 and OR-12; OR-8, OR-10, OR-16, PY-1, PY-3, PY-4, PY-6, IN-1 to
  IN-9 after their stated dependencies).
- No em dashes or listed tics in the four scoped documents.
