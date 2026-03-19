---
id: HARES-041
title: "hares-core — Clock and RNG"
kind: implement
depends_on: [HARES-002]
files_to_touch:
  - crates/hares-core/src/clock.rs
  - crates/hares-core/src/rng.rs
  - crates/hares-core/src/lib.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
Every HARES simulation needs a monotonically advancing clock that maps integer timestep indices to wall-clock `DateTime<Utc>` values, and a deterministic per-building RNG so that stochastic equipment behaviour can be replayed exactly regardless of how many buildings are present. These two primitives underpin all subsequent simulation code.

## Work to Do
- [ ] Implement `clock.rs`: `SimClock` struct
  - [ ] Fields: `start_time: DateTime<Utc>`, `time_res: Duration`, `duration: Duration`, `pub(crate) current_step: u64` with a public read-only accessor `fn current_step(&self) -> u64` — crate-internal mutability is preserved while external consumers use the accessor
  - [ ] `current_time() -> DateTime<Utc>`: returns `start_time + current_step * time_res`
  - [ ] `total_steps() -> u64`: returns `duration / time_res` (integer floor division)
  - [ ] Iterator boundary: after the iterator is exhausted (`current_step == total_steps()`), `current_time()` returns `start_time + total_steps() * time_res` (one step past the last simulated time). Document this in the struct's doc comment.
  - [ ] Implement `Iterator` yielding timestep indices `0..total_steps`
  - [ ] Support arbitrary simulation durations (hours, days, weeks, year)
- [ ] Implement `rng.rs`: `derive_dwelling_rng`
  - [ ] Signature: `derive_dwelling_rng(master_seed: u64, bldg_id: i64) -> ChaCha8Rng`
  - [ ] Seed construction: `seed[0..8] = master_seed.to_le_bytes()`, `seed[8..16] = bldg_id.to_le_bytes()`, remaining bytes zero
  - [ ] Deterministic: same `(master_seed, bldg_id)` pair always produces identical RNG output
  - [ ] Isolation: adding or removing buildings does not affect any other building's RNG stream
  - [ ] Negative `bldg_id` values: the seed construction uses `i64::to_le_bytes()`, so negative IDs are handled by two's-complement representation — this is intentional and must be tested
- [ ] Re-export `SimClock` and `derive_dwelling_rng` from `lib.rs`
- [ ] Initialize `tracing_subscriber::fmt::init()` in the integration test harness for `hares-core` so that all `tracing::warn!` and `tracing::info!` calls in downstream tickets are captured during testing

## Files to Touch
- `crates/hares-core/src/clock.rs`: new file — `SimClock` struct and `Iterator` impl
- `crates/hares-core/src/rng.rs`: new file — `derive_dwelling_rng` function
- `crates/hares-core/src/lib.rs`: declare and re-export new modules

## Measures of Success
- [ ] `SimClock` with 30-day duration and 60-second `time_res` yields exactly 43 200 steps
- [ ] `current_time()` at step 0 equals `start_time`
- [ ] After iterator exhaustion, `current_time()` returns `start_time + total_steps() * time_res`
- [ ] `current_step()` accessor returns the correct step index; the field itself is not directly accessible from outside the crate
- [ ] `derive_dwelling_rng` with the same seed and id produces identical byte sequences across calls
- [ ] Two different `bldg_id` values with the same `master_seed` produce different RNG streams
- [ ] `derive_dwelling_rng` with `bldg_id = -1` produces a valid, distinct RNG stream (negative id handled via two's-complement bytes)
- [ ] Generating RNG for `bldg_id = 1` and `bldg_id = 2` independently produces identical streams regardless of whether `bldg_id = 3` is also created (isolation test)

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
