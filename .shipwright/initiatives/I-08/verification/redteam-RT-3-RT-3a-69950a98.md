# Red team: RT-3 (44da330c) and RT-3a (69950a98)

## Verdicts

- RT-3 (`44da330c`, "step the thermal solver without heap allocation"): ATTACK-FAILED.
- RT-3a (`69950a98`, "compute the ideal-capacity solve's shared terms once per step"): ATTACK-FAILED.

## Counts by severity

- Blocker: 0
- Major: 1 (pre-existing, not introduced by either commit; see Finding 1)
- Minor: 2 (both pre-existing or coverage gaps, not functional regressions; see Findings 2-3)
- Informational: 0

No defect was found that is newly introduced by `44da330c` or `69950a98`. All findings below are either pre-existing latent risk that these commits touch without fixing, or test-coverage gaps around claims the code itself satisfies.

## Method

Built a detached worktree at `69950a98` (`.workstrees/redteam-69950a98`, removed at the end of this session). Read the RT-3 and RT-3a problem/change/acceptance-test sections of `06-runtime-memory.md`, then read both commits' full diffs (`state_space.rs`, `thermal_solver/mod.rs`, `thermal_solver/stepping.rs`, `thermal_solver/config.rs`, `lib.rs`, and the new tests in `radiant_gain_weights_zero_alloc.rs`).

Ran:
- `cargo nextest run -p hares-envelope` (441/441 pass).
- `cargo nextest run --workspace --no-fail-fast` (7130/7130 pass, 1 pre-existing unrelated failure excluded — see Finding 4).
- `cargo run -p frame_golden --release -- compare-all` and again with `--features dst` (all 6 golden fixtures, including `full_year_900s_dst`, bitwise identical).

