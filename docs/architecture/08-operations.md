# Operations & Performance

## Performance Approach

### The Core Problem

OCHRE's 1-year simulation takes ~300 seconds (~150 seconds with recent Numba JIT).
Profiling shows only ~20 seconds is actual physics computation — the rest is Python/pandas
overhead: per-timestep dict creation, DataFrame operations, object allocation, schedule
lookups via pandas indexing.

A Rust rewrite eliminates the orchestration overhead. The physics computation stays the
same (~20s of equivalent work), but the surrounding loop is native code with preallocated
buffers instead of Python objects. If we write good quality, idiomatic Rust with a sound
design, it will be as fast as it needs to be.

### Profiling-First, Not Claims-First

We do not make specific speedup claims in advance. Instead, we establish a benchmarking
framework early and let measurements drive optimization decisions:

**Benchmark suite** (established in Phase 1, run in CI):
- A standard 30-day single-building simulation (CZ 4A, gas furnace + AC, resistance WH)
- Same building, 1-year duration
- Multi-building: 10, 100, 1000 dwellings via fleet mode
- RL environment: single-step latency, vectorized step latency

**Profiling infrastructure** (behind `#[cfg(feature = "profiling")]`):
- Per-kernel wall time accumulators
- Summary after simulation:
  ```
  envelope_solve: 42% | hvac: 28% | water_heater: 11% | schedule_load: 8% | io: 5%
  ```
- Memory high-water mark per dwelling
- Allocation count per timestep (should be zero in hot path)

**What we track, not what we promise**:
- Wall time for the benchmark suite (compared to OCHRE on same inputs)
- Memory per dwelling under fleet execution
- RL step latency (single and vectorized)
- Bottleneck breakdown by kernel

If profiling reveals a specific kernel is disproportionately slow, we optimize that
kernel — not speculate about theoretical speedups in advance.

## Logging & Observability

Structured logging via `tracing` crate:

```rust
tracing::info!(bldg_id = %id, elapsed_s = %elapsed, "dwelling complete");
tracing::warn!(bldg_id = %id, zone = "indoor", temp_c = %t, "temperature out of range");
```

### Fleet Progress

For fleet runs:
```
[INFO] fleet progress: 850/10000 dwellings (8.5%) elapsed=42s rate=20.2 dw/s eta=453s
```

## Failure Handling

### Fleet Isolation

A panic in one dwelling must not abort the fleet. Use `catch_unwind` per dwelling:

```rust
let results: Vec<Result<DwellingResult, SimError>> = buildings
    .par_iter()
    .map(|b| {
        std::panic::catch_unwind(AssertUnwindSafe(|| simulate_dwelling(b)))
            .map_err(|e| SimError::Panic(format!("{:?}", e)))
            .and_then(|r| r)
    })
    .collect();
```

Failed dwellings are logged and excluded from fleet aggregates. A summary reports
which buildings failed and why.

### Numeric Containment

If zone temperatures escape sanity bounds (see Testing doc), the dwelling is flagged.
Fleet aggregation only includes successful dwellings.

```rust
pub enum SimStatus {
    Ok,
    Flagged(String),  // completed with warnings
    Failed(String),   // did not complete
}
```

## Checkpoint / Restart

For long fleet runs and RL training:

- Per-dwelling state: equipment state + RNG state + timestep index
- Written atomically (temp file + rename) at configurable intervals
- Restart loads checkpoint and continues from saved timestep

```rust
pub struct DwellingCheckpoint {
    pub bldg_id: i64,
    pub timestep_index: u64,
    pub equipment_states: Vec<(EquipmentId, Vec<u8>)>,
}
```

Checkpoint serialization format is versioned — checkpoints are valid only within the
same build of ochre_next. Cross-version checkpoint compatibility is not a goal.
