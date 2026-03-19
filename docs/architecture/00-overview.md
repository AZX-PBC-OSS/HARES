# Residential Load Simulation Workbench — Architecture Overview

**Rev 7 — 2026-03-18**

A Rust rewrite of OCHRE — the NREL residential building energy simulation tool — that
eliminates Python overhead while preserving OCHRE's proven physics and improving its
architecture for extensibility, performance, and future adoption.

## The Problem

OCHRE is a useful, validated, ~15k LOC Python simulation engine for residential buildings.
A typical 1-year simulation takes ~300 seconds (~150 seconds with recent Numba JIT
optimizations). Profiling shows only ~20 seconds is actual physics computation — the
rest is Python/pandas overhead: per-timestep dict creation, DataFrame operations, object
allocation, and GIL contention.

This project rewrites the hot path in Rust while keeping OCHRE's physics models,
improving its data model, and setting it up for fleet-scale synthetic load generation
and reinforcement learning.

## Why Rewrite — Not Optimize

OCHRE is too slow and too inflexible for the use cases that motivate this project.
RL training requires sub-millisecond step latency and vectorized environments — Python's
GIL and per-step overhead make this infeasible. HELICS co-simulation needs deterministic
timing without GC pauses. Fleet-scale simulation (1000+ dwellings) needs parallel
execution that Python's single-threaded hot path cannot provide. Custom equipment models
need to plug in without inheriting from a deep class hierarchy. These are the reasons
for the rewrite — RL, HELICS, fleet, and extensibility are v1 requirements, not
future aspirations.

## Goals

1. **Correct OCHRE physics in Rust** — Same RC envelope, biquadratic curves, stratified
   tank models, battery chemistry. Validated against OCHRE output on identical inputs.

2. **Significantly faster single-building simulation** — By eliminating Python overhead,
   not by inventing new algorithms. Actual speedup will be established by profiling.

3. **Fleet-scale synthetic generation** — Run thousands of dwellings via rayon
   parallelism. No exotic SoA vectorization needed for v1 — just concurrent independent
   simulations sharing nothing.

4. **RL Gymnasium environments** — Deterministic reset, fast save/load, vectorized
   environments inside Rust for batch RL training.

5. **HELICS co-simulation** — Python-orchestrated HELICS (proven NREL pattern). Rust
   engine exposes `step()` + pub/sub via PyO3; Python handles HELICS broker.

6. **Better data model than OCHRE** — Fix OCHRE's single-instance-per-equipment-type
   limitation. Support multiple solar arrays, multiple batteries, multiple EVs. Clean
   Equipment trait with composition over inheritance.

7. **Extensible control interfaces** — Typed control signals that map cleanly to
   real-world protocols (OpenADR, CTA-2045, IEEE 2030.5). Not full protocol adapters
   in v1 — just a control signal taxonomy that doesn't need rewriting when we add them.

8. **ResStock native** — Ingest HPXML 4.0 + schedule CSV + EPW from ResStock 2024/2025
   bundles directly.

## What This Is NOT

- **Not a physics rewrite.** OCHRE's thermal models, biquadratic curves, and coupling
  strategy are correct for residential simulation. We reimplement them in Rust, we don't
  reinvent them. Where OCHRE has known gaps (battery standby power, multi-array PV), we
  fix those — but we don't chase EnergyPlus-level surface heat balance.

- **Not over-engineered.** No WASM sandbox, no FMU process isolation, no multi-rate
  time stepping, no custom domain solvers, no YAML schema language. Those can come later
  if needed. The architecture should *allow* them without *requiring* them.

- **Not a grid simulator.** Electrical modeling is load/generation accounting. Detailed
  grid physics belongs in OpenDSS/GridLAB-D via HELICS.

## Design Principles

### Equipment as Independent Instances

Each equipment instance owns its state, runs independently, and communicates through
typed ports. Unlike OCHRE's one-instance-per-type model, a dwelling can have multiple
PV arrays, multiple batteries, multiple EVs — each with independent parameters and
control signals.

### Composition Over Inheritance

OCHRE uses deep class hierarchies (Equipment → Generator → Battery). This project uses
a flat `Equipment` trait with composable physics components (thermal model, electrical
model, control logic). New equipment types implement the trait — they don't inherit from
a chain of base classes.

### Port-Based Exchange

Equipment declares typed ports (thermal, electrical, fuel) and writes contributions each
step. The engine sums contributions per zone/bus. No shared mutable buffers. No direct
zone writes.

### OCHRE-Compatible API Surface

A Python compatibility layer (`ochre_next.compat.Dwelling`) accepts the same constructor
kwargs, equipment names, and control signal dicts as OCHRE. Existing scripts work with
minimal changes. The native API (`ochre_next.Dwelling`) adds typed control signals,
multi-instance equipment, and new capabilities.

### Clear Separation for Testability

Every physics computation is a pure function: inputs in, outputs out. No hidden state
mutation, no global variables, no implicit coupling through shared DataFrames. Each
equipment model can be unit-tested in isolation with fixed inputs.

## Architecture at a Glance

