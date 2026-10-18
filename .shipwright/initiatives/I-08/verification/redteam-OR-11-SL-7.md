# Red team: OR-11 (b8195c83) and SL-7 (7b7a6a7d)

Worktree: `/home/rich/src/HARES/.workstrees/redteam-7b7a6a7d` (checked out detached at `7b7a6a7d`, which is `b8195c83` plus SL-7's changes to `hares-envelope/src/state_space.rs`, `thermal_solver/mod.rs`, `thermal_solver/stepping.rs`, the `rc_solver` bench and the zero-alloc test). All reproductions below ran in that worktree with `cargo test`. Nothing was committed; the worktree is scratch only.

This red team goes past the two prior audits (`verification/audit-OR-11-b8195c83.md`, `verification/audit-SL-7-7b7a6a7d.md`) and the three recorded OR-11 gaps (zone fallback, synthetic envelope, IdealHvac end-use sign), which are not repeated here.

## Verdict: b8195c83 (OR-11) -- ATTACK-SUCCEEDED

Blockers: 2. Majors: 0. Minors: 1.

## Verdict: 7b7a6a7d (SL-7) -- ATTACK-FAILED

Blockers: 0. Majors: 0. Minors: 1 (pre-existing, not introduced by this commit; noted for completeness).

---

## Blocker B1 (OR-11): `add_equipment` pushes the candidate before the fallible fluid-loop-map rebuild, and never rolls back on failure

`crates/hares-core/src/dwelling/mod.rs:3678-3714` (`add_equipment`):

```
eq.mark_initialized();
self.equipment.push(eq);              // <-- 3701: candidate joins the vector
...
self.refresh_equipment_caches()?;      // <-- 3712: now fallible for a reason that did not
Ok(())                                 //           exist before this commit
```

`refresh_equipment_caches` (`mod.rs:4155-4244`) was changed by this commit
to call, for every equipment add/remove/replace before the first step
(`mod.rs:4237-4243`):

```
let fluid_loops = self.ports.fluid.iter().map(|f| (f.loop_id, f.fluid_type)).collect::<Vec<_>>();
self.fluid_solver.rebuild_loop_types(&fluid_loops)?;
```

`FluidSolver::rebuild_loop_types` (`crates/hares-envelope/src/fluid_solver.rs:209-216`) calls
`loop_type_map` (`fluid_solver.rs:100-114`), which returns
`Err(HaresError::Envelope(...))` the moment two declared ports share a
`LoopId` with different `FluidType`s. Before this commit,
`refresh_equipment_caches` had no fallible call that depended on the
equipment just pushed (confirmed by diffing `b8195c83^` --
`extract_fluid_loops_from_specs` was called only once, at initial assembly,
never from `add_equipment`/`refresh_equipment_caches`). This commit adds a
new, easily user-triggered failure mode to a function whose caller already
pushed the candidate and has no rollback path.

### Reproduction

Added `dwelling::tests::add_equipment_leaves_equipment_in_vector_after_fluid_type_conflict_rejection` to `crates/hares-core/src/dwelling/mod.rs` in the worktree: build a synthetic dwelling, `add_equipment` an `ElectricBoiler` declaring `loop_id = 7, fluid_type = Water` (accepted), then `add_equipment` a second `ElectricBoiler` declaring `loop_id = 7, fluid_type = Glycol`.

```
$ cargo test -p hares-core --lib dwelling::tests::add_equipment_leaves_equipment_in_vector_after_fluid_type_conflict_rejection -- --nocapture
...
thread '...' panicked at crates/hares-core/src/dwelling/mod.rs:17377:9:
assertion `left == right` failed: BUG REPRODUCED: add_equipment reported Err
(rejection) for the fluid-type-conflicting boiler, but the equipment vector
grew from 2 to 3 anyway -- the candidate was pushed before the fallible
refresh_equipment_caches call and never rolled back on failure. The
dwelling now silently carries a boiler the caller believes was rejected,
and its ports/fluid-loop map are now out of sync.
  left: 3
 right: 2
```

`add_equipment` returns `Err`, matching the documented contract ("a
rejected equipment never joins the vector" -- see the doc comment at
`mod.rs:3716-3720`, which this bug directly contradicts), yet
`dwelling.equipment.len()` grew by one. The second boiler is live in the
dwelling: it will execute on the next `step()`, contributing (with the
wrong, conflicting fluid type) to loop 7, and the fluid-loop map
(`self.fluid_solver`) was never actually rebuilt to agree with the new
port table, because the `?` fired mid-rebuild.

This is a direct violation of OR-11's own stated goal ("no silent
substitution" on the fluid-loop map) and of the broader checks-are-now-
unconditional-typed-errors theme: the typed error fires, but the caller
cannot trust it, which is worse than the silent warning it replaced,
because the caller now actively believes the rejection succeeded.

### Fix

`add_equipment` must validate everything that can fail, including the
fluid-loop-map rebuild, *before* pushing the candidate -- or must pop/roll
back the push on any `Err` from `refresh_equipment_caches`. The cleanest
fix: compute the candidate's port declarations and feed them through
`FluidSolver::rebuild_loop_types`'s `loop_type_map` check (a pure,
side-effect-free dry run) inside `validate_candidate_equipment`
(`mod.rs:3721-3736`), alongside the existing zone and
`ensure_ports_satisfied` checks, so the fluid-type conflict is caught
before `self.equipment.push(eq)` ever runs. `replace_equipment` needs the
equivalent fix (see B2).

