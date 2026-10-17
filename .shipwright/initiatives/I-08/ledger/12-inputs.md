# Inputs: records, validation, input schema, weather channels

Implements the parts of `plan.md` §2.7 "Inputs" and "Other built-now
capabilities" that `06-runtime-memory.md` does not: input digests for
provenance and replay, `DwellingBlueprint::validate()`
(`dl-i08-architecture.new_capability`: `validate()`), the machine-readable
input schema, and `resample_weather`. Content inputs and the shared,
reusable blueprint are `06-runtime-memory.md` RT-4; resolved-parameter
read-back is RT-6.

It also holds three input-contract entries the decision log adds: IN-5,
the setpoint-source rule (`dl-i08-architecture.setpoint_sources`); IN-6,
which takes the OCHRE resampling overrides out of the Rust core
(`dl-i08-architecture.ochre_compat`); and IN-7, the envelope scales that
never re-select a LUT row (`dl-i08-architecture.calibration_r_factor`).
And two found input defects that move numbers, each its own numeric entry
(`plan.md` §2.8): IN-8, schedule-file setpoint columns read in degrees
Fahrenheit, and IN-9, HPXML vehicles and their chargers.

Order (`ledger/00-schedule.md` carries it): IN-6 before
`05-python-surface.md` PY-8; IN-1 after RT-4; IN-5 before RT-6; IN-2 after
RT-6; IN-7 after RT-6; IN-3 after IN-1, IN-7, PY-8 and OD-2; IN-4 after
IN-2 and RT-2. All of these land before the consumer block.
`03-output-reporting.md` OR-16 (replay export) follows IN-1. IN-8 and
IN-9 are rows of the numeric block, after `11-other-defects.md` OD-9 and
before the consumer re-baseline; IN-9 also follows `09-numeric-fixes.md`
B-14, which moves the EV config to SI.

Test rules: as `03-output-reporting.md`.

## IN-1: input records and digests

**Kind:** initiative work (provenance for `dl-i08-architecture.results`
and replay export).

**Problem:** nothing records what inputs a run read. A result cannot be
tied to the exact HPXML, schedule, weather and defaults content, and
replay export (OR-16) has nothing to verify an input against.

**Change:** each parsed-input handle of RT-4 computes its `InputRecord`
(`plan.md` §2.7) once, when it is parsed: SHA-256 of the bytes read (`sha2`, which
`hares-io` has had since `03-output-reporting.md` OR-7). A `Path`
source keeps the path and its digest; content keeps its `Arc`. A schedule
generated from the HPXML (`11-other-defects.md` OD-6) has no record
(`InputRecords.schedule` is `None`); it is a function of the HPXML record.
`ParsedDefaults` records `defaults_files_sha256` over every file it reads,
in load order (relative path, `0x00`, bytes), and one `InputRecord` per
replaced file. `ParsedWeather` records `InputRecords.weather_format`, the
format it parsed the weather as: the `WeatherInput.format` given for
`Text` and `Bytes`, the detected one for a `Path`
(`detect_weather_format`, `crates/hares-io/src/weather.rs:43`, which
works only from a path's extension and CSV header, so content cannot be
re-detected). `WeatherFormat` (`weather.rs:16`, today `Debug, Clone, Copy,
PartialEq, Eq` only) gains `Serialize` and `Deserialize` with
`rename_all = "snake_case"` (`"epw"`, `"psm3"`, `"res_stock_csv"`,
`"tmy3"`). `Provenance.inputs` is assembled from the blueprint's handles
at `finish()`, sharing their `Arc`s (no copy of content).

This entry also gives `Provenance` its final settings field (`plan.md`
§2.1): `config: SimulationConfig` and `bldg_id: i64` (OR-6) are replaced
by `settings: DwellingSettings` (`06-runtime-memory.md` RT-4, an earlier
row), the settings the dwelling was built with, the master seed in
effect included, so replay export reads overrides, patches and the
warm-up duration from it; the tests that read `Provenance.config`
(`03-output-reporting.md` OR-6's
`provenance_records_callback_destination_marker`) read
`Provenance.settings.sim_config`. Python: the `Provenance` pyclass's `config` and
`bldg_id` getters read through `settings`, and it gains `overrides`,
`patches` and `inputs` getters (`inputs` a dict with each record's path
or content kind and hex digest, and `weather_format`), with their
`_hares.pyi` stubs and their lines in the `docs/python.md` changes
section.

**Acceptance tests:**
`input_digests_equal_for_path_and_content_sources`: the
`tests/fixtures/parity/cz4a_ashp_hpwh/` inputs as paths and as bytes give
equal digests. `defaults_digest_changes_when_one_file_is_replaced`.
`provenance_inputs_shared_not_copied`: `Arc::ptr_eq` between the
blueprint's content and `Provenance.inputs` for a content source.
`weather_record_carries_resolved_format`: the cz4a `weather.epw` as a
`Path` records `WeatherFormat::Epw` (detected), and its content as
`Text` with format EPW records `WeatherFormat::Epw` (given).
`provenance_settings_carry_overrides_and_seed`: the `consumer_shape_900s`
inputs (`02-measurement.md` M-2, which carry a `Gas Water Heater`
override), built with `build_with_seed(7)` and run 1 h: the result's
`Provenance.settings.sim_config.master_seed` is 7 and
`settings.overrides` holds that override.

**Verification:** `cargo nextest run -p hares-io -p hares-core input`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: record a digest of every input a run reads`

**Dependencies:** `06-runtime-memory.md` RT-4 (handles and
`DwellingSettings`); `03-output-reporting.md` OR-6 (`Provenance`);
`05-python-surface.md` PY-9 (the `Provenance` pyclass).

**Measurement:** digest cost per blueprint (bytes hashed and time),
recorded in `02-measurement.md`.

**State:** committed `9527aa6e` (2026-10-11, branch `row/r63-in1`, cut
from `9cc6d578`). The digest record lands complete on the surface the row
was cut against: the shared parsed-inputs row was not folded at the cut,
so the records are computed where the parse reads the files (both config
constructors) and carried through the blueprint into `Provenance.inputs`,
an `Option` whose `None` is the construction that reads no file (the
synthetic TOML dwellings). The record carries the entry's field list: the
HPXML, the schedule (`None` when generated: it is a function of the HPXML
record), the weather, the weather format (the detected one;
`WeatherFormat` serialises and deserialises snake_case), the defaults
digest over every file the store loads in load order, each file
contributing its relative path, a 0x00 separator and its bytes, directory
reads sorted by name, and the generated schedule's profiles file composed
after the store's read; `defaults_replacements` stays empty until the
per-file replacement surface lands. The digest is over the content, never
the path: the same bytes at a different path hash the same, the path is
metadata beside the digest, and both halves of that rule are pinned. The
acceptance on this surface: two runs on the same inputs record equal
records; editing one input moves exactly that input's digest (the
schedule edit pinned; the defaults replacement moves only the defaults
digest); a generated schedule carries no record; the weather record
carries the detected format; the Python getter's digests equal an
independent hashlib recomputation and two Python runs record equal dicts.
The settings-field replacement (config and bldg_id into the settings
type), the content-source records and the shared-Arc acceptance land with
the parsed-inputs row's fold; the Python `config` and `bldg_id` getters
are unchanged on this surface. RED first: the Rust acceptance file failed
to compile on the missing API and the Python test failed on the missing
getter, both witnessed at the base with only the tests applied. Gates
green on the staged change and again on the commit: fmt clean; clippy
`--workspace --all-targets --all-features -- -D warnings` clean and the
default, dst, profiling, observe and observe,profiling variants clean;
nextest `--workspace --all-features -j 4` 8003/8003 and `-F dst`
7933/7933 (1 skipped each: the ignored kind-pin generator); pytest
`-m ''` 1724 passed + 6 xfailed, 0 failed, 0 errors, the skip census
identical; pyright 0 errors; the nine goldens bitwise (compare-all
8 + dst 1).

## IN-2: `DwellingBlueprint::validate()`

**Kind:** initiative work (`dl-i08-architecture.new_capability`:
`validate()`).

**Problem:** a caller learns that an input is infeasible (for example
`GasFurnaceConfig` rejecting an AFUE outside [0, 1],
`crates/hares-equipment/src/hvac/heating_config.rs:87-90`) only by
building a dwelling, and building runs warm-up
(`crates/hares-core/src/dwelling/mod.rs:3000`, `run_warmup_converged(0.5, 25)`,
defined at `:5360`), days of simulated time before the first reportable
step.

**Change:** assembly (every construction-time check, `ResolvedModel`
computation) and warm-up are separate phases of `build()`.
`DwellingBlueprint::validate(&self) -> Result<ResolvedModel, HaresError>`
runs assembly only, through the same code, and returns the
`ResolvedModel` RT-6 defines. The same assembly gives
`DwellingBlueprint::catalogue(&self) -> Result<Arc<Catalogue>,
HaresError>` (`plan.md` §2.7): the catalogue `03-output-reporting.md`
OR-1 builds at assembly, for the equipment and auto-registered actors
assembly instantiates, with no warm-up and no step; variables of actors a
caller registers or `set_tariff` adds after build are not in it.
Python: `DwellingBlueprint.catalogue()` returns the `Catalogue` pyclass of
`05-python-surface.md` PY-4, and `DwellingBlueprint.validate()`
returns the same dict form as RT-6's `Dwelling.resolved_model()`; its
`site` entry is a dict with `latitude_deg`, `longitude_deg`,
`elevation_m` and `utc_offset_h` (plus each field's source), the form
IN-4's Python `site` argument takes. This entry adds `ResolvedModel.clock`
(`plan.md` §2.7 `ResolvedClock`: the first step's local standard time, the
site offset, the step and the simulated step count, `duration / time_res`
until `09-numeric-fixes.md` B-1c subtracts a skipped Feb 29), so a caller
can check a table's row count against the engine.

**Acceptance tests** (the `full_year_hourly` inputs of
`02-measurement.md` M-2: `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`,
whose HPXML has a gas furnace):
`validate_rejects_afue_above_one_with_run_time_message`: an override that
sets the furnace's AFUE to 1.33; `validate()` and `build()` return equal
errors (`PartialEq` and `Display`).
`validate_runs_no_step`: a `#[cfg(test)]` step counter reads 0 after
`validate()`.
`validate_equals_built_dwelling_resolved_model`: `validate()` equals
`build()?.resolved_model()`.
`resolved_clock_reports_steps`: the `full_year_hourly` inputs report
`clock.steps == 8760`, `time_res_s == 3600`, `utc_offset_s == -25200` and
`start` 2023-01-01 00:00.
`blueprint_catalogue_equals_built_dwelling_catalogue`: the blueprint's
`catalogue()` equals `build()?.catalogue()` (`Dwelling::catalogue()`,
OR-1) on the same inputs, and a `#[cfg(test)]` step counter reads 0 after
it. `blueprint_catalogue_in_python` (Python): the same equality through
`DwellingBlueprint.catalogue()` and `Dwelling.catalogue()`.

