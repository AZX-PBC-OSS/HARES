# Review: OR-19 follow-up commit ba44d9f1

Branch `fix-or19` at `ba44d9f1` (on `f77054df`). It claims to fix the
findings of `redteam-OR-19-f77054df.md`: M-1 (BMS restarted by a tariff
rebuild), m-1 (restored cursor unchecked), m-2 (kept driver keeps the old
EV's parameters), m-3 (declared capabilities unpinned), m-4 (stale cursor
doc), m-5 (checkpoint files not bitwise), n-1 (`pub` stream items), n-2
(telemetry reads reset after a rebuild). It also changes `set_tariff` to
advance a new evaluator to the current step.

Scratch copy: worktree `.workstrees/check-ba44` (detached at `ba44d9f1`),
`CARGO_TARGET_DIR=.workstrees/target-fold-or19`, `-j 8`, rustc 1.94.1,
nextest 0.9.137. Scratch tests only, all included from untracked files
(`dwelling/rt_scratch.rs`, `actors/rt_bms_scratch.rs`,
`actors/ev_driver/rt_ev_scratch.rs`, `tests/rt_alloc_scratch.rs`). Nothing
committed. Worktree and target removed afterwards.

## Verdict: CHANGES-REQUIRED

Counts: blocker 0, major 3, minor 5, nit 2, info 5.

Every claimed fix holds and each is pinned by a test that fails when its
hunk is reverted, with three exceptions: the BMS day-cache reset, the
`adjust_for_pv` signal check, and the `float_roundtrip` pin under a
workspace build are not pinned (m-2, m-3, m-4). The majors:

- The new "prices and bills from the current step" claim holds for prices
  but not for bills.
- The EV driver's take-over misses the SOC gate latch, the same class of
  bug as the BMS restart this commit fixes.
- The tariff evaluator is not in the checkpoint, so a resumed run with a
  tariff is not continuous. This contradicts the commit's bitwise-resume
  claim.

Per the instructions, the fold (cherry-picking onto `feat/output-results`)
did not start.

## Claimed fixes: reverted hunk, failing test

| Claim | Revert applied | Result |
|---|---|---|
| BMS takes over its predecessor (prior M-1) | skip `actor.take_over` in the battery arm of `build_actors_from_seeds` | `reattaching_the_same_tariff_mid_run_changes_nothing` fails |
| Mid-run tariff priced from the current step | advance loop in `set_tariff` runs 0 times | `reattaching_the_same_tariff_mid_run_changes_nothing` fails (net power) |
| EV driver carries its telemetry (prior n-2) | drop the telemetry `clone_from` in `EvDriverActor::take_over` | `a_mid_run_tariff_keeps_existing_ev_driver_state` fails at the telemetry assertion |
| BMS carries its telemetry | drop the telemetry `clone_from` in `BatteryManagementActor::take_over` | `reattaching_the_same_tariff_mid_run_changes_nothing` fails at the first telemetry assertion |
| BMS recomputes its day thresholds from the new prices | drop `self.current_day_ordinal0 = u32::MAX` | **all 1300 `hares-core` tests pass**; only the scratch `rt_take_over_recomputes_thresholds_from_new_prices` fails (m-2) |
| In-kind replace keeps a built-in actor only on an equal seed (prior m-2) | `seed_unchanged \|\| true` | `replacing_an_ev_in_kind_rebuilds_its_driver_for_the_new_ev` fails |
| Bound actor's signals checked against its declaration (prior m-3), decide site | remove the call at `dwelling/mod.rs:4904` | `a_bound_actor_sending_an_undeclared_signal_fails_the_step` fails |
| Same, `adjust_for_pv` site | remove the call at `dwelling/mod.rs:5206` | **all 1300 `hares-core` tests pass** (m-3) |
| Same, declaration coverage | drop `SELF_CONSUMPTION` from the BMS declaration | 27 tests fail (BMS unit tests through the `debug_assert`, `equipment_id_regressions` through the step error) |
| Restored cursor checked (prior m-1) | `false &&` in the restore check | `a_checkpoint_cursor_behind_a_restored_driver_is_rejected` fails |
| Checkpoint files round-trip floats (prior m-5) | drop `float_roundtrip` from `hares-core` | `-p hares-core`: the three `checkpoint_file_resume` tests fail. `--workspace`: **all three pass** (m-4) |
| `pub(crate)` stream items (prior n-1), cursor doc (prior m-4) | read | done |

## Findings

### M-1 (major): a mid-run tariff does not bill from the current step; it bills from the simulation start

