# Audit: SL-7, commit 7b7a6a7d (remove the dead coupled-LU path and identity solves)

Blockers: 0. Majors: 1. Minors: 1.

## Findings

### Major: the SL-7 ledger entry's own required Measurement was never recorded

**Evidence:** `plan.md` SL-7's row (`.shipwright/initiatives/I-08/plan.md:360-361`) states
`**Measurement:** share of \`simulate()\` in \`step_into\` and \`solve_for_output_input\`
before and after.` The corresponding log section,
`.shipwright/initiatives/I-08/ledger/02-measurement.md:854-867` ("SL-7, 2026-10-02, commit
7b7a6a7d"), records net line count, bitwise-identical goldens, the zero-sign argument, and the
oracle-test flake count, but contains no wall-time or phase-share figure for `step_into` or
`solve_for_output_input`, before or after. No other section of `02-measurement.md` carries this
number under an SL-7 heading either (checked via `grep -n "share of\|simulate()"`). The
commit's acceptance criteria (bitwise goldens, oracle tests) are met, so this is not a
correctness defect in the removed code, but the entry's own measurement commitment is unmet:
the claimed performance motivation for the removal (avoiding O(n^3) LU work on the hot
`step_into`/`solve_for_output_input` paths) is asserted, never quantified.

**Fix:** add a same-session A/B of `step_into` and `solve_for_output_input` share of
`simulate()` (or of the relevant profiling phase) before/after this commit to
`02-measurement.md`'s SL-7 section, following the pattern already used for OR-5
(`02-measurement.md:636-647`). Since the removed arm was provably unreachable, a worktree of
the parent commit `afa9a071` vs this commit is sufficient; no new fixture is needed.

### Minor: stale "CN implicit path" comment predates and survives this commit unexamined

**Evidence:** `crates/hares-envelope/src/state_space.rs:271` (unchanged by this diff) reads
"but CN implicit path remains well-behaved" inside `from_continuous`'s Gershgorin-exemption
comment. This is pre-existing (present at `7b7a6a7d^` too, confirmed by
`git show 7b7a6a7d^:crates/hares-envelope/src/state_space.rs | grep -n "CN implicit"`) and is
not itself part of SL-7's claimed scope (the ledger's evidence list does not mention it), so it
is not a defect introduced by this commit. It is flagged only because the commit's own doc
comment nearby (`:248-249`) was rewritten in the same diff to drop the stale "M = I" language,
making this leftover "CN implicit path" phrase (there is no implicit/coupled-LU path left to
call "CN implicit" after couplings became purely explicit/closed-form) a missed opportunity in
an otherwise careful documentation pass over the same function.

**Fix:** reword to describe what the exemption actually protects (the singular-A_c ZOH pole at
|lambda|=1 under the closed-form coupling scheme), dropping "CN implicit path." Low priority;
does not block SL-7.

## Verification performed

1. **Unreachability of the removed path (requirement 1).** Confirmed structurally, not just by
   absence of callers: `StateSpaceModel` no longer has an `m_mat`/`m_lu`/`m_is_identity` field
   at all (`state_space.rs:82-92`); both constructors, `from_discrete`
   (`state_space.rs:150-172`) and `from_continuous` (`:250-338`), build only `n_mat`/`b_eff`.
   There is structurally no way to produce a non-identity implicit matrix post-commit.
   `grep -rn 'm_lu|m_is_identity|build_coupled_lu|CoupledState::LU|m_scratch|CouplingData|SolverScratch|SolveTarget|_input_into|coupled_into' crates/hares-envelope`
   returns nothing (exit code 1), matching SL-7's acceptance test exactly.
   `grep -rn 'StateSpaceModel|CouplingData|SolverScratch|SolveTarget' crates/hares-python/src/`
   returns nothing; `cargo check --workspace --all-features` succeeds, confirming no
   out-of-crate caller (bindings, benches, tests) references the removed symbols.

