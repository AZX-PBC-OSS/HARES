# L4 resolution (d): consumer (08) and numeric fixes (09)

Scope: every finding of `verify-L4-d.md`; the findings of `verify-L4-e.md`
that touch `ledger/08-consumer.md` or `ledger/09-numeric-fixes.md` (2, 4,
10, 11, 12, 16, 21, 24, 25, 29); the items `L4-resolution-a.md`,
`L4-resolution-b.md` and `L4-resolution-c.md` hand to these files; and the
two entries `decision-log.md` requires (`dl-i08-architecture.unavailable_periods`
and `.wet_appliance_hot_water`). Binding: `decision-log.md`, read in full.
Engine read with `git show`/`git grep` at `afa9a071` (and `21d90588`
where an entry cites it); consumer at `5c651bc` on `i08-consumer`;
OpenStudio-HPXML v1.12.0 cloned into the scratch directory; `uom` 0.38.0
from the cargo registry; `cargo add` behaviour measured in a scratch
workspace. Files changed: `ledger/08-consumer.md`,
`ledger/09-numeric-fixes.md` and this file.

The engine moved while this ran: OR-11 committed as `b8195c83` and SL-7
(row 20) as `7b7a6a7d`, before CN-17. Citations stay at `afa9a071`, the
commit the entries name; CN-17's gating text was brought up to date (see
"Found while resolving").

**Counts.**
- `verify-L4-d.md`: 21 findings (8 major, 13 minor): 21 fixed, 0 rejected.
- `verify-L4-e.md`, findings touching these files: 10, all fixed (16 was
  already fixed by `L4-resolution-a.md`; checked against the text).
- Items handed over: `L4-resolution-a.md` 4, `L4-resolution-b.md` 1,
  `L4-resolution-c.md` 1: 6 fixed.
- New entries: 3 (B-15, numeric; B-16, triage measurement; B-17, numeric,
  contingent on B-16).
- Found while resolving: 9 (4 fixed in these files, 5 handed to other
  owners).

## verify-L4-d.md

**1 (major), `si_census` pathspec matches nothing.** Fixed. B-14 commit 4
Acceptance, `si_census`:
"`git grep -niE "$P" -- ':(glob)crates/*/src/**' ':(glob)python/hares/**' > target/si-census.txt`
... `grep -qE '^crates/hares-types/src/telemetry_keys\.rs:[0-9]+:pub const ELECTRIC_KW: &str = "electric_kw";$' target/si-census.txt`
... The `grep -q` line is the self-check: the census must list a line
known to stay (a reporting name), so a pathspec or pattern that matches
nothing fails the step instead of passing it." Measured at `afa9a071`:
8 857 lines in 132 files (over `python/ochre_next`), self-check line
present. The Evidence states the same command and count.

