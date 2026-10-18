---
retro_outcome: sound
---

# Retrospective: I-06 — EV under Nightly strategy stops charging after the first week

## What the bug actually was

The ticket presented as a control-policy failure: an EV with a Nightly charging
strategy charged for a few days, then stopped; trips were cancelled for range;
the range-anxiety backstop appeared never to fire. The natural reading was
"something in the driver or the equipment's hold/dispatch contract is wrong."

The actual cause was one layer below, in identity: no code path in a real
dwelling build assigned unique `EquipmentId`s. Every constructor defaulted its
descriptor id to 0 when the config lacked the key, and nothing populated the
key, so all 12 equipment in the ticket dwelling were `EquipmentId(0)`. Every
`EquipmentId`-keyed map collapsed to a single last-write-wins entry — in this
fixture, MELs, a 90 W plug load. The EV driver's observation channel
(`equipment_core[id].state.soc`) therefore read the wrong equipment, got no
SOC, and never reconciled its internal estimate, which decayed ~0.13 SOC/day
until every trip cancelled against a phantom empty pack. The physical pack was
at its 0.90 target almost every morning; the vehicle was healthy the entire
time. The bug was in observation, not in charging.

## Diagnosis divergence — and what was misleading about this bug

The divergence between presentation and cause is the instructive part:

- **"The backstop never fires" was the opposite of the truth.** The
  range-anxiety override fired *constantly* from day 9 — it was simply moot,
  dispatching a 0.22-band target against a pack already at 0.90. The ticket's
  strongest alternative explanation (override pre-empting the composer
  in-window) was real, observable behavior — but strictly downstream of the
  observation failure, as the RCA proved from the trace's causal ordering.
- **The telemetry lied by construction.** The reported "SOC 1.00 → 0.072"
  column silently fell back to the driver's un-reconciled internal estimate
  when observation returned `None`. The symptom everyone was chasing was an
  artifact of the fallback, not a measurement of the simulated pack. A
  telemetry channel that substitutes an estimate for a missing observation
  converts "the sensor is cross-wired" into "the physical system is broken."
- **Every existing safety net was blind to this exact failure mode.** The
  name-resolution warning only fires when the *name* is absent (the name
  resolved fine — to the shared id 0). The debug SOC-divergence warning only
  runs when `actual_soc` is `Some` — i.e. only when observation works. The
  "equipment_core missing entry" invariant is vacuous when all ids are equal,
  because one entry satisfies every lookup. The nets checked for *anomalous
  values on a working channel*, and the bug was a *dead channel*.

Lesson recorded: monitors that depend on the channel they monitor are not
monitors. Detect the absence of expected input, not just bad values in it.

## What defense would have caught this earlier — and was it added?

The bug had four independent catching points, and the fix installed a defense
at every one. Each was verified in the tree during this retrospective, not
taken from the artifacts' say-so:

1. **The defaulted identity itself.** `assign_equipment_ids`
   (`hares-io/src/hpxml/equipment.rs:405`) now injects unique ids at assembly
   through the config channel constructors already read; the six one-sided
   constructor reads were aligned onto one tri-state reader. Id 0 and duplicate
   ids are rejected at the assembly entrance; `add_equipment` /
   `replace_equipment` auto-assign from a never-reused counter
   (`assign_equipment_identity`, `dwelling/mod.rs:3459`). The sentinel default
   can no longer survive any entrance. The required `set_equipment_id` trait
   method (`hares-equipment/src/lib.rs:215`) makes the compiler enumerate every
   implementor — no default body to silently no-op.
2. **A write that doesn't land.** The entrance postcondition verifies
   `descriptor().id == assigned` before admission; a no-op setter body fails
   registration loudly. Pinned by `equipment_identity_boundaries.rs`.
