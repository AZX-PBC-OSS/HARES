# Review: OR-18 fixes on branch fix-or18 (9d599117, f1caa8da, 7aaaf53e on 1c18543a)

Verdict: CHANGES-REQUIRED

Counts: 0 blockers, 0 majors, 2 mediums, 5 lows, 3 infos. Every red-team and audit
finding of `1c18543a` is fixed in code and pinned by a test that fails when its
hunk is reverted, except the two mediums below (hunks that no test pins). The
physics, the zone and design questions all hold. Nothing here is a numeric or
correctness defect in shipped behaviour.

## How this was checked

- Worktree `.workstrees/review-or18` at 7aaaf53e, target dir
  `target-review-or18`. The full verification block, physics suites and Python
  were run on it; the worktree stayed clean (`git status` empty).
- The permission layer refused an in-place `Edit` of a worktree source file
  (it was to drop the unused `BoundaryDiagnosticInfo` import). I did not work
  around that in the worktree. Revert experiments, the sweep and the
  clippy-with-import-fixed run were done on `git archive` copies of 7aaaf53e
  and 1c18543a under the session scratchpad, with their own target dirs.
- The session scratchpad `logs/` directory is shared with another agent
  (verify-or19) using the same file names; its runs overwrote several of my
  first-pass logs (the paths inside `test-*.log` and `clippy-*.log` named
  `verify-or19`). I discarded those and re-ran fmt, all clippy lines, all
  nextest lines, bench and the three goldens into a private directory
  (`scratchpad/rev18-private/logs`); the figures below come from that re-run.
  Physics suites, ASHRAE gate and pytest logs were verified to name only
  `review-or18` (pytest from the worktree's own `target/pytest.log`).
- Upstream fetched: OS-HPXML v1.12.0 `waterheater.rb`, `geometry.rb`,
  `defaults.rb`, `hpxml.rb`; EnergyPlus v25.1.0 `WaterThermalTanks.cc`.

## Verification block results

| Line | Result |
|---|---|
| fmt | pass |
| clippy (all-features), default, dst, profiling, observe, observe+profiling | pass |
| clippy release, release -F dst | fail, only `unused import BoundaryDiagnosticInfo` at `hares-envelope/src/thermal_solver/mod.rs:1858` (one error each). With that import removed on the scratch copy both pass. |
| clippy release -F observe | fails on the same import AND, hidden behind it, on two more errors in hares-core (finding L5 below) |
| nextest default / observe / dst / profiling | 7192 / 7251 / 7205 / 7198 passed, 0 failed, 0 skipped |
| nextest hares-core observe+profiling | 1363 passed |
| nextest release | 7149 passed |
| bench --no-run | pass |
| goldens default (5), dst (1), profiling (5) | all identical |
| ASHRAE gate | PASS |
| bestest, parity, conditioned/freefloat/structural/envelope oracle (hares-core) | 25 passed, 0 skipped |
| physics_validation_tests, solar_parity | 53 passed |
| maturin develop --release, pytest -m "" | 1378 passed, 9 skipped; skip census matches exactly |
| writing check on 9d599117, f1caa8da, 7aaaf53e (message and added lines) | clean (no em dash, no ID, no banned token) |

## Findings

### Medium

**M1. The water heater default-location hierarchy order is unpinned.**
`crates/hares-io/src/hpxml/resolve_water_heater.rs:116-145`
(`default_water_heater_location`). Reverting the IECC 4-8 hierarchy to
"basement - conditioned" before "basement - unconditioned" (the opposite of
`defaults.rb:6104-6132`) passes every test, including the new
`unlocated_water_heater_takes_the_os_hpxml_default_location`
(`hpxml_parsing_tests.rs`), because each of its fixtures has only one basement
kind (no vendored OS-HPXML sample has both). The no-IECC order (conditioned
basement first) is likewise only tested on a building with a single basement
kind. Failure scenario: a later edit swaps the two entries; a home with both
basement kinds and no `<Location>` moves its water heater between zones with no
test failing. Fix: add a case built from a sample with both basement surfaces
(edit the XML) for IECC 5B and for no IECC, asserting each upstream order. (The
batch of other reverts in the same function, the unknown-IECC error, the
warning and the surface list, did fail
`unlocated_water_heater_takes_the_os_hpxml_default_location` and
`unlocated_water_heater_rejects_an_unknown_iecc_zone`.)

**M2. The dwelling-level served-zone capacitance fallback is unpinned.**
`crates/hares-core/src/dwelling/mod.rs:2588-2594`: the capacitance lookup now
uses `merged_cfg.zone_id().or_else(|| zone_map.get(ZoneRole::Indoor))` so an
HVAC unit with no `zone_id` still receives the conditioned zone's capacitance.
Removing the `.or_else` passes the whole hares-core, hares-io and
hares-equipment suites. The cooler test
(`heat_pump_coolers_pass_zone_capacitance_to_the_inner_air_conditioner`) feeds
the capacitance in directly and so does not cover the dwelling hunk. Failure
scenario: a Blueprint/Python HVAC unit without `zone_id` silently reverts to
`zone_capacitance_kwh_per_k = 0` in the equivalent-battery model. Fix: a
dwelling test that builds an HVAC spec with no `zone_id` and asserts the unit's
`zone_capacitance_kwh_per_k` equals the conditioned zone's.

### Low

**L1. Invented-zone/temperature defaults are not all gone.** The `ZoneId(1)`
fallbacks and invented zones are gone from logic (grep of every non-test
region: the only literal left is the `actor::testing::TestEnvBuilder` helper,
`hares-core/src/actor.rs:305`, which is a test-support builder). Three dead or
adjacent defaults remain: `crates/hares-equipment/src/hvac/heat_pump/constants.rs:3`
`pub const DEFAULT_ZONE_ID: u16 = 1;` (referenced nowhere);
`crates/hares-types/src/environment.rs:132-141` `impl Default for ZoneState`
(`ZoneId(0)`, 20.0 C, 0.008, 200 m3; used nowhere in the workspace);
`crates/hares-io/src/hpxml_schedule.rs:69-80` heating/cooling setpoint
`unwrap_or(20.0)` style defaults in the schedule generator for HPXML without a
schedule CSV (pre-existing, not introduced here). Delete the first two; make
the third an error or record why it is a model input rather than a missing
value.

**L2. IECC zone is not derived from the weather station when HPXML has none.**
`defaults.rb:970-980` (`apply_climate_and_risk_zones`) fills
`ClimateZoneIECC` from the weather file's WMO number before
`get_water_heater_location` runs; HARES reads only the HPXML element
(`resolve_water_heater.rs:121`) and treats absence as "nil", which selects the
third hierarchy. A file with no `ClimateZoneIECC` in a hot climate therefore
defaults to a basement where OS-HPXML picks garage or conditioned space. Rare
(ResStock and the OS-HPXML samples carry the element), but the doc comment
says the default is "implemented exactly". State the deviation in the function
doc or implement the WMO lookup.

**L3. Python contract is documented but not enforced in types or tests.**
`docs/python.md:136-149` documents that water heaters need `zone_id`, and the
Python tests were updated to pass it, but `python/ochre_next/_hares.pyi:1644-1743`
still types `zone_id` as optional (`int | None = ...`) for `GasWaterHeater`,
`ElectricResistanceWH` and `HeatPumpWH`, and no Python test asserts that omitting it raises. Add one
`pytest.raises` case per class family and a docstring line in the stubs.

**L4. Bookkeeping in the gitignored ledger (not code).**
`ledger/03-output-reporting.md:1634` still says the golden's ResStock homes
"bldg0000003, bldg0000004 and bldg0000006 DO carry" other-locations; only
bldg0000004 is in a golden (the audit's m2; the text at `:988-990` and the
schedule row are right). `ledger/02-measurement.md:955-970` still describes
`ambient_other_space_c.conditioned_zone_c`, which no longer exists, and has no
entry for the OR-18b physics (wet-bulb COP delta, HPWH wall-heat, ambient
placements) or the per-step cost measurement the audit's m3 asked for. The
OR-18b ledger entry promises "the measurement log's delta note gains the
corrected numbers". Fix in place (plan rule: fix existing files).

**L5. `[clippy-release-observe]` cannot go green when the import is fixed.**
Not this branch's defect, but it contradicts the stated expectation that the
import is the only release failure. With `BoundaryDiagnosticInfo` removed,
`cargo clippy --workspace --all-targets --release -F observe -- -D warnings`
still fails in hares-core: `crates/hares-core/src/dwelling/solver_builder.rs:512`
E0425 `r_total` (the binding at `:439-440` is `#[cfg(debug_assertions)]` but
the use at `:512` is `#[cfg(feature = "observe")]`), and `dwelling/mod.rs:108`
unused import `MoistureZoneInvariant` under release+observe. Both lines are
byte-identical on main `c6889589`, so they pre-date this branch, and
downstream crates were not reached after them. The owner of the import fix
needs to fix these too, and re-run to see what is behind them.

### Info

**I1. Humidity blend (physics): sound, deviation recorded in code.**
`hares-core/src/ambient_air.rs` blends humidity ratio with the dry-bulb weights
and holds it through the floor. That conserves vapour (ASHRAE Fundamentals ch.1
adiabatic mixing; sensible heating to the floor leaves W unchanged).
OS-HPXML (`waterheater.rb` `apply_hpwh_loc_temp_rh_sensors` and the program
that sets `rhamb = sum(RH_i/100)/n`) averages RH and applies it at the blended,
floored dry-bulb, which is physically inconsistent: e.g. indoor 22 C/40 %
(W 0.0066) with outdoor 0 C/80 % (W 0.0030) at the 20 C floor gives OS-HPXML
60 % RH (wet-bulb about 15.0 C) against HARES 33 % (about 12.2 C): up to roughly
3 K lower evaporator wet-bulb in winter, a few percent of COP. The deviation is
the right one under the ASHRAE/E+ rule, but its measured effect on an
ambient-placed HPWH is not recorded anywhere (see L4). Pressure is right:
the wet-bulb uses `weather.pressure_pa()`, the same pressure as the zone and
outdoor wet-bulbs, and E+ (`WaterThermalTanks.cc:6509,11819`) uses
`OutBaroPress` for the HPWH inlet. Over-saturated blends return the dry-bulb
(`psychrometrics.rs:134-139`). Floors are exact: 20.0 C (68 F), 10.0 C (50 F),
40/9 C (40 F); the test asserts `40.0/9.0`. The "other heated space" floor is
still the constant 20.0 C rather than the dynamic heating setpoint
(`geometry.rb:1595-1614`); that is OR-18a and out of scope here.

**I2. Multiple-conditioned-zone rejection cannot reject any real input.**
`build_zone_map` (`hares-io/src/hpxml/building.rs:2294`) inserts the key
"conditioned" once, and `build_synthetic_building`
(`dwelling/synthetic.rs:1499`) builds one zone, so HPXML (single and
multi-family, one `<Building>` at a time), ResStock and synthetic TOML always
have exactly one. The sweep below confirms no input changed outcome. Only a
hand-built `Building` can trip it.

**I3. Heat-pump wall-heat share, scheduled loads, ThermalSolverConfig.**
The HPWH wall share 0.5 only in the living zone and 0.0 elsewhere matches the
sources: OCHRE `WaterHeater.py:483-494` defaults 0.5 but needs an "Interior
Wall" surface, which `hpxml.py:758` builds only in Indoor (so any other zone
raises there); OS-HPXML adds all HPWH gains convectively (`frac_radiant 0`,
`waterheater.rb` `apply_hpwh_zone_heat_gain_program`). Python HPWH always
passes `wall_heat_fraction: None` (`py_water_heater.rs:479`) and previously
resolved to 0.0 as well, so no Python behaviour changed. `ThermalSolverConfig`
has no `Default`; `new(indoor_zone_id)` is the only constructor; the indoor
zone is the conditioned zone (not `zones.first()`); `NoZones` is a typed
error; non-test greps for `zones.first()`, `unwrap_or(ZoneId`, `21.0` and
`unwrap_or(2x.0)` in the solver builder, HVAC and ventilation are empty.

