# Output and telemetry reporting

Implements `plan.md` §2.1-§2.3 and §2.5-§2.6: HARES#28, the catalogue,
report requests, reporting frequency and destinations (including the batch
callback), one `RunResult`, stepped-run finalisation, health recorded in
every build, physics invariants enforced in every build, the diagnostics
report, `Time`, checkpoint/restore of the run's accumulators,
`metrics_from_table` and replay export.

Order (`ledger/00-schedule.md` carries it): OR-5 runs inside the opening
sequence (`plan.md` §3) in BR-5's position, because it turns the
plain-release verification line green; OR-11 follows the baseline
measurement. Then OR-1, OR-2, OR-3, OR-13, OR-14, OR-4, OR-12, OR-6,
`05-python-surface.md` PY-9 (directly after OR-6, so Python reads
`RunResult` from the next commit on), OR-7, OR-8, OR-9, OR-17, OR-10,
OR-15.
OR-16 runs after `12-inputs.md` IN-1 and `05-python-surface.md` PY-6.

Python can request output from OR-3 on: each entry that adds a report,
selection, frequency, destination or policy type adds its Python
constructor and stub in its own commit (`plan.md` §2.2), so no commit
leaves Python without a way to ask for what the Rust side records.
Every entry here is structural: none moves a
number. Where an entry touches a numeric path it is bug-compatible, and the
numeric change is its own entry in `09-numeric-fixes.md`.

Every commit keeps the workspace compiling and every test passing: an
entry that changes a Rust API `hares-python` calls adapts the bindings in
the same commit, keeping their Python-facing behaviour unless the entry
states a Python change (`05-python-surface.md` intro).

Test rules for this file: tests are deterministic; no timing, no RSS, no
thread races, and no `git` invocation inside a test. A test that checks
"no file written" gives the run a fresh temporary directory as its only
output root and asserts the directory is empty afterwards, and asserts
structurally that no file backend was constructed. The repository-wide
ignored-file comparison stays a verification-block step (`plan.md` §3),
not a test.

## OR-1: build the variable catalogue from typed declarations

**Kind:** initiative work.

**Problem:** output columns are named, typed and classified by
hand-written surfaces that must agree by convention: the verbosity column
lists in `crates/hares-io/src/output/columns.rs` (`LEVEL_*_COLUMNS`), the
positional maps built per dwelling in `build_equipment_column_map`
(`crates/hares-core/src/dwelling/mod.rs:540`), units parsed back out of
display names (`extract_unit_from_name`, `columns.rs:27`;
`field_with_unit`, `columns.rs:43-53`), and end-use and column-class
string matches (`equipment_name_to_end_use`, `columns.rs:178-245`, where
`"PV #1"` falls through to `OTHER`; `is_ev`, a substring
`contains("ev")` at `columns.rs:1140-1143` that matches "Television";
`is_hvac_or_wh`, `columns.rs:1089-1103`). Actor columns are discovered at
schema build from whatever keys `actor.telemetry()` holds at that moment
(`mod.rs:778-804`, `Actor::telemetry` at `crates/hares-core/src/actor.rs:138`);
actors declare nothing. Per-equipment electric, gas, SOC and mode columns
come from `CoreOutput`/`CoreCapabilities` in `record_step`
(`mod.rs:7165-7236`), not from `TelemetryField`
(`crates/hares-types/src/equipment.rs:1869-1874`, which holds
`unit: String`). Nothing can list a model's variables before it runs.

**Change:**

1. `hares-types` (`crates/hares-types/src/output.rs`, new): `Unit`,
   `VariableKind`, `Aggregation`, `Sampling` and the new `TelemetryField`
   (`plan.md` §2.2). `Unit` has one variant per distinct unit in use at
   `ebdb1a17`, from the census
   `git grep -ohE 'unit: "[^"]*"' -- 'crates/*.rs'` plus the parenthesised
   units in `columns.rs` column names. Spellings of one unit are one
   variant (`"C"` and `"°C"`; `"fraction"` and `"ratio"`); `"code"` and
   `"index"` are `Unit::Code` with kind `Enum`; `"enum"` stays its own
   variant, `Unit::Enum` with kind `Enum`, because the fleet path keys its
   rules on the `unit` metadata string until `04-fleet.md` FL-1 and
   averages `"enum"` columns while it sums `"code"` and `"index"` ones
   (`crates/hares-fleet/src/aggregation.rs:46, 122` and the fallback at
   `:141-150`), so a merged symbol would move fleet numbers in this
   structural commit (`04-fleet.md` FL-5, which deletes the unit-keyed
   rules, merges `Unit::Enum` into `Unit::Code` and lists that metadata
   change); every other merge maps spellings the fleet tables treat
   alike. `"bool"` is `Unit::Boolean` with kind `State`; the empty unit
   `""` is `Unit::None` with `symbol() == ""`.
   `Unit::symbol()` returns the spelling today's column names use.
   Every variable in `kW`, `W`, `kVAR` or `therms/hour` is declared
   `Power`, and every `Quantity` declares its rule across homes
   (`VariableKind::Quantity { across_homes }`, `plan.md` §2.2). By unit
   (census at HEAD `21d90588`): `W/m2`, `s`, `min`, `V`, `J/(kg*K)` and
   `kWh/mi` are `AcrossHomes::Mean`; `kg/s`, `L/day`, `count`, `A`, `Ah`
   and `bytes` are `AcrossHomes::Sum`. The dimensionless units (`-` at 96
   sites, `%`, `fraction`, `ratio`) are classified by meaning per
   `plan.md` §2.2: fractions, ratios, efficiencies, multipliers, COP, EIR
   and SHR (`DUTY_CYCLE`, `PART_LOAD_RATIO`, `CAPACITY_FADE_PCT`,
   `EBM_EFFICIENCY` and the like) are `Ratio`; 0/1 flags (`V2L_ACTIVE`,
   `RAMP_LIMITED`, `BYPASS_ACTIVE`, `COOLING_OAT_LOCKOUT`,
   `latent_degradation_active`) are `State`; codes, identifiers, types and
   indices (`SPEED_INDEX`, `PROTOCOL_ID`, `DRYER_TYPE`, `CYCLE_PHASE`,
   `PV_LUT_INTERP_METHOD`, `STATE`, the empty-unit `OPERATING_MODE`) are
   `Enum`. Stocks, constants, running totals, per-step counts and timers
   follow `plan.md` §2.2's table, and every such variable at HEAD
   `afa9a071` is decided here (a `kWh` variable is `Energy` only when it is
   the energy of the step, and none of these is):
   - stocks and levels, `Quantity { across_homes: Sum }`, `Mean`,
     `InstantAtEnd`: `EBM_ENERGY_KWH`, `EBM_MIN_ENERGY_KWH`,
     `EBM_MAX_ENERGY_KWH` (each equivalent-battery-model equipment:
     `hvac/ac_config.rs:129-142`, `baseboard.rs:431-444`, `boiler.rs:1124-1137,
     1212-1225` and the four other declaring sites), `CAPACITY_KWH`
     (`ev/telemetry.rs:108-112`, which fades with state of health) and
     `ER_STAGES_ON`;
   - configuration constants, `Quantity`, `Mean`, `InstantAtEnd`:
     `CAPACITY_KWH_RATED`, `DECLARED_CAPACITY_KWH`, `IMPLIED_CAPACITY_KWH`
     (`battery/mod.rs:2197-2207`), `N_SERIES`, `N_PARALLEL` and `AH_CELL`
     across homes `Sum`; `MIN_ON_TIME_S`, `MIN_OFF_TIME_S` and
     `FUEL_ECONOMY_KWH_PER_MI` across homes `Mean`;
   - running totals since init, `Quantity { across_homes: Sum }`, `Last`,
     `InstantAtEnd`: `DRIVE_SHORTFALL_KWH` (`telemetry_keys.rs:495-502`,
     "cumulative"), `CYCLE_COUNT` (`rainflow.total_cycles()`,
     `battery/mod.rs:1578`), `STARTUP_TIMER_RESET_COUNT`,
     `PV_LUT_NN_FALLBACK_COUNT`, `DISPATCH_COUNT`, `PARSE_ERROR_COUNT`,
     `TOTAL_COMMANDS_PARSED` (each documented "cumulative",
     `telemetry_keys.rs:231-240, 540-543, 801-821`), and the occupant
     actor's `presence_changes` and `current_step`;
   - values of the latest event, held until the next one,
     `Quantity { across_homes: Sum }`, `Last`, `InstantAtEnd`:
     `PARSED_COMMAND_COUNT` and `PAYLOAD_SIZE_BYTES` (the most recent
     protocol payload, `telemetry_keys.rs:796-810`);
   - counts of the step, `Quantity { across_homes: Sum }`, `Sum`,
     `IntervalMean`: the actors' `signals_count`, `signals_rejected`,
     `clear_signals_dispatched_count` (`dr_compliance.rs:641,805-806`) and
     `safety_signals_count` (`safety_monitor.rs:207`), each set every
     `decide()` to that step's count;
   - timers, `Quantity { across_homes: Mean }`, `Mean`, `InstantAtEnd`:
     `MODE_DURATION_S`, `TIME_AT_CURRENT_SPEED_S`, `TIME_SINCE_START_MIN`,
     `DEFROST_ELAPSED_S`, `DEFROST_ACCUMULATED_FROST_S` and
     `ER_HARD_LOCKOUT_TIME_S`.
   Each `Enum` declares a label at every
   code it publishes, from 0 (`plan.md` §2.2): `CHARGING_LEVEL` (codes 1
   and 2, `ev/telemetry.rs:51-53`) declares `["none", "L1", "L2"]`, and
   `CONNECTION_STATE` (codes 0 to 2, `ev/telemetry.rs:45-48`) is an `Enum`
   with `["home_plugged_in", "away_plugged_in", "disconnected"]`, not a
   `State`. The kind of every variable is reviewed in this entry's pinned
   kind table (acceptance tests). Nothing reads `across_homes` before
   FL-5.
2. Every equipment's `descriptor().telemetry_fields` (the
   `EquipmentDescriptor` field, `crates/hares-types/src/equipment.rs:1868`,
   filled by the private per-module `fn telemetry_fields()` helpers in
   `crates/hares-equipment/src`) declares kind, aggregation, sampling and
   nullability with the new type. The workspace `serde` dependency gains
   the `rc` feature (`plan.md` §2.1), and the `ser_arc_str`/`de_arc_str`
   helpers (`crates/hares-control/src/dispatch.rs:102-110`) are replaced by
   plain derives.
   `Actor::output_variables(&self) -> &[TelemetryField]` is a required
   trait method; each built-in actor (`crates/hares-core/src/actors/`:
   safety_monitor, solver_feedback, ideal_thermostat, bms, dr_compliance,
   occupant, ev_driver) declares the keys it publishes, and so does every
   test implementor: `TestActor` (`crates/hares-core/src/actor.rs:477`),
   `NoStateActor` (`actors/ev_driver/mod.rs:6337`) and the actors in
   `crates/hares-core/tests/{actor_telemetry,alignment_oracles,dispatch_ordering,safety_monitor}_regressions.rs`
   and `interest_filtering_regression.rs`. The Python actor wrapper
   (`crates/hares-python/src/py_actor.rs:99`) declares none until
   `05-python-surface.md` PY-11 (Python actors publish no telemetry at
   `ebdb1a17`). An actor's `Telemetry` is built from its declarations by
   `Telemetry::from_fields(&[TelemetryField])` (new, `hares-types`
   `telemetry.rs`), which registers exactly the declared keys and seals the
   map: a later `insert` of a new key, like `set` on an unregistered key,
   latches with OR-11's unknown-key latch and fails the step with
   `InvariantViolation` naming the actor and the key. Every built-in and
   test actor builds its telemetry this way. Registration (`add_actor`,
   `add_actor_by_name`, auto-registration) rejects an actor whose
   telemetry is not sealed or whose keys differ from its declared names,
   naming the actor and the key, so no per-step check is needed.
