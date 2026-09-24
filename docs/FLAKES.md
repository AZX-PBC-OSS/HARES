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

## Discipline

1. On any flaky failure: file the entry (name + command + feature flags +
   load context) the same day.
2. Never mark a flaky test `#[ignore]` — that silences the signal without
   the cause.
3. Close an entry only with a root cause and a regression test, or with
   N=100+ consecutive greens and a named environmental constraint.