## Findings from the first red team and audit: status

| Finding | Fixed | Pinned (reverted in the scratch copy, fails) |
|---|---|---|
| RT M1 wet-bulb | yes | `hpwh_other_housing_unit_runs_on_the_conditioned_zone_air`, `hpwh_outside_runs_on_the_outdoor_wet_bulb`, `..._buffer_space_location`, `ambient_air::tests::{wet_bulb_comes_from_the_blended_humidity_ratio, single_source_placements_carry_their_source_moisture}`, `environment::tests::ambient_air_follows_the_scheduled_space_table_for_occupied_placements` |
| RT M2 / audit M1 zone fallback | yes (one `Siting` and one `resolve_served_zone` path) | `water_heater_without_zone_or_location_fails_init`, `hvac_and_water_heaters_without_a_zone_fail_init` (17 classes), per-type `*_errors_without_a_zone` tests (baseboard, dehumidifier, ideal HVAC, ventilation, resistance), `zone_id_0_equipment_fails_init_naming_the_unit` |
| RT M3 add after stepping | yes (no thermal port for ambient placements) | `ambient_water_heater_joins_after_stepping_has_begun`, `ambient_placed_*_contributes_nothing_to_any_zone` for gas, resistance, indirect, HPWH |
| RT m1 no conditioned zone | yes (typed errors) | `ambient_placements_needing_the_conditioned_zone_are_rejected_without_one` |
| RT m2 NaN masked by floor | yes | `update_rejects_a_non_finite_outdoor_dry_bulb`, `ambient_air_rejects_a_non_finite_conditioned_zone` |
| RT m3 closure check | yes | `synthetic_closure_counts_slab_as_floor_and_foundation_wall_as_wall`, `..._counts_only_the_zones_own_boundaries`, `..._rejects_a_zero_area_face` |
| RT m4 / audit M2 HPWH wall share | undone: default 0.0, explicit 0.5 only for HPXML living zone | `hpwh_wall_heat_default_is_zero_without_an_explicit_value`, `parse_hpwh_wall_heat_fraction_is_explicit_per_location` |
| RT m5 multiple conditioned zones | yes | `manager_rejects_more_than_one_conditioned_zone`, `conditioned_zone_id_errors_when_two_conditioned_zones_exist` |
| audit M3 four commits as one | accepted deviation (recorded); not remediable | n/a |
| audit M4 public doc to ignored tree | yes (section removed, `docs/python.md` now documents the contract) | n/a |
| audit m1 release clippy | not fixed, owned elsewhere; see L5 | n/a |
| audit m2 ledger premise | half fixed (L4) | n/a |
| audit m3 measurement | not recorded (L4) | n/a |
| audit m4 DRY | yes: `Siting`, `served_zone_ports`, `resolve_served_zone`; forwarders and the tankless stored field removed | n/a |
| audit m5 silent substitutions | yes | covered above |
| audit m6 end use lag | yes | `end_use_reports_cooling_before_the_first_cooling_step` |
| audit m7 skin loss telemetry | description reworded; contributes-nothing tests added for four heaters | listed above |
| audit m8 dehumidifier ports | yes (`served_zone_ports`) | `dehumidifier_init_errors_on_unresolved_zone` |
| audit m9 golden provenance | yes (`f1caa8da`: git_head 9d599117, clean) | goldens compare identical |
| New: scheduled-load zone-role rule | yes | `heat_giving_load_errors_when_its_zone_role_is_missing`, `heat_giving_load_errors_without_a_zone_or_zone_map` |
| New: OS-HPXML default WH location | yes, except M1/L2 | `unlocated_water_heater_*` (order unpinned, M1) |
| New: IdealHvac, cooler capacitance | yes | `end_use_reports_cooling...`, `heat_pump_coolers_pass_zone_capacitance_to_the_inner_air_conditioner` |

