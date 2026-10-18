---
critique_outcome: clean
---

# Critique: I-06 fix plan, revision 6

**Verdict: clean.** Every finding from the previous round is closed with verifiable text,
and every fresh attack I ran this round came back empty. This document exists to make
that verdict checkable rather than to assert it: each attack is named below with what it
found instead, including one where I nearly filed a false finding and caught my own error.

A clean verdict from this seat needs a higher bar than a finding does, because five
consecutive rounds produced findings and the cheapest thing I could do is produce a sixth
set. So the account below is deliberately specific — a reader who disagrees can re-run
any line of it.

---

## Prior findings — closed, with the text that closes them

**Round-5 W1 — "covering all ~51 hand-written bodies" overstated ~5×.** Coverage item 12
is rewritten. The population mechanism is no longer a hand-maintained harness:

> "the **setter contract across the equipment population** — an exhaustive iteration over
> `CANONICAL_EQUIPMENT_NAMES` (`registry.rs:17`, 63 entries) in the precedent pattern of
> `crates/hares-equipment/tests/registry_coverage.rs` … Membership derives from the
> declared population, so every future equipment type is covered automatically — a
> hand-maintained harness cannot promise that, and the existing one has already drifted
> (`lifecycle.rs` covers 9 of the 63 canonical types)."

and what is *not* covered is now stated exactly: "test-stub implementors are
compile-checked for a body and postcondition-checked at use in any test that adds them —
not iterated, because they are not registered." The stale header is corrected as part of
touching the file, and the non-existent `test_equipment_lifecycle` name is called out
explicitly against the real `assert_equipment_lifecycle` (`lifecycle.rs:59`).

**Round-5 W2 — postcondition specified two non-equivalent ways.** Now unambiguous, and it
engages the exact chain I raised:

> "`add_equipment` checks **`descriptor().id == the assigned value`** — the equality, not
> a weaker 'non-zero and unique' gloss: assigned ids start at 1, so equality subsumes
> non-zero, and the counter never issues an in-use id, so equality subsumes uniqueness. A
> body that wrote nothing …, wrote the wrong field, or wrote a *wrong-but-plausible* value
> … all fail this check — the last case matters because an id that enters without going
> through the counter is one the counter cannot have advanced past."

The subsumption argument is sound on its own terms: ids start at 1, and the counter is
monotonic and advanced past every entering id, so equality is the strictest of the three
and implies the other two.

**Round-5 W3 — the gate contradicted the shift protocol.** Verification now carries the
qualification in the section that defines done, and names both failure modes it removes:

> "**The suite gate is green under the Expected-shifts protocol, and only under it** …
> An implementer holding an unqualified 'must stay green' against a legitimately moved
> fixture has two ways out — thrash on a shift the plan predicted, or quietly absorb it —
> and this qualification removes both."

**Round-5 W4 — the new guard test was presented as equivalent to its three precedents.**
Now characterised honestly rather than defended:

> "this guard is a heuristic, not a decision like its three [siblings which scan] *method
> definitions*, a syntactically unambiguous target `syn` resolves … a name-matched scan
> can [false-positive] … false-negative on UFCS or macro-generated calls. It is worth
> keeping because its failure mode is a loud CI error."

**Round-5 W5 — the declared population and its iteration precedent were unmentioned.**
Adopted as the primary mechanism (see W1 above). Worth recording against my own
calibration: I had told the operator W5 was loop-spinning. It produced the better
mechanism.

**Round-5 D1 — the postcondition reported a broken setter as "unassigned id".** Entrance 2
now raises its own typed error "pointing at the equipment's `Equipment` implementation of
the setter — not entrance 1's 'unassigned id' message, which would misdirect the
implementer to the injection path when the defect is the body they just wrote."

**Round-5 D2 — Alternative 6 priced at a stale cost.** Re-priced at the current inventory,
itemised (postcondition, debug assert, static guard, population test, lifecycle entry,
coverage arms), and the decision explicitly re-run: "**The decision is explicitly re-run
at this price and stands**: the class argument … is independent of the price and has not
changed across five revisions; the price has only been counted honestly."

---

## Why I found nothing — the attacks I ran this round

The new material is the population-coverage mechanism, so that is where I attacked
hardest. Each item is the attack, then what it found.