**Verification:** `cargo nextest run -p hares-core blueprint`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: validate a blueprint and read its resolved model without running it`

**State:** committed `906d4a1c` (2026-10-12, branch `row/r66-in2-in7`,
cut from `228eecbe`). The build's assembly and warm-up are separate
phases: `assemble_from_blueprint_with_seed` runs every construction-time
check, computes the resolved model, initialises the equipment and
registers the built-in actors, and the build runs
`run_warmup_converged` on the dwelling the assembly returns, the RNG
restore and the production-clock rebuild unchanged. `validate()` and
`catalogue()` run the assembly alone, the construction-time warnings
seeded and emitted on failure exactly as the build's failure path emits
them. The read-back is the blueprint's own `ResolvedModel`
(`crates/hares-core/src/resolved.rs`, the plan's shape less the
equipment, window and infiltration fields the resolved-parameter
read-back row owns, which had not landed at the cut, so the fold extends
the type): every boundary's `ResolvedBoundary` (id, kind, area, the
network's own `r_effective_m2k_w`, the films named apart, `c_j_k`, the
construction's `layers` outside to inside, the LUT row's or the material
layers', empty for the F-factor slab), the resolved `site` with its
per-field sources, and the `ResolvedClock` this entry adds (the first
step's local standard time, the site offset, the step, the configured
horizon's step count). The built dwelling carries the same model and
`Dwelling::resolved_model()` reads it, so validate and build cannot
disagree. RED first: the Rust acceptance file failed to compile on the
missing API and the Python test on the missing import, both witnessed at
the base with only the tests applied. The acceptance: the AFUE-1.33
override fails `validate()` and `build()` with equal errors, `PartialEq`
and `Display` both; the step counter reads 0 after `validate()` and 3
after three steps; `validate()` equals the built dwelling's read-back;
the full-year clock reads 8760 steps, 3600 s, -25200 s, 2023-01-01
00:00:00; the blueprint's catalogue equals the built dwelling's (the
digest, the length) with no step. Adaptations: the step counter is the
process-wide doc-hidden instrument the parse counters are (an
integration test cannot read a `cfg(test)` field); `validate()` and
`catalogue()` take the `Arc` receiver `build()` takes; the Python
surface clones the builder and freezes the clone for the read-only
calls, so `build()` still works after either. Python:
`DwellingBlueprint.validate()` returns the dict form (the site's values
beside their source labels, the clock, the boundaries' layer stacks) and
`DwellingBlueprint.catalogue()` returns the `Catalogue`, with the stub
and the docs line. Gates green on the staged change and again on the
commit: fmt clean; clippy `--workspace --all-targets --all-features -D
warnings` clean and the dst and observe variants clean; nextest
`--workspace --all-features -j 4` 8041/8041 (4 skipped: the ignored
kind-pin generator and the hand-run measurement tests); `pytest -m ''`
1730 passed + 6 xfailed, 0 failed, 0 errors; pyright 0; the nine goldens
bitwise (compare-all 8 + dst 1). The measurement: the cz4a `validate()`
3.3 ms and the cz2a AFUE rejection 6.6 ms in the release build, both
under R9's 100 ms target (`02-measurement.md`).

**Dependencies:** `06-runtime-memory.md` RT-6; `03-output-reporting.md`
OR-1 (the catalogue); `05-python-surface.md` PY-4 (the Python
`Catalogue`).

**Measurement:** `validate()` wall time for the cz4a fixture, recorded in
`02-measurement.md` against requirement R9's target (an infeasible input
rejected in under 100 ms); a time above it is reported to the operator
with the phase that dominates it.

## IN-3: machine-readable input schema

**Kind:** initiative work (`dl-i08-architecture.new_capability`).

**Problem:** nothing lists, from the API, which config keys, dwelling
keys, override targets, schedule columns, defaults files and HPXML
elements the engine accepts or reads, with types, units, bounds and
defaults; a consumer that classifies its fields must read HARES source.
For HPXML in particular, "accepted means applied" cannot be checked: the
parser (a quick-xml event reader building an owned `XmlNode` tree,
`crates/hares-io/src/hpxml/building.rs:294-420`) reads elements through
`child`, `children_named`, `first_descendant`, `descendants` and `path`
(`:301-336`) and the helpers in `xml_helpers.rs:34-139`, with element names
as inline string literals at 450 call sites (the census below, at HEAD
`21d90588`) in `building.rs`, `resolve_loads.rs`, `resolve_hvac.rs`,
`resolve_water_heater.rs`, `resolve_der.rs`, `resolve_pool.rs`,
`validation.rs` and `xml_helpers.rs`: 67 of them are `.path(&["..."])`
calls, nine of those written across lines (`building.rs:558`,
`resolve_loads.rs:57,118`,
`resolve_water_heater.rs:442,458,515,641,658,664`); the `xml_helpers`
functions `child_text`, `child_f64`, `child_energy_kwh` and
`descendants_named` take the name as their second argument
(`xml_helpers.rs:61-96,173`); and `validation.rs:259-282` holds its
required paths as a `&[(&[&str], &str)]` table passed to `path`.

**Change:** `hares_core::input_schema() -> InputSchema` (serde, JSON),
assembled in `crates/hares-core/src/input_schema.rs` (new), the lowest
crate that sees every described type: `SimulationConfig` and the override
structs in `hares-io`, the equipment configs in `hares-equipment`, and
`DwellingConfig` and `DwellingSettings` in `hares-core`
(`crates/hares-core/src/dwelling/mod.rs:210`). `InputSchema`,
`OverrideClassSchema` and `RemovedKeySchema` live in `hares-io`, and
`FieldSchema`, `ValueType` and `DescribeFields` in `hares-types`
(`plan.md` §2.7: the equipment configs live in `hares-equipment`, which
cannot see `hares-io`). `hares-types` has `serde_json` only under
`[dev-dependencies]` (`crates/hares-types/Cargo.toml`), and
`FieldSchema.default` is a `serde_json::Value`, so this entry adds
`serde_json = { workspace = true }` to its `[dependencies]`. One source of
truth: each config and override-accepting struct implements
`DescribeFields` next to its definition (`DwellingConfig` and
`DwellingSettings` in `hares-core`), and the strict override validation
of `11-other-defects.md` OD-2 accepts exactly the fields that list names,
so the schema cannot drift from enforcement. `InputSchema.removed_keys`
lists every `REMOVED_KEYS` row with its scope (IN-6). Equipment classes
come from `EquipmentRegistry::known_names()`
(`crates/hares-equipment/src/registry.rs:161`); schedule columns from the
schedule parser's column table (with units: the setpoint columns carry the
unit the parser reads them in, Celsius until IN-8 makes it Fahrenheit and
updates the table in its commit; `water_heater_setpoint` is not listed,
since IN-5 makes it an error): `schedule_columns` lists the columns the
engine reads (the mapped categories, `WaterHeater` and the occupancy
column, `03-output-reporting.md` OR-11), and `unused_schedule_columns`
lists every `NotUsed` entry with its reason (`plan.md` §2.7), so a column
the engine accepts without reading is visible with why; defaults files
from `ParsedDefaults`'s read list. `DescribeFields` is created here, so
this entry also adds `impl DescribeFields for EnvelopeScales` beside its
definition (`crates/hares-io/src/envelope_scales.rs`, IN-7), describing
`opaque_r_scale` and `thermal_mass_scale` with their default 1.0 and
bounds (0, 10], and lists `envelope` among the override schema's reserved
keys.

HPXML elements (`dl-i08-architecture.input_schema_scope`), generated from
the parser: `crates/hares-io/src/hpxml/elements.rs` (new) declares every
element the parser reads as a `pub static` `HpxmlElement { name: &'static
str, context: &'static str, unit: Option<Unit>, description: &'static str }`
(`context` is the parent path the parser reads it under), and a
`HPXML_ELEMENTS: &[&HpxmlElement]` list of them. The `XmlNode` accessors
and the `xml_helpers` functions take `&'static HpxmlElement` in place of a
`&str` name (`path` takes `&[&'static HpxmlElement]`), so the parser
cannot read an element it has not declared: this type change is the gate,
compile-enforced, and a `&[&str]` table such as `validation.rs`'s
required paths no longer type-checks until it is declared. Every literal
name and `name == "..."` comparison in the parser becomes a declared
element. `InputSchema` gains
`hpxml_elements: Vec<HpxmlElementSchema>` (`plan.md` §2.7), built from
`HPXML_ELEMENTS`.

The elements whose presence fails the parse are declared the same way, in
`HPXML_REJECTED_ELEMENTS` (`RejectedHpxmlElement`, `plan.md` §2.7), and
listed in `InputSchema.rejected_hpxml_elements`, so a caller can see what
the parser refuses and why. The parser's rejections go through that list,
and this entry moves the ones earlier rows added into it:
`11-other-defects.md` OD-19's setback and setup elements
(`SetbackTempHeatingSeason`, `TotalSetbackHoursperWeekHeating`,
`SetupTempCoolingSeason`, `TotalSetupHoursperWeekCooling`, under the
thermostat's `HVACControl`; the reason names the 24-hour weekday and
weekend setpoint profiles HARES reads) and OD-10's legacy
`HasFlueOrChimney` (under `BuildingConstruction/extension`; the reason names
`HasFlueOrChimneyInConditionedSpace`). `09-numeric-fixes.md` B-14's
equipment config commit renames the synthetic extension element
`active_power_kw` to `active_power_w` in `HPXML_ELEMENTS` and adds
`active_power_kw` to `HPXML_REJECTED_ELEMENTS` naming `active_power_w`.
`09-numeric-fixes.md` B-15 adds the
`SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod` elements
(`BeginMonth`, `BeginDayOfMonth`, `BeginHourOfDay`, `EndMonth`,
`EndDayOfMonth`, `EndHourOfDay`, `ColumnName`, `NaturalVentilation`) to
`HPXML_ELEMENTS` in its own commit, declared under that context. Python:
`hares.input_schema() -> dict`.

