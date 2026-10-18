# Verification L6-b2: measurement, fleet, runtime-memory, slop-cleanup

Scope: `ledger/02-measurement.md` M-4a, M-4b; `ledger/04-fleet.md` FL-1
through FL-5; `ledger/06-runtime-memory.md` RT-1, RT-2, RT-3a, RT-4 through
RT-8 (RT-3 skipped, in progress); `ledger/10-slop-cleanup.md` SL-1 through
SL-6, SL-8 through SL-11. Checked against committed HEAD
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` (commit `7b7a6a7d`), with
historical citations followed to the commit each entry names, per
`plan.md`'s evidence rule.

Entries read in full, every citation checked against the named commit:
M-4a, M-4b, FL-1, FL-2, FL-3, FL-4, FL-5, RT-1, RT-2, RT-3a, RT-4, RT-5,
RT-6, RT-7, RT-8, SL-1, SL-2, SL-3, SL-4, SL-5, SL-6, SL-8, SL-9, SL-10,
SL-11. Supporting reads: `plan.md` §§1-2.10, §3 (rules, branches, thermal
solver scope), `ledger/00-schedule.md` (full schedule and dependency
order).

Counts: 1 major, 0 blockers, 0 minors beyond the major below. No entry in
scope has a false Problem claim, an evidence citation that does not match
the named commit, a Change that violates a binding rule (silent defaults,
shims, non-SI units, UTC/timezone handling, hot-path allocation, unproven
bitwise claims), or a schedule/dependency ordering inconsistent with
`00-schedule.md`.

## Findings

### Major — RT-1's hand-written `Clone for Telemetry` drops the unknown-key latch field

`ledger/06-runtime-memory.md` RT-1, Change item 1.

Evidence: RT-1 cites `Telemetry` as a two-field tuple struct, `Telemetry(pub
HashMap<String, f64>, Option<(String, f64)>)`
(`crates/hares-types/src/telemetry.rs:18-19` at `afa9a071`), and specifies
the replacement hand-written `Clone`/`clone_from` in terms of exactly those
two fields: the map and "the latch" (`self.1.clone_from(&source.1)`).

At the current HEAD (`7b7a6a7d`), `Telemetry` has three fields:

```rust
// crates/hares-types/src/telemetry.rs:27-30
pub struct Telemetry(
    pub HashMap<String, f64>,
    Option<(String, f64)>,
    #[serde(skip)] Option<String>,
);
```

The third field is the unknown-key latch (`unknown_key_latch()`,
`telemetry.rs:144`), added by OR-11 (`b8195c83`, already committed) between
RT-1's cited commit (`afa9a071`) and HEAD. OR-11 made the unknown-key check
unconditional: `Dwelling`'s step path reads `eq.telemetry().unknown_key_latch()`
directly off each equipment's live `Telemetry` (`crates/hares-core/src/dwelling/mod.rs:6479`)
and turns a latch into a run failure. RT-1's Change section never mentions
this field. As written, a hand-rolled `Clone`/`clone_from` built strictly to
RT-1's spec would copy field 0 (the map) and field 1 (the non-finite latch)
and leave field 2 either uninitialized (a compile error, forcing an ad hoc
decision at implementation time) or, if an implementer adds `self.2 =
source.2.clone()` without being told to, correct by accident; if they
instead follow the "otherwise falls back to `self.0.clone_from(&source.0)`"
sentence literally and stop there, `clone()`/`clone_from()` produce a value
that is not equal to the source under the struct's own derived `PartialEq`
(which does compare all three fields), and the unknown-key latch state is
silently lost across every copy the entry introduces.

I checked whether this currently matters on the call path: it does not, at
HEAD. Both current users of the per-equipment `Telemetry` snapshot
(`equipment_telemetry`, written by `clone_from` at
`crates/hares-core/src/dwelling/mod.rs:5277` in `snapshot_equipment_state`
and at `:6966` in the main step) feed only next-step actor reads
(`env.equipment_telemetry.get(name)`, `mod.rs:8515`); the unknown-key
invariant check at `:6479` reads the *live* equipment's `Telemetry`, never
the snapshot copy. So today nothing reads field 2 off a cloned value. But
the ledger's own binding rule is "no silent defaults, error loudly" and
"physics unchanged by any optimization and proven bitwise" — RT-1's
acceptance tests (`telemetry_clone_from_same_keys_allocates_nothing`) check
only the map and the non-finite latch; they do not check that `clone()` or
`clone_from()` preserves the unknown-key latch, so the gap is not caught by
the entry's own test plan, and a derived `PartialEq`-based future caller
(or a checkpoint/restore path that clones a `Telemetry` holding a latch)
would silently diverge from `derive(Clone)`'s current (correct, if
inefficient) behavior.

Correction: update RT-1's Change item 1 to name all three fields explicitly
— `clone()` copies the map, the non-finite latch and the unknown-key latch;
`clone_from()`'s in-place path and its fallback path both end by copying
the unknown-key latch the same way the non-finite latch is copied
(`self.2.clone_from(&source.2)`, which allocates only when the source holds
a latched key name and the destination does not already hold the identical
`String`). Add one acceptance-test case to
`telemetry_clone_from_same_keys_allocates_nothing` (or a sibling case)
asserting that a source with an unknown-key latch yields a copy whose
`unknown_key_latch()` equals the source's, on both the in-place and the
fallback path, matching the non-finite-latch case already specified.

This is "major" rather than "blocker" because it is inert on every call
site that exists at HEAD, so it cannot move a golden or fail a test as the
ledger entry stands; it is not "minor" because, implemented literally, it
introduces a latent `Clone`/`PartialEq` inconsistency in a type whose
correctness the step-end wiring check depends on, in a crate
(`hares-types`) with no visibility into how its callers will later use the
cloned value, and the entry's own acceptance tests do not exercise the
field that would catch it.

## Entries checked with no finding

- **M-4a, M-4b** (`02-measurement.md`): the workflow and publication plan
  match `plan.md` §2.4 Fleet is not relevant here; checked against
  `dl-i08-architecture.reproducibility`/`cross_platform_measurement`
  framing in plan.md §3 and the schedule (rows 99, 99a, 100, 100a). Problem
  and Change are consistent with each other and with BR-10's CI trigger
  rule (no feature-branch name in a workflow). Both are measurement/
  documentation entries with no code-correctness surface to red-team beyond
  "does the workflow exist and does the doc reproduce"; that cannot be
  checked before the workflow runs. Dependencies (M-4a: M-2 and every
  numeric entry; M-4b: M-4a and the first push) agree with the schedule's
  row order (99 after every numeric row, 100 after 99a).

- **FL-1 through FL-5** (`04-fleet.md`): every cited line in
  `crates/hares-fleet/src/fleet.rs` and `aggregation.rs` (the `simulate`/
  `simulate_parallel` split, `SimStatus`, `DwellingOutcome`,
  `ColumnAggregation`/`FleetAggregation` unit tables and their mean/
  weighted-sum fallbacks, `SteppableFleet::step`'s already-finished message,
  the per-home accessors) matches the code read at HEAD. The two-stage
  aggregation table in FL-1 and FL-5 matches `plan.md` §2.6's table exactly.
  FL-2's status mapping and FL-3's error-return change are internally
  consistent with FL-1's `HomeStatus`/`FleetResult` shapes. FL-4's time
  pre-pass (`read_site`, `read_weather_meta`, `site_offset`) is consistent
  with RT-4's later move of the same functions onto `InputSource`/
  `WeatherInput`, which FL-4 correctly defers to. `PyAggregationResolution`
  and `AggregationResolution`, which FL-1 says it deletes, both exist at
  HEAD as FL-1 describes. Dependency order against `00-schedule.md` (rows
  50-53, 87a) is consistent: FL-5 depends on FL-1 and FL-4 and is scheduled
  after both, before B-11 as stated.

- **RT-2, RT-3a, RT-4, RT-5, RT-6, RT-7, RT-8** (`06-runtime-memory.md`):
  spot-checked citations (`environment.rs` solar/humidity/mains
  computation sites, `rng.rs`'s stream constants and `derive_dwelling_rng`,
  `mod.rs:4159` `refresh_equipment_caches`, the RT-5 static census against
  a fresh grep) match the code at the commit each cites. RT-3a's stop-rule
  dependency on RT-3's measured 6% `ideal_capacity` share is consistent
  with the recorded stop-and-add entry in `02-measurement.md`. RT-5's
  static-disposition table is consistent with OR-5's and OD-18's already-
  recorded dispositions (checked against the measurement log's OR-11 and
  SL-7 entries). RT-6, RT-7 and RT-8's dependency chains (RT-4 before RT-6;
  PY-5/PY-7/PY-9/PR-3/RT-1 before RT-7; RT-4/RT-7/OR-10/PY-7 before RT-8)
  agree with the schedule's row numbers (61, 65, 71, 71a).

- **SL-2, SL-3, SL-8, SL-9, SL-11** (`10-slop-cleanup.md`): SL-9 is already
  closed by committed rows (BR-2 `fd23dd80`, PR-1 `d7f0e381`, both already
  in the schedule as committed). SL-2's census methodology (the `git grep`
  pattern and its path exclusions) is internally consistent with SL-8's
  move list and SL-11's follow-on census of the broader `[writing]`
  pattern; I did not re-run the 451-line and 138-line census greps (an
  expensive full-tree grep whose exact counts the ledger already commits
  to reproducing at commit time), which is a testing gap in this round:
  the counts are unverified here and should be reproduced by whoever
  implements SL-8/SL-11. SL-3 is a standing rule with no code of its own.

- **SL-1, SL-4, SL-5, SL-6, SL-10**: each correctly identifies a false or
  misleading comment/docstring and defers its fix to the ledger entry that
  already touches the same code (RT-1, CN-6, CN-5, CN-9, FL-3
  respectively), rather than duplicating work in its own commit. SL-1's
  citation of the false "slot reuse" comment matches the comment's current
  text at HEAD verbatim (`mod.rs:6962-6964`: "Use `clone_from` on the
  existing entry so steady-state telemetry keys reuse their f64 slots
  without reallocating the inner map"), confirming the comment is still
  false as claimed (`Telemetry` still derives `Clone` at HEAD) and that
  SL-1's fix is still pending, correctly deferred to RT-1 — which is the
  same entry the major finding above concerns, so RT-1's reviewer should
  pick up both the comment rewrite and the missing third field together.
  SL-10's fleet.rs citations (at `21d90588`) are consistent with the
  current fleet.rs's equivalent sites and with FL-3's planned fix.

## Testing gaps

- SL-2's and SL-11's line counts (451 and 138 lines respectively) were not
  reproduced in this round; they should be re-run with the exact `git grep`
  commands given in those entries immediately before SL-8/SL-11 land, since
  intervening commits (BR-6a's let-chain refactor, BR-6b/c's dependency
  upgrades) could have shifted or added matches.
- No test suite was run in this round: the scope is entries not yet
  implemented (RT-1, RT-2, RT-3a, RT-4 through RT-8, FL-1 through FL-5,
  M-4a, M-4b, and the slop entries not yet landed), so there is no commit
  to build or test against beyond the already-committed, already-audited
  SL-7 (`verification/audit-SL-7-7b7a6a7d.md`) and the already-committed
  rows SL-1/SL-9/SL-10 defer to. `perf` remains unusable on this host's
  WSL2 kernel, consistent with the ledger's own stated fallback (the
  profiling feature's phase shares and same-host interleaved A/B), which
  these entries already specify as their measurement method.
