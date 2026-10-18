---
critique_outcome: findings
---

# Critique: fix-plan.md, revision 8 (I-05 — Fix 4 withdrawn; Fixes 1, 2, 3, 5, 6)

## Operator directive still in force

"No broken windows, we must fix all issues, pre-existing or not." This
round's central question is whether the plan's withdrawal of Fix 4 and
its `ready` verdict are compatible with that directive and with the
constitution's deferral rules. They are not, as written — but the reason
is narrower than the plan thinks, and it is fixable.

## What I reviewed and how

Full read of revision 8 (1509 lines) with attention to what changed:
the revision-8 note (173–220), the withdrawn Fix 4 section (962–1059),
the restated integration-test precondition and `LowSoc` inequality
(1316–1390), the heater decision (1445–1479), the W6 response
(1481–1509), and every remaining reference to Fix 4/`EvConfig`/
`ActorSeed::Ev`. Verified against code and the repository:
`SOC_GATE_DEFAULT_HYSTERESIS_BAND = 0.05` (`mod.rs:100`) and `SocGate`'s
recharge-at-lower-threshold behaviour (`soc_gate.rs:39–66`) — the plan's
inequality is right (with the recharge point more than one day's drain
above `anxiety_soc`, the drive that crosses it lands above the band); the
heater condition (`ev/mod.rs:1198–1206`); the schema claims in Fix 4's
withdrawal (v4 `HPXMLBaseElements.xsd` — `ElectricVehicle(s)` absent,
`Vehicles/Vehicle` present, confirmed again); and — because the plan's
verdict rests on it — whether a tracking mechanism exists in this
repository.

What holds up: Fixes 1, 2, 3, 5, 6 are unchanged from the versions
verified in prior rounds; the integration-test inequality corrects the
earlier arithmetic error properly; the arrival-step residual and
disconnect clears stand; the stale composer-test comment is scheduled
with Fix 6; the heater question now has a stated decision.

---

## Critical

### C1. The plan's verdict rests on a false claim — "no `tickets/` mechanism exists anywhere" — while the repository has one with 110 tickets in it; and `ready` with the ticket's own configuration left broken is not a verdict the plan is entitled to give on its own

The plan: "no `tickets/` mechanism exists anywhere in this initiative or
the one (`I-01`) the constitution cites as precedent (confirmed by
directory listing this round). Whoever closes I-05 must either establish
that tracking mechanism and open it, or route this specific question …
back through `diagnose`." And: "`plan_outcome` for this revision:
**ready** … Fix 4's withdrawal is a deliberate, evidenced scope boundary".