OS-HPXML default location, checked against `defaults.rb:6104-6132` and
`hpxml.rb:1760,1790-1797`: hierarchies match entry for entry (1-3: garage then
conditioned space; 4-8: unconditioned basement, conditioned basement,
conditioned space; no IECC: conditioned basement, unconditioned basement,
conditioned space); `has_location` searches roofs, rim joists, walls,
foundation walls, floors and slabs (HARES adds the legacy `FrameFloor`);
an IECC zone outside the table is an error in both. The "conditioned space
exists if a conditioned zone exists" fallback is sound (every HARES building
has the zone, and it is the last entry of every hierarchy). The warning names
the unit and its element id.

## Sweep: new construction or step errors versus 1c18543a

A scratch harness (not committed, run on archive copies) built
`Dwelling::from_config` and stepped 24 x 900 s for all 425 OS-HPXML sample
files in `vendors/OCHRE/test/OS-HPXML Sample Files` (schedule generated from
HPXML) and all 31 ResStock fixture homes (2024.2 and 2025.1, their own
schedules), at 1c18543a and at 7aaaf53e. Result: 400 OK and 56 ERR at both
commits, and the two outputs are byte-identical, including error text. All 56
errors are old HVAC, schema and domain-validation gaps (PTHP and
room-AC-reverse-cycle types, WSHP/GSHP init parameters, Generator and Battery
required fields and so on); none mentions a zone, water heater, location or
scheduled load. So the scheduled-load rule, the conditioned-zone resolution,
the unlocated-water-heater default and the single-conditioned-zone rule reject
no input that built before. Limits: single weather file, 6 simulated hours,
HVAC-failing samples are not reached past construction.

## Residual risk

- The sweep cannot show a numeric change on the 400 passing inputs; goldens and
  parity cover only the committed fixtures (all identical).
- No measurement of the HPWH RH-vs-humidity-ratio deviation exists (I1, L4).
- The first-pass logs in the shared scratchpad were overwritten by another
  agent; everything reported here comes from the private re-run.
