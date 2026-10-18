# Handoff: fix-od16, answer to review-OD-16-160ddb61

- Handoff branch: `handoff/fix-od16` (same commit as `fix-od16`)
- Worktree: `/home/rich/src/HARES/.workstrees/fix-od16`
- Tip: `a97fae2801fa5b52abde888aeb976f41ecd10767`
- Base: `db5efe80` (`feat/output-results` head at the time; check it has not moved)
- 37 commits on `db5efe80`: the 32 rebased commits of `160ddb61` plus 5 new ones.
- No uncommitted work. Nothing needed a separate handoff commit; the branch
  was created at the existing tip.

## Rebase (done)

`git rebase --onto db5efe80 2f85338d` applied all 32 commits with no
conflict. The three head literals missing `max_consecutive_step_failures`
(`crates/hares-core/tests/equipment_init_failure_assembly.rs:40`,
`crates/hares-core/src/dwelling/solver_builder.rs` two test literals near
2018 and 2097) were fixed and folded (autosquash) into `6251485a`
("a failed equipment step is surfaced ..."), the commit that adds the
field. `CHECKPOINT_VERSION` stays 12; the checkpoint format is unchanged
(RunHealth is not in the checkpoint), so the pin hash is untouched.

## Findings

### MJ1: done and tested (`c5cf4358`)

- `crates/hares-core/src/dwelling/mod.rs`: new `enum EndUseReach { Label,
  ThermostatAxis }` with `EndUseReach::of(signal)` (ThermalSetpoint and
  ThermalSetpointDelta, which includes the release, widen; every other
  signal keeps the label match). `addressed_by_end_use(eq, end_use, reach)`
  takes the reach. Callers: `ByEndUse` dispatch expansion in `drain_tiers`
  and `route_request` use the request's signal; the
  `EquipmentModeChange` trigger uses `ThermostatAxis` (wake on any flip of a
  dual-mode unit, comment there); the observe-only conflict scan
  `targets_conflict` now takes `&DispatchRequest` pairs and a helper
  `named_unit_addressed_by_end_use`, and `dispatch_into_observed` tracks
  `recorded_from` indices instead of re-comparing `SameTierConflict.target`.
- `crates/hares-core/src/actors/dr_compliance.rs`: `DrAction::TurnOff` doc.
- Test: `crates/hares-core/tests/dr_preconditioning.rs`
  `a_cooling_event_reaches_a_heating_dual_mode_unit_only_to_move_its_cooling_setpoint`
  (a `ThermostatProbe` with axes Both, labelled HVAC_HEATING, records the
  signals it receives: `TurnOff` on `ByEndUse(HVAC_COOLING)` never reaches
  it; `setpoint_delta(2.0)` reaches it as a cooling-only delta).
- Evidence (scratch probe, deleted): BESTEST 600, 2000 steps, `TurnOff` on
  `ByEndUse(HVAC_COOLING)`: heating 1088.08 kWh, 1123 rejected (the review's
  old-label row); no actor 1313.52 kWh. Not 166.0. The remaining 225 kWh
  drop is the sticky ModeOverride delivered while the unit is cooling and
  held into heating (pre-existing DR TurnOff semantics; the clear for
  TurnOff is a documented known limitation in `dispatch_clear_signals`).
  The setpoint test `an_end_use_event_on_a_dual_mode_unit_moves_only_its_axis`
  still passes (both axes, 0 rejections).
- Mutation: `EndUseReach::of` `_ => Self::ThermostatAxis` fails the new test
  ("TurnOff reached the unit: [ModeOverride { mode: Off } x3]"); reverted.

### m1: done and tested (`4b25c48b`)

- `crates/hares-core/src/actor.rs`: new `pub struct ActorTarget { target:
  DispatchTarget, required: ControlCapabilities }` (exported from
  `lib.rs`). `Actor::dispatch_target_name` and
  `Actor::required_control_capabilities` are replaced by
  `fn dispatch_targets(&self) -> &[ActorTarget]` (default empty).
  `check_declared_signals` checks each request against the declared target
  equal to its target. `validate_equipment` doc updated.
