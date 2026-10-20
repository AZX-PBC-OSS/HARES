---
"id": "I-04"
"title": "EV driver telemetry doesn't reflect charging"
"status": "active"
"kind": "fix"
"description": "Summary. The driver's own state channels do not track charging,
  on both construction routes including the working archetype one.\ 


  Evidence. 60 days, DailyCommuterL1, vehicle drew 449.9 kWh across 1037 live
  intervals: charge_kw identically 0.000; needed_charge_hours identically 0.000;
  soc monotone non-increasing, rises on 0% of charging intervals. plugged_in and
  phase work. On the HPXML route the pack reaches 0.000 on day 8 and the car
  draws a further 234 of its 318 kWh reading empty.\ 


  What we need. Are charge_kw and needed_charge_hours meant to be populated, and
  should soc reflect charging as well as driving?\ 


  Impact. We read charging from the EV End Use Electric Power (kW) column
  instead, so we're unblocked — but a caller trusting the driver's own state
  would conclude the car never charges.


  Check out other PRs, either build this on a worktree off of main, or maybe a
  PR  of the HPXML EV fix?  whatever makes most sense"
"created_at": "2026-09-13T04:33:07.455Z"
"created_by": "operator"
regression_tests:
  - crates/hares-core/src/actors/ev_driver/mod.rs
  - crates/hares-core/src/actors/ev_driver/soc_target.rs
---

Summary. The driver's own state channels do not track charging, on both construction routes including the working archetype one. 

Evidence. 60 days, DailyCommuterL1, vehicle drew 449.9 kWh across 1037 live intervals: charge_kw identically 0.000; needed_charge_hours identically 0.000; soc monotone non-increasing, rises on 0% of charging intervals. plugged_in and phase work. On the HPXML route the pack reaches 0.000 on day 8 and the car draws a further 234 of its 318 kWh reading empty. 

What we need. Are charge_kw and needed_charge_hours meant to be populated, and should soc reflect charging as well as driving? 

Impact. We read charging from the EV End Use Electric Power (kW) column instead, so we're unblocked — but a caller trusting the driver's own state would conclude the car never charges.

Check out other PRs, either build this on a worktree off of main, or maybe a PR  of the HPXML EV fix?  whatever makes most sense

## Implementation Notes

Answering the ticket's question directly: yes, `charge_kw` and
`needed_charge_hours` are meant to be populated, and yes, `soc` should reflect
charging as well as driving. All three channels now do, per the RCA's three
mechanisms (`rca.md`, "Root cause"), fixed in
`crates/hares-core/src/actors/ev_driver/`:

- **`soc`** reports the equipment's ground-truth SOC whenever the equipment
  core is observable, falling back to the driver's `estimated_soc` only in
  the pre-warned unresolved-equipment mode. The behavioral estimate and its
  documented divergence model are untouched — decisions still run on
  `perceived_soc()`.
