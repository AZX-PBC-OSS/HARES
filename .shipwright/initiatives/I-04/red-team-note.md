---
red_team_outcome: clean
failing_tests: []
---

## Red-team pass for I-04 (third round — attack the observed-SOC estimate semantics)

This pass targets what changed since the previous round: the
`needed_charge_hours` channel's face was changed from the driver's belief
to the **observed equipment SOC**, and `ChargingComposer::evaluate` was
made dispatch-only. Verdict: **clean** — no production defect found. Two
new attack tests were added and **both pass**: the change withstands the
attacks, and the suite gains permanent air cover over semantics of the new
code that nothing pinned.

### What changed since the last pass (the attacked surface)

`git diff --stat` confirms the change is confined to `composer.rs` and
`mod.rs` (all other files' diff stats identical to the previous round):

1. **`telemetry_estimate_context`** (mod.rs:836-846): the
   needed-charge-hours fold's context substitutes the observed equipment
   SOC (`actual_soc`) for `current_soc` whenever the equipment is
   observable, falling back to the belief in the warned
   unresolved-equipment mode — the same face as the `soc` channel, so the
   estimate tracks the shrinking gap during a charging session instead of
   freezing at the driver's belief.
2. **`ChargingComposer::evaluate` no longer folds** (composer.rs:83-93) —
   dispatch-only, deliberately, because its context carries the perceived
   SOC for dispatch and would clobber the observed-based estimate. The
   fold lives only in the actor's step-top refresh (mod.rs:939-940).
3. **`record_anxiety_plan_hours`** (mod.rs:853-857) now also runs on the
   observed SOC.
4. The implementer **rewrote the previous round's rollover test** to the
   new semantics (it now asserts the 23:59 estimate reports the observed
   0.9 gap ≈0 h despite the stale 0.5 belief, with an explicit
   stale-belief precondition so the ≈0 provably comes from the
   observation) and added
   `needed_charge_hours_decreases_while_equipment_charges_at_home`
   (mod.rs:1585) pinning that the estimate tracks the shrinking observed
   gap during a home charging session. Note-chain accuracy: the rollover
   test's previous belief-based ordering pin is subsumed, not
   contradicted — the belief no longer feeds the channel whenever the
   observation exists, which is a stronger guarantee than the ordering it
   tested.

### New attack tests (both passing — air cover, not findings)

All in `crates/hares-core/src/actors/ev_driver/mod.rs`. Each attacks a
specific, unpinned semantic of the new code; each names the mutation it
survives.

1. **`needed_charge_hours_under_anxiety_reports_observed_gap_not_belief`** —
   attacks the face of the *anxiety substitution* (mod.rs:853-857).
   Mutation it survives: swap `record_anxiety_plan_hours` back to the
   belief context and nothing else goes red — the previous round's anxiety
   tests plant observed == belief (0.2), so they pass under either face.
   The test drives belief 0.2 (anxiety fires) against observed 0.6 and
   asserts the exact observed-face charge-to-full figure (≈3.717 h), 3.7 h
   clear of the belief-face value (≈7.417 h) against a 1e-9 tolerance,
   with dispatch and static-belief preconditions asserted separately.
   **Result: pass.**
2. **`needed_charge_hours_decreases_during_away_charging`** — attacks the
   observed face in the **Away phase**, where the belief is mechanically
   static by design (frozen after the trip-completion credit), so it is
   the phase where the two faces diverge most and where a belief-face
   regression would freeze the channel through a whole workplace charge.
   The implementer's own decrease test covers only the home phase; the
   class guard only plugged-in steps. The test runs a 60-step away window
   with the observed SOC climbing 0.35 → 0.468 and 6.6 kW of away charge
   power reported, asserting the estimate falls >1.0 h (observed-face:
   ≈5.65 → ≈4.44 h) while proving the belief never moved. Mutation it
   survives: revert to the belief face and the value freezes
   (`last == first`). **Result: pass.**

Suite state after the additions: `cargo test -p hares-core --lib
ev_driver` → **179 passed, 0 failed** (177 + 2 new). `cargo clippy -p
hares-core --lib --tests -- -D warnings` → clean. `cargo fmt -p hares-core
-- --check` → clean. (All run after the final edit.)

### Mutation scan of the new code — what is already pinned (no test needed)

- **Delete the observed substitution from the fold** → the implementer's
  decrease test and the rewritten rollover test go red (both plant a
  frozen belief against a moving observation). Pinned.
- **Re-add the fold to `evaluate`** (perceived clobber on plugged-in
  driving-day steps) → the decrease test's steps are exactly that path.
  Pinned.
- **`record_anxiety_plan_hours` and `maybe_push_range_anxiety_override`
  call-site completeness** — grep-verified exactly two call sites each;
  the non-driving-day substitution value is pinned by the previous round's
  exact-value test.

### Attacks considered and rejected (no test written)

- **Flaky observation** (equipment core entry present one step, absent the
  next): the fold falls back to the belief and the estimate jumps — but
  the production equipment publishes its `CoreOutput` every step
  (hares-equipment/src/ev/mod.rs:1395-1411), the fallback is the
  documented degradation mode, and the `soc` channel already behaves
  identically, so both channels jump together consistently. No new defect.
- **Capacity fade vs. driver-config capacity in the estimate** (observed
  gap × rated capacity over-reports as the pack ages): pre-existing
  formula input, unchanged by this round, slow-moving (years). Not new
  surface.
- **Observed SOC above target**: the gap clamps to zero — pinned by the
  soc_target at/above-target zero tests.
- **Anxiety firing on belief while observed is *lower* than belief**
  (mirror-image divergence): the channel reports the larger observed gap
  while the driver idles — truthful reporting of the physical battery,
  coherent with the channel's stated face. No defect to prove.
- **Driving-phase face distinction**: belief and observed also diverge
  during driving, but the fold's face is chosen in exactly one place
  (`telemetry_estimate_context`) for the single phase-agnostic step-top
  refresh — there is no per-phase special case to regress, and the face is
  already pinned in two phases (home by the implementer's decrease test,
  Away by the new test above). A third phase adds no new failure mode.
- **Non-driving-day anxiety face**: the None-arm anxiety site and the
  `evaluate_charging` site share one helper (`record_anxiety_plan_hours`),
  so the face swap the new test survives would be in that one helper and
  is caught from either site; each site's call-existence is pinned by the
  previous round's exact-value tests. No fourth test.
- **Roll/refresh ordering under the observed face**: eliminated, not
  merely subsumed — when the equipment is observable the fold never reads
  the belief (ordering vs. reconcile is moot), and when it is
  unobservable `reconcile_soc` does nothing (no observation to order
  against). The hazard the previous round's rollover test pinned cannot
  exist under the new semantics.
- **One-step observation lag** (fold reads the previous step's committed
  output): documented semantics shared with the `soc`/`charge_kw`
  channels.
- **Concurrency**: single-threaded actor model; the refresh is a pure,
  idempotent fold (repeated `decide` on the same env republishes the same
  value).

### Conclusion

The observed-face change is coherent and correctly implemented: the fold
and both override substitutions run on one face, the dispatch path keeps
the belief face as the module doc requires, and the two previously
unpinned semantics (anxiety-substitution face, Away-phase tracking) are
now pinned by passing attack tests. No failing findings to report.
