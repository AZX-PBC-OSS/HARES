# Review and red team: OD-16 follow-ups, `fix-od16` at 53c536ba (on 2689f2af)

Commits `071affc9` (DR axis from the target), `575c406b` (water-heater
release restores the setpoint), `3c9134b9` (checkpoint contract on a
terminal dwelling), `3f29ff16` (gym deadband bounds per thermostat class),
`b6c9b734` (IdealHvac reads its zone once), `554c056d` (failed step state
columns, PV irradiance as flow), `d0d8def7`, `f8609384`, `53c536ba`
(docs). They answer `review-OD-16-2689f2af.md` (1 major, 7 minors).

Reviewer: independent (did not write any of the commits). Worktree
`.workstrees/fix-od16` at 53c536ba, `CARGO_TARGET_DIR=target-review-od16`
(deleted at the end). Mutation checks ran in a `git archive` copy of
53c536ba under the worktree's `target/`.

**Verdict: CHANGES-REQUIRED.**

Counts: 0 blockers, 1 major, 5 minors, 5 info.

Every finding of the previous review is fixed at its stated root for the
cases it described, and each fix is pinned by a test that fails without it
(table below). The major is new and lives in the fix for the previous
major: for a unit that serves both axes, the axis is now chosen from its
mode, and on a mode flip the old axis's override is left in place, so the
event ends up moving both setpoints toward each other.

## Findings

### Major

**MJ1. A dual-mode target keeps its first axis's override after its mode
flips, so a "one-axis" pre-conditioning event moves both setpoints, and the
first axis is a guess.**
`crates/hares-core/src/actors/dr_compliance.rs:596` (`axis_of`),
`:614` (`observe_active_modes`), `:775` (`last_dispatched.clear()`),
`crates/hares-equipment/src/hvac/thermostat.rs:588-594` (a one-axis delta
keeps the other axis at `prior`).

The doc on `DrAction::SetpointAdjust` promises that a positive delta moves
"the one setpoint the target serves". For an ideal unit the axis follows
its active, then last active mode. Each dispatch re-sends the delta on the
current axis only, and nothing releases the other axis, so the override
sent while the unit was in the other mode persists as an absolute value
until the event ends. After the first flip both axes are displaced: heating
up and cooling down.

- Probe (scratch crate, BESTEST 600 ideal unit, Denver weather, hourly,
  `DrCompliance` ByName on the ideal unit, `setpoint_delta(2.0)`, event
  active for the whole run, setpoints 20/27 C, deadband 0). Per step I
  compared the unit's operating mode with which setpoint left its schedule
  value:

  | start, length | unit heating / cooling steps | steps with both axes displaced | steps on the wrong axis |
  |---|---|---|---|
  | 2024-01-01, 365 d | 2120 / 2880 | 8747 of 8760 | 1 (the flip) |
  | 2024-04-15, 30 d | 201 / 197 | 683 of 720 | 1 |
  | 2024-10-01, 30 d | 216 / 193 | 717 of 720 | 1 |
  | 2024-07-01, 60 d | 129 / 700 | 1434 of 1440 | 1 |
  | 2024-06-10 05:00, 1 d | 2 / 12 | 22 of 24 | 1 |
  | 2024-08-10 04:00, 1 d | 1 / 11 | 21 of 24 | 1 |

  The wrong-axis count is small because the flip is followed at once; the
  damage is the other column. A pre-cool event that starts after a morning
  of heating holds the heating setpoint 2 C above schedule for the rest of
  the event, so the unit heats whenever the pre-cooled zone dips below
  that. A thermostat band of 7 C is held at 3 C.
- The first axis is a guess. In the 05:00 and 04:00 starts the event began
  on the cooling axis, the unit then heated for 1 to 3 steps in the cool
  morning (the 1 wrong step per row), and then flipped. The no-history case
  falls back to `hvac_axes`, which is `descriptor.end_use` read once at
  `refresh_equipment_caches` (`dwelling/mod.rs:4212`). `IdealHvac` rewrites
  its `descriptor.end_use` every step from the delivered capacity's sign
  (`ideal_hvac.rs:679`), so that "declared" end use is whatever the unit
  last delivered when the actor was registered, and heating at
  construction time.