`set_tariff` (`dwelling/roster.rs:416`) advances the new evaluator's
`step_index` to `clock.current_step`. `TariffEvaluator::advance`
(`hares-tariff/src/evaluator.rs:401`) moves only the price index. The
`BillingState` keeps the period built in `TariffEvaluator::new`, which is
anchored at the simulation start (`billing.rs` `BillingState::new`,
`period_start = simulation_start`). `step()` closes a period once
`current_time >= period_end`, then `reset(period_end)`. The commit message
says "A tariff attached mid-run prices and bills from the current step".
Prices are now aligned, but billing is not.

Repro (scratch `rt_billing_*`): BESTEST 600, 3600 s, start
2024-01-01T00:00-07:00, URDB `pge_e_tou_c.json` ($10/month fixed, $12/kW
demand), `America/Los_Angeles`. Dwelling A has the tariff from step 0.
Dwelling B attaches it at step k. Both call `finalize_billing`.

- k = 0: summaries bitwise equal to A (same path).
- k = 240 (Jan 10 23:00 PST), run to day 75: the later periods are bitwise
  equal to A's, which confirms price alignment (across the Mar 10 DST
  change too). The first summary is `2023-12-31 23:00 PST .. 2024-01-31`,
  fixed 10.0000, import 96.4 kWh. The period starts ten days before the
  tariff existed, and the fixed charge is a full month, not prorated as
  `finalize` prorates the last partial period.
- k = 1800 (day 75, Mar 15), run to day 120: B emits **two fabricated
  bills** for periods that ended before the tariff was attached:
  `2023-12-31 .. 2024-01-31` and `2024-01-31 .. 2024-02-29`, each with
  energy 0.0240, demand 4.80, fixed 10.00, import 0.2 kWh, net 14.824. One
  bill is closed per step until the anchor catches up, and each holds one
  post-attach step's energy, so the first two steps after the attach are
  billed into January and February. The March summary then starts Feb 29,
  carries a full $10 fixed charge, and is missing those two steps.
- Re-attaching (or replacing) a tariff mid-period throws away the outgoing
  evaluator's open period. It is neither emitted as a summary nor carried
  over. In the reattach scenario (40 steps), the open period reads import
  59.29 kWh continuous against 43.65 kWh re-attached. So the name
  `reattaching_the_same_tariff_mid_run_changes_nothing` holds for dispatch
  but not for billing, which it does not assert.

No test covers billing for a tariff attached mid-run. The anchor behaviour
predates this commit (before it, prices were misaligned too). But the
commit adds the claim, and the billing summaries are a user-facing product
(`Dwelling.billing_summaries()`, golden billing products).

Fix: give the evaluator one entry point that positions it at a step, for
example `TariffEvaluator::start_at(step, time)`. It sets `step_index`,
starts the billing state at the attach time (the first period runs from
the attach time to the next cycle boundary, with fixed charges prorated as
`finalize` prorates the last one), and emits nothing for elapsed periods.
For a replacement, `set_tariff` finalizes the outgoing evaluator's open
period at the current time into `billing_summaries` before installing the
new one. The current behaviour is a silent wrong bill, so the alternative
is a loud refusal of a mid-run `set_tariff`. Pin it with the k = 240 and
k = 1800 cases above: no summary ends at or before the attach time, the
first summary starts at the attach time with a prorated fixed charge, and
every later period is bitwise equal to the from-start run.

### M-2 (major): the EV driver's take-over (and its checkpoint) misses the SOC gate's hysteresis latch

`EvDriverActor::take_over` (`ev_driver/mod.rs:337`) carries
`save_state`/`load_state` plus telemetry. `EvDriverSnapshot`
(`ev_driver/mod.rs:1182`) does not hold the composer's preference state.
`SocGate::charging_allowed` (`ev_driver/soc_gate.rs:23`) is a latch:
`constraint` (`soc_gate.rs:39`) closes it at the upper threshold and holds
it closed until SOC drops below the lower threshold (band 0.05). The
`LowSoc` and `QuickThenWait` strategies build a `SocGate`
(`ev_driver/mod.rs:159-178`). A driver rebuilt mid-hold starts with the
gate open.

Repro (scratch `rt_take_over_carries_the_soc_gate_latch`, actor level):
`QuickThenWait { partial_soc: 0.8 }`. At SOC 0.85 the gate closes (explicit
zero hold). `take_over` into a fresh driver gives an equal `save_state`.
At SOC 0.77, inside the band:

- continuous: `PowerSetpoint { active_power_kw: 0.0 }` (holds);
- rebuilt: `SOCTarget { target_soc: 0.8 }` (charges).

The copied telemetry then disagrees with the state. The rebuilt driver
reports `soc_gate_charging_allowed = 0` while its gate is open (the n-2
fix copies a value the state no longer has). The same omission makes a
checkpoint resume of these strategies non-continuous. That part predates
this commit, but the commit's claim "takes over the decision state" and
the restore-equivalence claim both cover it. This is the same class of
defect as the prior red team's M-1, which this commit fixed for the BMS
only.

