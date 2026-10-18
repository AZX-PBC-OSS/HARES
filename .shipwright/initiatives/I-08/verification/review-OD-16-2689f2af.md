# Review and red team: OD-16 follow-ups, `fix-od16` at 2689f2af (on 9713fe20, on 50a5c261)

Commits: `feaff388` (DR pre-conditioning on one axis, relaxation on both,
release at event end), `211b530e` (band classes, release restores the
configured band, restore validation, IdealHvac zone error), `a1173a4a`
(terminal state after the budget error), `55854486` (state columns kept
on a failed step), `287b8385` (solar override must carry the PV
surfaces), `db644111` (`reported_core_output`, Python `equipment()`),
`2689f2af` (PV tilt and azimuth from input). They fix
`review-OD-16-9713fe20.md` (MJ1, MJ2, m1-m11, i3, i4).

Reviewer: independent (did not write any of the commits). Block and sweep
in the worktree `.workstrees/verify-od16b` (detached at 2689f2af,
vendors/OCHRE initialized), `CARGO_TARGET_DIR=.workstrees/target-verify-od16b`,
`-j 8`. Every revert ran in a disposable `git archive` copy of 2689f2af in
the session scratchpad; the 50a5c261 physics run in a second archive copy.
Logs: session scratchpad `od16b-final/logs/`.

**Verdict: CHANGES-REQUIRED.**

Counts: 0 blockers, 1 major, 7 minor, 6 info.

Every finding of the 9713fe20 review is fixed, and every fix I reverted is
pinned by a test that fails without it (table below). The regression block
is green apart from the known release-clippy import and the known
environmental OCHRE failure. The major is in the new contract itself:
`DrCompliance` chooses the pre-conditioning axis from the sign of outdoor
minus zone temperature, and on four ResStock homes that sign is wrong in
36 to 58 % of the hours the air conditioner runs.

## Findings

### Major

**MJ1. The DR pre-conditioning axis comes from outdoor minus zone
temperature, which is the wrong signal for about half of all cooling
hours, and on a cooling target it turns a pre-cool into a relaxation.**
`crates/hares-core/src/actors/dr_compliance.rs:879-904`
(`setpoint_adjust_signal`) sends a positive delta on the heating axis when
`weather.outdoor_temp_c < ambient_other_space_c.conditioned_zone_c`, else
on the cooling axis.

The sign of `T_out - T_zone` is only the conduction and infiltration term
of the zone load. Solar gains through glazing, internal gains and the heat
stored in the mass keep a home in cooling long after the outdoor air falls
below the zone: every summer evening and night, and through the shoulder
seasons. EnergyPlus does not decide heating or cooling from this sign. Its
predictor (`ZoneTempPredictorCorrector.cc`, `ZoneSysEnergyDemand`
`OutputRequiredToHeatingSP` / `OutputRequiredToCoolingSP`) computes the
load to each setpoint from the full zone heat balance, which includes
solar and internal gains. OCHRE has no axis guess at all: its DR controls
address `HVAC Heating` and `HVAC Cooling` separately
(`ochre/Equipment/HVAC.py`, `update_external_control`), so the target
equipment names the axis.

- Repro 1, frequency (scratch test `zz_axis_tally`, a full 2018 at hourly
  steps, default warm-up). Each hour the AC is in a cooling mode, I
  counted whether the rule, on the environment the actor sees, picks the
  heating axis:

  | home | cooling hours | rule picks heating | heating hours, rule picks cooling |
  |---|---|---|---|
  | bldg0000003 (MA) | 2456 | 1431 (58.3 %) | 0 of 4885 |
  | bldg0000001 (AL) | 4269 | 2017 (47.2 %) | 0 of 2722 |
  | bldg0000004 (TX, heat pump) | 3195 | 1137 (35.6 %) | 1 of 1908 |
  | bldg0176227 (CA) | 2571 | 1409 (54.8 %) | 0 of 2057 |

  The wrong hours are spread over every cooling month (MA: 251 in June,
  344 in July, 369 in August), and they are mostly afternoons after 15:00
  and evenings, which is when DR peak events run.
