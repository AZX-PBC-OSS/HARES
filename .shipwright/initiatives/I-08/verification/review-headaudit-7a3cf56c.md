# Review: fix-headaudit (6a499504..7a3cf56c on 249912f9)

Independent review, 2026-10-05. Branch worktree `.workstrees/fix-headaudit`
(read-only use), CARGO_TARGET_DIR `target-review-headaudit` (plus
`target-review-headaudit-drift` for the drift check), both deleted at the end.

## Verdict: CHANGES-REQUIRED

No high finding. The code changes are behaviour-neutral and the Python and CI
changes work. Two mediums keep it from SHIP: audit finding M3 is not fixed at
its root (three xfails still carry no triage class, no cause and no owner), and
the new CI job can go green with every test skipped. The zone-ordering mutation
run was not executed (see "Not verified").

## Findings

### Medium

**R1. M3 is not fixed at the root: three thermal-trace xfails have no triage
class, cause or owner.**
`tests/python/test_thermal_trace.py:345-355, 459-465, 520-526`. The reasons now
state measured values and are drift-checked, but each is only "HARES diverges
from OCHRE". `decision-log.md` `ochre_deviation_triage` (line 576) requires every
deviation to land in exactly one class (a: OCHRE wrong, cited at file:line and
recorded in `docs/alignment/DIVERGENCES.md`; b/c: fix HARES; d: fix the harness)
and says a deviation without an entry and a pinning test is not done. A 4.7 C
overnight divergence and a 30% window-solar overstatement are class b/c/d
candidates, not class a, and a strict xfail is only the permitted record for
class a. `grep` of `docs/alignment/DIVERGENCES.md` for thermal trace, window
solar and overnight: no hit. The implementer's write-up admits "No ledger entry
owns the three divergences". The audit's M3 asked for exactly this ownership;
the fix replaced an unsupported claim with a measured one and left the defect
unowned. Fix: open a numeric-block or other-defects entry per divergence, state
its class, and cite it in the reason (without a ticket ID in shipped text, name
the cause instead, for example "window transmitted solar gain uses X where
EnergyPlus uses Y"). If the class cannot yet be established, the reason must say
"cause not yet triaged" rather than imply an accepted divergence.
Confidence: high (read the rule, the files, grepped).

**R2. The new `ochre-python-test` job can pass with every OCHRE test skipped.**
`.github/workflows/ci.yml` (job `ochre-python-test`, step
`uv run pytest -m ochre -rxX`), with `tests/python/test_ochre_parity.py:135`,
`test_thermal_trace.py:51` and `test_ashrae_reference.py:23` using
`pytest.importorskip`. If `import ochre` fails (ImportError from a missing
transitive such as xmltodict, a pint/pvlib break, a submodule at the wrong
revision), pytest reports module-level skips and exits 0. That is exactly the
"pint release breaks OCHRE, passes unseen" harm the audit's M1 named; the old
zero-SKIPPED census gate lives only in the operator's local block, not in CI.
Reproduction path: reasoned from the code, not run. Fix: make the job fail on
any skip (a `conftest` option or `-W error::pytest.PytestUnraisableExceptionWarning`
is not enough; parse the junit XML for `skipped` and exit 1, or replace
`importorskip` in `ochre`-marked modules with a plain import, since the job
installs the group). Confidence: medium-high.

### Low

**R3. The 2% record margin is exclusive of the edge.** `test_thermal_trace.py:326`
(`RECORD_REL_MARGIN`) and `_check_record` use `>`; a record that is wrong by 2.0%
passes. Reproduced the failing side only: setting `OVERNIGHT_MEAN_C = 0.8776*1.025`
(scratch copy, not the branch) made `test_overnight_losses` FAIL, not XFAIL
(`1 failed`). The brief's "fails on 2% drift" holds for drift above 2%. Also the
records are 4 digits and the margin is wide enough that a real 1.5% change in
heating (a regression of the kind the audit worried about) passes. Acceptable;
mention only.

**R4. Part of the 48 h xfail reason is a hard-coded claim no test checks.**
`test_thermal_trace.py:351-352`: "The first component to diverge by more than 5%
is the infiltration heat gain, at step 1". It comes from the unseeded 02-measurement
run (+8.7%); no record or assertion covers it, so it can go stale silently, which
is the failure mode the records were added to prevent. Fix: compute and assert it
or drop the sentence.

**R5. Acceptance grep prints 30 lines, not 29.**
`git grep -nE '\b(allow|expect)\(' -- '*.rs' ':!vendors' | grep -v '\.expect('`
returns 30: the 29 kept attributes (22 pyo3 `too_many_arguments`, 2 panic_hook,
2 schedule enums, 1 `approx_constant`, 2 shared test helpers; every one has a
one-line `//` reason on the adjacent line, verified for all 22 pyo3 sites and the
7 others) plus a pre-existing comment,
`crates/hares-core/tests/autosize_from_config.rs:238`, containing the text
`expect()`. The ledger says the grep "lists only the kept suppressions". Zero
`#[expect(` and zero `cfg_attr(..., allow(` remain. Fix: tighten the grep to
`'^\s*#!?\[.*\b(allow|expect)\('`, which gives exactly 29.

**R6. No test pins the diagnostics scratch move or the longwave diagnostic push.**
`crates/hares-core/src/diagnostics.rs:497-558` (`mem::take` of a `StepScratch`
and back) and `crates/hares-envelope/src/thermal_solver/longwave.rs:652-664`
(push moved to its own gated loop). Reading shows both are equivalent
(`Vec::new()` does not allocate, so take/restore moves handles only; the loop has
no `continue`, pushes in the same order, bounded by `surfaces.len()`), and the
allocation tests pass (below), but `profiling_alloc.rs` does not exercise
`record_from_state`, and nothing asserts `int_surface_diag_buf` contents outside
the observe-only `conditioned_oracle` dumps. If `record_step` panics, the taken
scratch is dropped and later steps run with empty buffers (`nz = 0`): benign, panic
path only. Residual test gap, not a defect.

**R7. One en dash in an added line.** `crates/hares-core/src/dwelling/mod.rs:5645`
(`// Steps 1e–1f:`); it is the pre-existing text re-added when the comment was
rewritten. No em dashes, no ticket IDs, no agent references in added lines or in
any of the 15 commit messages (grep over `git log` and `git diff`). Fix: use
"1e to 1f".

## Verified (no finding)

1. Audit findings. M1 (CI job + marker), M2 (`test_parity_columns_present`, a
   missing column raises `LookupError`, which `raises=AssertionError` does not
   swallow), M3b (already `OD-47`), M4 (`save_versioned` deleted), M5, L1, L2,
   L4, L6-L14, BR-21 L1/L2: fixed as described. L3 and L5: see items 7 and note
   below. M3: see R1.
   **L15: the declaration is correct.** The local std source
   (`~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/num/uint_macros.rs`,
   `pub const fn is_multiple_of`) carries `#[stable(feature = "unsigned_is_multiple_of", since = "1.87.0")]`
   and the matching `rustc_const_stable`; `Cargo.toml:30` is `rust-version = "1.89"`.
2. CI (`.github/workflows/ci.yml`). Triggers are push and pull_request on
   `main`/`initial-implementation` plus dispatch. `ochre-python-test` runs on all
   three (no `if`), `needs: [test]`, initialises `vendors/OCHRE`, syncs
   `--group ochre`, builds `maturin develop --release`, runs `pytest -m ochre`. A
   command-line `-m` overrides `addopts`'s `-m 'not slow and not ochre'`:
   locally `-m ochre` selected 54 tests (50 passed, 4 xfailed), including the
   `slow`-marked thermal-trace tests, with the default `-n auto` and with `-n 0`.
   The default selector `-m "not slow and not ochre"` ran 1323 passed, 0 skipped
   (the fleet conversion tests are in it); the slow job is now `slow and not
   ochre`. Every test is selected by at least one job: ochre-marked tests by the
   ochre job; slow-only tests by the dispatch-only slow job (pre-existing; they
   never run on push or pull request). Artifact names are unique per matrix leg.
   Caveat: R2.
3. Seeding. Both modules pass `seed=OCHRE_SEED = 42` (`test_ochre_parity.py:74`,
   `test_thermal_trace.py:248`). Two consecutive runs of both modules with `-s`
   produced identical output apart from the timestamps (diff shows only
   `YYYY-MM-DD HH:MM:SS` log lines), including every printed per-step table and
   the full-precision parity pair in the xfail reason. xfail reasons are
   f-strings built from the records, cite no ticket IDs, and the Lighting reason
   matches `ledger/11-other-defects.md` OD-47 (measured pair identical). Class:
   Lighting is a HARES-side defect with an owner (OD-47); the thermal trace three
   are R1.
4. Tolerances. 1e-9 relative / 1e-12 kWh absolute: parity 11 passed + 1 xfail
   under the release extension build and again under the debug extension build
   (`maturin develop`), and the thermal records (2%) also held under both. The
   compared hour has HVAC heating and water heating at exactly 0 and the rest are
   schedule sums, so 1e-9 against a measured 1.8e-15 leaves six orders of room
   for platform and summation-order differences. Sound; the only brittleness is
   the `o_val == 0.0` exact comparison, acceptable because OCHRE's zero is
   exact here.
5. Removals. `git grep` for `save_versioned` (excluding `try_`) in the worktree:
   none (Rust, Python, pyi, docs). In `~/src/HARES_CALIBRATION_SYSTEM`: no caller
   outside its own vendored `HARES/` snapshot, where only the definition and
   re-export exist (it is a submodule on a different revision and unaffected).
   `zone_state_ordering` passes (2 tests) and asserts a real property: per step the
   Independent-stage stub's view equals `latest_env()` zone temperatures from
   before the step (bitwise), and the run must move the zone temperature. The
   mutation run was not performed (see "Not verified"). Goldens: `frame_golden
   compare-all` 6/6 identical and `-F dst` 1/1 identical on the release build.
   Allocation checks: `synthetic_box_zero_alloc` and
   `radiant_gain_weights_zero_alloc` pass, `profiling_alloc` (observe+profiling)
   2 passed, `hares-core -F observe diagnostics` 4 passed (but see R6). The
   `#[expect(` removals read as behaviour-preserving: `scheduler.build()` is
   `#[must_use] fn build(&mut self) -> &[PlanEntry]` and returns the cached plan
   when not dirty, so iterating it instead of `.plan()` after a discarded
   `build()` is the same plan (`scheduler.rs:186-190`); the dead capacitance field
   had no reader; `ScalarSolveTarget` is a `Copy` struct with identical field
   destructuring.
6. L10 (single `match`, Err arm returns the typed error, the debug log reads the
   RNG position before the restore as before), L11 (`garage_wall_height_m`
   extracted and asserted, 21 garage tests pass), L12 (both doc links, second one
   found in the same file), L13 (comments), L14: `PvRoofSiting` has no panel
   fields, so the override is impossible by construction; every caller is migrated
   (`py_dwelling.rs:1844`, four tests); `PvRoofTuning` remains only where panel
   fields are meaningful (`py_pv_sizing.rs:495, 547, 740, 800`). The ashrae152
   test-only `state` field is now `#[cfg(test)]` on both the field and the single
   literal; its doc says MB for Winnipeg, matching the test at `ashrae152.rs:1244`
   (the old comment said MN, which was wrong).
7. L3. Remaining skips are `gymnasium` (`test_gym_env.py:13`,
   `test_rl_warning_surfacing.py:105`; the `rl` extra), HELICS (`importorskip`/`skipif`
   in the helics modules; the default job installs `--extra helics`), and the
   ochre group (`importorskip`, see R2). The directly imported packages
   (`numpy`, `polars`, `pyarrow`, `ochre_next`) are hard dependencies in
   `pyproject.toml` (lines 10-13), so no test errors out where a dependency is
   genuinely optional. EV tests are seeded (`seed=42`) and now assert events.
   `pyright` on the eight changed test modules: 0 errors; `ruff`: clean.
8. Lint census: 29 kept, each with a reason at the site (R5 for the grep text).
9. Shipped text: clean (R7).
10. Head drift. The head is now `2f85338d`. `git cherry-pick 6a499504^..7a3cf56c`
    onto it in a scratch worktree applied all 15 commits with no conflicts (git
    auto-merged `benches/common.rs`, `ev_driver/mod.rs`, `py_dwelling.rs`). On
    the result, in its own target dir: `zone_state_ordering` 2 passed,
    `hares-envelope --lib state_space` 28 passed, `hares-python --lib size_pv` 4
    passed, `hares-io --lib garage` 18 passed, `hares-physics --lib ashrae152` 32
    passed, and `cargo check --workspace --all-targets -F hares-core/observe`
    exits 0, so no caller added by OR-19 uses a removed API. (An earlier attempt
    that shared the branch worktree's target dir produced two spurious compile
    errors, `plan_loop_types` and `EVENT_LOAD_STREAM_TAG`: cargo reused
    workspace-crate artifacts across worktrees by relative path and mtime. That
    run was discarded; the numbers above are from a separate target dir.)

## Not verified

- The `zone_state_ordering` mutation (running the envelope integration before the
  Independent phase and confirming the test fails). The attempt to edit the
  scratch worktree's `dwelling/mod.rs` was denied by the permission system, and
  I did not work around it. The test's design (bitwise equality against the
  pre-step `latest_env`, plus a must-move guard) means any such reordering makes
  `seen != before` on the first moving step, but this is by reading, not by run.
- Full nextest workspace runs, the nine clippy lines and `cargo deny` were not
  repeated; the review ran targeted tests, the goldens, the allocation tests and
  the compile check above.

## Cleanup

The scratch worktree `.workstrees/review-drift`, `target-review-headaudit`,
`target-review-headaudit-drift` and the scratch under the branch worktree's
`target/` are removed (see the report).
