---
id: ACTOR-014
title: Actor registry — user-extensible actors from Rust and Python
kind: implement
depends_on:
  - ACTOR-003
files_to_touch:
  - crates/hares-core/src/actor_registry.rs
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

- [x] Create `ActorRegistry` in `crates/hares-core/src/actor_registry.rs`
- [x] `type ActorFactory = Box<dyn Fn(ActorConfig) -> Box<dyn Actor> + Send + Sync>`
- [x] `ActorRegistry::register(name: &str, factory: ActorFactory)`
- [x] `ActorRegistry::create(config: ActorConfig) -> Result<Box<dyn Actor>>`
- [x] Built-in actors registered by default (IdealThermostat, Occupant, DrCompliance)
- [x] `Dwelling::add_actor(actor: Box<dyn Actor>)` already exists from ACTOR-003
- [x] Add `ActorConfig` struct — similar to `EquipmentConfig` with name, actor_type, parameters map
- [x] Export `ActorRegistry` and `ActorConfig` from `hares-core/src/lib.rs`

### Python-side actor bindings

- [x] Create `crates/hares-python/src/py_actor.rs`
- [x] `PyActor` base class for Python subclassing
- [x] `PyActorWrapper` implements Rust `Actor` trait via GIL-aware `decide()` bridge
- [x] `PyDwelling.add_actor(actor: PyActor)` — registers a Python actor
- [x] `PyDwelling.add_actor_by_name(actor_type: str, name: str, params: dict)` — creates from registry
- [x] Strongly typed `PyDispatchRequest`, `PySignal`, `PyMode`, `PyPriority` Python classes

### Design constraints

- [x] Actors are `Send + Sync` — Python actors use `Python::try_attach()` for GIL-aware wrapping
- [x] Actor ordering: actors run in registration order, last write wins (see ACTOR-003)
- [x] Strong typing: `PyMode`, `PyPriority`, `PySignal` enums prevent string-based errors

## Files Touched

- `crates/hares-core/src/actor_registry.rs`: **New** — ActorRegistry, ActorConfig
- `crates/hares-core/src/lib.rs`: Export actor_registry module
- `crates/hares-core/src/dwelling/mod.rs`: Add `add_actor_by_name()` method
- `crates/hares-python/src/py_actor.rs`: **New** — Python actor bindings with strong types
- `crates/hares-python/src/py_dwelling.rs`: `add_actor()`, `add_actor_by_name()` methods
- `crates/hares-python/src/lib.rs`: Export PyActor, PyDispatchRequest, PyMode, PyPriority, PySignal

## Measures of Success

- [x] Rust users can register and add custom actors to a Dwelling
- [x] Python users can subclass `Actor` base class and implement `decide(env)`
- [x] Control signals from Python actors flow to equipment correctly
- [x] Built-in actors are auto-registered

## Tests Added

### hares-core (actor_registry.rs)
- `actor_config_new_creates_empty_params`
- `actor_config_with_param_adds_parameter`
- `actor_registry_new_registers_builtins`
- `actor_registry_create_ideal_thermostat`
- `actor_registry_create_occupant`
- `actor_registry_create_dr_compliance`
- `actor_registry_create_unknown_type_returns_error`
- `actor_registry_get_returns_factory`

## Verification

- [x] `cargo build --workspace --exclude hares-python` passes
- [x] `cargo test --workspace --exclude hares-python` passes
- [x] `cargo clippy --workspace --exclude hares-python` passes
- [x] `cargo build -p hares-python` passes

## Implementation Notes

### Design

The implementation mirrors the `EquipmentRegistry` pattern:
- `ActorConfig` holds name, actor_type, and a `HashMap<String, ConfigValue>` for parameters
- `ActorRegistry::new()` auto-registers built-in actors (IdealThermostat, Occupant, DrCompliance)
- `register()` allows custom actor registration
- `create()` instantiates actors from configuration

### Python Bindings

Python actors use strongly typed enums:
- `PyMode`: Off, Heating, Cooling, Standby
- `PyPriority`: Schedule, UserOverride, Grid, Safety
- `PySignal`: ThermalSetpoint, ModeOverride, LoadFraction, PowerLimit

This prevents runtime errors from string-based APIs.

### GIL Handling

`PyActorWrapper::decide()` uses `Python::try_attach()` which:
- Returns `Some(result)` if called from Python context (GIL available)
- Returns `None` if GIL not available — now logs `tracing::error!` to surface the issue

## Post-Review Fixes (2026-03-25)

### CRITICAL

#### C-1: Python::try_attach error logging
**Issue**: `try_attach` returning `None` silently dropped all actor decisions.
**Fix**: Added `tracing::error!` when GIL unavailable — surfaces the issue clearly in logs.

### HIGH

#### H-1: from_py_object placement
**Issue**: Attribute was on `PyDispatchRequest` but pyo3 warnings indicated `PySignal`, `PyMode`, `PyPriority` needed it.
**Fix**: Moved `from_py_object` attribute to `PySignal`, `PyMode`, `PyPriority`.

