---
retro_outcome: sound
---

# Retrospective — I-04: EV driver telemetry inert during goal-based charging

## The bug, restated

`EvDriverActor` has a deliberate driver/equipment split: the driver dispatches
intent (goals like `SOCTarget`/`EvSetReadyBy`, or rates like `PowerSetpoint`),
and the equipment's own BMS decides the actual power. The defect was that the
driver's own telemetry (`charge_kw`, `soc`, `needed_charge_hours`) was built
entirely from the *dispatch side* of that split — it scanned the signals the
driver emitted and tracked the driver's own belief SOC. Six of nine charging
strategies vote goals, not rates, so for exactly the strategies that govern
ordinary scenarios the telemetry channels were structurally inert: `charge_kw`
stayed 0.0 while the equipment drew real power, `soc` only caught up at two
sparse reconciliation triggers, and `needed_charge_hours` collapsed the
"no estimate" sentinel to a value indistinguishable from "already done."

The class, in one sentence: **an observability channel was derived from
dispatched intent and internal belief instead of observed ground truth, so any
code path that achieved the same physical effect by a different dispatch means
left the channel silently flat.**

## What defense would have caught it earlier

A concrete one, and the fix installed it:

1. **A shared observation path.** `observed_charge_kw` (mod.rs:422) and
   `telemetry_estimate_context` (mod.rs:836) read the equipment's committed
   output back into the driver's telemetry, with the dispatched rate as a
   deliberate precedence case. The broken form — telemetry that never consults
   observation — is no longer the natural way to write a channel.
2. **An enum-exhaustive class guard.** `every_strategy_variant_telemetry_reflects_active_charging`
   (mod.rs:1655) iterates all nine `ChargingStrategy` variants and asserts the
   telemetry reflects active charging for each. A future strategy variant
   cannot silently inherit the gap — the constitution's existing rule on
   enum-variant exemptions applied here, and the fix honored it.
3. **A reproduction pitched at the rule, not the route.** The failing test
   asserted the invariant (telemetry must reflect charging for every strategy
   that governs the session), so a cosmetic one-channel or one-strategy fix
   stays red.

This is a mechanism, not "better testing": the broken form is now pinned by
tests over the shared mechanism, and the review plus red-team passes
verified the dispatch/telemetry split didn't leak observed state back into the
control path.

## Where the defense now lives

- **Mechanism + tests:** `crates/hares-core/src/actors/ev_driver/mod.rs`
  (`observed_charge_kw`, `telemetry_estimate_context`, the reproduction test,
  and the enum-exhaustive guard) and `composer.rs` (dispatch-only `evaluate`).
- **Constitution:** the general principle was not yet written down. The
  closest existing rules were the project-specific "truthful face" rule and
  the 3am-call reporting rule, neither of which names this exact failure
  shape. I added a principle to `.shipwright/constitution.md` under
  "accountable for what the system does when it goes wrong": **"Telemetry
  reports what the system did, not what it was told to do"** — read ground
  truth where observable, bound and document estimates where not, and pin the
  channel over every variant of the behavior enum. This is a written
  principle, not a mechanism, and I say so plainly: the judgement of *which*
  channels imply physical effect and *which* values are ground truth is
  semantic — no lint or type can decide that `dr_complied` (correctly
  intent-faced) differs from `charge_kw` (must be observed). The enforcing
  mechanism for this instance is the one the fix installed: the shared
  observation helpers mean a telemetry path that never consults the
  environment must now bypass `observed_charge_kw` deliberately, and the
  enum-exhaustive test goes red if a strategy variant inherits the gap. What
  the constitution entry adds is that the next driver/equipment split gets
  this scrutiny at design time rather than after a ticket.

## Diagnosis divergence

Minor, and worth recording because it shows where the investigation was
misleading. The RCA initially listed `SolarSurplus` among the strategies whose
`needed_charge_hours` collapsed to 0.0, then self-corrected (its stack always
contained a `DepartureDeadline`, so it reported a real estimate). The review
noted the ticket's Implementation Notes still carry the stale claim — harmless
to the code, but a reminder that a mechanism traced in source ("the fold
collapses INFINITY") can over-predict affected sites when a config-time detail
(stack composition) quietly rescues one. The reproduction test settled it by
testing the four actually-affected strategies rather than asserting the
traced list. The core diagnosis (three telemetry gaps, dispatch-side-only
channels) matched the actual cause exactly; there was no divergence in the
fix direction.

## Forward-looking note

The defense vocabulary this adds: **intent-derived telemetry is a class risk
in any driver/equipment (or controller/actuator) split.** When one component
dispatches and another executes, the dispatcher's observability channels must
read back the executor's committed output wherever it is published — and the
exhaustive-variant test is the cheap pattern that keeps the next strategy,
mode, or phase from reopening the class.

### Where else this shape is possible today

I searched for sibling sites myself rather than relying on the review's single
`telemetry.set("soc"` grep. Searches run: `grep -rn 'fn populate_telemetry'
crates/hares-core/src/actors/` (only `ev_driver` has one) and `grep -rn
'telemetry.set(' crates/hares-core/src/actors/ | grep -v ev_driver`, then each
hit classified by whether the channel implies a *physical effect* while ground
truth is observable in `env`:

- `bms.rs:1040-1061` — `soc`, `load_kw`, `pv_kw` all read `env` ground truth
  (`read_soc(env)`, `env.electrical.*`). Not the shape.
- `safety_monitor.rs:209` — `min_zone_temp_c` reads observed zone temps. Not
  the shape.
- `dr_compliance.rs` — `dr_complied`, `dr_level`, `signals_count`,
  `dr_freeze_guard`, `effective_compliance_rate`: every channel's face is the
  actor's own *decision or configuration*, and the actor is both decider and
  dispatcher (it simulates occupant compliance; the physical effect is owned
  by the target equipment). Intent is the correct face for these channels —
  the class appears only if a channel implying physical effect (e.g.
  `load_curtailed_kw`) is ever added here. None exists.
- `occupant.rs:447-448`, `ideal_thermostat.rs:306-317` — counts of own
  dispatches and own override state; the channel's face is the actor's own
  state, and there is no separate executor to read back. Not the shape.

The class does not currently recur elsewhere in the tree. The constitution
principle and the exhaustive-variant test pattern are the standing defense
for the next actor that gains an executor it could fail to read back.

Verdict: **sound** — the fix installed both the shared mechanism and the
class-wide test, review and red-team found nothing further, and the one
missing piece (a written principle) is now in the constitution.