- `crates/hares-core/src/dwelling/mod.rs`: `target_lost(target, before,
  after)` (a target is lost when the change takes away every unit it
  addresses, or every such unit accepting its capabilities; end-use targets
  use the widest reach) and `describe_target`.
- `crates/hares-core/src/dwelling/roster.rs`: `ActorRosterChange.orphaned`
  warnings; `equipment_change_actors(after, keep)` builds the keep mask and
  evicts actors with a lost target; `commit_actor_change` pushes each
  warning and increments `health.evicted_actors`. `clear_equipment`,
  `remove_equipment`, `remove_equipment_by_end_use` and `replace_equipment`
  use it (replace keeps the built-in seed-change rebuild rule). Removal is
  never refused for an actor. Same-name replacement that the actor's
  `validate_equipment` refuses is still refused (exit: remove then add).
- `crates/hares-core/src/health.rs`: `RunHealth::evicted_actors`.
- Actors declaring targets: `bms.rs` (field `battery`), `ev_driver/mod.rs`
  (field `vehicle`), `ideal_thermostat.rs` (`unit`, THERMAL_SETPOINT),
  `safety_monitor.rs` (`target: ActorTarget`, MODE_OVERRIDE),
  `occupant.rs` (`targets`, rebuilt by `declare_targets` from behaviour),
  `dr_compliance.rs` (`declared_targets`, `declare_targets`,
  `DrAction::required_capabilities`; TurnOff on a freeze-guarded HVAC
  target adds THERMAL_SETPOINT). Python actors declare none.