- Scope. Single-purpose units (furnace, boiler, baseboard, AC, the heat
  pump's heater and cooler) cannot hit it: their end use is fixed and the
  axis equals it. The four ResStock homes the previous review tallied are
  all built from such units (descriptors in
  `hvac/{furnace,air_conditioner,heat_pump/*}.rs`), so the new rule is
  correct for them in every hour by construction, and the CA shoulder-season
  evening is pinned by `dr_preconditioning::a_preconditioning_event_on_an_air_conditioner_precools`
  (passes). I did not rerun the 8760-hour four-home tally, because the
  rule no longer reads the weather and the target never changes axis. The
  case that remains is a dual-mode target: `IdealHvac` (BESTEST, any
  `IdealHVAC` HPXML equipment type, `resolve_hvac.rs:2681`, a custom
  equipment).
- Physics. OCHRE has no dual-mode HVAC at all: `HVAC.py:271-280`
  (`update_external_control`) addresses `{self.end_use} Setpoint (C)` and
  `{self.end_use} Deadband (C)`, one end use per equipment, so the target
  names the axis. EnergyPlus decides heating or cooling from the zone load
  to each setpoint (`OutputRequiredToHeatingSP` / `OutputRequiredToCoolingSP`);
  it has no pre-conditioning direction either, the setpoint schedule
  carries it. The doc comment at `dr_compliance.rs:586-595` cites that
  EnergyPlus mechanism as the basis for "the mode is the outcome of the zone's
  load", but the code does not compute those loads; it follows the last
  observed mode, which lags the load by a step and has no value while the
  unit is in its deadband. For a dual-mode unit the event has no direction,
  which is the root.
- Fix: for a dual-mode target either (a) make the direction part of the
  event (`PreHeat` / `PreCool`, or an axis field on
  `DrAction::SetpointAdjust`), or (b) on every axis change dispatch a
  release for the old axis before the new delta, and read the unit's
  declared capability rather than the mutable `descriptor.end_use`. Add a
  dwelling test with an `IdealHvac` whose mode flips mid-event and assert
  that only one setpoint differs from schedule at every step.

### Minor

**m1. `preconditioning_without_direction` stays at 1 after the event ends
or the actor stops complying.** `dr_compliance.rs:897` is the only place
the value is written; the Normal early return (`:724-742`) and the
not-complying return (`:776-780`) reset `signals_count`,
`signals_rejected` and `dr_freeze_guard` but not this key.

- Repro (scratch test in the archive copy): `DrCompliance` ByName "Lights"
  `setpoint_delta(2.0)`, `decide` at High gives 1.0; `set_dr_level(Normal)`
  then `decide` still reads `Some(1.0)`, expected 0.0
  (`assertion left == right failed: stale after the event ended, left:
  Some(1.0) right: Some(0.0)`).
- Consequence: an observer reading the counter after an event sees a
  target that was ignored in an event that no longer exists.
- Fix: write the key in both early returns, or in one place at the top of
  `decide`. The existing test pins only the in-event value.

**m2. A terminal-state load leaves `step_failed` set.**
`dwelling/mod.rs:5219` clears `terminal_error` and zeroes the ports, which
is correct and necessary (without `ports.zero()`
`a_checkpoint_restore_resumes_an_ended_run` fails, mutation E below). The
restore leaves `step_failed` as it was; it is only cleared at the start of
the next step (`:6060`) and in `refresh_equipment_ids`.

- Repro (archive copy, same fixture as the new test, then
  `load_checkpoint(start)`): `step_failed still flags equipment Some(1)
  after the restore`.
- Consequence: between the load and the next step,
  `reported_core_output(idx)` (`:5743`) and Python `Dwelling.equipment()`
  report the failed equipment's flows as zero although its state was just
  restored. Narrow window and no effect on the stepping; it is the same
  reader class the previous review's i3 documented. The same applies to
  any `load_checkpoint` after a tolerated failure, so it predates this
  commit; the commit's claim that the restore "drops the half-stepped
  state" is nearly complete, not complete.
- Fix: `self.step_failed.fill(false)` next to `ports.zero()`, in every
  load (not only the terminal one), and pin it.

**m3. The `equipment_zip` doc is now the doc of `thermostat_band_range`.**
`crates/hares-python/src/py_dwelling.rs:1043-1063`: the new method and its
three doc lines were inserted between the last line of the `equipment_zip`
doc block and `pub fn equipment_zip`. `help(Dwelling.thermostat_band_range)`
shows the ZIP documentation with the band lines appended at the end, and
`equipment_zip` has no doc. Same class as the previous review's m9.
Move the new method above the doc block (or below the `equipment_zip`
body).

**m4. The Rust `batch_step` looks the target's band up on every step, and
the vector env now refuses a mixed fleet.**
`py_gym.rs:442` builds a closure per dwelling per step, and
`action_bounds` (`:88`) calls `band_range_of` (`py_dwelling.rs:2123`: the
dwelling mutex, then a linear scan with a string compare per equipment) for
each `deadband_c` field of each dwelling on each step. The Python gym and
vec env compute the bounds once at construction and clip with the action
space, and `vec_env` already clips every row before calling `batch_step`,
so the Rust lookup is redundant on that path. It allocates nothing and runs
before the GIL is released, but it is the per-step work the hot-loop rule
rules out, and it only exists on this one of the three paths. Resolve the
(low, high) per layout column once, and clip with it.
`VecDwellingGymEnv` (`vec_env.py:76-85`) raises at construction when
dwellings of the fleet have different bounds for one action field, so one
home with an ideal unit and one with a furnace cannot share an env that
acts on `deadband_c`. That is a sound refusal, but no test builds it (see
m5).

**m5. Test gaps that leave the new behaviour unpinned.**
- No test drives a dual-mode target through a mode flip (MJ1).
- `vec_env` bounds per dwelling and the heterogeneous-fleet error have no
  test; `test_gym_batch_step.py` has no deadband case, so the Rust
  `batch_step` bound through a real dwelling is covered only by the stub
  `bands` closure in `py_gym.rs` unit tests plus the shared
  `band_range_of`.
- `step_failed` after a load (m2) and the telemetry reset (m1).
- The release on a ramped water heater (resistance, HPWH, indirect with
  `setpoint_ramp_rate`) restores the target and lets the setpoint ramp
  (`resistance.rs:921-930`); the shared contract test runs unramped.

### Info

- **i1. `hvac_axes` is a snapshot of a mutable field, and
  `last_active_axis` is keyed by name and never pruned.** Part of MJ1's
  root. A `replace_equipment` that keeps a name but changes the unit's kind
  inherits the old name's last axis, which wins over the new unit's end
  use (`axis_of`, `:596-611`). The name-keyed `Vec<(Arc<str>, _)>` is
  searched linearly for every HVAC unit on every `decide`, including steps
  with no event (`observe_active_modes` runs before the Normal check). It
  allocates only on the first sighting. Storing the axis in the
  `hvac_axes` entry (id-keyed) removes both the scan and the name
  dependency, while the checkpoint can stay name-keyed.
- **i2. A target with no direction logs a `tracing::warn!` on every step of
  the event** (`dr_compliance.rs:537`). The counter is the durable signal;
  the log repeats it per step.
- **i3. `ThermostatAxis::of_mode` duplicates the heating list of
  `OperatingMode::is_heating_variant`** (`equipment.rs:251`), with a
  narrower set on purpose (`HeatPumpWH`, `BackupElement` and EV `Heating`
  are not thermostat modes). A one-line comment of why would hold it; I
  did not find a wrong mapping.
- **i4. A positive delta on a target with no direction is dropped, while
  a negative one is sent to any target.** `relaxation_widens_both_axes`
  runs on a "Lights" target. Consistent with the docs; the asymmetry is
  intended and counted in telemetry only for the positive case.
- **i5. Shipped text.** Added lines and commit messages in the nine
  commits: no em or en dashes, ticket identifiers, agent references,
  `V8`-style tokens or co-author lines, except `071affc9`: its added line
  `actor.rs:79` (`/// Default: no-op — actors that do not depend ...`)
  has an em dash. `53c536ba` rewrites it, so the tip is clean, and no other
  commit in the range fails. The two known failures of the earlier range
  (`55854486`, `071affc9` of the previous list) are outside or fixed:
  `55854486` is before this range, and its `V8` line is reworded in
  `d0d8def7` ("Verbosity-8 per-equipment telemetry columns",
  `dwelling/mod.rs:445`).

## Earlier findings: status and pin

| finding | status | evidence |
|---|---|---|
| MJ1 (axis from weather) | fixed for every single-purpose target and `ByEndUse`; residual for dual-mode targets is MJ1 above | `preconditioning_moves_the_axis_the_target_serves`, `dr_preconditioning` AC shoulder evening and furnace January pass; mutation A below |
| m1 (water-heater release) | fixed for gas, resistance, HPWH, indirect and tankless, one shared `TankThermostatUpdate` (`water_heater/mod.rs:292-330`) and one shared contract test (`assert_tank_thermostat_contract`) | mutation B below fails the 4 shared-contract tests and the tankless test |
| m2 (terminal checkpoint) | `load_checkpoint` resumes, `restore_building_state` refused with the ending error; ports zeroed; `step_failed` left (m2 above) | mutation C and E below |
| m3 (gym band space) | fixed: per target class in the gym, vec env and `batch_step` (`m4`, `m5` above for the residual) | 53 passed in `test_gym_env.py`, 2 in `test_gym_batch_step.py`, 37 `py_gym` unit tests |
| m4 (tank restore check) | fixed and pinned inside the shared contract (`load_state` with a 11 C band must fail) | mutation D below |
| m5 (IdealHvac zone) | fixed by removal: one `lookup_zone_temp` in `update_mode`, then `ThermostatFsm::update_mode_at` (`thermostat.rs:396-406`); `update_mode` is the thin wrapper, `hvac_core.rs:895` and `heater.rs:2215` use it, nothing duplicated | `an_ideal_unit_reports_the_ideal_band_class` is trivial but real |
| m6 (state columns) | fixed: the test pins return temperature, minimum off time, supply air temperature, EV connection state and level, and PV irradiance is a flow | PV irradiance classification: `pv/mod.rs:1231` writes the mean absorbed irradiance the step computes |
| m7 (writing) | fixed by `d0d8def7` | grep |
| i3, i4 | documented (`invariants-and-observability.md`, `pv/config.rs:72-77`) | read |

**PV irradiance as a flow.** The change only alters what a failed step
records at verbosity 8 (`resolve_v8_columns`, `dwelling/mod.rs:884-891`);
a successful step is unchanged, so goldens and parity columns do not move.
No consumer in `HARES_CALIBRATION_SYSTEM` reads a PV irradiance column
(grep of `IRRADIANCE`, `irradiance_w_m2`). A zero irradiance in a failed
step is indistinguishable from night in the frame; the documented
`equipment_core` and `Equipment.telemetry()` readers keep the last value.
The frame test pins the PV irradiance and DC power flows at `[640, 0]` and
`[0.2, 0]`. Not run: the golden comparison (no code path on a successful
step changed).

**The `resolve_equipment_descriptors` hook.** Called from
`refresh_equipment_caches` only (`dwelling/mod.rs:4212-4215`), on the same
refreshes as `resolve_equipment_id`: construction, `add_equipment`,
`replace_equipment`, removal, `add_actor`. It builds one
`Vec<&EquipmentDescriptor>` per refresh, off the step path, and each
`DrCompliance` allocates its `hvac_axes` per refresh. The default no-op
cannot hide a missed override for the one actor that needs it: without the
call, `hvac_axes` stays empty, `observe_active_modes` sees nothing, and
every named target has no direction, which the dwelling-level tests
(`dr_preconditioning`) catch. Python-defined actors (`py_actor.rs`) cannot
override it, so a Python actor that wants end uses must read the
descriptors itself. The other actors with name bindings (`bms.rs`,
`ev_driver/mod.rs`) read only ids, and need nothing.

**Checkpoint of the last active mode.** `save_state` / `load_state`
(`dr_compliance.rs:948-983`) carry the name-keyed axes; the new tuple
format has no old-format fallback, which matches the greenfield policy. A
restored actor reproduces the idle-unit axis
(`a_dual_mode_target_preconditions_in_its_active_or_last_mode` restores
and checks it). `hvac_axes` is derived, so it is not saved.

## Mutation checks (reproduced in `git archive` copies of 53c536ba)

| # | mutation | result |
|---|---|---|
| A | `axis_of` ignores `last_active_axis` | `a_dual_mode_target_preconditions_in_its_active_or_last_mode` fails |
| B | `TankThermostatUpdate::setpoint_c` ignores the release | 4 `thermal_setpoint_band_follows_the_shared_contract` and `tankless::the_release_restores_the_configured_setpoint` fail (`left: (57.0, 2.0) right: (52.0, 2.0)`) |
| C | `load_checkpoint` no longer clears `terminal_error` | `a_checkpoint_restore_resumes_an_ended_run` fails |
| D | `restored_tank_deadband_c` skips the validation | the 4 shared-contract tests fail (`unwrap_err` on an `Ok`) |
| E | `terminal_error` cleared but `ports.zero()` dropped | `a_checkpoint_restore_resumes_an_ended_run` fails, so the zeroing is pinned |

Five mutations, five detected; the implementer's claim holds. The two
added probes of mine (m1, m2) failed as written in the archive copy and
prove those findings.

## Verification run (worktree at 53c536ba)

| line | result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy -p hares-core -p hares-equipment -p hares-types --all-targets -D warnings` | pass |
| `cargo clippy -p hares-python --all-targets -D warnings` | pass |
| `cargo test -p hares-types -p hares-control -p hares-equipment -p hares-core -p hares-io -p hares-fleet` (includes BESTEST 600/640/900, `conditioned_oracle`, `freefloat_oracle`, `dr_preconditioning`) | 5880 passed, 0 failed |
| `cargo test -p hares-python --lib -- py_gym` | 37 passed |
| `pytest tests/python/test_gym_env.py` and `test_gym_batch_step.py` (worktree venv; its `_hares` has `thermostat_band_range`) | 53 passed, 2 passed |

Not run: the golden frame comparison, release and feature-matrix test
profiles, the full pytest suite, the ASHRAE gate. I did not rebuild the
Python wheel; the venv's build exposes `thermostat_band_range`, so it is at
least `3f29ff16`, but I could not prove it is the tip.

## Consumer: `~/src/HARES_CALIBRATION_SYSTEM` (`i08-consumer` at `b71dbb4`)

The previous review's list stands. Changes at this tip, in order:

1. `calibration/ami2/dr_actuation.py:206`:
   `thermal_setpoint(None, None, 0.0)` raises at import. Use
   `thermal_setpoint_release()`. Rewrite the comments at `:175-205`, the
   docstring at `:12-38`, and the `:254` note ("Not `thermal_setpoint_delta(0.0)`").
2. `calibration/tests/suite_ami2.py:877` and `:1762`: same release
   constructor. Fix the comments at `:938-939` and `:1802-1804`.
3. `calibration/tests/suite_ami2.py:1760`: the pre-event
   `thermal_setpoint_delta(2.0, -2.0)` converges the setpoints. Send the
   unit its own axis: `(2.0, None)` to heating equipment, `(None, -2.0)` to
   cooling equipment, chosen by the equipment's end use. Never one pair to
   every unit.
4. `calibration/ami2/dr_actuation.py:248`:
   `thermal_setpoint_delta(d, -d)` is valid for `d <= 0` only. Assert the
   sign. The same one-pair-for-all-units loop (`for e in hvac`) needs the
   per-end-use axis.
5. If the consumer moves the pre-condition to the engine's `DrCompliance`:
   target by end use or by a single-purpose unit's name; for an ideal
   unit do not rely on the engine's axis (MJ1). Read
   `preconditioning_without_direction` from the actor telemetry, and
   remember it stays stale after an event (m1).
6. After every actuated run assert
   `dw.health().rejected_control_signals == 0` (rejections are counted,
   not raised).
7. `calibration/ami2/functions/_flexibility.py:219-221`: the note "The
   release must carry `deadband_c=0.0`" is wrong; point it at
   `thermal_setpoint_release()`.
8. `calibration/coupling.py:408,420` and `calibration/cosim.py:90,102`
   (`load_state` rewinds): this changed at this tip. After the
   failure-budget error `load_state` now resumes the dwelling, so a
   rewind-and-retry loop will repeat the same failing step until the
   budget is hit again. Treat the budget error as fatal for the home, or
   change the input before retrying. Run with
   `max_consecutive_step_failures = 0` and check `port_rollbacks`.
   `restore_building_state` is not called anywhere in the consumer.
9. A water-heater release now restores the configured setpoint as well as
   the band; the consumer's `dr_actuation.py` actuates only HVAC, so no
   code change, but any water-heater event the fold adds needs
   `thermal_release()` at the end and not a manual setpoint reset.
10. `ochre_next.rl` is not used by the consumer, so the gym, vec env and
    `batch_step` band change needs nothing there. No consumer reads a PV
    irradiance column or `MAX_THERMOSTAT_BAND_C`. PV: `der_inject.py:83-94`
    already writes `ArrayTilt` and `ArrayAzimuth`; no change.

## Cleanup

`target-review-od16` and the scratch copies under the worktree's
`target/` (`mut/`, `scratch-probe/`, review logs) are deleted. No file
under the worktree's tracked tree or the main tree was edited.
