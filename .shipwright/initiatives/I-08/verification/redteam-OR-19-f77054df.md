# Red team: OR-19 roster plan/install, follow-up commit f77054df

Branch `fix-or19` at `f77054df` (chain `12f753be`, `a3a21a92`, `1bb045c6`,
`f77054df` on `1c18543a`). Worktree `.workstrees/verify-or19` (detached,
vendors/OCHRE initialized), `CARGO_TARGET_DIR=.workstrees/target-verify-or19`,
`-j 8`. Scratch tests only: an `include!("rt_scratch.rs")` in the dwelling
test module and an integration file `crates/hares-core/tests/rt_or19_resume.rs`.
Nothing committed; worktree and target removed afterwards.

## Verdict: CHANGES-REQUIRED

Counts: blocker 0, major 1, minor 5, nit 2, info 3.

Every claimed fix is real and pinned: each revert below makes its test fail.
The full verification block passes. The major finding is the tariff-rebuild
decision for the battery management actor: the commit keeps EV driver state
across a rebuild but restarts the BMS, on a justification that does not hold.
Re-attaching the identical tariff mid-run changes the dwelling's net power on
the very next step.

## Findings

### M-1 (major): a mid-run tariff rebuild restarts the battery management actor, so re-attaching the same tariff changes dispatch at once

`plan_built_in_actor_rebuild` (`dwelling/roster.rs:362`) passes the live
built-in actors as `rebuilt`. `build_actors_from_seeds` carries state over
only to EV drivers. The doc at `dwelling/mod.rs:6942` says "Battery management
actors are always built fresh: their state is derived from the prices they are
built with", and `roster.rs:388` says "a battery management actor restarts from
the new prices". Only three of the ten `BmsSnapshot` fields (`actors/bms.rs:980`)
come from prices: `daily_avg_price`, `charge_price_threshold` and
`discharge_price_threshold`, all keyed by `current_day_ordinal0`. The other
seven do not: `last_action` (charge/discharge hysteresis), `step_count` and
`current_mode_entered_step` (the min-dwell timer), `storm_watch_active`,
`dr_active` (the DR latch) and `dwell_blocked_count`. A fresh BMS mid-run
therefore runs with the dwell timer at zero, no hysteresis memory and a
cleared DR latch. Its cumulative telemetry also restarts from zero in the
output columns.

Repro (`rt_reattaching_the_same_tariff_mid_run_is_a_no_op` and
`rt_reattach_isolated`, BESTEST 600, 3600 s; battery `Bat` with
`TimeOfUseOptimization { min_duration_steps: Some(3) }` and `min_dwell_steps: 3`,
URDB `tests/fixtures/urdb/pge_e_tou_c.json`, `America/Los_Angeles`). Two
identical dwellings each run `set_tariff` and 40 steps. Then dwelling B
re-attaches the same tariff, and both run 96 more steps:

- BMS only: net power differs from the first step after re-attach. Step 0 is
  -3.44 kW continuous against +4.70 kW re-attached; the next steps are
  `[0.205, 0.205, 0.205, 0.205]` against `[1.96, 0.205, -4.80, -4.31]`. Before
  the step, BMS telemetry is `bms_action=2, bms_dwell_blocked=2,
  bms_toggled=1, soc=0.862` continuous against `bms_action=-1,
  bms_dwell_blocked=0, bms_toggled=0, soc=0` re-attached.
- EV only (TouAware): net power is identical for all 96 steps, so the driver
  carry-over works. Between the re-attach and the next step, the driver's
  telemetry still reads reset (`charge_kw` 7.2 against 0); see m-5.

This is not a regression against `1bb045c6`, which restarted both. But it is
the discontinuity the previous red team's I-2 reported ("discarding driver
state"), fixed for one of the two built-in actor kinds, with a stated reason
that is false. The tariff golden path is not reached by any manifest, so no
golden catches it.

