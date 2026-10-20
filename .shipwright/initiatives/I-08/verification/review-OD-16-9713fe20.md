# Review and red team: OD-16 fixes, `fix-od16` at 9713fe20 (parent 50a5c261)

Commits: `cae2ab0a` (one band rule, release form, named axes, checkpointed
hysteresis), `10951bed` (failure budget, delivered output), `d960b82b`
(EBM window before commit, no zone-temperature default, negative EBM
energies), `b6b4caa1` (dispatch-time rejection, Python tests),
`9713fe20` (release build with `observe`). They fix
`audit-OD-16-fe804b59.md` (M1, M2, m1-m5) and
`redteam-OD-16-fe804b59.md` (B1, M1-M4, m1-m5).

Reviewer: independent (did not implement, audit or red-team OD-16). The
worktree `.workstrees/verify-od16` (detached at 9713fe20, vendors/OCHRE
initialized) was used for the block and the sweep. The worktree editor
refused source edits, so every revert ran in a disposable `git archive`
copy of 9713fe20 in the session scratchpad (vendors/OCHRE symlinked), and
the 50a5c261 comparisons ran in a second archive copy. Logs:
session scratchpad `logs/od16/`, `logs/sweep-*.tsv`.

**Verdict: CHANGES-REQUIRED.**

Counts: 0 blockers, 2 major, 11 minor, 4 info.

Every audit and red-team finding is fixed, and every one I reverted is
pinned by a test that fails without its fix (table below). Three new
behaviours are not pinned. The two majors are contract problems, not
regressions in the fixes themselves. First, the engine's own
`DrCompliance` actor emits the two-axis converging delta that the new
contract rejects. Second, the band floor is applied to the ideal
thermostat, which has no switching band in ASHRAE 140, and that moves
BESTEST without a recorded physics decision.

## Findings

### Major

**MJ1. The engine's own DR actor sends a signal the new contract rejects.**
`crates/hares-core/src/actors/dr_compliance.rs:502` maps
`DrAction::SetpointAdjust { delta_c }` to
`ThermalSetpointDelta { heating: Some(d), cooling: Some(-d) }`, and its
constructor documents a positive delta as "raises heating setpoint /
lowers cooling setpoint" (`:241-246`). A positive delta (pre-conditioning)
narrows the gap. With both axes named, the FSM now rejects any result
below `2 x hysteresis` (`hvac/thermostat.rs:563`). The rejection happens
at dispatch: it is counted and logged as a warning, and the event does
nothing.

- Repro (scratch FSM test in the mutation copy): base 21.1/24.4 °C
  (ResStock-typical), default hysteresis 1 °C.
  - Delta 0.5: accepted.
  - Delta 0.7: rejected (`gap 1.9 below the required deadband 2`).
  - Deltas 1.0 and 2.0: rejected.
- At 50a5c261 the same signal was auto-corrected (cooling moved to
  heating + 3), so heating pre-conditioning still happened. At 9713fe20
  the whole DR action is dropped.
- No test exercises a positive `SetpointAdjust` through a dwelling. The
  actor unit test (`setpoint_adjust_dispatches_delta_signal`) checks only
  the emitted shape.
- The consumer's GEB pre-event (`delta(2.0, -2.0)`) is the same shape and
  is already scheduled for migration at the fold. The first-party actor is
  not.
- Fix: see contract recommendation 2. Replace the converging two-axis
  delta with a directional action, add a dwelling-level test of a DR
  pre-condition event, and assert that the event moved the setpoint and
  that `rejected_control_signals` stays 0.

**MJ2. The band floor is applied to the ideal thermostat, and BESTEST moves
without a physics record.**
`IdealHvac` builds no equivalent battery (no `equivalent_battery` call in
`hvac/ideal_hvac.rs`), so a zero band cannot collapse anything there.
ASHRAE 140's case 600/900 thermostat is an ideal, non-proportional
control (heat below 20 °C, cool above 27 °C, no switching band), and so
are EnergyPlus ideal loads under `ZoneControl:Thermostat`. At 50a5c261,
`IdealHvac` accepted `deadband_c = 0.0`, which is exactly that spec.