1. **"Is the exhaustive iteration actually implementable?"** This was my own stated caveat
   when I raised the mechanism — constructing one instance per canonical name may need
   per-type config the registry cannot supply. **It is implementable, and the plan's
   reason is correct.** `EquipmentRegistry::create` returns `Ok(factory(config))`
   (`registry.rs:176`) — the factory itself is infallible; invalid configs defer their
   errors to `init`. The identity-setter contract does not need `init`, so a minimal
   config suffices. The plan's parenthetical ("the factories are infallible by contract —
   invalid configs defer errors to init, which this contract does not need") is accurate.

2. **"Is 63 the real count?"** **Yes, exactly 63.** Extracted the const body
   (`registry.rs:17-94`) and counted. My own round-5 figure of "92 entries as written" was
   the over-capture; the plan's number is right and mine was wrong.

3. **"Does the cited precedent exist and do what the plan says?"** **Yes.**
   `registry_coverage.rs:5-16` iterates `CANONICAL_EQUIPMENT_NAMES` and asserts
   registration — an exhaustive-iteration-over-declared-population test, exactly the
   pattern claimed.

4. **"Do the six macro-delegated wrappers fall inside the population, or are they a hole?"**
   This is the attack that nearly produced a false finding. My first three greps for
   `GasGenerator`, `ASHPHeater` etc. returned **zero** matches in `registry.rs`, which
   looked like the wrappers sitting outside the canonical list — the exact types whose
   setter-delegation and guard-forwarding the plan most depends on. Dumping the const
   verbatim resolved it: **canonical names contain spaces** — `"Gas Generator"`,
   `"Gas Fuel Cell"`, `"ASHP Heater"`, `"MSHP Heater"`, `"GSHP Heater"`, `"WSHP Heater"`.
   All six are in the population and are covered by the iteration. The defect was in my
   grep, not the plan.

5. **"Do the plan's own cross-references resolve?"** **Yes.** Matrix rows 1–48 are all
   present; every prose reference (rows 32, 33, 35, 41, 43, 45, 47) lands on an existing
   row.

6. **"Does the postcondition's equality argument actually hold?"** **Yes** — ids start at
   1 and the counter never issues an in-use id, so equality is strictly the strongest of
   the three candidate checks. No case passes equality while failing non-zero or
   uniqueness.

7. **"Is `plan_outcome: ready` the right verdict?"** **Yes.** My brief says a plan that
   finds the RCA's root cause wrong or unscopeable should be `blocked`. This plan targets
   the RCA's confirmed root cause (identity collapse at assembly), does not dispute it,
   and both reproduction gates are named as the change's gates. `ready` is correct.

8. **"Did work items 4 and 5 change without my noticing?"** **No.** Identical extents to
   revision 5 (WI-4: 46 lines, WI-5: 29 lines) and their distinctive content is intact
   ("Path exclusivity, as today"; "two tracking initiatives — one per defect"). I read
   both line-by-line in round 4 and carry that forward rather than re-reading.

9. **"Does the scope question deserve re-opening?"** **No, and I am recording that as a
   decision rather than silence.** The plan weighs assembly-only scope in Alternative 6,
   has now re-priced it twice at corrected figures, and re-runs the decision explicitly.
   The class argument (multi-add DER at the Python entrance; the incoherent
   entrance-conditional invariant; `PyEquipmentDescriptor.id` staying `0`) is sound and
   the constitution's "fix the class" rule supports it. I pushed on this in rounds 3 and
   5; it has been answered twice with an explicit price. Pushing a third time would be
   repetition, not review.

10. **"Does the change violate the constitution's file-size rule?"** Considered and
    dropped, recorded rather than left invisible. Work item 3 adds the counter,
    entrance-2 assignment, the postcondition and validation to
    `crates/hares-core/src/dwelling/mod.rs`, a file already far past the "~2000 lines is
    a signal" threshold. But the rule's own text is "Do not be dogmatic — a 2200-line
    file with a single coherent concern is fine … Judge by cohesion, not line count," and
    the additions are cohesive with the `add_equipment` and assembly code they sit
    beside. Raising it would be applying the rule in exactly the way the rule tells you
    not to.

11. **"Do the new ledger rows make claims the code contradicts?"** This is where an
    earlier round caught a false claim (revision 2's ledger cited a test helper as a
    production site), so the new rows get read, not counted. **Rows 43–48 read in full;
    each describes its round-5 finding accurately and each resolution matches text I
    verified independently in the plan body.** Row 46 also settles something I had left
    open as possibly under-specified — the authorization boundary for out-of-tree callers
    of the setter: "out-of-tree pre-registration use is harmless — the postcondition
    governs whatever id is present." That is correct: a pre-registration call just yields
    an explicit id, which entrance 2 collision-checks.

