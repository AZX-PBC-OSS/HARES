---
plan_outcome: blocked
---

# Fix plan (revision 9, post-critique): silence-contract fixes complete and verifiable; blocked on an operator decision about a separately-tracked HPXML defect (ticket 132)

## Target

The RCA's confirmed root cause: the actor/equipment "silence" contract is
undefined. When `ChargingComposer::evaluate` resolves to an idle vote
(`TimeWindowPref` outside its window, `SocGate` above its threshold, or any
other preference using `PreferenceVote::idle(...)`), `emit_vote` dispatches
nothing. A plugged-in EV that receives no instruction charges itself toward
`ready_soc` (default `soc_max = 1.0`) at rated power — the BMS's documented
default for an un-actored vehicle, silently applied to an actor-controlled
one that is momentarily silent. Every `ChargingStrategy` variant that ever
idles therefore converges on "charge on plug-in," which is the ticket's
symptom.

This plan also covers the two `range_anxiety_override_does_not_preempt_*`
tests. The RCA frames that mechanism (the override preempting a strategy's
own gate in the 0.15–0.28 SOC band) as a secondary, separately-diagnosed
defect — but this flow's `fix` step gate (`must_pass: from_key
failing_tests`) inherits all four tests `reproduce` listed, not just the two
that exercise the primary defect. A plan that leaves two of the four gate
tests red is not `ready`. Fixes 1 and 2 are scoped below as two
independent, separately-verifiable changes, landing together because the
flow requires both, not because they share a root cause.

**Revision 4 note (superseded by revision 5 below, kept for history):**
`plan-fix-critique` returned `findings` on revision 3 (Critical C1, C2;
Warning W1-W5). C1 (a claimed same-step dispatch-order bug) was addressed
with a direct code trace showing the claimed mechanism did not occur under
revision 4's structure — no design change was needed for it *then*. C2
(Fix 1 interacting with the untouched `decide` `None` arm to regress
non-driving days) was real and closed with a new Fix 3.

**Revision 5 note:** `plan-fix-critique` returned `findings` again on
revision 4 (Critical C1 — new, unrelated to the withdrawn prior C1;
Warning W1-W4). The critique's own reflection confirmed the prior round's
C1 was correctly refuted and withdrew it — the plan was right to verify
rather than redesign that time. This round's findings:

- **C1 (new): the latched hold is never cleared on disconnect, silently
  zeroing away charging.** Real, confirmed by trace against
  `EvPlugIn`/`AwayPluggedIn`/`EvAwayCharge` — closed by adding a targeted
  clear to `EvPlugIn`'s `Disconnected` arm (Fix 1, revised).
- **W1: Fix 3's claim about `DepartureDeadline::resolve_departure`
  falling back to `Inactive` on a non-driving day was wrong** — corrected
  with the actual fallback behaviour and an explicit, reasoned acceptance
  of its consequence, rather than a false safety claim.
- **W2: a second, independent silent-return path in `emit_vote`
  (`power_kw` exactly zero) survived Fix 1's idle check.** Closed by
  restructuring `emit_vote` to remove the special case entirely, so one
  rule covers both shapes.
- **W3: the integration test's acceptance criterion was unsatisfiable
  whenever the (correctly still-firing) anxiety override lands outside a
  configured window.** Closed by adding a precondition on the test's
  initial SOC/drive parameters, computed the same way Fix 2 computes the
  anxiety band, so the override provably does not fire during the test.
- **W4: the `emit_vote` `return` also silently dropped `target_soc`
  whenever `power_kw` was present** (beyond V2H/V2G, also `SolarSurplus`/
  `TouAware`) — fixed now, in the same `emit_vote` restructure as W2,
  rather than deferred, per the constitution's no-untracked-deferrals
  rule; also resolves the V2H/V2G instance the prior round left
  out-of-scope, as a direct consequence rather than a separate change.

This is not a re-scoping: every change below exists because a named
finding required it. No new fix targets anything the critique did not
raise.

**Revision 6 note:** `plan-fix-critique` returned `findings` again on
revision 5 (Critical C1, C2; Warning W1-W4), concentrated in the
interaction between residuals the plan accepted and the Verification
criteria the same plan required — self-contradictions the critique's own
reflection called "closable within the design by deciding," not a design
problem:

- **C1: the plan accepted an arrival-step residual three sections before
  requiring, in Verification, that exactly the quantity that residual
  produces be near-zero "for every simulated day."** Resolved by closing
  the residual rather than narrowing the criterion: the `Away` phase's
  arrival branch now calls `evaluate_charging` in the same step as the
  `HomePluggedIn` transition (Fix 1's disconnect-clearing subsection),
  so there is no longer a residual for the criterion to contradict.
- **C2: the Verification section's anxiety-band precondition and its
  `LowSoc` observable are mutually exclusive for the ticket's literal
  `LowSoc { threshold: 0.15 }`, because the hardcoded seed inputs put
  `anxiety_soc` above `0.15` regardless of the test's own chosen
  parameters.** Resolved by naming the correct lever (the test's `LowSoc`
  threshold, set above the computed `anxiety_soc`) and stating plainly
  that this demonstrates the mechanism rather than reproducing the
  ticket's literal value, which remains the already-tracked hardcoded-
  anxiety-band residual.
- **W1: the idle-check comment still overstated "the actual definition of
  'nothing to say'"** — a `{departure_hour}`-only vote (reachable via
  `TouAware`'s stack and `resolve`'s tie-breaking) passed all three of
  revision 5's steps undispatched. Fixed by dropping
  `departure_hour.is_none()` from the condition entirely — a
  `departure_hour` alone is not actionable by any of the three
  `ControlSignal` variants `emit_vote` can produce, so excluding it from
  "nothing dispatchable" was the wrong claim, not an incomplete one.
- **W2 (document-only): "one change closes the class" overstated what the
  hold reaches** — corrected to scope the claim to idle-*shaped* votes,
  naming which strategies (`Immediate`, `TouAware`, `PreDeparture`, `V2H`,
  `V2G`) charge at rated power by design, not by the bug this plan fixes.
- **W3: `soc_target` was left uncleared on disconnect on an unverified
  "might still matter,"** while `power_setpoint_kw` was cleared on a
  checked absence of any relying caller. Fixed by applying the same
  check to `soc_target` (none found) and clearing it too, for the same
  reason and to the same evidence standard.
- **W4: the accepted phantom-deadline consequence (Fix 3) named that a
  deadline latches but not that doing so defeats Fix 1's hold** for
  `TouAware`/`PreDeparture` on a non-driving day. Completed, not
  reversed — the acceptance still stands, stated fully.

O1–O4 (document-only) addressed in place: a check recorded (O2 → now
this round's O2 equivalent), two line-citation drifts corrected (O3), and
`before_out`'s unchanged role named explicitly (O4). O1 (document length)
is noted but not acted on — restructuring history out of this file into
the decision log is a larger, separately-scoped editorial change the
critique itself flagged as "not a correctness issue," and reorganizing an
882-line document under an active findings loop risks introducing exactly
the kind of cross-section contradiction C1/C2 this round were about,
for no verification benefit.

**Revision 7 note.** Mid-round the operator stated: "no broken windows,
we must fix all issues, pre-existing or not" — the critique applied this
to every place revision 6 said "not this ticket," "accepted," or
"already-tracked." Two of those were genuine deferrals with no actual
tracking behind them; the rest were either already correctly scoped or
became fixable once named precisely. `plan-fix-critique` returned
`findings` on revision 6 (Critical C1, C2; Warning W1-W6):

- **C1: "already-tracked" was false** — no `tickets/` directory exists for
  this initiative and none was created; the ticket's own `LowSoc { threshold:
  0.15 }` would have stayed governed by the anxiety override even after
  Fixes 1-3 landed. Attempted fix (superseded two rounds later — see the
  revision 9 note below): **Fix 4** proposed making
  `daily_drive_miles`/`range_anxiety_miles` configurable per-`EvConfig`
  (threaded through `ActorSeed::Ev`), so the anxiety band could be moved
  below `0.15`.
- **C2: the accepted phantom-deadline consequence was a defect this plan
  introduced, not a residual it inherited.** Fixed, not accepted: **Fix
  5** removes `resolve_departure`'s static-schedule fallback — a
  `DepartureDeadline`-bearing strategy is inactive on a day the actor
  already knows has no trip, closing the on-peak-charging-for-a-phantom-
  departure path Fix 3 opened.
- **W1: the integration test's `LowSoc` example (`threshold: 0.35`)
  breached the precondition it was meant to satisfy, and the precondition
  itself was stated as SOC-only when the real gate is time-and-SOC.**
  Corrected: the precondition is restated accurately, and — using Fix 4 —
  the test now uses the ticket's own `threshold: 0.15` with a
  small-enough configured `anxiety_soc`, rather than a substitute number.
- **W2: `resolve`'s tied-score comparison (`>`) silently drops a tied
  vote's target** — a pre-existing defect in the function this plan
  already edits. Fixed: **Fix 6** changes the comparison to `>=`.
- **W3, W4: real but checked-and-bounded consequences** (a dispatch
  reaching a phase/connection-state-mismatched EV; the arrival step's
  `reconcile_soc` timing) — addressed by tracing the exact code paths and
  stating precisely what does and doesn't happen, not by assertion.
- **W5: a held EV's heater is unaffected by this plan** — its activation
  condition never consulted `power_setpoint_kw`; named and left unchanged
  with the reasoning stated, not silently passed over.
- **W6: the verification gate was narrower than the change's actual
  reach** — widened to `cargo test -p hares-core --tests` and the four
  Python EV-related suites.

This round's new fixes (4, 5, 6) exist only because C1, C2, and W2 named
concrete, checked defects — not because the scope was reopened for its
own sake.

**Revision 8 note.** Critique found revision 7's Fix 4 wrong on its own
terms (imperial public fields, no validation, and a false claim that the
new fields weren't HPXML fields — this round's C1), and, in checking that
claim against the actual HPXML schema, found something this plan cannot
absorb: HARES's EV HPXML parser (`resolve_der.rs:176-212`) reads
`Systems/ElectricVehicles/ElectricVehicle`, an element that does not
exist in the HPXML 4.x or 5.x specification (this round's C2, verified
directly against `/home/rich/src/hpxml-rs/schemas/{v4,v5}/
HPXMLBaseElements.xsd`). **Fix 4 is withdrawn**, not repaired: correctly
sourcing driving-behaviour parameters from HPXML depends on first
resolving which of two very different, undiagnosed defects C2 actually
is (a parser bug, or an untested extension this repo has no reference
for), which is a second root-cause investigation into a different
subsystem, not a consequence of the silence-contract root cause this
initiative's RCA diagnosed. See Fix 4's section (below, kept as a
"withdrawn" section rather than deleted, so the reasoning and the schema
evidence are not lost) for the full account and what remains true about
the ticket's own `LowSoc 0.15` configuration. The Verification section's
integration test is restored to demonstrating the mechanism at a
threshold the fixture's existing hardcoded `anxiety_soc` permits — done
correctly this time, with the actual inequality the hysteresis and daily
drain require, not the arithmetic error critique W1 caught two rounds ago.

W2 (the other three hardcoded seed inputs) is moot now that Fix 4 is
withdrawn — there is no narrower-than-necessary config surface to
critique when no new config surface is added. W3 (a claimed reflection
section that did not exist) will not recur: this note is history, not a
forward reference to a section this document does not have. W4
(staging) is answered directly: Fixes 1, 2, 3, 5, 6 land as one change,
verified together by the "Interaction between the fixes" section and the
Verification section's full command list — there is no partial-green
intermediate state this plan asks the fix step to pass through. W5 (the
stale composer-test comment Fix 6 makes true) is fixed in Fix 6's test
audit, below.

**Revision 9 note.** Two problems in revision 8's own reasoning, both
corrected here rather than carried forward:

- **The heater decision's rationale was wrong.** Revision 8's W1 response
  said battery thermal conditioning "protects the pack" independent of
  the charging schedule. Critique W1 this round checked that against the
  actual gating condition — `would_charge_underated = !is_v2l_discharge
  && self.soc < self.effective_soc_limit()` (`ev/mod.rs:1198-1206`) — and
  found it is not pack protection at all: it is charge *preconditioning*,
  gated on whether the BMS would currently be charging. A full pack at
  −20°C gets no heater under this code, which contradicts "protects the
  pack ... regardless of schedule." Checked this round whether the fix is
  then to make the heater follow the hold (gate on the *actual* computed
  `charger_kw > 0` instead of the independent SOC check): it is not — the
  equipment test `heater_only_grid_draw_when_fully_cold_derated`
  (`ev/tests.rs:567-600`) pins the opposite case as intended: the heater
  must activate *precisely when* `charger_kw` is zero because
  `charge_derate` has zeroed it from cold temperature, so the pack can
  warm enough for charging to start. Gating on `charger_kw > 0` would
  break that test and the feature it verifies. The current condition
  cannot distinguish "charger_kw is zero because cold-derate blocks
  charging" (heater should fire, to enable charging) from "charger_kw is
  zero because a strategy is deliberately holding" (per this ticket's
  concern, arguably should not fire) — both are `power_setpoint_kw`-
  independent SOC/temperature facts today, and disambiguating them
  correctly requires the equipment to know *why* it isn't charging this
  step, information that does not exist anywhere in the current control
  signal vocabulary. Inventing that distinction is exactly the kind of
  infrastructure the RCA's root cause never called for — a real BMS
  preconditioning-scheduling design question, not a consequence of the
  silence contract. **Decision:** left unchanged, for the reason above
  (not "a real question" left open, an active choice against breaking a
  verified feature to fix an unconfigured-by-default edge case —
  `DEFAULT_HEATER_POWER_W = 0.0` means this has no effect on the ticket's
  fixture or completion bar either way).
- **The prior revision's "no tracking mechanism exists" claim was false**
  and its `ready` verdict was not this plan's to give unilaterally — see
  Fix 4's withdrawal section above (critique C1, this round) for the full
  correction: `docs/tickets/` exists with 110 entries, ticket 132 has been
  created in it this round, and `plan_outcome` is set to `blocked` below
  to put the "ship now / hold for ticket 132" choice to the operator
  rather than deciding it here.

`plan_outcome` for this revision: **blocked**. Fixes 1, 2, 3, 5, and 6
are complete, verified, and fully close the RCA's confirmed root
cause — every claim about them across five critique rounds has held or
been corrected in place, and none of this round's findings touch their
design. What blocks a `ready` verdict is narrower and precise: whether
those five fixes may ship while the ticket's own `LowSoc 0.15`
configuration remains ungoverned (tracked in ticket 132, a separate,
undiagnosed defect in a different subsystem) is a scope decision the
operator has not been asked to make. This is not a claim that the RCA's
root cause is wrong or unscopeable — it is fully scoped and verifiable —
it is a claim that shipping it now, with a named gap in the same
ticket's completion bar still open, needs the operator's sign-off before
this plan can honestly call itself done.

## Fix 1 — explicit hold on idle (the confirmed primary root cause)

### Alternative considered and rejected

**Change the equipment's default so that an actor-controlled EV does not
charge-to-`ready_soc` when it receives no signal**, instead of changing what
the composer dispatches. Rejected: `hares-equipment` sits below `hares-core`
in the DAG (constitution: "the DAG is law") and cannot know whether a given
`Ev` instance has a governing actor — that fact only exists in
`hares-core`'s `build_actors_from_seeds`. Implementing "actor-controlled"
awareness inside the equipment would require threading a new flag across
that boundary and would still need to preserve today's un-actored default
(`bms_default_charges_to_soc_max` deliberately tests charge-to-`ready_soc`
with no actor at all). The RCA itself frames the available primitives as
`PowerLimit`/`PowerSetpoint`, i.e. signals the actor sends — not an equipment
default it can flip. Speaking explicitly from the actor side is the smaller,
DAG-respecting change.

A second alternative — using `PowerLimit{max_power_kw: 0.0}` for the hold
instead of `PowerSetpoint` — was also considered and rejected:
`power_limit_kw` is independently populated from `EvConfig`'s
`KEY_POWER_LIMIT_KW` (`ev/mod.rs:370,491`) as a genuine hardware/config
ceiling. Un-holding would require resetting it to some "no limit" value
every active step, which would silently clobber a user-configured charger
power cap. `power_setpoint_kw` carries no such independent config meaning
(`ev/mod.rs:370`: always `None` at construction; the only config-level
field is `power_limit_kw`), so it is the safe field to repurpose.

A third alternative, raised in critique — **hold via `SOCTarget{target_soc:
ctx.current_soc}` instead of `PowerSetpoint{0}`**, using the channel the
active branches already use, so no new clearing rule is needed at all
(`self.soc_target` just gets overwritten by whatever the next dispatch
says, the same way it already works today) — was considered and rejected
on precision grounds. `ctx.current_soc` in the dispatch context is the
driver's *perceived* SOC (`decision_context`, `mod.rs:810-825`, documented
as deliberately distinct from the equipment's observed SOC — "changing
that would alter dispatch, which the module doc explicitly forbids"). If
perceived and actual SOC have drifted even slightly at the moment
`compute_charging_power_kw` runs, `target = current_soc` no longer equals
`self.soc` exactly, and the hold degrades from "exactly zero" to "taper to
a small nonzero trickle" (`ev/mod.rs:774-778`,
`taper_limit = (soc_limit - self.soc).max(0.0) * capacity / dt / efficiency`).
`PowerSetpoint{0}` forces exactly zero regardless of any perceived/observed
divergence, which is what "hold" needs to mean. This alternative is also
why W1/W2 below are answered by keeping `PowerSetpoint` rather than
switching representations.

### The change

**`crates/hares-core/src/actors/ev_driver/composer.rs`, `emit_vote`:**
revised from revision 4 in response to critique W2 and W4 (below), which
found the previous design left a second, independent "idle" path
undispatched and silently dropped a resolved vote's `target_soc` whenever
`power_kw` was also present. The whole function is restructured into three
ordered steps instead of the current four branches with an early
zero-power return:

```rust
fn emit_vote(&self, ctx: &DecisionContext, vote: &PreferenceVote, out: &mut Vec<DispatchRequest>) {
    // 1. Nothing dispatchable -- dispatch an explicit hold. Corrected per critique
    //    W1 (this round): the condition is `target_soc.is_none() && power_kw.is_none()`,
    //    NOT also requiring `departure_hour.is_none()`. A `departure_hour` with
    //    neither a target nor a rate cannot be expressed by any of the three
    //    ControlSignal variants below it (EvSetReadyBy needs a target to pair
    //    with the departure; SOCTarget and PowerSetpoint don't consume
    //    departure_hour at all) -- so it is not actionable on its own, and
    //    including it in the "nothing to say" condition was the wrong claim,
    //    not merely an incomplete one. `PreferenceVote::idle(...)` (`preference.rs:34-44`)
    //    is a specific case of this condition, not its definition.
    if vote.target_soc.is_none() && vote.power_kw.is_none() {
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 0.0, reactive_power_kvar: None, min_soc: None, max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        return;
    }

    // 2. Target-of-record, dispatched first: EvSetReadyBy when paired with a
    //    departure, else SOCTarget when a target_soc exists on its own. Firing
    //    this before step 3 means a same-step rate (below) is the more recent
    //    write and is never dropped by a `return` that used to precede it.
    if let (Some(departure), Some(target)) = (vote.departure_hour, vote.target_soc) {
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::EvSetReadyBy { departure_hour: departure, target_soc: target },
            priority: PriorityTier::Schedule,
        });
    } else if let Some(target) = vote.target_soc {
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::SOCTarget { target_soc: target, min_soc: vote.min_soc, max_soc: vote.max_soc },
            priority: PriorityTier::Schedule,
        });
    }

    // 3. Rate-of-record, dispatched second (unconditionally on `Some`, no
    //    zero-power special case -- see the response to W2 below).
    if let Some(unclamped) = vote.power_kw {
        let power = if unclamped > 0.0 { unclamped.min(ctx.max_charge_kw) } else { unclamped.max(-ctx.max_discharge_kw) };
        // (existing clamp-warning / debug_assert / observe blocks unchanged here)
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: power, reactive_power_kvar: None, min_soc: vote.min_soc, max_soc: vote.max_soc,
            },
            priority: PriorityTier::Schedule,
        });
    }
}
```

This single change point covers every preference and every resolved vote
that is idle-*shaped* — no `target_soc` and no `power_kw`, regardless of
`departure_hour` — which is `TimeWindowPref` and `SocGate`'s overrides,
`V2G`/`V2H`'s soc-floor overrides, `SolarSurplus`'s stack when surplus is
insufficient (no preference in that stack ever votes a `target_soc`), and
the `{departure_hour}`-only shape critique W1 (this round) found. It is
the shared choke point `evaluate()` already routes both the override
branch and the resolved-vote branch through, so one change closes every
*idle-shaped* instance, not one strategy's.

**Correction (critique W2, this round):** it does not close every strategy
that can converge on "charge on plug-in" — only every strategy whose idle
is idle-*shaped*. Any stack containing `SocTarget` (`Immediate`, `Nightly`,
`TouAware`, `PreDeparture`, `V2H`, `V2G`) never produces an idle-shaped
vote while SOC is below the configured target, because `SocTarget::score`
always votes `target_soc: Some(...)` regardless of urgency
(`soc_target.rs:16-27`) — for those strategies, dispatching
`SOCTarget{target}` at rated power *is* the strategy governing correctly,
not a residual defect. `Nightly` and `Immediate` are the ticket's own
strategies and this is their intended behaviour (`Nightly` gates via its
paired `TimeWindowPref`, which does produce idle-shaped votes outside the
window; `Immediate` is supposed to charge at every opportunity).
`TouAware`'s existing, currently-passing test
`tou_neutral_price_soc_target_drives_charging` (`mod.rs:4352`) already pins
rated-power charging at a neutral price as correct for that strategy. So
"one change closes the class" overstated the RCA's own "class of the bug"
framing (which named `TouAware outside off-peak` as an example) — this fix
closes the silent-idle-shape instances of the class; a `SocTarget`-bearing
strategy charging at rated power whenever its own gate says so is not a
symptom of this bug and was never claimed to be fixed by RCA's confirmed
root cause, only by this plan's earlier overclaim of the RCA's framing.

Hot-path note (critique O2): this adds one `DispatchRequest` push (one
`Arc<str>` clone via `dispatch_target.clone()`, no heap allocation beyond
that existing clone pattern already used by every other branch in
`emit_vote`) per idle plugged-in EV per step — the same per-step dispatch
shape every active strategy already produces, not a new allocation
category. Step 2 producing a second push on some active steps where
previously only one fired (see W4 below) is the same shape again, not a
new one.

### Response to critique C1 (prior round, withdrawn) and its consequence for this revision's W4 fix

The prior critique round's C1 claimed revision 4's `emit_vote` co-dispatched
`PowerSetpoint` then `SOCTarget` in one step; this critique round confirmed
that claim was wrong for revision 4's code (the `power_kw` branch's
unconditional `return` made `SOCTarget` unreachable in the same call once
`power_kw` was `Some`) and withdrew it. That confirms the *old* structure
never co-dispatched — it does not mean this revision's *new* structure,
adopted to fix W4, is safe by the same argument, since the new structure
deliberately introduces co-dispatch. Re-verified here rather than assumed:

- **Order is target-then-rate, always.** Step 2 (target-of-record) always
  runs before step 3 (rate-of-record) in the function body above, so `out`
  for a vote carrying both is `[SOCTarget{target}, PowerSetpoint{rate}]` (or
  `[EvSetReadyBy{...}, PowerSetpoint{rate}]`), never the reverse.
- **FIFO application means the rate is always the last write.** The
  dwelling applies `DispatchRequest`s FIFO within a tier
  (`dwelling/mod.rs:790-830`). `apply_control`'s `SOCTarget` and
  `EvSetReadyBy` arms clear `power_setpoint_kw` (Fix 1, below); with target
  dispatched first, that clear lands *before* the `PowerSetpoint` arm runs
  and sets the real rate — so the rate always stands as the final value for
  that step, and the target's ceiling (`self.soc_target`) is correctly
  updated alongside it. This is the same safety property the prior C1
  finding was checking for, re-derived for the new ordering rather than
  carried over from the old one.
- **Traced concretely for V2H's stack** (`SocTarget{0.9}` + `V2HDischarge`,
  `mod.rs:216-231`): the resolved vote while discharging is
  `{target_soc: Some(0.9), power_kw: Some(-discharge), departure_hour: None}`
  (`resolve()`'s independent folds, `composer.rs:131-198`, confirmed in the
  prior round). Under the new `emit_vote`, this dispatches
  `SOCTarget{0.9}` then `PowerSetpoint{-discharge}`. Applied in order:
  `soc_target = Some(0.9)`, `power_setpoint_kw` cleared to `None` then set
  to `Some(-discharge)`. `compute_charging_power_kw`'s V2G/V2L discharge
  fast path (`ev/mod.rs:592-599`) checks `power_setpoint_kw < 0.0` *before*
  consulting `soc_target` at all, so the discharge is unaffected by
  `soc_target` being set — this also *fixes* the pre-existing,
  previously-out-of-scope defect the prior round's C1 response surfaced
  (V2H/V2G's `target_soc` silently dropped every discharge step), as a
  direct consequence of the W4 fix below, not a separately scoped change.

### Response to critique W2 — the two definitions of "idle" are unified, not reconciled by a rule

The critique found a second, independent silent-return path:
`if unclamped.abs() < 1e-9 { return; }` inside the old `power_kw` branch
(`composer.rs:234-237`, comment: "Zero power = idle, no dispatch needed"),
reachable whenever a resolved vote carries `power_kw: Some(0.0)` with no
`target_soc`/`departure_hour` alongside it — concretely,
`SolarTracking::score` with `min_charge_rate_kw = 0.0` and zero PV surplus
votes exactly this (`solar.rs:22-33`: `surplus < min_charge_rate_kw` is
`0.0 < 0.0`, false, so the `else` branch fires with `clamped = 0.0`). This
is a second, independently-reachable "nothing to say" shape that Fix 1's
all-three-`None` check does not catch.

Rather than add a second check for it (two rules doing the same job is the
duplication the constitution's DRY principle targets, and is exactly what
the critique flagged), the restructured `emit_vote` above removes the
special case entirely: step 3 dispatches `PowerSetpoint` for *any*
`Some(power_kw)`, including exactly `0.0` — clamping `0.0` still yields
`0.0`, so the dispatch is `PowerSetpoint{active_power_kw: 0.0, ...}`,
identical in effect to Fix 1's explicit hold. One rule ("if there's
nothing to charge at, say so explicitly") now covers both shapes, rather
than two rules with two different equipment outcomes. Grepped for a test
that pins the removed special case directly (`power_kw: Some(0` anywhere
in `ev_driver/*.rs`): none found — the fix step should still watch for any
test that reaches this path indirectly (e.g. a `SolarSurplus` scenario
with `min_charge_rate_kw: 0.0` and zero surplus) and re-baseline it the
same way as the other idle-shaped tests in Fix 1's test audit.

Checked, per critique O2, the converse direction too: whether any existing
test asserts the *absence* of `SOCTarget` specifically on a step where a
rate is also present (the new step-2-then-3 co-dispatch this restructure
introduces for `V2H`/`V2G`/`SolarSurplus`-with-schedule stacks). Grepped
`mod.rs` for a `SOCTarget`-absence assertion outside the two
`range_anxiety_override_does_not_preempt_*` reproduction tests (which
assert absence in a scenario with no accompanying rate, unaffected by this
change): none found; the composer-level count assertions in
`composer.rs` (431-1077) all construct target-less votes for their `out`
checks, so none of them observe the new second push either. Recorded here,
not merely assumed, per the plan's own "verified, not assumed" standard.

### Response to critique W4 — the dropped-target defect is fixed now, not ticketed

The critique found that `emit_vote`'s old unconditional `return` after
`PowerSetpoint` silently dropped `target_soc` whenever a vote carried both
— not only for V2H/V2G (noted and left out of scope in the prior round) but
for `SolarSurplus`/`TouAware` charging steps too, meaning their configured
`target_soc` ceiling was never sent and the equipment would charge toward
`ready_soc` (default `1.0`) instead. This is in the same function this plan
already edits, in the same bug class the ticket reports ("the strategy's
content does not govern behaviour"), and — per the constitution's
no-untracked-deferrals rule — doable now rather than deferred: the
restructured `emit_vote` above dispatches the target-of-record
unconditionally in step 2 whenever `vote.target_soc` (or the ready-by pair)
is present, independent of whether `power_kw` is also present in step 3.
This is not a separate change from the W2/C1 restructure — it is the same
one, and the C1 re-verification above (target dispatched before rate,
every time) is what makes it safe rather than a second same-step
cancellation bug.

### Response to critique W1 — the `PowerSetpoint` contract change is real but not new in kind

A Python actor (`hares-python/src/py_actor.rs:404-409`) can dispatch
`PowerSetpoint` to the same EV a driver actor governs; the dwelling applies
signals last-write-wins with no ownership arbitration
(`dwelling/mod.rs:790-830`). Fix 1 does widen the window during which the
driver's own `PowerSetpoint` dispatches can clobber such an external
actor's setpoint: previously only *active* composer steps wrote
`PowerSetpoint` (rate-voting strategies, or the departure buffer/urgent
override); after Fix 1, *idle* steps write one too (the `0.0` hold), and
this is now a *continuous* per-step write rather than an occasional one
(critique O4) — a caller that set a rate once and expected it to stand
without re-sending now loses it every idle step, not just on the next
active-strategy transition.

There is an in-repo example, not merely a hypothetical Python one
(critique O4, correcting the prior round's understatement): the occupant
actor dispatches `PowerSetpoint` to the EV at `UserOverride` tier on every
present step (`occupant.rs:343-372`). Checked whether Fix 1 breaks it:
`UserOverride` is a higher priority tier than the driver's `Schedule` tier
and the dwelling drains tiers low-to-high with later tiers winning
(`dwelling/mod.rs:790-830`), and the occupant actor re-sends every step it
is active — so on any step both actors dispatch, the occupant's later,
higher-tier write is what stands, regardless of what the driver's hold
wrote first. The occupant actor is unaffected by this change; named here
because the prior round's "no ticket asks for multi-actor ownership"
framing understated the case by not naming the in-repo actor that already
exercises this exact interaction. This plan does not attempt to add
arbitration beyond what tier ordering already provides (out of scope: no
ticket asks for multi-actor ownership on one equipment target, and the
dwelling's last-write-wins-within-a-tier, tiers-drain-in-priority-order
model is a pre-existing, workspace-wide
contract, not an EV-specific one). Judged acceptable on that basis, and
named explicitly here rather than left implicit.

**`crates/hares-equipment/src/ev/mod.rs`, `apply_control`:** the `SOCTarget`
and `EvSetReadyBy` arms each gain one line clearing the hold:

```rust
self.soc_target = Some((*target_soc).clamp(0.0, 1.0));
self.soc_target_min = *min_soc;
self.soc_target_max = *max_soc;
self.power_setpoint_kw = None;       // new: a fresh target supersedes any
self.power_setpoint_min_soc = None;  // hold dispatched during a prior
self.power_setpoint_max_soc = None;  // idle window (RCA fact c).
```

and identically for `EvSetReadyBy`'s two assignments. Rationale (RCA fact
c): `compute_charging_power_kw` computes `requested =
self.power_setpoint_kw.unwrap_or(derated_rated)` *before* consulting
`soc_target`/`ready_by_soc` for anything except the SOC-limit gate — so a
`PowerSetpoint{0.0}` hold left over from an idle window would silently
veto a subsequent `SOCTarget`/`EvSetReadyBy` dispatch once the strategy
becomes active again, trading "always charges" for "never charges" (the
opposite failure mode the RCA warns about). Clearing it exactly where a
positive charging decision is received is the minimal rule that prevents
that: every dispatch path that means "charge toward X" now also means
"and stop holding," in the one place (`apply_control`, not scattered across
every composer/override call site) that already owns writing these fields.
This also fixes the *latched* case fact (c) describes for the range-anxiety
override in Fix 2 below, which dispatches `SOCTarget` only, without further
changes to `maybe_push_range_anxiety_override` itself.

Observation (critique O3): clearing `power_setpoint_kw` in the
`EvSetReadyBy` arm does not make "hold" mean "no charging" whenever
`ready_by_hour` is already latched under `ChargingPriority::DeadlineGuarantee`
(the default) — `compute_charging_power_kw`'s deadline branch computes
`requested = bms_power.max(requested)` (`ev/mod.rs:~660-668`), so the BMS
can still charge through a `0.0` hold once a deadline is urgent, by design
(the deadline-guarantee contract). This is correct, existing behaviour
(deadline enforcement is supposed to override a soft hold), not a defect
this plan introduces — named here because it means `reproduction.md`'s
"idle must produce no equipment charging" framing has this one documented
exception: idle-while-a-deadline-is-urgent legitimately still charges, by
the equipment's own documented contract, and the fix step's tests should
not assert zero charging in that specific combined scenario.

No change is made to `PowerLimit` handling or `init_typed`'s reset block.
**Revision 5 correction:** this section previously argued disconnect-
clearing was unnecessary in full, reasoning only from the home-side
`HomePluggedIn`/`evaluate_charging` path. That argument is still correct
for that path (below), but it did not consider the `AwayPluggedIn` phase,
which consults the same `power_setpoint_kw` field through a different step
arm that `evaluate_charging` never runs alongside. Critique C1 (this
round) found the gap; the blast-radius section above now adds a targeted
clear in `EvPlugIn`'s `Disconnected` arm for exactly that reason. What
remains true, scoped precisely:

- **On every step where `evaluate_charging` runs while `HomePluggedIn`**
  (confirmed: `DriverPhase::HomePluggedIn`'s non-departure branch,
  `mod.rs:1014`, and — after Fix 3 below — every non-driving-day step
  too), a stale hold or target from a previous plug-in cycle is
  overwritten before that step's physics runs. No *additional* clearing
  is needed for this path beyond the `Disconnected`-arm clear above,
  which now runs once, at the phase boundary, rather than relying on
  `evaluate_charging` to overwrite a stale value on every subsequent
  home-side step.
- **The arrival-transition step is closed, not accepted (critique C1,
  this round — the prior revision's "accepted, bounded residual" is
  withdrawn).** On arrival, the `DriverPhase::Away` arm dispatches
  `ControlSignal::EvPlugIn{HomePluggedIn}` and sets `self.phase =
  DriverPhase::HomePluggedIn` in the same step (`mod.rs:1082-1121`);
  `evaluate_charging` was previously only called from the *next* step's
  `DriverPhase::HomePluggedIn` arm, so the arrival step's own `step()` ran
  charging physics with no fresh decision — a full step at rated power
  toward whatever `soc_target` was last latched, every arrival, outside
  whatever window the strategy configures. This directly contradicted the
  Verification section's integration-test criterion (critique C1's
  cross-section finding), so it is closed at the source: the arrival
  branch, immediately after `self.reconcile_soc(env, "arrival")` and while
  `doing_plugin` is true, now also calls
  `self.evaluate_charging(env, current_minute, out)`:

  ```rust
  self.phase = DriverPhase::HomePluggedIn;
  self.reconcile_soc(env, "arrival");
  if doing_plugin {
      self.evaluate_charging(env, current_minute, out);
  }
  ```

  This is the same function Fix 3 already routes the non-driving-day arm
  through — a third call site, not new logic. Ordering is safe: the
  `EvPlugIn{HomePluggedIn}` dispatch was already pushed to `out` earlier in
  this same arm, and the dwelling applies `DispatchRequest`s FIFO within
  the `Schedule` tier (confirmed this round: actors decide, then the
  dispatcher applies controls, then equipment steps —
  `dwelling/mod.rs:~4945,~5036,5179`), so the connection-state transition
  is in effect before the charging decision is applied, and the charging
  decision is in effect before the equipment's own `step()` runs — closing
  the gap for the arrival step itself, not just every step after it.
  `reconcile_soc` runs first so the composer's decision uses the freshly
  reconciled `perceived_soc()`, not a stale pre-arrival estimate.

Disconnect-clearing now also covers `soc_target`/`soc_target_min`/
`soc_target_max`, not only `power_setpoint_kw` (critique W3, this round).
The prior revision left `soc_target` uncleared on the unverified claim
that some caller "might still matter" — checked this round the same way
`power_setpoint_kw` was checked (grep for a test or caller relying on
`soc_target` surviving a disconnect/reconnect cycle): none found
(`crates/hares-equipment/src/ev/tests.rs` has no test combining `soc_target`
persistence with a `Disconnected` transition). With the arrival-step fix
above, `soc_target` staleness on the home side is now moot regardless (the
composer re-asserts it on the very first arrival step); the remaining
reason to clear it is `AwayPluggedIn`'s own use of `soc_target` to bound
`soc_limit` (`ev/mod.rs:556`) — a stale home-side target (e.g. `0.95` from
the last in-window `Nightly` dispatch) is not wrong for away charging
(charging toward a sensible ceiling while away is reasonable), but it is
also not evidently *right* by design, only by accident of whatever the
home strategy last set. Clearing it lets away charging fall through to the
equipment's own configured `ready_soc`, which is the deliberate,
documented default rather than an incidental one:

```rust
if *state == EvConnectionState::Disconnected {
    self.ready_by_hour = None;
    self.ready_by_soc = None;
    self.power_setpoint_kw = None;
    self.power_setpoint_min_soc = None;
    self.power_setpoint_max_soc = None;
    self.soc_target = None;
    self.soc_target_min = None;
    self.soc_target_max = None;
}
```

The prior revision's stated reason for rejecting disconnect-clearing
outright — "changing behaviour for non-actor-driven callers (e.g. a
pre-scheduled V2G/DR target meant to survive a plug cycle)" — does not
apply to either field cleared here: neither has a caller or test in the
workspace relying on cross-plug-cycle persistence, checked by the same
grep-based standard for both, not asserted for one and assumed for the
other.

### Consequence for existing tests (blast radius, part 1) — corrected per critique W5

Only assertions of the literal shape `out.is_empty()` (or equivalent
"nothing was dispatched") break under Fix 1, and only when the scenario
reaches an idle composer decision. An assertion of the form "no
`SOCTarget` in `out`" (e.g. `out.iter().find_map(...SOCTarget...)` returning
`None`, or the `has_soc_target`/`has_charging` boolean-scan pattern used by
`quick_then_wait_idles_above_threshold`, `mod.rs:2716-2739`, which scans for
`SOCTarget | PowerSetpoint | EvSetReadyBy` together) is **not** affected —
a `PowerSetpoint{active_power_kw: 0.0, ..}` is a real `PowerSetpoint`
signal, so any assertion that already treats `PowerSetpoint` as evidence of
charging correctly reports "still idle" whether or not Fix 1 lands; the
critique's own `quick_then_wait_idles_above_threshold` example already
matches on `PowerSetpoint`. The prior revision's parenthetical ("or
equivalent: no `SOCTarget` in `out`") conflated the two forms; that was
wrong, corrected here.

The literal `out.is_empty()` assertions confirmed present at
`composer.rs:537` and `mod.rs:1818, 1875, 2042, 2962, 3631` must be
re-baselined to assert `out` contains a `PowerSetpoint{active_power_kw:
0.0, ..}` — not deleted or weakened, per the constitution's no-weakened-
assertions rule — but each must be read individually by the fix step
before editing: `2020`'s neighbourhood is an away-charging test (not an
idle-composer scenario) and is unaffected; the fix step must confirm each
of the others is genuinely an idle-composer scenario, not assume it from
this list. This is the exact gap the RCA names: "every existing 'idles'
test... asserts only 'out contains no SOCTarget'... That gap in test
design is exactly why fact (b) was never caught."

`mod.rs:2755,2758` (`no_event_day_emits_nothing`, `mod.rs:2745-2758`) and
the five non-driving-day tests referenced in Fix 2 are addressed separately
in Fix 3 below, not here — Fix 3 changes whether `decide`'s `None` arm
reaches the composer at all, which supersedes this plan's original claim
that those tests were unaffected by Fix 1.

## Fix 2 — gate the range-anxiety override on urgency

### Alternative considered and rejected

**Thread real driving-behaviour config (`daily_drive_miles`,
`departure_time`, `event_day_ratio`, `range_anxiety_miles`) from
HPXML/`EvConfig` through to `ActorSeed::Ev`, replacing the hardcoded
constants in `build_actors_from_seeds`.** This changes how often the
override's unconditional trigger condition is met, not whether crossing it
preempts the strategy — it cannot make the two `range_anxiety_override_does_not_preempt_*`
tests pass, since those construct the actor directly and never touch
`EvConfig`/HPXML at all. Real, separately-scoped gap; not this ticket's
fix.

### The change

Adopt the design already produced and technically verified (not
mechanically faulted) by the prior fix-plan revision for this initiative,
now re-scoped as Fix 2 alongside Fix 1 rather than as the whole fix:

`needs_range_anxiety_override` (`mod.rs:715-770`) is unchanged — it still
answers only "is perceived SOC below the anxiety-band threshold." The new
urgency condition is added in `maybe_push_range_anxiety_override`
(`mod.rs:786-804`): when `self.todays_event` is `Some(event)`, compute
`hours_left` (minutes until `event.departure_minute`, wraparound-safe) and
`needed` (`needed_charge_hours_to_target(anxiety_soc, CHARGING_EFFICIENCY,
&ctx)`, using `self.decision_context(env, current_minute)` — perceived SOC,
matching every other dispatch decision in this file). If `hours_left >=
needed * 1.2`, the strategy has time; return `false` instead of firing, so
the composer (now correctly holding or dispatching per Fix 1) gets to
govern. Otherwise fire exactly as today (`SOCTarget{target_soc: 1.0}`).

`anxiety_soc` (not `1.0`) is the correct target for the urgency test itself
— it is the SOC that actually prevents stranding; requiring time-to-full is
a stricter, charger-class-sensitive bar the override does not need to meet.
The wraparound-safe minutes-until-target-minute arithmetic, currently
private inside `DepartureDeadline::minutes_until_departure`
(`departure.rs:~39-48`), is extracted to a shared free function in
`preference.rs` (alongside `needed_charge_hours_to_target`) so
`DepartureDeadline` and the new urgency check share one implementation —
required by the DRY rule the moment a second call site needs it.
`maybe_push_range_anxiety_override`'s urgency gate applies only inside its
own `if let Some(event) = self.todays_event` branch — when `todays_event`
is `None` (non-driving day), that branch is skipped entirely and the
override still fires unconditionally on `needs_range_anxiety_override(env)`
alone, exactly as today. This part is unchanged by critique's findings.

What *does* change on non-driving days is not this override's own firing
rule but whether anything else runs when it doesn't fire — see Fix 3
below, which the critique's C2 makes necessary.

Test audit: the three tests the prior revision identified as encoding the
pre-fix unconditional contract on the in-phase path
(`needed_charge_hours_under_anxiety_reports_charge_to_full_plan`,
`needed_charge_hours_under_anxiety_reports_observed_gap_not_belief`,
`range_anxiety_overrides_strategy_when_soc_low`) must be re-baselined to
scenarios where the urgency condition genuinely holds, preserving each
test's actual purpose (telemetry substitution when the override fires;
proof the override still protects against genuine stranding risk) rather
than the specific pre-fix numbers. All other tests calling
`needs_range_anxiety_override` directly and `DepartureDeadline`'s own tests
are unaffected by Fix 2 itself (unchanged signature, unchanged behaviour on
those paths). The non-driving-day tests are addressed in Fix 3's own test
audit below, not here — some are genuinely unaffected by any of these
three fixes (the override-fires cases) and some need re-baseline because
of Fix 3, not Fix 2; listing them as a single "unaffected" group here (the
prior revision's error, per critique C2) conflated the two.

## Fix 3 — route `decide`'s `None` arm through `evaluate_charging` (closes critique C2)

### The regression C2 identifies

Fix 1 makes the composer's idle decision persist (a real `PowerSetpoint{0}`
hold, not silence) for as long as nothing re-dispatches. `decide`'s `None`
arm (non-driving day) never calls `self.composer.evaluate(...)` — confirmed
by direct read, `mod.rs:942-968` — it either fires the range-anxiety
override or returns after `populate_telemetry`. Before Fix 1 this was
harmless (the equipment's own BMS default filled the silence, charging
fully regardless). After Fix 1, it is not harmless: a hold dispatched on
the last idle step of the *previous* driving evening now persists,
unchanged, through an entire non-driving day, because nothing on that arm
ever re-dispatches to clear or reaffirm it.

Concretely, for the ticket's own `Nightly 02:00-06:00` configuration: at
23:55 the previous evening, outside the window, the composer holds
(`PowerSetpoint{0}`). At 00:00 a non-driving day rolls
(`event_day_ratio` makes ~20% of days non-driving in the fixture). For the
entire day, `decide` takes the `None` arm; if `needs_range_anxiety_override`
doesn't trip (SOC not yet in the anxiety band), nothing is dispatched at
all — the 23:55 hold from the night before is still in effect at 02:00,
04:00, and 06:00, so the vehicle does not charge during the very window the
user configured, on a day it previously (incorrectly, but at least fully)
charged. This is the constitution's "do not make HARES worse" gate: Fix 1
alone regresses a named ticket configuration on non-driving days, worse
than the pre-fix behaviour for that day. This is a direct, foreseeable
consequence of Fix 1 and must close in this same change, not be deferred.

### The change

`decide`'s `None` arm, while `DriverPhase::HomePluggedIn`, calls
`self.evaluate_charging(env, current_minute, out)` — the same function the
driving-day path already uses (`mod.rs:1014`) — instead of calling
`self.maybe_push_range_anxiety_override(env, out)` directly:

```rust
None => {
    if matches!(self.phase, DriverPhase::HomePluggedIn) {
        self.evaluate_charging(env, current_minute, out);
    }
    self.populate_telemetry(env, before_out, out);
    return;
}
```

`before_out` (captured at the top of `decide`, before the `match
self.todays_event` block) is untouched by this change — it continues to
feed only `populate_telemetry`, exactly as before; it is unrelated to the
dropped `debug_assert`, which compared `out.len()` against it for a
different purpose (critique O4, this round: naming this explicitly since
the snippet drops one use without a comment on the other).

`evaluate_charging` already tries the override first and returns before
reaching the composer if it fires (`mod.rs:860-873`), so the override's
unconditional-on-non-driving-days behaviour (confirmed above, in the Fix 2
section) is preserved exactly — this is not a new code path, it is
widening the call site of the existing one to a day type it did not
previously run on, the same relationship Fix 2 has to
`maybe_push_range_anxiety_override`. When the override does not fire, the
composer now runs — reasserting whatever the strategy's current preference
stack says (a fresh hold if still idle, a fresh target if now active) —
closing the gap C2 identifies, because the composer is reached on *every*
plugged-in step regardless of driving/non-driving, not just on the steps
where `todays_event` happens to be `Some`.

`TimeWindowPref`/`SocGate`/`SocTarget` — the three preferences behind the
ticket's `Nightly`/`LowSoc`/`Immediate` completion bar — never consult
`todays_event` at all, so nothing here changes their behaviour on a
non-driving day; they gate on time-of-day and SOC exactly as on a driving
day.

**Correction (critique W1):** the prior revision claimed
`decision_context`'s `next_departure_minute: None` on a non-driving day
hits `DepartureDeadline::resolve_departure`'s `Constraint::Inactive`
fallback. Re-read `resolve_departure` (`departure.rs:50-58`) to check this:
it does *not* return `None`/`Inactive` when `next_departure_minute` is
`None` — it falls through to `find_today`'s static schedule lookup and
returns that weekday's configured departure regardless:

```rust
fn resolve_departure(&self, ctx: &DecisionContext) -> Option<(u32, f64)> {
    if let Some(dep_min) = ctx.next_departure_minute {
        let dc = self.find_today(ctx)?;
        return Some((dep_min as u32, dc.target_soc));
    }
    let dc = self.find_today(ctx)?;          // <-- reached when None
    Some((dc.departure_minute, dc.target_soc))
}
```

`Inactive` only results when `find_today` itself has no schedule entry for
that weekday (`departure.rs:22-27`). So after Fix 3, on a non-driving day,
`PreDeparture`/`TouAware`/`SolarSurplus` with a schedule entry for that
weekday *would* score/evaluate against that weekday's usual departure and
target, even though no drive happens — and, if urgency or the buffer
window trips, dispatch `EvSetReadyBy` for a departure that will not occur,
latching `ready_by_hour`, defeating Fix 1's hold on receipt (the
`EvSetReadyBy` arm clears `power_setpoint_kw`), and switching
`compute_charging_power_kw` into its BMS deadline-pacing branch — for
`TouAware` specifically, on-peak, max-rate charging for a trip that will
not happen, defeating the strategy's entire purpose on that day.

### Fix 5 — `DepartureDeadline` is inactive when there is no known departure today (closes critique C2)

**Fixed, not accepted (critique C2, this round, under the operator's "fix
it, don't ticket it" directive).** The prior revision assessed this as a
"conservative outcome" and accepted it; that assessment is withdrawn as a
resolution — describing the SOC outcome as safe does not make the load
shape correct, and load shape (not just totals) is what the ticket is
about. The fix is smaller than the acceptance was: the actor already knows
whether today is a driving day — it set `todays_event = None` itself — and
that fact already reaches `DecisionContext` as `next_departure_minute:
None` (`decision_context`, unchanged). `resolve_departure` simply should
not invent a departure from the static schedule when the actor has no
known one for today:

```rust
fn resolve_departure(&self, ctx: &DecisionContext) -> Option<(u32, f64)> {
    let dep_min = ctx.next_departure_minute?;
    let dc = self.find_today(ctx)?;
    Some((dep_min as u32, dc.target_soc))
}
```

This removes the static-schedule-only fallback branch entirely rather than
adding a new one — smaller and DRYer than the code it replaces, not more
complex. In the real actor flow `next_departure_minute` is `Some`
if-and-only-if `todays_event` is `Some` (`decision_context`:
`next_departure_minute: self.todays_event.map(|e| e.departure_minute)`),
so this change is exactly "`DepartureDeadline` is inactive when there is
no known trip today" — nothing else. Every driving-day call already
supplies `next_departure_minute: Some(...)`, so no driving-day behaviour
changes; only the phantom-deadline path this plan's own Fix 3 opened up is
closed.

**Test consequence:** `next_departure_minute_overrides_schedule_minute`
(`departure.rs:386-436`) is the one test in the workspace that constructs
`next_departure_minute: None` for `DepartureDeadline` specifically
(confirmed by grep — no other call site does); its "Context B" currently
asserts `Constraint::Inactive` via the *old* fallback-then-not-urgent path
(comment: "fallback to schedule minute=480 (3h away) should be Inactive").
Under the new code the same context still resolves to `Inactive` (the `?`
now short-circuits before the urgency computation runs at all), so the
assertion does not change, but the comment and the test's premise do — it
no longer demonstrates a "fallback", it demonstrates "no known departure
→ inactive". The fix step must update the comment and the test's
framing (rename if warranted, per the constitution's test-naming rule) to
state what it now proves, not what it used to. `composer.rs`'s generic
`make_ctx` helper (`composer.rs:350-361`) also defaults
`next_departure_minute: None`; the fix step must check any composer test
using it together with a `DepartureDeadline`-bearing strategy for a
reliance on the old fallback (none identified by this plan's reading, but
not exhaustively re-verified against every such test this round).

The pre-existing `debug_assert!(!fired || out.len() > before_out, ...)`
invariant in the old `None`-arm code (checking the override, when fired,
always dispatches) is dropped, not kept (critique O2: a plan should decide
rather than leave "either is correct"): `evaluate_charging`'s override
branch already guarantees this unconditionally by construction (same
code, same guarantee, one caller earlier), so the duplicate check at this
call site adds no coverage, only a second place to keep in sync with the
first.

### Consequence for existing tests

This changes what `decide`'s `None` arm dispatches whenever the override
does not fire — every test built on "nothing happens on a non-driving day
unless the override fires" must be re-checked:

- `no_event_day_emits_nothing` (`mod.rs:2745-2758`): asserts `out.is_empty()`
  twice for `ChargingStrategy::Immediate{0.9}` on a permanently non-driving
  actor. `Immediate`'s stack is `[SocTarget{0.9}]` only — `SocTarget::score`
  always votes `target_soc: Some(0.9)` (`soc_target.rs:16-27`), so the
  resolved vote is never idle-shaped; `evaluate_charging` now dispatches
  `SOCTarget{0.9}` (or, once SOC reaches 0.9, a `PowerSetpoint{0}` hold —
  either way, not empty) on every such step. This test's name and
  assertion both encode the pre-fix contract and must be rewritten — per
  the constitution's test-naming rule, to something naming what actually
  happens now (e.g. a test that a non-driving day with `Immediate`
  dispatches the same strategy decision a driving day would).
- `non_driving_day_no_anxiety_when_soc_high` (`mod.rs:3615-3634`, named in
  the original Fix 2 test audit as "unaffected" — wrong, corrected here):
  `ChargingStrategy::Immediate{0.9}`, SOC 0.8 (above the anxiety band, so
  the override does not fire), `event_day_ratio = 0.0`. Asserts
  `out.is_empty()` (`mod.rs:3630`, comment: "early-return behaviour
  preserved"). Same mechanism as `no_event_day_emits_nothing`:
  `Immediate`'s `SocTarget` vote is never idle-shaped, so
  `evaluate_charging` now dispatches `SOCTarget{0.9}`. Needs the same
  re-baseline.
- `range_anxiety_triggers_on_non_driving_day_with_low_soc`,
  `non_driving_day_range_anxiety_emits_soc_target`,
  `non_driving_day_anxiety_charges_for_next_driving_day`,
  `needed_charge_hours_under_anxiety_on_non_driving_day_reports_charge_to_full_plan`
  — these all exercise the case where the override *does* fire; `evaluate_charging`'s
  override branch is unchanged, so these remain correct as originally
  audited under Fix 2, genuinely unaffected by Fix 3.

Critique O1: the module comment at `mod.rs:3549` ("On a non-driving day
`todays_event` is `None`, so `decide()` exits before reaching …") and the
non-driving-day test-section header comment at `mod.rs:3582` both describe
the pre-Fix-3 contract and go stale the moment `decide`'s `None` arm routes
through `evaluate_charging`; the fix step must update both, not just the
test bodies. The `3582` comment also carries a ticket ID (`T-0431`) — a
pre-existing violation of the constitution's no-ticket-refs-in-comments
rule, unrelated to this fix but touched by it; since the fix step is
already editing this comment for O1, it removes the ticket reference at
the same time rather than leaving it for a separate pass.

## Fix 4 — withdrawn this round (response to critique C1/C2)

### What was proposed, and why it is withdrawn rather than repaired

Revision 7 proposed two new imperial `Option<f64>` fields on `EvConfig`
(`daily_drive_miles`, `range_anxiety_miles`), defaulted via
`unwrap_or(30.0)`/`unwrap_or(20.0)` in `build_actors_from_seeds`, so the
integration test could use the ticket's literal `LowSoc { threshold:
0.15 }` and have it actually govern. Critique C1 this round found three
independent defects in that design (imperial fields in a public struct
outside `hares-io`, no validation, and the "these are not HPXML fields"
claim being factually wrong — verified below), and, while checking the
third, critique C2 found something this plan cannot responsibly absorb:
**HARES's own EV HPXML parser does not read a standard HPXML element.**

Verified this round, directly against the HPXML schema (a sibling
checkout at `/home/rich/src/hpxml-rs/schemas/{v4,v5}/HPXMLBaseElements.xsd`
— HARES vendors no schema of its own and has no `docs/hpxml/`, so this is
the best available reference, and HARES's own test fixtures declare
`schemaVersion="4.0"`, `hares-io/src/hpxml/mod.rs:231`): `resolve_ev`
(`crates/hares-io/src/hpxml/resolve_der.rs:176-212`) reads
`Systems/ElectricVehicles/ElectricVehicle` with children `BatteryCapacity`,
`MaxChargingPower`, `ChargingLevel`. **Corrected per critique W3 this
round:** the element *path* `Systems/ElectricVehicles/ElectricVehicle` and
its child element names are absent from both schemas (`grep
name="ElectricVehicle"` over both files finds only
`BatteryElectricVehicle`, `PlugInHybridElectricVehicle`,
`HybridElectricVehicle`, `FuelCellElectricVehicle`, and the unrelated
`ElectricVehicleChargers/ElectricVehicleCharger`) — `BatteryCapacity` does
exist, but only as a *type* (`BatteryCapacityType`, v4 schema lines
4077-4080), used by `Vehicle/.../Battery/{NominalCapacity,UsableCapacity}`
(2213-2218), not as the flat child element HARES's parser reads. The
standard structure is `BuildingDetails/Vehicles/Vehicle/VehicleType/
BatteryElectricVehicle/Battery/{NominalCapacity,UsableCapacity}` for
capacity; maximum charging power is not a vehicle-level field at all — it
lives on a separate `Systems/ElectricVehicleChargers/
ElectricVehicleCharger/ChargingPower` element (`[W]`), linked to the
vehicle via `VehicleType/BatteryElectricVehicle/ConnectedCharger` (a
`LocalReference`), a materially different, two-element mapping, not a
rename. The standard driving-behaviour fields the RCA's hardcoded seed
inputs should come from — `MilesDrivenPerYear`, `HoursDrivenPerWeek`,
`FuelEconomyCombined` — live on `Vehicle` directly (v4 schema lines
2382-2384), not under `ElectricVehicle`. HARES's parser is reading an
element that is not HPXML.

This is real, pre-existing, and not introduced by this initiative — but
it is also not reachable within this plan as a code change. Closing it
correctly requires: determining whether `Systems/ElectricVehicles` is a
documented OS-HPXML/ResStock extension this codebase intentionally
targets (checked this round — the OS-HPXML sample files vendored under
`vendors/OCHRE/test/OS-HPXML Sample Files` contain no `ElectricVehicle`
element, and OCHRE's own HPXML reader sources EV load from the standard
HPXML plug-load taxonomy instead, `vendors/OCHRE/ochre/utils/hpxml.py:
42,1728-1730` — no external source for HARES's element was found) or
whether the parser is simply wrong and must be rewritten against
`BuildingDetails/Vehicles/Vehicle`; deriving `capacity_kwh` and
`max_charging_power_kw` from the standard vehicle/battery/charger
structure (a real mapping decision spanning two linked elements, not a
rename); and re-verifying every existing HPXML EV fixture in the test
suite against whichever answer is correct. None of that is describable
as "the fix step reads this and implements it" the way every other item
in this plan is — it is a second root-cause investigation into a
different subsystem (`hares-io`'s HPXML resolver), not a consequence of
the actor/equipment silence contract this ticket's RCA diagnosed.

**Corrected per critique C1, this round:** the prior revision claimed no
tracking mechanism exists anywhere in the repository. That was false and
is withdrawn — `docs/tickets/` holds 110 numbered tickets in exactly this
format (`docs/tickets/081-charge-defect-ratio-parsed-not-applied.md` is
one worked example this review read in full to match its structure), and
the constitution's rule ("document the dependency concretely and create
the tracking ticket before marking the current ticket done") is
satisfiable today, by this plan, in that directory — not deferred to an
unnamed future closer. **Done, not deferred:** `docs/tickets/
132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md` has been
created this round, carrying the exact element names, file:line citations,
schema files checked, the standard path, and the required-behaviour steps
above, plus the note that `EvDriverActor`'s existing constructor already
has several imperial fields — a pre-existing SI-rule deviation the ticket
flags for whoever resolves it, not one this plan invents or fixes.

### What this means for the ticket's literal `LowSoc 0.15` configuration, and the question this plan puts to the operator

It remains true, after Fixes 1, 2, 3, 5, and 6 land, that an HPXML-declared
EV configured with `LowSoc { threshold: 0.15 }` is still governed by the
range-anxiety override rather than its own gate, because
`build_actors_from_seeds`'s hardcoded `daily_drive_miles = 30`,
`range_anxiety_miles = 20` put `anxiety_soc ≈ 0.25-0.28` above `0.15`
(`dwelling/mod.rs:6967-7034`, unchanged by this plan). This is real,
named, and now tracked in `docs/tickets/132-...md`, not hidden behind
"already-tracked." The ticket's own completion bar names `LowSoc` as one
of its two policy classes — whether shipping the silence-contract fixes
now, with that one configuration still ungoverned pending ticket 132, is
acceptable is not this plan's call to make unilaterally, per critique C1
this round. This plan sets `plan_outcome: blocked` (below) specifically
to put that question to the operator rather than asserting an answer:
**proceed with Fixes 1, 2, 3, 5, 6 now (closing the confirmed silence-
contract root cause and all four `reproduce` gate tests) with ticket 132
tracking the HPXML/anxiety-band gap for separate resolution, or hold I-05
until ticket 132 is diagnosed and resolved first.** The integration test
below (see Verification) demonstrates the mechanism (`LowSoc` differs
from `Immediate` by threshold, `Nightly` differs by window) using a
threshold chosen above the fixture's hardcoded `anxiety_soc`, not the
ticket's literal `0.15`, regardless of which way that question is
answered — the same limitation revision 6 stated.

## Fix 6 — `resolve()`'s tied-score target is silently dropped (closes critique W2)

### The defect

`ChargingComposer::resolve` (`composer.rs:141-147`) selects `target_soc`
only from a vote whose score is *strictly greater* than the running best:

```rust
if vote.score > best_score {
    best_score = vote.score;
    if vote.target_soc.is_some() { best_target_soc = vote.target_soc; }
    best_label = vote.label;
}
```

When the first-iterated vote sets `best_score` (even with no target) and
a later vote ties that score while carrying a target, the tie does not
win, and the target is silently lost — not chosen from a competing
preference, dropped entirely. Concretely for `TouAware`'s stack
(`PriceOptimizer`, `DepartureDeadline`, `SocTarget`, in that order,
`mod.rs:176-199`): at a neutral price `PriceOptimizer` votes idle (score
0); with SOC already at target, `SocTarget` also scores `gap = 0`.
`PriceOptimizer`'s idle vote (score 0, no target) is processed first and
sets `best_score = 0`; `SocTarget`'s vote (score 0, target `Some`) ties
rather than exceeds, so its target is dropped. This is a pre-existing
defect in the function this plan is already editing, in the same "a
strategy's content silently fails to reach the equipment" class the
ticket reports — under the operator's directive, in scope.

### The change

```rust
if vote.score >= best_score {
    best_score = vote.score;
    if vote.target_soc.is_some() { best_target_soc = vote.target_soc; }
    best_label = vote.label;
}
```

Changing `>` to `>=` makes a later, tied vote's target win over an
earlier, tied vote's absence of one — the correct outcome (never losing a
target that exists) without inventing new tie-break semantics for cases
where *both* tied votes carry a target (the later one wins, an arbitrary
but harmless choice among two equally-scored, equally-valid targets,
consistent with `power_kw`'s own existing tie-handling in the same
function, which is unconditional — `min_power_kw` folds every vote's
`power_kw` regardless of score at all).

### Consequence for existing tests

This changes `resolve()`'s output only when two votes tie in score — the
common case (a single vote's score strictly dominates) is unaffected.
**Verified this round (critique W5), not deferred to the fix step:** every
`resolve`-exercising test in `composer.rs:431-1077` uses distinct scores
or a single vote, so `>=` changes none of their assertions;
`tou_neutral_price_soc_target_drives_charging` (`mod.rs:4352-4410`) is the
one actor-level test that can tie, but its high-SOC output (`_out_high`)
is constructed and never asserted on, so it is unaffected too. One real
consequence: that same test's comment at `mod.rs:4404-4406` states the
resolved label is `"soc_target"` — true only once Fix 6 lands (under the
old `>` comparison, a tie at that point in the test's scenario would keep
whichever vote arrived first, which the comment doesn't account for); the
fix step corrects this comment alongside the code change that makes it
true, not as an afterthought.

## Interaction between the fixes

Fix 2 makes the composer reachable on evenings it previously never was (the
override no longer unconditionally preempts it). Fix 1 makes the composer's
idle decision actually hold instead of the equipment defaulting to full
charge. Fix 3 makes the composer reachable on non-driving days too, closing
the gap Fix 1 opens there, and (this round) makes `DepartureDeadline`
correctly inactive on those days instead of inventing a phantom departure
(Fix 5). Fix 6 stops `resolve()` from silently dropping a tied vote's
target. None is sufficient alone against the ticket's completion bar:
without Fix 1, a de-gated composer would still lose to the BMS default the
moment it idles; without Fix 2, the composer is rarely reached at all in
the fixture's SOC range; without Fix 3, Fix 1 regresses non-driving days
for any window strategy; without Fix 5, Fix 3 opens a phantom-deadline
path for deadline-aware strategies. They are independently testable
(Fix 1 against the two `idle_*_does_not_charge_equipment` tests, Fix 2
against the two `range_anxiety_override_does_not_preempt_*` tests, Fix 3
against the non-driving-day tests named above, Fix 5 against
`next_departure_minute_overrides_schedule_minute`, Fix 6 against a new or
re-baselined `TouAware`/tied-vote test) and touch disjoint functions or
fields, so the fix step can implement and verify them in any order, but
all must land together (see the revision 8 note above: one change, not a
staged sequence) for the ticket's completion bar to hold across driving
days and non-driving days. The ticket's own `LowSoc 0.15` configuration
is explicitly not part of what this plan closes — see Fix 4's withdrawal.

## Blast radius

**Touches:**
- `crates/hares-core/src/actors/ev_driver/composer.rs` — `emit_vote`
  restructured into the three ordered steps above: idle-hold, then
  target-of-record, then rate-of-record with no zero-power special case
  (Fix 1, responses to critique W2/W4). This is the one function every
  `ChargingPreference` in the module dispatches through
  (`TimeWindowPref`, `SocGate`, `SocTarget`, `DepartureDeadline`,
  `SolarTracking`, `PriceOptimizer`, `V2HDischarge`, `V2GExport`), so its
  behaviour change reaches all of them — named explicitly rather than left
  to be discovered per-preference.
- `crates/hares-equipment/src/ev/mod.rs` — `apply_control`'s `SOCTarget`
  and `EvSetReadyBy` arms each gain the three-line hold-clear (Fix 1); the
  `EvPlugIn` arm's `Disconnected` case gains a matching clear of
  `power_setpoint_kw`/`power_setpoint_min_soc`/`power_setpoint_max_soc`
  *and* `soc_target`/`soc_target_min`/`soc_target_max` (response to
  critique C1 last round and W3 this round, below).
- `crates/hares-core/src/actors/ev_driver/mod.rs` —
  `maybe_push_range_anxiety_override` gains the urgency branch on the
  `Some(event)` path (Fix 2); `decide`'s `None` arm routes through
  `evaluate_charging` instead of calling the override directly (Fix 3);
  the `DriverPhase::Away` arm's arrival branch also calls
  `evaluate_charging`, immediately after `reconcile_soc`, when
  `doing_plugin` is true (response to critique C1, this round — closes
  the arrival-step residual the previous revision accepted); the three
  named Fix-2 tests re-baselined; the `out.is_empty()`-asserting Fix-1
  tests re-baselined per the corrected list above; `no_event_day_emits_nothing`
  and `non_driving_day_no_anxiety_when_soc_high` re-baselined per Fix 3.
- `crates/hares-core/src/actors/ev_driver/preference.rs` — one shared
  wraparound-minutes free function, used by `DepartureDeadline` and the new
  urgency check (Fix 2).
- `crates/hares-core/src/actors/ev_driver/departure.rs` —
  `minutes_until_departure` delegates to the extracted function;
  behaviour-preserving (`departure_midnight_wrap` pins it);
  `resolve_departure` no longer falls back to the static schedule when
  `next_departure_minute` is `None` (Fix 5, closes critique C2 this
  round); `next_departure_minute_overrides_schedule_minute`'s comment and
  framing updated to match (its assertion is unchanged).
- `crates/hares-core/src/actors/ev_driver/composer.rs` (second change,
  beyond `emit_vote`) — `resolve`'s target-selection comparison changes
  from `>` to `>=` so a tied vote's target is not silently dropped (Fix 6,
  closes critique W2 this round).
- `v2g.rs`, `v2h.rs` — preference *logic* itself is untouched, but their
  behaviour through the shared `emit_vote` changes twice: their soc-floor
  idle overrides now dispatch a `PowerSetpoint{0}` hold instead of nothing
  (same as `TimeWindowPref`/`SocGate` — the fix step must not skip any
  existing V2G/V2H test asserting `out.is_empty()` on that idle path; it
  needs the same re-baseline), and their discharge steps now also dispatch
  `SOCTarget` (the W4 fix), which was previously silently dropped. This
  plan does not add new V2G/V2H tests beyond re-baselining the ones the
  `emit_vote` restructure touches — out of scope beyond that, not named in
  the ticket's completion bar.

**Deliberately untouched:**
- `hares-equipment/src/ev/mod.rs`'s `compute_charging_power_kw` fallback
  logic, `ready_soc` default, and `init_typed`'s reset block — the
  equipment's own default behaviour for an un-actored EV is correct and
  untouched.
- `power_limit_kw` handling and `EvPlugIn`'s clearing of
  `ready_by_hour`/`ready_by_soc` — untouched; `power_limit_kw` is the field
  this plan specifically avoids repurposing (see the rejected `PowerLimit`
  alternative under Fix 1) precisely so it stays free for its existing
  config meaning, and `ready_by_hour`/`ready_by_soc` clearing on disconnect
  already existed and needs no change.
- `crates/hares-core/src/dwelling/mod.rs`'s `build_actors_from_seeds`
  (`daily_drive_miles`, `range_anxiety_miles`, `event_day_ratio`, all
  hardcoded, `ActorSeed::Ev` arm, `6967-7034`) — untouched; Fix 4, which
  would have made two of these three configurable, is withdrawn this
  round (see Fix 4's section) pending resolution of a separate,
  undiagnosed defect in the HPXML EV parser (critique C2).
- `crates/hares-io/src/hpxml/resolve_der.rs`'s `resolve_ev` — untouched;
  this is the function critique C2 found reading a non-standard HPXML
  element. Named here explicitly as deliberately out of reach for this
  plan, not silently passed over — see Fix 4's withdrawal for the full
  account of why.
- `crates/hares-equipment/src/ev/config.rs` (`EvConfig`) — no new fields;
  Fix 4's two proposed fields are withdrawn.
- `crates/hares-equipment/src/lib.rs` (`ActorSeed::Ev`) — untouched.
- `soc_gate.rs`, `time_window.rs`, `soc_target.rs`, `solar.rs`, `price.rs`
  preference logic itself — confirmed correct by the RCA; only the shared
  dispatch points they flow through (`emit_vote`, `resolve`) change.

### Response to critique C1 — the latched hold silently zeroes away charging; closed by clearing on disconnect

Traced (confirmed against code): a window strategy's last home step before
departure is almost always idle (morning, outside the window), so Fix 1
dispatches `PowerSetpoint{0}` and `power_setpoint_kw = Some(0.0)` is
latched. `EvPlugIn{Disconnected}` (`ev/mod.rs:1728-1731`) clears only
`ready_by_hour`/`ready_by_soc`. While `AwayPluggedIn`, the step arm calls
`run_charging_physics(env, dt, self.away_charger_power_kw)`
(`ev/mod.rs:1355-1357`) → `compute_charging_power_kw` →
`requested = self.power_setpoint_kw.unwrap_or(derated_rated)` — with the
latched `Some(0.0)`, away charging delivers exactly `0.0` regardless of
`away_charger_power_kw`. This is a real, confirmed regression for any
driver configured with `away_charge_fraction > 0.0`: a working feature
(`ev_away_charge_increases_soc_no_residential_power` in the equipment
tests) becomes a silent no-op. The ticket's own fixture has
`away_charge_fraction = 0.0`, which is exactly why this plan's earlier,
fixture-scoped analysis missed it.

Fix 1's `apply_control` change gains one more clearing site:
`ControlSignal::EvPlugIn`'s `Disconnected` arm additionally clears
`power_setpoint_kw`, `power_setpoint_min_soc`, `power_setpoint_max_soc`,
and (per critique W3, this round — see Fix 3's revised disconnect-clearing
subsection for the full reasoning and code) `soc_target`,
`soc_target_min`, `soc_target_max`, alongside the existing
`ready_by_hour`/`ready_by_soc` clear. The critique asked whether any
caller relies on `power_setpoint_kw` surviving a plug cycle and found
none; this plan finds none either (the in-repo occupant actor, per
critique O4, re-sends its own `PowerSetpoint` every present step, so a
clear-on-disconnect does not defeat it — it would simply re-set on the
next step it runs, same as today), and the same check now applies to
`soc_target` too, rather than leaving it uncleared on an unverified
"might still matter."

**Test consequence:** the equipment test
`ev_away_charge_increases_soc_no_residential_power` and any other test
exercising `Disconnected → AwayPluggedIn` after a prior `PowerSetpoint`
dispatch must be checked by the fix step; none is expected to need
re-baselining (the new clear only removes a value these tests never set in
the first place), but this must be confirmed by reading each, not assumed.

## Verification

All six fixes must satisfy all four tests `reproduce` listed, plus the
full passing-command set. The four gate-test commands and the two `--lib`
commands are unchanged from `rca.md`/`reproduction.md`; the suite list
below them is widened this round (critique W6):

- `cargo test -p hares-core --lib actors::ev_driver::tests::idle_nightly_window_does_not_charge_equipment` — FAIL → PASS (Fix 1).
- `cargo test -p hares-core --lib actors::ev_driver::tests::idle_low_soc_gate_does_not_charge_equipment` — FAIL → PASS (Fix 1).
- `cargo test -p hares-core --lib actors::ev_driver::tests::range_anxiety_override_does_not_preempt_low_soc_gate` — FAIL → PASS (Fix 2).
- `cargo test -p hares-core --lib actors::ev_driver::tests::range_anxiety_override_does_not_preempt_nightly_window` — FAIL → PASS (Fix 2).
- `cargo test -p hares-core --lib` — full crate suite green, with every
  test named in "Blast radius" above, and in Fix 1's, Fix 3's, Fix 5's,
  and Fix 6's test-audit subsections, re-baselined (not deleted, not
  weakened) to the new contract — including `no_event_day_emits_nothing`,
  `non_driving_day_no_anxiety_when_soc_high` (Fix 3), and
  `next_departure_minute_overrides_schedule_minute` (Fix 5).
- `cargo test -p hares-equipment --lib` — must stay green; `SOCTarget`/
  `EvSetReadyBy` handler changes are additive clears, not behaviour changes
  to any value the equipment tests assert on for their existing scenarios.
- `cargo test -p hares-core --tests` and the four Python suites (critique
  W6, see below) — newly added to the gate this round.
- `cargo clippy --workspace -- -D warnings` — zero new warnings.
- `cargo fmt --check --all` — formatting clean.

The RCA's own stated completion bar requires more than the four unit tests:
"Verification of any fix must be end-to-end on the ticket's actual reported
path... asserting *equipment* charging energy per hour... confirming two
`Nightly` configurations differing only in `off_peak_start_hour` produce
measurably different hourly profiles, and `LowSoc` differs from
`Immediate`." No such test exists yet. "Differs" needs a stated observable,
not just existence — a plan cannot invent exact tolerances the fix step
must own, but it can and does name what must be measured. The fix step
must add one integration-level test building the EV through
`build_actors_from_seeds`/`ActorSeed::Ev` with an HPXML-style
`charging_strategy` override (mirroring the RCA's reverted probe
construction), stepped over multiple simulated days, computing an
hourly-energy histogram from equipment telemetry (`observed_charge_kw`,
per the RCA's own pointer, `ev_driver/mod.rs:464-467`) and asserting, for
each pair:

**Precondition (established two rounds ago, restated with the correct
condition per critique W1 this round):** criterion (a) below is only
meaningful if the range-anxiety override (Fix 2) provably never fires
during the test's simulated span. The condition that actually guarantees
this is **not** "perceived SOC never crosses `anxiety_soc`" — after Fix 2
the override fires only when `hours_left < needed_charge_hours_to_target(
anxiety_soc, ...) * 1.2`, a time-*and*-SOC condition, so SOC can legally
sit below `anxiety_soc` for hours without the override firing, provided
there is enough time in hand before the next departure. The prior
statement of the precondition overstated what was needed and, per critique
W1, its own worked example (`LowSoc { threshold: 0.35 }` against a
`~0.28` band) would in fact breach even that overstated version — the
fixture's ≈0.17-SOC/day drain crosses from above `0.35` to below `0.28`
in one drive, landing inside the band it was supposed to stay clear of.
That example is withdrawn.

**Resolved by choosing a `LowSoc` threshold whose recharge point sits
above `anxiety_soc` by more than one day's maximum drain — not by making
the seed inputs configurable (Fix 4 is withdrawn this round; see above).**
`SocGate`'s hysteresis means `LowSoc { threshold }` actually recharges at
`threshold - SOC_GATE_DEFAULT_HYSTERESIS_BAND` (`0.05`,
`ev_driver/mod.rs:99-104`), not at `threshold` itself. For the override
to stay provably inert, that recharge point must exceed `anxiety_soc` by
more than the fixture's maximum single-day SOC drain (drive energy ÷
capacity_kwh) — otherwise, as critique W1 (two rounds ago) correctly
showed for the withdrawn `threshold: 0.35` example, one day's drive can
carry SOC from just above the recharge point straight through
`anxiety_soc` before `LowSoc` gets another turn. The fix step must
compute, for its chosen fixture (capacity, charger rating,
`fuel_economy_kwh_per_mi`, and the fixture's own drive-energy-per-day),
a threshold satisfying:

```
threshold - SOC_GATE_DEFAULT_HYSTERESIS_BAND > anxiety_soc + (max_daily_drive_kwh / capacity_kwh) + margin
```

and state the chosen numbers and the arithmetic in the test itself, not
merely assert a round number — this is exactly the check that was skipped
two rounds ago and must not be skipped again. This demonstrates the
mechanism the ticket's completion bar asks for (`LowSoc` differs from
`Immediate`) using a threshold the fixture's own hardcoded `anxiety_soc`
permits, not the ticket's literal `0.15` — see "What this means for the
ticket's literal `LowSoc 0.15` configuration" under Fix 4 above for why
that gap is not closed by this plan.

Determinism (per the project's deterministic-tests rule) still requires
the test not rely on `event_day_ratio`'s RNG-seeded daily roll landing on
favourable days: fix `event_day_ratio = 1.0` (every day drives, matching
the ticket's own 30/30-day report) or use a fixed seed and verify by
computation, not by observation, that the chosen threshold satisfies the
inequality above regardless of which days roll as driving days.

**Arrival-step residual (established last round, restated for clarity):**
the prior revision required criterion (a) to hold "for every simulated
day" while accepting an arrival-step residual that violated it every day.
That residual is closed at the source, not narrowed around — the `Away`
phase's arrival branch calls `evaluate_charging` in the same step as the
`HomePluggedIn` transition (Fix 1's disconnect-clearing subsection) — so
criterion (a) genuinely holds for every simulated day once the
precondition above is satisfied.

- **Two `Nightly` configs differing only in `off_peak_start_hour`** (e.g.
  22:00 vs. 02:00), given the precondition above holds: (a) charging energy
  accumulated in hours *outside* each run's own configured window is within
  numerical noise of zero (a small fixed epsilon, e.g. well under one
  charging step's worth of energy at rated power) for every simulated day;
  (b) the hour of first non-zero charging energy after plug-in differs
  between the two runs by an amount consistent with the configured
  start-hour shift.
- **`LowSoc` vs. `Immediate`**: with `LowSoc`'s `threshold` chosen per the
  inequality above (demonstrating the mechanism, not the ticket's literal
  `0.15` — see Fix 4's withdrawal), SOC (or cumulative charging energy) at
  the first hour after plug-in differs between the two runs — `Immediate`
  shows non-zero charging energy in that hour, `LowSoc` shows none until
  SOC crosses its configured threshold on a later day.

This is the only test in the suite that directly checks the ticket's
literal completion bar rather than the unit-level mechanism proxies above;
the two observables above are what the fix step measures, not merely that
the two runs produce numerically different totals (which, per the ticket's
own report, two visibly-different-shape profiles can coincidentally share
to within 0.1 kWh in 330).

### Response to critique W3 — `PlugInPolicy::LowSoc` dispatching to a `Disconnected` EV is real, checked, and verified inert

`low_soc_policy_reconciliation_fires_on_arrival_without_plug_in`
(`mod.rs:5189`) pins that the actor's `DriverPhase` can be
`HomePluggedIn` while the equipment's `connection_state` is still
`Disconnected` (`PlugInPolicy::LowSoc` with SOC above its threshold).
After Fixes 1 and 3, `evaluate_charging` now dispatches every such step,
where before it was silent — but read against the equipment's own
`Disconnected` step arm (`ev/mod.rs:1369-1382`) to check the consequence,
not assumed harmless: that arm does not call `run_charging_physics` and
does not read `power_setpoint_kw`, `soc_target`, or any field these
dispatches write — it runs thermal drift only (`self.active_power_kw =
0.0`, `battery_temp_c` decays toward ambient). So a dispatch reaching a
`Disconnected` EV writes a value with no physics consequence: not
consumed until the vehicle actually plugs in, at which point the composer
recomputes fresh on the very next step, exactly as every other reconnect
case in this plan already relies on. The one real effect is
observational: the actor's own telemetry (fed from `out`, not from the
equipment) can report a commanded rate for a car not physically
connected. This is a pre-existing phase/connection-state mismatch
(`low_soc_policy_reconciliation_fires_on_arrival_without_plug_in`'s own
name states it is intentional, already-tested behaviour), and this plan's
change to it is real but bounded to telemetry accuracy, verified inert
for equipment physics by reading the exact code path rather than by
scope alone — accepted on that specific, checked basis, not on the
`PlugInPolicy::LowSoc` route being "not the ticket's route."

### Response to critique W4 — what `actual_soc(env)` holds on the arrival step, stated precisely

`reconcile_soc`'s `actual_soc(env)` reads `env.equipment_core`
(`mod.rs:665-671`), populated from the *previous* step's equipment output
(confirmed this round: actors decide, then the dispatcher applies
controls, then equipment steps, in that order within one `Dwelling::step`
— `dwelling/mod.rs:~4945,~5036,5179`). On the arrival step, "the previous
step" is the last driving step, during which `EvDrive{kwh}` was applied
directly to `self.soc` (a synchronous subtraction in `apply_control`, not
a per-step physics accumulation) — so the snapshot `actual_soc(env)` reads
on arrival already reflects the completed trip's consumption, which is
exactly the value reconciliation is for. The range-anxiety entry gate
(`perceived_soc() < anxiety_soc`) being evaluated on the arrival step for
the first time (via the new `evaluate_charging` call) is intended, not
incidental: if a long drive leaves the driver in the anxiety band, firing
the safety override immediately on arrival rather than waiting one more
step is the correct behaviour, and is exactly the kind of step the
override exists for.

### Response to critique W5 — a held EV's battery heater is unaffected by this plan, by design that predates it

`run_charging_physics`'s heater-activation condition
(`ev/mod.rs:1198-1206`, `would_charge_underated = !is_v2l_discharge &&
self.soc < self.effective_soc_limit()` gated further by
`battery_temp_c <= heater_threshold_c`) does not consult
`power_setpoint_kw` and never has — heater activation has always been
independent of the actor-commanded charge rate, gated only on SOC-below-
limit and cold temperature. Before this plan, an idling strategy's silent
step left the EV charging at rated power regardless (the bug this ticket
reports), so the heater condition and the charger were always both "on"
together; after this plan, an idling strategy's hold stops the charger
(`observed_charge_kw = 0`) while the heater condition, unchanged, can
still be true. This is not a new coupling this plan introduces — the
heater's own logic never depended on charge rate — it is a previously
unobservable combination becoming observable now that the charger and
"would the BMS want to charge" are no longer forced together by the bug.
**Decided, not deferred — corrected twice, see the revision 9 note at the
top of this document for the full reasoning (critique W1, that round):**
an earlier version of this response justified leaving the heater
unchanged on the claim that it provides general pack protection
independent of schedule. That claim was checked against the gating
condition and found wrong — the condition is charge *preconditioning*
(fires only when the BMS would currently be charging), not general
protection, confirmed by `heater_only_grid_draw_when_fully_cold_derated`
(`ev/tests.rs:567-600`), which requires the heater to activate precisely
when cold-derate has zeroed `charger_kw`, so charging can start. The
heater is left unchanged not because it "protects the pack regardless of
schedule" but because the current condition cannot safely be changed to
follow the hold without breaking that verified preconditioning behaviour,
and correctly distinguishing "blocked by cold-derate" from "blocked by a
deliberate strategy hold" requires information (why the equipment isn't
charging this step) that doesn't exist in the current control-signal
vocabulary — inventing it is infrastructure the RCA's root cause never
called for. `DEFAULT_HEATER_POWER_W = 0.0` (`config.rs:61`) means this
has no consequence for the ticket's fixture or completion bar either way.
The integration criteria above remain deliberately scoped to charger
energy (`observed_charge_kw`), not total EV electrical port load — a
stated boundary of that test, not an oversight.

### Response to critique W6 — the passing-command set is widened to the suites this change reaches

The RCA's `passing_commands` (two `--lib` runs, clippy, fmt) did not
anticipate a change this wide when it was written; this plan's blast
radius (every `ChargingPreference`, `decide`'s dispatch shape on every day
type, `resolve_departure`, `resolve`'s tie-break) reaches further than
`--lib` tests alone — `EvConfig` and `ActorSeed::Ev` are not touched by
this plan (Fix 4 is withdrawn), so they are not part of what these
commands need to cover, but every other change is. Extending the
verification set here is not inventing commands ungrounded in the
codebase — every command below targets a test suite that already exists
and already exercises EV dispatch:

- `cargo test -p hares-core --tests` — covers `crates/hares-core/tests/`,
  including `actor_telemetry_regressions.rs` (constructs an
  `EvDriverActor` and asserts on `charge_kw`) and
  `checkpoint_regressions.rs` (round-trips EV state); either must stay
  green or be re-baselined to the new contract, not weakened.
- `uv run pytest tests/python/test_ev_archetypes.py
  tests/python/test_ev_tariff_integration.py
  tests/python/test_py_dwelling_integration.py
  tests/python/test_bms_modes.py` — all four step EVs through the Python
  binding and can pin energy totals, night counts, or dispatch shape that
  moves under Fixes 1, 2, 3, 5, 6; per the constitution's testing rules,
  any that move must be re-baselined to the corrected behaviour, never
  weakened to pass.

This plan does not itself audit each of these six suites line-by-line
(that is execution, not planning), but it commits the fix step to running
and reconciling all of them, not only the two the RCA named before this
plan's scope grew.
