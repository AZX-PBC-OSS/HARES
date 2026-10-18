---
red_team_outcome: clean
failing_tests: []
---

# Red-team — I-05, fourth pass (the empty-blob truncation gate)

## Relationship to the third pass

Round 3 left
`dwelling::tests::checkpoint_truncated_stateful_blob_must_not_restore_as_silent_success`
red: a stateful actor's checkpoint blob truncated to empty passed the
schema-version gate (truncation predates the version stamp) and
`load_state`'s empty-blob early return silently no-oped, so
`load_checkpoint` returned `Ok(())` while discarding the checkpointed
decision-state. This pass fixed it with an ownership probe in the restore
loop (`dwelling/mod.rs:4246-4265`): an empty blob is valid only for an
actor that also *saves* empty — probed against the live actor's
`save_state()` — with named rejections for both the truncated-blob and
probe-failure arms. My test now passes, and the implementer added the two
missing arms as tests of their own:
`checkpoint_stateless_actor_empty_blob_restores_cleanly` (the convention
must not be over-rejected) and
`checkpoint_empty_blob_probe_failure_rejected_with_named_error` (a
failing probe must be a named rejection, not a panic or silent pass).
Full suite: hares-core 974 passed / 0 failed, hares-equipment 2254 / 0.

## Surface read this round

The only new production logic since round 3 is in `dwelling/mod.rs`: the
empty-blob probe inside the per-actor restore loop; the save loop's
`collect::<Result<..>>` (one failing `save_state` fails the whole save);
and `check_ev_rng_stream_no_collision` (debug-gated duplicate
seed/stream diagnostic wired into the EV-seed arm, streams
`RNG_STREAM_EV_DRIVER_BASE + index`, base 1 vs event-load base 1000).
Everything else (composer rework, scoped `max()`, `minutes_until`,
V2G/V2H floor move, checkpoint v7 plumbing) is unchanged from the round-3
read; integration-level EV checkpoint restart is pinned in
`tests/regression/checkpoint_restart.rs` with a real `EvDriverActor`.

## Hypotheses attacked by trace — none survived as a finding (one became a test)

- **Probe false positive** (actor stateless at save, stateful at restore,
  or vice versa): both directions require the actor's state model to
  differ between construction and the probe, but the probe runs on the
  freshly-constructed live actor — the same construction state the
  checkpoint's empty blob was written from. For every implementation in
  the tree, `save_state` is unconditionally non-empty when stateful (the
  EV driver serializes a ten-field snapshot; the stub serializes a
  `u32`) and unconditionally empty for the trait default. **The
  conditional-emptiness variant — an actor whose state legitimately
  drains to empty after starting non-empty — could not be settled by
  reading (both resolutions are defensible), so it is pinned as a test:
  `checkpoint_drained_state_actor_must_not_resume_on_fresh_state`
  (below).**
- **Probe side effects**: `save_state(&self)` — a pure snapshot by
  signature; no interior mutability in any implementation read.
- **Probe cost**: only reached for empty blobs — the stateless
  convention — never on the hot stateful path.
- **Partial mutation on mid-loop restore failure**: the version-mismatch
  rejection (pre-existing) aborts mid-loop the same way; the error
  contract implies the caller discards the dwelling. Consistent, not new
  to this pass.
- **Extra checkpoint actor entries silently ignored** (restored_count
  counts live-actor matches, not entries) and **duplicate-name last-wins
  in `cp_actor_map`**: both unreachable from `save_checkpoint`, which
  writes exactly one entry per registered live actor; carried over from
  the round-3 trace unchanged.
- **The `restored_count` error message says "checkpoint has N actor
  states" using the restored count, not the entry count**: cosmetic
  wording in an error path, indistinguishable diagnostics for two
  corruption shapes; not test-worthy.
- **`check_ev_rng_stream_no_collision` can never fire**: streams are
  distinct per EV by construction (`BASE + index`, `index` incremented
  only after the name-dedup `continue`, so skips don't shift streams),
  and (seed, stream) pairs from `derive_sub_rng` differ whenever the
  stream differs. It is a debug-gated diagnostic for future derivation
  changes, not a live defect; a test would pin that a tautology is
  false.
- **Duplicate live actor names reaching the restore loop's `HashMap`
  (last-wins blob routing)**: unreachable — `Dwelling::add_actor`
  rejects a duplicate name up front (`dwelling/mod.rs:2914-2918`), so
  `save_checkpoint` can never write two entries for one name.
- **Stateless actor fed a non-empty blob** (misrouted corruption): the
  trait-default `load_state` no-ops, and nothing is lost — the actor has
  no state to lose; the blob's true owner is caught by the same-name
  routing or the count check.
- Carried from prior passes (unchanged surface, previously traced): the
  composer's unconditional min/max folds vs. single-vote target source,
  idle votes dropping bounds, the anxiety gate's equal-minute boundary,
  stale `max_soc` ceiling, override top-up vs. latched deadline,
  degenerate direct-arrival sampling.

## Verdict

Clean. The fix pass closed the round-3 finding and covered all three arms
of its own new rule with tests; this pass added the fourth arm the
implementer's rule does not model — an actor whose statefulness is
time-varying — as permanent air cover (`checkpoint_drained_state_actor_must_not_resume_on_fresh_state`,
green: the probe rejects the ambiguous shape with a named error rather
than silently resuming on fresh state), and traced the remaining surface
to ground. No failing tests, no findings.

## Test added this pass

- **`dwelling::tests::checkpoint_drained_state_actor_must_not_resume_on_fresh_state`**
  (`crates/hares-core/src/dwelling/mod.rs`) — hypothesis: misuse of the
  empty-blob ownership rule by an actor whose `save_state` is empty at
  save time (state drained) but non-empty on the freshly-constructed
  restore-time actor. The probe cannot distinguish this from truncation,
  and the `load_state` empty-blob no-op would silently keep fresh state
  contradicting the checkpoint. The test asserts the restore never
  reports success while the actor carries state the checkpoint says it
  does not have: either a named rejection (current behavior) or a
  restore into the checkpointed drained state. A future loosening of the
  probe that falls back to the silent no-op turns this test red.

## Verification

```
$ cargo test -p hares-core --lib -- checkpoint_truncated_stateful_blob
  → ok. 1 passed; 0 failed
$ cargo test -p hares-core --lib -- checkpoint_drained_state_actor
  → ok. 1 passed; 0 failed
$ cargo test -p hares-core --lib        → 975 passed, 0 failed
$ cargo test -p hares-equipment --lib   → 2254 passed, 0 failed
$ cargo clippy --workspace --all-targets -- -D warnings   → clean
$ cargo fmt --check --all               → clean
```

## Scope statement

Production code touched this pass: none. Files edited:
`crates/hares-core/src/dwelling/mod.rs` — test-only additions in the
`#[cfg(test)]` module (the `DrainingStateActor` stub and one test fn,
inserted between the probe-failure and v1-gate tests) plus `cargo fmt`.
Every other modified file in `git status` is the implementer's working
tree, untouched here.
