# Review and red team: OD-30 round 2, `528ebc6c..8ded384b` (branch `fix-od30`)

Commits: `80f6315e`, `0a73dd8d`, `8bfc451c`, `8a632640`, `9b1e631f`, `1da9d150`,
`f28e1b9d`, `8ded384b`. The reviewer did not write any of it. Worktree
`.workstrees/fix-od30` at `8ded384b`; `CARGO_TARGET_DIR` outside the tree,
deleted afterwards. Mutations ran in an rsync copy (`target/mut`) with its own
target dir; nothing on the branch was modified. Reference sources fetched at
OS-HPXML v1.12.0 and EnergyPlus v24.2.0 (raw files from the release tags), and
read from `vendors/OCHRE` at its pin.

## Verdict: CHANGES-REQUIRED

No high finding. All nine findings of the previous review are fixed at their
root, with the evidence below. There are 2 medium findings and 8 low ones. The
mediums are not regressions of this round. Both are silent paths that the root
fix ("equipment init failure fails the build") left open, one of them inside the
very override channel that H1 was about. Neither blocks the numbers: goldens are
bitwise identical, parity and BESTEST pass, there is no slowdown over noise.

## Block results (at `8ded384b`)

| Line | Result |
|---|---|
| `cargo fmt --all --check` | clean (excluding my own scratch probe, removed) |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | clean |
| `cargo test` hares-types, -physics, -envelope, -equipment, -io, -control, -tariff, -fleet (default features) | 5732 passed, 0 failed, 3 ignored |
| `cargo test -p hares-core -F observe` (lib, BESTEST, parity, conditioned/freefloat/structural/envelope oracles, orchestration, doc tests) | 1387 passed, 0 failed, 6 ignored |
| `cargo test -p hares-core -F profiling --test profiling_alloc` | 2 passed |
| `frame_golden compare-all` release, default and `-F dst` | all identical (six goldens, and `full_year_900s_dst`) |
| Python `tests/python` (built `ochre_next` wheel release, cp313, all `ochre` and dev groups, `-m ""`, `-n 8`) | 1305 passed, 26 skipped, 3 xfailed; 1 failure and 1 collection error that are the environment, not the branch (`test_download_resstock_fixtures` shells out to a script that needs the `resstock` extra; `test_helics_integration` needs the `helics` extra) |
| OCHRE parity + thermal trace + observer (`-m ""`, `-rxXs`) | 11 passed, 3 xfailed (the stated trace xfails), 5 skipped (observer visualisation tools). No "HARES has no column matching" skip remains |
| `ruff check` on the touched `.py` | 3 F541 findings, all pre-existing lines, none in changed code |

No golden file changes in this range (`git diff 528ebc6c..8ded384b -- tests/fixtures/golden`
is empty), which is correct: nothing numeric moved. Both modes reproduce them bitwise.

## 1. The nine previous findings

