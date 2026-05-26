# Benchmark coverage gaps: no aggregation, co-simulation, or RL episode benchmarks
**Review ID**: infra-05
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
- `benches/fleet.rs` (48 lines)
- `benches/rl_step.rs` (57 lines)
- `benches/single_building.rs` (44 lines)
- `benches/common.rs` (178 lines)

## Vendor/Reference Files Consulted
- `crates/hares-fleet/src/aggregation.rs` (730 lines) — production `aggregate()` function with no benchmark coverage
- `crates/hares-fleet/src/fleet.rs` (960 lines) — `Fleet` and `SteppableFleet` with no per-step aggregate timing
- `crates/hares-python/src/py_gym.rs` (172 lines) — `batch_step_py()` used by RL vec env but never measured standalone
- `python/ochre_next/rl/gym_env.py` (450 lines) — `DwellingGymEnv` with episode lifecycle (`reset` + `step` x N) never benchmarked
- `python/ochre_next/helics/fleet.py` (374 lines) — `HELICSFleet._publish_results()` aggregate summing (line 226) never benchmarked
- `python/ochre_next/helics/dwelling.py` (298 lines) — `HELICSDwelling.run()` federate loop never benchmarked
- `crates/hares-io/src/hpxml/mod.rs` (143 lines) — production HPXML parser used in fleet/single-building benches via `OCHRE sample` but never with ResStock-sourced HPXML
- `crates/hares-io/src/resstock.rs` (662 lines) — ResStock metadata parser with no benchmark usage

## Findings

### Finding 1: No fleet aggregation benchmark despite production `aggregate()` implementation [Severity: high]

**Description**: The `hares_fleet::aggregation::aggregate()` function (`aggregation.rs:131`) is the sole code path for computing weighted fleet-level timeseries and per-dwelling metrics (annual energy, peak power, sample weight) from `DwellingOutcome` vectors. It performs two-phase aggregation: Phase 1 temporal resampling (15-min or hourly via `ColumnAggregation` at `aggregation.rs:38-61`) and Phase 2 weighted cross-dwelling combining (weighted sum for energy, weighted mean for temperature via `FleetAggregation` at `aggregation.rs:68-87`). At fleet-study scale (10,000+ dwellings), the aggregation pass must process thousands of output timeseries with identical time alignment, making it a compute-intensive bottleneck. However, zero benchmarks exist for `aggregate()`. The `fleet` bench at `benches/fleet.rs:18-42` measures `Fleet::simulate()` wall-clock time but discards the output via `criterion::black_box(results)` at line 36 without ever calling `aggregate()`.

**Code Location**: `crates/hares-fleet/src/aggregation.rs:131` — `pub fn aggregate(results: &[DwellingOutcome], resolution: AggregationResolution) -> FleetResults`

**Root Cause**: Benchmarks were written to measure raw simulation throughput only; aggregation was treated as downstream post-processing and never instrumented. The `fleet` bench at `benches/fleet.rs:34-37` collects `results` but drops them without aggregation.

**Impact**: Performance regressions in the aggregation pipeline (e.g., changes to `build_aggregate_batch()` at `aggregation.rs:311-405`, which constructs Arrow `RecordBatch`es from per-dwelling buckets) slip through undetected. Fleet-scale studies with 10,000+ dwellings may encounter unexpected aggregation slowdowns with no baseline to compare against.

**Recommended benchmark**: Add `bench_aggregation.rs` measuring `aggregate()` with 100, 1,000, and 10,000 dwelling outcomes. Parameters to instrument:
- Wall-clock time (Criterion primary measure)
- Memory usage: peak heap during `build_aggregate_batch()` (valgrind/massif or `stats_alloc`)
- Allocation count and total bytes allocated (tracking reader)
- Scale: dwelling counts of 100, 1,000, 10,000; dwelling counts of 100, 1,000, 10,000; dwelling durations of 1 day and 365 days
- Resolution: both `AggregationResolution::FifteenMin` and `AggregationResolution::Hourly`

### Finding 2: No full RL episode benchmark — only single timestep `step()` measured [Severity: high]

**Description**: The `rl_step` bench (`benches/rl_step.rs:14-51`) benchmarks only the raw `dwelling.step()` call (a single simulation timestep). A complete RL training episode requires: (a) environment reset (`reset_with_seed()` at `py_dwelling.rs:1156-1168`, or `DwellingGymEnv.reset()` at `gym_env.py:402`), (b) observation construction (`to_observation_vec()` at `telemetry.rs:32-108`, which selects named channels into a flat `Vec<f64>`), (c) action application (`apply_control()` via actor `DispatchRequest`s), and (d) N repeated `step()` calls until termination or truncation. None of these episode lifecycle overheads are measured. The `batch_step_py()` helper (`py_gym.rs:50`) used by `VecDwellingGymEnv` is also unmeasured — the vec bench at `benches/rl_step.rs:28-51` only measures raw `dwelling.step()` in a rayon `par_iter_mut`, missing the GIL-free batching overhead, observation extraction, and return-value construction that `batch_step_py()` performs.

