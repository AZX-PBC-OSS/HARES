# Red team: OD-16, engine fe804b59 (parent 1c18543a)

Red team: independent (did not implement, review or audit). Worktree of the
commit (detached, vendors/OCHRE initialized), scratch tests only, removed
afterwards with its target directory. The audit's findings
(`audit-OD-16-fe804b59.md` M1, M2, m1-m5) are not repeated here.

**Verdict: ATTACK-SUCCEEDED.**

Counts: 1 blocker, 4 major, 5 minor.

## Findings

### Blocker

B1. A named deadband that is positive but below the EBM's tolerance passes
every new check. It collapses the equivalent battery window, and the
reconciled tolerance turns that into a silent HVAC outage that lasts the
rest of the run.

All three new guards test only `db <= 0.0` (or `== 0.0`). The EBM
invariant (`equivalent_battery.rs`, `deadband_range <= f64::EPSILON`) fires
whenever `capacitance * hysteresis` is at or below 2.2e-16 kWh. Below about
1 ulp of the setpoint (~3.5e-15 C) the thresholds are also bitwise equal.

- Repro, equipment (scratch test in `hvac_tests.rs`): Electric Furnace with
  capacitance 2.0 kWh/K. `apply_control(ThermalSetpoint{Some(21.0), None,
  Some(db)})`, then a heating step at 18 C.
  - For db in {1e-17, 1e-300, 5e-324 (subnormal), f64::MIN_POSITIVE, 1e-16,
    2e-16, 1e-15}, the validator returns Ok and `apply_control` returns Ok.
    The step then fails with `EBM invariant violation: max_energy_kwh=22
    equals min_energy_kwh=22`.
  - db = 1e-12 is the first tested value that steps.
- Repro, dwelling (scratch test in `dwelling/mod.rs` tests): bestest
  `s54_heat_pump.toml` (electric furnace, -5 C outdoor). Run 30 steps, then
  `apply_control_validated("Electric Furnace", ThermalSetpoint{Some(21.0),
  None, Some(1e-17)})`, then 720 steps (12 h).
- Result: every step returns `Ok(())`. `health.port_rollbacks` = 720 and
  `rejected_control_signals` = 0. The zone falls from 19.43 C to **0.35 C**
  and the HVAC delivers 0 W the whole time. The only trace is the warning log.
- The same thing happens with the signal applied before the first step: 3/3
  steps are Ok and 3 rollbacks are counted.
- The EBM error text is wrong in this case. It says "The thermostat
  hysteresis is 0 ... rejected for deadband_c 0 at application, so check the
  configured hysteresis_c". In fact the hysteresis is 1e-17 and it came from
  a signal, not from config.

The fix's consistency-sum reconciliation is what makes this silent. Before
it, the stale `core_output` of a failing HVAC that had been running made the
next step fail loudly. Now both sums skip the failed equipment, and the
tolerance policy has no failure budget. So no persistent equipment failure
of this kind can ever end the run (see also M2).

Fix:
- Give the deadband a physical lower bound, enforced in ONE shared predicate
  used by the validator, the FSM and IdealHvac. For example, a minimum
  hysteresis well above the f64 resolution of a setpoint (0.01 C or a
  similar instrument resolution). Alternatively, reject when `C * db` would
  breach the EBM tolerance.
- Correct the EBM error text.
- Pin it with a test at a subnormal value and at 1e-16.

### Major

M1. A configured `hysteresis_c = 0` still collapses the window, and in a
dwelling it is the same silent outage as B1.

`ThermostatConfig::validate` accepts `hysteresis_c >= 0.0`
(`thermostat.rs:53`). The typed configs expose `hysteresis_c` (for example
`HeatPumpCommonConfig.hysteresis_c`, `CentralAirConditionerConfig`).

- Repro (scratch): ASHP Heater with `hysteresis_c: Some(0.0)` and
  capacitance 2.0, then a heating step at 15 C.
- Result: `init` returns Ok, and the heating step fails with the EBM
  invariant. The commit's own EBM message now points users at this input,
  but nothing rejects it.

Fix: reject a zero (or sub-resolution) hysteresis at config validation
whenever the EBM gate can open (positive zone capacitance), with the same
shared predicate as B1.

M2. A release with `deadband_c: None` is not a pass-through either. It
installs a frozen cooling override whenever the base gap is at or below
`2 * hysteresis`, and init's own reconciliation produces exactly that gap.