- Repro 2, outcome (scratch test `zz_precool_shoulder_afternoon`): CA home
  bldg0176227, 2018-05-08, a `DrCompliance` with
  `setpoint_delta(2.0)` aimed at the Air Conditioner, active 18:00-22:00.
  At 19:00 and 20:00 the outdoor air is 21.5 and 19.0 °C, and the zone is
  26.2 °C. The rule raises the AC's heating setpoint by 2 °C, which pushes
  its cooling setpoint up to keep the gap. The baseline AC is cooling in
  those hours; with the pre-cool event the AC is held Off. The event
  relaxes the home it was meant to pre-cool, and nothing is rejected or
  counted (`rejected_control_signals` 0).
- On the heating side the rule is right in every hour I found (winter has
  the zone above outdoor). The asymmetry is the physics: gains can only
  add heat, so "outdoor colder than the zone" is not evidence of a heating
  load.
- Coverage: the dwelling test `dr_preconditioning.rs` checks only a
  heater in January. No test exercises a cooling target or a shoulder
  season through a dwelling.
- Fix: choose the axis from what the target is, not from the weather.
  - A target whose end use is `HVAC_COOLING` moves the cooling axis; one
    whose end use is `HVAC_HEATING` moves the heating axis. In HARES every
    cycling unit (furnace, boiler, baseboard, AC, the heat pump's heater
    and cooler) is a separate equipment with one end use, so this decides
    every case except a dual-mode unit, and it is exactly OCHRE's model.
  - For a dual-mode target (IdealHvac), use the thermostat's active mode,
    or the last active mode when in the deadband (readable from
    `equipment_core` `operating_mode`), or require the event to name the
    direction (`PreHeat` / `PreCool`), as OpenADR and utility pre-cool
    programs do.
  - Add a dwelling test of a pre-cool event on an AC on a shoulder-season
    evening, asserting the cooling setpoint moved down and
    `rejected_control_signals` stays 0.

### Minor

**m1. The release form does not hand a water heater's setpoint back.**
`ControlSignal::thermal_release` is documented
(`crates/hares-types/src/control_signal.rs:215-217`) as handing "the
thermostat back to its schedule and its configured band", and the commit
message of `211b530e` says water heaters "do the same with their
deadband". They restore the deadband only. Every storage heater keeps the
event's setpoint after a release (`water_heater/gas.rs:924`,
`resistance.rs:917`, `heat_pump_wh.rs:1411`, `indirect_tank.rs:710`,
`tankless.rs:667`: `if let Some(sp) = update.setpoint_c`).

- Repro: scratch test `zz_release_restores_the_configured_setpoint` in
  `gas.rs`: setpoint +8 °C, then `thermal_release()`, then
  `assert_eq!(eq.setpoint_c, configured)` fails.