| # | Finding | Status | Evidence |
|---|---|---|---|
| H1 | an override of a load's sensible fraction drops it or skews the split | fixed at the root | see 1.1 |
| M1 | observer, diagnostics and Python export miss radiant and short-wave heat | fixed | `ThermalAccumulator::heat()` returns a `ZoneHeat` (convective, radiant, short-wave, latent). `diff_ports`, `capture_ports`, `diagnostics::capture` (`thermal_gains_w` is `heat().sensible_w()`), `EnvelopeDiag.port_shortwave_w` and CSV column, and the Python `thermal` 5-tuple plus `port_shortwave_w` all read it. Pinned by `observer_sees_radiant_and_shortwave_heat` and the extended diagnostics test. Probe 3 below confirms the observer's per-equipment contributions plus occupancy equal the port totals, channel by channel, on every step |
| M2 | OCHRE suites leave temp files | fixed for the two named modules | both write under `tmp_path_factory`; the other `mkdtemp` users are still there (finding L7) |
| L1 | debug breakdown and stale docs for short-wave | fixed | `ZoneSensibleBreakdown` has `shortwave_to_air_w` and `shortwave_to_surfaces_w`; the closure test includes them. Mutation: sending short-wave to the air node fails `zone_sensible_breakdown_debug_must_include_radiant_air_residual` and `shortwave_gains_are_absorbed_like_transmitted_diffuse_solar` |
| L2 | three unpinned behaviours | fixed | mutation: removing `apply_port_shortwave_inputs` from `build_input_vector` fails `production_step_injects_shortwave_gain`; `rollback_discards_all_of_the_zone_heat` and the `internal_gain_w == 400` assertion cover the other two (the rollback test pins the extracted sum helper, not a full rollback) |
| L3 | schedule row as an f64 in a reserved custom domain | fixed | typed `EnvironmentState::schedule_row: Option<usize>`, set once in `fill_step_state`; `SCHEDULE_ROW_DOMAIN_ID`, the swap buffer and `schedule_row()` are gone. See section 6 |
| L4 | `eq_ignore_ascii_case("true")` on `xs:boolean` elements | fixed | see section 5 |
| L5 | import order and blank lines | fixed | conftest passes ruff; the triple blank line is gone |
| L6 | OCHRE comparison compares no lighting | fixed | `write_output=True` brings the per-equipment columns; all nine `PARITY_CHECKS` columns compare (Indoor and Exterior Lighting, MELs, TV, Refrigerator, Ventilation Fan, HVAC heating, cooling, total) |

### 1.1 H1, every override combination (cz2a parity building, one day, each load stepped alone)

Share of input to conditioned zone, convective / radiant / short-wave / latent:

| Override | Range | Lighting |
|---|---|---|
| none | .288 / .432 / 0 / .08 | .2 / .6 / .2 / 0 |
| sensible 0.3 | .12 / .18 / 0 / .08 (60 % of sensible) | |
| sensible 0.9 | .36 / .54 / 0 / .08 (60 %) | |
| sensible 0.5 | | .1 / .3 / .1 / 0 (60 %, 20 %) |
| `radiative_gain_fraction` 0.2 (absolute) | .52 / .2 / 0 / .08 | |
| sensible 0.3 + absolute radiant 0.2 | .1 / .2 / 0 / .08 | |
| `radiant_share_of_sensible` 0.9 | .072 / .648 / 0 / .08 | |
| visible share 0 | | .4 / .6 / 0 / 0 |
| visible share .5, radiant share .5 | | 0 / .5 / .5 / 0 |
| TV sensible 0.5 | (TV) .2 / .3 / 0 / 0 | |

Invalid inputs fail the build, naming the equipment and the cause: sensible 0.95
with latent 0.08, sensible 0.3 with absolute radiant 0.5, latent 0.5, light radiant
share .9 with visible .2, light absolute radiant .45 with sensible .5, light latent
.1, share -0.1, `zone_id` 99. The wildcard `all` with a load-only key fails at the
first typed config (Gas Furnace, unknown field), loudly. The split is right in
every accepted combination and nothing is dropped. The silent cases are finding M1.

## Findings

### Medium

**M1. Overrides on raw-parameter loads are accepted without checking key or value type, so valid-looking overrides silently do nothing.**
`crates/hares-equipment/src/gain_fractions.rs:56-75` reads each key with `get_f64`, which returns `None` for a wrong type
and for an absent key; `crates/hares-core/src/dwelling/conversions.rs:911-946` merges any key in with `nested_update`;
`crates/hares-io/src/hpxml/equipment.rs:120-128` injects `sensible_gain_fraction` and `latent_gain_fraction`
defaults that win over the HPXML spellings.
Failure scenario (all observed on the cz2a fixture, one day, probe 1):
- `{"Cooking Range": {"frac_sensible": 0.3}}` builds and leaves the range at .288 / .432 / .08: the default
  `sensible_gain_fraction` outranks `frac_sensible` (`from_config` reads it first). The same for `frac_latent` 0.2 (latent stays .08), and for
  `convective_gain_fraction` alone.