12. **"Is there a `Dwelling` construction path the counter spec misses?"** The counter is
    "initialized to 1 at construction (so dwellings built without the assembly path are
    covered)" — I went looking for such a path. **There is exactly one public constructor,
    `from_config` (`dwelling/mod.rs:1669`); no `pub fn new`, no `Default`.** So every
    dwelling comes through assembly and the clause is defensive rather than load-bearing.
    Harmless, and it correctly covers the one reachable case it does serve — an assembly
    that produced zero equipment, where the counter stays 1 and the first add takes 1.

### The absence sweep — what the plan passes over, and whether it matters

Attacks 1–12 test what is on the page. This is the other half: going to the code for what
the page never mentions.

13. **Equipment ids are dwelling-local, and one consumer is process-wide.** The plan never
    says which scope the uniqueness guarantee has. It is per-dwelling: the counter lives
    on the `Dwelling`, so dwelling A and dwelling B both issue id 1. One consumer spans
    dwellings — `RangeViolationEvent.equipment_id` is written to a process-wide
    `static RANGE_EVENTS: std::sync::Mutex<RangeEventRing>` (`hares-types/src/equipment.rs:1309`),
    so in a fleet run two dwellings' violations are indistinguishable by id. **Not a
    defect and not a regression**: the ring is `#[cfg(feature = "observe")]` diagnostics,
    not simulation behaviour, and today *every* equipment in *every* dwelling is id 0, so
    the fix makes that ring strictly more informative rather than less. The plan's
    treatment of it as untouched is right. Recording it because "the scope of the
    uniqueness guarantee" is a real unstated property, and the honest result of chasing it
    is that nothing turns on it.

Other silences I checked and found benign: no second equipment-push path on `Dwelling`
beyond the two entrances (`clear_equipment`, `remove_equipment`,
`remove_equipment_by_end_use` only remove); no `hares-fleet` assembly path (verified in an
earlier round — `SteppableFleet::from_configs` delegates to `Dwelling::from_config`).

---

## What I did not check

Stated so the verdict's limits are visible:

- **I did not read all 954 lines this round.** Revision 6 adds ~106 lines over revision 5,
  and I accounted for all of them by section: work item 3 (+26), coverage (+18),
  Verification (+13), Alternatives (+9), blast radius (+5), matrix and ledger (+35). Each
  of those I read. Work items 1, 2, 4 and 5 I read line-by-line in earlier rounds and
  verified unchanged this round by extent (identical line counts) and by distinctive-string
  spot-check.
- **I did not run the suite.** No critique round has.
- **Carried forward without re-verification:** the read-channel table beyond the sites
  checked in rounds 1–3; the bodies of `evaluate_charging` and
  `maybe_push_range_anxiety_override` (work item 4's size claim still rests on the plan's
  own description of that control flow, which is self-consistent); that work item 4's
  raise side is reachable through real assembly.
- **Verified in earlier rounds and not re-checked here** because the plan's text on them
  is unchanged: the exposure boundary (`equipment` is a private field, `equipment()`
  returns an immutable slice, no `&mut dyn Equipment` escapes to `hares-fleet` or
  `hares-python`); the guard facts; the impl arithmetic (57 sites, 6 macro-delegated);
  the no-cached-id property; checkpoint byte-identity.

---

## On the verdict

`clean` here is not the absence of a search. Thirteen attacks are named above with their
results — ten against what the plan says, three against what it does not say — one of
them a near-miss I had to correct against myself.

**Split, as the role asks.** Findings that would change what gets built: none. Findings
that would change how the document reads: none. Both lists are empty, which is the whole
claim; the twelve prior-round findings that populated those lists are closed above with
the text that closes them.

It is also not a claim that the plan is beyond criticism. It is the narrower claim my role
actually supports: **I could not find a defect this round that would change what gets
built or mislead the fix step.** The three findings from round 5 that could have done
either — a coverage claim contradicted by counting, a postcondition that specified two
different checks, and a gate that contradicted its own protocol — are closed with text I
can quote and have quoted.

Two things a reader should weigh alongside this verdict. First, revision 6 is the first
where the plan's answers were not merely responsive but *corrective of me*: the 63-entry
count is right and my 92 was wrong, and the mechanism I had dismissed as loop-spinning
turned out to be the one worth adopting. Second, the signal for a gate is convergence,
not a finding count — five rounds of findings followed by a round where every prior
finding closes and every fresh attack comes back empty is what convergence looks like. If
the next reader wants a sixth round, the useful target is the code the fix step writes,
not this document.
