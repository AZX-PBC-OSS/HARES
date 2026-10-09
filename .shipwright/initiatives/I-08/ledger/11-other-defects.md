# Other confirmed defects

Confirmed defects outside I-08's purpose (runtime, telemetry and output
reporting, memory, performance) that no area above fixes. Each is its own
commit. OD-2, OD-3, OD-10 (its first commit), OD-17, OD-18, OD-19, OD-21,
OD-22, OD-24 and OD-25 turn silent paths into errors or counted events;
each is verified with `frame_golden compare-all`, and any golden
difference means a fixture relied on a silent path and fails the entry
until explained. OD-11 and OD-12 repair test tooling that hid an error or
rewrote committed fixtures. OD-13 and OD-14 make the permanently skipped
tests run and lift the dependency cap they needed. OD-15 triages energy a
seasonal test found booked to an end use the season does not explain.
OD-23 deletes a dead legacy file no loader reads. OD-26 corrects a
Python-only builder against the catalogue and the config-seeded path. OD-27
corrects a Python stub's claimed default against its binding. OD-9,
OD-2's site 6, OD-10's second commit and OD-20 move numbers and have rows
in the numeric block.

Three reviewed defects need no entry here (each test cited by its `fn`
line at `afa9a071`): V31 (below-grade node order) is
already pinned by `from_elements_external_node_positions_match_build_matrices_columns`
(`crates/hares-envelope/src/rc_network.rs:1038`) and
`multi_depth_ground_cols_match_assembled_matrix`
(`crates/hares-envelope/src/boundary_rc.rs:2385`), which checks depths of
0.3 m and 2.0 m including the `B_ext` wiring; V21 (a malformed EV
`charging_strategy` override) already fails construction and is pinned by
`init_errors_on_malformed_charging_strategy_override`
(`crates/hares-equipment/src/ev/tests.rs:8062`); V30 (vacuous determinism
test) is fixed by `04-fleet.md` FL-1 for fleets and `06-runtime-memory.md`
RT-5 for single dwellings.

## OD-1: engine exceptions do not survive pickling

**Kind:** fix. Fixes V3.

**Problem:** `pyo3::create_exception!(_hares, ...)` registers the four
engine exceptions with `__module__ == "_hares"`
(`crates/hares-python/src/py_dwelling.rs:30-37`), while the extension module
is `ochre_next._hares` (`pyproject.toml:81`). Unpickling looks up
`_hares.HaresConfigError`, which does not exist, so an exception raised in a
worker process cannot reach its parent.

**Change:** `create_exception!(hares._hares, ...)` for `HaresConfigError`,
`HaresEquipmentError`, `HaresSimulationError` and `FatalDwellingError`;
pyo3 stringifies the module expression into the qualified name. The import
package becomes `hares` in the package-rename entry of
`05-python-surface.md`, which this entry follows.

**Acceptance test:** `engine_exceptions_pickle_round_trip`
(`tests/python/`): for each of the four types, a worker started with the
`spawn` multiprocessing context raises it; in the parent, the received
exception's type `is` the class imported from `hares._hares` and its `args`
equal the raised `args`. The test iterates every exception class
`hares._hares` exports, so `HaresHealthPolicyError` (added by
`03-output-reporting.md` OR-12) and `HaresCallbackError` (added by
`05-python-surface.md` PY-10), both under the same
`create_exception!(hares._hares, ...)` form, are covered when they land.

**Verification:** `uv run pytest tests/python -k pickle`.

**Commit:** `fix: register engine exceptions under their importable module path`

**Dependencies:** the package-rename entry in `05-python-surface.md`.

## OD-2: strict configuration and override handling

**Kind:** fix. Fixes V11, V35, V36, V37, V38, V39 (the override half), V40 (the
EV half), V43, and the silent autosizing fallbacks found with them. Rule:
`dl-i08-architecture.config` (unknown keys are errors) and `precedence`
(an override the engine cannot apply is rejected; an accepted override is
applied).

**Problem and change, per site:**

1. **Heat-pump configs accept unknown keys.** `HeatPumpHeaterConfig`
   flattens `common` and `defrost` (`crates/hares-equipment/src/hvac/heat_pump_config.rs:263,298`)
   and `HeatPumpCoolerConfig` flattens `common` (`:484`); `HeatPumpConfig`
   is an alias of the heater (`:699`). serde cannot combine `flatten` with
   `deny_unknown_fields` (`:254-256, 476-478`). Change: each outer struct
   ends with a flattened `RejectUnknownKeys` field, a zero-sized type whose
   `Deserialize` reads the keys no other field took and fails naming them;
   it is not serialized. The tests that pin the permissive behaviour are
   rewritten to expect the error: `heater_ignores_cooler_only_field_stage_shrs`
   (`heat_pump_config.rs:1062`), `cooler_ignores_heater_only_field_hp_lockout`
   (`:1078`) and `toml_heat_pump_config_ignores_unknown_fields`
   (`crates/hares-io/tests/config_round_trip.rs:820-833`); the doc comments
   at `:19-22, 254-256, 476-478` are rewritten.
2. **`SimulationConfig` accepts unknown keys and hides a bad resolution.**
   The struct has no `deny_unknown_fields` (`crates/hares-io/src/config.rs:61-62`),
   and `time_res_secs_u32` turns an out-of-range resolution into 3600 s
   (`:139-141`). Change: `#[serde(deny_unknown_fields)]`;
   `time_res_secs_u32` returns `Result<u32, ConfigError>` and its callers
   propagate it. Replacement names for removed keys belong to the config
   entry that removes them.
3. **Override payloads are dropped silently.** In
   `crates/hares-core/src/dwelling/conversions.rs`: a per-equipment override
   whose value is not an object is ignored (`:945`); when both `"all"` and
   `"*"` are present, `"*"` is ignored (`:942`); on the untyped path, a value
   `json_value_to_config_value` cannot convert (null, object, mixed array) is
   dropped (`:683`, `:1085-1099`). Change: each is an error naming the
   equipment and key. A non-object top-level payload is already rejected
   (`:858-885`). The conversion at `:683` runs over every raw parameter,
   resolver output included, so the unconvertible-value check runs where the
   override payload is merged (`:906-944`) and names only override-supplied
   keys. Resolver output is not a caller input: the entry's first step is a
   census, a unit test over the resolved specs of every HPXML fixture under
   `tests/fixtures/` and `data/examples/`, listing each raw parameter the
   conversion drops; each resolver-emitted key it finds is either no longer
   emitted for raw specs or given a conversion, in this commit, and the
   census result is recorded in the commit body, so no golden stops the
   entry unexpectedly.
4. **Typed-config rebuild errors are swallowed during autosizing.**
   `rebuild_hvac_typed_config` returns `None` for any build error
   (`.ok().flatten()`) and for an unknown name
   (`crates/hares-io/src/hpxml/resolve_hvac.rs:309-360`), and autosizing
   assigns that `None` over the spec's typed config
   (`crates/hares-core/src/dwelling/autosize.rs:593-594`). The builders
   behind it also return `None` for missing inputs, not only on error:
   `try_build_central_ac_config` when SEER is missing (`resolve_hvac.rs:1150-1153`,
   "Skipping AC") or the cooling capacity is missing (`:1155`);
   `try_build_room_ac_config` when the cooling capacity or EER is missing
   (`:1240, 1244`); the electric furnace, electric boiler and baseboard
   builders `Ok(None)` when the heating capacity is missing (`:851, 994,
   1071`); and the ideal-HVAC, central-AC, room-AC and dehumidifier builders
   when `EquipmentConfig::from_typed` fails (`.ok()?`, `:1139, 1233, 1291,
   1322`). The initial resolver also swallows a mini-split cooler's builder
   error (`.ok().flatten()`, `:2233-2235`). The duct DSE inputs fall back to
   defaults on an error whose own message says no silent default is
   permitted (`crates/hares-core/src/dwelling/mod.rs:2285-2294`, against
   `resolve_hvac.rs:246-273`). Change: every HVAC builder returns
   `Result<EquipmentConfig, HpxmlError>`. A missing required input is
   `HpxmlError::MissingField` naming the element
   (`HeatingSystem/HeatingCapacity`, `CoolingSystem/CoolingCapacity`, or
   `AnnualCoolingEfficiency` in the units the system requires, SEER or
   EER), and a `from_typed` failure is an error naming the equipment and the
   config type. The one legitimate "no typed config yet" outcome is a
   capacity that autosizing will supply: the initial resolver maps a
   capacity `MissingField` to "typed config pending autosizing" only when
   the spec is flagged `autosize_heating` or `autosize_cooling`, as it
   already does for the gas furnace and gas boiler (`:2093-2116`), and fails
   on every other error, the mini-split cooler's included.
   `rebuild_hvac_typed_config` runs after autosizing, has no pending case,
   and returns `Result<EquipmentConfig, HpxmlError>` (an unknown name is an
   error naming it); `autosize_equipment_capacities` returns `Result`; and
   both errors fail construction.
5. **Water-heater efficiency.** A UA calculation error is replaced by a
   default (`crates/hares-io/src/hpxml/resolve_water_heater.rs:131-141`). The
   gas water heater uses `conversion_efficiency` ahead of `energy_factor` and
   `uniform_energy_factor`, and a built-in 0.78 when none is given
   (`crates/hares-equipment/src/water_heater/gas.rs:395-404`), so an
   `energy_factor` override is accepted and never applied whenever the
   resolver set `conversion_efficiency` (the consumer's `wh_energy_factor`
   control measures exactly this, `HARES_CALIBRATION_SYSTEM/calibration/registry.py:189-200`).
   Change: the UA error fails construction naming the water heater; an
   override of `energy_factor` or `uniform_energy_factor` on a water heater
   whose config carries `conversion_efficiency` is rejected naming
   `conversion_efficiency`; a gas water heater with no efficiency field is a
   construction error.
6. **Thermostat override.** The thermostat uses a schedule column, then the
   HPXML profile, then the constant setpoint
   (`crates/hares-equipment/src/hvac/thermostat.rs:276-283`), so an override
   of `setpoint.heating_setpoint_c` or `cooling_setpoint_c` is accepted and
   never applied. Change: when an override sets a constant setpoint,
   `merged_equipment_config` (`conversions.rs:773-812`) removes the matching
   `*_setpoint_source`, so the override applies. `set_typed_setpoint_source`
   returns silently when the spec has no typed object payload or the source
   cannot be serialized (`crates/hares-io/src/schedule_resolve.rs:741-757`);
   each case becomes an error. The precedence between an HPXML thermostat and
   a schedule-file setpoint column (`schedule_resolve.rs:673-690`) is
   `12-inputs.md` IN-5's. Making the override apply moves the numbers of
   every run that sets it, so site 6 is this entry's second commit, in its
   own row of the numeric block (`00-schedule.md` row 94e), under
   `09-numeric-fixes.md`'s rules, with its delta recorded: the frame of
   `constant_setpoint_override_applies`'s run (`base.xml`, heating setpoint
   override of 25 °C) before and after, per variable, in
   `02-measurement.md`'s log. No golden and no consumer path sets a setpoint
   override (the consumer moves setpoints through HPXML,
   `HARES_CALIBRATION_SYSTEM/calibration/registry.py:12-13`), so the
   goldens and `consumer_shape_900s` are expected unchanged; any change is
   recorded as the delta.
7. **EV charging power.** `MaxChargingPower` is read as kW with no check
   (`crates/hares-io/src/hpxml/resolve_der.rs:197`;
   `crates/hares-equipment/src/ev/config.rs:14`). Change: a value above the
   SAE J1772 AC Level 2 maximum of 19.2 kW is an error naming the value and
   the kW unit. `09-numeric-fixes.md` B-14 later moves the field and the
   check to W (19 200 W), and `12-inputs.md` IN-9, which reads HPXML
   vehicle chargers' `ChargingPower` in W, applies the same check. The
   battery-without-control half of V40 (`resolve_der.rs:163`) is OD-8.
8. **Site UTC offset.** An offset outside ±24 h becomes UTC, and a
   non-finite offset rounds to 0 and also becomes UTC
   (`crates/hares-core/src/dwelling/blueprint.rs:96-97`). Change: both are
   construction errors naming the value. The `.single().unwrap_or_else(..)`
   at `:103-104` cannot trigger for a fixed offset (`FixedOffset` has no
   gaps or repeated hours) and is replaced by the infallible conversion.
   This site is `review.md` V36. Its daylight-saving half needs no change:
   the civil time zone (`chrono_tz::Tz`, `blueprint.rs:112-120`, an
   unknown name already an error) is only ever applied to an instant
   (`clock.rs:132`, `environment.rs:422, 549`), which has exactly one civil
   time. Tests: `an_out_of_range_utc_offset_is_a_construction_error` and
   `a_non_finite_utc_offset_is_a_construction_error`, each failing
   construction with the value named; both fail at `7b7a6a7d`.
9. **`wall_area_m2`.** Parsed and never read, kept "for backward
   compatibility" (`crates/hares-core/src/dwelling/synthetic.rs:137-143`).
   Change: the field is removed, `SyntheticGeometryConfig` gets
   `deny_unknown_fields`, and every fixture carrying the key is migrated
   (`git grep -l wall_area_m2`, 25 files).

