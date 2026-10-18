---
retro_outcome: sound
---

# Retrospective — I-05 (EV charging-strategy override does not govern behaviour)

## What the bug actually was

An HPXML-declared EV with a `charging_strategy` override produced the same
charge-on-plug-in profile for every strategy variant. The confirmed root cause
(RCA revision 2) was an undefined silence contract across the actor/equipment
boundary: when a `ChargingStrategy` resolved to "wait," `ChargingComposer`
dispatched nothing at all — no hold, no inhibit — and the equipment's
deliberate, tested BMS default for an un-actored vehicle ("charge to
`ready_soc` = full") filled the gap for an actor-controlled one. Compounding
it, every actor-writable setpoint latched until `init_typed`, so a single
early `SOCTarget` governed all later plug-ins.

## Diagnosis divergence — worth recording

The first RCA blamed the range-anxiety override preempting the composer. That
defect is real (its reproduction tests genuinely fail), but it cannot produce
the ticket's 28/30-nights-identical profile — a conditional threshold crossing
never yields that shape. What misled the first pass: every existing "idles"
test asserted only "no `SOCTarget` in `out`," so "the actor said nothing" was
treated as equivalent to "the EV didn't charge." The divergence wasn't a wrong
hypothesis so much as a test-design blind spot — dispatch-absence assertions
one layer short of the effect. The fix-plan critique loop (`halt` on H1) is
what forced the re-diagnosis; that gate earned its keep this round.

## Was a defense added that would catch this class earlier?

Yes — three, and each is verified in the shipped change, not proposed:

1. **Mechanism (makes the broken form unrepresentable):** the shared choke
   point `ChargingComposer::emit_vote` (`crates/hares-core/src/actors/
   ev_driver/composer.rs:257-269`) now cannot resolve a decision without
   emitting something — idle-shaped votes dispatch an explicit
   `PowerSetpoint{0}` hold, and the zero-power early return that silently
   swallowed votes was removed. "Say nothing" is no longer expressible by the
   composer. Verified present in the code this session.
2. **Latch-clearing rule:** `apply_control` clears the hold on a fresh
   `SOCTarget`/`EvSetReadyBy` and on disconnect, closing the opposite failure
   mode ("never charges") that a naive hold would have introduced.
3. **Tests at the effect level:** the reproduction pair
   (`idle_nightly_window_does_not_charge_equipment`,
   `idle_low_soc_gate_does_not_charge_equipment`) and the completion-bar
   pair assert *equipment* SOC/energy across the boundary, not dispatch
   contents. The reviewer additionally proved the fix is causal by
   temporarily reverting it and watching the pre-fix signature return.

4. **Team vocabulary:** I added a "Silence is not a decision" principle to
   `.shipwright/constitution.md` (under Code quality), because the
   generalizable lesson is a judgement call no static check can enforce:
   every resolved decision — including "wait" — must be dispatchable;
   latched receiver state needs a named clearing rule; and tests must assert
   the receiver's observable state, not the sender's silence. The class is
   EV-shaped today, but any future actor/equipment protocol (battery, DR
   overrides, new equipment actors) hits the same trap.

## Same shape next door — the sweep I ran for this retrospective

The defense would be instance-only if the same composition — actor silence +
extreme equipment default + latched setpoint — survived elsewhere. I searched
the actor side fresh (`grep` for silent `return;` paths in every actor under
`crates/hares-core/src/actors/`, then read the equipment default each one
relies on); the equipment side was already swept by the review:

- **`bms.rs` (battery actor).** Silence exists: `BmsMode::Manual` sets
  `last_action = "idle"` and dispatches nothing (`bms.rs:206-208`), and the
  SelfConsumption/TOU arms early-return on `idle:no_soc`. But the battery's
  silent default is context-sensitive self-consumption (`battery/mod.rs`
  `determine_target_power` Priority 3, per the RCA's bounded check), not a
  fixed extreme — and its `soc_target`/`power_setpoint_kw` mutual clearing is
  pinned by tests this initiative added
  (`soc_target_supersedes_latched_power_setpoint_and_vice_versa`). One half of
  the dangerous composition is present, not both. Not the bug class today.
- **`dr_compliance.rs`.** Already embodies the defense: `track_dispatched`
  (`dr_compliance.rs:583-593`) records every sticky action (`PowerLimit`,
  `TurnOff`) and the actor emits explicit clears when the DR event ends —
  "latched receiver state must have a clearing rule" was already the norm
  here. The EV composer was the outlier, now brought to the same standard.
- **Generator / water heater / HVAC** (equipment-side sweep from the review,
  re-confirmed): generator defaults to its own last output with an explicit
  disarm path; WH/HVAC are thermostatically self-regulating around a setpoint
  band. No other charge-to-extreme default exists in the workspace.

**The check that now fails if this shape reappears:** for the EV path, the
composer choke point makes the silent form unwriteable, and the reproduction
pair (`idle_nightly_window_does_not_charge_equipment`,
`idle_low_soc_gate_does_not_charge_equipment`) fails red the moment equipment
charging energy appears across an idle window — that pair is the tripwire for
any future regression of the contract. For *new* actor/equipment pairs, no
static check exists; the defense is the constitution principle, which is now
where every agent reads it before touching code. I considered a mechanical
gate (e.g. a lint or workspace test over all actors) and rejected it: the
rule is a design judgement about what a receiver's default should be, not a
greppable shape, and a mechanism that pattern-matches `return;` would false-
positive on every legitimate idle in `bms.rs` and `dr_compliance.rs`.

## Overreach check

I changed exactly two files: `.shipwright/constitution.md` (the principle
above) and this retrospective. No behaviour, no refactors, no drive-by fixes.

## What remains open (tracked, not a gap)

The ticket's literal `LowSoc{0.15}` configuration is still governed by the
range-anxiety override rather than its own gate, because `anxiety_soc`
(≈0.25–0.28, from hardcoded seed inputs) sits above 0.15. This is
operator-approved and tracked in `docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`,
a separately-diagnosed HPXML parser/config-threading defect. The review
confirmed the tracking ticket exists and names the mechanism. The held-EV
heater ambiguity is documented and inert (`DEFAULT_HEATER_POWER_W = 0.0`).

## Verdict

`sound`. The fix targets the confirmed root cause at the shared choke point,
the review verified causality (not just green tests), the similar-issue sweep
checked battery/generator/HVAC for the same default-substitution shape, and
the defense now lives in three places a future change will hit: the composer
mechanism itself, the equipment's clearing rules, and the constitution.