This is a different trigger from audit M1, which needs a positive deadband
at or above the gap. Here the FSM's deadband constraint uses
`2 * hysteresis` for any signal without a deadband (including the release),
and the comparison is `<=`. `reconcile_for_deadband` at init widens any
narrow configured gap to exactly `2 * hysteresis`, so every such thermostat
is caught.

- Repro (scratch): Electric Furnace configured at 22.5/23.5 C (reconciled at
  init to 22/24), then `ThermalSetpoint{None, None, None}`.
- Result: `apply` returns Ok and the cooling setpoint becomes **25.0** (a
  runtime override, frozen against later schedule changes until the next
  signal). Base 20/22 with the same release gives cooling 23.
- With `deadband_c: Some(0.0)` the same thermostat stays at 24 (no
  override). The release's effect therefore depends on whether the consumer
  sends None or 0.0.
- ResStock homes with equal or near-equal heating and cooling setpoints hit
  this.

Fix: for a signal that names no setpoint, skip the deadband constraint and
the auto-correct entirely. This is audit M1's fix, but it must cover the
`None` deadband too, and the test must include `None`.

M3. The hysteresis a named event writes is not checkpointed for Electric
Furnace, Gas Furnace, Electric Baseboard or the boilers. A restore across a
named-then-release sequence diverges from the continuous run.

`FurnaceState`, `BoilerState` and `BaseboardState` carry no
`thermostat_hysteresis_c`. AC, HP heater and IdealHvac do.

- Repro, equipment (scratch): named `ThermalSetpoint{Some(21), None,
  Some(0.25)}`, release `{None, None, None}`, step, `save_state`, then
  `load_state` into a fresh instance.
- Result, equipment: for all three types the EBM window is 0.5 kWh in the
  continuous run and 2.0 kWh in the restored run, so the hysteresis falls
  back from 0.25 to the configured 1.0.
- Repro, dwelling (s54): the same sequence, then `save_checkpoint` and 240
  steps compared against `load_checkpoint` into a fresh dwelling plus 240
  steps.
- Result, dwelling: zone temperature max |dT| = **1.07 C**, first divergence
  at step 11.
- This predates the commit, but it is squarely on the release path this
  entry claims to make safe. The existing
  `checkpoint_restart_produces_identical_continuation` does not send a
  named deadband.

Fix: persist the thermostat hysteresis in every HVAC state struct (bump the
checkpoint versions) and add a named-then-release restore-equivalence test.

M4. A deadband-only override is now silently dropped. Before this commit it
applied, and the actor APIs still advertise it.

