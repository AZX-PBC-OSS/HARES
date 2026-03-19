---
id: HARES-018
title: "hares-equipment — Equipment Trait, Registry, and Config"
kind: implement
depends_on: [HARES-001, HARES-002, HARES-003, HARES-004]
files_to_touch:
  - crates/hares-equipment/src/lib.rs
  - crates/hares-equipment/src/registry.rs
  - crates/hares-equipment/src/config.rs
  - crates/hares-types/src/telemetry.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
All concrete equipment implementations depend on a shared `Equipment` trait, a config type for initialisation parameters, and a registry that maps OCHRE equipment names to factory functions. Establishing these in `hares-equipment` first unblocks every subsequent equipment implementation ticket (HARES-019 through HARES-032).

## Work to Do
- [ ] Define `Equipment` trait with methods: `descriptor() -> &EquipmentDescriptor`, `ports() -> &[PortDeclaration]`, `init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> Result<()>`, `apply_control(&mut self, signal: &ControlSignal) -> Result<()>`, `update_control(&mut self, env: &EnvironmentState) -> OperatingMode`, `step(&mut self, env: &EnvironmentState, dt: Duration, ports: &mut PortSlots) -> Result<(), HaresError>`, `telemetry(&self) -> Telemetry`, `save_state(&self) -> Vec<u8>`, `load_state(&mut self, state: &[u8]) -> Result<()>`
  - Returning `Result` allows the engine to quarantine a failing dwelling without crashing the fleet. Known-bad inputs (NaN, negative capacity) must return `Err` rather than writing garbage to ports.
- [ ] `apply_control()` must return `Err` when the signal type is not present in `descriptor().control_capabilities` — checked at the trait dispatch boundary before forwarding to the concrete implementation
- [ ] Move `Telemetry` to a dedicated module in `hares-types` (e.g. `crates/hares-types/src/telemetry.rs`), not `config.rs`. `Telemetry` is consumed by the Python layer and RL observation space construction; it must not be buried in equipment-internal config
- [ ] Define `Telemetry` struct as a newtype over `HashMap<String, f64>` with convenience accessors; re-export from both `hares-types` and `hares-equipment`
- [ ] Define `EquipmentConfig` struct holding initialisation parameters (name, OCHRE class, raw config map)
- [ ] Define `EquipmentRegistry` mapping OCHRE class name strings to boxed factory closures. Factory pattern: factory closure accepts `EquipmentConfig` and returns an **uninitialised** `Box<dyn Equipment>`; the caller is responsible for calling `.init(config, env)` separately after construction. This two-phase pattern ensures `init` always has access to `EnvironmentState` (which is not available at registration time)
- [ ] Factory closure type must be `Box<dyn Fn(EquipmentConfig) -> Box<dyn Equipment> + Send + Sync>` — the `Send + Sync` bound is required for safe use across threads during fleet simulation
- [ ] Implement `EquipmentRegistry::register`, `get`, and `create` methods
- [ ] Re-export all public types from `lib.rs`
- [ ] All `save_state`/`load_state` implementations must use `postcard` for serialization (`postcard::to_allocvec` / `postcard::from_bytes`). This ensures cross-equipment checkpoint coherence. Add `postcard` to workspace Cargo.toml dependencies.
- [ ] To avoid per-step heap allocation, the `Telemetry` HashMap should be pre-allocated at init time and updated in-place via `telemetry_mut() -> &mut Telemetry` or equivalent. The `telemetry(&self) -> Telemetry` method should return a clone of the pre-allocated map, not construct a new one.

## Files to Touch
- `crates/hares-equipment/src/lib.rs`: crate root — re-export trait, registry, config, and Telemetry
- `crates/hares-equipment/src/registry.rs`: new file — `EquipmentRegistry` with factory-function map
- `crates/hares-equipment/src/config.rs`: new file — `EquipmentConfig` (not `Telemetry`)
- `crates/hares-types/src/telemetry.rs`: new file — `Telemetry` type (consumed by Python layer and RL)

## Measures of Success
- [ ] `Equipment` trait compiles and is object-safe (`dyn Equipment` is valid)
- [ ] `EquipmentRegistry::create` returns `Err` for unknown equipment names
- [ ] `apply_control()` called with a signal type not in `control_capabilities` returns `Err` without panicking
- [ ] `Telemetry` can be constructed from an iterator of `(String, f64)` pairs
- [ ] `EquipmentConfig` is `Clone` and `Debug`
- [ ] All public types are `Send + Sync`
- [ ] Factory closure stored in registry is `Send + Sync`
- [ ] `Telemetry` is importable from `hares-types` without depending on `hares-equipment`
- [ ] `ExecutionStage` assignment is verified per equipment type at registry registration time (or documented as caller responsibility)
- [ ] `save_state` uses `postcard::to_allocvec` and `load_state` uses `postcard::from_bytes`

## Performance Notes
- **P0 — Telemetry hot-loop updates**: `Telemetry::set(&mut self, key: &str, value: f64)` added for in-place updates when the key already exists, avoiding `String` allocation on every equipment step. Equipment implementations call `set()` in their `step()` method instead of re-inserting with `String::from`.
- **P0 — Telemetry by-ref**: `Equipment::telemetry()` now returns `&Telemetry` instead of cloning the entire `HashMap`. The output writer and RL observation builder read telemetry by reference; cloning is deferred to the rare cases that actually need ownership.

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
