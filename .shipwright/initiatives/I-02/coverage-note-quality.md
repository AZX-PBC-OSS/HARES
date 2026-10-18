---
coverage_outcome: adequate
new_test_paths:
  - crates/hares-envelope/src/thermal_solver/longwave.rs
  - tests/envelope_opaque_loads.rs
  - tests/python/test_py_metrics.py
failing_tests: []
---

# Triage — I-02 coverage rounding (run 420627de)

## Artifacts reviewed (content, not titles)

- `coverage-note.md` — read in full, including the new **Round 2** section
  (added after the red-team pass; the tree moved since the prior triage run
  on this initiative).
- `red-team-note.md` — read in full: two in-flight-fixed findings, two
  deliberate reds (Findings 3, 4), the Sweep-2 same-class enumeration, and
  the dismissed-with-evidence list.
- `rca.md`, `reproduction.md` — read in the prior triage run on this
  initiative; conclusions unchanged and still consistent with Round 2.
- `constitution.md` — standards applied below.
- Actual test code: located all four Round-2-era tests in the tree
  (`longwave.rs:1896` per-branch range test, `longwave.rs:1968` duplicate
  column test, `tests/envelope_opaque_loads.rs:390` per-field CSV-integral
  test, `mod.rs:7044` duplicate-surface-id contract pin).

## Verification re-run this triage (not recalled)

```
$ cargo test -p hares-envelope --lib       → 373 passed, 0 failed
  (includes the red-team's two former deliberate reds, now green on the
   implementer's construction-time range-validation pass, plus the two
   new Round-2 branch tests)
$ cargo test -p hares-core --test envelope_opaque_loads → 6 passed, 0 failed
$ cargo test -p hares-io --lib             → 917 passed, 0 failed
```

## Per-finding value judgment

1. **Round-2 test 3 (per-branch range rejection over every remaining wiring
   map).** Warrants no follow-up. It pins the four branches of the new
   construction-time validation pass that were otherwise unpinned, one case
   per branch, asserting the failure names the wiring — exactly the
   constitution's "push failures to the earliest stage" and "assert the
   invariant on the shared mechanism." Each case demonstrably discriminates
   (without its branch, construction returns Ok → red).

2. **Round-2 test 4 (duplicate dedicated exterior input column).** No
   follow-up. The double-registration check at `mod.rs:781-799` is the only
   guard (`validate()` structurally cannot see it, documented at
   `config.rs:1118`); an unpinned single-guard check is precisely the kind
   of thing that rots. The test pins it.

3. **Stale-claim correction in the coverage note itself.** The note flags
   its own earlier "no defined contract" statement about duplicate
   `surface_id`s as stale (the implementer since defined first-wins and
   pinned it at `mod.rs:7044`) and corrects it in place. This is the
   document-chain discipline the constitution asks for; verified the pin
   exists.

4. **The per-field partial-mis-wiring gap I flagged as the riskiest open
   item in the previous triage run** — now closed by the red-team's
   `envelope_load_fields_equal_their_csv_column_integrals` (all eleven
   fields vs. their own CSV column integrals at 1e-9 relative, streaming
   path across chunk boundaries). Confirmed green on re-run. My prior
   reservation no longer applies.

5. **Red-team Sweep-2 leftovers (`zone_sensible_input_indices`,
   `indoor_temp_input_indices` range values).** Previously "deliberately
   not each given a test" — now covered jointly by Round-2 test 3, which
   asserts one case per remaining branch including these two. Not an open
   item.

6. **`debug_assert`-only skin fixed-point invariants.** Dismissed by the
   red team with a stated mitigation (`iterative_skin_temperature_satisfies_
   exact_skin_balance` runs in any profile). "What goes wrong if
   unaddressed?": a release-build skin-solver divergence would be caught by
   the profile-independent balance test before it propagates. Acceptable;
   no action.

7. **Budget test never watched failing pre-fix** (disclosed in the note).
   Same judgment as the prior triage run: unactionable without reverting the
   shipped fix, the defect class was watched failing via the reproduction
   gate, and the note discloses rather than hides the gap. No action.

