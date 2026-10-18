# Review and red team: OD-16 control-signal contract, `fix-od16` at 2fb9f63b (on 53c536ba, on 50a5c261)

Commits `34323f70` (an event names its axis, one axis displaced, release on
an axis change, `thermostat_axes()`, `Actor::validate_equipment`),
`c895352f` (a checkpoint load clears `step_failed`), `a78689f8`
(`batch_step` takes `action_bounds`), `5b66c7d3` (ramped water-heater
release test), `440cc540`, `21fdc032`, `2fb9f63b` (format, call sites,
imports). They answer `review-OD-16-53c536ba.md` (1 major, 5 minors, 5
info).

Reviewer: independent (did not write any of the commits). Worktree
`.workstrees/fix-od16` at 2fb9f63b, `CARGO_TARGET_DIR=target-review-od16`
(deleted). Probes ran in scratch crates and mutations in a `git archive`
copy of 2fb9f63b, both under the worktree's `target/` (deleted). The fold
ran in a scratch worktree at 2f85338d (deleted).

**Verdict: CHANGES-REQUIRED.**

Counts: 0 blockers, 1 major, 4 minors, 6 info.

The previous major is fixed at its root: for a dual-mode unit the axis is
named by the event, so one axis is displaced on every step of a year, in
both directions, through a checkpoint restore and a same-kind replacement.
The new contract is not closed, though: "an event that cannot be served is
refused when the actor is registered" holds only at the instant of
`add_actor`. Adding the target later, replacing it, supplying the actor
with its equipment, or misspelling its name leaves the event silently
skipped (MJ1 below, reproduced).

## Findings

### Major