- Message text: `preconditioning_axis` no longer prints `Some(Heating)`
  ("serves only the Heating setpoint, not the Cooling setpoint the event
  names", and a separate no-direction arm); the "is not in the dwelling"
  text now reads "register the actor after the equipment it names" and is
  only reachable at registration, supply or replacement.
- `crates/hares-types/src/error.rs`: `PreconditioningAxis` doc lists every
  refusal (m4 part).
- Goldens: the six dwelling goldens in `tests/fixtures/golden/` gained
  `"evicted_actors": 0` in `health` (hand edit, frames/metrics/digests
  unchanged; compare-all identical in release and release dst).
- Tests (`dr_preconditioning.rs`): `removing_an_actors_unit_evicts_it_with_a_counted_warning`
  (named pre-cool, named turn-off, end-use delta x remove/clear/remove by
  end use: 0 actors, `evicted_actors == 1`, one warning, then 6 steps with 0
  rejected signals), `an_actor_is_evicted_only_when_a_target_loses_its_last_unit`,
  `replacing_a_preconditioning_target_with_a_unit_it_cannot_serve_is_refused`
  (renamed from the old refusal test; uses `setpoint_probe` with caps). Test
  stubs in `dwelling/mod.rs` (`TargetingStubActor`, `Undeclared`) moved to
  `dispatch_targets` via `bound_to(name)`.
- Mutation: `match lost.filter(|_| false)` in `equipment_change_actors`
  fails 6 hares-core tests (both new DR tests with a PreconditioningAxis
  refusal, plus 4 existing roster tests); reverted.

### m2: done and tested (`6141f361`)

`dwelling/mod.rs` `a_failed_step_records_no_flow_and_keeps_the_committed_state`
now runs in `tempfile::tempdir()`. Verified alone with an empty TMPDIR: 0
entries left.

### m3: done and tested (`a97fae28`)

- `dwelling/mod.rs`: `pub fn check_control(&self, name, &signal)` (the
  non-immediate check `apply_control_validated` runs; that function now
  calls it).
- `crates/hares-python/src/py_gym.rs`: `check_signals_accepted` runs for
  every dwelling in the mapping loop before any apply; docstring and the
  stub comment in `python/ochre_next/_hares.pyi` updated.
- Test: `tests/python/test_gym_batch_step.py`
  `test_batch_step_refuses_a_missing_unit_before_any_dwelling_changes`
  (second home without the furnace; first home's heating setpoint after a
  step equals an untouched home's and is not 27). Passed (11/11 in file).

### m4: done and tested (`1aa0fc37`, plus text in `4b25c48b`)

`py_dwelling.rs` `add_actor_by_name` maps errors to `HaresConfigError`.
Test `tests/python/test_actor_param_boundary.py`
`test_a_preconditioning_event_its_unit_cannot_serve_is_a_config_error`
(passed, 15/15 in file).

## Block on a97fae28 (partial)

| line | result |
|---|---|
| tools, fmt, clippy x9 | pass |
| test-debug | 7430 passed |
| test-observe | 7488 passed |
| test-dst | 7443 passed |
| test-profiling | 7436 passed |
| test-observe-profiling | 1503 passed |
| test-release | 7387 passed |
| bench | pass |
| advisories | ok |
| golden x2 | all identical |
| python-build | built |
| pytest | 1429 passed, 4 xfailed, 21 errors: every error is the HELICS stale-state guard firing on another worktree's concurrent HELICS pytest (`fix-testhyg`, PIDs 2212362/3). Not a code failure, but the line is not green. |
| census, stray, writing | not reached |

## Unverified

- pytest clean run, skip census, `[stray]` (new paths and TMPDIR), and
  `[writing]` on the final tip. The earlier full Rust lines are green.
- The m2 branch leak is verified gone only by the single-test run.
- `pyright` (not in the block script used; the review listed it).
- Consumer call-site list impact (below) not re-grepped in the consumer repo.

## Remaining steps, in order

1. Check `git -C /home/rich/src/HARES rev-parse feat/output-results`; if it
   moved past `db5efe80`, rebase (`git rebase --onto <head> db5efe80`) and
   fix any new `SimulationConfig` literals and any new `Actor` impls using
   `dispatch_target_name`/`required_control_capabilities`.
2. Make sure no other worktree runs HELICS pytest (`pgrep -af helics`), then
   run the full `plan.md` §3 block from the worktree root with
   `CARGO_TARGET_DIR=$PWD/target RUSTC_WRAPPER=` (see traps). Report every
   line; for `[stray]` TMPDIR list only paths this branch adds (expect 0).
3. Run pyright.
4. Independent review, then reword/squash as the orchestrator wants.

## API change and consumer call sites

- Rust: `Actor::dispatch_target_name` and `Actor::required_control_capabilities`
  are removed; implement `dispatch_targets(&self) -> &[ActorTarget]`.
  `RunHealth` gains `evicted_actors`. `Dwelling::check_control` is new.
- Behaviour: removing, clearing or replacing equipment now evicts any actor
  (including a DR actor and a `ByEndUse` actor whose end use loses its last
  unit) instead of refusing, with a warning and the health count.
- Consumer (`~/src/HARES_CALIBRATION_SYSTEM`): item 9 of the review's list
  changes: removing or swapping a DR target is no longer refused but evicts
  the actor (check `take_warnings()`), and `add_actor_by_name` refusals are
  `HaresConfigError`. Item 9's "use ByName for non-setpoint actions on an
  ideal unit (MJ1)" is no longer needed: end-use non-setpoint signals reach
  a dual-mode unit only in the end use's mode. The rest of the list stands.

## Traps

- `sccache` fails inside this worktree ("path must be shorter than
  SUN_LEN"); set `RUSTC_WRAPPER=` for maturin and cargo.
- A concurrent HELICS pytest in another worktree makes the HELICS tests
  error at setup. Do not kill it; wait.
- An earlier stray block from another agent ran in this worktree via a
  shared `scratchpad/block.sh`; use uniquely named scripts.
- `RunHealth` is serialized into the goldens, so any field added to it moves
  the golden `health` block (frames do not change).
- The scratch block scripts and target dir were deleted at handoff; recreate
  from `plan.md` §3.
