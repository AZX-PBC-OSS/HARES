---
id: ACTOR-014
title: Actor registry — user-extensible actors from Rust and Python
kind: implement
depends_on:
  - ACTOR-003
files_to_touch:
  - crates/hares-core/src/actor.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-python/src/py_dwelling.rs
  - crates/hares-python/src/py_actor.rs
references:
  - crates/hares-equipment/src/registry.rs
  - crates/hares-equipment/src/lib.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

Library users need to register custom actors — both in Rust (native crate consumers) and Python (via pyo3 bindings) — to interact with internal equipment programmatically via the actor model. This mirrors the existing `EquipmentRegistry` pattern but for actors.

Use cases:
- Rust: custom RL agent, grid operator model, fleet controller
- Python: research scripts, Gym environments, custom occupant behavior models

## Work to Do

### Rust-side actor registry

- [ ] Create `ActorRegistry` in `crates/hares-core/src/actor.rs` (or `actor_registry.rs`)
- [ ] `type ActorFactory = Box<dyn Fn(ActorConfig) -> Box<dyn Actor> + Send + Sync>`
- [ ] `ActorRegistry::register(name: &str, factory: ActorFactory)`
- [ ] `ActorRegistry::create(name: &str, config: ActorConfig) -> Result<Box<dyn Actor>>`
- [ ] Built-in actors registered by default (IdealThermostat, Occupant, DRCompliance when implemented)
- [ ] `Dwelling::add_actor(actor: Box<dyn Actor>)` already exists from ACTOR-008
- [ ] Add `ActorConfig` struct — similar to `EquipmentConfig` with name, parameters map
- [ ] Expose registry on Dwelling for user access: `dwelling.actor_registry()` or pass at construction

### Python-side actor bindings

- [ ] Create `crates/hares-python/src/py_actor.rs`
- [ ] `PyActor` wrapper: Python class that implements the Rust `Actor` trait via pyo3
- [ ] Python users subclass a base class and implement `decide(env) -> list[DispatchRequest]`
- [ ] `PyDwelling.add_actor(actor: PyActor)` — registers a Python actor
- [ ] `PyDwelling.add_actor_by_name(name: str, config: dict)` — creates from registry
- [ ] DispatchRequest and ControlSignal must be exposed to Python (pyo3 wrappers)
- [ ] EnvironmentState must be readable from Python (already partially exposed?)

### Design constraints

- [ ] Actors are `Send + Sync` — Python actors need GIL-aware wrapping via `pyo3::Python::with_gil` in the `decide()` bridge
- [ ] Actor `decide()` is called every timestep — must be fast. Python actors should batch decisions or use vectorized logic. Add a design spike subtask: benchmark pyo3 GIL acquire/release overhead per timestep to verify <1ms latency at 1-min resolution
- [ ] Actor ordering: actors run in registration order, last write wins (see ACTOR-008). Document this in Python API docs.

## Files to Touch

- `crates/hares-core/src/actor.rs`: ActorRegistry, ActorConfig
- `crates/hares-core/src/dwelling/mod.rs`: Wire registry into Dwelling construction
- `crates/hares-python/src/py_actor.rs`: **New** — Python actor bindings
- `crates/hares-python/src/py_dwelling.rs`: add_actor, add_actor_by_name methods

## Performance Constraints

- [ ] Rust actors: zero per-step allocation. `decide()` writes into pre-allocated buffer.
- [ ] Python actors: GIL acquire/release is the main cost. Benchmark to verify <1ms overhead per step at 1-min resolution.
- [ ] Python `decide()` receives a read-only view of EnvironmentState (immutable reference via pyo3), returns dispatch requests into a pre-allocated list.
- [ ] Consider batched Python actor calls: if multiple Python actors exist, acquire GIL once and call all of them before releasing. Avoid GIL churn.
- [ ] DispatchRequest and ControlSignal Python wrappers should be lightweight (no deep copies of environment data).

## Measures of Success

- [ ] Rust users can register and add custom actors to a Dwelling
- [ ] Python users can subclass a base actor and have decide() called each step
- [ ] Control signals from Python actors flow to equipment correctly
- [ ] Built-in actors are auto-registered

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] Python: `from hares import Actor; class MyActor(Actor): ...` compiles and runs