- `{"Cooking Range": {"sensibel_gain_fraction": 0.1}}` (typo) builds and does nothing.
- `{"Indoor Lighting": {"visible_gain_fraction": 0.1}}` (the key this round renamed away) builds and does nothing.
- `{"Cooking Range": {"radiant_share_of_sensible": "0.9"}}` or `null` builds with radiant share 0: the range becomes 100 % convective,
  silently, because `unwrap_or(0.0)` at line 56 turns a wrong-typed share into "none". The same override on
  `sensible_gain_fraction` fails loudly ("missing"), so the channel is inconsistent.
This is the same class as H1 (an accepted override that does not do what it says) and the repo rule
"never silently substitute a fallback for invalid input". The typed configs already reject unknown fields; the raw loads do not.
Fix: validate override keys for raw loads against the set the load reads (error on an unknown key, naming the nearest known one);
read each gain key with a typed reader that errors on a present value of the wrong type; resolve the HPXML spelling and the
canonical key to one value before defaults are injected (an override of either replaces both). Pin with the cases above.
Confidence: high (reproduced). New since the previous round: the share keys' wrong-type case; pre-existing: the rest.

**M2. The root fix leaves other silent skips and defaults in dwelling assembly (outside the `hares-io` resolver sites of OD-48, the defaults file of OD-49, roughness OD-53, zone volume OD-31).**
Each of these warns or logs and continues with a different building:
1. `crates/hares-core/src/dwelling/conversions.rs:484` (`find_zone_idx`): a boundary with no interior zone type goes to zone index 0.
   `:451` (`zone_label`): an unrecognised zone type becomes Outdoor. `:564` and `:575` (`resolve_exterior`): an unrecognised or absent exterior zone becomes Outdoor.
   Reachable from input: `parse_zone_label` (`crates/hares-io/src/hpxml/building.rs:3150-3166`) maps any `AdjacentTo` text that
   is not recognised (a misspelling, `exterior wall`) to `ZoneType::Other`, so the surface is silently a wall to outdoors.
2. `crates/hares-core/src/dwelling/mod.rs:2432-2433` and `:2452-2453`: the two Occupancy invariants (negative occupant count; an
   Occupancy spec with no schedule column) are `#[cfg(any(test, debug_assertions))]`. In a release build neither is checked, and
   `apply_occupancy_gains` (`:5028`, `:5050`) then returns `Ok` with no heat, which the comment above the check calls
   "silently zeroing it produces a systematic underestimate". Debug and release builds differ in whether a defective input builds.
   Not reproduced end to end (needs a schedule with no occupancy column and no defaults); by reading, confidence medium.
3. `crates/hares-core/src/dwelling/autosize.rs:639` (unreasonable mains temperature becomes 25 degC), `:671` (missing bedroom count
   becomes 0 "conservative defaults"), `:818-838` (a water heater whose autosized capacity is zero gets a default element capacity and tank
   instead of an error), and the cooling gains with no floor area (`:225`).
4. `crates/hares-core/src/dwelling/mod.rs:2784-2792`, `:4434`: a `MetricsCalculator` init failure zeroes the run's metrics with a
   warning; `:4444` a recorder re-init failure drops all later output with a warning.
5. `crates/hares-core/src/dwelling/loop_allocator.rs:32-43` (`.ok()?`): a typed config that cannot be read yields "no loop id",
   so the allocator can hand its loop to someone else. It is masked today because the same config then fails `init`, which now
   fails the build, but the mask is accidental.
6. Run time, not assembly, but the same class: `mod.rs:5613, 5923, 5974, 6051` discard `update_control` errors with `let _ =`, and a
   failing `step` (for example an event replay whose row is past the series) is rolled back each step, so the load silently
   contributes nothing for the whole run; it is counted in `health.port_rollbacks` and warned, and the dwelling still returns `Ok`.
7. The auto-created specs of `ensure_specs_for_csv_columns` (`crates/hares-io/src/schedule_resolve.rs:1397-1445`): a CSV column for
   equipment the HPXML lacks now creates nothing and logs nothing (before: a zero-power spec that `init` then dropped). The new rule is
   defensible, but it is a silent skip, not an error or a warning.