Fix: carry each stateful preference's state in the driver snapshot, for
example a `ChargingPreference::save_state`/`load_state` pair, or the
composer serializing its preferences' latches. Bump the driver's
`checkpoint_version`. Pin it with the actor-level repro above and a
dwelling-level `LowSoc` reattach run whose net power and full driver state
are bitwise the continuous run's.

### M-3 (major, pre-existing, contradicts the commit's claim): the tariff evaluator is not in the checkpoint, so a resumed run with a tariff is not continuous

`DwellingCheckpoint` (`checkpoint.rs`) holds no tariff or billing state,
and `load_checkpoint` (`dwelling/mod.rs:3864`) does not touch
`tariff_evaluator`. A dwelling built with its tariff and then restored keeps
an evaluator at step 0 while its clock is at step k. Every price the actors
see is shifted by k steps, and the open billing period and prior-peak
ratchet history are lost.

Repro (scratch `rt_resume_with_a_tariff`): BESTEST 600 + `pge_e_tou_c` +
TOU battery. Continuous run 40 steps, then `save_checkpoint`. A fresh
dwelling built the same way (tariff and battery) restores it. Evaluator
`step_index`: continuous 40, resumed 0. Net power first differs at the 3rd
resumed step. The open period reads import 159.70 kWh continuous against
154.51 kWh resumed, and its end time differs.

The commit message says "a run resumed from a checkpoint file is bitwise
equal to the run that wrote it". `checkpoint_file_resume.rs` uses manifests
with no tariff. Golden manifests carry a tariff on the fold target
(`c6889589`), so any resumed tariff run is affected. The checkpoint shape
changes in this row anyway (cursor), so the evaluator's position and
`BillingState` belong in it now, at the same version bump. Pin it with the
repro above, extended through `save`/`load` and `billing_summaries`.

### m-1 (minor): the in-kind seed comparison reads live EV capacity, so an identically configured replacement never keeps its driver mid-run

`replace_equipment` keeps a built-in actor only when
`self.equipment[pos].actor_seed() == new_equipment.actor_seed()`
(`dwelling/roster.rs:850`). The EV seed's `capacity_kwh` is
`degradation_adjusted_capacity_kwh()` (`hares-equipment/src/ev/mod.rs:1971`),
which moves with the pack's daily degradation update. After 30 hourly
steps the installed EV's seed reads `capacity_kwh: 60.53999999999999`
against `60.0` for a fresh EV built from the same config.

Repro (scratch `rt_in_kind_replace_with_identical_ev_keeps_driver_state`):
EV1 (driven_ev config), 30 steps, then `replace_equipment("EV1",
identical)`. The driver is rebuilt on stream 2 instead of kept on stream
1, and its state resets. Both doc comments say an identically configured
replacement keeps the driver: `replace_equipment` in `roster.rs` and
`PyDwelling::replace_equipment` in `py_dwelling.rs`. In practice that
holds only before the first daily degradation update. The battery seed
holds no live quantity, so `actor_rebinds_to_replacement_equipment_after_replace`
passes.

Fix: compare configuration, not live state. For example, a seed built from
the rated capacity for the comparison, or a separate `actor_config_key()`.
Alternatively, rebuild from the replacement's seed and take over the old
actor's state (prior m-2's first option), which makes equality moot. Pin
it with the repro.

### m-2 (minor): the BMS day-cache reset in `take_over` is unpinned

Dropping `self.current_day_ordinal0 = u32::MAX` (`actors/bms.rs:52`)
leaves all 1300 `hares-core` tests green. A BMS rebuilt with a different
tariff then keeps the old tariff's charge and discharge thresholds until
the next midnight. Scratch
`rt_take_over_recomputes_thresholds_from_new_prices` (actor level: prices
tripled, one decide on the same day, threshold equals the percentile of
the new prices) fails without the line and passes with it. Add it, or a
dwelling-level equivalent with a different second tariff.

### m-3 (minor): the signal check at the `adjust_for_pv` site is unpinned

Removing `ensure_declared_signals` after `adjust_for_pv`
(`dwelling/mod.rs:5206`) leaves all 1300 `hares-core` tests green.
`a_bound_actor_sending_an_undeclared_signal_fails_the_step` only exercises
`decide`. Add a case whose undeclared signal comes from `adjust_for_pv`.

### m-4 (minor): the `float_roundtrip` pin is masked in every workspace block line

`frame_golden` already enables `serde_json/float_roundtrip`, and a
`--workspace` build unifies features. With the feature removed from
`crates/hares-core/Cargo.toml`, `cargo nextest run --workspace -E
'test(resumes_from_a_checkpoint_file)'` passes all three tests. Only
`[test-observe-profiling]` (`-p hares-core`) would catch the regression.
`hares-python` depends on `hares-core`, so the wheel does get the feature
today. Fix: declare the feature once on the workspace dependency
(`Cargo.toml` `[workspace.dependencies] serde_json`) and drop both
per-crate declarations, so every build of every crate parses JSON floats
the same way.