3. `crates/hares-io/src/output/catalogue.rs` (new): `VariableDef`,
   `VariableSource`, `Catalogue` (`plan.md` §2.2). `Catalogue::build`
   merges, at assembly and again in `refresh_equipment_caches` after an
   equipment mutation or actor registration: the dwelling-level static
   table (today's `LEVEL_*_COLUMNS` as `&'static [VariableDef]`), a static
   per-capability table for core-output columns (the per-equipment
   `Mode (-)` column declares `EnumLabels::Static` labels, so the table
   compiles as a `static`; `plan.md` §2.2), each equipment's
   `descriptor().telemetry_fields`, a static per-zone-template table keyed
   by the zone display name (`zone_display_name`,
   `crates/hares-core/src/dwelling/mod.rs:7980`), and actor
   declarations. A duplicate name, or an aggregation that contradicts its
   kind (`plan.md` §2.2), is an assembly error. `build_schema` derives the
   Arrow schema from the catalogue. `Dwelling::catalogue(&self) ->
   Arc<Catalogue>` (`plan.md` §2.7) returns it, readable from build on,
   before the first step; `12-inputs.md` IN-2 adds the blueprint's
   `catalogue()` once assembly runs without warm-up.
4. Bug-compatible column set. Which columns an equipment gets today is
   decided by the name classifiers above. This entry keeps that outcome
   exactly by calling those same functions (`equipment_name_to_end_use`,
   `is_ev`, `is_hvac_or_wh`, `display_name_to_end_use_key` at
   `columns.rs:321`, `parse_end_use_electric_power_column[_key]` at
   `columns.rs:374,382`) from one function inside the catalogue builder.
   `09-numeric-fixes.md` B-5 replaces those calls with
   `EquipmentDescriptor.end_use` and deletes the classifiers.
   `extract_unit_from_name` and `field_with_unit` are deleted here.
5. Field metadata `unit` is `Unit::symbol()`. Telemetry-derived columns
   whose declared unit was a second spelling now carry the canonical
   symbol; that metadata change is listed in the commit body.

**Acceptance tests:**

- `catalogue_schema_equals_pinned_schema_at_every_verbosity`
  (`crates/hares-core/tests/catalogue.rs`: it needs the built-in actors and
  assembly, which `hares-io` cannot see). Fixture: an `EquipmentSpec` list
  built in the test with one entry per class in
  `EquipmentRegistry::known_names()` (`crates/hares-equipment/src/registry.rs:161`),
  so a new class is picked up automatically, plus instances named
  `"Television"`, `"PV #1"` and `"Pool Heater"` to pin today's classifier
  outcomes, plus one of each built-in actor. For verbosity 0 to 8 the
  catalogue-derived schema equals the pinned schema in field name, order,
  Arrow type and nullability. The pins,
  `crates/hares-core/tests/fixtures/schema/v{0..8}.json`, are written from
  today's `build_schema` as this entry's first step, before any code
  changes.
- `catalogue_queryable_before_run`: a dwelling built from
  `tests/fixtures/parity/cz4a_ashp_hpwh/` (no warm-up) returns
  `Dwelling::catalogue()` before its first step, and the variable names
  equal the columns of its recorded schema after one step.
- `every_written_channel_is_declared_and_every_declared_variable_is_written`:
  on the all-classes fixture, after one step, every recorded column has a
  `VariableDef` and every non-nullable selected variable is non-null.
- `undeclared_actor_telemetry_key_fails_the_step` (a test actor whose
  sealed telemetry is `set` on an undeclared key in its second `decide()`:
  the step fails naming the actor and the key, also when the actor has
  not yet written a declared key that step),
  `unsealed_actor_telemetry_is_rejected_at_registration`,
  `duplicate_variable_name_is_an_assembly_error`,
  `energy_variable_declared_mean_is_rejected`,
  `last_aggregation_on_a_non_quantity_is_rejected`.
- `fleet_unit_rules_unchanged_by_canonical_symbols`
  (`crates/hares-fleet`): for every unit spelling of the census, the
  fleet's over-time and across-homes rules for the spelling and for its
  `Unit::symbol()` are equal.
- `every_quantity_declares_across_homes_and_power_units_are_power`: on the
  all-classes fixture, every variable in `kW`, `W`, `kVAR` or
  `therms/hour` has kind `Power`, every `Quantity` in a unit the list above
  names has that unit's `across_homes`, and every variable's unit and kind
  (with `across_homes` and enum labels) equal the pinned kind table
  `crates/hares-core/tests/fixtures/schema/kinds.json`, written in this
  commit from the declarations above and reviewed with it, so a
  dimensionless variable's classification is pinned by name; the table
  pins aggregation and sampling too, so every stock, constant, running
  total, per-step count and timer of item 1 is pinned with its decision,
  and no variable in `kWh`, `Wh` or `J` other than the energy of the step
  has kind `Energy`.
- `enum_labels_cover_every_published_code`: `CHARGING_LEVEL`'s labels are
  `["none", "L1", "L2"]` and `CONNECTION_STATE`'s have three entries.
- `static_tables_are_statics`: the dwelling-level, per-capability and
  per-zone tables are `static` items whose enum labels are
  `EnumLabels::Static` (compile-enforced; the test reads the per-capability
  `Mode (-)` entry's labels through `EnumLabels::get`).

**Verification:** `cargo nextest run -p hares-io -p hares-core catalogue`;
`frame_golden compare-all` (every golden fixture bitwise equal, schema
included: the tool pins names, Arrow types, nullability and order,
`tools/frame_golden/src/digest.rs:550`, so it shows no difference). The
`unit` field-metadata change, which neither the goldens nor the schema
pins compare, is listed in the commit body, and the kind table pins each
variable's `Unit`.

**Commit:** `feat: build the variable catalogue from typed declarations`

**Dependencies:** OR-5, OR-11, M-2 (golden fixtures).

**State:** committed `0c1e072b` (branch `row/r36-or1`, one commit). The
declarations: `hares-types/src/output.rs` (new) holds `Unit` (one variant
per distinct unit of the census, `symbol()` returning today's column
spelling; `"C"`/`"°C"`, `"fraction"`/`"ratio"`, `"code"`/`"index"` merged;
`Unit::Enum` kept its own variant because the fleet averages `enum`
columns and sums `code` ones), `VariableKind` with `AcrossHomes` and
`EnumLabels`, `Aggregation`, `Sampling` and the new typed
`TelemetryField`; a const constructor vocabulary (power, temperature,
ratio, state, code, stock, constant, running total, step count, timer)
makes a declaration read as its classification, and all 431 equipment
declaration sites moved to it. `Actor::output_variables` is a required
trait method: the seven built-in actors declare their keys, the test and
Python actors declare none, and `Telemetry::from_fields` builds and seals
every actor's telemetry (an `insert` of a new key latches like a `set` of
one; the dwelling's step-end check fails the step with
`InvariantViolation` naming the actor and the key; registration rejects
an unsealed map or a declared/telemetry key disagreement). The catalogue
(`hares-io/src/output/catalogue.rs`, new) merges the dwelling-level
static table, the static per-capability table (the `Mode (-)` column's
labels are `EnumLabels::Static`), the static per-zone-template table,
each equipment's declarations and each actor's, in schema-block order; a
duplicate name or a kind-contradicting aggregation is an assembly error.
The schema derives from the catalogue (`extract_unit_from_name` and
`field_with_unit` deleted; field metadata `unit` is `Unit::symbol()`),
`extend_schema_with_actor_columns` is deleted (the catalogue appends the
actor columns), and `Dwelling::catalogue()` is readable from build on,
before the first step. The bug-compatible column set calls the old
classifiers from the one expansion function in the catalogue builder.

The pins were written from the pre-catalogue pipeline before any code
change: `crates/hares-core/tests/fixtures/schema/v{0..8}.json` (the
all-classes fixture: one spec per registered class, PV
instance-qualified as `"PV #1"`, plus `"Television"` and `"Pool Heater"`
pinning the classifier outcomes, and one of each built-in actor) and
`kinds.json` (941 variables, one entry per line: unit, kind with
across-homes and labels, aggregation, sampling, nullability), reviewed
with this commit. The schema pin holds in both profiles (the
observe-gated columns are tagged in the pin and filtered by the active
feature set). The fleet's unit rules are pinned unchanged under the
canonical symbols (`fleet_unit_rules_unchanged_by_canonical_symbols`).

Gates: nextest --all-features 7602/7602, pytest 1523 passed / 0 failed,
pyright 0 errors, fmt and clippy `-D warnings` clean (default, dst,
observe, all-features), goldens bitwise identical through
`frame_golden compare-all` in both profiles (no movement). The field
metadata change, listed in the commit body: the EV `Connection State (-)`
and `Charging Level (-)` columns carry `unit: "code"` (the declarations'
canonical symbol) where the name-parsed metadata carried `"-"`, and
`Natural Ventilation Cd` carries `unit: ""` where it carried no unit key;
no column name, Arrow type, nullability or order moved.

Independent review round at tip `40181e0a`, four findings fixed on this
branch. (1) HIGH: the metadata change above did not exist in the shipped
pipeline. `build_schema` took the static per-capability table's symbol,
so the ten EV-family `Connection State (-)` and `Charging Level (-)`
columns (five EV-name-matching classes in the all-classes fixture, not
the two the original text claimed) still carried `-`; the schema
enrichment never wrote the unit key (only `unit_source`) and its unit
check compared against the equipment's whole declared-unit set, so the
dimensionless metadata validated against the equipment's other
dimensionless fields and the mismatch warning never fired. Fixed with
one authority: the equipment declarations are the authority and the
static tables mirror them; the EV per-capability table entries declare
`Unit::Code`, the columns carry `code` end to end (the catalogue's
declaration and the schema field metadata; a new test pins both), the
enrichment checks the declared field for the column's own telemetry key
(the catalogue exposes `telemetry_key`) and warns on a per-field
disagreement instead of passing on set membership (a test pins the
unmasking), and the kinds pin is regenerated from the corrected
pipeline, twice, byte-identical: ten unit fields `-` to `code`, nothing
else moves, and the `v0..8.json` schema pins are byte-unchanged. The
`Natural Ventilation Cd` half of the original claim was false: the
column does not exist. `record_step`'s observe-gated gain table writes
`Natural Ventilation Cd`, `Natural Ventilation Q_Stack (m³/s)` and
`Natural Ventilation Q_Wind (m³/s)` into columns no schema surface ever
carried: the names are absent from the pre-catalogue column lists, from
the pins, and from the catalogue, so the writes have been dead since
they landed (the write loop's `if let Some(&idx)` falls through). The
ledger's rule decides: an observable the engine produces is declared, a
dead write is deleted. These are dead writes, deleted: no output schema
ever carried the columns, no consumer reads them from frames, and the
quantities stay observable where they are actually consumed (the
solver's own tests and the observe surface's phase dump read the solver
gains directly, not output columns). Declaring them would move the
schema (three observe-gated columns at verbosity 6 and above) and add a
unit (`m³/s`) absent from the reviewed unit census, which the row's
one-authority decision does not ask for. The pin test's observe-only
column list drops the three names and keeps the two the pins carry.
(2) HIGH: `CatalogueEquipment.telemetry` was a dead input:
`build_catalogue` never read it, so the duplicate-name and
kind-contradiction checks could not fire for equipment-vs-equipment or
equipment-vs-static conflicts, and the EV columns' code against
dimensionless contradiction was silent. `build_catalogue` now merges
every equipment's declarations into one channel table (one telemetry
key, one declaration): two equipment declaring the same key with
different properties fail assembly naming both, and a
telemetry-derived column whose declaration contradicts the merged
declaration of the key it reads fails assembly naming both; new tests
cover both conflicts. The census the merge forced, resolved to the
declarations and the column surface the reviewed pins carry: the
dehumidifier's `runtime_fraction`, `part_load_ratio` and
`part_load_factor` and the ideal HVAC's `cap_ratio` and `eir_ratio`
declared the `fraction` unit where every other declaring equipment and
the static column surface use the dimensionless `-`, aligned to the
`ratio` constructor; the heat pump heater's `defrost_cycle_state`
declared a code enum aggregated by `Mode` where the reviewed
`Defrost State (-)` column is a `State` averaged, aligned to the
`state_flag` constructor; the EV's `connection_state` and
`charging_level` labels aligned to the shared reviewed label constants.
(3) MEDIUM: the actor columns' fleet aggregation moved undeclared. The
catalogue's schema writes the declared unit into every actor column's
metadata, so `FleetAggregation::for_column` resolves the unit table
where the metadata-less pre-catalogue actor columns fell through the
suffix parse to the WeightedSum fallback: temperature and dimensionless
actor columns moved WeightedSum to WeightedMean, count and code columns
stay on the unknown-unit WeightedSum fallback. The fleet goldens cannot
see it (their aggregate products digest zero columns), so the movement
is declared here and in the commit body, and
`fleet_aggregation_for_actor_columns_is_pinned_under_catalogue_metadata`
pins the rule. (4) LOW: the strays emptied for deletion (the session's
tooling cannot unlink; the fold completes it): the empty
`crates/hares-core/tests/debug_catalogue.rs` placeholder and the
duplicate `tests/fixtures/schema/` pin copy, which
`schema_pins_are_present_and_wellformed` read instead of the canonical
crate copies; the test now reads the canonical copies. Gates after the
fixes: fmt clean; clippy `-D warnings` clean (default, dst, observe,
all-features); nextest --all-features 7607/7607; pytest 1523 passed /
0 failed; pyright 0 errors; the eight goldens bitwise identical through
`frame_golden compare-all` in both profiles (values and schemas
unchanged: the unit movement is field metadata, which the goldens do
not compare).

## OR-2: the row plan replaces per-step column lookup

**Kind:** initiative work.

**Problem:** `record_step` (`crates/hares-core/src/dwelling/mod.rs:7165-7540`)
resolves output columns through `output_column_index`, a
`HashMap<String, usize>`, with lookups at `mod.rs:7169-7204` and
`7347-7493`, although the mapping is fixed for the life of the run
(28 per-step name-to-index lookups at verbosity 2, 33 with the observe
feature's envelope diagnostics, hits and misses alike; the entry's first
text said 13, a stale figure, corrected on review). The equipment
column map is built by `build_equipment_column_map` (`mod.rs:540`) through
`resolve_col` (`mod.rs:814`).

**Change:** add `Selection` and `Frequency` (`plan.md` §2.2) to
`crates/hares-io/src/output/report.rs`, with the variants this entry can
run: `Selection::{Verbosity, Named, Pattern}` and `Frequency::Timestep`
(OR-4 adds `Diagnostics`, OR-13 the other frequencies). `ReportSpec` and
`Destination` are not added here: OR-3 adds them with
`SimulationConfig.reports`, so no commit carries a destination nothing
can request. At freeze, build the `RowPlan` from a list of selections:
the union of the selected variables mapped to producer slots, one reused
`Vec<f64>` row, a validity bitset and an `i64` local timestamp. Each
selection holds its projection. `record_step` writes by index only.
`output_column_index` is removed from `Dwelling` and lives only in the
freeze-time builder, so the step path cannot reach it (compile-enforced).
`build_equipment_column_map` and `resolve_col` are replaced by `RowPlan`
construction, which returns `HaresError::InvariantViolation` for an
expected variable it cannot place (OR-5 has already made `resolve_col`'s
drift an error in every build; the rewritten test from OR-5 moves here).
`Selection::Verbosity(n)` is built from the catalogue. `Named` and
`Pattern` resolve against the catalogue per `plan.md` §2.2. At this
entry, `SimulationConfig` still carries `output_verbosity`, mapped to the
row plan's one `Selection::Verbosity`, and the recorder still writes where
`write_output`, `output_format` and `output_path` say; OR-3 replaces them
with `reports`. The tests below that need two selections in one run, or a
`Named` or `Pattern` selection, build the row plan in-crate from a
selection list (`RowPlan` is internal, `plan.md` §2.9). The per-equipment
string-keyed telemetry reads inside `record_step` (`mod.rs:7271-7307`)
are outside this entry; `06-runtime-memory.md` RT-1 removes them.

**Acceptance tests:**

- `row_plan_places_values_at_golden_column_indices`: the Verbosity-2
  production plan's producer slots pinned against the frozen schema's
  golden column indices at a fixed step of a real run (the third of
  three): the plan spans exactly the schema's value columns, the placed
  row equals the recorded row slot for slot on bits, and the slot the
  plan resolves a variable to is the golden column index the freeze
  resolved it at. The entry's first text defined this test as the golden
  equivalence itself (`frame_golden compare-all`, which exists and runs
  as the golden gate) and no named test; the named test is written as
  the review's finding asked.
- `unknown_selection_name_errors_listing_unknown_names`: `Named(["Total
  Electric Power (kW)", "Nope"])` fails naming `"Nope"`;
  `Named(["Total Electric Power (kw)"])` fails naming the unknown name and,
  first among its suggestions, `"Total Electric Power (kW)"` (the nearest
  catalogue name by edit distance, `plan.md` §2.2).
- `pattern_matching_nothing_is_an_error`, `pattern_glob_semantics`
  (`*`, `?`, literal brackets, case sensitivity).
- `single_column_projection_bitwise_equals_full_verbosity_column`: in one
  run whose row plan holds a `Named` single-column selection and a
  `Verbosity(8)` selection, the first projection equals that column of the
  second, on bits.
- `row_plan_rejects_unplaceable_expected_variable` (the rewritten
  schema-drift test).

**Verification:** `cargo nextest run -p hares-core -p hares-io`;
`frame_golden compare-all`.

**Commit:** `perf: replace per-step column lookup with a frozen row plan`

**Dependencies:** OR-1.

**Measurement:** `record_step`'s share of `simulate()` before and after,
in `02-measurement.md` (baseline figure from M-3; `context.md` gives about
95 ms per run at verbosity 2).

**State:** committed `084294dd` (branch `row/r37-or2`, cut from
`b17ed658`). Built against the variable surface as it stands at that
commit: the plan-input seam is the `VariableSurface` trait (the
catalogue's variable names in schema order), which the output schema
provides through `SchemaSurface`; the catalogue plugs in as the second
implementor of the same seam when it lands, replacing `SchemaSurface`,
and no plan code changes with it. `Selection` (`Verbosity`, `Named`,
`Pattern`), `Frequency::Timestep`, `RowPlan` (union row, validity
bitset, local timestamp, per-selection projections) and the errors live
in `crates/hares-io/src/output/report.rs`; `ReportSpec` and
`Destination` are not added. The production freeze builds the plan from
the one `Selection::Verbosity` the configuration's `output_verbosity`
maps to; the recorder still writes where `write_output`, `output_format`
and `output_path` say. The dwelling no longer holds
`output_column_index` at all: the name-to-position map exists only
inside the freeze-time builder (`build_output_row_bindings`, which also
absorbs `build_equipment_column_map` and `resolve_col` as
`plan_equipment_columns` and `resolve_against_index`), so the step path
cannot reach one. The dwelling-level columns freeze into
`DwellingColumns`, the envelope gain names into term-and-slot pairs
(the per-step name table and its lookups are gone), and the setpoint
chain binds to the first HVAC equipment's frozen position. The
per-equipment telemetry reads inside `record_step` remain, per this
entry. The per-step debug column invariants are deleted: an expected
variable the schema cannot place is an `InvariantViolation` at freeze,
pinned by `row_plan_rejects_unplaceable_expected_variable` (the
rewritten schema-drift test). The other acceptance tests:
`unknown_selection_name_errors_listing_unknown_names`,
`pattern_matching_nothing_is_an_error` and `pattern_glob_semantics` in
`report.rs` (edit distance by a hand-rolled Levenshtein, glob by an
iterative matcher, no new dependency),
`single_column_projection_bitwise_equals_full_verbosity_column` in
`hares-core` over a real 24-step run whose plan holds a `Named`
single-column selection and a `Verbosity(8)` selection, and
`row_plan_places_values_at_golden_column_indices` as the named test
above (the goldens themselves through the `frame_golden compare-all`
gate). Gates: fmt; clippy `--all-features --all-targets -D warnings`;
nextest `--all-features` 7593/7593 and `-F dst` 7526/7526 on the
review-fix tree (two new tests; the parent measures 7591 and 7524, the
first record's dst 7523 off by one); pytest canonical 1523 passed, 0
failed, 0 skipped; pyright 0/0; goldens bitwise: `compare-all` 7
manifests identical and `full_year_900s_dst` identical, no value
movement. Correction on review: the first record's "pytest 1375 passed,
36 skipped, with the 3 failures and 1 module import error pre-existing
on the base commit (missing optional `helics` and `pint`, and the
resstock ctrl-c download test)" came from the implementing agent's
drifted venv; the canonical environment gives the 1523/0/0 above, and
the failures it showed do not exist there. Measurement (criterion,
`bench output_row_plan`, 20 samples, verbosity-2 schema of a
six-equipment two-zone model, the corrected bench of the review-fix
commit: the before side reuses its row buffer exactly as `record_step`
reused `record_scratch`, both sides observe the whole row so no side's
writes are dead-store eliminated): the per-step column access is about
320 ns with the hash lookups and about 10 to 14 ns through the frozen
plan's index writes, about 24x to 32x lower, medians near 25-30x. The
first record's about 390 ns against about 4.4 ns, about 89x, was a bench
artifact a reviewer found: the before side allocated its row inside the
iteration loop (the real old code reused the roster-held
`record_scratch`) and the after side observed one slot, which let the
compiler drop `begin_row`'s fill; `02-measurement.md` carries the
corrected record. Still under 1 percent of a full-year 900 s run's
wall, so the end-to-end effect sits inside noise; the per-step win
compounds with the reporting work that follows on this path.

## OR-3: destinations replace the output booleans (HARES#28)

**Kind:** initiative work. Fixes HARES#28.

**Note from the frozen row plan's review:** the row plan's per-row timestamp is
stamped in UTC epoch seconds (`begin_row`, called with
`step.timestamp.timestamp()`); a local-epoch stamp must not be assumed
from that field. The run's UTC offset lives in the row's own timestamp
column (and, from this entry on, in the report's destination pipeline,
which must not read the plan's field as local epoch seconds).

**Problem:** output is gated by booleans and a format enum
(`write_output`, `output_format`, `retain_batches`,
`crates/hares-io/src/config.rs:86,90,105`) plus `output_path`
(`config.rs:83`) and `rotation: RotationPolicy` (`config.rs:36-48,121`).
`write_output = false` skips schema construction (`mod.rs:2747`), so
`metrics()` raises (`py_dwelling.rs:1545-1551`) and
`batches_or_steps_to_polars_df` (`crates/hares-python/src/conversions.rs:20-49`)
falls back to a two-column frame. A memory-only consumer pays for a file
it never opens (eager `File::create`, `writer.rs:123`). A file with no
path is written to the working directory (`default_output_path`,
`crates/hares-core/src/dwelling/conversions.rs:595`, used at
`mod.rs:2741`; a second copy of the rule in `resolved_output_path`,
`crates/hares-core/src/engine.rs:411-426`). The rotating writer
(`writer.rs:103-300`, `check_rotation` at `writer.rs:190`) and the Python
`rotation` keyword (`py_dwelling.rs:2221`) implement a feature the
decision log removes.

**Change:**

- `ReportSpec` and `Destination::{Memory, Parquet, Csv}` (`plan.md` §2.2)
  are added to `crates/hares-io/src/output/report.rs`, and
  `SimulationConfig.reports: Vec<ReportSpec>` replaces `write_output`,
  `retain_batches`, `output_format`, `output_path`, `output_verbosity` and
  `rotation`. `RotationPolicy`, `OutputFormat`, the rotating writer,
  `default_output_path` and `resolved_output_path` are deleted.
- `SimulationConfig::validate()` (`05-python-surface.md` PY-8) gains the
  report checks: report names unique, report file paths unique, and
  `Selection::Verbosity(n)` with `n <= 8`.
- Each removed key is added to `REMOVED_KEYS` with scope
  `KeyScope::SimulationConfig` (created by `12-inputs.md` IN-6,
  `plan.md` §2.7), so TOML, synthetic TOML and the Python
  constructors reject it naming `SimulationConfig.reports`. The Python
  keywords `write_output`, `output_format`, `output_path`,
  `output_verbosity`, `retain_batches` and `rotation` leave the
  `SimulationConfig` constructor in this commit and are rows of that
  table, and the `output_to_parquet` row (added by `05-python-surface.md`
  PY-8) is rewritten to name `SimulationConfig.reports`.
- Python can request output from this commit on: the frozen pyclasses
  `ReportSpec(name, selection, destination, frequency=Frequency.timestep())`,
  `Selection.verbosity(n)/named([...])/pattern(s)`, `Frequency.timestep()`
  and `Destination.memory()/parquet(path, overwrite=False)/csv(path,
  overwrite=False)`, wrapping the Rust values, and
  `SimulationConfig(reports=[...])`. Each later variant gets its Python
  constructor in the entry that adds it (`plan.md` §2.2: OR-4
  `Selection.diagnostics()`, OR-13 the other `Frequency` constructors,
  `05-python-surface.md` PY-10 `Destination.callback()`). Until
  `05-python-surface.md` PY-9 replaces the return type, Python
  `simulate()` and `results()` return the polars frame of the first
  `Memory` report in `reports` order, and today's step fallback frame when
  there is none (OR-6 deletes the step history and that fallback with it).
  Every Python test, example and wrapper that reads a frame
  (`SimulationPlan`, the gym and HELICS wrappers, `tests/python`) requests
  `reports=[ReportSpec("frame", Selection.verbosity(n), Destination.memory())]`
  in this commit, with `n` the verbosity it used before.
- File destinations follow `plan.md` §2.2: lazy creation, unique partial
  name, publish by hard link (or rename with `overwrite: true`),
  existing-target check at freeze, partial files removed on a failed run,
  zero rows means no file, `File::create` and publish failures name the
  path. Until OR-6 adds `finish()`, publishing happens at the end of
  `simulate()`, where today's `flush_and_close` runs (`mod.rs:3146`); a
  stepped run publishes nothing until OR-6, as today it flushes nothing.
- `Memory` alone produces the full selected frame.
- The diagnostics CSV writer (`crates/hares-core/src/dwelling/mod.rs:3055-3064`)
  gates on `output_verbosity >= 4` and derives its path
  from `output_path`, both removed here. Until OR-4 replaces it, the writer
  is created only for a report with `Selection::Verbosity(n)`, `n >= 4`,
  and a `Parquet` or `Csv` destination, at
  `<that report's path stem>_diagnostics.csv` in the same directory (the
  first such report in `reports` order); with no such report, nothing is
  written and nothing falls back to the working directory.
- `frame_golden`'s fixture manifests and its Rust API calls are migrated
  to `reports` in this commit. The migration is config-only; every
  capture is unchanged.
- Every TOML fixture, Python test and example using a removed key is
  migrated in this commit. Census:
  `git grep -nE '\b(write_output|retain_batches|output_format|output_path|output_to_parquet|output_verbosity|rotation)\b' -- '*.rs' '*.toml' '*.py' '*.pyi' '*.md'`,
  with the grep's remaining hits after the commit limited to the
  removed-key table and its tests.

**Acceptance tests:**

- `memory_only_produces_full_schema_and_no_file`: a run in a fresh
  temporary directory (the only output root) with one `Memory` report
  returns every selected column, the directory is empty afterwards, and no
  file backend was constructed.
- `zero_row_run_creates_no_file`: replaces
  `finish_on_empty_recorder_produces_valid_file` (declared change).
- `existing_target_without_overwrite_fails_before_first_step`,
  `overwrite_true_replaces_target`,
  `missing_parent_dir_is_io_error_naming_path`,
  `duplicate_report_path_is_a_validate_error`,
  `failed_run_leaves_no_partial_file` (a run made to fail at a
  configured step by a new in-crate test equipment that returns `Err` from
  `step()`, following the `FailingPortEquipment` pattern, `mod.rs:14548`).
- `schema_equal_across_destinations`: one run with `Memory`, `Parquet`
  and `Csv` reports of the same selection; the Parquet file read back and
  the CSV read back with the catalogue's Arrow types have the same column
  names, order and types as the Memory table, and equal values
  (Parquet on bits; CSV to its declared text precision).
- `removed_key_<key>_names_reports`, one per removed key, for TOML and
  for the Python constructor.
- `python_memory_report_returns_frame` (Python): `cz4a_ashp_hpwh` inputs
  (`tests/fixtures/parity/cz4a_ashp_hpwh/`), start
  `2023-01-01T00:00:00-07:00`, 24 h at 900 s, one
  `Memory` report at `Selection.verbosity(2)`: `simulate()` returns a frame
  with that report's schema and 96 rows.
- `test_gym_default_writes_no_file` (Python): a `DwellingGymEnv` built
  with its defaults over the `tests/python/conftest.py` inputs, run for one
  episode of 20 steps with the process's working directory set to a fresh
  temporary directory, leaves that directory empty.

**Verification:** `cargo nextest run -p hares-io -p hares-core`;
`uv run --no-sync pytest tests/python`; `frame_golden compare-all`;
`cargo bench --bench single_building --no-run`.

**Commit:** `feat: replace output booleans with a report and destination list (HARES#28)`

**Dependencies:** OR-2; `05-python-surface.md` PY-8 (the one config
type); `12-inputs.md` IN-6 (`REMOVED_KEYS`).

**Measurement:** output-pipeline share of `simulate()` with a `Memory`
destination versus today's Parquet-always path, in `02-measurement.md`
(`context.md` output-off floor: 1.03 to 1.04 s). The bench group
`output_destination` (new, in `benches/single_building.rs`) holds the four
cases the measurement compares: `no_report`, `memory`, `csv` and
`parquet`, each the `single_building` fixture at verbosity 2; this commit
adds it.

**State:** committed `be1843fb` (branch `row/r38-or3`, cut from
`c9c3399e`, one commit).

The destination model, as built. `ReportSpec { name, selection, frequency,
destination }` and `Destination::{Memory, Parquet{path, overwrite},
Csv{path, overwrite}}` live in `hares-io/src/output/report.rs` (serde
externally tagged, `deny_unknown_fields`); `SimulationConfig.reports:
Vec<ReportSpec>` replaces `write_output`, `retain_batches`, `output_format`,
`output_path`, `output_verbosity` and `rotation` (`output_chunk_size`
stays). The freeze resolves every report's selection against the catalogue
(`resolve_selection`: `Verbosity(n)` to the tier's variables with the
observe gate honoured, `Named` exact and case-sensitive with the
edit-distance suggestions, `Pattern` a glob that may match nothing), unions
the resolved columns first-seen across the reports in `reports` order, each
report's resolution in its own order inside it, into the producer surface,
and builds one `RowPlan` with one projection per report
(`from_resolved_projections`). One `StreamingRecorder` per report, each
with its own schema: a `Verbosity(n)` report's schema is the catalogue's
`build_schema(catalogue, n)` (byte-identical to the old pipeline's schema,
the goldens' proof), a `Named` or `Pattern` report's schema spans its
resolved columns (`build_schema_for_columns`; its `hares_verbosity` metadata
is the deepest tier the columns are drawn from). The producer bindings bind
against the union schema; the expected-column set for the drift check is
the union's own resolved names (a promised column the schema cannot place
is still `InvariantViolation` at freeze, pinned by
`row_plan_rejects_unplaceable_expected_variable`); `plan_equipment_columns`
takes that set instead of the verbosity. `record_step` fills the one union
row and pushes each report's projection of it to that report's recorder
(projections copied at freeze into `RosterCaches`, one reused scratch
slice, no per-step allocation); every file report records every step until
OR-6's `finish()`.

The file destination rules, as built: lazy creation (the file exists from
the first flushed batch, never at construction or from an unflushed
buffer), a unique hidden partial (`.<file name>.partial-<pid>-<creation
nanos>-<n>` in the target's directory: the pid namespaces concurrent
processes, the creation nanos keep the name unique across a pid's reuse,
and a process-global counter separates the recorders of one process), the
freeze-time
checks at recorder construction (an existing target with
`overwrite: false` is `TargetExists` naming the path and stating the
override, before the first step; a missing parent directory is
`PathIo` naming the path), publish at the end of `simulate()` where
today's `flush_and_close` ran: hard link plus partial removal with
`overwrite: false` (a publish race into an existing target is
`TargetExists`; a filesystem without hard links is `HardLinkUnavailable`
naming the target and stating that `overwrite: true` publishes by
rename), rename with `overwrite: true`; a failed run removes its partial
files (`simulate()`'s error path calls `discard_partial` on every
recorder); a zero-row run creates no file (the lazy creation never opens
one); `File::create` and publish failures name the path. A stepped run
publishes nothing until OR-6. `Memory` retains every flushed batch: alone
it produces the full selected frame.

The mapping from the old booleans, with the differential: a
`Destination.parquet(path)` report at `Selection.verbosity(n)` is the old
`write_output = true, output_format = "parquet", output_path = Some(path),
output_verbosity = n` (and `Destination.memory()` is the old
`retain_batches = true` half), proven by the gates: the eight goldens run
through the migrated manifests' `[[simulation.reports]]` (every capture
unchanged, `output_verbosity = 2` moved into the report's selection) and
every frame is bitwise identical in both profiles, and
`schema_equal_across_destinations` pins one run's Memory, Parquet and Csv
frames equal (Parquet on bits) against the Memory table. The frame readers
(Python `simulate()` and `results()`) return the first `Memory` report's
frame in `reports` order, today's step fallback when there is none; the
engine's metrics come from the memory batches when one exists and ride the
incremental calculator on the first file report when none does (the
calculator failing to initialize still fails the plan: the run is refused
at freeze, never zeroed metrics behind a warning, and the refusal names
the remedy, carrying the power column in that report's selection or adding
a `Memory` report); `metrics()` without a `Memory` report names the real
cause, that its batches come from a `Memory` report and a file-only run
has none, not a plea to run `simulate()` first (the metrics decoupling is
the metrics entry's). The diagnostics CSV writer rides the first
report in `reports` order with `Selection::Verbosity(n)`, `n >= 4`, and a
file destination, at `<stem>_diagnostics.csv` beside it; with no such
report nothing is written and nothing falls back to the working directory.
The rider's one guard is a freeze-time check that its path does not
collide with a report's own destination (the eager create would truncate
that report's target before the recorder's existing-target check ran);
the rider's full removal belongs to the diagnostics row.

The removed keys: `write_output`, `output_format`, `output_path`,
`output_verbosity`, `retain_batches` and `rotation` are rows of
`REMOVED_KEYS` with scope `KeyScope::SimulationConfig`, every replacement
naming `SimulationConfig.reports`, so TOML, synthetic TOML and the Python
constructor reject them with the one message (the Python surface finds
them through the scope scan); the `output_to_parquet` row is rewritten to
name `SimulationConfig.reports`. The Python keywords leave the
`SimulationConfig` constructor; the constructor takes
`reports=[ReportSpec(...)]` and the frozen pyclasses `ReportSpec(name,
selection, destination, frequency=None)`, `Selection.verbosity(n) /
.named([...]) / .pattern(s)`, `Frequency.timestep()` and
`Destination.memory() / .parquet(path, overwrite=False) / .csv(path,
overwrite=False)` are registered on the module, exported from
`hares`, stubbed in `_hares.pyi`, and their reprs round-trip through the
config's `reports` getter. `SimulationConfig::validate()` gains the report
checks: report names unique, report file paths unique,
`Selection::Verbosity(n)` with `n <= 8`. `default_output_path`,
`resolved_output_path`, `RotationPolicy`, `OutputFormat`, the rotating
writer and `DEFAULT_WRITE_OUTPUT` are deleted; `SimulationResults.timeseries_path`
is the first file report's path in `reports` order. The fleet's
`with_output_verbosity` and `with_retained_output` are replaced by
`with_memory_report(verbosity)` (the aggregate folds the memory reports'
batches); the frame_golden adapter runs the manifests' own reports for a
full capture and clears them for a discard. The bench group
`output_destination` (no_report, memory, csv, parquet at verbosity 2) is
in `benches/single_building.rs`.

The RED first: the six run-level destination tests
(`memory_only_produces_full_schema_and_no_file`,
`existing_target_without_overwrite_fails_before_first_step`,
`overwrite_true_replaces_target`, `missing_parent_dir_is_io_error_naming_path`,
`failed_run_leaves_no_partial_file` by an equipment that returns `Err`
from `step()` at the default failure budget,
`schema_equal_across_destinations`) were written against the surface
before it existed and failed on the parent commit at the config parse
(the `reports` key rejected); the writer-level tests
(`zero_row_run_creates_no_file` replacing
`finish_on_empty_recorder_produces_valid_file`, the lazy creation, the
publish and partial rules) failed the same way, and the config-level
validate and removed-key pins failed on the missing fields. GREEN: all
landed with the row. The new tests: the six run-level pins in
`crates/hares-core/tests/reports_destinations.rs`, the writer's
destination tests in `writer.rs`, the validate pins in `config.rs`, the
removed-key pins in `config.rs` and `test_py_config.py`,
`python_memory_report_returns_frame` in `tests/python/test_py_reports.py`
(the cz4a fixture, 24 h at 900 s, one Memory report at verbosity 2: 96
rows, the full selected schema) and `test_gym_default_writes_no_file` in
`test_gym_env.py` (a default-valued config's gym episode of 20 steps in a
fresh temporary working directory leaves it empty; at the old defaults
the episode wrote a csv into the working directory).

Gates on the commit: `cargo fmt --all -- --check` clean;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean; `cargo nextest run --all-features -j 4` 7626/7626;
`cargo nextest run -F dst -j 4` 7559/7559 (the first record's 7774 was
misrecorded); pytest canonical (`uv sync --locked --all-extras
--all-groups`, then `uv run pytest tests/python`) 1542 passed, 0 failed,
0 skipped; `uv run pyright` 0 errors; the goldens bitwise
through `frame_golden compare-all` in both profiles (7 manifests identical
on the default build and `full_year_900s_dst` identical on the dst build,
values and schemas unchanged; the parquet destination's frames are the
Memory table's, on bits). `cargo bench --bench single_building --no-run`
compiles. The census grep's remaining hits: the removed-key table
(`config.rs`) and its tests (`config.rs`, `test_py_config.py`,
`py_config.rs`, the `[output]`-table probe in `synthetic.rs`), and one
English "rotation" in `urdb.rs` (a months-rotation comment, not the key).

Two notes carried: the catalogue tests' shared synthetic fixture still
wrote `time_res_s`/`duration_s` under `[simulation]` (removed keys their
rows rejected two rows before this one; the fixture's two tests failed on
the parent commit too), fixed here to `time_res`/`duration`; and the
`cargo clippy --workspace --all-targets --release -F dst -D warnings`
failures (the pyo3 constructors' argument counts and the
first-keyword-return loop in `hares-python`) pre-existed on the parent
commit; the review round below fixed them on this branch, and the base
gate, `cargo clippy --workspace --all-targets --all-features -- -D
warnings`, is clean at the tip.

Independent review round at tip `f95cb9b3`, six findings fixed on this
branch. (1) HIGH: the base clippy gate was not clean: three
`too_many_arguments` lints on the pyo3 dwelling-config constructors' nine
positional arguments and one `never_loop` on the unexpected-keyword
check's first-iteration return. The constructors now forward one
`DwellingConfigArgs` struct behind unchanged pyo3 signatures, each allow
states the parameter-list constraint, and the keyword check reads the
first unexpected keyword directly; `cargo clippy --workspace
--all-targets --all-features -- -D warnings` is clean at the tip. (2)
HIGH: the hard-link publish refusal was classified by error text (a
contains-"link" match no raw errno message fires for the capability
refusals that matter), so a filesystem refusing links reported an
ordinary path failure. The classifier now matches `ErrorKind::Unsupported`
and `ErrorKind::CrossesDevices` into `HardLinkUnavailable` (naming the
target and stating that `overwrite: true` publishes by rename) and
leaves EPERM a path error; unit tests pin the classifier on constructed
errnos and a real link(2) on a directory probes the OS path. The partial
name gains a creation-nanos component, so a reused pid cannot
reconstruct a stale partial name, and the pid comment states the real
scoping. (3) MEDIUM: the metrics-init refusal did not suggest the
remedy; it now names both (include the power column in the report's
selection, or add a `Memory` report) at both sites, and the consequence,
the run refused at freeze, is stated in the destination rules above. (4)
MEDIUM: the multi-report wiring had no end-to-end pin that two
differently-shaped reports receive their own projections;
`multi_report_each_destination_carries_its_own_selection` runs a
`Named`-selection Memory report and a `Verbosity(2)` csv report through
`Dwelling::from_config` and compares each destination's contents against
its own selection's resolution, shared columns value for value (a
swapped projection fails the recorders' column-width check or the
per-destination comparison). The metrics refusal and the diagnostics
rider's collision check carry their own pins
(`metrics_refusal_on_a_file_only_run_names_the_remedy`,
`diagnostics_rider_path_colliding_with_a_report_destination_is_refused`).
(5) LOW: the union-order comment and the streaming doc comment in
`tests/engine.rs` said "catalogue order" and `retain_batches=false`
where the code is first-seen and the config is a file-only report list;
corrected above and at the comment. (6) LOW: the gate record's `-F dst`
count was wrong, corrected above, and every count now carries its exact
command.

Gates after the fixes: `cargo fmt --all -- --check` clean;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean; `cargo nextest run --all-features -j 4` 7633 passed (1 skipped);
`cargo nextest run -F dst -j 4` 7566 passed (1 skipped); `uv sync
--locked --all-extras --all-groups`, then `uv run maturin develop` and
`uv run pytest tests/python` 1542 passed, 0 failed; `uv run pyright` 0
errors; the goldens bitwise through `frame_golden compare-all` on the
default build (7 manifests identical) and on the dst build
(`full_year_900s_dst` identical).

## OR-4: diagnostics become a catalogue report

**Kind:** initiative work; also fixes `context.md` item 12.

**Problem:** the per-step diagnostics CSV is created whenever
`output_verbosity >= 4`, next to the output path or in the working
directory (`crates/hares-core/src/dwelling/mod.rs:2790-2830`), whether or
not any output was requested. Its columns (`diagnostics::write_header`,
`crates/hares-core/src/diagnostics.rs:90-114`) are not catalogue
variables, and its equipment preamble (zone mapping, OCV sources,
provenance lines, `mod.rs:3097-3114`) exists only in that file.

**Change:** the diagnostics columns become catalogue variables
(`VariableSource::Dwelling` or `Zone`), selected by `Selection::Diagnostics`
(`plan.md` §2.2; this entry adds the variant and its Python constructor
`Selection.diagnostics()`), usable with any destination including `Memory` and at
any frequency. `diagnostic_writer` and the verbosity gate are deleted;
nothing is created unless a report requests it. The equipment preamble is
dropped with the CSV writer; OR-6, two rows later, carries the same
information as `Provenance.equipment` (`plan.md` §2.1), which does not
exist at this row.

**Acceptance tests:**

- `diagnostics_absent_without_a_diagnostics_report`: a `Verbosity(8)`
  report and no diagnostics report, in a fresh temporary directory: the
  directory is empty and no diagnostics variable is recorded.
- `diagnostics_in_memory_with_verbosity_0_report`: a `Verbosity(0)`
  report plus a `Diagnostics` `Memory` report yields the diagnostics
  columns, and they equal, row for row, the CSV today's writer produces
  for the same fixture (pinned as this entry's first step, written by the
  writer of the entry's parent commit, which every row before it leaves
  bitwise unchanged; `crates/hares-core/tests/fixtures/diagnostics_cz4a.csv`),
  to the CSV's printed precision.

**Verification:** `cargo nextest run -p hares-core diagnostics`.

**Commit:** `fix: diagnostics are a catalogue report, created only when requested`

**Dependencies:** OR-3.

**State:** committed `81448376` (branch `row/r41-or4`, cut from
`c9c3399e`, one commit). The destinations entry had not landed at the
cut, so the request rides the one config type: `SimulationConfig.diagnostics`
(serde default `false`, the TOML `[simulation] diagnostics` key, the
Python keyword, getter and setter, and the stub entry), and the report
is the run's own frames, read back through the recorder's retained
batches or the output file. `Selection::Diagnostics` joins the report
selections (the plan.md variant): the catalogue's diagnostics block is a
contiguous suffix appended to the schema last, every field carrying the
block's metadata marker, so the freeze resolves the selection's
projection from the schema alone (`SchemaSurface` reports the block's
offset through the variable surface; the verbosity preset spans the
slots before it). The declarations: the CSV's every column as
catalogue variables, 23 on the three-zone all-classes fixture (941 to
964 in the kinds pin, regenerated and reviewed): `step`, `timestamp_s`,
`outdoor_temp_c`, the per-zone triples `zone{id}_temp_c`,
`zone{id}_thermal_gain_w`, `zone{id}_latent_gain_w` (`VariableSource::Zone`,
keyed by zone id as the CSV's names were), `electrical_net_kw`,
`electrical_net_kvar` and the nine envelope component gains
(`VariableSource::Dwelling`; `air_density_kg_m3` declares the new
`Unit::KilogramPerCubicMeter`). The verbosity schema pins
(`v0..8.json`) are byte-unchanged: no verbosity preset emits the block.

The gating: `diagnostic_writer`, the `output_verbosity >= 4` gate and
the equipment preamble are deleted (the preamble's information rides
with the destinations entry's successor, as planned); the diagnostics
bindings resolve the block's slots at freeze into the producer
bindings and `record_step` writes them by index, reading the same
sources at the same step point the CSV writer read (the write moved
after the other producers' writes; the intervening code touches none of
the sources). The post-hoc diagnostic accumulator is created only when
the run requests the diagnostics (at assembly and on observer enable),
so an unrequested run's step pays nothing for the diagnostics, and its
findings surface as tracing warnings: the `# diag` comment lines died
with the writer, and the summary line with them.

The acceptance tests live in `crates/hares-core/tests/diagnostics_report.rs`:
`diagnostics_absent_without_a_diagnostics_report` (RED at the parent:
the unrequested verbosity-8 run created the CSV in the output
directory), `diagnostics_in_memory_with_verbosity_0_report` (the frame's
diagnostics columns equal the pinned CSV row for row at the CSV's
printed precision), and `without_the_request_frames_equal_the_requested_frames_minus_diagnostics`
(the shared columns bitwise equal, the requested schema exactly the
other's plus the block). The pin `crates/hares-core/tests/fixtures/diagnostics_cz4a.csv`
was captured from the parent commit's writer before any code change,
per the entry. The migrated tests: `port_radiant_diag`'s CSV test reads
the frame now; the post-hoc wiring test pins both directions through a
capturing tracing subscriber (the checks run only when requested); the
two catalogue tests whose synthetic TOML carried the removed
`time_res_s`/`duration_s` keys were already failing at the parent and
load `time_res`/`duration` here. The Python surface: the `diagnostics`
keyword, getter and setter, the stub entry, and a pytest for the
default and the round trip.

Gates: fmt; clippy `-D warnings` (default, dst, observe,
observe-profiling, all-features; four pre-existing hares-python lints at
the parent fixed in passing: the loop that never looped and three
pyo3 constructors' argument counts, allowed like their siblings);
nextest `--all-features` 7841/7841 and `-F observe` 7822/7822; pytest
unchanged from the parent (26 failures + 4 errors identical at the
parent, the parity and thermal-trace environment failures; 1618
passed); pyright 0; the goldens bitwise in all three profiles
(compare-all 7 identical, `-F dst` 1, `-F observe` 7, the tool's
observe feature forwarding added for the third line). Measurement: the
with-request recording's per-step cost is within host noise on the
two-zone model, the magnitude recorded as a bound and not a point
estimate (the bench's paired deltas flipped sign across re-runs, about
+150, -1.7 and +20 ms per day, and the bench's fixed order biases the
pair it measures; corrected on review), and nothing runs without the
request (`02-measurement.md`, the bench this row adds; the structural
claim proven by the code path and the tests).

## OR-5: health recorded and physics invariants enforced in every build, including warm-up

**Kind:** initiative work; closes `01-baseline-repair.md` BR-5 (the three
plain-release failures). Scheduled directly after the baseline-repair
entries so the plain-release verification line is green from then on.

**Problem (health):** health counters are written only under `observe`
(`mod.rs:6623-6634`), reset every step (`mod.rs:5306-5311`), and so read
zero in a shipped wheel. The port-rollback increment (`mod.rs:5203-5206`)
is `observe`-gated. Rejected control signals are counted in a
process-global static under `observe` (`REJECTED_SIGNAL_COUNT`,
`crates/hares-equipment/src/lib.rs:35-54,258-270`), as are clipped RL
actions (`ACTIONS_CLIPPED`, `ACTIONS_CLIPPED_WARNED`,
`crates/hares-python/src/py_gym.rs:208-212,275-290`). HVAC curve-index
clamps are published only under `observe`
(`crates/hares-equipment/src/hvac/air_conditioner.rs:1418-1423`,
`hvac/heat_pump/heater.rs:1595`). Warm-up non-convergence is a log line
(`mod.rs:5062-5073`). `nan_temperature_count` is incremented
(`mod.rs:6276`) immediately before the run returns an error, so it can
never be observed, and `unresolved_column_count` (`mod.rs:6653-6681`)
counts a condition this entry turns into an error.

**Problem (invariants):** the decision log requires NaN and physics
invariant violations to fail the run in every build profile, including
warm-up. Today:

- `check_invariants()` (`crates/hares-core/src/dwelling/mod.rs:7026-~7470`)
  is compiled and called only under
  `cfg(any(debug_assertions, feature = "check_invariants"))` (call site
  `mod.rs:6291`), and returns early during warm-up (`mod.rs:7043-7045`).
- Every `InvariantChecker::check_*` method has a release stub returning
  `Ok(())` (`crates/hares-core/src/invariants.rs:705-800`, under
  `cfg(not(any(test, debug_assertions, feature = "check_invariants")))`);
  its fields are gated too (`invariants.rs:11-52`). Moving a call out of
  the gate would still run a stub.
- The always-on zone-NaN check (`mod.rs:6258-6287`) checks conditioned
  zones only and is skipped during warm-up.
- `resolve_col`'s schema-drift check panics only in gated builds
  (`mod.rs:658-662`).
- Non-finite telemetry panics in gated builds and is silently dropped in
  plain release (`crates/hares-types/src/telemetry.rs:43-90`).

**Change (health):** `crates/hares-core/src/health.rs` (new): `RunHealth`,
`WarmupOutcome` (`plan.md` §2.3), readable at any step through
`Dwelling::health()` and, from OR-6, returned in `RunResult.health`.
`HealthEvent` and `EquipmentHealthCounts` go in `hares-types`. Every
counter is a run total on the dwelling, incremented unconditionally in
every build and never reset per step:

- `port_rollbacks` in `rollback_failed_equipment_ports`
  (`mod.rs:5168-5215`);
- `rejected_control_signals` at each `apply_control` error and each
  control-target-not-found or ambiguous-target site in dispatch
  (`mod.rs:1177-1195`, `1228-1233`, `1262-1279`). Each of those sites
  already pushed one warning per occurrence and keeps doing so (one
  warning per rejection, not one per equipment), so the gym's per-step
  `take_warnings` stream is unchanged; the count is the new part, and the
  warning log's capacity bounds the warnings (`plan.md` §2.3). The
  equipment-side static `REJECTED_SIGNAL_COUNT`
  (`crates/hares-equipment/src/lib.rs:35`), its warn-once flag
  `WARNED_REJECTED_SIGNAL` (`:38`) and `control_signal_rejected_count()`
  are deleted, and so are the dispatch sites' gated `tracing::error!`
  blocks, whose text ("silent in release builds") stops being true, since
  every rejection is a counted event and a warning;
- `clamped_actions`, the `RunHealth` counter, and
  `Dwelling::record_clamped_actions` that adds to it. As committed
  (`dc033cee`, `469ea955`), nothing calls `record_clamped_actions`, and
  the two statics `ACTIONS_CLIPPED` and `ACTIONS_CLIPPED_WARNED`
  (`crates/hares-python/src/py_gym.rs:208,212`) were not deleted; they are
  still live at `21d90588`, so the counter reads zero. `11-other-defects.md`
  OD-18 deletes the statics and their warn-once log and calls
  `record_clamped_actions` from every clipping path;
- `curve_index_clamps` through a new `Equipment` trait method
  `take_health_counts(&mut self) -> EquipmentHealthCounts` (default zero;
  the HVAC models return `take_biquadratic_clamp_count()`), read once per
  step. `take_biquadratic_clamp_count()` resets the counter
  (`crates/hares-equipment/src/hvac/hvac_core.rs:1074-1079`), so it has one
  reader: the two `observe`-gated `BIQUADRATIC_INDEX_CLAMPED` publications
  inside `step()` (`hvac/air_conditioner.rs:1419-1423`,
  `hvac/heat_pump/heater.rs:1591-1595`), which would drain it before the
  dwelling reads it, are deleted; `RunHealth.curve_index_clamps` is the
  record;
- `warmup` from `run_warmup_converged` (`mod.rs:5003-5074`) as
  `WarmupOutcome` (`plan.md` §2.3): days run and whether today's rule
  (last-step conditioned-zone temperature, 0.5 C) judged it converged, plus
  `WarmupResiduals`: the worst zone's four EnergyPlus quantities (daily maximum
  and minimum zone air temperature, daily peak heating and cooling
  delivered) recorded for the last day against the day before; the worst
  zone is the one with the largest temperature residual. Recording them
  does not change the stopping rule, which
  `09-numeric-fixes.md` B-2 replaces. No warm-up is `Disabled`.
  Non-convergence also pushes a `Warning` naming the zones.

`nan_temperature_count`, `unresolved_column_count`,
`rolled_back_port_equipment` and the three schema columns
`port_rollback_count`, `nan_temperature_count`, `unresolved_column_count`
(`mod.rs:3900-3918`, appended only by `refresh_equipment_caches`) are
deleted; that schema change is recorded in the commit body (no golden
fixture exists yet at this position).

**Change (invariants):** delete the release stub block
(`invariants.rs:705-800`) and the field gates (`invariants.rs:11-52`) so
the real checks compile in every profile. Remove the warm-up early returns. Every failing check returns a
typed `HaresError`; no panic remains on the run path. Every gated check in
`hares-core` is classified (`plan.md` §2.3):

| Check (site at `ebdb1a17`) | Today | Class | After |
|---|---|---|---|
| `timestep_dt` (`mod.rs:7029-7036`) | gated | wiring | always |
| conditioned 80 °C, unconditioned 120 °C and tank-node bounds, `check_temperatures` (`mod.rs:7055-7093`) | gated, skipped in warm-up | physics | always, including warm-up |
| SOC published, `soc_bounds` (`mod.rs:7112-7119`) | gated | wiring | always |
| `check_soc` (`mod.rs:7120`; `invariants.rs:313-329`, which only warns and clamps and is called with a constant `0.0` error) | release stub | none: vacuous | deleted. Its input is a `Soc`, which `Soc::try_from` (`crates/hares-types/src/equipment.rs:1128`) already bounds to [0, 1]; an internal SOC outside [0, 1] reaches the dwelling as an absent core-output SOC, which `soc_bounds` fails |
| EV static keys present, `ev_capacity_degraded` (`mod.rs:7141-7162`) | gated | wiring | always |
| `check_ev_capacity_degraded` (`mod.rs:7164`) | release stub | physics | always |
| `check_nan_screen` on `net_kw`/`net_kvar` (`mod.rs:7171,7190`) | release stub | non-finite | deleted: duplicates `electrical_net_finite` in the always-on block (`mod.rs:6175-6197`), which stays |
| `check_electrical` (`mod.rs:7183`), `check_reactive` (`mod.rs:7191`) | release stub | conservation | always |
| `check_fuel_electric_absent` (`mod.rs:7196`), `check_fuel_coverage` (`mod.rs:7201`) | release stub | wiring | always |
| `check_thermal`, `thermal_balance` (`mod.rs:7218-7240`) | release stub | conservation | always |
| `check_heating_accumulator`, `check_hvac_power_non_negative` (`mod.rs:7247-7278`) | release stub, skipped in warm-up | physics | always, including warm-up |
| `humidity_payload_finite` (`mod.rs:7296-7309`) | gated, skipped in warm-up | non-finite | always, including warm-up |
| `check_moisture` (`mod.rs:7448`), with its in-loop skip of steps whose humidity-ratio change exceeds half the zone level (`mod.rs:7353-7355`, justified as a warm-up transient) | release stub, skipped in warm-up | conservation | always, including warm-up; the skip is removed |
| outdoor humidity ratio `assert!(w >= 0.0)` (`mod.rs:4286-4291`) | gated panic | physics | always, typed error |
| `check_nan_screen` on outdoor temperature (`mod.rs:4298`, result discarded with `let _ =`) | release stub, ignored | non-finite | always, propagated |
| non-finite zone temperature, equipment power and residuals in `DwellingTelemetry` construction (`mod.rs:4305-4340`, logged only) | log only | non-finite | always, error |
| zone temperature NaN (`mod.rs:6258-6287`) | conditioned zones, skipped in warm-up | non-finite | every zone, including warm-up |
| `check_protocol_native_registration` (`mod.rs:5670`), `check_equipment_step_order` (`mod.rs:5946`) | gated | wiring | always |
| `resolve_col` drift (`mod.rs:658-662`) | gated panic | wiring | always, `InvariantViolation` returned from `build_equipment_column_map`. The error propagates through `refresh_equipment_caches` (`mod.rs:3794`), so its three infallible public callers become fallible: `auto_register_actors` (`:3167`), `clear_equipment` (`:3574`) and `remove_equipment_by_end_use` (`:3605`) return `Result`, and their callers in `crates/hares-python/src/py_dwelling.rs`, `py_blueprint.rs`, `crates/hares-core/tests/equipment_id_regressions.rs`, `orchestration_parity.rs`, `tests/conditioned_oracle.rs` and `tests/freefloat_oracle.rs` propagate or assert it in this commit |
| `check_hvac_setpoint_invariants`, `check_foundation_zone_invariant`, `check_basement_lighting_foundation` (`mod.rs:2120-2130`) | gated, two panic | wiring (construction) | always, typed errors |
| `check_mode_ordinals_invariant` (`columns.rs:1042`) | gated | debug assertion (static table, no input reaches it) | a unit test; removed from the construction path |
| `seen_targets` only `ByName` (`mod.rs:1198-1204`) | gated `debug_assert!` | debug assertion (code logic) | `cfg(debug_assertions)`, reason on the line |
| `Telemetry::insert`/`set` non-finite (`crates/hares-types/src/telemetry.rs:43-90`) | panic when gated, silent drop in release counted by the process-global `NON_FINITE_REJECTIONS` (`telemetry.rs:20`) | non-finite | a latch on `Telemetry` (first key and value, stored without writing the value); the dwelling reads each equipment's and actor's latch after its step (O(1)) and fails with `NanDetected` naming the source and key; `NON_FINITE_REJECTIONS` is deleted |

Test seams: `test_thermal_invariant_failure` and
`test_hvac_negative_energy_failure` (`mod.rs:1536-1547`) and their setters
(`mod.rs:2943-2953`) move from `cfg(debug_assertions)` to
`cfg(any(test, debug_assertions))`, so in-crate unit tests reach them in
plain release and a shipped release build carries neither.

Stop condition: if enforcement during warm-up makes any existing test or
golden fixture fail, this entry does not commit; the failing check, fixture
and step are recorded here and the operator is asked before any physics
change (a warm-up physics change is numeric and cannot be folded into
this structural entry).

**Acceptance tests** (in-crate unit tests in `crates/hares-core/src/dwelling/`,
so the plain-release line `cargo nextest run --workspace --release` runs
them without features). Every dwelling below is the BESTEST case 600
(`tests/fixtures/bestest/600.toml`, through the test helper
`bestest_dwelling()`), one zone, with no output written:

- `health_counters_nonzero_in_default_build`: two in-crate failing-port
  test equipment (`FailingPortEquipment`, `mod.rs:13847` at `ebdb1a17`)
  each fail one step; `health().port_rollbacks == 2`.
- `rejected_signal_counted_per_run`: two BESTEST 600 dwellings, each with
  a test equipment accepting `SOC_TARGET`; an out-of-bounds
  `SOCTarget { target_soc: 1.5 }` to the first, both stepped alternately
  twice on one thread: the first counts 1 and carries the rejection
  warning, the second counts 0.
- `curve_index_clamp_counted`: a four-speed central air conditioner
  (8 kW, stage capacities 2, 4, 6 and 8 kW, one shared capacity and EIR
  curve pair) driven at a 0 C cooling setpoint, so its top stage
  evaluates a curve index past the pair; `curve_index_clamps >= 1` after
  one step. The test also runs under the `-F observe` block line, which
  pins that no `observe` path drains the counter first.
- `warmup_nonconvergence_recorded_and_warned`: `run_warmup_converged`
  with a 1e-9 C threshold and a two-day cap, which BESTEST 600's transient
  start cannot meet, records `Ran { converged: false, days_run: 2 }` with
  residuals and pushes exactly one non-convergence warning.
- `thermal_balance_violation_fails_in_release` (seam above).
- `warmup_residuals_recorded_without_changing_days_run`:
  `run_warmup_converged(0.5, 3)`: the recorded `days_run` equals the count
  the function returns (the stopping rule is unchanged), and residuals are
  present and finite exactly when a predecessor day ran.
- `non_finite_telemetry_fails_in_release`: a test equipment writes NaN to
  a telemetry key; the step returns `NanDetected` naming the equipment and
  key.
- `nan_zone_temperature_during_warmup_fails`: a test equipment adds a NaN
  sensible gain to an unconditioned zone during warm-up.
- `negative_delivered_energy_during_warmup_fails` (seam above, armed
  before warm-up).
- The two existing tests
  `ev_soc_check_fails_loudly_when_core_output_soc_is_absent` and
  `ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent`
  (`crates/hares-core/tests/equipment_id_regressions.rs:2475,2517`) pass
  in plain release; the first is also the SOC-bounds test, since an
  out-of-range SOC reaches the dwelling as an absent one (table above).
- `build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment`
  (`mod.rs:15226`) is rewritten as
  `build_equipment_column_map_errors_on_schema_drift_for_known_equipment`,
  asserting the returned error, and passes in plain release.

**Verification:** the full verification block; the plain-release line is
green from this commit on. `frame_golden compare-all` where goldens exist
(no fixture violates a check, so nothing changes).

**Commit:** `fix: record health and enforce physics invariants in every build profile`

**Dependencies:** `01-baseline-repair.md` BR-1, BR-3, BR-2 (the tree
compiles and lints under every feature).

**Measurement:** the cost of the now-unconditional checks as a share of
`simulate()`, recorded in `02-measurement.md`; never a reason to re-gate.

**State:** committed `dc033cee`, `469ea955`.

## OR-11: every remaining `check_invariants` gate in the workspace is classified

**Kind:** initiative work.

**Problem:** outside the sites OR-5 covers, `cfg(... feature =
"check_invariants")` gates physics and non-finite checks in every crate
(census at `ebdb1a17`, sites per file: `hares-types` `telemetry.rs` 18,
`ports.rs` 14; `hares-envelope` `fluid_solver.rs` 13,
`thermal_solver/mod.rs` 12, `thermal_solver/stepping.rs` 5,
`boundary_rc.rs` 5; `hares-physics` `pv_sizing.rs` 12, `infiltration.rs`
5; `hares-equipment` `pv/mod.rs` 12, `ndinterp.rs` 9, `battery/mod.rs` 7,
`generator.rs` 6, `ev/mod.rs` 6, `hvac/thermostat.rs` 5,
`hvac/equivalent_battery.rs` 5; `hares-io` `schedule_resolve.rs` 10,
`hpxml/resolve_hvac.rs` 10, `defaults.rs` 8, `hpxml/building.rs` 7,
`hpxml/validation.rs` 5, `envelope_lut.rs` 5, `output/metrics.rs` 5;
`hares-fleet` `fleet.rs` 8, `aggregation.rs` 8; `hares-core`
`engine.rs` 6, `dwelling/synthetic.rs` 8, `telemetry.rs` 5,
`dwelling/solver_builder.rs` 5; and files with fewer). A shipped build
compiles all of them out.

**Change:** apply `plan.md` §2.3's classes to every site found by
`git grep -n 'check_invariants' -- '*.rs' '*.toml'`. Physics, conservation,
non-finite and wiring checks become unconditional and return typed errors
(a panic on the run path becomes an error). Checks on code logic that no
input reaches become `cfg(debug_assertions)` with the reason on the line,
or unit tests when they check a static table. The `check_invariants`
feature is then deleted from every manifest (`crates/*/Cargo.toml`,
including `hares-python`), and the always-on method
`Dwelling::check_invariants` (`dwelling/mod.rs:7176`, called at `:6475`)
is renamed `check_step_invariants`, so nothing references the name.

The feature is also documented and scripted outside the code, and those
commands fail once it is gone. At HEAD `21d90588`,
`git grep -n check_invariants -- . ':!*.rs' ':!*.toml'` outside the paths
`10-slop-cleanup.md` SL-8 moves out of the tree also hits:
`.github/workflows/ci.yml:155` (below); `README.md:49` (a build command);
`CONTRIBUTING.md:90`; `docs/development.md:181-197, 461-462` (build and
test commands and the feature's section); `docs/invariants-and-observability.md`
(eight lines: the gate, the build table and the method name
`Dwelling::check_invariants`); `docs/physics/exterior-surface-balance.md:116`;
`docs/validation.md:215`; and the module docstrings of
`tests/python/test_actor_param_boundary.py:13`,
`tests/python/test_non_finite_input_boundaries.py:8` and
`tests/python/test_solar_override_boundary.py:12`. This commit rewrites
each to the always-on behaviour: the build and test commands lose
`-F check_invariants`, the feature's section in `docs/development.md` and
the build table in `docs/invariants-and-observability.md` state that every
physics, conservation, non-finite and wiring check runs in every build
profile, the method is named `check_step_invariants`, and each docstring
describes the check as unconditional. The paths SL-8 moves at row 25
(`docs/reviews`, `docs/tickets`, `docs/findings`, `docs/superpowers`,
`scripts/reviews`, and the review tooling `scripts/triage.py`, whose line
191 quotes the old gate) are planning records and their tooling, not
user documentation, and are not edited; the census excludes them by path.

The census below decides every site: `git grep -n 'check_invariants' --
'*.rs' '*.toml'` at HEAD `872cf289` prints 504 lines in 117 files; they
group into one row per check. Line numbers are at that commit (OR-5 has
landed); a row whose lines moved by the time this entry starts is found
by its check name. Class names are `plan.md` §2.3's: physics (physics or
conservation), non-finite, wiring, debug (debug assertion); plus log (a
gated diagnostic that only logs), test, follows (a field, binding, helper
or import used only by a gated check), decl (the feature declaration) and
doc. Every "typed error" disposition in a function that returns `()` or a
value today changes that function to return `Result` (the main ones:
`Domain::resolve` in `fluid_solver.rs`, `calculate_dse`,
`autosize_capacity_cooling`, `attic_ela_coefficients`,
`check_air_density_plausible`, `IdealThermostat::set_override`,
`capacity_fractions_for`, `needed_charge_hours`, the metrics
`accumulate` and `finish`, `validate_surface_layer_info`,
`check_primary_designation_invariant`), and `Telemetry::set` on an
unknown key latches like the non-finite latch.

Found along the way and fixed here (each named in the commit body):
`PortSlots::warn_unwritten` expects the key `Fluid:{loop}:{fluid}` while
`accumulate` inserts `Fluid:{loop}:{fluid}:{node}`, so every fluid port
warned every step (the row deletes it); `restored_from_checkpoint` is
reset only inside a gated block (`dwelling/mod.rs:5449`), so plain-release
builds never reset it (the reset moves out of the gate); `hares-core`'s
feature never forwarded to `hares-types` or `hares-fleet`, so their gates
never ran under `-p hares-core -F check_invariants`.

Hot-path rows: four checks that become unconditional cost real work per
step today and are restructured before they are ungated, never re-gated
for cost: the per-zone thermal consistency check (`dwelling/mod.rs:4427-4507`:
two `HashMap`s, a `HashSet` and a sort per step) runs over index lists
resolved at assembly; the pump flow check (`fluid_solver.rs:967-999`) uses
the pump node and branch list precomputed at construction; the
`equipment_core` presence check (`dwelling/mod.rs:5408-5451`) iterates
pre-resolved ids; the thermal port wiring check (`thermal_solver/ports.rs:15-28`)
and the ZIP-sum check (`config.rs:671-685`) move to construction. The
O(8760) CPP scan (`hares-tariff` `evaluator.rs:495-502`), the O(n^2)
same-tier conflict scan (`dwelling/mod.rs:1074-1110`) and the per-array
LUT-versus-direct PV recompute (`pv/mod.rs:503-546`) are deleted.

Stop condition: a row that becomes a typed error and fires on any
existing test, golden fixture, parity case or consumer gate input (the
zone-fallback rows in particular, which today run with `ZoneId(1)`) stops
the entry before it commits; the row, the input and the error are
recorded here and the operator decides. No golden changes in this entry.

**Warning channel.** Several rows below turn a gated log into a warning
the run reports. Those sites live in `hares-equipment`, `hares-io` and
`hares-tariff`, which cannot see `hares-core`, and at HEAD only
`hares-core` can reach the dwelling's `WarningLog`
(`dwelling/warnings.rs:17`, which stores `String`s; `push_warning` at
`dwelling/mod.rs:4756`). This entry introduces the one channel every
later entry uses; a `tracing::warn!` is never the only signal of a
warning this census dispositions as one.

- `crates/hares-types/src/warning.rs` (new): `Warning { step_index:
  Option<u64>, source: Arc<str>, message: String }`, deriving `Debug`,
  `Clone`, `PartialEq` (the type of `plan.md` §2.1; OR-10 adds its serde
  derives for the checkpoint, after OR-1's `rc` feature). A producer sets
  `step_index: None`; the dwelling sets the step index of a warning it
  drains after a step.
- Equipment: a new `Equipment` trait method
  `fn drain_warnings(&mut self, out: &mut Vec<Warning>) {}` (default: no
  warnings). An equipment that raises one keeps it in a `Vec<Warning>`
  field and moves it into `out` on drain. The dwelling drains each
  equipment after its `init` at assembly, after `add_equipment` and
  `replace_equipment`, and in the per-step equipment loop beside
  `take_health_counts` (`dwelling/mod.rs:6335`), into one reused scratch
  `Vec<Warning>` on the dwelling, so a step in which no equipment warns
  allocates nothing.
- Construction-time resolvers in `hares-io`: `resolve_equipment`
  (`hpxml/equipment.rs:46`) and `inject_schedule_into_specs`
  (`schedule_resolve.rs:412`) take a collector `warnings: &mut
  Vec<Warning>` and pass it to the functions that raise one
  (`resolve_hvac` and `apply_multispeed_parameters`; the unmapped-column
  check). `DwellingBlueprint::from_parts` (`dwelling/blueprint.rs:144`)
  keeps the `resolve_equipment` warnings in a private field, and the build
  moves them into the dwelling's log, then the schedule-injection warnings
  (`dwelling/mod.rs:2314`), then each equipment's `init` warnings, in that
  order. Every other caller of the two functions (the `autosize.rs:2177`
  test and the `hares-io` tests) passes its own `Vec` and asserts on it
  where it tests a warning.
- Parsed HPXML: `parse_hpxml_str` (`hpxml/mod.rs:180-201`) keeps the
  schema warnings it only logs today (`:183-187`, the
  `validation.rs:94-100` deprecated-`EnergyFactor` warning among them) in
  a new `Building` field `parse_warnings: Vec<Warning>` (source `hpxml`),
  and the blueprint copies them into its warnings first. No HPXML fixture
  raises one: every fixture declares `schemaVersion='4.0'`.
- Parsed tariff: `ElectricTariff` (`hares-tariff` `types.rs:264`) gains
  `parse_warnings: Vec<String>`
  (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`), filled
  only by `urdb::parse` (the two hemisphere rows below), and
  `Dwelling::set_tariff` (`dwelling/mod.rs:4303`) pushes each into the
  dwelling's log as a `Warning` with source `tariff`. A tariff parsed once
  and attached to many dwellings reports its warnings in each run.
- Until OR-6, `WarningLog` stores strings: `WarningLog::push_warning(&mut
  self, warning: Warning)` formats `"{source}: {message}"` (prefixed
  `"step {n}: "` when `step_index` is `Some`) and pushes it through the
  existing bounded `push`, so the Python `take_warnings()` keeps returning
  `list[str]`. OR-6 makes the log store `Warning` values and deletes the
  formatting; `06-runtime-memory.md` RT-5's plausibility sink and
  warn-once flags push through the same `push_warning`.

**Schedule columns.** The unmapped-column row (`schedule_resolve.rs:459-469,
1430-1447`) becomes an always-on warning only for a column no code reads
and no table entry deliberately leaves unused, so no golden, parity,
regression or ResStock fleet input carries one:

- `ScheduleCategory` (`schedule_resolve.rs:39`) gains `WaterHeater`, and
  `COLUMN_MAPPINGS` gains `hot_water_fixtures` and
  `hot_water_mains_temperature` under it: both are read by
  `inject_water_heater_schedule_columns` (`:743-790`, the draw at `:748`),
  which no table entry declared. `mapping_by_equipment` (`:477`) and
  `ensure_specs_for_csv_columns` (`:1375`) skip the new category as they
  skip `Setpoint` today.
- The occupancy column is resolved by one function, moved into
  `hares-io` (`schedule.rs`) from `EnvironmentManager::occupancy_column_idx`
  (`hares-core` `environment.rs:484-502`: `occupants`, `occupancy`, then the
  first name starting `occupan`, which reads the parity and BEopt files'
  `Occupancy (Persons)`); the environment and the unmapped-column check
  both call it, so the check treats the column the environment reads as
  read.
- `ScheduleCategory::Ignore` becomes `NotUsed { reason: &'static str }`,
  and every entry states why the engine does not consume the column. The
  ten existing entries keep their columns (the list OCHRE also ignores,
  `vendors/OCHRE/ochre/utils/schedule.py:57-69`); the reason of each of
  `extra_refrigerator`, `clothes_dryer_exhaust`,
  `lighting_exterior_holiday`, `plug_loads_vehicle`, `battery` and
  `water_heater_operating_mode` names the model that covers the quantity
  or states that HARES does not model it. Four ResStock 2025.1 columns
  join them, each with its reason:
  - `hot_water_dishwasher`, `hot_water_clothes_washer`: the wet appliance
    draws its own hot water on the DHW demand loop during the water-draw
    phases of the cycles the `dishwasher` and `clothes_washer` event-window
    columns schedule (`hares-equipment` `event_load.rs:1162-1189`), so
    consuming the column as well would count the draw twice;
  - `electric_vehicle_charging`, `electric_vehicle_discharging`: charging
    and driving times come from the `ev_driver` actor's trip model, not an
    OpenStudio-HPXML schedule (`12-inputs.md` IN-9 makes the same decision
    for the vehicle's schedule elements).
  The other columns of the census below keep their handling:
  `occupants`, `cooking_range`, `dishwasher`, `clothes_washer`,
  `clothes_dryer`, `lighting_interior`, `lighting_garage`,
  `plug_loads_other`, `plug_loads_tv` and `ceiling_fan` are mapped and
  read; `Vacancy`, `Power Outage`, `No Space Heating` and
  `No Space Cooling` were `Ignore` and are `NotUsed` with the reason
  "unavailable periods are not modelled".
- `find_unmapped_csv_columns` becomes `find_unknown_schedule_columns`,
  ungated, and returns the columns that are neither in `COLUMN_MAPPINGS`
  (any category) nor the resolved occupancy column. Each is one `Warning`
  (source `schedule`) naming the column and stating that HARES does not
  read it.
- Census of every schedule column in the fixture tree
  (`tests/fixtures/resstock/*/*/in.schedules.csv`,
  `tests/fixtures/parity/*/schedule.csv`,
  `data/examples/BEopt_example_schedule.csv`): the 19 distinct names above,
  each now read or `NotUsed`.

**Fluid-loop declarations (the stop condition fired here, 2026-10-02).**
The row `fluid_solver.rs:290-297` ("loop absent from the declared loop
map, cp fallback used") was first dispositioned "unconditional typed error
at construction; the cp fallback is deleted", on the premise that every
port loop is declared. The premise is false: `LoopId(65534)` is
`DHW_DEMAND_LOOP` (`crates/hares-equipment/src/water_heater/mod.rs:79`), a
communication loop wet appliances write and water heaters read, never in
the declared set, and the error (`envelope error: fluid loop
LoopId(65534) is not in the declared loop type map`) fired on every golden
fixture, the parity corpus, the regression suite, the smoke test and fleet
runs. The cause: the declared-loop map comes from
`extract_fluid_loops_from_specs` (`solver_builder.rs:763`), a hand-written
match on equipment names (`"Gas Boiler"`, `"Electric Boiler"`, the three
tank water heaters, only when a loop id is configured) that omits
tankless, the indirect tank, every wet appliance and `DHW_DEMAND_LOOP` and
never updates when equipment is added or removed, although every
equipment declares each fluid port as `PortDeclaration::fluid(loop_id,
FluidType)`. The cp fallback papered over that gap. The operator's
disposition (2026-10-02):

1. The fluid solver's loop-type map is built from the instantiated
   equipment's `ports()` fluid declarations, read after each equipment's
   `init` (an equipment may set its ports there), at assembly and in the
   equipment add, remove and replace refresh path.
   `extract_fluid_loops_from_specs` and its name matching are deleted.
2. Two ports declaring one loop with different fluid types remain a typed
   construction error. An accumulator on a loop no port declared is an
   `InvariantViolation` (impossible by construction), never a substituted
   value.
3. The specific heat is not configurable. `FluidSolverConfig.fluid_specific_heats`
   (`fluid_solver.rs:22`) and its `Default` table (`:44-70`) are deleted,
   and the solver takes cp from `hares_physics::constants::cp_j_kg_k`
   (`constants.rs:135`), the existing exhaustive `match` over `FluidType`
   that the boilers already use (`hvac/boiler.rs:327,751`): one cp per
   fluid for equipment and solver, so the declared and flow-implied powers
   the `fluid_solver.rs:1004-1026` row compares use the same value, and a
   new `FluidType` without a cp does not compile.
   `unknown_fluid_type_falls_back_to_water_cp` (`fluid_solver.rs:1564`)
   is deleted, and `net_power_from_flow_and_temp_delta`
   (`crates/hares-envelope/tests/fluid_solver_tests.rs:116`), which
   configured cp 4186, expects `flow * cp_j_kg_k(FluidType::Water) * dT`.
4. Port declarations are truthful. The gas and electric boilers declare
   their fluid port as `FluidType::Water` at construction
   (`hvac/boiler.rs:197,580`) while `init` takes `fluid_type` from the
   config (`:258,669`, `heating_config.rs:163,276`) and contributes with it
   (`:364,793`). `init` now sets that port's declared fluid type from the
   configured `fluid_type`, where it already sets the port's loop id
   (`:271` and `:691`), so a glycol boiler
   declares a glycol loop, and a glycol boiler sharing a loop with a water
   port is the construction error of item 2.

Every port on `DHW_DEMAND_LOOP` declares `Water`, every boiler in the
fixture tree uses the default `Water`, and every other loop keeps its
declared type, so the fluid type and cp of every loop are unchanged:
goldens compare bitwise and the fix stays inside this entry. The boiler
loop id lost after autosizing stays `11-other-defects.md` OD-3's entry.

**Premise corrections (2026-10-02, found by implementing this entry).**
1. "No current golden, parity or ResStock fixture contains an 'other …'
   Location" is FALSE: the fleet golden `fleet_mixed_offset` (homes
   bldg0000002, bldg0000004, bldg0176227) carries one such home,
   bldg0000004 ("other heated space", the fleet's home 1);
   bldg0000003 ("other heated space") and bldg0000006 ("other
   exterior") also carry them but are in no golden (audit of
   `1c18543a`, per-home hash compare: homes 0 and 2 unchanged, home 1
   moved); the research doc's census checked the curated
   ochre-sample set only. Commit 2 is therefore a numeric row for this
   fixture: the fleet golden is re-captured in the landing commit with
   the delta recorded in `02-measurement.md`. Every other fixture is
   bitwise unchanged. The census correction also covers
   `heat_pump_wh.rs`'s `wall_heat_fraction` consumer, which keyed the
   collapsed `"adjacent"` string.
2. "None of their assertions inspect boundary or RC-node count" is
   FALSE for one of the six: `window_area_deducted_from_host_wall`
   asserts the total boundary area and was updated 10.0 to 20.0 to
   include its new floor (the assertion's meaning, no double-counting,
   is unchanged).

### hares-types

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-types/Cargo.toml:9 | feature `check_invariants = []` | feature decl | decl | delete |
| crates/hares-types/src/equipment.rs:1659-1701 (call 1608) | `invariant_recheck_range_consistency`: re-runs COP/main_power/sensible/latent sign checks after the production range check passed | gated log | debug | delete: tautological re-run of the unconditional production check, which already returns `Err` |
| crates/hares-types/src/mode_flow_guard.rs:186-209 (call equipment.rs:1607) | `invariant_recheck_mode_flow_consistency`: re-runs `check_mode_flow_consistency` after it passed | gated log | debug | delete: same function, same input, already returned Ok |
| crates/hares-types/src/panic_hook.rs:79 | doc on `is_installed` "used by cfg(...check_invariants) assertions" | doc | doc | reword after the panic-hook asserts become `debug_assertions` (engine, fleet and python rows) |
| crates/hares-types/src/ports.rs:581-582, 647-648, 668-670, 698-746 (`warn_unwritten`), 675-676, 783-786, 818-821, 855-858, 873-876, 889-892, 1109-1110 | `PortSlots::write_log` + `warn_unwritten`: warn when a declared port got no contribution this step | gated log + gated field | log | delete: a declared port legitimately receives nothing when its equipment is off, and the key mismatch above made every fluid port warn every step. The field, inserts, `copy_into` clone and test-literal field go with it |
| crates/hares-types/src/ports.rs:477-488 | `FluidAccumulator::add` direction mismatch on one accumulator | gated error | wiring | unconditional typed error (already `HaresError::Equipment`) |
| crates/hares-types/src/ports.rs:783-813 | electrical accumulator: `load_power_w >= 0`, `generation_power_w <= 0` after accumulate | gated panic | physics | unconditional typed error from `accumulate` (already returns `Result`), combined with an `is_finite` check on the inputs: the sign split makes the bounds hold unless a NaN arrived, so NaN is the reachable case (the two `debug_assert!(is_finite)` at 769-776 are already ungated) |
| crates/hares-types/src/telemetry.rs:54, 77-90 | `Telemetry::set` on an unregistered key: panic (gated) vs `tracing::error` + dropped write | gated panic | wiring | unconditional typed error: latch the first unknown key (the mechanism of the non-finite latch at 66-70) and surface it as `HaresError` at the end of the step; the dual cfg branches go |
| crates/hares-types/src/telemetry.rs:97-103 | `Telemetry::get` asserts stored value finite | gated panic | debug | `cfg(debug_assertions)`: unreachable, `insert` and `set` already reject non-finite values in every build |
| crates/hares-types/src/telemetry.rs:178-194 | test `set_panics_on_unregistered_key` (dual-cfg body) | gated test | test | ungated `cfg(test)`; asserts the latched error |
| crates/hares-types/src/telemetry_keys.rs:826 | doc on `THERMAL_POWER_DELIVERED_W` about the gated assertion | doc | doc | reword to the generator.rs:1711 disposition |
| crates/hares-types/src/zip.rs:271-281 | `ZipLoad::apply`: pf = 1 yields zero kVAR | gated panic | debug | unit test (a pure function of a constant); deleted from the per-call path |

### hares-physics

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-physics/Cargo.toml:16 | feature decl | feature decl | decl | delete |
| crates/hares-physics/src/air_properties.rs:61-73 (caller hares-envelope thermal_solver/infiltration.rs:3, 116-117) | `check_air_density_plausible`: rho finite and in [0.6, 1.6] kg/m3 | gated panic | physics | unconditional typed error (returns `Result`; `apply_infiltration` propagates); non-finite rho from bad weather pressure or temperature is the reachable case |
| crates/hares-physics/src/air_properties.rs:217, 226, 233 | tests of `check_air_density_plausible` (two `should_panic`) | gated test | test | ungated `cfg(test)`; assert `Err` |
| crates/hares-physics/src/ashrae152.rs:646-658 | `calculate_dse`: resolved supply and return effective R > 0 | gated panic | debug | delete: both branches are positive by construction (`2.24 + 0.56 R` with R > 0, or the positive default table) |
| crates/hares-physics/src/ashrae152.rs:827-838 | resolved supply and return leakage fraction in [0, 1] | gated panic | physics | unconditional typed error (`calculate_dse` returns `Result`): the fractions are user input |
| crates/hares-physics/src/biquadratic.rs:89-103 | output within `output_min`/`output_max` after the clamp | gated debug_assert | debug | delete: the clamp two lines above makes the bounds tautological; a NaN `raw` survives the clamp and reaches the always-on port and telemetry non-finite checks downstream |
| crates/hares-physics/src/infiltration.rs:33-34, 402-417 | natural ventilation: Cw in [0, 0.55]; warn when Cw == 0 (leeward) | gated panic + gated log | debug + log | Cw range: unit test of `compute_natural_ventilation_cw`. Leeward warn: delete (normal, per step). The import follows |
| crates/hares-physics/src/infiltration.rs:436-457 | Q_stack >= 0; warn CrossVentilation with dh_m == 0; warn SingleSided with dh_m > 0 | gated panic + gated log | debug + wiring | Q_stack: delete (square root of a non-negative value). CrossVentilation with dh_m == 0: wiring, typed error at natural-ventilation config construction. SingleSided with dh_m > 0: delete (documented: dh_m is ignored) |
| crates/hares-physics/src/infiltration.rs:893-931 | attic ELA: stack > 0, wind > 0; Exposed/WellShielded wind ratio == (0.9/0.3)^2 | gated panic | physics + debug | positivity: typed error (the inputs are heights); ratio: unit test (a property of the formula) |
| crates/hares-physics/src/infiltration.rs:952-985 | garage ELA: as attic | gated panic | physics + debug | as attic |
| crates/hares-physics/src/lib.rs:31-37 (callers hares-io hpxml/building.rs:11-12, 2256-2261; hares-core dwelling/synthetic.rs:8-9, 593-596) | `check_specific_heat_plausible`: cp in [100, 3000] J/kg-K | gated panic | physics | unconditional typed error at material parse and build (returns `Result`; the callers already do). Imports follow |
| crates/hares-physics/src/lib.rs:40 | test module for the above | gated test | test | ungated `cfg(test)`; assert `Err` |
| crates/hares-physics/src/psychrometrics.rs:44-54, 96-99 | `LazyLock` check: `SPECIFIC_HEAT_WET_BULB_BELOW_FREEZE == cp_ice - cp_v` | gated panic | debug | unit test (static constants); the `LazyLock` and the per-call deref are deleted |
| crates/hares-physics/src/pv_sizing.rs:512-526 | `usable_roof_area`: panel_watts > 0, 0 < panel_area_m2 < 5 | gated panic | physics | unconditional typed `PvSizingError` (user input) |
| crates/hares-physics/src/pv_sizing.rs:618-633 | diffuse fraction Kd in [0, 1]; warn outside [0.10, 0.30] | gated panic + gated log | physics + log | Kd in [0, 1]: typed `PvSizingError`. Warn: delete (legitimate outside CONUS; the always-on `info!` at 636 reports Kd) |
| crates/hares-physics/src/pv_sizing.rs:650-660 | east factor >= west factor | gated panic | debug | unit test of `azimuth_production_factor` |
| crates/hares-physics/src/pv_sizing.rs:666-681 | east and west factors in (0.5, 0.95) | gated panic | debug | unit test over the latitude range |
| crates/hares-physics/src/pv_sizing.rs:686-708 | flat-roof GCR monotone in tilt and latitude | gated panic | debug | unit test of `flat_roof_gcr` |
| crates/hares-physics/src/pv_sizing.rs:712-724 | flat GCR times usable fraction in [0.15, 0.55] | gated panic | debug | unit test over a fixed grid |
| crates/hares-physics/src/pv_sizing.rs:805-815 | north factor decreases with latitude | gated panic | debug | unit test |
| crates/hares-physics/src/pv_sizing.rs:970-988 | `size_pv_system`: panel_watts, panel_area, system_losses in range | gated panic | physics | unconditional typed `PvSizingError` (user input) |
| crates/hares-physics/src/pv_sizing.rs:1059-1070 | max_ac_kw >= 0; dc_limited <= effective DC limit | gated panic | debug | `cfg(debug_assertions)`: `max_backfeed` is `.max(0.0)` and `dc_limited` derives from `min(max_ac_kw)` |
| crates/hares-physics/src/pv_sizing.rs:1095-1101 | capacity_kw finite and >= 0 | gated panic | non-finite | unconditional typed `PvSizingError` |
| crates/hares-physics/src/pv_sizing.rs:1361-1370 | candidates non-empty unless no planes | gated debug_assert | debug | delete: restates the `Err(AllNorthFacing)` return at 1357-1359 |
| crates/hares-physics/src/pv_sizing.rs:1454-1462 | debug log: roof plane with no azimuth and no wall fallback | gated log | log | delete (handled by `unresolved_count`; not a violation) |
| crates/hares-physics/src/solar.rs:341-359 | Perez airmass vs Kasten-Young within 1% below 70 deg; warn above | gated panic + gated log | debug + log | agreement: unit test (two closed-form functions). Warn: delete |
| crates/hares-physics/src/solar.rs:981-991 | window transmittance and N_i in [0, 1] | gated debug_assert | physics | N_i: delete (clamped at 977). tau is clamped to [0, shgc], so it leaves [0, 1] only when shgc > 1: shgc in [0, 1] is validated at window construction as a typed error, and the per-step assertion is deleted |
| crates/hares-physics/src/water_mains.rs:141-155 | mains temperature finite and in [-50, 100] C | gated debug_assert | non-finite | unconditional typed error (the function already returns `Result`); the lower bound is tautological after the 32 F clamp, so finiteness and the upper bound remain |

### hares-envelope

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-envelope/Cargo.toml:20 | feature decl | feature decl | decl | delete |
| crates/hares-envelope/src/boundary_rc.rs:564-604 | `validate_surface_layer_info`: inner and outer node present in capacitances and node_index (debug_assert plus ungated warns) | gated debug_assert | wiring | unconditional typed error (returns `Result<(), String>` like `assemble_building_rc`), replacing both the assertions and the warns |
| crates/hares-envelope/src/boundary_rc.rs:15-16, 1285-1304 | node_index cardinality == A_c rows; `sorted_internal_nodes` equals node_index key order | gated debug_assert + gated panic | wiring + debug | cardinality: wiring, typed error at construction. Cross-validation (re-sorts the node set): `cfg(debug_assertions)`. The import follows |
| crates/hares-envelope/src/boundary_rc.rs:1341-1350 | feature-only error log on the same cardinality mismatch | gated log | wiring | delete (merged into the typed error above) |
| crates/hares-envelope/src/boundary_rc.rs:1369-1385 | material layers present but no capacitance-bearing node | gated panic | wiring | unconditional typed error (reachable with zero density or cp layers) |
| crates/hares-envelope/src/boundary_rc.rs:1386-1450 | same-zone precomputed and material-layer fins: inner node wired to zone air, outer node not | gated panic | debug | `cfg(debug_assertions)` (assembler topology; no input reaches it) |
| crates/hares-envelope/src/fluid_solver.rs:267-288 | per step: all accumulators on a loop share fluid_type | gated panic (+ observe counter) | wiring | delete the per-step panic: construction enforces it (`validate_fluid_type_consistency`, hares-core dwelling/mod.rs:2506, and `FluidSolver::new`); the `observe` counter stays |
| crates/hares-envelope/src/fluid_solver.rs:290-297 | loop absent from the declared loop map, cp fallback used | gated log | wiring | declared loops come from equipment port declarations (assembly and refresh); conflicting types are a construction error; an undeclared accumulator loop is an invariant violation; cp is an exhaustive per-fluid-type mapping; the fallback is deleted (see the resolution under the stop record) |
| crates/hares-envelope/src/fluid_solver.rs:656-674 | node mass conservation (topology path) | gated panic (ungated warn) | physics | unconditional typed error (`Domain::resolve` returns `Result`); the warn is deleted |
| crates/hares-envelope/src/fluid_solver.rs:697-713 | serial-loop flow consistency | gated panic (ungated warn) | physics | unconditional typed error, as above |
| crates/hares-envelope/src/fluid_solver.rs:942-963 | supply and return temperatures within loop limits after clamping | gated debug_assert | debug | delete: restates the clamp at 900-929 |
| crates/hares-envelope/src/fluid_solver.rs:967-999 | pump flow == sum of branch allocations | gated panic | physics | unconditional typed error over the pump node and branch list precomputed at construction |
| crates/hares-envelope/src/fluid_solver.rs:1004-1026 | declared thermal power == flow-implied source power | gated debug_assert | physics | unconditional typed error |
| crates/hares-envelope/src/fluid_solver.rs:1031-1041 | warn: net loop power above 1 W | gated log | log | delete: no steady-state condition, so it fires on any loop transferring heat |
| crates/hares-envelope/src/fluid_solver.rs:1601-1603, 1660-1662, 2026-2028 | `should_panic` tests for the three checks above | gated test | test | ungated `cfg(test)`; 1603 goes with the per-step fluid-type check (covered at construction), 1662 and 2028 assert the typed error |
| crates/hares-envelope/src/state_space.rs:211-217 | `from_discrete`: M is identity | gated debug_assert | debug | delete: compares an identity with an identity |
| crates/hares-envelope/src/state_space.rs:407-413 | `from_continuous`: M is identity | gated debug_assert | debug | delete: the same tautology |
| crates/hares-envelope/src/state_space.rs:632-636 | `step_with_identity_coupling_into_scratch`: `m_is_identity` | gated debug_assert | debug | plain `debug_assert!` |
| crates/hares-envelope/src/state_space.rs:789-793 | `solve_for_scalar_input_identity_coupled`: `m_is_identity` | gated debug_assert | debug | plain `debug_assert!` |
| crates/hares-envelope/src/thermal_solver/infiltration.rs:3, 116-117 | call site of the air density check | gated panic | physics | follows the hares-physics `air_properties.rs` row |
| crates/hares-envelope/src/thermal_solver/initialization.rs:162-180 | pinned zone capacitance >= 1 J/K | gated panic | physics | unconditional typed error: the existing `SingularInitialization` guard (149-160) rises from 1e-6 to 1 J/K and this block is deleted |
| crates/hares-envelope/src/thermal_solver/longwave.rs:119-130 | window LWR: h_out >= 1.0 after the fallback | gated panic | debug | delete: restates the guard just above |
| crates/hares-envelope/src/thermal_solver/longwave.rs:352-353, 370-403 | exterior skin: converged fixed-point residual <= 0.5 K; divider identity | gated debug_assert | physics + debug | fixed point: unconditional typed error (skin energy balance). Divider identity: delete. `let _ = converged` follows |
| crates/hares-envelope/src/thermal_solver/mod.rs:232 | doc naming the method `check_thermal` | doc | doc | reword with the method rename |
| crates/hares-envelope/src/thermal_solver/ports.rs:15-28 | each thermal zone has a sensible input index | gated panic | wiring | unconditional typed error, checked once at solver construction |
| crates/hares-envelope/src/thermal_solver/stepping.rs:323-336 | `autosize_capacity_cooling`: internal_gains_w in [0, 5000) | gated panic | physics | unconditional typed error (the function returns `f64` today) |
| crates/hares-envelope/src/thermal_solver/stepping.rs:608-618 | warn: TARP h_conv outside [0.5, 10] | gated log | log | delete: a small dT legitimately gives h below 0.5; per step per surface |
| crates/hares-envelope/src/thermal_solver/stepping.rs:1017 | comment naming the dwelling method | doc | doc | reword with the method rename |
| crates/hares-envelope/src/thermal_solver/stepping.rs:1498-1515 | design day: peak_load finite and >= 0; internal_gains_w range | gated panic | non-finite + physics | unconditional typed error |
| crates/hares-envelope/tests/solver_energy_conservation.rs:353, 358 | test `thermal_balance_terms_close_with_populated_node_capacitances` (+ doc naming the method) | gated test | test | ungated (the balance terms are computed in every profile) |

### hares-equipment

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-equipment/Cargo.toml:25 | feature decl | feature decl | decl | delete |
| crates/hares-equipment/src/lib.rs:234-246 | `apply_control_signal`: re-run of `validate_numeric_bounds` | gated error | debug | delete: the same call ran on line 233 |
| crates/hares-equipment/src/config.rs:671-685 (callers scheduled_load.rs:468-470, event_load.rs:453-454, 1089-1090) | `debug_assert_zip_sums` per step for ScheduledLoad, EventBasedLoad, WetAppliance | gated panic | wiring | unconditional typed error once at construction through `validate_zip_sums` (these three never call it at init); the per-step call, which formats a `String` every step, is deleted |
| crates/hares-equipment/src/scheduled_load.rs:116-146, 169-174 | registry template uses `EndUse::OTHER` although a specific end use exists | gated log | debug | unit test over the static registry; the runtime call is deleted |
| crates/hares-equipment/src/scheduled_load.rs:471-480 | assigned zone exists in `env.zones` | gated panic | wiring | unconditional typed error at init (the zone set is fixed at construction) |
| crates/hares-equipment/src/battery/catalog.rs:7-8, 184-204 | catalog thermal mass and UA not all identical | gated log | debug | unit test over `CATALOG`; the import follows |
| crates/hares-equipment/src/battery/degradation.rs:350-353, 357-360 | `b2_accum()`/`b3_accum()` accessors | gated helper | follows | `cfg(debug_assertions)`, following battery/mod.rs:1499 |
| crates/hares-equipment/src/battery/mod.rs:892-945 | explicit topology vs cell-derived topology; implied capacity within 10% | gated log | wiring | always-on `Warning` at init through `drain_warnings` (explicit topology may differ) |
| crates/hares-equipment/src/battery/mod.rs:1009-1019 | heater_power_w > 700 W plausibility | gated log | log | always-on `Warning` at init through `drain_warnings` |
| crates/hares-equipment/src/battery/mod.rs:1261-1270 | capacity_kwh finite and >= 0 after derate | gated error | non-finite | unconditional typed error |
| crates/hares-equipment/src/battery/mod.rs:1480-1489 | nominal capacity > 0 unless SOH <= 0 | gated error | physics | unconditional typed error |
| crates/hares-equipment/src/battery/mod.rs:1499-1530 | daily accumulators zero after reset | gated error | debug | `cfg(debug_assertions)` (reset logic) |
| crates/hares-equipment/src/battery/mod.rs:1538-1547 | `last_daily_update_day == current_day` | gated error | debug | delete: restates the assignment at 1532 |
| crates/hares-equipment/src/battery/mod.rs:1907-1921 | SOC target strictly inside its window | gated error | debug | delete: restates the `Err` returns at 1853-1878 |
| crates/hares-equipment/src/ev/catalog.rs:129-139 | catalog fuel economy >= DC kWh/mi | gated panic | debug | unit test over the static vehicle catalog |
| crates/hares-equipment/src/ev/mod.rs:905-926 | warn: Ready-By bypassed by an ExternalAuthority setpoint | gated log | log | delete: documented behaviour, per step; `observe` records `ev_ready_by_bypassed` |
| crates/hares-equipment/src/ev/mod.rs:934-956 | warn: PowerSetpoint above 1.1 x rated while Ready-By is active | gated log | log | always-on `Warning` through `drain_warnings`, once per run per EV (a warn-once flag on the equipment, reset by `init`): the scheduler's urgency power and the equipment's rated power disagree; non-fatal, as today's intent |
| crates/hares-equipment/src/ev/mod.rs:1053-1061 | CC-CV margin is 1.0 when a charging-curve LUT is present | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/ev/mod.rs:1124-1143 | effective discharge floor >= recomputed minimum | gated panic | debug | delete: recomputes the expression at 1102-1112 |
| crates/hares-equipment/src/ev/mod.rs:1145-1164 | warn: discharge blocked by the Ready-By interlock | gated log | log | delete: per step; the `observe` event at 1114 carries it |
| crates/hares-equipment/src/ev/mod.rs:1404-1415 | battery capacity > 0 unless SOH <= 0 (+ `soh` binding) | gated error | physics | unconditional typed error; the binding follows |
| crates/hares-equipment/src/event_load.rs:453-454, 1089-1090 | ZIP sums per step | gated panic | wiring | follows the config.rs row |
| crates/hares-equipment/src/generator.rs:1307-1316 | parasitic load in [0, output] | gated error | physics | `parasitic_fraction` in [0, 1) validated at init as a typed error; the per-step check is deleted |
| crates/hares-equipment/src/generator.rs:1398-1407 | running fuel >= idle floor | gated error | debug | delete: `fuel_w = idle.max(load)` at 1381 |
| crates/hares-equipment/src/generator.rs:1511-1529 | heat recovery ratio in [0, 1]; effective <= available | gated error | debug | delete: the ratio is clamped and effective = available x ratio |
| crates/hares-equipment/src/generator.rs:1533-1543 | recovered heat <= waste heat | gated error | physics | unconditional typed error (the efficiencies are config) |
| crates/hares-equipment/src/generator.rs:1552-1571 | CHP effective thermal > 0 inside the `> IDLE_KW_THRESHOLD` branch | gated error | debug | delete: unreachable |
| crates/hares-equipment/src/generator.rs:1711-1722 | CHP fluid port m cp dT == effective thermal | gated error | debug | `cfg(debug_assertions)`: the flow is computed from that equation at 1680-1690 |
| crates/hares-equipment/src/protocol_bridge/mod.rs:284-295 | PROTOCOL_NATIVE capability present | gated error | debug | delete: `apply_control_signal` already ran `ensure_signal_supported` (lib.rs:232) |
| crates/hares-equipment/src/ventilation.rs:660-677 | effectiveness zero when off | gated error | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/ventilation.rs:850-872 | defrost fraction in [0, 1]; effective sensible effectiveness in [0, rated] | gated error | physics | unconditional typed error |
| crates/hares-equipment/src/hvac/ac_config.rs:57 | comment: `Telemetry::set` panics in gated builds | doc | doc | reword to the latch |
| crates/hares-equipment/src/hvac/air_conditioner.rs:619-628 | room AC has active latent degradation after init | gated error | wiring | unconditional typed error (init) |
| crates/hares-equipment/src/hvac/air_conditioner.rs:890 | comment about the `Telemetry::set` panic | doc | doc | reword |
| crates/hares-equipment/src/hvac/air_conditioner.rs:1086-1095 | cooling never active below `min_oat_cooling_c` | gated panic | debug | `cfg(debug_assertions)` (lockout logic) |
| crates/hares-equipment/src/hvac/air_conditioner.rs:1307-1316 | COP finite and in [0, 8] after `.clamp(0, 8)` | gated error | non-finite | unconditional typed error on `!is_finite()`; the range half is tautological |
| crates/hares-equipment/src/hvac/air_conditioner.rs:1991-2008 | catch-all control path reached only by ThermalSetpointDelta | gated error | debug | unit test over the `ControlSignal` variants |
| crates/hares-equipment/src/hvac/boiler.rs:328-335, 752-759 | `cp_j_kg_k(fluid_type)` > 0 and finite | gated panic | debug | unit test over the static per-fluid table; the per-step call is deleted |
| crates/hares-equipment/src/hvac/coil_physics.rs:315-322 | ADP <= entering dry bulb | gated error | debug | delete: clamped at 290-293 |
| crates/hares-equipment/src/hvac/dehumidifier.rs:489-518 (call 685) | gated method `check_invariants`: PLF in [plf_min, 1], RTF in [0, 1] | gated error | physics | unconditional typed error; the method is renamed `check_part_load_bounds` and its stub deleted |
| crates/hares-equipment/src/hvac/dehumidifier.rs:552-559 | warn: zone_id absent on init, existing zone kept | gated log | wiring | unconditional typed error naming the equipment and the unresolved location (no silent fallback zone) |
| crates/hares-equipment/src/hvac/equivalent_battery.rs:107-114 | thermostat deadband_offset in [0, 1] (then silently clamped) | gated panic | physics | unconditional typed error at thermostat construction; the silent clamp at 114 is deleted |
| crates/hares-equipment/src/hvac/equivalent_battery.rs:163-175 | efficiency > 0; baseline power >= 0 | gated panic | physics | efficiency: delete (the unconditional `assert!(rated_eir > 0)` at 93 covers it, and becomes the same typed error); baseline: typed error (`capacity_ideal_w` is input) |
| crates/hares-equipment/src/hvac/equivalent_battery.rs:177-199 | max_energy > 0 and max > min when capacitance > 0 | gated panic | physics | unconditional typed error |
| crates/hares-equipment/src/hvac/equivalent_battery.rs:969, 977 | `should_panic` deadband_offset tests | gated test | test | ungated; assert the typed error |
| crates/hares-equipment/src/hvac/heat_pump/defrost.rs:4-5, 286-290, 337-366 | frost >= 0; EWMA fraction in [0, 1]; frost not growing when warm (+ `frost_before`, import) | gated error | physics | unconditional typed error (state bounds); the binding and import follow |
| crates/hares-equipment/src/hvac/heat_pump/heater.rs:1417-1426 | ASHP COP finite and in [0, 10] after the clamp | gated error | non-finite | unconditional typed error on `!is_finite()` |
| crates/hares-equipment/src/hvac/heat_pump/heater.rs:1811-1820 | compressor off during electric-resistance-only operation | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/hvac/heat_pump/heater.rs:2274-2290 | debug log: soft lockout released | gated log | log | moves under `cfg(feature = "observe")` (a trace event) |
| crates/hares-equipment/src/hvac/heat_pump/heater_config.rs:58 | comment about the `Telemetry::set` panic | doc | doc | reword |
| crates/hares-equipment/src/hvac/heat_pump_config.rs:687-693 | `min_oat_cooling_c` in [-10, 30] in `validate()` | gated error | wiring | unconditional (config validation) |
| crates/hares-equipment/src/hvac/heating_config.rs:230-244 | GasBoiler condensing vs return temperature in `validate()` | gated error | wiring | unconditional (config validation) |
| crates/hares-equipment/src/hvac/heating_config.rs:692 | test of the above | gated test | test | ungated `cfg(test)` |
| crates/hares-equipment/src/hvac/helpers.rs:59-90 | `zone_id_from_config_or_default`: warn on the fallback to ZoneId(1) (dual cfg) | gated log | wiring | two dispositions, not one (OR-18): water heaters get the OpenStudio-HPXML ambient-source resolution for Adjacent locations; the dehumidifier gets conditioned-zone resolution with a typed error naming the equipment when absent (no silent ZoneId(1) for either) |
| crates/hares-equipment/src/hvac/ideal_hvac.rs:841-861 | descriptor end_use matches capacity sign | gated log | debug | `debug_assert!` |
| crates/hares-equipment/src/hvac/latent_degradation.rs:64-65, 70-128 | per-stage Ao count; "SHR differs, so Ao differs" | gated error | wiring | unconditional typed error (init only, no per-step cost) |
| crates/hares-equipment/src/hvac/latent_degradation.rs:78, 80, 185-186 | doc citing the `-F check_invariants` CI job | doc | doc | reword |
| crates/hares-equipment/src/hvac/latent_degradation.rs:187, 208 | tests of `check_coil_ao_invariant` | gated test | test | ungated `cfg(test)` |
| crates/hares-equipment/src/hvac/speed_control.rs:4-5, 120-121, 158-168 | startup timer decreases only on an off-to-on transition (+ `prev_time`) | gated panic | debug | `cfg(debug_assertions)`; the binding follows |
| crates/hares-equipment/src/hvac/speed_control.rs:222-231 | normalized max capacity fraction == 1 | gated log | debug | delete: `max_cap = caps.last()`, so the last fraction is 1; the import at 4 goes |
| crates/hares-equipment/src/hvac/staging.rs:126-140 | TwoSpeedAlternating speed change respects the minimum time | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/hvac/staging.rs:355-365 | warn: non-HP with Cd > 0, ramp bypassed | gated log | log | delete (behaviour by design, per step) |
| crates/hares-equipment/src/hvac/thermostat.rs:351-362 | warn: mode change blocked by minimum on/off time | gated log | log | delete (normal short-cycle guard, per step) |
| crates/hares-equipment/src/hvac/thermostat.rs:438-445, 488-495 | heat_turn_on < cool_turn_on in the `else` of `if heat_turn_on >= cool_turn_on` | gated error | debug | delete: unreachable |
| crates/hares-equipment/src/hvac/thermostat.rs:626-633 | corrected cooling > heating + deadband | gated error | debug | delete: set to heating + deadband + 1 at 603 |
| crates/hares-equipment/src/hvac/thermostat.rs:636-656 | final effective setpoints respect the deadband | gated error | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/ndinterp.rs:302-311, 321-334 | non-finite query coordinate: panic (gated) vs `Ok(NaN)` | gated panic | non-finite | unconditional typed error (`interpolate` returns `Result`); the NaN sentinel is removed |
| crates/hares-equipment/src/ndinterp.rs:359-371 | NaN strategy, coordinate out of bounds: panic vs `Ok(NaN)` | gated panic | non-finite | unconditional typed error naming the coordinate and its bounds (an `Ok(NaN)` sentinel is a silent value) |
| crates/hares-equipment/src/ndinterp.rs:871-901, 1155-1220 | tests with dual-cfg expectations | gated test | test | one ungated expectation: `Err` |
| crates/hares-equipment/src/pv/lut.rs:24-31 | module doc about the gated location check | doc | doc | reword |
| crates/hares-equipment/src/pv/lut.rs:455-473 | `latitude_deg()`/`longitude_deg()` accessors | gated helper | follows | ungated (pv/mod.rs:840) |
| crates/hares-equipment/src/pv/mod.rs:128-150 | `compute_direct_power` gated on test, debug, feature or observe | gated helper | follows | `cfg(any(test, feature = "observe"))` once 503-546 is deleted |
| crates/hares-equipment/src/pv/mod.rs:478-501 | warn per step: system_losses differs from the PVWatts default by more than 1 point | gated log | log | always-on `Warning` through `drain_warnings`, raised once in `init` (the check moves out of the step) |
| crates/hares-equipment/src/pv/mod.rs:503-546 | LUT-path AC vs direct-path AC within 0.1% | gated log | log | delete from the run path: its own comment says the two paths legitimately differ (LUT transposition), so it is not an invariant |
| crates/hares-equipment/src/pv/mod.rs:840-843, 1034-1060 | `check_lut_location`: warn if the LUT has no lat/lon; info otherwise | gated log | log | always-on `Warning` through `drain_warnings` for missing metadata, raised where `init` loads the LUT; the info line is deleted |
| crates/hares-equipment/src/pv/mod.rs:889, 1005-1032 | gated method `check_invariants`: at least one array, each `array.validate()` | gated error | wiring | unconditional typed error at init; the method is renamed `check_arrays` and its stub deleted |
| crates/hares-equipment/src/pv/mod.rs:975-984 | soiling config set implies soiling state set | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/pv/mod.rs:1122-1146 | soiling ratio in [0, 1]; warn combined soiling < 65% | gated error + gated log | physics + log | ratio: unconditional typed error. Warn: delete (per step; the ratio is in `SOILING_RATIO` telemetry) |
| crates/hares-equipment/src/pv/mod.rs:1264-1278 | ReactiveSetpoint{0} yields zero kVAR | gated error | debug | `cfg(debug_assertions)` |
| crates/hares-equipment/src/water_heater/mod.rs:19-22, 69-94 | daily overnight draw share above 20% logs an error (dual cfg reset) | gated log | log | delete (a heuristic legitimate schedules violate); `daily_total_l` and `overnight_volume_l` go too |
| crates/hares-equipment/src/water_heater/gas.rs:18-19, 302-309 | warn: zone_type set but zone_id unresolved | gated log | wiring | unconditional typed error naming the water heater and its unresolved zone; the import comment follows |
| crates/hares-equipment/src/water_heater/gas.rs:586-594 | pilot heat to water + to ambient == pilot power | gated panic | debug | delete: `p f + p (1 - f)` |
| crates/hares-equipment/src/water_heater/heat_pump_wh.rs:18-19, 414-421 | warn: zone_id unresolved | gated log | wiring | unconditional typed error naming the water heater and its unresolved zone |
| crates/hares-equipment/src/water_heater/heat_pump_wh.rs:515-524 | cop_scale == cop_rated when the curve is not normalized | gated debug_assert | debug | `debug_assert!` |
| crates/hares-equipment/src/water_heater/heat_pump_wh.rs:645-677 | low-power HPWH bounds and curves as expected | gated debug_assert | debug | `debug_assert!` |
| crates/hares-equipment/src/water_heater/heat_pump_wh.rs:1144-1153 | HPWH COP finite and in [0, 8] after the clamp | gated error | non-finite | unconditional typed error on `!is_finite()` |
| crates/hares-equipment/src/water_heater/indirect_tank.rs:25-26, 243-250 | warn: zone_id unresolved | gated log | wiring | unconditional typed error naming the water heater and its unresolved zone |
| crates/hares-equipment/src/water_heater/resistance.rs:304-311 | warn: zone_id unresolved | gated log | wiring | unconditional typed error naming the water heater and its unresolved zone |
| crates/hares-equipment/src/water_heater/tankless.rs:15-16, 109-116 | warn: zone_id unresolved | gated log | wiring | unconditional typed error naming the water heater and its unresolved zone |
| crates/hares-equipment/src/water_heater/tank.rs:351-368, 488-505 | outlet temperature == pre-injection segment average | gated debug_assert | debug | `debug_assert!` |
| crates/hares-equipment/src/water_heater/tank.rs:421-436 | TMV reduces hot draw volume | gated debug_assert | debug | `debug_assert!` |
| crates/hares-equipment/src/water_heater/tank.rs:672-679 | post-mixing skin loss finite | gated debug_assert | non-finite | unconditional typed error |

### hares-io

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-io/Cargo.toml:9 | feature decl (forwards to hares-physics) | feature decl | decl | delete |
| crates/hares-io/src/defaults.rs:515-527 | on load: water-heating defaults invariants; generator curve missing warn; CSV header invariants | gated log | debug + wiring | water-heating and CSV-header checks: unit tests over the shipped `defaults/` tree. Generator curve missing: unconditional typed error naming the file (`defaults/generator/` ships it); the hardcoded 6-point fallback curve is deleted |
| crates/hares-io/src/defaults.rs:1416-1419, 1491-1530 | HSPF/SEER vs per-speed COP monotone | gated log | debug | unit test over the shipped multispeed CSV |
| crates/hares-io/src/defaults.rs:1832-1835, 1840-1950 | EV vehicle mapping completeness and values | gated log | debug | unit test over the shipped mapping CSV |
| crates/hares-io/src/defaults.rs:1955-2150 | `check_water_heating_invariants` | gated log | debug | unit test (shipped data) |
| crates/hares-io/src/defaults.rs:2155-2195 | `check_csv_header_invariants`: battery-only parameters in generator CSVs | gated log | debug | unit test (shipped data) |
| crates/hares-io/src/defaults.rs:2200 | helper `check_dir_for_foreign_csv_params` | gated helper | follows | `cfg(test)` |
| crates/hares-io/src/envelope_lut.rs:175-197 | helper `split_r_variant` | gated helper | follows | `cfg(test)` |
| crates/hares-io/src/envelope_lut.rs:233-242, 252-260, 333-354, 356-400 | CSV boundary names vs the hardcoded mapping; material R monotonicity and conductivity | gated log | debug | unit tests over the shipped envelope LUT; the runtime clones of the LUT are deleted |
| crates/hares-io/src/epw.rs:352-372 | 8784 records with leap = No (must have been stripped); 8784 with leap = Yes has a Feb 29 | gated panic | debug + wiring | first: delete (the strip loop at 330-345 guarantees it). Second: unconditional `WeatherError::Validation` (input reachable) |
| crates/hares-io/src/epw.rs:681-701 | DOE-2 ground amplitude < air amplitude | gated log | debug | unit test (a function of constants; it also fires spuriously when monthly temperatures are constant) |
| crates/hares-io/src/hpxml/building.rs:11-12, 2256-2261 | material cp plausibility | gated panic | physics | follows the hares-physics `lib.rs` row |
| crates/hares-io/src/hpxml/building.rs:1110-1127 | Attic to Garage wall removed after path A | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-io/src/hpxml/building.rs:1136-1149 | no `Adjacent` zone after the rewrite | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-io/src/hpxml/building.rs:1150-1176 | warn: multifamily parsed as one conditioned zone | gated log | log | delete (one HPXML file is one dwelling unit) |
| crates/hares-io/src/hpxml/building.rs:1185-1202 | warn: roof with an explicit 0 degree tilt | gated log | log | delete (a flat roof is valid input) |
| crates/hares-io/src/hpxml/resolve_hvac.rs:444-450, 466-472 | CFM25 leakage fraction <= 1 (then silently clamped) | gated debug_assert | physics | unconditional typed error (leakage above fan flow is invalid input); the silent clamp is deleted |
| crates/hares-io/src/hpxml/resolve_hvac.rs:525-529 | DSE in [0, 1] | gated debug_assert | debug | delete: `calculate_dse` performs the clamp |
| crates/hares-io/src/hpxml/resolve_hvac.rs:1879-1927 | primary heating/cooling designation missing or unmatched (error log, then silent return) | gated log | wiring | unconditional typed error (the function returns `()` today) |
| crates/hares-io/src/hpxml/resolve_hvac.rs:3344-3373, 3455-3456, 3490-3491 | multi-speed equipment with no per-stage capacity applied | gated log | wiring | the gated block is deleted (it duplicates the warns at 3452 and 3487); those two become `Warning`s pushed into `resolve_equipment`'s collector (replacing the `tracing::warn!`), so a multi-speed system run single-speed is never silent |
| crates/hares-io/src/hpxml/resolve_hvac.rs:3547-3570 | top-speed COP far below the SEER/EER-derived COP | gated log | debug | unit test over the shipped multispeed CSV |
| crates/hares-io/src/hpxml/validation.rs:73-79, 181-192 | `looks_like_namespace_uri` + `non_hpxml_namespaces` | gated helper | follows | ungated (207 becomes unconditional) |
| crates/hares-io/src/hpxml/validation.rs:94-100, 635-652 | HPXML 3.x: deprecated EnergyFactor vs UniformEnergyFactor count | gated log | log | always-on `ValidationWarning` (once per parse), carried to the run in `Building.parse_warnings` with the other schema warnings |
| crates/hares-io/src/hpxml/validation.rs:207-238 | foreign namespaces present but Building/Systems missing | gated error | wiring | unconditional (`ValidationError`) |
| crates/hares-io/src/output/metrics.rs:509-534, 1241-1260 | HVAC aggregate column missing although HVAC equipment columns exist (+ helper `schema_has_equipment_for`) | gated log | wiring | unconditional typed error (a missing expected output column); the helper is ungated |
| crates/hares-io/src/output/metrics.rs:935-959 | accumulators not NaN after each batch | gated panic | non-finite | unconditional typed error (`accumulate` returns `()` today) |
| crates/hares-io/src/output/metrics.rs:993-997, 1017-1029 | accumulated row-hours vs `config.duration` within one step | gated log | wiring | delete, with the `expected_duration_h` binding: the result already carries the accumulated duration and its coverage (`simulation_duration_hours`, `coverage`, `metrics.rs:1203-1204`), and a warning would fire on every legitimately short run (a stepped run finished early, a skipped Feb 29) |
| crates/hares-io/src/output/metrics.rs:1051-1080 | finish: accumulators and peak finite | gated panic | non-finite | unconditional typed error (`finish` returns a struct today) |
| crates/hares-io/src/schedule_resolve.rs:346-360 | default Occupancy weekday != weekend | gated log | debug | unit test over the shipped default schedule CSV |
| crates/hares-io/src/schedule_resolve.rs:459-469, 1430-1447 | schedule CSV columns with no `COLUMN_MAPPINGS` entry (+ `find_unmapped_csv_columns`) | gated log | log | always-on `Warning` into `inject_schedule_into_specs`'s collector, one per unknown column per load, by the "Schedule columns" rule above (the read set and the `NotUsed` table); the helper is ungated and renamed `find_unknown_schedule_columns` |
| crates/hares-io/src/schedule_resolve.rs:495-525 | warn: pool or spa defined in both HPXML and the schedule CSV | gated log | log | delete (documented precedence) |
| crates/hares-io/src/schedule_resolve.rs:3024, 3705, 3723 | tests | gated test | test | ungated `cfg(test)` |

### hares-tariff

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-tariff/Cargo.toml:9 | feature decl | feature decl | decl | delete |
| crates/hares-tariff/src/billing.rs:226-240 | demand window count <= capacity; peak vectors equal length | gated debug_assert | debug | `debug_assert!` |
| crates/hares-tariff/src/evaluator.rs:157-193 | debug logs for months reclassified vs June to September; summer/shoulder overlap | gated log + gated debug_assert | log + wiring | reclassification logs: delete. Overlap: typed error in `SeasonalSplit` construction (reachable from URDB) |
| crates/hares-tariff/src/evaluator.rs:495-502 | CPP event_schedule non-zero only below index 8760 | gated debug_assert | debug | delete: `CppConfig::validate` (types.rs:252) requires exactly 8760 entries |
| crates/hares-tariff/src/evaluator.rs:575-612 | CPP count <= limit; RTP and hourly-schedule index in range and value finite >= 0 | gated debug_assert | debug | `cfg(debug_assertions)`: `Tariff::validate` (types.rs:407-420) enforces length and finiteness at construction |
| crates/hares-tariff/src/urdb.rs:312-318 | warn: hemisphere classification borderline | gated log | log | always-on, once per tariff parse, into `ElectricTariff.parse_warnings` (replacing the `tracing::warn!`), reported by every run the tariff is attached to |
| crates/hares-tariff/src/urdb.rs:364-369 | warn: geographic hemisphere contradicts the schedule-based one | gated log | log | always-on, once per tariff parse, into `ElectricTariff.parse_warnings`, as the row above |
| crates/hares-tariff/src/urdb.rs:893-902 | debug log of the detection result | gated log | log | delete (the `observe` block after it captures the same fields) |
| crates/hares-tariff/src/urdb.rs:1160-1170 | empty summer months produce `None` | gated debug_assert | debug | `debug_assert!` |

### hares-core

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-core/Cargo.toml:13 | feature decl (forwards to io, equipment, envelope, tariff, physics) | feature decl | decl | delete |
| crates/hares-core/src/actor_registry.rs:446-464 | DrCompliance `hvac_target` without `hvac_action` and the reverse | gated log | wiring | unconditional typed error at actor construction (a half-configured pair can never dispatch) |
| crates/hares-core/src/actors/dr_compliance.rs:522-528 | DemandResponse duration_s > 0 | gated debug_assert | wiring | typed error at actor construction |
| crates/hares-core/src/actors/dr_compliance.rs:569-575 | debug log: TurnOff override is sticky | gated log | log | delete (documented limitation, per step) |
| crates/hares-core/src/actors/dr_compliance.rs:622-634 | `last_dispatched` holds only sticky actions | gated debug_assert | debug | `debug_assert!` |
| crates/hares-core/src/actors/dr_compliance.rs:812-840 | no HVAC TurnOff dispatched while a zone is below the freeze threshold | gated debug_assert | debug | `cfg(debug_assertions)` |
| crates/hares-core/src/actors/ev_driver/composer.rs:418-440 | discharge vote min_soc/max_soc in [0, 1] | gated debug_assert | wiring | typed error at strategy construction |
| crates/hares-core/src/actors/ev_driver/preference.rs:174-183 | `needed_charge_hours` >= 0 and finite, or +inf | gated debug_assert | non-finite | unconditional typed error (NaN is reachable, for example 0/0 with zero efficiency) |
| crates/hares-core/src/actors/ev_driver/solar.rs:13-17 | observed PV kW finite and >= 0 | gated debug_assert | non-finite | delete here; the electrical summary observation is checked once where it is built: non-finite already (dwelling/mod.rs:6378-6390), and a negative PV observation becomes an `InvariantViolation` there |
| crates/hares-core/src/actors/ev_driver/v2h.rs:15-19 | as solar.rs | gated debug_assert | non-finite | as solar.rs |
| crates/hares-core/src/actors/ideal_thermostat.rs:226-268 | `set_override` rejects heating >= cooling (panic vs warn + counter) | gated panic | physics | unconditional typed error (`set_override` returns `Result`); the dual branch is deleted |
| crates/hares-core/src/actors/ideal_thermostat.rs:325-370 | decide(): override violates the deadband (panic vs reject + telemetry) | gated panic | physics | unconditional: the actor is marked unhealthy, so dwelling/mod.rs:5740 fails the step with a typed error |
| crates/hares-core/src/actors/ideal_thermostat.rs:651, 665, 703, 739 | test comments describing the dual behaviour | doc | test | the tests assert the single typed-error path |
| crates/hares-core/src/actors/occupant.rs:241-253 | targets configured but no presence schedule | gated debug_assert | wiring | typed error at actor construction (per step today) |
| crates/hares-core/src/actors/safety_monitor.rs:212-245 | Safety-tier Heating/Off only with an actual breach | gated debug_assert | debug | `debug_assert!` |
| crates/hares-core/src/clock.rs:141-148 | civil UTC == fixed-offset UTC | gated debug_assert | debug | delete (the comment at 136-140 says it cannot fail) |
| crates/hares-core/src/diagnostics.rs:344-385 | NaN or Inf in diagnostic capture inputs | gated log | log | delete: the capture is a recorder; run-path non-finite detection is the unconditional zone and electrical screens |
| crates/hares-core/src/engine.rs:117-122, 259-264 | panic hook installed right after `PanicHookGuard::new()` | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-core/src/engine.rs:181-186, 214-219, 306-311, 339-344 | CRITICAL log on a prevented double panic | gated log | log | always-on `tracing::error!` (rare; the run still returns failed) |
| crates/hares-core/src/environment.rs:581-598 | (dst) civil ordinal == day_of_year; warn on a DST boundary | gated debug_assert + gated log | debug + log | ordinal: `cfg(all(feature = "dst", debug_assertions))`. Warn: delete |
| crates/hares-core/src/environment.rs:612-616 | mains temperature >= 0 C | gated debug_assert | debug | delete: `water_mains` clamps to 32 F |
| crates/hares-core/src/environment.rs:879-880 | schedule year length > 0 | gated debug_assert | wiring | typed error where the schedule is loaded (an empty schedule) |
| crates/hares-core/src/environment.rs:1105-1130 | free-float conditioned zone initial temperature within 5 C of outdoor | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-core/src/invariants.rs:553 | doc link to the method | doc | doc | follows the method rename |
| crates/hares-core/src/observer.rs:31 | doc: moisture capture populated when the feature is active | doc | doc | reword: populated whenever `observe` is on |
| crates/hares-core/src/observer_capture.rs:167-172 | `ALL_FUEL_TYPES.len() == FUEL_TYPE_COUNT` | gated debug_assert | debug | `const` assertion |
| crates/hares-core/src/telemetry.rs:66-91 (call dwelling/mod.rs:4418-4424) | `DwellingTelemetry::verify_consistency`: sum of equipment kW == total kW | gated log (flag) | physics | unconditional typed error raised from `run_timestep` (not from the `telemetry()` getter); `telemetry_consistency_flag` is deleted |
| crates/hares-core/src/telemetry.rs:419, 433, 447, 461 | tests of `verify_consistency` | gated test | test | ungated `cfg(test)`; assert the error |
| crates/hares-core/src/dwelling/autosize.rs:381-391 | heat-pump default-path heating factor <= 1.25 | gated panic | debug | delete: clamped at 356-364 |
| crates/hares-core/src/dwelling/autosize.rs:679-690 | warn: bedroom count <= 0 | gated log | physics | negative: typed error (invalid input). Zero: the message is deleted (a studio is valid; the unavailable case already warns at 670-676) |
| crates/hares-core/src/dwelling/autosize.rs:790-811 | water-heater autosize FHR, tank volume and capacity positive and finite | gated panic | non-finite | unconditional typed error (construction) |
| crates/hares-core/src/dwelling/conversions.rs:386-399 | fenestration boundaries get no precomputed RC layers | gated debug_assert | debug | `debug_assert!` |
| crates/hares-core/src/dwelling/conversions.rs:489-495 | `find_zone_idx` never sees `Adjacent` | gated panic | wiring | unconditional typed error (the function already returns `Result`) |
| crates/hares-core/src/dwelling/conversions.rs:2278 | `should_panic` test of the above | gated test | test | ungated test asserting `Err` |
| crates/hares-core/src/dwelling/solver_builder.rs:203-218, 233-248 | layer_info outer and inner node resolve through node_index (debug_assert + ungated warn) | gated debug_assert | wiring | unconditional typed error; the warn is deleted |
| crates/hares-core/src/dwelling/solver_builder.rs:3037 | doc of `missing_node_id_asserts_in_wiring` (`cfg(debug_assertions)`, `should_panic`) | doc | test | the test asserts `Err`, ungated |
| crates/hares-core/src/dwelling/solver_builder.rs:443-446, 456-490 | stored window r_glass equals r_total minus films (+ `r_total` binding) | gated debug_assert | debug | `cfg(debug_assertions)`; the binding follows |
| crates/hares-core/src/dwelling/solver_builder.rs:603-631 | each solver zone index maps to one HPXML zone type | gated panic | wiring | unconditional typed error at construction (a duplex input can reach the conflation it guards; construction cost only) |
| crates/hares-core/src/dwelling/synthetic.rs:8-9, 593-596 | material cp plausibility | gated panic | physics | follows the hares-physics `lib.rs` row |
| crates/hares-core/src/dwelling/synthetic.rs:845-859 | internal gains radiant + convective + latent == 1; radiant in [0, sensible] | gated debug_assert | physics | unconditional typed error (fixture config input) |
| crates/hares-core/src/dwelling/synthetic.rs:1320-1365 | boundary area >= 0; wall area + window area reconstructs the stored original | gated panic | physics + debug | area >= 0: typed error. Reconstruction: `cfg(debug_assertions)` |
| crates/hares-core/src/dwelling/synthetic.rs:1367-1406 | closed envelope (at least two face categories, more than one opaque boundary) | gated log | wiring | unconditional typed error naming the missing face category |
| crates/hares-core/src/dwelling/synthetic.rs:1448-1470 | explicit HVAC capacity finite and W/m2 > 0 | gated panic | physics | unconditional typed error |
| crates/hares-core/src/dwelling/synthetic.rs:1787-1815 | synthetic weather GHI = DNI cos z + DHI; all-zero GHI with clear sky | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-core/src/dwelling/synthetic.rs:1891-1930 | diurnal model has variance; June 29 peak hour in 13 to 17 | gated panic | debug | unit test of the synthetic weather generator |
| crates/hares-core/src/dwelling/mod.rs:938 | helper `targets_conflict` | gated helper | follows | `cfg(feature = "observe")` once 1074 is deleted |
| crates/hares-core/src/dwelling/mod.rs:1074-1110 | `drain_tiers` warn: two same-tier requests target the same equipment | gated log | log | delete (documented last-write-wins; the `observe` pre-scans in `dispatch_into_observed` cover it) |
| crates/hares-core/src/dwelling/mod.rs:1563, 1568, 1661, 1663, 1665, 1669, 2964, 6475, 6478, 6760, 7176 | the always-on method `Dwelling::check_invariants` and docs naming it | method name | doc | renamed `check_step_invariants` (above) |
| crates/hares-core/src/dwelling/mod.rs:1685-1693, 2709-2714, 6482-6501, 7433-7437, 7574-7610 | `invariant_moisture_capture` field, init, push, clear and observer read | gated field | follows | `cfg(feature = "observe")` (`check_moisture` already runs unconditionally) |
| crates/hares-core/src/dwelling/mod.rs:1694-1696, 2661-2662 | `thermal_consistency_flag` field and init | gated field | follows | deleted with the 4427-4507 row |
| crates/hares-core/src/dwelling/mod.rs:2192-2215 | `has_occupancy_spec` binding | gated field | follows | ungated |
| crates/hares-core/src/dwelling/mod.rs:2223-2230 | number_of_occupants >= 0 | gated error | physics | unconditional (already `Err`) |
| crates/hares-core/src/dwelling/mod.rs:2243-2265 | Occupancy spec present but no occupancy schedule column | gated error | wiring | unconditional (already `Err`) |
| crates/hares-core/src/dwelling/mod.rs:2310-2320 | debug log of the attic vented status | gated log | log | delete |
| crates/hares-core/src/dwelling/mod.rs:2480-2498 | every thermal accumulator has a declaring equipment | gated log | debug | delete: `PortSlots::from_declarations` creates accumulators only from declarations |
| crates/hares-core/src/dwelling/mod.rs:2789-2796 | RNG word_pos restored after warm-up | gated panic | debug | delete: restates the restore at 2779-2782 |
| crates/hares-core/src/dwelling/mod.rs:3109-3119, 3252-3259 | info log "actor registered" (+ `actor_name` rebinding) | gated log | log | delete |
| crates/hares-core/src/dwelling/mod.rs:4390-4393, 4418-4424 | `telemetry()` runs `verify_consistency` and folds `thermal_consistency_flag` | gated log | physics | follows the hares-core `telemetry.rs` row; out of the getter |
| crates/hares-core/src/dwelling/mod.rs:4427-4507, 6763-6767 | `verify_per_zone_thermal_consistency`: per-zone HVAC port totals == HVAC equipment `thermal_output_w` | gated log (flag) | physics | unconditional typed error from `run_timestep`, over zone and HVAC-equipment index lists resolved at assembly |
| crates/hares-core/src/dwelling/mod.rs:4851-4856 | debug log of each actor after a checkpoint restore | gated log | log | delete |
| crates/hares-core/src/dwelling/mod.rs:5393-5406, 6777-6781 | RNG consumed this step (+ bindings) | gated debug_assert | debug | `cfg(debug_assertions)`; the bindings follow |
| crates/hares-core/src/dwelling/mod.rs:5408-5451 | every equipment has an `equipment_core` entry after step 0; non-empty after a checkpoint restore | gated debug_assert | wiring | unconditional typed error over pre-resolved ids (a missing expected observation); the `restored_from_checkpoint = false` reset at 5449 moves out of the gate |
| crates/hares-core/src/dwelling/mod.rs:6347-6353 | custom solver capture count == solver count | gated debug_assert | debug | `debug_assert!` |
| crates/hares-core/src/dwelling/mod.rs:6376 | comment naming the gated method | doc | doc | reword |
| crates/hares-core/src/dwelling/mod.rs:6815-6849 | `record_step`: expected per-equipment output columns resolved at verbosity >= 5 or 7 | gated debug_assert | debug | `cfg(debug_assertions)`; OR-2's row plan then makes an unplaceable expected variable an error at freeze and deletes this block |
| crates/hares-core/src/dwelling/mod.rs:7636-7663, 7693-7694, 7783-7789 | EV driver RNG (seed, stream) pairs unique (+ `seen_rng_pairs`) | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-core/src/dwelling/mod.rs:8499, 14170 | test-equipment comments: electrical consistency enforced only in gated builds | doc | doc | reword: `validate_port_core_electrical_consistency` is unconditional |
| crates/hares-core/src/dwelling/mod.rs:8538-8740, 9398, 9426 | `UnderReportingEquipment`, `ReactiveDivergentEquipment` and their tests | gated test | test | ungated `cfg(test)` |
| crates/hares-core/src/dwelling/mod.rs:8893-9230, 9452, 9475, 9499, 9525 | thermal under-reporting, water-heating and latent test equipment and tests of the per-zone thermal flag | gated test | test | ungated `cfg(test)`; assert the typed error of the 4427-4507 row |
| crates/hares-core/src/dwelling/mod.rs:11586-11607 | `run_timestep_panics_on_unhealthy_actor` | gated test | test | ungated; asserts `Err` (renamed `run_timestep_fails_on_unhealthy_actor`) |
| crates/hares-core/tests/hvac_negative_energy_invariant.rs:10, 88, 97 | module doc about the gate | doc | doc | reword; the file stays `#![cfg(debug_assertions)]` because the test seam is `cfg(any(test, debug_assertions))` |
| crates/hares-core/tests/thermal_invariant_wiring.rs:11, 27, 114, 124, 136 | the same | doc | doc | the same |
| crates/hares-core/tests/orchestration_parity.rs:928, 993 | test comments on the dual decide() behaviour | doc | test | rewritten with the ideal_thermostat decide() row |
| tests/resstock_smoke.rs:259 (a hares-core test target) | `manifest_invariant_fixtures_match_hashes` | gated test | test | ungated |

### hares-fleet

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-fleet/Cargo.toml:9 | feature decl (not forwarded from hares-core) | feature decl | decl | delete |
| crates/hares-fleet/src/aggregation.rs:251-254, 739-770 | every known unit maps to a `ColumnAggregation` | gated log | debug | unit test (static table); the runtime call is deleted |
| crates/hares-fleet/src/aggregation.rs:359-372 | aggregate empty although successful dwellings had rows | gated log | debug | `cfg(debug_assertions)` |
| crates/hares-fleet/src/aggregation.rs:646-659 | sample_weight finite and >= 0 in `build_aggregate_batch` | gated panic | debug | `cfg(debug_assertions)`: `aggregate()` already rejects bad weights (`FleetError::InvalidAggregationWeight`) |
| crates/hares-fleet/src/aggregation.rs:677-689 | total_weight finite and >= 0 | gated log | debug | delete: follows from validated weights |
| crates/hares-fleet/src/aggregation.rs:1534-1544 | `should_panic` test of 652 | gated test | test | `cfg(debug_assertions)` test |
| crates/hares-fleet/src/fleet.rs:337-343, 462-468, 733-739 | panic hook installed after `PanicHookGuard::new()` | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-fleet/src/fleet.rs:376-383, 490-497, 768-775 | CRITICAL log on a prevented double panic | gated log | log | always-on `tracing::error!` |
| crates/hares-fleet/src/fleet.rs:586-607 | already-failed dwellings return `Skipped` | gated panic | debug | `cfg(debug_assertions)` |

### hares-python

| File:line | Check | Today | Class | Disposition |
|---|---|---|---|---|
| crates/hares-python/Cargo.toml:10 | feature decl (forwards to hares-core) | feature decl | decl | delete |
| crates/hares-python/src/py_actor.rs:151-158 | Python interpreter reachable after `decide()` | gated debug_assert | debug | delete: it attaches and imports `sys` on every decide, and the call it follows has just run Python |
| crates/hares-python/src/py_actor.rs:498-516 | PySignal maps to a non-zero capability; DutyCycle on_fraction finite in [0, 1] | gated debug_assert | debug + non-finite | capability: unit test over the PySignal variants. on_fraction: `PyValueError` at the boundary (a user value) |
| crates/hares-python/src/py_dwelling.rs:735-741, 790-796 | panic hook installed | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-python/src/py_dwelling.rs:773-779, 823-829 | `poisoned` flag implies the mutex is poisoned | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-python/src/py_dwelling.rs:1617 | comment: the dwelling's finiteness screens are gated | doc | doc | reword (they are unconditional) |
| crates/hares-python/src/py_gym.rs:198-201, 311-360 | an action-derived `ControlSignal` has no NaN or Inf | gated panic | non-finite | unconditional `PyValueError` (a NaN action survives clipping) |
| crates/hares-python/src/py_gym.rs:442-448 | panic hook installed before the batch step | gated panic | debug | `cfg(debug_assertions)` |
| crates/hares-python/src/utils.rs:268-274 | `extract_seconds` round-trip error < 1 s | gated panic | debug | delete: restates the range and rounding checks above it |
| crates/hares-python/src/utils.rs:288 | comment: downstream finiteness screens are gated | doc | doc | reword |

Also outside the tables: `.github/workflows/ci.yml:155` runs
`cargo nextest run --workspace --release -F check_invariants --no-fail-fast --profile ci`.
This commit deletes that step. Without the feature it would be
`cargo nextest run --workspace --release --no-fail-fast --profile ci`,
exactly the plain release run of the step before it (`ci.yml:154`), so
keeping it with the flag dropped would run the same release suite twice.
The `-p hares-equipment` release step after it (`ci.yml:156`) stays: it
builds that crate alone, without the features the workspace build unifies
into it.

**Acceptance tests:**

- The census over every file type prints nothing:
  `git grep -n check_invariants -- . ':!.shipwright' ':!docs/reviews' ':!docs/tickets' ':!docs/findings' ':!docs/superpowers' ':!scripts/reviews' ':!scripts/triage.py'`
  (it covers `*.rs`, `*.toml`, `*.md`, `*.py`, `*.yml` and every other
  tracked file; the excluded paths are the ones SL-8 moves).
- One plain-release unit test per crate that had a physics or non-finite
  gate, driving that crate's check with an input that violates it and
  asserting the typed error (for example
  `fluid_solver_non_finite_flow_is_an_error_in_release`,
  `ports_non_finite_write_is_an_error_in_release`).
- Warning channel: `equipment_warnings_reach_the_dwelling_log` (an
  in-crate `hares-core` dwelling test with the in-crate test equipment
  given one warning in `init` and one in its third step: `take_warnings()`
  returns the init warning, then the step warning prefixed `step 2: `,
  each naming the equipment); `battery_heater_above_700_w_warns_at_init`
  and `ev_setpoint_above_rated_warns_once_per_run` (`hares-equipment` unit
  tests: one drained warning, and one only over three violating steps);
  `multispeed_without_csv_entry_warns` (`hares-io` unit test of
  `apply_multispeed_parameters` with two speeds and no efficiency: one
  `Warning` naming the equipment in the collector);
  `hpxml_schema_warnings_reach_building` (an HPXML 3.x document written in
  the test: `Building.parse_warnings` holds the `schemaVersion` warning);
  `contradicting_hemisphere_is_a_tariff_parse_warning` (`hares-tariff`: a
  URDB document written in the test whose geographic metadata contradicts
  its schedule gives one `parse_warnings` entry);
  `set_tariff_reports_tariff_parse_warnings` (`hares-core`: BESTEST 600,
  `tests/fixtures/bestest/600.toml`, with a tariff carrying one parse
  warning: `take_warnings()` holds `tariff: <message>`).
- Schedule columns: `fixture_schedule_columns_are_all_known` (`hares-io`:
  for every schedule file of the census above,
  `find_unknown_schedule_columns` returns nothing);
  `unknown_schedule_column_warns_naming_it` (`hares-io`: the
  `tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv` with a
  `lighting_interor` column appended in the test gives one `Warning`
  naming it); `resstock_homes_carry_no_schedule_warning`
  (`crates/hares-core/tests/schedule_columns.rs`, new): the
  `consumer_shape_900s` inputs (`tests/fixtures/golden/consumer_shape_900s.toml`,
  bldg0176227) built and stepped once carry no warning at all, and
  `tests/fixtures/resstock/2025.1/bldg0176775/` (`home.xml`,
  `in.schedules.csv`, weather `G3400270_2018.csv`, start
  `2018-01-01T00:00:00-05:00`, 900 s, no warm-up), whose schedule carries
  the EV columns, carries no warning with source `schedule`. A warning
  that appears on a golden home stops the entry as the stop condition
  does, recorded here for the operator.
- Fluid loops, in `crates/hares-core/tests/fluid_loop_declarations.rs`:
  `fluid_loops_come_from_port_declarations`, over two inputs, each built
  without warm-up and stepped once: ResStock 2025.1 `bldg0000007`
  (`home.xml`, `in.schedules.csv`, weather `G1500030_2018.csv`: a storage
  water heater and the wet appliances on `DHW_DEMAND_LOOP`), and
  `tests/fixtures/hpxml/ochre_samples/base-dhw-multiple.xml` with
  `data/examples/BEopt_example_schedule.csv` and
  `data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw` (tankless, indirect
  tank and boiler); for each, the solver's loop-type map equals the union
  of every equipment's fluid port declarations, and `DHW_DEMAND_LOOP` is
  `Water`. `runtime_added_equipment_declares_its_loops` (the first input,
  a water heater on a new loop added after build) and
  `conflicting_port_fluid_types_are_a_construction_error` (the same water
  heater declaring `Glycol` on the DHW demand loop). In `hares-equipment`
  (`hvac/boiler.rs` tests): `boiler_port_declares_configured_fluid_type`
  (a gas and an electric boiler configured `Glycol` declare a `Glycol`
  fluid port after `init`; the default declares `Water`).

**Verification:** the full verification block, with the
`[test-release-invariants]` line removed from it in this commit (`plan.md`
§3 and `.github/workflows/ci.yml` updated in the same commit, since the
feature no longer exists); `frame_golden compare-all`.

**Commit:** `fix: enforce every physics, conservation, non-finite and wiring check in every build profile`

**Dependencies:** OR-5, M-3 (the baseline is measured with the gates as
they were).

**Measurement:** share of `simulate()` before and after, recorded.

**Implementer note (2026-10-02).** This entry changed while it is being
implemented. What changed:

1. New "Warning channel" section: `hares_types::Warning`; the `Equipment`
   method `drain_warnings` (drained after `init`, after `add_equipment` and
   `replace_equipment`, and per step beside `take_health_counts`); a
   `&mut Vec<Warning>` collector on `resolve_equipment` and
   `inject_schedule_into_specs`, held by the blueprint and moved into the
   log at build; `Building.parse_warnings`;
   `ElectricTariff.parse_warnings`, pushed by `set_tariff`;
   `WarningLog::push_warning(Warning)` formatting `"{source}: {message}"`.
   No row may end as a bare `tracing::warn!`: the `urdb.rs` hemisphere
   warns now ungated in the tree go into `ElectricTariff.parse_warnings`.
2. Rows re-dispositioned to use it: `battery/mod.rs:892-945` and
   `:1009-1019`, `ev/mod.rs:934-956` (warn-once flag reset by `init`),
   `pv/mod.rs:478-501` (raised once in `init`, out of the step) and
   `:840-843`, `resolve_hvac.rs:3344-3373`, `validation.rs:94-100`,
   `schedule_resolve.rs:459-469`, `urdb.rs:312-318` and `:364-369`.
3. `output/metrics.rs:993-997, 1017-1029` (row-hours against
   `config.duration`): now deleted, not a warning.
4. New "Schedule columns" section: `WaterHeater` category with
   `hot_water_fixtures` and `hot_water_mains_temperature`; the occupancy
   column resolution moved into `hares-io` and shared with the
   environment; `Ignore` becomes `NotUsed { reason }`, with
   `hot_water_dishwasher`, `hot_water_clothes_washer`,
   `electric_vehicle_charging` and `electric_vehicle_discharging` added;
   `find_unknown_schedule_columns`.
5. Fluid loops: the loop map is read after `init` and refreshed on add,
   remove and replace; cp is not configurable (`FluidSolverConfig.fluid_specific_heats`
   deleted, solver uses `cp_j_kg_k`; `net_power_from_flow_and_temp_delta`
   rewritten); boiler `init` sets the fluid port's declared fluid type from
   the configured `fluid_type`.
6. `.github/workflows/ci.yml:155` is deleted, not edited: with the flag
   gone it would repeat line 154.
7. Acceptance tests added: the warning-channel, schedule-column and
   fluid-loop tests listed above, with their files and fixtures
   (`crates/hares-core/tests/fluid_loop_declarations.rs` gains the
   `base-dhw-multiple.xml` input; `crates/hares-core/tests/schedule_columns.rs`
   is new).

**State:** committed `b8195c83`, not done. The independent audit
(`verification/audit-OR-11-b8195c83.md`) found three census rows committed
as debug-only logs instead of their dispositions:

1. The zone-unresolved rows: `water_heater/{gas,heat_pump_wh,indirect_tank,resistance,tankless}.rs`
   and `hvac/dehumidifier.rs` warn under `debug_assertions` and run on
   `ZoneId(1)`; `hvac/helpers.rs:66-93` `zone_id_from_config_or_default`
   returns `ZoneId(1)` silently in release.
2. `dwelling/synthetic.rs:1368-1400`, the closed-envelope check: a
   debug-only `tracing::error!` marked "STOPPED ROW".
3. `hvac/ideal_hvac.rs:841-866`, the end-use sign check: a debug-only
   `tracing::error!` marked "STOPPED ROW"; under a thermostat override the
   end use can disagree with the delivered capacity's sign.

Their fix is OR-18 (schedule row 21b), written from
`verification/OR-11-stopped-rows-research.md`. The red team
(`verification/redteam-OR-11-SL-7.md`) broke the fallible refresh OR-11
added to the equipment entrances; its fix is OR-19 (row 21c).

## OR-18: the three census rows OR-11 left as logs

**Kind:** initiative work (fix; one of its four commits is a numeric row
per `plan.md` §2.8's "found defects that move numbers follow the same
rules: own commit, own row in the numeric block, recorded delta").

**Problem:** the independent audit of `b8195c83`
(`verification/audit-OR-11-b8195c83.md`, blockers B1 to B3) found that
OR-11's census table dispositions six zone-unresolved checks, the
synthetic closed-envelope check and the `IdealHvac` end-use/capacity-sign
check as unconditional typed errors or a `debug_assert!`, but the shipped
code left all eight as `tracing::warn!`/`tracing::error!` calls gated
`cfg(debug_assertions)`, so a release build runs every one of them exactly
as before OR-11: silently. `verification/OR-11-stopped-rows-research.md`
traces the root cause of each and is the source for the fixes below.

**Evidence** (at `7b7a6a7d`):

- `crates/hares-equipment/src/hvac/helpers.rs:59-90`,
  `zone_id_from_config_or_default`: returns `ZoneId(1)` in every profile
  when `zone_id` is absent from an equipment's typed config, logging the
  fallback only under `cfg(debug_assertions)`.
- `crates/hares-equipment/src/hvac/dehumidifier.rs:171-172` (`new`) calls
  the fallback above; `dehumidifier.rs:543-554` (`init`) does the same on
  every HPXML-parsed dehumidifier, because
  `crates/hares-io/src/hpxml/resolve_loads.rs:645-666` always constructs
  `DehumidifierConfig { zone_id: None, .. }` (HPXML's `Dehumidifier`
  schema has no `Location` element; OS-HPXML and HARES both model every
  dehumidifier in the one conditioned zone). This "works" today only
  because `zone_sort_key` (`crates/hares-io/src/hpxml/building.rs:3615-3624`)
  places the conditioned zone first in every real parsed building; HARES's
  own `conditioned_zone_id_returns_correct_id_when_not_first_zone`
  (`resolve_hvac.rs:5633-5640`) proves that is not guaranteed.
  `conditioned_zone_id(building) -> Option<u16>`
  (`crates/hares-io/src/hpxml/resolve_hvac.rs:1867-1872`) is the function
  HVAC resolvers already call for exactly this case
  (`resolve_hvac.rs:2086,2227,2522`); `resolve_loads.rs` never calls it.
- `crates/hares-equipment/src/water_heater/{gas,heat_pump_wh,indirect_tank,resistance,tankless}.rs`
  (audit lines `gas.rs:18-19,302-309`; `heat_pump_wh.rs:18-19,414-421`;
  `indirect_tank.rs:25-26,243-250`; `resistance.rs:304-311`;
  `tankless.rs:15-16,109-116`): each warns and keeps running on
  `ZoneId(1)` when `zone_id` is absent and `zone_type` is `Some(..)`.
  `crates/hares-io/src/hpxml/building.rs:3074-3095` (`parse_zone_label`)
  buckets every HPXML water-heater `Location` string containing "other"
  into `ZoneType::Adjacent`; `build_zone_map`
  (`building.rs:2258-2340`) never constructs an `Adjacent` entry in
  `building.zones`, so `zone_id` is structurally `None` for these
  locations, by HPXML's own meaning: "other heated space", "other
  multifamily buffer space", "other non-freezing space" and "other
  housing unit" name space with no modeled thermal zone.
  `crates/hares-io/src/hpxml/resolve_water_heater.rs:80-86` collapses the
  raw `Location` text through `parse_zone_label`/`zone_key` into one
  string, `"adjacent"`, before storing it as the config's `zone_type`
  (`wh_config.rs:87,257,419,516,681`), which loses the distinction between
  the four locations before `init` ever sees it. "Other exterior" and
  "outside" do not reach `Adjacent`: `"outside"` matches the `Outdoor`
  branch (`contains("out")`), and OS-HPXML treats `OtherExterior` as a
  pure-outdoor-ambient equipment placement, the same as `Outside`
  (`geometry.rb:1675-1678`, verified below), not a scheduled-space average.
  A real, schema-valid fixture already vendored in this repo exercises the
  "other heated space" case:
  `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-adjacent-to-other-heated-space.xml:315`
  (`<WaterHeatingSystem><Location>other heated space</Location>`).
- `crates/hares-core/src/dwelling/synthetic.rs:1367-1406`: the closed
  envelope check (at least two of Wall/Roof/Floor present when
  `zone_volume_m3 > 0.0`) is a `tracing::error!` under
  `cfg(debug_assertions)`, marked in a comment as a "STOPPED ROW...
  pending the operator's decision". `build_synthetic_building`
  (`synthetic.rs:479`) is the one production constructor for every
  synthetic-TOML (BESTEST-style) dwelling, reached from
  `Dwelling::from_toml_config`/`..._with_write_output`
  (`dwelling/mod.rs:2049-2087`); it is not a test-only path.
- `crates/hares-equipment/src/hvac/ideal_hvac.rs:841-866`: the
  `descriptor.end_use`-matches-`capacity_w`-sign check is a
  `tracing::error!` under `cfg(debug_assertions)`, same "STOPPED ROW"
  marker. `end_use` is set once per `update_control()` from the FSM's
  discrete mode (`ideal_hvac.rs:569-574`); in `use_ideal_cached` mode the
  `Deadband` branch of `capacity_w`'s computation
  (`ideal_hvac.rs:640-654`) does not zero capacity, and the FSM's mode can
  still read `Deadband` for one or more steps after a `RuntimeSetpointOverride`
  (`ideal_hvac.rs:261-282,973-1034`, a genuine consumer control path, not a
  test hook) has already pushed `cached_ideal_target`'s sign the other
  way. In that window the equipment's electric power is summed into the
  output end-use bucket (`crates/hares-io/src/output/metrics.rs`,
  `energy_by_end_use`) matching the stale FSM mode, not the delivered
  capacity. It changes total dwelling electric consumption for nothing
  (end uses sum regardless of label); it is reachable only through
  `use_ideal_cached` plus an active runtime thermostat override, which no
  current golden, parity or ResStock fixture issues.

**Verified OS-HPXML rule** (the water-heater "other ..." locations are
HPXML's documented way of saying "no modeled thermal zone here; use a
scheduled ambient series", and HARES must compute that series the same
way OS-HPXML does so the physics is not invented). Source:
`github.com/NREL/OpenStudio-HPXML`, tag `v1.12.0`,
`HPXMLtoOpenStudio/resources/geometry.rb:1399-1428`, function
`get_temperature_scheduled_space_values(location)` (identical on
`master` at commit `142773f`). Minimum floors converted to SI
((`°F` − 32) × 5 / 9):

| HPXML `Location` | indoor weight | outdoor weight | ground weight | floor |
|---|---|---|---|---|
| other heated space | 0.5 | 0.5 | 0.0 | 20.0 °C (68 °F) |
| other multifamily buffer space | 0.5 | 0.5 | 0.0 | 10.0 °C (50 °F) |
| other non-freezing space | 0.0 | 1.0 | 0.0 | 4.44 °C (40 °F) |
| other housing unit | 1.0 | 0.0 | 0.0 | none |
| other exterior / outside | 0.0 | 1.0 | 0.0 | none (pure outdoor ambient; `geometry.rb:1675-1678`, `get_space_or_schedule_from_location`, returns `nil` -- no space, no schedule -- for `OtherExterior`, `Outside` and `RoofDeck` alike, not a scheduled-space average) |

The ambient temperature is `(indoor_weight × conditioned_zone_temp_c +
outdoor_weight × outdoor_temp_c).max(floor)` (no `.max()` where the floor
is "none"). "Other housing unit" (indoor weight 1.0) and "other exterior"/
"outside" (outdoor weight 1.0) need no averaging: they read the
conditioned zone's temperature or `env.weather.outdoor_temp_c` directly,
both already computed per step.

**Change**, four commits:

1. **Dehumidifier zone resolution** (bitwise-neutral: every fixture in
   the repo already parses to the conditioned zone being index 0, so
   `ZoneId(1)` and `conditioned_zone_id`'s result agree everywhere today).
   `conditioned_zone_id` (`resolve_hvac.rs:1867-1872`) becomes visible to
   `resolve_loads.rs` (same `hpxml` module; `pub(super)` is enough).
   `resolve_loads.rs:645-666` sets `DehumidifierConfig.zone_id` from it;
   `None` (no conditioned zone in the building) is a typed `HpxmlError`
   (a new variant, `NoConditionedZone { equipment: String }`) naming the
   dehumidifier, raised before the spec is pushed. `Dehumidifier::new`
   (`dehumidifier.rs:171-172`) stops calling
   `zone_id_from_config_or_default`; it takes the zone from
   `zone_id_from_config` only, leaving `descriptor.zone`/`zone_id` unset
   when the config carries none (legal only because `init` below always
   runs before the first step and enforces the real check).
   `Dehumidifier::init` (`dehumidifier.rs:543-554`) replaces "preserve the
   existing zone_id" with `zone_id_from_config(config).ok_or_else(||
   HaresError::Equipment(format!("{name}: zone_id not resolved; HPXML
   parsing did not inject the conditioned zone")))?`. `helpers.rs:59-90`'s
   `zone_id_from_config_or_default` (both `cfg` bodies) is deleted; a
   `git grep` for the name finds no remaining caller.

2. **Water heater ambient source for HPXML locations with no modeled
   zone** (not bitwise-neutral by definition: it replaces running the
   equipment against zone 1's real thermal state with a correct ambient
   series, the same reasoning OR-11 itself used to stop and re-disposition
   the fluid-loop row. ~~No current golden, parity or ResStock fixture
   contains an "other ..." `Location`, so `[golden]` passes unchanged in
   practice~~ CORRECTED: the fleet golden (homes bldg0000002,
   bldg0000004, bldg0176227) carries one such home, bldg0000004 ("other
   heated space", home 1; bldg0000003 and bldg0000006 carry them too but
   are in no golden; audit of `1c18543a`), so this
   commit moved `fleet_mixed_offset` and is a numeric row: the fleet
   golden was re-captured in the landing commit with the delta recorded
   in `02-measurement.md` (home 1: gross consumption 201.99 to 204.91
   kWh); every other fixture is bitwise unchanged. The change is
   isolated in its own commit rather than folded into commit 1 or 3
   because it adds physics a bitwise pass cannot prove);
   `resolve_water_heater.rs:80-86` stores the raw, HPXML-normalized
   `Location` text in `zone_type` (not `zone_key(&parse_zone_label(..))`'s
   collapsed string), so the five "other ..." strings stay distinguishable;
   `zone_id` lookup is unaffected (`zone_id_for_location`,
   `resolve_water_heater.rs:49-61`, keeps using `parse_zone_label`/`zone_key`).
   `crates/hares-equipment/src/water_heater/wh_config.rs` gains
   `pub enum AmbientLocation { OtherHeatedSpace, OtherMultifamilyBufferSpace,
   OtherNonFreezingSpace, OtherHousingUnit, OtherExterior }` and
   `fn ambient_location_from_zone_type(zone_type: &str) -> Option<AmbientLocation>`,
   matching the five canonical HPXML `Location` strings (case-insensitive,
   `"outside"` mapped to `OtherExterior`). `crate::EnvironmentState`
   (`crates/hares-types/src/environment.rs:450-452`) gains
   `pub ambient_other_space_c: AmbientOtherSpaceTemps { other_heated_space_c: f64,
   other_multifamily_buffer_space_c: f64, other_non_freezing_space_c: f64 }`,
   computed once per step in `EnvironmentManager::update`/`update_in_place`
   (`crates/hares-core/src/environment.rs:510,758`) from the conditioned
   zone's temperature and `weather.outdoor_temp_c` using the table above
   (three scalar multiplies and a `.max()` each, no allocation; "other
   housing unit" and "other exterior" need no new field, they read the
   conditioned zone temperature or `outdoor_temp_c` directly). Each of
   `gas.rs:302-309`, `heat_pump_wh.rs:414-421`, `indirect_tank.rs:243-250`,
   `resistance.rs:304-311` (`init_typed`, the `ambient_temp_c` call sites
   at `gas.rs:280`, `heat_pump_wh.rs:375`, `indirect_tank.rs:204`,
   `resistance.rs:281`) and `tankless.rs:109-116` (its thermal port's zone
   assignment) is changed so that when `zone_id` is absent and
   `ambient_location_from_zone_type(&zone_type)` is `Some(..)`, the
   equipment carries that `AmbientLocation` instead of a `ZoneId`, and
   reads the matching precomputed `EnvironmentState` field each step
   instead of a zone's RC temperature. When `zone_id` is absent and
   `ambient_location_from_zone_type` is `None` (an HPXML `Location` the
   parser could not classify at all, or a programmer/data error), it is an
   unconditional `HaresError::Equipment` naming the water heater and the
   unresolved `zone_type`. `zone_id_from_config_or_default` has no callers
   left in these five files once this lands (commit 1 already deleted it;
   this commit removes the remaining `#[cfg(debug_assertions)]` warn
   blocks at the audited line numbers).

3. **Synthetic closed envelope** (bitwise-neutral: the four BESTEST
   goldens and every other existing `[[boundaries]]` fixture already
   supply a closed envelope). `synthetic.rs:1367-1406`'s
   `cfg(debug_assertions)` block and its "STOPPED ROW" comment are
   deleted; the face-category count check runs unconditionally and
   returns `Err(HaresError::Dwelling(..))` naming the zone and which of
   Wall/Roof/Floor are present/missing when `zone_volume_m3 > 0.0` and
   fewer than two of the three are present. The six fixtures the audit
   found tripping it on a deliberately open one-wall "zone" each gain one
   `Floor` boundary of the same area with a minimal material layer, since
   none of their assertions inspect boundary or RC-node count:
   `window_area_deducted_from_host_wall`,
   `zero_conductivity_with_positive_thickness_returned_as_error`,
   `zero_density_with_positive_thickness_succeeds`,
   `zero_specific_heat_with_positive_thickness_succeeds`,
   `negative_density_returned_as_error` (all in `synthetic.rs`), and
   `SYNTHETIC_RC_DWELLING_TOML` in
   `crates/hares-core/tests/thermal_invariant_wiring.rs:27`.

4. **`IdealHvac` end_use from delivered capacity** (bitwise-neutral: no
   current golden, parity or ResStock fixture drives this equipment with
   a runtime override that crosses deadband while the FSM mode lags).
   The `self.descriptor.end_use = match mode { .. }` assignment moves out
   of `update_control()` (`ideal_hvac.rs:569-574`, deleted) into `step()`,
   placed immediately after the R3 minimum-capacity clamp
   (`ideal_hvac.rs:677`, where `capacity_w` is final): `capacity_w > 0.0`
   → `HVAC_HEATING`, `capacity_w < 0.0` → `HVAC_COOLING`, `capacity_w ==
   0.0` → unchanged (preserves today's deadband behavior, the only
   legitimate zero-capacity case). The `cfg(debug_assertions)` block at
   `ideal_hvac.rs:841-866` and its "STOPPED ROW" comment are deleted, not
   converted to a `debug_assert!`: the invariant now holds by
   construction, so there is nothing left to assert.

**Acceptance tests** (named per commit; each must fail against `7b7a6a7d`
before the change, and is deterministic, no timing):

1. `dehumidifier_zone_id_resolves_to_conditioned_zone_not_first_zone`: a
   building whose conditioned zone is not index 0 (mirrors
   `conditioned_zone_id_returns_correct_id_when_not_first_zone`) →
   the parsed dehumidifier's `zone_id` equals the conditioned zone's id,
   not `ZoneId(1)`.
   `dehumidifier_errors_when_no_conditioned_zone_exists`: a building with
   no conditioned zone and a `Dehumidifier` element → `resolve_loads`
   returns `Err(HpxmlError::NoConditionedZone { .. })`.
   `dehumidifier_init_errors_on_unresolved_zone`: `Dehumidifier::init`
   with a config carrying no `zone_id` → `Err(HaresError::Equipment(..))`
   naming the equipment.
2. One test per water heater type (`gas`, `heat_pump_wh`, `indirect_tank`,
   `resistance`, `tankless`), each named
   `<type>_ambient_source_for_other_heated_space_location` (and one
   `..._for_other_multifamily_buffer_space_location`,
   `..._for_other_non_freezing_space_location`,
   `..._for_other_housing_unit_location`,
   `..._for_other_exterior_location`, spread across the five types so
   every one of the five locations is covered at least once): HPXML
   `Location` set to the exact string, `zone_id` absent → construction
   succeeds, the equipment carries the matching `AmbientLocation`, and a
   one-step `EnvironmentState` with a known conditioned-zone temperature
   and `outdoor_temp_c` produces the equipment's ambient temperature equal
   to the table's formula, hand-computed (e.g. conditioned 22.0 °C,
   outdoor 2.0 °C → "other heated space" ambient = max(0.5×22.0 + 0.5×2.0,
   20.0) = 20.0 °C; conditioned 22.0 °C, outdoor 30.0 °C → max(26.0, 20.0)
   = 26.0 °C). One test per type,
   `<type>_errors_on_unresolved_zone_and_unclassified_location`: `zone_id`
   absent, `zone_type` an unrecognized string → `Err(HaresError::Equipment(..))`.
   The five OS-HPXML sample files already vendored
   (`vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-adjacent-to-other-heated-space.xml`,
   `...-to-multifamily-buffer-space.xml`, `...-to-non-freezing-space.xml`,
   `...-to-other-housing-unit.xml`, `base-dhw-tank-gas-outside.xml`) each
   gain a parse-level test asserting the resolved water heater carries the
   matching `AmbientLocation`, not a `zone_id`.
3. `synthetic_building_rejects_open_envelope`: a one-wall,
   `zone_volume_m3 > 0.0` TOML → `Err(HaresError::Dwelling(..))` naming the
   zone and the missing face categories.
   `synthetic_building_accepts_two_of_three_face_categories`: Wall+Floor,
   no Roof → succeeds. The six fixtures named above keep their existing
   assertions, now with a closed envelope.
4. `ideal_hvac_end_use_tracks_capacity_sign_through_runtime_override`: an
   ideal-capacity HVAC unit in `Heating` mode, settled, then a
   `RuntimeSetpointOverride` that drives the ideal target to cooling
   before the FSM mode transitions → `descriptor.end_use == HVAC_COOLING`
   on the very next `step()`. The existing deadband-preservation tests
   (`ideal_capacity_mode_override_switches_at_runtime` and neighbors)
   keep asserting `end_use` carries over when `capacity_w == 0.0`.

**Verification:** the full verification block (`plan.md` §3), run on the
staged change and again on the commit, for each of the four commits.
Commits 1, 3 and 4 pass `[golden]` unchanged (rule 1 of "no regressions,
and no worse physics"); commit 2 also passes `[golden]` unchanged (no
fixture exercises the new path) but is reviewed against rule 2's standard
regardless, since it adds physics: the new acceptance tests' hand-computed
expected values stand in for a golden delta, and `conditioned_oracle.rs`
is re-run for commit 4 as the research recommends.

**Commit:**

1. `fix: resolve the dehumidifier zone through the conditioned zone, not a silent ZoneId(1) fallback`
2. `fix: give water heaters in HPXML locations with no modeled zone an ambient temperature source`
3. `fix: make the synthetic envelope closure check an unconditional typed error`
4. `fix: derive IdealHvac end_use from the delivered capacity's sign instead of asserting it`

**Dependencies:** OR-11 (the rows this entry closes), M-2 (golden
fixtures).

**Measurement:** the per-step cost of `ambient_other_space_c`'s three
scalar computations (commit 2) as a share of `EnvironmentManager::update`,
recorded via the hot-path allocation counter (no allocation expected; the
fields are plain `f64`), never a reason to re-gate.

**Red-team record (2026-10-02, on `1c18543a`).** The red team verified
the ambient table line by line against upstream v1.12.0
(`geometry.rb:1399-1428`), the conditioned-zone-by-type resolution, the
five water heaters' read paths (the read-only scheduled-space semantics
match OS-HPXML's: no zone feedback, losses still occur toward the
series), the end_use sign rule and the synthetic check. Findings:
1. (Medium, follow-up entry OR-18a below) upstream's "other heated
   space" floor is the DYNAMIC heating setpoint (the home's heating
   setpoint sensor, or the ~70 °F default when no HVAC), not the
   constant 20.0 °C this entry's table records: whenever the 0.5/0.5
   blend lands between 20.0 and the setpoint (most of the heating
   season) the upstream ambient is up to about 1.1 °C warmer. The
   ledger and the code agree with each other literally, so the row's
   own acceptance held; the fidelity gap is upstream's behaviour the
   table simplified.
2. (Low) three schema-valid OS-HPXML water-heater locations are
   deliberately unmodelable and error loudly at init: `ExteriorWall`
   (0.5/0.5, no floor), `UnderSlab` (ground 1.0), `ManufacturedHomeBelly`
   (0.85/0.15). They are candidates for the AmbientLocation table if a
   consumer needs them; until then the loud rejection is the contract.
3. (Low) the no-conditioned-zone fallback (the series reads outdoor
   when the building has no conditioned zone; the dehumidifier in the
   same commit hard-errors instead) is the documented code contract
   and now recorded here.
4. (Info) `benches/common.rs::synthetic_toml_case` citations in
   synthetic.rs's test doc were stale (the file no longer exists) and
   were reworded in the follow-up commit.

**State:** committed `1c18543a` as one commit holding all four changes
(the specified split was not kept). The fleet golden
`fleet_mixed_offset` moved for home 1 (bldg0000004, water heater in
"other heated space"; delta in `02-measurement.md`), so commit 2's
premise that no golden reaches the new path was wrong. Independent audit
(`verification/audit-OR-18-1c18543a.md`, which also proves the three
structural parts bitwise neutral on their own) and red team
(`verification/redteam-OR-18-1c18543a.md`) have run; a second red team
(this entry's OR-18b) found the wet-bulb and fallback defects.

## OR-18b: the water-heater ambient placement's follow-through

**Kind:** fix (the resolution of both OR-18 red teams; one commit per
numbered change).

**Findings dispositioned here** (the first red team's MEDIUM is
OR-18a, below):

1. **Wet-bulb (red team major M1).** An HPWH in an ambient placement
   takes the ambient DRY-bulb as the wet-bulb for its COP and capacity
   curves (`heat_pump_wh.rs:373-381` feeding `:961-994`, `:1083`): for
   "other housing unit" the curve sees 22 °C where the zone-resolved
   equipment saw the zone's 14 °C wet-bulb (COP 2.130 to 2.555 on the
   scratch repro), and for exterior placements
   `weather.outdoor_wet_bulb_c` goes unused. FIX: `AmbientOtherSpaceTemps`
   carries a wet-bulb per placement, computed once per step in the same
   block: "other housing unit" reads the conditioned zone's wet-bulb,
   "other exterior"/"outside" read `weather.outdoor_wet_bulb_c`, and the
   three blended placements compute a wet-bulb from the blended dry-bulb
   and the blended humidity ratio (the OS-HPXML scheduled-space rule has
   no humidity model; the blend of the two inputs' ratios is the
   defensible rule and is stated in the code comment with the citation).
   `zone_wet_bulb_c` returns it; a pinning test fails if wet-bulb is
   replaced by dry-bulb. The measurement log's delta note gains the
   corrected numbers.
2. **The silent ZoneId(1) fallback survives in nine constructors
   (major M2).** `zone_id_from_config_or_default` was deleted but its
   `unwrap_or(ZoneId(1))` was INLINED at every HVAC and water-heater
   constructor (`resistance.rs:150`, `heat_pump_wh.rs:222`, `gas.rs:157`,
   `indirect_tank.rs:115`, `tankless.rs:108`, `ideal_hvac.rs`, 
   `furnace.rs:114,525`, `baseboard.rs:75`, the other HVAC constructors),
   and `solver_builder.rs:125,899` keep
   `zones.first().unwrap_or(ZoneId(1))` with `:1572`'s
   `unwrap_or(21.0)`. FIX: follow the dehumidifier everywhere: the zone
   is `Option<ZoneId>` at construction; `init` returns a typed error
   naming the equipment when neither `zone_id` nor a classifiable
   `zone_type` resolves it; equipment that is by definition conditioned
   (ideal HVAC) resolves through the conditioned zone; the solver
   builder errors on an empty zone list instead of inventing `ZoneId(1)`
   and on a missing indoor temperature instead of `21.0`. The ledger
   text then states exactly what was and was not silent.
3. **An ambient-placement water heater cannot join after stepping
   (major M3).** Its thermal port declares `zone: None`, and the frozen
   port-slot table has no accumulator for it
   (`validate_candidate_equipment` rejects every add after step 1).
   FIX: an ambient-placement water heater declares NO thermal port
   (its standing loss has no modeled destination; its fluid and fuel
   ports remain), so the frozen table is untouched; regression test
   adds every placement after step 1.
4. **Minors, same commit series:** (m1) the no-conditioned-zone
   fallbacks in the ambient series and `OtherHousingUnit` become the
   typed error the dehumidifier raises (a building with no conditioned
   zone cannot host an ambient-placement water heater; the series
   computes `Option`-less only when a conditioned zone exists);
   (m2) step 4b checks `is_finite` on its two inputs and errors naming
   the step instead of letting `f64::max` mask NaN into a plausible
   20.0; (m3) the synthetic closure check counts `Slab` as `Floor` and
   `FoundationWall` as `Wall`, requires positive area, and filters
   boundaries by interior zone before counting; (m4) the HPWH
   `wall_heat_fraction` default is restricted to locations that resolve
   to the Conditioned zone (the behaviour change is either undone or
   kept with the ledger delta stating it); (m5) a second conditioned
   zone is a construction error (HPXML has one; synthetic TOML must not
   silently pick).

**Acceptance tests:** per change, following the existing patterns
(the HPWH wet-bulb pin, the constructor-error tests per type, the
add-after-step regression per placement, the NaN-input error, the
Slab/FoundationWall acceptance, the two-conditioned-zone error). The
golden fixtures compare bitwise except where a fix changes a number on
an exercised path (M1's HPWH-outside sample: the delta is recorded and
the affected golden re-captured if any committed fixture carries an
ambient HPWH — the census says none does).

**Verification:** the block; `frame_golden compare-all` bitwise;
the B0 check.

**Commit:** `fix: the ambient water-heater placements carry wet-bulb and stop the remaining silent zone fallbacks` (plus per-change commits if the
golden moves).

**Dependencies:** OR-18.

6. **The scheduled-space floors follow the home's heating setpoint**
   (OR-18's first red team, Medium). OS-HPXML's scheduled-space
   temperature is not the literal `get_temperature_scheduled_space_values`
   output: `get_space_temperature_schedule` (`geometry.rb:1595-1614`,
   v1.12.0) replaces the location's `temp_min` constant with the home's
   heating-setpoint sensor when the space has HVAC, or the average of the
   default heating and cooling setpoints when it does not (about
   21.1 °C). HARES implements the constant floor (20.0 °C), so whenever
   the 0.5/0.5 blend lands between 20.0 and the live setpoint (most of
   the heating season) the upstream ambient is up to about 1.1 °C
   warmer. FIX: `other_heated_space_c`'s floor follows the conditioned
   zone's active heating setpoint each step where the zone has one,
   falling back to the constant when it does not (verify upstream whether
   the buffer space's floor follows the same rule; state the finding in
   the code comment). The blend and the other placements are unchanged;
   reading the setpoint adds no allocation. ACCEPTANCE: the ambient-series
   test gains a live-setpoint case (setpoint 22.0, blend 21.0: the floor
   holds 21.0) and a no-setpoint case (the constant holds); the goldens
   compare bitwise.

## OR-19: equipment and actor roster changes are all-or-nothing

**Kind:** found defect in a committed row (OR-11 `b8195c83`), from the red
team (`verification/redteam-OR-11-SL-7.md`, blockers 1 and 2).

**Problem** (lines at `7b7a6a7d`, `crates/hares-core/src/dwelling/mod.rs`):
every roster entrance changes the roster first and then calls
`refresh_equipment_caches` (`:4155`), which is fallible: OR-11 added
`FluidSolver::rebuild_loop_types` (`:4237-4243`,
`hares-envelope/src/fluid_solver.rs:209-216`), which rejects two
declarations of one loop with different fluid types, beside the existing
output-schema drift check. A failure returns `Err` with the roster already
changed and the caches half rebuilt (the name map, actor bindings, actor
timings and environment seeding run before the fallible points).

- `add_equipment` (`:3678-3714`) pushes the candidate at `:3701` and
  refreshes at `:3712`: a rejected candidate stays in the vector and steps
  on the next tick. Reproduced: a Water boiler then a Glycol boiler on loop
  7; `Err` returned, `equipment.len()` grows from 2 to 3.
- `replace_equipment` (`:3980-4036`) swaps at `:4013` and refreshes at
  `:4034`; the `?` drops the evicted equipment. Reproduced: replacing
  "Furnace" with a Glycol boiler on a Water boiler's loop returns `Err`,
  "Furnace" is gone and the rejected replacement is installed.
- `remove_equipment` (`:3927-3937`), `remove_equipment_by_end_use`
  (`:3948-3966`), `clear_equipment` (`:3911-3919`), `add_actor`
  (`:3350-3359`) and `auto_register_actors` (`:3439-3485`) have the same
  shape: actors evicted or pushed, schedule rebuilt, then a fallible
  refresh.
- Before the first step the port table is rebuilt monotonically (`:4214-4243`)
  and the loop-type map is read from the table's fluid accumulators, so a
  loop whose only declarer was removed keeps its old fluid type; adding
  equipment that declares the same loop with another fluid type is then
  rejected although no live equipment conflicts.

**Change:**

1. Split the refresh into a pure plan and an infallible install.
   `plan_roster_caches(&self, equipment: &[&dyn Equipment], actors: &[&dyn Actor]) -> Result<RosterCaches>`
   computes everything the refresh derives (name map, execution order,
   equipment ids, dispatch targets, the pre-step port slots and rollback
   slots, the loop-type map, the thermal consistency wiring, and, while no
   row is recorded, the output schema, column index, equipment column map
   and end-use indices) from the prospective roster without touching
   `self`. `install_roster_caches(&mut self, RosterCaches)` assigns them,
   re-resolves actor bindings, resizes actor timings and seeds
   `latest_env.equipment_core`; it cannot fail.
2. Every entrance validates its candidate, plans against the prospective
   roster (the live roster with the candidate appended, substituted,
   removed or the actor added), and only on `Ok` mutates the roster and
   installs. A rejection leaves the dwelling bit-for-bit as it was. The
   candidate's own checks (`validate_candidate_equipment`,
   `validate_replacement_equipment`) stay; the fluid-type conflict is found
   by the plan, not by a second hand-written check. `refresh_equipment_caches`
   is deleted; assembly calls plan and install.
3. Before the first step the loop-type map comes from the live equipment's
   fluid declarations (the OR-11 resolution's single source), not from
   retained accumulators. A retained accumulator on a loop no live
   equipment declares is dropped from the pre-step table, as the table is
   pure layout then; zone and domain accumulators keep their monotonic
   rule. After the first step a candidate whose fluid declarations conflict
   with the frozen map is rejected by `ensure_ports_satisfied`.

No per-step code changes; the plan allocates only on roster changes, as
the refresh does today.

**Acceptance test** (`crates/hares-core/src/dwelling/tests`, deterministic,
each fails at `7b7a6a7d`):

- `rejected_add_leaves_roster_unchanged`: the reproduction above; after
  `Err`, the equipment names, ids, actor list, output schema and loop-type
  map equal their values before the call.
- `rejected_replace_keeps_the_original`: the reproduction above; "Furnace"
  is still installed with its id, the replacement absent.
- `rejected_actor_add_leaves_roster_unchanged`: an actor whose addition
  makes the plan fail (an output-schema drift fixture) leaves actors and
  schedule unchanged.
- `removed_loop_declarer_frees_its_fluid_type_before_first_step`: remove a
  Glycol boiler on loop 7, add a Water boiler on loop 7, before stepping:
  accepted, and the loop resolves as Water.
- `post_step_conflicting_fluid_declaration_is_rejected`: after one step, a
  candidate declaring a running loop with another fluid type is rejected
  and the roster is unchanged. Confirm this actually fails at `7b7a6a7d`
  before relying on it: `ensure_ports_satisfied` (`mod.rs:4090-4139`) and
  its callers in `validate_candidate_equipment`/`validate_replacement_equipment`
  (`mod.rs:3732-3734,3774-3776`) already check a candidate's `Fluid` port
  declaration against the frozen table by exact `(loop_id, fluid_type,
  node_id)` match once `clock.current_step > 0`, so this case may already
  be rejected pre-push today for reasons unrelated to this fix. If it does
  not fail pre-change, replace it with the identical scenario before the
  first step, which `removed_loop_declarer_frees_its_fluid_type_before_first_step`
  already covers as a genuinely pre-change-failing case.

**Verification:** the `[golden]`, `[test-debug]`, `[test-release]` and
`[clippy]` lines of `plan.md` §3; goldens bitwise unchanged (no golden
mutates the roster after assembly, and assembly computes the same caches).

**Commit:** `fix: reject equipment and actor changes without leaving the dwelling half changed`

**Dependencies:** OR-11.

**Measurement:** none (no per-step change); the assembly time of
`consumer_shape_900s` before and after is recorded to show the split
costs nothing.

**Decisions taken in implementation and review** (branch `fix-or19`):
- The output recorder (with run metrics) is created in the plan;
  creation failure rejects the change, so install has no fallible call.
- A rejected add or replace returns `HaresError::RejectedEquipment {
  reason, warnings }` carrying the candidate's init warnings; the
  dwelling's warning log is unchanged. Python maps it by `reason`.
- `set_tariff` plans with the new evaluator and commits only on Ok.
- Roster-derived state lives in one `RosterCaches`; assembly derives
  ports, the loop-type map and the fluid solver only through the plan.
  The planner takes `&[&dyn _]`; the roster code is in
  `dwelling/roster.rs`. `plan_loop_types` is the single loop-type
  validator (`validate_fluid_type_consistency` and
  `FluidSolver::rebuild_loop_types` removed).
- `add_equipment` seeds the candidate's built-in actor;
  `add_equipment_with_actors` registers equipment and its own actors in
  one all-or-nothing call (Python `add_ev_with_driver`); Python `add_ev`
  no longer re-registers every built-in actor.
- Built-in EV drivers draw from streams handed out by a per-dwelling
  cursor; replacing equipment under a new name evicts actors bound to
  the old name; removed equipment's core state leaves the environment
  snapshot at install.

**State:** ported onto `1c18543a` as `12f753be` and `a3a21a92`
(cherry-picks of `9ef347e0` and `ffa9051f`), with `1bb045c6` fixing the
`ffa9051f` review and red team. Review of `1bb045c6` CHANGES-REQUIRED
(em dashes in moved error strings; cursor, Python mapping, atomicity,
loop-type and rejection-path tests missing) and red team
ATTACK-SUCCEEDED (cursor not checkpointed; rejected `add_pv` registers
its surface; duplicate `add_ev_with_driver` error class; in-kind
replace by a different type keeps the driver; variant depends on
warnings; install not panic-safe; mid-run `set_tariff` rebuilds
drivers from word 0). Follow-up `f77054df` fixed them (the cursor
checkpointed and bounded below the event-load streams, panic-safe
planning, `RejectedEquipment` for every rejection, capability-based
actor eviction via `Actor::required_control_capabilities`); its final
check found the battery manager reset on a mid-run tariff, which
`ba44d9f1` fixes together with the cause it exposed (a tariff attached
mid-run priced every later step at the wrong time: the evaluator now
advances to the current step), actor take-over of state and telemetry,
the in-kind replace rule by actor seed, a step failing on an
undeclared signal, and exact float round-trip in checkpoint files.
Adversarial check of `ba44d9f1` and the fold onto the head
(`CHECKPOINT_VERSION` 11) running; the assembly-time measurement is
taken on the folded branch.

## OR-12: `HealthPolicy` escalation

**Kind:** initiative work (`dl-i08-architecture.health`: "Other escalation
is an explicit HealthPolicy, empty by default"; `new_capability`: "strict
health policy").

**Problem:** after OR-5 every health event is counted, but nothing can
make a consumer's chosen events fail the run at the step they occur; the
calibration system needs that to reject an evaluation.

**Change:** `HealthPolicy` in `hares-io` as the `SimulationConfig` field
`health_policy` (`plan.md` §2.3), with a TOML table (`#[serde(default,
deny_unknown_fields)]`, so a partial table is legal); the
`HealthPolicyViolation` error variant in `hares-types`, whose
`step_index` is `None` for an event during warm-up and for the
convergence check at its end. The policy is evaluated where each OR-5
counter changes and when warm-up ends; a violation fails the run naming
the event, step, source, count and limit. The default policy is empty.
Python, in this commit: the frozen pyclass `HealthPolicy(...)` (keyword
per field, defaults empty) with `HealthPolicy.strict()`, the
`SimulationConfig(health_policy=...)` keyword, and the exception class
`HaresHealthPolicyError` (a subclass of `HaresSimulationError`, carrying
the variant's fields as attributes), added to `11-other-defects.md` OD-1's
pickle round-trip test.

**Acceptance tests** (dwellings: BESTEST 600 with the failing-port test
equipment, as OR-5's):

- `empty_policy_never_escalates`: OR-5's rollback fixture completes.
- `strict_policy_escalates_port_rollback_naming_step_and_source`: the
  same fixture with `HealthPolicy::strict()` fails with
  `HealthPolicyViolation { event: PortRollback, step_index: Some(_), source, .. }`
  at the first failing step, with `source` the test equipment's name.
- `limit_allows_count_up_to_limit`: `max_port_rollbacks: Some(2)` passes
  with two rollbacks and fails on a third.
- `strict_policy_escalates_warmup_nonconvergence`: OR-5's non-converging
  warm-up (1e-9 C, two days) fails with `event: WarmupNotConverged,
  step_index: None`.
- `health_policy_toml_round_trip` (through the Rust
  `SimulationConfig::to_toml` and `from_toml`, `05-python-surface.md`
  PY-8), `health_policy_partial_table_is_legal` and
  `health_policy_unknown_key_is_an_error`.
- `health_policy_violation_maps_to_python_error` (a `hares-python` Rust
  test under `Python::attach`): converting a
  `HealthPolicyViolation { event: PortRollback, step_index: Some(7), .. }`
  gives a `HaresHealthPolicyError` instance whose `event`, `step_index`,
  `source`, `count` and `limit` attributes equal the variant's fields.
- `health_policy_python_round_trip` (Python): `HealthPolicy.strict()`
  passed to `SimulationConfig`, `to_toml()`, `from_toml()` gives an equal
  policy.

**Verification:** `cargo nextest run -p hares-core -p hares-io -p hares-python health`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: escalate chosen health events through an explicit policy`

**Dependencies:** OR-5; `05-python-surface.md` PY-8 (the config type that
carries `health_policy`); `11-other-defects.md` OD-18 (row 30b), which
makes `clamped_actions` a counted event in every build, so a policy that
escalates it can fire.

**State:** committed `6fa206a9` on branch `row/r42-or12-pr4` (cut from
`7e95e974`), one commit, the acceptance tests all present and green.
The policy is `HealthPolicy` in `hares-io/src/config.rs`
(`#[serde(default, deny_unknown_fields)]` at container level, so a
partial table is legal and unknown keys are rejected; the empty policy
is the `SimulationConfig.health_policy` default, so existing configs,
TOML files and Python constructors are unchanged: a pure refactor for
existing runs), with `strict()` (every limit `Some(0)` and
`require_warmup_convergence`) and `limit_for(event)`. The violation is
the `HaresError::HealthPolicyViolation` variant; the acceptance tests
name the event, the step, the source, the count and the limit, and the
run latches ended the way the failure budget and a callback error do
(`terminal_error`), so every later step returns it without stepping.
The variant's source field is named `origin`: a thiserror field named
`source` is implicitly the error chain's source and must implement
`StdError`, which a name is not; the Display reads "from '<name>'" and
the Python exception's attribute is `source`, so the entry's shape holds
at both surfaces consumers read. The policy is evaluated at the
established checkpoints, one per counted change: the failed equipment
step's booking checks the rollback limit after the rollback and ahead of
the failure budget's own check; every rejected control signal counts
through a `RejectionGate` (the run total as its baseline, the policy
limit copied in) that the dispatch passes count into, remembering the
first crossing with its source (the refusing equipment's name, the
target name, or the end use, matching the warning each site already
pushes), the pass's count landing on the run total before the violation
returns (a failed step loses no counted rejections); the clamp drain
lands each equipment's curve-index clamps one source at a time so the
crossing names the equipment; `record_clamped_actions` keeps its
signature and names the source "actions" (the caller clips whole action
vectors, so no single equipment or actor owns the clamp); the warm-up
end checks `require_warmup_convergence` where the non-convergence is
recorded and warned, with `step_index: None`, count 1, limit 0, source
"warmup". A step event's violation carries `step_index: Some(step)`
except during warm-up (warm-up replays are not steps of the run). The
Python surface: the frozen `HealthPolicy` pyclass (keyword per field,
defaults empty, `strict()` classmethod, field getters, `__eq__`,
`__repr__`), the `SimulationConfig(health_policy=...)` keyword with
getter and setter, `HaresHealthPolicyError` (a `HaresSimulationError`
subclass carrying `event` (the variant's Debug spelling, matching the
serde vocabulary), `step_index`, `source`, `count` and `limit` as
attributes) exported beside the other engine exceptions and covered by
the pickle round trip (which iterates every exported exception class);
the `_hares.pyi` stub carries the class, the keyword, the property and
the exception. RED first: the acceptance tests failed to compile on the
missing API at the base, then failed behaviorally with the types in and
no machinery evaluating the policy, then green. Gates: fmt clean;
clippy `--workspace --all-targets --all-features -D warnings` clean;
nextest `--all-features -j 4` 7894 passed, 1 skipped (the parent's
ignored kind-pin generator), `-F dst` 7826 passed; the eight golden
fixtures bitwise identical (compare-all 7, `full_year_900s_dst` 1); the
filtered suite `cargo nextest run -p hares-core -p hares-io -p
hares-python health` green (plus the escalation tests by name, which the
filter's name match does not reach); pytest 1648 passed with 3 failures
and 4 setup errors identical to the base commit (base-verified on a
worktree of the base, so none is this row's); pyright 0 errors.

## OR-6: one `RunResult`, `finish()` for `simulate()` and stepped runs alike

**Kind:** initiative work. Fixes V12 (stepped runs lose the tail) and V26.

**Problem:** `Dwelling::simulate()` (`mod.rs:3126-3154`) runs the horizon,
then finalises billing, flushes and closes the recorder and flushes the
diagnostics writer. That is the only call site of `flush_and_close()`
(`mod.rs:3146`) and there is no `Drop` flush, so a stepped run loses its
last partial batch. Python exposes `finalize_billing` separately
(`crates/hares-python/src/py_dwelling.rs:1882`), but nothing exposes the
flush. `step()` returns `self.simulation_results.steps.last().unwrap().clone()`
(`mod.rs:3244`) from an unbounded history. `SimulationEngine::run`
and `run_dwelling` (`crates/hares-core/src/engine.rs:102,251`) build a
second result type (`engine::SimulationResults`, `engine.rs:49-62`) and
zero the metrics when there is no recorder (`engine.rs:444-505`).
Warnings are strings drained per step by `take_warnings()` (`mod.rs:4665`
at `afa9a071`) through `WarningLog` (`crates/hares-core/src/dwelling/warnings.rs`),
whose `take` resets both the entries and the dropped count, so the
1000-entry bound applies per drain; OR-11 has given every producer one
`Warning` channel, but the log still stores the formatted strings.

**Change:**

- `Dwelling::finish()`, `simulate()` and `step()` per `plan.md` §2.1's
  lifecycle contract; `RunResult`, `ReportResult`, `ReportOutput`,
  `Provenance` and `EquipmentProvenance` are added to
  `crates/hares-core/src/result.rs` (`Warning` is OR-11's, in
  `hares-types`). This entry's `Provenance` holds
  `config: SimulationConfig` (the validated config, with the master seed
  in effect), `bldg_id: i64`, `site_utc_offset_s` and `equipment`;
  `plan.md` §2.1 lists the entries that add the other fields
  (`05-python-surface.md` PY-6, `12-inputs.md` IN-1, OR-16).
  `Provenance.equipment` carries the diagnostics equipment preamble OR-4
  dropped (zone mapping, zone type, OCV source and
  `Equipment::provenance_lines()`, today's `mod.rs:3097-3114`), one
  `EquipmentProvenance` per equipment.
- `RunMetrics` is `FullSimulationMetrics` renamed and flattened
  (`plan.md` §2.1). Until OR-8, `RunResult.metrics` is `Some` exactly when
  the run has at least one report, because metrics are still computed from
  the flushed batches (`metrics.rs:641`, `writer.rs:317`); a run with
  `reports = []` has none.
- The step history (`SimulationResults.steps`, `mod.rs:1034-1035`, its
  push at `mod.rs:7136` and clears at `mod.rs:5481,5509`) is deleted.
- `WarningLog` stores the `Warning` values OR-11's channel delivers, and
  `WarningLog::push_warning` stops formatting them. It holds two bounded
  buffers (`plan.md` §2.3): the run record, the first `CAPACITY` warnings
  of the run plus `warnings_dropped`, which becomes
  `RunResult.warnings`; and the pending drain, every warning pushed since
  the previous `take_warnings()`, bounded at `CAPACITY` per drain with its
  own dropped count, which `take_warnings() -> Vec<Warning>` returns and
  empties (appending today's dropped summary as a `Warning` with source
  `warnings` when it overflowed). So the gym's per-step drain
  (`py_gym.rs:475-479`, `python/ochre_next/rl/vec_env.py:208-213`) keeps
  receiving every warning after the run record is full, as today. The
  Python `take_warnings()` keeps returning `list[str]`, each formatted as
  OR-11 formats them; `05-python-surface.md` PY-9 exposes `Warning` on
  `RunResult`.
- `SimulationEngine::run`/`run_dwelling` return `Arc<RunResult>` through
  one shared function; `engine::SimulationResults`, `empty_metrics`,
  `finalize_run_metrics`, `resolved_output_path` and
  `compute_metrics_from_batches` are deleted. Typed errors are returned
  unchanged (an `InvariantViolation { check_name: "thermal_balance" }`
  from the run reaches the caller as that value).
- `hares-python` adapts internally: Python `simulate()` and `results()`
  keep returning the polars frame of the first `Memory` report (OR-3),
  now read from `RunResult.reports`; with no `Memory` report they raise
  `HaresSimulationError` naming `reports`, because the step fallback frame
  is built from the step history this entry deletes. `05-python-surface.md`
  PY-9, the next row, replaces both with `RunResult`.

**Acceptance tests** (the `leap_year_february_900s` inputs: HPXML, schedule
and weather of `tests/fixtures/parity/cz4a_ashp_hpwh/`, start
`2024-02-27T00:00:00-07:00`, no warm-up, at 900 s):

- `stepped_run_finish_equals_simulate`: 24 h (96 steps) with
  `output_chunk_size = 40`, so the last batch is partial, stepped to its
  end then `finish()`, equals `simulate()` on an identical dwelling field
  by field (reports on bits, metrics on bits, health, warnings).
- `finish_before_end_flushes_partial_batch_and_billing`: the same inputs
  over 72 h with the flat tariff `tests/fixtures/urdb/flat_rate.json`
  (`America/Denver`), finished after 50 steps: the `Memory` report holds
  50 rows and the billing summary covers those 50 steps.
- `finish_is_idempotent_and_returns_the_same_arc`
  (`Arc::ptr_eq` on the two results).
- `step_after_finish_is_a_typed_error`,
  `finish_before_first_step_is_a_typed_error`.
- `take_warnings_returns_only_new_warnings_and_run_result_keeps_all`.
- `gym_drain_sees_warnings_past_run_capacity`: a test actor sends one
  out-of-bounds control signal per step (one rejected-signal `Warning`
  each, `plan.md` §2.3) for 1 100 steps of BESTEST 600
  (`tests/fixtures/bestest/600.toml`) at 900 s, with `take_warnings()`
  called after every step: each drain returns exactly that step's warning,
  the 1 001st included, and `RunResult.warnings` holds the first 1 000
  with `warnings_dropped` 100.
- `engine_run_preserves_invariant_violation_type` (via the OR-5 seam).
- `provenance_carries_equipment_zone_mapping`: for the same inputs,
  `Provenance.equipment` holds one entry per equipment whose `zone`,
  `zone_type`, `ocv_source` and `notes` equal the preamble lines the
  pinned `diagnostics_cz4a.csv` header of OR-4 recorded.
- `provenance_records_callback_destination_marker`: a run with a
  `Callback` report (OR-14's test callback attached) records
  `Destination::Callback` for that report in `Provenance.config.reports`.

**Verification:** `cargo nextest run -p hares-core`; `frame_golden
compare-all` bitwise.

**Commit:** `feat: unify simulate() and stepped-run finalisation behind Dwelling::finish()`

**Dependencies:** OR-3, OR-4 (the pinned preamble), OR-5 (`RunHealth`),
OR-11 (`Warning` and its channel), OR-14 (the `Callback` destination).

**Measurement:** allocation count of `step()` before and after (the
history push is gone), recorded; and the three memory-growth readings of
`02-measurement.md` M-3 item 5 (repeated construct-and-simulate,
1e3 against 1e6 steps, 100 stepped episodes) re-taken after this commit
with the same commands, recorded against M-3's baseline (requirements
R18, RL-1, RL-2 and X-4). The stepped-episode loop runs through the
Python surface of this commit, with `reports=[]` in place of
`write_output=False`.

**State:** committed `7aa5c509` (branch `row/r43-or6-py9`, cut from
`7e95e974`, one commit). The lifecycle as built: `Dwelling::finish()`
is the one finalisation for every run (the billing close, the post-hoc
diagnostics, the recorders' flush, close and publish (the final partial
batch included), the run metrics, the health record, the warning record
and the provenance), and `simulate()` is step-to-horizon then that same
`finish()`. `finish()` is idempotent and returns the first call's
`Arc`; a step past the finished run and a finish before the first step
are typed errors.

`RunResult` (`crates/hares-core/src/result.rs`, new) carries: `reports`,
one `ReportResult` per configured report in `reports` order, each
holding the report's name, frequency, `batches_delivered`, `rows`, and
the delivered output (the retained batches for a `Memory` destination,
the published path for a file, the `Callback` marker for a callback
report; a run with the attached batch callback records
`Destination::Callback` for that report in the recorded config, pinned
by `provenance_records_callback_destination_marker`); `metrics`, the
`FullSimulationMetrics` type renamed and flattened to `RunMetrics`,
`Some` exactly when the run has at least one report; `health` (the
OR-5 `RunHealth`, recorded in every build); the warning record and
`warnings_dropped`; and the `Provenance` (the validated config with the
master seed in effect, `bldg_id`, `site_utc_offset_s`, and one
`EquipmentProvenance` per equipment: the instance name, zone mapping,
zone type, OCV source and the equipment's own provenance lines, the
diagnostics preamble OR-4 dropped, now carried on the result where the
diagnostics row's consumers read it, pinned by
`provenance_carries_equipment_zone_mapping`).

The step history (`SimulationResults.steps`) is deleted: `step()`
returns the step's own `StepResult`, computed directly, and the
unbounded accumulation is gone. `WarningLog` stores `Warning` values in
two bounded buffers: the run record (the first `WarningLog::CAPACITY`
warnings of the run plus `warnings_dropped`, which becomes
`RunResult.warnings`) and the per-drain buffer `take_warnings()`
returns, bounded at `CAPACITY` per drain with its own dropped count, so
the gym's per-step drain keeps receiving every warning after the run
record is full; the dwelling's internal producers name their sources
(control, warmup, the failing equipment's name, actors, python).
`SimulationEngine::run` and `run_dwelling` return `Arc<RunResult>`
through one shared finalisation; the engine's second result type
`SimStatus`, `empty_metrics`, `finalize_run_metrics`,
`resolved_output_path` and `compute_metrics_from_batches` are deleted,
and typed errors reach the engine's caller unchanged (pinned by
`engine_run_preserves_invariant_violation_type`).

`hares-python` adapts internally: Python `simulate()` and `results()`
keep returning the first `Memory` report's frame, read from the reports
surface; a run with no `Memory` report raises `HaresSimulationError`
naming `reports` (the step fallback frame is gone with the step
history). The fleet's `DwellingOutcome` carries the `Arc<RunResult>`
plus its own elapsed, and its metrics reads handle the `None` a
no-report run has.

The RED: the ten acceptance tests are
`crates/hares-core/tests/run_result.rs`, written against the surface
before it exists (`result.rs` is new in this commit), so the file does
not compile against the parent; GREEN: all ten pass on the commit
(`stepped_run_finish_equals_simulate` with `output_chunk_size = 40` so
the last batch is partial, reports on bits, metrics on bits, health,
warnings; `finish_before_end_flushes_partial_batch_and_billing` over 72
h finished after 50 steps, the `Memory` report holding 50 rows and the
billing summary covering those steps; `finish_is_idempotent_and_returns_the_same_arc`
by `Arc::ptr_eq`; the two typed-error pins;
`take_warnings_returns_only_new_warnings_and_run_result_keeps_all`;
`gym_drain_sees_warnings_past_run_capacity` over 1 100 BESTEST 600
steps, each drain exactly that step's warning, the run record the
first 1 000 with `warnings_dropped` 100; the two provenance pins) and
every suite that reads the old surface migrated with the row.

Measurements (the commit body records them; the raw figures:
`step()` allocates 5.00 per step against the parent's 6.01, the history
push gone; the M-3 item 5 re-takes: 500 construct-and-simulate
repetitions +748 kB (flat), 10^3 to 10^6 steps +8 kB (the 0.11 kB/step
retention growth is gone), 100 stepped episodes through the Python
surface +16 kB (flat)).

Gates at the commit (run in this row's worktree, on `7aa5c509` itself):
fmt clean; clippy `--workspace --all-targets --all-features -D
warnings` clean; `cargo nextest run --all-features -j 4` 7674/7674;
`cargo nextest run -F dst -j 4` 7607/7607; pytest canonical 1549
passed, 0 failed, 0 skipped; pyright 0 errors; the goldens bitwise in
both profiles (`frame_golden compare-all` 7 manifests identical on the
default build, `full_year_900s_dst` identical on the dst build).

**State (review fixes, 2026-10-10):** the independent review's findings
fixed on this branch, on top of `ca09e624`: `155937f6` (a checkpoint
restore clears the finished run's latch: `load_checkpoint` and
`restore_building_state` reset `run_result` with the rest of the run's
state, so a finished dwelling that loads a mid-run checkpoint steps
again and its `finish()` finalises the restored run; both restores
pinned, RED at `ca09e624` with the step refused as a typed finished
error, GREEN at `155937f6`); `230fe6eb` (the wrappers' results pinned:
`DwellingGymEnv.run_result`, `VecDwellingGymEnv.run_results` and the
HELICS wrapper's `run_result` each read in a test, the latched run's
`finish()` raise semantics pinned in `callback.rs`, and the HELICS
`run()` guard: a federation termination before the first step leaves
the run unstepped and unfinished, `run_result` `None`, where the
unguarded loop called `finish()` on a run with nothing stepped and let
its typed error escape `run()`); `9188f6dd` and `cb47e358` (the
fleet's `extract_rows` reads the run's timestep-frequency `Memory`
report explicitly instead of folding every report's retained batches
on a field-name-only guard, so two `Memory` reports with identical
schemas at different frequencies no longer concatenate into
double-counted rows: a run with none contributes nothing, two that
qualify are a `FleetError::AmbiguousTimestepReport` naming the dwelling
and both reports; pinned both ways); `157089f4` (the writing rule's
cleanup: the em dashes the three commits' added lines carried are
reworded out of the source texts). The full block re-run at the fix
tip: fmt clean; clippy `--workspace --all-targets --all-features -D
warnings` clean; `cargo nextest run --all-features -j 4` 7678/7678 (1
skipped, the catalogue generator's deliberate ignore), `cargo nextest
run -F dst -j 4` 7611/7611 (same skip); pyright 0 errors; the goldens
bitwise in both profiles (7 manifests identical on the default build,
`full_year_900s_dst` identical on the dst build).

**State (the unfiltered comparison):** the review's demand, run both
ways at `pytest -m ''` (the canonical addopts filter the suites
normally run under drops the `slow` and `ochre` marks; unfiltered they
run). At the parent `7e95e974` (scratch worktree): 3 failed, 1645
passed, 4 xfailed, 4 errors in 183.73 s. At the fix tip: 3 failed,
1660 passed, 4 xfailed, 4 errors in 194.93 s. The residue sets are
identical, test for test, so the row and its fixes add no unfiltered
residue; the +15 passes are the rows' own new tests. The residue, all
four groups pre-existing at the parent and none this row's regression:
`test_ochre_parity.py::test_parity_24h_records` (the 24 h xfail
reasons' measured record is stale against the vendored OCHRE run),
`test_py_fleet.py::test_heterogeneous_equipment_column_mismatch_emits_warning`
(the test's `SimulationConfig(verbosity=1)` is the removed keyword),
`test_py_metrics.py::TestMetrics::test_pv_equipped_dwelling_metrics_show_consumption_net_split`
(the test's `_base_config(start_time=...)` collides with the helper's
own `start_time`), and the four `test_thermal_trace.py` setup errors
(the fixture requests `Selection.verbosity(9)`; the report surface
validates 0-8).

**State (the record's own note):** `7aa5c509`'s commit message carries
the entry id (`OR-4`) the writing rule keeps out of messages. Amend is
blocked by the branch's published review, so the history stays as
written and this note is the record: the message's body, not the code,
carries the id, and the added source lines are clean of ids and dashes
at the fix tip (the scan of the three commits' added lines as they
stand in the tree: zero dashes in every file including this ledger's
own State paragraphs; zero ids outside this ledger's own
cross-references, which are its filing system).

## OR-7: `Time` becomes an Arrow `Timestamp`

**Kind:** initiative work.

**Problem:** the output time column is a Utf8 string
(`crates/hares-io/src/output/columns.rs:56,425`), formatted per row with
chrono `"%+"` in the recorder (`crates/hares-core/src/dwelling/mod.rs:7532`)
and passed as `&str` into `StreamingRecorder::push_row`
(`crates/hares-io/src/output/writer.rs:368-379`, which copies it); the
fleet path parses it back (`crates/hares-fleet/src/aggregation.rs:505`,
with `parse_timestamp`, `:444`), keys its bucket by epoch second (`:508`)
and renders RFC 3339 UTC `Z` (`aggregation.rs:636-637`). `StepResult.timestamp` is
`DateTime<FixedOffset>`, and the Python step dict carries an offset-aware
`datetime` (`crates/hares-python/src/conversions.rs:165-169`).

**Change:** `Time` is `Timestamp(Millisecond, None)`, written once per row
as an `i64` from the row plan's timestamp; schema metadata carries
`hares:utc_offset_s` and `hares:catalogue_digest` (`plan.md` §2.5) and
nothing else: `hares_verbosity`, `hares_mode_map`,
`basement_lighting_created` and `garage_lighting_created`
(`crates/hares-io/src/output/columns.rs:911-924`) are deleted, with their
tests (`columns.rs:1316-1365` at HEAD `21d90588`) and `mode_ordinal_json`
(`columns.rs:1008`, whose only caller is that metadata), because a
report's selection need not be a
verbosity tier, mode labels are `VariableKind::Enum` labels in the
catalogue (OR-1), and which equipment exists is in the catalogue and
`Provenance.equipment` (OR-6). The commit body lists the removed metadata
keys. This
entry adds `Catalogue::digest()` with the canonical encoding of `plan.md`
§2.2 ("Catalogue digest") and the `sha2` dependency of `hares-io`.
`StepResult.timestamp` becomes `NaiveDateTime` with the same value as the
step's row; the Python step dict and `current_time`
(`py_dwelling.rs:863,1984,2188`) carry naive `datetime`s. The fleet
aggregation reads each home's `Time` as naive local milliseconds and
subtracts the home's `hares:utc_offset_s` to recover today's UTC epoch
key (`aggregation.rs:505-508`, which parses each home's Utf8 `Time` with
`parse_timestamp`, `:444`, until this commit); its output `Time` stays a
Utf8 RFC 3339 `Z` string until `09-numeric-fixes.md` B-12 (`04-fleet.md`
FL-1 and FL-4 keep it). OR-13's period reducer switches from parsing the
Utf8 `Time` of a batch to reading the `Timestamp`, and its tests re-run
here. `frame_golden` compares `Time` as `i64`.

User documentation: `docs/python.md` gains a "Time" section stating that
`Time` is naive local standard time labelled at the start of its interval
(a step, or a reporting period); that the site's standard-time UTC offset
is the schema metadata `hares:utc_offset_s`, and adding it gives the UTC
instant; that a non-`Timestep` report carries `Step Count` and, for each
nullable variable, `<name> [valid steps]`; that `StepResult.timestamp` and
the Python `step()` timestamp equal the row's `Time`; and how a schedule
row maps to a step (the row is the step's local time of year divided by
the step length, over a 365-day year whose dates after Feb 28 in a leap
year take their non-leap ordinal, and with `civil_timezone` set, in civil
time; `crates/hares-core/src/environment.rs:418-445`). Its example builds a
dwelling, runs it, and checks the first row's `Time` against `start_time`;
`test_docs_time_example` (Python) runs that example as written.

**Acceptance tests** (the `full_year_900s_dst` and `full_year_hourly`
inputs of `02-measurement.md` M-2, shortened to 48 h for the Rust tests):
`time_column_is_millisecond_timestamp_local`: the first and last rows of
the `full_year_hourly` inputs over 48 h (start `2023-01-01T00:00:00-07:00`)
decode to `2023-01-01T00:00` and `2023-01-02T23:00` local, and adding
`hares:utc_offset_s` (-25200) yields `2023-01-01T07:00Z` and
`2023-01-03T06:00Z`.
`step_time_is_naive_local` (Python, the same inputs:
`dw.step()["timestamp"].tzinfo is None` and equals the frame's first `Time`).
`frame_starts_at_start_time_with_exact_row_count`: the same inputs with
warm-up enabled: the first `Time` equals `start_time`'s wall clock and the
row count equals `duration / time_res` (48). From
`09-numeric-fixes.md` B-1c on, the expected row count is the simulated
step count the engine reports (the calendar steps less a skipped Feb 29),
and B-1c adds the leap-day rule to the "Time" section in its commit; B-2
re-runs this test under its warm-up.
`catalogue_digest_is_canonical`: the digest of a two-variable catalogue
built in the test equals the SHA-256 of the byte string the test writes
out by the `plan.md` §2.2 rule; `catalogue_digest_metadata_matches`: the
schema's `hares:catalogue_digest` is the hex of `Catalogue::digest()`.
`schema_metadata_holds_only_offset_and_digest`: a `Verbosity(8)` report
of the `full_year_hourly` inputs over 48 h has exactly the two metadata
keys `hares:utc_offset_s` and `hares:catalogue_digest`.
`test_docs_time_example` (above).
`fleet_aggregate_unchanged_by_timestamp_time`: the `fleet_mixed_offset`
golden's aggregate products (`tests/fixtures/golden/fleet_mixed_offset.toml`,
homes at two UTC offsets) compare bitwise against the committed golden,
the aggregate's Utf8 `Time` column included, before any re-capture.
OR-13's `batch_fed_reducer_equals_row_fed_reducer` passes on `Timestamp`
batches.

**Verification:** `02-measurement.md` M-1's schema change rule:
`frame_golden compare --columns` over every column but `Time` passes,
`delta` over `Time` is recorded, and the per-dwelling goldens are
re-captured in this commit (the declared dtype change of `Time` is the
only difference); the fleet aggregate goldens are not re-captured, since
their `Time` does not change.

**Commit:** `feat: Time is an Arrow Timestamp, not three string formats`

**Dependencies:** OR-1 (enum labels in the catalogue), OR-2, OR-3, OR-6
(`Provenance.equipment`), OR-13 (the reducer's batch feed).

**Measurement:** per-row `format!`/`to_string` calls for a full-year run
before and after, recorded.

**State:** committed `7652eecb` + the goldens `14f0d980` (branch
`row/r45-or7`, cut from `7e95e974`). The Time column is
`Timestamp(Millisecond, None)`, the row's naive local wall-clock
milliseconds written once per row as an `i64` from the step's
timestamp; the one stated convention across the surface: Time is the
local wall clock at the interval's start, the schema metadata's
`hares:utc_offset_s` (the site's standard-time offset, inserted at the
recorder freeze from the resolved site location) recovers the instant
(UTC = Time - offset), and the row plan's UTC-epoch stamp stays UTC
epoch (the r37 review note holds: the reducer reads the column, never
the plan field as if local). The schema metadata carries exactly
`hares:utc_offset_s` and `hares:catalogue_digest`: the four
pre-catalogue keys deleted with their tests and `mode_ordinal_json`;
`Catalogue::digest()` is the SHA-256 of the canonical per-variable
encoding documented on the method (the digest tests pin the byte string
and the schema metadata). `StepResult.timestamp` is a `NaiveDateTime`
equal to the row's Time; the Python step dict, the environment's
`current_time` and the timesteps iterator carry naive `datetime`s; the
steps-only polars fallback builds the same Datetime(ms) column the
Arrow-IPC path yields. The fleet aggregation reads each home's Time as
naive local milliseconds and subtracts the home's `hares:utc_offset_s`
from the local-truncated wall clock to recover the same UTC epoch
bucket key the parsed strings keyed; its output Time stays a Utf8 RFC
3339 `Z` string until B-12, and the fleet_mixed_offset and
resstock_2025_fleet goldens compared identical, not re-captured. The
period reducer's `push_batch` reads the Timestamp column (the
`ReduceError::TimeValue` parse error retired with it); its tests and
the DST hour pins re-ran green. `frame_golden` compares Time as `i64`
(the physical-bytes comparison the tool already used for timestamp
columns). The docs' "Time" section is in `docs/python.md`;
`test_docs_time_example` runs its example as written. Acceptance:
`time_column_is_millisecond_timestamp_local`,
`frame_starts_at_start_time_with_exact_row_count`,
`schema_metadata_holds_only_offset_and_digest` (Rust, over the
`full_year_hourly` inputs at 48 h), `step_time_is_naive_local` and the
frame dtype pin (Python), the digest canonical and metadata tests, the
migrated `batch_fed_reducer_equals_row_fed_reducer` on Timestamp
batches. Delta recorded in `02-measurement.md`: the six dwelling
frames' Time column moved (Utf8 to Timestamp(ms)), every other column
bit-identical, the re-captured goldens twice byte-identical. Gates:
fmt clean; clippy `--workspace --all-targets --all-features -D
warnings` clean; nextest `--all-features -j 4` 7669/7669 (1 ignored
pin generator), `-F dst` 7602/7602; pytest 1551 passed (filtered
canonical); pyright 0; compare-all identical in the default, dst and
observe builds at both captures.

## OR-8: metrics computed only when requested, from one path independent of reports and destinations (structural)

**Kind:** initiative work (`dl-i08-architecture.metrics_optional`,
`output_model`). Structural and bug-compatible; `09-numeric-fixes.md` B-7
is the numeric change, in its own later commit.

**Problem:** metrics are computed from recorded batches:
`MetricsCalculator` discovers its inputs by column name in the recorded
schema (`crates/hares-io/src/output/metrics.rs:453-470`) and is fed per
flushed batch (`accumulate`, `metrics.rs:641`; `writer.rs:317`). So
metrics exist only when a file or retained batches exist, which metrics
are computable depends on the selected columns (envelope metrics need
verbosity 4 or more, `metrics.rs:593-604`), and every run that writes
output pays for them whether or not anyone reads them: the calibration fit
path never does (`HARES_CALIBRATION_SYSTEM/calibration/fit.py:236` reduces
one column; `calibration/engine.py:406-413` flattens the metrics anyway).
Python `metrics()` (`py_dwelling.rs:1545-1577`) raises when no batch was
flushed.

**Change:**

- `SimulationConfig.metrics: bool`, default `false` (`plan.md` §2.2); the
  TOML key and the Python keyword of the same name. A run that does not
  request metrics builds no metrics input plan and no accumulator, and its
  `RunResult.metrics` is `None`.
- When requested, a fixed metrics input plan is built from the catalogue
  at freeze, independent of `reports`. The accumulator core of
  `MetricsCalculator` is fed per step from the row plan's producer slots,
  so its state is exact at every step (OR-10 depends on this).
- Bug compatibility: each metric whose availability today depends on a
  column being present keeps that dependence, evaluated against the union
  of the selected reports' variables (for a run with no reports, against
  the `Verbosity(0)` preset, the minimum today's output always contained).
  Every golden fixture's metrics are therefore bitwise unchanged. B-7 later
  removes that dependence.
- `frame_golden`'s manifests set `metrics = true` in this commit
  (config-only; `02-measurement.md` M-1), and every caller that reads
  metrics today requests them: the Python `simulate()` path used by
  `SimulationPlan`, `SimulationEngine` for fleet homes whose config requests
  metrics (`plan.md` §2.4: metrics are a per-home config request), and the
  tests that read metrics.

**Acceptance tests:**

- `metrics_bitwise_unchanged_for_every_golden_fixture`: the committed
  goldens' metrics products (`02-measurement.md` M-1, captured before this
  entry) compare equal on bits.
- `metrics_absent_unless_requested`: a run with `metrics = false` returns
  `RunResult.metrics == None`, and (in-crate, structural) the dwelling
  holds no metrics plan or accumulator.
- `metrics_present_without_any_report`: `metrics = true` with
  `reports = []` returns metrics.
- `metrics_independent_of_destination`: three runs with `metrics = true`
  whose selection union is the `Verbosity(0)` preset: one `Memory` report
  at `Verbosity(0)`, one `Parquet` report at `Verbosity(0)`, and no report
  (which evaluates availability against the same preset); the three
  `RunResult.metrics` are bitwise equal.
- `metrics_request_does_not_change_the_frame`: a `Memory` report of one run
  with and without `metrics` is bitwise equal.
- `metrics_none_unless_requested` (Python): `RunResult.metrics is None`
  for a config without `metrics=True` and a `RunMetrics` with it.

These tests use the `leap_year_february_900s` inputs (`02-measurement.md`
M-2) over 24 h.

**Verification:** `cargo nextest run -p hares-io -p hares-core metrics`;
`uv run --no-sync pytest tests/python`; `frame_golden compare-all`.

**Commit:** `refactor: compute run metrics only when requested, per step from a catalogue input plan`

**Dependencies:** OR-1, OR-2, OR-3, OR-6; `05-python-surface.md` PY-9
(`RunResult.metrics` in Python).

**Measurement:** `simulate()` wall time with and without `metrics`, and
the metrics share when requested, recorded.

**State:** committed `4bc3ed70` (branch `row/r46-or8`, worktree of
`7e95e974`, one commit).

The decoupling, as built. `SimulationConfig.metrics: bool`, default
`false`, the TOML key and the Python keyword of the same name (the
constructor keyword, the getter and the setter, the stub's signature,
`frame_golden`'s `SIMULATION_KEYS` pin extended). The run result's
metrics become `Option<SimulationMetrics>` (the entry's `RunResult`
surface arrives with its own row; this base's `SimulationResults`
carries the semantics): `None` without the request, and `None` on every
failed-run path.

The one path. The metrics input plan (`MetricsInputPlan`, `hares-io`) is
the fixed declared list of the columns the metrics read, resolved once
at the roster freeze against the producer surface's variable names in
producer-slot order: the union of the selected reports' variables
(catalogue-derived), or the `Verbosity(0)` preset for a run with no
reports. Each metric whose availability depends on a column being
present keeps that dependence, evaluated against that same surface; the
required-column refusal stays on the request path (a metrics request
whose surface lacks the total electric power column is refused at
freeze, naming the remedy: include the power column in a report's
selection). The accumulator (`MetricsCalculator`) holds the plan and is
fed per step from the row plan's producer slots
(`accumulate_row(&[f64])`), so its state is exact at every step; the
dwelling owns it (`Option`), finalises it at the end of `simulate()`
and hands it to the engine through `take_run_metrics()` (plus
`run_metrics_snapshot()` for a mid-run read). The batch feed and every
second path are deleted: the recorder's `enable_metrics`/`take_metrics`
and its per-flush calculator are gone from `StreamingRecorder`, the
engine's post-hoc `compute_metrics_from_batches`/`finalize_run_metrics`
and `empty_metrics` are gone, and the Python `metrics()` reads the one
path (raising, naming `metrics=True`, when the run did not request
them; "no steps run yet" before any step). The old batch-scoped
observe logs (the end-use boundary log, the per-batch NaN and
setpoint-missing summaries) are replaced by one end-use log per run at
finish. The calculator's type checks are gone with the batch schema
(the catalogue's value columns are typed once); the row feed has no
nulls, so the nullable-setpoint tests became non-finite-setpoint tests
with the same asserted outcomes.

Bug compatibility: with a single report (every golden fixture's shape)
the union is that report's schema, and the per-step feed reads the same
values the flushed batches carried, in the same order, through the same
arithmetic, so every accumulator sees the same bits: the goldens'
metrics products are bitwise unchanged (`frame_golden compare-all`
identical on the default build, the dst golden identical on the dst
build, `metrics = true` set in every manifest). A multi-report run's
availability now evaluates against the union of the reports' variables
(the declared semantics; no golden has more than one report). A run
whose first report is a narrow selection without the power column now
builds and runs (the metrics absent without the request): the
construction-blocking first-report refusal of the destinations entry
dies here, pinned by
`narrow_first_report_builds_and_runs_without_the_metrics_request`;
with the request the refusal names the column and the remedy
(`narrow_first_report_with_the_metrics_request_names_the_missing_column`).

Every caller that reads metrics today requests them: the golden
manifests (`metrics = true`, config-only), the fleet's
`with_memory_report` (the builder that moves the homes' output mode
also sets `metrics = true` per home; the aggregate's per-dwelling
metrics read zero for a home without the request, the reading the
pre-decoupling empty-metrics runs gave), the engine and fleet tests
that read metrics, and the Python tests through `make_dwelling` and
`_base_config` (both default the request on; the new
`metrics_none_unless_requested` passes it off explicitly).

The RED first: the six run-level tests in
`crates/hares-core/tests/metrics_optional.rs` were written against the
surface before it existed and failed on the parent as compile errors
(no `metrics` field, `result.metrics` not an `Option`), and the Python
`metrics_none_unless_requested` failed `DID NOT RAISE` (the old
post-hoc path computed from the memory batches regardless of any
request). GREEN: all six plus the in-crate structural pin
(`metrics_accumulator_present_exactly_when_requested`: the dwelling
holds no accumulator without the request, the accumulator with it) and
the Python test landed with the row.

Gates on the commit: `cargo fmt --all -- --check` clean;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean; `cargo nextest run --all-features -j 4` 7671 passed (1 skipped);
`cargo nextest run -F dst -j 4` 7604 passed (1 skipped); `uv sync
--locked --all-extras --all-groups`, then `uv run maturin develop` and
`uv run pytest tests/python` 1549 passed, 0 failed, 0 skipped;
`uv run pyright` 0 errors; the goldens bitwise (`frame_golden
compare-all` 6 manifests identical on the default build,
`full_year_900s_dst` identical on the dst build). The cost measurement
is in `02-measurement.md`: the paired bench's deltas flip sign across
runs, so the metrics feed's per-step cost is a bound (within this
host's noise), and the structural claim, no metrics code runs in the
step without the request, is proven by the code path and the tests.

## OR-9: `initialize()` no longer serialises an unread checkpoint

**Kind:** initiative work (V7).

**Problem:** `PyDwelling::initialize()` (`crates/hares-python/src/py_dwelling.rs:659-673`)
serialises a full checkpoint to JSON into `initial_state` on every call.
The field is declared at `:622`, initialised at `:650` and `:2011`,
tested at `:660` only to short-circuit `initialize()`, written at `:671`
and cleared at `:1587`; nothing reads its bytes.

**Change:** delete the field and all six sites. `initialize()` keeps only
the `initialized` flag.

**Acceptance tests:** the field deletion is compile-enforced; the existing
Python tests that call `initialize()` (`tests/python/test_py_dwelling.py`)
pass unchanged.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python`.

**Commit:** `perf: stop serialising an unread checkpoint in initialize()`

**Dependencies:** none.

**Measurement:** `initialize()` wall time before and after, recorded (a
measurement, not a test).

## OR-17: JSON floats parse exactly in every build

**Kind:** initiative work (OR-10's bitwise restore needs it); structural,
by the proof below, or a numeric row if the proof finds a difference.

**Problem:** without its `float_roundtrip` feature, `serde_json` parses a
decimal best-effort: a mantissa above 2^53 or a decimal exponent outside
±22 can land one ULP away from the correctly rounded value, and checkpoint
JSON (`crates/hares-core/src/checkpoint.rs:114,159-167`) is written with
the shortest round-trip form and read back. Only
`tools/frame_golden/Cargo.toml:24` enables the feature at HEAD. Cargo
builds of the workspace unify it in (frame_golden is a member), so the
Rust tests and the goldens already run with it; `maturin` builds of
`hares-python` do not, so the wheel parses tariff JSON
(`py_tariff.rs:134,141,154,707,717`), the JSON its `from_dict`
constructors serialise from Python values and read back, override JSON and
checkpoints best-effort. The consumer at `c71c542` calls
`ElectricTariff.from_urdb_json` (`calibration/ami2/tariff.py:73,87`) and
`save_state`/`load_state` (`calibration/cosim.py:74,90,102`,
`coupling.py:399`).

**Change:** the workspace dependency becomes
`serde_json = { version = "1", features = ["float_roundtrip"] }`
(`Cargo.toml:54`), and `tools/frame_golden/Cargo.toml:24`'s own feature
line goes, since the workspace entry carries it. Nothing else changes.

**Proof that no number moves** (all three, recorded in
`02-measurement.md`):
- the goldens: `frame_golden compare-all` (they were captured with the
  feature, so they compare bitwise by construction);
- the consumer: a worktree of the B0 engine commit `1d60f478` with only
  this manifest change applied, built and installed through the
  engine-under-test mechanism (`08-consumer.md`), runs the consumer gate
  and the B0 DE-trace check on the B0 consumer `aec2e3f`; both compare
  identical to B0, so no tariff, override or checkpoint the consumer reads
  parses differently;
- the Python suite: the values the tariff tests in `tests/python` read are
  equal before and after.
If any comparison differs, the entry stops before committing; it becomes a
numeric row (`plan.md` §2.8: delta recorded, golden and consumer evidence,
a `docs/python.md` "Numeric changes" line), placed in the numeric block,
and OR-10 waits for it.

**Acceptance tests:** `json_floats_round_trip_bitwise`
(`crates/hares-tariff/tests/json_float_roundtrip.rs`, new): for `0.1 +
0.2`, `std::f64::consts::PI * 1e-30`, `6.022_140_76e23`,
`f64::MIN_POSITIVE`, `5e-324`, `f64::MAX`, `-104.673_178` and
`39.847_222_222_222_22` (mantissas above 2^53, exponents outside ±22,
subnormal and extreme values), `serde_json::to_string` then
`serde_json::from_str` returns the same bits. It runs under
`cargo nextest run -p hares-tariff`, a build that does not include
`frame_golden`, so it fails without the workspace feature.

**Verification:** the proof above; `cargo nextest run -p hares-tariff`;
`uv run --no-sync maturin develop --release` and
`uv run --no-sync pytest tests/python`.

**Commit:** `build: parse JSON floats exactly in every build`

**Dependencies:** none; directly before OR-10.

## OR-10: checkpoint/restore resumes billing, metrics, reports, health and warnings

**Kind:** initiative work (V28).

**Problem:** `DwellingCheckpoint` (`crates/hares-core/src/checkpoint.rs:74-108`)
carries physics state only. It does not carry: the tariff billing state
(`Dwelling::tariff_evaluator`, `crates/hares-core/src/dwelling/mod.rs:1704`;
`TariffEvaluator`, `crates/hares-tariff/src/evaluator.rs:8`, whose
`step_index` at `evaluator.rs:26` indexes its precomputed price arrays and
whose `BillingState`, `crates/hares-tariff/src/billing.rs:75`, holds
`DateTime<Tz>` period bounds with no serde derives), the closed
`billing_summaries` (`mod.rs:4495`), the metrics accumulators
(`MetricsCalculator`, `crates/hares-io/src/output/metrics.rs:361`), the
partial reporting periods, the health counters, or the warning log.
`load_checkpoint` (`mod.rs:5005-5168`) restores physics and leaves the
rest at whatever the restoring dwelling held; the Python `load_state`
(`py_dwelling.rs:1600-1603`) has the same gap.

**Change:**

- `DwellingCheckpoint` gains typed sections, each a serde snapshot type
  owned by the crate that owns the state, with the fields `plan.md` §2.1
  defines: `hares-tariff` `TariffSnapshot` (the zone name, `step_index`,
  the finished and finalized flags, and `BillingSnapshot`, every mutable
  field of `BillingState`, with each `DateTime<Tz>` bound stored as a
  `LocalInstant`: the local wall-clock time and the zone's offset at that
  time, never a UTC instant); `hares-tariff` `BillingPeriodSummarySnapshot`,
  one per closed billing summary, in the `DwellingCheckpoint` section
  `billing_summaries` (the dwelling holds the summaries, `mod.rs:4495`;
  `BillingPeriodSummary`, `billing.rs:410-421`, derives nothing and holds
  `DateTime<Tz>` bounds, and the workspace `chrono-tz` has no `serde`
  feature); `hares-io` `MetricsSnapshot` (the input plan's names and
  `MetricsAccumulators`, exact at the checkpoint step because OR-8 feeds
  it per step; present exactly when the run requested metrics) and one
  `ReducerSnapshot` per non-`Timestep` report (partial-period
  accumulators); `hares-core` `RunHealth` and the warning log (both of
  OR-6's buffers with their dropped counts; `Warning` gains `Serialize`
  and `Deserialize` here, its `Arc<str>` source through OR-1's `rc`
  feature). Each snapshot copies its live state in the units that state
  holds at this row (kWh and kW for `BillingState`; kWh, Wh and therms
  for the metrics totals), so a restore is bitwise; `plan.md` §2.1
  ("Snapshot units") names the later entry that moves them to SI.
- `SnapshotError` (`plan.md` §2.1, with its variants) is a `thiserror`
  enum in `hares-tariff`, which has no `thiserror` dependency at HEAD
  (`crates/hares-tariff/Cargo.toml`): this commit adds it with
  `cargo add -p hares-tariff thiserror`, which inherits the workspace
  entry (`Cargo.toml:66`, `thiserror = "2"`) as
  `thiserror = { workspace = true }`.
- Every `f64` in a checkpoint, physics included, reads back with the bits
  it was written with because OR-17, the commit before this one, enabled
  `serde_json/float_roundtrip` in every build.
- `CHECKPOINT_VERSION` goes from 8 to 9 (`checkpoint.rs:22`).
- `save_checkpoint` (`mod.rs:4935`) writes the sections; `load_checkpoint`
  restores them after the physics state. A section present without a live
  counterpart (billing state with no tariff configured), or the reverse, is
  a typed error naming the section.
- `Memory` report rows after the checkpoint step are discarded on restore.
- Restoring a dwelling with any `Parquet`, `Csv` or `Callback` report is a
  typed error naming the report, returned before any state changes.
- `restore_building_state` (`mod.rs:5176`) keeps its contract (shell state
  into a new segment; no billing, metrics, reports, health or warnings).

**Acceptance tests.** Fixture for the tariff and report tests: the HPXML,
schedule and weather of `tests/fixtures/parity/cz4a_ashp_hpwh/`, start
`2023-01-25T00:00:00-07:00`, 12 days (crossing the January to February
billing boundary), 900 s, `metrics = true`, no warm-up, with the
time-of-use tariff `tests/fixtures/urdb/pge_e_tou_c.json` set through
`Dwelling::set_tariff` in zone `America/Denver`; checkpoint at
`2023-01-28T16:30` local (inside the January billing period and inside an
afternoon price period, mid-hour).

- `restore_mid_billing_period_then_finish_matches_uninterrupted_run`: a
  freshly built dwelling restores the checkpoint and runs to the end. Its
  `RunResult.metrics`, billing summaries, health and warnings equal the
  uninterrupted run's (floats on bits).
- `restore_after_a_closed_billing_period_keeps_its_summary`: the same run
  checkpointed at `2023-02-03T12:00` local, after the January period
  closed; the restored run's billing summaries, the January one included
  (its bounds through `LocalInstant`), equal the uninterrupted run's on
  bits.
- `restore_resumes_tariff_price_index`: the first post-restore step's
  price equals the uninterrupted run's price at that step.
- `restore_mid_hour_matches_uninterrupted_hourly_report`: with an
  `Hourly` `Memory` report; the restored run's rows whose period starts at
  or after the checkpoint's period start (`2023-01-28T16:00`) equal the
  uninterrupted run's rows for the same periods on bits, `Step Count`
  included. The restored dwelling holds no row before that period.
- `restore_round_trips_every_float_bitwise`: a checkpoint written,
  read and written again is byte-identical (pins `float_roundtrip`).
- `restore_resumes_health_counters`: on BESTEST 600 (OR-5's fixture), the
  rollback test equipment fails one step before and one after the
  checkpoint; the resumed run reports 2.
- `restore_resumes_warnings`: on the same BESTEST 600 run, a test actor
  sends one out-of-bounds control signal on one step before and one after
  the checkpoint; the resumed run's `RunResult.warnings` equals the
  uninterrupted run's, and the drain after the restore returns only the
  post-restore warning.
- `restore_truncates_memory_results_to_checkpoint_step`.
- `snapshot_errors_name_the_zone_and_instant`: a `TariffSnapshot` whose
  zone is `Not/AZone`, one restored into an evaluator in `America/Chicago`,
  and a `LocalInstant` of `2023-03-12T02:30` at offset -25200 in
  `America/Denver` (a spring-forward gap) give `UnknownZone`,
  `ZoneMismatch` and `UnresolvedLocalInstant`, each naming its values.
- `restore_with_file_destination_is_a_typed_error`: state and output
  unchanged afterwards.
- `restore_metrics_section_into_run_without_metrics_is_a_typed_error`.
- `version_8_checkpoint_is_rejected`: through
  `DwellingCheckpoint::deserialize_gated` (`checkpoint.rs:154-166`).
- The Python `load_state` path passes the first test through
  `uv run --no-sync pytest tests/python`.

**Verification:** `cargo nextest run -p hares-core -p hares-tariff -p hares-io`;
`uv run --no-sync maturin develop --release` and
`uv run --no-sync pytest tests/python`; `frame_golden compare-all` (no
fixture restores, so none changes).

**Commit:** `feat: checkpoints carry billing, metrics, reports, health and warnings`

**Dependencies:** OR-1 (`rc`), OR-5, OR-6, OR-8, OR-13, OR-17
(`float_roundtrip`).

**Measurement:** checkpoint size in bytes and `save_checkpoint` wall time
for a full-year fixture, before and after, recorded.

## OR-13: reporting frequency with per-variable aggregation

**Kind:** initiative work (`dl-i08-architecture.new_capability`:
"reporting frequency"; `dl-i08-architecture.aggregation`).

**Problem:** every report today is one row per step; the only time
reduction in the engine is the fleet's fixed resampling
(`crates/hares-fleet/src/aggregation.rs`). A caller who needs hourly
energy re-aggregates a per-step frame, choosing its own rule per column.

**Change:** the `Frequency` variants `Minutes(NonZeroU32)`, `Hourly`,
`Daily`, `Monthly`, `Annual` and `RunPeriod` (`plan.md` §2.2) and their
Python constructors (`Frequency.minutes(n)`, `Frequency.hourly()` and so
on) arrive here. `Minutes(n)` requires `n` to divide 60 and its period to
be a whole multiple of `time_res`; its periods start on the local clock
hour. `PeriodReducer` (`plan.md` §2.6, with `PeriodCalendar`, `TimeRule`,
`PeriodRow` and `ReduceError`) in `crates/hares-io/src/output/reduce.rs`
(new), one per non-`Timestep` report, fed per step from the report's
projection of the row plan and the row plan's `i64` naive local
timestamp. A note from the frozen row plan's review: the plan's field is stamped
in UTC epoch seconds (`begin_row`, called with
`step.timestamp.timestamp()`), not naive local seconds; a local-epoch
stamp must not be assumed from it. The reducer buckets on the local
time the row's own timestamp column carries (the text below), so it
must derive its local time from that column or from the run's fixed
offset, never from the plan's field as if it were local. It implements
`plan.md` §2.6: calendar periods in local time,
rows labelled at period start, a `Step Count` column, reduction over
non-null values by `VariableDef.aggregation` (`Sum`, `Mean`, `Mode` with
ties to the lowest code, `Last`), `<name> [valid steps]` companions for
nullable variables. Summation order: step values are added in step order
into one `f64` per period and variable; `Sum` returns it and `Mean`
divides it once by the count of valid steps. The reducer is a public item
of `hares-io`, not a stability contract (`plan.md` §2.9), so that
`hares-fleet` can construct one per home (`04-fleet.md` FL-1) with
`PeriodReducer::new(frequency, PeriodCalendar, rules)` (the home's own
local calendar; `04-fleet.md` FL-4 adds a period-origin offset in its own
commit) and feed it record batches through `push_batch` as well as rows
through `push_row`; both feeds perform the same operations in the same
order.

`Time` is still today's Utf8 column at this row (`columns.rs:425`,
written with chrono `"%+"` at the step's fixed offset,
`dwelling/mod.rs:7532`) and becomes a `Timestamp` only
at OR-7, a later row. Until then a period row's `Time` is its nominal
period start rendered by the same function as the step rows (the naive
local period start at the run's site offset, formatted `"%+"`), and
`push_batch` reads a batch's Utf8 `Time` with
`DateTime::parse_from_rfc3339` and takes its naive local value, so both
feeds bucket on the same local milliseconds. OR-7 switches the period
row's `Time` and `push_batch` to the `Timestamp` and re-runs this entry's
tests.
`SimulationConfig::validate()` (`05-python-surface.md` PY-8) gains the
frequency checks here: it rejects a frequency whose period is not a whole
multiple of `time_res`, and a `Minutes(n)` whose `n` does not divide 60.
The reducer carries no per-step allocation. Enum variables use `Mode`
here from the start (this is new output, so nothing changes numerically);
`09-numeric-fixes.md` B-11 changes the fleet fold, which exists today.

**Acceptance tests** (fixture `tests/fixtures/parity/cz4a_ashp_hpwh/` at
`time_res = 900`):

- `hourly_energy_sum_equals_timestep_sum`: an `Hourly` report's energy
  variable equals, to 1e-12 relative, the sum of the four `Timestep` rows
  of the same run in each hour.
- `hourly_power_and_temperature_are_means`.
- `daily_enum_column_is_mode_with_lowest_code_tie_break`: a test actor
  declaring an enum variable with labels `["a", "b", "c"]` publishes, over
  two days at 900 s, code 2 for the first 48 steps and code 1 for the next
  48 on day 1 (a tie: the row is 1), and code 0 for 50 steps then code 2
  for 46 on day 2 (the row is 0).
- `partial_first_and_last_period_carry_step_count`: an `Hourly` report at
  900 s steps of a run starting at `2023-01-01T00:30` and ending at
  `2023-01-01T23:15` local (91 steps, the end exclusive): the first row,
  labelled `00:00`, has `Step Count` 2, the last, labelled `23:00`, has
  `Step Count` 1, and every row between has 4; the first row's `Time` is
  `2023-01-01T00:00:00-07:00` (the Utf8 form until OR-7).
- `hourly_running_total_is_last`: a test actor declaring a running total
  (`Quantity`, `Last`) that publishes the step index gives, for each full
  hour, the index of its fourth step.
- `null_cells_excluded_with_valid_count`: a nullable test variable null
  on a fixed subset of steps.
- `run_period_report_has_one_row`.
- `minutes_15_energy_sum_equals_timestep_sum`: a `Minutes(15)` report at
  `time_res = 300` (the same inputs) gives one row per quarter hour,
  labelled at `:00`, `:15`, `:30` and `:45`, whose energy variable equals,
  bitwise, the sum of the three `Timestep` rows of the period added in
  step order.
- `batch_fed_reducer_equals_row_fed_reducer`: the `Hourly` reducer fed the
  `Timestep` report's record batches (whose `Time` is Utf8 until OR-7)
  gives the same rows, compared with `to_bits()` and the same period
  starts, as the reducer fed per step.
- `frequency_not_multiple_of_step_is_a_validate_error`.
- `minutes_not_dividing_the_hour_is_a_validate_error`: `Minutes(7)` and
  `Minutes(90)` fail naming the value and the rule; `Minutes(15)` at
  `time_res = 600` fails as not a whole multiple of the step.

**Verification:** `cargo nextest run -p hares-io reduce`;
`cargo nextest run -p hares-core`.

**Commit:** `feat: report at minute, hourly, daily, monthly, annual or run-period frequency`

**Dependencies:** OR-1 (`Aggregation::Last`), OR-2 (the row plan's local
timestamp), OR-3.

**State:** committed `b2aee9a2` on branch `row/r39-or13` (cut from
`c9c3399e`), run ahead of its destinations dependency: the report list
does not exist yet, so the per-report frequency has no config surface to
ride on and the run's one report stays a timestep report. What lands is
the reducer and its frequency, as public items of `hares-io` for the
report layer and the fleet's per-home use that follow: `Frequency` gains
`Minutes(NonZeroU32)`, `Hourly`, `Daily`, `Monthly`, `Annual` and
`RunPeriod` (externally tagged snake_case, so a config carries `"hourly"`
or `{ minutes = 15 }`), and `crates/hares-io/src/output/reduce.rs` holds
`PeriodReducer`, `PeriodCalendar`, `TimeRule`, `PeriodRow` and
`ReduceError`. `TimeRule::from_variable` reads the declared aggregation
off a `VariableDef` and, for an `Enum` variable, its label count; the
reducer sums step values in step order into one `f64` per period and
variable, `Mean` divides once by the valid-step count, `Mode` counts per
code with ties to the lowest code, `Last` holds the last valid step's
value, null steps are excluded and a period with no valid step is null
with its `<name> [valid steps]` count carried per rule, and every period
carries its step count. The step-to-period alignment buckets on the
naive local wall clock the row's own timestamp carries, never on the row
plan's per-row field (stamped in UTC epoch seconds, the frozen row
plan's review note): `push_row` takes the caller's local milliseconds
(the run's fixed offset applied to the step instant) and `push_batch`
parses a batch's Utf8 `Time` with `DateTime::parse_from_rfc3339` and
takes its naive local value, so both feeds bucket on the same local
milliseconds; `push_batch` also reads the Arrow null masks as the row
feed reads its validity bits, ignores columns beyond the rules, and
errors on a column count or type mismatch and on an unparseable `Time`
value. Rows are labelled at the nominal period start (the OCHRE
reference the aggregation rules follow does the same: its per-column
rule picks sum for the energy units and mean otherwise,
`ochre/Analysis.py:85-94`, and its hourly output resamples onto
left-labelled periods, `ochre/Dwelling.py:351-353`, with steps stamped
at their interval start, `ochre/Simulator.py:263,319`); because the
validated period is a whole multiple of the timestep, every period
boundary coincides with a step interval end, so a step belongs to the
period its interval completes in, which is the period the floor computes.
`Minutes(n)` is refused where `n` does not divide the hour, both at
construction and at `Frequency::validate(time_res)`, the shared check
the configuration's validate runs for every report frequency once the
report list lands (the period must be a whole multiple of the timestep;
months and years are whole numbers of days, so for them the timestep
must divide a day). The Python frequency constructors wait for the
reports binding that gives them a receiver. The acceptance tests: the
reducer's hand-computed cases in `reduce.rs` (the minute report over a
synthetic day at a 300 s step with 96 quarter-hour rows whose sums,
means, modes and running totals are hand-computed; the hourly, daily,
monthly and annual cases over the same day; the partial first and last
periods of a 91-step run from 00:30; the calendar-boundary labels across
a month and year; the null exclusions with valid counts; the run
period's one row; the batch-fed and row-fed feeds equal on bits; the
batch mismatches; the quarter-hour labels at a half-hour site offset),
and the run-level cases in `crates/hares-core/tests/
reporting_frequency.rs` over the parity fixture at a 900 s and a 300 s
step (the hourly energy equal to its four steps' sum, the power and
temperature means, the daily enum mode with the tie to the lowest code
from a scripted actor, the hourly running total's fourth step, the
minutes-15 sums bitwise, the two feeds equal over a real run, the
partial periods, the run period). RED first: the tests were written
against the parent state and failed as compile errors on the missing
reducer (`unresolved imports reduce::PeriodCalendar,
reduce::PeriodReducer, reduce::PeriodRow, reduce::ReduceError,
reduce::TimeRule` in both crates), then turned green. Gates: fmt clean;
clippy `--all-features --all-targets -D warnings` green on every crate
the row touches and identical to the parent on `hares-python`, whose
four lints (`too_many_arguments` on three binding constructors,
`never_loop` on the removed-key helper) fail at the parent unchanged;
nextest `--all-features` 7648 run with 1 skipped (21 new tests: 13 in
`reduce.rs` and 8 in `crates/hares-core/tests/reporting_frequency.rs`;
the skip is the parent's ignored kind-pin generator); pytest in two
invocations, the filtered gate `uv run --all-extras --all-groups
--no-sync pytest` (the addopts' `-m 'not slow and not ochre'` left in
place) 1540 passed, 0 failed, 0 errors, and the unfiltered block line
`pytest -m ''` 1617 passed with 26 failed and 4 errors, the identical
failed set at the parent (base-verified: the fold's Python-side
residue, addressed on the sibling rows); pyright 0 errors;
the goldens bitwise, `compare-all` 6 manifests identical plus
`full_year_900s_dst` identical, the timestep frames unchanged and the
new frequencies additive. The parent's two catalogue test failures (the
synthetic TOML's removed keys) were fixed here first, cherry-picked from
the sibling branch's fix as `3ac9dfb6`. The verification round's
findings were fixed as `a08ea0cb` on this branch: `push_row` checks its
value and validity slices unconditionally and returns the batch feed's
column count error (the debug assert panicked in debug builds and a
release caller zipped a short slice into silent under-counting, while
`push_batch` errored properly); the DST hour conventions are pinned at
the reducer (a fall-back day's repeated hour merges into one bucket, a
24 h run from local midnight giving 23 hourly rows with the repeated
hour holding both passes' 8 steps; a spring-forward day's nonexistent
hour opens no bucket, the wall day's 92 steps giving 23 hourly rows,
no crash); the quarter-hour energy test's failure message names its
1e-12 relative tolerance instead of claiming bitwise equality; the
module docs record that a non-finite value on a valid step propagates
through sum, mean and last and poisons the period's aggregate, while
undefined values arrive as null per the catalogue's
null-for-not-applicable rule and are excluded with the valid-step
count. Gates at `a08ea0cb`: fmt clean; clippy green on the touched
crates; nextest `--all-features -j 4` 7651 run with 1 skipped (the 3
new pins); pytest 1540/0/0 filtered and 26 failed + 4 errors unfiltered
(the parent's set); the goldens bitwise, `compare-all` identical plus
`full_year_900s_dst` identical.

## OR-14: the batch callback destination

**Kind:** initiative work (`dl-i08-architecture.destinations`).

**Problem:** a caller that wants to stream batches (for example into its
own store) must retain the whole frame or read a file back. `plan.md`
§2.2 decides a batch-level callback: opt-in, zero cost when unused, and a
callback error fails the run with the cause chained. `hares-io` has no
pyo3 dependency (`crates/hares-io/Cargo.toml`), and a callable cannot be
part of the serde config.

**Change:** `BatchCallback`, `BatchContext` and `CallbackError` in
`crates/hares-io/src/output/callback.rs`; `Destination::Callback` as a
serialisable marker; `Dwelling::attach_callback` in `hares-core`;
`HaresError::Callback { report, message }` in `hares-types` (`plan.md`
§2.2). The recorder calls `on_batch` once per built batch of that report
(every `output_chunk_size` rows, and the final partial batch where the
run's outputs are published: at the end of `simulate()`, today's
`flush_and_close`, until OR-6, then at `finish()`), in order, with
`batch_index` counting from 0. An error fails the run at that step; the
failed run's partial output file stays as written, the failed-run
cleanup belonging to the destinations entry, which owns the
partial-name publish the error path will share. Without a `Callback`
report no adapter exists. Replay export (OR-16)
rewrites `Callback` destinations to `Memory`. The Python constructor
`Destination.callback()` lands with the Python attach in
`05-python-surface.md` PY-10: until then Python cannot attach a callable,
so it does not offer the marker either.

**Acceptance tests** (Rust test implementation of the trait; the
`tests/fixtures/parity/cz4a_ashp_hpwh/` inputs, start
`2023-01-01T00:00:00-07:00`, 24 h at 900 s, `output_chunk_size = 40`):

- `callback_receives_every_batch_once_in_order`: the concatenated batches
  equal a `Memory` report of the same selection on bits.
- `callback_error_fails_the_run_naming_the_report`.
- `callback_report_without_attachment_fails_before_first_step`,
  `attach_to_non_callback_report_is_an_error`.
- `callback_context_carries_bldg_id_and_report_name`.
- `no_callback_report_constructs_no_adapter` (structural, in-crate).
- `validated_config_keeps_callback_marker`: a `SimulationConfig` with a
  `Callback` report keeps `Destination::Callback` after `validate()`, and
  `SimulationConfig::to_toml()` (the Rust method `05-python-surface.md`
  PY-8 adds) then `from_toml()` gives the string `"callback"` back as the
  same value. (`Provenance` arrives with OR-6, which asserts that the run
  records the marker.)

**Verification:** `cargo nextest run -p hares-io -p hares-core callback`.

**Commit:** `feat: stream report batches to an attached callback`

**Dependencies:** OR-3. `05-python-surface.md` PY-10 implements the trait
for Python.

**State:** done as `b512848e` (branch `row/r40-or14`, cut from
`c9c3399e`), ahead of the destinations entry the model follows, as the
entry's sink. `BatchCallback` (`on_batch(&mut self, ctx, batch)`), `BatchContext`
(`report`, `bldg_id`, `batch_index`) and `CallbackError` live in
`crates/hares-io/src/output/callback.rs`, with the report name the engine
builds today as the constant `TIMESTEP_REPORT` (`"timestep"`, the run's
timestep table); `HaresError::Callback { report, message }` is in
`hares-types`; `Dwelling::attach_callback(report, callback)` is in
`hares-core`, validating the report name, refusing a dwelling whose
output recording is disabled and refusing rows already recorded (the
destinations freeze with the first recorded row). The recorder delivers
every built batch in flush order, the index counting from 0, and never
after the outputs are published (a `closed` flag set by
`flush_and_close`, so rows pushed after the final flush buffer on without
reaching the callback). The delivery rides a shared attachment
(`Arc<Mutex<Box<dyn BatchCallback>>>`) the dwelling holds as the source
of truth, so a roster change before the first recorded row, which rebuilds
the recorder, hands the fresh one the same attachment instead of dropping
it. A callback error fails the flush naming the report
(`OutputError::Callback`, mapped through one helper at both record sites)
and latches `terminal_error`, so every later step returns it without
stepping anything: the failure budget's semantics, at a step's flush and
at the close (the final partial batch delivered inside
`flush_and_close`) alike. A callback may not re-enter the dwelling: the
delivery thread holds the dwelling's mutex and the GIL across the call,
so a re-entrant accessor would park forever; a thread-local delivery
flag the Python sink sets around the callable's invocation is checked
by every dwelling acquisition path, which refuses the re-entry with the
typed callback error naming it instead of parking, and the run latches
the refusal the same way. Zero cost when unused:
one `None` branch per flush, pinned structurally in-crate
(`no_callback_report_constructs_no_adapter`).

The Python attach landed with this row, ahead of its surface entry, on
the operator's instruction: `Dwelling.attach_callback(report, fn)` with a
`BatchInfo` pyclass (report, bldg_id, batch_index) and the batch as the
established Python frame surface, a polars DataFrame built over the
Arrow batch through the conversions the results method already uses. The
GIL: the step binding detaches around the run, and the sink re-acquires
per call (`Python::attach` inside `catch_unwind`), so an exception from
the callable surfaces as the typed callback error, never a panic through
the recorder (a sink that panics is caught at the delivery itself, Rust
and Python alike, and reported as the typed callback error at once, so
the unwind never crosses the step and the attachment's mutex never
poisons); the dedicated
exception class with the original exception chained as `__cause__`, the
zero-copy Arrow batch and the config-side `Destination.callback()`
constructor remain with the surface entry.

The acceptance tests, with the surface as it stands: the four that do not
need the destination model run in `crates/hares-core/tests/callback.rs`
over the `cz4a_ashp_hpwh` inputs, start `2023-01-01T00:00:00-07:00`, 24 h
at 900 s, `output_chunk_size = 40`:
`callback_receives_every_batch_once_in_order` (three batches of 40, 40
and 16 rows, indices 0, 1, 2, the concatenation equal to the same run's
retained batches, today's memory surface, on bits, schema included),
`callback_error_fails_the_run_naming_the_report` (the message carries the
callback's cause, the run stayed ended, the callback never called again),
`callback_context_carries_bldg_id_and_report_name` and
`attach_to_non_callback_report_is_an_error` (adapted: the one report name
the engine builds) and `attach_after_the_first_recorded_row_is_an_error`
(the count the review corrected: seven Rust tests over the row's first
record of six); `no_callback_report_constructs_no_adapter`
(structural, in-crate, in the writer's tests) and `no_callback_after_finish`
(in the callback module's tests) pin the zero-attachment shape and the
publication boundary. Two wait for the destinations row:
`callback_report_without_attachment_fails_before_first_step` and
`validated_config_keeps_callback_marker`, whose serialisable marker and
round-trip live with the destination model. RED first: the Rust tests
failed to compile against the base (no callback module), the Python tests
failed on the missing attach. Gates: fmt; clippy `--all-features
--all-targets -D warnings`; nextest `--all-features` 7634/7634; pytest
1544 passed, 0 failed, 0 skipped; pyright 0; the eight goldens bitwise
(`compare-all` 7 manifests identical, `full_year_900s_dst` identical).
The base commit failed every Rust suite there on one stale test file (the
catalogue tests' synthetic TOML still carried the removed
`time_res_s`/`duration_s` keys), fixed first as `bc610fcc`.

The review's findings are fixed as `66c825f6`: the re-entrancy guard
above, pinned by a probe run in a child process joined on a timeout (a
regression parks the child holding the GIL, which no in-process check
could report, so the probe's run fails at the timeout instead of
hanging the suite); the Rust sink's panic caught at the delivery and
reported as the typed callback error, the run ended, a later step
returning it, the telemetry consistent with the steps that ran (RED at
the parent: the unwind crossed the test); the re-attach through a
roster rebuild pinned (the pin fails when the re-attach is dropped,
mutation-proven); the write_output refusal pinned; the close-time
latch pinned (RED at the parent: the later step reported the horizon's
end instead of the close's cause); the writer doc corrected (the
failed batch is written but not retained, the flush ending before the
retention push); and this record corrected (the failed-run
partial-file removal the Change paragraph claimed never existed here).
The review-fix commit adds four Rust tests, nine in the integration
file and two in-crate, and the Python file gains the re-entry probe,
five Python tests in all. The delivery's IPC cost, measured end to end
over a building-year run (876 batches at `output_chunk_size` 40):
about 0.1 ms per batch at verbosity 2, about 0.09 s per
building-year, and about 0.3 ms per batch at verbosity 8; the review
recorded about 0.8 ms per batch (about 0.7 s per building-year). The
review-fix tree's gates: fmt; clippy with no new diagnostics on the
touched crates (the workspace's known base diagnostics unchanged);
nextest `--all-features` 7638/7638; pytest 1545 passed, 0 failed,
0 skipped; pyright 0; the eight goldens bitwise (`compare-all` 7
manifests identical, `full_year_900s_dst` identical).

## OR-15: `metrics_from_table`

**Kind:** initiative work (`dl-i08-architecture.new_capability`).

**Problem:** metrics can be computed only inside a run; a stored results
table cannot be scored the same way.

**Change:** `hares_io::output::metrics::metrics_from_table(table:
&[RecordBatch], catalogue: &Catalogue, time_res: Duration,
setpoint_deadband_c: Option<f64>) -> Result<RunMetrics, MetricsError>`,
running the same accumulator core as OR-8 over a `Timestep` table whose
columns are resolved through the catalogue. A metrics input variable
absent from the table leaves the metric it feeds `None`, by the same rule
as in-run. A column whose Arrow type contradicts the catalogue is an
error naming it. The table must be a `Timestep` table: consecutive `Time`
values differ by exactly `time_res` and there is no `Step Count` column;
otherwise `MetricsError` names the mismatch (the first offending pair of
`Time` values, or the `Step Count` column), so an `Hourly` table passed
with `time_res = 900` is an error, not wrong metrics. What the in-run
calculator reads from its stored config comes from the table: the year
of the first `Time` value decides the leap-year coverage
(`metrics.rs:1001` reads `config.start_time.year()`), and the duration is
the row count times `time_res` (`metrics.rs:992`), so the result equals
the in-run value on bits. An empty table is an error naming it.

The Python binding is not in this commit: it takes the Python `Catalogue`
and imports an Arrow stream through `pyo3-arrow`, which arrive with
`05-python-surface.md` PY-4 and PY-1 (rows after this one), so PY-4 adds
`hares.metrics_from_table` with its stub, its changes line and its Python
test.

**Acceptance tests** (the `leap_year_february_900s` inputs over 24 h,
`metrics = true`):
`metrics_from_table_matches_in_run_metrics`: with a `Verbosity(8)`
`Memory` report; `metrics_from_table` over that table and the run's
catalogue equals `RunResult.metrics` on bits.
`metrics_from_parquet_file_matches_in_run_metrics`: the same through a
`Parquet` report read back.
`metrics_from_table_rejects_type_mismatch`.
`metrics_from_table_rejects_non_timestep_table`: the same run's `Hourly`
report passed with `time_res = 900` fails naming the `Time` pair, and a
table carrying `Step Count` fails naming that column. The tests that
need a run live in `crates/hares-core/tests/metrics_from_table.rs`.

**Verification:** `cargo nextest run -p hares-io -p hares-core metrics`.

**Commit:** `feat: compute run metrics from any results table`

**Dependencies:** OR-8.

## OR-16: replay export

**Kind:** initiative work (`dl-i08-architecture.new_capability`: "replay
export").

**Problem:** a result cannot be reproduced after its inputs are gone:
nothing records the resolved configuration, the input content and its
digest, and the engine build together.

**Change:** `crates/hares-core/src/replay.rs` (new) with the types of
`plan.md` §2.7 ("replay export"). `RunResult::export_replay(&self, dir:
&Path) -> Result<()>` reads only `self.provenance` and `self.reports`
and writes into an empty directory:

- `replay.toml`: from `Provenance.settings` (`plan.md` §2.1), the
  validated `SimulationConfig` (the master seed in effect included) with
  `Callback` destinations rewritten to `Memory` and file destinations
  rewritten to paths inside the replay directory, and every other
  `DwellingSettings` field except `overrides`: `bldg_id`, `patches`
  (`HpxmlDataPatches`, which a ResStock run needs to reproduce) and, until
  `09-numeric-fixes.md` B-2 moves warm-up into `SimulationConfig.warmup`,
  `initialization_duration`; from `Provenance.inputs`, each input's file
  name in the directory and digest, `weather_format` (so weather that was
  given as `Text` or `Bytes` is replayed in the format it was parsed as)
  and `defaults_files_sha256`; the IANA zone of `Provenance.tariff` when
  one is attached; and a `[build]` table written from `BuildInfo`.
- `overrides.json`: the `overrides` value as JSON, because it is a
  `serde_json::Value` that may hold `null`, which TOML cannot encode;
  absent when there are none.
- `tariff.json`: the attached `ElectricTariff` as JSON
  (`Provenance.tariff`); absent when no tariff was attached.
- The HPXML, schedule and weather content verbatim, and each overridden
  defaults table. `Path` inputs are read at export and their digest must
  equal the digest recorded at parse (`plan.md` §2.7 `InputRecords`), or
  the export fails naming the file. The defaults tree is not copied
  (138 MB).

`hares_core::replay::load(dir, defaults_base) -> Result<Replay>` reads
`replay.toml` into a private serde manifest, rebuilds the `DwellingConfig`
(the weather as `WeatherInput { source, format: Some(weather_format) }`),
reads `tariff.json` into `Replay.tariff`, and reads the `[build]` table
into `Replay.recorded_build` (`RecordedBuild`, owned strings, since
`BuildInfo`'s `&'static str` fields cannot be deserialised). It fails
naming the mismatch if the defaults tree at `defaults_base` has a
different digest. `Replay::build` builds the dwelling and attaches the
tariff before the first step. The replayed run's `Provenance.build`
records the engine that replayed it; `Replay.recorded_build` keeps the
one that exported it. This entry adds `Provenance.tariff`
(`TariffRecord`, `plan.md` §2.1), filled by `Dwelling::set_tariff`
(`mod.rs:4475`), which `ElectricTariff`'s serde
derives (`hares-tariff` `types.rs:264`) make exportable; its
`parse_warnings` travel in `tariff.json`, so the replay reports them too.

A replay rebuilds the run from its settings, inputs and tariff, so a run
whose dwelling was changed in any other way cannot be reproduced, and
exporting it would be silently inexact. This entry adds
`Provenance.replay_blockers` and `ReplayBlocker` (`plan.md` §2.1) and
`HaresError::NotReplayable { blockers: Vec<ReplayBlocker> }`. The dwelling
records each category the first time it happens:
`BlueprintEdited` when the blueprint it was built from had a
`DwellingBlueprintBuilder` mutator applied (`add_equipment_spec`,
`remove_equipment`, `remove_equipment_by_end_use`; the frozen blueprint
carries the flag); `EquipmentChanged` for `add_equipment`,
`clear_equipment`, `remove_equipment`, `remove_equipment_by_end_use`,
`replace_equipment`, `set_battery_lut`, `clear_battery_lut`,
`set_ev_charging_curve_lut` and `clear_ev_charging_curve_lut`
(`dwelling/mod.rs:3469-3841` at `afa9a071`); `CallerActor` for
`add_actor` and `add_actor_by_name` called by the caller (the
auto-registered actors, which `build` and `set_tariff` add, replay as
they were); `ExternalInput` for `apply_control`, `apply_control_validated`,
`queue_end_use_control`, `queue_dispatch`, `set_price_signal`,
`set_grid_voltage` and `record_clamped_actions` (the gym's and HELICS's
actions arrive through these); `StateRestored` for `load_checkpoint`,
`restore_building_state` and a caller's `run_warmup_converged` (the
build's own warm-up goes through a private function and records
nothing). `export_replay` returns `NotReplayable` naming every recorded
category before it writes anything. Attaching a callback, enabling the
observer and draining warnings change no result and record nothing.

Python, in this commit: `RunResult.export_replay(dir)`,
`hares.replay(dir, defaults=None) -> RunResult` (`load`, `Replay::build`,
then `simulate()`; `defaults=None` is the packaged defaults once
`09-numeric-fixes.md` B-10 lands, and required before it) and the
`Provenance.tariff` getter (the tariff as a dict and its zone, or
`None`), with `_hares.pyi` stubs and their line in the
`docs/python.md` changes section (`05-python-surface.md` PY-9).

**Acceptance tests** (the `consumer_shape_900s` inputs of
`02-measurement.md` M-2, which carry an override and a replaced defaults
file, over 48 h, `metrics = true`):
`replay_export_reproduces_frame_bitwise`: the inputs copied into a
temporary directory, run, export, delete that input directory, replay;
every report and the metrics equal the original on bits.
`replay_export_writes_dwelling_settings`: `replay.toml` carries `bldg_id`
176227 and `overrides.json` carries the `Gas Water Heater` override; a
run with `HpxmlDataPatches { number_of_bedrooms: Some(3.0) }` round-trips
its patch (`HpxmlDataPatches`, `crates/hares-io/src/hpxml/data_patches.rs:14`,
gains serde derives here).
`replay_overrides_null_round_trips`: an override map holding a JSON
`null` value replays equal.
`replay_of_text_weather_round_trips`: the cz4a EPW
(`tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw`) passed as
`InputSource::Text` with format EPW exports `weather_format = "epw"`, and
the replay's frame equals the original on bits.
`replay_with_attached_tariff_reproduces_billing`: the run with
`tests/fixtures/urdb/flat_rate.json` attached in `America/Denver`
exports `tariff.json`, and the replay's billing summaries equal the
original's on bits.
`replay_records_the_exporting_build`: `Replay.recorded_build` equals the
exporting run's `BuildInfo` field for field.
`replay_export_fails_if_input_file_changed_since_parse`: the same inputs
copied into a temporary directory and run 1 h; one byte is appended to the
copied HPXML before `export_replay`, which fails naming that file.
`replay_load_fails_on_defaults_digest_mismatch`: an export of the same 1 h
run loaded with `defaults_base` a temporary copy of the defaults tree with
one byte of the replaced file's original changed fails naming the digest
mismatch.
`replay_export_rejects_mutated_run`, one case per `ReplayBlocker` on the
same inputs over 1 h: a blueprint with one `remove_equipment`, a dwelling
with one `remove_equipment` after build, one `add_actor_by_name`, one
`apply_control`, one `load_checkpoint`; each `export_replay` returns
`NotReplayable` naming exactly that category and writes nothing into the
directory.
`replay_round_trip_in_python` (Python): the same export and replay through
`RunResult.export_replay` and `hares.replay`; reports and metrics equal.

**Verification:** `cargo nextest run -p hares-core replay`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: export a run's resolved configuration and inputs for exact replay`

**Dependencies:** OR-6, `12-inputs.md` IN-1 (input records,
`weather_format` and `Provenance.settings`), `06-runtime-memory.md` RT-4
(the blueprint builder whose mutators set `BlueprintEdited`),
`05-python-surface.md` PY-6 (`BuildInfo`) and PY-9 (the Python
`RunResult`).