#### H-2: ActorFactory cannot fail
**Issue**: Factory returned `Box<dyn Actor>` directly, forcing user factories that need validation to panic.
**Fix**: Changed `ActorFactory` to return `Result<Box<dyn Actor>, HaresError>`.

### MEDIUM

#### M-1: environment_to_py_dict incomplete
**Issue**: Missing grid state, humidity, wind, time_res, solar components.
**Fix**: Added `grid.voltage_pu`, `grid.frequency_hz`, zone humidity fields, wind speed/direction, solar altitude, time_res_s.

#### M-2: PySignal missing variants
**Issue**: Only 4 of 17 ControlSignal variants were exposed.
**Fix**: Added `ThermalSetpointDelta`, `PowerSetpoint`, `SOCTarget`, `DutyCycle`, `DemandResponse` with factory methods.

#### M-3: add_actor_by_name allocates registry each call
**Issue**: `ActorRegistry::new()` called on every `add_actor_by_name()`.
**Fix**: Cached `ActorRegistry` as field on `PyDwelling`.

### LOW

#### L-1: py_to_config_value bool ordering bug
**Issue**: Python `bool` extracts as `f64` (True → 1.0), so `f64` check before `bool` meant `always_comply=True` became `Float(1.0)` not `Bool(true)`.
**Fix**: Check `bool` before `f64` in extraction order.

#### L-2: Missing __repr__ methods
**Fix**: Added `__repr__` to `PyDispatchRequest`, `PySignal`, `PyMode`, `PyPriority`, `PyDRLevel`.

## Extended Python API

After fixes, Python actors have full access:

### Environment Dict Keys
- `zones`: list of dicts with `id`, `temperature_c`, `humidity_ratio`, `relative_humidity`, `volume_m3`
- `weather`: dict with `outdoor_temp_c`, `outdoor_humidity_ratio`, `wind_speed_m_s`, `wind_dir_deg`, `ground_temp_c`, `sky_temp_c`, `pressure_kpa`, `ghi_w_m2`, `dni_w_m2`, `dhi_w_m2`, `solar_altitude_deg`
- `grid`: dict with `voltage_pu`, `frequency_hz`
- `current_time`: ISO 8601 string
- `time_res_s`: timestep resolution in seconds

### Signal Types
- `Signal.thermal_setpoint(heating_c, cooling_c, deadband_c)`
- `Signal.thermal_setpoint_delta(heating_delta_c, cooling_delta_c)`
- `Signal.mode_override(mode)`
- `Signal.load_fraction(fraction)`
- `Signal.power_limit(max_kw)`
- `Signal.power_setpoint(active_power_kw, reactive_power_kvar)`
- `Signal.soc_target(target_soc, min_soc, max_soc)`
- `Signal.duty_cycle(on_fraction, period_s)`
- `Signal.demand_response(level, duration_s)`

### DR Levels
- `DRLevel.normal()`, `DRLevel.moderate()`, `DRLevel.high()`, `DRLevel.critical()`, `DRLevel.grid_emergency()`

## End-to-End Review Fixes (2026-03-25)

### C-1: PyControlSignal Factory Methods for IdealCapacity

**Status**: ✅ Already implemented in `py_control.rs` lines 131-137 (`from_dict` case exists)

**Additional Fix**: Added static factory methods to `PyControlSignal`:
- `ControlSignal.ideal_capacity(capacity_w)`
- `ControlSignal.thermal_setpoint_delta(heating_delta_c, cooling_delta_c)`
- `ControlSignal.load_fraction(fraction)`
- `ControlSignal.mode_override(mode)` (string-based)
- `ControlSignal.demand_response(level, duration_s)` (string-based)

### C-2: PyO3 Deprecation Warnings

**Status**: ✅ Fixed

All `#[pyclass]` types now have explicit `from_py_object` attribute:
- `PySignal`, `PyMode`, `PyPriority`, `PyDRLevel`, `PyDispatchRequest`, `PvSoilingConfig`

Note: PyO3 0.28 deprecation warning still fires even with `from_py_object` opt-in — this is a known pyo3 issue, not introduced by this implementation.

### Python API Completeness

The Python bindings now expose all essential control signals:

| ControlSignal Variant | from_dict | Factory Method | PySignal Enum |
|-----------------------|-----------|----------------|---------------|
| IdealCapacity | ✅ | ✅ | - |
| ThermalSetpoint | ✅ | ✅ | ✅ |
| ThermalSetpointDelta | ✅ | ✅ | ✅ |
| ModeOverride | ✅ | ✅ | ✅ |
| LoadFraction | ✅ | ✅ | ✅ |
| PowerLimit | ✅ | - | ✅ |
| PowerSetpoint | ✅ | ✅ | ✅ |
| SOCTarget | ✅ | ✅ | ✅ |
| DutyCycle | ✅ | - | ✅ |
| DemandResponse | ✅ | ✅ | ✅ |