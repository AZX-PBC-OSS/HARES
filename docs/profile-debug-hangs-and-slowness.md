# Profiling And Debugging Hangs Or Slowness

This playbook is for root-causing tests or runs that appear to hang or become unexpectedly slow.

## 1) Reproduce In A Controlled Way

Run one target with a timeout and single test thread:

```bash
RUST_TEST_THREADS=1 timeout 120s cargo test --test bestest bestest::bestest_case_600ff -- --exact --nocapture
```

Use `timeout` to distinguish "slow" from "infinite/stuck".

## 2) Break Into The Process With A Debugger

Build test binary first:

```bash
cargo test --test bestest --no-run
ls target/debug/deps/bestest-*
```

Run under gdb:

```bash
RUST_TEST_THREADS=1 rust-gdb --args target/debug/deps/bestest-<hash> bestest::bestest_case_600ff --exact --nocapture
```

Inside gdb:

```gdb
set pagination off
run
# after it appears stuck:
interrupt
thread apply all bt
```

Interpretation rule:
- If main thread is parked in test harness and worker thread is in hot math/kernel code, it is compute-bound (slow), not deadlocked.
- If threads are waiting on locks/channels, inspect lock ownership and wait graph.

## 3) Use Built-In Engine Profiling (Production Path)

HARES has internal stage timing behind the `profiling` feature:

```bash
RUST_LOG=info cargo test --features profiling --test bestest bestest::bestest_case_600ff -- --exact --nocapture
```

Look for reported buckets:
- `schedule_load`
- `hvac`
- `envelope_solve`
- `io`
- `other`

This identifies the subsystem before deeper tooling.

## 4) Use Observer For Runtime Physics Context (Not For Raw CPU Cost)

For physics/debug signals:
- Enable observer (`observe` feature).
- Capture a short horizon.
- Inspect envelope gains (`window_solar_w`, `infiltration_w`, `internal_gain_w`, `hvac_*_w`).

Reference: `docs/invariants-and-observability.md`.

## 5) Optional System Profiler (Linux `perf`)

If internal profiling is insufficient:

```bash
RUST_TEST_THREADS=1 timeout 120s perf record -F 99 -g -- cargo test --test bestest bestest::bestest_case_600ff -- --exact --nocapture
perf report
```

Use this for flame/hotspot confirmation across crates and dependencies.

## 6) Known High-Risk Hotspot In This Codebase

A known startup stall class is exact eigendecomposition during solver construction (`StateSpaceModel::from_continuous`), especially in stability checks that call `complex_eigenvalues()` on dense matrices.

When you see stacks in:
- `nalgebra::linalg::schur::*`
- `complex_eigenvalues`
- `hares_envelope::state_space::*`

then the run is stuck in numerical decomposition, not simulation step progression.

## 7) Fast Triage Checklist

1. Reproduce with `timeout` + single thread.
2. Break with debugger and capture `thread apply all bt`.
3. Map top frame to crate/module owner.
4. Confirm subsystem with `--features profiling`.
5. Use observer only for runtime physics anomalies.
6. Patch production code path (avoid test-only hacks).
7. Re-run the minimal target.