2. **SL-7 requirements and non-regression (requirement 2).**
   - `step_into` (`state_space.rs:345-348`) now computes `N*x + B_eff*u` directly with no
     solve; `verify_stability` (`:365-374`) takes `self.n_mat.complex_eigenvalues()` directly,
     matching the spec's "`verify_stability` takes the eigenvalues of N."
   - `verify_stability`'s only callers in the crate are two unit tests
     (`state_space.rs:1322`, `:1336`); confirmed by
     `grep -rn "verify_stability(" crates/hares-envelope/src crates/*/src` across the
     workspace returning only those two call sites plus the definition, matching the entry's
     "the discarded verdict's sole caller is a test."
   - Every symbol SL-7 said to remove is gone: `build_coupled_lu`,
     `step_with_coupled_lu_into[_with_forcing]`, `solve_for_scalar_input_coupled[_into]`,
     `CouplingData`, `CoupledState::LU` and its three call-site arms
     (`thermal_solver/stepping.rs` diff), the `m_is_identity()` branch in the energy-balance
     code (replaced by a single `else` since only two `CoupledState` variants remain),
     `ThermalSolver.m_scratch`, `solve_for_output_input_into`/`SolverScratch`/`SolveTarget` and
     their `lib.rs` re-export, the vacuous `debug_assert`s, the two `step_into_with_forcing`
     variants (`#[allow(dead_code)]`), the stale "coupled LU is cached" doc sentence
     (`stepping.rs` diff, now "Zero allocation." only), and the bench's
     `bench_coupled_step_lu`/its `criterion_group!` entry plus the `m_is_identity()` assert in
     `bench_coupled_step_identity`.
   - Nothing SL-7 said to keep was deleted: `step_with_identity_coupling_into[_scratch]`,
     `build_coupled_rhs`, `solve_for_scalar_input_identity_coupled`,
     `solve_for_output_input`/`solve_for_input`, and the five oracle tests all remain and pass.
   - Oracle soundness: before this commit, the five oracle tests built their "ground truth" by
     calling `model.build_coupled_lu(...)`, part of the very production code path being
     removed, factoring the identity-plus-couplings matrix through production code rather than
     an independent implementation (confirmed via
     `git show 7b7a6a7d^:crates/hares-envelope/src/state_space.rs | grep -n "build_coupled_lu("`,
     6 call sites inside tests). This commit replaces that with a test-local
     `factor_identity_plus_couplings`/`reference_coupled_scalar_solve` (`state_space.rs:1027-1073`)
     that builds `DMatrix::identity(n,n)` and adds the coupling diagonal itself, independent of
     any production method, then LU-factors and solves it. This is a strict improvement in
     oracle independence (removes the prior tautology risk) and is not itself tautological: the
     closed-form production path divides by `1+d_i` directly, the oracle solves a full `(I+D)`
     linear system via `nalgebra`'s `LU`, a different code path computing the same math.
   - Term order of the identity-solve replacements is unchanged: e.g. `solve_for_scalar_input`
     previously did `x_next_fixed = m_lu.solve(rhs_fixed)` (LU-of-identity, a no-op) then used
     `x_next_fixed`; now `rhs_fixed` is used directly in the same position with the same sign
     conventions (verified in the diff, `state_space.rs` around the old line 594-610).

3. **Tests and goldens (requirement 3).** All run in the worktree
   `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` at `7b7a6a7d`:
   - `cargo nextest run -p hares-envelope --no-fail-fast`: 438 tests run, 438 passed, 0 failed.
   - `cargo nextest run -p hares-envelope --release --no-fail-fast`: 435 tests run, 435 passed,
     0 failed. The 3-test gap vs. debug (`boundary_rc::tests::missing_surface_layer_node_asserts`,
     `humidity_solver::tests::humidity_ratio_missing_zone_panics_in_debug`,
     `thermal_solver::tests::rc_node_boundary_diagnostic_uses_tarp_natural_convection`) is
     `#[cfg(any(debug_assertions, feature = "observe_detailed"))]`-gated and pre-exists this
     commit (confirmed the gate is untouched by `git show 7b7a6a7d` on
     `thermal_solver/mod.rs`); not a regression.
   - `cargo run --release -p frame_golden -- compare-all`: `consumer_shape_900s`,
     `fleet_mixed_offset`, `full_year_hourly`, `leap_year_february_900s`, `weather_wrap_900s`
     all report `identical`.
   - `cargo run --release -p frame_golden -F dst -- compare-all`: `full_year_900s_dst` reports
     `identical`.
   - `cargo bench -p hares-envelope --no-run`: compiles clean (one pre-existing, unrelated
     unused-import warning under this feature/profile combination from the
     `#[cfg(any(debug_assertions, feature = "observe_detailed"))]`-gated test noted above;
     confirmed pre-existing by checking the gate and import were both present and unchanged in
     `7b7a6a7d^`, i.e. not introduced by this commit).
   - `cargo clippy -p hares-envelope --all-targets --all-features -- -D warnings`: clean.
   - `cargo check --workspace --all-features`: clean.
   - Did not run the consumer repo's B0 check (`~/src/HARES_CALIBRATION_SYSTEM`) or the
     `ashrae-reference`/bestest/parity suites named in plan.md §3; these require the second
     repository and/or longer offline data and were out of reach in this pass. This is a
     testing gap, not a pass/fail claim: the measurement log's SL-7 entry asserts "the
     consumer's B0 identical" but I did not independently verify that claim.