Fix: each becomes a typed error naming the element or field, or a documented OS-HPXML/ASHRAE default applied with a warning that names
the value (item 1: unknown zone text is an error; item 2: drop the `cfg`; items 3, 4: error, or file as defaults with citations).
File as one ledger entry beside OD-48. Confidence: high for items 1 to 5 and 7 (by reading), medium for 6.

### Low

**L1. Comments that describe the removed skip-and-warn behaviour remain.**
`crates/hares-core/src/dwelling/mod.rs:2812-2815` ("a non-critical init failure drops specs between the two"),
`crates/hares-core/src/dwelling/conversions.rs:765-767` ("init rejections for non-critical loads are deliberately skip-and-warn,
so this boundary is where the typo must fail the build"), `crates/hares-io/src/hpxml/equipment.rs:419` ("dropped non-critical equipment
leaves gaps"), `crates/hares-core/tests/equipment_identity_boundaries.rs:475-477`, `crates/hares-core/tests/equipment_id_regressions.rs:527`,
`crates/hares-core/tests/equipment_identity_misattribution.rs:401`. They now state something false. The `end_use_aggregate_indices`
positional-map comment at `mod.rs:2810-2817` is the one most likely to mislead. Fix: reword or delete.

**L2. DIVERGENCES.md is accurate but not complete, and arrived late.**
Every number in D-005 to D-007 reproduces (mutations below), every file:line citation checks against the tags (see section 7), and every
pin fails when reverted. Gaps: (a) D-007 says HARES spreads all visible light over the interior surfaces and cites EnergyPlus as doing the
same; EnergyPlus also counts the windows' diffuse transmittance and absorptance in `SUM1`, so part of the visible light leaves through the glazing.
That is a place HARES goes beyond EnergyPlus's behaviour (it is queued as OD-44), and the register's own rule (`ochre_deviation_triage`:
"every place HARES goes beyond OS-HPXML or EnergyPlus gets an entry") needs it stated in D-007, with OD-44 named in prose or the fold
note. (b) D-005 says all appliance heat is 0.6 radiant; the ceiling fan is 0.558 (`hvac.rb`, `resolve_loads.rs:1244`), unlisted.
(c) The rule says the entry lands "in the same commit as the change"; the entries are in the last of 19 commits. If the branch folds as a
sequence, the earlier commits carry unrecorded class (a) deviations; squash or note it at fold.

**L3. The tightened ratchets overstate what they catch, and one note is attached to the wrong line.**
`1da9d150` says each tightened band is "the new residual plus the suite's ~1% margin, so any return of the old drift fails". Measured
values at head are in `target/parity.log` (this review): every tightened band is the new value plus 0.0086 to 0.0168 °C, rounded up to 0.01,
the same rule as the pre-existing bands, and nothing is loosened (every changed line decreases). But with all heat made convective
(mutation M-A below) seven of the nine zone-temperature bands fail and two do not: `cz2a_gas_furnace_ac_res_wh` (0.6895 against 0.69) and
`cz4a_ashp_hpwh` (0.8880 against 0.90). `cz5a_minisplit_gas_wh` zone temperature is 1.7433 against 1.75, headroom 0.0067, below the
margin that the others carry. The cause note for the ResStock fixture (`tests/parity/mod.rs:127-136`) describes zone-temperature,
HVAC-energy and site-energy residuals but is attached to the `METRIC_PEAK_HVAC_POWER` override at `:138`, a metric it does not mention;
the three metrics it describes sit on default bands. Fix: say in the commit text or the file which bands catch a revert; move the note to a
comment block under the fixture's heading, not to a single override line.

**L4. The default (StarMesh) path of the short-wave deposit has no pin of its own.**
`apply_port_shortwave_inputs` calls `distribute_transmitted_solar`, which has two branches, `interior_lwr_zones` (ScriptF) and
`interior_solar_zones` (StarMesh, the default: `thermal_solver/config.rs:774`). `shortwave_gains_are_absorbed_like_transmitted_diffuse_solar`
and `production_step_injects_shortwave_gain` both build their solver on `interior_lwr_zones`. A break that affects only the StarMesh
deposit for short-wave would pass every unit test and be caught only by goldens. Add the same two checks on an
`interior_solar_zones` solver.

**L5. Two structs for the same four numbers.**
`hares_equipment::gain_fractions::ZoneGain` (`gain_fractions.rs:30`) and `hares_types::ZoneHeat` (`ports.rs`) both hold convective,
radiant, short-wave and latent watts. `GainFractions::of` and `accumulate_zone_gain` could use `ZoneHeat`; `EnvelopeComponentGains`,
`EnvelopeDiag` and the Python dict still spell the four channels as separate fields. Not a bug; one definition would stop the next
channel being added in three places and missed in one (that is how M1 of the previous round arose).

**L6. Documentation and tool lag behind the new channel.**
`docs/invariants-and-observability.md:223-226` and `docs/thermal-envelope-solver.md:430-433` list `port_convective_w` and
`internal_gain_w` but not `port_radiant_w` or `port_shortwave_w`; `docs/equipment/loads.md:33-34` ("Heat distribution") gives
sensible and latent only; `tests/python/parity_diagnostics.py:117-127` (`ENVELOPE_GAIN_KEYS`) omits `port_shortwave_w`, so that tool's
gain decomposition does not close with lights on. The Python observer's `thermal` entries changed from 3-tuples to 5-tuples
(`py_dwelling.rs:2698-2710`); `_hares.pyi` types the dict as `Any`, so nothing documents it. Greenfield, but the doc line should say it.

**L7. The remaining `mkdtemp` users leak, and the previous review's ledger request was not done.**
Besides the two modules fixed, `test_observer_diagnostics.py:70` (a file this branch edited), `test_py_metrics.py:21`,
`test_ev_tariff_integration.py:17`, `test_gym_env.py:35`, `test_py_fleet.py:90` and `test_rl_warning_surfacing.py:120` still write to
`tempfile.mkdtemp()` directories nobody removes. The `[stray]` rule fails on them. Nothing in `ledger/` records them. File an entry or fix them.

**L8. Two of the four "fails the build" sites of the root fix have no test, and the absolute-fraction override has none.**
`mod.rs:2297` (autosize duct DSE parameters now an error) and `schedule_resolve.rs:517-523` (a failed `lighting_basement` copy now an
error) have no test that reaches them from a dwelling (grep for the messages finds only the sources); `assembly_errors.rs` covers only the
ventilation config. `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares` covers sensible overrides, not
`radiative_gain_fraction` or the share keys. I did not mutation-test the two sites. Add one test each.

### Info

- `fill_step_state` now computes the schedule row even when the schedule is empty; `EnvironmentManager::new` rejects an empty schedule
  (`EmptySchedule`, `environment.rs:214`), so the removed guard cannot divide by zero. Fine.
- `compute_schedule_idx` (`environment.rs:433-476`) duplicates its leap-day logic in two branches and in the test-only
  `compute_schedule_offset`. Pre-existing.
- `test_thermal_trace.py` still warns and skips the `HVAC Heating/Cooling Electric Power` columns when HARES lacks them. Pre-existing.
- The ledger row in `plan-critique.md` cites `ca3e35d2` for the cause notes; the commit in range is `f28e1b9d`. Fix the reference at fold.

## 2. The root fix: every remaining skip path

Done in M2 (list, file:line). The equipment path itself is now clean: `build_from_blueprint_inner` (`mod.rs:2565-2625`) propagates every
`create_equipment_from_spec`, identity and `init` error. The only `continue` is `HANDLED_OUTSIDE_REGISTRY = ["Occupancy"]`, which is
explicit. A probe that builds the cz2a dwelling with an impossible sensible, latent or radiant fraction, a bad `zone_id` or a typed-config
type error fails the build in each case with the equipment named.

## 3. Energy conservation, probe over a day (ResStock homes, observe build)

Probe: `bldg0000007` (July, Hawaii), `bldg0176227` (January, gas dryer, garage), `bldg0101439` (April), 900 s, 96 steps, observer on. Per step it
sums every equipment's `ZoneHeat` contribution over both equipment phases, adds nothing, and compares with the final port totals of each
zone and with the thermal solver's `port_convective_w`, `port_radiant_w` and `port_shortwave_w` for the indoor zone.

- Port totals minus equipment contributions: exactly the occupancy gain. For `bldg0000007` the difference over the day is 945.0 Wh
  convective, 405.0 Wh radiant and 990.0 Wh latent, which is 18 person-hours at 75 W (0.7 / 0.3) and 55 W latent. No channel has
  a residual beyond that; no watt is lost or counted twice between equipment and ports. Same for the other two homes.
- Ports minus solver gains (indoor zone, all three channels): worst |difference| 0.0 W on every step.
- Per-equipment day split (share of input): range .288 / .432 / 0 / .08, lighting .2 / .6 / .2 / 0 (interior and garage lights), TV .4 / .6,
  refrigerators .4 / .6, ceiling fan .442 / .558, MELs .4 / .6 (with its latent), baseboard 1.0 convective. The sums of conv + rad + sw
  equal input times the sensible fraction; latent likewise. The gas furnace and water heater rows are HVAC and outside this item.
- Rollback: `rollback_failed_equipment_ports` swaps `rollback_ports` back with `PortSlots::copy_into`, which `clone_from`s every
  `ThermalAccumulator` whole, including `shortwave_gain_w`, so a failed step discards short-wave with the rest; the reported discarded
  total now includes it (`discarded_thermal_w`). A failing load never leaves half a gain (convective and radiant go in one `accumulate`,
  short-wave in a second call that cannot fail once the first succeeded on the same zone).
- Both solve paths: production `build_input_vector` and the debug breakdown both call convective, radiant and short-wave in the same order.
  Both interior-radiation modes deposit every watt (the ScriptF test closes `u.sum() == shortwave_w`); the StarMesh branch is the same
  generic distribution but is not pinned (L4).
- Not covered by this probe: a non-indoor zone's short-wave and radiant deposit in the solver (the garage lights' 0.2 / 0.6 / 0.2 reach the
  garage port; the solver deposit for a non-indoor zone falls back to the zone's air node when it has no surface list, and I did not see it
  drop a watt, but the public API does not expose `u`).

