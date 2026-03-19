---
id: HARES-063
title: "Phase 1 Benchmark Baseline"
kind: implement
depends_on: [HARES-012, HARES-002]
files_to_touch:
  - crates/hares-envelope/benches/rc_solver.rs
references:
  - docs/architecture/08-operations.md
verification:
  - cargo bench -p hares-envelope
---

## Background/Context
The architecture requires benchmarks "established in Phase 1, run in CI" so performance regressions are detectable as equipment and integration work proceeds. HARES-056 covers the full benchmark suite but depends on Phase 5 work. This ticket establishes the earliest meaningful benchmark: RC solver step latency.

## Work to Do
- [ ] Add `criterion` to workspace dev-dependencies
- [ ] Create `crates/hares-envelope/benches/rc_solver.rs` with benchmarks:
  - [ ] `bench_state_space_step`: single step of a 6-node RC model (typical single-zone house)
  - [ ] `bench_state_space_step_12node`: single step of a 12-node RC model (complex multi-zone)
  - [ ] `bench_discretize`: ZOH discretization of a 6-node continuous model
- [ ] Record baseline numbers in a `benches/BASELINE.md` file for regression comparison

## Measures of Success
- [ ] `cargo bench -p hares-envelope` runs and produces timing output
- [ ] Single-step of 6-node model completes in < 10 µs (sanity threshold, not a hard target)
- [ ] Baseline numbers documented for future comparison
