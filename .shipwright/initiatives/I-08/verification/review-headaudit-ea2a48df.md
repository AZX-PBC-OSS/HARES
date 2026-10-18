# Review of `fix-headaudit` tip `ea2a48df`

Verdict: SHIP on content. The branch is not a fast-forward of `feat/output-results` (now `1b708ccb`); it needs a rebase before the fold. That rebase and a `fix-testhyg` merge are discussed in finding 5. No finding blocks the content.

## Findings

### 1. Low: a module path given directly bypasses the collection rule
- Location: `tests/python/conftest.py:49-54` (`pytest_ignore_collect`).
- Scenario: `pytest tests/python/test_thermal_trace.py` with the default `-m 'not slow and not ochre'`. The explicit argument is still collected and imported (output: "no tests collected (4 deselected)"). In an environment without the `ochre` group that is now a collection error. Before this change it was a skip.
- Evidence, with a scratch plugin setting `sys.modules["ochre"] = None`:
  - Default selector alone: 1345 of 1385 collected, exit 0.
  - Default selector plus the path: "1 error during collection".
- This is loud, not silent, so it is not a drop-out. It does contradict the claim that a default run never imports OCHRE.
- Related: `pytest -m "not slow"` in a plain env also imports the modules, because `selects_ochre("not slow")` is true.
- Fix: document it in the conftest comment, or ignore the modules by node ID too.

### 2. Low: `ochre_records` has no zero or NaN test
- Location: `tests/python/ochre_records.py:17-18`, `tests/python/test_ochre_records.py`.
- The tests pin only ±50 and 0.8776. The behaviour is correct but unpinned:
  - A zero record holds only for an exact zero measurement (the margin is 0).
  - A NaN recorded or measured value fails closed (`nan <= x` is False).
  - `inf` against `inf` also fails closed (`inf - inf` is NaN).
- No current record is zero, so nothing is wrong today.
- The write-up said "pinned for positive, negative and zero"; the file pins only positive and negative.
- Fix: add parametrized zero, NaN and inf cases.

### 3. Low: two private or implicit dependencies in the gate
- `conftest.py:8` imports `_pytest.mark.expression.Expression`, which is private pytest API. It works on the pinned pytest 9.1.1, which is the version I ran.
- `skip_gate.py:26-27` returns silently when there is no terminal reporter (`-p no:terminal`), so the gate is disabled. I saw exit 4 in that case, from a different cause. This is contrived.

### 4. Info: the xfail records are 2%-pinned seeded values
- The records are checked on the Python 3.12 and 3.13 CI matrix, and I could not run 3.12 here. If numpy or numba differ across those versions, `check_record` fails in CI. I ran 3.13 only.
- The reason text "every sampled step outside the 5% band" is true (36 of 36 steps are at +29% to +31.5%). It is not itself pinned, but the strict xfail still holds.

### 5. Fold readiness (item 9): needs a rebase, and the `fix-testhyg` merge conflicts
- `feat/output-results` is `1b708ccb`, six commits past `2f85338d`. `ea2a48df` is not a fast-forward.
- `fix-headaudit` onto `1b708ccb`: `git rebase --onto 1b708ccb 2f85338d ea2a48df` in a scratch worktree applied all 19 commits with no conflict ("Successfully rebased"). The worktree was deleted afterwards.
- I did not rebuild or run tests on the rebased result. The six operator commits touch the goldens and `crates/hares-core/src/dwelling/mod.rs`. The golden, nextest and pytest lines need re-running after the rebase.
- Merging `fix-testhyg` (`c318e431`) onto `ea2a48df` conflicts in three files. `git merge-tree` shows the same three:
  - `tests/python/conftest.py`: import block. testhyg adds `Callable`, `Mapping`, `pytest_plugins` and `output_in_test_dir`; headaudit adds `pytest`, `Expression` and `skip_gate`. A mechanical merge resolves it.
  - `pyproject.toml`: the `markers` list. testhyg rewords the `ochre` marker text and adds a `network` marker.
  - `tests/python/test_py_fleet.py`: `_build_simulatable_fleet`. testhyg's side keeps `pytest.skip("OCHRE fixtures not available")` and adds an `output_dir` parameter. headaudit's `218b8ed2` removed that skip. A merge must drop the skip, or the `[pytest]` skip census and `--fail-on-skip` will fail.
- A per-commit cherry-pick of testhyg onto headaudit conflicted in many more files. I did not use that as the measure: many testhyg commits depend on earlier testhyg commits.
- Files touched by both branches: `benches/common.rs`, `dwelling/mod.rs`, `zone_state_ordering.rs`, `py_dwelling.rs`, `pyproject.toml`, `conftest.py`, `test_ev_tariff_integration.py`, `test_gym_env.py`, `test_ochre_parity.py`, `test_py_fleet.py`, `test_rl_warning_surfacing.py` and `test_thermal_trace.py`. Git auto-merged all except the three above.

## Checks

1. **Rebase integrity: pass.** `git range-diff 6a499504..7a3cf56c 2f85338d..5f31d549` shows 14 of 15 commits `=`.
   - The 15th (`73a05023`, was `260cae7b`) is `!` with context-line differences only, in `crates/hares-core/src/dwelling/mod.rs` (the `timestamp_buf` and `zone_capacitances_j_k` neighbourhood).
   - `ee12383c` is shown as an added commit because it sits on the new base. It is the shim-deletion commit and was the old base `6a499504`.

