# Test Flakes — Named, Not Ignored

A suite that occasionally cries wolf degrades every other test's signal.
Any flaky failure gets an entry here before anyone learns to ignore red
runs: test name, context, reproduction attempts, and status. CI should run
with `--test-threads=1` attribution available so flakes are nameable.

## F-001: hares-io generator-defaults tests (status: unreproduced)
- **Observed once (2026-09-11):**
  `defaults::tests::malformed_generator_efficiency_curve_returns_none` and
  `defaults::tests::generator_dir_ignores_csv_with_battery_header_columns`
  failed together in one full observe-featured three-crate run
  (`cargo test --features observe -p hares-envelope -p hares-io -p hares-core`),
  then passed in every subsequent run.
- **Context:** both tests use per-test `tempfile::tempdir()` (no shared
  fixture between them); failure occurred under heavy parallel load with the
  observe feature enabled (917 tests in the binary).
- **Reproduction attempts:** 6 consecutive plain runs, 4 observe-featured
  runs, 15 looped runs, 20 targeted metrics-module runs — 45+ consecutive
  greens. Not reproduced.
- **Second observation, weaker:** one unnamed hares-io test failed in a
  single run immediately after a metrics edit (log captured only the count
  `914 passed; 1 failed` before being overwritten); every subsequent run
  green. The name is unknown — this entry exists so the next occurrence is
  captured with `--nocapture` and full logs rather than lost.
- **Next step on recurrence:** run the failing binary alone with
  `--test-threads=1` in a loop until it reproduces; capture the panic
  output; check for `/tmp` pressure, fd limits, or parallel-build artifacts.

## F-002: tests/python TestSharedHttpClient construction-serialization tests (status: reproduced, pre-existing)

- **Observed:** `test_the_first_burst_of_downloads_builds_one_client`
  (and once its `test_every_burst_of_downloads_shares_one_client`
  sibling) failed with `assert len(registered) == 1` (got 2) in a full
  `uv run pytest tests/python` run, then passed on re-run. The suite
  also fails the same assertions nearly every time run in isolation
  (`pytest tests/python/test_resstock_data.py::TestSharedHttpClient`:
  1-2 of 5 tests fail per run, 5 of 5 runs observed).
- **Context:** pure-Python threading test: mocked clients behind a
  start barrier across 16 worker threads, asserting a module-level
  registry also collects unrelated exit callbacks (the observed second
  entry is the `exit_cacert_ctx` teardown helper). The suite runs under
  pytest-randomly, so the order changes every run and the failure
  tracks the order, not the code.
- **Not this branch:** the file is byte-identical to the row's base
  commit (`git diff 7187e86a HEAD -- tests/python/` is empty) and no
  `python/` change rides the branch; the branch's changes are Rust-only.
- **Reproduction:** full suite: 1483 passed twice, 1 failed once across
  three runs of the same tree. With `-p no:randomly` the full suite
  passes.
- **Next step on recurrence:** pin down which earlier test's import
  chain registers the extra exit callback in the passing orders, and
  make the assertion count only the registry entries the test itself
  causes (snapshot the registry before the burst and assert the delta).

## Discipline

1. On any flaky failure: file the entry (name + command + feature flags +
   load context) the same day.
2. Never mark a flaky test `#[ignore]` — that silences the signal without
   the cause.
3. Close an entry only with a root cause and a regression test, or with
   N=100+ consecutive greens and a named environmental constraint.