`cae2ab0a` sends the IdealHvac deadband through `ThermostatConfig::validate`,
so 0.0 is now rejected, and moves `tests/fixtures/bestest/{600,640,900}.toml`
to `deadband_c = 0.1`. The fixture comment says "the band only gates the
reported thermostat mode", but `IdealHvac::step` delivers capacity from
the FSM mode, and annual loads move:

| metric | 50a5c261 | 9713fe20 | change |
|---|---|---|---|
| 600 annual heating kWh | 3227.901596 | 3227.903428 | +0.00006% |
| 600 annual cooling kWh | 6133.952799 | 6134.465844 | +0.0084% |
| 900 annual heating kWh | 948.188657 | 948.983303 | +0.084% |
| 900 annual cooling kWh | 2847.802189 | 2849.365450 | +0.055% |

Each move is toward or inside its band, and each is inside the ±1%
ratchet. Plan §3 rule 2 still requires the before and after values to be
recorded and the moved values to rest on a named reference. "Toward the
band" is not a reference, and the reference that applies (ASHRAE 140's
ideal thermostat) argues for the zero band. The ledger row says only
"BESTEST moves toward its bands"; `ledger/02-measurement.md` has no
numbers.

Fix: let `IdealHvac` keep a zero band, because the floor protects the
EBM, which IdealHvac does not have. Either give IdealHvac its own
`[0, MAX]` check, or apply the band rule only to cycling thermostats.
Then restore the fixtures to 0.0, so BESTEST is bitwise to 50a5c261. If
0.1 is kept, record the table above in the measurement log, with the
physics decision and a reference.

### Minor

**m1. The release guard is load-bearing but not pinned.**
`hvac/thermostat.rs:557` (`if heating_named || cooling_named`) is the
only thing that keeps a release from pushing an axis. Making it `true`
passes all 2615 hares-equipment tests. The release test uses a base gap
of exactly `2 x hysteresis`, and the push condition is a strict `<`, so
the test never reaches the push.

- Repro: schedule 22/23 °C, release `(None, None, None)`. With the guard
  removed, the release installs a heating override of 21 °C. A scratch
  test (`zz_release_under_a_narrow_schedule_installs_nothing`) fails with
  the guard removed and passes at 9713fe20.
- Add that test to `thermostat.rs` tests.

**m2. The water heaters' cooling-setpoint rejection is not pinned.**
`water_heater/mod.rs:309`: disabling the rejection passes every test. At
50a5c261 a cooling setpoint was taken as the tank setpoint, so this is an
unpinned behaviour change. Add a case to the shared-contract tests.

**m3. The end-use aggregate exclusion is not pinned.**
`dwelling/mod.rs:7444` (`&& !self.step_failed[eq_idx]`): disabling it
passes all 1299 hares-core tests. `FlakyPortEquipment` has no end-use
aggregate column. Add an assertion on an end-use power column to
`a_failed_step_records_no_flow_for_the_failed_equipment`, with the
equipment's end use set to one that aggregates.

**m4. "Ends the run" is not enforced: a retry after the budget error
double-steps the equipment that already ran.**
`record_equipment_step_failure` (`dwelling/mod.rs:5721`) returns `Err`
in the middle of a stage loop. The clock does not advance, `self.ports`
keeps the contributions of the equipment that already stepped, and
nothing marks the dwelling terminal. Python `poisoned` is set only on a
panic.

- Repro (scratch dwelling test): a steady 500 W equipment followed by a
  flaky 300 W one with outcomes `[ok, fail, fail, ok]`.
  - Step 3 returns the budget error.
  - Calling `run_timestep` again steps the steady unit a second time for
    the same timestep and fails with: `telemetry electrical consistency
    at step 2: sum(equipment_power_kw) 0.800000 kW does not match
    total_power_kw 1.300000 kW`.
- The retry is loud, but the error is misleading and the steady unit's
  internal state has advanced twice.
