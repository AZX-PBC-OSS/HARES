---
id: ACTOR-012
title: Occupant actor — lights, appliances, EV behavioral control
kind: implement
depends_on:
  - ACTOR-003
  - ACTOR-001
files_to_touch:
  - crates/hares-core/src/actors/occupant.rs
  - crates/hares-core/src/actors/mod.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - crates/hares-equipment/src/scheduled_load.rs
  - crates/hares-equipment/src/event_load.rs
  - crates/hares-equipment/src/ev/mod.rs
  - crates/hares-types/src/control_signal.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
---

## Background/Context

Equipment is self-contained: lighting has its own power schedule, appliances have their own cycle profiles, HVAC has its thermostat schedule. These internal schedules are part of the equipment and stay there.

The Occupant actor models *external human behavior* that acts on equipment from outside:
- Turning lights off when leaving the room (override equipment's default schedule)
- Starting a washing machine cycle (trigger an event on the appliance)
- Plugging/unplugging an EV (connect/disconnect)
- Changing EV charge mode (eco/fast)
- Overriding thermostat setpoint (covered by ACTOR-009)

This is foundational for DR RL simulation where the occupant must decide "do I comply with this DR request?" — which requires modeling presence, preferences, and decision-making.

Equipment already handles these signals via `apply_control_unchecked` (ScheduledLoad supports LOAD_FRACTION, MODE_OVERRIDE; EV supports POWER_SETPOINT, SOC_TARGET). The actor just needs to push them.

## Work to Do

- [x] Create `crates/hares-core/src/actors/occupant.rs` implementing `Actor` trait
- [x] Occupant fields: name, presence schedule (home/away/sleeping), behavioral preferences
- [x] `decide(env, out)` dispatches ControlSignals only — never mutates environment or equipment state directly. Equipment receives signals via `apply_control` and adjusts its own internal operational state. Examples:
  - `ModeOverride { mode: Off }` → lighting equipment sets internal state to off → `step()` produces 0W
  - `ModeOverride { mode: On }` → washing machine sets internal state to running → `step()` produces rated power draw + thermal gain
  - EV: use existing `ModeOverride { mode: Off }` for disconnect, `ModeOverride { mode: On }` for connect → EV equipment sets plugged/unplugged state → `step()` computes charge power or 0W. Use `PowerLimit` to change charge rate. No new signal types needed — existing ControlSignal variants cover EV use cases.
  - When occupant is home and not intervening, emit nothing — equipment runs on its internal schedule
- [x] `decide(env, out)` pushes into pre-allocated buffer (ACTOR-008 pattern), zero per-step allocation
- [x] Occupant presence derived from schedule — stored as pre-computed array (`Vec<Presence>`), not cloned each step
- [x] Pass `&EnvironmentState` by immutable reference — actors never mutate environment

## Files to Touch

- `crates/hares-core/src/actors/occupant.rs`: **New** — Occupant actor
- `crates/hares-core/src/actors/mod.rs`: Export occupant module
- `crates/hares-core/src/dwelling/mod.rs`: Optionally create Occupant actor in from_config()

## Measures of Success

- [x] Occupant actor emits control signals that equipment responds to
- [x] Without actor: equipment runs on internal schedules (baseline behavior unchanged)
- [x] With actor: external overrides modify equipment behavior (lights off when away, etc.)
- [x] Actor can be subclassed/replaced for DR compliance modeling
- [x] EV plug/unplug events flow through actor→equipment control signals

## Tests Added

### hares-core (actors/occupant.rs)
- `presence_default_is_home` - verifies default presence state
- `presence_is_away` - verifies away detection
- `equipment_behavior_default_is_none` - verifies default behavior config
- `equipment_behavior_builder_methods` - verifies builder pattern
- `occupant_name_returns_expected_value` - verifies Actor::name()
- `occupant_default_presence_is_home` - verifies initial state
- `occupant_presence_schedule_works` - verifies schedule advancement
- `occupant_no_targets_emits_nothing` - verifies empty case
- `occupant_lighting_off_when_away_emits_signal` - verifies ModeOverride Off
- `occupant_lighting_no_signal_when_home` - verifies no signals when home
- `occupant_appliance_on_when_home_emits_signal` - verifies ModeOverride Standby
- `occupant_ev_plug_unplug_on_transition` - verifies EV connect/disconnect
- `occupant_ev_power_setpoint_when_home` - verifies PowerSetpoint for EV
- `occupant_plug_loads_by_end_use` - verifies ByEndUse targeting
- `occupant_load_fraction_signal` - verifies LoadFraction signal
- `occupant_uses_user_override_priority` - verifies UserOverride priority
- `occupant_multiple_equipment_targets` - verifies multi-equipment dispatch
- `occupant_no_duplicate_signals_on_same_presence` - verifies transition-only behavior
- `occupant_sleeping_is_present` - verifies Sleeping presence state
- `occupant_transition_home_to_sleeping_no_signal` - verifies sleeping != away

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes (20 occupant tests + all existing tests)
- [x] `cargo clippy --workspace` passes (no warnings)
