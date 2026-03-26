---
id: ACTOR-011
title: IdealThermostat actor — extract setpoint schedule from IdealHvac
kind: implement
depends_on:
  - ACTOR-007
  - ACTOR-003
files_to_touch:
  - crates/hares-core/src/actors/ideal_thermostat.rs
  - crates/hares-core/src/actors/mod.rs
  - crates/hares-core/src/actor.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
  - crates/hares-types/src/control_signal.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo test --test conditioned_oracle --features observe
---

## Background/Context

IdealHvac (ACTOR-004) correctly embeds its own setpoint schedule — this is internal equipment state, not external control. The thermostat schedule (24h heating/cooling profiles from HPXML) is part of how the equipment operates normally.

The IdealThermostat actor represents an *external user override* — a person walking to the thermostat and changing the setpoint, putting it on hold, or a smart thermostat program pushing a schedule change. It does NOT replace the equipment's internal schedule; it pushes `ThermalSetpoint` overrides via the control surface, exactly like a human would.

This demonstrates the actor→equipment push pattern: equipment works self-contained with internal schedules, actors push external behavioral changes.

## Work to Do

- [x] Create `crates/hares-core/src/actors/` directory with `mod.rs`
- [x] Create `ideal_thermostat.rs` implementing `Actor` trait
- [x] This actor models *external setpoint overrides* — NOT the base thermostat schedule (which stays in IdealHvac equipment)
- [x] Use cases: user override ("hold at 68°F"), smart thermostat program ("pre-cool before DR event"), occupant away mode ("setback to 60°F")
- [x] `decide(env, out)`: dispatches `ControlSignal::ThermalSetpoint` with `priority: PriorityTier::UserOverride` — never mutates equipment or environment directly. IdealHvac receives the signal via `apply_control`, adjusts its internal runtime setpoint override, and the thermostat FSM responds on the next `update_control` cycle
- [x] When no override is active, emit nothing — equipment uses its internal schedule
- [x] IdealHvac keeps its embedded setpoint schedules (ACTOR-003) — the actor only overrides when it has a reason to
- [x] Verify conditioned oracle tests still pass (actor is optional, not required for basic operation)

## Files to Touch

- `crates/hares-core/src/actors/ideal_thermostat.rs`: **New** — IdealThermostat actor + OverrideState struct
- `crates/hares-core/src/actors/mod.rs`: Export IdealThermostat and OverrideState

## Measures of Success

- [x] IdealThermostat pushes ThermalSetpoint signals each step when override is active
- [x] IdealThermostat emits nothing when no override is active
- [x] Signals use `PriorityTier::UserOverride` (higher than Schedule)
- [x] Pattern is reusable for other actor→equipment control flows
- [x] Conditioned oracle test results unchanged (actor not used in those tests)

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes (112 tests in hares-core, all pass)
- [x] `cargo clippy --workspace` passes
- [x] `cargo test --test conditioned_oracle --features observe` - 5/6 pass; 1 pre-existing deviation in summer scenario (unrelated to this change)

## Tests Added

### hares-core (actors/ideal_thermostat.rs)
- `override_state_is_active_when_any_set` - verifies is_active() detection
- `override_state_heating_factory` - verifies OverrideState::heating() factory
- `override_state_cooling_factory` - verifies OverrideState::cooling() factory
- `override_state_dual_factory` - verifies OverrideState::dual() factory
- `override_state_with_deadband` - verifies deadband chaining
- `ideal_thermostat_name` - verifies Actor::name() returns "IdealThermostat"
- `ideal_thermostat_no_override_emits_nothing` - verifies no signals when inactive
- `ideal_thermostat_heating_override_emits_signal` - verifies heating override dispatch
- `ideal_thermostat_cooling_override_emits_signal` - verifies cooling override dispatch
- `ideal_thermostat_dual_override_emits_signal` - verifies dual override dispatch
- `ideal_thermostat_deadband_override_included` - verifies deadband in signal
- `ideal_thermostat_clear_override_stops_emission` - verifies clear() stops signals
- `ideal_thermostat_set_override_updates_state` - verifies set_override() mutation
- `ideal_thermostat_uses_user_override_priority` - verifies UserOverride tier usage
- `ideal_thermostat_target_name_accessible` - verifies target_name() accessor
- `ideal_thermostat_with_override_state` - verifies with_override() builder

## Implementation Notes

### Design

The `OverrideState` struct captures the thermostat override configuration:
- `heating_setpoint_c: Option<f64>` - override heating setpoint
- `cooling_setpoint_c: Option<f64>` - override cooling setpoint  
- `deadband_c: Option<f64>` - override deadband

Factory methods provide ergonomic construction:
- `OverrideState::heating(20.0)` - heating-only override
- `OverrideState::cooling(24.0)` - cooling-only override
- `OverrideState::dual(20.0, 24.0)` - dual heating/cooling override
- `.with_deadband(1.0)` - add deadband override

### Zero-Allocation Hot Path

- Pre-cached `DispatchTarget` avoids per-step Arc construction
- `decide()` clones only when override is active
- When inactive, `decide()` returns immediately with no allocation

### Integration Pattern

```rust
// Create thermostat actor targeting "HVAC" equipment
let thermostat = IdealThermostat::new("HVAC")
    .with_setpoints(20.0, 24.0);  // Hold at 20°C heat, 24°C cool

// Add to dwelling
dwelling.add_actor(Box::new(thermostat));

// Actor will dispatch ThermalSetpoint signals each step at UserOverride priority
// Equipment receives signals via existing dispatch mechanism
```

### No Changes to IdealHvac

Per ticket requirements, IdealHvac is unchanged. It already handles `ThermalSetpoint` signals via `apply_control_unchecked()` which sets `runtime_setpoints` override. The thermostat FSM respects this override in `effective_setpoints()`.