4. **Performance (requirement 4).** `ThermalSolver.m_scratch` (the field, its construction, and
   every `build_coupled_lu(&mut self.m_scratch, ...)` call) is removed by this commit
   (`thermal_solver/mod.rs` diff lines around `:63-93`, `:855-987`), exactly as SL-7 specifies.
   The other allocations named in `06-runtime-memory.md` RT-3: `exterior_surface_temps.clone()`
   (`stepping.rs:653`), the dropped latent map from `build_input_vector`
   (`stepping.rs:654`/`mod.rs:1203`), `self.last_u.clone_from(&u)`, `model.output()`'s two-vector
   build, the two solves inside `solve_ideal_capacity_for_target`, and
   `refresh_solar_slot_map` rebuilding its `HashMap` twice per step (`mod.rs:583`, `:1217`);
   all are still present post-commit, confirmed by direct grep of current line numbers above.
   This is correct and intentional: SL-7's own ledger entry text says only `m_scratch` and the
   coupled-LU factorization are in its scope, and `plan.md` §3 names RT-3 as the entry that
   lands after SL-7 and owns the rest. RT-3's problem section explicitly labels its line numbers
   "at afa9a071" (the pre-SL-7 commit) and separately states "The coupled LU factorization and
   `m_scratch` are dead code removed by SL-7" (`06-runtime-memory.md:371-372`) in the future
   tense relative to its own unlanded state; this is accurate as written and not stale,
   since RT-3 has not yet landed and its own text already accounts for SL-7's effect.

5. **Quality (requirement 5).**
   - No leftover fields/parameters that only the removed path used: `last_coupling`,
     `last_coupled_state`, `d_agg_buf`, `balance_buf_a/b/c` are all still read by the surviving
     `CoupledState::Identity`/`Uncoupled` arms (grep confirmed each use site in
     `stepping.rs`/`mod.rs`).
   - No duplicated logic introduced; the `else if self.model.m_is_identity() { ... } else { ... }`
     three-way branches collapse correctly to two-way `if`/`else` with the surviving arm's body
     unchanged, in both `prepare_inputs_inner`/`integrate_inner` and the energy-balance
     decomposition in `stepping.rs`.
   - `benches/rc_solver.rs`: `bench_coupled_step_lu` and its `criterion_group!` entry are gone;
     `bench_coupled_step_identity` still measures the real (now the only) coupled-step path and
     drops the now-meaningless `assert!(model.m_is_identity())`. The bench still measures
     something meaningful (the closed-form coupled step), consistent with the spec.
   - Comments: the diff removes stale claims ("M is caller-supplied," "the coupled LU is
     cached," ticket reference "T-0034" in the `convection_forcing` doc comment) and does not
     introduce new ticket IDs or restated-code comments. All em dashes touched by this diff are
     removed, not added (`git show 7b7a6a7d | grep "^+" | grep "—"` returns nothing; all four
     em-dash instances found are on removed `-` lines).

## Scope note

This audit covers `hares-envelope` only, per the entry's own stated scope and
`dl-i08-process.scope`'s authorization list in `plan.md` §3. I did not re-verify other rows'
unrelated claims (M-1/M-2/BR-6x/etc.) beyond what was needed to establish SL-7's baseline.