```
┌──────────────────────────────────────────────────────────┐
│                   SIMULATION ENGINE (Rust)                │
│                                                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────┐ │
│  │ REGISTRY │  │  CLOCK   │  │SCHEDULER │  │ OUTPUT  │ │
│  │          │  │          │  │          │  │         │ │
│  │ Equip.   │  │ time     │  │ Dep.     │  │ Arrow/  │ │
│  │ factories│  │ time_res │  │ order    │  │ Parquet │ │
│  └──────────┘  └──────────┘  └──────────┘  └─────────┘ │
│                                                          │
│  ┌──────────────────────────────────────────────────┐    │
│  │              EQUIPMENT INSTANCES                  │    │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐  │    │
│  │  │HVAC  │ │HVAC  │ │WH    │ │Batt  │ │Batt  │  │    │
│  │  │Heat  │ │Cool  │ │      │ │  #1  │ │  #2  │  │    │
│  │  └──────┘ └──────┘ └──────┘ └──────┘ └──────┘  │    │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐           │    │
│  │  │PV #1 │ │PV #2 │ │EV #1 │ │Loads │           │    │
│  │  └──────┘ └──────┘ └──────┘ └──────┘           │    │
│  └──────────────────────────────────────────────────┘    │
│           │ write ports              ▲ read env          │
│  ┌────────┴──────────────────────────┴───────────┐      │
│  │         PORT ACCUMULATION + ENVELOPE           │      │
│  │  Thermal gains → RC solve → zone temperatures  │      │
│  │  Electrical  → P/Q summation per bus           │      │
│  └────────────────────────────────────────────────┘      │
│                                                          │
│  ┌────────────────────────────────────────────────┐      │
│  │            CONTROL SIGNAL DISPATCH              │      │
│  │  Typed ControlSignal → equipment by ID/name     │      │
│  └────────────────────────────────────────────────┘      │
└──────────────────────────────────────────────────────────┘
              ▲ PyO3 bindings
┌─────────────┴────────────────────────────────────────────┐
│                    PYTHON LAYER                           │
│  ochre_next.Dwelling (native API)                        │
│  ochre_next.compat.Dwelling (OCHRE migration)            │
│  DwellingGymEnv (RL Gymnasium)                           │
│  HELICS orchestration                                    │
│  SAM / PyBaMM adapters (LUT generation)                  │
└──────────────────────────────────────────────────────────┘
```

## Implementation Language

**Rust** with **PyO3** Python bindings.

- Ownership model prevents shared-state mutation bugs at compile time
- `rayon` for data-parallel fleet execution (per-dwelling, no shared state)
- No GC pauses for deterministic co-simulation timing
- `uom` crate for compile-time dimensional analysis — catches unit/dimension errors at compile time with zero runtime cost

Python is the configuration, analysis, control scripting, and external tool integration
layer (SAM, PyBaMM, HELICS, Gymnasium).

## Key Crates

| Crate | Purpose |
|-------|---------|
| `rayon` | Per-dwelling parallel fleet execution |
| `nalgebra` | Matrix operations for RC solve |
| `arrow` + `parquet` | Columnar output, streaming export |
| `pyo3` + `maturin` | Python bindings |
| `serde` + `toml` | Configuration parsing |
| `uom` | Compile-time units of measure (zero runtime cost) |
| `chrono` | Date/time |
| `rand_chacha` | Deterministic RNG |
| `tracing` | Structured logging |

## Document Index

| Document | Contents |
|----------|----------|
| [01 — Simulation Core](01-sim-core-and-solver.md) | Timestep loop, coupling, scheduling |
| [02 — Equipment & Ports](02-equipment-and-ports.md) | Equipment trait, port types, multi-instance |
| [03 — Control Interfaces](03-control-interfaces.md) | Control signals, RL Gym, HELICS |
| [04 — Data Ingestion & Fleet](04-data-ingestion-and-fleet.md) | ResStock, HPXML, fleet execution |
| [05 — External Tool Integration](05-external-tools.md) | SAM, PyBaMM, Python equipment adapters |
| [06 — Input/Output](06-input-output.md) | Validation, output formats, OCHRE compatibility |
| [07 — Testing & Verification](07-testing-and-verification.md) | OCHRE parity, BESTEST, numerical invariants, test matrix |
| [08 — Operations & Performance](08-operations.md) | Targets, profiling, failure handling |
| [09 — Roadmap](09-roadmap.md) | Phased implementation plan |
| [Appendix — Physics Improvements](appendix-physics-improvements.md) | Verified equations, coefficients, data interfaces |

## Improvements Over OCHRE

| OCHRE Limitation | ochre_next Improvement |
|-----------------|----------------------|
| Single instance per equipment type | Multiple PV arrays, batteries, EVs per dwelling |
| Deep inheritance hierarchy | Flat trait + composition |
| Per-timestep DataFrame/dict overhead | Preallocated typed structs |
| No battery standby power | Configurable standby consumption |
| Battery self-discharge 0.05%/day (Li-NMC only) | Chemistry-aware defaults via PyBaMM LUTs |
| Outdated SAM 2020 cell parameters | SAM adapter generates current parameters |
| Single PV array | Multiple arrays with independent orientation/capacity |
| Python-only equipment models | Rust equipment + Python adapter for custom models |
| GIL-bound, single-threaded | rayon parallel fleet execution |
| Tightly coupled, hard to test | Pure-function physics, independent equipment |
| Broken thermostat deadband logic | Clean deadband model with consistent data source |
| Air density hardcoded (sea level) | Altitude/temperature-corrected air density |
| Hardcoded terrain/wind coefficients | Configurable terrain, shielding, wind exposure |
| Hardcoded HVAC supply air temps | Configurable per equipment instance |
| No dehumidifier / humidity control | Dehumidifier equipment type via latent port |
| Inconsistent error handling (bare except) | Rust `Result<T, E>` — no silent failures |
| Unexplained irradiance corrections | Documented in PHYSICS_DECISIONS.md with rationale |
| Cannot modify envelope mid-simulation | Control signal support for parameter updates |