**Acceptance test** (each fails at HEAD):
`heat_pump_config_rejects_unknown_keys` (JSON and TOML),
`simulation_config_rejects_unknown_key`,
`time_res_beyond_u32_seconds_is_an_error`,
`non_object_equipment_override_is_an_error`,
`all_and_star_overrides_together_are_an_error`,
`unconvertible_raw_override_value_is_an_error`,
`hvac_typed_config_rebuild_error_fails_construction`,
`hvac_builder_missing_input_names_the_element` (an `Air Conditioner` spec
without SEER, a `Room AC` spec without EER, and an `Electric Furnace` spec
without capacity and without `autosize_heating`, each failing with
`MissingField` naming the element),
`missing_capacity_is_pending_only_when_autosizing` (the same furnace with
`autosize_heating = true` builds with its typed config after autosizing),
`duct_dse_error_fails_construction`,
`water_heater_ua_failure_is_an_error`,
`shadowed_energy_factor_override_is_rejected`,
`gas_water_heater_without_efficiency_is_an_error`,
`constant_setpoint_override_applies` (`base.xml`, heating setpoint override
of 25 °C; the frame's heating setpoint is 25 °C on every step),
`ev_max_charging_power_above_level_2_is_rejected` (`MaxChargingPower` 11500),
`site_utc_offset_out_of_range_is_an_error` (30 h and NaN),
`synthetic_geometry_rejects_wall_area_m2`.

**Verification:** `cargo nextest run -p hares-equipment -p hares-io -p hares-core`;
`frame_golden compare-all` after each commit. The site 6 commit adds its
`docs/python.md` "Numeric changes" line (`plan.md` §2.8): a constant
setpoint override now replaces the schedule and HPXML setpoints, with the
goldens it re-captured or the statement that none was. From row 94c
`[golden]` also compares the `resolved` product
(`09-numeric-fixes.md` B-14 commit 1); M-1's capture re-captures it, and
the site 6 commit records its resolved delta, none expected since the
commit changes no equipment parameter.

**Commits:** sites 1 to 5 and 7 to 9 (`00-schedule.md` row 31),
`fix: reject unknown keys and inapplicable overrides instead of dropping them`;
then site 6 with its recorded delta (row 94e, in the numeric block),
`fix: a constant setpoint override replaces the schedule and HPXML setpoint sources`.
`constant_setpoint_override_applies` lands with the site 6 commit.

**Dependencies:** M-2 (the goldens that prove no fixture relied on a silent
path). It lands before `05-python-surface.md` PY-8, which builds on the
strict `SimulationConfig`; `06-runtime-memory.md` RT-4 later carries site
8's fix into the rewritten blueprint code. The site 6 commit follows the
consumer block and `12-inputs.md` IN-5, whose precedence rule it builds on. Consumer effect: site 5 turns
the consumer's `wh_energy_factor` reachability probe from an inert run into
a typed rejection; `08-consumer.md` CN-1 makes the probe assert that
rejection (`dl-i08-architecture.consumer_inert_override_probe`). The probe
is not on the DE-trace path, so the B0 check and the consumer-shaped golden
are unaffected. `12-inputs.md` IN-3 later makes site 3's accepted field
set the `DescribeFields` lists.

## OD-3: indirect water-heater loop wiring fails silently

**Kind:** fix. Fixes V46.

**Problem:** an autosized boiler loses its loop id, so an indirect tank
wired to it ends up on a different fluid loop. The wiring pass
`resolve_loop_wiring` runs at resolve time
(`crates/hares-io/src/hpxml/equipment.rs:197-236`), before autosizing. A
gas boiler with no `HeatingCapacity` is flagged `autosize_heating` and gets
`typed_config = None` on purpose, pending autosizing
(`crates/hares-io/src/hpxml/resolve_hvac.rs:2105-2115`), so
`set_boiler_loop_id` skips it (`equipment.rs:247-249`) while
`set_indirect_tank_boiler_loop_id` puts the tank on the new loop. The
central allocator (`crates/hares-core/src/dwelling/loop_allocator.rs:100`,
called at `mod.rs:2240`, before autosizing at `:2299`) also skips a spec
whose typed config is `None`. Autosizing then rebuilds the boiler's typed
config (`crates/hares-core/src/dwelling/autosize.rs:593-594`) through
builders that set `loop_id: None` (`resolve_hvac.rs:957` for the gas
boiler, `:1040` for the electric boiler), and the boiler falls back to
`LoopId::default()` at construction
(`crates/hares-equipment/src/hvac/boiler.rs:169-170` and `:549-550`,
`unwrap_or_default`; the typed `loop_id` overrides it only when present,
`:255` and `:666`). A wired
boiler that is not autosized keeps its id, because autosizing skips it
(`autosize.rs:324-326`). Two further silent paths sit in the wiring pass:
`set_boiler_loop_id` and `set_indirect_tank_boiler_loop_id` log and
continue when a typed config does not deserialize as the expected type
(`equipment.rs:252-274, 289-299`), and `set_boiler_loop_id` ignores a name
that is not a boiler (`:277`).

(All lines at `21d90588`.)

**Change:**
- The loop id lives in the spec's raw parameters, which the post-autosize
  rebuild reads: `resolve_loop_wiring` writes `loop_id` into the boiler
  spec's `parameters` (and into its typed config when present), and
  `allocate_loop_ids` does the same for a boiler whose typed config is
  pending autosizing. `try_build_gas_boiler_config` and
  `try_build_electric_boiler_config` read `loop_id` from the parameters in
  place of `loop_id: None` (`resolve_hvac.rs:957, 1040`), so the rebuilt
  config carries the id the boiler was wired or allocated.
- A boiler built with no loop id is a construction error naming the boiler
  (both constructors lose `unwrap_or_default`, and `init` fails when the
  typed `loop_id` is `None`), since every resolved boiler now carries one;
  test builders that construct a boiler config without a loop id set one
  (census `git grep -n 'loop_id: None' -- crates tests` over the boiler
  configs).
- In the wiring pass, a typed config that does not deserialize as the
  expected type, and a wiring target that is not a boiler, return an
  `HpxmlError` naming the equipment, the expected config type and the loop
  id. A boiler pending autosizing is not an error: its id goes into its
  parameters, as above.

**Acceptance test:**
- `autosized_boiler_keeps_its_indirect_tank_loop`:
  `tests/fixtures/hpxml/ochre_samples/base-dhw-multiple.xml` (a boiler,
  `HeatingSystem1`, and an indirect tank whose `RelatedHVACSystem` names it,
  `:411`) with the boiler's `HeatingCapacity` (`:330`) removed in the test,
  so the resolver flags it for autosizing: after build, the boiler's port
  loop id equals the tank's `boiler_loop_id`. At the parent they differ.
- `indirect_tank_wiring_errors` (unit tests of the wiring pass over
  `EquipmentSpec`s): a boiler spec whose typed payload is a different
  config type, a wiring target that is not a boiler, and a tank spec whose
  payload is not `IndirectTankConfig` each return an error naming the
  equipment; a boiler spec pending autosizing returns `Ok` with `loop_id`
  in its parameters.
- `boiler_without_loop_id_is_a_construction_error`.

**Verification:** `cargo nextest run -p hares-io -p hares-core -p
hares-equipment`; `frame_golden compare-all`: the one golden boiler
(`tests/fixtures/resstock/2025.1/bldg0000002/home.xml`, a gas boiler with
`HeatingCapacity` 37881.0, `:740-746`, in `fleet_mixed_offset`) is not
autosized and keeps its allocated id, so every golden compares bitwise; any
change stops the entry.

**Commit:** `fix: keep an autosized boiler on its indirect tank's loop and fail when wiring cannot apply`

**State:** FIXED (2026-10-04, commit `249912f9`), with the mechanism
extended beyond this entry's literal list by two findings the
implementer and the red team proved: the ledger's own premise that a
wired non-autosized boiler keeps its id was outdated (the element's
missing CoolingCapacity flags cooling autosizing, whose trailing
rebuild wiped the typed id; the unmodified base-dhw-multiple boiler
sat on the LoopId(0) fallback), and the tank's own water-heater
autosize wiped its wired id through the stale serialized null in its
parameters. The fix: the loop id lives in the raw parameters, every
water-heater and indirect-tank allocator arm mirrors its final id the
same way (preserving a wired id without consuming the counter), every
water-heater rebuild builder reads it, the wiring pass's
log-and-continue paths are resolve errors naming the tank, the idref
and the resolved target, and a boiler constructed without a loop id
fails init naming the boiler. The generator arm keeps typed-only
assignment: no generator rebuild path exists (documented). The red
team proved the topology (boiler supply == tank supply == the wired
loop, not the fallback), the counter integrity (two pairs: loops 1
then 2), the adversarial error paths (a string loop_id is a typed
error, not a coercion), and the sweep: 425 samples, build errors 112
to 112, zero new. Red-green proven in place at the base head. Goldens
bitwise identical, both profiles; nextest 7075 default (the four
failures are the red team's untracked scratch files pinning the
pre-fix behavior the fix reverses: expected); pytest 1412 passed,
4 xfailed, 0 skipped. Filed with it: the Python surface still cannot
pair a boiler with an indirect tank (no loop_id exposure on the
boiler builders, no system ids on Python specs: pairing is HPXML-only;
a follow-up entry when the Python equipment surface is next touched).

**Dependencies:** none. Lands before OD-2, whose site 4 makes the
autosizing rebuild return `Result`; the rebuild's loop id is carried here
first.

## OD-6: a missing schedule file silently becomes a generated schedule

**Kind:** fix.

**Problem:** when `schedule_path` does not exist, construction generates a
schedule from the HPXML instead (`crates/hares-core/src/dwelling/blueprint.rs:46-56`,
duplicated at `crates/hares-core/src/dwelling/mod.rs:1937-1948`), so a
mistyped path runs with different occupancy and loads and no error.

**Change:** `DwellingConfig.schedule_path` becomes `Option<PathBuf>`.
`None` asks for a schedule generated from the HPXML; a path that cannot be
read is an error naming it. Python's `DwellingConfig(schedule=...)` accepts
`None` for the same request. Every caller is migrated in this commit.

**Acceptance test:** `nonexistent_schedule_path_is_an_error`;
`no_schedule_generates_from_hpxml` (equal to today's generated schedule for
`base.xml`).

**Verification:** `cargo nextest run -p hares-core`; `uv run pytest
tests/python`.

**Commit:** `fix: a schedule path that does not exist is an error, not a generated schedule`

**State:** FIXED (2026-10-04, commit `13881b48`). Every caller migrated
in the commit (30 Rust test files wrapped, the three fake-path tests
now pass None, the Python bindings and the stub updated, the fleet and
the golden adapter wrapped): the two duplicated resolution sites
collapsed into one helper; exactly one production caller of the
generator remains, the explicit None arm. The red team proved the
hazard was real and the semantics preserved: a HEAD-worktree run on a
fake path silently generated a DIFFERENT schedule (digest
fb3ae7ac...) than the real path's, and the None arm's digest
(ca65bbaf...) is bitwise the old silent generation's. The fleet's
per-building failures name the path (probed: mixed fleet, one broken
building, exactly one error naming it); the golden adapter fails
loudly on a manifest with a missing schedule (probed in /tmp). The
2025.1 ResStock fixture bldg0527060 ships no in.schedules.csv and
HEAD silently ran it generated; the migration names that building's
None explicitly by building id. Gym's legacy dict path passing a None
schedule now fails naming "None" (pre-existing site, OD-7's
territory). Goldens bitwise identical, both profiles; nextest 7066
default and 7124 observe; pytest 1412 passed, 4 xfailed, 0 skipped;
pyright 0 errors.

**Dependencies:** none. Lands before `06-runtime-memory.md` RT-4.

## OD-8: an HPXML battery with no declared control is a construction error

**Kind:** fix (`dl-i08-architecture.battery_control`: strict everywhere).
Fixes the battery half of V40.

**Problem:** HPXML has no battery control element in HARES's parser, so
every HPXML battery is built with `bms_mode: None`, which becomes
`BmsMode::Manual`; Manual seeds no battery-management actor, so the battery
never cycles unless something commands it. A caller who forgot to declare a
control gets a battery that is silently inert (the consumer measured it:
`HARES_CALIBRATION_SYSTEM/calibration/der_inject.py:15`, "A battery from
HPXML is INERT"). One test built on such a battery passes vacuously:
`tests/python/test_tariff_integration.py` runs the Manual battery of
`data/examples/pv_battery_example.xml` and guards its assertions with
`if total > 0.5`, `if len(socs) > 10` and `if len(post_peak_charge) > 0`
(`:131, 152, 163, 182`), which an idle battery never enters. A fifth guard,
`if len(summaries) > 0` in `TestBillingSummary` (`:277`), never enters for
another reason: the shared run starts `2019-07-01T12:00` and lasts 7 d
(`:32, 50`), so it crosses no billing-period boundary and produces no
summary.

**Evidence:** `crates/hares-io/src/hpxml/resolve_der.rs:99-175`
(`resolve_batteries` reads `RatedPowerOutput`, `NominalCapacity`,
`RoundTripEfficiency` and sets `bms_mode: None` at `:163`);
`crates/hares-equipment/src/battery/mod.rs:522` (default Manual),
`:1049-1056` (a declared `bms_mode` replaces it), `:1626-1628`
(`actor_seed()` is `None` for Manual). The ways to declare a control today:
the equipment override `{"Battery": {"bms_mode": "<BmsMode JSON>"}}`
(`crates/hares-core/src/dwelling/conversions.rs:906-944`; example
`crates/hares-core/tests/equipment_id_regressions.rs:836-850`), or a
registered `BatteryManagement` actor (`crates/hares-core/src/actor_registry.rs:468-566`).
Checked-in HPXML files with a stationary battery, none declaring control:
`data/examples/pv_battery_example.xml:462`,
`tests/fixtures/hpxml/ochre_samples/base-battery.xml:434`,
`base-pv-battery.xml:462`, `tests/fixtures/parity/cz4a_battery_only/building.xml:905`,
`cz4a_pv_battery/building.xml:922`, `cz6b_pv_battery_ev/building.xml:922`
(the `<Battery>` elements of ResStock `bldg0176775` and `bldg0442669` belong
to vehicles).

**Change:**
- Assembly fails with `HaresError::Equipment` (the variant an invalid
  `bms_mode` already returns, `battery/mod.rs:1060`) naming the battery when a
  stationary battery has neither a declared `bms_mode` nor a
  `BatteryManagement` actor targeting it. The check runs where both are
  known (dwelling assembly), so `DwellingBlueprint::validate()` reports it
  (`12-inputs.md` IN-2). `Manual` declared explicitly is a declared control
  (the battery follows commands, as a gym or co-simulation drives it).
- Every caller of the fixtures above declares its control: the parity
  cases whose OCHRE reference holds the battery idle declare
  `bms_mode = "Manual"` through a new `[overrides]` table in their
  `config.toml` (parsed by the parity harness, `tests/parity/mod.rs`, into
  the override map); `alignment_oracles_regressions.rs:116` and
  `core_output_regressions.rs:206` (which drives the battery by
  `PowerSetpoint`) declare `Manual`; `tests/python/test_py_fleet.py:435`
  declares the control it tests; `test_tariff_integration.py` declares
  self-consumption and its four battery guards become unconditional.
  `TestBillingSummary` gets its own run of the same inputs and tariff,
  starting `2019-06-28T00:00:00` for 7 d so it crosses the July 1 boundary,
  and asserts that the summaries are non-empty and that the first one, the
  June period, passes the four plausibility checks unconditionally.
- `docs/` user documentation for batteries states the rule and the two
  ways to declare a control.

**Acceptance test:**
- `battery_without_declared_control_is_a_construction_error`
  (`base-battery.xml`, no override): `build()` and `validate()` fail naming
  the battery.
- `explicit_manual_battery_builds` and
  `bms_actor_counts_as_declared_control`.
- `test_tariff_integration.py` passes with every assertion executed (none
  of the five guards remains).
- Every parity case passes at its existing tolerance.

**Verification:** whole block; `frame_golden compare-all` (no golden has a
battery).

**Commit:** `fix: a battery with no declared control is a construction error`

**Dependencies:** OD-2 (strict overrides, so a misspelled `bms_mode` key is
already an error). Consumer effect: the TRAP 2 case becomes a refusal in
`08-consumer.md` CN-1; the DE-trace premises declare no stationary battery,
which the row's B0 check confirms.

## OD-9: a generated schedule flattens the HPXML setpoints to one constant

**Kind:** fix, numeric: its own commit and its own row in the numeric
block, following `09-numeric-fixes.md`'s rules (delta recorded, goldens
re-captured).

**Problem:** when no schedule file is given, `generate_schedule_from_hpxml`
writes `heating_setpoint` and `cooling_setpoint` columns holding one
constant each: the first hourly value of the HPXML weekday profile, or
20.0 C and 24.0 C when the HPXML has none. Because a setpoint column
outranks the HPXML profile (`12-inputs.md` IN-5), the dwelling runs at the
hour-0 value all day instead of the declared 24-hour weekday and weekend
profiles, and a home with no thermostat input gets two silent defaults
instead of the default profile the resolver otherwise applies.

**Evidence:** `crates/hares-io/src/hpxml_schedule.rs:36`
(`generate_schedule_from_hpxml`), `:66-73` (`heating_weekday_setpoints_c`
`.map(|v| v[0]).unwrap_or(20.0)`), `:75-83` (cooling, `unwrap_or(24.0)`);
`crates/hares-io/src/schedule_resolve.rs:655-693` (a column wins over the
HPXML source).

**Change:** the generated schedule carries no setpoint columns, so each
side resolves by IN-5's rule to the HPXML setpoints or, without them, to
the default profile, recorded as the active `SetpointSource`. The two
literal defaults are deleted, and with them
`SetpointSource::GeneratedConstant` and the recording IN-5 adds for a
generated schedule's constant columns (`plan.md` §2.7), so nothing remains
for `12-inputs.md` IN-8's Fahrenheit reading to convert in a generated
schedule. `tracing` at `debug`: the source chosen per side.

**Acceptance test** (fixture: `tests/fixtures/resstock/2025.1/bldg0000002/home.xml`,
whose heating profile varies by hour and differs between weekday and
weekend: weekday 67 F at hours 0 to 2 and 5 to 12, 70 F at hours 3, 4
and 13 to 17; weekend 70 F from hour 3 to hour 17; with no schedule file,
weather `tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv`, start
`2018-01-05T00:00:00-05:00` (a Friday), 3 d at 3600 s, no warm-up).
`base.xml` cannot show the fix: it declares only the constant
`SetpointTempHeatingSeason` 68 and `SetpointTempCoolingSeason` 78
(`base.xml:357-358`), which the parser expands to 24 equal values, so every
hour equals hour 0.
- `generated_schedule_keeps_the_hpxml_setpoint_profile`: the scheduled
  heating setpoint at 03:00 on Friday is 70 F, at 06:00 on Friday 67 F and
  at 06:00 on Saturday 70 F, each equal on bits to the parser's Fahrenheit
  to Celsius conversion of the profile value, and `SetpointSource` is
  `Hpxml`. At the parent every step reads 67 F (the weekday hour-0 value),
  so the first and third assertions fail.
- `generated_schedule_without_thermostat_uses_the_default_profile`: the same
  home with its `HVACControl` removed in the test resolves to the default
  profile and records it as the active source.

**Verification:** `cargo nextest run -p hares-io -p hares-core`; goldens
compared (no golden runs a generated schedule, so none is expected to
change; any change is recorded as the delta); the delta of the acceptance
run above (every variable, before and after) recorded in
`02-measurement.md`. The commit adds its `docs/python.md` "Numeric
changes" line (`plan.md` §2.8): a generated schedule keeps the HPXML
thermostat profile, no golden re-captured unless one moved.

**Commit:** `fix: a generated schedule keeps the HPXML thermostat profile instead of one constant`

**Dependencies:** `12-inputs.md` IN-5; OD-6 (the explicit request for a
generated schedule); OD-17 (the generator's profiles fail loudly) and
OD-19 (a malformed or setback thermostat profile is an error), so the
profile this entry passes through is the one the HPXML declares. Lands
before `12-inputs.md` IN-8 (row 94b).

## OD-7: the gym wrapper coerces and drops configuration values

**Kind:** fix. Fixes V44.

**Problem:** in `python/ochre_next/rl/gym_env.py`, `_coerce_seconds` turns
any value that is not a number or `timedelta` into 60 s (`:100-105`);
`_normalize_config` accepts a plain mapping through a second, untyped path
(`:108-134`); `_sim_config_to_kwargs` tests `start_time`, `duration_s`,
`time_res_s` and `master_seed` by truthiness (`:141-157`), so a zero
resolution is dropped and replaced by the default instead of reaching
validation. `_sim_config_to_kwargs` also forwards only the fields it lists,
so `write_output` and every other `SimulationConfig` field it omits never
reach the dwelling: a gym construction cannot disable recording.

Forwarding field by field cannot be made complete: the Python
`SimulationConfig` has 17 fields (`crates/hares-python/src/py_config.rs:48-64`
at `21d90588`), while `Dwelling.from_hpxml`'s keyword list
(`KNOWN_KWARGS`, `crates/hares-python/src/py_dwelling.rs:2199`) uses other
names for some (`duration_s`, `time_res_s`) and has none for
`retain_batches`, which the keyword path hard-sets to `true` (`:2465`).
`from_hpxml` already takes the whole object as `config=` (`:2304-2315`).

**Change:** `_coerce_seconds` raises `TypeError` naming the type; the gym
environment accepts only `GymDwellingConfig` and raises `TypeError` for a
mapping; every presence test is `is not None` (including the gym's own
`time_res_s` read, `gym_env.py:511`); `_sim_config_to_kwargs` is deleted,
and the gym passes `config=sim_config` to `from_hpxml` unchanged, so every
field reaches the dwelling by construction. `05-python-surface.md` PY-8
later makes that object the one config type.

**Acceptance test:** `test_gym_rejects_non_numeric_time_res`,
`test_gym_rejects_mapping_config`, `test_gym_zero_time_res_reaches_validation`
(raises the engine's resolution error),
`test_gym_zero_master_seed_reaches_the_dwelling` (a `master_seed` of 0,
which the truthiness test dropped for the default, is the seed the built
dwelling's `Dwelling.config()` reports; at the parent it reports the
default), `test_gym_forwards_every_sim_config_field`:
a gym built with a `SimulationConfig` whose every field differs from its
default, `write_output=False` among them, records no output, and the
config read back from the built dwelling (`Dwelling.config()`,
`py_dwelling.rs:1624`, built by `PySimulationConfig::from_sim_config`)
equals the one passed on every field the class exposes. At the parent the
test fails on `write_output` and on every field `_sim_config_to_kwargs`
omits.

**Verification:** `uv run pytest tests/python -k gym`.

**Commit:** `fix: the gym wrapper rejects bad configuration instead of coercing it`

**Dependencies:** the package-rename entry in `05-python-surface.md` (the
file moves to `python/hares/rl/gym_env.py`).

## OD-10: infiltration set-up substitutes undocumented defaults for missing inputs

**Kind:** fix (found defect, `dl-i08-process.found_defects`; silent
defaults, against `dl-i08-architecture.config`'s strictness). Two commits
in two rows: a structural commit that turns every undocumented default
into an error and stays bitwise on every fixture (`00-schedule.md` row
24a), and a numeric commit that adopts the OpenStudio-HPXML documented
defaults, with its own row in the numeric block and its recorded delta.

**Problem:** the infiltration set-up fills every missing input with a
default of its own, and none is OpenStudio-HPXML's.
- A building without a conditioned volume gets 400 m3 with no error,
  warning or record, both in the CFM50 or effective-leakage-area to ACH50
  conversion and in the AIM-2 coefficients. The duct DSE path treats the
  same missing value as an error whose message says no silent default is
  permitted. The HPXML parser already requires `ConditionedBuildingVolume`;
  the default is reached by a `hares_io::Building` built in code, which
  the public Rust API allows (the field is `Option<f64>`).
- `NumberofConditionedFloorsAboveGrade` is parsed as optional and the
  solver substitutes 1.0. OpenStudio-HPXML requires it, with no default.
- A missing `InfiltrationHeight` becomes the ceiling height times the
  floors above grade. OpenStudio-HPXML estimates it "from other inputs
  (e.g., ConditionedFloorArea, NumberofConditionedFloorsAboveGrade,
  attics/foundations with WithinInfiltrationVolume=true, etc.)".
- A missing `HasFlueOrChimneyInConditionedSpace` becomes `false`.
  OpenStudio-HPXML defaults it to true when "heating system is non-electric
  Furnace, Boiler, WallFurnace, FloorFurnace, Stove, or SpaceHeater located
  in conditioned space and AFUE/Percent is less than 0.89, heating system is
  non-electric Fireplace located in conditioned space, or water heater is
  non-electric and located in conditioned space with EnergyFactor less than
  0.59", otherwise false.
- The element is parsed only when its text is the literal `true`
  (case-insensitive), so the `xsd:boolean` form `1` reads as false, and the
  same test is repeated at nine other boolean elements of the parser.
- Thirteen checked-in HPXML files declare the flue as
  `<HasFlueOrChimney>false</HasFlueOrChimney>`, the element's older name,
  which the parser does not read: the declaration is dropped without a
  word (today's default happens to equal it).
- A missing `ShieldingofHome`, and any string other than `exposed` or
  `well-shielded`, becomes `normal`. OpenStudio-HPXML defaults it to
  "'normal' for single-family detached or manufactured home and
  'well-shielded' for single-family attached or apartment unit", and its
  allowed values are `normal`, `exposed` and `well-shielded`.
- A `SiteType` other than `rural`, `suburban` or `urban` is kept as
  `SiteType::Other` and treated as suburban with a warning; a missing one
  is suburban with a warning. OpenStudio-HPXML's choices are those three,
  defaulting to suburban.
- A code-built `Building` with no ceiling height gets 2.5 m, and an
  envelope zone with no matching environment zone gets
  `ZoneId(zone_idx)`.

**Evidence** (at `21d90588`; documentation: OpenStudio-HPXML "Workflow
Inputs", <https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>,
sections "HPXML Building Construction", "HPXML Air Infiltration" and
"HPXML Site"):
`crates/hares-core/src/dwelling/solver_builder.rs:1371` and `:1440` (the
400 m3), `:1382` (`ceiling_height_m.unwrap_or(2.5)`), `:1391-1408` (site
type and shielding, with the two warnings), `:1414-1423` (the zone-id
fallback), `:1434-1436` (infiltration height), `:1445` (flue), `:1447`
(floors); `crates/hares-core/src/dwelling/conversions.rs:1047-1056`
(`site_type_to_terrain`, `_ => Suburban`) and `:1063-1072`
(`shielding_str_to_class`, `_ => Normal`);
`crates/hares-io/src/hpxml/building.rs:491-496` (site type and shielding
parsed as text), `:524-551` (the volume required; the ceiling height
derived from it), `:557-563` (floors above grade optional), `:578-581`
(infiltration height), `:594-596` (the flue boolean), `:3106-3113`
(`parse_site_type`'s `Other`); `crates/hares-io/src/hpxml/resolve_hvac.rs:247-256`
(the duct DSE `MissingField`); the boolean census
`git grep -n 'eq_ignore_ascii_case("true")' -- crates/hares-io/src`
(twelve sites: `building.rs:596, 623, 1623, 2140, 2356, 2411`;
`resolve_loads.rs:322, 490, 623, 989, 991`; `resolve_water_heater.rs:478`;
at `7b7a6a7d` the same twelve, with `building.rs` at `:599, 626, 1578,
2095, 2310, 2365`; no other boolean parse form, `== "true"`,
`parse::<bool>` or a `"true" =>` match arm, exists in `hares-io/src`);
`git grep -l '<HasFlueOrChimney>' -- . ':!vendors'` (thirteen files: the
eleven `tests/fixtures/parity/*/building.xml`,
`data/examples/BEopt_example.xml` and `tests/fixtures/hpxml/base-all-north.xml`).
Checked-in HPXML census (67 files): every one carries
`ConditionedBuildingVolume` and `SiteType`; two lack
`NumberofConditionedFloorsAboveGrade` (`crates/hares-io/tests/fixtures/hpxml-v3-minimal.xml`,
`hpxml-v4-buildingsync.xml`); 17 lack `InfiltrationHeight`, 30 lack
`HasFlueOrChimneyInConditionedSpace` and 17 lack `ShieldingofHome`
(`base.xml` among them; two of those 17, the `base-bldgtype-multifamily*`
files, are apartment units). Every golden's HPXML carries all of them.

**Change, structural commit** (bitwise on every fixture):
- The volume: both infiltration sites return the `MissingField` that
  `resolve_hvac.rs:247` returns, naming
  `BuildingSummary/BuildingConstruction/ConditionedBuildingVolume` with the
  reason that infiltration needs it; the volume is resolved once before
  both sites.
- `NumberofConditionedFloorsAboveGrade` is a parser `MissingField`, and
  `Building.floors_above_grade` becomes `f64`, so the solver's 1.0 goes.
  The two parser test fixtures that lack it (minimal documents with no
  `NumberofConditionedFloors` either) gain
  `<NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>`,
  the value the solver substitutes for them today, and a test asserts the
  error.
- `Building.ceiling_height_m` becomes `f64` (the parser always derives it
  from the volume and floor area), so the 2.5 m default goes; an envelope
  zone with no matching environment zone is an assembly error naming the
  zone index, in place of `ZoneId(zone_idx)`.
- `ShieldingofHome` and `SiteType` are parsed into enums; a string outside
  each element's allowed values is a parse error naming the element, the
  value and the allowed values (`SiteType::Other` is deleted). A missing
  `SiteType` resolves to suburban by OpenStudio-HPXML's documented default,
  without a warning, cited in the code and in `docs/PHYSICS_DECISIONS.md`
  (today's value, so no number moves).
- One helper, `parse_xsd_boolean`, reads every boolean element the parser
  reads: `true` and `1` are true, `false` and `0` are false (the
  `xsd:boolean` lexical space after whitespace collapse), anything else is
  a parse error naming the element and the value; all twelve sites of the
  census use it. The entry's first step lists every value of those
  elements in the checked-in HPXML files; a value outside the lexical space
  (for example `True`) is corrected in the fixture in this commit.
- `<HasFlueOrChimney>` is a parse error naming its current name
  `HasFlueOrChimneyInConditionedSpace`; the thirteen files are migrated to
  the current name with their declared `false`, which is the value the run
  uses today.
- The test builders that construct a `Building` with
  `conditioned_volume_m3: None` and an infiltration input that needs the
  volume (`solver_builder.rs` tests, `autosize.rs:1120`,
  `conversions.rs:1179`, `resolve_hvac.rs:3979`) set a volume, and every
  code-built `Building` sets the two fields that become required.
- `ConditionedBuildingVolume` stays required by the parser. OpenStudio-HPXML
  defaults it to `ConditionedFloorArea * AverageCeilingHeight +
  ConditionedCrawlspaceVolume` with an 8 ft `AverageCeilingHeight`; HARES
  derives its ceiling height and zone air volume from the declared volume
  (`building.rs:529-551`), so a defaulted volume would also replace those
  with an 8 ft constant, and every HPXML that OpenStudio-HPXML writes after
  applying its defaults, ResStock's included, carries the element. The
  stricter rule and its reason are recorded in `docs/PHYSICS_DECISIONS.md`.

**Change, numeric commit** (each rule cited in the code and in
`docs/PHYSICS_DECISIONS.md`, and recorded in `ResolvedModel` once
`06-runtime-memory.md` RT-6 has landed, which it has by this row):
- A missing `InfiltrationHeight` is OpenStudio-HPXML's estimate, ported
  from its defaults code with the source file and function cited.
- A missing `HasFlueOrChimneyInConditionedSpace` is OpenStudio-HPXML's rule
  quoted above, evaluated over the resolved heating systems and water
  heaters (fuel, location, AFUE or percent, energy factor).
- A missing `ShieldingofHome` is `normal` for a single-family detached or
  manufactured home and `well-shielded` for a single-family attached home
  or an apartment unit, read from `ResidentialFacilityType`; a building
  with neither element is a `MissingField` naming
  `ResidentialFacilityType`.

**Acceptance test:**
- Structural commit:
  `infiltration_without_conditioned_volume_is_an_error` (the `Building`
  parsed from `tests/fixtures/hpxml/ochre_samples/base.xml` with
  `conditioned_volume_m3` set to `None` and its ACH50 replaced by a CFM50
  of the same leakage fails solver construction naming
  `ConditionedBuildingVolume`; with its ACH50 kept and the AIM-2 path
  taken, it fails the same way);
  `missing_floors_above_grade_is_a_parse_error`;
  `unknown_shielding_or_site_type_is_a_parse_error` (`ShieldingofHome`
  `windy` and `SiteType` `coastal`, each naming the element, the value and
  the allowed values);
  `xsd_boolean_accepts_one_and_zero` (`HasFlueOrChimneyInConditionedSpace`
  `1` parses as true, `0` as false, `yes` is an error);
  `every_xsd_boolean_site_uses_parse_xsd_boolean`: a table test, one case
  per one of the twelve census sites (at `7b7a6a7d`: `building.rs:599,
  626, 1578, 2095, 2310, 2365`; `resolve_loads.rs:322, 490, 623, 989,
  991`; `resolve_water_heater.rs:478`), each parsing the element's `1` as true
  and `0` as false through its own parse path, so a site the fix misses
  fails this test instead of only `xsd_boolean_accepts_one_and_zero`'s
  single flue site;
  `legacy_flue_element_names_its_replacement`;
  `zone_without_environment_zone_is_an_assembly_error`.
- Numeric commit: `missing_infiltration_height_follows_openstudio_hpxml`
  (`base.xml`, which lacks it: the resolved height is 9.75 ft, 2.9718 m,
  within OpenStudio-HPXML's own relative 0.01, the value OpenStudio-HPXML
  v1.12.0 defaults for its `base.xml` with the height removed,
  `HPXMLtoOpenStudio/tests/test_defaults.rb`,
  `test_infiltration_height_and_volume`. The fixture's inputs the estimate
  reads match that file's: 2700 ft² conditioned floor area, two conditioned
  floors with one above grade, an 8 ft average ceiling height, a
  conditioned basement and an unvented attic outside the infiltration
  volume, `base.xml:48-54, 82-107`; the test asserts them first, so a
  fixture change cannot leave the expectation stale);
  `missing_flue_follows_openstudio_hpxml` (`base.xml`, with its furnace's
  AFUE edited in the test to 0.80 and located in conditioned space: true;
  at the fixture's own AFUE 0.92: false);
  `missing_shielding_follows_facility_type`
  (`base-bldgtype-multifamily.xml`, an apartment unit without
  `ShieldingofHome`: `well-shielded`; `base.xml`: `normal`).

**Verification:** `cargo nextest run -p hares-core -p hares-io`;
`frame_golden compare-all` after each commit. The structural commit
compares bitwise on every golden (every golden's HPXML carries every
element above). The numeric commit is expected to leave every golden
unchanged for the same reason; its delta is recorded on the fixtures that
lack the elements (`base.xml` and the two `base-bldgtype-multifamily*`
files, 7 d at 900 s with `data/examples/BEopt_example_schedule.csv` and
`USA_CO_Denver.Intl.AP.725650_TMY3.epw`, every variable before and after)
in `02-measurement.md`; every parity case passes at its existing
tolerance (each parity HPXML carries `InfiltrationHeight`, `ShieldingofHome`
and, after the structural commit, the flue element). The numeric commit
adds its `docs/python.md` "Numeric changes" line (`plan.md` §2.8): the
three OpenStudio-HPXML defaults, and the goldens re-captured or the
statement that none was. From row 94c `[golden]` also compares the
`resolved` product (`09-numeric-fixes.md` B-14 commit 1); M-1's capture
re-captures it, and the numeric commit records its resolved delta, none
expected since neither commit touches an equipment parameter.

**Commits:** structural (row 24a),
`fix: infiltration set-up fails on missing or unknown inputs instead of assuming defaults`;
numeric (row 94f in the numeric block, after the consumer block),
`fix: default infiltration height, flue and shielding by the OpenStudio-HPXML rules`.

**Dependencies:** M-2 (the goldens that prove no fixture relied on a
default). The numeric commit follows `06-runtime-memory.md` RT-6 (the
read-back it records into) and the consumer block, under
`09-numeric-fixes.md`'s rules.

## OD-11: the thermal-trace parity tests hide their own errors

**Kind:** fix (found defect, recorded in
`decision-log.md` `dl-i08-execution.br6c-notes`).

**Problem:** `tests/python/test_thermal_trace.py` reads `VENDOR_OCHRE`
in `_ensure_ochre_path` and never defines it, so the fixture that calls it
raises `NameError`. Its three tests carry `pytest.mark.xfail(strict=False)`
with no `raises=`, so that `NameError`, or any other error before the
asserted comparison, is reported as an expected failure: during
`01-baseline-repair.md` BR-6c a broken OCHRE import passed as "3 xfailed".
No other Python test uses a non-strict `xfail` (`git grep -n 'strict=False'
-- tests/python` lists only these three).

**Evidence** (at `8b09ac4a`): `tests/python/test_thermal_trace.py:83-84`
(the undefined name), `:229-232`, `:324-327`, `:375-378` (the three marks).

**Change:** `VENDOR_OCHRE` is defined from the repository root
(`Path(__file__).resolve().parents[2] / "vendors" / "OCHRE"`, as
`tests/python/generate_pvlib_solar_override.py:22-23` does). Each mark
becomes `xfail(raises=AssertionError, strict=True)`, so only the
documented divergence assertion is expected, a fixture or setup error
fails the test, and an unexpected pass fails it; each reason states the
divergence measured in this commit (zone temperature, overnight losses and
window solar gain against OCHRE), recorded in `02-measurement.md`. The
tests run their comparison for the first time since the `NameError` was
introduced, so a test may now pass; `strict=True` would turn that pass into
a failure. A test that passes in this commit loses its `xfail` mark in this
commit, and its measured agreement (the asserted quantity and its
tolerance) is recorded in `02-measurement.md` in place of a divergence.
A mark is kept only when its reason cites the reference showing that
HARES's value is the correct one (the EnergyPlus Engineering Reference or
source, or ASHRAE) or the documented OCHRE defect that explains the
difference. Any other divergence is a HARES defect handled by
`01-baseline-repair.md` BR-12's rule: a bitwise-neutral fix in this
commit, or a new `09-numeric-fixes.md` entry with the test asserting
today's value until it lands.

**Acceptance test:** `PYTHONPATH=vendors/OCHRE uv run --all-groups --no-sync pytest tests/python/test_thermal_trace.py -m "" -rxX`
(the `ochre` group installs OCHRE's dependencies and the path makes the
vendored OCHRE importable, which OD-13 later makes the block's default):
each test is either an `XFAIL` raised by its assertion or, with
its mark removed, a pass; none is an error or an `XPASS`. With
`_ensure_ochre_path` temporarily made to raise in a scratch copy of the
module, the run reports errors, not `XFAIL`s (checked once in the entry and
recorded).

**Verification:** `[pytest]` (the skip census is unchanged: where `ochre`
does not import, the module is skipped at `:45` as today).

**Commit:** `test: the thermal-trace parity tests expect only their divergence assertion`

**Dependencies:** none beyond M-1 and M-2. It lands before
`05-python-surface.md` PY-7, whose rename then carries the file.

## OD-12: the pvlib solar-override generator overwrites committed fixtures

**Kind:** fix (found defect, recorded in
`decision-log.md` `dl-i08-execution.br6c-notes`).

**Problem:** `tests/python/generate_pvlib_solar_override.py` writes its
output into the committed fixtures directory with no argument, so any run
of it, by hand or from a verification loop, rewrites the three freefloat
`pvlib_solar_override.csv` fixtures with values from whatever pvlib is
installed (pvlib 0.16 writes non-zero solar-midnight rows where 0.15 wrote
zeros). The freefloat oracle chain depends on those exact bytes.

**Evidence** (at `8b09ac4a`): `tests/python/generate_pvlib_solar_override.py:29`
(`OUTPUT_DIR = REPO_ROOT / "tests" / "fixtures" / "freefloat"`), `:174-175`
(the write); the consumers `tests/freefloat_oracle.rs`,
`tests/python/test_solar_override_boundary.py` and
`tests/python/test_py_solar_override.py:18-24`. Two run hints invoke the
generator with no output argument: the panic message of
`tests/freefloat_oracle.rs:1232` and the module docstring (`:8`), both as
`PYTHONPATH=vendors/OCHRE vendors/OCHRE/.venv/bin/python tests/python/generate_pvlib_solar_override.py`.
The module imports `numpy`, `pandas` and `pvlib` and inserts
`vendors/OCHRE` into `sys.path` at import time (`:18-25`), although it
never imports OCHRE. (Lines at `21d90588`.)

**Change:** the generator takes a required `--out DIR` and writes only
there, and prints the pvlib version it ran under (the files keep their
format, which the consumers parse). It parses its arguments before any
third-party import (the imports move into `main()` after the parse), so a
missing `--out` is reported whatever the environment, and the unused
`sys.path` insertion goes. Both run hints read
`uv run --group ochre python tests/python/generate_pvlib_solar_override.py --out <dir>`,
followed by the deliberate copy below. Replacing a committed fixture is a
deliberate copy in a commit that states the pvlib version and the reason.

**Acceptance test:** `test_pvlib_generator_requires_an_output_directory`
(Python): running the module with no arguments, in a subprocess whose
environment cannot import `pvlib` (`PYTHONPATH` pointing at a temporary
directory holding a `pvlib.py` that raises on import), exits non-zero
naming `--out`, and leaves `tests/fixtures/freefloat/` unchanged (its file
digests before and after are equal).

**Verification:** `[pytest]`; `[stray]`.

**State:** FIXED (2026-10-04, commit `901354e0`), with a found defect
fixed inside the row: the generator's blind `except Exception` had been
swallowing pvlib's call failures and writing all-zero irradiance
(pvlib 0.15's perez raised ZeroDivisionError on every dhi==0 row, so
the COMMITTED fixture's night rows carry the except's marker: aoi
0.054831, the only rows where a regeneration under the fixed code does
not reproduce the fixture; every dhi>0 row is byte-identical, and the
pvlib numerics are bit-identical across 0.15.0 and 0.16.1 by a
differential sweep). The except is deleted; the extraction handles
pvlib's scalar returns; non-finite components raise a typed error; a
second pinned test proves the generator fails loudly instead of
writing an all-zero override. Replacing the fixture to retire the
marker rows is a deliberate-copy decision for the numeric block: the
freefloat oracle consumes the zeros at night (physically correct
irradiance), so the fixture is fit for purpose as committed. Both
acceptance tests discriminate (each fails against the old code).
Pytest: 1398 passed, 4 xfailed, 0 skipped; pyright 0 errors.

**Commit:** `test: the pvlib solar-override generator writes only to a named directory`

**Dependencies:** none beyond M-1 and M-2.

## OD-13: the OCHRE comparison and fleet conversion tests never run

**Kind:** fix (found defect, `dl-i08-process.found_defects`; a test that
needs a fixture gets one, and no test stays skipped or ignored).

**Problem:** `ledger/pytest-skip-census.txt` fixes nine skips as the
accepted state of `[pytest]`, and nothing makes them run.
- Three modules skip at `pytest.importorskip("ochre")` although `[pytest]`
  installs the `ochre` dependency group. The group installs OCHRE's
  dependencies, not OCHRE: OCHRE is the `vendors/OCHRE` submodule (version
  0.9.2), and nothing puts it on `sys.path` before the module-level skip
  runs. With it on the path, OCHRE imports in the block's environment
  (pint 0.24.4, numpy 2.5.3, pandas 3.0.6; checked at `21d90588`). The
  skips hid the OCHRE comparison from the gate: during
  `01-baseline-repair.md` BR-6c a pint release that breaks OCHRE passed
  the block unseen.
- Six fleet tests skip because their fixture reads
  `tests/fixtures/resstock_metadata.parquet`, `building_energy_models/`
  and `weather/`, none of which exist, through `Fleet.from_resstock`,
  which cannot build a runnable fleet today (`09-numeric-fixes.md` B-9a).
  What the six tests check (the fleet's aggregate and per-home frames
  reach Polars with the expected shape) does not depend on that
  constructor.
- Behind the module skips, five tests are skipped unconditionally and
  assert nothing, two more assert nothing, and two skip themselves (at
  three sites) when a column is missing, so making OCHRE importable alone
  would only move the skips.
- CI's `slow-python-test` job runs the `ochre` marker without checking out
  the submodule.

**Evidence** (at `21d90588`): `tests/python/test_ochre_parity.py:190`,
`tests/python/test_observer_diagnostics.py:89`,
`tests/python/test_thermal_trace.py:45` (the module skips);
`test_thermal_trace.py:83-84` (`_ensure_ochre_path`, reached only after the
skip); `pyproject.toml` `[dependency-groups] ochre` (OCHRE's dependencies,
no OCHRE); `tests/python/test_py_conversions.py:52-74` (the fleet fixture
and its three missing paths) and `:164-214` (the six tests);
`test_observer_diagnostics.py:92`, `:180`, `:224`, `:255`, `:300` (five
`pytest.mark.skip("diagnostic visualization tool, not regression test")`,
no `assert` in the file); `test_ochre_parity.py:203`
(`test_print_comparison`, "never fails") and `:348`
(`test_7day_benchmark`), neither with an `assert`;
`test_ochre_parity.py:252-256` and `test_thermal_trace.py:389-392` (skips
on a missing column); `.github/workflows/ci.yml:211-234` (no submodule
step); `02-measurement.md`, BR-6c record (the hidden pint break).

**Change:**
- `pyproject.toml` `[tool.pytest.ini_options]` gains
  `pythonpath = ["vendors/OCHRE"]`, so `import ochre` resolves to the
  vendored reference in every pytest run, its dependencies coming from the
  `ochre` group. The module-local `sys.path` insertions of the test modules
  go (`test_ochre_parity.py:56-57`, `_ensure_ochre_path`); scripts run
  outside pytest keep theirs. The module-level `importorskip("ochre")`
  guards stay for a developer environment without the `ochre` group; the
  block's environment has it, and the empty census makes any such skip
  fail `[pytest]`.
- Tests that assert nothing are deleted, as `01-baseline-repair.md` BR-8
  deleted their Rust counterparts: `test_observer_diagnostics.py` (its five
  tests and helpers), `test_print_comparison`, `test_7day_benchmark` and
  the helpers only they use (`_run_ochre_7d`, `_run_hares_7d`). A timing
  comparison with OCHRE is a measurement, not a test.
- The three missing-column skips become assertion failures naming the
  column and the simulator that lacks it.
- The `fleet` fixture builds `Fleet.from_buildings` from one
  `DwellingConfig`: `tests/fixtures/resstock/2025.1/bldg0000002/home.xml`
  and `in.schedules.csv`, weather
  `tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv`, start
  `2018-01-01T00:00:00-05:00`, one day at 3600 s, no warm-up,
  `write_output=True` with the output under the test's `tmp_path` (with
  `False` the engine skips the output pipeline and the frames the tests
  assert on do not exist), the repository's `defaults/`. The three constants
  naming missing paths go. The six tests keep their assertions.
- `ci.yml`'s `slow-python-test` job gains the `test` job's
  `git submodule update --init vendors/OCHRE` step.
- `ledger/pytest-skip-census.txt` is emptied in this entry's verification,
  with the reason in the `02-measurement.md` log.
- A test that fails once it runs is a defect report. A fix that is
  bitwise-neutral is made here; a fix that moves numbers follows
  `01-baseline-repair.md` BR-12's rule (a new `09-numeric-fixes.md` entry
  in the numeric block; the test lands here asserting today's value).
  `test_thermal_trace.py`'s strict `xfail`s (OD-11) are the declared
  divergences and run as `XFAIL`; a test OD-11 found passing has no mark
  and runs as a pass. A mark is kept only when its reason cites the
  reference (EnergyPlus Engineering Reference or source, ASHRAE) showing
  HARES's value is correct, or the documented OCHRE defect; any other
  divergence is a defect handled by the rule of this bullet.

**Acceptance test:** the block's `[pytest]` line passes with an empty
census and its log holds no `SKIPPED` line; in that log
`test_ochre_parity.py::test_parity` runs its nine cases,
`test_thermal_trace.py` reports each of its three tests as OD-11 left it
(an `XFAIL` raised by its assertion, or a pass), and the six `test_py_conversions.py` fleet tests pass. Run once as CI's job
does, `uv run --all-extras --all-groups --no-sync pytest -m "slow or ochre"`
passes with no skip.

**Verification:** whole block; the OCHRE parity metrics recorded in the
`02-measurement.md` log as the first reference record of `plan.md` §3's
physics rule.

**State:** FIXED (2026-10-04, twelve parity cases not nine: Water
Heating, Lighting, and Other joined when the comparison's silent
narrowing was found; eleven pass, the Lighting case is a strict xfail
citing the measured pair and OD-47's cause). The census file is empty;
the module guards remain for environments without the ochre group.
The fleet fixture builds from one committed ResStock 2025.1 building
(`Fleet.from_buildings`, one day at 3600 s, `write_output=True`: with
`False` the engine skips the whole output pipeline and the frames the
tests assert on do not exist, so the entry's literal `write_output=False`
was unsatisfiable together with its own acceptance). The aggregate and
per-dwelling frames are pinned to exact shapes, the sample weight and
the aggregation math to a relative error of 1e-12. The column mapping
lives in one shared module (tests/python/ochre_names.py); a third copy
in scripts/bench_4month_der.py is a later pass. The six fleet tests and
the parity module carry the ochre marker: the default job deselects
them, CI's slow job (which now checks out vendors/OCHRE) runs them.
The deleted assert-nothing tests: test_observer_diagnostics.py (347
lines, five skip-marked tests, no assertion anywhere), test_print_comparison,
test_7day_benchmark, and their private helpers; the timing comparison's
live successor is scripts/bench_4month_der.py. OD-32's HELICS
worker-crash flake was observed on a second test in that module
(test_multi_rate_dwelling_steps_only_at_own_period; intermittent under
xdist, green standalone, module untouched by this row).

**Commit:** `test: run the OCHRE comparison and fleet conversion tests instead of skipping them`

**Dependencies:** OD-11 (the thermal-trace tests fail on their own errors
before they start running in the block), OD-12. Lands before
`05-python-surface.md` PY-7, whose rename carries these files, and before
OD-14.

## OD-14: pint is held below its latest release for the OCHRE reference

**Kind:** fix (`dl-i08-architecture.dependencies`: every dependency at its
latest release, with every incompatibility fixed, not suppressed).

**Problem:** `01-baseline-repair.md` BR-6c holds pint at
`>=0.24.4,<0.25`. pint 0.25 renamed the unit the vendored OCHRE uses for
infiltration pressure: 0.24.4 defines
`inch_H2O_39F = inch * water_39F * g_0` with
`water_39F = 0.999972 * kilogram / liter`, and the current registry defines
`inch_H2O_4C = inch * water_density_4C * g_0` with
`water_density_4C = 999.972 kg/m^3` (`pint/default_en.txt`,
`pint/constants_en.txt`), the same quantity under a new name. OCHRE is not
HARES code and is not edited; HARES owns the environment it runs OCHRE in.

**Evidence:** `pyproject.toml` `[dependency-groups] ochre` (the cap and its
comment); `vendors/OCHRE/ochre/utils/units.py:5` (OCHRE's module registry
`ureg = UnitRegistry()`); `vendors/OCHRE/ochre/utils/envelope.py:551`,
`:595`, `:600`, `:622`, `:623` (the five conversions);
`02-measurement.md`, BR-6c record (0.25.2 and 0.26.1 raise
`UndefinedUnitError` at OCHRE's `Dwelling` construction).

**Change:**
- `tests/python/ochre_units.py` (new): `register_removed_units(ureg)`
  defines `inch_H2O_39F = inch_H2O_4C` on the registry it is given. It is
  called on `ochre.utils.units.ureg` right after `ochre` is imported, before
  any OCHRE dwelling is built, by every module that runs OCHRE in the HARES
  environment: the two test modules and the scripts that put
  `vendors/OCHRE` on `sys.path` (`tests/python/generate_parity_reference.py`,
  `generate_conditioned_oracle.py`, `generate_freefloat_oracle.py`,
  `parity_diagnostics.py`, `bench_4month_der.py`,
  `tests/fixtures/parity/extract_ochre_rc.py`,
  `scripts/ochre_bestest_600.py`). The two scripts documented to run under
  OCHRE's own environment (`tests/python/extract_ochre_rc.py`,
  `tests/python/dump_ochre_energy_balance.py`, with
  `vendors/OCHRE/.venv`, whose pinned pint still defines the name) do not
  call it. Redefinition is an error: pint's registry warns and replaces on
  a redefinition by default (`on_redefinition="warn"`,
  `pint/facets/plain/registry.py`), and OCHRE builds its registry with
  that default, so the helper checks `"inch_H2O_39F" in ureg` before
  defining and raises naming the unit and the installed pint version when
  the registry already has it. A pint release that defines the name again
  therefore fails the OCHRE modules, which is the signal to delete the
  helper.
- `pyproject.toml`: the pint requirement becomes `>=` its latest release,
  with no cap and no comment; `uv lock --upgrade-package pint`.

**Acceptance test:**
- `test_inch_h2o_39f_matches_the_pint_0_24_definition`
  (`tests/python/test_ochre_units.py`): on a fresh `pint.UnitRegistry()`
  after `register_removed_units`, `Quantity(1, "inch_H2O_39F").to("Pa")`
  equals `0.0254 * 999.972 * 9.80665` Pa within a relative 1e-12 (the
  249.0819355 Pa pint 0.24.4 gives); on a registry that already defines
  `inch_H2O_39F` (defined in the test first), `register_removed_units`
  raises naming the unit and leaves the existing definition in place.
- `[pytest]` passes on the uncapped lock with the census empty, the OCHRE
  modules of OD-13 running.

**Verification:** whole block. The nine OCHRE parity metrics are recorded

**State:** FIXED (2026-10-04, commit `699cfad2`, pint 0.24.4 to
0.26.1). One reconciliation of this entry's letter, found by review:
tests/fixtures/parity/extract_ochre_rc.py is a vendored-venv script
(its docstring says to run it from vendors/OCHRE, whose pinned pint
0.23 natively defines the name, where the helper's guard raises and
the alias's source name does not exist), so despite appearing in this
entry's caller list it does NOT call the helper; the caller/non-caller
split follows the environments, not the entry's list. The guard is
re-entrant for its own registrations (the two OCHRE test modules share
one registry per xdist worker) and raises on a foreign one, naming the
unit and the installed pint version: the delete-the-helper signal.
The quantity identity holds to the ULP: the alias's base-unit scale is
bitwise pint 0.24.4's, the Pa conversion differs from 0.24.4's native
path by one ULP through pint 0.26.1's own machinery (a control
experiment defining 0.24.4's exact water constant under 0.26.1 shifts
the same ULP: the drift is pint's, not the alias's), and the full
one-hour parity dwelling under 0.26.1 is bitwise identical to 0.24.4's
across all twelve end uses (MELs 0x1.125839d10d1dep-3 on both); the
72 h freefloat envelope's indoor flow and heat gain move at most
4 ULP, the same machinery drift. The five OCHRE conversion sites
resolve under 0.26.1 with the alias. Census: 1400 passed, 4 xfailed,
0 skipped; pyright 0 errors; B0 identical; goldens bitwise identical.
before (pint 0.24.4) and after (latest) in the `02-measurement.md` log;
the unit's value is unchanged, so any difference in OCHRE's output stops
the entry until its cause is found.

**Commit:** `build: lift the pint cap and register the pressure unit the OCHRE reference needs`

**Dependencies:** OD-13 (the OCHRE tests run in the block, so they prove
the lift), `01-baseline-repair.md` BR-6c. Lands before
`05-python-surface.md` PY-7.

## OD-15: a January run books energy to cooling

**Kind:** investigation of a found defect (`dl-i08-process.found_defects`),
with a fix entry if the cause is a defect.

**Problem:** the 2024.2 `bldg0000002` winter case (Idaho, weather
station `G1600270`; 72 h from 2018-01-15 at 3600 s, one warm-up day;
the seasonal test `01-baseline-repair.md` BR-12 compiles; an earlier
draft of this entry misnamed the state and station as "Connecticut,
`G0900090`", corrected by the investigation) books 3.45 kWh to
`hvac_cooling` (measured at `8b09ac4a`). Nothing in the run explains a
cooling load in mid-January. The home's central air conditioner declares
`CrankcaseHeaterPowerWatts` 50.0, and 3.45 kWh over 72 h is a mean of
48 W, so the likely source is the crankcase heater; whether it is, whether
it runs when the references run it, and whether its energy belongs to the
cooling end use are not established.

**Evidence:** `tests/fixtures/resstock/2024.2/bldg0000002/home.xml:653-674`
(the `CoolingSystem`, `CrankcaseHeaterPowerWatts` at `:672`);
`crates/hares-equipment/src/hvac/cooling_config.rs:56-64`
(`crankcase_heater_kw`, `crankcase_heater_threshold_c`,
`crankcase_capacity_curve_coeffs`); the seasonal test's inputs in
BR-12.

**Change:**
1. Reproduce with the seasonal test's inputs: per step, the air
   conditioner's electric power by component (compressor, fan, crankcase
   heater) and the outdoor dry-bulb, summed to the run's `hvac_cooling`.
   Every kWh is attributed to a component.
2. Compare each contributing component with the references: when
   EnergyPlus's DX cooling coil runs its crankcase heater (the outdoor
   temperature limit and the compressor-off condition) and which end use
   its electricity meters to (EnergyPlus Engineering Reference and
   Input Output Reference for `Coil:Cooling:DX`), and OpenStudio-HPXML's
   crankcase heater defaults. The findings, with the citations, go into the
   `02-measurement.md` log.
3. If every kWh is the crankcase heater, operating and attributed as the
   references do, this entry commits a test that pins it:
   `winter_cooling_energy_is_the_crankcase_heater` in
   `tests/resstock_smoke.rs`: the run's `hvac_cooling` equals the sum of
   the crankcase heater's energy over the run, and the heater draws only on
   steps whose outdoor dry-bulb is below its threshold with the compressor
   off.
4. Otherwise the cause is a defect. This entry commits the reproduction as
   a test that asserts today's value, with a doc comment stating the
   behaviour the reference requires, and adds the fix to this file as
   OD-16, a numeric entry with its own row in the numeric block before
   `08-consumer.md` CN-3 (`00-schedule.md` contingent rows), under
   `09-numeric-fixes.md`'s rules; OD-16 inverts the assertion.

**Acceptance test:** the log holds the per-component attribution and the
reference comparison, and either
`winter_cooling_energy_is_the_crankcase_heater` passes or the reproduction
test passes on today's value and OD-16 exists in this file and the
schedule.

**Verification:** whole block; no golden changes (a test-only commit).

**Commit:** `test: pin where a January run's cooling energy comes from`

**Dependencies:** `01-baseline-repair.md` BR-12 (the seasonal tests
compile).

## OD-17: missing default schedule profiles become full load around the clock

**Kind:** fix (found defect, `dl-i08-process.found_defects`; silent
defaults, against `dl-i08-architecture.config`'s strictness). Structural:
bitwise on every fixture.

**Problem:** the default schedule profiles fail silently at every level.
- `load_default_profiles` warns and returns an empty map when
  `Default Schedule Parameters.csv` cannot be read. Within the file it
  skips a row with fewer than five fields, drops a value that does not
  parse, skips a row whose value count is not 24 (fractions) or 12
  (monthly multipliers), gives a profile with no weekend row its weekday
  fractions, and gives a profile with no monthly row twelve multipliers of
  1.0 (the file's `Ceiling Fan` profile has no `month_multipliers` row).
- `generate_schedule_from_hpxml`, the generated-schedule path
  `06-runtime-memory.md` RT-4 and OD-9 build on, then fills every load and
  hot-water column whose profile is missing with a constant 1.0, full load
  around the clock; with no defaults directory at all
  (`config.defaults_path` is `None`, allowed until `09-numeric-fixes.md`
  B-10) every column is 1.0. B-10 makes a `DefaultsStore::load` failure an
  error; this file is read separately and B-10 does not catch it.
- The generator pushes two columns named `occupants`: a constant 1.0 and
  the `Occupancy` profile from `COLUMN_TO_PROFILE`. The name index keeps
  the second, so the doc comment's "occupancy is constant 1.0" is false,
  and the column count (14) differs from the index's and from
  `column_aggregations`' length (13), so a downsample of the generated
  schedule would drop its last column, `cooling_setpoint`, while the index
  still points at it (`schedule.rs:286` zips the columns with their
  aggregations).
- The schedule resolver reads the same loader for schedule-file runs. When
  appending a derived power column fails it falls back to a constant 0.0 kW
  with a log line, and an equipment with no schedule column, no HPXML
  profile and no default profile (`Freezer`, `Microwave` and
  `Basement Lighting` have no row in the file) runs at a constant power
  under a "THIS MAY BE INCORRECT" log line.

**Evidence** (at `7b7a6a7d`): `crates/hares-io/src/schedule_resolve.rs:342-437`
(`load_default_profiles`: the empty map at `:346-355`, the skipped rows and
values at `:366-368`/`:383-400`, the weekend and monthly substitutions at
`:406-407`), `:554` (the resolver's load), `:996-1011` (the 0.0 kW
fallback), `:1028-1034` (the constant-power fallback);
`crates/hares-io/src/hpxml_schedule.rs:31-35` (the false doc comment),
`:43` (`defaults_dir.map(load_default_profiles).unwrap_or_default()`),
`:52-54` (the constant `occupants`), `:16-17` (the `Occupancy` row),
`:57-65` (the 1.0 fallback), `:92-99` (the index and aggregation lengths);
`crates/hares-io/src/schedule.rs:286`;
`crates/hares-core/src/dwelling/blueprint.rs:52-62` and
`crates/hares-core/src/dwelling/mod.rs:1937-1941` (the callers, passing
`config.defaults_path.as_deref()`); `defaults/Default Schedule Parameters.csv`
(23 profiles; `Ceiling Fan` with no `month_multipliers` row).

**Change:**
- `load_default_profiles(dir) -> Result<DefaultProfiles, HaresError>`:
  an unreadable file is an error naming its path; a row with fewer than
  five fields, a value that does not parse, or a value count other than 24
  or 12 is an error naming the file, the line, the profile and the
  element; every profile needs its `weekday_fractions`,
  `weekend_fractions` and `month_multipliers` rows, and a missing one is an
  error naming the profile. The file gains the `Ceiling Fan`
  `month_multipliers` row with twelve values of 1.0, the multipliers the
  loader substitutes today, its `Data Source` cell stating that the profile
  has no monthly variation.
- `DefaultProfiles::get(name) -> Result<&DefaultScheduleProfile, HaresError>`
  names the missing profile and the file. The generator takes
  `&DefaultProfiles` in place of `Option<&Path>`, and its callers load the
  profiles from the config's defaults directory; a generated schedule with
  no defaults directory is a construction error naming the missing
  directory and the file the generator needs. Each `COLUMN_TO_PROFILE`
  lookup uses `get`, so the constant 1.0 goes.
- The constant `occupants` column is deleted and the doc comment states
  that occupancy follows the `Occupancy` profile. The generated schedule's
  columns, names and aggregations are pushed together, one per column, so
  their lengths cannot differ.
- In the resolver, a failed derived-column append is an error naming the
  equipment, and an equipment with no schedule column, no HPXML profile and
  no default profile is an error naming the equipment and the three
  sources looked for, in place of the constant-power fallback.
- From `06-runtime-memory.md` RT-4 on, the profiles are a table of the
  `ParsedDefaults` handle, read once and shared (RT-4's change).

**Acceptance test:**
- `generated_schedule_without_defaults_file_is_an_error`: `base.xml` with
  no schedule file and a copy of `defaults/` in a temporary directory from
  which `Default Schedule Parameters.csv` is removed fails construction
  naming the file.
- `missing_default_profile_is_an_error`: the copy with the `Indoor
  Lighting` rows removed fails naming `Indoor Lighting`.
- `malformed_default_profile_row_is_an_error`: the copy with one
  `weekday_fractions` row cut to 23 values fails naming the line.
- `generated_schedule_requires_a_defaults_directory`: no defaults directory
  and no schedule file fails naming the missing directory.
- `generated_schedule_has_one_occupants_column`: the generated schedule's
  column names are unique, its three per-column vectors have one length,
  and `occupants` equals the `Occupancy` profile series.
- `equipment_without_any_schedule_source_is_an_error`: a resolved spec set
  with a `Freezer` and a schedule file with no `freezer` column and no
  HPXML profile fails naming `Freezer`.
- `11-other-defects.md` OD-6's `no_schedule_generates_from_hpxml` still
  equals today's generated schedule on every column the index names.

**Verification:** `cargo nextest run -p hares-io -p hares-core`; `uv run
pytest tests/python`; `frame_golden compare-all` (no golden runs a
generated schedule, and every golden home's schedule file names every
mapped column it needs; any difference stops the entry). Tests that
generate a schedule with no defaults directory, or reach the
constant-power fallback, pass a defaults directory or a schedule column in
this commit (census: the callers of `generate_schedule_from_hpxml`, and a
unit test over every checked-in HPXML that lists the specs reaching the
fallback, recorded in the commit body).

**Commit:** `fix: missing or malformed default schedule profiles are errors, not full load`

**Dependencies:** OD-6 (the explicit request for a generated schedule).
Lands before `06-runtime-memory.md` RT-4, which moves the profiles into
`ParsedDefaults`, and before OD-9.

## OD-18: action clipping is never counted in `RunHealth`

**Kind:** fix (a finding against committed `03-output-reporting.md` OR-5,
`dc033cee` and `469ea955`, which promised this deletion and call;
`dl-i08-architecture.health`: every clamp is recorded in every build).

**Problem:** the gym's action clipping is counted only in two
process-global statics compiled under `observe`, and never reaches the
run's health. `ACTIONS_CLIPPED` and `ACTIONS_CLIPPED_WARNED` are still
declared and incremented process-wide; `Dwelling::record_clamped_actions`
exists and nothing calls it, so `RunHealth.clamped_actions` is always 0
(every golden records `"clamped_actions": 0`). `HealthPolicy`'s
`max_clamped_actions` (`03-output-reporting.md` OR-12) can therefore
never fire, and `health()` in Python (`05-python-surface.md` PY-9)
under-reports. The pure-Python gym paths clip with `np.clip` against the
action space and count nothing, with a second copy of the bound table
(`_field_bounds` in `gym_env.py`, documented as matching the Rust
`field_bounds`).

**Evidence** (at `21d90588`): `crates/hares-python/src/py_gym.rs:56-80`
(`field_bounds`), `:206-212` (the two statics), `:219-309`
(`map_action_to_signals`; the count under `observe` at `:233-249`, the
process-wide increment and warn-once at `:274-288`), `:423` (the batch
path's call); `crates/hares-core/src/dwelling/mod.rs:4826` at `7b7a6a7d`
(`4631` at this entry's pinned `21d90588`; `record_clamped_actions`);
`git grep -n record_clamped_actions -- crates
python` (only the definition and its doc reference);
`python/ochre_next/rl/gym_env.py:521-524` and
`python/ochre_next/rl/vec_env.py:162-165` (`np.clip`), `gym_env.py:200`
(`_field_bounds`).

**Change:**
- The two statics and the warn-once log are deleted. One function,
  `clamp_action(action, action_layout) -> (Vec<f64>, u64)`, clamps each
  value to `field_bounds` and counts the clamped values in every build;
  `map_action_to_signals` uses it.
- `batch_step` calls `record_clamped_actions` on each environment's
  dwelling with that environment's count before applying its signals.
- The pure-Python paths call a new binding,
  `hares._hares.clip_action(dwelling, action, action_layout) -> list[float]`,
  which runs `clamp_action`, records the count on the dwelling and returns
  the clamped values, in place of `np.clip`; they keep building their
  signals from the returned values. `_field_bounds` stays only to build the
  declared action space, and a test pins it to the Rust table.
- `docs/python.md`'s RL section states that clamped action values are
  counted in `health().clamped_actions`.

**Acceptance test:**
- `batch_step_records_clamped_actions` (`hares-python` unit test, a
  default build without `observe`): two `base.xml` dwellings stepped once
  through `batch_step` with an action layout of one pair, the gas
  furnace's `heating_setpoint_c`, the first row 100.0 (above the 80.0
  bound) and the
  second 20.0: the first dwelling's `health().clamped_actions` is 1 and
  the second's is 0. At the parent both are 0.
- `clip_action_records_and_returns_clamped_values` (`hares-python` unit
  test): the same action through `clip_action` returns `[80.0]` and
  records 1.
- `test_python_action_bounds_match_the_engine` (Python): for every field
  `field_bounds` names, `_field_bounds` returns the same bounds as the
  binding's table.
- `git grep -n 'ACTIONS_CLIPPED' -- crates` prints nothing.

**Verification:** `cargo nextest run -p hares-python`; `cargo nextest run
-p hares-python -F observe`; `uv run pytest tests/python -k gym`;
`frame_golden compare-all` (no golden steps a gym).

**Commit:** `fix: count clamped RL actions in each run's health in every build`

**Dependencies:** `03-output-reporting.md` OR-5 (`RunHealth`,
`record_clamped_actions`); `05-python-surface.md` PY-7 (the
`python/hares/rl/` paths) and PY-5 (the same files). Lands before
`03-output-reporting.md` OR-12, whose `max_clamped_actions` then counts
real events, and before `06-runtime-memory.md` RT-5, whose census it
closes.

## OD-19: a malformed HPXML setpoint profile or a declared setback is ignored

**Kind:** fix (found defect, `dl-i08-process.found_defects`; silent paths in
the parser `12-inputs.md` IN-5 and OD-9 build on). Structural: bitwise on
every fixture.

**Problem:** `parse_setpoint_from_control` reads a
`WeekdaySetpointTemps{Heating,Cooling}Season` or
`WeekendSetpointTemps{Heating,Cooling}Season` extension by splitting on
commas and dropping every token that does not parse; when the remaining
count is not 24 it falls back to the constant
`SetpointTemp{Heating,Cooling}Season` without a word, or to no profile. A
profile with one
misspelt value among 25 is accepted with the rest shifted by an hour. The
HPXML setback and setup elements (`SetbackTempHeatingSeason`,
`TotalSetbackHoursperWeekHeating`, `SetupTempCoolingSeason`,
`TotalSetupHoursperWeekCooling`) are read nowhere in `hares-io`, so a
declared setback is dropped and the dwelling runs at the constant
setpoint. A constant `SetpointTemp{Heating,Cooling}Season` element that is
present but does not parse is also dropped (`xml_helpers.rs:237-245`), so
that side falls through to no profile.

**Evidence** (at `21d90588`): `crates/hares-io/src/hpxml/xml_helpers.rs:214-247`
(the token filter `filter_map(parse_trimmed_f64)`, the `vals.len() == 24`
test and the constant fallback); callers `crates/hares-io/src/hpxml/building.rs:1263`
and `crates/hares-io/src/hpxml/resolve_hvac.rs:3730-3731`;
`git grep -n 'Setback\|Setup' -- crates/hares-io/src` prints nothing.
Census: the 156 profile elements in the checked-in HPXML files (49 files)
all hold 24 numeric values, the 47 constant setpoint elements (34 files,
at `afa9a071`) all hold a number, and no checked-in file declares a
setback or setup element.

**Change:**
- A profile element that is present must hold 24 values that each parse;
  otherwise the parse fails naming the element, the value count and the
  first token that does not parse. A constant setpoint element that is
  present must parse; otherwise the parse fails naming the element and its
  text.
- A thermostat that declares a setback or setup element fails the parse
  naming the element: HARES reads a schedule of setpoints only as the
  24-hour weekday and weekend profiles (the extension above, which
  OpenStudio-HPXML also accepts), and the error message names that form.
  Applying OpenStudio-HPXML's setback rule is a capability HARES does not
  have; the parsed thermostat is its seam, and rejecting the input keeps
  "accepted means applied" (`12-inputs.md` IN-3 lists the elements the
  parser reads).
- `tracing` at `debug`: the profile source per side (extension or
  constant).

**Acceptance test:**
- `setpoint_profile_with_wrong_count_is_an_error`: `base.xml` with a
  23-value `WeekdaySetpointTempsHeatingSeason` added in the test fails
  naming the element and the count 23.
- `setpoint_profile_with_a_bad_token_is_an_error`: 25 tokens of which one
  is `abc` fails naming `abc` (today 24 values are accepted, shifted).
- `setpoint_constant_with_a_bad_value_is_an_error`: `base.xml` with its
  `SetpointTempHeatingSeason` text replaced by `68F` in the test fails
  naming the element and `68F` (today the heating side silently has no
  setpoint profile).
- `declared_setback_is_rejected`: `base.xml` with
  `SetbackTempHeatingSeason` and `TotalSetbackHoursperWeekHeating` added
  fails naming `SetbackTempHeatingSeason`.

**Verification:** `cargo nextest run -p hares-io`; `frame_golden
compare-all` (bitwise; the census above shows no fixture changes).

**Commit:** `fix: reject malformed HPXML setpoint profiles and setbacks HARES does not model`

**Dependencies:** none. Lands before OD-9 and before `12-inputs.md` IN-5,
which read these profiles.

## OD-20: PV roof coverage reads untyped keys and substitutes defaults

**Kind:** fix, numeric (found defect, `dl-i08-process.found_defects`,
found while verifying the ledger): its own commit and its own
row in the numeric block (`00-schedule.md` row 94i), following
`09-numeric-fixes.md`'s rules; delta recorded.

**Problem:** the dwelling decides which roof a PV array covers, and by how
much, from the PV spec's untyped JSON parameters rather than its typed
`PvConfig`, and fills every missing value with a constant: a PV without
`tilt_deg` or `azimuth_deg` is taken as 20° and 180°, a roof boundary
without an azimuth as 180° and one without a tilt as 0°, and a roof whose
area cannot be read as 1.0 m². The PV code's roof orientation is its own:
the envelope treats a roof without an azimuth as omnidirectional
(azimuth-averaged Perez irradiance, `environment.rs:934-997`), and a
boundary without a tilt as 90°, so the roof a PV is matched to and the
roof surface whose solar it scales can disagree. The coverage fraction is
silently clamped to `[0.0, 1.0]` (`environment.rs:376-379`,
`set_pv_roof_coverage`) when the sum of attached PV collector areas
exceeds the roof's area, so an over-subscribed roof is hidden instead of
reported. The coverage step never registers coverage for an HPXML PV; the
cause was not pinned.
`09-numeric-fixes.md` B-14's equipment config commit renames the key the
step reads (`capacity_kw` to `capacity_w`) and keeps that read working,
coverage unchanged, until this entry replaces it with the typed config.

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:1485-1508`
(`register_pv_surfaces`: `tilt_deg` and `azimuth_deg` read from JSON with
`unwrap_or(20.0)` and `unwrap_or(180.0)`), `:1513-1566`
(`attach_pv_to_roofs`: the same fallbacks for the PV, `b.azimuth_deg.unwrap_or(180.0)`
and `b.tilt_deg.unwrap_or(0.0)` for the roof, a 15° azimuth and 10° tilt
tolerance, and
`attached_boundary_id` written into the JSON parameters), `:1570-1600`
(`register_pv_roof_shading`: `capacity_kw` read from JSON, roof area
`unwrap_or(1.0)`, 2 m² of collector per 420 W, coverage keyed by the
boundary index and summed without a bound), call sites `:2244-2249`;
`crates/hares-core/src/environment.rs:376` (`set_pv_roof_coverage`) and
`:640-649` (the coverage scales the irradiance of the surface with that
id), `:925-997` (`build_surface_geometry`: tilt `unwrap_or(90.0)`, a
roof without an azimuth registered `omni_directional: true`), `:599-637`
(an omnidirectional surface takes `omni_directional_irradiance`, averaged
over azimuths). `tests/fixtures/hpxml/ochre_samples/base-pv.xml` has one
roof of 1509.3 ft² (140.2 m²) with no `Azimuth` and `Pitch` 6 (26.6°),
and two PV systems (180°, 20°, 4000 W and 90°, 20°, 1500 W).

**Change:**
- The test below is written first and run at the parent; its recorded
  result (for each PV of `base-pv.xml` and of the PV goldens' homes
  `bldg0176227` and `bldg0352714`: the roof it attaches to or why none,
  and the coverage registered on the surface id the environment scales)
  is the entry's triage, stated in the commit body.
- The three functions read the PV's typed `PvConfig` (capacity, tilt,
  azimuth) through the spec's typed config, and the attachment is a field
  of the resolved PV, not a JSON key. A PV without a tilt or an azimuth
  is an assembly error naming the PV and the HPXML element
  (`ArrayTilt`, `ArrayAzimuth`). A roof is matched by the orientation the
  envelope already uses for that roof's solar gains, read from the
  envelope's registered surface geometry (tilt, azimuth, omnidirectional
  flag), so the PV and the roof it shades agree; the PV code holds no
  orientation of its own and no constant stands in for a missing value.
- Matching rule: a roof with an azimuth matches a PV within 15° of azimuth
  and 10° of tilt, the closest by summed difference. An omnidirectional
  roof (no `Azimuth`) has no azimuth to compare, because the envelope
  averages its solar gain over every azimuth, so it matches by tilt alone:
  every PV within 10° of its tilt attaches to it, whatever the PV's
  azimuth, and their coverages sum on that roof.
- A PV that matches no roof is attached to none, and the run's warning log
  names the PV and the nearest roof's azimuth (when it has one) and tilt
  differences. Coverage is registered on the surface id the
  environment scales. A roof whose summed coverage exceeds 1 is an
  assembly error naming the roof, its area and the PVs on it.
- The collector area per watt is a named constant with its source in
  `docs/PHYSICS_DECISIONS.md` (a section: the attachment rule, the
  coverage rule and what OCHRE does).
- `tracing` at `debug`: each PV's attachment decision with the
  orientations compared.

**Acceptance test:**
- `hpxml_pv_registers_roof_coverage` (`crates/hares-core`):
  `base-pv.xml`, whose roof is omnidirectional: both arrays (tilt 20°
  against the roof's 26.6°) attach to the roof, no warning names either,
  and the environment's coverage of that roof's surface equals
  (4000 W + 1500 W) times the collector area per watt divided by the
  roof's area.
- `pv_on_an_oriented_roof_matches_by_azimuth`: `base-pv.xml` with
  `<Azimuth>180</Azimuth>` added to its roof in the test: the 180° array
  attaches and the roof's coverage equals 4000 W times the collector area
  per watt divided by the roof's area; the 90° array attaches to no roof,
  and the run's warning log names it with an azimuth difference of 90°.
- `pv_without_orientation_is_an_assembly_error`: `base-pv.xml` with the
  first PV's `ArrayTilt` removed in the test fails naming the PV and
  `ArrayTilt`.
- `roof_coverage_above_one_is_an_assembly_error`: `base-pv.xml` (both
  arrays on its omnidirectional roof) with the first PV's
  `MaxPowerOutput` raised in the test until the two arrays' summed
  collector area exceeds the roof's fails naming the roof and both PVs.

**Verification:** `cargo nextest run -p hares-core`; goldens per M-1's
numeric entry procedure: the PV goldens (`consumer_shape_900s`,
`fleet_same_offset_300s`, `fleet_mixed_offset`, `resstock_2025_fleet`,
`der_pv_battery_900s`, `der_pv_battery_ev_900s`) are re-captured if their
roof solar moves, and the delta is recorded in `02-measurement.md`. The
commit adds its `docs/python.md` "Numeric changes" line (`plan.md` §2.8).

**Commit:** `fix: shade a roof by the PV arrays attached to it, read from their typed config`

**Dependencies:** `09-numeric-fixes.md` B-14 (the SI `PvConfig` it reads;
its equipment config commit renames the key this entry replaces);
`06-runtime-memory.md` RT-6 (the resolved PV the attachment is recorded
in); M-1 and M-2. In the numeric block it follows B-14's control-layer
commit (row 94h) and precedes `08-consumer.md` CN-3.

## OD-21: HPXML attribute handling and the time-zone lookup have no pinning test, and a malformed attribute is dropped

**Kind:** fix (found defect, `dl-i08-process.found_defects`; the two notes
the red team of `01-baseline-repair.md` BR-6b recorded in the
`02-measurement.md` log, and the silent attribute path found while
checking them). Structural: bitwise on every fixture (`00-schedule.md`
row 24h).

**Problem:**
- BR-6b moved quick-xml to 0.42, which normalizes attribute values and
  text line endings by XML 1.0 (a literal tab or line break in an
  attribute value becomes a space, a character reference such as `&#9;`
  keeps its character, CR LF in text becomes LF), where 0.39 did not. No
  fixture contains such input, so no golden moved, and no test pins the
  behaviour the parser now has.
- The same parser drops a malformed attribute without a word:
  `tag.attributes().flatten()` discards every attribute the reader
  rejects (a duplicate name, a missing quote), and
  `normalized_value(..).unwrap_or_default()` turns a value that cannot be
  unescaped (an undefined entity) into the empty string. An HPXML element
  then parses as if the attribute were absent or empty, against
  "accepted means applied".
- BR-6b also moved the tzf-rs data release (2026-b-fix1 to 2026-d-fix1).
  The site UTC offset falls back to that lookup when neither the caller,
  the HPXML `Site/TimeZone/UTCOffset` nor the weather file supplies one
  (an OCHRE sample HPXML with a ResStock CSV weather file). Every golden
  pins its offset explicitly, so only the four `site_location` unit tests,
  which build synthetic sites, reach the lookup; no test reads a real
  fixture's coordinates through it.

**Evidence** (at `afa9a071`): `crates/hares-io/src/hpxml/building.rs:353-460`
(`parse_xml_document`: `trim_text(true)`; `tag.attributes().flatten()`
at `:366` and `:384`; `attr.normalized_value(XmlVersion::Implicit1_0)
.map(..).unwrap_or_default()` at `:368-371` and `:386-389`;
`text.xml_content(XmlVersion::Implicit1_0)` at `:405`);
`crates/hares-io/src/site_location.rs:65-80`
(`standard_offset_from_coords`), `:319-361` (`resolve_utc_offset`: caller,
HPXML, weather, then the lookup, then the longitude estimate) and the unit
tests at `:529-563`; `crates/hares-core/src/dwelling/blueprint.rs:75`
(the blueprint resolves the site with `resolve_site_location`);
`crates/hares-io/src/resstock_csv.rs:363` (a ResStock CSV carries no
embedded location); `tests/fixtures/hpxml/ochre_samples/base.xml`
(`Latitude` 40.01, `Longitude` -105.27, no `UTCOffset`);
`02-measurement.md`, BR-6b record.

**Change:**
- An attribute the reader rejects, or a value it cannot unescape, fails
  the parse with `HpxmlError::Parse` naming the element, the attribute
  and the byte position, through the same error path the reader's other
  errors take. No attribute is dropped or emptied.
- The two tests below pin the normalization and the lookup as they stand.
  If either finds behaviour other than XML 1.0's or a fixture offset
  other than the zone's standard offset, the entry stops and the cause is
  fixed here when the fix is bitwise-neutral, or becomes a
  `09-numeric-fixes.md` entry when it moves numbers.

**Acceptance test:**
- `xml_attribute_and_line_end_normalization_follow_xml_1_0` (unit test in
  `crates/hares-io/src/hpxml/building.rs`): `parse_xml_document` of a
  document whose attribute value holds a literal tab, a literal CR LF, a
  `&#9;` and a `&amp;` reads the value with a space for the tab, one
  space for the CR LF, a tab for `&#9;` and `&` for `&amp;`; an element
  whose text holds `a` CR LF `b` reads `a` LF `b`.
- `malformed_hpxml_attribute_is_a_parse_error`: `parse_hpxml_str` of
  `base.xml` with, in separate cases, a duplicated attribute name and an
  attribute value holding an undefined entity, fails with
  `HpxmlError::Parse` naming the element and the attribute.
- `site_offset_from_the_time_zone_database_for_a_fixture_site`
  (`crates/hares-io/tests/site_location_lookup.rs`): `base.xml` read with
  `hares_io::parse_hpxml` and `tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv`
  read with `hares_io::parse_weather`, resolved with
  `resolve_site_location` and an empty override, as the blueprint does,
  give `utc_offset_h == -7.0` (America/Denver standard time) with
  `utc_offset_source == FieldSource::TimezoneLookup`.

**Verification:** whole block; `[golden]` unchanged (every checked-in
HPXML file parses as before, which the block's test lines and goldens
exercise).

**Commit:** `fix: fail the HPXML parse on a malformed attribute and pin attribute normalization and the time-zone lookup`

**Dependencies:** `01-baseline-repair.md` BR-6b. Lands directly after
OD-19 (row 24g), which edits the same parser's setpoint reading.

## OD-22: an equipment whose initialisation fails is dropped from the model with a warning

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
checking which run warnings a fleet home can carry; a silent path,
against `dl-i08-architecture.config`'s strictness and "accepted means
applied"). Structural: bitwise on every fixture.

**Problem:** dwelling assembly calls `init` on every resolved equipment.
When it fails for an equipment whose end use is not HVAC heating or
cooling, water heating, EV, battery or PV, the equipment is left out of
the dwelling, the error is logged, and the run continues with a warning:
a lighting, appliance, plug, ventilation or other load the input declares
is simply absent from the energy totals and the zone gains, and the run
reports success.

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:2593-2614`
(`match eq.init(..)`: the `is_critical` list, `tracing::error!` and
`warnings.push` of "equipment '…' init failed, skipping"); no test pins
the skip (`git grep -n 'init failed, skipping'` finds only that line). A
non-critical init error is reachable from input: `ScheduledLoad`'s
`init_from_config` rejects a `sensible_gain_fraction` plus
`latent_gain_fraction` above 1 (`crates/hares-equipment/src/scheduled_load.rs:325-330`
at `7b7a6a7d`).

**Change:**
- An `init` error fails assembly for every equipment, with
  `HaresError::Equipment` naming the equipment and its init error; the
  `is_critical` list goes.
- The entry's first step is a census: every checked-in HPXML fixture that
  builds at the parent is built once, and every equipment the skip
  removes is listed in the commit body. A fixture that relied on the skip
  is a defect of its own: its input or the resolver output behind it is
  corrected in this commit when the correction is bitwise on every golden
  and parity case, and otherwise the entry stops and the fix becomes a
  `09-numeric-fixes.md` entry.

**Acceptance test:**
- `equipment_init_failure_fails_assembly`: `base.xml` with an equipment
  override setting `sensible_gain_fraction` to 1.5 on its first
  `ScheduledLoad` (named in the test from the resolved specs), so the
  sensible plus latent fraction exceeds 1, fails
  construction naming that equipment and `sensible_gain_fraction`. At the
  parent it builds without the equipment.
- The census test above, kept as `every_fixture_builds_every_equipment`
  (`crates/hares-core/tests/`): for each checked-in HPXML fixture that
  builds, the built dwelling holds one equipment per resolved spec.

**Verification:** `cargo nextest run -p hares-core`; `frame_golden
compare-all` (bitwise; any difference means a golden relied on the skip
and stops the entry); every parity case passes at its existing tolerance.

**Commit:** `fix: an equipment whose initialisation fails stops construction instead of being dropped`

**Dependencies:** none. Consumer effect: the row's B0 check shows whether
a DE-trace home relied on the skip; any difference stops the row.

## OD-23: the shipped battery CSV is a dead legacy file

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
checking what `defaults.rs` loads for battery). Structural: bitwise on
every fixture (the file is read by no code).

**Problem:** `defaults/battery/default_parameters.csv` ships beside
`defaults/battery/*.toml`, carried over from the OCHRE source tree
(`vendors/OCHRE/ochre/defaults/Battery/default_parameters.csv`), but
battery defaults load from the TOML directory only.

**Evidence** (at `7b7a6a7d`): `crates/hares-io/src/defaults.rs:500`
(`store.battery = load_toml_dir(&defaults_dir.join("battery"))?`) and
`:764` (`fn load_toml_dir`, `std::fs::read_dir` filtered to TOML entries);
`git grep -n 'default_parameters.csv'` finds the file only referenced in
a comment at `:2103-2104` (the hardcoded battery-specific parameter names
in `check_foreign_battery_parameters` are "derived from" the CSV, not
read from it). The file exists on disk
(`defaults/battery/default_parameters.csv`) and ships with every
install.

**Change:** `defaults/battery/default_parameters.csv` is deleted; the
comment at `:2103-2104` that cited it as the source of the hardcoded
names is corrected to cite the TOML tree instead. A test fails the build
if a non-TOML file reappears under `defaults/battery/`, so a future
addition cannot resurrect an unread parallel source silently.

**Acceptance test:** `battery_defaults_directory_holds_only_toml`: every
entry of `defaults/battery/` has a `.toml` extension.

**Verification:** `cargo nextest run -p hares-io`; `frame_golden
compare-all` (bitwise; the file is read by nothing, so no fixture moves).

**Commit:** `fix: delete the dead battery defaults CSV that no loader reads`

**Dependencies:** none.

## OD-24: the EV vehicle-mapping loader skips malformed rows and missing files silently

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
reading `load_vehicle_mapping_csv`). Structural: bitwise on every fixture
whose vehicle mapping parses cleanly today.

**Problem:** `load_vehicle_mapping_csv` treats a missing
`vehicle_mapping.csv`, an unreadable file, a malformed row and an
out-of-range field (non-positive capacity or charger power, efficiency
outside `(0.0, 1.0]`) as warnings, not errors: it logs and continues,
returning `None` for the whole mapping on a missing or unopenable file
and silently dropping the row otherwise. A caller that queries a vehicle
column HARES later has to map gets `None` with no indication whether the
column was deliberately excluded or lost to a row the loader silently
dropped.

**Evidence** (at `7b7a6a7d`): `crates/hares-io/src/defaults.rs:1653-1781`
(`fn load_vehicle_mapping_csv`): the missing-file branch at `:1655-1662`
returns `None` with a `tracing::warn!`; the open-error branch at
`:1665-1673` the same; the per-row `continue` on a CSV parse error
(`:1680-1687`), empty required field (`:1696-1701`), non-positive or
unparseable `capacity_kwh` (`:1706-1723`), `charger_power_kw`
(`:1726-1743`) and out-of-range or unparseable `efficiency`
(`:1746-1768`) each log a warning and drop the row rather than failing
the load.

**Change:** a missing file, an unopenable file, a malformed row and an
out-of-range field each become a load error (`HaresError` naming the
path, the row number and the field), so `load_vehicle_mapping_csv`
returns `Result<VehicleMapping, HaresError>`; the existing
fewer-than-50-rows check (which already errors) joins the same error
type. The doc comment's claim that a missing file is "non-fatal" is
removed with the behaviour.

**Acceptance test:**
- `vehicle_mapping_missing_file_is_an_error` and
  `vehicle_mapping_malformed_row_is_an_error` (a row with a non-numeric
  `capacity_kwh`) each name the path or the row.
- `vehicle_mapping_loads_the_shipped_csv`: the shipped
  `defaults/ev/vehicle_mapping.csv` still loads with exactly 50 entries.

**Verification:** `cargo nextest run -p hares-io`; `frame_golden
compare-all` (bitwise; the shipped mapping parses cleanly today, so no
fixture moves).

**Commit:** `fix: a missing EV vehicle mapping file or a malformed row is an error`

**Dependencies:** none.

## OD-25: an equipment with no determinable schedule power silently runs at 0 kW

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
reading the schedule-column fallback chain). Numeric: a home that falls
into this branch changes from a silent 0 kW load to a construction
error.

**Problem:** when neither a CSV schedule column nor a default profile
names an equipment's power, `inject_schedule_into_specs` falls back to
`determine_max_kw(...).unwrap_or(0.0)` or
`determine_constant_kw(...).unwrap_or(0.0)`: a load the resolver cannot
size runs at exactly 0 kW for the rest of the simulation, with only a
`tracing::warn!` that says "THIS MAY BE INCORRECT" as its only trace.

**Evidence** (at `7b7a6a7d`): `crates/hares-io/src/schedule_resolve.rs:1019`
(`let max_kw = determine_max_kw(spec, annual_mean_fraction(&profile)).unwrap_or(0.0);`),
`:1026` (the default-profile branch, same pattern) and `:1029`
(`let constant_kw = determine_constant_kw(spec).unwrap_or(0.0);`), each
feeding `inject_compact_profile_power` or `inject_compact_constant_power`
with the zero.

**Change:** each `unwrap_or(0.0)` is replaced with a resolve error naming
the equipment and the missing field `determine_max_kw` or
`determine_constant_kw` needed (annual electric or gas energy, or a rated
power); `inject_schedule_into_specs` returns `Result`, and its callers
propagate the error instead of building a dwelling with a load silently
pinned at 0 kW.

**Acceptance test:** `schedule_resolve_equipment_with_no_determinable_power_is_an_error`:
an `EquipmentSpec` with no annual energy and no rated power field, routed
through each of the three branches above, fails naming the equipment and
the missing field.

**Verification:** `cargo nextest run -p hares-io`; `frame_golden
compare-all` (bitwise; every checked-in fixture's equipment already
resolves a power through one of the two prior branches, so no fixture
moves).

**Commit:** `fix: an equipment whose schedule power cannot be determined is a resolve error`

**Dependencies:** none.

## OD-26: the Python EV builder disagrees with the catalogue and itself

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
reading `add_ev_with_driver` against the seed-path actor registration).
Numeric: changes an EV driver's away-charging telemetry and the fuel
economy a Python-constructed EV reports.

**Problem:** three inconsistencies in `add_ev_with_driver`:
1. It constructs the `EvDriverActor` with `away_charge_power_kw: 0.0`
   while the config-seeded path (`auto_register_actors`) defaults the
   same field to 6.6 kW; the two disagree on what an EV charges at while
   away whenever `away_charge_fraction` is later set above 0.0 on an
   actor built this way.
2. It computes `fuel_economy = spec.capacity_kwh / spec.range_miles`
   instead of reading `preset.fuel_economy_kwh_per_mi`, the catalogue's
   own per-archetype figure (`fueleconomy.gov`-sourced, e.g. 0.280 kWh/mi
   for the Bolt EV against the capacity/range quotient, which is a
   different number for every archetype in the catalogue).
3. The L1 charging-power cap `spec.max_l2_power_kw.min(1.8)` is written
   out twice, once in each of `py_dwelling.rs` and `py_equipment.rs`,
   instead of being a single named constant.

**Evidence** (at `7b7a6a7d`): `crates/hares-python/src/py_dwelling.rs:1322`
(`let fuel_economy = spec.capacity_kwh / spec.range_miles;`), `:1349`
(`0.0,  // away_charge_power_kw`) against
`crates/hares-core/src/actor_registry.rs:425`
(`config.get_f64("away_charge_power_kw").unwrap_or(6.6)`); the L1 cap at
`py_dwelling.rs:1260-1263` and `py_equipment.rs:777-780`; the catalogue's
own figures at `crates/hares-equipment/src/ev/catalog.rs:207-267`
(`fuel_economy_kwh_per_mi`, one line per archetype).

**Change:** `add_ev_with_driver` reads `preset.away_charge_power_kw` (a
new field on the EV archetype preset, defaulting every existing
archetype to 6.6 kW, the seed path's default, so no archetype's
behaviour changes) instead of the literal `0.0`; it reads
`preset.fuel_economy_kwh_per_mi` instead of computing the quotient; the
L1 cap becomes one constant, `hares_equipment::ev::catalog::L1_CAP_KW`
(1.8), used at both Python call sites and nowhere re-derived.

**Acceptance test:**
- `add_ev_with_driver_away_charge_power_matches_the_seed_path`: an EV
  added through `add_ev_with_driver` with `away_charge_fraction` set
  above 0.0 reports `away_charge_power_kw` telemetry of 6.6, matching an
  equivalent actor built through `auto_register_actors` with no override.
- `add_ev_with_driver_fuel_economy_matches_the_catalogue`: one case per
  archetype, `fuel_economy_kwh_per_mi` on the constructed actor equals
  `preset.fuel_economy_kwh_per_mi`, not `capacity_kwh / range_miles`.
- `l1_cap_is_one_constant`: `py_dwelling.rs` and `py_equipment.rs` both
  resolve through `L1_CAP_KW`.

**Verification:** `cargo nextest run -p hares-core -p hares-python`;
`frame_golden compare-all` (no golden constructs an EV through
`add_ev_with_driver` with `away_charge_fraction` above 0.0, so none
moves; any that does is a defect of its own, handled by this entry's
census).

**Commit:** `fix: the Python EV builder matches the catalogue's fuel economy and the seed path's away-charge power`

**Dependencies:** none.

## OD-27: `add_actor_by_name`'s Python stub claims a default its binding does not have

**Kind:** fix (found defect, `dl-i08-process.found_defects`, found while
checking the PyO3 0.29 binding against its stub). Structural: no run
behaviour changes; a caller relying on the stub's claimed default breaks
at call time today, this entry only makes the stub and the binding agree.

**Problem:** the generated stub states `params: dict[str, Any] | None =
...` (a default), but the Rust method carries no `#[pyo3(signature =
...)]`, so under PyO3 0.29 every parameter without one is
positional-or-keyword with no default: `params` is required.

**Evidence** (at `7b7a6a7d`): `crates/hares-python/src/py_dwelling.rs:907-925`
(`pub fn add_actor_by_name(&self, actor_type: String, name: String,
params: Option<&Bound<'_, PyDict>>) -> PyResult<()>`, no `#[pyo3(signature)]`
above it); `python/ochre_next/_hares.pyi:2438-2443` (`params: dict[str,
Any] | None = ...`).

**Change:** `add_actor_by_name` gains `#[pyo3(signature = (actor_type,
name, params=None))]`, matching the stub's claimed default; the stub
needs no change once the binding carries it.

**Acceptance test:** `test_add_actor_by_name_without_params_uses_the_default`:
`dwelling.add_actor_by_name("IdealThermostat", "t1")` (no third argument)
succeeds.

**Verification:** `cargo nextest run -p hares-python`; `uv run pytest`.

**Commit:** `fix: add_actor_by_name's params argument carries the default its stub claims`

## OD-29: event loads ignore their RNG sub-stream index

**Kind:** fix (found defect, red team of OR-19 at `1bb045c6`,
`verification/redteam-OR-19-1bb045c6.md` I-4). Numeric for any run with
more than one stochastic event load: their draws become independent.

**Problem:** each event load is given a sub-stream index, but the draws
ignore it and all event loads draw from the dwelling RNG's stream 0, so
two event loads see correlated (identical-position) random sequences
and adding or removing one shifts the draws of the others.

**Evidence:** to be cited file:line by the implementer at `1c18543a`
(where the sub-stream index is assigned, where it is dropped, and where
the draws read stream 0).

**Change:** each event load draws from its own sub-stream, derived
deterministically from the dwelling seed and the load's stable identity
(not its roster position), so adding, removing or reordering loads does
not shift another load's draws; checkpoint and restore carry each
stream's position.

**Acceptance test:** `event_loads_draw_from_independent_streams`: two
event loads in one dwelling produce draw sequences that differ and that
are each unchanged when the other load is removed;
`event_load_streams_survive_checkpoint`: resumed equals continuous with
two event loads.

**Measurement:** goldens with stochastic event loads move; each moved
fixture is re-captured with the per-column delta recorded in
`02-measurement.md` and the reason stated; every other fixture is
bitwise unchanged.

**Verification:** the full block and the physics suites.

**Commit:** `fix: give each event load its own random stream`

**Dependencies:** none.

## OD-30: HPXML appliance event loads drop their internal gains

**Kind:** fix (found defect, OR-18 follow-up implementation on
`fix-or18`, by code reading; numeric for every HPXML run with a clothes
washer, dryer, dishwasher or cooking range).

**Problem:** event loads take their zone only from `zone_id`; neither
HPXML parsing nor dwelling assembly sets one and event loads do not
read the zone map, so their sensible and latent gains are never
accumulated (`event_load.rs` accumulates only when `descriptor.zone` is
`Some`), although `resolve_loads.rs` gives them non-zero gain fractions
(cooking range 0.64 sensible, 0.16 latent). The conditioned zone runs
without the appliance gains OS-HPXML and OCHRE apply.

**Evidence:** to be confirmed by a run and cited file:line by the
implementer at the engine head (a dwelling with a cooking range: zone
gain from the range is zero today).

**Change:** event loads resolve their zone through the zone map exactly
as scheduled loads do (one shared resolution, not a copy), with the
same rule: a load with non-zero zone gains and no resolvable zone is a
typed error. The gain fractions and their split follow OS-HPXML
(`resolve_loads.rs` values checked against the OS-HPXML source, cited).

**Acceptance test:** `hpxml_appliance_gains_reach_the_conditioned_zone`:
a dwelling with a cooking range, clothes washer, dryer and dishwasher;
the zone's internal gain over a day equals the sum of each appliance's
energy times its sensible fraction (and latent likewise) within float
tolerance; `event_load_without_a_zone_errors_when_it_has_gains`.

**Measurement:** every golden with an HPXML appliance moves; each is
re-captured from a clean tree with the per-column delta recorded in
`02-measurement.md` and the physical reason (more internal gain: less
heating, more cooling) checked in sign and order of magnitude; the
physics suites pass; the OCHRE parity suite is re-run and its MAE
recorded before and after.

**Verification:** the full block and the physics suites.

**Commit:** `fix: give HPXML appliance loads their zone so their heat reaches it`

**Dependencies:** OR-18's fixes (the scheduled-load zone rule this
reuses).

## OD-31: zone volume falls back to an estimate or 200 m³ with only a warning

**Kind:** fix (found defect, OR-18 follow-up implementation on
`fix-or18`). Silent-default class (`decision-log.md`: no silent
defaults).

**Problem:** `environment.rs` `initial_zones` substitutes a geometry
estimate, or 200 m³, for a zone volume the input does not give, with a
warning only; the volume sets the zone air capacitance and the
infiltration and ventilation air changes.

**Change:** every zone volume comes from the input or from a documented
derivation (HPXML `ConditionedBuildingVolume`, or OS-HPXML's default
volume rule for the zone type, cited file:line); a zone with neither is
a typed error. No numeric constant fallback remains.

**Acceptance test:** `zone_without_volume_or_derivation_errors`;
`zone_volume_follows_the_os_hpxml_default`.

**Measurement:** any golden whose zone took the fallback moves and is
re-captured with the delta recorded; the rest are bitwise unchanged.

**Verification:** the full block and the physics suites.

**Commit:** `fix: derive every zone volume from the input instead of a fallback constant`

**Dependencies:** none.

## OD-32: the HELICS integration test is non-deterministic

**Kind:** fix (found defect, BR-21 follow-up's `[pytest]` run at
`4e54d56b`). Test-integrity class (`decision-log.md`: deterministic
tests).

**Problem:** `tests/python/test_helics_integration.py::test_single_dwelling_completion_signal_unblocks_aggregator`
crashed its pytest worker in the full `[pytest]` run; run alone four
times at host load about 18 it passed twice and hit the pytest timeout
twice. The `[pytest]` line cannot be reliably green while it flakes.

**Change:** find the cause (a wall-clock wait, an unsynchronised
broker start, a fixed port, a race on the completion signal, a
missing teardown) and remove it: the test synchronises on the
federation's own events, never on time, uses an isolated broker and
port per test, and tears down on failure. No retry, no raised timeout,
no skip.

**Acceptance test:** the HELICS file passes 50 consecutive runs under
`pytest -n 8` at full host load (the block's other lines running) with
no timeout.

**Verification:** `[pytest]`.

**Commit:** `fix: synchronise the HELICS completion test on federation events instead of time`

**Dependencies:** none.

## OD-16: controlled runs drive the HVAC equivalent battery degenerate and the telemetry power sum does not reconcile

**Kind:** found defect (`dl-i08-process.found_defects`), blocker for the
consumer gate (CN-17's stop rule).

**Problem:** on the gate engine `1c18543a`, four `ami2` measurement
cases fail in `actuate` runs (the controlled-load path):
`a_cooling_event_actually_sheds_cooling_load`,
`an_actuated_event_sheds_load_and_F7_is_right_for_compensating_reasons`,
`F7_predicts_the_event_it_is_asked_about_rather_than_the_average_of_the_year`,
`an_executed_GEB_shape_moves_load_off_the_peak_and_the_comfort_cost_is_measured`.
Two failure shapes, same runs:

1. `HaresSimulationError: invalid state: telemetry electrical
   consistency at step 18321: sum(equipment_power_kw) 1.762214 kW does
   not match total_power_kw 1.480145 kW (diff 0.282069 kW)` (step 482 in
   the two 7-day actuations, step 18321 in the year case; home 46363).
2. A debug-profile probe at the pre-enforcement gate engine `75a182a0`
   (the same control sequence) PANICS at the same step on
   `equivalent_battery.rs:193`: `max_energy_kwh == min_energy_kwh` with
   positive capacitance: the controlled run was already driving the
   HVAC equivalent battery into a degenerate state when the bug was
   invisible (the check was warn-only and release builds compiled it
   out). OR-11's enforcement made it loud; the defect predates it.

**Evidence:** the consumer gate's bisect pins the LOUDNESS to
`b8195c83` (OR-11 made `DwellingTelemetry::verify_consistency`
unconditional and fatal), and the DEFECT to earlier (the debug panic
exists at `75a182a0`). The 0.282 kW gap says some equipment power is
booked into `equipment_power_kw` but not into `total_power_kw` (or the
EBM's degenerate state double-books): the accounting around the
actuated dispatch and the EBM's min/max construction under a control
signal is where the truth is.

**Fix direction:** trace the actuated step's power accounting on home
46363 (which equipment is unbooked), and the EBM's min/max energy
construction under the control signal that degenerates it; fix the
accounting (or the EBM's degenerate construction, which is the physics
question) with the typed-error contract kept: an inconsistent run fails
loudly. The four cases then pass against a reconciled trace, and the
B0/baseline fixtures stay bitwise (none of them runs actuate).

**State:** FIXED at engine `fe804b59` (2026-10-02). One defect, two
symptoms: the actuate RELEASE signal (`ThermalSetpoint` naming no
setpoint) applied its `deadband_c` through four duplicated sites and
permanently zeroed the thermostat hysteresis, collapsing the EBM's
energy window (`capacitance x hysteresis`) to zero width; the failed
equipment step was then tolerated (port rollback) while the dwelling's
electrical and per-zone thermal consistency sums kept reading the
stale `core_output` of the failed equipment. The release form is a
pure pass-through (autonomous control resumes, hysteresis untouched);
a named form with a zero or non-finite deadband is a typed
`HaresError::Control` at `validate_numeric_bounds`, the FSM and the
ideal HVAC path; the duplicated writes are collapsed into one
signal-driven site; both consistency checks take `step_succeeded` and
skip rolled-back equipment (the rollback is an atomic swap, no partial
state). The four consumer cases pass; the full consumer gate is 336
passed / 0 failed / 7 precondition skips; the B0 de-trace compares
identical and every golden fixture is bitwise unchanged (none runs
actuate). Independent review (read-only) and red team both passed; the
review's NaN-deadband asymmetry was closed and the red team's
consumer-doc drift (`dr_actuation.py` documenting the old zeroing as
load-bearing) is recorded for the consumer's next documentation pass.

**Dependencies:** none.

**State (OD-15):** INVESTIGATED, no defect, pinned at `3ff4583a`
(2026-10-02). The 3.45 kWh is the crankcase heater to the last Wh: 69
of 72 steps below the 12.78 °C activation threshold each draw exactly
the rated 0.05 kW (compressor and fan zero, cooling RTF 0), the three
warm-afternoon steps draw nothing, and `hvac_cooling` equals the
crankcase energy (3.45 kWh) within 1e-9. HARES's run condition
(below-threshold OAT, power times the compressor-off fraction) and the
cooling end-use booking match BOTH references: EnergyPlus
(`DXCoils.cc`: `MaxOATCrankcaseHeater` gate, `capacity x (1 - RTF)`,
`EndUseCat::Cooling`) and OCHRE (`HVAC.py:1059-1077`: crankcase into
`electric_kw` when off and below 55 °F). The pin test
`winter_cooling_energy_is_the_crankcase_heater` binds the composition;
no golden covers this fixture, all goldens bitwise unchanged.

## OD-33: the vendored OCHRE's event sentinel crashes under pandas 3

**Kind:** found defect (`dl-i08-process.found_defects`); blocks OD-13
and the thermal-trace comparisons (OD-11).

**Problem:** the vendored OCHRE's `EventBasedLoad.end_event`
(`vendors/OCHRE/ochre/Equipment/EventBasedLoad.py:201`) writes
`pd.Timestamp.max` (nanosecond precision) into a
`datetime64[us]` column; pandas 3.0.6 rejects the lossless-upgrade
check (`Cannot losslessly convert units`), so every OCHRE run over a
document with `EventBasedLoad` equipment dies at the sentinel write:
BEopt_example.xml carries four such appliances (three instances each
of ClothesWasher, ClothesDryer, CookingRange, Dishwasher), so the
thermal-trace parity tests error before any comparison and the
divergences OD-11's strict marks must cite cannot be measured. The
 Ochre-not-installed module skips in the pytest census hide this
surface; BR-6c's pandas 3 lift (the cap removal) is what reaches it.

**Evidence:** OD-11's runs (2026-10-02): the fixture errors at
40 of 48 simulated hours, deterministic, on pandas 3.0.6; the vendor's
own declared requirement is `pandas >= 1.4, < 3.0` — the ochre
dependency-group's pandas cap removal (BR-6c) exceeded the vendor's
support window, and the vendor crash is the proof.

**Fix direction:** the sentinel is the vendor's "events ended" marker:
either (a) patch the vendored module to write a us-precision sentinel
(`pd.Timestamp.max` floored to `us` — `Timestamp.max.as_unit("us")`)
if the vendor is patched in this tree (check whether vendors/OCHRE is
a submodule: a submodule fix is its own commit + pointer bump; a plain
vendor directory is committable here), or (b) cap pandas below 3.0 for
the ochre group (reverting BR-6c's cap removal for pandas only, with
the naming comment) until the vendor is fixed upstream. (a) is the
right fix if the tree owns the vendor; the OCHRE comparison tests
(OD-13) then run and the divergences are measured.

**State:** open; found by OD-11's first real run (2026-10-02).

## OD-34: a tariff attached or replaced mid-run bills from the wrong period, and the evaluator is not checkpointed

**Kind:** fix (found defect, adversarial check of OR-19 `ba44d9f1`,
`verification/review-OR-19-ba44d9f1.md` M-1 and M-3; pre-existing).
Numeric for runs that attach or replace a tariff after step 0 or resume
with one.

**Problem:** `set_tariff` after step 0 advances the price index (OR-19)
but the billing periods stay anchored at the simulation start: a tariff
attached at day 75 produces bills for January and February, each with a
full fixed and demand charge and one step of energy; the first partial
period's fixed charge is not prorated; re-attaching or replacing a
tariff discards the outgoing period's accrued charges. The tariff
evaluator (price index, open period, accruals) is not in
`DwellingCheckpoint`, so a resumed dwelling with a tariff restarts it at
step 0 (net power diverges at the third resumed step and billing state
is lost); the golden manifests now carry tariffs.

**Change:** billing periods start at the attach step (the period
containing it, prorated per the tariff's own rule or per the billing
cycle definition cited); replacing a tariff closes the outgoing period
with its accruals and opens the new one at the switch step; the
evaluator's full state is in the checkpoint (`CHECKPOINT_VERSION`
bumped, shape re-pinned).

**Acceptance test:** `a_tariff_attached_mid_run_bills_from_the_attach_period`
(no bill before the attach; first period prorated; later periods
bitwise equal to a from-start run's); `replacing_a_tariff_closes_the_outgoing_period`;
`a_resumed_dwelling_with_a_tariff_equals_the_continuous_run` (step
results and billing summaries).

**Verification:** the full block; goldens (the tariff fixture) identical
or re-captured with the delta stated.

**Commit:** `fix: bill a mid-run tariff from its attach period and carry the evaluator through the checkpoint`

**Dependencies:** OR-19 folded (it introduces the evaluator advance).

## OD-35: an EV's battery capacity rises above its rating early in a run

**Kind:** fix (found defect, same check, info). Physics.

**Problem:** the EV actor seed observed a live capacity of 60.54 kWh
against a rated 60.0 kWh after 30 steps: a negative capacity fade.
Capacity can only fall from its rated value under the degradation
model.

**Change:** find where the fade term can go negative (calendar or cycle
model, its initialisation, or a sign) and fix it against the model's
source (cite); capacity is bounded by its rated value by construction,
and the invariant is enforced in every build.

**Acceptance test:** `ev_capacity_never_exceeds_its_rating` over a year
of driving and idle at several temperatures; the degradation model's
reference curve reproduced.

**Verification:** the full block and the physics suites; goldens with
EVs identical or re-captured with the delta stated.

**Commit:** `fix: keep the EV battery's capacity at or below its rating`

**Dependencies:** none.

## OD-36: the remaining silent geometry, location, schedule and humidity defaults

**Kind:** fix (found defects, OR-18 review follow-up and OD-31
implementation on `fix-or18c`, 2026-10-04). Silent-default class.

**Problem** (each a substitution the input does not state):
- ceiling height falls back to 2.5 m in `solver_builder` (the synthetic
  and BESTEST paths rely on it);
- duct-efficiency latitude and longitude fall back to 40 / -100;
- Python water heater tank volume and capacity `unwrap_or` calls (dead
  behind the constructor checks, still present);
- the generated default schedule emits `occupants` twice and constant
  1.0 columns when no profile exists;
- initial zone humidity is fixed at 0.008 kg/kg;
- `defaults/Default Schedule Parameters.csv` carries HVAC setpoint rows
  nothing reads (the cooling row says 75 °F, contradicting the OS-HPXML
  78 °F default HARES now applies);
- the HPXML foundation zone has no floor area, so it gets no foundation
  furniture and the conditioned/basement split uses a floor-count ratio;
- the fleet's path-based IECC reader (`parse_iecc_climate_zone`) does
  not use the weather-station default OR-18's follow-up added;
- `attach_pv_to_roofs` defaults a roof boundary's azimuth to 180° and
  tilt to 0°, and `register_pv_roof_shading` a missing roof area to
  1.0 m² (found by OD-16's PV orientation fix, `2689f2af`).

**Change:** each value comes from the input, from the OS-HPXML or
EnergyPlus default it represents (cited and recorded as a run warning),
or is a typed error; dead code and unread rows are removed (the defaults
digest change re-captured with the goldens); one IECC resolution path
serves the dwelling and the fleet.

**Acceptance test:** one test per item that fails at the head.

**Verification:** the full block and the physics suites; moved goldens
re-captured with deltas.

**Commit:** `fix: replace the remaining silent geometry, location, schedule and humidity defaults`

**Dependencies:** OR-18's follow-up and OD-31 (`fix-or18c`) folded.

## OD-37: a gas boiler's equivalent battery stays at zero while the room AC cools at night

**Kind:** investigation, then fix if a defect (found by the same
implementation, ResStock bldg0000002, CT, January).

**Problem:** in a January run of bldg0000002 the gas boiler's
equivalent-battery values stay at 0 for the whole run while the room
air conditioner reaches cooling mode at night; both look wrong for a
heating-dominated January home.

**Change:** establish with a run what drives each (setpoints after
OR-18's schedule change, zone assignment, the AC's mode logic, the
boiler's EBM inputs); fix any defect found with a test; if both are
correct, record why with a pinning test.

**Acceptance test:** a test pinning the corrected (or explained)
behaviour for that home and month.

**Commit:** `fix: …` (named by the finding) or `test: pin …` if no defect.

**Dependencies:** OR-18's follow-up folded.

## OD-38: HVAC equipment runs whole steps on or off with no part-load runtime fraction

**Kind:** fix (architecture flaw, found by OD-30's implementation,
2026-10-04). Physics; numeric.

**Problem:** a cycling HVAC unit is either on for the whole step at
full capacity or off for the whole step. At 900 s a room AC alternates
about 5 kW and 0, swinging the zone 21 to 28 °C around a setpoint it
should hold within its deadband; the DST golden's heat pump swings 19
to 30 °C. `test_tariff_integration.py::test_ac_runs_with_low_setpoint`
fails by 0.07 °C once appliance gains reach the zone (OD-30). EnergyPlus
models a cycling unit by its part-load ratio and runtime fraction within
the step (with the part-load curve and cycling losses); OCHRE uses a
duty cycle at sub-hourly resolution.

**Change:** cycling HVAC (air conditioners, heat pumps heating and
cooling, furnaces, boilers, baseboards) delivers within the step the
fraction of capacity the zone needs to reach and hold the setpoint
band, as a runtime fraction in [0, 1] with the part-load degradation
(PLF curve, cycling losses) applied per EnergyPlus (cite file:line) or
OS-HPXML's equivalent; the thermostat FSM and the equivalent battery
stay consistent with it; telemetry reports the runtime fraction. Fixed
at the design level (one runtime-fraction path for every cycling unit),
not per equipment.

**Acceptance test:** `test_ac_runs_with_low_setpoint` with its original
strict assertion; a zone held within its deadband plus a stated
tolerance at 900 s and 3600 s for each cycling class; energy over a day
matches the continuous-capacity integral within the part-load
degradation; BESTEST and parity suites before and after recorded.

**Measurement:** every golden with cycling HVAC moves; deltas recorded
with the physical reason; OCHRE parity MAE before and after; per-step
cost (profiling phases) before and after.

**Verification:** the full block and the physics suites.

**Commit:** `fix: run cycling HVAC for the fraction of the step the zone needs`

**Dependencies:** OD-16's fixes folded (the thermostat and EBM
contract).

## OD-39: a conditioned basement is modelled as an unheated zone

**Kind:** fix (found by the review of `fix-or18c`,
`verification/review-OR-18c-b68a8aca.md`; pre-existing). Physics;
numeric.

**Problem:** OS-HPXML's `base.xml` basement ("basement - conditioned")
sits at 7.6 °C on 15 January: the zone exists but no HVAC serves it and
it is not merged into or coupled as conditioned space. OS-HPXML treats
a conditioned basement as part of conditioned space (one thermal zone
served by the HVAC, `geometry.rb` / `hpxml.rb` location handling, cite).

**Change:** a conditioned basement is conditioned space per OS-HPXML:
either merged into the conditioned zone or a conditioned zone served by
the HVAC with the OS-HPXML airflow split (cite and justify the choice);
its loads, surfaces and ducts accounted accordingly.

**Acceptance test:** `conditioned_basement_is_held_at_the_setpoint`
(base.xml, January: the basement within the deadband);
`conditioned_basement_loads_reach_the_hvac`.

**Measurement:** every input with a conditioned basement moves;
goldens re-captured with deltas; BESTEST and parity before and after.

**Verification:** the full block and the physics suites.

**Commit:** `fix: treat a conditioned basement as conditioned space`

**Dependencies:** `fix-or18c` folded (zone geometry).

## OD-40: an autosized heating-only heat pump violates the core-output contract

**Kind:** fix (found by the same review; pre-existing at
`5d66dafc` and `b68a8aca`).

**Problem:** the autosize heating-only OS-HPXML samples raise a heat
pump `core_output` contract violation during the sweep; the ASHP and
MSHP cooler standby step also errors on those samples (the same sweep,
`fix-or18c` follow-up); both are investigated together.

**Change:** reproduce, find the cause (autosizing a heating-only unit's
cooling capacity, a zero-capacity path, or the core output's sign
contract) and fix it at its root; test each affected sample.

**Acceptance test:** every autosize heating-only sample builds and
steps a day without a contract violation; a unit test for the cause.

**Verification:** the full block; the sweep.

**Commit:** named by the cause.

**State:** FIXED (2026-10-04, commit `a7a9253d`). Cause A: the cooler's
flow-activity rule counted only the electric flow while the reference
scales power by the space fraction and leaves the envelope gains
unscaled, so a heating-only unit's active cooling delivery (thermal
negative, electric zero) was demoted to standby and rejected by the
thermal-sign rule; the rule now counts the delivered thermal flow
(the exact expression the core output publishes), matching the
heater's and the guard's. Cause B: the heater never declared the FUEL
capability while publishing the combustion backup's fuel flow; it now
declares FUEL exactly when the backup fuel is combustion and publishes
the flow on every step of such units (Some(0.0) idle and on restore;
the restore's zero is consumed identically to the old None by the
metrics' map_or). The 425-sample sweep: thirteen contract violations
to zero, step errors zero, 112 build errors unchanged (pre-existing,
three distinct causes now filed as their own entries in this file).
Red-green proven at the HEAD worktree for both causes; the
red team's independent review found one follow-up defect in the fix's
own surface (OD-45: the declaration and the flow read different config
generations under overrides) and one doc inaccuracy (the ground-to-air
sample's failure is heating-capacity staging, not the EER-only path;
the test's doc corrected). Goldens bitwise identical, both profiles;
default nextest 7243; pytest 1400 passed, 4 xfailed, 0 skipped.

**Commit:** `fix: an autosized heating-only heat pump keeps the core-output contract`

**Dependencies:** none.

## OD-41: water heating in the Python OCHRE parity week is about seven times OCHRE's

**State:** CLOSED, no HARES change (2026-10-07, branch
`row/phys-od41-od56`, folded). Triage: class (a), OCHRE is wrong.
OCHRE's tank never receives the fixtures' hot-water draw: its schedule
builder names the column "Water Fixtures (L/min)" (schedule.py:47,
convert_water_column :350-368) while the tank reads "Water Heating
(L/min)" (Water.py:179, :287); the release tag v0.9.2 carries the
mismatch, so update_water_draw sees the default 0 every step and only
the dishwasher and clothes washer columns flow. Measured on the parity
window: OCHRE's tank receives 15.6 L over 24 h (the dishwasher alone)
and heats 1.01 kWh against HARES's 12.28 (12.2x), the week 11.37
against 78.56 (6.9x). HARES keeps the reference draw: the
RESNET-normalized 256.1 L/day (3.6 adjusted bedrooms, standard
distribution) through the TMV tempered path, and the EnergyPlus-grade
balance closes: element 10.27 kWh/day = draw enthalpy 9.14 + standby
1.16 + storage 0.00 on a flat draw (0.3 %). Registered as D-017 in
docs/alignment/DIVERGENCES.md with the closed-form pinning test (a lost
draw collapses to the standby floor and fails it) and the 24 h parity
xfail. No HARES physics changed; the water-heating goldens moved
±0.05 %.

**Kind:** investigation, then fix (found by OD-30's follow-up on
`fix-od30`, 2026-10-04). Physics. Confirmed again by the head-audit fix
branch (2026-10-05): a 24 h window from 2019-05-05 00:00 gives HARES
14.76 kWh against OCHRE 1.01 kWh (14.6x).

**Problem:** in `test_ochre_parity.py`'s 7-day week HARES water heating
is 78.56 kWh against OCHRE's 11.37 kWh, the largest part of the 7-day
total-electric gap (HARES 216.10 against OCHRE about 149 kWh) now that
the appliances match OCHRE within 10%.

**Change:** triage per `decision-log.md` `ochre_deviation_triage`
before changing anything. Establish with a run where the energy goes (draw volumes
and schedule alignment, mains temperature, setpoint, tank losses, heat
pump vs resistance mode, element sizing) against OCHRE and against
OS-HPXML/EnergyPlus `WaterHeater:Mixed`/`Stratified` for the same unit;
fix the defect at its root (not toward OCHRE where OCHRE is wrong;
EnergyPlus-grade physics is the target), each change cited.

**Acceptance test:** the parity week's water heating within a stated
tolerance of the EnergyPlus-grade reference for the unit, with the
remaining difference to OCHRE explained; a unit test for each root
cause.

**Measurement:** goldens with water heaters re-captured with deltas;
parity before and after.

**Verification:** the full block and the physics suites.

**Commit:** named by the cause.

**Dependencies:** OD-30 folded.

## OD-42: equipment parameter overrides on a spec never reach the typed configs

**Kind:** fix (architecture flaw, found by OD-16's PV fix, `2689f2af`;
pre-existing). Silent-ignore class: a user-supplied value is dropped.

**Problem:** overrides set on an equipment spec's `parameters` (the
blueprint and Python paths) do not reach the typed configs the
equipment initialises from, so a caller who overrides a parameter gets
the resolved default with no error and no warning.

**Change:** one path from spec to typed config: every override is
applied to the typed config before init, an override naming an unknown
field is a typed error naming the equipment and field, and a value that
fails the typed config's validation errors at the call. No parallel
untyped parameter bag survives where a typed config exists.

**Acceptance test:** for each equipment class, an override of a typed
field changes its behaviour; an unknown or invalid override errors;
Python likewise.

**State:** FIXED (2026-10-04, commit `ee7bee85`, with OD-45 in the
same commit: the two entries share the config surface). The mapping
found TWO config generations per equipment (the constructor read the
un-merged typed payload, init the merged one: OD-45's root) and the
spec's parameter bag never reached any typed payload. The landing is
per-field with schema-gated normalization: the Python builders'
raw-text vocabulary ("natural gas", "electric") lands in the canonical
serde form when the schema accepts it, an unparseable or unknown
override is a typed error naming the equipment, the field and the
value, and the heat pump configs reject unknown keys (previously a
pinned silent no-op). The dwelling-level override channel is fully
strict; the blueprint bag channel still passes payload-unknown keys
silently (the machinery keys, autosize flags and duct inputs, are
indistinguishable from a caller's typo without an EquipmentSpec shape
change: filed as OD-46). Red-green proven for the silent-ignore
(blueprint PV at 5 kW instead of the overridden 9), the heat-pump
unknown-field no-op, and OD-45's both-direction override (Rust and
Python). The first review round caught the landing corrupting three
working Python constructions (raw-text enum values landed onto
canonical payloads: GasWaterHeater, TanklessWaterHeater, ASHPHeater
all failed to build); the fix round's per-field validation with the
normalize-and-retry resolved it and the verification round confirmed
no silent-ignore resurrection. The goldens are bitwise identical in
both profiles (no fixture carried a dropped override); assembly
performance within noise (a 3-fleet-dwelling probe, -0.5%);
nextest 7051 default and 7109 observe; pytest 1412 passed, 4 xfailed,
0 skipped; pyright 0 errors.

**Commit:** `fix: apply equipment parameter overrides to the typed configs they name`

**Dependencies:** none.

## OD-46: the blueprint bag channel still drops payload-unknown keys

(Extended 2026-10-05 by the audit of `1b708ccb`: `land_spec_parameters`
in `conversions.rs` also skips an override when the payload lacks the
key. HVAC, water-heater and heat-pump configs omit `None` optionals
when serialized, so a legitimate override of an unset optional field is
dropped silently, not only a typo. The tests cover only PV, the
water-heater setpoint, battery and dehumidifier overrides. The fix must
land every schema field whether or not the payload carries it, with a
test per config class.)

**Kind:** found defect (the OD-42 review's disclosed residual; the
no-silent-ignore law's follow-up).

**Problem:** `apply_spec_parameters_onto_typed_payload` lands only the
bag keys the typed payload carries; a blueprint caller's typo'd config
key that matches no payload key is dropped silently, asymmetric with
the dwelling-level override channel's strict unknown-key error. The
machinery keys (autosize flags, duct inputs, raw-name source keys) are
legitimate bag content the payload does not carry, so full strictness
needs a shape change: a dedicated typed-override field on
`EquipmentSpec`, or the builders stopping the machinery keys in bags.
Python callers cannot inject bag keys (the PyO3 builders own the
bag), so the exposure is Rust-side.

**Change:** give `EquipmentSpec` a dedicated override field (or move
the machinery keys out of the bag); the bag becomes either raw or
typed, never both; unknown keys then error on the whole channel.

**Acceptance test:** a typo'd config key on a blueprint spec errors
naming the equipment and field.

**Verification:** the full block; goldens identical.

**State:** FIXED (2026-10-07, the silent-default sweep's commit
`3099d2ce`, folded in `31732dc2`): the dedicated `typed_overrides`
field on `EquipmentSpec` (the entry's first shape option) — the bag
becomes machinery/raw only, overrides land from the dedicated field,
every schema field is landable whether or not the payload serializes
it (the per-class test lands an unset optional on all 21 registered
config classes: the audit's M-1 finding closed with it), and a key no
schema field reads errors naming the equipment and the field (the
acceptance). Red proven by reverting to the carried-keys rule.

**Dependencies:** none; lands with the next equipment-spec surface
work.

## OD-43: a schedule's microwave column double-counts energy already in the residual plug loads

**Kind:** fix (found by OD-30's follow-up, 2026-10-04). Physics.

**Problem:** OS-HPXML v1.12.0 has no microwave appliance: microwave
energy is inside the residual "other" plug loads
(`defaults.rb` `get_residual_mels_values`, from RECS). HARES creates a
Microwave event load from a `microwave` schedule column or an HPXML
`<Microwave>` element (100 kWh/yr default, ANSI/RESNET 301-2014, the
electric range's 0.72/0.08 split, no radiant part), so an
OS-HPXML-derived building whose schedule carries a `microwave` column
counts that energy twice.

**Change:** one rule: for an OS-HPXML-derived building, microwave
energy stays in the residual plug loads and the schedule column shapes
nothing additional (or the residual is reduced by exactly the
microwave's energy, cited); for a BEopt/OCHRE-format input, the
microwave is modelled once with OS-HPXML's plug-load radiant split.
State the choice and its source.

**Acceptance test:** total plug-load plus microwave energy for an
OS-HPXML building with a `microwave` column equals the residual MELs
alone; a BEopt-format input keeps its microwave once.

**Verification:** the full block; goldens with a microwave column
re-captured with deltas.

**Commit:** `fix: count microwave energy once`

**Dependencies:** OD-30 folded.

**Numbering note (2026-10-07, the register audit):** the branch's
register entry is renumbered D-015 → D-019: the folded
conditioned-basement merge already holds D-015 on the head. The entry
of record is D-019 in docs/alignment/DIVERGENCES.md at the audit
branch `row/register-audit`; the branch's own D-015 section is dropped
at the fold, and the commit message's "Recorded as D-015" reads D-019.

## OD-44: short-wave radiation that leaves back out through the windows is not modelled

**Kind:** fix (found by OD-30's follow-up). Physics.

**Problem:** EnergyPlus distributes an enclosure's diffuse short-wave
(transmitted diffuse solar and lights' visible part) over the inside
surfaces including the windows, whose share leaves back out through
the glazing (`HeatBalanceSurfaceManager.cc` 4300-4315, `solVMULT`
denominator with window transmittance). HARES distributes it over the
opaque surfaces only, so all of it is absorbed indoors.

**Change:** the diffuse short-wave distribution includes each window's
inside-face share and its back-transmittance (lost) and absorptance
(to the window's inner pane), per EnergyPlus (cite); one path for
transmitted solar and lighting.

**Acceptance test:** an enclosure with a known window fraction loses
the EnergyPlus share of diffuse short-wave through the glazing; the
energy balance closes.

**Measurement:** goldens with windows move (every one); deltas
recorded; BESTEST (the solar cases) and parity before and after.

**Verification:** the full block and the physics suites.

**Commit:** `fix: let diffuse short-wave leave back out through the windows`

**Dependencies:** OD-30 folded.

## OD-47: the Lighting end-use aggregate excludes Exterior Lighting

(Filed first under a second OD-34 heading; renumbered 2026-10-05 so each
ID names one entry. OD-34 is the mid-run tariff entry.)

**Kind:** found defect (the OD-13 parity comparison; schema definition,
not step physics). Routed per BR-12's rule: the fix moves the output
frame's aggregate column, so it follows the numeric block (a
09-numeric-fixes.md entry with the goldens re-captured and the delta
stated), not this row.

**Problem:** `crates/hares-io/src/output/columns.rs`'s LIGHTING arm
tags `Lighting`, `Indoor Lighting`, `Outdoor Lighting`, and
`Garage Lighting`, but not `Exterior Lighting`, which falls through to
the Other aggregate. The `Lighting End Use Electric Power (kW)` column
therefore holds indoor lighting only, while the reference (OCHRE's
`Lighting` output, and the plain meaning of the column's name) sums
indoor and exterior. Measured over the OD-13 parity hour: OCHRE
0.09256122877254946 kWh against HARES 0.08545972626253563 kWh, a
relative error of 7.7 percent; Exterior Lighting's own verbatim column
accounts for the entire gap (0.007101502510013806 kWh).

**Disposition:** undecided until the numeric block reaches it. The
parity case runs as a strict xfail citing the measured pair
(test_ochre_parity.py's PARITY_XFAILS); the mark comes off when the
aggregate includes exterior lighting. Any fix must state whether the
`Other` aggregate's fifteen-member definition (OD-13's Other mapping)
loses Exterior Lighting and how the consumer's channel mapping follows.

**Dependencies:** the numeric block's ordering.

## OD-45: the heat-pump heater's FUEL declaration and its fuel flow read different configs

**Kind:** found defect (the OD-40 red team; the fix's own new surface).

**Problem:** the heater's `core_capabilities` FUEL declaration is gated
on the UN-merged typed config in the constructor
(`heat_pump/heater.rs:519`, through `equipment_config_from_spec`), while
`backup_fuel_type` is set in `init_from_typed` from the OVERRIDE-MERGED
config (`heater.rs:981`, through `merged_equipment_config`). An override
that flips the backup fuel's class (gas to electric) produces a declared
FUEL capability with a structurally None flow (a new violation class
this fix introduces); electric to gas produces an undeclared populated
flow (pre-existing at HEAD). Both fail loudly per step
(`dwelling/mod.rs:6009`), not silently, and no in-repo config exercises
the split.

**Change:** the declaration and the flow must read the SAME config
generation (both post-merge, or the declaration recomputed at init and
the capability set there). The fix is at the root: one config
generation owns the backup fuel's class.

**Acceptance test:** an override flipping the backup fuel's class in
both directions steps a day with the contract satisfied (the
declaration follows the flow's config generation).

**Verification:** the full block; the goldens identical (no captured
path carries an override of this field).

**Dependencies:** none; the next equipment-config surface row may fix
it alongside itself.

## OD-48: the HPXML resolvers skip or default unsupported and incomplete inputs silently

**Kind:** found defect (the OD-30 review round, 2026-10-05; the same class
as the dwelling assembly's silent "init failed, skipping" path, which
OD-30 removes at the root, but in `hares-io`'s resolvers).

**Problem:** the resolvers drop or replace input with only a log line:
`resolve_hvac.rs` skips an AC with no SEER (~1152), a HeatingSystem with
an empty type (~2008), and a CoolingSystem with an empty or unsupported
type (~2155-2161); it falls back to `DuctConfig::default()` for an
unrecognised duct zone (~410); its typed-config reads use
`.ok().flatten()` fallbacks (~320-370), and so does
`rebuild_hvac_typed_config` (~1303-1338), which autosize uses: a typed
config that fails to build silently leaves the raw spec in place (found
again on `fix-od30`, 2026-10-06). `resolve_pool.rs` skips a spa
whose idref does not resolve (~311). `building.rs` skips a duct leakage
entry with an unrecognised unit (~2565). `resolve_loads` skips an
occupancy entry, and maps an unrecognised lighting location to "Indoor
Lighting" with only a warning (`resolve_loads.rs:701-707`, found by the
OD-30 round, 2026-10-05). Each one turns an input defect into a home that is
missing equipment or has wrong parameters, with nothing in the output to
say so.

**Change:** each site becomes a typed error that names the element, its
id and the field. If OS-HPXML v1.12.0 has a defaulting rule for the
input (cited by file:line), apply that rule with a warning naming the
default. An unsupported system type is an error that names the type.

**Acceptance test:** for each site, an input that hits it fails with the
typed error, or resolves to the cited OS-HPXML default; a test proves
the pre-fix code skipped it silently.

**Verification:** the full block; ResStock fleet resolution over the
committed buildings reports every home that now errors (expected none;
any that do are input defects to record).

**Dependencies:** none.

## OD-50: the seeded thermal trace diverges from OCHRE from the first step

**State:** FIXED (the solar record), the temperature residual
re-triaged and re-measured (2026-10-07, branch `row/phys-od41-od56`,
folded). The window-solar record's 1.3048x was a HARES output-column
bug: the "Window Transmitted Solar Gain (W)" column carried the zone's
total window injection, the through-glass flux plus the
inward-flowing share of the glass-absorbed flux, a fixed 1.3048
multiplier on OCHRE's through-glass definition (Envelope.py:1160).
The fix splits the reported channel (through-glass) from the injected
total (the net-sensible sum keeps both shares); measured after: ratio
0.9991, worst sampled step 0.96 %, the solar xfail off. The step-1
infiltration lead does not reproduce seeded at the tip (0.7 % at
step 1, nothing past 5 % early). The temperature trace's remaining
divergence is the envelope thermal response, D-012's class: worst
6 h 0.7066 at t=248 min, mean 0.3697, overnight 1.3031/0.3672,
heating 41.553 against 47.052, near-flat early and growing through
day 2 (day-1 heating +4.8 %, 48 h −11.7 %). The records are
re-measured and tightened; the two temperature cases stay strict
xfails with the cause updated.

**Kind:** investigation, then fix (found on `fix-headaudit`,
2026-10-05, once both OCHRE modules ran seeded and repeat runs agreed
to the last bit). Physics.

**Problem:** the thermal-trace comparison against OCHRE, seeded:
- first 6 h worst zone error 2.41 °C against a 0.1 °C threshold;
- 48 h mean 0.82 °C against 0.5 °C;
- overnight worst 4.72 °C, mean 0.88 °C;
- window solar 1.305 times OCHRE's, all 36 sampled steps outside 5%;
- 48 h heating 39.3 kWh against OCHRE's 47.1 kWh.

An earlier unseeded run (`02-measurement.md`) printed infiltration heat
gain as the first component past 5%, at step 1; no seeded measurement or
test checks that, so it is a lead, not a cause. The three thermal-trace
cases are strict xfails whose reasons quote these records and say the
divergence is not yet triaged to a cause; a measurement more than 2% off
its record fails (`tests/python/ochre_records.py`, edge inclusive).

**Change:** triage per `decision-log.md` `ochre_deviation_triage`: each
divergence is classed as OCHRE wrong, HARES wrong, HARES architecture,
or the comparison wrong, with evidence, before anything changes. Trace
each diverging input to its cause against OS-HPXML v1.12.0 and
EnergyPlus v24.2.0, cited by file:line. Start with step-1
infiltration (flow, the air temperature it carries, and its zone),
then window solar (transmittance, angle modifier, and the frame or
shading factors). Where OCHRE is the one in error, HARES keeps the
reference physics and the record documents the deviation. Where HARES
is in error, fix it. OD-44 (diffuse short-wave out through the
windows) may own part of the solar gap; establish how much.

**Acceptance test:** each fixed input matches the reference within
its stated tolerance from step 1. The xfail records are re-measured
and tightened, and an xfail comes off when its case passes.

**Verification:** the full block; every moved golden re-captured
clean with deltas stated; BESTEST and the oracles before and after.

**Dependencies:** OD-44 shares the window-solar path.

## OD-51: a 24-hour OCHRE parity window shows end-use gaps the one-hour window hides

**State:** CLOSED (two class-b fixes, one class-a registration;
2026-10-07, branch `row/phys-od41-od56`, folded). The parity suite
gained the 24 h window (seeded both sides, 2019-05-05 00:00,
one-minute steps). Re-measured at the tip, the spec's cooling 2x and
Other +18 % are gone: cooling is exact (0.950 = 0.950) and Other
closed exactly (7.4094 = 7.4094, worst member 1.9e-14) by the bedroom
fix: HARES's appliance energies read the raw NumberofBedrooms where
OCHRE hpxml.py:791-800 (applied at :1664) applies the
occupant-adjusted count; the dishwasher's cycles-per-year ratio 0.9023
matched the raw-3 hypothesis exactly. The fix lands the shared
adjusted_bedroom_count for the appliance loads and the water heater's
draw parse. Heating's residual is +4.83 % (32.495 against 30.997),
the D-012 envelope class, a strict xfail with the pair quoted; Water
Heating 12.2x is D-017, strict xfail; Total is their sum, strict
xfail. A records test binds the quoted pairs. B-16/B-17 own about
0.35 kWh of the 24 h water-heating gap (the wet appliances'
per-cycle volume; already recorded there, B-17 stays scheduled).

**Kind:** found defect and test gap (found on `fix-headaudit`,
2026-10-05). Physics.

**Problem:** the parity suite compares one hour, so it cannot see
gaps that build up over a day. A 24 h window from 2019-05-05 00:00:

| End use | HARES (kWh) | OCHRE (kWh) | Difference |
|---|---|---|---|
| Water Heating | 14.76 | 1.01 | 14.6x (OD-41) |
| HVAC Cooling | 1.94 | 0.95 | 2x |
| HVAC Heating | 24.08 | 31.00 | -22% |
| Other | 8.77 | 7.41 | +18% |
| Total | 52.2 | 43.1 | +21% |

**Change:** triage every gap per `decision-log.md`
`ochre_deviation_triage` (OCHRE wrong, HARES wrong, HARES architecture,
or the comparison wrong), with evidence, before changing anything. The
parity suite gains a 24 h window, seeded on both sides.
Then trace the cooling, heating and Other gaps to their causes against
OS-HPXML and EnergyPlus, and fix HARES wherever HARES is wrong. Water
heating is OD-41's. Check whether B-16 and B-17 own part of these gaps.
Any end use that still differs after the fixes, because OCHRE deviates
from the reference, is a strict xfail with its measured pair and cause.

**Acceptance test:** the 24 h parity cases pass within the suite's
tolerances, or are strict xfails with a cited cause.

**Verification:** the full block; parity before and after; goldens.

**Dependencies:** OD-41; OD-50 shares the heating path.

## OD-52: the vector env keeps a pure-Python batch-step fallback the extension never needs

**Kind:** slop (found on `fix-headaudit`, 2026-10-05).

**Problem:** `python/ochre_next/rl/vec_env.py` keeps a pure-Python path
for `batch_step`, which the compiled extension always registers. The
fallback is dead code, it can drift from the Rust path, and it hides an
import or registration failure.

**Change:** delete the fallback; a missing `batch_step` is an import
error.

**Acceptance test:** the vec-env tests pass on the Rust path only; a
test proves the env refuses to build without it.

**Verification:** the block's pytest and pyright lines.

**Dependencies:** OD-16's fold touches `vec_env.py` deadband bounds;
this follows it.

## OD-53: exterior surface roughness falls back to a silent default

**Kind:** found defect (the `fix-or18c` attic triage, 2026-10-05).
Physics; fixed on `fix-or18c` with the attic work.

**Problem:** a roof with no `<Siding>` gets MediumRough (r_f 1.52)
through a warn-and-default fallback, and vinyl walls get Smooth
(`crates/hares-physics/src/film_coefficients.rs:103-128`). OS-HPXML
v1.12.0 passes no roughness, so every material takes EnergyPlus's
'Rough' default (`model.rb:49`, `:95`). HARES therefore departs from the
reference through a silent default.

**Change:** roughness comes from the input's material, through the
behaviour EnergyPlus's own dataset records define: the convective
film's roughness class is the material record's field (EnergyPlus
v24.2.0 `ASHRAE_2005_HOF_Materials.idf`: F07 Smooth at :124, Asphalt
shingles VeryRough at :1897, Siding wood bevel Rough at :2047,
Hollow-backed Smooth at :2062: disclosed in D-010 of
docs/alignment/DIVERGENCES.md). An input with no material takes
OS-HPXML's default material with a warning naming the source.

**Acceptance test:** a test that fails before the fix for a roof with
no siding element; the attic and indoor MAE measured before and after.

**Verification:** the full block; goldens with deltas.

**State:** FIXED on `fix-or18c` (the branch awaits its fold): the
policy implemented as the change states (absent material → OS-HPXML's
default material with a warning; a name-no-material input treated as
absent with a warning; an unrecognized material an error); all 18
records round-2 verified; D-010 carries the dataset citations.

**Dependencies:** none.

## OD-54: an air-source heat pump's compressor COP drops below 1 at -17 °C

**Kind:** investigation, then fix (found on `fix-or18c` while tracing
the furnace-to-ASHP swap test, 2026-10-05). Physics.

**Problem:** a 12 kW, HSPF 9.5 ASHP on default curves draws about
1.25 kW and delivers about 1.15 kW at -17 °C outdoor (COP about 0.9),
with the compressor running just above its -17.8 °C lockout. A
compressor's heating COP below 1 at that temperature is not physical for
this class of unit.

**Change:** trace the capacity and EIR curves, their bounds and
extrapolation, and the defrost treatment against OS-HPXML v1.12.0
(`hvac.rb` defaults and curve limits) and EnergyPlus
`Coil:Heating:DX:SingleSpeed` / `VariableSpeed` (Engineering Reference,
defrost and curve clamping), cited by file:line. Triage per
`decision-log.md` `ochre_deviation_triage`, and fix HARES at the root.

**Acceptance test:** at the reference's rating and low-temperature
points, capacity and COP match the reference within a stated tolerance;
a test pins -17 °C.

**Verification:** the full block; heat-pump goldens with deltas.

**Dependencies:** none.

## OD-55: the divergence register misses deviations HARES already makes

**Kind:** found gap (operator, 2026-10-05). Documentation, tests and
architecture.

**Problem:** `docs/alignment/DIVERGENCES.md` lists D-001 to D-004, but
HARES deviates from OCHRE, and in places from OS-HPXML, in many more
places. Some are on the head (for example the OD-30 radiant and visible
gain split, the terrain factor, BESTEST-driven envelope choices, and
cause notes beside parity ratchets in `tests/parity/mod.rs`). Others sit
only in code comments, ledger entries or test comments. A deviation
recorded nowhere can be "fixed" back toward OCHRE, and one with no
pinning test can regress silently.

**Change:** audit the head for every deviation: code comments naming
OCHRE, OS-HPXML or EnergyPlus; parity and oracle cause notes; xfail
reasons; ledger entries classed (a); `docs/physics`, `docs/findings` and
`docs/eplus`. For each one, establish with evidence that HARES is right.
Write the D-entry in the register's format, and confirm a test pins it
(write one if none does). Any deviation the audit finds HARES wrong on
becomes its own ledger entry and is fixed.

**Acceptance test:** every deviation found has a D-entry and a pinning
test; each pinning test fails when its behaviour is reverted to
OCHRE's.

**Verification:** the block's test lines and `[writing]`.

**Dependencies:** branches in flight add their own entries at fold; the
audit runs on the head after them.

**State:** CLOSED (2026-10-07, branch `row/register-audit`, the audit
commit). Every register entry (D-001 to D-018) verified against the
code at the head; four citation defects corrected (D-004's pinning
citations moved to the metrics tests that hold them, D-014's defaulted
datapoint citation moved to `hvac.rb`'s `correct_ft_cap_eir` and its
DXCoils lines re-cited to v24.2.0 where the entry had cited the
vendored v26.2.0 tree's lines, D-015's `create_or_get_space` renamed
`get_space_from_location` at the same lines). The register's format
made consistent (every entry carries Quantity and Class; the sections
reordered numerically; D-013 moved to its sequential place). The
numbering collision resolved: the in-flight count-once block's
microwave entry is renumbered D-015 → D-019 and placed as D-019 of
record (see OD-43's numbering note). The unregistered-deviation hunt
added nine entries from the code's own divergence comments, the parity
bands and the findings docs: D-020 (the MSHP's default speed stages,
class unset), D-021 (the HPWH low-power UEF transition), D-022 (the
ZIP real/reactive wiring, with a new discriminating pinning test),
D-023 (the window U-factor decomposition), D-024 (the leap-year EPW
preservation), D-025 (the zone capacitance's air density), D-026 (the
windows' exterior long-wave exchange), D-027 (the scheduled space's
humidity-ratio blend), D-028 (the ASHP compressor lockout default).
Examined and needing no entry: the gas water heater's burner
efficiency (the resolver derives it by the references' own formulas;
the equipment honours the resolved value; both pinned), the boundary
halving's node order (topologically equivalent to OCHRE's), the secant
solver's degenerate-case guard (defined where OCHRE's path is not),
and the same-zone resistor trim (a fixed bug, now OCHRE's topology).
The resstock peak-power band note carries its ledger citation.
Bitwise-neutral on every frame: the goldens compare identical, both
profiles, and the census holds (1583 passed, 5 xfailed, 0 failed).

## OD-56: the attic and free-floating zones still diverge from the oracle after the attic fixes

**State:** PARTIALLY CLOSED (2026-10-07, branch `row/phys-od41-od56`,
folded). The window-solar candidate was a HARES reporting bug and is
fixed (the through-glass column split): the oracles' window solar now
closes to −0.5/−2.1/−1.1 W against OCHRE's 190.8/208.6/147.1. The
ground candidate is classed (a) and registered as D-018: OCHRE's slab
ground is a DOE-2 air-temperature correlation (schedule.py
import_weather) while OS-HPXML v1.12.0 reads the EPW's measured
monthly shallow ground temperatures (location.rb apply_ground_temps),
which HARES uses; measured, the winter ground drives 2.78 against
OCHRE's correlated 6.53, the conditioned winter floor −216.6 against
−724.8 W, the free-float floors −26.7 against +183.3 W (winter) and
−68.1 against −379.7 W (summer), and D-018 owns the bulk of the
free-float indoor MAE (winter means 11.43 against 13.38, MAE 1.944;
summer 25.82 against 23.75, MAE 2.070; spring 0.250 unchanged). The
conditioned-attic winter bias (−1.26 °C) remains open: the free-float
attic matches (0.283 MAE, 3.25 against 3.26) while the conditioned
attic warms +0.32 in HARES against OCHRE's +1.56 from the warmer
floor below, pointing at the attic-floor coupling; OCHRE's fixed
interior films measured 0.536 (deck) and 0.561 (floor) m²·K/W, the
same order as HARES's dynamic TARP films, no HARES defect
established. All oracle bounds unchanged and holding (the conditioned
attic 0.773/0.896/1.271 dynamic, 0.791/0.899/1.299 ideal; the
previously recorded mode labels were swapped, the values unchanged).

The last two candidates triaged 2026-10-08 (branch
`row/attic-film-triage`, the record in `02-measurement.md`'s "the
conditioned attic, 2026-10-08" entry), each reference-equivalent, no
defect. (1) The interior film on the roof deck and the attic floor:
the references read OS-HPXML v1.12.0's TARP inside convection
(simcontrols.rb:24-25) with explicit air-film layers
(constructions.rb:405, :508, :607), EnergyPlus v24.2.0's per-face
per-step TARP (ConvectionCoefficients.cc:1879-1925, the interior
face's negative cos at :1938-1940, the Walton magnitudes in
ConvectionCoefficients.hh:409, :433, :455) and OCHRE's frozen TARP at
the clamped 12.9 K (envelope.py:342-402); HARES freezes the ASHRAE
Simple values (film_coefficients.rs:444-460) with the per-step
correction disabled (solver_builder.rs:1248-1252), the divergence
registered as D-031. Freezing OCHRE's form on the deck face and the
attic floor moves the winter bias −1.25 → −1.11 and the summer bias
+0.51 → +0.34 (both toward OCHRE, at most 0.17 °C); enabling the
per-step form moves the attic 0.03 °C the other way. The films do not
establish the bias. (2) Ground long-wave exchange: no
ground-temperature long-wave term exists in EnergyPlus v24.2.0 (its
ground view's temperature is the outdoor dry-bulb,
HeatBalanceSurfaceManager.cc:9199), in OCHRE (the non-sky share at the
outdoor-air temperature, Envelope.py:145-149, outdoor-facing surfaces
only, :186, :772) or in HARES (longwave_radiation.rs:194-196, pinned;
ground-facing boundaries take no exterior long-wave,
solver_builder.rs:148-152). The upper-bound probe (the whole non-sky
share at the EPW shallow ground temperature, a term neither reference
has) moves the bias −1.22 → −1.13 winter and +0.49 → +0.28 summer and
would diverge from both references and EnergyPlus at once. The winter
bias stays open at −1.25 (ideal) / −1.22 (dynamic) with its cause
unestablished; the oracle bounds reproduce unchanged at the branch
head.

**Kind:** investigation, then fix (found on `fix-or18c`, 2026-10-05).
Physics.

**Problem:** with the attic fixes (capacitance multiplier, terrain
factor, geometric gable volume, roughness from material), the
conditioned oracle passes its 3.0 °C bound but still shows systematic
gaps. The attic runs about +0.5 °C hot in summer and -1.2 °C cold in
winter (winter MAE 1.28 to 1.31 °C). The free-floating indoor MAE is
summer 2.06 °C and winter 1.94 °C, against spring 0.25 °C. Roughness
from material (asphalt shingles Very Rough) moves the summer attic from
1.48 to 0.89 °C by departing from OCHRE's 1.67, which may be
compensating an untriaged difference.

**Change:** triage per `decision-log.md` `ochre_deviation_triage`.
Candidates: sky and ground long-wave exchange, roof absorptance and
emittance, radiant barrier, attic ventilation flow, deck and framing
mass, and the free-float case's boundary conditions. The `fix-or18c`
review (2026-10-05) ruled out sky long-wave, which matches EnergyPlus,
absorptance and emittance, which are equal on both sides, and a radiant
barrier, which BEopt does not have. It measured ventilation sensitivity
at about 0.1 °C and roughness at about 0.05 °C in winter. Attic means
are HARES 3.56 °C against OCHRE 4.82 °C in winter, a -1.26 °C bias that
roughness does not touch. The remaining candidates are the interior
TARP film on the roof deck and attic floor, and ground long-wave. Check each against
OS-HPXML v1.12.0 and EnergyPlus v24.2.0, cited by file:line, and fix
HARES where it is wrong.

**Acceptance test:** each candidate classed with evidence; each HARES
error fixed with a test that fails before the fix; the oracle
re-measured against its unchanged bounds.

**Verification:** the full block; oracles and goldens before and after.

**Dependencies:** `fix-or18c` folded first.

**State:** FOLDED as `69a00804`..`20b2cbd0` on the head (2026-10-08,
branch `row/attic-film-triage` rebased onto the folded b17 and d020
chain). The interior film and ground long-wave candidates triaged (the
"the conditioned attic, 2026-10-08" entry in `02-measurement.md`): the
per-step TARP correction is wired but disabled in production, the
enum's `#[default]` derive re-pointed at the production pin
(`AshraeSimple`) with `the_enum_default_is_the_production_pin` pinning
it, the floor coupling's frozen films carrying at most about 0.14 °C of
the winter cold bias on the two probed boundaries, and no
ground-temperature long-wave term on either side of the oracle (the
upper-bound probe moves the bias −1.22 → −1.13 winter and +0.49 → +0.28
summer); the winter bias stays open at −1.25 (ideal) / −1.22 (dynamic)
with its cause unestablished; the oracle bounds reproduce unchanged at
the branch head, the goldens identical (no re-capture).

## OD-57: dwelling assembly and stepping still substitute, skip or discard silently

**Kind:** found defect (the OD-30 review of `8ded384b`, 2026-10-05).
Same class as OD-48, but in `hares-core` assembly and stepping rather
than the `hares-io` resolvers.

**Problem:**
- `conversions.rs:451,484,564,575`: unrecognised or absent zone text
  becomes zone 0 or Outdoor; `parse_zone_label` (`building.rs:3150-3166`)
  lets a misspelt `AdjacentTo` reach it.
- `dwelling/mod.rs:2432,2452`: the occupancy invariants hold only under
  `cfg(debug_assertions)`, so in release a negative occupant count or a
  missing occupancy column silently gives zero occupant heat.
- `autosize.rs:225,639,671,818-838`: mains temperature, a missing bedroom
  count and a zero-capacity water heater are substituted with defaults;
  cooling-load autosizing runs with no floor area (occupancy-only gains).
- `dwelling/mod.rs:2784,4434,4444`: metrics are zeroed when their
  calculator fails to initialise, and output is dropped when the
  recorder fails to re-initialise, each with only a warning.
- `loop_allocator.rs:32-43`: `.ok()?` swallows typed-config errors.
- At run time, `update_control` errors are discarded with `let _ =`; a
  load whose step fails is rolled back every step and contributes
  nothing for the whole run.
- `schedule_resolve.rs:1397-1445`: a schedule column for equipment the
  HPXML lacks creates nothing and logs nothing.
- Found on `fix-headbreaks` (2026-10-06): `Generator::new` takes
  `typed().ok()` and falls back to Gas as the fuel; a zone with no
  infiltration input runs at 0 ACH; HVAC with no setpoint source runs
  on static setpoints with only a tracing warning, and `value_at().ok()`
  swallows source errors; the boiler's `loop_return_temp_c` decodes the
  fluid payload with `.ok()?` every step (an error lost, and a decode in
  the hot loop); the water heaters take `unwrap_or_default()` for the
  loop id (the boiler's is now bound at init on `fix-headbreaks`); one
  `type: ignore` remains in `notebooks/`, outside pyright's include.

**Change:** each becomes a typed error naming the input, or, where
OS-HPXML v1.12.0 has a defaulting rule (cited by file:line), that rule
with a warning naming the default. Invariants that guard physics hold
in release builds. A control update error reaches the caller. A load
that fails every step fails the run under the failure budget instead of
contributing nothing.

**Acceptance test:** for each site, an input that hits it fails with
the typed error or resolves to the cited default; a test proves the
code before the fix accepted it silently.

**Verification:** the full block; fleet resolution over the committed
buildings (any home that now errors is an input defect to record).

**Dependencies:** OD-48 (the same rule in `hares-io`); fix after
`fix-od30` folds.

**State:** FOLDED as `ee386cc7`..`5c1bf548` on the head (2026-10-08,
branch `row/od57-silent-substitution` rebased onto the folded b17, d020
and attic-triage chain, with the branch's fold fixes `65a89b0d` (the
red probes' water-heater configs carry the folded appliance draw flow
rate source field) and `5c1bf548` (the six dwelling goldens re-captured
at the folded head's wet-appliance volumes and multispeed stage
tables)). The branch's register entry, filed as D-032 on the branch,
folds as **D-033** (the HPXML "other" location family): d020's
multispeed Air Flow Ratio entry keeps D-032, `building.rs`'s comment
renumbered with the heading. The fold gates: nextest 7575
(`--all-features`), 7508 (`-F dst`), 7557 (`-F observe`), pytest 1483,
pyright 0 errors, the eight goldens identical. Site-by-site disposition
at the branch's head. Already strict on the base commit (the
silent-default sweep's content, folded in `c561eb69`, and the fixes it
carried; each verified in the branch's code, not assumed): the zone-text
sites `conversions.rs` `zone_type_to_label` (an unrecognised text is an
error naming the boundary and the text) and `resolve_exterior`
(unrecognised or absent exterior adjacency errors), `parse_zone_label`
(a misspelt label returns `ZoneType::Other` and the consumer errors);
the occupancy invariants hold in release builds
(`dwelling/mod.rs:2733-2760`); the cooling autosizing's missing floor
area is an error (`autosize.rs` `compute_default_internal_gains`); the
metrics calculator's init failure fails the metrics
(`engine.rs` `compute_metrics_from_batches`, pinned by
`a_metrics_calculator_init_failure_fails_not_zeroes`); the output
recorder's and the streamed metrics' init failures fail the roster plan
(`roster.rs` `plan_fresh_schema`, pinned by
`crates/hares-core/tests/roster_recorder_failure.rs`); the loop
allocator's `allocated_loop_ids` errors on a config that does not read
as its spec's type; the failure budget ends the run
(`record_equipment_step_failure`, pinned by
`the_run_stays_ended_after_the_failure_budget_error`); the schedule
column for equipment the HPXML lacks creates the equipment from the
cited RESNET default or warns naming the column
(`ensure_specs_for_csv_columns`); the conditioned zone's missing
infiltration measurement errors citing `airflow.rb:124` and the attic,
garage and foundation rules follow `airflow.rb`; a missing HVAC setpoint
source resolves to the cited OS-HPXML manual-thermostat defaults
(68/78 °F, `defaults.rb:2827-2848`) with a warning naming the default,
and a source whose read fails errors
(`resolve_profile_setpoints`); the storage water heaters' step-time
source reads propagate errors (`resolve_storage_step_inputs`); the
`update_control` discard had become the deferred control-error contract
(the error recorded on the equipment, the following step fails with it
via `take_control_error`, under the failure budget). Fixed by this row
(the remaining sites): a boundary whose interior adjacency is absent
errors naming the boundary instead of the zone-0 fallback
(`find_zone_idx`), with a window's or skylight's interior zone derived
from its attached surface (the OS-HPXML subsurface rule,
`geometry.rb` `set_surface_interior` :1470-1476 via :510-523,
:555-559, :602-606) and a dangling `AttachedToWall` failing the parse
naming both ids; the autosizer's water-heater mains temperature, missing
bedroom count and non-positive capacity are typed errors naming the
input (no OS-HPXML default exists: the mains temperature is
EnergyPlus's weather-derived value, `measure.rb:484`;
`NumberofBedrooms` is required and `apply_water_heaters` reads it with
no default, `defaults.rb:3303`; a zero-bedroom studio keeps its 50 gal
default tank); the boiler's `loop_return_temp_c` decode errors naming
the boiler and the loop instead of substituting the configured return
temperature (the first step's absent payload keeps it); every water
heater class requires the loop id its wiring or the loop allocator
assigned at init instead of the sentinel (the boiler's `require_loop_id`
lifted to `hvac/helpers.rs` and shared); the tankless's init-time mains
source read propagates its error naming the heater; the generator's
stated-fuel init error is pinned; the last `type: ignore` is gone with
`notebooks/` inside pyright's include. RED evidence: the probes of
`crates/hares-core/tests/od57_red_probes.rs` fail against the reverted
sources (the four water heaters' and the tankless's init returned Ok on
the silent sentinel and the silent mains substitution; the parse
accepted the dangling subsurface wall with the window's interior zone
None), and the base's own tests asserted the silent substitutions being
removed (`wh_autosize_unreasonable_mains_temp_uses_default`,
`wh_autosize_none_bedrooms_uses_conservative_defaults`,
`find_zone_idx_none_zone_type_returns_zero`), each flipped to the strict
contract. Gates: fmt, clippy `--all-features --all-targets -D warnings`,
nextest 7518 (`--all-features`), 7670 (`-F dst`), 7719 (`-F observe`),
7663 (`-F profiling`), 7611 (`--release`), pytest 1483, pyright 0 errors
with notebooks included. The eight goldens bitwise identical
(`compare-all` plus `-F dst`); no re-capture: the committed homes'
inputs already resolved to the values the strict sites demand (the
subsurface inheritance lands on the zone the fallback named, the loop
ids were allocated, the fluid payload decodes). Fleet resolution green:
`resstock_2025_fleet` identical; no committed home errors; no input
defects to record. No checkpoint state shape change, no version bump.

## OD-49: a missing defaults file warns and runs with empty defaults

**Kind:** found defect (the OD-30 review round, 2026-10-05).

**Problem:** `crates/hares-core/src/dwelling/blueprint.rs` logs a warning
and continues with empty defaults when the defaults file cannot be
loaded. Every value the file would supply then falls to whatever each
consumer does when a key is absent. When the OD-30 round made it an
error, 306 tests failed, because synthetic TOML and Python test setups
depend on this path instead of loading the shipped defaults.

**Change:** a missing or unreadable defaults file is an error. Tests and
Python setups load the shipped defaults through one helper. Any test
that really needs empty defaults says so explicitly through a typed
constructor, not through a missing file.

**Acceptance test:** a dwelling built with a defaults path that does
not exist fails with a typed error naming the path; the 306 tests pass
on the shipped defaults with their assertions unchanged.

**Verification:** the full block; the goldens identical, since they
already load the shipped defaults.

**Dependencies:** none.

## OD-58: the HPXML v3 ShieldingofHome spelling is read by no parse path

**Kind:** found defect (the OD-10 review's observation; the same
silent-drop family the flue fix closed).

**Problem:** the parser reads only `ShieldingOfHome` (the current
element, case-sensitive exact match). Two committed parity fixtures
declare the v3 spelling `ShieldingofHome`; their declaration is
dropped silently and resolves to the default. Today's default equals
their declared value, so no number moves, but a document with the v3
spelling and a non-default value would silently lose it.

**Change:** reject the v3 spelling with a parse error naming the
current element (the OD-10 rule: a legacy name is named and migrated,
not silently honored); the two committed fixtures migrate to the
current spelling.

**Acceptance test:** a document with the v3 spelling fails the parse
naming the current element.

**Verification:** `[pytest]`; goldens identical (both committed
fixtures' values equal today's default).

**Dependencies:** none; lands with OD-10's numeric commit or the next
parser pass.

## OD-59: the helics wheel's libzmq declares no libstdc++ dependency

**Kind:** found defect (an upstream wheel defect; the local mitigation
in place).

**Problem:** the helics 3.6.1 wheel's `libzmq.so.5` exports the C++
standard-library symbols it was built against (2022 `_ZSt`/`_ZNSt`/
`__cxa` definitions) with no `libstdc++.so.6` NEEDED entry. Importing
helics before pyarrow binds those symbols from libzmq; the two
libraries then disagree about the C++ runtime and the first pyarrow
parquet write segfaults the process (measured: a minimal probe, and
the full suite's workers crashed in test_py_fleet's parquet writes
whenever the helics module shared the worker). Importing pyarrow
first resolves every symbol from libstdc++ and both libraries work.

**Change:** tests/python/conftest.py imports pyarrow before any helics
import can run (every pytest worker imports the conftest first); the
defect is pinned loudly by
tests/python/test_helics_pyarrow_symbol_order.py (the ELF read of
libzmq's NEEDED entry, and a subprocess canary asserting the bad
order still segfaults: both flip when a fixed wheel lands, which is
the signal to delete them and the conftest workaround).

**Disposition:** upstream report to the helics maintainers (the wheel
should declare or link its C++ runtime); the operator sends it. The
local mitigation stands until then.

**Dependencies:** none.

## OD-54's State (2026-10-07, branch `row/phys-od54-od47`, folded)

FIXED. Triage: class (a), OCHRE is wrong. The single-speed ASHP default
curves were OCHRE's `Biquadratic ASHP Heater.csv` `Single_1` column:
the pre-v1.12.0 DOE-2 fits, with bounds that clamp nothing. Below the
17 °F fit anchor the quadratic extrapolation is unphysical: at
−17 °C capacity 0.485 and EIR 1.545 where the reference reads 0.432
and 1.785. The fix: the defaults become the OS-HPXML v1.12.0 RESNET
Addendum 82 anchor model — the capacity line `0.813 + 0.02244·To` and
the quadratic through the reference's three EIR points, exact at the
anchors and at −17 °C to 5e-5; the full-load compressor COP at
−17 °C is 1.56 for the 12 kW HSPF 9.5 class. The premise's arithmetic,
corrected: the compressor-only COP never fell below 1 above the lockout
(1.80 full-load measured pre-fix); the ledger's 0.9 was the
whole-equipment delivered COP at roughly 20 % part load, dominated by
the fan and crankcase overhead. The defrost treatment is not the
defect (at −17 °C with physical air the frost delta-w sits at the
model's 1e-6 floor). The absolute COP stays below v1.12.0's read of
the same unit (about 2.0): HARES keeps OCHRE's HSPF conversion where
v1.12.0 uses the cop47full table, registered with the biquadratic
form's mid-band deviations and the unmeasured multi-speed default sets
as D-014 in docs/alignment/DIVERGENCES.md. Pins: the curve anchors and
the −17 °C extrapolation, the live step at −17 °C, the multi-speed
columns; each fails when the defaults revert. The heat-pump goldens
re-captured with the deltas.

## OD-47's State (2026-10-07, branch `row/phys-od54-od47`, folded)

FIXED. Root cause: two name maps disagreed — the registry tags
"Exterior Lighting" LIGHTING (matching OCHRE's LightingLoad) while the
output frame's equipment_name_to_end_use did not, so Exterior
Lighting's power landed in the Other aggregate. The fix: the LIGHTING
arm gains the tag; the aggregate holds indoor plus exterior. The Other
mapping keeps its members (Exterior Lighting was never one on OCHRE's
side); the consumer's channel mapping follows unchanged (the fleet
aggregation maps by unit). The strict xfail heals: the mark is off,
the case passes within 1e-9, all twelve parity cases pass. The goldens
re-captured: the Lighting column rises, the Other column falls by the
same share, energy conserved between the buckets.

## OD-60: inject_constant_power_schedule still zero-fills an absent power

**Kind:** found defect (the OD-24/25 review's observation; the same
class OD-25 closed at its three sites).

**Problem:** `inject_constant_power_schedule`'s
`power_w ... unwrap_or(0.0)` (schedule_resolve.rs, near :1488) runs a
constant-power schedule at 0 kW when the spec carries no power, the
same silent class OD-25 made a resolve error at the profile and
column branches. It predates I-08 and no cited entry covers it.

**Change:** the same resolve error naming the equipment and the
missing field, through OD-25's `resolve_max_kw`.

**Dependencies:** none; lands with the next pass over
schedule_resolve.rs.

## OD-61: two OCHRE parity residuals carry no established cause

**Kind:** investigation (found on `fix-or18c`, 2026-10-05, after the
parity comparison was made fair). Physics or comparison.

**Problem:**
- `resstock_bldg0112631_24h`: the peak HVAC power residual is 25.4 %
  over a 24 h window, held by a 25.7 % lock from `3f463c34` with no
  stated cause.
- `cz5a_minisplit_gas_wh`: with OCHRE's duct losses removed (its
  distribution efficiency forced to 1 in seeded runs), HARES's total
  site energy is 5 to 8.5 % above OCHRE's. The 33.6 % and 32.3 %
  residuals against the corpus reference are class (a), OCHRE's duct
  losses on a ductless unit; this remainder is unclassed.

**Change:** triage each per `decision-log.md` `ochre_deviation_triage`,
by end use and step against the reference, citing OS-HPXML v1.12.0 and
EnergyPlus v24.2.0 by file:line. Fix HARES or the harness where they are
wrong; keep a band only on a proven class (a), with the measurement
beside it.

**Acceptance test:** each residual classed with evidence; the resstock
peak lock removed or justified beside the band.

**Verification:** the parity suite; the full block.

**State:** RESOLVED on the branch (2026-10-06, commit `5135087a` and
`ffffff2c`; the branch awaits its fold):
- `cz4a_ashp_hpwh`: the comparison was unfair, not the physics: the
  reference carried OCHRE's own unseeded hot-water draw and the vendored
  fork's `WaterHeater.py` read `self.hp_cop` before it was set. The
  fork's fix (local commit `58f39d8`: `cop_nominal`, set from kwargs
  before use), the draw pinned at `18.083026715940175` in the fixture's
  config, numpy seeded, columns ordered. Measured residual now
  5.26 % HVAC / 4.89 % site, inside the default bands, in line with the
  sibling heated fixture. Two regenerations byte-identical.
- `cz5a_minisplit_gas_wh`: the duct remainder closed (1.17 % site after
  the orphan-duct removal; the 34.6/33.3 overrides dropped). What
  remains is a PEAK residual: 20.46 % against the 20 % band, the
  recovery minute's max draw (HARES 17.84 kW against OCHRE's 14.81,
  the energies 1.2 % apart). Band 21.5 with the measured cause beside
  it; WHICH side's maximum draw the reference tools support is not yet
  established: the open question lives in the parity band's comment
  (tests/parity/mod.rs).
- The cz5a peak question: ESTABLISHED on the d020-mshp-stages branch
  (2026-10-08). The recovery minute is ER-only on both sides: the
  compressor locks out at OCHRE's 0 F default (-17.78 C,
  ochre/utils/hpxml.py:956-963) above the window's -19.4 C. HARES's
  binary backup strip fires its full 17.58 kW rating for the minute
  plus ~0.3 kW fan (17.89 kW measured); OCHRE's ER fills the ideal
  load 14.41 kW plus its 0.25 kW fan and the 0.15 kW pan heater
  (ochre/Equipment/HVAC.py:1396-1407, 1427-1436, 1482-1497; 14.81 kW).
  EnergyPlus's unitary heat pump fires the full supplementary element
  as HARES does; OCHRE's ideal ER fill is its own continuous dispatch.
  The default stage table has no vote: the window never operates the
  compressor. The question's resolution is written beside the band in
  tests/parity/mod.rs; the default stage table itself is fixed and
  registered as D-020 in docs/alignment/DIVERGENCES.md (the shipped
  multispeed CSV's renormalized MSHP Heater ratios restored to OCHRE's,
  the heater's even-spacing fallback replaced by OCHRE's stage table).
- `resstock_bldg0112631_24h`: cz6b's residual classed (a) as
  D-012 in docs/alignment/DIVERGENCES.md: OCHRE's fitted RC network
  against the constructions HARES keeps: the same rating, a different
  duty (24 against 30 on-minutes), the window ending mid-cycle, the
  blower accounting 0.20 kWh of the 1.259 kWh difference; the indoor
  temperatures agree to 0.41 °C MAE. Bands 23.1/21.9 with the
  measurement beside them. No HARES change toward OCHRE's numbers.
- The resstock peak lock: cz6b's disposition above; the lock's home
  fixture's peak is the same fitted-network class and stays on its band
  with the cause.

**Dependencies:** `fix-or18c` folded first.

## OD-62: a non-ideal unit's mode override sticks after its demand-response event ends

**State:** FOLDED as e3aae247..a0870f28 on the head (2026-10-08). The
release path landed (ControlSignal::ModeOverrideRelease + mode_release(),
UserOverride tier, every override-writing class clears); the independent
review's blocker (the Python to_dict surface missing the arm: the
implementer's full-gate claim was false) fixed with the Python emit/parse
surface made symmetric; the review's tracking hole fixed faithfully
(cumulative tracking across rejected re-dispatches; the freeze-guard's
downgraded setpoints tracked and released at event end, checkpoint v3
with both earlier layouts refused by the version gate); the tier doc
table completed; the probe test renamed to cover both layouts. Review
SHIP with mutation proofs (the wholesale-clear regression fails the
rejected-mid-event tests; the old dedup fails the multi-step ByEndUse
test). Gates 7487/7487, goldens bitwise. Pre-existing residual: an
overlapping ByName and ByEndUse target still yield two event-end
releases (cross-variant equality; unchanged behaviour, noted).

**Kind:** found defect (the OD-16 finisher's instrumentation; a
documented limitation both prior reviews accepted as a residual).

**Problem:** for the non-ideal units the mode override is stored
(`heat_pump/heater.rs:177`, `air_conditioner.rs:116`) and the event's
end cannot clear it (`dr_compliance.rs:766-775`): `OperatingMode::Auto`
does not exist, so a demand-response event that set a unit's mode
sticks that unit at its forced mode after the event ends. Only the
single-mode unit the event named is affected; heating scoping is
unaffected, and the ideal units re-derive every step.

**Change:** an equipment-side contract for clearing a mode override (a
clear path in the mode-override surface) so an event's end returns the
unit to its own control; the DR event's release semantics then hold
for every unit class.

**Acceptance test:** a non-ideal unit forced off by an event resumes
its own control at the event's end (the step after the last event step
delivers per the thermostat again).

**Dependencies:** the next equipment control-surface pass.

## OD-63: Fleet.from_resstock points every building at the dataset zip and never sets the weather FIPS

**State:** FOLDED as 5ccdea61..bc40326d on the head (2026-10-08). The
constructor requires the unpacked buildings directory (zip paths are
ResStockError::MissingLayout naming the fix; build_zip_path deleted);
the FIPS threads from each building's own HPXML with typed failures
(Unreadable/MissingName/NotAFips wrapped as WeatherStation naming the
building and path); the 2024.x and 2025.1 weather filenames resolved
per version. A fleet from the committed fixture builds and runs 24 h
end to end through the public path with resolved weather. A first
deferral of the ledgered golden surface was OVERRIDDEN: the
resstock_fleet manifest kind landed (the [fleet] table with a
FleetReport construction summary, Fleet::report()/bldg_ids()/
with_output_verbosity/with_retained_output), the three-row
metadata_fleet3.parquet fixture and the .gitignore exception landed,
the golden captured twice byte-identical. Review SHIP; gates 7501/7501,
compare-all 7/7, pytest 1480, pyright 0. Known tolerance (pre-existing,
later row errors it): unresolved weather warns with an empty path.

**Kind:** found defect (the fix-headbreaks session's F1 investigation;
recorded at the handoff, unledgered until now).

**Problem:** `Fleet::from_resstock` builds every config's
`hpxml_path` and `schedule_path` through `build_zip_path`
(`hares-io/src/resstock.rs`), pointing them at the dataset `.zip`
archive, which nothing in the engine unpacks; `weather_fips` is always
`None`, so the weather mapping cannot resolve. Its fleets cannot build
a dwelling from the dataset as-is; the tests that use the constructor
swap in unpacked files.

**Change:** the constructor unpacks or requires unpacked paths (the
dataset layout resolved to real files), and the weather FIPS reaches
the weather mapping; the fixture-swap tests then build through the
public path.

**Dependencies:** none; lands with the next ResStock surface pass.

## OD-64: dwelling assembly falls back to an empty defaults store with only a warning

**State:** FOLDED as a9a11eeb on the head (2026-10-08). The row's
implementer established the silent fallback was already gone at the
phase-1 head: the error policy of the OD-49 fold governs this site too
(every DefaultsStore::load failure propagates as an error naming the
path; the empty store reachable only through the typed constructor).
The landed work is the pinning tests for the uncovered branches (a
malformed defaults file; a defaults path naming a regular file) plus
the shared failure assertion; mutation-verified (reintroducing the
fallback fails all four error pins). Review SHIP. Gates 7464/7464,
goldens bitwise. One policy, both sites, no special cases.

**Kind:** found defect (OD-49's class, a second site; recorded at the
handoff).

**Problem:** `blueprint.rs` near :117-131 falls back to an empty
defaults store with only a warning when the defaults load fails, the
same silent-degradation class OD-49 ledgered for its own site: the
dwelling builds with class-table defaults and the run reports success.

**Change:** the same rule as OD-49 (a defaults failure is an error, or
the degradation is typed and counted in the run's health, per OD-49's
landed shape; the two sites take ONE policy).

**Dependencies:** OD-49's shape (the silent-default sweep).


