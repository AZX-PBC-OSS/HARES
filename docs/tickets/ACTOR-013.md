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

- [x] Create `crates/hares-core/src/actors/dr_compliance.rs` implementing `Actor` trait
- [x] Receives DR events from environment or control signals (PriceSignal, DemandResponse)
- [x] Decision model interface: `trait ComplianceModel { fn should_comply(&self, dr_level: DRLevel, env: &EnvironmentState) -> bool; }`
- [x] Built-in models: `AlwaysComply`, `NeverComply`, `Probabilistic { compliance_rate: f64 }`
- [x] When complying, dispatches ControlSignals with `priority: PriorityTier::Grid` (overrides UserOverride and Schedule) — equipment receives and adjusts its internal operational state:
  - `LoadFraction { fraction: 0.0 }` → equipment sets internal load fraction → `step()` produces reduced output
  - `ThermalSetpoint` → HVAC adjusts internal setpoint → thermostat FSM responds on next cycle
  - `ModeOverride { mode: Off }` → non-essential equipment sets internal state to off → `step()` produces 0W
  - `PowerLimit` → EV equipment clamps internal charge rate → `step()` produces limited power draw
- [x] When not complying, emits nothing — equipment continues on internal schedule/state
- [x] Actor never directly mutates equipment fields or environment state
- [x] Future: RL agent replaces ComplianceModel with learned policy

## Files to Touch

- `crates/hares-core/src/actors/dr_compliance.rs`: **New** — DR compliance actor
- `crates/hares-core/src/actors/mod.rs`: Export dr_compliance module

## Measures of Success

- [x] DR compliance actor can be constructed with different compliance models
- [x] AlwaysComply correctly sheds loads when DR event is active
- [x] NeverComply produces no equipment changes during DR events
- [x] ComplianceModel trait is extensible for RL agent integration

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace --exclude hares-python` passes
- [x] `cargo clippy --workspace --exclude hares-python` passes

## Tests Added

### hares-core (actors/dr_compliance.rs)
- `always_comply_returns_true` - verifies AlwaysComply always returns true
- `never_comply_returns_false` - verifies NeverComply always returns false
- `probabilistic_compliance_rate_zero` - verifies 0% rate never complies
- `probabilistic_compliance_rate_one` - verifies 100% rate always complies
- `probabilistic_deterministic_with_seed` - verifies same seed produces same results
- `probabilistic_different_seeds_differ` - verifies different seeds can produce different results
- `dr_action_curtail_creates_correct_variant` - verifies DrAction::curtail() factory
- `dr_action_off_creates_correct_variant` - verifies DrAction::off() factory
- `dr_action_limit_power_creates_correct_variant` - verifies DrAction::limit_power() factory
- `dr_compliance_name_returns_expected_value` - verifies Actor::name()
- `dr_compliance_no_dr_active_emits_nothing` - verifies no signals when DR inactive
- `dr_compliance_always_comply_dispatches_signals` - verifies AlwaysComply dispatches
- `dr_compliance_never_comply_dispatches_nothing` - verifies NeverComply emits nothing
- `dr_compliance_load_curtailment_dispatches_correct_signal` - verifies LoadFraction signal
- `dr_compliance_power_limit_dispatches_correct_signal` - verifies PowerLimit signal
- `dr_compliance_multiple_targets_dispatches_multiple_signals` - verifies multi-target dispatch
- `dr_compliance_uses_grid_priority` - verifies PriorityTier::Grid usage
- `dr_compliance_none_action_dispatches_nothing` - verifies DrAction::None produces no signal
- `dr_compliance_current_dr_level_accessor` - verifies current_dr_level() accessor
- `dr_compliance_is_dr_active_accessor` - verifies is_dr_active() accessor
- `dr_compliance_probabilistic_respects_rate` - verifies probabilistic model rate distribution

## Implementation Notes

### Design

The `DrCompliance` actor has:
- A boxed `ComplianceModel` trait object for extensible decision-making
- Optional HVAC target and action for thermal adjustments
- Optional load targets with individual actions per equipment
- DR state tracking (`current_dr_level`, `dr_active`)

### ComplianceModel Trait

The trait is designed for RL agent integration:
```rust
pub trait ComplianceModel: Send + Sync {
    fn should_comply(&self, dr_level: DRLevel, env: &EnvironmentState) -> bool;
}
```

Future RL agents implement this trait with learned policies.

### Probabilistic Model

Uses deterministic hashing for reproducibility:
- Hashes seed + DR level + timestamp
- Normalizes to [0.0, 1.0]
- Compares against compliance_rate

### DR Actions

`DrAction` enum covers common DR responses:
- `LoadCurtailment { fraction }` - partial load reduction
- `SetpointAdjust { delta_c }` - thermal setpoint modification
- `TurnOff` - complete equipment shutdown
- `PowerLimit { max_kw }` - power cap for EVs/batteries
- `None` - no action (for testing/configuring optional actions)

### Zero-Allocation Hot Path

- Pre-cached `DispatchTarget` values avoid per-step Arc construction
- `decide()` only allocates when DR is active and compliance decision is true
- `DrAction::None` skips signal generation entirely

### Integration Pattern

```rust
// Create DR compliance actor with AlwaysComply model
let mut dr_actor = DrCompliance::new("DRResponder")
    .with_compliance_model(AlwaysComply)
    .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
    .with_hvac_action(DrAction::setpoint_delta(2.0))
    .with_load_target(
        DispatchTarget::ByName("EV".into()),
        DrAction::limit_power(3.3)
    );

// Set DR state from schedule or external signal
dr_actor.set_dr_state(DRLevel::Critical, true);

// Add to dwelling
dwelling.add_actor(Box::new(dr_actor));
```

### DR State Propagation

DR state (`level`, `active`) must be set externally via `set_dr_state()`:
- Schedule loaders can read DR event columns from CSV
- Grid actors can dispatch DR signals
- External control systems can inject DR commands

The actor does NOT directly read from `EnvironmentState::custom_domains` — this allows flexibility in how DR events are signaled and decouples the actor from specific data formats.