### m-5 (minor): a rebuild clears the EV driver's sticky health latch

`estimate_error` (`ev_driver/mod.rs:328`) is set by a failed
needed-hours estimate and never cleared. After a `decide` that sets it,
the step fails (`dwelling/mod.rs:4970`). `take_over` does not carry it, so
a caller that catches that error and then calls `set_tariff` gets a
healthy driver and the run continues past the failure. Carry it in
`take_over`, or refuse a rebuild while any built-in actor is unhealthy.

### n-1 (nit): `ensure_declared_signals` ignores the request's target

`undeclared_capability` (`actor.rs`) holds every request a bound actor
emits to the bound target's declaration, whatever `request.target` is.
Built-in actors send only to their own target today (`bms.rs:165`,
`ev_driver` `self.dispatch_target`), so nothing is wrong now. Filter on
`request.target` matching `dispatch_target_name()`, or document that a
bound actor sends only to its target.

### n-2 (nit): the declaration is checked in two places

The BMS (`bms.rs` `push`) and the EV driver (`populate_telemetry`) each
`debug_assert!` what `ensure_declared_signals` already enforces as a step
error in every profile. The step check alone covers every bound actor.
Keep one, or say why both are wanted.

### Info

- I-1 (allocation): the check allocates nothing on the success path.
  `profiling_alloc`-style measurement (scratch `rt_alloc_scratch.rs`,
  `-F profiling`, BESTEST + tariff + TOU battery + EV, 500 steps after 48
  warm steps) gives 47556 allocations with the check and 47556 with both
  call sites disabled. The cost is a `find_map` over the requests the actor
  emitted plus two virtual calls per request, and nothing for unbound or
  silent actors. `format!` runs only on the error path.
- I-2 (`float_roundtrip`): it changes parsing only, since `serde_json`
  serialization is unchanged. Through feature unification it reaches every
  crate built alongside `hares-core` (`hares-io` configs, `hares-tariff`
  URDB parsing, `JsonHandler` messages). Inputs parsed from JSON in a
  Python or library build can now differ from before by one ULP, towards
  the exact value. Goldens are unaffected because `frame_golden` already
  had the feature. The cost is init-time parsing, plus per-message parsing
  in `JsonHandler`; nothing in the step loop.
- I-3 (pre-existing, billing anniversary drift): `reset(period_end)`
  re-anchors the Monthly cycle on the clamped day. A Dec 31 start closes
  Jan 31, Feb 29, Mar 29, Apr 29, and the anchor never returns to the
  31st. `09-numeric-fixes.md` B-4's note records the anniversary semantics
  but not this drift. The BESTEST fixture start (`-07:00`) also anchors
  the Los Angeles tariff at Dec 31 23:00 PST.
- I-4 (pre-existing physics check): the EV's degradation-adjusted capacity
  rose above rating (60.0 to 60.54 kWh, a negative capacity fade) within
  the first simulated days. Worth confirming against the degradation
  model's reference (OCHRE `Battery.py` and its source) under the numeric
  entries.
- I-5 (held): with the tariff from the start and attached at step 0,
  billing summaries are bitwise equal. Periods wholly after a mid-run
  attach are bitwise equal across the Mar 10 DST change. `ActorSeed`
  `PartialEq` compares floats exactly, and NaN cannot arise from validated
  configs. The BMS take-over restores all ten snapshot fields, and every
  other BMS field is configuration rebuilt from the seed: after one
  post-reattach step, the BMS `save_state` is byte-equal to the continuous
  run for 30 steps.

## Not run

The fold was not started, so the full verification block was not run on
`ba44d9f1`. Run here: the commit's nine new or changed tests, the full
`hares-core` suite under three of the reverts, the BMS-declaration revert,
and the scratch cases above.

## Files

- crates/hares-core/src/dwelling/roster.rs (`set_tariff`, `replace_equipment`)
- crates/hares-core/src/dwelling/mod.rs (`ensure_declared_signals`, `load_checkpoint`, `build_actors_from_seeds`)
- crates/hares-core/src/actors/bms.rs (`take_over`)
- crates/hares-core/src/actors/ev_driver/mod.rs (`take_over`, `EvDriverSnapshot`)
- crates/hares-core/src/actors/ev_driver/soc_gate.rs
- crates/hares-core/src/checkpoint.rs
- crates/hares-core/Cargo.toml
- crates/hares-tariff/src/evaluator.rs, crates/hares-tariff/src/billing.rs
- crates/hares-equipment/src/ev/mod.rs (`actor_seed`)