2. **R2 collection rule: sound, apart from finding 1.** `--co` counts at `ea2a48df`:

   | Selector | Collected |
   |---|---|
   | `-m ochre` | 54 of 1439 |
   | `-m 'ochre or slow'` | 94 |
   | `-m 'not slow'` | 1395 |
   | `-m ''` or `-o addopts=""` | 1439 |
   | no `-m` (default) | 1345 of 1385 |
   | `-m 'not ochre'` | 1385 |
   | `-m 'slow and ochre'` | 4 |
   | `-m 'not (slow or ochre)'` | 1345 |
   | `PYTEST_ADDOPTS="-m ochre"` | 54 |
   | `-m ochre -n2` | 54 |

   - `-m ochre -k solar` gives 2 tests, and `-k nothing` still imports the modules. A parse error (`-m 'ochre and'`) is a loud collection error, exit 4. `-p no:mark` is refused.
   - Only three modules carry the `ochre` marker (`grep` of `tests/python`), and all three are in `OCHRE_MODULES`.
   - CI has exactly three pytest lines: default `-m "not slow and not ochre"`, OCHRE `-m ochre -rxX --fail-on-skip`, and slow `-m "slow and not ochre"`. The four ochre+slow tests run in the OCHRE job. No test drops out of every job.
   - The plan block line uses `-m ""` with the census. It collects everything and the census is empty (`ledger/pytest-skip-census.txt` has 0 lines). It does not pass `--fail-on-skip`, but the census diff does the same job.
   - `--fail-on-skip` under xdist: a skip gave exit 1 and "--fail-on-skip: 1 skipped". An xfail-then-skip also gave a skip. A deselected skipper is not counted.
   - Broken-import proof reproduced. With `ochre` and `xmltodict` set to None, `-m ochre --fail-on-skip` gives "3 errors" (ashrae, parity, thermal_trace), exit 1. The write-up says 2 because it broke only `ochre`.

3. **R1, R3, R4: pass.**
   - The xfail reasons print as shipped with "not yet triaged" and no ID or class. The infiltration sentence is gone from the reasons and labelled an unchecked lead in OD-50.
   - The boundary is inclusive and correct; see finding 2 for the unpinned zero and NaN behaviour.
   - `pytest.fail` raises `Failed`, not `AssertionError`, so a stale record fails the strict xfail instead of passing as the expected failure.

4. **R5: pass.** The grep prints 29.

5. **R6: pass, mutations reproduced** in a scratch worktree, since deleted. I restricted the work to the scratch copy.
   - Cloning `scratch.zone_temps` before `self.scratch = scratch` makes `record_from_state_returns_the_same_scratch_buffers_with_the_steps_values` fail with a changed pointer.
   - Swapping the `IntSurfaceDiag` fields in `longwave.rs` makes `interior_surface_diagnostic_pairs_follow_surface_order` fail on `left == right`.
   - The scratch-buffer test checks three of the scratch vectors, not `equipment_modes` and the rest.

6. **R7 and shipped text: pass.**
   - The net diff `2f85338d..ea2a48df` has no em or en dash, and no ticket or agent reference, in added lines. All commit messages are clean.
   - The en dash in `73a05023`'s added line ("Steps 1e–1f") is fixed by the tip commit. The `[writing]` check runs per HEAD commit, so it is clean at the tip only.

7. **Block, run in the worktree at `ea2a48df`:**

   | Line | Result |
   |---|---|
   | `cargo fmt --check` | exit 0 |
   | nextest debug | 7326 passed, 0 skipped |
   | nextest release | 7282 passed, 0 skipped |
   | golden | all identical |
   | golden `-F dst` | identical |
   | `pytest -m "" -rs --fail-on-skip` | 1435 passed, 4 xfailed, 0 skipped, exit 0 |
   | CI OCHRE command (`-m ochre -rxX --fail-on-skip`) | 50 passed, 4 xfailed, exit 0 |

   - Not run: the other nextest lines, clippy, bench and advisories.
   - `[pyright]` is inconclusive. The `.venv` pyright reports 1111 environment import-resolution errors, including `ochre_next` and `polars` in the changed files, and I did not have an all-groups environment. No type error appeared in the new files beyond unresolved imports.

8. **OR-19 mutation write-up: consistent.** I reproduced it. At `roster.rs:1041-1043`, `.unwrap_or_default()` on `plan_loop_types` makes `an_unwirable_roster_add_is_rejected_and_leaves_the_dwelling_untouched` fail at `expect_err` ("cannot be wired: ()"). The plan step is the only gate, which is why the bypass is real and the failure is the right one.

9. **Fold readiness:** see finding 5.

## Cleanup
- `target-review-headaudit2` and the worktree's `target/` are deleted.
- Both scratch worktrees (`review-ha2-scratch`, `review-ha2-fold`) are removed.
- The main tree and the `fix-headaudit` worktree files are untouched.
- No checkout, stash, reset or revert was run on the main tree. A `git reset --hard` was run only inside the scratch fold worktree.