3. **Future mutation paths.** The syn-based static guard
   (`hares-equipment/tests/identity_guard.rs`) scans for identity-setter call
   sites outside the authorized file — the threat ("some future code mutates an
   id post-registration") is a static property, so it gets a static check that
   fires on existence, not on execution.
4. **The test-level blind spot.** The root enabler was that every unit test
   hand-built `equipment_core` with unique ids, so the suite never exercised
   the real assembly path where the bug lived. The permanent pins build through
   `Dwelling::from_config`: `equipment_core_has_one_entry_per_equipment`
   (population-level invariant, fix-agnostic) and
   `nightly_strategy_keeps_ev_charged_over_long_horizon` (45-day behavioral
   contract). Re-run this session: 37 passed, 0 failed. The review phase added
   the class-level pins: `equipment_identity_misattribution.rs`,
   `equipment_readd_after_removal.rs` (the re-add regression review round 2
   found and fixed in-session), plus the checkpoint-v8 identity-keyed restore
   and the port-slot rebuild the same-class audit surfaced.

The class was fixed, not the instance: the BMS actor (the RCA's second misbound
consumer), `prev_equipment_modes` interest filtering, checkpoint restore, and
the positional output column maps were all repaired under the same identity
mechanism, with zero consumer-side edits — one assignment point made every
consumer correct by construction.

## What this retrospective added

The mechanisms above are enforced by code, so the class is defended for
equipment identity specifically. The *generalizable* principle — sentinel
identity defaults silently collapse keyed populations; entrance-validate,
postcondition-verify, test through the real assembly, and distrust monitors
gated on the channel they monitor — is a judgement call no single mechanism
enforces for the next analogous structure. It was not yet in the team's shared
vocabulary (the constitution referenced I-01 and I-05 precedents but nothing
for this shape). I added it as a constitution section, **"Identity is assigned
at the boundary, never defaulted into use,"** placed beside "Silence is not a
decision," with the I-06 reference, in the established precedent-carrying
style.

## Defense installation audit (reassessment)

**Overreach.** `git status --porcelain` on the final tree: the only tracked
file this phase modified is `.shipwright/constitution.md` (+9 lines, the new
principle section). Everything else in the diff — 52 files across six crates —
is the fix and review phases' work, attested in `rca.md`'s Resolution section
and `review.md`. This phase changed no behavior, refactored nothing, and
carried out no hardening pass of its own.

**Shortfall — where the defense lives and what it makes impossible.** Two
layers, named separately because they defend different things:

- *The concrete class (sentinel-defaulted `EquipmentId` collapsing keyed
  maps)* is defended by mechanisms the fix phase installed and this phase
  verified in the tree: entrance rejection of id 0 / duplicates
  (`assign_equipment_identity`, `crates/hares-core/src/dwelling/mod.rs:3459`;
  assembly validation with `id_first_owner` at `mod.rs:2365`), the required
  `set_equipment_id` trait method (`hares-equipment/src/lib.rs:215`), the
  landed-write postcondition, and the static call-site guard
  (`hares-equipment/tests/identity_guard.rs`). If the same shape reappeared —
  a constructor defaulting an id that an entrance admits —
  `equipment_core_has_one_entry_per_equipment`
  (`crates/hares-core/tests/equipment_id_regressions.rs`) fails, and a no-op
  identity write fails registration via the postcondition arm pinned in
  `equipment_identity_boundaries.rs`. Re-verified green this session.
- *The general principle* lives in `.shipwright/constitution.md`, the section
  I added: "Identity is assigned at the boundary, never defaulted into use."
  It is a note, and I asked whether a mechanism could carry it instead: it
  cannot, in the general form. The shape is defined by intent — "this field is
  an identity used for keying downstream" — which no syntactic scan can
  detect; a guard keyed on `EquipmentId` specifically already exists
  (`identity_guard.rs`), and a guard keyed on "any field that might be an
  identity" has no decidable target. The constitution is where design-time
  judgements live in this repo (same status as "Silence is not a decision").

**Shortfall — where the shape survives next door.** Searched, with commands:
`rg 'HashMap<EquipmentId|BTreeMap<EquipmentId' crates/` (every id-keyed map),
`rg 'ActorId' crates/hares-types crates/hares-core` (actor identity),
`rg 'ZoneId\(' crates/hares-core/src/dwelling crates/hares-io` (zone identity
assignment), `rg '_by_name' crates/hares-core/src` (name-keyed maps), and a
read of `add_actor` and `populate_telemetry`. Findings:

- `equipment_core` (`hares-types/src/environment.rs:464`) is the **only**
  `EquipmentId`-keyed map outside the defended dwelling module — it is the
  defended map itself. No other equipment-id-keyed structure exists.
- Zones: `ZoneId` is index-assigned at assembly
  (`dwelling/mod.rs:2219`, `ZoneId(idx + 1)`) — unique by construction, no
  sentinel default, no entrance that admits a caller-supplied zone id.
- Actors: no numeric actor id exists; actors are name-keyed, and
  `add_actor` (`dwelling/mod.rs:3059`) rejects duplicate names with a typed
  error — the fix phase closed that neighbor.
- Names: `equipment_id_by_name` is the only name→id map; name collisions are
  rejected at all three entrances (pinned by
  `replace_equipment_name_collision.rs`,
  `add_equipment_empty_name_exemption.rs`).
- The telemetry fallback that masked this bug survives in narrowed form:
  `populate_telemetry` (`actors/ev_driver/mod.rs:495`) still emits
  `observed_soc.unwrap_or(self.estimated_soc)` — but post-fix `actual_soc` is
  `Some` whenever the equipment is registered, so the fallback is reachable
  only in the unregistered-equipment mode, where `resolve_equipment_id` has
  already warned and the comment at the site documents the degradation. The
  I-06 shape — the channel lying during normal operation — is closed by
  identity assignment and pinned behaviorally: the 45-day regression test
  asserts on the run's CSV output, the telemetry face itself. Making the
  unregistered-mode channel emit nothing rather than an estimate is a
  behavior change the fix phase considered and documented; I left it as
  recorded rather than expanding scope.

Conclusion: no surviving neighbor with the same shape lacks a defense today.

**Abdication — first diagnosis vs. real cause.** The divergence section above
carries this; sharpened to name what made the wrong diagnosis look right: the
*ticket's own evidence was produced by the bug*. The "SOC drains to 0.072"
column was the driver's un-reconciled estimate wearing the telemetry channel's
name; the "backstop never fires" observation was inverted (it fired every
step, moot against a full pack); and the real override pre-emption — genuine,
observable misbehavior — sat exactly where a code reader would land, while
being strictly downstream. Three independent artifacts of one misbinding, each
pointing at the driver, is what made the behavioral reading look right.

## Assessment

Fix is sound; the class is closed; the defense is layered (entrance validation
+ required trait method + postcondition + static guard + real-assembly
regression tests + constitution principle), each layer matching its threat.
The one defect the review found post-fix (re-adding a removed guard-tracking
equipment tripping the identity guard) was fixed at the cause in review and
pinned. No new bug was uncovered by this retrospective. Outcome: **sound** —
with the vocabulary entry now installed in the constitution where the next
identity-keyed structure will hit it.
