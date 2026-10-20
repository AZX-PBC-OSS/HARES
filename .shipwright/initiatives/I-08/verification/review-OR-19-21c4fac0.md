# Review: OR-19 follow-up commit 21c4fac0

Branch `fix-or19` at `21c4fac0` (on `ba44d9f1`). It claims to fix the
OR-19-owned findings of `review-OR-19-ba44d9f1.md`: M-2 (SOC gate latch lost
through `take_over` and the checkpoint), m-1 (seed compared on live
capacity), m-2 to m-4 (unpinned BMS day reset, `adjust_for_pv` check and
workspace `float_roundtrip`), m-5 (sticky `estimate_error` cleared by a
rebuild), n-1 (declaration check ignores the request's target) and n-2
(two checks). Out of scope, tracked as their own entries: mid-run tariff
billing periods and the tariff evaluator checkpoint (M-1, M-3 of the prior
review), the Monthly anchor drift and the EV capacity above rating (I-3,
I-4).

Scratch copy: worktree `.workstrees/check-21c4` (detached at `21c4fac0`),
`CARGO_TARGET_DIR=.workstrees/target-fold-or19`, `-j 8`. Every revert was
made with the editor in the scratch copy and undone before the next one.
Baseline: `cargo nextest run -p hares-core`, 1340 of 1340 pass.

## Verdict: PASS

Counts: blocker 0, major 0, minor 0, nit 2, info 4.

Every claimed fix holds, and each is pinned: reverting it fails exactly the
test written for it, and nothing else in `hares-core` (or
`hares-equipment` for the seed).

## Claimed fixes: reverted hunk, failing test

All runs: `cargo nextest run -p hares-core -E 'not test(regression_full_suite)'`
(1339 tests) unless stated.

| Claim | Revert applied | Result |
|---|---|---|
| Gate latches carried through `take_over` and the checkpoint (M-2) | `load_state` ignores `snap.gate_latches` | 1 fails: `take_over_and_restore_carry_the_soc_gate_latch` (covers both the take-over and the `save_state`/`load_state` restore) |
| Sticky estimate failure carried (m-5) | `load_state` ignores `snap.estimate_error` | 1 fails: `take_over_keeps_the_estimate_failure_latch` |
| Driver checkpoint version 3 | read | the version-gate test asserts `expected=3`; a v2 blob is rejected by name before `postcard` |
| Seed holds rated capacity (m-1) | `capacity_kwh: self.degradation_adjusted_capacity_kwh()` (`-p hares-core -p hares-equipment`, 3937 tests) | 1 fails: `an_identical_ev_replaced_mid_run_keeps_its_driver` (driver stream 2, expected 1) |
| BMS day reset pinned (m-2) | drop `self.current_day_ordinal0 = u32::MAX` in `BatteryManagementActor::take_over` | 1 fails: `take_over_recomputes_the_day_thresholds_from_the_new_prices` |
| `adjust_for_pv` signal check pinned (m-3) | remove the `check_declared_signals` call after `adjust_for_pv` | 1 fails: `an_undeclared_signal_from_the_pv_re_evaluation_fails_the_step` |
| Target-scoped declaration (n-1) | `... if **name == *target) \|\| true` in the filter | 1 fails: `a_bound_actors_declaration_covers_only_its_target` |
| `float_roundtrip` on the workspace dependency (m-4) | drop the feature from `[workspace.dependencies] serde_json` | `--workspace`: 4 fail (`checkpoint_file_round_trips_floats_exactly` and the three `checkpoint_file_resume` tests). `-p hares-core`: the same 4 fail. The workspace masking of the prior review is gone, since no crate declares the feature on its own |
| One check (n-2) | read | the dwelling and both built-in actors call the same `check_declared_signals`; the doc says why the actors also assert it (their unit tests hold their declarations without a dwelling) |

## Saved-state audit: every field against the snapshot

`EvDriverActor` (`ev_driver/mod.rs`):

- In `EvDriverSnapshot`: `estimated_soc`, `current_day_ordinal`,
  `todays_event`, `phase`, the RNG (seed, stream, word position),
  `needs_away_charge`, `drive_cancelled`, the composer's gate latches,
  `estimate_error`.
- Configuration rebuilt from the seed and spec: `name`, `dispatch_target`,
  `strategy`, `plug_in_policy`, the four schedule sources,
  `event_day_ratio`, `fuel_economy_kwh_per_mi`, `capacity_kwh`,
  `max_charge_kw`, `average_speed_mph`, `range_anxiety_miles`,
  `away_charge_fraction`, `away_charge_power_kw`, `expected_daily_miles`
  (the mean of the miles schedule).
- Rebound or recomputed before use: `equipment_id`
  (`resolve_equipment_id`), `time_res_minutes` (set at the top of every
  `decide`).
- `telemetry`: carried by `take_over`; not in the checkpoint, and every
  `decide` repopulates it before it is read.
- Composer: `scratch` is a buffer. `last_action`,
  `last_resolved_target_soc`, `last_resolved_power_kw` and
  `last_resolved_max_soc` are written by `evaluate` and read only by
  `range_anxiety_override_request`, which runs after `evaluate` in the
  same step (its only caller, `evaluate_charging`).
  `last_needed_charge_hours` is refreshed at the top of every `decide`
  before any read.
- Preferences: `SocGate::charging_allowed` is the only cross-step state of
  `SocGate`, `SocTarget`, `DepartureDeadline`, `TimeWindowPref`,
  `SolarTracking`, `V2GExport` and `V2HDischarge`. `PriceOptimizer` caches
  its day thresholds behind `current_day = u32::MAX` at construction, so a
  rebuilt driver recomputes them from its own prices on its first decision
  (the counterpart of the BMS reset, correct by construction) and a
  restored one recomputes the same values from the same prices.

`BatteryManagementActor` (`bms.rs`): the ten `BmsSnapshot` fields
(`current_day_ordinal0`, `daily_avg_price`, both thresholds, `last_action`,
`storm_watch_active`, `dr_active`, `step_count`,
`current_mode_entered_step`, `dwell_blocked_count`) are its whole runtime
state. `telemetry` is carried by `take_over`. `equipment_id` is rebound.
Everything else (`name`, `dispatch_target`, `bms_mode`,
`grid_export_rule`, power limits, `price_schedule`, `steps_per_day`,
`min_dwell_steps`) is configuration from the seed.

The only actors rebuilt from seeds are these two (`ActorSeed::Battery`,
`ActorSeed::Ev`), and the only `take_over` callers are the two rebuild
sites in `dwelling/mod.rs`. Nothing else is missed.

## Findings

### n-1 (nit): `restore_charging_allowed` defaults to a no-op

`ChargingPreference::charging_allowed` and `restore_charging_allowed` are
separate trait methods, and the restore has an empty default. A future
gating preference that reports `Some` from `charging_allowed` but does not
override the restore would save its latch and silently drop it on
restore; the latch-count check would still pass. One method pair that
cannot diverge (for example `fn gate_latch(&mut self) -> Option<&mut bool>`
used for both save and restore) closes it.

### n-2 (nit): the target filter matches only `DispatchTarget::ByName`

`check_declared_signals` holds requests addressed `ByName(target)`. A bound
actor that sends a `ByEndUse` request reaching its own target escapes the
declaration. No built-in actor does this (both send only
`self.dispatch_target`, a `ByName`). Say so in the doc, or hold `ByEndUse`
requests of a bound actor too.

### Info

- I-1: the requested dwelling-level `LowSoc` reattach pin was not added.
  The actor-level test drives the same `take_over` and `load_state` the
  dwelling calls, and the dwelling's routing into `take_over` is pinned by
  `a_mid_run_tariff_keeps_existing_ev_driver_state`, so the dwelling-level
  case would add no revert-detecting power.
- I-2: the seed now carries the rated capacity. At construction the fade
  is 0, so a fresh EV's seed and every from-config build are unchanged; a
  tariff rebuild mid-run now seeds the same capacity the original driver
  was built with, which is more continuous than before.
- I-3: `EvDriverActor::load_state` assigns fields before
  `restore_gate_latches` can fail, so a latch-count mismatch leaves a
  partly loaded actor. In `take_over` the target is the new, discarded
  actor; in `load_checkpoint` the restore was already non-atomic for
  equipment blobs. Not a roster-change path.
- I-4: the test `ev_driver_v1_snapshot_blob_rejected_by_named_version_gate`
  now tampers a v3 blob to 1; its name states a version, not the
  behaviour. Renamed in the fold.

## Writing check on the six OR-19 commits

`12f753be`, `a3a21a92`, `f77054df`, `ba44d9f1`, `21c4fac0`: clean.
`1bb045c6`: four added lines carry an em dash (error messages in
`dwelling/roster.rs`); reworded as it is replayed in the fold.

## Files

- crates/hares-core/src/actors/ev_driver/mod.rs (`EvDriverSnapshot`, `save_state`, `load_state`, `take_over`)
- crates/hares-core/src/actors/ev_driver/composer.rs (`gate_latches`, `restore_gate_latches`)
- crates/hares-core/src/actors/ev_driver/preference.rs, soc_gate.rs
- crates/hares-core/src/actors/bms.rs (`take_over`, `BmsSnapshot`)
- crates/hares-core/src/actor.rs (`check_declared_signals`)
- crates/hares-core/src/dwelling/mod.rs (both check sites, new tests)
- crates/hares-equipment/src/ev/mod.rs (`actor_seed`)
- Cargo.toml, crates/hares-core/Cargo.toml, tools/frame_golden/Cargo.toml