---

## Blocker B2 (OR-11): `replace_equipment` performs the swap before the fallible refresh, silently destroying the original equipment on rejection

`crates/hares-core/src/dwelling/mod.rs:3980-4036` (`replace_equipment`):

```
new_equipment.mark_initialized();
let mut old = std::mem::replace(&mut self.equipment[pos], new_equipment);  // 4013: swap happens here
...
self.refresh_equipment_caches()?;   // 4034: fails -> `?` returns Err, `old` is dropped
Ok(old)
```

This is strictly worse than B1: when `refresh_equipment_caches` fails
after the swap, the function's early return via `?` drops the local `old`
binding -- the original, evicted equipment -- without ever handing it back
to the caller. The caller receives `Err`, which every other rejection path
in this function (duplicate name, zone validation, `ensure_ports_satisfied`)
correctly means "the vector is untouched, the original equipment is
intact." Here it instead means the original equipment has been
permanently destroyed and the rejected replacement is installed in its
place.

### Reproduction

Added `dwelling::tests::replace_equipment_destroys_original_on_fluid_type_conflict_rejection`: build a synthetic dwelling with a `Furnace`, `add_equipment` an independent `ElectricBoiler` on `loop_id = 7, fluid_type = Water`, then `replace_equipment("Furnace", <ElectricBoiler on loop_id = 7, fluid_type = Glycol>)`. `validate_replacement_equipment` (`mod.rs:3743-3778`) does not check fluid-type consistency -- it only runs `ensure_ports_satisfied` once stepping has begun (`current_step > 0`), so at step 0 the candidate passes validation and the swap proceeds.

```
$ cargo test -p hares-core --lib dwelling::tests::replace_equipment_destroys_original_on_fluid_type_conflict_rejection -- --nocapture
...
thread '...' panicked at crates/hares-core/src/dwelling/mod.rs:17517:9:
BUG REPRODUCED: replace_equipment returned Err (implying the replacement
did not happen and the original 'Furnace' survives unchanged), but the
swap at std::mem::replace already ran before the fallible
refresh_equipment_caches call. furnace_survived=false,
replacement_installed=true. The original equipment the caller believed
was untouched has actually been permanently destroyed (dropped as `old`
on the `?` early return) and replaced by the equipment the caller believes
was rejected.
```

Confirmed: `result.is_err()` is true, `Furnace` is gone, and
`FurnaceReplacement` (the fluid-type-conflicting candidate the API just
reported rejecting) is in the equipment vector and will step next tick.
Any caller (the Python surface, a controller, test harness) that retries
on error or assumes the original equipment survives a failed
`replace_equipment` call has silently lost that equipment and is now
stepping an unvalidated, type-conflicting boiler instead.

### Fix

Validate the fluid-loop-map consistency of the replacement's declarations
against the rest of the dwelling's live ports (excluding the evictee)
inside `validate_replacement_equipment`, before the swap, mirroring B1's
fix. As defense in depth, `refresh_equipment_caches`'s remaining fallible
step (the output-schema-drift `InvariantViolation`, pre-existing) should
also be validated pre-swap, or `replace_equipment` should restore `old`
into `self.equipment[pos]` on any `Err` from the post-swap refresh instead
of letting it drop.

---

## Minor M1 (OR-11): partial-failure exposure is architectural, not limited to the fluid map

