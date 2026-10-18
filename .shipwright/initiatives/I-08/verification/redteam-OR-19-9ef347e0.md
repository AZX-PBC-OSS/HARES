# Red team: OR-19, commit 9ef347e0

Verdict: ATTACK-SUCCEEDED

Counts by severity: Blocker 0, Major 1, Minor 0, Informational 1

## Scope and method

Commit 9ef347e0 ("fix: reject equipment and actor changes without leaving the dwelling half changed") rewrites every roster-mutating entrance on `Dwelling` (`add_equipment`, `replace_equipment`, `remove_equipment`, `remove_equipment_by_end_use`, `clear_equipment`, `add_actor`, `auto_register_actors`) around a plan/mutate/install split: `plan_roster_caches` computes every fallible derivation against a prospective roster, and only on `Ok` does the caller mutate `self.equipment`/`self.actors` and call the now-infallible `install_roster_caches`. The stated contract is that a rejected change leaves the dwelling exactly as it was before the call.

I built a disposable detached worktree at commit 9ef347e0 (`git worktree add --detach .workstrees/redteam-or19 9ef347e0`), read the full diff to `crates/hares-core/src/dwelling/mod.rs` and `crates/hares-envelope/src/fluid_solver.rs`, and added one scratch test inside `crates/hares-core/src/dwelling/mod.rs`'s existing `mod tests` block (same file, never committed, worktree later removed with `git worktree remove --force`). I ran the full existing `dwelling::tests::` suite (1005 tests, 132 of which actually execute given the filter used) to confirm the pre-existing suite is green on this commit before adding anything, then added my repro and reran.

All 132 pre-existing tests in `crates/hares-core/src/dwelling/mod.rs`'s test module pass on 9ef347e0, including the five new OR-19 tests the commit itself adds (`rejected_add_leaves_roster_unchanged`, `rejected_replace_keeps_the_original`, `rejected_actor_add_leaves_roster_unchanged`, `removed_loop_declarer_frees_its_fluid_type_before_first_step`, `post_step_conflicting_fluid_declaration_is_rejected`). The zero-allocation thermal test (`hares-envelope/tests/radiant_gain_weights_zero_alloc.rs`) and the identity/fluid-loop regression suites (`equipment_id_regressions.rs`, `equipment_identity_misattribution.rs`, `fluid_loop_declarations.rs`) all pass unmodified. I did not find a perf regression or a panic path in `plan_roster_caches` itself (no `unwrap`/`expect`/indexing that can fail on a well-formed candidate), and the Python bindings (`crates/hares-python/src/py_dwelling.rs`) only call straight through to the Rust methods audited here, with no parallel caches of their own, so they inherit whatever the Rust layer does.

## Finding 1 (Major): a rejected `add_equipment`/`replace_equipment` still advances the never-reused equipment-id counter

File/line: `crates/hares-core/src/dwelling/mod.rs:3957-4055` (`assign_equipment_identity`), called from `validate_candidate_equipment` at `mod.rs:3882` and `validate_replacement_equipment` at `mod.rs:3924`.

`add_equipment` and `replace_equipment` both call their entrance validator (`validate_candidate_equipment` / `validate_replacement_equipment`) *before* building the prospective roster and calling `plan_roster_caches`. That validator calls `assign_equipment_identity`, which directly mutates `self.next_equipment_id` (mod.rs:4051-4053: `self.next_equipment_id = self.next_equipment_id.max(next)`) as a side effect of assigning the candidate's id — this runs on `self`, not on a planned/staged copy, and is never rolled back.

If the candidate then fails the later, roster-level check inside `plan_roster_caches` (a fluid-loop fluid-type conflict, an output-schema drift error, or, post-step, `ensure_ports_satisfied`), the call returns `Err` and the equipment/actor vectors, port tables, schema, and fluid-loop map are all correctly rolled back by the commit's new plan/install split — but `next_equipment_id` is not, because it was never part of `RosterCaches`/`install_roster_caches` in the first place. The counter silently advances on a call that the commit's own doc comments and tests claim leaves "the equipment and actor rosters... exactly as before the call."

This directly contradicts the commit's stated invariant and the ledger's OR-19 claim ("every ... roster change ... is all-or-nothing"). It is a real, provable violation of atomicity, not a cosmetic one: the same mechanism (`assign_equipment_identity`) runs unconditionally before every fallibility check that can reject the add, so every rejected `add_equipment`/`replace_equipment` call burns at least one id, auto-assigned or explicit, permanently and unrecoverably (ids are documented as never-reused/never-decremented by design, so this is not freed by any later path).

### Reproduction

Added to `crates/hares-core/src/dwelling/mod.rs`, directly after the commit's own `rejected_add_leaves_roster_unchanged` test (same `bestest_dwelling`/`boiler_on_loop` fixtures the commit itself introduces):