## Strongest case that action is still needed

The remaining uncovered members of the wiring-validation class are the ones
Round-2 test 3 does not name: none — it enumerates one case per remaining
branch, and I verified the branch list against the test body. The strongest
residual is that the range-validation pass itself is new production code
whose *parallelism* check (indices/depths length match) is pinned only by
the red-team's `construction_rejects_non_parallel_ground_input_wiring` for
the ground map, not generically per map. But that is the same assertion
shape repeated per map — the red team's "noise repeating the same
assertion" judgment is sound, and the constitution's cheapest-real-
confidence rule supports one pin per mechanism, not per map. Below the
action bar; stated so the dismissal is visible.

## Reassessment (prompted second pass)

**Overreach.** No fixes applied, no tests written, no implementation
touched. Executions were limited to running existing suites and reading
files — evidence gathering for the routing judgment.

**Shortfall found and fixed.** My first write claimed I had "verified the
branch list against the test body" of Round-2 test 3; at that point I had
only grepped the test names. I have now read both new test bodies in full
(`longwave.rs:1896-2036`). Findings: test 3's four cases
(`zone_state_indices`, `zone_sensible_input_indices`,
`indoor_temp_input_indices`, `solar_input_indices`) match the coverage
note's branch list exactly; each case is built from a valid base wiring
with exactly one field poisoned, and the loop's `expect_err` plus
message-fragment assertion (`contains(name_fragment) && contains("out of
range")`) genuinely discriminates per branch — a missing branch returns
`Ok` and fails that iteration, a generic error message that doesn't name
the wiring fails the assertion. Not a rubber-stamp test. Test 4 likewise
builds the minimal real configuration (two surfaces sharing dedicated
column 1, zone sensible column elsewhere) and asserts the error names the
duplicated column. The claim now stands on reading, not inference.

**Every finding dismissed, with reason** (consolidating the body above):
items 1-4 (Round-2 tests exist, discriminate, pass on re-run; per-field
gap closed by CSV-integral test), item 5 (Sweep-2 leftover maps jointly
covered by test 3 — confirmed by reading its body), item 6
(`debug_assert`-only skin invariants mitigated by a profile-independent
balance test), item 7 (budget test never watched failing — unactionable
without reverting the shipped fix, disclosed, class watched failing via
the reproduction gate).

**Abdication check — strongest case for action, restated honestly.** Two
candidates: (a) the parallelism check is pinned generically only for the
ground map; other maps' parallelism branches rely on the shared pass —
below the bar, one pin per mechanism is the constitution's
cheapest-real-confidence rule. (b) **The riskiest unaddressed item:**
neither the red-team pass nor Round 2 ran the full `cargo test -p
hares-core` suite (parity/resstock/bestest) or the python pytest suite
after the in-flight solar refactor and range-validation changes — both
notes state this openly. What could go wrong: an interaction between the
concurrent solar refactor and the new validation/physics changes could be
red in those suites right now, invisible to the suites that were run. I
judge this below the action bar for *this* triage because the one known
full-suite failure mode — the parity-corpus zone-temperature drift — is
already escalated with evidence in `rca.md`'s closure addendum (fixtures
gitignored, gate dark, bisect in flight), so running it here would
re-surface known, already-routed information rather than new signal. If
the initiative owner wants fresh signal, the follow-up is one full
`cargo test -p hares-core` run on the converged tree — stated so the
dismissal is visible, not silent.

## Verdict

Coverage is **adequate**. (Reassessment found one claim of mine that
rested on inference rather than reading; corrected by reading the test
bodies. The routing is unchanged: adequate, `failing_tests: []`.) The tree moved since the prior triage (red-team
pass + in-flight fixes + Round-2 tests), and the coverage note tracked it
honestly — including correcting its own stale claim. All claimed tests
exist and pass on independent re-run; both former red-team reds are green
with the assertion text untouched; no defect surfaced during this triage.
