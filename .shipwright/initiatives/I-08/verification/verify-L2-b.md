# Verification L2-b: plan.md §2, ledger/03, ledger/05, ledger/12

HEAD `ebdb1a17` (confirmed with `git rev-parse HEAD`). Read-only. Crate graph
from the manifests: `hares-types` <- `hares-physics`, `hares-control` <-
`hares-envelope`, `hares-equipment` <- `hares-io` <- `hares-core` (also
`hares-tariff`) <- `hares-fleet`, `hares-python`. `hares-core` lists
`hares-fleet` only as a dev-dependency. The `Equipment` trait lives in
`hares-equipment` (`crates/hares-equipment/src/lib.rs:173`), which sits
below `hares-io`. §2.1's one-line graph leaves that crate out, and two of
the findings below come from that gap. Registry sources checked: pyo3
0.29.2, pyo3-ffi 0.29.2, pyo3-arrow 0.19.0, arrow 59.3.0, serde 1.0.229.

Counts: 0 blockers, 8 majors, 14 minors.

## Majors

### 1. `ResolvedParameter` is in the wrong crate for the `Equipment` trait method that returns it (§2.7)
Evidence: §2.7 puts `ResolvedParameter` in `hares-core`
(`crates/hares-core/src/resolved.rs`). It also adds "a new `Equipment` trait
method `resolved_parameters(&self) -> Vec<ResolvedParameter>`". But the
`Equipment` trait is in `hares-equipment`
(`crates/hares-equipment/src/lib.rs:173`), which cannot see `hares-core`.
As written, this does not compile.
Correction: move `ResolvedParameter { name: &'static str, value: f64, unit: Unit }`
to `hares-types` (for example `crates/hares-types/src/equipment.rs`, beside
`EquipmentHealthCounts`). In §2.7, mark it `// crate: hares-types`, and keep
`ResolvedModel`, `ResolvedEquipment`, `SetpointSource` and the other types
in `hares-core`. In §2.1, replace the one-line graph with the real one:
`hares-types <- hares-physics, hares-control <- hares-envelope, hares-equipment <- hares-io <- hares-core (+ hares-tariff) <- hares-fleet, hares-python`.

### 2. `DescribeFields`/`FieldSchema` cannot be implemented "next to its definition" for equipment configs (§2.7, IN-3)
Evidence: §2.7 defines `DescribeFields`, `FieldSchema` and `ValueType` in
`hares-io` (`input_schema.rs`). IN-3 says "each config and
override-accepting struct implements `DescribeFields` next to its
definition". The override-accepting configs are in `hares-equipment`, for
example `HeatPumpHeaterConfig` at
`crates/hares-equipment/src/hvac/heat_pump_config.rs:262` and
`GasFurnaceConfig` in `hvac/heating_config.rs`. `hares-equipment` does not
depend on `hares-io`, so it cannot name the trait. IN-7 has the same
problem for its `envelope` fields if they are declared outside `hares-io`.
Correction: move `DescribeFields`, `FieldSchema` and `ValueType` to
`hares-types` (a new `crates/hares-types/src/schema.rs`; `hares-types`
already has `serde_json` and owns `Unit`). Keep `InputSchema`,
`OverrideClassSchema`, `HpxmlElementSchema`, `HpxmlElement`,
`HPXML_ELEMENTS` and `input_schema()` in `hares-io`, and update the §2.7
code block's crate comments to match.

### 3. OR-7 writes `hares:catalogue_digest` 13 rows before PY-6 defines the digest
Evidence: OR-7 (schedule row 45) says "schema metadata carries
`hares:utc_offset_s` and `hares:catalogue_digest`". The digest's canonical
encoding and its crate (hares-core, sha2) are defined only in PY-6 (row
58). §2.5 names both keys. The schema is built in `hares-io`
(`build_schema`, OR-1), which has no `sha2` until IN-1 (row 63). An OR-7
implementer would have to invent the digest.
Correction: move the digest out of PY-6 into OR-7 (or into OR-1). Put
`Catalogue::digest(&self) -> [u8; 32]` in `hares-io` with PY-6's encoding
(name, `Unit::symbol()`, kind tag and enum labels, aggregation tag, sampling
tag, nullable byte, source tag, each followed by `0x00`), and add `sha2` to
`hares-io` in that commit. OR-7 writes the metadata key from it. PY-6 then
only copies it into `Provenance.catalogue_digest` and keeps its two digest
tests. Update §2.1's `catalogue_digest` comment ("canonical encoding in 05
PY-6") to name the new owner.