**Code Location**: `benches/rl_step.rs:14-51` — only `dwelling_step_single` and `vec_dwelling_step` measured; no episode lifecycle or `batch_step_py` benchmark

**Root Cause**: RL benchmarks focused narrowly on simulation engine performance per timestep and ignored the overhead of the surrounding environment contract that dominates RL training throughput (reset, observation extraction, reward computation).

**Impact**: RL training at scale (hundreds of thousands of episodes) may be bottlenecked by episode lifecycle overhead rather than raw step time. Without episode benchmarks, optimization effort may be misdirected at the step loop when the real cost is in observation construction or reset path.

**Recommended benchmark**: Add `bench_rl_episode.rs` measuring a full episode with:
- Wall-clock time per episode (Criterion) for single-dwelling and vec-dwelling (16, 64, 256 dwellings)
- Wall-clock time for `reset()` path separately (save/load state, re-initialization)
- Wall-clock time for `to_observation_vec()` across configurable observation fields (narrow: 5 fields; wide: 50+ fields)
- Wall-clock time for `batch_step_py()` directly from Rust (not through Python FFI)
- Allocation count per episode
- Episode length: 96 timesteps (1 day at 15-min resolution) and 8,760 timesteps (1 year at 1-hour resolution)
- Number of dwellings: 1, 16, 64, 256

### Finding 3: No HELICS co-simulation latency benchmark [Severity: medium]

**Description**: The production HELICS integration spans `HELICSDwelling` (`python/ochre_next/helics/dwelling.py:40`) wrapping a single `PyDwelling` as a value federate and `HELICSFleet` (`python/ochre_next/helics/fleet.py:25`) wrapping `PySteppableFleet` as a single federate publishing aggregate power. Co-simulation performance is dominated by federate time synchronization overhead (HELICS `request_time`) and signal publication/subscription latency — neither is measured. The `HELICSFleet._publish_results()` method (`helics/fleet.py:226-243`) iterates all dwellings to compute aggregate power and publishes per-dwelling + aggregate signals; at fleet scale this per-step aggregation loop embedded inside the time-request loop is a latency-critical path. Existing HELICS tests (`tests/python/test_helics_integration.py`, `tests/python/test_helics_fleet.py`) validate correctness but do not measure wall-clock time or allocation overhead.

**Code Location**: `python/ochre_next/helics/fleet.py:226-243` — `_publish_results()` aggregate summing loop never benchmarked

**Root Cause**: HELICS benchmarks require a running broker, which adds setup complexity beyond simple Criterion benches. No benchmark harness exists for HELICS latency measurement.

**Impact**: Co-simulation deployments cannot be sized for real-time constraints. Without latency baselines, it is unknown how many dwellings a single federate can represent before per-step publish overhead exceeds the co-simulation time step budget.

**Recommended benchmark**: Add Python-based HELICS latency benchmarks (e.g., via `pytest-benchmark`) measuring:
- Wall-clock time for `HELICSDwelling.run()` single-step through `request_time` grant cycle
- Wall-clock time for `HELICSFleet._publish_results()` with 10, 100, 1,000 dwellings
- Wall-clock time for federate creation (`helics.CreateValueFederate`) and destruction
- End-to-end wall-clock time for a 96-step co-simulation with 1, 10, and 100 dwellings
- Measure sub-millisecond precision (HELICS time grants are typically sub-millisecond)

### Finding 4: All benchmarks use synthetic data — no ResStock-sourced HPXML [Severity: medium]

**Description**: Every benchmark constructs data programmatically: `benches/single_building.rs` and `benches/fleet.rs` use `build_dwelling_config()` (`common.rs:108-137`) which references the OCHRE sample HPXML (`tests/fixtures/hpxml/ochre_samples/base.xml`) but pairs it with a 4-row synthetic schedule CSV (`common.rs:30-39`) and a synthetic EPW with identical 19°C dry-bulb every hour (`common.rs:42-106`). `benches/rl_step.rs` uses `synthetic_toml_case()` (`common.rs:139-178`) with hardcoded geometry (48m² floor area, 120m³ volume), single HVAC equipment, constant weather, and constant occupancy schedule. Real ResStock HPXML buildings (`crates/hares-io/src/resstock.rs`) have varied equipment configurations (heat pump + backup resistance, gas furnace + AC), multiple thermal zones with different thermostat schedules, and complex DER assets (batteries, EVs, PV) — all of which exercise different code paths in the HPXML parser, equipment resolver, and simulation engine. Synthetic data provides zero coverage of these production code paths in a benchmarking context.

**Code Location**: `benches/common.rs:30-39` (synthetic schedule: 4 rows), `benches/common.rs:42-106` (synthetic weather: uniform 19°C), `benches/common.rs:139-178` (synthetic TOML: minimal fixed config)

**Root Cause**: Benchmarks were bootstrapped for developer convenience with fast-to-generate synthetic data. ResStock data requires downloading building bundles from OEDI S3 (handled by `python/ochre_next/data/resstock.py:295`), which adds external dependency and longer setup time.

