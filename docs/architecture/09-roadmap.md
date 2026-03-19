# Implementation Roadmap

A focused port of OCHRE to Rust with architectural improvements. Each phase produces
a working, testable system — not a partial framework.

## Phase 1: Core Kernel + Envelope (Months 1–3)

**Deliverable**: A Rust library that can solve a single-zone RC envelope with weather
inputs and produce correct zone temperatures. Plus the HPXML parser.

- RC envelope state-space solver (A_d, B_d matrices via ZOH discretization)
- HPXML 4.0 parser in Rust → building geometry, equipment specs, zones
- Schedule CSV loader with zero-order hold resampling
- EPW weather file parser with per-surface solar irradiance calculation
- Biquadratic curve evaluation
- Psychrometric functions (ported from OCHRE's JIT-compiled versions)
- Port accumulation model (thermal, electrical, fuel, fluid, custom)
- Equipment trait definition + DomainSolver trait
- `uom`-based compile-time unit system for physics quantities
- Infiltration (ASHRAE/ELA/ACH — matching OCHRE)
- Humidity mass balance
- PyO3 bindings: basic `Dwelling.from_hpxml()` → `simulate()` → results
- OCHRE compat constructor (`ochre_next.compat.Dwelling`)

**Validation**:
- RC golden cases (known analytical solutions)
- RC eigenvalue stability checks (continuous + discrete)
- HPXML property parity: parse 10 ResStock bundles, compare equipment sets
- Psychrometric function unit tests against ASHRAE tables

## Phase 2: Equipment + Single-Building (Months 3–6)

**Deliverable**: A complete single-building simulator matching OCHRE's output on the
same inputs. Both APIs working.

- All HVAC heating types (furnace, boiler, baseboard, heat pump, mini-split)
- All HVAC cooling types (AC, room AC, ASHP cooler, mini-split cooler)
- Water heater models (resistance, gas, HPWH, tankless) with stratified tank
- PV model (PVWatts equations, **multiple array support**)
- Battery model with improvements:
  - Configurable standby power (OCHRE gap)
  - Chemistry-aware defaults via SAM/PyBaMM adapters
  - Multiple battery instance support
  - Thermal model with zone heat feedback
- EV model (**multiple EV support**, stochastic schedule)
- Generator (gas generator, fuel cell — with thermal port for CHP waste heat)
- Scheduled and event-based loads
- Dehumidifier equipment type (OCHRE gap — writes latent port)
- Full OCHRE equipment name registry
- OCHRE bug fixes:
  - Clean thermostat deadband model (OCHRE's is broken)
  - Configurable HVAC supply air temperatures (OCHRE hardcodes)
  - Altitude/temperature-corrected air density (OCHRE hardcodes sea level)
  - Configurable terrain/shielding/wind exposure coefficients
- Heat pump splitting (HPXML → heater + cooler)
- `nested_update` override semantics for Equipment kwargs
- ZIP voltage-dependency model
- Multi-instance equipment in native API
- OCHRE compat API: `simulate()`, `update_model()`, `generate_results()`
- Output: CSV and Parquet with OCHRE-compatible column names
- Verbosity levels matching OCHRE
- SAM adapter for PV LUT generation
- SAM/PyBaMM adapters for battery parameters

**Validation**:
- OCHRE output parity on 10 reference buildings (30-day summer + winter each)
- Equipment fixture tests (fixed inputs → expected outputs)
- Multi-instance tests (2 batteries, 2 PV arrays behave independently)
- Benchmark suite: single-building profiled against OCHRE on same inputs
- BESTEST Cases 600FF/900FF/640 as firm deliverables

## Phase 3: Control + RL + HELICS (Months 6–9)

**Deliverable**: The engine works as an RL environment and a HELICS co-sim federate.
Custom Python controllers work.

- Typed ControlSignal dispatch (by instance name and end-use category)
- OCHRE compat control signal mapping (string dict → typed)
- Custom Python controller API (PythonEquipment adapter)
- Custom Rust equipment registration
- Gymnasium RL interface (`DwellingGymEnv`)
  - Deterministic reset (seed-based)
  - Fast save/load
  - Configurable observation/action spaces
- `VecDwellingGymEnv` — rayon-parallel vectorized environments
- HELICS co-simulation via Python orchestration
  - Single dwelling federate
  - Fleet-as-single-federate pattern
- Deterministic RNG (hierarchical ChaCha8 seeding)

**Validation**:
- RL determinism: same seed → identical trajectory
- HELICS round-trip: pub/sub with mock aggregator
- Python controller: custom battery dispatch produces correct output
- Control signal parity: same OCHRE control dict → same equipment behavior

## Phase 4: Fleet + Polish (Months 9–12)

**Deliverable**: Production-ready fleet simulation with ResStock ingestion.

- ResStock metadata parquet ingestion
- Fleet execution via rayon (`par_iter` over independent dwellings)
- `sample_weight` propagation through all aggregations
- Fleet output: per-dwelling metrics, periodic timeseries, weighted aggregate
- Checkpoint/restart for long fleet runs
- Failure isolation (`catch_unwind` per dwelling)
- Fleet progress reporting
- Performance benchmarking against targets
- Documentation: API reference, migration guide from OCHRE
- `PHYSICS_DECISIONS.md` documenting all physics modeling choices and OCHRE divergences

**Validation**:
- Fleet scale: 1000 dwellings without OOM on 32 GB machine
- Weighted aggregation correctness
- Checkpoint restart produces identical continuation
- 63-case regression corpus (if time allows: + BESTEST subset)

## What's Explicitly Deferred

Items below are not in v1 scope. The architecture *accommodates* them but does not
guarantee zero-cost addition. SoA batching and GPU dispatch in particular would
require an executor-layer refactor — the v1 per-instance `Equipment::step(&mut self)`
AoS model is not directly batchable. The port-based exchange model and Equipment trait
semantics carry forward; the execution and storage layer would change.

| Feature | Why Deferred | How It Fits Later |
|---------|-------------|-------------------|
| WASM plugin sandbox | No demand yet | Equipment trait is the extension point |
| FMU import | Niche use case | Equipment trait + config-driven loading |
| Protocol adapters (OpenADR, CTA-2045, etc.) | Control signal taxonomy ready | Each adapter = translation layer to ControlSignal |
| SoA fleet vectorization | Simple par_iter is fast enough | Requires executor refactor: batch by topology key + equipment signature, rewrite kernels to operate on SoA tables. Port semantics and Equipment trait carry forward; storage/dispatch do not |
| Archetype clustering | Not needed for v1 fleet | k-prototype clustering; compute optimization only — destroys stochastic diversity |
| Streaming schedule/weather | ~5 MB/dwelling is manageable for v1 | Iterator-based ingestion, no full year materialization per dwelling |
| Multi-rate time stepping | Sequential at single rate is fine | Needs sub-stepping and ZOH coupling |
| Iterative coupling | Sequential matches OCHRE, works well | Needs convergence loop + double buffering |
| Parallel (Jacobi) coupling | Sequential is simpler and more accurate | Needs double-buffered environment state |
| GPU acceleration | CPU is fast enough | Requires SoA executor first, then alternative kernel dispatch (WGPU compute) |
| Surface heat balance / solar distribution | EnergyPlus territory | Enhanced thermal solver |
| Multizone airflow | CONTAM territory | New domain solver + port type |
| Stochastic event-based loads | OCHRE has stubs (`NotImplementedError`) | Equipment type with configurable event distributions |
| Mid-simulation envelope changes | Not possible in OCHRE | ParameterUpdate control signal for DR envelope setback |
| Multifamily shared systems | OCHRE: unit-by-unit with adiabatic boundaries only | Inter-unit thermal coupling via port model (v2) |

### Coupling Mode Design Notes (Future Reference)

The sequential coupling mode (v1) is fully specified in `01-sim-core-and-solver.md`.
For future reference, the parallel and iterative coupling modes are designed as follows:

- **Parallel (Jacobi)**: All equipment reads frozen previous-step environment, steps
  concurrently. Requires double-buffered environment state. Per-equipment port slot
  arrays with deterministic ordered reduction (sorted by equipment ID) for bitwise
  reproducibility.
- **Iterative**: Equipment + envelope loop repeats until zone temperature convergence.
  Under-relaxation (ω=0.7, configurable) prevents oscillation. Fallback to sequential
  on divergence (residual increases for 3 consecutive iterations).
- **Automatic escalation**: Equipment declares a `CouplingRequirement` field on its
  descriptor (Loose, TightThermal, TightElectrical — added to `EquipmentDescriptor` when
  coupling modes ship). If any equipment declares `TightThermal` and the user selected
  parallel, the engine escalates to iterative for that dependency group — preventing
  silent accuracy loss without requiring users to understand coupling theory.

These are fully designed but not scheduled for v1. The architecture (port-based exchange,
equipment communicating only through typed ports) supports adding them without core changes.

## Anti-Patterns Avoided

| Anti-Pattern | OCHRE | ochre_next |
|-------------|-------|-----------|
| Deep inheritance | Equipment → Generator → Battery | Flat trait + composition |
| Per-timestep DataFrame ops | schedule lookups via pandas | Preallocated arrays, direct indexing |
| Per-timestep dict creation | control signal routing | Typed ControlSignal enum |
| Single-instance equipment | One battery, one PV | Multiple instances with independent state |
| Python-only hot path | GIL-bound, single-threaded | Rust hot path, GIL released |
| Monolithic dwelling state | Shared DataFrames between equipment | Equipment owns its state |
| Fixed execution order | Hardcoded in Dwelling.update_model | Fixed 4-stage order matching OCHRE's proven causality; generalizable to dependency graph post-v1 |