**Acceptance tests:**
`input_schema_config_fields_equal_accepted_keys`: a fully populated
`SimulationConfig` serialised to TOML has exactly the schema's field
names as keys; any other key is rejected.
`input_schema_overrides_match_enforcement`: for every class and field in
the schema, an in-bounds override is accepted by assembly, and a field
name not in the schema is rejected naming the class and the field; the
test iterates the schema and holds no list of its own.
`input_schema_classes_equal_registry`.
`input_schema_schedule_columns_equal_the_column_table`: `schedule_columns`
and `unused_schedule_columns` together hold every entry of the schedule
column table and the occupancy column, each once, and every
`unused_schedule_columns` entry has a non-empty reason.
`envelope_scales_described_with_bounds`: the `envelope` reserved key lists
`opaque_r_scale` and `thermal_mass_scale` with default 1.0 and bounds
(0, 10], and IN-7's `unknown_envelope_override_field_is_rejected` still
passes with the schema-driven validation.
`hpxml_elements_cover_every_parser_read`: parsing every HPXML fixture under
`tests/fixtures/` with a `#[cfg(test)]` recorder on the accessors records
only elements in `HPXML_ELEMENTS`, and every entry of `HPXML_ELEMENTS` is
read by at least one fixture or listed in the test's exemption list, where
each entry states why no checked-in fixture carries that element.
`no_literal_element_name_in_the_parser` (a census in the verification
step that lists the sites to convert; the type change above is what
enforces it): `rg -U -n
'\.(child|children_named|first_descendant|descendants)\(\s*"|\.path\(&\[\s*"|\b(child_text|child_f64|child_energy_kwh|descendants_named)\([^,]*,\s*"'
crates/hares-io/src/hpxml` prints nothing. `-U` with `\s*` matches the
calls written across lines; it counts 450 sites at HEAD `21d90588` (the
line-based `git grep` form counted 440). Names reached indirectly, such
as `validation.rs`'s required-path table or `resolve_hvac.rs`'s lockout
element lists (`:2407-2420`), are not literals in the name position; the
type change makes them fail to compile until they are declared elements.
The pattern matches only a string literal in the name position, so the
declared-element calls the change leaves are not hits;
the `name == "..."` comparisons in `resolve_loads.rs`, `resolve_pool.rs`
and `equipment.rs` compare equipment names, not XML elements, and are
out of its scope.
`input_schema_defaults_files_equal_files_read` (the full `defaults/`
tree).
`rejected_elements_fail_the_parse_as_listed`: for every entry of
`HPXML_REJECTED_ELEMENTS`, `base.xml` with that element added under its
context in the test fails the parse with the entry's reason, and the
schema's `rejected_hpxml_elements` lists exactly those entries.
`input_schema_json_is_deterministic`.

**Verification:** `cargo nextest run -p hares-io -p hares-core input_schema`;
the census command above; `uv run --no-sync pytest tests/python`.

**Commit:** `feat: publish a machine-readable input schema`

**Dependencies:** IN-1, IN-5 (column units), IN-7 (the `envelope`
override fields); `05-python-surface.md` PY-8; `11-other-defects.md` OD-2,
OD-10's structural commit and OD-19 (the rejections this entry declares).