- OCHRE also keeps an external water heater setpoint
  (`WaterHeater.py:91-112`, "Setpoint will only reset back to default
  value when {'Setpoint': None} is passed"). So either restore the
  configured setpoint on release (consistent with the HVAC release and
  with the doc), or narrow the doc and the commit's claim to the band.
  Pin whichever is chosen in the shared-contract tests.
- `DrCompliance` cannot reach this today (its `AbsoluteSetpoint` names a
  cooling setpoint, which a water heater rejects), so it affects Python,
  gym and HELICS callers.

**m2. A terminal dwelling accepts `load_checkpoint` and
`restore_building_state`, and then still refuses every step.**
`dwelling/mod.rs:5043` and `:5219` leave `terminal_error` as it is.

- Repro (scratch test `zz_terminal_then_load_checkpoint`): checkpoint at
  start, run to the budget error, then `load_checkpoint(cp)` returns
  `Ok(())`, `restore_building_state(&cp)` returns `Ok(())`, and
  `run_timestep`, `simulate()` and `run_warmup_converged` all return "the
  run ended at an earlier step".
- A full checkpoint restore replaces all of the half-stepped state, so the
  state is consistent again. Either clear `terminal_error` on
  `load_checkpoint` (a full restore makes the dwelling resumable; the RL
  `reset` and the consumer's co-simulation rewind both rely on it), or
  refuse the load with the terminal error. A load that succeeds into a
  dwelling that cannot step is the inconsistent choice.
- `restore_building_state` is a partial restore, so refusing it on a
  terminal dwelling is the safe choice.
- Python `reset_with_seed` rebuilds the dwelling, so the gym and vector
  env recover; Rust callers and the Python `load_state` path do not.

**m3. The gym's deadband action space spans the tank range for HVAC
targets.** `python/ochre_next/rl/gym_env.py:222-225` and
`crates/hares-python/src/py_gym.rs:84` bound every `deadband_c` action to
[0.1, 44.4] °C, the union of the classes. An HVAC target holds it to
[0.1, 10] (cycling) or [0, 10] (ideal). A policy that samples uniformly
over the action space sends a rejected HVAC action about 78 % of the time,
counted as a rejection rather than refused by the space. The ideal unit's
valid 0 is outside the space. Bound the action by the target's class
(the equipment's descriptor gives it).

**m4. The tank checkpoint restore check is not pinned.**
`water_heater/mod.rs:316` (`restored_tank_deadband_c`): with the
validation removed, all hares-equipment and hares-core tests pass. Only
the HVAC restore is pinned (`furnace::tests::a_checkpoint_carrying_an_invalid_band_is_rejected_on_restore`).
Add the same case to the water heaters' shared-contract tests.

**m5. IdealHvac's ideal-capacity zone lookup is not pinned.**
`hvac/ideal_hvac.rs:301`: restoring `unwrap_or(21.0)` on this line passes
every test. `a_step_without_its_zone_errors_instead_of_assuming_a_temperature`
does not reach the `use_ideal_cached` branch. Add an ideal-capacity case.

**m6. Four of the state columns kept on a failed step are not pinned, and
one is not a state.** `55854486` keeps every `v8_state_columns` entry
(`dwelling/mod.rs`, `resolve_v8_columns`). The test pins speed, defrost
state, supply temperature and minimum on time. Return temperature,
minimum off time, supply air temperature, PV irradiance and the EV
connection state and charging level changed from zero to the committed
value with no test. PV irradiance is an input of the timestep, not the
array's state: after a failed step the column reports the previous
timestep's irradiance (at sunset, a nonzero value for a dark step).
Treat irradiance as a flow, or read it from the environment, and pin the
EV and remaining columns.

The split is otherwise complete for every equipment type at verbosity 8.
I classified every per-equipment column `record_step` writes (`mode`,
`setpoint`, `soc`, `schedule`, `speed`, `defrost_state` and the v8 state
columns are states; electric, gas, capacity, COP, reactive power, power
factor, energy, ER power, SHR, fan, main power, runtime fraction, latent
gains, duct losses, compressor, fan, pan heater and capacity columns are
flows). The end-use aggregate skips the failed unit (`:7480`). Water
heaters, batteries and generators have no other per-equipment columns.

**m7. `[writing]` fails on `55854486`.** The added line
`dwelling/mod.rs:445` (`/// V8 per-equipment telemetry diagnostic
columns, as`) matches `\bV[0-9]{1,2}\b`. The rule rewords any such line
("verbosity-8 per-equipment telemetry columns"). The other six commits
pass.

### Info

- **i1. Every pre-conditioning step logs a warning and counts a
  violation.** On a 21.1/24.4 °C home a +2 °C heating delta always pushes
  the cooling setpoint (`thermostat.rs:624-633`), and `DrCompliance`
  re-dispatches every step, so each event step adds a `tracing::warn!`
  and increments `setpoint_violation_count`. Expected behaviour, but
  noisy for a routine event.
- **i2. A pushed axis is held as an absolute value through the event.**
  A one-axis delta keeps the other axis at `prior` (`thermostat.rs:588-594`),
  so the value pushed at the first step stays fixed if the schedule moves
  during the event. The release clears it at the end.
- **i3. Two readers still see a failed step's stale values.** The actors'
  `latest_env.equipment_core` keeps the previous successful snapshot
  (`dwelling/mod.rs:7085`), and Python `PyEquipment.telemetry`
  (`py_dwelling.rs:1026`) passes the telemetry, whose power keys are the
  previous step's. Both are "last known" rather than this step's.
  Document it next to `reported_core_output`.
- **i4. A multi-array PV is not attached to a roof automatically.**
  `dwelling/mod.rs:1540` skips any PV with more than one array. Before,
  it attached by the top-level orientation, which for a multi-array PV
  was a default and not an array's. Each array's `attached_boundary_id`
  is the only route now. Note it in the PV docs.
- **i5. `ArrayOrientation` is matched case-insensitively and trimmed**
  (`resolve_der.rs:39`); the HPXML schema enumerates lowercase values only
  and OS-HPXML would reject `South`. Harmless leniency.
- **i6. The pytest skip census.** The census file in the main tree's
  `ledger/pytest-skip-census.txt` is empty now (OD-13 is not on this
  base), so the block's census diff reports all 19 skips. They are the
  same 19 the 9713fe20 review recorded with `PYTHONPATH=vendors/OCHRE`
  (5 observer diagnostics, 8 OCHRE parity columns, 6 ResStock conversion
  fixtures).

## Earlier findings: fixed and pinned

Each fix was reverted alone, or with fixes whose tests are disjoint, in
the archive copy. The named crate suites were then run. "Fails" lists the
tests that fail with the revert applied. Everything passes with no revert.

| finding | revert | fails |
|---|---|---|
| MJ1 (converging pair) | positive delta sends `(d, -d)` again | `preconditioning_moves_the_axis_of_the_envelope_load`, `dr_preconditioning::a_preconditioning_event_raises_the_heating_setpoint_and_nothing_is_rejected` |
| i3 (DR delta never released) | setpoint actions non-sticky | `setpoint_actions_are_released_when_the_event_ends` |
| MJ2 (ideal zero band) | Ideal range floor back to 0.1 | `bestest_case_600/640/900` (and their `regression` copies), `an_ideal_thermostat_accepts_a_zero_band`, the class-range tests, and 39 dwelling unit tests and the occupant-count tests built on the BESTEST fixture |
| release restores the band (HVAC) | `else if is_release` disabled | `the_release_restores_the_configured_band` |
| release restores the band (tanks) | `(None, true) => current` | 4 `thermal_setpoint_band_follows_the_shared_contract` (gas, resistance, HPWH, indirect) |
| m1 (release guard) | guard forced true | `a_release_under_a_narrow_schedule_installs_nothing` |
| m2 (tank cooling setpoint) | rejection disabled | the 4 shared-contract tests |
| m3 (end-use aggregate) | read, not reverted: the test asserts the `HVAC Heating` end-use column is `[0.5, 0.0]` | |
| m4 (terminal step) | `run_timestep` terminal check disabled | `the_run_stays_ended_after_the_failure_budget_error` |
| m4 (terminal checkpoint) | `save_checkpoint` terminal check disabled | same |
| m5 (v8 state columns) | state loop zeroes on failure | `a_failed_step_records_no_flow_and_keeps_the_committed_state` |
| m5 (speed column) | speed zeroed on failure | same |
| m6 (override without PV surfaces) | `set_solar_override` PV check disabled | `a_solar_override_must_carry_every_pv_surface`, `pv_override_observation::an_override_without_the_pv_surfaces_is_rejected_at_the_call` |
| m6 (registration under an override) | `register_pv_surface` check disabled | `a_pv_surface_the_active_override_lacks_is_rejected` |
| m6 (empty override) | emptiness check disabled | `an_empty_solar_override_is_rejected` |
| m7 (IdealHvac fallback) | `unwrap_or(21.0)` at `:301` | none (minor m5 above) |
| m8 (HVAC restore check) | `restore_hysteresis` skips the check | `furnace::a_checkpoint_carrying_an_invalid_band_is_rejected_on_restore` |
| m8 (tank restore check) | `restored_tank_deadband_c` skips the check | none (minor m4 above) |
| m9 (misplaced doc) | read | `validate_finite` has its doc back |
| m10 (field name) | Ideal `band_field` reports `hysteresis_c` | `an_ideal_band_error_names_the_configured_field_and_the_bound` |
| m10 (reason per bound) | read; `the_error_gives_the_reason_of_the_bound_crossed` covers each bound | |
| m11 (tank maximum) | tank maximum back to 10 °C | `a_tank_deadband_spans_the_widest_tank_thermostat`, `each_class_has_its_range`, `a_signal_deadband_must_fit_some_class`, `thermal_setpoint_named_deadband_is_held_to_some_thermostat_class` |
| i4 (stale Python flows) | `reported_core_output` returns the raw output | `a_failed_step_reports_no_flow_and_the_committed_state` |
| PV tilt from HPXML | `ArrayTilt` falls back to 20 | `hpxml_pv_without_array_tilt_is_a_missing_field` |
| PV orientation map | southwest mapped to 220 | `hpxml_pv_azimuth_comes_from_its_orientation_with_a_warning` |
| PV surface resolution | resolution fixed at 5° | `a_pv_surface_follows_the_configured_resolution`, `every_array_carries_its_own_orientation_at_the_configured_resolution` |

The Python constructors (`thermal_setpoint_release`, the HELICS guard,
the gym builder, the `MissingInput` and `SolarOverrideMissingSurface`
mappings to `HaresConfigError`) were checked by reading and by the green
pytest run. They were not revert-checked, which would need a wheel
rebuild per revert.

## The new contracts, checked against building physics and controls

**Pre-conditioning axis.** See MJ1. Relaxation as the widening pair is
right in every season, and the release at event end is right. Choosing
the axis from the weather is not. Other cases I considered:

- Sunny cold day in cooling: covered by MJ1. The rule picks heating, and
  the push raises the cooling setpoint.
- Summer night: in the AL July evenings I ran (20:00-24:00) the outdoor
  air stayed above the zone, so the rule picked cooling and pre-cooled
  correctly. In MA and CA the night outdoor air drops below the zone
  while the AC still runs, which is where the tally's wrong hours come
  from.
- Thermostat active mode, and the HVAC's current call: both are better
  signals than the weather, and the target's end use is better still,
  because it is known before the event and cannot flip mid-event. With
  the weather rule the axis flips when the outdoor temperature crosses
  the zone's (sunset, sunrise), and the earlier axis's override stays
  (i2).
- A heater targeted in a warm spell: the rule sends a cooling delta to
  the furnace, which only pushes its heating setpoint down. No harm, but
  no pre-heat.

**Release restores the configured band.** It is right for HVAC: the band
a named signal sets is part of the event, as the 9713fe20 review
recommended. Interactions:

- A schedule change during the event: the delta is recomputed from the
  current schedule at every dispatch, and the release hands back to the
  schedule as it is then, so a setback that started during the event
  applies after it.
- `configured_hysteresis_c` is recorded at `validate_configuration` (in
  `HvacCore::init` and `IdealHvac::init`) after the configuration is
  applied, so it is the configured band, not the default.
- Checkpoint across the release: `configured_hysteresis_c` comes from the
  configuration at construction, the event band and the runtime override
  come from the checkpoint, and
  `restored_run_during_a_named_band_event_matches_the_continuous_run`
  covers a release after a restore. It passes.
- Water heaters do not follow the same rule (minor m1).

**Terminal state.** Every stepping entry point honours it:

- Rust: `step`, `simulate`, `run_warmup_converged` (all through
  `run_timestep`) and `save_checkpoint`.
- Python: `step`, `simulate`, `save_state` and `save_checkpoint`, through
  the same Rust calls.
- The gym terminates the episode, and its `reset` rebuilds the dwelling.
- HELICS: the exception leaves the step loop and `finalize` runs.
- The fleet marks the home failed.

The gap is restore (minor m2).

**Solar override validation and the `register_pv_surface` Result.** The
only callers are Python `set_solar_override` and `add_pv`
(`py_dwelling.rs`), `register_pv_surfaces` in construction, and the tests.
Both Python paths map the error to `HaresConfigError`, and none swallows
it. The gym, HELICS, the fleet and the Python adapters do not call
either. A Python user who sets the override before `add_pv` now gets an
error and must clear the override, add the PV, and set it again. That
order dependence is intended.

**PV orientation.** The `ArrayOrientation` map equals OS-HPXML
`defaults.rb` `get_azimuth_from_orientation` (north 0 through northwest
315), applied only when `ArrayAzimuth` is absent, as `apply_pv_systems`
does. `ArrayTilt` has no `_isdefaulted` in OS-HPXML `hpxml.rb` and no
default in `defaults.rb`, so requiring it matches OS-HPXML. Multi-array
registration now registers each array's surface at the configured
resolution, which fixes a multi-array PV at mixed orientations failing
init. No fixture moved: all 45 `PVSystem` elements in vendors/OCHRE,
`tests/` and `data/` carry both `ArrayTilt` and `ArrayAzimuth`, and every
golden is identical.

## Regressions: verification block (worktree of 2689f2af, `-j 8`)

| line | result |
|---|---|
| `[fmt]` | pass |
| `[clippy]`, default, dst, profiling, observe, observe+profiling | pass |
| `[clippy-release]`, `-release-dst`, `-release-observe` | fail only on `hares-envelope/src/thermal_solver/mod.rs:1858` unused `BoundaryDiagnosticInfo` (the known one); `--keep-going` shows no other error |
| `[test-debug]` | 7227 passed |
| `[test-observe]` | 7286 passed |
| `[test-dst]` | 7240 passed |
| `[test-profiling]` | 7233 passed |
| `[test-observe-profiling]` | 1370 passed |
| `[test-release]` | 7184 passed |
| `[bench]` | pass |
| `[golden]` default | all six identical |
| `[golden]` `-F dst` | `full_year_900s_dst` identical |
| `-F profiling` compare-all | all six identical |
| BESTEST and physics suites (`bestest parity conditioned_oracle freefloat_oracle structural_envelope_oracle envelope_oracle`) | 25 passed; every printed metric byte-identical to 50a5c261 (only load and elapsed times differ), e.g. 600 heating 3227.901596, 600 cooling 6133.952799, 900 heating 948.188657 kWh; fixtures at `deadband_c = 0.0` as at 50a5c261 |
| `[python-build]` | pass |
| `[pytest]` with `PYTHONPATH=vendors/OCHRE` | 1393 passed, 19 skipped, 3 xfailed, 1 failed: `test_ochre_parity.py::test_7day_benchmark`, the vendored OCHRE's own pandas `datetime64[us]` overflow, as at 9713fe20 (environmental). Skips: i6 |
| ASHRAE gate (`uv run --extra ochre scripts/check_ashrae_reference.sh`) | PASS |
| `[writing]` | six commits clean; `55854486` fails on one added line (minor m7) |
| ruff on the seven changed `.py`/`.pyi` files | 157 findings at 9713fe20 and at 2689f2af, identical set, none on a changed line |

## Consumer: what `~/src/HARES_CALIBRATION_SYSTEM` must change at the fold

From `i08-consumer` at `b71dbb4`:

1. `calibration/ami2/dr_actuation.py:206`:
   `RELEASE = on.ControlSignal.thermal_setpoint(None, None, 0.0)` now
   raises `ValueError` at import. Replace it with
   `on.ControlSignal.thermal_setpoint_release()`. Rewrite the comment block
   at `:175-205`: the release now restores the configured band, and a
   deadband on a release is rejected. Rewrite the module docstring at
   `:12-38`: a release exists, the engine's `DrCompliance` sends it, and
   item 11 is resolved.
2. `calibration/tests/suite_ami2.py:877` and `:1762`: the same release
   constructor change.
3. `calibration/tests/suite_ami2.py:1760`: the GEB pre-event
   `thermal_setpoint_delta(2.0, -2.0)` converges the setpoints and is
   rejected at dispatch whenever the gap falls below `2 x hysteresis_c`.
   Send the pre-event on one axis, chosen by the receiving unit: the
   heating delta `(2.0, None)` to heating equipment, and the cooling delta
   `(None, -2.0)` to cooling equipment.
4. `calibration/ami2/dr_actuation.py:248`: `thermal_setpoint_delta(d, -d)`
   is valid only for `d <= 0` (relaxation). Assert the sign, or reject a
   positive `setpoint_delta_c`, since a positive one is rejected at
   dispatch. Both this call and item 3 apply the same two-axis signal to
   every HVAC unit (`for e in hvac`). Send each unit only its own axis.
5. After every actuated run, assert
   `dw.health().rejected_control_signals == 0`. Dispatch rejections are
   counted, not raised.
6. `calibration/ami2/functions/_flexibility.py:219-221`: the note "The
   release must carry `deadband_c=0.0`" is now wrong. Point it at
   `thermal_setpoint_release()`.
7. `calibration/tests/suite_ami2.py:938-939` and `:1802-1804`: comments
   describing the old release forms (`(None, None, None)` as not
   releasing, `deadband_c=0.0` as the real release) are wrong now.
8. `calibration/coupling.py:408,420` and `calibration/cosim.py:90,102`
   rewind with `load_state`. After a failure-budget error the dwelling is
   terminal: `save_state` raises, and `load_state` succeeds but every
   later `step` raises (minor m2). Treat the budget error as fatal for
   that home, and do not rewind and retry. Calibration should also run
   with `max_consecutive_step_failures = 0` and check `port_rollbacks`.
9. PV: `calibration/der_inject.py:83-94` always writes `ArrayTilt` and
   `ArrayAzimuth`, and `hares_der_remeasure.py:85` passes `tilt` and
   `azimuth` to `add_pv`, so neither needs a change. If any injected
   HPXML drops `ArrayTilt`, construction now fails with a
   `MissingField`.
10. No consumer code calls `set_solar_override`, `register_surface` /
    `register_pv_surface`, or imports `MAX_THERMOSTAT_BAND_C`. That
    constant was removed from `ochre_next._hares` and replaced by
    `MAX_HVAC_THERMOSTAT_BAND_C` and `MAX_TANK_THERMOSTAT_BAND_C`, so
    nothing breaks there.