## 4. `ZoneHeat`, observer, diagnostics, Python, cost

- One accessor (`ThermalAccumulator::heat`) feeds `diff_ports`, `capture_ports`, `diagnostics::capture` and the solver's `indoor_heat`.
  `total_gain_w` is now `heat().total_w()`. Duplicates of the field list remain (L5).
- Hot loop: `ZoneHeat` is `Copy`, `heat()` builds four floats, nothing allocates. `capture_ports` and `thermal_gains_w` still
  collect a `Vec`, as before, and only on the observe/diagnostic paths. `profiling_alloc` passes.
- Step time, release, ResStock year at 900 s (35040 steps), eight alternating runs each, `528ebc6c` against `8ded384b`, median (min to max), us per step:

  | Home | `528ebc6c` | `8ded384b` |
  |---|---|---|
  | bldg0000007 | 16.40 (16.01 to 17.94) | 16.57 (16.27 to 17.59) |
  | bldg0101439 | 14.97 (14.85 to 15.28) | 15.18 (15.01 to 16.68) |
  | bldg0176227 | 36.10 (33.10 to 41.29) | 34.86 (33.72 to 39.29) |

  Differences are +1.0 %, +1.4 % and -3.4 %, inside the run-to-run range; no slowdown over noise. (The machine was shared; load average 11.)