- The gym batch path sets `terminated`, so a well-behaved RL loop resets.
  A Rust caller, a Python script or a HELICS federate that retries does
  not.
- This class of partial step predates the branch (any mid-loop `?`), but
  the budget makes it a routine, documented outcome.
- Fix: keep the budget error and refuse every later step with it (a
  terminal state), or zero the ports at step entry and document that a
  failed step cannot be retried.

**m5. The frame zeroes some state columns, despite the doc.**
`dwelling/mod.rs:7351`: the `continue` skips every column below it. That
includes `speed` (`CoreState::speed_index`, `:7404`) and the
state-valued telemetry columns: EV connection state and charging level,
minimum on and off time, defrost state, and PV irradiance.
`docs/invariants-and-observability.md` says "its state columns (mode,
setpoint, SOC) keep its committed state". After a failed step the EV
reads as disconnected, for example.

Fix: write the state-valued columns (speed, EV state, defrost state,
min on/off, irradiance) above the `continue`, or narrow the doc to name
exactly what stays.

**m6. The PV night test was replaced, and the misconfiguration is caught
late.**
`b6b4caa1` replaced `test_pv_produces_zero_at_night_with_summer_override`
with `test_override_without_the_pv_surfaces_ends_the_run`
(`tests/python/test_py_solar_override.py:446`). The original intent, PV
output at night under a summer override, has no Python test any more.

The failure itself is a configuration error: the override lacks the PV
arrays' orientation surfaces. It surfaces only at the second `step()`,
through the failure budget.

Fix: restore a night-output test with an override that carries the PV
surfaces, and have `set_solar_override` reject an override that misses a
registered PV surface, with a typed error at the call.

**m7. IdealHvac keeps three silent zone-temperature fallbacks.**
`hvac/ideal_hvac.rs:296, 530, 579` keep
`lookup_zone_temp(...).unwrap_or(21.0)`. `d960b82b` removes the same
fallback from every other HVAC ("errors when the zone is missing,
instead of assuming 20 C"), and the branch edits this file. Under the
no-silent-defaults and slop-as-you-go rules, these should error the same
way.

**m8. A checkpoint restore writes the hysteresis without checking it.**
For example, `hvac/furnace.rs:447` and `:873`, `boiler.rs:482` and
`:932`, `baseboard.rs:311`, `ideal_hvac.rs:876`. A restored band is
never passed through `validate_thermostat_band_c`, yet the EBM error
text (`equivalent_battery.rs`) states that every source of the band is
validated to at least 0.1 °C. Validate on restore, or narrow the
message.

**m9. A doc comment moved to the wrong function.**
`crates/hares-python/src/utils.rs:287`: `validate_thermal_setpoint_band`
was inserted between `validate_finite` and its doc line. The new
function's doc now starts "Reject non-finite f64 values at the boundary,
naming the field.", and `validate_finite` has no doc.

**m10. The band error gives the wrong reason and field in some cases.**
`hares-types/src/error.rs:36-45`:

- The `ThermostatBand` message gives the sensor-resolution reason for
  every violation, including a value above 10 °C and NaN.
- For IdealHvac, the field is reported as `hysteresis_c`, while the user
  wrote `deadband_c`.

Give a reason per bound, and name the configured field.

**m11. The upper bound is justified only by the engine's own defaults.**
`thermostat_band.rs:22-25` sets `MAX_THERMOSTAT_BAND_C = 10.0` because
it "covers the widest default any equipment carries". Real tank controls
are wider. HPWHsim's product presets (`src/HPWHpresets.cc`, the model
CBECC-Res uses) turn heat sources on at 20, 25, 30, 40, 60 and 80 °F
below setpoint (11 to 44 °C). No current input reaches 10 °C: the sweep
shows no change. But an OCHRE-style `Deadband Temperature (C)` input or
a signal for such a product is rejected.

Fix: a separate tank maximum (for example 30 °C) or per-device ranges,
justified by device data rather than by the engine's defaults.

### Info