```rust
#[test]
fn rejected_add_still_advances_the_equipment_id_counter() {
    let mut dwelling = bestest_dwelling();
    let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
    dwelling
        .add_equipment(water)
        .expect("the first declarer of loop 7 is accepted");

    let next_id_before = dwelling.next_equipment_id;

    let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, &dwelling.latest_env);
    dwelling
        .add_equipment(glycol)
        .expect_err("a second declarer of loop 7 with a conflicting fluid type is rejected");

    assert_eq!(
        dwelling.next_equipment_id, next_id_before,
        "a fully rejected add must not advance the never-reused equipment id counter"
    );
}
```

Result: fails on 9ef347e0.

```
thread 'dwelling::tests::rejected_add_still_advances_the_equipment_id_counter' panicked at crates/hares-core/src/dwelling/mod.rs:15674:9:
assertion `left == right` failed: a fully rejected add must not advance the never-reused equipment id counter -- ...
  left: 5
 right: 4
```

The second (rejected) boiler consumed id 4, advancing the counter to 5, even though it never joined `self.equipment`, `self.equipment_id_by_name`, or any other roster-derived cache — all of which correctly show no trace of it. The same call path is shared by `replace_equipment` (`validate_replacement_equipment` → `assign_equipment_identity`, mod.rs:3924), so a rejected replacement burns an id the same way; I did not write a second repro for that path since the root cause and fix are identical.

### Impact

Realistic impact is bounded: ids are `u32`, so the leak cannot be exhausted by ordinary workloads, and nothing downstream currently depends on ids being contiguous or on the counter's exact value (only on each id being assigned at most once, which still holds). The bug does not cause misattribution, data loss, or a crash. But it is a genuine, cheaply-reproducible break of the specific guarantee this commit exists to establish, it is observable through a public API (`add_equipment`/`replace_equipment`, including the Python bindings, which call straight through with no additional guard), and nothing in the commit's own test suite (five new tests, all of which check roster/schema/fluid-map state but none of which check `next_equipment_id`) would have caught it. I am rating this Major rather than Blocker because there is no plausible exploit path to data corruption or crash, and not Minor because it is a direct, demonstrated violation of the ticket's core correctness claim rather than a cosmetic nit.

### Fix

Move identity assignment into the plan/install split like everything else `plan_roster_caches` touches, instead of mutating `self.next_equipment_id` from inside entrance validation:

1. Have `plan_roster_caches` (or a step that runs immediately before it, operating on the candidate roster rather than on `self`) compute the candidate's assigned id and the resulting counter value, and carry both inside `RosterCaches` (e.g. `assigned_equipment_id: Option<(EquipmentId, u32)>`).
2. Have `assign_equipment_identity` stop writing `self.next_equipment_id` directly; it should return the computed `(assigned_id, next_counter_value)` to its caller instead.
3. Have `install_roster_caches` write `self.next_equipment_id` from the planned value, exactly like every other field it installs, so the counter only ever advances when the roster change is actually committed.

This keeps the write inside the same infallible install step as `equipment_id_by_name`, closing the gap without changing the public behavior of a successful add/replace.

## Finding 2 (Informational): equipment added via `add_equipment` before the first step gets the same output schema as equipment assembled at construction

I checked this specific attack idea directly: `plan_roster_caches`'s `OutputCachesPlan::FreshSchema` branch (mod.rs, around the `equipment_descriptor_specs(equipment)` / `build_schema` call) runs identically whether triggered by `build_from_blueprint_inner` at construction or by `add_equipment`/`replace_equipment`/etc. before the first row is recorded — both paths end up calling the same `build_schema`/`build_equipment_column_map`/`build_actor_column_map` functions over the live `equipment`/`actors` slices, with no distinction in verbosity, telemetry-unit metadata, or column ordering. I did not find a discrepancy here; this is a non-finding, recorded for completeness since it was an explicit attack idea in scope.

## What I did not chase further

Given the scope of this pass, I verified but did not write additional repro tests for: checkpoint save/restore interaction with a rejected roster change (the existing `checkpoint_round_trip_preserves_equipment_identity` and `checkpoint_restore_rejects_spec_reorder_with_named_error` tests pass unmodified, and checkpoint save/restore does not touch the roster-mutation entrances at all); and panic-safety of the `std::mem::take`-then-match pattern (no `unwrap`/`expect`/fallible indexing exists inside `plan_roster_caches` on a well-formed candidate, so I found no realistic panic trigger to repro inside the time available for this pass). Both are reasonable follow-ups if a deeper pass is wanted, but I am not reporting them as findings since I have no reproduction for either.

## Confidence

High confidence on Finding 1: it is a compiled, run, and observed test failure on the exact commit under review, isolated to a single root cause with a clear code citation. Moderate confidence that Finding 1 is the only atomicity break of this kind in the commit — I read the full diff and specifically traced every direct `self.*` mutation in each entrance against whether it happens before or after the `plan_roster_caches` gate, and `next_equipment_id` was the only field I found mutated outside the plan/install split. I did not exhaustively fuzz every attack idea in the prompt (panic injection and checkpoint interaction were spot-checked, not stress-tested).
