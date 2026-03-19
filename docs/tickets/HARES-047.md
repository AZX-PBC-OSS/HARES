---
id: HARES-047
title: "hares-fleet — Fleet Execution"
kind: implement
depends_on: [HARES-046, HARES-040]
files_to_touch:
  - crates/hares-fleet/src/fleet.rs
  - crates/hares-fleet/src/lib.rs
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
  - docs/architecture/08-operations.md
verification:
  - cargo check -p hares-fleet
  - cargo test -p hares-fleet
  - cargo clippy -p hares-fleet -- -D warnings
---

## Background/Context
`Fleet` runs many independent `Dwelling` simulations in parallel using Rayon. Each dwelling is isolated: a panic or numerical failure in one must not abort the rest of the fleet. Failed dwellings are logged and excluded from aggregates. A progress callback lets callers report status to users without coupling fleet logic to any particular UI.

## Work to Do
- [ ] Implement `fleet.rs`: `Fleet` struct and supporting types
  - [ ] `Fleet::from_buildings(configs: Vec<DwellingConfig>) -> Fleet` — `DwellingConfig` is defined in HARES-043 (`hares-core::DwellingConfig`)
  - [ ] `Fleet::from_resstock(metadata_path: &Path, hpxml_dir: &Path, weather_dir: &Path, resstock_version: Option<ResStockVersion>, filter: Option<HashMap<String, String>>) -> Result<Fleet>` — reads ResStock metadata parquet (results_up00.parquet) via HARES-040's parquet loader, applies optional column filter, builds `DwellingConfig` list; `resstock_version` selects the `ColumnMapper` implementation (parquet files carry no embedded version tag, so the caller must supply it; defaults to latest known version when `None`)
  - [ ] `Fleet::simulate(n_threads: usize) -> Vec<Result<DwellingOutcome, SimError>>` — uses `rayon::par_iter`; `n_threads > 0` builds a local `ThreadPool` via `rayon::ThreadPoolBuilder::new().num_threads(n).build()` and calls `.install(|| ...)`. `n_threads = 0` (or `None` from Python) uses the Rayon global pool. The global pool cannot be resized per-call.
  - [ ] Wrap each dwelling closure in `std::panic::catch_unwind(AssertUnwindSafe(...))` per the architecture doc. `DwellingConfig` and any types it holds must satisfy `UnwindSafe` or be explicitly wrapped.
  - [ ] `DwellingOutcome` — define `DwellingOutcome { result: SimulationResults, sample_weight: f64, status: SimStatus }` as a fleet-specific wrapper struct in `hares-fleet`. Do NOT add `sample_weight` to `SimulationResults` in hares-core — it is a fleet concern.
  - [ ] Failed or panicking dwellings log a structured warning and are returned as `Err` entries; they do not affect other dwellings
  - [ ] Accept an optional progress callback: `with_progress(cb: impl Fn(usize, usize) + Send + Sync + 'static)`
  - [ ] Python-side progress callbacks must acquire the GIL via `Python::with_gil()` inside the Rayon thread to update e.g. tqdm. Document this pattern and add a test to prevent deadlocks.
  - [ ] `SimStatus` enum: `Ok`, `Flagged(String)`, `Failed(String)`
- [ ] Re-export `Fleet`, `SimStatus`, and `DwellingOutcome` from `lib.rs`

## Files to Touch
- `crates/hares-fleet/src/fleet.rs`: new file — `Fleet` struct, constructors, `simulate()`
- `crates/hares-fleet/src/lib.rs`: declare and re-export new module

## Measures of Success
- [ ] A fleet of three test dwellings runs to completion and returns three `Ok` results
- [ ] A dwelling that panics during simulation returns a `Failed` entry; the other dwellings complete successfully and their results are not corrupted by the panic
- [ ] Setting `n_threads = 1` runs sequentially; `n_threads = 4` uses multiple threads (verifiable via elapsed time or Rayon thread pool configuration)
- [ ] Setting `n_threads = 0` uses the Rayon global pool without error
- [ ] The progress callback is invoked at least once during a non-trivial fleet run

## Verification
- [ ] `cargo check -p hares-fleet` passes
- [ ] `cargo test -p hares-fleet` passes
- [ ] `cargo clippy -p hares-fleet -- -D warnings` passes