**MJ1. A pre-conditioning event that cannot be served is refused only at
`add_actor`; every other path accepts it and the event never fires, with
no rejection counted.**
`crates/hares-core/src/dwelling/mod.rs:3407` (the only call of
`validate_equipment`), `crates/hares-core/src/actors/dr_compliance.rs:487`
(`resolve_axes`: `preconditioning_axis(..).unwrap_or(None)` turns the same
refusal into a silent `None` on every identity refresh),
`dr_compliance.rs:1055-1057` (a `ByName` target with no such equipment
returns `Ok(None)`: accepted), `dr_compliance.rs:610-612` (the only
runtime signal: one `tracing::warn!` per event whose text, "the target is
not in the dwelling", is false when the unit is in the dwelling),
`dr_compliance.rs:418,955` (the counter is called
`preconditioning_without_direction` but counts "not in the dwelling" as
well). The doc on `DrAction::SetpointAdjust` (`:205-217`) promises the
refusal without the qualification.

Reproduced on 2fb9f63b (BESTEST 600 ideal unit, Denver, hourly, a
`DrCompliance` ByName with `AlwaysComply`, event level Moderate for the
whole run; "moved" is whether the heating or cooling setpoint left its
schedule value; `rejected` is `health().rejected_control_signals`):

| scenario | add / step result | steps | setpoint moved (heat, cool) | rejected |
|---|---|---|---|---|
| actor `setpoint_delta(2.0)` added after the ideal unit was removed, then the ideal unit re-added with `add_equipment` | both `Ok`, nothing refused | 500 | (false, false) | 0 |
| baseline: same actor added with the unit present | refused (`PreconditioningAxis`) | | | |
| `precool(2.0)` on the unit (control) | `Ok` | 8759 | (false, true) every step | 0 |

On the fold the same holds for the new roster paths: `add_equipment`
after the actor, `add_equipment_with_actors(unit, [direction-less DR
actor])` (`roster.rs:479`, accepted), and a misspelt target name
("Idael HVAC", precool): all `Ok`, 300 steps, nothing moved,
`rejected_control_signals` 0. A `replace_equipment` that swaps a
single-purpose unit for an ideal unit under the same name goes the same
way by code (`replace_equipment` never calls `validate_equipment`; the
re-resolve in `refresh_equipment_caches` hits `unwrap_or(None)`); I did
not run it, because I could not build a same-named ideal replacement for a
furnace in a scratch crate without the full ResStock fixture.

Consequence: an experiment that moves the unit's setpoint reports a
pre-conditioning response of exactly zero, and the only trace is a telemetry
column and one log line. This is the failure mode MJ1 of the previous
review had (a confident number from a contract that did not hold), with
the opposite sign.

Fix: validate where the roster is planned. `plan_roster_caches` already
receives the prospective equipment and the prospective actors
(`roster.rs:1019`), so `validate_equipment` belongs there: `add_actor`,
`add_equipment`, `replace_equipment`, `add_equipment_with_actors` and the
tariff rebuild then refuse atomically, and the actor never holds a target
it cannot serve. For a named target that matches nothing, choose one
behaviour and test it: refuse (the actor must be added after its
equipment), or accept and count each undelivered step in
`rejected_control_signals` so `health()` shows it. Rename the counter to
what it counts, and make the warning say why. Add tests for add after the
actor, replace-in-place with another kind, supply with the equipment, and a
misspelt name.

### Minor

**m1. A `ByEndUse` pre-conditioning target on a dual-mode unit counts a
rejected control signal on most steps.**
`crates/hares-control/src/dispatch.rs` (`ByEndUse` expansion matches
`descriptor.end_use`), `crates/hares-equipment/src/hvac/ideal_hvac.rs:679`
(the unit rewrites its end use from the sign of the delivered capacity).
Probe on 2fb9f63b, the same unit, `setpoint_delta(2.0)` through
`ByEndUse(HVAC_COOLING)`: 1322 rejected signals in 2000 steps (the unit
heated 615 and cooled 474 of them); through `ByEndUse(HVAC_HEATING)`: 654.
The setpoint is still displaced on 1987 and 1999 of those steps (the
override persists), so the physics is right and the health counter is not.
Any consumer that asserts `rejected_control_signals == 0` after an
actuated run (consumer list item 5) fails on such a run. On 2f85338d the
unit labels its end use by its thermostat call before dispatch
(`route_end_use_by_mode`), which probably narrows this; I did not rerun
the probe there. Document it, or deliver an end-use signal to a dual-mode
unit whatever its current label.

**m2. `batch_step` trusts `action_bounds` completely.**
`crates/hares-python/src/py_gym.rs:225-226`:
`raw_value.clamp(low, high)` panics when `low > high` or either is NaN
(`f64::clamp` documents it), so a bad pair from a direct caller of the
public `batch_step` aborts with a `PanicException` instead of a
`ValueError`. The only length check is `action_bounds.len() ==
action_layout.len()` (`:207`), and there is no test of a bad pair. The
bounds are also one list for every dwelling in the call, where the removed
Rust code looked each dwelling's band up; the heterogeneous-fleet refusal
exists only in `VecDwellingGymEnv` (`vec_env.py:76-85`), so a direct caller
with a mixed fleet is clipped to the wrong thermostat class and gets
counted rejections. Validate each pair (`low <= high`, finite) and say in
the docstring that the bounds are per call, not per dwelling.

**m3. The `register_pv_surfaces` doc now documents `actor_equipment`.**
`crates/hares-core/src/dwelling/mod.rs:1486-1499`: the new helper was
inserted between the three-line doc of `register_pv_surfaces` and its
`fn`, so rustdoc attaches that doc to `actor_equipment` and
`register_pv_surfaces` has none. The same class as the previous review's
m3 (the `equipment_zip` doc), which this range fixed in `py_dwelling.rs`
and then repeated here. Move the helper above the doc block.

**m4. Test gaps that leave new behaviour unpinned.**
- No test builds the cases of MJ1 (add after the actor, replace in place,
  supply with equipment, misspelt name).
- `thermostat_axes()` defaults to `None` and nothing checks it against
  the types that have a thermostat. I read every `impl Equipment` (below)
  and every HVAC type is declared correctly, but a new HVAC type that
  forgets the method would only fail at a by-name `add_actor`, and never for
  a by-end-use target. A test that builds each registered HVAC type and
  asserts `thermostat_band_class().is_some_and(|c| c != Tank) ==
  thermostat_axes().is_some()` closes it.
- The ramped release is pinned for `ResistanceWH` only
  (`resistance.rs:1466`). `HeatPumpWH` has the same ramp branch
  (`heat_pump_wh.rs:1421`) and is unpinned.
- No test for a bad `action_bounds` pair (m2).

### Info

- **i1. `thermostat_axes()` is complete for every thermostat.** From the
  full `impl Equipment` grep over `hares-equipment` and `hares-core`
  (test doubles excluded): `One(Heating)`:
  `ElectricBaseboard`, `ElectricBoiler`, `GasBoiler`, `ElectricFurnace`,
  `GasFurnace`, and `HeatPumpHeaterCore` (so `ASHPHeater`,
  `MinisplitHeater`, `GshpHeater`, `WshpHeater` through
  `delegate_equipment!`, `macros.rs:109-115`; dual-fuel backup lives inside
  the heater core, one unit). `One(Cooling)`: `AirConditioner`, `RoomAC`,
  `HpCooler`, `GshpCooler`, `WshpCooler` (each forwards to its inner
  `AirConditioner`). `Both`: `IdealHvac`. `None`: the five water heaters
  (resistance, gas, heat pump, tankless, indirect): correct, since none
  advertises `THERMAL_SETPOINT_DELTA` and a `ThermalSetpointDelta` is not
  applied to them; a tank setpoint is moved by `AbsoluteSetpoint`, which the
  trait doc ("a setpoint a demand-response event can move") does not say.
  Dehumidifier, ventilation, generators, batteries, EV, PV, loads: no
  thermostat. Heater and cooler cannot share a name: duplicate names are
  rejected at build (`mod.rs:2687`), `add_equipment` (`:3777`) and
  `replace_equipment`, so `ByName` resolves to exactly one unit.
- **i2. Python cannot read a unit's axes.** `thermostat_axes` is not in
  `_hares.pyi`; a consumer chooses `preheat` or `precool` from the unit's
  end use or type.
- **i3. `preheat(-x)` and `precool(-x)` ignore the direction** and relax
  both axes (`dr_compliance.rs:609-615`, the negative-delta arm). It is the
  documented relaxation, and valid for any unit.
- **i4. The HELICS boundary check holds a deadband to
  `MAX_TANK_THERMOSTAT_BAND_C` for every equipment class**
  (`helics/dwelling.py`, `_validate_thermal_setpoint_band`, added before
  this range). An HVAC deadband of 20 C passes the HELICS check and is
  rejected by the equipment (counted).
- **i5. Two parallel vectors, one `zip`.** `save_checkpoint`
  (`mod.rs:5001`) zips `equipment` with `consecutive_step_failures`, and
  `refresh_equipment_ids` (`:5711`) is the only place that keeps the two
  the same length. A desync would silently drop equipment states from the
  checkpoint. It is safe on this branch; on the fold, the first resolution
  broke it and `save_checkpoint_propagates_equipment_error` returned `Ok`
  (fold item 12). Store the streak with the equipment record, or assert the
  lengths.
- **i6. The tank-setpoint and `DrCompliance` state layouts changed with no
  per-actor schema bump** (`checkpoint_version()` stays at its default).
  Greenfield, and the fold bumps `CHECKPOINT_VERSION`; noted only so the
  fold does not read it as an oversight.

## Earlier findings: status and evidence

| finding | status | evidence |
|---|---|---|
| MJ1 (dual-mode keeps its first axis) | fixed at the root: the event names the axis (`DrAction::preheat` / `precool`, `SetpointAdjust:<d>:PreHeat|PreCool`), a dual-mode target without one is refused, and a change of axis releases the old one first | year probe below |
| m1 (stale counter) | fixed (`dr_compliance.rs:789,829`) | mutation 1 |
| m2 (`step_failed` after a load) | fixed (`mod.rs:5231`) | mutation 2 |
| m3 (`equipment_zip` doc) | fixed (`py_dwelling.rs:1032-1062`); repeated as m3 above in `mod.rs` | read |
| m4 (per-step band lookup, mixed fleet) | fixed: one bounds list per call, Python `_field_bounds` / `action_bounds` is the single source, vec env refuses a mixed fleet | below |
| m5 (test gaps) | closed for the dual-mode flip, `step_failed`, ramped release (`ResistanceWH`), vec-env bounds; open items in m4 above | mutations 3, 4 |
| i1 (`hvac_axes`, name-keyed last axis) | removed: no mode is read; `displaced_axes` is keyed by `DispatchTarget` and checkpointed | read, `a_change_of_axis_within_an_event_releases_the_old_axis_first` |
| i2 (warn every step) | now once per event (`warned_without_direction`, reset when the event ends); the text is wrong (MJ1) | read |
| i3 (`of_mode` duplicate) | removed with the mode logic (`ThermostatAxis::of_end_use` maps two end uses) | read |
| i4, i5 | documented, text clean | grep |

### 1. MJ1 of the previous review, run

BESTEST 600 ideal unit (heating 2029 / cooling 2616 steps in the preheat
year, 1834 / 2818 in the precool year), Denver, 8760 steps, event active
every step, delta 2 C, `ByName` on the ideal unit:

| event | steps with both axes displaced | heating moved | cooling moved | rejected signals |
|---|---|---|---|---|
| `preheat(2.0)` | 0 | 8759 | 0 | 0 |
| `precool(2.0)` | 0 | 0 | 8759 | 0 |

(Previous review, 53c536ba: 8747 of 8760 with both displaced.)
`dr_preconditioning` passes 4 of 4, including
`a_dual_mode_unit_displaces_only_the_named_axis_for_a_year`.

Attacks:
- Checkpoint restore mid-event (save at step 200, run 100, load, run 100):
  heating setpoint, cooling setpoint and mode identical on all 100 steps;
  0 rejected.
- Unit replaced mid-event (same name, another ideal unit): cooling
  setpoint stays at schedule minus 2 C (max deviation 0), heating never
  moves, 0 rejected, the actor survives (`actor_count` 1).
- An end-use target covering several units: the axis comes from the end
  use, so every covered unit gets one axis by construction. Not run on a
  home with two covered units; m1 is the dual-mode case.
- A heater and a cooler sharing a name: cannot happen (i1).
- A target added before its equipment, a replaced unit, and a supplied
  actor: MJ1.

## Mutation checks (disposable `git archive` copy of 2fb9f63b)

Four mutations applied together; each of the four tests named failed.

| # | mutation | failing test, assertion |
|---|---|---|
| 1 | drop the `preconditioning_without_direction` reset in the event-ended branch | `a_target_not_in_the_dwelling_is_counted_and_the_count_clears`: "cleared when the event ends" (left `Some(1.0)`, right `Some(0.0)`) |
| 2 | drop `self.step_failed.fill(false)` in `load_checkpoint` | `a_checkpoint_load_clears_the_failed_step_flags`: left `Consumption(0.0)`, right `Consumption(0.3)` |
| 3 | `ResistanceWH` release sets the ramped `setpoint_c` at once | `a_release_ramps_the_setpoint_back_to_the_configured_one` (`resistance.rs:1483`) |
| 4 | no release when the axis changes | `a_change_of_axis_within_an_event_releases_the_old_axis_first` (`dr_compliance.rs:1727`: 1 request, expected 2) |

## Item 4: `batch_step` bounds

One source of truth: the Rust `field_bounds` and `action_bounds` (about 50
lines and 4 tests) are gone; Python `_field_bounds` and `_action_bounds`
(`gym_env.py`) compute the bounds, once, at construction, and
`VecDwellingGymEnv` passes `self._action_bounds` (`vec_env.py:88,243`) to
`batch_step`. Rust holds one lookup, `band_range_of`, behind
`thermostat_band_range`. Per step, the dwelling mutex and the per-field
linear scan are gone; the bounds list is converted once per call (one small
`Vec`, beside the existing `action_layout` and `signal_type_by_equipment`
conversions, which allocate per call and are not new). Allocation inside
`map_action_to_signals` (two `HashMap`s per dwelling per step) is as
before. Open: m2.

## Item 5: physics

No physics file is touched. In the range `hares-equipment` gains
`thermostat_axes()` bodies and one test, `hares-core` gains the actor
hooks, the `step_failed` reset on load and tests. BESTEST 600, 640 and 900
and the `regression` and `parity` targets pass in the run below. I did not
compare output bytes against 53c536ba; the diff scope and the passing
reference tests are the evidence.

## Item 6: API and consumer

Breaking changes in 53c536ba..2fb9f63b:
1. Rust: `DrAction::SetpointAdjust { delta_c }` is now
   `{ delta_c, direction: Option<ThermostatAxis> }`; new `DrAction::preheat`
   and `precool`. Config string: `SetpointAdjust:<d>[:PreHeat|:PreCool]`
   (`actor_registry.rs:155-171`).
2. Behaviour: `add_actor` / `add_actor_by_name` of a `DrCompliance` whose
   positive-delta target is a dual-mode unit without a direction, a unit that
   does not serve the named direction, a unit with no thermostat setpoint, or
   a non-HVAC end use now fails with `HaresError::PreconditioningAxis`
   (Python `HaresConfigError`). Before, the axis came from the unit's mode.
3. Rust trait `Actor`: `resolve_equipment_descriptors` is now
   `resolve_equipment(&[ActorEquipment])`; new `validate_equipment`; new
   public `ActorEquipment`. Trait `Equipment`: new `thermostat_axes()`
   (default `None`); `delegate_equipment!` forwards it. New public
   `ThermostatAxis`, `ThermostatAxes`, `HaresError::PreconditioningAxis`
   (exhaustive matches on `HaresError` break).
4. Python: `batch_step(.., signal_type_by_equipment, action_bounds)` has a
   new required sixth argument (`_hares.pyi:2557-2560`);
   `Dwelling.thermostat_band_range(name)` and the band constants are new
   (the latter two in the previous range).
5. `Dwelling.load_state` / `load_checkpoint` now clears the failed-step
   flags, so `equipment()` flows after a rewind are the restored ones.
6. DR actor checkpoint state layout (i6).

`~/src/HARES_CALIBRATION_SYSTEM` at `i08-consumer` (`b71dbb4`). The list of
`review-OD-16-2689f2af.md` (items 1-10) and of `review-OD-16-53c536ba.md`
(items 1-10) stands, with these changes:

- 53c536ba item 5 is replaced: if the consumer uses the engine's
  `DrCompliance` for pre-conditioning it must name the direction
  (`SetpointAdjust:2.0:PreCool` through `add_actor_by_name`), and register
  the actor after its equipment; a missing direction raises at registration,
  but a misspelt name or a later add does not (MJ1), so also check the
  setpoint it displaced. Do not target an ideal unit by end use (m1). The
  stale counter is fixed; the counter's name is misleading (MJ1).
- 53c536ba item 8 stays (a rewind after the budget error resumes the home,
  and a retry repeats the failing step), and now also: a rewind restores the
  failed equipment's flows to readers before the next step.
- Still the call sites to change (unchanged line numbers at `b71dbb4`):
  `calibration/ami2/dr_actuation.py:206` and `:248`,
  `calibration/tests/suite_ami2.py:877`, `:1760-1762`, `:938-939`,
  `:1802-1804`, `calibration/ami2/functions/_flexibility.py:219-221`,
  `calibration/coupling.py:408,420`, `calibration/cosim.py:90,102`.
- `batch_step`, `ochre_next.rl` and `DrCompliance` are not used under
  `calibration/` (grep). The untracked nested `HARES/` directory in the
  consumer repo holds an old copy whose `tests/python/test_py_dwelling_integration.py`
  calls `batch_step` with five arguments (`:497,522,552,570`); it is not
  consumer code and is not tracked.
- The list is complete for this range: every consumer reference to
  `thermal_setpoint_delta`, `thermal_setpoint(None, None, 0.0)`,
  `load_state`, `equipment()` was checked against the greps above, and no
  other changed symbol appears in `calibration/`.

## Item 7: fold onto 2f85338d

Method: a scratch worktree at 2f85338d. A sequential `git cherry-pick
50a5c261..2fb9f63b` (28 commits) stops at its first commit, `cae2ab0a`,
with 11 conflicted files (`baseboard.rs`, `boiler.rs`, `furnace.rs`,
`heat_pump/heater.rs`, `heat_pump_wh.rs`, `tankless.rs`, `hvac_tests.rs`,
`hares-types/src/error.rs`, `hares-types/src/lib.rs`, `_hares.pyi`,
`helics/dwelling.py`). To inventory the whole range I merged 2fb9f63b
(`--no-commit`), which conflicts in 24 files (about 50 hunks):
`hares-core/src/{actor,checkpoint}.rs`, `dwelling/{mod,solver_builder}.rs`,
`tests/pv_override_observation.rs`, `resstock_smoke.rs` (modify/delete),
`hares-equipment/src/hvac/{air_conditioner,baseboard,boiler,furnace,ideal_hvac}.rs`,
`heat_pump/heater.rs`, `water_heater/{heat_pump_wh,tankless}.rs`,
`tests/hvac_tests.rs`, `hares-io/src/hpxml/equipment.rs`,
`hares-python/src/py_dwelling.rs`, `hares-types/src/{error,lib}.rs`,
`_hares.pyi`, `helics/dwelling.py`, `rl/{gym_env,vec_env}.py`,
`tests/conditioned_oracle.rs`. 107 files differ from 2f85338d after my
resolution.

Semantic interactions, each fixed in the scratch fold (none are textual
conflicts):
1. `CHECKPOINT_VERSION`: the branch claims 9 (the streak field in
   `EquipmentStateCheckpoint`), HEAD is at 11 (its own note: "the fold of
   the two v9-claiming branches"). It must be 12, with
   `checkpoint_shape_is_pinned_to_its_version` re-pinned (fingerprint
   `96c7cfce4f45f9111258d6df7a7d7041bb377f572be4c9b0a771e66dec8c0d11`
   with the streak field and nothing else new).
2. OR-19 plan-then-install removed `refresh_equipment_caches`. The hooks
   must move into `install_roster_caches` (`roster.rs:1175`):
   `resolve_equipment` beside `resolve_equipment_id`, the failure-streak
   carry keyed by the old `roster.equipment_ids`, and the `step_failed`
   resize. HEAD's `joining_core_outputs` replaces the branch's
   `seed_equipment_core` (drop it). `validate_equipment_stage` moves into
   `validate_candidate_ports` (`roster.rs:578`).
3. `validate_equipment` against OR-19's planner: `add_actor` plans with
   `change.supplied` and `add_equipment_with_actors` supplies actors with
   equipment, so a validation placed before the push (as on the branch)
   covers `add_actor` only; supplied actors, `add_equipment` and
   `replace_equipment` are unchecked (MJ1, reproduced on the fold). The fix
   is one call in `plan_roster_caches`, which has both prospective lists.
4. Actor eviction: `DrCompliance` has no `dispatch_target_name`, so OR-19's
   eviction on removal and its capability check on replacement do not cover
   it. A DR actor whose target is removed survives as an orphan (counted
   only in telemetry), the same silence as MJ1.
5. Transactional restore (`checkpointing.rs`): the branch's streak restore
   and terminal-state clearing must run after `load_component_states`
   succeeds, not inside the per-equipment loop; `matched_equipment_blobs`
   has to return the states. The terminal checks in `save_checkpoint` and
   `restore_building_state` are re-applied in the new file.
6. HVAC step ordering: HEAD's `compute_and_write_ebm_telemetry` and
   `served_zone()` against the branch's `step_equivalent_battery` before any
   state commit and `write_ebm_telemetry(window)`. Resolved to the branch's
   order with `served_zone()` in the window; `IdealHvac` merges HEAD's
   `route_end_use_by_mode` and `served_zone()` with the branch's
   `control_error` and `update_mode_at`.
7. PV: the branch registers every array's orientation before `init`; HEAD
   registers after the panel is accepted. Resolved to register-after-accept
   with the per-array orientations. `register_surface` is
   `register_pv_surface` after the merge.
8. `SimulationConfig.max_consecutive_step_failures` is a new required field:
   13 HEAD files build a literal without it (`checkpoint_file_resume`,
   `checkpoint_regressions`, `schedule_path_contract`,
   `roster_recorder_failure`, `roster_plan_schema_parity`,
   `event_load_rng_streams`, `equipment_zone_resolution`,
   `equipment_override_typed_configs`, `backup_fuel_override_contract`,
   `roster_add_wiring_failure`, `heating_only_heat_pump_contract`,
   `indirect_tank_loop_wiring`, `tests/resstock_smoke.rs` twice).
9. HEAD changed `schedule_path` to `Option<PathBuf>` (the branch's new
   `dr_preconditioning` fixture needs `Some`), removed
   `ambient_other_space_c.conditioned_zone_c` (the branch's `env_with` test
   helper sets it), and removed the `is_conditioned_location` export.
10. `record_step` loops moved to `roster.*`; the `step_failed` skip for
    flow columns and the end-use aggregate had to be re-applied there, and
    `log_output_keys_without_columns` (`roster.rs`, `observe` feature) reads
    the split `v8_state_columns` and `v8_flow_columns`. I did not build with
    `observe`.
11. Python: HEAD made `gym_env` helpers public (`field_bounds`,
    `build_control_signal`, `drain_step_warnings`, `sorted_action_layout`,
    `observation_field_bounds`, `validate_control_signal`). The branch's
    `_action_bounds`, `_validate_thermal_setpoint_band` and its tests that
    import `_sorted_action_layout` (`test_gym_env.py:229-230`) need the
    public names; `_hares.pyi` moved its `TypedDict`s to `_hares_types`, so
    the three band constants, the `max_consecutive_step_failures` property
    and the new `batch_step` argument are re-applied.
12. HEAD's `replace_equipment_for_test` writes the roster fields directly:
    it must size `step_failed` and `consecutive_step_failures` and seed
    `equipment_core`. Without it five tests fail
    (`a_successful_step_resets_the_failure_streak`,
    `failed_equipment_core_output_not_snapshotted_to_environment`,
    `failed_equipment_step_prevents_snapshot_and_surfaces_warning`,
    `failing_equipment_does_not_contaminate_downstream_equipment_ports`,
    `save_checkpoint_propagates_equipment_error`, which returned `Ok`
    because `zip` truncated the equipment list: i5).
13. HEAD deleted `crates/hares-core/tests/resstock_smoke.rs`, the branch
    modified it: delete.

Results in the fold, after those fixes: `cargo check --workspace
--all-targets` clean; `cargo test -p hares-core --lib` 1091 passed;
`cargo test -p hares-types -p hares-equipment -p hares-core --no-fail-fast`
passes in every target except three that need `vendors/OCHRE`, which the
scratch worktree does not have (`backup_fuel_override_contract`,
`equipment_zone_resolution::hvac_and_water_heaters_without_a_zone_fail_init`,
`heating_only_heat_pump_contract`; environmental); `dr_preconditioning`
4 of 4, including the 8760-hour dual-mode year. Not run on the fold:
`hares-python`, `hares-io` and `hares-fleet` tests, pytest, the `observe`
and release profiles. The fold is a mechanical port plus one design
decision (MJ1): it is not a merge to commit as it stands.

## Item 8: shipped text

The seven commits of this range (added lines and messages): no em or en
dashes, ticket identifiers, agent references, `V8`-style tokens or
co-author lines. Across the whole range 50a5c261..2fb9f63b, two commits
fail `[writing]` standing alone, both already known and fixed by the tip:
`55854486` (an added line with a `V8` token) and `071affc9` (an added line
with an em dash, reworded in `53c536ba`).

## Verification run (worktree at 2fb9f63b)

| line | result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy -p hares-core -p hares-equipment -p hares-types --all-targets -- -D warnings` | pass |
| `cargo clippy -p hares-python --all-targets -- -D warnings` | pass |
| `cargo test -p hares-types -p hares-control -p hares-equipment -p hares-core -p hares-io -p hares-fleet` (BESTEST 600/640/900, `regression`, `parity`, `dr_preconditioning`) | 5888 passed, 0 failed (53c536ba: 5880) |
| `cargo test -p hares-python --lib -- py_gym` | 33 passed (four removed `field_bounds_*` tests) |
| scratch probes (year preheat and precool, checkpoint, replacement, end use, add order) | as above |

Not run: the Python tests (`test_gym_batch_step.py`, `test_gym_env.py`:
they need a rebuilt extension, and the worktree's is older than the tip),
the golden byte comparison, release and feature-matrix profiles, the ASHRAE
gate.

## Cleanup

`target-review-od16`, the scratch crate and mutation copy under the
worktree's `target/`, and the fold worktree
`.workstrees/review-od16-fold` are deleted. No tracked file in the
worktree or the main tree was edited, and the scratchpad logs were removed.
