# Rust Primer for HARES

Key Rust language features and patterns used in the HARES codebase. This
is not a Rust tutorial — the [Rust Book](https://doc.rust-lang.org/book/)
covers the language comprehensively. This page highlights what you will
encounter navigating the code and why HARES uses these patterns.

---

## Enums as Data Models

Rust enums can carry data in each variant, unlike C-style enums which are
just integer constants. HARES uses this for type-safe domain modelling
throughout. For example, `PortContribution` represents what an equipment
model outputs each timestep:

```rust
pub enum PortContribution {
    Thermal  { zone: ZoneId, sensible_gain_w: f64, latent_gain_w: f64, ... },
    Electrical { active_power_kw: f64, ... },
    Fuel     { fuel_type: FuelType, consumption_w: f64 },
    Fluid    { loop_id: LoopId, flow_rate_kg_s: f64, ... },
}
```

Each variant carries different fields appropriate to that domain. Pattern
matching (`match`) is used to handle each variant, and the compiler
rejects incomplete matches — if a new variant is added, every `match`
must be updated or the code won't compile.

`ControlSignal` follows the same pattern: one variant per control type
(setpoints, power limits, mode overrides, etc.).

---

## Traits (Interfaces)

Traits define shared behaviour that different types can implement. The
`Equipment` trait is the central abstraction in HARES — all equipment
types (HVAC, battery, PV, water heater, EV) implement it:

```rust
pub trait Equipment {
    fn step(&mut self, env: &EnvironmentState, ports: &mut PortSlots, dt: Duration);
    fn telemetry(&self) -> &Telemetry;
    fn apply_control(&mut self, signal: &ControlSignal) -> Result<()>;
    // ...
}
```

This enables heterogeneous collections: all equipment is stored as
`Vec<Box<dyn Equipment>>` and stepped uniformly regardless of concrete
type.

The `DomainSolver` trait serves a similar role for solvers, allowing
custom solver implementations to be registered at runtime.

---

## Iterators and Lazy Evaluation

HARES favours iterators over indexed `for` loops. Iterator chains
compose operations (`.map()`, `.filter()`, `.zip()`, `.fold()`,
`.sum()`) and the compiler optimises them into tight machine code with
no intermediate allocations:

```rust
let total: f64 = ports.thermal.iter()
    .map(|t| t.sensible_gain_w)
    .sum();
```

Iterators are lazy — they do no work until consumed (by `.sum()`,
`.collect()`, `.for_each()`, etc.). This means chaining multiple
operations does not allocate intermediate collections.

---

## Ownership and Borrowing

Rust's ownership system prevents data races and use-after-free at
compile time. Every value has a single owner; when the owner goes out of
scope, the value is freed. References (`&` for shared, `&mut` for
exclusive) allow temporary access without transferring ownership.

In HARES, the key borrowing patterns are:

- **`&EnvironmentState`** — equipment receives a shared (read-only)
  reference to the environment each timestep. Multiple equipment models
  read the same environment simultaneously.
- **`&mut PortSlots`** — equipment receives an exclusive (mutable)
  reference to write its contributions. Only one equipment writes at a
  time.
- **`Box<dyn Equipment>`** — heap-allocated trait object with single
  ownership, stored in a `Vec`. The dwelling owns all its equipment.

---

## Pre-Allocated Buffers

The simulation loop runs hundreds of thousands of timesteps per year.
Allocating heap memory inside that loop is avoided because the
allocation overhead compounds across timesteps and fleet-scale runs.

HARES pre-allocates buffers at initialisation and reuses them each
timestep:

- **Swap-and-zero**: the thermal solver moves its input vector out via
  `std::mem::replace()`, zeroes it in-place with `.fill(0.0)`,
  populates it, then moves it back. No allocation occurs.
- **Fixed-size stack arrays**: port accumulators use `[f64; N]` for
  per-zone category breakdowns instead of `Vec`, keeping the data on
  the stack.
- **Column-major indexed lookup**: weather and schedule data are stored
  as `Vec<f64>` per column, indexed by timestep for O(1) access with
  no per-step allocation.
- **HashMap reuse**: latent gain buffers use `std::mem::take()` +
  `.clear()` to reset without deallocating bucket storage.

---

## Feature-Gated Code

Code wrapped in `#[cfg(feature = "...")]` attributes is only compiled
when the named feature flag is enabled via `-F`. When the feature is
off, the compiler excludes that code entirely — it does not exist in the
binary.

```rust
#[cfg(feature = "observe")]
fn capture_snapshot(&self) -> StepSnapshot {
    // only compiled when `observe` feature is enabled
}
```

HARES uses this for optional instrumentation: the observer system,
invariant checks, and profiling are all feature-gated. See
[Feature Flags](development.md#feature-flags) for the full list.

---

## Further Reading

- [The Rust Programming Language](https://doc.rust-lang.org/book/) — the official book
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/) — learn through annotated examples
- [The Cargo Book](https://doc.rust-lang.org/cargo/) — workspaces, dependencies, features