### 4. IN-6 adds `resample_overrides` to `REMOVED_KEYS` before `REMOVED_KEYS` exists
Evidence: IN-6 is schedule row 33 and says "`resample_overrides` joins
`REMOVED_KEYS`". Its test is `removed_key_resample_overrides_is_an_error
(TOML and Python)`, and its acceptance grep expects "only the
`REMOVED_KEYS` row and its test". `REMOVED_KEYS` and the Python-constructor
check that reads it are created by PY-8 (row 34; OR-3 says "the removed-key
table built by `05-python-surface.md` PY-8"). `git grep REMOVED_KEYS` at
HEAD finds nothing. Also, `resample_overrides` is a `DwellingConfig` field
and a `from_hpxml` keyword, not a `SimulationConfig` TOML key, so "TOML"
does not say which table is checked.
Correction: either (a) IN-6 creates `hares_io::config::REMOVED_KEYS` with
its one row plus the shared lookup used by the Python `from_hpxml` keyword
check, and PY-8 extends it (update PY-8's change and OR-3's "built by PY-8"
wording); or (b) move IN-6's `REMOVED_KEYS` row and its test into PY-8,
and have IN-6 only delete the machinery, with the Python keyword becoming
an unknown-keyword error until PY-8. For the test, state the TOML surface
(synthetic TOML's dwelling table) or drop "TOML".

### 5. IN-6's held weather in the PSM3 layout drops EPW fields, so it changes more than the resampling
Evidence: the parity cases use EPW weather (`tests/fixtures/parity/*/weather.epw`,
12 cases). The PSM3 reader reads only Temperature, Dew Point, Relative
Humidity, Pressure, GHI, DNI, DHI, Wind Speed, Wind Direction and the
optional Surface Albedo and Lwdown columns (`crates/hares-io/src/psm3.rs:147-226,368-392`).
Opaque sky cover, ground temperatures, liquid precipitation and design
conditions are estimated or absent for PSM3 (`weather.rs:480-515`). The EPW
reader rejects anything other than one record per hour
(`crates/hares-io/src/epw.rs:481-482`). So the held file changes the
sky-temperature and ground inputs as well as the hold, and it moves from
the EPW interval-end label to the PSM3 interval-start label (§2.7
`RecordLabel`). IN-6 still requires the oracles and parity suite to "pass
at their existing tolerances", with no step for when they do not.
Correction: specify a held format that carries every field the EPW source
has. Either (a) `hold_weather` writes PSM3 with a `Lwdown` column from EPW
horizontal infrared, and IN-6 states that opaque sky cover, ground
temperature and precipitation then come from the PSM3 estimators, records
that as part of the delta, and cites the lines; or (b) have IN-6 depend on
sub-hourly EPW reading (move it after B-1a, which adds sub-hourly files)
and emit EPW with `records per hour = 3600 / time_res_s`. In both cases,
add a stop condition: if a parity case or oracle leaves its tolerance, the
entry stops and the delta is recorded for the operator.

### 6. IN-5 misfires on generated schedules between rows 64 and 94a
Evidence: `generate_schedule_from_hpxml` writes `heating_setpoint` and
`cooling_setpoint` columns in °C from the HPXML
(`crates/hares-io/src/hpxml_schedule.rs:66-83`). OD-9 removes them only at
row 94a. From IN-5 (row 64) until then, every run with a generated schedule
and HPXML setpoints:
- gets IN-5's "column and HPXML both present" warning;
- records `SetpointSource::ScheduleColumn` for a column that came from the
  HPXML;
- under §2.4 ("a home whose ... `warnings` is non-empty is `Degraded`")
  counts as a degraded fleet home.

IN-5 does not mention generated schedules. It is only safe on the
Fahrenheit point if "at parse" means CSV parsing alone.
Correction: IN-5 states that the warning and the `ScheduleColumn` source
apply only to a schedule read from a file (`DwellingConfig.schedule` is
`Some`). Until OD-9, a generated schedule's setpoint columns record a
distinct source, for example `SetpointSource::GeneratedConstant { value_c }`,
which OD-9 then deletes, or `Hpxml` with the flattened values. They raise
no warning and are never converted from Fahrenheit. Add the test
`generated_schedule_setpoint_columns_raise_no_warning_and_are_celsius`.

### 7. IN-5 bundles a number-moving input reinterpretation into a structural commit
Evidence: IN-5 changes how `heating_setpoint`/`cooling_setpoint` column
values are read, from °C to °F converted to °C. Every user schedule with
those columns then produces different numbers. §2.8 says "no numeric
change is folded into a structural commit ... the numeric half follows as
its own commit", and the criteria require each numeric or behaviour change
to be isolated. IN-5's verification only notes that no golden has the
column. It records no delta.
Correction: split IN-5 into IN-5a (precedence, warning, recorded
`SetpointSource`; bug-compatible) and IN-5b (fix, numeric:
`setpoint_column_is_read_in_fahrenheit` and
`schedule_column_wins_over_hpxml_and_warns`'s 70 °F expectation). Give
IN-5b its own commit, its own row in the numeric block, and a recorded
delta on a fixture with a setpoint column. Also state the decision for
`water_heater_setpoint`, which OS-HPXML also specifies in °F and the column
table maps (`schedule_resolve.rs:186-189`): convert it in the same entry,
or record why not.

### 8. Between OR-3 and PY-9, Python has no way to request output
Evidence: OR-3 (row 38) removes `write_output`, `output_to_parquet`,
`output_path`, `output_verbosity` and `rotation` from Python. The Python
constructors for `ReportSpec`, `Selection`, `Frequency` and `Destination`
arrive in PY-9 (row 54). The 05 intro says OR-3 keeps "Python-facing
behaviour" apart from removing the keywords. With `reports = []`, nothing
is recorded (§2.2: "Nothing is written unless a `ReportSpec` requests
it"). So `Dwelling.simulate()`'s frame, `SimulationPlan` (reads
`dw.results()`) and every Python test that reads a frame have nothing to
return for rows 38-53. That breaks "every commit keeps every test
passing". OR-12 has the same problem for "the Python constructor keyword"
of `health_policy`, whose `HealthPolicy` class is also PY-9's.
Correction: move the constructor half of PY-9 (`ReportSpec`,
`Selection.*`, `Frequency`, `Destination.*`, `HealthPolicy`,
`HealthPolicy.strict()`, and their acceptance by `SimulationConfig(...)`)
into OR-3's commit, or into a new PY entry ordered directly before OR-3.
OR-3 migrates the Python tests to `reports=[ReportSpec(..., Destination.memory())]`
in the same commit. PY-9 keeps the result classes and the lifecycle.

## Minors

9. **serde `rc` is not enabled, but new serde types hold `Arc<str>`**
   (§2.1, §2.2, OR-1, OR-10). The workspace has `serde = { features = ["derive"] }`,
   and `cargo tree -e features -i serde` shows no `rc`. The tree already
   works around this with `ser_arc_str`/`de_arc_str`
   (`crates/hares-control/src/dispatch.rs:102-110`). The new types that
   hold `Arc<str>` all need to derive serde:
   - `TelemetryField` (via `EquipmentDescriptor`, which derives
     `Serialize, Deserialize`, `crates/hares-types/src/equipment.rs:1904`);
   - `VariableKind::Enum { labels: Arc<[Arc<str>]> }`;
   - `VariableSource::Actor(Arc<str>)`;
   - `Catalogue::from_json`;
   - the checkpointed `Warning { source: Arc<str> }`.

   Correction: in the BR-6b or OR-1 commit, add `"rc"` to the workspace
   serde features, and state that in §2.2. Or name the per-field
   `serialize_with`/`deserialize_with` helpers.
10. **`Warmup`'s TOML form cannot be derived as stated** (§2.7). "TOML:
    `warmup = "disabled"`, or a `[warmup]` table" is neither of serde's
    derived forms. Externally tagged gives `[warmup.converge]`. Untagged
    cannot take the string `"disabled"` for a unit variant, and it loses
    `deny_unknown_fields`' field-naming errors. Correction: specify a
    hand-written `Deserialize`/`Serialize` for `Warmup`: a string must be
    `"disabled"`, and a table deserialises as `WarmupConfig` with
    `deny_unknown_fields`. Specify the serde representation of every config
    enum in §2.2/§2.7 (`#[serde(rename_all = "snake_case")]` on
    `Selection`, `Frequency` and `Destination`). Also give `HealthPolicy`,
    `WarmupConfig` and `Destination::{Parquet, Csv}.overwrite`
    `#[serde(default)]`, so partial tables are legal.
11. **The rename `defaults_path` -> `defaults` is not in `REMOVED_KEYS`,
    and the Python keyword names of the new `SimulationConfig` are not
    pinned** (§2.7, PY-2, PY-8). Today's keywords are `defaults_path`,
    `time_res_s`, `duration_s`, `latitude`, `longitude`, `elevation_m` and
    `utc_offset_h` (`py_dwelling.rs:2142-2165`). The Rust fields are
    `time_res`, `duration` and `site_location: SiteLocationOverride`. PY-8
    says "`duration_s` [is] required" while §2.7 says the keyword defaults
    come from the Rust fields. Correction: in PY-8, list the Python
    keyword for every `SimulationConfig` field. Say whether `site_location`
    is a `SiteLocationOverride` pyclass or flat keywords. Add
    `defaults_path` -> `defaults` (and any other renamed keyword) to the
    §2.7 table.
12. **Observe builds under-count curve-index clamps** (OR-5).
    `air_conditioner.rs:1419-1423` and `hvac/heat_pump/heater.rs:1591-1595`
    call `take_biquadratic_clamp_count()`, which swaps the atomic to 0
    (`hvac_core.rs:1074-1079`), inside `#[cfg(feature = "observe")]`
    during `step()`. The dwelling's later `take_health_counts()` then reads
    0 under `observe`. Correction: OR-5 deletes the two observe-gated
    `BIQUADRATIC_INDEX_CLAMPED` publications, since the count is now
    `RunHealth.curve_index_clamps`, or has them read a non-resetting value.
    Run `curve_index_clamp_counted` under the `-F observe` line as well.
13. **`HealthPolicyViolation.step_index: u64` has no value for warm-up**
    (§2.3). `require_warmup_convergence` is checked when warm-up ends, and
    rollbacks can happen during warm-up. `Warning.step_index` is
    `Option<u64>` for exactly this case. Correction: make it
    `step_index: Option<u64>` (`None` during warm-up) in §2.3 and in
    OR-12's test expectation.
14. **OR-5 changes rejected-signal warnings without saying so.** Today
    every rejected signal, and every "control target not found", pushes a
    warning (`mod.rs:1180`, `1186-1189`, `1223-1233`, `1240-1245`,
    `1277`). OR-5 says "the first rejection per equipment in a run is also
    a `Warning`". Correction: state that the per-occurrence pushes at those
    sites are replaced by the counter plus a warn-once warning. Say whether
    target-not-found is counted and warned once per target. Note the
    change in the gym's per-step `take_warnings` stream in the commit body.
15. **PY-11's `OutputVariable` has no `labels`** (§2.2 says "Python actors
    declare labels at runtime"). Correction: `OutputVariable(name, unit,
    kind, aggregation=None, sampling="interval_mean", nullable=False,
    labels=None, description="")`, where `labels` is required exactly when
    `kind == "enum"`. Add the test `python_actor_enum_output_decodes_labels`.
16. **Wrong accessor for equipment telemetry fields** (§2.2, OR-1).
    Equipment telemetry fields are `EquipmentDescriptor.telemetry_fields`
    (a field, `equipment.rs:1921`), not a `telemetry_fields()` trait
    method. The `fn telemetry_fields()` items are private per-module
    helpers. Correction: say "each equipment's
    `descriptor().telemetry_fields`". OR-1 should also name the test
    `Actor` implementors that must gain `output_variables()`
    (`crates/hares-core/tests/{actor_telemetry,alignment_oracles,dispatch_ordering,safety_monitor}_regressions.rs`,
    `interest_filtering_regression.rs`, the two in `actors/ev_driver/mod.rs`,
    `actor.rs`).
17. **IN-3's literal-name grep misses most literal reads.**
    `no_literal_element_name_in_the_parser` greps only
    `.child|children_named|first_descendant|descendants("`. It misses 58
    `.path(&["..."])` calls, including in `hpxml/validation.rs`, which IN-3's
    file list omits. It also misses the `xml_helpers` calls
    `child_text`/`child_f64`/`child_energy_kwh(node, "...")`
    (`xml_helpers.rs:61-96`). Correction: extend the pattern to
    `\.(child|children_named|first_descendant|descendants|path)\(|child_(text|f64|energy_kwh)\([^,]*, *"`,
    and add `validation.rs` to the evidence list.
18. **`metrics_independent_of_destination` compares different metric
    inputs** (OR-8). While OR-8 is bug-compatible, "no report" evaluates
    availability against `Verbosity(0)`, but the `Memory` and `Parquet`
    reports' union is unspecified. Envelope metrics need verbosity 4 or
    more (`metrics.rs:593-604`). Correction: pin all three runs to a
    `Verbosity(0)` selection, or compare only the two reports and test
    "no report" separately against a `Verbosity(0)` report.
19. **`metrics_from_table` does not say how it rejects a non-`Timestep`
    table** (OR-15). An `Hourly` table passed with `time_res = 900` would
    give wrong metrics silently. Correction: the function checks that
    consecutive `Time` values differ by `time_res` and that there is no
    `Step Count` column, and otherwise returns `MetricsError` naming the
    mismatch. Add the test `metrics_from_table_rejects_non_timestep_table`.
20. **`restore_mid_hour_matches_uninterrupted_hourly_report` does not say
    which rows it compares** (OR-10). A freshly built dwelling that
    restores holds no pre-checkpoint `Memory` rows. Correction: compare
    rows whose period starts at or after the checkpoint's period start.
21. **The replay binding and test cannot land at PY-9** (PY-9 row 54;
    OR-16 row 69). PY-9 lists `RunResult.export_replay`, `hares.replay` and
    `replay_round_trip_in_python` "if OR-16 has landed", which it never has
    at row 54. OR-16 also says "Python binding in PY-9". Correction: OR-16
    owns the Python binding and `replay_round_trip_in_python`. Remove both
    from PY-9. OR-16 should also list the dwelling-level fields it writes
    (`bldg_id`, `overrides`, `patches` (`HpxmlDataPatches`, which ResStock
    runs need to reproduce), warm-up), and state how `overrides`, a
    `serde_json::Value` that may hold `null` (TOML cannot), is encoded:
    for example as an embedded JSON string or a sidecar `overrides.json`.
22. **The Python return type `pyo3_arrow.Table` does not exist** (PY-1).
    `PyTable` is `#[pyclass(module = "arro3.core._core", name = "Table")]`
    (`pyo3-arrow-0.19.0/src/table.rs:36`), and there is no `pyo3_arrow`
    Python module. Returning `PyTable` directly works without arro3
    installed, but `_hares.pyi` (PY-9's `stubs_cover_every_binding`)
    needs a real annotation. Correction: state that `arrow()` returns the
    pyo3-arrow `PyTable` class, and annotate it in the stub as a
    `typing.Protocol` with `__arrow_c_stream__`. PY-10's
    `pyo3_arrow.RecordBatch` gets the same fix (`__arrow_c_array__`, which
    exists at `record_batch.rs:183`).
23. **Citation and wording nits.**
    - PY-5: the reward is at `py_gym.rs:494`, not 493.
    - PY-6: `rerun-if-changed` paths are relative to `crates/hares-core`.
      Say they are resolved from `CARGO_MANIFEST_DIR` to the workspace
      root (`../../.git/HEAD`, and so on), because a path that does not
      exist makes cargo rerun the build script on every build.
    - §2.7 lists `ParsedHpxml`, `ParsedWeather`, `ParsedSchedule`,
      `ParsedDefaults`, `ForcingKey`, `ForcingTable`,
      `DwellingBlueprintBuilder` and `ObservationPlan` as unit structs.
      State that they are opaque: private fields, and a public API limited
      to the listed methods.
    - `TariffSnapshot`, `MetricsSnapshot` and `ReducerSnapshot` have
      fields only in OR-10's prose, although §1 says every type is defined
      in §2. Add them to §2.1 with their fields.
    - IN-4's Python `resample_weather(weather, site, config)` has no
      Python `SiteLocation` type, and IN-2 returns a dict. Say that `site`
      is that dict's `site` entry, or add a pyclass.
24. **L1-resolution-b row 2 cites a test that is not in the ledger.** It
    lists `soc_out_of_bounds_fails_in_release` as added. OR-5 instead
    deletes `check_soc` as vacuous and relies on
    `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`. The
    reasoning holds: `Soc::try_from` bounds [0, 1] at
    `equipment.rs:1134-1140`, and `check_soc` only warns
    (`invariants.rs:313-329`). The resolution record is wrong, not the
    plan. Correction: amend L1-resolution-b row 2 to name the substitution.

## Verified correct

- Crate homes for the other §2 types are consistent with the graph:
  - in `hares-types`: `HealthEvent`, `EquipmentHealthCounts`, `Unit`,
    `VariableKind`, `Aggregation`, `Sampling`, `TelemetryField`,
    `CountingAllocator`;
  - in `hares-io`: `VariableDef`, `Catalogue`, `ReportSpec`, `Selection`,
    `Frequency`, `Destination`, `BatchCallback`, `HealthPolicy`, `Warmup`,
    `InputSource`, `WeatherInput` (`WeatherFormat` exists at
    `weather.rs:16`), `InputRecords`, `HpxmlElement`;
  - in `hares-core`: `RunResult`, `Provenance` (holding `SimulationConfig`
    and `InputRecords` from `hares-io`), `RunHealth`, `ResolvedModel`
    (`EndUse` from `hares-types`, `BoundaryType` at
    `hpxml/building.rs:59`, `SiteLocation` at `site_location.rs:159`);
  - in `hares-fleet`: `FleetReport` (with `Selection` and `Frequency`).

  `ZoneId` and `EquipmentId` are in `hares-types` (`environment.rs:96`,
  `equipment.rs:19`). The `HaresError` derives
  (`Clone, PartialEq, Serialize, Deserialize`, `error.rs:8`) accept the
  new variants provided `HealthEvent` derives the same.
- `BatchCallback: Send + Sync` with `on_batch(&mut self, &BatchContext<'_>, &RecordBatch)`
  is object safe. `Box<dyn BatchCallback>` keeps `PyDwelling` `Sync`, which
  pyo3 0.29 requires for `#[pyclass]` (pyo3 CHANGELOG, #4566). `Py<PyAny>`
  and `PyErr` are `Send + Sync`. `Actor: Send + Sync + 'static`
  (`actor.rs:44`) accepts a required `output_variables(&self) -> &[TelemetryField]`.
- pyo3 0.29.2: `Python::attach` (`marker.rs:414`), `Python::detach` with
  `F: Ungil + FnOnce() -> T, T: Ungil` (`marker.rs:562`), which matches
  PY-3's `construct_detached<T: Send>`, and `pyo3::ffi::PyGILState_Check`
  (`pyo3-ffi cpython/pystate.rs:61`).
- pyo3-arrow 0.19.0 depends on arrow-* 59, pyo3 0.29 (chrono, chrono-tz,
  indexmap features), numpy 0.29 and thiserror 1. `PyTable::try_new`,
  `batches()` and `from_arrow_pycapsule` exist. `__arrow_c_stream__`
  takes `&self` and clones the batches, so it is repeatable (table.rs:263).
  `PyRecordBatch` has `__arrow_c_array__`. `concat_batches` comes from
  `arrow_select::concat`.
- The decision log is reflected:
  - results: one `RunResult`, no history, no fallback frame, OR-9, no
    per-step callback;
  - output model: catalogue -> request -> row plan -> destination;
  - destinations: runtime-attached callback, temporary name published on
    finish, nothing in the working directory;
  - strict config with one type;
  - health recorded in every build, invariants in every profile including
    warm-up, with a stop condition;
  - per-run counters;
  - `metrics: bool`, default false;
  - frequencies and aggregation kinds including Mode with lowest-code ties;
  - package `hares-py`/`hares`/`~/.cache/hares` (the XDG logic exists at
    `weather.py:32-36`, `resstock.py:124-127`; 787 `ochre[_-]next`
    occurrences confirmed);
  - content inputs, `validate()`, input schema including HPXML elements,
    replay, `metrics_from_table`, `resample_weather`;
  - the OS-HPXML setpoint precedence (the column wins; the warning covers
    the simple setpoint only; fetched from `hvac.rb` master);
  - `ochre_compat` leaving the core, `defaults_path`.
- 12-inputs evidence:
  - AFUE bound (`heating_config.rs:87-90`);
  - `run_warmup_converged(0.5, 25)` (`mod.rs:2740`);
  - `WeatherTimeSeries::resample` (`weather.rs:568`);
  - `ochre_compat` (`weather.rs:431-446`);
  - `inject_setpoint_schedules` (`schedule_resolve.rs:655`) and
    `set_typed_setpoint_source` (`:695`);
  - the setpoint column table (`:176-190`);
  - `effective_setpoints` (`thermostat.rs:269`);
  - nearest-R selection (`envelope_lut.rs:583-590`);
  - `extract_insulation_details` returning `None` (`building.rs:2215`);
  - override functions (`conversions.rs:858,906`);
  - `HpxmlDataPatches` holding only `number_of_bedrooms`;
  - fleet `resample_overrides: None` (`fleet.rs:207,1178,1195`);
  - `tests/parity/mod.rs:484`;
  - the IN-6 census, exactly 189 lines in 39 files;
  - `OCHRE_COMPAT_RESAMPLE` with 7 fields;
  - the reserved `"zip"` override key (`conversions.rs:694-790`).
- 03/05 evidence re-checked:
  - `SimulationResults.steps` (`mod.rs:860`);
  - `FullSimulationMetrics` and its `Deref` (`metrics.rs:237-249`),
    `Reliability` (`:186`);
  - `DwellingCheckpoint` (`checkpoint.rs:74`);
  - `zone_display_name` (`mod.rs:806`);
  - `default_output_path` (`conversions.rs:590`);
  - the diagnostics gate (`mod.rs:2790`);
  - the `initial_state` sites 624, 652, 662, 673, 1528, 1954;
  - `rollback_failed_equipment_ports` (`mod.rs:5168`),
    `run_warmup_converged` (`:5003`);
  - the fallible-caller sites 3167, 3574, 3605 and `refresh_equipment_caches` (3794);
  - the test seams (`mod.rs:1536`, `2943`), `FailingPortEquipment` (`:13849`),
    and the drift test (`:15226`);
  - schema metadata (`columns.rs:916-927`);
  - `chrono_to_py_datetime` (`hares-python/conversions.rs:165-169`);
  - the statics (`hares-equipment/lib.rs:35,38`; `py_gym.rs:208,212`;
    `telemetry.rs:20`), the release stubs (`invariants.rs:705`);
  - `KNOWN_KWARGS` (`py_dwelling.rs:2142-2165`), the Python default
    constants (`py_config.rs:17-18`, `py_dwelling.rs:61-62`);
  - `SyntheticSimulationConfig` (`synthetic.rs:125-131`),
    `SyntheticOutputConfig` (`:291`), `HeatPumpHeaterConfig` flatten
    (`heat_pump_config.rs:262`);
  - `ConfigError` and `time_res_secs_u32`'s `unwrap_or(3600)`
    (`config.rs:139-141`).
- verify-L1-b findings 1-23 and 25-30, which L1-resolution-b marks fixed,
  are fixed in the current text:
  - 13 uses the test name `capsule_export_is_bitwise_identical` in place of
    `capsule_export_matches_golden_bitwise`, which is equivalent;
  - 30 now reads "which `[advisories]` does not check", consistent with
    `dl-i08-architecture.dependencies`;
  - 2 is fixed except for the test name recorded in minor 24;
  - 24 now lists all six sites;
  - 26 is fixed for `master_seed` and `TypeError`, but see minor 11 for
    `defaults_path`.
- Schedule order holds for OR-1 -> OR-2 -> OR-3 -> OR-13/14/4/12 -> OR-6 ->
  OR-7 -> OR-8 -> OR-10 -> OR-15; PY-9 -> PY-1 -> PY-3/4/6/10/11; RT-4 ->
  IN-1 -> IN-5 -> RT-6 -> IN-2/IN-7 -> IN-3 -> IN-4 -> OR-16, except where
  majors 3, 4 and 8 and minor 21 say otherwise.
- Tests are deterministic (no timing, threads or git inside tests). PY-3's
  GIL check and PY-6's synthetic `.git` directories meet the test rules.
