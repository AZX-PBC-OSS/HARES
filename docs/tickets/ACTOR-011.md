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

- [ ] Create `crates/hares-core/src/actors/` directory with `mod.rs`
- [ ] Create `ideal_thermostat.rs` implementing `Actor` trait
- [ ] This actor models *external setpoint overrides* — NOT the base thermostat schedule (which stays in IdealHvac equipment)
- [ ] Use cases: user override ("hold at 68°F"), smart thermostat program ("pre-cool before DR event"), occupant away mode ("setback to 60°F")
- [ ] `decide(env, out)`: dispatches `ControlSignal::ThermalSetpoint` with `priority: PriorityTier::UserOverride` — never mutates equipment or environment directly. IdealHvac receives the signal via `apply_control`, adjusts its internal runtime setpoint override, and the thermostat FSM responds on the next `update_control` cycle
- [ ] When no override is active, emit nothing — equipment uses its internal schedule
- [ ] IdealHvac keeps its embedded setpoint schedules (ACTOR-003) — the actor only overrides when it has a reason to
- [ ] In `from_config()`: optionally create IdealThermostat actor for testing override patterns
- [ ] Verify conditioned oracle tests still pass (actor is optional, not required for basic operation)

## Files to Touch

- `crates/hares-core/src/actors/ideal_thermostat.rs`: **New** — IdealThermostat actor
- `crates/hares-core/src/actors/mod.rs`: **New** — actors module
- `crates/hares-core/src/actor.rs`: Potentially move trait here or re-export
- `crates/hares-core/src/dwelling/mod.rs`: Create IdealThermostat actor alongside IdealHvac equipment
- `crates/hares-equipment/src/hvac/ideal_hvac.rs`: No changes — keeps internal schedule, actor only overrides

## Measures of Success

- [ ] IdealThermostat pushes ThermalSetpoint signals each step
- [ ] IdealHvac responds to ThermalSetpoint without needing internal schedule
- [ ] Conditioned oracle test results unchanged (same MAE)
- [ ] Pattern is reusable for other actor→equipment control flows

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo test --test conditioned_oracle --features observe` passes (same MAE as before)