**State:** committed `16517eb7` (2026-10-12, branch `row/r67-in3-in4`,
cut from `228eecbe`). Landed: the schema's vocabulary in `hares-types`
(`crates/hares-types/src/schema.rs`: `ValueType`, `Bounds`,
`FieldSchema`, the `FieldMeta` rows and the `describe` helper; serde_json
added to the crate's `[dependencies]`), `InputSchema` and its section
types in `hares-io` (`crates/hares-io/src/input_schema.rs`: the removed
keys from `REMOVED_KEYS` itself, the schedule columns from the column
table through `schedule_resolve::schedule_column_schemas`, the defaults
files from `ParsedDefaults::read_files` (the store's input log, the
digest's material — the read list, never a parallel list), the HPXML
elements from the declared lists), and `hares_core::input_schema()`
(`crates/hares-core/src/input_schema.rs`) assembling it, the config
fields from `SimulationConfig`'s, `HealthPolicy`'s and
`SiteLocationOverride`'s `DescribeFields` impls beside their definitions,
the dwelling and settings fields from `DwellingConfig`'s and
`DwellingSettings`'s, and the override classes from the registry's
`known_names()` through `sample_config::override_fields_for_class` — the
typed configs' field lists from the deserialiser's own unknown-field
probe (its "expected one of" list is the complete accepted set), the
raw-parameter classes' from the parameter lists' `field_schemas`, and the
reserved `equipment_id` filtered out (the assembly assigns ids; an
override of it is rejected naming the reservation, so it is not a field
the override accepts). The HPXML elements:
`crates/hares-io/src/hpxml/elements/table.rs` declares 371 elements the
parser reads — `HpxmlElement { name, context, unit, description }` statics
whose contexts are the fixture-harvested parent paths (a throwaway walk of
every fixture's parsed tree; the elements no fixture carries contexted
from the call sites' receivers) — and the type change is the gate:
`XmlNode`'s `child`, `children_named`, `first_descendant`, `descendants`
and `path`, and every name-taking helper (`child_text`, `child_f64`,
`child_bool`, `child_energy_kwh`, `child_power_w`,
`find_descendant_f64`, `descendants_named`, `resolve_local_ref`), take
`&'static HpxmlElement`; the parser's rejections go through
`HPXML_REJECTED_ELEMENTS` (`RejectedHpxmlElement`), scanned where the
document parses (the scan searches the tree's own names, both entrances:
`parse_hpxml_str` and `parse_building`), and OD-10's legacy
`HasFlueOrChimney` moved into it from building.rs's inline check. The
census command prints nothing. Python: `hares.input_schema() -> dict`,
the `ParsedDefaults.read_files()` getter, the stubs' sections and the
docs' changes lines. RED first: the acceptance files failed to compile on
the missing API and the Python test on the missing function, witnessed at
the base with only the tests applied. Two adaptations, both recorded for
the fold that lands their prerequisites (the branch was cut from a base
the two dependencies had not folded into): (1) IN-7's `EnvelopeScales`
does not exist on this base, so `impl DescribeFields for EnvelopeScales`,
the `envelope` reserved key and the `envelope_scales_described_with_bounds`
acceptance are the fold's additions beside IN-7's definition, not this
commit's; (2) OD-19's setback and setup elements are still read on this
base (the parser applies them), so they are declared in `HPXML_ELEMENTS`
(the coverage reads them) and move to `HPXML_REJECTED_ELEMENTS` at OD-19's
fold, whose rejections then go through the same scan. The override
acceptance's per-class in-bounds value comes from the class's sample
config (`sample_config_for_class`, the lifecycle helpers moved into
production code beside the registry), the enforcement's own merge
(`merged_equipment_config`) the acceptance's surface: an override of
every schema field is accepted; a field name not in the schema is
rejected naming the class and the field. Gates: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean; nextest
`--workspace --all-features -j 4` 8049/8049 (3 skipped: the ignored
kind-pin generator and the two hand-run measurement tests) and `-F dst`
7979/7979 (3 skipped); `pytest -m ''` 1731 passed, 6 xfailed, 0 failed,
0 errors; pyright 0; the nine goldens bitwise (compare-all 8 identical).

Review round at this tip, two findings fixed on the branch. First: the
choice-element reads bypassed the statics, so the census did not hold.
The type-element dispatches took the type element's first child and
branched on its name with raw strings: the FoundationType children
(the foundation-name match, the conditioned and vented reads), the
AtticType FlatRoof child and the space-location type children the
infiltration reader matches. Those elements were read and erred on
undeclared, the published schema's hpxml_elements omitted them, and
nothing guarded the read path. Fixed: the table declares the seven
missing statics (the six choice children and the document root; 378
elements now) and the three choice slices; the dispatch gate is
`XmlNode::declared_choice`, which matches a node against the declared
choices by identity and records the read like the other accessors', so
a choice arm must be a declared static (the compile gate covers the
choice path); the foundation-name dispatch, the conditioned and vented
reads, the flat-roof skip, the space-location match and the
document-root check route through the declared elements, and the
appliance kinds' dispatch matches an `ApplianceTag` kind the loop's
declared elements name, never a raw string. The census covers the
choice path: a node name read for a match (the as_str binding or a
literal comparison, outside the equipment specs' receivers) fails the
source-scan census in elements.rs, and the census command gains the
same pattern; both print nothing. RED first: the scan flagged the
sites this fixes (the review's three choice sites, the appliance
kinds', and the root's) and the schema pin failed on the missing
choice children; GREEN holds the coverage test with no new exemptions
(every choice child is read by checked-in fixtures). Second:
describe_fields' doc claimed a metadata row naming no probed field
panics, but the check existed only in the other direction;
`field_schemas` now asserts it the same way the shared `describe`
helper does, pinned by the fake-row test (RED: the test did not panic
on the unfixed tree). Gates after the fixes: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean; nextest
`--workspace --all-features -j 4` 8053/8053 (3 skipped); `pytest -m ''`
1734 passed, 6 xfailed, 0 failed, 0 errors; pyright 0; the nine
goldens bitwise (compare-all 8 identical, dst identical); the census
commands print nothing.

## IN-4: `resample_weather`, the engine's own per-step weather channels

**Kind:** initiative work (`dl-i08-architecture.new_capability`).

**Problem:** the per-step weather a dwelling uses is computed inside the
environment update (`crates/hares-core/src/environment.rs:510-755`; record
index and the weather lookup at `:515-534` inside `update_in_place`,
reached through the public entry point `update()` at `:758`), and no API
returns it.
`WeatherTimeSeries::resample` (`crates/hares-io/src/weather.rs:568`) is a
file-level resample, not that per-step read. The calibration consumer
re-implements the mapping and hard-codes a one-row offset to match it
(`calibration/irradiance.py:48-67` in the consumer at `c71c542`).

**Change:** the per-step weather evaluation is one function, called by the
environment update and by `hares_core::resample_weather(weather:
&WeatherInput, site: &SiteLocation, config: &SimulationConfig) ->
Result<RecordBatch, HaresError>` (`plan.md` §2.7), which returns one row
per step of the configured horizon: `Time` and the per-step weather
channels of `WeatherState` (`crates/hares-types/src/environment.rs:186`)
that the weather file drives, in this order and with these catalogue
names: `Outdoor Dry Bulb (C)` (`outdoor_temp_c`), `Outdoor Humidity
Ratio (kg/kg)` (`outdoor_humidity_ratio`), `Outdoor Pressure (kPa)`
(`pressure_kpa`), `Wind Speed (m/s)` (`wind_speed_m_s`),
`Sky Temperature (C)` (`sky_temp_c`), `GHI (W/m2)`, `DNI (W/m2)`,
`DHI (W/m2)` (`ghi_w_m2`, `dni_w_m2`, `dhi_w_m2`),
`Temperature - Ground (C)` (`ground_temp_c`) and
`Hot Water Mains Temperature (C)` (`mains_temp_c`). The three the
catalogue already holds keep their declarations; the other seven join the
dwelling-level static table here as `VariableSource::Dwelling` variables
(temperatures `Temperature`, `Mean`; humidity ratio `Ratio`, `Mean`;
pressure, wind speed and the irradiances `Quantity { across_homes: Mean
}`, `Mean`; sampling by the field kinds of
`dl-i08-architecture.weather`: the irradiances, interval averages,
`IntervalMean`, the others, point values, `InstantAtEnd`), in no
verbosity preset, so no golden schema changes; `Unit` gains the variants
they need that OR-1's census lacks, and OR-1's pinned kind table gains
them in this commit. The wind direction (`wind_dir_deg`) is
not one of them: a direction has no mean over a period, so it is not a
catalogue variable.
Python: `hares.resample_weather(weather, site, config) ->
polars.DataFrame`, where `weather` is a path, content with its format, or
a `hares.ParsedWeather`, `site` is the `site` dict of
`DwellingBlueprint.validate()` (IN-2; a missing coordinate is a
`KeyError` naming it), and `config` a `SimulationConfig`; with its
`_hares.pyi` stub. The binding builds the `SiteLocation` from the dict's
four values with every `*_source` field set to
`FieldSource::CallerOverride` (`crates/hares-io/src/site_location.rs:100`),
because the caller supplied each value; the dict's source keys, when
present, are not read (no source changes the resampled weather).

**Acceptance tests:**
`resample_weather_equals_run_weather_columns_at_900_and_3600`: the
`tests/fixtures/parity/cz4a_ashp_hpwh/` inputs, start
`2023-01-01T00:00:00-07:00`, 48 h, at `time_res` 900 and 3600 with a
`Timestep` `Memory` report of
the weather variables; `resample_weather` with the same weather input,
`ResolvedModel.site` from `validate()`, and the same config returns
columns equal on bits. `09-numeric-fixes.md` B-1a, B-1b and B-1c re-run this test after
changing the alignment; it holds by construction. The per-step weather
function this entry factors out is also the one B-2's
`warmup_days_repeat_the_first_hour_rule` calls for each warm-up day.
`resample_weather_matches_in_python` (Python): the same 900 s case
through `hares.resample_weather` with the `site` dict from
`validate()`.

**Verification:** `cargo nextest run -p hares-core environment`.

**Commit:** `feat: expose the engine's per-step weather channels`

**Dependencies:** IN-2; `06-runtime-memory.md` RT-2 (the forcing tables
the environment update reads from).

**State:** committed `9d6e6b53` (2026-10-12, branch `row/r67-in3-in4`,
one commit with the input schema row's). Landed: the per-step weather
evaluation factored out of `update_in_place` into
`StepWeatherEvaluator::evaluate` (`crates/hares-core/src/environment.rs`),
one function the environment update and `hares_core::resample_weather`
call — the resampled series, the resolved metadata (the site's values),
the start offset, the mains constants, the hemisphere and phase rules,
the civil timezone and the forcing table for the time grid, constructed
through the same derivation `from_resampled` runs (the manager's Step-1
fields replaced by the evaluator field; the forcing table still resolved
on the parsed-weather handle, shared with the handle's other
constructions; the observe capture (the raw day-of-year pair and the raw
mains °F) rides the evaluation's step struct). `hares_core::resample_weather(weather,
site, config) -> RecordBatch`: one row per step of the configured
horizon, `Time` (the step's naive local wall clock, the report's own
convention) and the ten channels in the catalogue's order; the wind
direction not among them. The catalogue: the seven new weather variables
in the dwelling-level static table at level 9 — in the catalogue (a named
selection resolves them; `build_schema`'s verbosity gate keeps them out
of every preset, so no golden schema changes), the kind pin regenerated
(`kinds.json` gains exactly the seven: the humidity ratio a `Ratio` in
`kg/kg`, the pressure and wind speed `Quantity`-`Mean` in the new
`Kilopascal` and `MeterPerSecond`, the sky temperature a `Temperature`,
the three irradiances `Quantity`-`Mean` in `W/m2` with `IntervalMean`
sampling, the rest `InstantAtEnd`) — and the row plan's dwelling columns
carry the seven slots, written per step from the environment's own
weather state. `Unit` gained `KilogramPerKilogram`, `Kilopascal` and
`MeterPerSecond`. Python: `hares.resample_weather(weather, site, config)
-> polars.DataFrame` (a path, a `(text, format)` pair or a
`ParsedWeather`; the site dict's four values with every field's source
the caller's override, a missing coordinate a `KeyError` naming it; the
parse and resample with the GIL released), the `ParsedWeather` header
getters the site dict's construction reads, the stub's section and the
docs' changes line. RED first: the Rust acceptance file failed to compile
on the missing function and the Python test on the missing import,
witnessed at the IN-3 tip with only the tests applied. One adaptation,
recorded for the fold: IN-2's `validate()` is not on this base, so the
Python test's site dict is the fixture's own resolution (the HPXML's
latitude, longitude and UTC offset, the elevation falling back to the
weather header, read through the new getters) — the form
`validate()`'s `site` entry carries; the fold switches the dict's source
to `validate()` and the acceptance re-runs (the exposed channels match
the run's series bitwise either way: the values are the resolved site's,
whatever names them). The bitwise acceptance:
`resample_weather_equals_run_weather_columns_at_900_and_3600` (the
cz4a fixture's inputs, 48 h, 900 s and 3600 s, the run's report and the
exposed rows equal on bits, every f64 by `to_bits()`, the Time column
included) and `resample_weather_matches_in_python`. Gates: fmt clean;
clippy `--workspace --all-targets --all-features -D warnings` clean;
nextest `--workspace --all-features -j 4` 8051/8051 (3 skipped: the
ignored kind-pin generator and the two hand-run measurement tests) and
`-F dst` 7981/7981 (3 skipped); `pytest -m ''` 1734 passed, 6 xfailed,
0 failed, 0 errors; pyright 0; the nine goldens bitwise (compare-all 8
identical).

## IN-5: thermostat setpoint source by the OpenStudio-HPXML rule

**Kind:** initiative work (`dl-i08-architecture.setpoint_sources`).

**Problem:** when a schedule file has a `heating_setpoint` or
`cooling_setpoint` column and the HPXML declares thermostat setpoints,
HARES silently lets the column win, records the choice only as a typed
config variant, and reads the column's values as degrees Celsius.

**Evidence (the rule HARES must follow):** OpenStudio-HPXML
`HPXMLtoOpenStudio/resources/hvac.rb` `apply_setpoints` (master `57bd1b88`,
lines 1641-1741, https://github.com/NREL/OpenStudio-HPXML/blob/master/HPXMLtoOpenStudio/resources/hvac.rb):
for heating and for cooling separately, a schedule-file column is used when
present ("permit mixing detailed schedules with simple schedules",
line 1665) and the HPXML setpoints only otherwise; when both are given it
warns "Both 'heating_setpoint' schedule file and heating setpoint
temperature provided; the latter will be ignored." (lines 1669, 1675; the
same for cooling). `HPXMLtoOpenStudio/tests/test_validation.rb` lists the
case `schedule-file-and-setpoints` under expected warnings (lines
2181-2183, 2348-2352): it is a warning, not an error. The heating-above-
cooling adjustment (lines 1678-1725) applies only when both sides come from
HPXML. The schedule-file setpoint columns are degrees Fahrenheit
(documentation, "Detailed Schedule Inputs",
https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html;
converted in `schedules.rb` lines 1465-1484). OCHRE also lets the column
win, silently (`vendors/OCHRE/ochre/utils/schedule.py:408-427`, converting
F to C). The ResStock fixtures never exercise the overlap: 0 of the 33
`in.schedules.csv` files under `tests/fixtures/resstock/` has a setpoint
column, and the HPXMLs carry hourly
`Weekday/WeekendSetpointTemps*Season` arrays (for example
`2025.1/bldg0176227/home.xml:1320-1323`).

**Evidence (HARES):** `crates/hares-io/src/schedule_resolve.rs:701-740`
(`inject_setpoint_schedules`: column, then HPXML-derived source, then the
default profile; no warning), `:741-789` (`set_typed_setpoint_source`),
`:184-199` (the column table: setpoint columns carry no unit and no
conversion); `crates/hares-equipment/src/hvac/thermostat.rs:269-302`
(`effective_setpoints`, which evaluates whichever source was configured).

**Change** (structural and bug-compatible: no value any fixture reads
changes; the Fahrenheit reading is IN-8, its own numeric entry):
- The precedence stays OpenStudio-HPXML's (it already matches): per side,
  a schedule-file column, else the HPXML setpoints (the simple
  `SetpointTemp*Season` elements or the hourly weekday and weekend
  arrays), else the default profile. The profiles this rule reads are
  strict from `11-other-defects.md` OD-19 (row 24g) on: a malformed array
  fails the parse, and a declared setback or setup element, which HARES
  does not model, fails it too, so no HPXML setpoint input is accepted and
  then ignored.
- The rule applies to a schedule read from a file
  (`DwellingConfig.schedule` is `Some`). A schedule generated from the
  HPXML (`generate_schedule_from_hpxml`,
  `crates/hares-io/src/hpxml_schedule.rs:66-83`) writes constant
  `heating_setpoint` and `cooling_setpoint` columns in Celsius taken from
  the HPXML itself; until `11-other-defects.md` OD-9 removes them, each
  such column records `SetpointSource::GeneratedConstant { value_c }`
  (`plan.md` §2.7), raises no warning, is never read as Fahrenheit, and
  so never marks a fleet home `Degraded` (`plan.md` §2.4).
- When a file column and any HPXML setpoint input for the same side are
  both present, the run records a `Warning` naming the column and the
  ignored HPXML elements. OS-HPXML warns only for the simple
  `SetpointTemp*Season` element; HARES warns for every ignored input, so
  no choice is silent.
- The active source per side is recorded where it is resolved, in
  `hares-io`: this entry adds `SetpointSource` (`plan.md` §2.7,
  `crates/hares-io/src/setpoint_source.rs`, new) and the
  `EquipmentSpec` fields `heating_setpoint_source` and
  `cooling_setpoint_source`, which `inject_schedule_into_specs`
  (`schedule_resolve.rs:412`) sets for every heating or cooling spec and
  leaves `None` for every other. Today the resolver stores only a typed
  `ScheduleSourceConfig::ColumnRef { col_idx, .. }` in the thermostat
  config (`set_typed_setpoint_source`, `:741-789`): a column index, not a
  name, and nothing that marks an HPXML, default or generated source.
  `06-runtime-memory.md` RT-6 copies the two fields into
  `ResolvedEquipment` (`plan.md` §2.7). The calibration setpoint factor
  applies to the active source: `08-consumer.md` CN-1 checks that its
  setpoint-shift factors edit the source the engine reports as active.
- The setpoint warnings go into the `&mut Vec<Warning>` collector that
  `inject_schedule_into_specs` takes since `03-output-reporting.md`
  OR-11 (source `schedule`, `step_index` `None`, `plan.md` §2.1), which
  the build moves into the dwelling's warning log at assembly; no second
  channel is added.
- Column values are read as today (Celsius) in this entry.
- `water_heater_setpoint`: the column table maps it
  (`schedule_resolve.rs:196`), but no code reads it
  (`inject_water_heater_schedule_columns`, `:790-837`, reads only the draw
  and mains columns), so a file carrying it is accepted and silently not
  applied. A schedule file with a `water_heater_setpoint` column becomes
  a construction error naming the column and stating that HARES does not
  apply it (`dl-i08-architecture.precedence`: accepted means applied).
  Applying it would be a new water-heater setpoint schedule, which no
  decision calls for; no fixture carries the column (the census below),
  so no run changes.
- `docs/PHYSICS_DECISIONS.md` gains the rule with the citations above when
  `09-numeric-fixes.md` B-1a creates it; until then the rule is stated in
  the doc comment of `inject_setpoint_schedules`.

**Acceptance tests** (`tests/fixtures/parity/cz4a_ashp_hpwh/`: its HPXML
declares hourly `Weekday/WeekendSetpointTempsHeatingSeason` and a simple
`SetpointTempCoolingSeason`, `building.xml:780-795`; each test writes,
into its temporary directory, the fixture's `schedule.csv` with one
constant column appended: `heating_setpoint` of 21.0, or
`cooling_setpoint` of 24.0; 48 h at 900 s from `2023-01-01T00:00:00-07:00`).
Source assertions read the heat pump's `EquipmentSpec` after
`inject_schedule_into_specs` (`crates/hares-io/tests/setpoint_source.rs`);
setpoint and warning assertions run the dwelling
(`crates/hares-core/tests/setpoint_source.rs`):
- `schedule_column_wins_over_hpxml_and_warns`: with the heating column,
  the heating setpoint is 21.0 C on every step,
  `heating_setpoint_source` is
  `ScheduleColumn { column: "heating_setpoint" }`, and one warning names
  the column and the ignored HPXML heating elements.
- `sides_resolve_independently`: with the heating column only, cooling
  resolves to `Hpxml` and raises no warning.
- `hpxml_setpoints_used_without_a_column`: the fixture's own schedule:
  `SetpointSource::Hpxml` with the 24 weekday and weekend values of the
  HPXML, converted from Fahrenheit.
- `generated_schedule_setpoint_columns_raise_no_warning_and_are_celsius`:
  `schedule = None` (generated): no setpoint warning,
  `SetpointSource::GeneratedConstant { value_c }` with `value_c` the
  HPXML's first weekday heating value in Celsius, and the run's heating
  setpoint equal to it.
- `water_heater_setpoint_column_is_an_error`: the fixture's schedule with
  a `water_heater_setpoint` column appended fails construction naming it.

**Verification:** `cargo nextest run -p hares-io -p hares-core --test setpoint_source`;
`cargo nextest run -p hares-io schedule_resolve`;
`frame_golden compare-all` (no golden has a setpoint column, and the
generated-schedule path changes no value, so none changes). Census for the
water-heater column: `grep -l water_heater_setpoint -r tests/fixtures data`
prints nothing at HEAD `872cf289`.

**Commit:** `fix: resolve thermostat setpoints by the OpenStudio-HPXML rule and record the active source`

**State:** DONE as `08c010ed` (branch `row/r64-in5`, cut from
`9cc6d578`). Landed: `SetpointSource` (`crates/hares-io/src/setpoint_source.rs`,
new; the plan §2.7 shape without `GeneratedConstant`: the generated
schedule has carried no setpoint column since the phase-1 commit
`c561eb69`, so nothing remains for the variant to record) with the three
live tiers, a serde-tagged round-trip test, and the provenance `label()`;
the `EquipmentSpec` fields `heating_setpoint_source` and
`cooling_setpoint_source`, set by `inject_setpoint_schedules` for every
heating and cooling spec and `None` for every other, and carried across
the blueprint's override merge; the overlap warning, one per side, naming
the column and the ignored HPXML elements by name
(`SetpointTemp*Season` and the `Weekday/WeekendSetpointTemps*Season`
arrays), where OS-HPXML warns only for the simple element
(hvac.rb:1669-1675) — HARES warns for every ignored input, so no choice
is silent; the construction error for a `water_heater_setpoint` column,
naming the column and stating HARES does not apply it; the rule in
`inject_setpoint_schedules`'s doc comment and in
`docs/PHYSICS_DECISIONS.md` (the file exists since the strictness sweep's
commits, so the entry's "until then" is spent and the section is added
now), citing hvac.rb:1641-1741, `get_hvac_setpoints` :1768-1829 (the
hourly arrays, else the constant element with its setback, :1785-1798)
and the manual-thermostat defaults `defaults.rb:2827-2848` — the 68/78 F
defaults the strict-setpoint-defaults sites cite are this rule's third
tier, not the whole rule; the default tier keeps its substitution
warning. RED: the differential is the parity fixture's schedule with a
constant `heating_setpoint` column appended beside its HPXML's declared
setpoints (hourly heating arrays, a simple cooling element): at the
parent the run recorded no warning (the only warning is the unrelated
`TemperatureCapacitanceMultiplier` one) and carried no provenance (the
fields did not exist: the hares-io tests compile RED on
`SetpointSource`), and the values already matched the rule —
`hpxml_setpoints_used_without_a_column` passed at the parent and after,
so the fix changes no value. Adaptation: the generated-schedule
acceptance test's premise died with the phase-1 removal of the generated
setpoint columns; the adapted pin
`missing_setpoints_resolve_to_the_default_profile_source` runs a
synthetic home with no `HVACControl` and asserts both sides resolve to
`DefaultProfile` with the cited values; a generated schedule raises no
setpoint warning and resolves `Hpxml` (pinned by the no-column tests on
both levels). Fleet movement attributed honestly: the fleet and
regression schedule fixtures carried `heating_setpoint` (and cooling)
columns beside `base.xml`'s declared `SetpointTemp*Season` elements —
exactly the overlap the rule warns — and every fleet home degraded
through the warned-home-never-a-success contract, so the fixtures drop
the setpoint columns (the ResStock convention; the census at this head:
0 of the 33 `in.schedules.csv` under `tests/fixtures/resstock/` carries a
setpoint column, and `grep -l water_heater_setpoint -r tests/fixtures
data` prints nothing); no committed golden moved. OCHRE parity: the
setpoints feed the HVAC unchanged; OCHRE lets the column win silently
(`vendors/OCHRE/ochre/utils/schedule.py:408-427`) and the warning follows
OS-HPXML, not OCHRE, recorded in the physics-decisions section. Setpoint
movement declared: none; the nine goldens (compare-all 8 + dst 1) are
bitwise identical, compared twice on the final state, so nothing was
re-captured. Gates: fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
-j 4` 8005/8005 (1 skipped: the parent's ignored kind-pin generator) and
`-F dst` 7935/7935 (1 skipped); `pytest -m ''` 1723 passed, 6 xfailed,
0 failed, 0 errors; pyright 0. The resolved-parameter read-back entry
copies the two fields into `ResolvedEquipment` next.

**Dependencies:** IN-1; `06-runtime-memory.md` RT-4; `11-other-defects.md`
OD-19 (the strict profiles it reads) and OD-17 (the default profile, read
from `ParsedDefaults`, fails loudly). Lands before RT-6.

## IN-7: envelope resistance and thermal-mass scales that never re-select a LUT row

**Kind:** initiative work (`dl-i08-architecture.calibration_r_factor`:
scaling a construction's R for calibration never re-selects its
capacitance row; capacitance changes only through the thermal-mass factor,
so the fit surface stays continuous; the ledger specifies the mechanism).

**Problem:** an opaque above-grade boundary's layers (resistance and
capacitance together) come from one envelope LUT row, chosen by
construction and finish filters and then by the row whose assembly R plus
films is nearest the declared R (`crates/hares-io/src/envelope_lut.rs:507-603`,
nearest-R at `:583-590`; HPXML's own `Wall`, `Roof`, `Floor` and `Door`
elements carry no `InsulationDescription` child, a fact of the schema
itself, not a HARES parsing site, so nearest R decides). Scaling the
declared `<AssemblyEffectiveRValue>`
therefore moves a boundary across row boundaries and its capacitances jump
with the row: the fit surface in R has steps. Today the calibration
consumer avoids that by scaling the `Resistance (m^2-K/W)` column of a
copied defaults tree (`HARES_CALIBRATION_SYSTEM/calibration/defaults.py:27,
128-175`), which `09-numeric-fixes.md` B-13 makes inert (the declared R
then sets the layer resistances), and scales capacitance the same way. No
engine input scales envelope layers after row selection: the override map
takes equipment names only (`crates/hares-core/src/dwelling/conversions.rs:858,
906`), and `HpxmlDataPatches` holds only `number_of_bedrooms`
(`crates/hares-io/src/hpxml/data_patches.rs:14-16`).

**Change:**
- A reserved `envelope` key in the override map, with two fields:
  `opaque_r_scale` and `thermal_mass_scale` (both `f64`, default 1.0,
  bounds (0, 10]). The key deserialises into `EnvelopeScales`
  (`plan.md` §2.7; `#[serde(default, deny_unknown_fields)]`), defined in
  `hares-io` (`crates/hares-io/src/envelope_scales.rs`, new), so the
  envelope code in `hares-io` (`envelope_lut.rs`), the override
  application in `hares-core`
  (`crates/hares-core/src/dwelling/conversions.rs:858,906`) and IN-3's
  schema assembly in `hares-core` all see the one definition. Its bounds
  are enforced by `EnvelopeScales::validate()`, called where the override
  is deserialised, an error naming the field, its value and the bounds
  (`11-other-defects.md` OD-2's rule). `DescribeFields` does not exist yet
  at this row: IN-3, the next row, creates it and adds the impl, so IN-3's
  schema lists the two fields.
  They apply to every opaque boundary built from LUT or material layers,
  after row selection and after B-13's scaling: every layer resistance is
  multiplied by `opaque_r_scale`, every layer capacitance by
  `thermal_mass_scale`; films are unscaled. Row selection reads the
  declared R unscaled, so no scale ever changes the selected row. Slab
  boundaries (F-factor) are outside both scales and are named in the
  field descriptions.
- An `envelope` key whose fields are unknown or out of bounds is rejected
  (`11-other-defects.md` OD-2's rule); at scale 1.0 the run is bitwise
  today's.
- The scaled values are what `ResolvedModel` reports
  (`06-runtime-memory.md` RT-6: `ResolvedBoundary.layers`,
  `r_effective_m2k_w`, `c_j_k`).

**Acceptance tests:**
- `unit_envelope_scales_are_bitwise_neutral`: every M-2 golden with
  `envelope = { opaque_r_scale = 1.0, thermal_mass_scale = 1.0 }` equals
  the golden.
- `envelope_scale_out_of_bounds_is_rejected`: `opaque_r_scale = 0.0` and
  `thermal_mass_scale = 10.5` each fail naming the field and (0, 10].
- `opaque_r_scale_never_reselects_the_row`: for
  `tests/fixtures/hpxml/ochre_samples/base.xml` and scales
  0.5, 0.8, 1.0, 1.25, 2.0, each boundary's resolved layer capacitances
  equal the unscaled ones on bits and its layer resistances are the
  unscaled ones times the scale.
- `thermal_mass_scale_moves_capacitance_only`.
- `resolved_r_is_continuous_in_the_scale`: across 200 scales from 0.5 to
  2.0, each boundary's `r_effective_m2k_w` minus its films is strictly
  proportional to the scale.
- `unknown_envelope_override_field_is_rejected`.

**Verification:** `cargo nextest run -p hares-core -p hares-io`;
`frame_golden compare-all`.

**Commit:** `feat: envelope resistance and thermal-mass scales applied after LUT row selection`

**State:** committed `e1c5a689` (2026-10-12, branch `row/r66-in2-in7`,
after the validate row's `906d4a1c`, cut from `228eecbe`). Landed:
`EnvelopeScales` (`crates/hares-io/src/envelope_scales.rs`, new; the
plan's shape, `#[serde(default, deny_unknown_fields)]`, default 1.0 and
1.0, the field descriptions naming the slab exclusion), the reserved
`envelope` key exempted from the equipment-name override validation, the
extraction and `EnvelopeScales::validate()` at the assembly boundary
before any side effect (an unknown field or an out-of-bounds value is an
error naming the key, the field, its value and the bounds, OD-2's rule),
and the application after the row selection: the boundary construction
picks the LUT row on the declared R unscaled, then every opaque layer's
resistance multiplies by `opaque_r_scale` and its capacitance by
`thermal_mass_scale`, on the LUT row's layers, the input's own material
layers and the single-resistor fallback, films unscaled, the F-factor
slab and the fenestration outside both. A material layer scales through
its conductivity's inverse (the resistance) and its density (the
capacitance), so the layer's own R and C are the values the scales
multiply and the layer-splitting the RC network derives from the scaled
material preserves the totals. The scaled values are what the resolved
model reports: the boundary read-back's `films_m2k_w` names apart the
film resistance the boundary's total includes (the exterior one only
when the two sides face different zones), so the material R the scales
act on is `r_effective_m2k_w` minus its films. The data sources
`build_default_solvers` takes are grouped into one `SolverInputs` struct
(clippy's argument bound). RED first, witnessed at the validate row's
commit with only the tests applied: every acceptance test failed on the
base's `unknown equipment override key 'envelope'` rejection (the wrong
reason, the key unimplemented), the scale-response assertions
unreachable behind it. The acceptance green: unit scales bitwise-neutral
on the M-2 fixture sets a config can build (cz2a and cz4a, every column
of every batch by bits, the metrics read the same; the fleet and the
defaults-file-replacement surface ride the gate's compare-all, bitwise);
0.0 and 10.5 rejected naming the field, the value and (0, 10]; the
never-reselects pin on the ochre-samples home at 0.5, 0.8, 1.0, 1.25 and
2.0 (every layer capacitance bitwise the unscaled one, every layer
resistance the unscaled one times the scale, on bits); the mass scale
moving capacitance only (times 2 on bits, resistances bitwise); 200
scales from 0.5 to 2.0 with every opaque boundary's material R
proportional to the scale within 1e-9 relative (the per-layer scaling is
exact on bits, the summed total's rounding inside the band) and the slab
and fenestration unmoved on bits; the unknown field rejected naming it.
Movement declared: none; no fixture carries the `envelope` key and the
unit scales are bitwise today's, so the nine goldens (compare-all 8 +
dst 1) are bitwise identical, compared on the staged change and again on
the commit, nothing re-captured. Gates: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean and the dst
and observe variants clean; nextest `--workspace --all-features -j 4`
8050/8050 (4 skipped: the ignored kind-pin generator and the hand-run
measurement tests); `pytest -m ''` 1730 passed + 6 xfailed, 0 failed, 0
errors; pyright 0.

**Dependencies:** RT-6 (`ResolvedModel` layers); OD-2.

## IN-6: the OCHRE resampling overrides leave the Rust core

**Kind:** initiative work (`dl-i08-architecture.ochre_compat`: no OCHRE
legacy in the core; any OCHRE comparison mapping lives in the Python
adapter layer).

**Problem:** the engine carries an OCHRE compatibility surface in its core
config: `ResampleOverrides`, `ResampleMethod` and the preset
`ResampleOverrides::ochre_compat()` (zero-order hold on every field) in
`hares-io`, the `DwellingConfig.resample_overrides` field that threads them
to the environment, and the Python `resample_overrides` keyword. Its only
production user is none: the fleet and benches pass `None`
(`crates/hares-fleet/src/fleet.rs:207, 1178, 1195`). Its users are tests:
the Rust parity harness and oracles, a set of Rust regression tests that
copied the parity setup, and the Python OCHRE parity tests, whose own
`OCHRE_COMPAT_RESAMPLE` dict holds 7 fields (not solar or wind), so it is
not the same mapping as the Rust preset.

**Evidence:** `crates/hares-io/src/weather.rs:431-446` (`ochre_compat`) and
the `ResampleOverrides`/`ResampleMethod` definitions and uses in that file
(59 lines); `crates/hares-io/src/lib.rs` (re-export);
`crates/hares-core/src/dwelling/mod.rs:220-223` (the field and its doc
"Use `Some(ResampleOverrides::ochre_compat())` for OCHRE parity testing"),
`:2022, 2105, 2196, 12991`; `crates/hares-core/src/environment.rs` (3
sites); `crates/hares-python/src/py_config.rs` (13 lines),
`py_dwelling.rs` (7), `utils.rs` (10), `python/ochre_next/_hares.pyi` (4);
`tests/parity/mod.rs:484` (the Rust parity harness uses
`ochre_compat()`), `tests/{conditioned,freefloat,envelope}_oracle.rs`,
`tests/envelope_opaque_loads.rs`, `tests/regression/smoke_test.rs`,
`tests/resstock_smoke.rs` and the `crates/hares-core/tests` files
`equipment_id_regressions.rs` (7 sites), `ev_thermal_regressions.rs` (4),
`alignment_oracles_regressions.rs`, `ct_boiler_diag.rs`, the
`equipment_identity_*` files, `add_equipment_empty_name_exemption.rs`,
`replace_equipment_name_collision.rs`, `equipment_readd_after_removal.rs`
and `resstock_observer_diag.rs` (all use `ochre_compat()`; the
never-compiled `crates/hares-core/tests/resstock_smoke.rs` is gone since
`01-baseline-repair.md` BR-12, which moved its seasonal tests into the
compiled `tests/resstock_smoke.rs` listed above); `weather_integration.rs`, `engine.rs` and
`wiring_properties.rs` (use `ResampleOverrides` directly);
`tests/python/test_ochre_parity.py:37-45, 99, 137, 322`
(`OCHRE_COMPAT_RESAMPLE`), `tests/python/test_py_config.py:157-190`;
`crates/hares-io/tests/weather_parity.rs:845-920` (tests of the override
machinery); and every test helper that sets the field to `None` (census: `git grep -n -E
'ResampleOverrides|ResampleMethod|ochre_compat|resample_overrides' --
'*.rs' '*.py' '*.pyi' '*.toml' '*.md'`, 189 lines in 39 files at
`ebdb1a17` outside the planning records SL-8 moves).

**Change:**
- `ResampleOverrides`, `ResampleMethod`, `ochre_compat()`, the
  `DwellingConfig.resample_overrides` field, the Python keyword and every
  site above are deleted. Each weather field has one method, chosen by one
  exhaustive `match` (today's defaults until `09-numeric-fixes.md` B-1a
  replaces them with the EnergyPlus methods).
- This entry creates `hares_io::config::REMOVED_KEYS` with `KeyScope`
  and `RemovedKey` (`plan.md` §2.7), its first row, `resample_overrides`
  in the `DwellingTable` scope, naming no replacement in the engine and
  pointing at the adapter below, and the one lookup
  `removed_key(scope, key) -> Option<&'static str>` that every surface
  uses with the scopes `plan.md` §2.7 assigns it: the TOML loaders check
  each key of a `SimulationConfig` table (scope `SimulationConfig`) and of
  a synthetic TOML file's top level (scope `DwellingTable`) before
  deserialising, and the Python `DwellingConfig` and `from_hpxml`
  keyword checks call it through the binding. `from_hpxml` already takes
  `**kwargs` (`build_config`, `py_dwelling.rs:2285` at `afa9a071`), but
  `PyDwellingConfig.__new__` (`crates/hares-python/src/py_config.rs:582`)
  has a fixed signature, so once the parameter is removed pyo3 would raise
  its own `TypeError` before the body runs and the message could not name
  the adapter (the mechanism `05-python-surface.md` PY-8 describes): this
  entry ends that signature in `**unexpected`, looks each key up in the
  `DwellingTable` scope and raises the removed-key message, and raises
  `TypeError` naming any other key, as Python's own would.
  `05-python-surface.md` PY-2 later replaces the constructor with the same
  `**unexpected` rule. A key removed in one scope
  stays valid in another, so a key that is valid in one table and
  removed in another is never rejected where it is valid. Later entries
  add rows (`05-python-surface.md` PY-8 and PY-2, `03-output-reporting.md`
  OR-3, `09-numeric-fixes.md` B-2, B-9a and B-14).
- The OCHRE mapping moves to the Python adapter layer:
  `python/hares/adapters/ochre.py` (new, beside the package's existing
  adapters), `hold_weather(weather, time_res_s) -> str`, which reads an
  hourly EPW file and returns EPW content at `time_res_s`, each hourly
  record held over the steps of its hour. The held file keeps every field
  the EPW path uses, so the hold is the only change: header lines 1 to 7
  (location, design conditions, typical and extreme periods, ground
  temperatures, holidays and daylight saving, comments) are copied
  verbatim; line 8 (`DATA PERIODS`) states `3600 / time_res_s` records per
  hour; each hourly record is written `3600 / time_res_s` times with its
  minute field set to the end of each sub-interval (EnergyPlus's
  sub-hourly EPW convention), every other field unchanged.
- The EPW reader reads such a file: `ensure_hourly_data_period`
  (`crates/hares-io/src/epw.rs:468-489`, called at `:172`, which rejects anything but one
  record per hour) accepts `N` records per hour for `N` dividing 60 and
  reads the records in order with `source_step_secs = 3600 / N` and
  `midpoint_offset_secs = 1800 / N` (the same interval-end label as
  hourly EPW, at the file's interval; `09-numeric-fixes.md` B-1a later
  replaces the offset by `RecordLabel::IntervalEnd`). Location, design
  conditions, ground temperatures and the leap-year flag are parsed from
  the header exactly as for an hourly file. A file whose interval equals
  the step is read record by record (today's resample at factor 1); B-1a
  makes any other interval an error.
- OCHRE comparisons pass that content as `WeatherInput` text with format
  EPW (`plan.md` §2.7; before `06-runtime-memory.md` RT-4 lands, written
  to a temporary file and passed as a path).
- `tests/python/test_ochre_parity.py` uses `hold_weather` in place of
  `OCHRE_COMPAT_RESAMPLE`. `hold_weather` holds every field; the old dict
  held 7 and left solar and wind to the engine's methods, so the test's
  metric deltas against today are recorded and its tolerances must hold.
- A Rust test that used `ochre_compat()` only because it copied the parity
  setup, and whose assertion does not compare against OCHRE output (the
  `equipment_*`, `ev_thermal_regressions`, `ct_boiler_diag`,
  `resstock_*` and `alignment_oracles_regressions` files), runs on the
  engine's own methods; one that compares against OCHRE output (the
  oracles, `envelope_opaque_loads.rs`, the parity harness) reads held
  weather as below. `crates/hares-io/tests/weather_parity.rs:845-920` and
  the `test_py_config.py` round-trip cases are deleted with the machinery
  they test.
- The Rust parity harness (`tests/parity/mod.rs`) and the OCHRE oracles
  are not Python adapters, so they read held weather prepared by
  `hold_weather`: each parity case directory (whose weather is
  `weather.epw`) gains `weather_ochre_hold_<time_res>s.epw`, and the
  oracles' shared weather (`data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`)
  gains `tests/fixtures/ochre_hold/USA_CO_Denver_<time_res>s.epw` for each
  step the oracles use. Each is generated by
  `python -m hares.adapters.ochre hold <weather> <time_res_s>` and
  committed, and the harness passes it as the case's weather. A pytest
  case regenerates each file and asserts it equals the committed bytes, so
  the committed data cannot drift from the adapter.

**Acceptance tests:**
- `git grep -n -E 'ResampleOverrides|ResampleMethod|ochre_compat|resample_overrides' -- crates tests benches python`
  prints only the `REMOVED_KEYS` row and its tests.
- `removed_key_resample_overrides_is_an_error`, on each surface that
  carried the key: a synthetic TOML file whose dwelling table holds
  `resample_overrides` (`tests/fixtures/bestest/600.toml` with the key
  added in the test), the Python `DwellingConfig(..., resample_overrides=...)`
  and `Dwelling.from_hpxml(..., resample_overrides=...)`; each message
  names the Python adapter.
- `removed_key_is_scoped`: `removed_key(KeyScope::DwellingTable,
  "resample_overrides")` names the adapter, and
  `removed_key(KeyScope::SimulationConfig, "resample_overrides")` is
  `None`.
- `dwelling_config_unknown_keyword_is_type_error` (Python): another
  unknown keyword on `DwellingConfig(...)` raises `TypeError` naming it.
- `hold_weather_holds_each_hour` (Python): `cz4a_ashp_hpwh/weather.epw`
  held at 900 s has header lines 1 to 7 byte-equal to the source, four
  records per hour equal to the source hour in every field but the
  minute, and minutes 15, 30, 45, 60.
- `subhourly_epw_header_fields_equal_hourly` (Rust): the held 900 s file
  and its hourly source parse to equal location, design conditions,
  ground temperatures and leap-year flag, and the held file's records
  equal the source's, each repeated four times.
- `committed_held_weather_matches_the_adapter` (Python, over every
  committed held file).
- The Rust parity suite (`parity_outputs_against_reference_corpus`), the
  oracles and `test_ochre_parity.py` pass at their existing tolerances;
  the per-case metric deltas against today are recorded in
  `02-measurement.md`. A regression test moved to the engine's own methods
  passes with its assertions unchanged; if one fails, its assertion
  depended on the OCHRE hold and it reads held weather instead.

**Stop condition:** the held file reproduces the old zero-order hold on
every field, but not necessarily on the same step: the hold was applied
inside the engine's resampling with today's half-hour shift, and the held
file is read through the interval-end label at its own interval. If any
parity case, oracle or `test_ochre_parity.py` metric leaves its existing
tolerance, the entry stops before committing: the case, metric, value
and tolerance are recorded in `02-measurement.md` and the operator
decides (a tolerance is never widened inside this structural entry).

**Verification:** whole block; `frame_golden compare-all` (no golden uses
the overrides, so none changes).

**Commit:** `refactor: move OCHRE weather resampling out of the engine into the Python adapter`

**State:** STOPPED at this entry's stop condition (2026-10-09, branch
`row/r33-in6`, worktree of `56ae417e`; the commit above is not made: the
full implementation sits in the working tree with the block green except
the three parity cases). The boundary drawn per the entry:
`ResampleOverrides`, `ResampleMethod`, `ochre_compat()`, the
`DwellingConfig.resample_overrides` field, the Python keyword and every
site threading them are deleted; the engine keeps the resampling
computation, each weather field at its one method with today's defaults
baked in (the arithmetic untouched: the default-method pins passed
through unchanged and every golden compares bitwise, compare-all and
-F dst); `hares_io::config::REMOVED_KEYS` with `KeyScope`, `RemovedKey`
and `removed_key` created (first row `resample_overrides`,
`DwellingTable` scope, naming the adapter), consulted by
`SimulationConfig::from_toml`, the synthetic TOML loader (top level and
`[simulation]`) and both Python surfaces (`PyDwellingConfig.__new__`
ends in `**unexpected`; `from_hpxml`'s kwargs check); the adapter
`python/hares/adapters/ochre.py` `hold_weather` and its
`python -m hares.adapters.ochre hold` CLI; the EPW reader accepts N
records per hour dividing 60, reading in order at `3600/N` and
`1800/N`, with the DOE-2 ground-temperature fallback consuming one
value per source hour; the parity harness and the OCHRE oracles read
committed held weather (`weather_ochre_hold_60s.epw` per parity case,
`tests/fixtures/ochre_hold/USA_CO_Denver_60s.epw` for the oracles,
`USA_CO_Denver_900s.epw` for the reader pin, each byte-pinned by
`committed_held_weather_matches_the_adapter`); the override-machinery
Rust tests deleted with it and the copied-the-parity tests moved to the
engine's own methods, one (`alignment_oracles_regressions`) reading
held weather under the entry's fallback rule. nextest 7782/7782,
pytest 1522, pyright 0, the acceptance census printing only the
removed-keys row and its tests. The stop condition fired on
`test_ochre_parity.py`: 1 h Total 6.018e-02 and 1 h HVAC Cooling
9.667e-01 relative error against 1e-9, the 24 h HVAC Heating record
2.5% against 2% — the entry's own phase-shift warning realised (the
held file is read through the interval-end label at its own interval,
about 29.5 minutes ahead of OCHRE's midpoint-stamped ffill); case,
metric, value and tolerance in `02-measurement.md`, operator decides.

**Dependencies:** PY-7 (the adapter lives in
`python/hares/`). Lands before PY-8 and PY-2, which then carry no
`resample_overrides`, and before `03-output-reporting.md` OR-3, which adds
rows to the table it creates.

## IN-8: schedule-file setpoint columns are read in degrees Fahrenheit

**Kind:** fix, numeric (`plan.md` §2.8): its own commit and its own row in
the numeric block, delta recorded, `docs/PHYSICS_DECISIONS.md` section.

**Problem:** a schedule file's `heating_setpoint` and `cooling_setpoint`
columns are read as degrees Celsius
(`crates/hares-io/src/schedule_resolve.rs:184-199`: the column table
carries no unit and no conversion). OpenStudio-HPXML specifies them in
degrees Fahrenheit and converts them (IN-5's evidence: "Detailed Schedule
Inputs"; `schedules.rb` lines 1465-1484), and OCHRE converts them from
Fahrenheit (`vendors/OCHRE/ochre/utils/schedule.py:408-427`). A schedule
written for the HPXML ecosystem therefore drives HARES at a setpoint of,
for example, 70 C.

**Change:** both columns are read as degrees Fahrenheit and converted to
Celsius at parse with `uom` (`ThermodynamicTemperature` from
`degree_fahrenheit` to `degree_celsius`), once per value. The input
schema's column table (IN-3) states the unit as Fahrenheit. The rule and
its citations are added to `docs/PHYSICS_DECISIONS.md` (created by
`09-numeric-fixes.md` B-1a), replacing the doc-comment statement IN-5
left on `inject_setpoint_schedules`. A generated schedule is not affected:
`11-other-defects.md` OD-9, an earlier row, has removed its setpoint
columns.

**Acceptance tests** (IN-5's inputs and its written schedules, with the
appended columns now 70.0 for heating and 75.0 for cooling):
- `setpoint_column_is_read_in_fahrenheit`: with the 70 F heating column,
  every step's heating setpoint equals, on bits, the `uom` conversion of
  70 F to Celsius computed in the test; with the 75 F cooling column, the
  same for cooling.
- `schedule_column_wins_over_hpxml_and_warns` and
  `sides_resolve_independently` write 70.0 and expect the `uom` value;
  their source and warning assertions are unchanged.

**Verification:** `cargo nextest run -p hares-io schedule_resolve`;
`frame_golden compare-all` (no golden has a setpoint column: the census
`grep -l -E '(heating|cooling)_setpoint' -r tests/fixtures/golden tests/fixtures/resstock`
lists no schedule; any golden change stops the entry). Delta: a 48 h run
of the IN-5 inputs with a 70.0 heating column, before (read as 70 C)
and after, annual-equivalent heating energy and mean zone temperature,
recorded in `02-measurement.md`, and the entry's line in the
`docs/python.md` "Numeric changes" list (`plan.md` §2.8).

**Commit:** `fix: read schedule-file thermostat setpoint columns in degrees Fahrenheit`

**State:** DONE (2026-10-17, branch `row/r94ab`, worktree `r94ab`, on the
generated-schedule row's commit `a36f3485`). Landed: both columns read as
degrees Fahrenheit and converted to Celsius at the parse with the `uom`
conversion (`hares_physics::units::temperature_f_to_c`,
`ThermodynamicTemperature::new::<degree_fahrenheit>` to `degree_celsius`),
once per value in `parse_schedule_csv_str`, keyed on the column table's
two thermostat setpoint columns (`column_read_in_fahrenheit`; the
`water_heater_setpoint` column stays the construction error, no unit ever
applies), with a `tracing` debug line per converted column naming the
column and the unit conversion; the input schema's column table states
the unit as Fahrenheit, the parse stated in the description (the schema
test's pin moved from Celsius with it); the doc comment on
`inject_setpoint_schedules` points at the physics decisions;
`docs/PHYSICS_DECISIONS.md`'s setpoint section states the rule, the
citations (the detailed-schedule documentation, `schedules.rb:1465-1484`
`convert_setpoints`, OCHRE `schedule.py:408-427`), what changed and the
direction of the delta, replacing the pending-Celsius statement. A
generated schedule is untouched: it carries no setpoint column (the
generated-schedule row's removal, this branch's first commit), so nothing
in it converts. RED first: at the parent the acceptance tests fail
reading the 70.0 heating column as 70 C (the run's heating setpoint
carries the bits of 70.0 where the conversion is wanted); GREEN at the
row: `setpoint_column_is_read_in_fahrenheit` (the heating leg every step
on bits the uom conversion of 70 F to Celsius computed in the test; the
cooling leg, from a July start, every step the cooler's own setpoint
column on bits the conversion of 75 F — the cooling witness is the
cooler's own column from a summer start because a January run's
compressor outdoor lockout never evaluates the cooler's control and the
dwelling-level scheduled-cooling read reports the first HVAC unit's
telemetry, a heating-only unit on this fixture);
`schedule_column_wins_over_hpxml_and_warns` and `sides_resolve_independently`
(both levels) write 70.0 and expect the conversion, their source and
warning assertions unchanged; `appended_column_values_are_the_injected_ones`
pins the parsed column's values as the parser read them; the io level's
appended-column helper now reads through the parser (a temporary file
parsed by `parse_schedule_csv`), which is where the reading applies.
Delta: the 48 h run of the parity inputs with a 70.0 heating column,
before (read as 70 C) and after: delivered heating 788.4359349702 to
137.3248583510 kWh over the 48 h (the annual-equivalent at the 48 h to
year scale, 143889.5581320653 to 25061.7866490558 kWh) and the mean zone
temperature 64.2784703820 to 21.1235089246 C, both down, recorded in
`02-measurement.md`; the commit adds its `docs/python.md` "Numeric
changes" line. Goldens: compare-all 8 and the dst set's `full_year_900s_dst`
bitwise identical, compared twice on the final state; no golden carries a
setpoint column, none re-captured, any change would have stopped the
entry. Gates: fmt clean; clippy `--workspace --all-targets --all-features
-D warnings` clean; nextest `--workspace --all-features -j 2` 8148/8148
(8 skipped); `pytest -m ''` 1766 passed + 6 xfailed, 0 failed, 0 errors;
pyright 0 errors. The commit is unsigned through the host's 1Password
signing failure (the bridge unreachable from this session's shell), in
the re-signing record's pattern for the span to re-sign.

**Amended the same day by the review's pin discipline:** the Fahrenheit
test's expectation is now an independent literal, never computed through
the production conversion, and the pin exposed a numeric finding: the
`uom` conversion's arithmetic deviates from the faithful formula. `uom`
reaches Celsius through the kelvin base with rounded offset constants
(`(t + 459.67) * 5/9 - 273.15`), so the production
`temperature_f_to_c(70.0)` carried the bits 0x40351c71c71c71e0
(21.1111111111112), 25 ulps above the formula's exact
0x40351c71c71c71c7 (21.111111111111114), and 75 F landed 23 ulps above
(0x4037e38e38e38e50, 23.88888888888897, against the faithful
0x4037e38e38e38e39, 23.88888888888889); the literals the row's landing
pinned were the deviated values. The conversion now computes
`(t - 32.0) * 5.0 / 9.0` directly and the pins carry the faithful bits.
Two ripples recorded with it. First, the water-heater autosizing's
mains bound moves from the exclusive (0, 50) C to [0, 50) C: the
Burch-Christensen clamp floor (32 F, the EnergyPlus rule) converts to
exactly 0 C under the faithful formula, while the `uom` rounding sat an
epsilon above the floor and never met the exclusive bound; 0 C is the
weather's own value, not a stand-in, negatives stay rejected, and the
validation's pin (the zero case) moves from the failing list to the
accepted one with the reason. Second, the water mains temperature moves
at the ulp scale in every run, so all nine goldens are re-captured
(compare-all 8 + dst 1 bitwise identical after; the movement is the
metrics' last few digits, for example the consumer shape's heating COP
39.192071398252686 to 39.192071398252665); the `docs/python.md`
"Numeric changes" list carries the entry. The `temperature_c_to_f`
counterpart carries the same `uom` offset-path deviation (0 C reads
31.999999999999897 through the base path) and is untouched: no pin
covers it and the review names the Fahrenheit-to-Celsius conversion
only. The row's gates' nextest count above corrects 8149 to 8148 (the
suite's true count). Amendment gates: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean; nextest
`--workspace --all-features -j 2` 8148/8148 (8 skipped); `pytest -m ''`
1766 passed + 6 xfailed, 0 failed, 0 errors; pyright 0; the goldens
bitwise (compare-all 8 + dst 1, after the re-capture).

**Dependencies:** IN-5; `11-other-defects.md` OD-9; `09-numeric-fixes.md`
B-1a (`docs/PHYSICS_DECISIONS.md`).

## IN-9: HPXML vehicles and their chargers are simulated

**Kind:** fix, numeric (`plan.md` §2.8; `dl-i08-architecture.precedence`:
accepted means applied): its own commit and its own row in the numeric
block, delta recorded.

**Problem:** the parser reads electric vehicles only from
`Systems/ElectricVehicles/ElectricVehicle` with `BatteryCapacity` and a
`MaxChargingPower` in kW (`crates/hares-io/src/hpxml/resolve_der.rs:177`).
HPXML v4 as the ResStock 2025.1 fixtures write it declares a vehicle as
`Systems/Vehicles/Vehicle/VehicleType/BatteryElectricVehicle`, with
`Battery/NominalCapacity` and `Battery/UsableCapacity` (each `Units` kWh
plus `Value`), and a `ConnectedCharger idref` pointing at
`Systems/ElectricVehicleChargers/ElectricVehicleCharger`, whose
`ChargingPower` is in W and whose `ChargingLevel` is the SAE level
(`tests/fixtures/resstock/2025.1/bldg0176775/home.xml:1814-1857`: 63.433
kWh usable, 5690.0 W, level 2; `bldg0442669/home.xml` the same shape). The
parser reads none of it, so those EVs are silently not simulated. The
`ElectricVehicles` form appears only in HARES's own tests
(`crates/hares-io/src/hpxml/equipment.rs:959-969`,
`crates/hares-io/tests/silent_default_regressions.rs`,
`crates/hares-core/tests/equipment_id_regressions.rs`,
`crates/hares-core/tests/ev_thermal_regressions.rs`,
`tests/python/test_ev_tariff_integration.py`), never in a fixture from the
HPXML ecosystem.

**Change:**
- `resolve_ev` reads each `Vehicles/Vehicle` whose `VehicleType` is
  `BatteryElectricVehicle`: capacity from `Battery/UsableCapacity` (kWh;
  another `Units` value is an error naming it; absent is a
  `MissingField` error naming the element, as today's capacity is); the
  charger from `ConnectedCharger idref`, resolved among
  `ElectricVehicleChargers/ElectricVehicleCharger` by `SystemIdentifier`
  (an unresolved idref, or a vehicle with no `ConnectedCharger`, is an
  error naming the vehicle); `ChargingPower` read in W into
  `EvConfig.max_charging_power_w` (SI since `09-numeric-fixes.md` B-14,
  an earlier row, so no conversion is needed;
  `dl-i08-architecture.si_units`), and `Battery/UsableCapacity` converted
  from kWh to `EvConfig.capacity_j` with `uom` at the parse boundary;
  `ChargingLevel` mapped to today's `charging_level` strings. Vehicle
  types other than battery-electric are an error naming the type (none is
  simulated).
- Every other element of a ResStock 2025.1 vehicle
  (`bldg0176775/home.xml:1814-1857`) is decided here, so
  `HPXML_ELEMENTS` is exact:
  - read: `FuelEconomyCombined` (`Units` `kWh/mile` plus `Value`; another
    unit is an error naming it) into `EvConfig.fuel_economy_j_per_m`
    (B-14's SI field), converted with `uom` as
    `Energy::new::<kilowatt_hour>(x) / Length::new::<mile>(1.0)`;
  - not read, `Battery/NominalCapacity`: the battery model cycles the
    usable capacity, which is read;
  - not read, `Battery/BatteryType` (`Li-ion`): it names no chemistry
    `EvConfig.chemistry` distinguishes (`"NMC"` and the like), so the
    vehicle keeps the catalogue's chemistry;
  - not read, `Battery/NominalVoltage`: the pack voltage comes from the
    config's cell arrangement (`n_series`), not from a declared value;
  - not read, `MilesDrivenPerYear`, `HoursDrivenPerWeek`,
    `FractionChargedLocation` and the `extension` weekday, weekend and
    monthly schedule fractions: trips, departures and plug-in times come
    from the `ev_driver` actor's trip model
    (`crates/hares-core/src/actors/ev_driver/mod.rs`), not from an
    OpenStudio-HPXML driving schedule, and mapping that schedule onto the
    actor is a new driving model no decision calls for;
  - not read, the schedule-file columns `electric_vehicle_charging` and
    `electric_vehicle_discharging` (`bldg0176775` and `bldg0442669`
    `in.schedules.csv`), for the same reason: they stay `NotUsed` entries
    of the schedule column table (`03-output-reporting.md` OR-11), so they
    raise no warning and IN-3 lists them as accepted and not read.
  `docs/equipment/ev.md` lists the vehicle elements and schedule columns
  read and not read, with these reasons, in this commit.
- The `ElectricVehicles` path is deleted, and the tests above move to the
  `Vehicles` form.
- A home that declares both a `Vehicles` EV and an `electric vehicle
  charging` plug load (`resolve_loads.rs:866-918`) is a construction
  error naming both, so the EV is never counted twice.
- `11-other-defects.md` OD-2 site 7's range check (the SAE J1772 AC
  Level 2 maximum, 19 200 W since B-14) applies to the value read.
- `HPXML_ELEMENTS` (IN-3) declares every element read here, and the
  deleted ones leave it.

**Acceptance tests:**
- `resstock_vehicle_ev_is_simulated`: `bldg0176775` (its
  `in.schedules.csv` and `tests/fixtures/resstock/2025.1/weather/G3400270_2018.csv`),
  7 days from `2018-01-01T00:00:00-05:00` at 900 s, no warm-up: the
  dwelling has one EV whose `capacity_j` equals, on bits, the `uom`
  conversion of 63.433 kWh, with a charger of 5690.0 W, its EV end-use
  energy is non-zero, and the run carries no warning with source
  `schedule` (its EV columns are `NotUsed`).
- `charger_power_read_in_watts`: an HPXML in the test with
  `ChargingPower` 7200.0 gives `max_charging_power_w == 7200.0`, and
  `ResolvedModel` (`06-runtime-memory.md` RT-6) reports
  `max_charging_power_w` 7200.0 and `battery_capacity_j` equal to the
  config's `capacity_j`.
- `vehicle_fuel_economy_read_in_joules_per_metre`: `bldg0176775`'s EV
  holds `fuel_economy_j_per_m` equal, on bits, to the `uom` conversion of
  0.22002 kWh/mile; a `FuelEconomyCombined` in another unit is an error
  naming it.
- `vehicle_without_charger_is_an_error`, `unknown_charger_idref_is_an_error`,
  `ev_plug_load_and_vehicle_together_is_an_error`.
- `hpxml_elements_cover_every_parser_read` (IN-3) passes with the new
  elements read by `bldg0176775`.

**Verification:** `cargo nextest run -p hares-io -p hares-core`;
`uv run --no-sync pytest tests/python`; `frame_golden compare-all` (no
golden home declares a vehicle: the goldens use `bldg0000002`,
`bldg0000004`, `bldg0176227` and the parity cases, so none changes; any
change stops the entry). Delta: the `resstock_vehicle_ev_is_simulated`
run of `bldg0176775` and the same run of `bldg0442669` (weather
`G4100510_2018.csv`, start `2018-01-01T00:00:00-08:00`), before (no EV)
and after, annual-equivalent EV and total electric energy, recorded in
`02-measurement.md`, and the entry's line in the `docs/python.md`
"Numeric changes" list (`plan.md` §2.8). From row 94c `[golden]` also
compares the `resolved` product (`09-numeric-fixes.md` B-14 commit 1);
M-1's capture re-captures it, and this entry records its resolved delta:
`bldg0176775` and `bldg0442669` each gain an EV equipment parameter set
(`capacity_j`, `max_charging_power_w`, `fuel_economy_j_per_m`) that the
unfixed-engine capture reports as absent.

**Commit:** `fix: simulate HPXML vehicles and their chargers`

**Dependencies:** IN-3 (`HPXML_ELEMENTS`); `11-other-defects.md` OD-2
(site 7's check); `09-numeric-fixes.md` B-14 (the EV config in SI);
`06-runtime-memory.md` RT-6 (the read-back the second test checks).
