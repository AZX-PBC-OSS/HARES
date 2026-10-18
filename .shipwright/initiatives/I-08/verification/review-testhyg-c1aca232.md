# Review: fix-testhyg at c1aca232 (PARTIAL)

PARTIAL: the session ended before the fold-readiness walk and the overlap check finished. Everything else was run.

## Verdict: CHANGES-REQUIRED (fold blocked by a build break in a mid-branch commit; the tip itself is clean)

0 high, 1 medium, 3 low/info. The tip passes the whole plan block. The medium finding is a fold-rule violation, not a code defect.

## Findings

### Medium

**M1. Six commits of the branch do not build (`cargo check --workspace --all-targets`), so the fold rule fails unless squashed.**
`fa9699ec` (test: write every test output under a per-test temporary directory) through `ac151cc1` fail with
`error[E0425]: cannot find function unique_temp_name` and `cannot find ... TempFile` at
`crates/hares-core/src/checkpoint.rs:647` and `:651` (lib test target). Failing: `fa9699ec`, `11a35f7f`, `f31b6c35`,
`0d354790`, `458a9879`, `ac151cc1`. Only `3ce55556` (the first commit) passed. Commits after `ac151cc1` were not walked
(see Not run). The helper is introduced by a later commit (the shared temp-file helper, `8ca4fbaa`) and used earlier. Fix:
squash the branch per row on fold, or reorder the helper before its first use. Python collection (`pytest --collect-only`,
1415 tests) passed on all 7 commits walked.
Reproduction: `git checkout --detach fa9699ec && cargo check --workspace --all-targets` in a scratch worktree.

### Low

**L1. The new `core._finalizer()` call depends on a private attribute of the helics wrapper, with no upper pin.**
`python/ochre_next/helics/federate.py:264`, `pyproject.toml:28` (`helics>=3.6.1`). If a release drops or renames
`_finalizer`, `_release` raises `AttributeError` during cleanup of a failed core. The real-helics test
`test_a_released_helics_core_leaves_its_object_nothing_to_free` would catch an upgrade, so this is detectable. Also,
`_finalizer()` runs the wrapper's cleanup callback, which calls the process-global `helicsCleanupLibrary()` when
`PYHELICS_CLEANUP_ON_DESTRUCTION` is set (`capi.py:969`). That is the wrapper's behaviour at collection too, but it now
happens synchronously inside `create_value_federate` while other federates may be live. Default is off.

**L2. `restore_helics_modules` is not applied to `test_helics_integration.py::test_helics_import_guard`.**
`tests/python/test_helics_integration.py:494-499` pops the `ochre_next.helics*` modules and never restores them (the
same pollution class, pre-existing). It did not break any order I ran.

**Info. `readelf -d @optionsfile`.** The guard admits exactly `readelf -d <one operand>` by real path, and the operand is
unconstrained, so `@file` (binutils options-file expansion) is admitted. readelf only reads files, so this is not a network
path. Not a defect for a test seatbelt.

## Checks completed, with counts

1. Double-free root cause: CONFIRMED from source. `helics/capi.py:962-980`: `_HelicsCHandle.__init__` sets
   `self._finalizer = weakref.finalize(self, cleanup, handle)`; for `HelicsCore` the callback is `helicsCoreFree`
   (`capi.py:952`), gated by `PYHELICS_FREE_ON_DESTRUCTION` (`bool(env.get(..., True))`, so only an empty value disables).
   `weakref.finalize.__call__` runs the callback once and is a no-op when already dead. An explicit `helicsCoreFree` plus
   the wrapper finalizer is therefore a double free. The C-side consequence (a second free landing on a reused address) was
   reasoned, not reproduced. No other explicit free exists: grep over `python/`, `tests/python/` and `scripts/` finds no
   `helicsFederateFree`, `helicsBrokerFree`, `Destroy` or `helicsCloseLibrary` call. Federates and brokers are only
   disconnected (`broker.py:132`, `federate.py:402`); their frees come from the wrapper once.
2. Retry logic, read: bounded by `_CORE_PORT_ATTEMPTS = 4`, in-process cores 1 attempt; every failed core goes through
   `_release` before the next attempt or the raise; a release error returns instead of raising and is chained
   (`from connect_error or cleanup`) or added as a note; a failed release stops the retry (a core that may hold its port
   proves nothing). Mutations in scratch worktree (federate tests, `-n0`, 26 tests, 3 repeats where noted):
   - c1aca232 fix reverted (explicit `helicsCoreFree` instead of `_finalizer()`): 6 failed, 20 passed, identical in 3 of 3 runs.
   - 240bf3eb connect catch removed: 5 failed, 21 passed, identical in 2 of 2 runs.
   - 240bf3eb `_release` reverted to sequential raising calls: 10 failed, 16 passed.
   - Unmutated: 26 passed.
   The fake-core tests are deterministic (no wall-clock waits, ports patched).