**2 (major), census incomplete.** Fixed. Commit 2 Evidence gains "Also
written by commit 2's boundaries" (plug-load EV `range_miles` and
`EV_FUEL_ECONOMY`, `VehicleMappingEntry`, `py_hvac.rs:306`,
`child_energy_kwh`). Commit 3 Evidence gains the electrical solver and its
payload ("which no reader takes and no checkpoint holds; the checkpoint
holds the electrical summary"), `ElectricalAccumulator`, `zip.rs`,
`pump.rs`, `pack_electrical.rs`, `mode_flow_guard.rs`, `invariants.rs`,
`observer.rs`, `observer_capture.rs`, `diagnostics.rs`,
`determine_max_kw`/`determine_constant_kw` and the annual raw keys
(`DefaultScheduleProfile` has no `max_kw` field; the kW lives in
`determine_max_kw`), the Python telemetry surfaces, `TariffEvaluator::step`
and its Python binding, `PyTariffTelemetry`, `estimate_pv_capacity`, and
"every reader of run state that publishes it" (HELICS dwelling and fleet,
the gym bounds and reactive observation, the Rust gym reward). Commit 4
Evidence gains the rest of the EV driver and its modules, the BMS, DR
compliance and occupant actors, the `Actor` trait's `pv_kw`, `ActorSeed`,
the literal EV-driver arguments, and the control layer (`hares-control`
`signal.rs`, `compat.rs`, `dispatch.rs`; `py_control.rs`; `py_actor.rs`
`DispatchRequest`/`Signal`; `py_gym.rs:87-129`; the protocol bridge; the
HELICS signal readers). The `units.rs` helpers are named with call counts,
and commit 3's Change says which go: "`power_kw_to_w`, `power_w_to_kw` and
`WATTS_PER_KILOWATT`, `energy_therms_to_kwh`, `energy_kwh_to_therms`,
`power_watt_to_kwh_per_year` and `power_btuh_to_therms_per_year` are
deleted ... `power_btu_h_to_w`, `power_w_to_btu_h` and
`power_btu_h_to_kbtu_h` are rewritten on `uom`". "Boundaries that keep
their units" is rewritten as classes, each with its conversion site
(reporting names, output records, tariff rate data, catalogues, external
data files, HPXML inputs "converted once where read" including annual
kWh/yr and therm/yr, OCHRE's `P Setpoint`, external libraries, HELICS).

**3 (major), "exactly three places" against kW publishers.** Fixed by
moving the publishers to SI. Commit 3 Change: "The values leave the run
state in a reporting unit only at the output records of 'Boundaries that
keep their units' ... the HELICS publications become `total_power_w`
(`units=W`), `reactive_power_var` (`units=VAR`), `equipment_{i}/power_w`
and `pv_generation_w` ... the gym's observations become `total_power_w`
... with their `Box` bounds in W and VAR ... `PyTariffTelemetry`'s live
billing fields become `cumulative_import_j`, `cumulative_export_j` and
`peak_demand_w`". Boundary: "HELICS publications carry SI values and
declare SI units ... a federate that subscribes in kW gets HELICS's own
unit conversion." Test: `test_helics_publications_are_si_with_their_units`
(the dwelling's telemetry values on bits, `units=W` or `units=VAR`, for
`HELICSDwelling` on `der_pv_battery_900s` and `HELICSFleet`). The built-in
gym reward stays kW by design ("so a trained policy's reward scale does
not change"), converted once, and is listed as an output record.

**4 (major), positional Python parameters.** Fixed. Commit 2: "every
renamed parameter is keyword-only (a `*` before it in the
`#[pyo3(signature)]`) ... under PyO3 0.29 every parameter without a
signature, and every one before `*`, is positional-or-keyword, so
`PV("pv", 5.0, 25.0, 180.0)` would otherwise run after this commit and
build a 5 W array", with each signature written out. The same holds for
`size_pv_system`, `required_main_panel_ampacity`, `estimate_pv_capacity`,
`TariffEvaluator.step` (commit 3) and all three control-signal surfaces
(commit 4). Tests gain the positional forms
(`test_equipment_constructor_removed_keyword_names_replacement`,
`pv_sizing_returns_watts`, `test_tariff_evaluator_step_takes_watts`,
`removed_control_signal_keyword_names_its_replacement`), and the
`docs/python.md` changes section "states the keyword-only change".

**5 (major), resolved-model guard.** Fixed, keeping a permanent guard with
a re-capture rule rather than a one-commit test. The capture: "a
`resolved` product for every golden manifest ... `DwellingBlueprint::validate()`
... through the one adapter function the dwelling and fleet paths both
call ... per equipment in model order, its name and each
`ResolvedParameter`'s name, `Unit::symbol()` and value as the 16-digit hex
of `f64::to_bits`, written as JSON by `frame_golden`'s own writer
(`tools/frame_golden/src/resolved.rs`, new) ... so `ResolvedModel` gains no
serde derive." It compares only equipment parameters. Re-capture: "From
this commit on `[golden]` compares the `resolved` product of every golden,
so every later row that moves a resolved equipment parameter re-captures
it in its own commit through M-1's numeric entry procedure ... The later
rows known to touch the resolved model: `12-inputs.md` IN-9 (row 94d) ...
OD-2 site 6 (row 94e) and OD-10's numeric commit (row 94f) ... (outside
`equipment`, so their product compares unchanged unless an equipment
parameter moves) ... B-17 when it runs". Coverage of every parameter:
`equipment_config_values_are_unit_invariant`, "one case per row of the
commit 2 table and per field in it ... builds the config through its real
input path ... within 2 ULP ... This covers every parameter the resolved
model does not report". The fuel-economy claim is corrected in the golden
table: "the plug-load EV leaves `fuel_economy_j_per_m` unset,
`resolve_loads.rs:902`".

**6 (major), test cannot compile in `hares-io`.** Fixed (also L4-e 4 and
the `L4-resolution-b.md` note). `equipment_config_fields_are_si` stays a
`hares-io` walk that "calls nothing in `hares-core`"; new
`input_schema_override_classes_are_si`
(`crates/hares-core/tests/si_config_schema.rs`) asserts on
`hares_core::input_schema().overrides` and walks `SyntheticEventLoadConfig`,
a `hares-core` type.

**7 (major), relative difference near zero.** Fixed: "the scaled
difference is `max_t |a_t - b_t| / max_t |a_t|` (0 when both columns are
all zero), and the scale ratio is `max_t |b_t| / max_t |a_t|`. Each moved
column is recorded with both; a scaled difference above 1e-9, or a scale
ratio within 1e-6 of a factor a missed or doubled conversion explains
(1000, 3600, 3.6e6 or the therm's 105 505 600 J, or their inverses), stops
the entry."

**8 (major), consumer telemetry reads.** Fixed in CN-3: "every read of the
telemetry field `Dwelling.telemetry().reactive_power_kvar`
(`calibration/cosim.py:81`, `calibration/coupling.py:393`) reads
`reactive_power_var` and converts to the consumer's kVAR through that
helper, and the timing of that read at `calibration/scale.py:316` ...
reads `reactive_power_var`"; text sites `coupling.py:359-361`,
`cosim.py:25`, `scale.py:319`, `suite_ami2.py:2016` and
`suite_scale.py:160`; "The override rule `^power_limit_kw$`
(`calibration/coverage.py:106`) ... becomes `^power_limit_w$`"; census
extended with `telemetry\(\)\.[a-z_]*_(kw|kvar)\b|\^power_limit_kw\$`
(checked: at `5c651bc` it prints exactly the listed sites).

**9 (minor), repeated `-p`.** Fixed: "one call per crate (`cargo add`
takes one `-p` and has no `--workspace` flag, measured with cargo
1.94.1): `cargo add -p hares-core uom` and `cargo add -p hares-python
uom`"; commit 3 the same for `hares-types` and `hares-tariff`. (The
suggested `--workspace` form does not exist either; measured.)

**10 (minor), `PHASE_POWER_SUFFIX_*`.** Fixed: "`git grep -niE 'const
[A-Z0-9_]+: &str = "[A-Za-z0-9_]*(_kw|_kwh|_kva|_miles)[A-Za-z0-9_]*"'`
... Every `&str` constant counts ... at `afa9a071` the first command
prints 22 lines, among them `crates/hares-equipment/src/event_load.rs:136`"
(measured).

**11 (minor), `wh` matches water heaters.** Fixed: watt-hours match "only
as the energy totals that carry them ... because `_wh` alone is also the
water-heater abbreviation"; therms only in the plural, because `e_therm`
and `burner_w_therm` are thermal (both measured).

**12 (minor), existing `si_guard`.** Fixed: `si_guard` "widens its scan
... to the `src` of every workspace crate and gains the unit-suffix rule
... with a per-file allowlist whose every entry names the boundary ...
commits 3 and 4 shrink it, and after commit 4 it lists only boundaries";
"A file `syn` cannot parse is a test failure naming it"; "Commit 3 removes
`scheduled_load.rs` from the `therm` rule's allowlist". The Python half
stays with `si_census`, so the two do not overlap.

**13 (minor), therm and Btu/h in `uom`.** Fixed: the two units are defined
"once with `uom`'s `unit!` macro in `hares-types`
(`crates/hares-types/src/si_units.rs`, new ...): `therm` (energy,
1.055056e8 J, 100 000 of `uom`'s `btu_it`, which is the value
`energy_therms_to_kwh` computes today) and `btu_it_per_hour` (power,
0.29307107 W, today's `BTU_PER_HOUR_TO_WATT`)", pinned by
`therm_and_btu_per_hour_units_keep_the_parent_factors`. Measured: `uom`
has `therm_us` and `therm_ec`, neither equal to HARES's therm, and no
Btu/h; `unit!` compiles outside the crate.

**14 (minor), actor fixture coverage.** Fixed: "`der_pv_battery_900s`
reaches the BMS actor and its control signals and `der_pv_battery_ev_900s`
reaches the BMS and EV-driver actors and theirs (`month_boundary_billing_3600s`,
whose cz2a building has no PV, battery or EV, reaches the billing state
only)".

**15 (minor), net metric tolerance.** Fixed: "`|a - b| <= 1e-12 * G`,
where `G` is the sum over steps of the absolute step contribution for an
accumulated metric ... and the magnitude of the value itself for a peak or
a count".

**16 (minor), transitional signal conversion.** Fixed in commit 3:
"Control signals still carry kW, kVAR and kWh at this commit ... so each
equipment that applies a signal value converts it to SI once with `uom`
where it applies the signal ... commit 4 deletes those conversions"; commit
4: "the conversion commit 3 put at each application is deleted".

**17 (minor), `max_charging_kw` row.** Fixed: "`max_charging_kw` keyword,
which is not a field of `EvConfig`, takes the Rust field's name and its
removed-key row is written out explicitly (`EvConfig.max_charging_kw`
names `EvConfig.max_charging_power_w (W)`)".

**18 (minor), reporting names in the boundary list.** Fixed: the first
boundary class, "Reporting names", lists the catalogue columns, the 34
`telemetry_keys.rs` constants and the actor variables `pv_kw`, `load_kw`,
`bms_pv_stale_kw`, `bms_pv_actual_kw`, `charge_kw`.

**19 (minor), CN-4 dependencies.** Fixed: "`11-other-defects.md` OD-9,
OD-2's site 6, OD-10's numeric commit and OD-20; `12-inputs.md` IN-8,
IN-9; `04-fleet.md` FL-5", in the Change's list and Dependencies.

**20 (minor), contract drift after CN-3.** Fixed. Preamble assignment:
"CN-15 for the engine rows committed after CN-3's engine:
`02-measurement.md` M-4a and M-4b and any CI fix entry of the closing
sequence". CN-15: "every DRIFT item it reports is resolved in this commit
... a CI fix entry that moves a number ... stops the closing sequence
before M-4b for an operator decision, because
`dl-i08-architecture.consumer` allows one consumer re-baseline (B1)".

**21 (minor), citation order.** Fixed: "Python `ChargingStrategy.v2g`,
`.solar_surplus` (`py_enums.rs:1699, 1721`)".

## verify-L4-e.md (findings touching these files)

**2 (blocker), redirect restore and bundle outside the repositories.**
Fixed. Gate block: "`# only on a gate run that ends in a commit:
build_bundle.py on the staged change`" and "`build_bundle.py --out
scratch/i08/<sha>/bundle.zip`"; bullet: "A gate run that ends in no commit
(a B0 check, the row 16g re-check, a measurement) runs the other gate lines
and records that the bundle step did not run, so it changes no file in
either repository." No `git show HEAD:<file> > <file>` remains in either
file. CN-17 adds the argument (`build_bundle.py:344` has no argument
parsing; `:384-391` writes beside `ROOT`): "`build_bundle.py` gains
`--out <file>` ... The zip writing moves into one function,
`write_zip(root: str, dest: str)`", test
`suite_calibration.bundle_zip_goes_to_the_named_path`, and CN-17's
Verification lists the files beside the repository before and after.
CN-15's block passes `--out` too. The `02-measurement.md:586` half is
another owner's (below).

**4 (major), `hares_io::input_schema()`.** Fixed as verify-L4-d 6.

**10 (major), R7.** Fixed in CN-1: "A fit writes no file (requirement R7):
every evaluation's injected inputs are content handed to the engine" via
`Text`, `Defaults` and one `engine.engine_inputs(spec)`; "The fit-level
`scratch` argument ... is deleted"; the per-slot injectors become "one
`Injector` per `Fitter`". Tests `suite_fit.fit_writes_no_slot_files` and
`suite_engine.content_spec_runs_like_a_path_spec`. The dependent text of
CN-3 (`no_evaluation_replaces_a_defaults_file`; the `Defaults` form goes
with `DefaultsVariant`), CN-6's precondition, CN-10 and CN-11 (member
index, replay test) follows.

**11 (major), R20 warm-up documentation.** Fixed in B-2: "`docs/python.md`
gains a 'Warm-up' section (requirement R20): the four criteria and their
defaults ... that warm-up steps are never reported ... that `warmup=None`
disables warm-up ... the outcome is read from `RunResult.health.warmup`
... `test_docs_warmup_example` (Python) runs it as written", named in
Verification.

**12 (major), `[writing]` in the consumer gate.** Fixed (with
`L4-resolution-a.md`'s item): "`# ids and writing() exactly as plan.md
§3's verification block defines them` / `writing
~/src/HARES_CALIBRATION_SYSTEM # [writing]`", and the bullet "`writing`
runs on the entry's commit, after it is made". The definitions are
referenced, not copied, so the two cannot drift. CN-15 runs it too.

**16 (minor), draft history in 08.** Already fixed by
`L4-resolution-a.md`; checked: neither passage remains.

**21 (minor), B-14's verification round.** Fixed in B-14 as well: "Row
94c starts only after a verification round over this entry records zero
blockers and zero majors in `plan-critique.md`".

**24 (minor), physics-decision record.** Fixed for this file's named
entries: B-2 ("`docs/PHYSICS_DECISIONS.md` gains the warm-up section ...
OCHRE runs a fixed `initialization_time` with no convergence test
(`vendors/OCHRE/ochre/Simulator.py:136-140`)"), B-11 and B-12 (their
Verification); the preamble rule already covers every entry, B-15 and
B-17 included.

**25 (minor), `legacy_fleet_rules`.** Fixed at B-11:
"`unit_keyed_fleet_rules` and FL-5 deletes it".

**29 (minor), V-IDs.** Fixed on the Kind lines: B-1a and B-1b V4; B-1c V5;
B-2 V22, V24; B-3 V23; B-4 V6; B-5 V9, V10; B-6 V14; B-7 V29; B-8 V8;
B-9 V42; B-10 V19; B-11 V49 ("its explicit `Mode` aggregation kind").

## Items handed to these files

- `L4-resolution-a.md`, CN-17 patch target: "with `HC.paths.HARES_REPO`
  (the module object the script holds, not `calibration.paths.HARES_REPO`)
  patched", and in the Change "The script imports `paths` flat (as it
  imports `hares_contract`; `paths.py` imports only the standard library)
  and reads `paths.HARES_REPO` at call time".
- `L4-resolution-a.md`, BR-19's test: CN-17 Problem 6, Change and
  Acceptance `suite_scale.capture_refuses_an_engine_built_outside_the_resolved_tree`.
- `L4-resolution-a.md`, the `writing` line: as L4-e 12 above.
- `L4-resolution-a.md`, CN-0 Landed: "The capture's provenance guard ...
  landed after it as `f7131d7` and `aec2e3f`, recorded as
  `01-baseline-repair.md` BR-19 (`00-schedule.md` row 0e)". The guard's
  bullet left CN-0's Change, which had claimed it.
- `L4-resolution-b.md`, the `input_schema` test: as verify-L4-d 6.
- `L4-resolution-c.md`, the rename at B-11: as L4-e 25.

## New entries (decision log)

**B-15: OpenStudio-HPXML unavailable periods are applied.** From
OpenStudio-HPXML v1.12.0: the input is the HPXML
`SoftwareInfo/extension/UnavailablePeriods` element; the end-use table is
`resources/data/unavailable_periods.csv`; the interval is half-open
`[begin day and hour, end day and hour)` with year wrap; no space heating
or cooling sets the load fraction to 0 and leaves the setpoints; the water
heater holds 2 C; PV runs through an outage; design levels come from the
schedules before the periods; the ResStock `in.schedules.csv` flag
columns are OpenStudio-HPXML's own export. HARES reads the element, gates
schedule-driven and equipment end uses by the table (equipment through
`EnvironmentState` beside `bus_energized`), applies the natural-ventilation
option, and turns the four `NotUsed` columns into check columns, so it
accepts nothing it ignores. Census: `bldg0000001` 168 h,
`bldg0000003` 72 h, `bldg0000007` and `bldg0176227` 24 h of
`No Space Heating`, `bldg0527060` vacant all year. (`bldg0176227` carries
96 steps, 24 hours on 2018-10-04, not 96 hours.) Golden re-capture:
`consumer_shape_900s`; the physics suites at the parent and after; the
consumer side in CN-3 (the four columns keep their calendar rows under
`Schedule.rotated`).

**B-16: wet-appliance hot-water triage** (measurement). HARES draws 15 L and
6 L per cycle (`resolve_loads.rs:258, 432`), and on the HPXML path the
realised volume is the draw rate times the schedule run length, so it
depends on the step. The entry records, per golden home, the
OpenStudio-HPXML and OCHRE daily volumes and HARES's realised volume, with
a rule: a difference above 1% from OpenStudio-HPXML, or above 1% between
900 s and 3600 s, confirms the defect. Planning-time arithmetic
(`bldg0176227` washer 28.19 L/d against 17.11 and 39.35 L/d) is recorded
as such, to be re-measured.

**B-17: wet-appliance hot water follows OpenStudio-HPXML** (numeric,
contingent on B-16). Daily volume by `calc_clothes_washer_energy_gpd` and
`calc_dishwasher_energy_gpd`, drawn through the `hot_water_*` columns,
untempered; the fixed per-cycle draw and its key deleted; found with it
and fixed in it: pre-Addendum-81 cycle equations, `AdjustedAnnualkWh`
adjusted twice, `UsageMultiplier` not scaling water, and the silent
constant-window fallback.

## Schedule placements (for the `00-schedule.md` owner)

All three are in the numeric block, after the consumer migration, after
B-14 and OD-20 (row 94i) and before CN-3 (row 95):

| Row | Entry | Title | File | Commit | Consumer check |
|---|---|---|---|---|---|
| 94j | B-15 | OpenStudio-HPXML unavailable periods applied | 09 | `fix: apply OpenStudio-HPXML unavailable periods to the end uses they name` | delta |
| 94k | B-16 | wet-appliance hot-water volume measured against OpenStudio-HPXML and OCHRE (triage) | 09 | none (recorded in `02-measurement.md`) | recorded |
| 94l | B-17 | wet-appliance hot water by the OpenStudio-HPXML daily volume; runs only when B-16 confirms the defect | 09 | `fix: draw wet-appliance hot water by the OpenStudio-HPXML daily volume` | delta (contingent) |

Other schedule changes:
- Row 16a (CN-17): commit message `fix: the contract check reads the
  engine tree every gate resolves, and the bundle is written where the
  gate names`; gated on the latest engine commit when its gate runs
  (`7b7a6a7d`, SL-7, row 20, not OR-11's `b8195c83`), census rows 1 to 20;
  the row 16g re-check adds `b8195c83`; CN-13's census is rows 16b to 16h
  and 21 to 26.
- Row 95 (CN-3): its drift range is rows 83 to 94l.

## Found while resolving

Fixed in these files:
1. SL-7 (`7b7a6a7d`) committed after OR-11 and before CN-17, so CN-17's
   "gated on OR-11's commit, before any further engine row" no longer
   holds. The 08 preamble, assignment, CN-17 and the B0 re-check now gate
   CN-17 on the latest engine commit, census rows 1 to 20, with OR-11's
   `b8195c83` added to the re-check worktrees.
2. `observer_capture.rs:147-151, 194-196` store W in fields named
   `electrical_load_kw` and `electrical_gen_kw`: B-14 commit 3 renames
   them to `_w`, test `observer_fields_hold_their_named_unit`.
3. The Python stubs disagree with the bindings (`estimate_pv_capacity`,
   `Battery`, `PV`, `EV`, `BmsMode.self_consumption`): B-14 rewrites each
   stub it touches to the binding in full.
4. The three Python signal surfaces name the same fields differently and
   `Signal.power_limit` has no ramp rate: B-14 commit 4 unifies them.

For other owners:
5. `11-other-defects.md`: `defaults/battery/default_parameters.csv` is
   read by no code (`defaults.rs:500, 764` load only `*.toml`); a dead
   data file, to delete or load.
6. `11-other-defects.md`: `load_vehicle_mapping_csv`
   (`defaults.rs:1653-1781`) skips a malformed row with a warning and
   returns `None` for a missing file, a silent default.
7. `11-other-defects.md`: `schedule_resolve.rs:1019, 1026, 1029`
   substitute 0.0 when a load's power cannot be determined
   (`determine_max_kw(...).unwrap_or(0.0)`, `determine_constant_kw(...).unwrap_or(0.0)`),
   a silent default.
8. `11-other-defects.md`: the Python `add_ev_with_driver` uses away-charge
   power 0.0 where the seed path uses 6.6 kW, and fuel economy
   `capacity / range` where the catalogue gives its own figure
   (`py_dwelling.rs:1322, 1349`); the L1 cap 1.8 kW is written twice
   (`py_dwelling.rs:1260-1263`, `py_equipment.rs:777-780`). Each is a
   behaviour difference for a numeric entry of its own, kept out of
   B-14's unit move.
9. `03-output-reporting.md` / `05-python-surface.md`: `Dwelling.add_actor_by_name`'s
   stub claims `params` has a default, which the PyO3 0.29 binding does
   not give it (`pyi:2438-2443`).

## Items for other owners

- `00-schedule.md`: the rows and changes in "Schedule placements".
- `02-measurement.md`: drop the `git show c71c542:<file>` redirection at
  `:586` (verify-L4-e 2); document the `resolved` product in M-1's
  format section (B-14 commit 1 adds it under M-1's schema change rule);
  log lines for B-15's census, B-16's measurement and B-14's `si_census`
  counts.
- `decision-log.md`: `dl-i08-process.readiness` (`status`,
  `executed_order`, `next`) still describes OR-11 as uncommitted and says
  rows 16a to 16h run before any further row, while `b8195c83` and
  `7b7a6a7d` (SL-7) have landed; the operator decides whether SL-7
  before CN-17 stands (08 now gates CN-17 on it).
- `plan.md` §2.7: the `REMOVED_KEYS` table and the surface list gain
  `estimate_pv_capacity` and `TariffEvaluator.step` (scope `PythonKeyword`,
  `net_power_kw` naming `net_power_w`), the `EvConfig.max_charging_kw`
  row, and the `ControlSignal` surfaces `ControlSignal.from_dict`,
  `DispatchRequest`, `Signal`, the Rust gym action reader and the protocol
  bridge; §2.1 or §2.2 may state that every renamed Python parameter is
  keyword-only.
- `12-inputs.md` IN-9, `11-other-defects.md` OD-2 site 6 and OD-10:
  from row 94c `[golden]` compares the `resolved` product, which M-1's
  capture re-captures; each records its resolved delta (none expected for
  the OD rows).
- `12-inputs.md` IN-3: `HPXML_ELEMENTS` gains the `UnavailablePeriod`
  elements in B-15's commit.
- `11-other-defects.md`: items 5 to 8 above.