Wrote and ran scratch probes in the worktree (deleted before finishing, never committed):
1. A multi-equipment-per-zone probe (3 `solve_ideal_capacity_for_target` calls per zone per step, simulating several equipment sharing a zone) under the `CountingAllocator` the committed test uses: still zero allocations. This is the actual RT-3a reuse scenario; the committed zero-alloc test (`thermal_solver_step_allocation_free_after_first_step`) only exercises one call per zone per step, so it doesn't bracket this path, but the code is in fact allocation-free there too.
2. A generic nalgebra differential test comparing `c.mul_to(x,&mut y); d.mul_to(u,&mut du); y+=du` (the new `output_into`) against the owned `&c*x + &d*u` expression template, with normal, NaN, signed-zero (`-0.0`), and large-magnitude (40x40) inputs: bitwise identical in every case.
3. A differential test reimplementing the pre-`44da330c` `solve_for_scalar_input_identity_coupled` body (owned expressions, no scratch) and comparing it bit-for-bit against the current scratch-based implementation, across: normal inputs, a negative coupling diagonal (physically legal, not rejected by this function), two coupling entries hitting the same state index (aggregation order), signed zero and NaN in `x`/`u`/forcing, and empty couplings. All five cases bitwise identical.
4. Full source audit of every mutation site of `self.x`, `self.last_u`, `self.last_coupled_state`, `self.last_coupling` in `thermal_solver/*.rs` to confirm every one is preceded by `invalidate_shared_prefix()` (RT-3a's cache-staleness contract). Confirmed: `prepare_inputs_inner`, `integrate_inner`, and `restore_state` each invalidate before mutating; no other site touches these fields.
5. Checked the construction-time divisor guard (`CouplingCoefficientInvalid`/`ConvectionInjectionInvalid`) against every coupling-producing site (infiltration, exterior LWR, interior TARP convection) to see whether a value that passes construction could still drive the per-step divisor `1 + d_i` to zero, negative, or near-zero at runtime. Traced `h_inf_w_k`, `h_rad_w_k`, and `delta_h` (TARP) to their construction, all physically non-negative or explicitly gated (`delta_h <= 0.0` skips the coupling). `c_surface`/`c_zone` are clamped to `1e-12` at the use site (`stepping.rs:666-667`), which can make `d` very large but never makes `1 + d_i` approach zero from the legal, non-negative side; it is pre-existing and not a near-zero-divisor defect.

## Findings

### Finding 1 (Major, pre-existing, not introduced by RT-3/RT-3a): `restore_state` does not restore `last_coupled_state`/`last_coupling`, and RT-3a's caching makes this harder to notice

`crates/hares-envelope/src/thermal_solver/mod.rs:1235-1350` (`ThermalSolver::restore_state`): phase 2 restores `self.x` and `self.last_u` from the snapshot (and, after `69950a98`, correctly calls `self.invalidate_shared_prefix()` first, `mod.rs:1333`), but never restores `self.last_coupled_state` or `self.last_coupling`. Both fields keep whatever value they held immediately before the restore call.

`solve_ideal_capacity_for_target` (`stepping.rs:394-438`) dispatches on `self.last_coupled_state` and, for the coupled path, uses `self.last_coupling` (the per-state coupling diagonals and forcing terms) together with the *restored* `self.x`/`self.last_u`. If a checkpoint is restored mid-run (e.g. an RL episode reset, a calibration candidate replay, or a stepped-run rollback) at a point where the coupling state differs from the state immediately before the restore call (for example restoring into a step where infiltration/TARP convection would be inactive, from a moment where it was active, or vice versa), the first `solve_ideal_capacity_for_target` call after the restore computes a HVAC capacity using coupling terms that belong to the pre-restore state, not the restored one. This can select the wrong tail function (coupled vs. uncoupled) and, even when the variant happens to match, apply stale per-state `d_diag`/`forcing` values alongside the newly restored `x`.

This is **pre-existing**: `restore_state` never touched `last_coupled_state`/`last_coupling` before `RT-3` either (checked at `7b7a6a7d`), and the pre-RT-3 code read exactly the same two fields on every call, so the bug is identical in effect before these two commits. I flag it here as major, not blocker, because:
- it requires restoring into a step whose coupling state differs from the solver's state at the moment of restore (not every restore triggers it);
- it affects exactly one step's ideal-capacity solve (the next `prepare_inputs` call rebuilds `last_coupling`/`last_coupled_state` correctly from the restored state);
- it is not introduced by RT-3/RT-3a's own acceptance tests, which cannot catch it because `shared_terms_computed_once_per_step`'s "Window 3: restore_state" check (`mod.rs`, new in `69950a98`) builds its own bitwise reference from `solver.last_coupling` read at the same point the shared path reads it — both sides observe the same (possibly stale) value, so the test proves the cache reuse is faithful to whatever `last_coupling` currently holds, not that `last_coupling` is correct after a restore.

**Fix:** add `last_coupled_state` and `last_coupling` (or enough information to recompute them, e.g. recomputing coupling from the restored `x`/weather via `build_coupling()`) to `ThermalSnapshot`, and restore them in `restore_state` phase 2 alongside `x`/`last_u`, before `invalidate_shared_prefix()` is relied upon. At minimum, document that a solve called between `restore_state` and the next `prepare_inputs` is using the pre-restore coupling state, and audit callers (dwelling checkpoint restore, Python `reset_with_seed`, RL episode reset) for whether they call `solve_ideal_capacity_for_target` in that window.

### Finding 2 (Minor, coverage gap, not a defect): the committed zero-allocation test does not bracket the actual RT-3a reuse scenario

`crates/hares-envelope/tests/radiant_gain_weights_zero_alloc.rs:238-243` (`run_dwelling_step`) calls `solve_ideal_capacity_for_target` exactly once per zone per step. RT-3a's own motivating scenario is several equipment sharing one zone, each calling `solve_ideal_capacity_for_target` once per step (`SolverFeedbackActor::collect_and_solve`, `crates/hares-core/src/actors/solver_feedback.rs:52-66`, one call per equipment with an ideal target). The committed zero-alloc test never exercises more than one call per zone, so it cannot catch a regression that only allocates on the second-or-later tail call in a step (e.g. a future change to `tail_rhs`/`gain`/`d_agg` handling that reallocates).

I verified by probe (not committed) that the real code *is* allocation-free for 3 calls per zone per step, both with and without couplings, so this is a coverage gap rather than a bug. **Fix:** extend `thermal_solver_step_allocation_free_after_first_step` (or add a sibling test) to call `solve_ideal_capacity_for_target` 2-3 times per zone per step inside the allocation bracket, matching `shared_terms_computed_once_per_step`'s three-targets-per-zone shape, so the allocation claim and the reuse claim are checked together.

### Finding 3 (Minor, pre-existing, latent): `ThermalSolver::config_mut()` can desynchronize the fixed-size scratch buffers RT-3 introduced

`crates/hares-envelope/src/thermal_solver/mod.rs:438` exposes `pub fn config_mut(&mut self) -> &mut ThermalSolverConfig`, giving external callers a mutable reference to `exterior_surfaces`, `interior_lwr_zones`, and `interior_convection_injections`. RT-3 added `exterior_surface_temps`/`ext_temps_save_buf` (sized once, at construction, to `config.exterior_surfaces.len()`, `mod.rs:981-982`) and restores them with `copy_from_slice` (`stepping.rs:707-711`), which panics on a length mismatch between the two buffers (it would not panic here, since neither buffer is ever resized after construction — but a config mutation that changes `exterior_surfaces.len()` desyncs `config.exterior_surfaces` from `exterior_surface_temps`, and `build_input_vector`'s per-surface loop indexes `exterior_surface_temps` by position, which is an out-of-bounds panic risk if surfaces are added, or silently reads/writes the wrong surface's slot if surfaces are reordered/removed).

Today's only production caller of `config_mut()` (`crates/hares-core/src/dwelling/mod.rs:6536`) only mutates the `ventilation` field, which does not change any buffer's required length, so this is **not currently reachable** from production code. It is pre-existing (the buffer-vs-config coupling predates RT-3; RT-3 only added one more buffer pinned to the same assumption) and is not a regression introduced by these commits. Flagging it because RT-3's design note ("These are the step path's allocation sites... a buffer sized at construction") makes an implicit invariant — config is immutable in the dimensions that matter after construction — that `config_mut()`'s signature does not enforce. **Fix:** either narrow `config_mut()`'s return type to the fields that are safe to mutate post-construction (e.g. a dedicated `ventilation_mut()`), or add a debug-assertion in `prepare_inputs`/`build_input_vector` that `config.exterior_surfaces.len() == exterior_surface_temps.len()` (and similarly for the LWR zone buffers) so a future misuse fails loudly instead of corrupting state.

### Finding 4 (informational, out of scope): one unrelated pre-existing test failure in the worktree

`cargo nextest run --workspace` reports `hares-io::hpxml_parsing_tests pool_and_hot_tub_loads_parsed_from_ochre_fixture` failing with `base-misc-usage-multiplier fixture should be readable: Os { code: 2, kind: NotFound }`. This is a missing test fixture file unrelated to the thermal solver, HPXML parsing rather than envelope/state-space code, and not touched by either commit under review (`git log` on that test file's last few commits predates RT-3/RT-3a). Excluded from the pass/fail counts above; re-ran the full suite with that one test filtered out and got 7130/7130 passing.

## Residual risk / verification gaps

- I did not run the full BESTEST suite outside what `cargo nextest run -p hares-envelope` already includes (`bestest_900ff_root_cause` module, which passed); I relied on the existing BESTEST-derived unit tests plus `frame_golden compare-all`'s fixtures rather than an independent BESTEST case list.
- I did not fuzz the ideal-capacity solves with randomized model matrices beyond the handful of hand-built adversarial cases above (normal, NaN, signed-zero, negative/duplicate coupling, large 40x40). A property-based fuzz comparing old-vs-new across random seeds would increase confidence further but was not run here.
- Finding 1 is a correctness gap in checkpoint-restore semantics that predates both commits; I have not traced every production caller of `restore_state` to determine whether any of them can actually land in the window where the stale coupling state matters (this would require tracing the dwelling-level checkpoint/replay and RL-reset call paths, which are outside `hares-envelope`). I recommend this be looked at as its own ticket rather than folded into RT-3/RT-3a, since it is not something either commit introduced.