**Impact**: Benchmarks may overestimate performance because synthetic dwellings are simpler than real ResStock buildings; conversely, they may miss code paths that are slow for complex equipment configurations (e.g., heat pump coefficient-of-performance lookups, battery degradation models). Performance characteristics measured with synthetic data do not reliably predict real-world ResStock performance.

**Recommended benchmark**: Add a `bench_resstock.rs` that:
- Uses a pre-downloaded ResStock building bundle (e.g., 100 building HPXML + schedules from OEDI)
- Measures `Fleet::from_resstock()` construction time at 100, 1,000 building scale
- Measures `Fleet::simulate()` throughput (dwellings/sec) with real ResStock buildings
- Measures `aggregate()` with real ResStock output (varied equipment → varied output columns)
- Compares throughput vs synthetic data to quantify the realism gap

### Finding 5: No benchmark for multi-zone dwellings [Severity: low]

**Description**: All benchmarks use dwellings with a single thermal zone: the OCHRE sample HPXML has one `BuildingConstruction` zone, and `synthetic_toml_case()` defines a single `[geometry]` block with one `zone_volume_m3`. The simulation engine supports multi-zone dwellings (multiple `BuildingConstruction` elements in HPXML, each with independent thermostat schedules and heating/cooling equipment). Multi-zone simulation requires zone-to-zone heat transfer calculations that scale with zone count but are entirely absent from benchmarks.

**Code Location**: `benches/common.rs:139-178` — `synthetic_toml_case()` defines only one zone; `benches/common.rs:25-27` — OCHRE sample HPXML is single-zone

**Root Cause**: Single-zone benchmarks were the natural starting point since most ResStock dwellings are single-zone. Multi-zone performance has not been a priority so far.

**Impact**: Multi-zone dwelling performance (common in multi-family and mixed-use buildings) is unmeasured. Zone-to-zone heat transfer scaling behavior is unknown.

**Recommended benchmark**: Extend `single_building.rs` or add a new bench with TOML-cased multi-zone dwellings (2, 4, 8 zones) measuring per-step throughput.

## Summary
- **Total findings**: 5
- **Critical / High / Medium / Low**: 0 / 2 / 2 / 1

## Recommendations

1. **Add `benches/aggregation.rs`** (highest priority): Benchmark `aggregation::aggregate()` at 100, 1,000, and 10,000 dwelling scale with both 15-min and hourly resolution. Measure wall-clock time, allocations, and peak memory. Fleet studies at 10,000+ dwellings cannot be optimized without aggregation baselines.

2. **Add `benches/rl_episode.rs`** (second priority): Benchmark full RL episode lifecycle including `reset()`, `to_observation_vec()`, `apply_control()`, and N `step()` calls. Cover single-dwelling and vec-dwelling (16, 64, 256) at 96-step and 8760-step episode lengths. Include direct `batch_step_py()` measurement. RL training throughput depends on episode-level overhead, not just per-step cost.

3. **Add HELICS latency benchmarks** (third priority): Python-based (pytest-benchmark) benchmarks for federate creation, `request_time` grant cycle, and `_publish_results()` aggregate summing at 10–1,000 dwelling scale. Measure 96-step co-simulation end-to-end. Real-time co-simulation requires latency baselines for federate sizing.

4. **Add `benches/resstock.rs`**: Benchmark against real ResStock HPXML buildings (pre-downloaded bundle of 100+ buildings) to quantify the realism gap vs synthetic data. Measure construction throughput, simulation throughput, and aggregation throughput. Compare against synthetic baselines.

5. **Add multi-zone benchmarks**: Extend existing benchmarks or add a new bench with 2, 4, and 8-zone dwellings to establish zone-scaling performance baselines.

## References / Citations
- Aggregation implementation: `crates/hares-fleet/src/aggregation.rs:131` (`aggregate()`), `aggregation.rs:311-405` (`build_aggregate_batch()`)
- Fleet simulation bench (no aggregation): `benches/fleet.rs:34-37`
- RL single-step bench: `benches/rl_step.rs:14-51`
- RL episode environment: `python/ochre_next/rl/gym_env.py:402` (`reset()`), `gym_env.py:421` (`step()`)
- RL batch step helper: `crates/hares-python/src/py_gym.rs:50` (`batch_step_py()`)
- RL observation construction: `crates/hares-core/src/telemetry.rs:32-108` (`to_observation_vec()`)
- HELICS fleet publish: `python/ochre_next/helics/fleet.py:226-243` (`_publish_results()`)
- HELICS dwelling federate: `python/ochre_next/helics/dwelling.py:40` (`HELICSDwelling`), `dwelling.py:127` (`run()`)
- ResStock parser: `crates/hares-io/src/resstock.rs:224` (`parse_resstock_metadata()`)
- Synthetic data generators: `benches/common.rs:30-39` (schedule), `common.rs:42-106` (weather), `common.rs:139-178` (TOML)
- Benchmark Cargo.toml: `crates/hares-core/Cargo.toml:45-58` (three `[[bench]]` entries)