Fix: carry the predecessor's `BmsSnapshot` over in the rebuild, as the driver
path does with `save_state`/`load_state`. Then invalidate only the
price-derived cache, by setting `current_day_ordinal0 = u32::MAX` (the
constructor's sentinel), so thresholds recompute from the new prices on the
next decide. Two ways to do this: an `Actor` hook that takes over a
predecessor (`take_over(&dyn Actor)`), or a BMS-specific branch beside the
driver's in `build_actors_from_seeds`. Correct the two doc comments. Pin it
with the repro above, asserting that re-attaching the identical tariff leaves
net power and BMS telemetry bitwise unchanged.

### m-1 (minor): the restored stream cursor is not checked against the restored drivers' streams

`load_checkpoint` assigns `self.next_ev_driver_stream = cp.next_ev_driver_stream`
(`dwelling/mod.rs:4008`) without checking it. Restore already validates
equipment names and ids, counts and actor schema versions loudly. It does not
check that the cursor lies past every restored built-in driver's stream.

Repro (`rt_restored_cursor_below_a_restored_driver_lets_two_drivers_share_a_stream`):

1. In the continuous run, add EV1, remove it and add it back (stream 2,
   cursor 2), step 4, then save a checkpoint.
2. Set `cp.next_ev_driver_stream = 1` (`DwellingCheckpoint` fields are `pub`)
   and restore into a fresh dwelling with EV1.
3. The restore returns `Ok`. Adding EV2 then puts EV1 and EV2 on the same
   stream: `EV1 stream 2 EV2 stream 2`.

A cursor beyond the reserved range restores fine and then rejects the next EV
loudly (`no EV driver RNG stream is left ...`). That case holds.

Fix: on restore, reject a cursor that is not past the highest offset any
restored built-in driver holds (`rng_pair()` is already on `Actor`). A test
should restore a lowered cursor and expect `Err`.

### m-2 (minor): the kept-actor rule checks signals, not configuration, so a driver kept across an in-kind replacement keeps the old EV's parameters

`replace_equipment` (`roster.rs:844`) now keeps an actor bound to the replaced
name when the replacement accepts every signal the actor requires. The EV
driver is built from the old EV's seed: `capacity_kwh`, `max_charge_kw` and
`fuel_economy_kwh_per_mi` are constructor parameters, not state. It keeps
those after the EV is replaced by another EV of another size. The
replacement's own seed builds nothing, because the name is held.

Repro (`rt_driver_kept_across_in_kind_replace_matches_a_fresh_driver`): EV1 at
30 kWh, replaced in kind by EV1 at 100 kWh, compared with a dwelling that had
the 100 kWh EV1 from the start (same stream). Driver telemetry differs:
`range_anxiety_active=1` (kept) against `0` (fresh). Net power over 7 BESTEST
days is equal in this fixture only because the BESTEST schedule never drives
the EV.

A tariff rebuild (M-1 path) later rebuilds this driver from the new seed with
the old state, so its parameters jump at an unrelated call. Fix: on an in-kind
replacement, rebuild a built-in actor from the replacement's seed and carry
its state over. The mechanism is the same as M-1's predecessor carry-over.
Alternatively, evict it when the replacement's `actor_seed()` differs from the
seed the actor was built from.

### m-3 (minor): an actor's declared capabilities and the signals it sends are two sources of truth, and the BMS declaration is unpinned

`Actor::required_control_capabilities` (`actor.rs:67`) is hand-listed beside
the signals each actor emits. The two lists match today:

- BMS: PowerSetpoint, SOCTarget, PowerLimit, GridConnect, SelfConsumption.
- EV driver: PowerSetpoint, SOCTarget, EvPlugIn, EvDrive, EvAwayCharge,
  EvSetReadyBy.

They are checked against `actors/bms.rs` and `actors/ev_driver/{mod,composer}.rs`.
Only the BMS and the EV driver override `dispatch_target_name`, so no live
actor that binds by name relies on the empty default. Nothing ties either
list to the signals, though:

- Replacing the BMS declaration with `ControlCapabilities::empty()` leaves all
  1335 `hares-core` tests green. Only my scratch test failed.
- Any actor that overrides `dispatch_target_name` and forgets the second
  method silently keeps its binding across an in-kind replacement that
  refuses its signals.

Fix: pin it with a test that drives each built-in actor through its modes and
asserts that every emitted signal's `required_capability()` is in its
declaration. Better still, make the binding one declaration, for example a
`dispatch_target() -> Option<(&str, ControlCapabilities)>`, so a target cannot
be declared without its capabilities.

### m-4 (minor, stale doc): the cursor field still says a rebuild restarts it at 0

`dwelling/mod.rs:1762-1765`: "A rebuild of every built-in actor restarts it at
0". `f77054df` removed the reset (the cursor now only grows). No code writes 0
except construction (`mod.rs:2724`). Reword the comment to say the cursor
never moves back and is checkpointed.

### m-5 (minor, pre-existing, exposed by the claim): a resumed run through the checkpoint file is not bitwise-continuous

`DwellingCheckpoint::save`/`load` (`checkpoint.rs:120,181`) use `serde_json`.
The `float_roundtrip` feature is enabled only by `frame_golden`, so a
`hares-core` or `hares-python` build without it parses some `f64` one ULP off.

Repro (`rt_or19_resume.rs` with `RT_FILE=1`, release). The first resumed
step's zone temperature differs by 1 ULP:

- `full_year_hourly`, step 29, zone 2: `-6.475377404377305` against
  `...304`.
- `weather_wrap`, step 191, zone 1: `29.52179059318259` against `...594`.
- `leap_year_february`, net power at step 203: `1.0725342325188838` against
  `...825`.

With the in-memory checkpoint all four fixtures are bitwise equal (see What
held). The commit's message and tests present resumption as "as it would in
the uninterrupted run". That holds in memory, not through the file that
resumption uses. Fix (separate row, or here since the checkpoint format moved
to v9): enable `float_roundtrip` for `hares-core`'s `serde_json`, and run the
resume test through `save`/`load`.

Related, also outside OR-19 (info-level): restored `latest_env` still differs
from the continuous run in `ambient_other_space_c` (computed from the
construction-time zone buffer), `electrical` (reset by `update_in_place`), the
humidity solver's custom domain entry, and `equipment_core` performance
fields (`main_power_kw: None`). The commit claims only time and weather, and
those match. No equipment initialised after a resume read the differing
fields in four fixtures (EV2's core output and telemetry were identical).

### n-1 (nit): new public items with no consumer outside the crate

`rng::ev_driver_stream` and `rng::EV_DRIVER_STREAM_COUNT` (`rng.rs:29,40`) are
`pub` in a `pub mod`. Nothing outside `hares-core` uses them. Make them
`pub(crate)`.

### n-2 (nit): actor telemetry reads reset between a tariff rebuild and the next step

The rebuilt driver is a new object, so `Actor::telemetry()` between steps
shows constructor values, for example `charge_kw` 0 against 7.2. A Python
caller polling `dw.telemetry().actors()` right after `set_tariff` sees this.
It corrects itself on the next step. Fold it into M-1's take-over hook
(copy the telemetry too) or document it.

### Info

- I-1: `load_checkpoint` stays non-atomic: clock, RNG and equipment state are
  set before later fallible steps. The new `update_in_place` call
  (`mod.rs:3904`) adds one more mutation before them. This was not
  introduced here.
- I-2: `remove_equipment` does not drain the removed equipment's pending
  warnings into the log; `replace_equipment` does for the evictee. The caller
  receives the equipment, so nothing is lost, but the two removal paths
  differ.
- I-3: `PyActorWrapper::decide` refreshes its cached name from Python each
  step (`py_actor.rs:110-116`), so a Python actor can rename itself past the
  duplicate-name check. This predates the commit and is outside the roster
  change.

## Claimed fixes: verified and pinned (revert in the worktree, test fails, restored)

| Claim | Revert applied | Failing test(s) |
|---|---|---|
| Cursor in checkpoint (v9) | drop `self.next_ev_driver_stream = cp.next_ev_driver_stream` | `a_resumed_dwelling_gives_a_new_ev_driver_the_stream_of_the_continuous_run` |
| Restored env = last step | disable the `update_in_place` block in `load_checkpoint` | `a_restored_dwelling_reports_the_environment_of_its_last_step`, and the resumed-stream test (EV2 trajectory) |
| Bounded driver range | `ev_driver_stream` always `Ok` | `exhausted_ev_driver_streams_reject_the_next_ev`, `rng::ev_driver_streams_stay_between_...` |
| Tariff rebuild keeps driver state | pass `&[]` as `rebuilt` | `a_mid_run_tariff_keeps_existing_ev_driver_state` |
| `RejectedEquipment` always | wrap only when warnings exist | `rejected_add_and_replace_return_rejected_equipment_without_warnings`, `add_equipment_with_actors_rejects_both_or_neither`, `rejected_add_leaves_roster_unchanged`, `rejected_replace_keeps_the_original` |
| Candidate before supplied actors | restore names-first order | `add_equipment_with_actors_rejects_both_or_neither`; pytest `test_duplicate_add_ev_with_driver_reports_the_duplicate_equipment` |
| `add_pv` surface after accept | register before `add_equipment` | pytest `test_rejected_add_pv_registers_no_irradiance_surface` |
| Evict on missing capability | always keep on same name | `replace_in_kind_evicts_actors_whose_signals_the_replacement_rejects` |
| No equipment code in install | move the `core_output` read back into install | `a_panicking_candidate_leaves_the_dwelling_unchanged` |
| Every entrance atomic (prior m5) | `remove_equipment` commits before planning | `every_roster_entrance_leaves_the_dwelling_unchanged_when_its_plan_fails` |
| Rejected plan leaves file alone | truncate the output file at plan start | `a_rejected_roster_change_leaves_the_live_output_file_untouched` |
| Em dashes | `[writing]` on f77054df and on the cumulative `1c18543a..f77054df` diff | clean (1bb045c6 alone still fails `[writing]`; the fold removes it) |
| Release `-F observe` compiles | `[clippy-release-observe]` | passes |

Two pins were confirmed by reading: the Python mapping of the reason classes
(`rejected_equipment_raises_the_exception_of_its_reason`, which runs
`to_py_err` under `Python::attach`) and the `plan_loop_types` and
`fluid_loop_declarations` unit tests (the previous review's m4). Their
assertions cover the reverted behaviour directly.

## What held

- Removed EVs before a checkpoint: the continuous run adds EV1 and EV2, then
  removes EV1 (cursor 2). The resumed run replays that roster history (equipment
  ids must match), has its cursor forced to 0, and restores. EV2 gets its
  stream back, EV3 gets the continuous run's stream (3, distinct from EV2's),
  and 96 steps are bitwise equal.
- Resume equivalence in memory: `consumer_shape`, `full_year_hourly`,
  `leap_year_february` and `weather_wrap` inputs (manifest HPXML, schedule,
  weather, start, resolution, warm-up). Each was checked with and without an
  EV added before the checkpoint, with a checkpoint mid-run, an EV added after
  the resume, and 2-3 days in total. Every `StepResult` is bitwise equal,
  including after the post-resume add. The first resumed step sees the same
  environment: `step()` re-runs `feed_zones` and `update_in_place` for its own
  step, and nothing reads `latest_env` before that, except the per-step
  checks on `equipment_core`.
- Pre-step tariff: attaching the TOU tariff before or after adding the
  battery and EV gives the same actor order, the same driver stream and
  bitwise-equal 120 steps.
- Exhaustion: a restored cursor beyond the bound rejects the next EV with a
  named `Dwelling` error inside `RejectedEquipment`, and the fingerprint is
  unchanged (the commit's own test).
- `required_control_capabilities` coverage: only the BMS and the EV driver
  bind by name, and both declare what they send (m-3 is about pinning, not
  current correctness).
- Panic safety: after commit, install calls only `Actor::name` (also from
  `commit_actor_change` and `rebuild_schedule`) and
  `Actor::resolve_equipment_id`. No equipment code runs after commit.
  `replace_equipment` drains the evictee's and the candidate's warnings and
  calls `mark_initialized` before any field is assigned.
- `RejectedEquipment`: every rejection of an add, add-with-actors or replace
  in Rust returns it; Python classes by the reason, and every Python entrance
  reaches it through `add_equipment`, `add_equipment_with_actors` or
  `replace_equipment`.
- Step path: no step hunk in `f77054df`; the goldens are identical.

## Verification block (worktree at f77054df; rustc 1.94.1, nextest 0.9.137)

- `[fmt]` pass. `[clippy]`, `-default`, `-dst`, `-profiling`, `-observe`,
  `-observe-profiling`, `-release`, `-release-dst` and `-release-observe` all
  pass.
- nextest, 0 failed and 0 skipped in each: default 7186; `-F observe` 7245;
  `-F dst` 7199; `-F profiling` 7192; `-p hares-core -F observe,profiling`
  1381; `--release` 7143.
- `profiling_alloc` passes under `-F profiling`, debug and release.
- `[bench]` `--no-run` passes.
- `[golden]`: default, 5 fixtures identical; `-F dst` `full_year_900s_dst`
  identical; `-F profiling` 5 identical.
- `[python-build]` passes. `[pytest]` `-m ""`: 1383 passed, 9 skipped, and the
  skip census matches. `[stray]` clean.
- `[writing]`: f77054df clean; cumulative range clean.
- Not run: `[advisories]` (`cargo deny`).

## Files

- crates/hares-core/src/dwelling/roster.rs
- crates/hares-core/src/dwelling/mod.rs
- crates/hares-core/src/actors/bms.rs
- crates/hares-core/src/actor.rs
- crates/hares-core/src/rng.rs
- crates/hares-core/src/checkpoint.rs
- crates/hares-python/src/py_dwelling.rs
- tests/python/test_roster_changes.py