Both B1 and B2 stem from the same root cause: `refresh_equipment_caches`
was given a new fallible, data-dependent check (the fluid-loop-map
rebuild) without auditing every call site for push-then-validate ordering.
The output-schema-drift check inside the same function
(`mod.rs:4152-4154`'s documented `HaresError::InvariantViolation`) has the
identical pre-existing exposure in both `add_equipment` and
`replace_equipment`, just with a much narrower trigger (schema/column-map
drift is not normally user-reachable with the shipped equipment set, so it
was not reproduced here). Recommend a structural fix -- validate everything
fallible before mutating `self.equipment`, or make `refresh_equipment_caches`
infallible and move all its error conditions into pre-mutation validation --
rather than patching the fluid-type case alone, since any future fallible
check added to `refresh_equipment_caches` will reintroduce this class of
bug.

---

## SL-7 (7b7a6a7d): could not break it

Checked for exactly the risks the task calls out:

- **Was `m_is_identity` ever `false` in any production or reachable test path before the removal?** No. Grepped the pre-commit `state_space.rs` and every caller for `m_is_identity: false` / any constructor that could set it: `from_continuous` and `from_discrete` both hardcoded `m_is_identity: true` and `m_mat: eye` unconditionally, with no parameter through which a caller could supply a non-identity M. `from_discrete`'s own pre-commit doc comment claiming "M and N are caller-supplied" was already inaccurate -- the function signature never took an `M` argument. The removed LU/coupled-LU path was dead code on every construction path that exists in the codebase, confirmed by `git grep` across the full pre-commit tree for `build_coupled_lu`, `step_with_coupled_lu_into`, `step_with_coupling_into`, `CouplingData`, `SolverScratch`: the only call sites were `state_space.rs` itself, `thermal_solver/stepping.rs`'s now-removed branches, the now-removed `bench_coupled_step_lu`, and the doc-string update in `radiant_gain_weights_zero_alloc.rs`.
- **-0.0/+0.0 and the closed-form diagonal solve**: `step_with_identity_coupling_into_scratch` (`state_space.rs:454-492`) divides by `1.0 + d_agg[i]` only when `d_agg[i] != 0.0`; this logic is byte-for-byte unchanged by the commit (it existed identically before, duplicated for the identity-fast-path). Not a regression.
- **Ill-conditioned couplings (`1 + d_i` near zero or negative)**: no bounds check exists on `d_agg[i]` before the division, so a caller-supplied coupling with `d_i <= -1` would still divide by zero/flip sign -- but this is pre-existing (present identically in the pre-commit identity branch) and not something SL-7 introduced or could have caught, since the removed LU path had the equivalent failure mode (`ImplicitMatrixSingular` only fires on an exactly-singular matrix, not an ill-conditioned one near it). Recorded as a pre-existing minor, not charged against this commit.
- **Bench still measuring the real path**: `bench_coupled_step_identity` is unchanged except for the removal of the now-nonexistent `model.m_is_identity()` assert; it is still exercising `step_with_identity_coupling_into`, which is still the only production code path. `bench_coupled_step_lu`, which benchmarked the removed dead function, was correctly deleted rather than left to silently benchmark nothing meaningful.
- **Zero-alloc hot path**: ran `cargo test -p hares-envelope --test radiant_gain_weights_zero_alloc` in the worktree; passes. The commit's own doc-comment update (dropping `build_coupled_lu` from the list of tracked non-SIMD allocation sources) matches the removal; no new allocation was introduced.
- **`from_discrete` test-only non-identity M**: every production and test call site for `from_discrete` (checked via `git grep -rn "from_discrete("`) passes only `(a_d, b_d, c, d)` -- there is no code path, including the Python bindings, ResStock loader, or parity harness, that constructs a `StateSpaceModel` with a custom non-identity `M`. The removal is behavior-preserving for every construction path that exists.

No wrong physics, no panic, no hot-path cost regression, and no construction path where the removed branch would have been reachable was found for SL-7. Verdict stands: ATTACK-FAILED.

---

## Summary

| Commit | Verdict | Blockers | Majors | Minors |
|---|---|---|---|---|
| b8195c83 (OR-11) | ATTACK-SUCCEEDED | 2 | 0 | 1 |
| 7b7a6a7d (SL-7) | ATTACK-FAILED | 0 | 0 | 1 |

The two OR-11 blockers (B1, B2) are reproduced with passing-to-failing unit
tests added in the scratch worktree (not committed). Both are
Python-reachable through the public `Dwelling::add_equipment` /
`Dwelling::replace_equipment` API and require no unusual configuration --
two hydronic boilers (or a boiler and any other fluid-port equipment)
sharing a loop id with different fluid types, added or swapped in before
the first step, which is a configuration mistake a real HPXML/ResStock
pipeline or a Python caller doing dynamic equipment swaps could plausibly
make.