## 5. `xs:boolean`

Grep of `hares-io` and `hares-core` for `"true"`, `"false"`, `eq_ignore_ascii_case`, `to_lowercase`, `as_bool`, `parse::<bool>`:
no text-to-bool conversion of an HPXML element remains outside `xs_boolean` / `child_bool`. The 12 sites are `HasFlueOrChimneyInConditionedSpace`,
`Basement/Conditioned`, `RadiantBarrier`, `InsulationSpansEntireSlab`, attic `Vented`, foundation `Vented` (all `building.rs`),
`ClothesDryer/Vented`, `CookingRange/IsInduction`, `Refrigerator/PrimaryIndicator`, two `VentilationFan/UsedFor...` (`resolve_loads.rs`),
`WaterFixture/LowFlow` (`resolve_water_heater.rs`). The remaining lowercase conversions are enumerations (`AtticType`, units, fuel,
`CompressorType`), not booleans. Errors are `HpxmlError::InvalidField` whose `path` is the element path and whose message prints the value and
the rule; `assert_reads_xs_boolean` pins, per site, that `1` and `0` mean `true` and `false`, that the two differ, and that `yes`, `True1` and the
empty string fail naming that path. Note the deliberate tightening: `True` and `TRUE`, accepted before, are now errors (correct for
`xs:boolean`); the shipped fixtures contain neither.

