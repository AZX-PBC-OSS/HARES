# Audit: RT-3 (commit 44da330c) and RT-3a (commit 69950a98), the thermal solver's heap-allocation removal and shared-term resolve

Blockers: 0. Majors: 0. Minors: 1 (RT-3, pre-existing, not introduced by either commit). Informational: 1.

Worktree: `.workstrees/audit-69950a98`, head `69950a98`. No code was edited; no destructive git was run.

## Method

Read both ledger entries (`06-runtime-memory.md` RT-3 and RT-3a) in full, including RT-3's
stop-and-add record and stop-rule record. Read both commits' full diffs
(`state_space.rs`, `thermal_solver/mod.rs`, `thermal_solver/stepping.rs`,
`thermal_solver/config.rs`, `lib.rs`, `radiant_gain_weights_zero_alloc.rs`) line by line against
the ledger's "Change" and "Acceptance test" lists. Ran the named tests, the physics suites, the
golden comparisons in all three profiles, clippy, fmt, and `cargo test --workspace` in debug and
release. Checked `02-measurement.md`'s RT-3 and RT-3a sections against the ledger's Measurement
requirement.

## Findings

### Minor (pre-existing, not introduced by RT-3 or RT-3a): `cargo clippy --release --all-targets` fails on an unused import that only the debug/`observe_detailed` test path uses

**Evidence:** `cargo clippy --workspace --release --all-targets -- -D warnings` fails:

```
error: unused imports: `BoundaryDiagnosticInfo` (and, at the pre-RT-3 parent, also `BoundaryCategory`)
  --> crates/hares-envelope/src/thermal_solver/mod.rs:1858:27
```

The only use of `BoundaryDiagnosticInfo::RCNode` in the test module is inside
`rc_node_boundary_diagnostic_uses_tarp_natural_convection`
(`crates/hares-envelope/src/thermal_solver/mod.rs:6611`), which is gated
`#[cfg(any(debug_assertions, feature = "observe_detailed"))]`. `cargo clippy --release` builds
with `debug_assertions` off and no `observe_detailed` feature, so the only call site compiles
out and the import becomes genuinely unused in that one profile/feature combination.

