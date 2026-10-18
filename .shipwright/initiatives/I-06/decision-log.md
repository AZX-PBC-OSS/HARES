# Decision Log

## fix-phase-1 — plan correction: three equipment entrances, not two

The fix plan (revision 6, work item 3) and its critique's absence sweep
claimed exactly two equipment-push paths into a dwelling. `Dwelling::
replace_equipment` is a third (Python-reachable via `py_dwelling.rs`).
What was wrong: the "entrance-uniform invariant" as specified would have
left `replace_equipment` inserting equipment with unassigned or colliding
ids. What it says now: the fix-plan correction section records three
entrances; the implemented code enforces identity validation,
auto-assignment, the landed-write postcondition, and registration guarding
at all three. Documents carrying the old claim: `fix-plan.md` (work item 3,
critique attack 13) — corrected in place by the appended corrections
section.

## fix-phase-2 — operator instruction: no deferrals; WI-5 tracking
obligations discharged by fixing

The operator ordered the two tracking initiatives the plan's WI-5 would
have created (checkpoint positional restore; port slots never rebuilt for
post-construction equipment) deleted, and the defects fixed in the same
change. Both are fixed in-tree: checkpoint restore is identity-keyed
(`DwellingCheckpoint` format v8 — `equipment_states` entries carry name and
id, restore validates both, reorder is a typed error); port slots are
rebuilt monotonically before the first step and post-step adds with
unsatisfiable or unknown-zone declarations are rejected at the entrance.
The same-class audit additionally found and fixed the positional output
column maps (mid-run removal silently misattributed output values). All
three fixes carry regression tests proven to fail with the fix neutered.
The withdrawn initiative stubs could not be deleted by this session (the
permission layer denies all file-deletion commands); the operator must
remove `.shipwright/initiatives/I-07` and `I-08` manually. — Update
2026-09-23: the operator removed both directories; the discharge-by-fixing
stand. The superseded `crates/hares-core/tests/i06_ev_nightly_diag.rs`
removal is still pending the same operator action (recorded in the ticket's
Known Limitations). — Update (fix-phase-8): that claim was stale — the
file is absent from the tree (removed after this entry was written); the
ticket's Known Limitations entry now records the resolution. The python
CI gate remains recorded-but-not-encoded in `flow.yaml`, pending the flow
owner it was recorded for.

## fix-phase-3 — WI-4 precedence change landed as planned

The range-anxiety override now stands down in-window when the composer's
resolved target is at or above the anxiety band (the composer resolves
first; path exclusivity preserved). Six actor-level tests that pinned the
old precedence under in-window strategies were updated to out-of-window
strategies with the trace written into each test; a stand-down test and a
dwelling-level raise-side test were added.

## fix-phase-4 — behavioral-breadth fixture constraint

The second-fixture Nightly test uses bldg0000006 (warm climate), not
bldg0000002: the EV model's cold-pack charge derate legitimately blocks
charging through bldg0000002's January (mean −3.4 °C) — the pack pins at
its floor with zero charging, which is the thermal model working, not an
identity failure. Verified by running the scenario on bldg0000002 before
switching; the constraint is documented in the test.

## fix-phase-5 — second audit pass: actor bindings and mid-run core entries

Attacking the replace entrance and actor wiring found two more members of
the identity class, both fixed: (a) actor equipment bindings never
re-resolved on identity change — user-added actors never resolved at all
(only the auto-seed loop did), and an actor surviving `replace_equipment`
kept the evictee's id, decaying its observation channel to the 0.0 sentinel;
fixed with a trait-level `Actor::resolve_equipment_id` (default no-op)
called for every actor on every `refresh_equipment_caches`; (b) equipment
entering mid-run had no `equipment_core` entry until its first step
completed — actors read nothing for it on that step; fixed by seeding the
refresh with each entering equipment's current `core_output()`. The rebind
fix is pinned end to end and proven by neutering the resolve loop (test
fails "last soc = 0"). An earlier, weaker max-based assertion in that test
passed under the neuter — the first post-replace step still sees the
evictee's snapshot entry before the end-of-step retain drops it — so the
pin asserts the sustained final-row value, which the stale binding cannot
satisfy.

## fix-phase-6 — coverage-triage defects resolved

The coverage round (run concurrently with fix-phase-5's landing, against a
build that predated the identity-refresh seeding — its quoted panic line
numbers match the pre-seeding source) surfaced two CI blockers, both now
resolved on the current tree: (1) `resolve_col` demanded output columns for
equipment the frozen schema never allocated, panicking debug builds on a
mid-run `add_ev` with recording active — fixed by scoping the `expected`
predicate to schema-known equipment (empty maps, missing values, never
misattributed); pinned in Rust (neuter-verified) and by the previously-red
Python test; (2) the tolerated-PV-failure observability panic — mechanism
closed by fix-phase-5's seeding (verified by a new neuter-verified
behavioral pin, `pv_override_observation.rs`, and the previously-red Python
test green); the start-of-step invariant message corrected to name the
actual contract. Process finding adopted: the fix gates now include CI's
python flow (`uv run maturin develop` + `pytest -m "not slow and not
ochre"` — 1172 passed), which is how both defects were caught; recorded
for the flow owner as a standing gate for work beneath `hares-python`.