Checked: `.shipwright/initiatives/*/tickets/` does not exist, true — but
`docs/tickets/` does, with 110 numbered tickets
(`001-unify-hfg-add-humidity-port.md` … `081-charge-defect-ratio-parsed-not-applied.md`
…). That is the project's tracking mechanism; the constitution's own
rule the plan quotes ("create the tracking ticket before marking the
current ticket done") is satisfiable today, by this plan, in that
directory. The plan instead hands the obligation to an unnamed "whoever
closes I-05" — the same ownerless deferral that was flagged three rounds
ago (revision 3's W4) and that revision 6 was faulted for calling
"already-tracked".

Two things follow:

1. The constitution's cross-subsystem exception — which the plan invokes
   verbatim to justify withdrawing Fix 4 — is conditional on the ticket
   existing *before* done. Without naming the ticket the plan will
   require (a path under `docs/tickets/`; what it carries is the
   author's to write), the exception does not apply and the withdrawal
   is an untracked deferral of exactly the kind the directive forbids.

2. Even with the ticket, `ready` is the wrong outcome for the plan to
   self-assign. The plan states, in its own words, that after it lands
   "an HPXML-declared EV configured with `LowSoc { threshold: 0.15 }` is
   still governed by the range-anxiety override rather than its own
   gate". The ticket's completion bar names `LowSoc` as one of its two
   policy classes, and the operator's directive says fix everything.
   Whether shipping the silence-contract fixes now, with that
   configuration still ungoverned and the parser question open, is
   acceptable is the **operator's** decision, not the plan's. The
   plan-fix role's own rule is that a plan which finds part of the
   requirement unscopeable is `blocked`, not `ready`. The honest outcome
   is `blocked` with the decision stated as a question: proceed with
   1/2/3/5/6 and ticket C2 + the `0.15` residual in `docs/tickets/`, or
   hold I-05 until the HPXML vehicle question is diagnosed. The plan
   makes a strong case for the former; it should make it as a request,
   not a verdict.

Consequence if left as is: the fix step implements a `ready` plan whose
own text says the ticket's configuration stays broken, the constitution's
ticket precondition is unmet because the plan told the closer no
mechanism exists, and the operator's directive is silently overridden by
a scope argument nobody asked them to approve.

---

## Warning

### W1. The heater decision's rationale contradicts what the heater code actually does

"battery thermal conditioning stays independent of the charging-strategy
hold … a real EV's BMS manages pack temperature via its own AC-coupled
conditioning path whenever plugged in … protecting the battery is not
conditional on the grid-side schedule".

The code's heater is not pack protection. It is gated on
`would_charge_underated = !is_v2l_discharge && self.soc <
self.effective_soc_limit()` (`ev/mod.rs:1198–1206`): it runs only when the
BMS *would be charging* — i.e. it is charge preconditioning. A pack at
its SOC limit in −20 °C gets no heater under this code; that is the
opposite of "protecting the battery is not conditional". So the plan has
decided to keep the heater independent of the hold on the grounds that
it is pack protection, when the code says it is charge preconditioning —
and charge preconditioning running for hours during a deliberate no-charge
window is the behaviour the decision was supposed to justify. Either the
rationale is right and the gating condition is wrong (a pre-existing
defect, in scope under the directive), or the gating is right and the
heater should follow the hold. The decision must be made against what
the code does. (`DEFAULT_HEATER_POWER_W = 0.0` keeps the ticket's fixture
unaffected either way; the plan is right to say so.)

### W2. The document now carries two contradictory revision notes and stale scope statements about Fix 4

The revision-7 note (lines 135–142) still reads "Fixed, not ticketed:
**Fix 4** makes `daily_drive_miles`/`range_anxiety_miles` configurable …
so the anxiety band can be moved below `0.15`", and the W1 bullet
(149–154) "the test now uses the ticket's own `threshold: 0.15`". The
revision-8 note withdraws both. The W6 response still says the blast
radius reaches "`EvConfig`, `ActorSeed::Ev`" and that suites "move under
Fixes 1-6" (1485, 1502). A reader who starts at the top is told Fix 4
ships and the `0.15` test exists; a reader of the blast radius is told
neither. The plan keeps history "so the reasoning is not lost" — fine —
but superseded statements must be marked as such where they sit, not
only in a later note.

### W3. Fix 4's schema evidence is overbroad in one particular that the future ticket will copy

"None of `ElectricVehicles`, `ElectricVehicle`, `BatteryCapacity`, or
`MaxChargingPower` exists in either schema". `BatteryCapacity` does exist
in the v4 schema — as a *type* (`BatteryCapacityType`, `type="BatteryCapacity"`,
lines 4077–4080), used by `Battery/NominalCapacity`/`UsableCapacity`
(2213–2218). The element names HARES reads are absent; the type is not.
Since this paragraph is the evidence the C2 ticket will be written from,
it should be exact: the non-standard part is the element path, and the
standard battery capacity lives at `…/BatteryElectricVehicle/Battery/
NominalCapacity` with a `Units` sibling.

---

## Observation

- **O1.** The plan is now 1509 lines and its outcome depends on a decision
  the operator has not been asked. Once the operator answers C1, the
  history (three revision notes, seven "Response to critique" sections,
  a withdrawn fix kept in full) should move to the decision log and the
  plan should read as the change.
- **O2.** `docs/hpxml/` remains absent while the constitution mandates
  consulting it for any HPXML work; the C2 ticket should note that the
  reference itself has to be regenerated (`fetch_hpxml_dd.py` is also
  absent) before the parser question can be answered rigorously.

---

## Reflection

*Artifact.* Revision 8: the silence-contract fixes (1, 2, 3, 5, 6)
unchanged; Fix 4 withdrawn on schema evidence; integration test restated
with a correct hysteresis-aware inequality; heater decided; verification
widened; `ready` asserted with the ticket's `LowSoc 0.15` explicitly left
ungoverned.

*Claims verified.* The inequality and `SocGate` recharge point —
confirmed. Schema absence of `ElectricVehicle(s)` — confirmed again;
`BatteryCapacity` as a type — **exists** (W3). "No `tickets/` mechanism
exists anywhere" — **refuted** (`docs/tickets/`, 110 entries). Heater =
pack protection — **refuted** by the gating condition (W1). "This note is
history, not a forward reference" — true, but the earlier notes now
contradict it (W2).

*Checked this round rather than accepted.* Whether
`Systems/ElectricVehicles` is an OS-HPXML/OCHRE convention: the OS-HPXML
sample files vendored under `vendors/OCHRE/test/OS-HPXML Sample Files`
contain no `ElectricVehicle` element at all; OCHRE itself sources its EV
from the standard HPXML plug load `PlugLoadType="electric vehicle
charging"` (`vendors/OCHRE/ochre/utils/hpxml.py:42, 1728–1730`), not from
any vehicle element. The only definitions of `ElectricVehicles` in the
workspace are HARES's own (`hares-io/src/hpxml/equipment.rs`,
`resolve_der.rs`, one regression test, and the Python injection test).
So the element is a HARES invention with no documented external source —
which is what the C2 ticket must say, and it also means the reference
implementation reaches EV data by a path HARES does not parse either.
That is a fact for the ticket, not a decision made here.

*What the plan never mentions.* `docs/tickets/` (C1); that the heater is
gated on would-charge (W1); that its own earlier notes now say the
opposite of its current scope (W2).

*Overreach check.* C1 names the two outcomes the operator can choose and
says the plan should ask, not decide; it does not choose for them. W1
names the two consistent resolutions and asks for the decision to be
made against the code, not which one.

*Build-changing vs. document-only.* Build-changing: C1 (outcome and
ticket), W1. Document-only: W2, W3, O1, O2.

*Severity.* `findings`, not `halt`. I considered `halt`: the plan has
reached a decision only the operator can make. But the mechanism to
surface that decision exists inside the loop — the plan sets `blocked`
and states the question — and the plan's false claim about tracking is
a plain correction. The design is sound; what is wrong is the verdict
and the ownerless deferral, both fixable in one revision.