3. Offline guard: `_local_command` requires argv to equal the fixed prefix plus exactly N operands (`readelf -d X`: 3 words,
   `uname -p`: 2) and the program (`executable`, or argv[0] resolved through the env's PATH) to realpath-equal that
   command's own binaries, so another binary under the argv, an extra operand, or a PATH impostor is refused. Per-command
   binary sets also stop `readelf` argv running `uname` and the reverse. Mutation: guard reverted to 5134a92e~1 with the
   new tests: `test_a_local_command_runs[dynamic-section]` fails (1 failed, 34 passed); unmutated 35 passed. See Info above.
4. `restore_helics_modules`: files test_helics_{broker_runner,dwelling,federate,fleet}.py and
   test_helics_pyarrow_symbol_order.py (140 tests), `-n0`, forward order: 140 passed; reversed file order: 140 passed.
   pytest-randomly is not installed. Mutation, fixture body returning right after `yield`: 12 failed, 128 passed
   (federate tests got the fake `helics`), so the fixture is what keeps them green.
5. Plan section 3 block on c1aca232, run verbatim in a clean scratch worktree (OCHRE submodule initialised, empty census
   file), `bash -euxo pipefail`, own `CARGO_TARGET_DIR`: BLOCK_RC=0.
   - tools: rustc 1.94.1, cargo-nextest 0.9.137, cargo-deny 0.20.2, uv 0.12.8.
   - fmt pass; clippy all 9 lines pass with `-D warnings`.
   - nextest debug 7348 passed, 0 skipped; observe 7406; dst 7361; profiling 7354; `-p hares-core -F observe,profiling` 1462;
     release 7305; each 0 skipped.
   - bench `--no-run` pass; deny advisories ok.
   - frame_golden compare-all: 6 manifests identical (default), 1 identical (`-F dst`).
   - python-build ok; pytest `-m ""`: 1493 passed, 4 xfailed, 16 warnings, 0 skipped, census diff empty.
   - `[stray]`: `comm -13` of before/after empty; TMPDIR (`target/block-tmp`) empty apart from the tool-owned entries.
   - `[writing]` on HEAD: pass.
   - pyright (`uv run --no-sync pyright`): 0 errors, 0 warnings, 0 informations.
   Worktree `git status` clean after the block.
6. HELICS stability: `pytest -n 8 -m "" tests/python/test_helics_integration.py tests/python/test_helics_federate.py`
   (59 tests), 50 runs, each with its own empty TMPDIR. 50 of 50: rc 0, 59 passed, no crash, nothing left in TMPDIR
   (apart from pytest's empty per-user root, which the plan excludes). Run time 77 to 108 s. Load average (1 min) at the end
   of each run: runs 1 to 20 between 8.6 and 40.5 (the full block was running at the same time; 15 min load up to 26.8),
   runs 21 to 50 between 0.1 and 0.6. Per-run lines: `scratchpad/.../helics_loop.log` (deleted with the scratch directory;
   all 50 read `rc=0 tmpdir_left=0 59 passed`).
7. Rebase: `git range-diff 2f85338d..c318e431 db5efe80..c1aca232` pairs all 22 old commits: 20 identical (`=`), 2 differ
   (`aa079f15`->`fa9699ec`, `34503778`->`aff2f2ce`, conflict resolutions); 7 new commits (`2db4466b`, `c32da835`, `d3293221`,
   `54d750a3`, `5134a92e`, `240bf3eb`, `c1aca232`). The branch has 29 commits on `db5efe80`, descends from it. Of the 78
   files the head changed since `2f85338d`, 18 are also in the branch's net diff; every line the head added in those files is
   present in the tip (0 absent), and for 15 of the 18 the branch's `+/-` lines equal the old branch's exactly (0
   difference). The other three differ only through new commits or head-added files: `test_helics_integration.py`,
   `conftest.py` (head's pyarrow import is kept), `equipment_init_failure_assembly.rs` (a head-added file the branch edits).
   Head-touched files the branch did not touch equal the head at the tip. Nothing of `db5efe80` is lost.
9. Shipped text: all 29 commit messages and added lines checked with the plan's `ids` pattern plus U+2013, U+2014,
   `Co-Authored`, `Generated`: 0 matches.
8a. Fold walk (partial, see M1): 7 of 29 commits checked.

## Not run

- Fold walk for the last 22 commits (cargo check and pytest collection per commit). I expect the break to clear once
  `8ca4fbaa` lands, but did not verify.
- Overlap of the branch with `fix-headbreaks` (`cd1b5972`) and `fix-headaudit` (`4061da95`).
- Reproduction of the C-side double-free effect (testing gap).
- Review of the earlier-round fixes `c32da835`, `d3293221`, `54d750a3` and the README finding L4 of the previous review beyond
  the range-diff and writing checks.

## Handoff

A successor reviewer must:
1. Resume the fold walk: in a scratch worktree, for each commit from `8ca4fbaa` to `c1aca232` run
   `cargo check --workspace --all-targets` and `pytest --collect-only -m "" tests/python`; report which fail. Decide with the
   implementer between squash-per-row and reordering the temp-file helper (M1).
2. Compute overlap with `fix-headbreaks` and `fix-headaudit`: `git diff --name-only` of each branch against its merge-base with
   `db5efe80`, intersected with this branch's file list, then read overlapping hunks.
3. Check the L4 README finding (`README.md` httpx wording) is fixed at the tip.
4. Decide on L1 and L2 (pin or guard the `_finalizer` use; apply the restore fixture to `test_helics_import_guard`).
Scratch worktrees, target directories and the private scratch directory were deleted.
