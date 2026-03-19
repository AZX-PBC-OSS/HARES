---
id: HARES-056
title: "Benchmark Suite and Performance Gates"
kind: test
depends_on: [HARES-043, HARES-047]
files_to_touch:
  - benches/single_building.rs
  - benches/fleet.rs
  - benches/rl_step.rs
references:
  - docs/architecture/08-operations.md
verification:
  - cargo bench
---

## Background/Context
The architecture mandates a benchmark suite established in Phase 1 and run in CI. Profiling infrastructure behind `#[cfg(feature = "profiling")]` provides per-kernel wall time breakdown. The suite tracks performance over time — it does not make claims, it measures.

## Work to Do
- [ ] Create criterion benchmark suite in `benches/`:
  - [ ] `single_building.rs`:
    - [ ] 30-day single-building simulation (CZ 4A, gas furnace + AC, resistance WH) — primary benchmark
    - [ ] Same building, 1-year duration
  - [ ] `fleet.rs`:
    - [ ] 10-dwelling fleet run
    - [ ] 100-dwelling fleet run
    - [ ] 1000-dwelling fleet run (must complete without OOM on 32 GB)
  - [ ] `rl_step.rs`:
    - [ ] Single `Dwelling::step()` latency (target: sub-millisecond)
    - [ ] `VecDwellingGymEnv` vectorized step latency for N=10, N=100
- [ ] Implement profiling feature (`#[cfg(feature = "profiling")]`):
  - [ ] Per-kernel wall time accumulators: envelope_solve, hvac, water_heater, schedule_load, io, other
  - [ ] Summary emitted after simulation via `tracing::info!`:
    ```
    envelope_solve: 42% | hvac: 28% | water_heater: 11% | schedule_load: 8% | io: 5%
    ```
  - [ ] Memory high-water mark per dwelling (via custom allocator stats or `/proc/self/status`)
  - [ ] Allocation count per timestep assertion: should be zero in hot path (debug mode check)
- [ ] CI integration:
  - [ ] Benchmark results stored as JSON artifacts
  - [ ] Regression detection: warn if any benchmark regresses >10% vs previous commit

## Measures of Success
- [ ] `cargo bench` completes for all benchmark cases
- [ ] 1000-dwelling fleet run completes without OOM on 32 GB
- [ ] Profiling feature compiles and produces kernel breakdown summary
- [ ] Hot-path allocation count = 0 per timestep (verified with profiling feature)
- [ ] Benchmark results are deterministic across runs (±5% wall time variance)

## Verification
- [ ] `cargo bench` passes
- [ ] `cargo test --features profiling -p hares-core` passes