`ThermalSetpoint{None, None, Some(x)}` is classified as the release form, so
`deadband_c` is ignored with no error and no warning. These producers still
build exactly that signal:
- `OverrideState::is_active()` counts `deadband_c.is_some()` as active, and
  `OverrideState::none().with_deadband(1.5)` (doc: "Adds a deadband
  override") makes `IdealThermostat` emit the release form every step.
- `PyDispatchRequest.thermal_setpoint(target, deadband_c=x)` and
  `PyControlSignal.thermal_setpoint(deadband_c=x)` do the same from Python.

A user who asks for a deadband change gets nothing, silently. This
contradicts the no-silent-defaults rule.

Fix: either reject a release form that carries a deadband (typed error), or
remove deadband-only construction from the actor and Python APIs. Then
document the single release shape (all three `None`).

### Minor

m1. The gym action space disagrees with the contract.
- Both `py_gym.rs:79` and `gym_env.py:208` bound `deadband_c` to
  `(0.0, 30.0)`. Clipping to the lower bound yields `0.0`. With a named
  setpoint, this commit turns that into a rejected signal, and the
  setpoints in it are dropped too (surfaced only in `info["warnings"]`).
- Values in (5, 30] were already always rejected.
- The HELICS boundary guard (`helics/dwelling.py`) mirrors the setpoint
  ranges but has no deadband guard. Its own comment says mismatched ranges
  give inconsistent diagnostics.
- Fix: bound the action at the validator's range with a strictly positive
  floor (the B1 minimum), and add the deadband to the HELICS guard.

m2. A named partial form is silently rewritten (predates the commit; it is
the partial-form attack's result).
- Repro (scratch): ASHP, `ThermalSetpoint{None, Some(21.0), None}` and
  `{None, Some(21.0), Some(2.0)}`.
- Result: `apply` returns Ok and the effective cooling setpoint is **23.0**,
  not 21. The auto-correct raises cooling to heating + deadband + 1 instead
  of honouring the named axis.
- A pre-cool event is therefore defeated. Only the first violation per
  equipment logs a warning.
- Fix: when only one axis is named, move the unnamed axis (or reject),
  never the named one.

m3. "The rollback is an atomic swap, no partial state" holds for the ports
only, not for the equipment.
- A failing HVAC step mutates internal state before the EBM error.
  Baseboard (`baseboard.rs` step), for example, adds `run_time_s`, sets
  `ELECTRIC_KW`/`THERMAL_OUTPUT_W` telemetry and resolves the operating
  mode before `compute_and_write_ebm_telemetry` returns Err.
- `save_state` reads `ELECTRIC_KW`/`THERMAL_OUTPUT_W` back from telemetry.
  A checkpoint taken after a tolerated failure therefore persists the
  failed step's unbooked power into the restored `core_output`.
- Code-read evidence; the consequence class extends audit m4.
- Fix: compute the EBM before the equipment commits any state, or snapshot
  and restore equipment state on rollback.

m4. `ExecutionStage::EnvelopeResolution` equipment is never stepped.
- None of the three stage loops handles it, so its `step_succeeded` stays
  `false` forever. "Skipped by the scheduler" and "failed and rolled back"
  are conflated by the new filter.
- The stage is reachable from Python (`PyExecutionStage::
  EnvelopeResolution`).
- Predates the commit; code-read. Fix: step it, or reject it at
  registration.

m5. The test helper `replace_equipment_for_test` does not seed
`latest_env.equipment_core`.
- A `FlakyPortEquipment` that fails on its first step through this helper
  fails every later step with `equipment_core missing entry ... at start of
  step 1`.
- Through the real `add_equipment` path the same equipment is tolerated
  (5 Ok steps, 5 rollbacks), so this is a test-infra gap, not a product
  defect.
- Any future first-step-failure test written with the helper would fail
  for the wrong reason. Fix: have the helper run the same seeding as the
  refresh.

## What held

- Validator, FSM and IdealHvac reject a named `-0.0` (IdealHvac: `got -0`).
  NaN and inf are rejected by the validator's range check before either
  arm runs.
- Release `{None, None, Some(0.0)}` sent during ASHP heating with the
  defrost telemetry active: accepted, and the next step's window is 2.0 kWh
  (hysteresis untouched).
- Release `{None, None, Some(0.0)}` and `{None, None, Some(5.0)}` on
  IdealHvac: accepted. IdealHvac builds no EBM, so there is no window to
  collapse.
- Tolerated failures, both consistency sums:
  - Consecutive failures (720 in a row, B1 repro) and a failure on the
    first step (signal before step 0; `add_equipment` with zero successes)
    never fire the electrical or per-zone thermal check.
  - A failure of the only HVAC on its zone (s54) passes the per-zone check
    end to end. This exercises the filter audit m1 found untested, though
    not as a revert-proof pin.
  - Billing and net grid read the solver total, which excludes the
    rolled-back ports, so they agree with the filtered sums.
- `step_succeeded` and `hvac_thermal_consistency` both index
  `self.equipment`, so several HVAC units on one zone are filtered
  per unit.
- Every Rust producer of `ThermalSetpoint` reaches equipment through
  `apply_control` or `apply_control_unchecked`, and both run
  `validate_numeric_bounds`:
  - the dispatch path, `apply_control_validated` and gym `step_batch`;
  - the HELICS `ControlSignal.from_dict` and the Python actor
    `DispatchRequest`;
  - `DrCompliance` (always `deadband_c: None`), `IdealThermostat` and OCHRE
    compat (always None).
- Goldens, `frame_golden compare-all` (release, `-j 8`):
  - default: consumer_shape_900s, fleet_mixed_offset, full_year_hourly,
    leap_year_february_900s and weather_wrap_900s all identical;
  - `-F dst`: full_year_900s_dst identical.

## Not run

The Python suites and the consumer gate: no maturin build in this worktree.
The Python-path findings (M4, m1) are from code reading plus the Rust
engine paths those bindings call.