- **i1. Consumer contract break.** Two consumer calls stop working:
  - `RELEASE = on.ControlSignal.thermal_setpoint(None, None, 0.0)`
    (`calibration/ami2/dr_actuation.py:206`, `tests/suite_ami2.py:877`
    and `:1762`) now raises `ValueError` at construction.
  - The GEB pre-event `delta(2.0, -2.0)` (`suite_ami2.py:1760`) is
    rejected at dispatch.

  Both are recorded in `ledger/00-schedule.md` row 16a2 and in
  `plan-critique.md` as a migration at the fold. The consumer's
  `dr_actuation.py` comments (`:195-205`) document the old "inert"
  deadband and must be rewritten with it.
- **i2. A failure after commit is still possible, but not reachable.**
  `EquivalentBatteryWindow::with_load` can still fail after the step
  has committed state, for a NaN or negative load. Every caller passes a
  product of non-negative finite terms, so it is unreachable today.
- **i3. Pre-existing: a DR delta is never cleared.**
  `DrCompliance` treats `SetpointAdjust` as non-sticky
  (`dr_compliance.rs:561-568`, `track_dispatched`). A delta installs a
  runtime override that persists, and the actor never sends the release.
  The consumer documents this ("`thermal_setpoint_delta(0.0)` does not
  hand control back"). Record it in the ledger. It belongs with MJ1's
  fix.
- **i4. Python `Dwelling.equipment()` returns stale flows after a failed
  step.** It returns each equipment's committed `core_output`, so after a
  failed step its flows are stale. This is consistent with "committed
  state", but it differs from every other power reader. Document it.

## Audit and red-team findings: fixed and pinned

Each fix was reverted alone in the mutation copy and the named crate
suites were run. "Fails" lists the tests that fail with the fix
reverted. Everything passes with no revert applied.

| finding | revert | fails |
|---|---|---|
| audit M1, RT M2 (release installs an override) | guard `if heating_named \|\| cooling_named` forced true | none (m1); the scratch test with a narrow schedule fails |
| audit M2 (water heaters apply the release deadband) | `tank_thermostat_update` passes the deadband unchecked | 4 shared-contract tests (gas, resistance, HPWH, indirect) |
| audit M2 (tankless) | tankless band rejection disabled | `thermal_setpoint_band_is_rejected_for_a_heater_without_a_tank_thermostat` |
| audit m1 (per-zone thermal filter) | per-zone sum reads every `core_output` | `tolerated_heater_failure_reconciles_the_zone_thermal_sum` |
| audit m2 (validator) | validator skips `thermal_setpoint_band_c` | 4 `control_signal` tests, `a_deadband_override_without_a_setpoint_is_rejected_not_dropped` |
| audit m3 (one predicate) | band floor changed to `> 0` | 14 tests across hares-types, hares-equipment, hares-core |
| audit m4 (telemetry power) | `DwellingTelemetry` power reads `core_output` | `a_failed_step_reports_no_power_for_the_failed_equipment` |
| audit m4 (electrical summary) | battery/EV/PV summary reads `core_output` | same |
| audit m4 (frame flow columns) | `continue` disabled | `a_failed_step_records_no_flow_for_the_failed_equipment` |
| audit m4 (end-use aggregates) | aggregate filter disabled | none (m3) |
| RT B1 (sub-tolerance band) | band floor changed to `> 0` | as audit m3, including `sub_band_deadbands_are_rejected_and_the_unit_keeps_heating` and the dwelling `a_sub_band_deadband_is_rejected_and_the_furnace_keeps_heating` |
| RT M1 (configured zero hysteresis) | `ThermostatConfig::validate` band check removed | `configured_hysteresis_below_a_thermostat_band_is_rejected` |
| RT M3 (hysteresis not checkpointed) | baseboard restore drops the hysteresis | `signalled_band_survives_a_checkpoint_on_every_simple_heater` |
| RT M4 (deadband-only override dropped) | `IdealThermostat::decide` validation removed | `a_deadband_override_without_a_setpoint_is_rejected_not_dropped` |
| RT m1 (gym) | gym builder skips the band contract | `build_thermal_setpoint_rejects_a_deadband_without_a_setpoint` |
| RT m2 (named axis moved) | one-axis arm moves the named cooling axis | `a_named_cooling_setpoint_is_honoured_and_pushes_heating_down` |
| both named rejected | `(true, true)` pushes instead | `both_named_setpoints_violating_the_gap_are_rejected`, `ideal_hvac_rejects_invalid_runtime_thermal_setpoint_override` |
| RT m3 (partial state on EBM failure) | baseboard computes the window after commit | `a_failed_hvac_step_leaves_no_partial_state` |
| RT m4 (EnvelopeResolution) | stage check disabled | `equipment_in_the_envelope_resolution_stage_is_rejected_at_registration` |
| RT m5 (helper seeding) | helper skips `seed_equipment_core` | 4 dwelling tests |
| budget | budget check disabled | 3 dwelling tests, `a_pv_failing_every_step_ends_the_run_under_the_default_budget` |
| streak checkpoint | restore skips the streak | `the_failure_streak_survives_a_checkpoint` |
| streak reset | success does not reset | `a_successful_step_resets_the_failure_streak` |
| negative EBM energies | `max_e <= 0` check restored | `a_setpoint_below_the_reference_temperature_gives_negative_energies` |
| missing zone | `unwrap_or(20.0)` restored | `a_step_without_its_zone_errors_instead_of_assuming_a_temperature` |
| water heater cooling setpoint | rejection disabled | none (m2) |
| audit m5 (slop) | (read) | `apply_heating_control_unchecked` inlined; `hvac_tests` reads the default hysteresis; history prose gone; the rationale is stated once, in `thermostat_band.rs` |

The Python constructor wiring (`py_control`, `py_actor`, `from_dict`,
HELICS guard) was checked by reading and by the green pytest run. It
was not revert-checked: that needs a wheel rebuild per revert.

All new tests are deterministic (scripted outcomes, fixed fixtures, no
RNG or stochastic schedules).

## The new contracts, checked against building physics and controls

**Band range [0.1, 10] °C.**

- The floor is right for cycling thermostats.
  - The narrowest residential differential is 0.5 °F (0.28 °C); the
    ecobee installation guides give it as the default heat differential.
  - 0.1 °C keeps the EBM window `C x band` many orders of magnitude
    above `f64::EPSILON` for any physical zone capacitance.
- The floor is wrong for the ideal thermostat (MJ2): ASHRAE 140 and
  EnergyPlus ideal loads have no band, and IdealHvac has no EBM.
- The ceiling is self-referential for tanks (m11).

Nothing valid is rejected today. I swept every OS-HPXML sample in
vendors/OCHRE (425), `tests/fixtures/hpxml/ochre_samples` (14), the two
`data/examples` HPXMLs, and all 35 ResStock fixtures (2024.2 and
2025.1). Each was built with the default 7-day warm-up, then run for 24
hourly steps on 15 January and again on 15 July: 944 runs.

- The output at 9713fe20 is byte-identical to 50a5c261: 783 OK, 161
  construction errors with identical messages (all unrelated to bands),
  and 0 port rollbacks in every run.
- The HPWH default (14.7 °F, 8.17 °C) and the storage default (10 °F,
  5.56 °C) pass.
- DR events: `DrCompliance` positive deltas are rejected (MJ1). Its
  relaxation (negative) deltas and its absolute setpoints with an
  adequate gap pass.

**Both axes named and the gap violated: rejected.**

Recommended contract:

- An absolute `ThermalSetpoint` naming both axes with a crossed or
  too-narrow gap is rejected (as implemented). Precedent:
  - EnergyPlus terminates on crossed dual setpoints
    (`ZoneTempPredictorCorrector.cc`, `DualSetPointWithDeadBand:
    Effective heating set-point higher than effective cooling set-point`
    followed by `ShowFatalError`).
  - The Google Nest SDM API returns `INVALID_ARGUMENT`, "Cool value must
    be greater than heat value", for `SetRange`.
- OS-HPXML averages overlapping heating and cooling schedules
  (`hvac.rb` `apply_setpoints`, warning "HVAC setpoints have been
  automatically adjusted…"). That is input preprocessing, and HARES
  already mirrors it at init (`reconcile_for_deadband`). It is not a
  runtime-command rule.
- An auto-changeover thermostat (ecobee "Heat/Cool Min Delta",
  Honeywell auto mode) pushes the other setpoint when the user moves
  one. That is the one-axis rule HARES now implements, correctly.
- A `ThermalSetpointDelta` naming both axes that converges them should
  also be rejected in the engine. The engine cannot know which axis the
  sender cares about without guessing the season. Clamping the "less
  constrained" axis would silently defeat half the command in one
  season or the other, which is exactly how the pre-cool was lost at
  50a5c261.
- The fix belongs to the producers:
  - `DrCompliance` should express pre-conditioning as a single-axis
    delta, chosen from the target's active thermostat mode, which the
    actor can read from `equipment_core`.
  - It should express relaxation as the widening pair, which is always
    valid.
  - The consumer's GEB pre-event should do the same.
- Rejection is surfaced at dispatch, as a warning and in
  `rejected_control_signals`. A scripted consumer must check that
  counter. The calibration consumer should assert it is 0 after each
  actuated run.

**The release form (None, None, None) as the only pass-through, and
(None, None, 0.0) rejected.**

- This is the right API: one canonical release, and no deadband silently
  dropped (no-silent-defaults).
- Recommendations:
  - Add an explicit constructor (`ControlSignal::thermal_release()` /
    `ControlSignal.thermal_setpoint_release()`), so the release is not an
    implicit encoding.
  - State in the `ThermalSetpoint` doc that a signalled band outlives
    the release (`a_band_set_by_a_named_signal_outlives_the_release`).
    That matches OCHRE (`HVAC.py:275-280` keeps an external deadband),
    but it surprises a DR user: an event's band changes the home's
    cycling for the rest of the run.
  - Better: have the release restore the configured band, since the band
    was part of the event. If parity with OCHRE is chosen instead,
    document it.

**Failure budget, default 1, per-equipment streak.**

- The streak logic is correct: per equipment, reset by success, carried
  across add and replace by id, and checkpointed.
- No legitimate intermittent failure exists in the sweep, the goldens or
  BESTEST.
  - PV at night does not fail: every PV sample stepped January nights
    with 0 rollbacks.
  - The PV failures in the tests come from a solar override that lacks
    the PV surfaces, a configuration error (m6).
- Default 1 is acceptable for every user class:
  - Calibration should set 0 (any failure is a model defect) and check
    `port_rollbacks`.
  - The fleet already isolates a failing home (`SimError::Failed`).
  - The RL gym batch path terminates the episode.
  - HELICS co-simulation ends the federate.
- The gap is m4: nothing makes the run terminal after the error.

**EBM split and negative energies.**

- Correct. OCHRE's EBM (`HVAC.py:626-637`) uses a reference temperature
  (10 °C heating, 30 °C cooling) as a datum, with no sign constraint.
  Energy, min and max are `C x (T - T_ref)`, so a heating setpoint below
  10 °C gives negative energies and an intact window `C x band`. The
  removed `max_e <= 0` check was a false invariant.
- Checking the load-independent window before any state is committed,
  and booking the load last, is the right order. The remaining
  post-commit error is unreachable (i2).

**`delivered_output` on failed steps.** Every power reader follows it:

- the electrical and per-zone consistency sums;
- `DwellingTelemetry` power;
- the next step's battery/EV/PV summary;
- the frame's flow, energy, capacity, COP, power-factor and telemetry
  columns;
- the end-use aggregates (unpinned, m3).

Billing and net grid read the solver total, which excludes the
rolled-back ports. Metrics read the frame. Fleet aggregates read the
frames and step results.

Not covered: Python `equipment()` (i4), and the state columns zeroed by
the frame's `continue` (m5).

## Regressions: verification block (worktree of 9713fe20, `-j 8`)

| line | result |
|---|---|
| `[fmt]` | pass |
| `[clippy]`, default, dst, profiling, observe, observe+profiling | pass |
| `[clippy-release]`, `-release-dst`, `-release-observe` | fail only on `hares-envelope/src/thermal_solver/mod.rs:1858` unused `BoundaryDiagnosticInfo` (the known one); re-run with `--keep-going` shows no other error in any crate |
| `[test-debug]` | 7203 passed |
| `[test-observe]` | 7262 passed |
| `[test-dst]` | 7216 passed |
| `[test-profiling]` | 7209 passed |
| `[test-observe-profiling]` | 1360 passed |
| `[test-release]` | 7160 passed |
| `[bench]` | pass |
| `[golden]` default | consumer_shape_900s, fleet_mixed_offset, full_year_hourly, leap_year_february_900s, month_boundary_billing_3600s, weather_wrap_900s: identical |
| `[golden]` `-F dst` | full_year_900s_dst identical |
| `-F profiling` compare-all | all six identical |
| `[python-build]` | pass |
| `[pytest]` (block line as written) | 1390 passed, 9 skipped, 0 failed; census differs only because the vendored OCHRE is not importable without `PYTHONPATH` (3 "OCHRE not installed" skips in place of the census's 13 OCHRE-module skips) |
| `[pytest]` with `PYTHONPATH=vendors/OCHRE` | 1392 passed, 19 skipped, 3 xfailed, 1 failed; skip census identical. The failure, `test_ochre_parity.py::test_7day_benchmark`, is raised inside the vendored OCHRE's own `Dwelling.simulate()` (pandas `datetime64[us]` overflow at `Timestamp('2262-04-11…')`) in `_run_ochre_7d`, which never touches HARES; neither the test nor the Python environment changed in this branch, so it is environmental and not attributable |
| ASHRAE gate (`uv run --extra ochre scripts/check_ashrae_reference.sh`) | PASS |
| physics suites | hares-core bestest/parity/oracles: 25 passed; hares-physics `physics_validation_tests` and `solar_parity`: 53 passed |
| `[writing]` | all five commit messages and their added lines clean; no history prose in added lines |
| ruff on the eight changed `.py` files | 66 findings at 50a5c261 and at 9713fe20, none on a changed line |

The non-debug counts exclude one scratch sweep test that ran as a no-op.

BESTEST moves: see MJ2. Only 600 and 900 move (600FF, 900FF, 640 and
the oracles are unchanged). The moves come from the 0.1 °C ideal band:
the FSM gates delivery at the band's thresholds instead of at the
setpoint.

## Quality

- One predicate (`validate_thermostat_band_c`) and one signal helper
  (`thermal_setpoint_band_c`) serve:
  - the validator;
  - the FSM;
  - `ThermostatConfig`, the heat pump configs and the water-heater
    configs;
  - the water-heater arms, through `tank_thermostat_update`;
  - the actor, gym and Python constructors.

  The three copies the audit found are gone, and the hysteresis has one
  writer (the FSM) besides the checkpoint restores. The Python HELICS
  and gym guards repeat the range in Python, which is acceptable at a
  language boundary: they read the Rust constants.
- Error types: `ThermostatBand` covers ranges. `Control` covers
  structural signal errors (release with a deadband, crossed axes, a
  water heater's cooling setpoint, a tankless band). That split is
  coherent, apart from m10.
- Public API growth:
  - Justified: the band constants and functions, the
    `ThermostatBand` variant, `SimulationConfig::max_consecutive_step_failures`
    with its Python surface, and `OverrideState::signal`.
  - Could be `pub(crate)`: `EquivalentBatteryWindow` and
    `HvacEquipment::equivalent_battery_window` are `pub` but used only
    inside hares-equipment.
- Hot path: `step_failed` and `consecutive_step_failures` are
  pre-allocated; the per-step `vec![false; n]` is gone.
  `refresh_equipment_ids` allocates a `HashMap` only on add and replace.
- Slop: m9 (misplaced doc), m7 (fallbacks left in a touched file). No
  IDs, no em dashes, no history prose.
