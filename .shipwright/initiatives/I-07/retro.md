---
retro_outcome: sound
---

# Retrospective: I-07 — EV pack thermal sub-model divergence (gh #32)

## The bug, restated

The reporter saw a Nightly-strategy EV charge on 0 of 5 cold nights —
plugged in, dispatch active, exactly 0.00 kW — and hypothesized a
wiring/identity residual in the raw-HPXML injection path. The actual
cause was physics, not wiring: the EV equipment's pack thermal sub-model
was a from-scratch reimplementation of behavior the stationary Battery
equipment already modeled correctly, and it had diverged in three coupled
ways. The pack's thermal mass default was ~24× too small (20 kJ/K for a
75 kWh pack, uncited), so the pack relaxed to nighttime ambient in ~1.4 h
and the (physically correct) 0 °C charge cutoff blacked out every
sub-freezing night. Charger conversion losses ((1−η)·P ≈ 1.15 kW at L2)
were injected into the pack as heat instead of I²R cell heating, driving
the pack to 165–281 °C on routine sessions and throwing the Arrhenius
degradation model out of its domain (negative fade, capacity whipsaw).
And heater heat was gated on charge power being nonzero, so
preconditioning — the one mechanism that could rescue a cold night — was
dead exactly when needed while still billing energy. Cold weather turned
all three into the ticket's signature.

## Scope of this pass (overreach check)

This retrospective changed exactly two files: `.shipwright/constitution.md`
(the two principle additions quoted below) and this document — plus one
tracking ticket (`tickets/I-07-T5-...`) the constitution's
no-untracked-deferrals rule requires for the mechanism proposed here.
No production code, no tests, no refactors, nothing drive-by. The
mechanisms described in the next section were installed by the fix loop,
not by me; I verified each one on the tree rather than taking the prior
artifacts' word.

## Was the defense added?

Yes — the fix installed mechanisms, not just corrections. For each, what
it makes impossible and which check fails if the shape reappears:

- **A shared home for the electrical half.**
  `crates/hares-equipment/src/pack_electrical.rs` now owns the
  terminal-voltage solve and I²R heating, called by both families
  (verified: `battery/mod.rs:551`, `ev/mod.rs:1430-1431`); its module
  doc names the loss-attribution rule explicitly. What it makes
  impossible: attributing conversion loss to the pack *through the
  shared path* — a future `(1−η)·P`-into-pack change has to bypass the
  shared module and delete a documented invariant, not just add a line.
  What it does not make impossible: a hand-rolled heating term outside
  the module — that is caught one layer down, by the tests and the
  domain guard below.
- **A runtime domain guard.** The degradation model
  (`battery/degradation.rs`, shared by the EV — verified at
  `ev/mod.rs:177`, `:1425`) now errors loudly outside a [−100, +130] °C
  physical envelope. The check that fails: the simulation itself, with a
  typed error — the 281 °C class can no longer silently produce negative
  fade; it stops the run. Verified present with its doc comment naming
  this bug as the motivation.
- **Permanent behavior-named regression tests.**
  `crates/hares-core/tests/ev_thermal_regressions.rs` (8 tests —
  counted on the tree) pin each defect by its physical invariant:
  `l2_charge_session_keeps_pack_temperature_physical` fails if
  loss-attribution or thermal mass regresses;
  `parked_pack_stays_above_charge_cutoff_after_a_cold_night` fails if
  the pack's thermal inertia drops back to hour-scale;
  `battery_heater_warms_pack_while_cold_derate_blocks_charging` fails if
  heater heat is ever re-gated on charge power. These are the checks
  that would have caught D1/D2/D3 the day they shipped.
- **A class sweep that ran in both directions.** The fix's own audit
  found the same double-application shape (LUT temperature axis ×
  derate) in the *Battery*, not just the EV — the class was fixed, not
  the instance.

## What was still missing — and where it now lives

Two defenses are judgement calls no mechanism can enforce, and neither
was recorded anywhere the next person would hit it. I added both to the
constitution myself in this pass:

1. **"One model per physical phenomenon — a reimplementation is
   duplication"** (new section under Code quality, after DRY). The
   existing DRY principle covers constants and formulas — duplication
   you can grep for. This bug was a parallel *implementation*, which
   shares no identifiers with the original and is invisible to that
   rule as written. The new section names the I-07 instance and the
   class-audit direction (align every model of the phenomenon on one
   implementation, in both directions).
2. **"Assert physical plausibility, not just functional behaviour"**
   (Testing → What to test). All three defects shipped behind green
   suites because no test asked whether the numbers were physical; a
   wrong constant passes every test that only checks a code path ran.
   The principle: every simulated physical state has absolute bounds,
   asserting them is nearly free, and validated domains get loud
   boundary guards (the degradation guard as the pattern).

## Where the broken shape still survives — searched, not assumed

Searched: `grep` for `(1.0 - …efficiency` patterns across
`crates/hares-equipment/` (only power-derate uses in PV/ventilation —
no other conversion-loss-as-heat attribution), for the thermal-update
equation shape (`ua_w_per_k *`, `heater_active =`) across equipment, and
read of the `CoreOutput`/`CoreState` validation layer in
`hares-types/src/equipment.rs`. Three survivors, named:

1. **The pack *thermal* models remain parallel implementations.**
   `battery/mod.rs:1346-1367` (heater gate + `q_loss`/thermal-mass
   update) and `ev/mod.rs:1644-1709` (`apply_soc_and_thermal`, same
   equation shape) are two bodies of one phenomenon. This is a
   *deliberate* fix-plan decision ("behavioral alignment, not a
   structural merge"), and the new constitution section now governs it:
   if a third pack-thermal consumer appears, extract a shared module
   rather than write a third body. Until then the alignment is
   maintained by review, not by structure — the accepted residual of a
   reasoned decision, recorded so it is a choice rather than an
   accident.
2. **No plausibility bound on pack temperature at any validation choke
   point.** The range layer exists — `validate_core_contract`
   (`equipment.rs`) already rejects/warns on cited physical bounds for
   COP, main power, cooling sign conventions, setpoints, and thermal
   output, with a `RangeViolationEvent` observability ring — but
   `CoreState` (`equipment.rs:1189-1198`) carries no temperature field,
   so the layer cannot bound the quantity this bug class corrupts. The
   degradation guard covers only equipment using that shared model, and
   only at the accumulation point. **This is the one real defense gap,
   and it is now tracked:** `tickets/I-07-T5-pack-temperature-plausibility-guard.md`
   proposes the mechanism (carry pack/cell temperature through
   `CoreState` and bound it in the existing range layer) with its
   boundaries and a weaker alternative. Not implemented in this pass
   because adding a `CoreState` field is a cross-cutting API decision
   (every equipment's CoreOutput construction, Python bindings,
   telemetry columns) — installing it unilaterally from a retrospective
   would be overreach, and the constitution requires a ticket for
   exactly this kind of deferral.
3. **The citation gate catches wrong citations, not missing ones.**
   `scripts/check-ashrae-chapters.sh` scans `crates`/`docs`/`tests` but
   flags citations whose chapter is *wrong for its topic*; a default
   with no citation at all — the D1 shape (the uncited 20 kJ/K) —
   passes it. The rule against uncited defaults already lives in the
   constitution ("If you cannot name the source of a default, it must
   not exist in the code"), so this is recorded where the next person
   reads; enforcement stays with review. A missing-citation lint is
   conceivable but heuristic-noisy (not every numeric literal is a
   sourced default); I did not build it, and that is a judgment call
   rather than a tracked gap.

## What is a mechanism and what is a note — stated honestly

The two constitution additions I made are *notes every agent reads*, not
mechanisms — they make nothing impossible. They are the right level for
the two judgement calls they carry ("don't reimplement a sub-model" and
"assert plausibility in tests"), because no static check can recognize a
parallel reimplementation or decide which states deserve bounds. The
constitution is this team's designated channel for such rules — its own
header says every agent reads it before touching code, and its existing
principles (I-01 ordering, I-05 silence, I-06 identity) are the
precedent this pass follows. Where a mechanism *could* carry the weight
— the pack-temperature bound — I did not settle for the note: it is
ticketed as I-07-T5 with a concrete design.

## Diagnosis divergence — informative

The initial hypothesis (reporter's, and plausible given team context)
was an identity/binding residual — the I-06 class. The actual cause was
weather plus thermal physics. Two things are worth carrying forward:

- **Recency bias is real in diagnosis.** The previous two initiatives
  were wiring bugs (I-05 dispatch silence, I-06 identity cross-wiring),
  and the hypothesis reached for that class. The RCA's discipline of
  refuting the hypothesis *by execution* (the literal recipe with the
  fixture's own weather is healthy: 39/45 charge-days) before touching
  code is what prevented a wrong fix.
- **The reporter's own telemetry was the decisive evidence.** The
  `needed_charge_hours` series is internally consistent only at ambient
  ≈ −6.5 °C — the run's climate was recoverable from the ticket's
  numbers, which pinned weather as the trigger before any weather file
  was inspected. "Interrogate the ticket's telemetry for internal
  consistency" is a reusable diagnostic move.

A second, smaller divergence surfaced post-fix (RCA addendum): the
capacity-whipsaw mechanism was attributed to out-of-domain Arrhenius
evaluation alone, but the degradation constants themselves had diverged
from the NREL SSC reference implementation. The constitution's
"verify every citation against the primary source" rule already covers
this; the instance is a reminder that inherited-from-OCHRE constants are
cross-checks, not specifications.

## Verdict

`sound` — chosen deliberately over `defense_gap`. Every defect of *this*
class now has a check that fails (three test invariants, the degradation
guard's hard error, the shared module's documented invariant). The one
generalization that is missing — a pack-temperature bound at the
validation choke point, which would extend the net to future equipment
families — is not absent, it is *tracked* with a concrete design in
I-07-T5, which is the constitution's designed disposition for a
cross-cutting API change that a retrospective must not unilaterally
install. If the coordinator reads `defense_gap` as "route more defense
work now," that work is already specified in the ticket; nothing is
left untracked.

The fix aligned the EV with the Battery's architecture rather
than patching constants; the class was swept in both directions; the
regression tests, shared mechanism, and domain guard are all verified
present on the tree; the review loop closed the adjacent boundary class
it found (non-finite floats at the Python boundary) with the same
one-home discipline. The two residual gaps were unrecorded principles,
and they are now recorded in `.shipwright/constitution.md` — the next
agent who reaches for a fresh sub-model implementation, or writes a
functional test for a physical quantity, hits the rule before writing
the code.