I confirmed this is pre-existing: `cargo clippy -p hares-envelope --release --all-targets -- -D
warnings` in `.workstrees/rt3-parent` (commit `7b7a6a7d`, RT-3's own stated parent) fails the
same way, flagging both `BoundaryCategory` and `BoundaryDiagnosticInfo` unused at the same import
line. RT-3's own test `coupled_solve_divisor_checked_at_construction` added an un-gated use of
`BoundaryCategory::Wall`, which is why `BoundaryCategory` stopped being flagged after RT-3;
`BoundaryDiagnosticInfo` was left as-is and still only has the gated use site.

This is **not** a failure of the canonical verification line. `plan.md` §3's `[clippy]` line is
`cargo clippy --workspace --all-targets --all-features -- -D warnings` (dev profile, not
`--release`), which enables `debug_assertions` and every feature including `observe_detailed`;
I ran that exact line and it passes clean. The `--release`-profile failure is a narrower
combination plan.md's block does not require. I report it because the audit task asked for
clippy `-D warnings` to be checked broadly, and because a release clippy run is one plausible
reading of "every build profile" in the surrounding initiative's standing rule
(`feedback_ochre_parity_rules`/OR-11's precedent of enforcing checks in every profile). Neither
RT-3 nor RT-3a caused it and neither claims responsibility for it; no fix is required of this
pair of commits.

**Fix (optional, not blocking either commit):** move `BoundaryDiagnosticInfo::RCNode` or add a
second, un-gated test use, or gate the import itself with the same `cfg` as its only user.

### Informational: RT-3a's per-call-count measurement (23,072 / 11,968 solves) was not independently reproduced

**Evidence:** `02-measurement.md`'s RT-3a section states "On `consumer_shape_900s` the fixture's
35,040 steps run 23,072 single solves and 11,968 double solves." This audit did not add
instrumentation to independently recount these; it verified the mechanism that would produce
such a split (one prefix fill followed by N tails, counted by the test-only
`shared_prefix_fills`/`solve_tail_calls` counters) is real and exercised by
`shared_terms_computed_once_per_step`, and that the full-year allocation count claim (bit-
identical to RT-3's 7,957,954) is consistent with RT-3a touching no allocation site. The step-
count split itself is accepted as recorded, not re-derived; a cheap reproduction would be to run
the fixture once with the test counters compiled in (`#[cfg(test)]` currently restricts them to
test builds) and print the totals, which this audit did not do since it would require a non-test
build change. This is a gap in independent verification, not a defect.

## Requirement-by-requirement check

### RT-3 ("Change" list)

1. Persistent `ext_temps_save_buf: Vec<f64>`, sized at construction
   (`thermal_solver/mod.rs:978`), saved/restored with `copy_from_slice` in
   `prepare_inputs_inner` (`stepping.rs:675-679`) — done, matches.
2. `prepare_inputs_inner` returns the latent map to `self.latent_buf`
   (`stepping.rs:726`, `self.latent_buf = latent;`) — done.
3. `last_u` sized at construction (`mod.rs:965`), updated with `copy_from` at both call sites
   (`stepping.rs:688`, `:1174`) instead of `clone_from` — done.
4. `output_into(x, u, y, du_scratch)` added (`state_space.rs:401-117`), computing `C·x` into
   `y` and `D·u` into `du_scratch` via `mul_to`, summed elementwise, same operation order as the
   allocating form — done; `integrate_inner` uses a swapped-out `y_buf`/`du_buf` pair
   (`stepping.rs:730-732, 1177`) instead of calling `model.output`.
5. Solar slot map rebuilt only when the surface-id sequence changes, compared id by id
   (`thermal_solver/mod.rs:1689-1697`, `solar_slot_map_keys` sequence) — done, with its own test
   `solar_slot_map_rebuilt_only_on_surface_change`.
6. `SolveScratch` (`n_x`, `b_u`, `rhs`, `gain`, `d_agg`) owned by `ThermalSolver`, sized to state
   dimension at construction (`state_space.rs:25-51`, `mod.rs:970`) — done. Both scalar solves
   use `mul_to` into scratch, same operand order, identity-coupled solve copies the gain column
   with `copy_from` and divides in place, uncoupled solve reads the column view directly — all
   confirmed in the diff and matching the spec's description word for word.
   Autosizing/design-day callers pass a local `SolveScratch` (`stepping.rs:1271-1273`) — done.
7. Stop-and-add: the three additional allocation sites
   (`ext_surface_diag_buf`/`int_surface_diag_buf`/`window_solar_diag_buf` clones and the
   capacity-0 `infiltration_by_zone_buf`/`lwr_by_zone_buf` round trip) are fixed by moving the
   buffers through the struct replacement instead of cloning/swapping against a fresh
   `Vec::new()` (`mod.rs:1489-1519, 1585-1601`) — done, matches the stop-and-add record
   description exactly.
8. Stability-check comment rewrite (`state_space.rs:60-66, 73-81`): the "CN implicit path"
   language is gone, replaced with the ZOH-maps-to-unity-eigenvalue / explicit-step-bounded
   justification the ledger specifies, with no em dash in the new text — done.
9. `1 + d_i` divisor construction check: every coupling site's `B_eff` coefficient checked finite
   and non-negative at `ThermalSolver::new` (`mod.rs:840-874`, zone and exterior-LWR sites), plus
   the TARP interior-convection injection's area/film-resistance/tilt checked finite and
   non-negative (`mod.rs:883-910`), each naming the state and returning a typed
   `ThermalSolverError::CouplingCoefficientInvalid` / `ConvectionInjectionInvalid`
   (`config.rs:811-847`) — done, with `coupled_solve_divisor_checked_at_construction` covering
   both a zone site and a surface site, zero/negative/non-finite, and a healthy-model pass.
10. The "Zero allocation" doc-comment claim on `solve_ideal_capacity_for_target` is rewritten to
    state what the test proves (`stepping.rs:359-371`) — done.
11. Factorization count: unchanged by this commit's diff (no factorization call sites touched);
    the zero-allocation test's "therefore zero factorizations" claim follows from the stated
    fact that every nalgebra factorization allocates, which still holds.

### RT-3 ("Acceptance test" list)

- `coupled_solve_divisor_checked_at_construction` — present, passes (verified by running
  `cargo nextest run -p hares-envelope --release`).
- `thermal_solver_step_allocation_free_after_first_step` replaces the old 11/step-bound test,
  brackets `prepare_inputs` → per-zone `solve_ideal_capacity_for_target` → `integrate`, asserts
  exactly 0 over 100 steps after a warm-up step, runs both with couplings active and without,
  and separately brackets 100 `ThermalSolver::resolve` calls — present, passes. Header and
  assertion text rewritten, no "ticket 040" references remain anywhere in the tree
  (`git grep -n "ticket 040\|ticket_040"` returns nothing).
- `solar_slot_map_rebuilt_only_on_surface_change` — present, passes.
- Goldens bitwise equal — reran `frame_golden compare-all` in default, `-F dst`, `-F profiling`:
  all 5+1+5 fixtures "identical" at the current (RT-3a) head (see Verification re-run below;
  RT-3's own commit was not re-checked in isolation since RT-3a's head includes it and the
  ledger's own log already records this at `44da330c`).

### RT-3a ("Change" list)

1. `SolveScratch` split into shared prefix (`n_x`, `b_u`, `rhs`) and per-call tail region
   (`tail_rhs`, `gain`, `d_agg`) — done (`state_space.rs:116-153`).
2. `fill_shared_prefix` computes `N·x` and `B_eff·u` and their sum once
   (`state_space.rs:725-745`); `solve_identity_coupled_tail` and `solve_uncoupled_tail` read the
   shared `rhs` into a per-call `tail_rhs` copy and run only the target-dependent remainder (the
   `b_col·u_i` subtraction, couplings, `d_agg` aggregation, gain division, 1×1 products) — done,
   and the full solves (`solve_for_scalar_input_identity_coupled`, `solve_for_scalar_input`) are
   kept as thin wrappers (`fill_shared_prefix` + tail) so single-call correctness is unchanged.
3. `ThermalSolver` tracks `shared_prefix_valid: bool` and `shared_prefix_variant: CoupledState`
   (`mod.rs:113-116`); `solve_ideal_capacity_for_target` fills the prefix only when invalid or
   the variant differs from `last_coupled_state`, else reuses it (`stepping.rs:399-412`) — done.
4. Invalidation: `invalidate_shared_prefix` is called at the top of `prepare_inputs_inner`
   (`stepping.rs:705`), at the top of `integrate_inner` (`stepping.rs:740`), and in
   `restore_state` before `self.x`/`self.last_u` are overwritten (`mod.rs:1331-1338`). I checked
   every assignment site of `self.x` and `self.last_u` in `hares-envelope`
   (`grep -n "self\.x\b.*=" `, `last_u\.copy_from\|last_u ="`): the only three mutation paths are
   exactly these three functions, and each invalidates before its first ideal-capacity-solve-
   reachable mutation. No path mutates `x`/`last_u` without invalidating.
5. Equipment add/remove and setpoint-target changes do not need their own invalidation path: the
   shared prefix depends only on `x` and `last_u` (zone/equipment-independent); the per-target
   and per-equipment differences live entirely in the tail (`output_index`, `input_index`,
   `y_target`, `couplings`), confirmed by reading `collect_and_solve`
   (`crates/hares-core/src/actors/solver_feedback.rs:53-66`), which calls
   `solve_ideal_capacity_for_target` once per equipment with an ideal target against the same
   step's solver state — matches the ledger's stated caller exactly.
6. Autosizing/design-day callers (`stepping.rs:1271-1273, 1413-1420`) call the full
   `solve_for_output_input` with a local `SolveScratch`, bypassing the shared-prefix fields on
   `self` entirely — correct per spec ("the autosizing and design-day callers pass local
   scratches and keep their own sequencing").
7. Bitwise identity: every tail path performs the identical sequence of operations the un-split
   solve performed, confirmed by direct diff comparison (the `fill_shared_prefix` body is
   byte-for-byte what RT-3's inline prefix computation did; the tail bodies are byte-for-byte
   what remained after removing the prefix lines) — matches "the prefix is the same numbers the
   repeated calls computed."

### RT-3a ("Acceptance test")

- Zero-allocation test still reads zero: reran `thermal_solver_step_allocation_free_after_first_step`
  at the RT-3a head — passes (the RT-3a diff does not add any new scratch field that isn't sized
  at construction; `tail_rhs` is sized in `SolveScratch::new`).
- `shared_terms_computed_once_per_step` — present (`mod.rs:7589-7756`), passes. It is more
  thorough than the ledger's minimum description: besides the one-fill/three-tail count and the
  bitwise A/B against a full-solve-with-local-scratch reference, it separately exercises and
  re-asserts exactly one refill at each of the three invalidation windows (post-`integrate`,
  post-`prepare_inputs` with changed weather, post-`restore_state` with a different snapshot),
  for both the coupled and uncoupled paths. This directly answers the audit's invalidation
  concern (setpoint/zone/coupling/equipment changes, and the three mutation paths) with a
  passing, bitwise-checked test rather than only a structural read of the invalidation call
  sites.

## Verification re-run (at `69950a98`, in `.workstrees/audit-69950a98`)

| Check | Command | Result |
|---|---|---|
| hares-envelope unit+integration tests | `cargo nextest run -p hares-envelope --release` | 438/438 passed, including the three tests named above |
| ASHRAE RC reference | `bash scripts/check_ashrae_reference.sh` (after `uv sync --extra ochre` to provide `xmltodict`, absent in the worktree's fresh `.venv`) | PASS |
| hares-core oracles | `cargo nextest run -p hares-core --release --test bestest --test parity --test envelope_oracle --test structural_envelope_oracle` | 25/25 passed |
| hares-physics | `cargo nextest run -p hares-physics --release --test physics_validation_tests --test solar_parity` | 53/53 passed |
| frame_golden compare-all, default | `cargo run --release -p frame_golden -- compare-all` | 5/5 identical |
| frame_golden compare-all, `-F dst` | `cargo run --release -p frame_golden -F dst -- compare-all` | 1/1 identical |
| frame_golden compare-all, `-F profiling` | `cargo run --release -p frame_golden -F profiling -- compare-all` | 5/5 identical |
| `cargo test --workspace` (debug) | plain `cargo test --workspace` | all crates ok, 0 failed |
| `cargo nextest run --workspace` (debug) | `cargo nextest run --workspace --no-fail-fast` | 7130/7130 passed |
| `cargo nextest run --workspace --release` | `cargo nextest run --workspace --release --no-fail-fast` | 7087/7087 passed |
| clippy, canonical `[clippy]` line | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| clippy, `--release` (not the canonical line) | `cargo clippy --workspace --release --all-targets -- -D warnings` | fails, pre-existing (see Minor finding) |
| fmt | `cargo fmt --check --all` | clean |

`frame_golden` and the test binaries were built fresh in this worktree; nothing was built in
`/home/rich/src/HARES` itself.

## Quality check

- DRY: `fill_shared_prefix` is the single place the shared prefix is computed; both tails and
  both legacy full-solve wrappers route through it. No duplicated prefix-computation logic found.
- Dead code: no `#[allow(dead_code)]` or `#[allow(unused)]` added by either commit (`git show
  44da330c 69950a98 | grep "^+" | grep -i "allow(dead_code\|allow(unused"` empty). The old
  `SolverScratch` type the ledger says SL-7 deletes is confirmed absent
  (`grep -rn "struct SolverScratch\b" crates/` empty).
- Scratch buffers sized at construction: `SolveScratch::new`, `y_buf`, `du_buf`,
  `ext_temps_save_buf`, `solar_slot_map_keys` (via `with_capacity`) are all sized in
  `ThermalSolver::new` — confirmed by reading the constructor.
- Comments: no em dash in any line added by either commit (`git show 44da330c 69950a98 | grep
  "^+" | grep "—"` empty; the em dashes that do appear in the diff context are all pre-existing
  unmodified lines). No ticket IDs, ledger IDs ("RT-3", "RT-3a", "SL-7", "OR-11"), or agent/LLM
  references in any added line. No "ticket 040" references remain anywhere in the tree.

## Conclusion

Both commits implement their ledger entries' requirements in full; no requirement was silently
dropped and no deviation was found beyond the one pre-existing, unrelated clippy issue noted
above. Physics is unchanged (bitwise goldens in all three profiles, full physics suites passing,
counts matching the ledger's own log: 25/25 hares-core oracle tests, 53/53 hares-physics tests).
The zero-allocation guarantee holds with couplings active and inactive, bracketing the full
per-step sequence. RT-3a's shared-term caching invalidates correctly on every input-changing
path (verified both structurally and by the bitwise `shared_terms_computed_once_per_step` test
exercising all three invalidation windows) and produces numerically identical results to the
per-call path. The performance claim is recorded in `02-measurement.md` for both rows, consistent
with the stated unavailability of `perf` on this host and the use of profiling-phase shares and
A/B wall-clock measurement as the documented fallback; RT-3a's own step-count breakdown
(23,072/11,968) was accepted as recorded rather than independently re-measured (informational
gap, not a defect).

**0 blockers, 0 majors.**