## 6. Typed `schedule_row`

No site infers the row another way: the only reader is `EventReplay::locate` (`event_load.rs:118`); scheduled loads read the schedule
payload the environment already indexed; `SCHEDULE_ROW_DOMAIN_ID` has no references left; `compute_schedule_offset` is `cfg(test)` only.
Calendar behaviour is pinned (date, Feb 29 skip, year wrap, DST). The field is not part of the checkpoint, and `EventReplay.current` is
recomputed from the row each step, so checkpoint and `load_checkpoint` have no replay state to restore; after a restore the next step's
row comes from the restored clock. `EnvironmentState::schedule_row` has `#[serde(default)]`, and `None` means "built without one", which makes
event replay error (tested). Resolutions 60, 300, 900, 1800, 3600 and 7200 s on `bldg0176227` run two days with zero rollbacks; 1200 s fails the
build with a clear "not an integer multiple" error before any row is read.

## 7. Parity bands, cause notes, citations

- Values (head, `parity` test output): zone-temperature MAE and bands, `observed` then `allowed`: cz2a_gas 0.6758 / 0.69, cz4a_ashp 0.8843 / 0.90,
  cz4a_battery_only 0.8632 / 0.88, cz4a_pv_battery 0.8342 / 0.85, cz4a_pv_only 0.8798 / 0.89, cz5a_ev_charging 0.8652 / 0.88,
  cz5a_ev_only 0.8623 / 0.88, cz6b_pv_battery_ev 0.8575 / 0.87, cz6b_resistance 0.7084 / 0.72, cz5a_minisplit 1.7433 / 1.75;
  mini-split short-window HVAC 64.150 / 64.8. Each is the measured value plus 0.0086 to 0.0168 (rounded up to 0.01); nothing loosened
  (the diff only lowers). See L3 for what that margin catches.
- Cause notes (class (a)): checked against the sources. `vendors/OCHRE/ochre/Equipment/Equipment.py:80-85` sums the convective and radiative
  fractions into `sensible_gain_fraction`, and its own comment says "FUTURE: separate convection and radiation, move radiation gains to the
  surfaces around the zone"; `:197` adds the sum to `zone.internal_sens_gain`; `ochre/utils/hpxml.py:1522-1527` is the lighting
  dict with the `# TODO: get default fractions/multipliers for lighting` line, "Convective Gain Fraction" 1, "Radiative Gain Fraction" 0
  (the dict spans 1523-1528; the citation is one line early at the top, correct in substance). OS-HPXML v1.12.0 `model.rb:249-251` is
  `setFractionRadiant(0.6)`, `setFractionVisible(0.2)`, `setReturnAirFraction(0.0)`. EnergyPlus v24.2.0 `InternalHeatGains.cc:7640`
  (`VisGainRate = Q * FractionShortWave`) and `:7649` (`QLTSW += VisGainRate`), `:1393` (`FractionConvected = 1 - (return air + radiant + short-wave)`),
  `:7666-7667` (electric equipment radiant and convected gain); `HeatBalanceSurfaceManager.cc:3687-3693` (`EnclSolQSWRad = EnclSolQD + sumSpaceQLTSW`).
  OS-HPXML `hotwater_appliances.rb` lines 80, 121, 132, 172, 220, 268, 309, 320 and `misc_loads.rb:89` are the `0.6 *` radiant fractions. All as cited.
