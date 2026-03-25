---
id: ACTOR-013
title: DR compliance actor — demand response decision modeling
kind: implement
depends_on:
  - ACTOR-003
  - ACTOR-012
files_to_touch:
  - crates/hares-core/src/actors/dr_compliance.rs
  - crates/hares-core/src/actors/mod.rs
references:
  - crates/hares-control/src/types.rs (PriceSignal — verify exists, create if needed)
  - crates/hares-types/src/control_signal.rs (DemandResponse variant already exists)
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
---

## Background/Context

For DR RL simulation, we need to model occupant compliance decisions: when a DR signal arrives, does the occupant comply? Do they shed load, unplug EV, raise thermostat setpoint? This actor receives DR signals (from a grid actor or external control) and decides what equipment actions to take based on occupant preferences, comfort constraints, and behavioral models.

This ticket creates the infrastructure. The initial implementation uses simple rule-based compliance (always comply, never comply, probabilistic). Future work replaces rules with RL agents.

## Work to Do

- [ ] Create `crates/hares-core/src/actors/dr_compliance.rs` implementing `Actor` trait
- [ ] Receives DR events from environment or control signals (PriceSignal, DemandResponse)
- [ ] Decision model interface: `trait ComplianceModel { fn should_comply(&self, dr_level: u8, env: &EnvironmentState) -> bool; }`
- [ ] Built-in models: `AlwaysComply`, `NeverComply`, `Probabilistic { compliance_rate: f64 }`
- [ ] When complying, dispatches ControlSignals with `priority: PriorityTier::Grid` (overrides UserOverride and Schedule) — equipment receives and adjusts its internal operational state:
  - `LoadFraction { fraction: 0.0 }` → equipment sets internal load fraction → `step()` produces reduced output
  - `ThermalSetpoint` → HVAC adjusts internal setpoint → thermostat FSM responds on next cycle
  - `ModeOverride { mode: Off }` → non-essential equipment sets internal state to off → `step()` produces 0W
  - `PowerLimit` → EV equipment clamps internal charge rate → `step()` produces limited power draw
- [ ] When not complying, emits nothing — equipment continues on internal schedule/state
- [ ] Actor never directly mutates equipment fields or environment state
- [ ] Future: RL agent replaces ComplianceModel with learned policy

## Files to Touch

- `crates/hares-core/src/actors/dr_compliance.rs`: **New** — DR compliance actor
- `crates/hares-core/src/actors/mod.rs`: Export dr_compliance module

## Measures of Success

- [ ] DR compliance actor can be constructed with different compliance models
- [ ] AlwaysComply correctly sheds loads when DR event is active
- [ ] NeverComply produces no equipment changes during DR events
- [ ] ComplianceModel trait is extensible for RL agent integration

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