## fix-phase-7 — red-team findings fixed at the cause

Three red-team attacks, each left red on purpose, each fixed where the
cause lives: (1) the io pass's id classifier conflated malformed with
absent and silently overwrote a malformed explicit id — now tri-state,
malformed left untouched for loud assembly rejection, `null` (unset typed
`Option<u32>`) classified absent (the first cut missed this and was caught
by the dwelling suite before landing); (2) `in_schema`'s name-prefix match
false-positived on prefix-colliding names, re-opening the mid-run-add
panic — now exact membership on the schema's unconditional electric-power
column; (3) the id counter's saturating advance pinned it on an in-use
explicit `u32::MAX` and auto-assignment silently re-issued it — now a
checked advance plus an in-use skip in auto-assignment, with exhaustion a
loud typed error. All three red tests green; the prior rounds' pins
(counter sequence, never-reissue after removal, drift-check-bites,
midrun-add) all still green.

dl-05dc33fe-9bcf-4e8a-a1b0-1058a38b9498 (below) is the original operator
flow-recipe selection record.

## dl-05dc33fe-9bcf-4e8a-a1b0-1058a38b9498

```yaml
event_id: dl-05dc33fe-9bcf-4e8a-a1b0-1058a38b9498
timestamp: 2026-09-22T19:47:47.448Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 857ab6a3ed3c0e283c8ef77f2f596cbde15393369c2a69347be7f0fb8b88b778
reason: workspace_override
```



## Done-challenge produced changes — 2026-09-23T01:30:52.278Z

- **Phase:** round-out-coverage
- **Run:** I-06-round-out-coverage-26bbb540-c707-4884-a2fc-6bbc544a9e2c

## Done-challenge produced changes — 2026-09-23T02:25:11.193Z

- **Phase:** fix
- **Run:** I-06-fix-85a4e114-ffe7-4cdf-bd78-d15b53927d12

## fix-phase-9 — red-team round 4: replace-entrance name contract, WI-4 cap dimension, telemetry guard on syn

Red-team round 4 (three findings) fixed at the cause; all three red
tests green, full suite green (6940 passed, 12 skipped), clippy/fmt
clean, python gate re-run green after `maturin develop`.

1. **`replace_equipment` name collision** — the identity contract's
   blind spot at the replace entrance (ids fully validated, names not).
   One check added, mirroring `add_equipment`'s duplicate-name
   rejection with the evictee exempt; no consumer changes. Entrances
   audited again for the class: `add_equipment` and the assembly loop
   already reject duplicate names; `clear_equipment`/`remove_*` only
   remove — the replace entrance was the last hole.
2. **WI-4 stand-down `max_soc` blind spot** — the composer now records
   the folded cap (`last_resolved_max_soc`) beside the resolved target
   and rate, and the stand-down requires the effective ceiling
   (cap, defaulting to the target) to cover the band. No plan change;
   this is the third dimension of the same precedence predicate
   (target, rate, cap).
3. **Telemetry guard rewritten on `syn`** (per the red team's fix
   direction — replace the scanner, don't patch the apostrophe arm).
   Decision recorded here: **five pre-existing production telemetry
   reads surfaced by the accurate detector were placed under explicit
   `// allowed:` contracts rather than migrated to `CoreOutput`** — the
   two EV-driver reads flagged by the guard's first accurate run, and
   the three `should_reject_dispatch` reads identified by the same
   detector rule set in the pre-run audit and proven load-bearing
   (contracts stripped → the guard flags `dr_compliance.rs:454`,
   `:457`, `:463`; contracts restored → green).
   Grounds, verified per site before contracting: `CoreState` carries
   no defrost, tank-temperature, or capacity field to migrate to;
   away charge power is telemetry-only by design (the equipment zeroes
   its residential `active_power_kw` while `AwayPluggedIn`, so
   `core_output.flows.electric_kw` cannot carry it without corrupting
   the dwelling's electrical balance); and `equipment_telemetry` is
   the only name-keyed channel (dispatch targets are names;
   `equipment_core` is id-keyed). This is the guard's designed
   per-site contract mechanism — the same shape the dwelling's own
   invariant-checker reads (capacity keys, tank-node keys, setpoint
   keys) already operate under — not a suppression: each comment
   names its specific constraint, and the guard stays enforceable
   (a future uncontracted read fails CI loudly, as these five would
   have under a working scanner). The alternative — adding
   defrost/tank-temperature/capacity fields to `CoreState` plus a
   name-keyed core-output channel — is a cross-crate type and plumbing
   change (hares-types `EnvironmentState`, dwelling snapshot
   population) with behavior consequences beyond this initiative's
   scope; if the team wants telemetry reads gone rather than
   contracted, that is the shape of the work.

## Done-challenge produced changes — 2026-09-23T19:47:01.109Z

- **Phase:** round-out-coverage
- **Run:** I-06-round-out-coverage-445f5f24-64d3-4d5b-b01b-50621c7cbf4e

## Done-challenge produced changes — 2026-09-23T20:35:16.401Z

- **Phase:** fix
- **Run:** I-06-fix-89d70f65-7314-431d-bf78-dcfd721ffa90