- The class is established by measurement, not only by citation: with every internal gain forced convective (mutation M-A, the OCHRE
  treatment) the 12 fixtures reproduce the D-005 baselines to within 0.001 °C (1.711, 0.746, 0.408 °C and so on), and the HARES values with the split
  are the ones the register states.

## 8. D-005 to D-007: reproduction and pins

Mutations (rsync copy, separate target dir, each restored from the branch after):

| Mutation | Result |
|---|---|
| M-A: `GainFractions::of` returns no radiant and no short-wave (all convective, OCHRE) | `hares-equipment`: `radiant_part_of_sensible_leaves_the_rest_convective` and `visible_part_takes_the_short_wave_path` fail. `hares-core` `appliance_zone_gains`: `hpxml_appliance_gains_reach_the_conditioned_zone`, `resstock_event_load_replay_delivers_its_gains`, `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`, `hpxml_lighting_splits_convective_radiant_and_visible` all fail. The parity suite on this tree reproduces D-005's "before" numbers for all 12 fixtures to within 0.001 °C. |
| M-B: the resolver gives lighting no radiant and no visible share (appliances kept) | `resolve_loads::tests::lighting_carries_a_visible_part` and `resolver_carries_the_radiant_share` fail. Parity gives the appliance-only numbers of D-005 (1.622, 0.696, 0.421 °C, site 17.147 %, HVAC 64.249 %) and the starting point of D-006. |
| M-C: short-wave goes to the zone air node instead of the surfaces | `thermal_solver::tests::shortwave_gains_are_absorbed_like_transmitted_diffuse_solar` and `thermal_pathway_physics::zone_sensible_breakdown_debug_must_include_radiant_air_residual` fail (the two D-007 pins). Parity gives D-007's "before" numbers (1.749, 0.719, 0.885, 0.436 °C). |
| M-D: `apply_port_shortwave_inputs` not called in `build_input_vector` | `production_step_injects_shortwave_gain` fails (the previous L2 gap is closed). |

All figures in the three entries reproduce, so they are accurate. See L2 for the two things they leave out.

## 9. Goldens

None changed in this range and both modes are bitwise identical (section "Block results"). The goldens that moved for OD-30 were checked in the
previous round. Their per-column deltas, as recorded in `ledger/00-schedule.md` row 21h, are the only statement of them; the measurement
ledger `02-measurement.md` still has no OD-30 entry (grep), which the ledger's own "Measurement" line for OD-30 requires. Record
the deltas there at fold.

## 10. Test quality and shipped text

- Tests assert the physics of the change (shares of input to each channel, the closure of `u`, a bitwise-checked `xs:boolean` contract for each element), are
  deterministic and do not depend on RNG. `assert_reads_xs_boolean` is a good shape: one helper, each site names its own failing-before test.
  Weaknesses: L4, L8, and `rollback_discards_all_of_the_zone_heat` tests the sum helper, not a rolled-back step.
  `equipment_init_failure_fails_the_dwelling` and `init_failure_fails_the_build_after_the_identity_checks` cover one root twice.
- Added lines and the eight commit messages: no em dashes, no ticket IDs, no agent or tool references, no `TODO`s (the one `TODO`
  is a quotation of OCHRE inside D-006), no "see other_file" comments. `ruff` and `rustfmt` clean on changed code.

## Probes used (scratch, not committed, deleted)

1. Override matrix (section 1.1 and M1): `Dwelling::from_config` on the cz2a fixture, then `remove_equipment` and step of each load alone.
2. ResStock resolutions (section 6).
3. Day conservation under `-F observe` (section 3).
4. Step-time A/B (section 4): two release binaries, built in separate target dirs because the workspace crates hash to the same artifact name across trees.