- **`charge_kw`** reports, in order of precedence: a rate dispatched this
  step (`PowerSetpoint` / `EvAwayCharge` — the driver's own command), else
  the latest observed equipment charging power (typed
  `CoreOutput.flows.electric_kw` while plugged in at home; the equipment's
  `away_charge_power_kw` telemetry while away, since away charging is
  deliberately excluded from the residential flow). Goal-based dispatches
  (`SOCTarget`, `EvSetReadyBy`) no longer leave the channel structurally 0 —
  this covers every strategy uniformly, with no per-strategy branches.
- **`needed_charge_hours`** is a real estimate for every strategy:
  `SocTarget` and `SocGate` now provide time-to-charge through the same
  shared physics helper as `DepartureDeadline`
  (`preference::needed_charge_hours_to_target`), so the "no estimate"
  sentinel no longer collapses to 0.0 for `Immediate`, `Nightly`, `LowSoc`,
  `QuickThenWait`, `V2H`, or `V2G`. (`SolarSurplus` was never affected —
  its stack has always contained a `DepartureDeadline` whose estimate the
  fold picked up; the RCA's earlier claim that it collapsed was corrected,
  see the decision log.)
- **Override and non-driving-day paths** (`every_strategy_variant_…` class
  guard): a guard iterating all nine `ChargingStrategy` variants (with a
  compile-time exhaustiveness anchor) exposed a residual instance of the
  sentinel-collapse class the four RCA-named strategies had missed — when
  the range-anxiety override fires, `evaluate_charging` returns before the
  composer's fold, and the non-driving-day early return skips it entirely,
  so `needed_charge_hours` kept the initial "no estimate" sentinel → 0.0
  exactly when the battery was lowest. Both bypass paths now record the
  estimate for the plan actually dispatched (the override's charge-to-full
  target; the standing strategy plan on a resting non-driving day), via
  `ChargingComposer::refresh_needed_charge_hours` /
  `set_needed_charge_hours` — the fold has one home and every dispatch path
  feeds it.
- **Review round** (`needed_charge_hours_decreases_while_equipment_charges_at_home`):
  the telemetry-truth / dispatch-belief split was applied to `soc` and
  `charge_kw` but not to `needed_charge_hours` — the fold ran on
  `perceived_soc()`, freezing at the arrival-time gap through a whole home
  charging session while the other two channels reported ground truth
  ("not charging, battery full, still needs four hours"). The channel now
  runs on the observed equipment SOC via `telemetry_estimate_context`
  (belief fallback in the warned unresolved-equipment mode, same as `soc`),
  refreshed once per step; dispatch contexts keep `perceived_soc()` — the
  fold was removed from `ChargingComposer::evaluate` so the belief-based
  dispatch context can no longer clobber the observed-based estimate. The
  day-rollover test's 23:59 precondition was updated to the observed
  contract it was named for (its stale-belief assertion encoded the wrong
  semantic — sanctioned by the review), and the two anxiety exact-value
  tests are face-neutral (equipment SOC = belief in their setups) and pass
  unchanged.
- **Red-team round** (`needed_charge_hours_grows_as_soc_drains_while_driving`,
  `needed_charge_hours_survives_checkpoint_restore_while_driving`): the
  refresh sites were still coupled to *dispatch paths* (plugged-in arms
  only), so the channel froze at the last plugged-in value through the
  Driving/Away/departure/arrival steps while the battery drained, and a
  checkpoint restore into a fresh actor (whose composer estimate slot
  starts at the "no estimate" sentinel) re-published the collapsed 0.0
  mid-trip. Root fix, not per-arm patches: the standing-plan fold is
  refreshed once per step at the top of `decide` — every phase, every
  path — before any arm runs; the range-anxiety override paths replace it
  with the override's own (charge-to-full) estimate via the single
  `record_anxiety_plan_hours` helper. Both red-team tests pass with their
  assertions untouched.

### Deviations

- The reproduction test's environment mock now also publishes
  `flows.electric_kw` alongside `state.soc` (`set_core_charging` helper in
  the ev_driver test module), mirroring what the EV equipment actually
  commits to `CoreOutput` while charging. The assertion set is unchanged,
  and the test failed identically before the fix (the old
  `populate_telemetry` never read the environment at all). The fix reads the
  equipment's committed electric flow — the same face as the "EV End Use
  Electric Power (kW)" column the team was falling back to — rather than
  deriving power from the SOC delta, which would report a battery-side face
  the meter never sees and would only ever execute in tests (production
  equipment always publishes the flow).

### Known Limitations

- **Verification environment (not a code defect, but it will bite the next
  reviewer):** intermittent full-workspace failures and vanishing compile
  errors (e.g. `missing fields azimuth_deg/tilt_deg` in `solver_builder.rs`)
  were root-caused to a concurrent agent's build from the `/tmp/hares-f13`
  git worktree (branch `f13-final`) sharing this checkout's `target/`
  directory — identical crate name/version/features yield identical
  metadata hashes, so either tree can link the other's artifacts. One
  earlier full-suite run reported `1 failed` whose test name was never
  captured and could not be reproduced in five subsequent runs; every
  scoped and hermetic run was clean. All final gates here were run with an
  isolated `CARGO_TARGET_DIR`. Fixing the sharing (per-worktree target dirs)
  belongs to the environment owner, not this ticket.

- **What:** `charge_kw` under goal-based control reflects the equipment's
  charging power with a one-step lag, so the driver channel at output row N
  mirrors the equipment's end-use power column at row N−1 (RCA mechanism 1:\n  `charge_kw` only recognized rate signals — now observation-based for goal
  dispatches).
  **Constraint:** the dwelling's `run_timestep` runs actors against the
  previous step's committed equipment output (equipment phases step after
  actor `decide()`; the per-step snapshot in `dwelling/mod.rs` feeds the
  *next* step's actors), so the current step's power does not exist yet at
  the moment the driver populates telemetry.
  **Tried:** deriving power from the observed SOC delta (same lag, and a
  battery-side face rather than the meter face); reporting the driver's
  modeled estimate instead of an observation (not truth — the defect this
  ticket exists to fix).
  **Resolvable when:** never as a driver-side change — fundamental to the
  observe-then-decide ordering; a post-equipment telemetry refresh phase in
  the dwelling loop would remove it.
- **What:** when the target equipment is not registered
  (`resolve_equipment_id` unresolved — warned at startup), `soc` and
  `needed_charge_hours` degrade to the driver's belief and `charge_kw` to
  dispatched rates only, so the channels can again read as "never
  charging" / "still needs hours" in that mode.
  **Constraint:** `hares_types::Telemetry` is a `HashMap<String, f64>` with
  no absent/nullable channel, so "no observation" cannot be distinguished
  from a value without colliding with a meaningful number.
  **Tried:** sentinel encodings for "unobserved" — they collide with
  meaningful values, which is exactly the sentinel-collapse defect this
  ticket fixed for `needed_charge_hours`.
  **Resolvable when:** `Telemetry` gains nullable channels or per-key
  presence semantics.
