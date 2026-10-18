# Review and red team: OD-16 control-signal contract, `fix-od16` at 160ddb61 (on 2f85338d)

32 commits `99bedc22` .. `160ddb61` on the OR-19 fold `2f85338d` (the
previous round was `50a5c261..2fb9f63b`, 28 commits, reviewed in
`review-OD-16-2fb9f63b.md`: 1 major, 4 minor, 6 info). This round adds ten
commits (`882e596f` .. `160ddb61`) that answer that review.

Reviewer: independent (did not write any of the commits). Worktree
`.workstrees/fix-od16` at 160ddb61, `vendors/OCHRE` updated to the pinned
`8681122b` with `git submodule update --init`.
`CARGO_TARGET_DIR=/home/rich/src/HARES/target-review-od16b`,
`CARGO_INCREMENTAL=0`. Probes and mutations ran in scratch crates and a
`git archive` copy under the worktree's `target/`; the head-drift fold ran
in a scratch worktree. All of them, the target directory and the
worktree's `target/` are deleted. No tracked file was edited.

**Verdict: CHANGES-REQUIRED.**

Counts: 0 blockers, 1 major, 4 minors, 6 info.

The previous major is fixed at its root, and the fix holds on every roster
path I attacked (validation runs inside `plan_roster_caches`; an unserveable
event is refused atomically, nothing is left half-done). The rebase lost
nothing of OD-16 and nothing of the head, BESTEST is byte-identical to
`2fb9f63b`, the goldens are identical in three modes, and the full block is
green except `[stray]`, whose one branch-owned leak is minor m2. What keeps
the verdict at CHANGES-REQUIRED: the end-use widening that fixes the old m1
also changed what every non-setpoint signal reaches (MJ1 below), the refusal
rule has no exit and disagrees with OR-19's eviction rule (m1), and `[stray]`
fails from a leak this branch added (m2).

## Findings

### Major

**MJ1. The end-use widening applies to every signal type, so a cooling
curtailment now switches off a dual-mode unit's heating.**
`crates/hares-core/src/dwelling/mod.rs:1510` (`addressed_by_end_use`, used at
`:1143`, `:1305`, `:1426`, `:4256`), commit `4b909be6`. The commit message
and the doc say "an end-use target now also addresses any unit whose
thermostat serves that end use's setpoint". That is the right rule for a
thermostat-setpoint signal. The function is not told which signal it routes,
so a `ModeOverride` (what `DrAction::TurnOff` sends), a power limit or a DR
level on `ByEndUse(HVAC_COOLING)` also reaches a unit that is heating.

Reproduced on this tip (BESTEST 600 ideal unit, Denver, 2000 hourly steps,
one `DrCompliance` at `Moderate` for the whole run, `AlwaysComply`,
`TurnOff` on `ByEndUse(HVAC_COOLING)`; heating energy is the sum of
`StepResult::hvac_heating_w`):

| case | unit modes (Off, Heating, Cooling, Standby steps) | heating energy | rejected signals |
|---|---|---|---|
| no actor | 829, 603, 447, 121 | 1313.5 kWh | 0 |
| this tip | 1146, 161, 0, 693 | 166.0 kWh | 0 |
| tip with `addressed_by_end_use` reverted to the old label match (scratch copy) | 724, 506, 5, 765 | 1088.1 kWh | 1123 |

A cooling curtailment removes 87 % of the unit's winter heating, and the
unit is never in cooling mode again because the sticky override is held.
With the old match the same event also leaked (the override stuck once
delivered) but reached the unit only on steps it was labelled cooling, and
counted the others as rejected. The setpoint case is right on this tip:
`ByEndUse(HVAC_COOLING)` and `ByEndUse(HVAC_HEATING)` `setpoint_delta(2.0)`
over 2000 steps move only their axis on 1999 of 1999 steps, 0 steps move the
other axis, 0 rejected signals (old match: 1334 and 640 rejected, the
previous review's 1322 and 654). No test pins a non-setpoint signal on a
dual-mode end-use target.

The DR freeze guard downgrades a `TurnOff` only below the freeze threshold,
so it does not protect an occupied winter home.

Fix: pass the signal's capability into the match and widen only for the
thermostat-setpoint signals (`ThermalSetpoint`, `ThermalSetpointDelta`, the
release); keep the label match for mode, power and DR signals, or refuse
them with a typed error on a unit serving both. Add a test that a
`TurnOff` on `ByEndUse(HVAC_COOLING)` leaves a heating ideal unit's heating
untouched (or is counted), next to
`an_end_use_event_on_a_dual_mode_unit_moves_only_its_axis`.

### Minor

**m1. A refused removal has no exit, and the rule disagrees with OR-19's
for the same actor.**
`crates/hares-core/src/dwelling/roster.rs:1031-1034` (the validation),
`:705-722` (`clear_equipment`), `:728-760` (`remove_equipment`),
`crates/hares-core/src/actor.rs:100-109` (the eviction contract),
`crates/hares-core/src/actors/dr_compliance.rs:1089-1097`.

Probes on this tip (BESTEST 600, one `DrCompliance` pre-cooling the ideal
unit by name):

- `remove_equipment`, `clear_equipment` and
  `remove_equipment_by_end_use([HVAC_HEATING, HVAC_COOLING])` are all
  refused; 2 equipment and 1 actor remain.
- There is no `remove_actor` or `clear_actors` in the Rust API or the stub
  (grep), so after the actor is registered the unit can be neither removed
  nor swapped for a unit with other axes for the life of the dwelling. The
  only exit is to rebuild the dwelling.
- The refusal text says "no equipment named 'Ideal HVAC' is in the
  dwelling; the actor is registered after its equipment". On a removal that
  is false: the equipment is there and the actor was registered after it.
- Same actor, `TurnOff` on `ByName(unit)` (not pre-conditioning):
  `remove_equipment` succeeds, the actor is kept, and 5 steps later 5
  signals are rejected and 5 "control target not found by name" warnings
  are logged (it is an orphan, which `actor.rs:100-109` says the dwelling
  evicts for every actor with a `dispatch_target_name`).
- Same actor, `setpoint_delta(2.0)` on `ByEndUse(HVAC_HEATING)`:
  `remove_equipment_by_end_use` removes the only heating unit, the actor is
  kept, 6 steps later 6 rejected signals.

So one actor has three outcomes for the same loss of its target: refuse
(named pre-conditioning), keep and count (named other action, end-use),
and, for OR-19's single-target actors, evict. `clear_equipment`'s doc ("actors
bound to any equipment are evicted with it") is also false for it.

Recommendation: one rule for all actors, and it is the OR-19 rule, widened.
Refuse at registration and at supply (`add_actor`,
`add_equipment_with_actors`), where the caller holds the input and can fix
it; refuse a replacement that cannot serve a named event only if the actor
can still be removed. For removal, do not refuse. A refusal is only useful
when the caller has a way out, and the dwelling offers none, so it turns
every later `clear_equipment`, `remove_equipment_by_end_use` and equipment
swap into a trap. Replace the single-name `dispatch_target_name` by a
list (`DrCompliance` has several targets) and apply eviction to an actor
left with no target; for an actor that keeps some targets, drop the lost
one and raise a dwelling warning (the `IdealThermostat` precedent: evicted
with a dwelling warning), counted in `health()`. Add `remove_actor` if
refusal is kept for any path. Whatever is chosen, fix the message on
removal and test `clear_equipment` and `remove_equipment_by_end_use` with
the actor present.

**m2. `[stray]`: a test added by this branch leaves a file in `TMPDIR`.**
`crates/hares-core/src/dwelling/mod.rs:14068`
(`a_failed_step_records_no_flow_and_keeps_the_committed_state`, commit
`27c51ada`). It guards the `.toml` and the `.csv` but the recorder at
`output_verbosity = 8` also writes `<stem>_diagnostics.csv`. Alone, with an
empty `TMPDIR`, one run leaves
`hares-checkpoint-failed_step_row-<nanos>_diagnostics.csv`; the block left
6 (one per nextest configuration). Fix: add a `TempFile` guard for
`output_path.with_file_name(<stem>_diagnostics.csv)`.

**m3. `batch_step` documents all-or-nothing and is not.**
`crates/hares-python/src/py_gym.rs:420-427` (docstring: "Any refusal is a
`ValueError` raised before a signal reaches any dwelling"), `:455-480`,
commit `f07a2067` message ("a refusal reaches no dwelling"). Bounds and
mapping errors are all raised before the apply loop, as claimed. The apply
loop (`apply_control`, which checks the equipment and its capability) can
still refuse dwelling N after dwelling N-1 took its signal. Reproduced with
two furnace homes where the second has no equipment: `batch_step` raised
`ValueError: equipment error: equipment 'Gas Furnace' not found`, and the
first home's heating setpoint was 27.0 C afterwards (20.0 C before). Either
validate equipment and capability for every dwelling in the mapping loop, or
narrow the claim to the bounds.

**m4. Refusal text and docs.**
- `crates/hares-types/src/error.rs:48-52`: the `PreconditioningAxis` doc
  lists three causes; since `882e596f` it also covers a named target absent
  from the dwelling, a direction on a relaxing or non-finite delta, and a
  removal or replacement that loses the target.
- `crates/hares-core/src/actors/dr_compliance.rs:1108-1110`: the message
  prints `not the Some(Heating) the event names` (a Rust `Debug` fragment).
  Seen from Python: "it serves only the Cooling setpoint, not the
  Some(Heating) the event names".
- `crates/hares-python/src/py_dwelling.rs:925`: `add_actor_by_name` maps
  every error to a plain `ValueError`, so the refusal reaches Python as
  `ValueError`, not `HaresConfigError` (which the previous review and the
  stub's roster methods use; `HaresConfigError` is a `ValueError` subclass,
  so only `except HaresConfigError` misses it). No Python test covers a
  `PreconditioningAxis` refusal through `add_actor_by_name`.

### Info

- **i1. The dwelling refusal by DR actor version is not tested.**
  `the_snapshot_schema_version_moved_with_the_displaced_axes` asserts a
  first-layout blob fails to decode and `checkpoint_version() == 2`. It does
  not run a checkpoint with a version-1 actor blob through the dwelling gate
  its commit message describes. Reverting the version to 1 does fail the
  test (mutation 3 below), through the equality, which is a pin, not a
  behaviour check.
- **i2. `batch_step` re-reads the band range per call.**
  `check_deadband_bounds` takes each dwelling's mutex and scans equipment
  once per `deadband_c` column per call. Cheap and only for deadband
  columns; the previous round removed a per-step lookup and this is a
  smaller per-call one. Not a finding unless the call rate matters.
- **i3. The HELICS band screen (`_validate_thermal_setpoint_band`) is a
  documented coarse screen** (`d25b5fc4`, doc only); a 20 C band on a
  furnace passes it and is refused and counted by the equipment. The
  previous review's i4 is closed by documentation, not by a test.
- **i4. Three head-side test literals break the fold** (item 10): three
  `SimulationConfig` literals added at the head lack
  `max_consecutive_step_failures`. Fold work, not a branch defect.
- **i5. The consumer's `DrCompliance` use is prose only** (comments at
  `dr_actuation.py:36-39`, `suite_ami2.py:789-792`, `f7_dr.py:13`); nothing
  there calls the engine's actor, so the new refusal and `m1` do not bite the
  consumer today.
- **i6. `from_hpxml` default budget is 1** (`hares-io/src/config.rs:148`):
  the second consecutive failure ends a run. Earlier reviews recommended 0
  for calibration (consumer list item 6).

## Check results

### 1. Rebase integrity

`git range-diff 50a5c261..2fb9f63b 2f85338d..160ddb61`: 28 old commits map
to 22 new ones. Five are unchanged in content (`=`: `2fa080cc`, `12c80f19`,
`bc780bfc`, `a0f23216`, `049527e1`), six old commits have no counterpart
(`9713fe20`, `d0d8def7`, `53c536ba`, `440cc540`, `21fdc032`, `2fb9f63b`,
each a style, import, doc or release-build fix whose target the head
changed), and the 10 new commits `882e596f` .. `160ddb61` follow
(22 + 10 = 32). Every
commit differs from its old counterpart because the base moved. Method, since
`-X ours` is risky and a per-hunk read of 32 commits across 108 files is not
verifiable by eye alone:

1. File sets. The old range touched 95 files, the new 108. Differences are
   exactly the head's reshaping: `checkpointing.rs` and `roster.rs` replace
   `solver_builder.rs`, `resstock_smoke.rs` is gone, and 13 head test files
   gained `max_consecutive_step_failures`.
2. For the 21 files the head did not touch, the added and removed lines of
   the old and new diffs are identical except in five files that carry the
   ten new commits (`dr_compliance.rs`, `dr_preconditioning.rs`,
   `py_gym.rs`, `thermostat_axis.rs`, `test_gym_batch_step.py`); I read
   those diffs in full.
3. For the 74 files both sides touched, I checked every added line of the
   head's diff (`50a5c261..2f85338d`) against the tip, and every removed
   line of it against the tip. Head additions missing from the tip, all
   explained: `CHECKPOINT_VERSION` 11 to 12 and its pin (below), the HVAC
   `lookup_zone(..)` calls replaced by `step_equivalent_battery` (reads the
   same `served_zone()` before any state commit; `helpers.rs:501-513`), the
   `v8_columns` loop split into state and flow columns (`mod.rs:5910`,
   `:6050`), `register_surface` replaced by `register_pv_surface` with
   per-array orientations (`mod.rs:1527`), `lookup_zone_temp` once per
   control update in `ideal_hvac.rs` (`:306`), `field_bounds(field)` replaced
   by `action_bounds` in the two gym files, and import lists. Head
   removals reintroduced by the tip: none that is not a test helper line
   (`replace_equipment_for_test` re-seeds `equipment_core`, same as the head).
4. Read in full: `roster.rs`, `checkpointing.rs`, `checkpoint.rs`,
   `actor.rs`, `helpers.rs`, `baseboard.rs` and the whole `mod.rs` diff
   against the head (30 KB).

Preserved from the head: OR-19's plan-then-install (`install_roster_caches`
still returns `()`; the new hooks, `resolve_equipment` and the streak carry,
are infallible and run after commit, `roster.rs:1186-1222`),
`joining_core_outputs` (`roster.rs:123,1107,1239`), `served_zone()`
(`ideal_hvac.rs:294`, air_conditioner `:1482`), `route_end_use_by_mode`
(`ideal_hvac.rs:286,519,521,558`), and the gym helpers the head made public
(`field_bounds`, `build_control_signal`, `drain_step_warnings`,
`sorted_action_layout`, `observation_field_bounds`). Nothing of OD-16 was
lost: every old-range test name I diffed is present. `CHECKPOINT_VERSION` is
12 (`checkpoint.rs:45`); the pin is `PINNED_VERSION = 12` and
`96c7cfce4f45f9111258d6df7a7d7041bb377f572be4c9b0a771e66dec8c0d11`, the
fingerprint the previous review computed independently for "the streak field
and nothing else new"; `checkpoint_shape_is_pinned_to_its_version` passes.
The other state bumps are DR actor 2 (`dr_compliance.rs:1024`), HPWH 3 and
the baseboard-family hysteresis versions.

### 2. Roster paths (previous MJ1)

Validation runs in `plan_roster_caches` (`roster.rs:1031-1034`) for the 9
callers: `add_actor` (`:214`), `auto_register_actors` and `set_tariff`
(`:404`, `:434`), `add_equipment` / `add_equipment_with_actors` (`:502`),
`clear_equipment` (`:719`), `remove_equipment` (`:754`),
`remove_equipment_by_end_use` (`:802`), `replace_equipment` (`:884`). There is
no unwrap-to-`None` left: `grep unwrap_or(None)` over `dr_compliance.rs` is
empty, `resolve_axes` keeps the `Err` reason.

| path | result |
|---|---|
| `add_actor`, target missing | refused, actor not added (tests, and Python below) |
| equipment added later than the actor | the actor cannot be added first (named target must exist) |
| `add_equipment_with_actors` with an actor | prospective equipment includes the new unit; test `an_actor_supplied_with_a_unit_it_cannot_serve_is_refused_with_it` |
| `replace_equipment`, no axes or the other axis | refused, roster untouched (`replacing_or_removing_...`) |
| `remove_equipment`, `clear_equipment`, `remove_equipment_by_end_use` | refused, roster untouched (probe), see m1 |
| tariff rebuild | plan has the unchanged equipment and the existing actor: no new refusal possible |
| checkpoint load into a different roster | not reachable: equipment count and name/id mismatches are refused before any load (`checkpointing.rs:268-308`); the DR snapshot carries `displaced_axes` only |
| Python `add_actor_by_name`: misspelt name, `end_use:hvac_cooling` with `:PreHeat`, `name:Gas Furnace` with `:PreCool` | `ValueError`, 0 actors (m4 for the class); `end_use:hvac_heating` + `SetpointAdjust:2.0`, `name:Gas Furnace` + `SetpointAdjust:2.0` accepted |
| TOML-declared actors | none exist (no `[[actors]]` in `config.rs`) |

Mutation of the validation call to `let _ =` fails 4 of 8 tests in
`dr_preconditioning` (the 4 refusal tests).

Eviction versus refusal: see m1 for the evidence and the recommendation.

### 3. Old m1: end-use event on a dual-mode unit

BESTEST 600 ideal unit, `setpoint_delta(2.0)` for 2000 steps:

| target | steps | axis moved | other axis moved | rejected |
|---|---|---|---|---|
| `ByEndUse(HVAC_COOLING)` | 1999 | 1999 | 0 | 0 |
| `ByEndUse(HVAC_HEATING)` | 1999 | 1999 | 0 | 0 |

(The unit was off 788/793, heating 615/627, cooling 474/457 steps.) Heat
pump, `bldg0000004` (ASHP Heater `One(Heating)`, ASHP Cooler `One(Cooling)`),
`ByEndUse`, 250 steps: cooling in July 250 of 250 unit-steps displaced, 0
rejected; heating in January 250 of 250, 0 rejected. Fixed for setpoint
signals. The widening for other signals is MJ1.

### 4. `batch_step` bounds

Attacks through the Python entry (the stub signature from `f07a2067`):
`(30, 10)`, `(nan, 80)`, `(-50, nan)`, `(-inf, inf)` each raise
`ValueError` naming the pair and the column; `(0, 0)` and `(-0.0, 0.0)` are
accepted (degenerate but defined); a `deadband_c` column with bounds
`(0, 30)` against the furnace's band `(0.1, 10)` raises `ValueError`; a
length mismatch is refused. I found no panic path: the only `clamp` takes
validated pairs, the action length is checked first, and the empty `actions`
case no longer indexes. Mapping is done for every dwelling before any is
applied, but apply-stage errors are not covered (m3).

Contract: one bounds list per call, refusing a dwelling whose deadband
target holds a narrower band, is the right one. The caller of `batch_step` is
a vector environment, whose `single_action_space` is one `Box`; per-dwelling
bounds would mean per-dwelling action spaces, which the gymnasium vector
contract does not have. The alternatives are worse: clipping to the
narrower dwelling's range silently changes every other dwelling's action;
clipping to each dwelling's own range makes the agent's action space a lie.
`VecDwellingGymEnv` refuses a fleet whose bounds differ at construction
(`vec_env.py:90-100`), and `batch_step` refuses it again per call
(`check_deadband_bounds`), which also catches a unit replaced after the env
was built. Keep it. Only the all-or-nothing wording is wrong (m3).

### 5. HPWH ramp (old m4)

Correct. `setpoint_ramp_rate_c_per_s` was set to `None` at every `init`
(removed line `heat_pump_wh.rs:416`) and nothing set it otherwise, so the ramp
branch in `update_control` and the second `target_setpoint_c` could not run.
The configuration has no ramp key for this type: `max_setpoint_ramp_rate_c_per_min`
exists only on `ElectricResistanceWaterHeaterConfig` (`wh_config.rs:268`, read
at `resistance.rs:389`); `HeatPumpWaterHeaterConfig` (`:640`) has none. A grep
for `ramp_rate`, `setpoint_ramp` and `ramp` over the repo and the consumer's
`calibration/` finds no HPWH ramp (the consumer hits are unrelated load
ramps). The state loses a field, so the version bump to 3 is required and
done (`heat_pump_wh.rs:47`). The new test
`a_release_restores_the_configured_setpoint_at_once` pins a set and a release
at once and across a control update. `ResistanceWH` keeps its ramp and its
ramped-release test (`resistance.rs:1466`).

### 6. i1 to i6 of the previous review

| finding | fix and test |
|---|---|
| i1 axes complete | `every_registered_thermostat_declares_the_setpoints_it_serves` (`lifecycle.rs`, `746fe835`) builds every registered type and ties band class to axes and end use |
| i2 Python cannot read axes | `Dwelling.thermostat_axes(name)` (`16e32ff5`), stub and 2 pytest cases |
| i3 `preheat(-x)` ignores direction | a direction on a relaxing, zero or non-finite delta is refused (`dr_compliance.rs:1073-1084`), cases in `an_event_without_an_axis_for_its_target_is_refused_at_registration` |
| i4 HELICS screen | documentation only (`d25b5fc4`), not tested |
| i5 two parallel vectors | `save_checkpoint` refuses a misaligned streak vector (`checkpointing.rs:39-45`), test `save_checkpoint_refuses_streaks_misaligned_with_the_equipment` |
| i6 per-actor schema bump | `DrCompliance::checkpoint_version() == 2` (`8fbfdc77`) |

Mutations (disposable `git archive` copy of 160ddb61), each failed the test
named; three of them reproduced here:

| # | mutation | failing test |
|---|---|---|
| 1 | `actor.validate_equipment(&prospective)?` replaced by `let _ = ..` (`roster.rs:1033`) | `an_event_without_a_direction_on_a_dual_mode_unit_is_refused` (`dr_preconditioning.rs:330`), `an_event_naming_a_unit_not_in_the_dwelling_is_refused` (`:465`), `replacing_or_removing_a_preconditioning_target_it_cannot_lose_is_refused` (`:517`), `an_actor_supplied_with_a_unit_it_cannot_serve_is_refused_with_it` (`:496`) |
| 2 | the length check in `save_checkpoint` replaced by `if false` (`checkpointing.rs:39`) | `save_checkpoint_refuses_streaks_misaligned_with_the_equipment` (`mod.rs:13704`) |
| 3 | `checkpoint_version` 2 to 1 (`dr_compliance.rs:1025`) | `the_snapshot_schema_version_moved_with_the_displaced_axes` (`left: 1, right: 2`) |
| 4 | `addressed_by_end_use` reverted to the old label match (scratch, used for MJ1's third row) | reproduces the old m1: 640 and 1334 rejected signals, matching the previous review |

### 7. Physics

BESTEST byte-identical to `2fb9f63b`: both trees built in scratch and ran
`Dwelling::simulate()` over every fixture in `tests/fixtures/bestest`,
hashing the `Debug` text of every `StepResult` (timestamp, net power, zone
temperatures, heating and cooling watts, gas power; 64-bit hash): 600
(8760 steps) `9c1bc43f92ccb96b`, 600ff `e911a1f82a1a25ff`, 610 and 620
(2880) `3773ffb67eb7fd8b`, 640 `c766ebd4a5ced366`, 900 `a22074aadb4b42fd`,
900ff `a5d362c32bddf87a`, ce100 `833ce6ad53e53da6`, ce200
`07a20eafb399dc15`, s54_heat_pump `b579fa2bc9c159ed`: all 10 equal on both
trees. Goldens: `frame_golden compare-all` identical in debug, release and
release `-F dst`; no golden file is in the diff.

### 8. The full block on 160ddb61 (every line, `bash` with `TMPDIR` set)

| line | result |
|---|---|
| `[tools]` | recorded |
| `[fmt]` | pass |
| `[clippy]` and the 8 variants (`default`, `dst`, `profiling`, `observe`, `observe+profiling`, `release`, `release dst`, `release observe`) | all exit 0 |
| `[test-debug]` | 7405 passed, 0 skipped |
| `[test-observe]` | 7463 passed |
| `[test-dst]` | 7418 passed |
| `[test-profiling]` | 7411 passed |
| `[test-observe-profiling]` (`hares-core`) | 1492 passed |
| `[test-release]` | 7362 passed |
| `[bench]` | pass |
| `[advisories]` | advisories ok |
| `[golden]` x2 | all identical |
| `[python-build]` | wheel built, installed editable |
| `[pytest]` (default pythonpath, pinned OCHRE `8681122b`) | 1445 passed, 4 xfailed, 0 skipped; census diff empty |
| `[pyright]` | 0 errors, 0 warnings |
| `[stray]` new paths | 0 |
| `[stray]` `TMPDIR` | FAILS: 38930 entries |

`[stray]` TMPDIR prefixes (count of entries): `pytest-of-rich` children
(32219), `hares-regr-output-` (6096), `hares-fleet-` (213+),
`hares-test-batchstep-*` (about 280), `hares-core-engine-` (18), `pv_lut_*`
(about 120), `hares-unwired-indirect-tank.xml`,
`hares-autosized-boiler-indirect-tank.xml`, and the Python `tmp*` directories
(`tempfile`). All come from tests already on the head (the files or fixtures
exist at `2f85338d`, which is what `fix-testhyg` fixes), with one exception:
`hares-checkpoint-failed_step_row-*_diagnostics.csv` (6) comes from
`mod.rs:14068`, added by this branch (m2). Everything else the branch added
that touches a temp path (the `dr_preconditioning.rs` home file, the
`save_checkpoint_misaligned_streaks` toml) removes itself; none of the
branch's Python tests uses `tmp_path` or `tempfile`.

### 9. Consumer migration, `~/src/HARES_CALIBRATION_SYSTEM` (`i08-consumer`, `b71dbb4`)

Merged from `review-OD-16-2689f2af.md`, `-53c536ba.md`, `-2fb9f63b.md` and
this tip, re-checked by grep at `b71dbb4` (line numbers unchanged). The
nested untracked `HARES/` directory in the consumer repo is an old engine
copy and not consumer code.

Must change at the fold:

1. `calibration/ami2/dr_actuation.py:206`:
   `RELEASE = on.ControlSignal.thermal_setpoint(None, None, 0.0)` raises
   `ValueError` at import. Use `on.ControlSignal.thermal_setpoint_release()`.
   Rewrite the comments at `:175-205`, the docstring at `:12-38`, and the
   note at `:254` ("Not `thermal_setpoint_delta(0.0)`").
2. `calibration/tests/suite_ami2.py:877` and `:1762`: the same release
   constructor. Fix the comments at `:938-939` and `:1802-1804` (they
   describe `(None, None, None)` and `deadband_c=0.0` as the releases).
3. `calibration/tests/suite_ami2.py:1760` and
   `calibration/ami2/dr_actuation.py:248`: a positive
   `thermal_setpoint_delta(d, -d)` converges the setpoints and is rejected at
   dispatch; `d > 0` must be sent on one axis (`(d, None)` to heating
   equipment, `(None, -d)` to cooling), per unit and chosen from
   `Dwelling.thermostat_axes(name)` (new at this tip), never one pair for
   `for e in hvac`. `(-3.0, 3.0)`, `(-3.0)` and the default `(-1.0, 1.0)` at
   `dr_actuation.py:79` and `suite_ami2.py:885-886,1761` are relaxations and
   stay valid.
4. After every actuated run assert `dw.health().rejected_control_signals == 0`;
   dispatch rejections are counted, not raised. The consumer calls `health()`
   nowhere (grep).
5. `calibration/ami2/functions/_flexibility.py:219-221`: the note "the
   release must carry `deadband_c=0.0`" is wrong; point it at
   `thermal_setpoint_release()`.
6. `max_consecutive_step_failures` (new `SimulationConfig` field, default 1,
   `config.rs:148`; Python kwarg on `from_hpxml`, property on the config):
   `calibration/engine.py:352` (`from_hpxml(**kw)` built in `build_kwargs`
   around `:340-345`) should pass `max_consecutive_step_failures=0` and check
   `health().port_rollbacks`. The other `from_hpxml` call sites inherit the
   default: `calibration/scale.py:267`, `hares_ua_xval.py:72`,
   `hares_l1_archetype_sweep.py:77`. Add the kwarg to the `from_hpxml`
   registry in `calibration/registry.py` (`KWARG` rows, `:40`, `:305`) and to
   `INPUT_RUN_CONFIG_FIELDS` in `hares_contract.py` (checked by
   `hares_contract_check.py:117`; that check only fails on a field the
   adapter writes that no longer exists, so a new field shows as `NEW`, not
   `DRIFT`).
7. Budget-error handling: a run that exceeds the budget raises and the
   dwelling is terminal; `save_state` raises, `load_state` resumes the home,
   and a retry repeats the failing step. `calibration/coupling.py:399,408,420`,
   `calibration/cosim.py:74,90,102` and `calibration/scale.py:308-311,323-327`
   rewind with `load_state`: treat the budget error as fatal for that home,
   or change the input before retrying.
8. Stored checkpoints: `CHECKPOINT_VERSION` 12 rejects any blob from an
   earlier engine build. The consumer keeps `save_state()` blobs in memory
   only (`coupling.py:399`, `cosim.py:74`, `scale.py:308,323`; no file
   writes), so nothing to migrate.
9. If the consumer moves pre-conditioning to the engine's `DrCompliance`
   (today it is prose in `dr_actuation.py:36-39`, `suite_ami2.py:789-792`,
   `f7_dr.py:13`): register it with `add_actor_by_name("DrCompliance", ..)`
   after its equipment and name the direction
   (`SetpointAdjust:2.0:PreCool` or `:PreHeat`) for a unit serving both
   (read `Dwelling.thermostat_axes`); handle the `ValueError` at registration
   (a dual-mode unit without a direction, a direction the unit does not
   serve, a misspelt or absent name, a target with no thermostat); read the
   actor telemetry `preconditioning_unserved`; do not remove or swap the
   target afterwards (m1); use `ByName` for non-setpoint actions on an ideal
   unit (MJ1).
10. `DrAction` direction, `batch_step` `action_bounds` and the gym: the
    consumer does not use `ochre_next.rl`, `batch_step` or `DrAction`
    (grep), so nothing to change. A water-heater event needs
    `thermal_release()` at the end (the release restores the configured
    setpoint and band); `dr_actuation.py` actuates only HVAC.
11. PV: `calibration/der_inject.py:83-94` writes `ArrayTilt` and
    `ArrayAzimuth` and `hares_der_remeasure.py:85` passes `tilt` and
    `azimuth` to `add_pv`: no change. Nothing in `calibration/` calls
    `set_solar_override`, `register_surface`, or imports
    `MAX_THERMOSTAT_BAND_C` (replaced by `MAX_HVAC_THERMOSTAT_BAND_C` and
    `MAX_TANK_THERMOSTAT_BAND_C`).

### 10. Head drift: fold onto `db5efe80`

The head is 14 commits past `2f85338d` (`db5efe80` is `feat/output-results`).
In a scratch worktree at `db5efe80`: `git merge --no-commit 160ddb61` is
textually clean (0 conflicted files), and `git cherry-pick 2f85338d..160ddb61`
applies all 32 commits with no conflict. It does not compile:
`cargo check --workspace --all-targets` fails on three
`SimulationConfig` literals the head added, each missing
`max_consecutive_step_failures`:
`crates/hares-core/tests/equipment_init_failure_assembly.rs:40` and
`crates/hares-core/src/dwelling/solver_builder.rs:2018`, `:2097` (the same
class as the 13 literals fixed in this branch). With
`max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES`
added to those three (scratch only), `cargo check` is clean, `cargo fmt
--check` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and
`cargo nextest run --workspace` passes 7427 of 7427 (the 15 tests that read
`vendors/OCHRE` need the submodule, which the scratch worktree lacks; with
the submodule linked in, they pass). Not run on the fold: pytest, pyright,
observe, dst, profiling and release profiles. The fold must add the three
literals. The scratch worktree is removed and pruned.

### 11. Shipped text

Each of the 32 commits, message and added lines (the block's `[writing]`
patterns plus an en dash, plus `Co-Authored-By`): no match in any commit. Em
and en dashes, ticket and ledger IDs, review rounds, `V8` tokens and agent or
model references are absent. The earlier `55854486` and `071affc9` failures
are fixed in the rebased commits (`da92867b`, `2ae995f9`).

### Not run

pytest, pyright and the non-default profiles on the fold; the Python tests
for any behaviour in this review other than through the probes above (the
block's pytest run on the branch passed); a `HaresConfigError` catch check
on the roster methods other than `add_actor_by_name`; the `observe`-feature
read of `log_output_keys_without_columns` beyond clippy and the observe test
runs.

## Cleanup

`/home/rich/src/HARES/target-review-od16b`, the worktree's `target/` (probes,
the `git archive` copies, the block's outputs and `TMPDIR`), and the scratch
worktree `.workstrees/review-od16b-fold` are deleted. `git status` of the
worktree is clean; `vendors/OCHRE` is at `8681122b`. The only change outside
the report is the submodule checkout the task authorised.
